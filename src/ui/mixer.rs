use dioxus::prelude::*;

use super::{app_state, AppState, DECK_NAMES};
use crate::audio::{XfCurve, NUM_DECKS};
use crate::keys::{label, DECK_KEYS};

const BANDS: [&str; 3] = ["LOW", "MID", "HIGH"];

#[component]
fn Knob(deck: usize, band: usize) -> Element {
    let app = app_state();
    let m = app.mixer.read();
    let v = m.ch[deck].eq[band];
    let killed = m.ch[deck].kill[band];
    let k = &DECK_KEYS[deck];
    rsx! {
        div { class: "knob-row",
            span { class: "knob-lbl", "{BANDS[band]}" }
            input {
                r#type: "range",
                class: if killed { "knob killed" } else { "knob" },
                min: "0",
                max: "1",
                step: "0.01",
                value: "{v}",
                title: "⌥{label(k.eq_up[band])} / ⌥{label(k.eq_down[band])} · double-click resets",
                ondoubleclick: move |_| app.set_mixer(|m| m.ch[deck].eq[band] = 0.5),
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f32>() {
                        app.set_mixer(|m| m.ch[deck].eq[band] = v);
                    }
                },
            }
            button {
                class: if killed { "kill on" } else { "kill" },
                title: "Kill (⌥{label(k.kill[band])})",
                onclick: move |_| app.set_mixer(|m| m.ch[deck].kill[band] = !m.ch[deck].kill[band]),
                "K"
            }
        }
    }
}

fn channel_strip(app: AppState, deck: usize) -> Element {
    let m = *app.mixer.read();
    let ch = m.ch[deck];
    let level = app.snap.read().decks[deck].level;
    let k = &DECK_KEYS[deck];
    let filter_txt = if ch.filter.abs() < 0.02 {
        "FILTER".to_string()
    } else if ch.filter < 0.0 {
        format!("LPF {:.0}%", -ch.filter * 100.0)
    } else {
        format!("HPF {:.0}%", ch.filter * 100.0)
    };
    rsx! {
        div { class: if deck == 0 { "strip strip-a" } else { "strip strip-b" },
            div { class: "strip-name", "{DECK_NAMES[deck]}" }
            div { class: "knob-row",
                span { class: "knob-lbl", "TRIM" }
                input {
                    r#type: "range",
                    class: "knob",
                    min: "-12",
                    max: "12",
                    step: "0.5",
                    value: "{ch.trim_db}",
                    title: "{ch.trim_db:+.1} dB · double-click resets",
                    ondoubleclick: move |_| app.set_mixer(|m| m.ch[deck].trim_db = 0.0),
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f32>() {
                            app.set_mixer(|m| m.ch[deck].trim_db = v);
                        }
                    },
                }
                span { class: "knob-val", "{ch.trim_db:+.0}" }
            }
            for band in (0..3).rev() {
                Knob { deck, band }
            }
            div { class: "knob-row",
                span { class: "knob-lbl", "{filter_txt}" }
                input {
                    r#type: "range",
                    class: if ch.filter.abs() >= 0.02 { "knob filter active" } else { "knob filter" },
                    min: "-1",
                    max: "1",
                    step: "0.01",
                    value: "{ch.filter}",
                    title: "⌥{label(k.filter_down)} LPF / ⌥{label(k.filter_up)} HPF · ⌥{label(k.filter_reset)} or double-click resets",
                    ondoubleclick: move |_| app.set_mixer(|m| m.ch[deck].filter = 0.0),
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f32>() {
                            app.set_mixer(|m| m.ch[deck].filter = v);
                        }
                    },
                }
            }
            div { class: "fader-wrap",
                div { class: "meter",
                    div { class: if level > 0.98 { "meter-fill clip" } else { "meter-fill" }, style: "height: {level.min(1.0) * 100.0}%" }
                }
                input {
                    r#type: "range",
                    class: "fader",
                    min: "0",
                    max: "1",
                    step: "0.01",
                    value: "{ch.volume}",
                    title: "Volume (⌥{label(k.vol_up)} / ⌥{label(k.vol_down)})",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f32>() {
                            app.set_mixer(|m| m.ch[deck].volume = v);
                        }
                    },
                }
            }
        }
    }
}

#[component]
pub fn Mixer() -> Element {
    let app = app_state();
    let m = *app.mixer.read();
    rsx! {
        div { class: "mixer",
            div { class: "strips",
                for d in 0..NUM_DECKS {
                    {channel_strip(app, d)}
                }
            }
            div { class: "xfader-row",
                span { class: "kbd", "←" }
                input {
                    r#type: "range",
                    class: "xfader",
                    min: "-1",
                    max: "1",
                    step: "0.01",
                    value: "{m.xfader}",
                    title: "Crossfader (← → hold, ⇧ to slam, Space centres)",
                    ondoubleclick: move |_| app.set_mixer(|m| m.xfader = 0.0),
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f32>() {
                            app.set_mixer(|m| m.xfader = v);
                        }
                    },
                }
                span { class: "kbd", "→" }
            }
            div { class: "master-row",
                span { class: "knob-lbl", "MASTER" }
                input {
                    r#type: "range",
                    class: "knob",
                    min: "0",
                    max: "1",
                    step: "0.01",
                    value: "{m.master}",
                    title: "Master (⌥- / ⌥=)",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f32>() {
                            app.set_mixer(|m| m.master = v);
                            app.mark_dirty();
                        }
                    },
                }
                select {
                    title: "Crossfader curve",
                    onchange: move |e| {
                        let v = e.value();
                        if let Some(c) = XfCurve::ALL.into_iter().find(|c| c.label() == v) {
                            app.set_mixer(|m| m.curve = c);
                            app.mark_dirty();
                        }
                    },
                    for c in XfCurve::ALL {
                        option { value: "{c.label()}", selected: c == m.curve, "{c.label()}" }
                    }
                }
            }
        }
    }
}
