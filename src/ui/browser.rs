use dioxus::prelude::*;

use super::{app_state, fmt_time, SortKey, DECK_NAMES};
use crate::audio::NUM_DECKS;

/// Rendering tens of thousands of rows makes keyboard browsing sluggish; search narrows it.
const MAX_ROWS: usize = 1500;

#[component]
fn SortHeader(label: &'static str, sort_key: SortKey, class: &'static str) -> Element {
    let app = app_state();
    let (cur, asc) = (app.sort)();
    let arrow = if cur == sort_key { if asc { " ▲" } else { " ▼" } } else { "" };
    rsx! {
        div {
            class: "th {class}",
            onclick: move |_| {
                let mut s = app.sort;
                let (cur, asc) = *s.peek();
                s.set(if cur == sort_key { (sort_key, !asc) } else { (sort_key, true) });
            },
            "{label}{arrow}"
        }
    }
}

#[component]
pub fn Library() -> Element {
    let app = app_state();
    let filtered = app.filtered.read();
    let db = app.db.read();
    let selected = (app.selected)().min(filtered.len().saturating_sub(1));
    let decks = app.decks.read();
    let loaded: [Option<String>; NUM_DECKS] = std::array::from_fn(|d| decks[d].track.as_ref().map(|t| t.path.clone()));
    let total = filtered.len();
    let scanning = (app.scanning)();
    let roots = db.roots.clone();

    rsx! {
        div { class: "library",
            div { class: "lib-bar",
                input {
                    id: "search",
                    r#type: "search",
                    placeholder: "Search  (Tab)",
                    value: "{app.query}",
                    oninput: move |e| {
                        let mut q = app.query;
                        q.set(e.value());
                        let mut sel = app.selected;
                        sel.set(0);
                    },
                }
                span { class: "count",
                    if scanning { "Scanning… " }
                    "{total} tracks"
                }
                div { class: "spacer" }
                for (i, root) in roots.iter().enumerate() {
                    span { class: "root", title: "{root}",
                        "{short_root(root)}"
                        button {
                            class: "x",
                            title: "Remove folder from library",
                            onclick: move |_| {
                                let mut db = app.db;
                                db.write().roots.remove(i);
                                app.rescan();
                            },
                            "×"
                        }
                    }
                }
                button {
                    class: "small",
                    onclick: move |_| {
                        spawn(async move {
                            if let Some(dir) = rfd::AsyncFileDialog::new().set_title("Add music folder").pick_folder().await {
                                let path = dir.path().to_string_lossy().into_owned();
                                let mut db = app.db;
                                if !db.peek().roots.contains(&path) {
                                    db.write().roots.push(path);
                                }
                                app.rescan();
                            }
                        });
                    },
                    "+ Folder"
                }
                button { class: "small", disabled: scanning, onclick: move |_| app.rescan(), "Rescan" }
                button {
                    class: "small",
                    title: "Where set recordings are saved",
                    onclick: move |_| {
                        spawn(async move {
                            if let Some(dir) = rfd::AsyncFileDialog::new().set_title("Recordings folder").pick_folder().await {
                                let mut db = app.db;
                                db.write().record_dir = Some(dir.path().to_string_lossy().into_owned());
                                app.mark_dirty();
                                app.toast(format!("Recordings will be saved to {}", dir.path().display()));
                            }
                        });
                    },
                    "Rec folder…"
                }
            }
            div { class: "table",
                div { class: "thead",
                    div { class: "th deck-col", "" }
                    SortHeader { label: "Title", sort_key: SortKey::Title, class: "title-col" }
                    SortHeader { label: "Artist", sort_key: SortKey::Artist, class: "artist-col" }
                    SortHeader { label: "BPM", sort_key: SortKey::Bpm, class: "num-col" }
                    SortHeader { label: "Time", sort_key: SortKey::Duration, class: "num-col" }
                    SortHeader { label: "Album", sort_key: SortKey::Album, class: "album-col" }
                    SortHeader { label: "Plays", sort_key: SortKey::Plays, class: "num-col" }
                }
                div { class: "tbody",
                    for (row, &ti) in filtered.iter().enumerate().take(MAX_ROWS) {
                        {
                            let t = &db.tracks[ti];
                            let on_deck: Vec<&str> = (0..NUM_DECKS)
                                .filter(|d| loaded[*d].as_deref() == Some(t.path.as_str()))
                                .map(|d| DECK_NAMES[d])
                                .collect();
                            let mut cls = String::from("lib-row");
                            if row == selected { cls.push_str(" sel"); }
                            if t.play_count > 0 { cls.push_str(" played"); }
                            let bpm = t.bpm.map(|b| format!("{b:.1}")).unwrap_or_default();
                            let dur = t.duration.map(fmt_time).unwrap_or_default();
                            let dur = dur.split('.').next().unwrap_or("").to_string();
                            rsx! {
                                div {
                                    key: "{t.path}",
                                    class: "{cls}",
                                    onclick: move |_| {
                                        let mut sel = app.selected;
                                        sel.set(row);
                                    },
                                    ondoubleclick: move |_| {
                                        let mut sel = app.selected;
                                        sel.set(row);
                                        app.load_to_idle_deck();
                                    },
                                    div { class: "td deck-col",
                                        for d in on_deck {
                                            span { class: "badge badge-{d}", "{d}" }
                                        }
                                    }
                                    div { class: "td title-col", "{t.title}" }
                                    div { class: "td artist-col", "{t.artist}" }
                                    div { class: "td num-col", "{bpm}" }
                                    div { class: "td num-col", "{dur}" }
                                    div { class: "td album-col", "{t.album}" }
                                    div { class: "td num-col", if t.play_count > 0 { "{t.play_count}" } }
                                }
                            }
                        }
                    }
                    if total > MAX_ROWS {
                        div { class: "more", "Showing {MAX_ROWS} of {total} — type to search" }
                    }
                    if total == 0 && !scanning {
                        div { class: "more",
                            if db.tracks.is_empty() {
                                "No tracks found. Add a music folder with “+ Folder”."
                            } else {
                                "No matches."
                            }
                        }
                    }
                }
            }
        }
    }
}

fn short_root(root: &str) -> String {
    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
    if !home.is_empty() && root.starts_with(&home) {
        format!("~{}", &root[home.len()..])
    } else {
        root.to_string()
    }
}
