//! Capture session control: enforces the consent gate, runs the mic and loopback
//! capture threads, streams each channel to STT, runs triggers and the coach, and
//! forwards results to the UI.

use crate::audio::gate::SilenceGate;
use crate::audio::hearing::{HearingChange, HearingCheck};
use crate::audio::{duration_of, CaptureEvent, Channel, LoopbackTarget, SttAudio};
use crate::coach::{self, CallBrief, CoachInput, CoachSettings};
use crate::config::env_f64;
use crate::consent::{self, ConsentState};
use crate::cpu::CpuSampler;
use crate::stt::{SttEvent, SttProvider, Transcript};
use crate::trigger::{Line, TriggerEngine};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// How often the trigger engine checks for silence. Bounds how late a silence trigger
/// can fire past the threshold.
const TRIGGER_TICK: Duration = Duration::from_millis(250);
const CPU_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_MAX_CALL_MINUTES: f64 = 180.0;
const DEFAULT_AUTO_STOP_SILENCE_MINUTES: f64 = 15.0;
const DEFAULT_MAX_CALL_COST_USD: f64 = 5.0;
/// How far ahead of an automatic stop the UI is warned (less for very short limits).
const WARNING_LEAD: Duration = Duration::from_secs(60);
/// Share of the spend limit at which the UI is warned.
const COST_WARNING_SHARE: f64 = 0.9;

/// Ends a call nobody stopped: a forgotten Stop would otherwise keep capturing (and billing)
/// after the consented call is over.
#[derive(Clone, Copy, Debug)]
struct CallLimits {
    max_length: Duration,
    /// No speech on either side for this long, not counting time paused.
    silence: Duration,
    /// Spend on hints and transcription for this call, in USD.
    max_cost_usd: f64,
}

/// Where a running call stands against its limits.
#[derive(Clone, Copy, Debug, Default)]
struct CallProgress {
    call_length: Duration,
    since_speech: Duration,
    spent_usd: f64,
}

impl CallLimits {
    fn from_env() -> Result<Self, String> {
        let minutes = |key: &str, default: f64| -> Result<Duration, String> {
            let value = env_f64(key)?.unwrap_or(default);
            if !(value.is_finite() && value > 0.0) {
                return Err(format!(
                    "{key} must be a number of minutes above 0, got {value}"
                ));
            }
            Ok(Duration::from_secs_f64(value * 60.0))
        };
        let max_cost_usd = env_f64("CALL_MAX_COST_USD")?.unwrap_or(DEFAULT_MAX_CALL_COST_USD);
        if !(max_cost_usd.is_finite() && max_cost_usd > 0.0) {
            return Err(format!(
                "CALL_MAX_COST_USD must be a dollar amount above 0, got {max_cost_usd}"
            ));
        }
        Ok(Self {
            max_length: minutes("CALL_MAX_MINUTES", DEFAULT_MAX_CALL_MINUTES)?,
            silence: minutes(
                "CALL_AUTO_STOP_SILENCE_MINUTES",
                DEFAULT_AUTO_STOP_SILENCE_MINUTES,
            )?,
            max_cost_usd,
        })
    }

    /// Why the call should end now, if it should.
    fn stop_reason(&self, progress: CallProgress) -> Option<String> {
        if progress.call_length >= self.max_length {
            Some(format!(
                "Call ended automatically: it reached the {} minute limit.",
                minutes_label(self.max_length)
            ))
        } else if progress.since_speech >= self.silence {
            Some(format!(
                "Call ended automatically: no speech for {} minutes.",
                minutes_label(self.silence)
            ))
        } else if progress.spent_usd >= self.max_cost_usd {
            Some(format!(
                "Call ended automatically: it reached the ${:.2} spend limit.",
                self.max_cost_usd
            ))
        } else {
            None
        }
    }

    /// A heads-up shortly before `stop_reason` would end the call.
    fn warning(&self, progress: CallProgress) -> Option<String> {
        if self.stop_reason(progress).is_some() {
            return None;
        }
        let lead = |limit: Duration| WARNING_LEAD.min(limit / 4);
        if progress.call_length + lead(self.max_length) >= self.max_length {
            Some(format!(
                "This call ends automatically soon: it is reaching the {} minute limit.",
                minutes_label(self.max_length)
            ))
        } else if progress.since_speech + lead(self.silence) >= self.silence {
            Some(format!(
                "No speech for a while: the call ends automatically after {} silent minutes unless someone speaks.",
                minutes_label(self.silence)
            ))
        } else if progress.spent_usd >= self.max_cost_usd * COST_WARNING_SHARE {
            Some(format!(
                "This call has used about ${:.2} of its ${:.2} spend limit; it ends automatically at the limit.",
                progress.spent_usd, self.max_cost_usd
            ))
        } else {
            None
        }
    }
}

