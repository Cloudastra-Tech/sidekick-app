//! Streaming speech-to-text, vendor-abstracted. Each audio channel gets its own stream,
//! so transcripts stay attributed to "me" or "them".

mod deepgram;
pub mod vocabulary;

use crate::audio::{Channel, SttAudio};
use crate::config::env_or_dotenv;
use serde::Serialize;
use std::time::Instant;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

#[derive(Clone, Serialize)]
pub struct Transcript {
    pub channel: Channel,
    pub text: String,
    /// Interim results are replaced by later ones; final results are stable.
    pub is_final: bool,
    /// The speaker paused long enough for the provider to mark the end of an utterance.
    pub speech_final: bool,
    /// When the last audio covered by this result was captured, for latency measurement.
    #[serde(skip)]
    pub audio_end_at: Option<Instant>,
}

pub enum SttEvent {
    Transcript(Transcript),
    /// The speaker finished, and there was no new transcript to say so with (an empty final or
    /// an UtteranceEnd). Ends a turn the trigger engine is still waiting on; the timing comes
    /// from the speaker's last final, which already said when their last word was captured.
    TurnEnd {
        channel: Channel,
    },
    Failed {
        channel: Channel,
        message: String,
    },
    /// The connection dropped or stalled; retrying after `retry_in_secs`.
    Reconnecting {
        channel: Channel,
        message: String,
        retry_in_secs: u64,
    },
    /// Connected again after `Reconnecting`.
    Reconnected {
        channel: Channel,
    },
    /// The stream finished; `audio_seconds` is how much audio was sent (what is billed).
    Closed {
        channel: Channel,
        audio_seconds: f64,
    },
}

#[derive(Clone)]
pub enum SttProvider {
    Deepgram {
        api_key: String,
        /// Deepgram language code, e.g. `en`, `en-IN`, or `multi` (code-switching).
        language: String,
        /// Keep call audio out of Deepgram's Model Improvement Program (costs the full rate).
        mip_opt_out: bool,
        /// The playbook's custom vocabulary, sent as key terms.
        keyterms: Vec<String>,
    },
}

const DEFAULT_DEEPGRAM_LANGUAGE: &str = "en";

/// Whether Deepgram is asked not to keep call audio for model training. On unless
/// `DEEPGRAM_MIP_OPT_OUT=off`: prospects haven't agreed to that. Opting out doubles the rate.
pub fn deepgram_mip_opt_out() -> bool {
    env_or_dotenv("DEEPGRAM_MIP_OPT_OUT").is_none_or(|v| !v.trim().eq_ignore_ascii_case("off"))
}

/// For the live end-to-end test, which needs WASAPI.
#[cfg(all(test, windows))]
pub use deepgram::run as deepgram_run_for_tests;

/// The transcription key to use now: one the backend minted for this person's calls, else one on
/// this machine. Read again on every reconnect, so a key refreshed mid-call is picked up.
pub fn current_api_key() -> Option<String> {
    crate::backend::cached_stt_key().or_else(|| env_or_dotenv("DEEPGRAM_API_KEY"))
}

impl SttProvider {
    /// Reads provider settings from the environment, falling back to a local `.env`.
    pub fn from_env() -> Result<Self, String> {
        let api_key = current_api_key().ok_or(
            "No transcription key: sign in to your workspace, or set DEEPGRAM_API_KEY \
                 (environment or .env)",
        )?;
        let language =
            env_or_dotenv("DEEPGRAM_LANGUAGE").unwrap_or_else(|| DEFAULT_DEEPGRAM_LANGUAGE.into());
        // Goes straight into the query string, so only allow language-code characters.
        if !language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(format!("Invalid DEEPGRAM_LANGUAGE: {language}"));
        }
        let playbook = std::fs::read_to_string(crate::coach::playbook_path()).unwrap_or_default();
        Ok(Self::Deepgram {
            api_key,
            language,
            mip_opt_out: deepgram_mip_opt_out(),
            keyterms: vocabulary::from_playbook(&playbook),
        })
    }

    /// Streams 16 kHz mono PCM chunks from `audio` until the sender is dropped.
    pub fn spawn_stream(
        &self,
        channel: Channel,
        audio: UnboundedReceiver<SttAudio>,
        events: UnboundedSender<SttEvent>,
    ) {
        match self {
            Self::Deepgram {
                api_key,
                language,
                mip_opt_out,
                keyterms,
            } => {
                tauri::async_runtime::spawn(deepgram::run(
                    api_key.clone(),
                    language.clone(),
                    *mip_opt_out,
                    keyterms.clone(),
                    channel,
                    audio,
                    events,
                ));
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::audio::{wasapi_capture, CaptureEvent, LoopbackTarget};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    /// Live end-to-end check: system-audio loopback → Deepgram. Play speech through the
    /// speakers while it runs: `cargo test live_loopback -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_loopback_to_deepgram() {
        let provider = SttProvider::from_env().expect("DEEPGRAM_API_KEY");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();

        let stop = Arc::new(AtomicBool::new(false));
        let (capture_tx, capture_rx) = mpsc::channel();
        let handle = wasapi_capture::spawn(
            Channel::Them,
            LoopbackTarget::DefaultOutput,
            stop.clone(),
            capture_tx,
        )
        .unwrap();

        let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        match &provider {
            SttProvider::Deepgram {
                api_key,
                language,
                mip_opt_out,
                keyterms,
            } => {
                runtime.spawn(deepgram::run(
                    api_key.clone(),
                    language.clone(),
                    *mip_opt_out,
                    keyterms.clone(),
                    Channel::Them,
                    audio_rx,
                    events_tx,
                ));
            }
        }

        let forward = std::thread::spawn(move || {
            for event in capture_rx {
                match event {
                    CaptureEvent::Chunk(c) => {
                        let _ = audio_tx.send(SttAudio {
                            samples: c.samples,
                            captured_at: c.captured_at,
                            ends_speech: false,
                        });
                    }
                    CaptureEvent::Failed { message, .. } => panic!("capture: {message}"),
                }
            }
        });

        std::thread::sleep(Duration::from_secs(18));
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        forward.join().unwrap();

        let finals = runtime.block_on(async {
            let mut finals = Vec::new();
            let mut triggers = crate::trigger::TriggerEngine::with_default_keywords();
            while let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_secs(5), events_rx.recv()).await
            {
                match event {
                    SttEvent::Transcript(t) => {
                        if let Some(trigger) = triggers.on_transcript(&t, std::time::Instant::now())
                        {
                            println!(
                                "trigger: {:?} ({} lines)",
                                trigger.reason,
                                trigger.lines.len()
                            );
                        }
                        if t.is_final {
                            finals.push(t.text);
                        }
                    }
                    SttEvent::Failed { message, .. } => panic!("stt: {message}"),
                    SttEvent::Closed { .. }
                    | SttEvent::TurnEnd { .. }
                    | SttEvent::Reconnecting { .. }
                    | SttEvent::Reconnected { .. } => {}
                }
            }
            finals
        });
        println!("finals: {finals:?}");
        assert!(!finals.is_empty(), "no final transcripts received");
    }
}
