use dioxus::prelude::*;

use super::{app_state, do_action, fmt_time, DECK_NAMES};
use crate::audio::{engine, Cmd, NUM_HOT_CUES};
use crate::keys::{label, Act, DECK_KEYS, NUDGE};

const PITCH_RANGES: [f64; 4] = [0.08, 0.16, 0.25, 0.5];

fn loop_label(beats: f64) -> String {
    if beats >= 1.0 {
        format!("{}", beats as u32)
    } else {
        format!("1/{}", (1.0 / beats).round() as u32)
    }
}

#[component]
pub fn DeckPanel(deck: usize) -> Element {
    let app = app_state();
    let snap = app.snap.read();
    let s = snap.decks[deck].clone();
    drop(snap);
    let info = app.decks.read()[deck].clone();
    let k = &DECK_KEYS[deck];
    let name = DECK_NAMES[deck];
    let side = if deck == 0 { "deck deck-a" } else { "deck deck-b" };

    let (title, artist) = match &info.track {
        Some(t) => (t.title.clone(), t.artist.clone()),
        None => ("No track loaded".to_string(), format!("Select a track and press {}", label(k.load))),
    };
    let bpm = if s.bpm > 0.0 { format!("{:.1}", s.effective_bpm()) } else { "—".into() };
    let pitch = format!("{:+.2}%", s.pitch * 100.0);
    let elapsed = fmt_time(s.pos);
    let remaining = format!("-{}", fmt_time(s.remaining()));
    let ending = s.playing && s.remaining() < 30.0;
    let range = s.pitch_range;

    let act = move |a: Act| move |_: MouseEvent| do_action(app, a);

    rsx! {
        div { class: "{side}",
            div { class: "deck-head",
                div { class: "deck-letter", "{name}" }
                div { class: "deck-title",
                    div { class: "title", title: "{title}", "{title}" }
                    div { class: "artist", "{artist}" }
                }
                div { class: "deck-bpm",
                    div { class: "bpm", "{bpm}" }
                    div { class: if s.pitch.abs() > 1e-6 { "pitch changed" } else { "pitch" }, "{pitch}" }
                }
            }
            if info.loading {
                div { class: "loading", "Loading & analysing…" }
            }
            canvas { id: "ov-{deck}", class: "overview", "data-deck": "{deck}" }
            div { class: "times",
                span { class: "elapsed", "{elapsed}" }
                span { class: if ending { "remaining ending" } else { "remaining" }, "{remaining}" }
            }

            div { class: "transport",
                button {
                    class: "pad cue",
                    onmousedown: move |_| engine().send(Cmd::CueDown(deck)),
                    onmouseup: move |_| engine().send(Cmd::CueUp(deck)),
                    onmouseleave: move |_| engine().send(Cmd::CueUp(deck)),
                    "CUE"
                    span { class: "kbd", "{label(k.cue)}" }
                }
                button {
                    class: if s.playing { "pad play on" } else { "pad play" },
                    onclick: act(Act::PlayPause(deck)),
                    if s.playing { "❚❚" } else { "▶" }
                    span { class: "kbd", "{label(k.play)}" }
                }
                button {
                    class: if s.synced { "pad sync on" } else { "pad sync" },
                    onclick: act(Act::Sync(deck)),
                    "SYNC"
                    span { class: "kbd", "{label(k.sync)}" }
                }
                div { class: "nudge",
                    button {
                        class: "small",
                        onmousedown: move |_| engine().send(Cmd::Bend(deck, -NUDGE)),
                        onmouseup: move |_| engine().send(Cmd::Bend(deck, 0.0)),
                        onmouseleave: move |_| engine().send(Cmd::Bend(deck, 0.0)),
                        "◀"
                        span { class: "kbd", "{label(k.nudge_back)}" }
                    }
                    button {
                        class: "small",
                        onmousedown: move |_| engine().send(Cmd::Bend(deck, NUDGE)),
                        onmouseup: move |_| engine().send(Cmd::Bend(deck, 0.0)),
                        onmouseleave: move |_| engine().send(Cmd::Bend(deck, 0.0)),
                        "▶"
                        span { class: "kbd", "{label(k.nudge_fwd)}" }
                    }
                }
            }

            div { class: "pitch-row",
                span { class: "kbd", "{label(k.pitch_down)}" }
                input {
                    r#type: "range",
                    class: "pitch-slider",
                    min: "{-range}",
                    max: "{range}",
                    step: "0.0005",
                    value: "{s.pitch}",
                    ondoubleclick: move |_| engine().send(Cmd::SetPitch(deck, 0.0)),
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f64>() {
                            engine().send(Cmd::SetPitch(deck, v));
                        }
                    },
                }
                span { class: "kbd", "{label(k.pitch_up)}" }
                select {
                    class: "range",
                    title: "Pitch range",
                    onchange: move |e| {
                        if let Ok(v) = e.value().parse::<f64>() {
                            engine().send(Cmd::SetPitchRange(deck, v));
                        }
                    },
                    for r in PITCH_RANGES {
                        option { value: "{r}", selected: (r - range).abs() < 1e-6, "±{(r * 100.0) as u32}%" }
                    }
                }
            }

            div { class: "hotcues",
                for h in 0..NUM_HOT_CUES {
                    button {
                        class: if s.hot_cues[h].is_some() { "hot set hot-{h}" } else { "hot" },
                        title: "Click to set/jump, right-click to delete",
                        onclick: act(Act::HotCue(deck, h)),
                        oncontextmenu: move |e| {
                            e.prevent_default();
                            engine().send(Cmd::HotCueClear(deck, h));
                        },
                        span { class: "num", "{h + 1}" }
                        if let Some(t) = s.hot_cues[h] {
                            span { class: "t", "{fmt_time(t)}" }
                        }
                        span { class: "kbd", "{label(k.hot[h])}" }
                    }
                }
            }

            div { class: "loops",
                button { class: "small", onclick: act(Act::LoopHalve(deck)),
                    "½"
                    span { class: "kbd", "{label(k.loop_halve)}" }
                }
                button {
                    class: if s.loop_active { "loop-btn on" } else { "loop-btn" },
                    onclick: act(Act::LoopToggle(deck)),
                    "LOOP {loop_label(s.loop_beats)}"
                    span { class: "kbd", "{label(k.loop_toggle)}" }
                }
                button { class: "small", onclick: act(Act::LoopDouble(deck)),
                    "×2"
                    span { class: "kbd", "{label(k.loop_double)}" }
                }
                button { class: "small", title: "Loop in (Shift+{label(k.loop_halve)})", onclick: act(Act::LoopIn(deck)), "IN" }
                button { class: "small", title: "Loop out (Shift+{label(k.loop_double)})", onclick: act(Act::LoopOut(deck)), "OUT" }
                button { class: "small", title: "Beat jump back (Shift+{label(k.nudge_back)})", onclick: act(Act::BeatJump(deck, -1.0)), "⇤" }
                button { class: "small", title: "Beat jump forward (Shift+{label(k.nudge_fwd)})", onclick: act(Act::BeatJump(deck, 1.0)), "⇥" }
            }

            div { class: "grid-tools",
                span { class: "lbl", "Grid" }
                button { class: "small", title: "Halve BPM", onclick: move |_| engine().send(Cmd::ScaleBpm(deck, 0.5)), "BPM ÷2" }
                button { class: "small", title: "Double BPM", onclick: move |_| engine().send(Cmd::ScaleBpm(deck, 2.0)), "BPM ×2" }
                button { class: "small", title: "Put a beat at the current play position", onclick: move |_| engine().send(Cmd::GridHere(deck)), "Beat here" }
                button {
                    class: "small",
                    title: "Eject",
                    disabled: s.playing,
                    onclick: move |_| {
                        engine().send(Cmd::Eject(deck));
                        let mut decks = app.decks;
                        decks.write()[deck] = Default::default();
                        let _ = app.bridge.send(serde_json::json!({"k": "wave", "deck": deck, "id": 0, "data": ""}));
                    },
                    "Eject"
                }
            }
        }
    }
}
