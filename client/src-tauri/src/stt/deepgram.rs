//! Deepgram live streaming (`/v1/listen` over WebSocket), with automatic reconnection.

use super::{SttEvent, Transcript};
use crate::audio::{Channel, SttAudio, SAMPLE_RATE};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// Deepgram closes the socket after ~10 s without audio or a KeepAlive.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
/// After CloseStream, how long to wait for Deepgram's last results and close frame. The
/// session summary waits on this, so it must be bounded.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest a connection attempt may take. Without it a half-dead network (wi-fi roaming, a
/// captive portal) held one reconnect for 33 s on a real call, with no retry and no transcripts.
/// TLS to the US endpoint measured 0.8-1.1 s from India, so this is still five times that.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Waits between reconnection attempts; the last one repeats until capture stops. The first
/// retry is immediate: most drops are one dead socket, and the prospect is still talking.
const RECONNECT_DELAYS: [Duration; 6] = [
    Duration::ZERO,
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(10),
];
/// Audio held while disconnected and sent after reconnecting: the last 30 s (100 ms chunks).
///
/// Measured on a real call: a laptop's connection dropped Deepgram and the Claude proxy in the
/// same second and the outage outlasted the old 10 s window, so words spoken during it were
/// gone. Thirty seconds of held speech costs about a megabyte per stream, which is nothing, and
/// covers the blips a home or café connection actually takes. An outage longer than this still
/// loses its oldest audio; the wall only moved.
const BACKLOG_CHUNKS: usize = 300;
/// Speech sent with no response for this long means the connection is stuck (half-open
/// socket, dead network) even though no error was raised.
///
/// Eight seconds, not fifteen: on wi-fi a socket dies silently — the laptop roams to another
/// access point, or the link drops — and until this fires, speech is going into a socket nobody
/// is reading. Deepgram normally answers in under half a second, so this still leaves about
/// twenty times its usual latency before we give up on a connection, and a reconnect that turns
/// out to be unnecessary costs nothing: the held audio is replayed either way.
#[cfg(not(test))]
const STALL_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(test)]
const STALL_TIMEOUT: Duration = Duration::from_secs(2);
const STALL_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// A connection that lasted this long counts as working again, so the next drop starts the
/// reconnect delays over (the first retry is immediate). One dropped sooner keeps backing off.
const HEALTHY_CONNECTION: Duration = Duration::from_secs(5);
/// Silent chunks in a row that end an utterance when the silence gate isn't the one saying so
/// (gate off, paused, a loopback gap): 300 ms, the same as Deepgram's own endpointing.
const FINALIZE_AFTER_SILENT_CHUNKS: usize = 3;

/// Decides when to send Finalize, which Deepgram answers with a final that ends the turn. Silent
/// chunks aren't sent (streamed audio is billed by duration), so Deepgram can't see a pause for
/// itself; this is how it learns of one. Only real silence counts: the gate closing after its
/// hangover, or 300 ms of it. A single silent chunk inside a sentence (a codec gap, a 200 ms
/// loopback gap fill) would otherwise end the prospect's turn mid-sentence.
#[derive(Default)]
struct SilenceRun {
    chunks: usize,
    finalized: bool,
}

impl SilenceRun {
    /// Returns whether to send Finalize for this chunk.
    fn on_chunk(&mut self, audio: &SttAudio) -> bool {
        if !audio.samples.iter().all(|&s| s == 0) {
            *self = Self::default();
            return false;
        }
        self.chunks += 1;
        let ended = audio.ends_speech || self.chunks >= FINALIZE_AFTER_SILENT_CHUNKS;
        let send = ended && !self.finalized;
        self.finalized |= send;
        send
    }

    fn in_silence(&self) -> bool {
        self.chunks > 0
    }
}

pub async fn run(
    api_key: String,
    language: String,
    mip_opt_out: bool,
    keyterms: Vec<String>,
    channel: Channel,
    audio: UnboundedReceiver<SttAudio>,
    events: UnboundedSender<SttEvent>,
) {
    let url = listen_url(&language, mip_opt_out, &keyterms);
    let without_vocabulary =
        (!keyterms.is_empty()).then(|| listen_url(&language, mip_opt_out, &[]));
    run_with_url(
        &url,
        without_vocabulary,
        Some(api_key),
        true,
        channel,
        audio,
        events,
    )
    .await;
}

