//! Platform audio capture. Mic ("me") and system audio ("them") are always captured as
//! two separate streams and never mixed — that separation is what gives us diarization.

#[cfg(target_os = "macos")]
pub mod coreaudio_capture;
#[cfg(windows)]
pub mod echo_cancel;
pub mod gate;
pub mod hearing;
#[cfg(windows)]
pub mod wasapi_capture;

/// The name of the default speaker or headset that system audio is captured from, for the
/// call's log; `None` when it can't be read.
pub fn default_output_name() -> Option<String> {
    #[cfg(windows)]
    return wasapi_capture::default_output_name();
    #[cfg(target_os = "macos")]
    return coreaudio_capture::default_output_name();
    #[cfg(not(any(windows, target_os = "macos")))]
    None
}

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Both streams are delivered as 16 kHz mono signed 16-bit PCM (Deepgram `linear16`).
pub const SAMPLE_RATE: u32 = 16_000;
/// Size of each chunk handed downstream.
pub const CHUNK_MS: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Local microphone.
    Me,
    /// System audio (loopback) — the remote side of the call.
    Them,
}

/// Where the "them" stream comes from.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub enum LoopbackTarget {
    /// Whatever the default render device plays. Wrong if the call app outputs to a
    /// different device than the default.
    DefaultOutput,
    /// Only the call app's audio (and its child processes), on whatever device it uses.
    /// Preferred once call detection can identify the call app's process.
    #[allow(dead_code)]
    Process(u32),
}

pub struct AudioChunk {
    pub channel: Channel,
    pub samples: Vec<i16>,
    /// When the last sample of the chunk was captured. Latency is measured from here, not from
    /// when the chunk was sent, so time spent queued, gated or replayed after a reconnect counts.
    pub captured_at: Instant,
}

/// Audio on its way to speech-to-text: 16 kHz mono samples and when the last one was captured.
pub struct SttAudio {
    pub samples: Vec<i16>,
    pub captured_at: Instant,
    /// The silence gate closed on this chunk: the speaker has been quiet for its whole hangover.
    pub ends_speech: bool,
}

/// How long `samples` samples last at `SAMPLE_RATE`.
pub fn duration_of(samples: usize) -> Duration {
    Duration::from_micros(samples as u64 * 1_000_000 / u64::from(SAMPLE_RATE))
}

/// When the last sample of a chunk was captured, given that `queued_bytes` of later audio (16-bit
/// samples) were already waiting behind it when it was handed on.
pub fn captured_before(queued_bytes: usize) -> Instant {
    let now = Instant::now();
    now.checked_sub(duration_of(queued_bytes / 2))
        .unwrap_or(now)
}

pub enum CaptureEvent {
    Chunk(AudioChunk),
    Failed { channel: Channel, message: String },
}

#[cfg(test)]
mod tests {
    /// Prints the default speaker or headset's name, as a call's log records it.
    /// `cargo test default_output_name_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn default_output_name_live() {
        let name = super::default_output_name();
        println!("default output: {name:?}");
        #[cfg(any(windows, target_os = "macos"))]
        assert!(name.is_some_and(|n| !n.trim().is_empty()));
    }
}
