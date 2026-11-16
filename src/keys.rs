//! Keyboard layout. Left hand controls deck A, right hand deck B (mirrored).
//! Holding Option/Alt switches the letter keys to the mixer layer (EQ / filter / volume).

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    // Momentary: act on press and release.
    Cue(usize),
    Nudge(usize, f64),
    // One-shot.
    PlayPause(usize),
    HotCue(usize, usize),
    HotCueClear(usize, usize),
    Sync(usize),
    PitchReset(usize),
    PitchStep(usize, f64),
    BeatJump(usize, f64),
    LoopToggle(usize),
    Reloop(usize),
    LoopHalve(usize),
    LoopDouble(usize),
    LoopIn(usize),
    LoopOut(usize),
    JumpStart(usize),
    Load(usize),
    Kill(usize, usize),
    FilterReset(usize),
    EqReset(usize),
    XfaderSet(f32),
    LibMove(i32),
    LoadToIdleDeck,
    ToggleRecord,
    ToggleHelp,
    Escape,
    Zoom(f64),
    ToggleQuantize,
    // Continuous while held: value is the rate in units per second.
    Volume(usize, f32),
    Eq(usize, usize, f32),
    Filter(usize, f32),
    Xfader(f32),
    Master(f32),
}

impl Act {
    pub fn is_continuous(&self) -> bool {
        matches!(self, Act::Volume(..) | Act::Eq(..) | Act::Filter(..) | Act::Xfader(..) | Act::Master(..))
    }
    pub fn is_momentary(&self) -> bool {
        matches!(self, Act::Cue(..) | Act::Nudge(..))
    }
    /// Whether OS key-repeat should re-trigger the action.
    pub fn repeats(&self) -> bool {
        matches!(self, Act::PitchStep(..) | Act::LibMove(..) | Act::BeatJump(..) | Act::Zoom(..))
    }
}

