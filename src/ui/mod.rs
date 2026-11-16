mod deck;
mod help;
mod browser;
mod mixer;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use dioxus::document::Eval;
use dioxus::prelude::*;
use serde::Deserialize;
use serde_json::json;

use crate::analysis;
use crate::audio::{engine, Cmd, EngineSnapshot, LoadRequest, MixerState, TrackAudio, NUM_DECKS};
use crate::decode;
use crate::keys::{self, Act};
use crate::library::{self, LibraryDb, TrackInfo};
use crate::recorder::{self, Recording, TracklistEntry};

const CSS: &str = include_str!("../../assets/style.css");
const BRIDGE_JS: &str = include_str!("../../assets/bridge.js");

pub const DECK_NAMES: [&str; NUM_DECKS] = ["A", "B"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckInfo {
    pub track: Option<TrackInfo>,
    pub load_id: u64,
    pub loading: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Artist,
    Title,
    Bpm,
    Duration,
    Album,
    Plays,
}

/// All UI state. Every field is a Copy handle, so the struct is cheap to pass around.
#[derive(Clone, Copy)]
pub struct AppState {
    pub snap: Signal<EngineSnapshot>,
    pub mixer: Signal<MixerState>,
    pub decks: Signal<[DeckInfo; NUM_DECKS]>,
    pub db: Signal<LibraryDb>,
    pub query: Signal<String>,
    pub selected: Signal<usize>,
    pub sort: Signal<(SortKey, bool)>,
    pub filtered: Memo<Vec<usize>>,
    pub help: Signal<bool>,
    pub toast: Signal<Option<(u64, String)>>,
    pub recording: Signal<Option<Recording>>,
    pub scanning: Signal<bool>,
    pub zoom: Signal<f64>,
    pub quantize: Signal<bool>,
    pub bridge: Eval,
    db_dirty: Signal<bool>,
    next_load_id: Signal<u64>,
}

pub fn app_state() -> AppState {
    use_context::<AppState>()
}

impl AppState {
    pub fn toast(mut self, msg: impl Into<String>) {
        let id = self.toast.peek().as_ref().map(|t| t.0 + 1).unwrap_or(1);
        self.toast.set(Some((id, msg.into())));
        spawn(async move {
            tokio::time::sleep(Duration::from_millis(2800)).await;
            if self.toast.peek().as_ref().map(|t| t.0) == Some(id) {
                self.toast.set(None);
            }
        });
    }

    pub fn set_mixer(mut self, f: impl FnOnce(&mut MixerState)) {
        let mut m = *self.mixer.peek();
        f(&mut m);
        if m != *self.mixer.peek() {
            self.mixer.set(m);
            engine().send(Cmd::Mixer(m));
        }
    }

    pub fn mark_dirty(mut self) {
        self.db_dirty.set(true);
    }

    pub fn selected_track(&self) -> Option<TrackInfo> {
        let filtered = self.filtered.peek();
        let idx = *filtered.get(*self.selected.peek())?;
        self.db.peek().tracks.get(idx).cloned()
    }

    pub fn update_track(mut self, path: &str, f: impl FnOnce(&mut TrackInfo)) {
        let mut db = self.db.write();
        if let Some(t) = db.tracks.iter_mut().find(|t| t.path == path) {
            f(t);
        }
        drop(db);
        self.mark_dirty();
    }

    pub fn set_zoom(mut self, z: f64) {
        let z = z.clamp(0.25, 8.0);
        self.zoom.set(z);
        let _ = self.bridge.send(json!({"k": "zoom", "z": z}));
    }

    pub fn toggle_quantize(mut self) {
        let q = !*self.quantize.peek();
        self.quantize.set(q);
        engine().send(Cmd::Quantize(q));
        self.toast(if q { "Quantize on" } else { "Quantize off" });
    }

    pub fn load_selected(self, deck: usize) {
        match self.selected_track() {
            Some(t) => self.load_track(deck, t),
            None => self.toast("No track selected"),
        }
    }

    pub fn load_to_idle_deck(self) {
        let snap = self.snap.peek().clone();
        let decks = self.decks.peek().clone();
        let deck = match (snap.decks[0].playing, snap.decks[1].playing) {
            (true, true) => {
                self.toast("Both decks are playing");
                return;
            }
            (false, true) => 0,
            (true, false) => 1,
            (false, false) => {
                if decks[0].track.is_some() && decks[1].track.is_none() {
                    1
                } else {
                    0
                }
            }
        };
        self.load_selected(deck);
    }

    pub fn load_track(mut self, deck: usize, track: TrackInfo) {
        if self.snap.peek().decks[deck].playing {
            self.toast(format!("Deck {} is playing — pause it before loading", DECK_NAMES[deck]));
            return;
        }
        let id = *self.next_load_id.peek() + 1;
        self.next_load_id.set(id);
        self.decks.write()[deck] = DeckInfo { track: Some(track.clone()), load_id: id, loading: true };
        let _ = self.bridge.send(json!({"k": "wave", "deck": deck, "id": id, "data": ""}));

        spawn(async move {
            let t = track.clone();
            let res = tokio::task::spawn_blocking(move || prepare_track(&t)).await;
            // A newer load may have replaced this one while decoding.
            if self.decks.peek()[deck].load_id != id {
                return;
            }
            match res {
                Ok(Ok(prep)) => {
                    let duration = prep.audio.duration();
                    engine().send(Cmd::Load(
                        deck,
                        Box::new(LoadRequest {
                            audio: prep.audio,
                            load_id: id,
                            bpm: prep.bpm,
                            grid_offset: prep.grid_offset,
                            auto_gain: prep.gain,
                            cue: track.cue.unwrap_or(0.0),
                            hot_cues: track.hot_cue_array(),
                        }),
                    ));
                    let _ = self.bridge.send(json!({
                        "k": "wave", "deck": deck, "id": id, "rate": prep.wave_rate, "data": prep.wave_b64
                    }));
                    self.update_track(&track.path, |t| {
                        t.bpm = prep.bpm;
                        t.grid_offset = Some(prep.grid_offset);
                        t.gain = Some(prep.gain);
                        t.duration = Some(duration);
                    });
                    let mut decks = self.decks.write();
                    decks[deck].loading = false;
                    if let Some(t) = decks[deck].track.as_mut() {
                        t.bpm = prep.bpm;
                        t.grid_offset = Some(prep.grid_offset);
                        t.duration = Some(duration);
                    }
                }
                Ok(Err(e)) => {
                    self.decks.write()[deck] = DeckInfo::default();
                    engine().send(Cmd::Eject(deck));
                    self.toast(format!("Couldn't load: {e}"));
                }
                Err(e) => {
                    self.decks.write()[deck] = DeckInfo::default();
                    self.toast(format!("Loader crashed: {e}"));
                }
            }
        });
    }

    pub fn toggle_recording(mut self) {
        let current = self.recording.peek().clone();
        match current {
            Some(rec) => {
                recorder::stop();
                recorder::write_tracklist(&rec);
                self.recording.set(None);
                self.toast(format!("Saved {}", rec.wav_path.display()));
            }
            None => {
                let dir = self
                    .db
                    .peek()
                    .record_dir
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(library::default_record_dir);
                match recorder::start(&dir) {
                    Ok(rec) => {
                        self.toast(format!("Recording to {}", rec.wav_path.display()));
                        self.recording.set(Some(rec));
                    }
                    Err(e) => self.toast(e),
                }
            }
        }
    }

    pub fn rescan(mut self) {
        if *self.scanning.peek() {
            return;
        }
        self.scanning.set(true);
        let (roots, existing) = {
            let db = self.db.peek();
            (db.roots.clone(), db.tracks.clone())
        };
        spawn(async move {
            let res = tokio::task::spawn_blocking(move || library::scan(&roots, &existing)).await;
            self.scanning.set(false);
            if let Ok(tracks) = res {
                let n = tracks.len();
                // Keep DJ data written while the scan was running.
                let mut db = self.db.write();
                let latest: HashMap<String, TrackInfo> = db.tracks.drain(..).map(|t| (t.path.clone(), t)).collect();
                db.tracks = tracks
                    .into_iter()
                    .map(|mut t| {
                        if let Some(prev) = latest.get(&t.path) {
                            t.bpm = prev.bpm.or(t.bpm);
                            t.grid_offset = prev.grid_offset;
                            t.gain = prev.gain;
                            t.cue = prev.cue;
                            t.hot_cues = prev.hot_cues.clone();
                            t.play_count = prev.play_count;
                        }
                        t
                    })
                    .collect();
                drop(db);
                self.mark_dirty();
                self.toast(format!("Library: {n} tracks"));
            }
        });
    }
}

struct Prepared {
    audio: Arc<TrackAudio>,
    bpm: Option<f64>,
    grid_offset: f64,
    gain: f32,
    wave_b64: String,
    wave_rate: f64,
}

fn prepare_track(t: &TrackInfo) -> Result<Prepared, String> {
    let audio = decode::decode_file(std::path::Path::new(&t.path))?;
    let gain = t.gain.unwrap_or_else(|| analysis::auto_gain(&audio));
    let (bpm, grid_offset) = match (t.bpm, t.grid_offset) {
        (Some(b), Some(o)) => (Some(b), o),
        _ => match analysis::detect_beatgrid(&audio) {
            Some(g) => (Some(g.bpm), g.offset),
            None => (t.bpm, 0.0),
        },
    };
    let wave = analysis::waveform(&audio, gain);
    Ok(Prepared {
        audio: Arc::new(audio),
        bpm,
        grid_offset,
        gain,
        wave_b64: base64::engine::general_purpose::STANDARD.encode(&wave.data),
        wave_rate: wave.rate,
    })
}

#[derive(Deserialize, Debug)]
struct InputMsg {
    k: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    shift: bool,
    #[serde(default)]
    alt: bool,
    #[serde(default)]
    ctrl: bool,
    #[serde(default)]
    rep: bool,
    #[serde(default)]
    deck: usize,
    #[serde(default)]
    frac: f64,
    #[serde(default)]
    factor: f64,
}

type Held = Rc<RefCell<HashMap<String, Act>>>;

pub fn do_action(app: AppState, act: Act) {
    let e = engine();
    let snap = app.snap.peek().clone();
    match act {
        Act::Cue(d) => e.send(Cmd::CueDown(d)),
        Act::Nudge(d, amt) => e.send(Cmd::Bend(d, amt)),
        Act::PlayPause(d) => e.send(Cmd::PlayPause(d)),
        Act::HotCue(d, h) => e.send(Cmd::HotCue(d, h)),
        Act::HotCueClear(d, h) => e.send(Cmd::HotCueClear(d, h)),
        Act::Sync(d) => {
            if snap.decks[d].bpm <= 0.0 || snap.decks[1 - d].bpm <= 0.0 {
                app.toast("Sync needs a BPM on both decks");
            }
            e.send(Cmd::SyncToggle(d))
        }
        Act::PitchReset(d) => {
            e.send(Cmd::SyncOff(d));
            e.send(Cmd::SetPitch(d, 0.0));
        }
        Act::PitchStep(d, s) => e.send(Cmd::PitchStep(d, s)),
        Act::BeatJump(d, dir) => e.send(Cmd::BeatJump(d, dir * snap.decks[d].loop_beats)),
        Act::LoopToggle(d) => e.send(Cmd::LoopToggle(d)),
        Act::Reloop(d) => e.send(Cmd::Reloop(d)),
        Act::LoopHalve(d) => e.send(Cmd::LoopHalve(d)),
        Act::LoopDouble(d) => e.send(Cmd::LoopDouble(d)),
        Act::LoopIn(d) => e.send(Cmd::LoopIn(d)),
        Act::LoopOut(d) => e.send(Cmd::LoopOut(d)),
        Act::JumpStart(d) => e.send(Cmd::JumpToStart(d)),
        Act::Load(d) => app.load_selected(d),
        Act::Kill(d, b) => app.set_mixer(|m| m.ch[d].kill[b] = !m.ch[d].kill[b]),
        Act::FilterReset(d) => app.set_mixer(|m| m.ch[d].filter = 0.0),
        Act::EqReset(d) => app.set_mixer(|m| {
            m.ch[d].eq = [0.5; 3];
            m.ch[d].kill = [false; 3];
        }),
        Act::XfaderSet(x) => app.set_mixer(|m| m.xfader = x),
        Act::LibMove(n) => {
            let len = app.filtered.peek().len();
            if len > 0 {
                let mut sel = app.selected;
                let cur = *sel.peek() as i64;
                sel.set((cur + n as i64).clamp(0, len as i64 - 1) as usize);
                let _ = app.bridge.send(json!({"k": "scroll"}));
            }
        }
        Act::LoadToIdleDeck => app.load_to_idle_deck(),
        Act::ToggleRecord => app.toggle_recording(),
        Act::ToggleHelp => {
            let mut h = app.help;
            let v = !*h.peek();
            h.set(v);
        }
        Act::Escape => {
            let mut h = app.help;
            h.set(false);
        }
        Act::Zoom(f) => app.set_zoom(*app.zoom.peek() * f),
        Act::ToggleQuantize => app.toggle_quantize(),
        Act::Volume(..) | Act::Eq(..) | Act::Filter(..) | Act::Xfader(..) | Act::Master(..) => {}
    }
}

fn release_action(act: Act) {
    match act {
        Act::Cue(d) => engine().send(Cmd::CueUp(d)),
        Act::Nudge(d, _) => engine().send(Cmd::Bend(d, 0.0)),
        _ => {}
    }
}

fn handle_input(app: AppState, held: &Held, msg: InputMsg) {
    match msg.k.as_str() {
        "down" => {
            let Some(act) = keys::map(&msg.code, msg.shift, msg.alt, msg.ctrl) else { return };
            if msg.rep {
                if act.repeats() {
                    do_action(app, act);
                }
                return;
            }
            if act.is_continuous() {
                held.borrow_mut().insert(msg.code, act);
            } else if act.is_momentary() {
                held.borrow_mut().insert(msg.code, act);
                do_action(app, act);
            } else {
                do_action(app, act);
            }
        }
        "up" => {
            if let Some(act) = held.borrow_mut().remove(&msg.code) {
                release_action(act);
            }
        }
        "blur" => {
            for (_, act) in held.borrow_mut().drain() {
                release_action(act);
            }
        }
        "seek" => engine().send(Cmd::SeekFrac(msg.deck.min(NUM_DECKS - 1), msg.frac)),
        "zoom" => app.set_zoom(*app.zoom.peek() * msg.factor),
        "search_exit" => {
            if msg.code == "Enter" {
                // Enter in search jumps to the first result.
                let mut sel = app.selected;
                sel.set(0);
            }
        }
        _ => {}
    }
}

/// Applies held continuous controls. Knobs stop at their centre detent so a
/// sweep back to neutral is easy to hit; press again to continue past it.
fn apply_held(app: AppState, held: &Held, dt: f32) {
    let acts: Vec<(String, Act)> = held.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect();
    if acts.is_empty() {
        return;
    }
    let mut stop = Vec::new();
    app.set_mixer(|m| {
        for (code, act) in &acts {
            match *act {
                Act::Volume(d, r) => m.ch[d].volume = (m.ch[d].volume + r * dt).clamp(0.0, 1.0),
                Act::Master(r) => m.master = (m.master + r * dt).clamp(0.0, 1.0),
                Act::Xfader(r) => m.xfader = (m.xfader + r * dt).clamp(-1.0, 1.0),
                Act::Eq(d, b, r) => {
                    let old = m.ch[d].eq[b];
                    let mut new = (old + r * dt).clamp(0.0, 1.0);
                    if (old - 0.5) * (new - 0.5) < 0.0 {
                        new = 0.5;
                        stop.push(code.clone());
                    }
                    m.ch[d].eq[b] = new;
                }
                Act::Filter(d, r) => {
                    let old = m.ch[d].filter;
                    let mut new = (old + r * dt).clamp(-1.0, 1.0);
                    if old * new < 0.0 {
                        new = 0.0;
                        stop.push(code.clone());
                    }
                    m.ch[d].filter = new;
                }
                _ => {}
            }
        }
    });
    for code in stop {
        held.borrow_mut().remove(&code);
    }
}

#[component]
pub fn App() -> Element {
    let initial_db = use_hook(library::load_db);
    let db = use_signal(|| initial_db.clone());
    let mixer = use_signal(|| {
        let mut m = MixerState::default();
        if let Some(saved) = initial_db.mixer {
            m.curve = saved.curve;
            m.master = saved.master;
        }
        engine().send(Cmd::Mixer(m));
        m
    });
    let query = use_signal(String::new);
    let sort = use_signal(|| (SortKey::Artist, true));
    let filtered = use_memo(move || filter_tracks(&db.read().tracks, &query.read(), *sort.read()));
    let bridge = use_hook(|| document::eval(BRIDGE_JS));

    let app = use_context_provider(|| AppState {
        snap: Signal::new(engine().snapshot()),
        mixer,
        decks: Signal::new(Default::default()),
        db,
        query,
        selected: Signal::new(0),
        sort,
        filtered,
        help: Signal::new(false),
        toast: Signal::new(None),
        recording: Signal::new(None),
        scanning: Signal::new(false),
        zoom: Signal::new(1.0),
        quantize: Signal::new(true),
        bridge,
        db_dirty: Signal::new(false),
        next_load_id: Signal::new(0),
    });

    let held: Held = use_hook(|| Rc::new(RefCell::new(HashMap::new())));

    // Initial library scan.
    use_hook(move || app.rescan());

    // Keyboard and mouse input from the bridge.
    let held_in = held.clone();
    use_future(move || {
        let held = held_in.clone();
        async move {
            let mut bridge = app.bridge;
            loop {
                match bridge.recv::<InputMsg>().await {
                    Ok(msg) => handle_input(app, &held, msg),
                    Err(e) => {
                        eprintln!("input bridge closed: {e:?}");
                        break;
                    }
                }
            }
        }
    });

    // Main UI tick: held controls, waveform state, snapshot, persistence, tracklist.
    let held_tick = held.clone();
    use_future(move || {
        let held = held_tick.clone();
        async move {
            let mut last = Instant::now();
            let mut frame: u64 = 0;
            let mut meters = [0.0f32; NUM_DECKS + 2];
            let mut logged = [0u64; NUM_DECKS];
            let mut counted = [0u64; NUM_DECKS];
            let mut last_save = Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(16)).await;
                let now = Instant::now();
                let dt = (now - last).as_secs_f32().min(0.1);
                last = now;
                frame += 1;

                apply_held(app, &held, dt);
                let mut snap = engine().snapshot();
                let _ = app.bridge.send(waveform_state(&snap));

                // Meters with decay.
                for d in 0..NUM_DECKS {
                    meters[d] = snap.decks[d].level.max(meters[d] * 0.88);
                    snap.decks[d].level = meters[d];
                }
                for c in 0..2 {
                    meters[NUM_DECKS + c] = snap.master_level[c].max(meters[NUM_DECKS + c] * 0.88);
                    snap.master_level[c] = meters[NUM_DECKS + c];
                }

                track_side_effects(app, &snap, &mut logged, &mut counted);

                if frame.is_multiple_of(2) {
                    let mut s = app.snap;
                    s.set(snap);
                }

                if *app.db_dirty.peek() && last_save.elapsed() > Duration::from_secs(2) {
                    let mut dirty = app.db_dirty;
                    dirty.set(false);
                    last_save = Instant::now();
                    let mut db = app.db.peek().clone();
                    db.mixer = Some(*app.mixer.peek());
                    std::thread::spawn(move || library::save_db(&db));
                }
            }
        }
    });

    rsx! {
        style { {CSS} }
        div { class: "app",
            TopBar {}
            div { class: "waves",
                canvas { id: "wf-0", class: "scroller deck-a" }
                canvas { id: "wf-1", class: "scroller deck-b" }
            }
            div { class: "decks",
                deck::DeckPanel { deck: 0 }
                mixer::Mixer {}
                deck::DeckPanel { deck: 1 }
            }
            browser::Library {}
            if (app.help)() {
                help::Help {}
            }
            Toast {}
        }
    }
}

