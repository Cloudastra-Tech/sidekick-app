//! A call's logs from this laptop, sent to the workspace server with the call, so a rep's "no
//! hints" can be checked without asking them for files:
//!
//! - the metrics log: every Claude request with its trigger, timings and tokens, every hint
//!   painted, and the end-of-call numbers (counts and timings only; no transcript text);
//! - the consent log entries for the call: who confirmed, what audio was captured, why it stopped.
//!
//! - the transcript, for admins (and the rep) to review.
//!
//! Calls the server never got (the app closed mid-call, or nobody was signed in) go up the next
//! time the app starts.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::backend::CallForBackend;

/// Which metrics files the server already has, by file name.
const UPLOADED_FILE: &str = "uploaded-call-logs.json";
/// A metrics file written to this recently may belong to a call still running.
const STILL_WRITING: Duration = Duration::from_secs(120);
/// How close a consent log's "capture started" is to the metrics file's own start time.
const SAME_CALL_MS: u128 = 5_000;
/// Calls sent per app start; the rest go next time.
const MAX_PER_START: usize = 100;
/// A saved call ends within this long of its metrics log's last entry (the summary is written
/// after the log's last line).
const SAME_CALL_END_MS: u128 = 10 * 60 * 1000;

fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

/// The session's start, from its metrics file name `session-<unix ms>.jsonl`.
fn session_ms(path: &Path) -> Option<u128> {
    path.file_stem()?
        .to_str()?
        .strip_prefix("session-")?
        .parse()
        .ok()
}

/// Every entry of a metrics log; a line that doesn't parse (a crash mid-write) is skipped.
fn events(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// `2026-10-22T10:02:08.375` (UTC, no zone), as the server stores times.
fn iso_utc(unix_ms: u128) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        unix_ms % 1000
    )
}

fn consent_entries(consent_log: &str) -> Vec<Value> {
    consent_log
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The "capture started" entry written in the same moment as the metrics file `session_ms` names.
fn capture_started(entries: &[Value], session_ms: u128) -> Option<&Value> {
    entries
        .iter()
        .filter(|entry| entry["kind"] == "capture_started")
        .filter_map(|entry| {
            let at = u128::from(entry["timestamp_unix_ms"].as_u64()?);
            Some((at.abs_diff(session_ms), entry))
        })
        .filter(|(gap, _)| *gap <= SAME_CALL_MS)
        .min_by_key(|(gap, _)| *gap)
        .map(|(_, entry)| entry)
}

/// Who was signed in when the call started, if the log says (it does from this version on).
fn call_account(consent_log: &str, session_ms: u128) -> Option<String> {
    let entries = consent_entries(consent_log);
    capture_started(&entries, session_ms)?["account"]
        .as_str()
        .map(str::to_string)
}

/// The consent log's entries for the call whose metrics started at `session_ms`, in the server's
/// shape. Matched by the "capture started" entry written in the same moment.
fn consent_events(consent_log: &str, session_ms: u128) -> Vec<Value> {
    let entries = consent_entries(consent_log);
    let at = |entry: &Value| entry["timestamp_unix_ms"].as_u64().map(u128::from);
    let Some(consent_id) = capture_started(&entries, session_ms)
        .map(|entry| entry["consent_id"].clone())
        // Without an id every id-less entry from every call would match.
        .filter(|id| !id.is_null())
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|entry| entry["consent_id"] == consent_id)
        .filter_map(|entry| {
            let text = |key: &str| entry[key].as_str().unwrap_or_default().to_string();
            // The stop reason rides in `audio_source`'s place: the server keeps one detail field.
            let detail = match entry["kind"].as_str()? {
                "capture_stopped" => text("reason"),
                _ => text("audio_source"),
            };
            Some(json!({
                "event": entry["kind"],
                "acknowledged_by": text("acknowledged_by"),
                "audio_source": detail,
                "at": iso_utc(at(entry)?),
            }))
        })
        .collect()
}

