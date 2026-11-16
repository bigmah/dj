use std::sync::Arc;

use crate::analysis;
use crate::audio::deck::Deck;
use crate::audio::{LoadRequest, TrackAudio, XfCurve};

/// Synthetic track: decaying sine "kicks" on every beat plus noisy off-beat hats.
fn click_track(bpm: f64, offset: f64, secs: f64, sr: u32) -> TrackAudio {
    let frames = (secs * sr as f64) as usize;
    let mut samples = vec![0.0f32; frames * 2];
    let beat = 60.0 / bpm;
    let mut seed = 12345u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0
    };
    for i in 0..frames {
        let t = i as f64 / sr as f64;
        let mut v = 0.0f32;
        if t >= offset {
            let into = (t - offset) % beat;
            if into < 0.15 {
                v += (0.8 * (-into * 30.0).exp() * (2.0 * std::f64::consts::PI * 55.0 * into).sin()) as f32;
            }
            let into_off = (t - offset + beat / 2.0) % beat;
            if into_off < 0.03 {
                v += 0.15 * noise() * (-(into_off as f32) * 120.0).exp();
            }
        }
        samples[i * 2] = v;
        samples[i * 2 + 1] = v;
    }
    TrackAudio { samples, sample_rate: sr, frames }
}

fn phase_err(detected: f64, truth: f64, beat: f64) -> f64 {
    let d = (detected - truth).rem_euclid(beat);
    d.min(beat - d)
}

#[test]
fn detects_common_tempos() {
    for (bpm, off) in [(128.0, 0.21), (174.0 / 2.0, 0.05), (140.0, 0.4), (93.5, 0.33), (122.0, 0.0)] {
        let audio = click_track(bpm, off, 60.0, 44_100);
        let grid = analysis::detect_beatgrid(&audio).expect("grid");
        assert!((grid.bpm - bpm).abs() < 0.05, "bpm {bpm}: got {}", grid.bpm);
        let err = phase_err(grid.offset, off, 60.0 / bpm);
        assert!(err < 0.015, "bpm {bpm}: offset {} vs {off} (err {err})", grid.offset);
    }
}

#[test]
fn waveform_and_gain() {
    let audio = click_track(120.0, 0.0, 10.0, 48_000);
    let w = analysis::waveform(&audio, 1.0);
    assert_eq!(w.data.len() % 3, 0);
    assert!((w.data.len() / 3) as f64 >= 10.0 * analysis::WAVE_RATE - 1.0);
    assert!(w.data.iter().any(|&v| v > 100));
    let g = analysis::auto_gain(&audio);
    assert!((0.25..=2.5).contains(&g));
}

fn loaded_deck(bpm: f64) -> Deck {
    let audio = Arc::new(click_track(bpm, 0.0, 30.0, 44_100));
    let mut d = Deck::new(44_100.0);
    d.load(LoadRequest {
        audio,
        load_id: 1,
        bpm: Some(bpm),
        grid_offset: 0.0,
        auto_gain: 1.0,
        cue: 0.0,
        hot_cues: [None; 8],
    });
    d
}

fn run(d: &mut Deck, secs: f64) {
    let mut buf = vec![0.0f32; 512 * 2];
    let blocks = (secs * 44_100.0 / 512.0) as usize;
    for _ in 0..blocks {
        d.render(&mut buf, 44_100.0, [1.0; 3]);
    }
}

#[test]
fn cue_preview_and_return() {
    let mut d = loaded_deck(120.0);
    assert!(d.cue_down(true), "at cue point, holding cue previews");
    run(&mut d, 0.5);
    assert!(d.pos > 0.4 * 44_100.0);
    d.cue_up();
    assert!(!d.playing);
    assert_eq!(d.pos, 0.0);

    // Play during preview latches playback.
    d.cue_down(true);
    d.play_pause();
    d.cue_up();
    assert!(d.playing);

    // Cue while playing: back to cue, stopped.
    run(&mut d, 1.0);
    d.cue_down(true);
    assert!(!d.playing && d.pos == 0.0);

    // Cue while paused elsewhere sets a new (quantized) cue point.
    d.seek(44_100.0 * 2.02);
    d.cue_down(true);
    assert!((d.cue - 44_100.0 * 2.0).abs() < 1.0, "cue quantized to beat, got {}", d.cue / 44_100.0);
}

#[test]
fn auto_loop_wraps_and_resizes() {
    let mut d = loaded_deck(120.0); // 0.5 s per beat
    d.play_pause();
    run(&mut d, 1.0);
    d.loop_toggle(true);
    let start = d.loop_start.unwrap();
    assert!((start - 44_100.0).abs() < 1.0);
    run(&mut d, 5.0);
    let end = d.loop_end.unwrap();
    assert!(d.pos >= start && d.pos < end, "stays inside loop");
    d.loop_halve();
    assert_eq!(d.loop_beats, 2.0);
    run(&mut d, 3.0);
    assert!(d.pos < start + 44_100.0 + 1.0);
    d.loop_toggle(true);
    assert!(!d.loop_active);
}

