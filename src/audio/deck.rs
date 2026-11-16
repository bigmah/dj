//! Per-deck playback state, owned by the audio thread.

use std::sync::Arc;

use super::dsp::{cubic, DjFilter, Isolator};
use super::types::*;

pub const MIN_LOOP_BEATS: f64 = 1.0 / 32.0;
pub const MAX_LOOP_BEATS: f64 = 64.0;

pub struct Deck {
    pub track: Option<Arc<TrackAudio>>,
    pub load_id: u64,
    /// Native BPM of the track, 0 when unknown.
    pub bpm: f64,
    /// Grid anchor, in track frames.
    pub grid_offset: f64,
    pub auto_gain: f32,

    /// Play position in track frames.
    pub pos: f64,
    pub playing: bool,
    pub pitch: f64,
    pub pitch_range: f64,
    pub bend: f64,
    /// Small rate correction used by sync to keep phase locked.
    pub sync_bend: f64,
    pub synced: bool,

    pub cue: f64,
    /// Cue button held while previewing from the cue point.
    cue_preview: bool,
    /// Play pressed during a cue preview: keep playing on release.
    cue_latched: bool,
    pub hot_cues: [Option<f64>; NUM_HOT_CUES],

    pub loop_start: Option<f64>,
    pub loop_end: Option<f64>,
    pub loop_active: bool,
    pub loop_beats: f64,

    pub eq: Isolator,
    pub filter: DjFilter,
    pub peak: f32,
}

impl Deck {
    pub fn new(out_sr: f64) -> Self {
        Self {
            track: None,
            load_id: 0,
            bpm: 0.0,
            grid_offset: 0.0,
            auto_gain: 1.0,
            pos: 0.0,
            playing: false,
            pitch: 0.0,
            pitch_range: 0.08,
            bend: 0.0,
            sync_bend: 0.0,
            synced: false,
            cue: 0.0,
            cue_preview: false,
            cue_latched: false,
            hot_cues: [None; NUM_HOT_CUES],
            loop_start: None,
            loop_end: None,
            loop_active: false,
            loop_beats: 4.0,
            eq: Isolator::new(out_sr),
            filter: DjFilter::new(out_sr),
            peak: 0.0,
        }
    }

    pub fn sr(&self) -> f64 {
        self.track.as_ref().map(|t| t.sample_rate as f64).unwrap_or(44_100.0)
    }

    pub fn frames(&self) -> f64 {
        self.track.as_ref().map(|t| t.frames as f64).unwrap_or(0.0)
    }

    /// Frames per beat at the track's native tempo.
    pub fn fpb(&self) -> Option<f64> {
        (self.bpm > 0.0 && self.track.is_some()).then(|| self.sr() * 60.0 / self.bpm)
    }

    pub fn speed(&self) -> f64 {
        (1.0 + self.pitch + self.bend + self.sync_bend).max(0.0)
    }

    pub fn beat_pos(&self, pos: f64) -> Option<f64> {
        self.fpb().map(|fpb| (pos - self.grid_offset) / fpb)
    }

    pub fn nearest_beat(&self, pos: f64) -> f64 {
        match self.fpb() {
            Some(fpb) => self.grid_offset + ((pos - self.grid_offset) / fpb).round() * fpb,
            None => pos,
        }
    }

    pub fn quantized(&self, pos: f64, quantize: bool) -> f64 {
        if quantize {
            self.nearest_beat(pos).max(0.0)
        } else {
            pos
        }
    }

    pub fn load(&mut self, req: LoadRequest) -> Option<Arc<TrackAudio>> {
        let sr = req.audio.sample_rate as f64;
        let old = self.track.replace(req.audio);
        self.load_id = req.load_id;
        self.bpm = req.bpm.unwrap_or(0.0);
        self.grid_offset = req.grid_offset * sr;
        self.auto_gain = req.auto_gain;
        self.cue = req.cue * sr;
        self.hot_cues = req.hot_cues.map(|c| c.map(|s| s * sr));
        self.pos = self.cue;
        self.playing = false;
        self.bend = 0.0;
        self.sync_bend = 0.0;
        self.cue_preview = false;
        self.cue_latched = false;
        self.loop_start = None;
        self.loop_end = None;
        self.loop_active = false;
        self.eq.reset();
        old
    }

