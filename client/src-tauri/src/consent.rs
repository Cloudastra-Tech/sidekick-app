//! Per-call consent gate. Capture must never start before consent is acknowledged
//! and logged locally — this applies to dev/test builds too.
//!
//! An acknowledgement covers exactly one call: it is used up when that call starts, and
//! expires if no call starts soon after. The log records each acknowledgement and the start
//! and end of the call it covered.

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

pub(crate) const CONSENT_LOG_FILE: &str = "consent_log.jsonl";
/// How long an acknowledgement stays valid for starting a call.
const VALID_FOR: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct ConsentState {
    pending: Mutex<Option<Pending>>,
}

struct Pending {
    consent_id: u128,
    at: Instant,
}

impl ConsentState {
    pub fn is_acknowledged(&self) -> bool {
        self.pending
            .lock()
            .expect("consent lock poisoned")
            .as_ref()
            .is_some_and(|p| p.at.elapsed() < VALID_FOR)
    }

    /// The ID of a valid, unused acknowledgement, if there is one. Doesn't use it up.
    pub fn valid_id(&self) -> Option<u128> {
        let pending = self.pending.lock().expect("consent lock poisoned");
        pending
            .as_ref()
            .filter(|p| p.at.elapsed() < VALID_FOR)
            .map(|p| p.consent_id)
    }

    /// Uses up the acknowledgement: the call it covered has started (or ended).
    pub fn reset(&self) {
        *self.pending.lock().expect("consent lock poisoned") = None;
    }
}

#[derive(Serialize)]
struct ConsentEvent<'a> {
    kind: &'static str,
    consent_id: u128,
    timestamp_unix_ms: u128,
    acknowledged_by: &'a str,
    call_label: &'a str,
    /// The consent text said the transcript is saved for the team. Only these calls' transcripts
    /// leave the laptop; calls confirmed under the older text ("will be transcribed") keep theirs.
    transcript_shared: bool,
}

#[derive(Serialize)]
struct CaptureEvent<'a> {
    kind: &'static str,
    consent_id: u128,
    session_id: u64,
    timestamp_unix_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    /// What "them" audio was captured: one app, or all system audio.
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_source: Option<&'a str>,
    /// The workspace account signed in when the call started, so a call sent later is filed
    /// under whoever made it.
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

#[tauri::command]
pub fn acknowledge_consent(
    app: AppHandle,
    state: State<'_, ConsentState>,
    acknowledged_by: String,
    call_label: String,
) -> Result<(), String> {
    let acknowledged_by = acknowledged_by.trim();
    if acknowledged_by.is_empty() {
        return Err("Enter who is acknowledging consent".into());
    }

    let timestamp_unix_ms = unix_ms();
    let event = ConsentEvent {
        kind: "consent_acknowledged",
        consent_id: timestamp_unix_ms,
        timestamp_unix_ms,
        acknowledged_by,
        call_label: call_label.trim(),
        transcript_shared: true,
    };

    // Log first: consent only counts once it is durably recorded.
    append_to_log(&app, &event).map_err(|e| format!("Could not write consent log: {e}"))?;
    *state.pending.lock().expect("consent lock poisoned") = Some(Pending {
        consent_id: timestamp_unix_ms,
        at: Instant::now(),
    });
    Ok(())
}

/// Records that a call covered by `consent_id` started capturing.
pub fn log_capture_started(
    app: &AppHandle,
    consent_id: u128,
    session_id: u64,
    mode: &str,
    audio_source: &str,
) -> Result<(), String> {
    let event = CaptureEvent {
        kind: "capture_started",
        consent_id,
        session_id,
        timestamp_unix_ms: unix_ms(),
        mode: Some(mode),
        reason: None,
        audio_source: Some(audio_source),
        account: crate::backend::signed_in_email(),
    };
    append_to_log(app, &event).map_err(|e| format!("Could not write consent log: {e}"))
}

/// Records that a live call stopped capturing. Best effort: capture has already stopped.
pub fn log_capture_stopped(
    app: &AppHandle,
    consent_id: u128,
    session_id: u64,
    reason: Option<&str>,
) {
    let event = CaptureEvent {
        kind: "capture_stopped",
        consent_id,
        session_id,
        timestamp_unix_ms: unix_ms(),
        mode: None,
        reason: Some(reason.unwrap_or("stopped by user")),
        audio_source: None,
        account: None,
    };
    if let Err(e) = append_to_log(app, &event) {
        eprintln!("Could not write consent log: {e}");
    }
}

#[tauri::command]
pub fn consent_status(state: State<'_, ConsentState>) -> bool {
    state.is_acknowledged()
}

fn append_to_log<T: Serialize>(
    app: &AppHandle,
    event: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    fs::create_dir_all(&dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(CONSENT_LOG_FILE))?;
    writeln!(file, "{}", serde_json::to_string(event)?)?;
    Ok(())
}