/// This laptop, for telling apart "Windows 11 app-only capture" from "all system audio" calls.
fn client_info(app: &AppHandle) -> Value {
    json!({
        "app_version": app.package_info().version.to_string(),
        "os": std::env::consts::OS,
        "windows_build": crate::call_detect::windows_build(),
    })
}

/// Adds the call's logs to what gets sent: its metrics log, its consent entries, and this
/// laptop's version and OS. Best effort: a call is never held back for a missing log.
pub fn attach(app: &AppHandle, call: &mut CallForBackend) {
    let metrics_file = call
        .metrics
        .as_ref()
        .and_then(|m| m["metrics_file"].as_str())
        .map(PathBuf::from);
    let mut shared = false;
    if let Some(path) = &metrics_file {
        call.events = Some(events(path));
        if let (Some(ms), Some(dir)) = (session_ms(path), data_dir(app)) {
            let log =
                fs::read_to_string(dir.join(crate::consent::CONSENT_LOG_FILE)).unwrap_or_default();
            call.consent_events = Some(consent_events(&log, ms));
            shared = transcript_shared(&log, ms);
        }
    }
    if let Some(metrics) = call.metrics.as_mut().and_then(Value::as_object_mut) {
        metrics.insert("client".into(), client_info(app));
    }
    // Only when everyone on the call was told the transcript is saved for the team.
    if !shared {
        call.transcript = None;
    } else if call.transcript.is_none() {
        call.transcript = saved_transcript(app, &call.call_id);
    }
}

/// Whether the call's consent was confirmed under the text saying its transcript is saved for
/// the team. Calls from before that text, or with no consent entry found, say no.
fn transcript_shared(consent_log: &str, session_ms: u128) -> bool {
    let entries = consent_entries(consent_log);
    let Some(consent_id) = capture_started(&entries, session_ms)
        .map(|entry| entry["consent_id"].clone())
        .filter(|id| !id.is_null())
    else {
        return false;
    };
    entries.iter().any(|entry| {
        entry["kind"] == "consent_acknowledged"
            && entry["consent_id"] == consent_id
            && entry["transcript_shared"] == true
    })
}

/// The transcript from the call's saved record, `calls/<call id>.json`.
fn saved_transcript(app: &AppHandle, call_id: &str) -> Option<Vec<Value>> {
    if !call_id.starts_with("call-") || !call_id[5..].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let path = crate::history::calls_dir(app)
        .ok()?
        .join(format!("{call_id}.json"));
    let record: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    record["transcript"].as_array().cloned()
}

fn uploaded(app: &AppHandle) -> BTreeSet<String> {
    data_dir(app)
        .and_then(|dir| fs::read_to_string(dir.join(UPLOADED_FILE)).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Whether the server stores call logs. An older one would take the call but drop its logs, and
/// marking it sent would lose them for good. Asking for an unknown call's logs tells them apart:
/// a server with the route says it has no such call, an older one has no such route.
pub async fn server_keeps_logs() -> bool {
    // A server that keeps logs doesn't stop, so one yes lasts the app's run; a no is asked again.
    static KEEPS: AtomicBool = AtomicBool::new(false);
    if KEEPS.load(Ordering::Relaxed) {
        return true;
    }
    let keeps =
        match crate::backend::call_backend(reqwest::Method::GET, "/calls/-/logs", None).await {
            Ok(_) => true,
            Err(detail) => detail.contains("No such call"),
        };
    KEEPS.store(keeps, Ordering::Relaxed);
    keeps
}

fn save_uploaded(app: &AppHandle, done: &BTreeSet<String>) {
    if let Some(dir) = data_dir(app) {
        let _ = fs::write(
            dir.join(UPLOADED_FILE),
            serde_json::to_string(done).unwrap_or_default(),
        );
    }
}

/// Remembers that the server has this metrics file's call.
pub fn mark_uploaded(app: &AppHandle, metrics_file: &str) {
    let Some(name) = Path::new(metrics_file).file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let mut done = uploaded(app);
    if done.insert(name.to_string()) {
        save_uploaded(app, &done);
    }
}

/// A call saved on this laptop: its id, when it ended, which metrics file it names (older
/// records and recovered calls name none), and the record.
struct SavedCall {
    call_id: String,
    ended_ms: u128,
    metrics_name: Option<String>,
    record: Value,
}

fn saved_calls(app: &AppHandle) -> Vec<SavedCall> {
    let Ok(dir) = crate::history::calls_dir(app) else {
        return Vec::new();
    };
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let call_id = path.file_stem()?.to_str()?.to_string();
            let ended_ms = call_id.strip_prefix("call-")?.parse().ok()?;
            if path.extension()? != "json" {
                return None;
            }
            let record: Value = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
            let metrics_name = record["stats"]["metrics_file"]
                .as_str()
                .and_then(|file| Path::new(file).file_name()?.to_str().map(str::to_string));
            Some(SavedCall {
                call_id,
                ended_ms,
                metrics_name,
                record,
            })
        })
        .collect()
}