/// `no_delay`: with `smart_format`, Deepgram otherwise holds results back while it waits to
/// format numbers. Measured with `live_stt_latency` (18 s of speech): final results p50 850 ms
/// without it, about 450 ms with it; interim results about 420 ms and 300 ms.
///
/// `utterance_end_ms`: an UtteranceEnd message after a second without words. It ends a turn that
/// endpointing missed (noise kept Deepgram's detector in speech), which otherwise waited for the
/// 2 s silence fallback.
///
/// `keyterms`: the custom vocabulary (`stt::vocabulary`), so names are spelled right.
fn listen_url(language: &str, mip_opt_out: bool, keyterms: &[String]) -> String {
    let mut url = format!(
        "wss://api.deepgram.com/v1/listen?model=nova-3&language={language}&encoding=linear16\
         &sample_rate={SAMPLE_RATE}&channels=1&interim_results=true&smart_format=true\
         &endpointing=300&no_delay=true&utterance_end_ms=1000"
    );
    if mip_opt_out {
        url.push_str("&mip_opt_out=true");
    }
    url.push_str(&super::vocabulary::query(keyterms));
    url
}

/// Streams until capture stops, reconnecting after drops and stalls. Only errors that a retry
/// can't fix (rejected key, no credit, bad settings) end the stream early.
///
/// `live_key`: re-read the transcription key before each reconnect, and fetch a new one once if
/// Deepgram rejects it after the stream had been working. Keys the backend mints expire after a
/// few hours, so a long call can outlive the key it started with.
async fn run_with_url(
    url: &str,
    // The same stream without the custom vocabulary, to fall back to if Deepgram refuses it.
    without_vocabulary: Option<String>,
    api_key: Option<String>,
    live_key: bool,
    channel: Channel,
    mut audio: UnboundedReceiver<SttAudio>,
    events: UnboundedSender<SttEvent>,
) {
    let mut audio_seconds = 0.0;
    let mut failures = 0usize;
    let mut backlog = VecDeque::new();
    let mut api_key = api_key;
    let mut ever_connected = false;
    let mut key_refreshed = false;
    let mut url = url.to_string();
    let mut without_vocabulary = without_vocabulary;

    loop {
        if live_key && failures > 0 {
            if let Some(key) = super::current_api_key() {
                api_key = Some(key);
            }
        }
        let attempt_started = Instant::now();
        let outcome = stream(
            &url,
            api_key.as_deref(),
            channel,
            &mut audio,
            &mut backlog,
            &events,
            failures > 0,
        )
        .await;
        audio_seconds += outcome.audio_seconds;
        if outcome.connected {
            ever_connected = true;
            // A connection Deepgram accepts and then drops at once doesn't reset the delays, or
            // the immediate first retry would reconnect in a tight loop.
            if attempt_started.elapsed() >= HEALTHY_CONNECTION {
                failures = 0;
            }
        }

        match outcome.end {
            StreamEnd::AudioClosed => break,
            StreamEnd::KeyRejected(_) if live_key && ever_connected && !key_refreshed => {
                key_refreshed = true;
                let _ = events.send(SttEvent::Reconnecting {
                    channel,
                    message: "The transcription key expired; getting a new one".into(),
                    retry_in_secs: 0,
                });
                if let Err(e) = crate::backend::refresh_stt_key().await {
                    eprintln!("Could not refresh the transcription key: {e}");
                }
                if let Some(key) = super::current_api_key() {
                    api_key = Some(key);
                }
                failures = 1;
            }
            // Key terms are the newest setting and the likeliest to be refused (e.g. for a
            // language they don't support yet): a call without them beats no call.
            StreamEnd::SettingsRejected(message)
                if !ever_connected && without_vocabulary.is_some() =>
            {
                eprintln!(
                    "Deepgram refused the custom vocabulary ({message}); continuing without it"
                );
                url = without_vocabulary.take().unwrap_or_default();
            }
            StreamEnd::Fatal(message)
            | StreamEnd::KeyRejected(message)
            | StreamEnd::SettingsRejected(message) => {
                let _ = events.send(SttEvent::Failed { channel, message });
                break;
            }
            StreamEnd::Dropped(message) => {
                let delay = RECONNECT_DELAYS[failures.min(RECONNECT_DELAYS.len() - 1)];
                failures += 1;
                let _ = events.send(SttEvent::Reconnecting {
                    channel,
                    message,
                    retry_in_secs: delay.as_secs(),
                });
                // Keep collecting audio while waiting, so speech in the gap is still sent.
                let wait = tokio::time::sleep(delay);
                tokio::pin!(wait);
                let capture_ended = loop {
                    tokio::select! {
                        _ = &mut wait => break false,
                        chunk = audio.recv() => match chunk {
                            Some(samples) => push_backlog(&mut backlog, samples),
                            None => break true,
                        },
                    }
                };
                if capture_ended {
                    break;
                }
            }
        }
    }

    let _ = events.send(SttEvent::Closed {
        channel,
        audio_seconds,
    });
}

fn push_backlog(backlog: &mut VecDeque<SttAudio>, audio: SttAudio) {
    if backlog.len() == BACKLOG_CHUNKS {
        backlog.pop_front();
    }
    backlog.push_back(audio);
}