/// Persist cue/grid edits, log the recording tracklist and count plays.
fn track_side_effects(app: AppState, snap: &EngineSnapshot, logged: &mut [u64; NUM_DECKS], counted: &mut [u64; NUM_DECKS]) {
    let decks = app.decks.peek().clone();
    let mixer = *app.mixer.peek();
    let (xl, xr) = mixer.curve.gains(mixer.xfader);
    let xf = [xl, xr];
    for d in 0..NUM_DECKS {
        let s = &snap.decks[d];
        let info = &decks[d];
        let Some(track) = info.track.as_ref() else { continue };
        if info.loading || s.load_id != info.load_id || !s.loaded {
            continue;
        }

        // Persist cue / hot cue / grid edits made on the deck.
        let stored_hot = track.hot_cue_array();
        let changed_hot = stored_hot.iter().zip(s.hot_cues.iter()).any(|(a, b)| match (a, b) {
            (Some(x), Some(y)) => (x - y).abs() > 1e-4,
            (None, None) => false,
            _ => true,
        });
        let changed_cue = (track.cue.unwrap_or(0.0) - s.cue).abs() > 1e-4;
        let bpm = (s.bpm > 0.0).then_some(s.bpm);
        let changed_grid = bpm != track.bpm || (track.grid_offset.unwrap_or(0.0) - s.grid_offset).abs() > 1e-4;
        if changed_hot || changed_cue || changed_grid {
            let path = track.path.clone();
            let (hot, cue, off) = (s.hot_cues.to_vec(), s.cue, s.grid_offset);
            let apply = |t: &mut TrackInfo| {
                t.hot_cues = hot.clone();
                t.cue = Some(cue);
                t.bpm = bpm;
                t.grid_offset = Some(off);
            };
            app.update_track(&path, apply);
            let mut decks_sig = app.decks;
            let mut decks_w = decks_sig.write();
            if let Some(t) = decks_w[d].track.as_mut() {
                apply(t);
            }
        }

        let audible = s.playing && mixer.ch[d].volume > 0.05 && xf[d] > 0.05;
        if audible && counted[d] != info.load_id {
            counted[d] = info.load_id;
            app.update_track(&track.path, |t| t.play_count += 1);
        }
        if audible && logged[d] != info.load_id {
            let mut rec_sig = app.recording;
            let entry_added = {
                let mut rec = rec_sig.write();
                match rec.as_mut() {
                    Some(r) if snap.recording => {
                        r.tracklist.push(TracklistEntry {
                            at_secs: snap.recorded_frames as f64 / snap.sample_rate.max(1) as f64,
                            artist: track.artist.clone(),
                            title: track.title.clone(),
                        });
                        Some(r.clone())
                    }
                    _ => None,
                }
            };
            if let Some(r) = entry_added {
                logged[d] = info.load_id;
                std::thread::spawn(move || recorder::write_tracklist(&r));
            }
        }
    }
    // A new recording should re-log whatever is currently playing.
    if !snap.recording {
        *logged = [0; NUM_DECKS];
    }
}

