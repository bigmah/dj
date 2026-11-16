use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};

pub const NUM_DECKS: usize = 2;
pub const NUM_HOT_CUES: usize = 8;

/// Fully decoded track, interleaved stereo f32.
pub struct TrackAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub frames: usize,
}

impl TrackAudio {
    pub fn duration(&self) -> f64 {
        self.frames as f64 / self.sample_rate as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum XfCurve {
    /// Constant power, dips nothing at centre.
    Smooth,
    /// Both channels full at centre (dipless).
    #[default]
    Blend,
    /// Scratch cut: very fast at the edges.
    Cut,
}

impl XfCurve {
    pub const ALL: [XfCurve; 3] = [XfCurve::Smooth, XfCurve::Blend, XfCurve::Cut];
    pub fn label(self) -> &'static str {
        match self {
            XfCurve::Smooth => "Smooth",
            XfCurve::Blend => "Blend",
            XfCurve::Cut => "Cut",
        }
    }
    /// Returns (left gain, right gain) for crossfader x in -1..1.
    pub fn gains(self, x: f32) -> (f32, f32) {
        let x = x.clamp(-1.0, 1.0);
        match self {
            XfCurve::Smooth => {
                let t = (x + 1.0) * 0.5 * std::f32::consts::FRAC_PI_2;
                (t.cos(), t.sin())
            }
            XfCurve::Blend => ((1.0 - x).min(1.0), (1.0 + x).min(1.0)),
            XfCurve::Cut => {
                let edge = 0.94;
                let l = if x <= edge { 1.0 } else { ((1.0 - x) / (1.0 - edge)).max(0.0) };
                let r = if x >= -edge { 1.0 } else { ((1.0 + x) / (1.0 - edge)).max(0.0) };
                (l, r)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelState {
    /// Fader 0..1
    pub volume: f32,
    /// Manual trim in dB on top of auto gain.
    pub trim_db: f32,
    /// EQ knobs [low, mid, high], 0..1 with 0.5 = unity.
    pub eq: [f32; 3],
    pub kill: [bool; 3],
    /// -1 (LPF) .. 0 (off) .. 1 (HPF)
    pub filter: f32,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self { volume: 0.8, trim_db: 0.0, eq: [0.5; 3], kill: [false; 3], filter: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerState {
    pub ch: [ChannelState; NUM_DECKS],
    /// -1 = full left, 1 = full right
    pub xfader: f32,
    pub curve: XfCurve,
    pub master: f32,
}

impl Default for MixerState {
    fn default() -> Self {
        Self { ch: [ChannelState::default(); NUM_DECKS], xfader: 0.0, curve: XfCurve::Blend, master: 0.8 }
    }
}

/// Everything needed to put a track on a deck.
pub struct LoadRequest {
    pub audio: Arc<TrackAudio>,
    pub load_id: u64,
    pub bpm: Option<f64>,
    /// Seconds to the first beat of the grid.
    pub grid_offset: f64,
    pub auto_gain: f32,
    pub cue: f64,
    pub hot_cues: [Option<f64>; NUM_HOT_CUES],
}

pub struct RecordSink {
    /// Full buffers of interleaved stereo master audio.
    pub tx: Sender<Vec<f32>>,
    /// Empty buffers coming back from the writer for reuse.
    pub pool: Receiver<Vec<f32>>,
}

pub enum Cmd {
    Load(usize, Box<LoadRequest>),
    Eject(usize),
    PlayPause(usize),
    CueDown(usize),
    CueUp(usize),
    /// Jump to (or set if empty) a hot cue.
    HotCue(usize, usize),
    HotCueClear(usize, usize),
    /// Relative pitch change (0.001 = 0.1 %).
    PitchStep(usize, f64),
    SetPitch(usize, f64),
    SetPitchRange(usize, f64),
    /// Temporary pitch bend while a nudge key is held (0 releases).
    Bend(usize, f64),
    SyncToggle(usize),
    SyncOff(usize),
    LoopToggle(usize),
    Reloop(usize),
    LoopHalve(usize),
    LoopDouble(usize),
    LoopIn(usize),
    LoopOut(usize),
    BeatJump(usize, f64),
    /// Seek to fraction of track (0..1).
    SeekFrac(usize, f64),
    JumpToStart(usize),
    /// Multiply BPM (0.5 / 2.0 for fixing detection) keeps grid anchor.
    ScaleBpm(usize, f64),
    /// Move the beat grid so the current play position is on a beat.
    GridHere(usize),
    Mixer(MixerState),
    Quantize(bool),
    StartRecording(RecordSink),
    StopRecording,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckSnap {
    pub loaded: bool,
    pub load_id: u64,
    pub pos: f64,
    pub duration: f64,
    pub playing: bool,
    pub pitch: f64,
    pub pitch_range: f64,
    /// Total current playback speed multiplier (pitch + bends).
    pub speed: f64,
    /// Track's native BPM (0 if unknown).
    pub bpm: f64,
    pub grid_offset: f64,
    pub cue: f64,
    pub hot_cues: [Option<f64>; NUM_HOT_CUES],
    pub loop_active: bool,
    pub loop_start: Option<f64>,
    pub loop_end: Option<f64>,
    pub loop_beats: f64,
    pub synced: bool,
    /// Post-EQ, pre-fader peak level.
    pub level: f32,
}

impl DeckSnap {
    pub fn effective_bpm(&self) -> f64 {
        self.bpm * (1.0 + self.pitch)
    }
    pub fn remaining(&self) -> f64 {
        (self.duration - self.pos).max(0.0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct EngineSnapshot {
    pub decks: [DeckSnap; NUM_DECKS],
    pub master_level: [f32; 2],
    pub sample_rate: u32,
    pub quantize: bool,
    pub recording: bool,
    pub recorded_frames: u64,
    pub record_dropouts: u64,
}
