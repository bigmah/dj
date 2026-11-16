//! Real-time audio engine: owns the decks and mixer on the cpal callback thread.
//! The UI talks to it through a command channel and reads back a snapshot.

pub mod deck;
pub mod dsp;
pub mod types;

use std::sync::{Arc, OnceLock};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;

use deck::Deck;
use dsp::{eq_knob_gain, fader_gain, soft_clip};
pub use types::*;

const BLOCK: usize = 256;
const RECORD_CHUNK_FRAMES: usize = 4096;

#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Cmd>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
    pub sample_rate: u32,
    pub device_name: String,
}

impl EngineHandle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
    pub fn snapshot(&self) -> EngineSnapshot {
        self.snapshot.lock().clone()
    }
}

static ENGINE: OnceLock<EngineHandle> = OnceLock::new();

pub fn engine() -> &'static EngineHandle {
    ENGINE.get().expect("audio engine not started")
}

/// Starts the output stream on a dedicated thread (cpal streams are !Send on some hosts).
pub fn start() -> Result<(), String> {
    let (tx, rx) = unbounded::<Cmd>();
    let snapshot = Arc::new(Mutex::new(EngineSnapshot::default()));
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(u32, String), String>>(1);
    let snap2 = snapshot.clone();

    std::thread::Builder::new()
        .name("audio-output".into())
        .spawn(move || {
            let stream = match build_stream(rx, snap2) {
                Ok((stream, sr, name)) => {
                    let _ = ready_tx.send(Ok((sr, name)));
                    stream
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _keep = stream;
            loop {
                std::thread::park();
            }
        })
        .map_err(|e| e.to_string())?;

    let (sample_rate, device_name) = ready_rx.recv().map_err(|e| e.to_string())??;
    let _ = ENGINE.set(EngineHandle { tx, snapshot, sample_rate, device_name });
    Ok(())
}

fn build_stream(
    rx: Receiver<Cmd>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
) -> Result<(cpal::Stream, u32, String), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output device")?;
    let name = device.name().unwrap_or_else(|_| "Output".into());
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let mut config: cpal::StreamConfig = supported.config();
    // Lower latency makes the keyboard feel tight.
    if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size() {
        let want = 256u32.clamp(*min, *max);
        config.buffer_size = cpal::BufferSize::Fixed(want);
    }
    let sr = config.sample_rate.0;

    let stream = match format {
        cpal::SampleFormat::F32 => make_stream::<f32>(&device, &config, rx, snapshot),
        cpal::SampleFormat::I16 => make_stream::<i16>(&device, &config, rx, snapshot),
        cpal::SampleFormat::U16 => make_stream::<u16>(&device, &config, rx, snapshot),
        cpal::SampleFormat::I32 => make_stream::<i32>(&device, &config, rx, snapshot),
        other => return Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, sr, name))
}

fn make_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    rx: Receiver<Cmd>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let sr = config.sample_rate.0 as f64;
    let try_build = |cfg: &cpal::StreamConfig| {
        let mut mixer = Mixer::new(sr, rx.clone(), snapshot.clone());
        device.build_output_stream(
            cfg,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| mixer.fill(data, channels),
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
    };
    match try_build(config) {
        Ok(s) => Ok(s),
        Err(_) => {
            // Fixed buffer size not accepted: fall back to the device default.
            let mut cfg = config.clone();
            cfg.buffer_size = cpal::BufferSize::Default;
            try_build(&cfg).map_err(|e| e.to_string())
        }
    }
}

struct Recorder {
    sink: RecordSink,
    buf: Vec<f32>,
    frames: u64,
    dropouts: u64,
}

pub(crate) struct Mixer {
    sr: f64,
    rx: Receiver<Cmd>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
    pub(crate) decks: [Deck; NUM_DECKS],
    mixer: MixerState,
    quantize: bool,
    /// Smoothed per-deck output gain (fader * crossfader).
    ch_gain: [f32; NUM_DECKS],
    /// Smoothed EQ gains.
    eq_gain: [[f64; 3]; NUM_DECKS],
    master_gain: f32,
    scratch: [Vec<f32>; NUM_DECKS],
    master: Vec<f32>,
    levels: [f32; NUM_DECKS],
    master_level: [f32; 2],
    recorder: Option<Recorder>,
    garbage: Sender<Arc<TrackAudio>>,
}

