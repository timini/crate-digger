use std::fs::File;
use std::path::{Path, PathBuf};

use serde::Serialize;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

/// Formats advertised as supported, by file extension. Each one has a decode
/// test over a synthetic fixture in `tests/decode.rs`.
pub const SUPPORTED_FORMATS: &[(&str, &str)] = &[
    ("wav", "WAV (PCM)"),
    ("aif", "AIFF (PCM)"),
    ("aiff", "AIFF (PCM)"),
    ("flac", "FLAC"),
    ("mp3", "MP3"),
    ("m4a", "AAC-LC or ALAC in MP4"),
    ("mp4", "AAC-LC or ALAC in MP4"),
    ("ogg", "Ogg Vorbis"),
];

pub fn is_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            SUPPORTED_FORMATS.iter().any(|(ext, _)| *ext == e)
        })
        .unwrap_or(false)
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("file not found: {0}")]
    NotFound(PathBuf),
    #[error("cannot read {path}: {message}")]
    Unreadable { path: PathBuf, message: String },
    #[error("unsupported audio format: {0}")]
    Unsupported(String),
    #[error("audio data is damaged: {0}")]
    Corrupt(String),
}

impl AudioError {
    /// A message that tells the user what to do.
    pub fn user_message(&self) -> String {
        match self {
            AudioError::NotFound(p) => format!(
                "The file is missing from {}. It may have been moved, renamed or deleted, or its drive may be \
                 disconnected. Use Relink to find it.",
                p.display()
            ),
            AudioError::Unreadable { message, .. } => {
                format!("The file could not be opened ({message}). Check that its drive is connected and readable.")
            }
            AudioError::Unsupported(m) => {
                format!("This audio format is not supported ({m}). Convert it to FLAC, WAV, AIFF or MP3.")
            }
            AudioError::Corrupt(m) => format!(
                "The audio could not be decoded ({m}). The file may be damaged or incomplete; replace it with a \
                 good copy."
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AudioInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// From container headers; may be absent (for example some VBR MP3s).
    pub duration_ms: Option<u64>,
}

fn open_reader(path: &Path) -> Result<Box<dyn FormatReader>, AudioError> {
    let file = File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AudioError::NotFound(path.to_path_buf()),
        _ => AudioError::Unreadable {
            path: path.to_path_buf(),
            message: e.to_string(),
        },
    })?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| match e {
            SymError::Unsupported(m) => AudioError::Unsupported(m.to_string()),
            other => AudioError::Corrupt(other.to_string()),
        })
}

/// Streams decoded audio as interleaved `f32` frames.
pub struct Decoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    pub info: AudioInfo,
    /// Frames decoded since the start or the last seek.
    position_frames: u64,
    /// Count of packets that failed to decode and were skipped.
    pub decode_errors: u64,
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Self, AudioError> {
        let reader = open_reader(path)?;
        let track = reader
            .default_track(TrackType::Audio)
            .ok_or_else(|| AudioError::Unsupported("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| AudioError::Unsupported("no audio codec parameters".into()))?
            .clone();
        let track_id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| AudioError::Unsupported(e.to_string()))?;
        let sample_rate = params
            .sample_rate
            .ok_or_else(|| AudioError::Corrupt("missing sample rate".into()))?;
        let channels = params.channels.as_ref().map(|c| c.count() as u16).unwrap_or(0);
        let duration_ms = match (track.num_frames, track.time_base, track.duration) {
            (Some(frames), _, _) if sample_rate > 0 => Some(frames * 1000 / sample_rate as u64),
            (_, Some(tb), Some(d)) => tb.calc_duration(d).map(|t| t.as_millis().max(0) as u64),
            _ => None,
        };
        let codec = decoder.codec_info().short_name.to_string();
        Ok(Decoder {
            reader,
            decoder,
            track_id,
            info: AudioInfo {
                codec,
                sample_rate,
                channels,
                duration_ms,
            },
            position_frames: 0,
            decode_errors: 0,
        })
    }

    /// Decode the next packet into `out` (cleared first) as interleaved
    /// samples. Returns `Ok(false)` at end of stream.
    pub fn next_chunk(&mut self, out: &mut Vec<f32>) -> Result<bool, AudioError> {
        out.clear();
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(false),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(false)
                }
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(AudioError::Corrupt(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    if buf.frames() == 0 {
                        continue;
                    }
                    // Channel count can differ from the header (for example
                    // mono AAC signalled as stereo); trust the buffer.
                    let ch = buf.spec().channels().count() as u16;
                    if ch != 0 {
                        self.info.channels = ch;
                    }
                    buf.copy_to_vec_interleaved(out);
                    self.position_frames += buf.frames() as u64;
                    return Ok(true);
                }
                Err(SymError::DecodeError(e)) => {
                    self.decode_errors += 1;
                    tracing::debug!("skipped undecodable packet: {e}");
                    if self.decode_errors > 50 {
                        return Err(AudioError::Corrupt(format!("too many decode errors, last: {e}")));
                    }
                }
                Err(e) => return Err(AudioError::Corrupt(e.to_string())),
            }
        }
    }

    /// Seek to `ms` from the start. Returns the position actually reached.
    pub fn seek_ms(&mut self, ms: u64) -> Result<u64, AudioError> {
        let time = Time::try_from_secs_f64(ms as f64 / 1000.0).unwrap_or(Time::ZERO);
        let seeked = self
            .reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| AudioError::Corrupt(format!("seek failed: {e}")))?;
        self.decoder.reset();
        let frames = seeked.actual_ts.get().max(0) as u64;
        self.position_frames = frames;
        Ok(frames * 1000 / self.info.sample_rate as u64)
    }

    pub fn position_ms(&self) -> u64 {
        self.position_frames * 1000 / self.info.sample_rate.max(1) as u64
    }
}

/// Open a file and decode its first packets to confirm it is playable.
pub fn probe(path: &Path) -> Result<AudioInfo, AudioError> {
    let mut dec = Decoder::open(path)?;
    let mut buf = Vec::new();
    let mut decoded = 0usize;
    for _ in 0..8 {
        if !dec.next_chunk(&mut buf)? {
            break;
        }
        decoded += buf.len();
    }
    if decoded == 0 {
        return Err(AudioError::Corrupt("no audio frames could be decoded".into()));
    }
    Ok(dec.info)
}

/// Result of decoding a whole file, used to validate downloads.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DecodeReport {
    pub info: AudioInfo,
    pub decoded_ms: u64,
    pub decode_errors: u64,
}

pub fn decode_all(path: &Path) -> Result<DecodeReport, AudioError> {
    let mut dec = Decoder::open(path)?;
    let mut buf = Vec::new();
    let mut samples: u64 = 0;
    while dec.next_chunk(&mut buf)? {
        samples += buf.len() as u64;
    }
    let ch = dec.info.channels.max(1) as u64;
    let decoded_ms = samples / ch * 1000 / dec.info.sample_rate.max(1) as u64;
    if decoded_ms == 0 {
        return Err(AudioError::Corrupt("no audio frames could be decoded".into()));
    }
    Ok(DecodeReport {
        info: dec.info.clone(),
        decoded_ms,
        decode_errors: dec.decode_errors,
    })
}
