//! Offline track analysis: 3-band waveform, BPM + beat grid, loudness.

use crate::audio::TrackAudio;

/// Waveform bins per second of audio.
pub const WAVE_RATE: f64 = 150.0;

pub struct Waveform {
    /// Bins of [low, mid, high] peak amplitudes (0..255).
    pub data: Vec<u8>,
    pub rate: f64,
}

/// One-pole low-pass coefficient for cutoff `f`.
fn one_pole(sr: f64, f: f64) -> f32 {
    (1.0 - (-2.0 * std::f64::consts::PI * f / sr).exp()) as f32
}

fn mono(audio: &TrackAudio, i: usize) -> f32 {
    (audio.samples[i * 2] + audio.samples[i * 2 + 1]) * 0.5
}

pub fn waveform(audio: &TrackAudio, gain: f32) -> Waveform {
    let sr = audio.sample_rate as f64;
    let a_low = one_pole(sr, 250.0);
    let a_high = one_pole(sr, 2500.0);
    let bin_len = sr / WAVE_RATE;
    let bins = (audio.frames as f64 / bin_len).ceil() as usize;
    let mut data = vec![0u8; bins * 3];
    let (mut lp1, mut lp2) = (0.0f32, 0.0f32);
    let mut peaks = [0.0f32; 3];
    let mut bin = 0usize;
    let mut next_edge = bin_len;
    let scale = |v: f32| ((v * gain).sqrt().min(1.0) * 255.0) as u8;
    for i in 0..audio.frames {
        let x = mono(audio, i);
        lp1 += a_low * (x - lp1);
        lp2 += a_high * (x - lp2);
        let low = lp1;
        let mid = lp2 - lp1;
        let high = x - lp2;
        peaks[0] = peaks[0].max(low.abs());
        peaks[1] = peaks[1].max(mid.abs());
        peaks[2] = peaks[2].max(high.abs());
        if (i + 1) as f64 >= next_edge || i + 1 == audio.frames {
            if bin < bins {
                data[bin * 3] = scale(peaks[0]);
                data[bin * 3 + 1] = scale(peaks[1]);
                data[bin * 3 + 2] = scale(peaks[2]);
            }
            peaks = [0.0; 3];
            bin += 1;
            next_edge += bin_len;
        }
    }
    Waveform { data, rate: WAVE_RATE }
}

/// Linear gain to bring the track's RMS to roughly -13 dBFS.
pub fn auto_gain(audio: &TrackAudio) -> f32 {
    // Measure in 400ms windows and take the loud part (ignores intros/breaks).
    let win = (audio.sample_rate as usize * 2 / 5).max(1);
    let mut energies: Vec<f64> = audio
        .samples
        .chunks(win * 2)
        .map(|c| c.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / c.len().max(1) as f64)
        .collect();
    if energies.is_empty() {
        return 1.0;
    }
    energies.sort_by(|a, b| a.total_cmp(b));
    let upper = &energies[energies.len() / 2..];
    let rms = (upper.iter().sum::<f64>() / upper.len() as f64).sqrt();
    if rms < 1e-5 {
        return 1.0;
    }
    let target = 10f64.powf(-13.0 / 20.0);
    (target / rms).clamp(0.25, 2.5) as f32
}

pub struct BeatGrid {
    pub bpm: f64,
    /// Seconds to a beat (the grid anchor).
    pub offset: f64,
}

const MIN_BPM: f64 = 75.0;
const MAX_BPM: f64 = 155.0;

/// Onset strength envelope at `sr / hop` Hz.
fn onset_envelope(audio: &TrackAudio, hop: usize) -> Vec<f32> {
    let sr = audio.sample_rate as f64;
    let a_low = one_pole(sr, 150.0);
    let a_high = one_pole(sr, 3000.0);
    let n = audio.frames / hop;
    let mut bands = vec![[0.0f32; 3]; n];
    let (mut lp1, mut lp2) = (0.0f32, 0.0f32);
    for (h, b) in bands.iter_mut().enumerate() {
        let mut e = [0.0f32; 3];
        for i in h * hop..(h + 1) * hop {
            let x = mono(audio, i);
            lp1 += a_low * (x - lp1);
            lp2 += a_high * (x - lp2);
            let low = lp1;
            let mid = lp2 - lp1;
            let high = x - lp2;
            e[0] += low * low;
            e[1] += mid * mid;
            e[2] += high * high;
        }
        for k in 0..3 {
            b[k] = (1.0 + 1000.0 * e[k] / hop as f32).ln();
        }
    }
    let weights = [1.0f32, 0.5, 0.6];
    let mut flux = vec![0.0f32; n];
    for i in 1..n {
        let mut f = 0.0;
        for k in 0..3 {
            f += weights[k] * (bands[i][k] - bands[i - 1][k]).max(0.0);
        }
        flux[i] = f;
    }
    // Subtract a moving average and half-wave rectify.
    let w = 16usize;
    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + flux[i] as f64;
    }
    (0..n)
        .map(|i| {
            let a = i.saturating_sub(w);
            let b = (i + w + 1).min(n);
            let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
            (flux[i] as f64 - mean).max(0.0) as f32
        })
        .collect()
}

