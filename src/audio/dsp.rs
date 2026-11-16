//! Small, allocation-free DSP building blocks used on the audio thread.

use std::f64::consts::PI;

/// Stereo RBJ biquad in transposed direct form II.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: [f64; 2],
    z2: [f64; 2],
}

impl Default for Biquad {
    fn default() -> Self {
        // Identity filter.
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: [0.0; 2], z2: [0.0; 2] }
    }
}

pub const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

impl Biquad {
    pub fn lowpass(sr: f64, freq: f64, q: f64) -> Self {
        let mut b = Self::default();
        b.set_lowpass(sr, freq, q);
        b
    }

    pub fn highpass(sr: f64, freq: f64, q: f64) -> Self {
        let mut b = Self::default();
        b.set_highpass(sr, freq, q);
        b
    }

    fn set(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    pub fn set_lowpass(&mut self, sr: f64, freq: f64, q: f64) {
        let w0 = 2.0 * PI * (freq / sr).clamp(1e-5, 0.49);
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        self.set((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }

    pub fn set_highpass(&mut self, sr: f64, freq: f64, q: f64) {
        let w0 = 2.0 * PI * (freq / sr).clamp(1e-5, 0.49);
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        self.set((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha);
    }

    #[inline(always)]
    pub fn process(&mut self, ch: usize, x: f64) -> f64 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }
}

/// 4th order Linkwitz-Riley section (two cascaded Butterworth biquads).
#[derive(Clone, Copy, Debug, Default)]
struct Lr4 {
    a: Biquad,
    b: Biquad,
}

impl Lr4 {
    fn lowpass(sr: f64, f: f64) -> Self {
        Self { a: Biquad::lowpass(sr, f, BUTTERWORTH_Q), b: Biquad::lowpass(sr, f, BUTTERWORTH_Q) }
    }
    fn highpass(sr: f64, f: f64) -> Self {
        Self { a: Biquad::highpass(sr, f, BUTTERWORTH_Q), b: Biquad::highpass(sr, f, BUTTERWORTH_Q) }
    }
    #[inline(always)]
    fn process(&mut self, ch: usize, x: f64) -> f64 {
        self.b.process(ch, self.a.process(ch, x))
    }
    fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
}

/// DJ-style 3 band isolator: splits the signal with Linkwitz-Riley crossovers so
/// each band can be fully killed.
#[derive(Clone, Copy, Debug)]
pub struct Isolator {
    low_lp: Lr4,
    low_hp: Lr4,
    high_lp: Lr4,
    high_hp: Lr4,
}

pub const EQ_LOW_XOVER: f64 = 250.0;
pub const EQ_HIGH_XOVER: f64 = 2500.0;

impl Isolator {
    pub fn new(sr: f64) -> Self {
        Self {
            low_lp: Lr4::lowpass(sr, EQ_LOW_XOVER),
            low_hp: Lr4::highpass(sr, EQ_LOW_XOVER),
            high_lp: Lr4::lowpass(sr, EQ_HIGH_XOVER),
            high_hp: Lr4::highpass(sr, EQ_HIGH_XOVER),
        }
    }

    /// `g` = [low, mid, high] linear gains.
    #[inline(always)]
    pub fn process(&mut self, ch: usize, x: f64, g: [f64; 3]) -> f64 {
        let low = self.low_lp.process(ch, x);
        let rest = self.low_hp.process(ch, x);
        let mid = self.high_lp.process(ch, rest);
        let high = self.high_hp.process(ch, rest);
        low * g[0] + mid * g[1] + high * g[2]
    }

    pub fn reset(&mut self) {
        self.low_lp.reset();
        self.low_hp.reset();
        self.high_lp.reset();
        self.high_hp.reset();
    }
}

/// One-knob DJ filter: negative = low-pass, positive = high-pass.
#[derive(Clone, Copy, Debug)]
pub struct DjFilter {
    sr: f64,
    biquad: Biquad,
    current: f64,
}

const FILTER_Q: f64 = 0.9;
const FILTER_DEADZONE: f64 = 0.02;

impl DjFilter {
    pub fn new(sr: f64) -> Self {
        Self { sr, biquad: Biquad::default(), current: 0.0 }
    }

    /// Update the knob position (-1..1). Cheap if unchanged.
    pub fn set(&mut self, v: f64) {
        if (v - self.current).abs() < 1e-6 {
            return;
        }
        let was_bypassed = self.current.abs() < FILTER_DEADZONE;
        let was_lp = self.current < 0.0;
        self.current = v;
        if v.abs() < FILTER_DEADZONE {
            return;
        }
        if was_bypassed || was_lp != (v < 0.0) {
            self.biquad.reset();
        }
        if v < 0.0 {
            // 20 kHz -> 60 Hz, exponential.
            let f = 20_000.0 * (60.0f64 / 20_000.0).powf(-v);
            self.biquad.set_lowpass(self.sr, f, FILTER_Q);
        } else {
            // 20 Hz -> 8 kHz
            let f = 20.0 * (8_000.0f64 / 20.0).powf(v);
            self.biquad.set_highpass(self.sr, f, FILTER_Q);
        }
    }

    #[inline(always)]
    pub fn process(&mut self, ch: usize, x: f64) -> f64 {
        if self.current.abs() < FILTER_DEADZONE {
            x
        } else {
            self.biquad.process(ch, x)
        }
    }
}

/// Catmull-Rom cubic interpolation.
#[inline(always)]
pub fn cubic(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

/// Gentle soft clipper for the master bus: transparent below 0.9.
#[inline(always)]
pub fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.9 {
        x
    } else {
        let y = 0.9 + 0.1 * ((a - 0.9) / 0.1).tanh();
        y.copysign(x)
    }
}

/// EQ knob (0..1, 0.5 = unity) to linear gain. 0 is a full kill, 1 is +6 dB.
pub fn eq_knob_gain(v: f32) -> f64 {
    let v = v.clamp(0.0, 1.0) as f64;
    if v >= 0.5 {
        10f64.powf(((v - 0.5) / 0.5) * 6.0 / 20.0)
    } else {
        let t = v / 0.5;
        t * t
    }
}

/// Channel fader position to gain.
pub fn fader_gain(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    v * v
}
