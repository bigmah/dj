use dioxus::prelude::*;

use super::app_state;
use crate::keys::help_sections;

#[component]
pub fn Help() -> Element {
    let app = app_state();
    let sections = help_sections();
    rsx! {
        div {
            class: "help-backdrop",
            onclick: move |_| {
                let mut h = app.help;
                h.set(false);
            },
            div { class: "help", onclick: move |e| e.stop_propagation(),
                h2 { "Keyboard" }
                p { class: "hint",
                    "Left hand drives Deck A, right hand drives Deck B. Hold ⌥ Option to turn the same keys into the mixer. "
                    "Press ` or Esc to close."
                }
                div { class: "help-grid",
                    for (title, rows) in sections {
                        div { class: "help-section",
                            h3 { "{title}" }
                            table {
                                thead {
                                    tr {
                                        th { "Deck A" }
                                        th { "Deck B" }
                                        th { "" }
                                    }
                                }
                                tbody {
                                    for (a, b, desc) in rows {
                                        tr {
                                            td { kbd { "{a}" } }
                                            td {
                                                if !b.is_empty() {
                                                    kbd { "{b}" }
                                                }
                                            }
                                            td { "{desc}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