/// Score how well a tempo explains the onsets: strength of the dominant phase bin.
fn comb_score(env: &[f32], period: f64, bins: usize, hist: &mut Vec<f64>) -> (f64, usize) {
    hist.clear();
    hist.resize(bins, 0.0);
    let inv = 1.0 / period;
    for (i, &v) in env.iter().enumerate() {
        if v > 0.0 {
            let ph = (i as f64 * inv).fract();
            hist[((ph * bins as f64) as usize).min(bins - 1)] += v as f64;
        }
    }
    let mut best = (0.0, 0);
    for b in 0..bins {
        let s = hist[b] + 0.5 * (hist[(b + bins - 1) % bins] + hist[(b + 1) % bins]);
        if s > best.0 {
            best = (s, b);
        }
    }
    best
}

pub fn detect_beatgrid(audio: &TrackAudio) -> Option<BeatGrid> {
    let sr = audio.sample_rate as f64;
    let hop = (sr / 200.0).round() as usize;
    let env_rate = sr / hop as f64;
    let env = onset_envelope(audio, hop);
    if env.len() < (env_rate * 8.0) as usize {
        return None;
    }

    // Coarse: how tightly onsets stack up in phase, scored per ~20 s segment so
    // small tempo errors don't smear the histogram. Unlike autocorrelation this
    // rejects 2:3 / 3:4 / 4:5 relatives of the true tempo (their beats spread
    // over several phase bins).
    let sparse: Vec<(u32, f32)> = env.iter().enumerate().filter(|(_, v)| **v > 0.0).map(|(i, v)| (i as u32, *v)).collect();
    let seg_len = (env_rate * 20.0) as u32;
    let bins = 36usize;
    let mut seg_hist = vec![0.0f64; bins];
    let mut segmented = |period: f64| -> f64 {
        let inv = 1.0 / period;
        let mut total = 0.0;
        let mut seg = 0u32;
        seg_hist.iter_mut().for_each(|h| *h = 0.0);
        let flush = |h: &mut Vec<f64>| {
            let mut m = 0.0f64;
            for b in 0..bins {
                m = m.max(h[b] + 0.5 * (h[(b + bins - 1) % bins] + h[(b + 1) % bins]));
            }
            h.iter_mut().for_each(|v| *v = 0.0);
            m
        };
        for &(i, v) in &sparse {
            if i / seg_len != seg {
                total += flush(&mut seg_hist);
                seg = i / seg_len;
            }
            let ph = (i as f64 * inv).fract();
            seg_hist[((ph * bins as f64) as usize).min(bins - 1)] += v as f64;
        }
        total + flush(&mut seg_hist)
    };
    let mut coarse = (f64::MIN, 120.0);
    let mut bpm = MIN_BPM;
    while bpm <= MAX_BPM {
        let s = segmented(60.0 * env_rate / bpm);
        if s > coarse.0 {
            coarse = (s, bpm);
        }
        bpm += 0.05;
    }
    let coarse_bpm = coarse.1;

    // Fine: phase histogram sharpness over the whole track.
    let mut hist = Vec::new();
    let mut best = (f64::MIN, coarse_bpm);
    let mut bpm = coarse_bpm - 0.1;
    while bpm <= coarse_bpm + 0.1 {
        let (s, _) = comb_score(&env, 60.0 * env_rate / bpm, 48, &mut hist);
        if s > best.0 {
            best = (s, bpm);
        }
        bpm += 0.005;
    }
    let mut bpm = best.1;
    // Most dance music sits on whole numbers.
    if (bpm - bpm.round()).abs() < 0.08 {
        bpm = bpm.round();
    } else {
        bpm = (bpm * 100.0).round() / 100.0;
    }

    let period = 60.0 * env_rate / bpm;
    let bins = 96;
    let (_, b) = comb_score(&env, period, bins, &mut hist);
    let phase = (b as f64 + 0.5) / bins as f64;
    let beat_secs = 60.0 / bpm;
    // Flux at hop i reflects the window starting at i * hop; nudge half a hop back.
    let offset = (phase * period * hop as f64 / sr - 0.5 * hop as f64 / sr).rem_euclid(beat_secs);
    Some(BeatGrid { bpm, offset })
}