struct StreamOutcome {
    connected: bool,
    /// Audio sent (and billed) on this connection.
    audio_seconds: f64,
    end: StreamEnd,
}

enum StreamEnd {
    /// Capture stopped and Deepgram flushed its last results.
    AudioClosed,
    /// Connection lost or stuck; worth reconnecting.
    Dropped(String),
    /// Retrying won't help.
    Fatal(String),
    /// Deepgram refused the key (HTTP 401/403). Fatal, unless a new key fixes it.
    KeyRejected(String),
    /// Deepgram refused the stream's settings (HTTP 400). Fatal, unless dropping the custom
    /// vocabulary fixes it.
    SettingsRejected(String),
}

enum WriterEnd {
    AudioClosed,
    Socket(WsError),
    Stalled,
}

/// Maps Deepgram's audio clock (seconds of audio *sent* on this connection, which skips unsent
/// silence) back to the wall-clock time the audio was captured.
#[derive(Default)]
struct SentAudio {
    /// (audio offset at end of chunk, when the chunk's last sample was captured), oldest first.
    chunks: VecDeque<(f64, Instant)>,
    total_seconds: f64,
}

impl SentAudio {
    fn record(&mut self, samples: usize, at: Instant) {
        self.total_seconds += samples as f64 / f64::from(SAMPLE_RATE);
        self.chunks.push_back((self.total_seconds, at));
    }

    /// Wall-clock capture time of the audio at `offset`; forgets chunks well before it. Within a
    /// chunk the audio is continuous, so a point inside it was captured that much before its end.
    fn captured_at(&mut self, offset: f64) -> Option<Instant> {
        while self
            .chunks
            .front()
            .is_some_and(|&(end, _)| end < offset - 30.0)
        {
            self.chunks.pop_front();
        }
        self.chunks
            .iter()
            .find(|&&(end, _)| end >= offset - 1e-3)
            .map(|&(end, at)| {
                let before_end = Duration::from_secs_f64((end - offset).max(0.0));
                at.checked_sub(before_end).unwrap_or(at)
            })
    }
}

