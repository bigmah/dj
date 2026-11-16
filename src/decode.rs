//! Audio file decoding and tag reading via symphonia.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey, Tag};
use symphonia::core::probe::{Hint, ProbeResult};

use crate::audio::TrackAudio;

pub const EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "mp4", "ogg", "oga", "aif", "aiff", "caf", "mka", "webm"];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn probe(path: &Path) -> Result<ProbeResult, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
        .map_err(|e| format!("unsupported file {}: {e}", path.display()))
}

#[derive(Debug, Default, Clone)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub bpm: Option<f64>,
    pub duration: Option<f64>,
}

fn apply_tags(tags: &[Tag], out: &mut Tags) {
    for tag in tags {
        let val = tag.value.to_string();
        let val = val.trim().to_string();
        if val.is_empty() {
            continue;
        }
        match tag.std_key {
            Some(StandardTagKey::TrackTitle) => out.title = Some(val),
            Some(StandardTagKey::Artist) => out.artist = Some(val),
            Some(StandardTagKey::AlbumArtist) if out.artist.is_none() => out.artist = Some(val),
            Some(StandardTagKey::Album) => out.album = Some(val),
            Some(StandardTagKey::Genre) => out.genre = Some(val),
            Some(StandardTagKey::Bpm) => out.bpm = val.parse::<f64>().ok().filter(|b| *b > 30.0 && *b < 300.0),
            _ => {}
        }
    }
}

/// Reads tags and duration without decoding audio.
pub fn read_tags(path: &Path) -> Tags {
    let mut tags = Tags::default();
    let Ok(mut probed) = probe(path) else { return tags };
    if let Some(rev) = probed.metadata.get().as_ref().and_then(|m| m.current().cloned()) {
        apply_tags(rev.tags(), &mut tags);
    }
    if let Some(rev) = probed.format.metadata().current() {
        apply_tags(rev.tags(), &mut tags);
    }
    if let Some(track) = probed.format.default_track() {
        let p = &track.codec_params;
        if let (Some(n), Some(sr)) = (p.n_frames, p.sample_rate) {
            tags.duration = Some(n as f64 / sr as f64);
        }
    }
    tags
}

/// Decodes the whole file to interleaved stereo f32 at the file's native rate.
pub fn decode_file(path: &Path) -> Result<TrackAudio, String> {
    let probed = probe(path)?;
    let mut format: Box<dyn FormatReader> = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no audio track in file")?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("no decoder: {e}"))?;

    let mut samples: Vec<f32> = Vec::new();
    if let Some(n) = track.codec_params.n_frames {
        samples.reserve(n as usize * 2);
    }
    let mut buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if samples.is_empty() {
                    return Err(format!("read error: {e}"));
                }
                break;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
            Err(e) => return Err(format!("decode error: {e}")),
        };
        let spec = *decoded.spec();
        sample_rate = spec.rate;
        let channels = spec.channels.count();
        let needed = decoded.capacity() as u64;
        if buf.as_ref().map(|b| b.capacity() < needed as usize).unwrap_or(true) {
            buf = Some(SampleBuffer::new(needed, spec));
        }
        let b = buf.as_mut().unwrap();
        b.copy_interleaved_ref(decoded);
        let s = b.samples();
        match channels {
            0 => {}
            1 => {
                for &v in s {
                    samples.push(v);
                    samples.push(v);
                }
            }
            2 => samples.extend_from_slice(s),
            n => {
                for frame in s.chunks_exact(n) {
                    samples.push(frame[0]);
                    samples.push(frame[1]);
                }
            }
        }
    }

    if samples.is_empty() {
        return Err("file contains no audio".into());
    }
    samples.shrink_to_fit();
    let frames = samples.len() / 2;
    Ok(TrackAudio { samples, sample_rate, frames })
}