    pub fn eject(&mut self) -> Option<Arc<TrackAudio>> {
        self.playing = false;
        self.synced = false;
        self.loop_active = false;
        self.load_id = 0;
        self.track.take()
    }

    pub fn play_pause(&mut self) -> bool {
        if self.track.is_none() {
            return false;
        }
        if self.cue_preview {
            // Serato/CDJ: play while holding cue keeps playing on release.
            self.cue_latched = true;
            return false;
        }
        self.playing = !self.playing;
        if self.playing && self.pos >= self.frames() - 1.0 {
            self.pos = self.cue;
        }
        self.playing
    }

    /// Returns true if playback started (so the caller can phase-align).
    pub fn cue_down(&mut self, quantize: bool) -> bool {
        if self.track.is_none() {
            return false;
        }
        if self.playing {
            self.playing = false;
            self.pos = self.cue;
            return false;
        }
        let sr = self.sr();
        if (self.pos - self.cue).abs() < sr * 0.005 {
            self.cue_preview = true;
            self.cue_latched = false;
            self.playing = true;
            true
        } else {
            self.cue = self.quantized(self.pos, quantize);
            self.pos = self.cue;
            false
        }
    }

    pub fn cue_up(&mut self) {
        if !self.cue_preview {
            return;
        }
        self.cue_preview = false;
        if !self.cue_latched {
            self.playing = false;
            self.pos = self.cue;
            self.loop_active = false;
        }
        self.cue_latched = false;
    }

    pub fn hot_cue(&mut self, idx: usize, quantize: bool) {
        if self.track.is_none() {
            return;
        }
        match self.hot_cues[idx] {
            None => self.hot_cues[idx] = Some(self.quantized(self.pos, quantize)),
            Some(target) => {
                let mut dest = target;
                // Keep the groove: land at the same position within the beat.
                if quantize && self.playing {
                    if let Some(fpb) = self.fpb() {
                        let into_beat = self.pos - self.nearest_beat(self.pos);
                        if into_beat.abs() < fpb * 0.5 {
                            dest = target + into_beat;
                        }
                    }
                }
                self.seek(dest);
                if !self.playing {
                    self.cue = target;
                }
            }
        }
    }

    pub fn seek(&mut self, frames: f64) {
        self.pos = frames.clamp(0.0, (self.frames() - 1.0).max(0.0));
        if self.loop_active {
            if let (Some(s), Some(e)) = (self.loop_start, self.loop_end) {
                if self.pos < s || self.pos > e {
                    self.loop_active = false;
                }
            }
        }
    }

    fn beat_len_or_half_second(&self) -> f64 {
        self.fpb().unwrap_or(self.sr() * 0.5)
    }

    pub fn loop_toggle(&mut self, quantize: bool) {
        if self.track.is_none() {
            return;
        }
        if self.loop_active {
            self.loop_active = false;
            return;
        }
        let fpb = self.beat_len_or_half_second();
        let start = self.quantized(self.pos, quantize || self.fpb().is_some());
        let start = start.min(self.pos + fpb * 0.25).max(0.0);
        self.loop_start = Some(start);
        self.loop_end = Some(start + self.loop_beats * fpb);
        self.loop_active = true;
    }

    pub fn reloop(&mut self) {
        if let (Some(s), Some(_)) = (self.loop_start, self.loop_end) {
            self.loop_active = true;
            self.pos = s;
        }
    }

    fn resize_loop(&mut self, beats: f64) {
        self.loop_beats = beats.clamp(MIN_LOOP_BEATS, MAX_LOOP_BEATS);
        if !self.loop_active {
            return;
        }
        if let Some(s) = self.loop_start {
            let len = self.loop_beats * self.beat_len_or_half_second();
            self.loop_end = Some(s + len);
            if self.pos >= s + len {
                self.pos = s + (self.pos - s) % len;
            }
        }
    }

    pub fn loop_halve(&mut self) {
        self.resize_loop(self.loop_beats / 2.0);
    }