fn minutes_label(duration: Duration) -> String {
    let minutes = (duration.as_secs_f64() / 60.0 * 100.0).round() / 100.0;
    format!("{minutes}")
}

/// What the session loop needs to end a call on its own.
struct AutoStop {
    limits: CallLimits,
    paused: Arc<AtomicBool>,
    /// Audio streams billed while the call isn't paused: 2 for a real call, 0 for a demo.
    stt_streams: u32,
}

#[derive(Default)]
pub struct CaptureState {
    session: Mutex<Option<Session>>,
    /// ID of the most recently started call. A call keeps working after Stop (flushing
    /// transcripts, writing its summary); once a newer call starts, its events are dropped.
    latest_id: AtomicU64,
}

impl CaptureState {
    /// True until a newer call starts.
    pub fn is_latest(&self, session_id: u64) -> bool {
        self.latest_id.load(Ordering::Relaxed) == session_id
    }

    /// Whether this call is still capturing (not stopped, and no newer call started).
    pub fn is_running_call(&self, session_id: u64) -> bool {
        self.session
            .lock()
            .expect("capture lock poisoned")
            .as_ref()
            .is_some_and(|session| session.id == session_id)
    }

    pub fn is_running(&self) -> bool {
        self.session
            .lock()
            .expect("capture lock poisoned")
            .is_some()
    }

    /// The running call's ID and the call app it was started for, if any.
    pub fn running_call_app(&self) -> Option<(u64, String, String)> {
        let session = self.session.lock().expect("capture lock poisoned");
        let session = session.as_ref()?;
        let (key, name) = session.call_app.clone()?;
        Some((session.id, key, name))
    }