pub struct DeckKeys {
    pub hot: [&'static str; 8],
    pub cue: &'static str,
    pub play: &'static str,
    pub sync: &'static str,
    pub nudge_back: &'static str,
    pub nudge_fwd: &'static str,
    pub pitch_down: &'static str,
    pub pitch_up: &'static str,
    pub loop_toggle: &'static str,
    pub loop_halve: &'static str,
    pub loop_double: &'static str,
    pub load: &'static str,
    // Alt layer. Arrays are [low, mid, high].
    pub eq_up: [&'static str; 3],
    pub eq_down: [&'static str; 3],
    pub filter_up: &'static str,
    pub filter_down: &'static str,
    pub vol_up: &'static str,
    pub vol_down: &'static str,
    pub kill: [&'static str; 3],
    pub filter_reset: &'static str,
    pub eq_reset: &'static str,
}

pub const DECK_KEYS: [DeckKeys; 2] = [
    DeckKeys {
        hot: ["Digit1", "Digit2", "Digit3", "Digit4", "KeyQ", "KeyW", "KeyE", "KeyR"],
        cue: "KeyA",
        play: "KeyS",
        sync: "KeyD",
        nudge_back: "KeyF",
        nudge_fwd: "KeyG",
        pitch_down: "KeyZ",
        pitch_up: "KeyX",
        loop_toggle: "KeyC",
        loop_halve: "KeyV",
        loop_double: "KeyB",
        load: "KeyT",
        eq_up: ["KeyE", "KeyW", "KeyQ"],
        eq_down: ["KeyD", "KeyS", "KeyA"],
        filter_up: "KeyR",
        filter_down: "KeyF",
        vol_up: "KeyT",
        vol_down: "KeyG",
        kill: ["Digit3", "Digit2", "Digit1"],
        filter_reset: "Digit4",
        eq_reset: "Digit5",
    },
    DeckKeys {
        hot: ["Digit7", "Digit8", "Digit9", "Digit0", "KeyU", "KeyI", "KeyO", "KeyP"],
        cue: "Semicolon",
        play: "KeyL",
        sync: "KeyK",
        nudge_back: "KeyH",
        nudge_fwd: "KeyJ",
        pitch_down: "Period",
        pitch_up: "Slash",
        loop_toggle: "Comma",
        loop_halve: "KeyM",
        loop_double: "KeyN",
        load: "KeyY",
        eq_up: ["KeyO", "KeyI", "KeyU"],
        eq_down: ["KeyL", "KeyK", "KeyJ"],
        filter_up: "KeyP",
        filter_down: "Semicolon",
        vol_up: "KeyY",
        vol_down: "KeyH",
        kill: ["Digit9", "Digit8", "Digit7"],
        filter_reset: "Digit0",
        eq_reset: "Digit6",
    },
];

pub const PITCH_STEP: f64 = 0.001;
pub const PITCH_STEP_COARSE: f64 = 0.01;
pub const NUDGE: f64 = 0.04;
const FADER_RATE: f32 = 0.8;
const EQ_RATE: f32 = 0.7;
const FILTER_RATE: f32 = 1.2;
const XFADER_RATE: f32 = 1.6;

pub fn map(code: &str, shift: bool, alt: bool, ctrl: bool) -> Option<Act> {
    use Act::*;
    if ctrl {
        return match code {
            "KeyR" => Some(ToggleRecord),
            "KeyQ" => Some(ToggleQuantize),
            _ => None,
        };
    }
    if alt {
        let fast = if shift { 3.0 } else { 1.0 };
        for (d, k) in DECK_KEYS.iter().enumerate() {
            for b in 0..3 {
                if code == k.eq_up[b] {
                    return Some(Eq(d, b, EQ_RATE * fast));
                }
                if code == k.eq_down[b] {
                    return Some(Eq(d, b, -EQ_RATE * fast));
                }
                if code == k.kill[b] {
                    return Some(Kill(d, b));
                }
            }
            if code == k.filter_up {
                return Some(Filter(d, FILTER_RATE * fast));
            }
            if code == k.filter_down {
                return Some(Filter(d, -FILTER_RATE * fast));
            }
            if code == k.vol_up {
                return Some(Volume(d, FADER_RATE * fast));
            }
            if code == k.vol_down {
                return Some(Volume(d, -FADER_RATE * fast));
            }
            if code == k.filter_reset {
                return Some(FilterReset(d));
            }
            if code == k.eq_reset {
                return Some(EqReset(d));
            }
        }
        return match code {
            "Equal" => Some(Master(FADER_RATE * fast)),
            "Minus" => Some(Master(-FADER_RATE * fast)),
            _ => None,
        };
    }

    for (d, k) in DECK_KEYS.iter().enumerate() {
        if let Some(h) = k.hot.iter().position(|c| *c == code) {
            return Some(if shift { HotCueClear(d, h) } else { HotCue(d, h) });
        }
        let act = match code {
            c if c == k.cue => {
                if shift {
                    JumpStart(d)
                } else {
                    Cue(d)
                }
            }
            c if c == k.play => PlayPause(d),
            c if c == k.sync => {
                if shift {
                    PitchReset(d)
                } else {
                    Sync(d)
                }
            }
            c if c == k.nudge_back => {
                if shift {
                    BeatJump(d, -1.0)
                } else {
                    Nudge(d, -NUDGE)
                }
            }
            c if c == k.nudge_fwd => {
                if shift {
                    BeatJump(d, 1.0)
                } else {
                    Nudge(d, NUDGE)
                }
            }
            c if c == k.pitch_down => PitchStep(d, if shift { -PITCH_STEP_COARSE } else { -PITCH_STEP }),
            c if c == k.pitch_up => PitchStep(d, if shift { PITCH_STEP_COARSE } else { PITCH_STEP }),
            c if c == k.loop_toggle => {
                if shift {
                    Reloop(d)
                } else {
                    LoopToggle(d)
                }
            }
            c if c == k.loop_halve => {
                if shift {
                    LoopIn(d)
                } else {
                    LoopHalve(d)
                }
            }
            c if c == k.loop_double => {
                if shift {
                    LoopOut(d)
                } else {
                    LoopDouble(d)
                }
            }
            c if c == k.load => Load(d),
            _ => continue,
        };
        return Some(act);
    }

    match code {
        "ArrowLeft" => Some(if shift { XfaderSet(-1.0) } else { Xfader(-XFADER_RATE) }),
        "ArrowRight" => Some(if shift { XfaderSet(1.0) } else { Xfader(XFADER_RATE) }),
        "Space" => Some(XfaderSet(0.0)),
        "ArrowUp" => Some(LibMove(if shift { -10 } else { -1 })),
        "ArrowDown" => Some(LibMove(if shift { 10 } else { 1 })),
        "PageUp" => Some(LibMove(-20)),
        "PageDown" => Some(LibMove(20)),
        "Enter" => Some(LoadToIdleDeck),
        "Equal" => Some(Zoom(1.25)),
        "Minus" => Some(Zoom(0.8)),
        "Backquote" | "F1" => Some(ToggleHelp),
        "Escape" => Some(Escape),
        _ => None,
    }
}

/// Short label for a `KeyboardEvent.code`.
pub fn label(code: &str) -> &str {
    if let Some(rest) = code.strip_prefix("Key") {
        return rest;
    }
    if let Some(rest) = code.strip_prefix("Digit") {
        return rest;
    }
    match code {
        "Semicolon" => ";",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Minus" => "-",
        "Equal" => "=",
        "Backquote" => "`",
        other => other,
    }
}

/// Help sheet: (section, [(left deck keys, right deck keys, description)]).
pub fn help_sections() -> Vec<(&'static str, Vec<(String, String, &'static str)>)> {
    let [a, b] = &DECK_KEYS;
    let l = |s: &str| label(s).to_string();
    let pair = |x: &str, y: &str| format!("{} {}", label(x), label(y));
    vec![
        (
            "Decks",
            vec![
                (l(a.play), l(b.play), "Play / pause"),
                (l(a.cue), l(b.cue), "Cue (hold to preview when at cue point)"),
                (format!("⇧{}", l(a.cue)), format!("⇧{}", l(b.cue)), "Jump to track start"),
                (l(a.sync), l(b.sync), "Sync to other deck"),
                (format!("⇧{}", l(a.sync)), format!("⇧{}", l(b.sync)), "Reset pitch to 0 (sync off)"),
                (pair(a.nudge_back, a.nudge_fwd), pair(b.nudge_back, b.nudge_fwd), "Nudge slower / faster (hold)"),
                (format!("⇧{} ⇧{}", l(a.nudge_back), l(a.nudge_fwd)), format!("⇧{} ⇧{}", l(b.nudge_back), l(b.nudge_fwd)), "Beat jump back / forward"),
                (pair(a.pitch_down, a.pitch_up), pair(b.pitch_down, b.pitch_up), "Pitch -/+ 0.1% (⇧ = 1%)"),
                (l(a.load), l(b.load), "Load selected track"),
            ],
        ),
        (
            "Hot cues",
            vec![
                ("1 2 3 4 Q W E R".into(), "7 8 9 0 U I O P".into(), "Hot cue 1-8 (set if empty, else jump)"),
                ("⇧ + hot cue".into(), "⇧ + hot cue".into(), "Delete hot cue"),
            ],
        ),
        (
            "Loops",
            vec![
                (l(a.loop_toggle), l(b.loop_toggle), "Auto loop on / off"),
                (format!("⇧{}", l(a.loop_toggle)), format!("⇧{}", l(b.loop_toggle)), "Reloop"),
                (l(a.loop_halve), l(b.loop_halve), "Halve loop"),
                (l(a.loop_double), l(b.loop_double), "Double loop"),
                (format!("⇧{} ⇧{}", l(a.loop_halve), l(a.loop_double)), format!("⇧{} ⇧{}", l(b.loop_halve), l(b.loop_double)), "Manual loop in / out"),
            ],
        ),
        (
            "Mixer (hold ⌥ Option, ⇧ for faster)",
            vec![
                ("⌥Q / ⌥A".into(), "⌥U / ⌥J".into(), "High EQ up / down"),
                ("⌥W / ⌥S".into(), "⌥I / ⌥K".into(), "Mid EQ up / down"),
                ("⌥E / ⌥D".into(), "⌥O / ⌥L".into(), "Low EQ up / down"),
                ("⌥1 ⌥2 ⌥3".into(), "⌥7 ⌥8 ⌥9".into(), "Kill high / mid / low"),
                ("⌥R / ⌥F".into(), "⌥P / ⌥;".into(), "Filter → HPF / → LPF"),
                ("⌥4".into(), "⌥0".into(), "Filter off"),
                ("⌥5".into(), "⌥6".into(), "Reset EQ"),
                ("⌥T / ⌥G".into(), "⌥Y / ⌥H".into(), "Channel volume up / down"),
            ],
        ),
        (
            "Global",
            vec![
                ("← →".into(), String::new(), "Crossfader (hold); ⇧ slams to edge"),
                ("Space".into(), String::new(), "Centre crossfader"),
                ("↑ ↓".into(), String::new(), "Browse library (⇧ = 10 rows)"),
                ("Enter".into(), String::new(), "Load selected into the stopped deck"),
                ("Tab".into(), String::new(), "Search library (Enter/Esc returns)"),
                ("- =".into(), String::new(), "Waveform zoom"),
                ("⌥- ⌥=".into(), String::new(), "Master volume"),
                ("⌃R".into(), String::new(), "Start / stop recording"),
                ("⌃Q".into(), String::new(), "Toggle quantize"),
                ("` / F1".into(), String::new(), "This help"),
            ],
        ),
    ]
}
