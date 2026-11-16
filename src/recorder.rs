//! Records the master output to a 24-bit WAV and writes a tracklist + CUE sheet.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::audio::{engine, Cmd, RecordSink};

#[derive(Clone, Debug, PartialEq)]
pub struct TracklistEntry {
    pub at_secs: f64,
    pub artist: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub wav_path: PathBuf,
    pub started: chrono::DateTime<chrono::Local>,
    pub tracklist: Vec<TracklistEntry>,
}

pub fn start(dir: &Path) -> Result<Recording, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let started = chrono::Local::now();
    let wav_path = dir.join(format!("Set {}.wav", started.format("%Y-%m-%d %H.%M.%S")));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: engine().sample_rate,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let writer = hound::WavWriter::create(&wav_path, spec).map_err(|e| format!("can't create recording: {e}"))?;

    // ~45 seconds of buffering at 4096-frame chunks before anything is dropped.
    let (tx, rx) = bounded::<Vec<f32>>(512);
    let (pool_tx, pool_rx) = bounded::<Vec<f32>>(64);
    for _ in 0..32 {
        let _ = pool_tx.try_send(Vec::with_capacity(8192));
    }
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || writer_thread(writer, rx, pool_tx))
        .map_err(|e| e.to_string())?;
    engine().send(Cmd::StartRecording(RecordSink { tx, pool: pool_rx }));
    Ok(Recording { wav_path, started, tracklist: Vec::new() })
}

fn writer_thread(mut writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>, rx: Receiver<Vec<f32>>, pool: Sender<Vec<f32>>) {
    const MAX: f32 = 8_388_607.0;
    let mut last_flush = Instant::now();
    while let Ok(mut buf) = rx.recv() {
        for &s in &buf {
            let v = (s.clamp(-1.0, 1.0) * MAX).round() as i32;
            if writer.write_sample(v).is_err() {
                eprintln!("recording write failed");
                return;
            }
        }
        buf.clear();
        let _ = pool.try_send(buf);
        // Keep the header valid so a crash still leaves a playable file.
        if last_flush.elapsed() > Duration::from_secs(1) {
            let _ = writer.flush();
            last_flush = Instant::now();
        }
    }
    // Sender dropped by the engine: recording stopped.
    if let Err(e) = writer.finalize() {
        eprintln!("failed to finalise recording: {e}");
    }
}

pub fn stop() {
    engine().send(Cmd::StopRecording);
}

pub fn fmt_hms(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Writes `<set>.txt` and `<set>.cue` next to the WAV.
pub fn write_tracklist(rec: &Recording) {
    let txt = rec.wav_path.with_extension("txt");
    let cue = rec.wav_path.with_extension("cue");
    if let Ok(mut f) = std::fs::File::create(&txt) {
        let _ = writeln!(f, "DJ set recorded {}", rec.started.format("%Y-%m-%d %H:%M"));
        let _ = writeln!(f);
        for (i, e) in rec.tracklist.iter().enumerate() {
            let name = if e.artist.is_empty() { e.title.clone() } else { format!("{} - {}", e.artist, e.title) };
            let _ = writeln!(f, "{:>2}. [{}] {}", i + 1, fmt_hms(e.at_secs), name);
        }
    }
    if let Ok(mut f) = std::fs::File::create(&cue) {
        let file_name = rec.wav_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let esc = |s: &str| s.replace('"', "'");
        let _ = writeln!(f, "TITLE \"DJ Set {}\"", rec.started.format("%Y-%m-%d"));
        let _ = writeln!(f, "FILE \"{}\" WAVE", esc(&file_name));
        for (i, e) in rec.tracklist.iter().enumerate() {
            // CUE spec: the first track must start at 00:00:00.
            let total_frames = if i == 0 { 0 } else { (e.at_secs * 75.0) as u64 };
            let (m, s, fr) = (total_frames / 75 / 60, (total_frames / 75) % 60, total_frames % 75);
            let _ = writeln!(f, "  TRACK {:02} AUDIO", i + 1);
            let _ = writeln!(f, "    TITLE \"{}\"", esc(&e.title));
            if !e.artist.is_empty() {
                let _ = writeln!(f, "    PERFORMER \"{}\"", esc(&e.artist));
            }
            let _ = writeln!(f, "    INDEX 01 {:02}:{:02}:{:02}", m, s, fr);
        }
    }
}
