//! Music library: folder scanning, tag cache and per-track DJ data (cues, BPM).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audio::{MixerState, NUM_HOT_CUES};
use crate::decode;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub path: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub duration: Option<f64>,
    /// Analysed (or user corrected) BPM.
    #[serde(default)]
    pub bpm: Option<f64>,
    #[serde(default)]
    pub grid_offset: Option<f64>,
    #[serde(default)]
    pub gain: Option<f32>,
    #[serde(default)]
    pub cue: Option<f64>,
    #[serde(default)]
    pub hot_cues: Vec<Option<f64>>,
    #[serde(default)]
    pub play_count: u32,
    /// File modification time when tags were read.
    #[serde(default)]
    pub mtime: u64,
}

impl TrackInfo {
    pub fn hot_cue_array(&self) -> [Option<f64>; NUM_HOT_CUES] {
        std::array::from_fn(|i| self.hot_cues.get(i).copied().flatten())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LibraryDb {
    pub roots: Vec<String>,
    pub tracks: Vec<TrackInfo>,
    #[serde(default)]
    pub mixer: Option<MixerState>,
    #[serde(default)]
    pub record_dir: Option<String>,
}

pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("dj-rs")
}

fn db_path() -> PathBuf {
    data_dir().join("library.json")
}

pub fn default_record_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("DJ Sets")
}

pub fn load_db() -> LibraryDb {
    let mut db: LibraryDb = std::fs::read(db_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if db.roots.is_empty() {
        if let Some(music) = dirs::audio_dir() {
            db.roots.push(music.to_string_lossy().into_owned());
        }
    }
    db
}

/// Writes atomically (temp file + rename) so a crash never corrupts the library.
pub fn save_db(db: &LibraryDb) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join("library.json.tmp");
    if let Ok(bytes) = serde_json::to_vec(db) {
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, db_path());
        }
    }
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "Artist - Title.mp3" style fallback when tags are missing.
fn names_from_filename(path: &Path) -> (String, String) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unknown").to_string();
    match stem.split_once(" - ") {
        Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
        None => (String::new(), stem),
    }
}

/// Walks all roots and returns the merged track list. Existing entries keep their DJ data.
pub fn scan(roots: &[String], existing: &[TrackInfo]) -> Vec<TrackInfo> {
    let mut known: HashMap<&str, &TrackInfo> = existing.iter().map(|t| (t.path.as_str(), t)).collect();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        for entry in walkdir::WalkDir::new(root).follow_links(true).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if !entry.file_type().is_file() || !decode::is_audio_file(path) {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with("._")).unwrap_or(false) {
                continue;
            }
            let key = path.to_string_lossy().into_owned();
            if !seen.insert(key.clone()) {
                continue;
            }
            let m = mtime(path);
            if let Some(prev) = known.remove(key.as_str()) {
                if prev.mtime == m {
                    out.push(prev.clone());
                    continue;
                }
            }
            let tags = decode::read_tags(path);
            let (fa, ft) = names_from_filename(path);
            let prev = existing.iter().find(|t| t.path == key);
            out.push(TrackInfo {
                path: key,
                title: tags.title.unwrap_or(ft),
                artist: tags.artist.unwrap_or(fa),
                album: tags.album.unwrap_or_default(),
                genre: tags.genre.unwrap_or_default(),
                duration: tags.duration,
                bpm: prev.and_then(|p| p.bpm).or(tags.bpm),
                grid_offset: prev.and_then(|p| p.grid_offset),
                gain: prev.and_then(|p| p.gain),
                cue: prev.and_then(|p| p.cue),
                hot_cues: prev.map(|p| p.hot_cues.clone()).unwrap_or_default(),
                play_count: prev.map(|p| p.play_count).unwrap_or(0),
                mtime: m,
            });
        }
    }
    out
}