fn waveform_state(snap: &EngineSnapshot) -> serde_json::Value {
    let decks: Vec<_> = snap
        .decks
        .iter()
        .map(|d| {
            json!({
                "l": d.loaded,
                "pos": d.pos,
                "dur": d.duration,
                "play": d.playing,
                "sp": d.speed,
                "bpm": d.bpm,
                "off": d.grid_offset,
                "cue": d.cue,
                "hot": d.hot_cues,
                "ls": d.loop_start,
                "le": d.loop_end,
                "la": d.loop_active,
            })
        })
        .collect();
    json!({"k": "state", "d": decks})
}

fn filter_tracks(tracks: &[TrackInfo], query: &str, sort: (SortKey, bool)) -> Vec<usize> {
    let terms: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    let mut idx: Vec<usize> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            if terms.is_empty() {
                return true;
            }
            let hay = format!("{} {} {} {}", t.title, t.artist, t.album, t.genre).to_lowercase();
            terms.iter().all(|term| hay.contains(term.as_str()))
        })
        .map(|(i, _)| i)
        .collect();
    let (key, asc) = sort;
    let lc = |s: &str| s.to_lowercase();
    idx.sort_by(|&a, &b| {
        let (ta, tb) = (&tracks[a], &tracks[b]);
        let ord = match key {
            SortKey::Artist => lc(&ta.artist).cmp(&lc(&tb.artist)).then_with(|| lc(&ta.title).cmp(&lc(&tb.title))),
            SortKey::Title => lc(&ta.title).cmp(&lc(&tb.title)),
            SortKey::Bpm => ta.bpm.unwrap_or(0.0).total_cmp(&tb.bpm.unwrap_or(0.0)),
            SortKey::Duration => ta.duration.unwrap_or(0.0).total_cmp(&tb.duration.unwrap_or(0.0)),
            SortKey::Album => lc(&ta.album).cmp(&lc(&tb.album)),
            SortKey::Plays => ta.play_count.cmp(&tb.play_count),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
    idx
}

pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0);
    let m = (s / 60.0).floor() as u64;
    let rest = s - m as f64 * 60.0;
    format!("{m}:{rest:04.1}")
}