/// The saved call a metrics file belongs to: the one naming it, else one that names no file
/// and ended while that log was being written. Without the second, such a call would go up
/// twice, once as `call-…` and once as `session-…`, and count twice in a rep's numbers.
fn saved_call_for<'a>(
    saved: &'a [SavedCall],
    name: &str,
    session_ms: u128,
    log: &[Value],
) -> Option<&'a SavedCall> {
    if let Some(found) = saved
        .iter()
        .find(|s| s.metrics_name.as_deref() == Some(name))
    {
        return Some(found);
    }
    let last = log
        .iter()
        .filter_map(|e| e["at_unix_ms"].as_f64())
        .fold(session_ms as f64, f64::max) as u128;
    saved.iter().find(|s| {
        s.metrics_name.is_none()
            && s.ended_ms >= session_ms
            && s.ended_ms <= last + SAME_CALL_END_MS
    })
}

/// The call as the server takes it, from its saved record when there is one, else from its
/// metrics log alone (a call the app closed during).
fn call_from(
    metrics_path: &Path,
    record: Option<(&String, &Value)>,
    events: &[Value],
) -> CallForBackend {
    let summary = events.iter().rev().find(|e| e["kind"] == "session_summary");
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let id_of = |value: &Value| value.as_str().map(str::to_string);
    match record {
        Some((call_id, record)) => CallForBackend {
            call_id: call_id.clone(),
            prospect: text(&record["brief"]["prospect"]),
            company: text(&record["brief"]["company"]),
            mode: text(&record["mode"]),
            duration_minutes: record["stats"]["duration_minutes"]
                .as_f64()
                .unwrap_or_default(),
            cost_usd: record["stats"]["cost_usd"]["total"].as_f64(),
            crm_record_url: None,
            crm_person_id: id_of(&record["brief"]["crm_person_id"]),
            crm_company_id: id_of(&record["brief"]["crm_company_id"]),
            summary: (record["call"]["status"] == "ready")
                .then(|| record["call"]["summary"].clone()),
            metrics: Some(record["stats"].clone()).filter(Value::is_object),
            events: None,
            consent_events: None,
            transcript: record["transcript"].as_array().cloned(),
        },
        None => {
            let at = |e: &Value| e["at_unix_ms"].as_f64();
            let span = match (events.first().and_then(at), events.last().and_then(at)) {
                (Some(first), Some(last)) => (last - first) / 60_000.0,
                _ => 0.0,
            };
            CallForBackend {
                call_id: metrics_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("session")
                    .to_string(),
                prospect: String::new(),
                company: String::new(),
                mode: summary
                    .map(|s| text(&s["mode"]))
                    .unwrap_or_else(|| "live".into()),
                duration_minutes: summary
                    .and_then(|s| s["duration_minutes"].as_f64())
                    .unwrap_or(span),
                cost_usd: None,
                crm_record_url: None,
                crm_person_id: None,
                crm_company_id: None,
                summary: None,
                // Without an end-of-call entry the app closed mid-call; say so, and keep where
                // the log is so `attach` finds it.
                metrics: Some(match summary {
                    Some(s) => s.clone(),
                    None => {
                        json!({ "note": "the app closed during this call", "metrics_file": metrics_path.display().to_string() })
                    }
                }),
                events: None,
                consent_events: None,
                transcript: None,
            }
        }
    }
}