/// One connection: sends the backlog, then live audio, until capture stops or the
/// connection fails.
async fn stream(
    url: &str,
    api_key: Option<&str>,
    channel: Channel,
    audio: &mut UnboundedReceiver<SttAudio>,
    backlog: &mut VecDeque<SttAudio>,
    events: &UnboundedSender<SttEvent>,
    reconnecting: bool,
) -> StreamOutcome {
    let not_connected = |end| StreamOutcome {
        connected: false,
        audio_seconds: 0.0,
        end,
    };

    let mut request = match url.into_client_request() {
        Ok(request) => request,
        Err(e) => return not_connected(StreamEnd::Fatal(e.to_string())),
    };
    if let Some(key) = api_key {
        match HeaderValue::from_str(&format!("Token {key}")) {
            Ok(value) => {
                request.headers_mut().insert("Authorization", value);
            }
            Err(e) => return not_connected(StreamEnd::Fatal(e.to_string())),
        }
    }
    // Nagle off: the Finalize frame that ends a turn is tiny, and with Nagle on it waits for the
    // audio before it to be acknowledged, about one round trip.
    let connect = connect_async_with_config(request, None, true);
    let socket = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok((socket, _))) => socket,
        Ok(Err(e)) => {
            let status = match &e {
                WsError::Http(response) => Some(response.status().as_u16()),
                _ => None,
            };
            let message = describe_connect_error(e);
            return not_connected(match status {
                Some(401 | 403) => StreamEnd::KeyRejected(message),
                Some(400) => StreamEnd::SettingsRejected(message),
                Some(401..=402) => StreamEnd::Fatal(message),
                _ => StreamEnd::Dropped(message),
            });
        }
        Err(_) => {
            return not_connected(StreamEnd::Dropped(format!(
                "Deepgram didn't answer within {} s",
                CONNECT_TIMEOUT.as_secs()
            )))
        }
    };
    if reconnecting {
        let _ = events.send(SttEvent::Reconnected { channel });
    }

    let (mut sink, mut source) = socket.split();
    let sent = Arc::new(Mutex::new(SentAudio::default()));
    let sent_by_writer = sent.clone();
    let last_message_at = Arc::new(Mutex::new(Instant::now()));
    let last_message_seen_by_writer = last_message_at.clone();

    let writer = async move {
        let mut silence = SilenceRun::default();
        // When the oldest speech Deepgram hasn't responded to yet was sent. Tracking the
        // latest send instead would never look stalled, since audio goes out every 100 ms.
        let mut unanswered_since: Option<Instant> = None;
        let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
        let mut stall_check = tokio::time::interval(STALL_CHECK_INTERVAL);

        loop {
            let chunk = if let Some(chunk) = backlog.pop_front() {
                chunk
            } else {
                tokio::select! {
                    chunk = audio.recv() => match chunk {
                        Some(chunk) => chunk,
                        None => {
                            // Capture ended: ask Deepgram to flush and close.
                            let _ = sink.send(Message::Text(r#"{"type":"CloseStream"}"#.into())).await;
                            return WriterEnd::AudioClosed;
                        }
                    },
                    _ = keepalive.tick() => {
                        if silence.in_silence() {
                            if let Err(e) = sink.send(Message::Text(r#"{"type":"KeepAlive"}"#.into())).await {
                                return WriterEnd::Socket(e);
                            }
                        }
                        continue;
                    }
                    _ = stall_check.tick() => {
                        let last_message = *last_message_seen_by_writer.lock().expect("last-message lock poisoned");
                        if let Some(since) = unanswered_since {
                            if last_message >= since {
                                // Deepgram has answered since then; start tracking afresh.
                                unanswered_since = None;
                            } else if since.elapsed() >= STALL_TIMEOUT {
                                return WriterEnd::Stalled;
                            }
                        }
                        continue;
                    }
                }
            };

            // Streamed audio is billed by duration, so don't send digital silence (loopback
            // gap fill, pauses). Finalize flushes the pending utterance instead.
            let finalize = silence.on_chunk(&chunk);
            if silence.in_silence() {
                if finalize {
                    if let Err(e) = sink
                        .send(Message::Text(r#"{"type":"Finalize"}"#.into()))
                        .await
                    {
                        return WriterEnd::Socket(e);
                    }
                }
                continue;
            }
            keepalive.reset();
            let bytes: Vec<u8> = chunk.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            if let Err(e) = sink.send(Message::Binary(bytes.into())).await {
                // Keep the chunk for the next connection.
                backlog.push_front(chunk);
                return WriterEnd::Socket(e);
            }
            unanswered_since.get_or_insert_with(Instant::now);
            sent_by_writer
                .lock()
                .expect("sent-audio lock poisoned")
                .record(chunk.samples.len(), chunk.captured_at);
        }
    };

    let reader = async {
        while let Some(message) = source.next().await {
            *last_message_at.lock().expect("last-message lock poisoned") = Instant::now();
            match message? {
                Message::Text(text) => match parse_result(channel, &text) {
                    Some(Parsed::Transcript(mut transcript, speech_end)) => {
                        transcript.audio_end_at = sent
                            .lock()
                            .expect("sent-audio lock poisoned")
                            .captured_at(speech_end);
                        let _ = events.send(SttEvent::Transcript(transcript));
                    }
                    Some(Parsed::TurnEnd) => {
                        let _ = events.send(SttEvent::TurnEnd { channel });
                    }
                    None => {}
                },
                Message::Close(_) => break,
                _ => {}
            }
        }
        Ok::<(), WsError>(())
    };

    let mut writer = std::pin::pin!(writer);
    let mut reader = std::pin::pin!(reader);
    let end = tokio::select! {
        // Deepgram closed or errored while capture is still running: reconnect.
        read = &mut reader => StreamEnd::Dropped(match read {
            Ok(()) => "Deepgram closed the connection".into(),
            Err(e) => format!("Deepgram connection lost: {e}"),
        }),
        written = &mut writer => match written {
            WriterEnd::AudioClosed => {
                // Give Deepgram a bounded time to send its last results.
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, &mut reader).await;
                StreamEnd::AudioClosed
            }
            WriterEnd::Socket(e) => StreamEnd::Dropped(format!("Deepgram connection lost: {e}")),
            WriterEnd::Stalled => StreamEnd::Dropped(format!(
                "Deepgram stopped responding for {} s",
                STALL_TIMEOUT.as_secs()
            )),
        },
    };

    let audio_seconds = sent.lock().expect("sent-audio lock poisoned").total_seconds;
    StreamOutcome {
        connected: true,
        audio_seconds,
        end,
    }
}

#[derive(Deserialize)]
struct DeepgramResults {
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    speech_final: bool,
    /// The result that answers a Finalize, which the writer sends once the silence gate has heard
    /// no speech for its 800 ms hangover, or after 300 ms of silence where the gate isn't in play:
    /// the speaker has stopped, whatever Deepgram's own detector thinks.
    #[serde(default)]
    from_finalize: bool,
    #[serde(default)]
    start: f64,
    #[serde(default)]
    duration: f64,
    channel: DeepgramChannel,
}

#[derive(Deserialize)]
struct DeepgramChannel {
    alternatives: Vec<DeepgramAlternative>,
}

#[derive(Deserialize)]
struct DeepgramAlternative {
    transcript: String,
    #[serde(default)]
    words: Vec<DeepgramWord>,
}

#[derive(Deserialize)]
struct DeepgramWord {
    end: f64,
}

/// A Deepgram message worth passing on. Offsets are on Deepgram's audio clock, in seconds.
enum Parsed {
    /// A transcript, and where its last word ends.
    Transcript(Transcript, f64),
    /// The speaker finished, with nothing new to transcribe.
    TurnEnd,
}

fn parse_result(channel: Channel, raw: &str) -> Option<Parsed> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    match value.get("type")?.as_str()? {
        // No words for `utterance_end_ms` after the last one: the turn is over.
        "UtteranceEnd" => return Some(Parsed::TurnEnd),
        "Results" => {}
        _ => return None,
    }
    let message: DeepgramResults = serde_json::from_value(value).ok()?;
    let alternative = message.channel.alternatives.into_iter().next()?;
    let ends_turn = message.speech_final || (message.is_final && message.from_finalize);
    if alternative.transcript.trim().is_empty() {
        // Deepgram often reports the end of a turn it has already transcribed as an empty
        // result; dropping it left the turn to the 2 s silence fallback.
        return ends_turn.then_some(Parsed::TurnEnd);
    }
    // The last word's end, not the result window's: the window runs on through the silence that
    // endpointing waited for, and measuring from there hid that wait from the latency numbers.
    let speech_end = alternative
        .words
        .last()
        .map_or(message.start + message.duration, |word| word.end);
    let transcript = Transcript {
        channel,
        text: alternative.transcript,
        is_final: message.is_final,
        speech_final: ends_turn,
        audio_end_at: None,
    };
    Some(Parsed::Transcript(transcript, speech_end))
}

fn describe_connect_error(err: WsError) -> String {
    match err {
        WsError::Http(response) => match response.status().as_u16() {
            401 | 403 => "Deepgram rejected the API key".into(),
            400 => "Deepgram rejected the stream settings — check DEEPGRAM_LANGUAGE".into(),
            402 => "Deepgram account is out of credit".into(),
            status => format!("Deepgram refused the connection (HTTP {status})"),
        },
        other => format!("Could not connect to Deepgram: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_out_adds_query_parameter() {
        assert!(listen_url("en", true, &[]).ends_with("&mip_opt_out=true"));
        assert!(!listen_url("en", false, &[]).contains("mip_opt_out"));
    }

    #[test]
    fn vocabulary_is_sent_as_key_terms() {
        let terms = vec!["Acme".to_string(), "Hugging Face".to_string()];
        assert!(listen_url("en", false, &terms).ends_with("&keyterm=Acme&keyterm=Hugging%20Face"));
        assert!(!listen_url("en", false, &[]).contains("keyterm"));
    }

    #[test]
    // The rejection type comes from tungstenite's handshake API.
    #[allow(clippy::result_large_err)]
    fn a_vocabulary_deepgram_refuses_is_dropped_and_the_call_goes_on() {
        use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
        use tokio_tungstenite::tungstenite::http::StatusCode;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("ws://{}", listener.local_addr().unwrap());
            tokio::spawn(async move {
                while let Ok((tcp, _)) = listener.accept().await {
                    // Like Deepgram refusing a setting it doesn't support: HTTP 400.
                    let refuse_terms = |request: &Request, response: Response| {
                        if request.uri().query().unwrap_or("").contains("keyterm") {
                            let mut refused = ErrorResponse::new(Some("bad keyterm".into()));
                            *refused.status_mut() = StatusCode::BAD_REQUEST;
                            Err(refused)
                        } else {
                            Ok(response)
                        }
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, refuse_terms).await
                    else {
                        continue;
                    };
                    tokio::spawn(async move {
                        let mut answered = false;
                        while let Some(Ok(message)) = ws.next().await {
                            match message {
                                Message::Binary(_) => {
                                    let reply = if answered {
                                        EMPTY_INTERIM
                                    } else {
                                        FINAL_RESULT
                                    };
                                    answered = true;
                                    let _ = ws.send(Message::Text(reply.into())).await;
                                }
                                Message::Text(text) if text.contains("CloseStream") => {
                                    let _ = ws.close(None).await;
                                    break;
                                }
                                _ => {}
                            }
                        }
                    });
                }
            });

            let with_terms = format!("{base}/v1/listen?model=nova-3&keyterm=Acme");
            let without_terms = format!("{base}/v1/listen?model=nova-3");
            let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel();
            let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
            let stream = tokio::spawn(async move {
                run_with_url(
                    &with_terms,
                    Some(without_terms),
                    None,
                    false,
                    Channel::Them,
                    audio_rx,
                    events_tx,
                )
                .await;
            });
            for _ in 0..20 {
                let _ = audio_tx.send(SttAudio {
                    samples: vec![1000i16; 1600],
                    captured_at: Instant::now(),
                    ends_speech: false,
                });
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            drop(audio_tx);
            tokio::time::timeout(Duration::from_secs(20), stream)
                .await
                .expect("stream ends")
                .unwrap();
            let mut events = Vec::new();
            while let Ok(event) = events_rx.try_recv() {
                events.push(event);
            }
            let kinds: Vec<&str> = events.iter().map(kind).collect();
            assert_eq!(kinds, ["transcript", "closed"], "{kinds:?}");
        });
    }

    /// Live latency bench: streams a 16 kHz mono WAV to Deepgram at real-time pace (100 ms chunks,
    /// then 2 s of silence) and prints how long after the audio each interim and final result
    /// arrived. `STT_LATENCY_WAV=<path> cargo test live_stt_latency -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_stt_latency() {
        let path = std::env::var("STT_LATENCY_WAV").expect("STT_LATENCY_WAV");
        let key = crate::config::env_or_dotenv("DEEPGRAM_API_KEY").expect("DEEPGRAM_API_KEY");
        let wav = std::fs::read(path).unwrap();
        let data = wav
            .windows(4)
            .position(|w| w == b"data")
            .expect("data chunk")
            + 8;
        let samples: Vec<i16> = wav[data..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_le_bytes(b))
            .collect();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel();
            let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
            let url = listen_url("en", true, &[]);
            let stream = tokio::spawn(async move {
                run_with_url(
                    &url,
                    None,
                    Some(key),
                    false,
                    Channel::Them,
                    audio_rx,
                    events_tx,
                )
                .await;
            });
            let feeder = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_millis(100));
                for chunk in samples
                    .chunks(1600)
                    .chain(std::iter::repeat_n(&[0i16; 1600][..], 20))
                {
                    tick.tick().await;
                    let _ = audio_tx.send(SttAudio {
                        samples: chunk.to_vec(),
                        captured_at: Instant::now(),
                        ends_speech: false,
                    });
                }
            });

            let (mut interim, mut finals) = (Vec::new(), Vec::new());
            let mut last_final_end: Option<Instant> = None;
            while let Some(event) = events_rx.recv().await {
                if let SttEvent::TurnEnd { .. } = event {
                    let since = last_final_end.map(|end| end.elapsed().as_millis());
                    println!("{since:>5?} ms  turn end (after the last final's last word)");
                    continue;
                }
                if let SttEvent::Transcript(t) = event {
                    let Some(end) = t.audio_end_at else { continue };
                    if t.is_final {
                        last_final_end = Some(end);
                    }
                    let ms = Instant::now().saturating_duration_since(end).as_millis() as u64;
                    println!(
                        "{:>5} ms  {}  {}",
                        ms,
                        match (t.is_final, t.speech_final) {
                            (true, true) => "final, turn end",
                            (true, false) => "final          ",
                            _ => "interim        ",
                        },
                        t.text
                    );
                    if t.text.trim().is_empty() {
                        continue;
                    }
                    if t.is_final {
                        finals.push(ms)
                    } else {
                        interim.push(ms)
                    }
                }
            }
            feeder.await.unwrap();
            stream.await.unwrap();
            let pct = |v: &mut Vec<u64>, p: f64| {
                v.sort_unstable();
                v.get(((v.len() as f64 * p).ceil() as usize).saturating_sub(1))
                    .copied()
            };
            println!(
                "interim n={} p50={:?} p90={:?} | final n={} p50={:?} p90={:?}",
                interim.len(),
                pct(&mut interim, 0.5),
                pct(&mut interim, 0.9),
                finals.len(),
                pct(&mut finals, 0.5),
                pct(&mut finals, 0.9),
            );
        });
    }

    #[test]
    fn parses_final_transcript() {
        let raw = r#"{"type":"Results","is_final":true,"speech_final":true,"start":1.5,"duration":2.0,
            "channel":{"alternatives":[{"transcript":"hello there","confidence":0.98}]}}"#;
        let Some(Parsed::Transcript(t, audio_end)) = parse_result(Channel::Them, raw) else {
            panic!("a transcript")
        };
        assert_eq!(audio_end, 3.5);
        assert_eq!(t.text, "hello there");
        assert!(t.is_final);
        assert!(t.speech_final);
    }

    #[test]
    fn maps_audio_offsets_to_capture_time() {
        let base = Instant::now();
        let mut sent = SentAudio::default();
        for i in 0..5u64 {
            // 100 ms chunks sent 100 ms apart.
            sent.record(1600, base + Duration::from_millis(100 * i));
        }
        // Inside a chunk: that much before the chunk's last sample.
        assert_eq!(
            sent.captured_at(0.25),
            Some(base + Duration::from_millis(150))
        );
        assert_eq!(
            sent.captured_at(0.5),
            Some(base + Duration::from_millis(400))
        );
        assert_eq!(sent.captured_at(9.0), None);
        assert!((sent.total_seconds - 0.5).abs() < 1e-9);
    }

    /// A local WebSocket server standing in for Deepgram: `script` decides what each
    /// accepted connection does.
    async fn fake_deepgram<F, Fut>(connections: usize, script: F) -> String
    where
        F: Fn(usize, tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            for index in 0..connections {
                let (tcp, _) = listener.accept().await.unwrap();
                let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                tokio::spawn(script(index, ws));
            }
        });
        url
    }

    const FINAL_RESULT: &str = r#"{"type":"Results","is_final":true,"speech_final":true,"start":0,"duration":0.1,
        "channel":{"alternatives":[{"transcript":"after reconnect"}]}}"#;

    const EMPTY_INTERIM: &str =
        r#"{"type":"Results","is_final":false,"channel":{"alternatives":[{"transcript":""}]}}"#;

    /// Feeds 100 ms speech chunks for `seconds`, then ends capture; returns every event.
    async fn run_against(url: String, seconds: u64) -> Vec<SttEvent> {
        let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = tokio::spawn(async move {
            run_with_url(&url, None, None, false, Channel::Them, audio_rx, events_tx).await;
        });
        for _ in 0..seconds * 10 {
            let _ = audio_tx.send(SttAudio {
                samples: vec![1000i16; 1600],
                captured_at: Instant::now(),
                ends_speech: false,
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        drop(audio_tx);
        tokio::time::timeout(Duration::from_secs(20), stream)
            .await
            .expect("stream ends")
            .unwrap();
        let mut events = Vec::new();
        while let Ok(event) = events_rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn kind(event: &SttEvent) -> &'static str {
        match event {
            SttEvent::Reconnecting { .. } => "reconnecting",
            SttEvent::Reconnected { .. } => "reconnected",
            SttEvent::Transcript(_) => "transcript",
            SttEvent::TurnEnd { .. } => "turn_end",
            SttEvent::Failed { .. } => "failed",
            SttEvent::Closed { .. } => "closed",
        }
    }

    #[test]
    fn reconnects_after_the_socket_drops_and_counts_all_audio() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let url = fake_deepgram(2, |index, mut ws| async move {
                if index == 0 {
                    // Take a little audio, then drop the connection without a close frame.
                    for _ in 0..3 {
                        let _ = ws.next().await;
                    }
                    drop(ws);
                    return;
                }
                // Like Deepgram, answer while audio flows (one real line, then empty interims),
                // so the stall check stays quiet on a healthy connection.
                let mut answered = false;
                while let Some(Ok(message)) = ws.next().await {
                    match message {
                        Message::Binary(_) => {
                            let reply = if answered {
                                EMPTY_INTERIM
                            } else {
                                FINAL_RESULT
                            };
                            answered = true;
                            let _ = ws.send(Message::Text(reply.into())).await;
                        }
                        Message::Text(text) if text.contains("CloseStream") => {
                            let _ = ws.close(None).await;
                            break;
                        }
                        _ => {}
                    }
                }
            })
            .await;

            let events = run_against(url, 4).await;
            let kinds: Vec<&str> = events.iter().map(kind).collect();
            assert_eq!(
                kinds,
                ["reconnecting", "reconnected", "transcript", "closed"],
                "{kinds:?}"
            );
            let Some(SttEvent::Closed { audio_seconds, .. }) = events.last() else {
                unreachable!()
            };
            assert!(
                *audio_seconds > 2.0,
                "audio from both connections is counted: {audio_seconds}"
            );
        });
    }

    #[test]
    fn reconnects_when_deepgram_stops_responding() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let url = fake_deepgram(2, |index, mut ws| async move {
                // The first connection accepts audio but never answers; the second answers.
                let mut answered = false;
                while let Some(Ok(message)) = ws.next().await {
                    match message {
                        Message::Binary(_) if index == 1 && !answered => {
                            answered = true;
                            let _ = ws.send(Message::Text(FINAL_RESULT.into())).await;
                        }
                        Message::Text(text) if text.contains("CloseStream") => {
                            let _ = ws.close(None).await;
                            break;
                        }
                        _ => {}
                    }
                }
            })
            .await;

            let events = run_against(url, 7).await;
            let reason = events.iter().find_map(|e| match e {
                SttEvent::Reconnecting { message, .. } => Some(message.clone()),
                _ => None,
            });
            assert!(
                reason
                    .as_deref()
                    .is_some_and(|m| m.contains("stopped responding")),
                "expected a stall reconnect, got {reason:?}"
            );
            assert!(events
                .iter()
                .any(|e| matches!(e, SttEvent::Transcript(t) if t.text == "after reconnect")));
        });
    }

    #[test]
    fn rejected_key_is_not_retried() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                while let Ok((mut tcp, _)) = listener.accept().await {
                    let mut request = [0u8; 2048];
                    let _ = tcp.read(&mut request).await;
                    let _ = tcp
                        .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                        .await;
                }
            });
            let events = run_against(url, 2).await;
            let kinds: Vec<&str> = events.iter().map(kind).collect();
            assert!(
                events.iter().any(|e| matches!(e, SttEvent::Failed { message, .. } if message.contains("rejected the API key"))),
                "{kinds:?}"
            );
            assert!(!kinds.contains(&"reconnecting"), "{kinds:?}");
        });
    }

    fn chunk(silent: bool, ends_speech: bool) -> SttAudio {
        SttAudio {
            samples: vec![if silent { 0 } else { 900 }; 1600],
            captured_at: Instant::now(),
            ends_speech,
        }
    }

    #[test]
    fn one_silent_chunk_inside_a_sentence_does_not_finalize() {
        let mut run = SilenceRun::default();
        assert!(!run.on_chunk(&chunk(false, false)));
        assert!(!run.on_chunk(&chunk(true, false)), "a codec gap");
        assert!(!run.on_chunk(&chunk(false, false)));
        // 300 ms of silence where the gate isn't deciding: once.
        let sent: Vec<bool> = (0..6).map(|_| run.on_chunk(&chunk(true, false))).collect();
        assert_eq!(sent, [false, false, true, false, false, false]);
    }

    #[test]
    fn the_gate_closing_finalizes_at_once() {
        let mut run = SilenceRun::default();
        run.on_chunk(&chunk(false, false));
        assert!(run.on_chunk(&chunk(true, true)));
        assert!(!run.on_chunk(&chunk(true, false)), "only once per silence");
        run.on_chunk(&chunk(false, false));
        assert!(run.on_chunk(&chunk(true, true)), "again after new speech");
    }

    #[test]
    fn measures_to_the_last_word_not_the_end_of_the_window() {
        let raw = r#"{"type":"Results","is_final":true,"speech_final":true,"start":1.0,"duration":1.6,
            "channel":{"alternatives":[{"transcript":"what does it cost","words":[
                {"word":"what","start":1.0,"end":1.2},{"word":"cost","start":1.8,"end":2.25}]}]}}"#;
        let Some(Parsed::Transcript(_, speech_end)) = parse_result(Channel::Them, raw) else {
            panic!("a transcript")
        };
        assert_eq!(speech_end, 2.25);
    }

    #[test]
    fn a_finalize_answer_ends_the_turn() {
        let raw = r#"{"type":"Results","is_final":true,"speech_final":false,"from_finalize":true,
            "start":3.0,"duration":0.4,"channel":{"alternatives":[{"transcript":"okay then"}]}}"#;
        let Some(Parsed::Transcript(t, _)) = parse_result(Channel::Them, raw) else {
            panic!("a transcript")
        };
        assert!(t.speech_final);
        // An interim is never the end of a turn, whatever else it says.
        let interim = r#"{"type":"Results","is_final":false,"from_finalize":true,"start":3.0,
            "duration":0.4,"channel":{"alternatives":[{"transcript":"okay"}]}}"#;
        let Some(Parsed::Transcript(t, _)) = parse_result(Channel::Them, interim) else {
            panic!("a transcript")
        };
        assert!(!t.speech_final);
    }

    #[test]
    fn empty_turn_ends_and_utterance_ends_are_passed_on() {
        let empty_final = r#"{"type":"Results","is_final":true,"speech_final":true,"start":4.2,
            "duration":0.8,"channel":{"alternatives":[{"transcript":""}]}}"#;
        assert!(matches!(
            parse_result(Channel::Them, empty_final),
            Some(Parsed::TurnEnd)
        ));
        let utterance_end = r#"{"type":"UtteranceEnd","channel":[0,1],"last_word_end":5.75}"#;
        assert!(matches!(
            parse_result(Channel::Them, utterance_end),
            Some(Parsed::TurnEnd)
        ));
        // An empty final that doesn't end a turn is still nothing.
        let empty = r#"{"type":"Results","is_final":true,"speech_final":false,"start":4.2,
            "duration":0.8,"channel":{"alternatives":[{"transcript":""}]}}"#;
        assert!(parse_result(Channel::Them, empty).is_none());
    }

    #[test]
    fn ignores_empty_and_non_result_messages() {
        let empty =
            r#"{"type":"Results","is_final":false,"channel":{"alternatives":[{"transcript":""}]}}"#;
        assert!(parse_result(Channel::Me, empty).is_none());
        assert!(parse_result(Channel::Me, r#"{"type":"Metadata"}"#).is_none());
    }
}