#[test]
fn hot_cue_set_and_jump() {
    let mut d = loaded_deck(120.0);
    d.play_pause();
    run(&mut d, 2.0);
    d.hot_cue(0, true);
    let hc = d.hot_cues[0].unwrap();
    assert!((hc / 44_100.0 - 2.0).abs() < 0.01);
    run(&mut d, 3.0);
    d.hot_cue(0, true);
    assert!((d.pos - hc).abs() < 0.25 * 44_100.0);
}

#[test]
fn pitch_changes_speed() {
    let mut d = loaded_deck(120.0);
    d.pitch = 0.08;
    d.play_pause();
    run(&mut d, 10.0);
    let played = d.pos / 44_100.0;
    assert!((played - 10.0 * 1.08).abs() < 0.05, "played {played}");
}

#[test]
fn crossfader_curves() {
    for c in XfCurve::ALL {
        let (l, r) = c.gains(-1.0);
        assert!(l > 0.99 && r < 0.01, "{c:?}");
        let (l, r) = c.gains(1.0);
        assert!(r > 0.99 && l < 0.01, "{c:?}");
        let (l, r) = c.gains(0.0);
        assert!(l > 0.7 && r > 0.7, "{c:?}");
    }
}

#[test]
fn decodes_wav_roundtrip() {
    let dir = std::env::temp_dir().join(format!("dj-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Artist Name - Song Title.wav");
    let spec = hound::WavSpec { channels: 1, sample_rate: 22_050, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..22_050 {
        w.write_sample(((i as f32 * 0.05).sin() * 10_000.0) as i16).unwrap();
    }
    w.finalize().unwrap();

    let audio = crate::decode::decode_file(&path).unwrap();
    assert_eq!(audio.sample_rate, 22_050);
    assert_eq!(audio.frames, 22_050);
    assert_eq!(audio.samples[200], audio.samples[201], "mono duplicated to stereo");

    let tracks = crate::library::scan(&[dir.to_string_lossy().into_owned()], &[]);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].artist, "Artist Name");
    assert_eq!(tracks[0].title, "Song Title");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sync_matches_tempo_and_phase() {
    use crate::audio::{Cmd, Mixer, NUM_HOT_CUES};
    use parking_lot::Mutex;
    let (_tx, rx) = crossbeam_channel::unbounded();
    let mut m = Mixer::new(44_100.0, rx, Arc::new(Mutex::new(Default::default())));
    for (i, bpm) in [(0usize, 128.0), (1, 125.0)] {
        m.handle(Cmd::Load(
            i,
            Box::new(LoadRequest {
                audio: Arc::new(click_track(bpm, 0.0, 60.0, 44_100)),
                load_id: 1,
                bpm: Some(bpm),
                grid_offset: 0.0,
                auto_gain: 1.0,
                cue: 0.0,
                hot_cues: [None; NUM_HOT_CUES],
            }),
        ));
    }
    m.handle(Cmd::PlayPause(0));
    for _ in 0..200 {
        m.render_block(256);
    }
    // Start B a third of a beat out of phase, then sync it.
    m.decks[1].pos = 44_100.0 * 60.0 / 125.0 * 0.33;
    m.handle(Cmd::PlayPause(1));
    m.handle(Cmd::SyncToggle(1));
    for _ in 0..(44_100 * 5 / 256) {
        m.render_block(256);
    }
    let b = &m.decks[1];
    assert!((125.0 * (1.0 + b.pitch) - 128.0).abs() < 1e-6, "tempo matched");
    let pa = m.decks[0].beat_pos(m.decks[0].pos).unwrap().rem_euclid(1.0);
    let pb = b.beat_pos(b.pos).unwrap().rem_euclid(1.0);
    let err = (pa - pb).rem_euclid(1.0).min((pb - pa).rem_euclid(1.0));
    assert!(err < 0.01, "phase locked, err {err}");
    // Touching the pitch turns sync off.
    m.handle(Cmd::PitchStep(1, 0.001));
    assert!(!m.decks[1].synced);
}

/// Manual check against a real library: `DJ_MUSIC=~/Music cargo test --release real_library -- --ignored --nocapture`
#[test]
#[ignore]
fn real_library() {
    let root = std::env::var("DJ_MUSIC").expect("set DJ_MUSIC");
    let tracks = crate::library::scan(&[root], &[]);
    for t in tracks.iter().filter(|t| t.duration.unwrap_or(0.0) > 30.0) {
        let start = std::time::Instant::now();
        let audio = match crate::decode::decode_file(std::path::Path::new(&t.path)) {
            Ok(a) => a,
            Err(e) => {
                println!("ERR {e}");
                continue;
            }
        };
        let dec = start.elapsed();
        let grid = analysis::detect_beatgrid(&audio);
        let _ = analysis::waveform(&audio, 1.0);
        println!(
            "{:>6.1}s decode {:>5}ms total {:>5}ms bpm {:>7}  {} - {}",
            audio.duration(),
            dec.as_millis(),
            start.elapsed().as_millis(),
            grid.map(|g| format!("{:.2}", g.bpm)).unwrap_or("-".into()),
            t.artist,
            t.title
        );
    }
}