    fn next_id(&self) -> u64 {
        self.latest_id.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// Emits a call's event to the UI, unless a newer call has started since.
pub fn emit_for_session<S: Serialize + Clone>(
    app: &AppHandle,
    session_id: u64,
    event: &str,
    payload: S,
) {
    if app.state::<CaptureState>().is_latest(session_id) {
        let _ = app.emit(event, payload);
    }
}

#[derive(Clone, Serialize)]
struct SessionPayload {
    session_id: u64,
}

/// A call (live or demo) has started: clear the overlay and bring it up.
#[derive(Clone, Serialize)]
struct SessionStartedPayload {
    session_id: u64,
    mode: &'static str,
}

/// A call (live, solo test, or demo) has started: tell every window and bring up the overlay.
fn started(app: &AppHandle, session_id: u64, mode: &'static str) {
    let _ = app.emit(
        "session-started",
        SessionStartedPayload { session_id, mode },
    );
    crate::overlay::show(app);
}

struct Session {
    id: u64,
    /// The consent covering a call that records audio; `None` for demos, which don't.
    consent_id: Option<u128>,
    /// The detected call app (registry key, display name) this call was started for.
    call_app: Option<(String, String)>,
    stop: Arc<AtomicBool>,
    /// While set, audio is replaced with silence (not transcribed, not billed) and the demo
    /// script holds; the call itself stays open.
    paused: Arc<AtomicBool>,
    /// The rep asking for a hint (`None`) or typing a question, into the session loop.
    ask: UnboundedSender<Option<String>>,
}

#[derive(Clone, Serialize)]
struct LevelPayload {
    channel: Channel,
    /// RMS of the chunk, 0.0–1.0.
    rms: f32,
}

/// Live transcription connection state for one channel, for the UI.
#[derive(Clone, Serialize)]
struct SttStatusPayload {
    channel: Channel,
    state: &'static str,
    message: Option<String>,
    retry_in_secs: Option<u64>,
}

#[derive(Clone, Serialize)]
struct ErrorPayload {
    channel: Channel,
    message: String,
}

#[tauri::command]
pub fn start_capture(
    app: AppHandle,
    consent: State<'_, ConsentState>,
    capture: State<'_, CaptureState>,
    // Testing alone: treat the mic as the prospect so speaking into it triggers hints.
    solo_test: bool,
    // Prospect, company, and agenda entered before the call; all optional.
    brief: Option<CallBrief>,
    // The call app detected when the call was offered, if it was started from that offer.
    call_app: Option<crate::call_detect::DetectedCall>,
    // The start dialog is still reading the CRM record and earlier calls, and will hand them over
    // with `provide_call_history`. Capture starts now; only the opening hints wait for them.
    history_pending: Option<bool>,
) -> Result<u64, String> {
    let Some(consent_id) = consent.valid_id() else {
        return Err(
            "Consent must be acknowledged before capture starts (it is valid for one call, \
                    started within 5 minutes)"
                .into(),
        );
    };
    let mut session = capture.session.lock().expect("capture lock poisoned");
    if session.is_some() {
        return Err("Capture is already running".into());
    }
    let provider = SttProvider::from_env()?;
    let coach_settings = CoachSettings::from_env()?.with_brief(brief.unwrap_or_default());
    let limits = CallLimits::from_env()?;
    let session_started = Instant::now();
    let mode = if solo_test { "solo_test" } else { "live" };

    // From here on the call exists: older calls stop reaching the UI.
    let id = capture.next_id();
    // The log links this capture to its consent before any audio is recorded.
    // Only the call app's audio when Windows supports it (build 20348+), else all system audio.
    let target = match call_app.as_ref().and_then(|c| c.pid) {
        Some(pid) if crate::call_detect::supports_app_audio_capture() => {
            LoopbackTarget::Process(pid)
        }
        _ => LoopbackTarget::DefaultOutput,
    };
    let audio_source = match (&target, &call_app) {
        (LoopbackTarget::Process(_), Some(call)) => format!("{} only", call.app),
        _ => "all system audio".to_string(),
    };
    consent::log_capture_started(&app, consent_id, id, mode, &audio_source)?;
    let failed = |message: String| {
        consent::log_capture_stopped(
            &app,
            consent_id,
            id,
            Some(&format!("failed to start: {message}")),
        );
        message
    };

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    spawn_capture(Channel::Me, target, &stop, &tx).map_err(failed)?;
    if let Err(e) = spawn_capture(Channel::Them, target, &stop, &tx) {
        stop.store(true, Ordering::Relaxed);
        return Err(failed(e));
    }
    drop(tx);
    app.state::<coach::SummaryState>().begin(id);

    // One STT stream per channel keeps "me" and "them" transcripts separate.
    let (stt_tx, stt_rx) = unbounded_channel();
    let (me_tx, me_rx) = unbounded_channel();
    let (them_tx, them_rx) = unbounded_channel();
    provider.spawn_stream(Channel::Me, me_rx, stt_tx.clone());
    provider.spawn_stream(Channel::Them, them_rx, stt_tx);

    let paused = Arc::new(AtomicBool::new(false));
    let auto_stop = AutoStop {
        limits,
        paused: paused.clone(),
        stt_streams: 2,
    };
    let history = history_pending
        .unwrap_or(false)
        .then(|| app.state::<coach::PendingHistory>().expect(id));
    let (pump_coach_tx, ask) = spawn_session_loop(
        &app,
        id,
        coach_settings,
        stt_rx,
        session_started,
        mode,
        auto_stop,
        history,
    );

    // For the call's log: which speaker or headset "them" is recorded from. Its own thread, so
    // reading it never delays capture.
    let device_coach = pump_coach_tx.clone();
    let _ = thread::Builder::new()
        .name("output-device".into())
        .spawn(move || {
            let _ =
                device_coach.send(CoachInput::OutputDevice(crate::audio::default_output_name()));
        });

    let pump_result = thread::Builder::new().name("capture-pump".into()).spawn({
        let app = app.clone();
        let paused = paused.clone();
        // In a solo test the mic plays the prospect, so silence from the call is expected.
        let hearing = HearingCheck::new(!solo_test);
        move || pump(app, id, rx, me_tx, them_tx, pump_coach_tx, paused, hearing)
    });
    if let Err(e) = pump_result {
        stop.store(true, Ordering::Relaxed);
        return Err(failed(e.to_string()));
    }

    // This call has used up the consent it was given.
    consent.reset();
    *session = Some(Session {
        id,
        consent_id: Some(consent_id),
        call_app: call_app.map(|c| (c.key, c.app)),
        stop,
        paused,
        ask,
    });
    started(&app, id, mode);
    Ok(id)
}

/// Plays a scripted sales call instead of capturing audio: no mic, no recording, and no
/// STT usage, so no consent banner is needed. Everything after transcription runs for real.
#[tauri::command]
pub fn start_demo_call(app: AppHandle, capture: State<'_, CaptureState>) -> Result<u64, String> {
    let mut session = capture.session.lock().expect("capture lock poisoned");
    if session.is_some() {
        return Err("A call is already running".into());
    }
    let coach_settings = CoachSettings::from_env()?;
    let limits = CallLimits::from_env()?;
    let session_started = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let id = capture.next_id();
    app.state::<coach::SummaryState>().begin(id);

    let (stt_tx, stt_rx) = unbounded_channel();
    let paused = Arc::new(AtomicBool::new(false));
    let auto_stop = AutoStop {
        limits,
        paused: paused.clone(),
        stt_streams: 0,
    };
    let (_, ask) = spawn_session_loop(
        &app,
        id,
        coach_settings,
        stt_rx,
        session_started,
        "demo",
        auto_stop,
        None,
    );

    let demo_app = app.clone();
    let demo_stop = stop.clone();
    let demo_paused = paused.clone();
    tauri::async_runtime::spawn(async move {
        crate::demo::run(stt_tx, demo_stop, demo_paused).await;
        // Script finished on its own: end the session like Stop would, unless the user
        // already stopped it (or started another).
        let capture = demo_app.state::<CaptureState>();
        let mut session = capture.session.lock().expect("capture lock poisoned");
        if session.as_ref().is_some_and(|s| s.id == id) {
            *session = None;
            drop(session);
            let _ = demo_app.emit("session-ended", SessionPayload { session_id: id });
            crate::overlay::hide(&demo_app);
        }
    });

    *session = Some(Session {
        id,
        consent_id: None,
        call_app: None,
        stop,
        paused,
        ask,
    });
    started(&app, id, "demo");
    Ok(id)
}

/// Runs triggers over incoming transcripts, feeds the coach, and forwards everything to the
/// UI. Returns a coach sender for the capture pump's error reporting.
#[allow(clippy::too_many_arguments)]
fn spawn_session_loop(
    app: &AppHandle,
    session_id: u64,
    coach_settings: CoachSettings,
    mut stt_rx: UnboundedReceiver<SttEvent>,
    session_started: Instant,
    mode: &'static str,
    auto_stop: AutoStop,
    history: Option<tokio::sync::oneshot::Receiver<coach::CallHistory>>,
) -> (UnboundedSender<CoachInput>, UnboundedSender<Option<String>>) {
    let solo_test = mode == "solo_test";
    let (ask_tx, mut ask_rx) = unbounded_channel::<Option<String>>();
    // Spend so far is estimated while the call runs; the session summary has exact figures.
    // Transcription is counted for all unpaused time, an upper bound since silence isn't sent.
    let stt_usd_per_second = coach_settings.stt_price_per_minute().unwrap_or(0.0) / 60.0
        * f64::from(auto_stop.stt_streams);
    let llm_spend = Arc::new(AtomicU64::new(0));
    // The playbook's trigger words (English and Hindi), or the built-in English list.
    let trigger_words = crate::trigger::keywords_from_playbook(coach_settings.playbook());
    let (coach_tx, coach_rx) = unbounded_channel();
    app.state::<coach::CoachInbox>().set(&coach_tx);
    // Ticks off the opening hints as the call covers them, beside the coach, never in its way.
    let (checklist_tx, checklist_rx) = unbounded_channel();
    tauri::async_runtime::spawn(crate::checklist::run(
        app.clone(),
        session_id,
        coach_settings.provider(),
        checklist_rx,
        coach_tx.downgrade(),
    ));
    let coach_checklist = checklist_tx.clone();
    tauri::async_runtime::spawn(coach::run(
        app.clone(),
        session_id,
        coach_settings,
        coach_rx,
        session_started,
        mode,
        llm_spend.clone(),
        history,
        Some(coach_checklist),
    ));

    let returned_tx = coach_tx.clone();
    let stt_app = app.clone();
    // Holds coach_tx: when every transcript source closes this loop ends and, once the
    // capture pump has exited too, the coach finishes and writes the session summary.
    tauri::async_runtime::spawn(async move {
        let mut triggers = TriggerEngine::new(trigger_words);
        let mut tick = tokio::time::interval(TRIGGER_TICK);
        let mut cpu_tick = tokio::time::interval(CPU_SAMPLE_INTERVAL);
        let mut cpu = CpuSampler::new();
        let mut last_speech = Instant::now();
        let mut unpaused = Duration::ZERO;
        let mut warning: Option<String> = None;
        let mut auto_stopped = false;
        loop {
            // A turn end or the silence fallback fires at its deadline, not on the next tick after it.
            let silence_due = triggers.deadline();
            let silence = async move {
                match silence_due {
                    Some(due) => tokio::time::sleep_until(due.into()).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                // The rep asked: fire now, with what was said since the last hint.
                Some(question) = ask_rx.recv() => {
                    let trigger = triggers.ask(question, Instant::now());
                    emit_for_session(&stt_app, session_id, "trigger", &trigger);
                    let _ = coach_tx.send(CoachInput::Trigger(trigger));
                }
                _ = silence => {
                    if let Some(trigger) = triggers.on_tick(Instant::now()) {
                        emit_for_session(&stt_app, session_id, "trigger", &trigger);
                        let _ = coach_tx.send(CoachInput::Trigger(trigger));
                    }
                }
                event = stt_rx.recv() => match event {
                    Some(SttEvent::Transcript(transcript)) => {
                        if !transcript.text.trim().is_empty() {
                            last_speech = Instant::now();
                            // Demo lines have no real audio behind them, so no STT delay to measure.
                            if mode != "demo" {
                                if let Some(captured) = transcript.audio_end_at {
                                    let _ = coach_tx.send(CoachInput::SttLatency {
                                        ms: last_speech.saturating_duration_since(captured).as_millis() as u64,
                                        is_final: transcript.is_final,
                                    });
                                }
                            }
                        }
                        if transcript.is_final {
                            let speaker = if solo_test { Channel::Them } else { transcript.channel };
                            let _ = coach_tx.send(CoachInput::FinalTranscript {
                                channel: transcript.channel,
                                speaker,
                                text: transcript.text.clone(),
                            });
                            let _ = checklist_tx.send(crate::checklist::ChecklistInput::Line(Line {
                                channel: speaker,
                                text: transcript.text.clone(),
                            }));
                        }
                        let trigger = if solo_test && transcript.channel == Channel::Me {
                            let as_prospect = Transcript { channel: Channel::Them, ..transcript.clone() };
                            triggers.on_transcript(&as_prospect, Instant::now())
                        } else {
                            triggers.on_transcript(&transcript, Instant::now())
                        };
                        emit_for_session(&stt_app, session_id, "transcript", transcript);
                        if let Some(trigger) = trigger {
                            emit_for_session(&stt_app, session_id, "trigger", &trigger);
                            let _ = coach_tx.send(CoachInput::Trigger(trigger));
                        }
                    }
                    Some(SttEvent::TurnEnd { channel, .. }) => {
                        // Only the prospect's turn ends a trigger (in a solo test, the mic plays them).
                        if channel == Channel::Them || solo_test {
                            if let Some(trigger) = triggers.on_turn_end(Instant::now()) {
                                emit_for_session(&stt_app, session_id, "trigger", &trigger);
                                let _ = coach_tx.send(CoachInput::Trigger(trigger));
                            }
                        }
                    }
                    Some(SttEvent::Failed { channel, message }) => {
                        let _ = coach_tx.send(CoachInput::StreamError {
                            channel,
                            stage: "stt",
                            message: message.clone(),
                        });
                        emit_for_session(&stt_app, session_id, "capture-error", ErrorPayload { channel, message });
                    }
                    Some(SttEvent::Reconnecting { channel, message, retry_in_secs }) => {
                        let _ = coach_tx.send(CoachInput::StreamError {
                            channel,
                            stage: "stt-reconnecting",
                            message: message.clone(),
                        });
                        emit_for_session(&stt_app, session_id, "stt-status", SttStatusPayload {
                            channel,
                            state: "reconnecting",
                            message: Some(message),
                            retry_in_secs: Some(retry_in_secs),
                        });
                    }
                    Some(SttEvent::Reconnected { channel }) => {
                        emit_for_session(&stt_app, session_id, "stt-status", SttStatusPayload {
                            channel,
                            state: "connected",
                            message: None,
                            retry_in_secs: None,
                        });
                    }
                    Some(SttEvent::Closed { channel, audio_seconds }) => {
                        let _ = coach_tx.send(CoachInput::SttUsage { channel, audio_seconds });
                    }
                    // Both STT streams have closed: the session is over.
                    None => break,
                },
                _ = cpu_tick.tick() => {
                    if let Some(percent) = cpu.as_mut().and_then(CpuSampler::sample) {
                        let _ = coach_tx.send(CoachInput::CpuSample(percent));
                    }
                }
                _ = tick.tick() => {
                    let now = Instant::now();
                    // Time spent paused isn't silence, and isn't billed.
                    if auto_stop.paused.load(Ordering::Relaxed) {
                        last_speech = now;
                    } else {
                        unpaused += TRIGGER_TICK;
                    }
                    if !auto_stopped {
                        let progress = CallProgress {
                            call_length: now.saturating_duration_since(session_started),
                            since_speech: now.saturating_duration_since(last_speech),
                            spent_usd: llm_spend.load(Ordering::Relaxed) as f64 / 1_000_000.0
                                + unpaused.as_secs_f64() * stt_usd_per_second,
                        };
                        if let Some(reason) = auto_stop.limits.stop_reason(progress) {
                            auto_stopped = true;
                            end_call(&stt_app, Some(session_id), Some(reason));
                        } else {
                            let next = auto_stop.limits.warning(progress);
                            // Only on changes: a warning appears once, and clears when speech resumes.
                            if next.is_some() != warning.is_some() {
                                emit_for_session(&stt_app, session_id, "call-ending-soon", CallEndingSoonPayload {
                                    message: next.clone(),
                                });
                            }
                            warning = next;
                        }
                    }
                    if let Some(trigger) = triggers.on_tick(now) {
                        emit_for_session(&stt_app, session_id, "trigger", &trigger);
                        let _ = coach_tx.send(CoachInput::Trigger(trigger));
                    }
                }
            }
        }
    });
    (returned_tx, ask_tx)
}

#[tauri::command]
pub fn stop_capture(app: AppHandle, consent: State<'_, ConsentState>) -> Option<u64> {
    consent.reset();
    let stopped = end_call(&app, None, None);
    // Stop always closes the overlay, even with no call running (reopened with its shortcut, or
    // showing a call-detected prompt): the button is there, and it must do something.
    crate::overlay::hide(&app);
    stopped
}

#[derive(Clone, Serialize)]
struct CallEndingSoonPayload {
    /// Why the call will end automatically soon; `None` clears an earlier warning.
    message: Option<String>,
}

#[derive(Clone, Serialize)]
struct CallStoppedPayload {
    session_id: u64,
    /// Set when the call was ended automatically rather than by the user.
    reason: Option<String>,
}

/// Stops the running call (only if it is `only_session`, when given) and returns its ID.
pub fn end_call(app: &AppHandle, only_session: Option<u64>, reason: Option<String>) -> Option<u64> {
    let capture = app.state::<CaptureState>();
    let session = {
        let mut current = capture.session.lock().expect("capture lock poisoned");
        let running_id = current.as_ref()?.id;
        if only_session.is_some_and(|id| id != running_id) {
            return None;
        }
        current.take()?
    };
    // Capture threads see this within one wait timeout, stop their streams, and exit.
    session.stop.store(true, Ordering::Relaxed);
    if let Some(consent_id) = session.consent_id {
        consent::log_capture_stopped(app, consent_id, session.id, reason.as_deref());
    }
    // Consent covers one call.
    app.state::<ConsentState>().reset();
    // Stop can come from the overlay or the call limits too; every window needs to know.
    let _ = app.emit(
        "call-stopped",
        CallStoppedPayload {
            session_id: session.id,
            reason,
        },
    );
    // The call is over, so the overlay goes now. It used to wait for the post-call summary
    // (10-90 s) plus a linger, so after Stop it stayed up for a minute, or for good when the
    // summary hung. The summary still arrives in the main window.
    crate::overlay::hide(app);
    Some(session.id)
}

/// Pauses (`true`) or resumes (`false`) the running call; every window is told.
#[tauri::command]
pub fn set_call_paused(
    app: AppHandle,
    capture: State<'_, CaptureState>,
    paused: bool,
) -> Result<(), String> {
    let session = capture.session.lock().expect("capture lock poisoned");
    let session = session.as_ref().ok_or("No call is running")?;
    session.paused.store(paused, Ordering::Relaxed);
    let _ = app.emit("call-paused", CallPausedPayload { paused });
    Ok(())
}

#[derive(Clone, Serialize)]
struct CallPausedPayload {
    paused: bool,
}

#[tauri::command]
pub fn capture_status(capture: State<'_, CaptureState>) -> bool {
    capture
        .session
        .lock()
        .expect("capture lock poisoned")
        .is_some()
}

#[cfg(windows)]
fn spawn_capture(
    channel: Channel,
    target: LoopbackTarget,
    stop: &Arc<AtomicBool>,
    tx: &mpsc::Sender<CaptureEvent>,
) -> Result<(), String> {
    crate::audio::wasapi_capture::spawn(channel, target, stop.clone(), tx.clone())
        .map(|_| ())
        .map_err(|e| format!("Could not start capture thread: {e}"))
}

#[cfg(target_os = "macos")]
fn spawn_capture(
    channel: Channel,
    target: LoopbackTarget,
    stop: &Arc<AtomicBool>,
    tx: &mpsc::Sender<CaptureEvent>,
) -> Result<(), String> {
    crate::audio::coreaudio_capture::spawn(channel, target, stop.clone(), tx.clone())
        .map(|_| ())
        .map_err(|e| format!("Could not start capture thread: {e}"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn spawn_capture(
    _channel: Channel,
    _target: LoopbackTarget,
    _stop: &Arc<AtomicBool>,
    _tx: &mpsc::Sender<CaptureEvent>,
) -> Result<(), String> {
    Err("Audio capture is only implemented on Windows so far".into())
}

/// Shown while the rep talks and nothing comes from the prospect's side; `None` clears it.
#[derive(Clone, Serialize)]
struct ProspectNotHeardPayload {
    message: Option<String>,
}

fn not_heard_message(heard_before: bool) -> String {
    if heard_before {
        "Can't hear the prospect for a while. If they're talking, check that the call plays on \
         your usual speaker or headset."
            .into()
    } else {
        "Can't hear the prospect. Check that the call plays on your usual speaker or headset, \
         or no hints can come."
            .into()
    }
}

/// Runs until every capture thread has exited; dropping the STT senders then closes the
/// STT streams.
#[allow(clippy::too_many_arguments)]
fn pump(
    app: AppHandle,
    session_id: u64,
    rx: mpsc::Receiver<CaptureEvent>,
    me: UnboundedSender<SttAudio>,
    them: UnboundedSender<SttAudio>,
    coach: UnboundedSender<CoachInput>,
    paused: Arc<AtomicBool>,
    mut hearing: HearingCheck,
) {
    // Don't bill STT for stretches where nobody is speaking (`STT_SILENCE_GATE=off` sends
    // everything, e.g. to compare transcript accuracy).
    let gate_enabled = !crate::config::env_or_dotenv("STT_SILENCE_GATE")
        .is_some_and(|v| v.eq_ignore_ascii_case("off"));
    let mut gates = [SilenceGate::default(), SilenceGate::default()];
    for event in rx {
        match event {
            CaptureEvent::Chunk(mut chunk) => {
                // Paused: keep the STT streams open with silence, which the STT layer doesn't
                // send (so it isn't billed) while still sending keep-alives.
                let paused_now = paused.load(Ordering::Relaxed);
                if paused_now {
                    chunk.samples.fill(0);
                }
                emit_for_session(
                    &app,
                    session_id,
                    "audio-level",
                    LevelPayload {
                        channel: chunk.channel,
                        rms: rms(&chunk.samples),
                    },
                );
                if !paused_now {
                    if let Some(change) = hearing.on_chunk(chunk.channel, &chunk.samples) {
                        let (message, rep_speech_seconds) = match change {
                            HearingChange::NotHeard {
                                rep_speech,
                                heard_before,
                            } => (
                                Some(not_heard_message(heard_before)),
                                Some(rep_speech.as_secs_f64().round()),
                            ),
                            HearingChange::Heard => (None, None),
                        };
                        let _ = coach.send(CoachInput::ProspectNotHeard { rep_speech_seconds });
                        emit_for_session(
                            &app,
                            session_id,
                            "prospect-not-heard",
                            ProspectNotHeardPayload { message },
                        );
                    }
                }
                let (stt, gate) = match chunk.channel {
                    Channel::Me => (&me, &mut gates[0]),
                    Channel::Them => (&them, &mut gates[1]),
                };
                if gate_enabled && !paused_now {
                    // At speech onset the gate releases the chunks it held before this one; they
                    // were captured one chunk apart, so each keeps its own capture time.
                    let out = gate.process(chunk.samples);
                    let closed = gate.just_closed();
                    let count = out.len();
                    for (i, samples) in out.into_iter().enumerate() {
                        let earlier = duration_of(samples.len()) * (count - 1 - i) as u32;
                        let captured_at = chunk
                            .captured_at
                            .checked_sub(earlier)
                            .unwrap_or(chunk.captured_at);
                        let _ = stt.send(SttAudio {
                            samples,
                            captured_at,
                            ends_speech: closed && i + 1 == count,
                        });
                    }
                } else {
                    let _ = stt.send(SttAudio {
                        samples: chunk.samples,
                        captured_at: chunk.captured_at,
                        ends_speech: false,
                    });
                }
            }
            CaptureEvent::Failed { channel, message } => {
                let _ = coach.send(CoachInput::StreamError {
                    channel,
                    stage: "capture",
                    message: message.clone(),
                });
                emit_for_session(
                    &app,
                    session_id,
                    "capture-error",
                    ErrorPayload { channel, message },
                );
            }
        }
    }
    let _ = coach.send(CoachInput::Audio(hearing.report()));
}

fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let v = f64::from(s) / f64::from(i16::MAX);
            v * v
        })
        .sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// A typed question as the coach gets it: trimmed, at most `MAX_QUESTION_CHARS`; blank means
/// "what should I say now?".
fn clean_question(question: Option<String>) -> Option<String> {
    let question = question?;
    let question = question.trim();
    (!question.is_empty()).then(|| question.chars().take(MAX_QUESTION_CHARS).collect())
}

/// The rep asks the coach for a hint now (`None`: "what should I say?"). From the button or
/// question box (`ask_coach`) and the global shortcut.
pub fn ask(app: &AppHandle, question: Option<String>) -> Result<(), String> {
    let capture = app.state::<CaptureState>();
    let session = capture.session.lock().expect("capture lock poisoned");
    let session = session.as_ref().ok_or("No call is running")?;
    session
        .ask
        .send(clean_question(question))
        .map_err(|_| "The call is ending".to_string())
}

#[tauri::command]
pub fn ask_coach(app: AppHandle, question: Option<String>) -> Result<(), String> {
    ask(&app, question)
}

const MAX_QUESTION_CHARS: usize = 300;

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: CallLimits = CallLimits {
        max_length: Duration::from_secs(180 * 60),
        silence: Duration::from_secs(15 * 60),
        max_cost_usd: 5.0,
    };
    const MINUTE: Duration = Duration::from_secs(60);

    fn at(call_length: Duration, since_speech: Duration, spent_usd: f64) -> CallProgress {
        CallProgress {
            call_length,
            since_speech,
            spent_usd,
        }
    }

    #[test]
    fn calls_within_limits_keep_running() {
        let progress = at(MINUTE * 150, MINUTE * 10, 1.0);
        assert_eq!(LIMITS.stop_reason(progress), None);
        assert_eq!(LIMITS.warning(progress), None);
    }

    #[test]
    fn long_silence_ends_the_call() {
        let reason = LIMITS
            .stop_reason(at(MINUTE * 20, MINUTE * 15, 0.0))
            .unwrap();
        assert_eq!(
            reason,
            "Call ended automatically: no speech for 15 minutes."
        );
    }

    #[test]
    fn max_length_ends_the_call_even_with_speech() {
        let reason = LIMITS
            .stop_reason(at(MINUTE * 180, Duration::ZERO, 0.0))
            .unwrap();
        assert_eq!(
            reason,
            "Call ended automatically: it reached the 180 minute limit."
        );
    }

    #[test]
    fn spend_limit_ends_the_call() {
        let reason = LIMITS
            .stop_reason(at(MINUTE * 30, Duration::ZERO, 5.0))
            .unwrap();
        assert_eq!(
            reason,
            "Call ended automatically: it reached the $5.00 spend limit."
        );
    }

    #[test]
    fn warns_a_minute_before_each_limit() {
        assert!(LIMITS
            .warning(at(MINUTE * 179, Duration::ZERO, 0.0))
            .is_some());
        assert!(LIMITS.warning(at(MINUTE * 20, MINUTE * 14, 0.0)).is_some());
        assert!(LIMITS.warning(at(MINUTE * 20, MINUTE * 13, 0.0)).is_none());
        assert!(LIMITS
            .warning(at(MINUTE * 20, Duration::ZERO, 4.5))
            .is_some());
        // Once the call is stopping, the stop reason replaces the warning.
        assert!(LIMITS
            .warning(at(MINUTE * 180, Duration::ZERO, 0.0))
            .is_none());
    }

    #[test]
    fn short_limits_warn_in_proportion() {
        let short = CallLimits {
            max_length: Duration::from_secs(40),
            ..LIMITS
        };
        assert!(short
            .warning(at(Duration::from_secs(20), Duration::ZERO, 0.0))
            .is_none());
        assert!(short
            .warning(at(Duration::from_secs(30), Duration::ZERO, 0.0))
            .is_some());
    }

    #[test]
    fn fractional_minutes_are_labelled() {
        assert_eq!(minutes_label(Duration::from_secs(45)), "0.75");
    }

    #[test]
    fn typed_questions_are_tidied() {
        assert_eq!(
            clean_question(Some("  How do I handle AWS?  ".into())).as_deref(),
            Some("How do I handle AWS?")
        );
        assert_eq!(clean_question(Some("   ".into())), None);
        assert_eq!(clean_question(None), None);
        let long = "क".repeat(400);
        assert_eq!(
            clean_question(Some(long)).unwrap().chars().count(),
            MAX_QUESTION_CHARS
        );
    }
}