/// Sends every call on this laptop the server doesn't have yet. Runs once at start-up; quiet on
/// failure, since the next start tries again.
pub async fn upload_missing(app: AppHandle) {
    if !crate::backend::signed_in() || !server_keeps_logs().await {
        return;
    }
    let Some(dir) = data_dir(&app).map(|d| d.join("metrics")) else {
        return;
    };
    let mut done = uploaded(&app);
    let saved = saved_calls(&app);
    let me = crate::backend::signed_in_email();
    let consent_log = data_dir(&app)
        .and_then(|dir| fs::read_to_string(dir.join(crate::consent::CONSENT_LOG_FILE)).ok())
        .unwrap_or_default();
    let now = SystemTime::now();
    let mut pending: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| session_ms(path).is_some())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let settled = fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| now.duration_since(at).ok())
                .is_some_and(|age| age > STILL_WRITING);
            !done.contains(name) && settled
        })
        .collect();
    pending.sort();
    for path in pending.into_iter().rev().take(MAX_PER_START) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let Some(ms) = session_ms(&path) else {
            continue;
        };
        // A call made under another account goes up when that account signs in here again,
        // not under whoever is signed in now. Calls from before the log said are taken as the
        // laptop owner's.
        if let (Some(account), Some(me)) = (call_account(&consent_log, ms), me.as_deref()) {
            if !account.eq_ignore_ascii_case(me) {
                continue;
            }
        }
        let log = events(&path);
        if log.is_empty() {
            done.insert(name);
            continue;
        }
        let record = saved_call_for(&saved, &name, ms, &log).map(|s| (&s.call_id, &s.record));
        let mut call = call_from(&path, record, &log);
        if let Some(metrics) = call.metrics.as_mut().and_then(Value::as_object_mut) {
            metrics.insert("metrics_file".into(), json!(path.display().to_string()));
        }
        attach(&app, &mut call);
        match crate::backend::send_call(&call).await {
            Ok(_) => {
                done.insert(name);
            }
            Err(error) => {
                eprintln!("Could not upload the logs of {name}: {error}");
                break;
            }
        }
    }
    save_uploaded(&app, &done);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_utc_times_the_server_reads() {
        assert_eq!(iso_utc(1_792_663_360_405), "2026-10-22T10:02:40.405");
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000");
    }

    #[test]
    fn finds_the_calls_consent_entries_by_its_start() {
        let log = [
            r#"{"kind":"consent_acknowledged","consent_id":1,"timestamp_unix_ms":1000,"acknowledged_by":"Old","call_label":"x"}"#,
            r#"{"kind":"capture_started","consent_id":1,"session_id":1,"timestamp_unix_ms":1010,"mode":"live","audio_source":"all system audio"}"#,
            r#"{"kind":"consent_acknowledged","consent_id":9,"timestamp_unix_ms":90000,"acknowledged_by":"Priya","call_label":"Ravi"}"#,
            r#"{"kind":"capture_started","consent_id":9,"session_id":1,"timestamp_unix_ms":90010,"mode":"live","audio_source":"Google Chrome only"}"#,
            r#"{"kind":"capture_stopped","consent_id":9,"session_id":1,"timestamp_unix_ms":120000,"reason":"stopped by user"}"#,
        ]
        .join("\n");
        let found = consent_events(&log, 90_011);
        let kinds: Vec<_> = found.iter().map(|e| e["event"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            ["consent_acknowledged", "capture_started", "capture_stopped"]
        );
        assert_eq!(found[0]["acknowledged_by"], "Priya");
        assert_eq!(found[1]["audio_source"], "Google Chrome only");
        assert_eq!(found[2]["audio_source"], "stopped by user");
        assert!(
            consent_events(&log, 500_000).is_empty(),
            "no capture near that start"
        );
    }

    #[test]
    fn only_calls_consented_under_the_new_text_share_their_transcript() {
        let log = [
            r#"{"kind":"consent_acknowledged","consent_id":1,"timestamp_unix_ms":1000,"acknowledged_by":"Priya"}"#,
            r#"{"kind":"capture_started","consent_id":1,"timestamp_unix_ms":1010}"#,
            r#"{"kind":"consent_acknowledged","consent_id":2,"timestamp_unix_ms":90000,"acknowledged_by":"Priya","transcript_shared":true}"#,
            r#"{"kind":"capture_started","consent_id":2,"timestamp_unix_ms":90010}"#,
        ]
        .join("\n");
        assert!(
            !transcript_shared(&log, 1_011),
            "told only 'will be transcribed'"
        );
        assert!(transcript_shared(&log, 90_011));
        assert!(!transcript_shared(&log, 500_000), "no consent entry found");
    }

    #[test]
    fn a_capture_without_a_consent_id_matches_nothing() {
        let log = [
            r#"{"kind":"consent_acknowledged","timestamp_unix_ms":1000,"acknowledged_by":"Old"}"#,
            r#"{"kind":"capture_started","timestamp_unix_ms":1010,"mode":"live"}"#,
            r#"{"kind":"capture_stopped","timestamp_unix_ms":9000,"reason":"stopped by user"}"#,
        ]
        .join("\n");
        assert!(consent_events(&log, 1_011).is_empty());
    }

    #[test]
    fn the_account_comes_from_the_calls_capture_entry() {
        let log = [
            r#"{"kind":"capture_started","consent_id":1,"timestamp_unix_ms":1010,"account":"priya@example.com"}"#,
            r#"{"kind":"capture_started","consent_id":2,"timestamp_unix_ms":90010}"#,
        ]
        .join("\n");
        assert_eq!(
            call_account(&log, 1_011).as_deref(),
            Some("priya@example.com")
        );
        assert_eq!(
            call_account(&log, 90_011),
            None,
            "older entries name nobody"
        );
    }

    #[test]
    fn a_saved_call_naming_no_metrics_file_is_matched_by_when_it_ended() {
        let saved = vec![
            SavedCall {
                call_id: "call-5000".into(),
                ended_ms: 5_000,
                metrics_name: None,
                record: json!({}),
            },
            SavedCall {
                call_id: "call-99000000".into(),
                ended_ms: 99_000_000,
                metrics_name: None,
                record: json!({}),
            },
        ];
        let log = vec![json!({ "at_unix_ms": 4_000.0 })];
        let found = saved_call_for(&saved, "session-1000.jsonl", 1_000, &log);
        assert_eq!(found.map(|s| s.call_id.as_str()), Some("call-5000"));
        assert!(saved_call_for(&saved, "session-1000.jsonl", 200_000_000, &log).is_none());
    }

    #[test]
    fn a_call_without_a_saved_record_is_built_from_its_log() {
        let path = Path::new("C:/x/metrics/session-1792663360405.jsonl");
        let log = vec![
            json!({ "kind": "opening_hints", "at_unix_ms": 0.0 }),
            json!({ "kind": "llm_call", "at_unix_ms": 90_000.0 }),
        ];
        let call = call_from(path, None, &log);
        assert_eq!(call.call_id, "session-1792663360405");
        assert!((call.duration_minutes - 1.5).abs() < 1e-9);
        assert_eq!(
            call.metrics.unwrap()["note"],
            "the app closed during this call"
        );
    }
}