    pub fn loop_double(&mut self) {
        self.resize_loop(self.loop_beats * 2.0);
    }

    pub fn loop_in(&mut self, quantize: bool) {
        if self.track.is_none() {
            return;
        }
        self.loop_start = Some(self.quantized(self.pos, quantize));
        self.loop_end = None;
        self.loop_active = false;
    }

    pub fn loop_out(&mut self, quantize: bool) {
        let Some(s) = self.loop_start else { return };
        let e = self.quantized(self.pos, quantize);
        if e > s + 64.0 {
            self.loop_end = Some(e);
            self.loop_active = true;
            if let Some(fpb) = self.fpb() {
                self.loop_beats = (e - s) / fpb;
            }
            self.pos = s;
        }
    }

    pub fn beat_jump(&mut self, beats: f64) {
        if self.track.is_none() {
            return;
        }
        let delta = beats * self.beat_len_or_half_second();
        if self.loop_active {
            if let (Some(s), Some(e)) = (self.loop_start, self.loop_end) {
                self.loop_start = Some(s + delta);
                self.loop_end = Some(e + delta);
            }
        }
        self.pos = (self.pos + delta).clamp(0.0, (self.frames() - 1.0).max(0.0));
    }

    pub fn scale_bpm(&mut self, factor: f64) {
        if self.bpm > 0.0 {
            self.bpm *= factor;
        }
    }

    pub fn grid_here(&mut self) {
        if self.fpb().is_some() {
            self.grid_offset = self.pos;
        }
    }

    pub fn snapshot(&self, prev_level: f32) -> DeckSnap {
        let sr = self.sr();
        let to_s = |f: f64| f / sr;
        DeckSnap {
            loaded: self.track.is_some(),
            load_id: self.load_id,
            pos: to_s(self.pos),
            duration: to_s(self.frames()),
            playing: self.playing,
            pitch: self.pitch,
            pitch_range: self.pitch_range,
            speed: self.speed(),
            bpm: self.bpm,
            grid_offset: to_s(self.grid_offset),
            cue: to_s(self.cue),
            hot_cues: self.hot_cues.map(|c| c.map(to_s)),
            loop_active: self.loop_active,
            loop_start: self.loop_start.map(to_s),
            loop_end: self.loop_end.map(to_s),
            loop_beats: self.loop_beats,
            synced: self.synced,
            level: prev_level,
        }
    }

    /// Render `out.len() / 2` frames of interleaved stereo (post EQ + filter, pre fader).
    pub fn render(&mut self, out: &mut [f32], out_sr: f64, eq_gains: [f64; 3]) {
        let track = match (&self.track, self.playing) {
            (Some(t), true) => t.clone(),
            _ => {
                out.fill(0.0);
                self.peak = 0.0;
                return;
            }
        };
        let samples = &track.samples;
        let last = track.frames.saturating_sub(1) as isize;
        let step = track.sample_rate as f64 / out_sr * self.speed();
        let gain = self.auto_gain as f64;
        let mut peak = 0.0f32;

        let at = |i: isize, ch: usize| -> f32 { samples[(i.clamp(0, last) as usize) * 2 + ch] };

        for frame in out.chunks_exact_mut(2) {
            if !self.playing {
                frame[0] = 0.0;
                frame[1] = 0.0;
                continue;
            }
            let i = self.pos.floor() as isize;
            let t = (self.pos - i as f64) as f32;
            for ch in 0..2 {
                let s = cubic(at(i - 1, ch), at(i, ch), at(i + 1, ch), at(i + 2, ch), t) as f64 * gain;
                let s = self.eq.process(ch, s, eq_gains);
                let s = self.filter.process(ch, s) as f32;
                peak = peak.max(s.abs());
                frame[ch] = s;
            }

            let prev = self.pos;
            self.pos += step;
            if self.loop_active {
                if let (Some(s), Some(e)) = (self.loop_start, self.loop_end) {
                    if prev < e && self.pos >= e && e > s {
                        self.pos -= e - s;
                    }
                }
            }
            if self.pos >= last as f64 {
                self.pos = last as f64;
                self.playing = false;
            }
        }
        self.peak = peak;
    }
}