impl Mixer {
    pub(crate) fn new(sr: f64, rx: Receiver<Cmd>, snapshot: Arc<Mutex<EngineSnapshot>>) -> Self {
        // Deallocating a decoded track can take a while: never do it on the audio thread.
        let (garbage, gc_rx) = unbounded::<Arc<TrackAudio>>();
        std::thread::spawn(move || while gc_rx.recv().is_ok() {});
        Self {
            sr,
            rx,
            snapshot,
            decks: std::array::from_fn(|_| Deck::new(sr)),
            mixer: MixerState::default(),
            quantize: true,
            ch_gain: [0.0; NUM_DECKS],
            eq_gain: [[1.0; 3]; NUM_DECKS],
            master_gain: 0.8,
            scratch: std::array::from_fn(|_| vec![0.0; BLOCK * 2]),
            master: vec![0.0; BLOCK * 2],
            levels: [0.0; NUM_DECKS],
            master_level: [0.0; 2],
            recorder: None,
            garbage,
        }
    }

    fn fill<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T], channels: usize) {
        while let Ok(cmd) = self.rx.try_recv() {
            self.handle(cmd);
        }
        let total_frames = data.len() / channels.max(1);
        let mut done = 0;
        while done < total_frames {
            let n = (total_frames - done).min(BLOCK);
            self.render_block(n);
            for f in 0..n {
                let base = (done + f) * channels;
                let l = self.master[f * 2];
                let r = self.master[f * 2 + 1];
                for c in 0..channels {
                    let v = match c {
                        0 => l,
                        1 => r,
                        _ => 0.0,
                    };
                    data[base + c] = T::from_sample(if channels == 1 { (l + r) * 0.5 } else { v });
                }
            }
            done += n;
        }
        self.publish();
    }

    fn update_sync(&mut self) {
        for i in 0..NUM_DECKS {
            if !self.decks[i].synced {
                self.decks[i].sync_bend = 0.0;
                continue;
            }
            let leader = 1 - i;
            let (lb, lpos, lplaying, llooping) = {
                let l = &self.decks[leader];
                if l.track.is_none() || l.bpm <= 0.0 {
                    self.decks[i].sync_bend = 0.0;
                    continue;
                }
                (l.bpm * (1.0 + l.pitch), l.beat_pos(l.pos), l.playing, l.loop_active)
            };
            let me = &mut self.decks[i];
            if me.bpm <= 0.0 {
                continue;
            }
            // Pick half/normal/double time, whichever needs the least pitch change.
            let mult = [0.5, 1.0, 2.0]
                .into_iter()
                .min_by(|a, b| {
                    let da = ((lb * a) / me.bpm - 1.0).abs();
                    let db = ((lb * b) / me.bpm - 1.0).abs();
                    da.total_cmp(&db)
                })
                .unwrap_or(1.0);
            me.pitch = lb * mult / me.bpm - 1.0;
            me.sync_bend = 0.0;
            // Loops (especially fractional ones) shift phase on purpose: don't fight them.
            if lplaying && me.playing && me.bend == 0.0 && !me.loop_active && !llooping {
                if let (Some(lp), Some(mp)) = (lpos, me.beat_pos(me.pos)) {
                    let err = wrap_phase((lp * mult).rem_euclid(1.0) - mp.rem_euclid(1.0));
                    if err.abs() > 0.003 {
                        me.sync_bend = (err * 0.8).clamp(-0.03, 0.03);
                    }
                }
            }
        }
    }

    /// Snap deck `i`'s phase to its sync leader immediately.
    fn align_phase(&mut self, i: usize) {
        let leader = 1 - i;
        let l = &self.decks[leader];
        let Some(lp) = l.beat_pos(l.pos) else { return };
        let lb = l.bpm * (1.0 + l.pitch);
        let me = &self.decks[i];
        let (Some(mp), Some(fpb)) = (me.beat_pos(me.pos), me.fpb()) else { return };
        let mult = if me.bpm > 0.0 {
            [0.5, 1.0, 2.0]
                .into_iter()
                .min_by(|a, b| ((lb * a) / me.bpm - 1.0).abs().total_cmp(&((lb * b) / me.bpm - 1.0).abs()))
                .unwrap_or(1.0)
        } else {
            1.0
        };
        let err = wrap_phase((lp * mult).rem_euclid(1.0) - mp.rem_euclid(1.0));
        let new_pos = (me.pos + err * fpb).max(0.0);
        self.decks[i].pos = new_pos;
    }

    pub(crate) fn render_block(&mut self, n: usize) {
        self.update_sync();
        let (xl, xr) = self.mixer.curve.gains(self.mixer.xfader);
        let xf = [xl, xr];
        // ~5ms smoothing per block keeps knob moves click-free.
        let smooth = 1.0 - (-(n as f64) / (0.008 * self.sr)).exp();

        for i in 0..NUM_DECKS {
            let ch = self.mixer.ch[i];
            self.decks[i].filter.set(ch.filter as f64);
            let mut targets = [0.0; 3];
            for b in 0..3 {
                targets[b] = if ch.kill[b] { 0.0 } else { eq_knob_gain(ch.eq[b]) };
                self.eq_gain[i][b] += (targets[b] - self.eq_gain[i][b]) * smooth;
            }
            let eq = self.eq_gain[i];
            let buf = &mut self.scratch[i][..n * 2];
            self.decks[i].render(buf, self.sr, eq);
            self.levels[i] = self.levels[i].max(self.decks[i].peak);
        }

        let master_target = fader_gain(self.mixer.master) * 1.25;
        let mut peak = [0.0f32; 2];
        let start_gain = self.ch_gain;
        let target_gain: [f32; NUM_DECKS] = std::array::from_fn(|i| {
            let ch = self.mixer.ch[i];
            fader_gain(ch.volume) * 10f32.powf(ch.trim_db / 20.0) * xf[i]
        });
        let m0 = self.master_gain;
        for f in 0..n {
            let t = (f + 1) as f32 / n as f32;
            let mg = m0 + (master_target - m0) * t;
            for c in 0..2 {
                let mut s = 0.0;
                for i in 0..NUM_DECKS {
                    let g = start_gain[i] + (target_gain[i] - start_gain[i]) * t;
                    s += self.scratch[i][f * 2 + c] * g;
                }
                let s = soft_clip(s * mg);
                peak[c] = peak[c].max(s.abs());
                self.master[f * 2 + c] = s;
            }
        }
        self.ch_gain = target_gain;
        self.master_gain = master_target;
        self.master_level = [self.master_level[0].max(peak[0]), self.master_level[1].max(peak[1])];

        if let Some(rec) = &mut self.recorder {
            let mut remaining = &self.master[..n * 2];
            while !remaining.is_empty() {
                let room = RECORD_CHUNK_FRAMES * 2 - rec.buf.len();
                let take = room.min(remaining.len());
                rec.buf.extend_from_slice(&remaining[..take]);
                remaining = &remaining[take..];
                if rec.buf.len() >= RECORD_CHUNK_FRAMES * 2 {
                    let fresh = rec.sink.pool.try_recv().unwrap_or_else(|_| Vec::with_capacity(RECORD_CHUNK_FRAMES * 2));
                    let full = std::mem::replace(&mut rec.buf, fresh);
                    if rec.sink.tx.try_send(full).is_err() {
                        rec.dropouts += 1;
                    }
                }
            }
            rec.frames += n as u64;
        }
    }

    fn publish(&mut self) {
        if let Some(mut snap) = self.snapshot.try_lock() {
            for i in 0..NUM_DECKS {
                snap.decks[i] = self.decks[i].snapshot(self.levels[i]);
                self.levels[i] = 0.0;
            }
            snap.master_level = self.master_level;
            self.master_level = [0.0; 2];
            snap.sample_rate = self.sr as u32;
            snap.quantize = self.quantize;
            snap.recording = self.recorder.is_some();
            snap.recorded_frames = self.recorder.as_ref().map(|r| r.frames).unwrap_or(0);
            snap.record_dropouts = self.recorder.as_ref().map(|r| r.dropouts).unwrap_or(0);
        }
    }

    pub(crate) fn handle(&mut self, cmd: Cmd) {
        let jumped = matches!(
            cmd,
            Cmd::SeekFrac(..) | Cmd::HotCue(..) | Cmd::JumpToStart(..) | Cmd::BeatJump(..) | Cmd::CueDown(..) | Cmd::Reloop(..) | Cmd::LoopOut(..)
        );
        self.apply(cmd);
        // After a jump on either deck, snap the synced follower straight back into phase.
        if jumped {
            for j in 0..NUM_DECKS {
                if self.decks[j].synced && self.decks[j].playing {
                    self.update_sync();
                    self.align_phase(j);
                }
            }
        }
    }

    fn apply(&mut self, cmd: Cmd) {
        let q = self.quantize;
        match cmd {
            Cmd::Load(i, req) => {
                if let Some(old) = self.decks[i].load(*req) {
                    let _ = self.garbage.send(old);
                }
            }
            Cmd::Eject(i) => {
                if let Some(old) = self.decks[i].eject() {
                    let _ = self.garbage.send(old);
                }
            }
            Cmd::PlayPause(i) => {
                if self.decks[i].play_pause() && self.decks[i].synced {
                    self.align_phase(i);
                }
            }
            Cmd::CueDown(i) => {
                self.decks[i].cue_down(q);
            }
            Cmd::CueUp(i) => self.decks[i].cue_up(),
            Cmd::HotCue(i, h) => self.decks[i].hot_cue(h, q),
            Cmd::HotCueClear(i, h) => self.decks[i].hot_cues[h] = None,
            Cmd::PitchStep(i, d) => {
                let d0 = &mut self.decks[i];
                d0.synced = false;
                d0.pitch = (d0.pitch + d).clamp(-d0.pitch_range, d0.pitch_range);
            }
            Cmd::SetPitch(i, p) => {
                let d0 = &mut self.decks[i];
                d0.synced = false;
                d0.pitch = p.clamp(-d0.pitch_range, d0.pitch_range);
            }
            Cmd::SetPitchRange(i, r) => {
                let d0 = &mut self.decks[i];
                d0.pitch_range = r;
                if !d0.synced {
                    d0.pitch = d0.pitch.clamp(-r, r);
                }
            }
            Cmd::Bend(i, b) => self.decks[i].bend = b,
            Cmd::SyncToggle(i) => {
                let on = !self.decks[i].synced;
                self.decks[i].synced = on;
                if on {
                    self.decks[1 - i].synced = false;
                    self.update_sync();
                    self.align_phase(i);
                }
            }
            Cmd::SyncOff(i) => self.decks[i].synced = false,
            Cmd::LoopToggle(i) => self.decks[i].loop_toggle(q),
            Cmd::Reloop(i) => self.decks[i].reloop(),
            Cmd::LoopHalve(i) => self.decks[i].loop_halve(),
            Cmd::LoopDouble(i) => self.decks[i].loop_double(),
            Cmd::LoopIn(i) => self.decks[i].loop_in(q),
            Cmd::LoopOut(i) => self.decks[i].loop_out(q),
            Cmd::BeatJump(i, b) => self.decks[i].beat_jump(b),
            Cmd::SeekFrac(i, f) => {
                let d0 = &mut self.decks[i];
                let target = f.clamp(0.0, 1.0) * d0.frames();
                d0.seek(target);
            }
            Cmd::JumpToStart(i) => self.decks[i].seek(0.0),
            Cmd::ScaleBpm(i, f) => self.decks[i].scale_bpm(f),
            Cmd::GridHere(i) => self.decks[i].grid_here(),
            Cmd::Mixer(m) => self.mixer = m,
            Cmd::Quantize(q) => self.quantize = q,
            Cmd::StartRecording(sink) => {
                self.recorder = Some(Recorder {
                    sink,
                    buf: Vec::with_capacity(RECORD_CHUNK_FRAMES * 2),
                    frames: 0,
                    dropouts: 0,
                });
            }
            Cmd::StopRecording => {
                if let Some(rec) = self.recorder.take() {
                    if !rec.buf.is_empty() {
                        let _ = rec.sink.tx.try_send(rec.buf);
                    }
                    // Dropping the sender tells the writer thread to finalise the file.
                }
            }
        }
    }
}

fn wrap_phase(x: f64) -> f64 {
    let mut x = x.rem_euclid(1.0);
    if x > 0.5 {
        x -= 1.0;
    }
    x
}