#[component]
fn TopBar() -> Element {
    let app = app_state();
    let snap = app.snap.read();
    let rec = app.recording.read();
    let rec_secs = snap.recorded_frames as f64 / snap.sample_rate.max(1) as f64;
    let quantize = (app.quantize)();
    let ml = snap.master_level;
    let clip = ml[0] > 0.98 || ml[1] > 0.98;
    let eng = engine();
    rsx! {
        div { class: "topbar",
            div { class: "brand", "DJ" }
            div { class: "device", "{eng.device_name} · {eng.sample_rate} Hz" }
            div { class: "spacer" }
            button {
                class: if quantize { "toggle on" } else { "toggle" },
                title: "Snap cues and loops to the beat grid (Ctrl+Q)",
                onclick: move |_| app.toggle_quantize(),
                "Quantize"
                span { class: "kbd", "⌃Q" }
            }
            div { class: "master-meter", title: "Master level",
                div { class: if clip { "bar clip" } else { "bar" }, style: "width: {ml[0].min(1.0) * 100.0}%" }
                div { class: if clip { "bar clip" } else { "bar" }, style: "width: {ml[1].min(1.0) * 100.0}%" }
            }
            button {
                class: if rec.is_some() { "rec on" } else { "rec" },
                title: "Record the master output (Ctrl+R)",
                onclick: move |_| app.toggle_recording(),
                span { class: "dot" }
                if rec.is_some() {
                    "REC {recorder::fmt_hms(rec_secs)}"
                } else {
                    "Record"
                }
                span { class: "kbd", "⌃R" }
            }
            if let Some(r) = rec.as_ref() {
                div { class: "rec-info",
                    "{r.tracklist.len()} tracks logged"
                    if snap.record_dropouts > 0 {
                        span { class: "warn", " · {snap.record_dropouts} dropouts" }
                    }
                }
            }
            button {
                class: "toggle",
                onclick: move |_| {
                    let mut h = app.help;
                    let v = !*h.peek();
                    h.set(v);
                },
                "Keys"
                span { class: "kbd", "`" }
            }
        }
    }
}

#[component]
fn Toast() -> Element {
    let app = app_state();
    let toast = app.toast.read();
    rsx! {
        if let Some((_, msg)) = toast.as_ref() {
            div { class: "toast", "{msg}" }
        }
    }
}
