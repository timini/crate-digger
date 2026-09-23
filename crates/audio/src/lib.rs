//! Audio decoding, playback and waveforms.
//!
//! Decoding uses symphonia (pure Rust) so every platform decodes the same
//! formats in the same way. Only formats with a passing decode test are
//! listed in [`SUPPORTED_FORMATS`]; see `docs/formats.md`.

pub mod decode;
pub mod fingerprint;
pub mod player;
mod resample;
pub mod synth;
pub mod waveform;

pub use decode::{probe, AudioError, AudioInfo, Decoder, SUPPORTED_FORMATS};
pub use player::{OutputConfig, PlayState, Player, PlayerStatus};
