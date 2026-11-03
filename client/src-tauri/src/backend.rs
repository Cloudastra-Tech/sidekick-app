//! Talking to the Sidekick backend.
//!
//! What it does today: sign in, pull the workspace's settings and playbook so a fresh install is
//! configured by logging in, and send each finished call (summary, numbers, consent events).
//! Transcripts stay on this machine.
//!
//! The backend is optional. With no URL set, or signed out, the app works exactly as before:
//! settings come from `settings.json` and calls stay local. A failed push never blocks a call.

use crate::config::{app_data_dir, env_or_dotenv, saved_settings, settings_path};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Deepgram keys the backend mints last a few hours; ask for a new one well before that.
const STT_KEY_MAX_AGE: Duration = Duration::from_secs(2 * 60 * 60);
/// The short-lived Deepgram key for this session, and when it arrived.
static STT_KEY: Mutex<Option<(String, Instant)>> = Mutex::new(None);
/// The workspace CRM's address, remembered so a record link can be built after a write.
static CRM_BASE_URL: Mutex<Option<String>> = Mutex::new(None);
const SESSION_FILE: &str = "backend-session.json";
/// The workspace playbook, written beside the app's data rather than over the shipped copy.
const WORKSPACE_PLAYBOOK_FILE: &str = "workspace-playbook.md";

/// The workspace server an installed copy talks to. An installed app has no `.env`, so the
/// release build compiles in the `SIDEKICK_BACKEND_URL` it was built with. A build without one
/// stays fully local unless `SIDEKICK_BACKEND_URL` is set when it runs.
const DEFAULT_BACKEND_URL: Option<&str> = option_env!("SIDEKICK_BACKEND_URL");

/// Where the backend lives. Set `SIDEKICK_BACKEND_URL` to an empty value to stay fully local.
fn base_url() -> Option<String> {
    env_or_dotenv("SIDEKICK_BACKEND_URL")
        .or_else(|| DEFAULT_BACKEND_URL.map(str::to_string))
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

fn session_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join(SESSION_FILE))
}

/// The signed-in user, as the app remembers them between restarts.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Session {
    token: String,
    email: String,
    workspace_name: String,
    #[serde(default)]
    is_admin: bool,
}

fn session() -> Option<Session> {
    let text = fs::read_to_string(session_path()?).ok()?;
    serde_json::from_str::<Session>(&text)
        .ok()
        .filter(|s| !s.token.is_empty())
}

fn save_session(session: Option<&Session>) -> Result<(), String> {
    let path = session_path().ok_or("The app data folder isn't available")?;
    match session {
        None => {
            let _ = fs::remove_file(path);
            Ok(())
        }
        Some(session) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let text = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
            fs::write(path, text).map_err(|e| format!("Could not save the sign-in: {e}"))
        }
    }
}

/// Longest a new connection may take. TLS to the backend through Cloudflare measured under
/// 0.6 s, so a connect still going after this is stuck, and trying again beats waiting.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long an unused connection is kept. Between hints a call can go quiet for minutes (a demo,
/// the prospect reading); reqwest's default of 90 s meant the next hint reconnected.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .tcp_keepalive(Duration::from_secs(30))
        // HTTP/2 pings keep an idle connection open through Cloudflare, and find a dead one
        // before a hint is sent down it instead of after.
        .http2_keep_alive_interval(Duration::from_secs(20))
        .http2_keep_alive_timeout(Duration::from_secs(5))
        .http2_keep_alive_while_idle(true)
        .build()
        .map_err(|e| e.to_string())
}

/// One client for the app's whole life: backend reads, the CRM and every Claude request share its
/// connections. Built fresh per call, signing in meant three handshakes back to back — login,
/// settings, transcription key — and each call's first hint paid for its own; sharing it, the
/// reads made while the start dialog is open leave a warm connection for the first hint.
static HTTP: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(build_client);
/// A second pool, for a hint request sent again when the first seems stuck: a request repeated on
/// the same HTTP/2 connection would be stuck behind the same problem.
static HEDGE_HTTP: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(build_client);

pub(crate) fn client() -> Result<reqwest::Client, String> {
    // Cloning a client shares the same connection pool; it is the documented way to pass it on.
    HTTP.clone()
}

pub(crate) fn hedge_client() -> Result<reqwest::Client, String> {
    HEDGE_HTTP.clone()
}

/// Is a backend configured, and are we signed in to it?
#[tauri::command]
pub fn backend_status() -> Value {
    let session = session();
    json!({
        "url": base_url(),
        "signed_in": session.is_some(),
        "email": session.as_ref().map(|s| s.email.clone()),
        "workspace": session.as_ref().map(|s| s.workspace_name.clone()),
        "is_admin": session.as_ref().is_some_and(|s| s.is_admin),
    })
}

#[derive(Deserialize)]
struct LoginResponse {
    access_token: String,
    user: LoginUser,
}

#[derive(Deserialize)]
struct LoginUser {
    email: String,
    workspace_name: String,
    is_admin: bool,
}

/// Signs in and remembers the token, then pulls the workspace's settings.
#[tauri::command]
pub async fn backend_login(email: String, password: String) -> Result<Value, String> {
    let url = base_url().ok_or("No backend URL is set (SIDEKICK_BACKEND_URL)")?;
    let response = client()?
        .post(format!("{url}/auth/login"))
        .json(&json!({ "email": email, "password": password }))
        .send()
        .await
        .map_err(|e| format!("The backend didn't answer: {e}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("That email and password don't match".into());
    }
    if !response.status().is_success() {
        return Err(format!("Signing in failed ({})", response.status()));
    }
    let login: LoginResponse = response
        .json()
        .await
        .map_err(|e| format!("The backend sent something unreadable: {e}"))?;
    save_session(Some(&Session {
        token: login.access_token,
        email: login.user.email,
        workspace_name: login.user.workspace_name,
        is_admin: login.user.is_admin,
    }))?;
    // A fresh install should be ready to work right after signing in.
    let settings = pull_settings().await?;
    // Best-effort: a workspace whose Deepgram key can't mint short-lived keys still signs in,
    // and the Settings screen says so.
    if let Err(problem) = refresh_stt_key().await {
        eprintln!("No transcription key from the backend: {problem}");
    }
    Ok(json!({ "status": backend_status(), "settings": settings }))
}

#[tauri::command]
pub fn backend_logout() -> Result<(), String> {
    *STT_KEY.lock().expect("stt key lock poisoned") = None;
    save_session(None)
}

#[derive(Deserialize)]
struct WorkspaceSettings {
    language: String,
    max_call_minutes: f64,
    auto_stop_silence_minutes: f64,
    max_call_cost_usd: f64,
    call_detection: bool,
    deepgram_mip_opt_out: bool,
    playbook: String,
    playbook_version: i64,
}

/// Fetches the workspace's settings and writes them into the local settings and playbook, so the
/// rest of the app reads them the way it always has.
#[tauri::command]
pub async fn backend_sync_settings() -> Result<Value, String> {
    pull_settings().await
}

async fn pull_settings() -> Result<Value, String> {
    let url = base_url().ok_or("No backend URL is set (SIDEKICK_BACKEND_URL)")?;
    let session = session().ok_or("Sign in to the backend first")?;
    let response = client()?
        .get(format!("{url}/settings"))
        .bearer_auth(&session.token)
        .send()
        .await
        .map_err(|e| format!("The backend didn't answer: {e}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        save_session(None)?;
        return Err("The sign-in expired; sign in again".into());
    }
    let settings: WorkspaceSettings = response
        .json()
        .await
        .map_err(|e| format!("The backend sent something unreadable: {e}"))?;

    let mut saved = saved_settings();
    saved.insert("DEEPGRAM_LANGUAGE".into(), json!(settings.language));
    saved.insert(
        "CALL_MAX_MINUTES".into(),
        json!(settings.max_call_minutes.to_string()),
    );
    saved.insert(
        "CALL_AUTO_STOP_SILENCE_MINUTES".into(),
        json!(settings.auto_stop_silence_minutes.to_string()),
    );
    saved.insert(
        "CALL_MAX_COST_USD".into(),
        json!(settings.max_call_cost_usd.to_string()),
    );
    saved.insert(
        "CALL_DETECTION".into(),
        json!(if settings.call_detection { "on" } else { "off" }),
    );
    saved.insert(
        "DEEPGRAM_MIP_OPT_OUT".into(),
        json!(if settings.deepgram_mip_opt_out {
            "on"
        } else {
            "off"
        }),
    );
    // The workspace's playbook goes next to the app's other data, never over the copy shipped
    // with the app, and `PLAYBOOK_PATH` points the coach at it.
    if settings.playbook.trim().len() >= 200 {
        let playbook_path = app_data_dir()
            .ok_or("The app data folder isn't available")?
            .join(WORKSPACE_PLAYBOOK_FILE);
        fs::write(&playbook_path, &settings.playbook)
            .map_err(|e| format!("Could not save the playbook: {e}"))?;
        saved.insert(
            "PLAYBOOK_PATH".into(),
            json!(playbook_path.display().to_string()),
        );
    }

    let path = settings_path().ok_or("The app data folder isn't available")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(saved)).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Could not save the settings: {e}"))?;

    Ok(json!({
        "language": settings.language,
        "playbook_version": settings.playbook_version,
        "applied_to": path.display().to_string(),
    }))
}

/// A finished call, as the backend takes it. The same shape the CRM write uses, plus the numbers.
#[derive(Deserialize, Serialize)]
pub struct CallForBackend {
    pub call_id: String,
    #[serde(default)]
    pub prospect: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub duration_minutes: f64,
    pub cost_usd: Option<f64>,
    pub crm_record_url: Option<String>,
    /// The CRM person the call was with, so their next call can find this one's summary.
    #[serde(default)]
    pub crm_person_id: Option<String>,
    /// Their company, so a call with a colleague of theirs finds this one too.
    #[serde(default)]
    pub crm_company_id: Option<String>,
    pub summary: Option<Value>,
    pub metrics: Option<Value>,
    /// The call's metrics log and consent entries, added here from the laptop's files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_events: Option<Vec<Value>>,
    /// What was said, from the call's saved record, for admins to review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Vec<Value>>,
}

/// Whether there's a workspace server to send to and someone signed in to it.
pub fn signed_in() -> bool {
    base_url().is_some() && session().is_some()
}

/// Who is signed in, for knowing whose a recorded call is.
pub fn signed_in_email() -> Option<String> {
    session().map(|s| s.email).filter(|e| !e.is_empty())
}

/// Sends a finished call, with its logs from this laptop. Sending the same call again updates
/// it, so a retry is safe.
#[tauri::command]
pub async fn push_call_to_backend(
    app: tauri::AppHandle,
    mut call: CallForBackend,
) -> Result<Value, String> {
    crate::call_logs::attach(&app, &mut call);
    let sent = send_call(&call).await?;
    // Only once the server keeps logs; an older one dropped them, so the call goes again later.
    if let Some(file) = call
        .metrics
        .as_ref()
        .and_then(|m| m["metrics_file"].as_str())
    {
        if crate::call_logs::server_keeps_logs().await {
            crate::call_logs::mark_uploaded(&app, file);
        }
    }
    Ok(sent)
}

pub(crate) async fn send_call(call: &CallForBackend) -> Result<Value, String> {
    let url = base_url().ok_or("No backend URL is set (SIDEKICK_BACKEND_URL)")?;
    let session = session().ok_or("Sign in to the backend first")?;
    let response = client()?
        .post(format!("{url}/calls"))
        .bearer_auth(&session.token)
        .json(&call)
        .send()
        .await
        .map_err(|e| format!("The backend didn't answer: {e}"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        save_session(None)?;
        return Err("The sign-in expired; sign in again".into());
    }
    if !response.status().is_success() {
        return Err(format!(
            "The backend refused the call ({})",
            response.status()
        ));
    }
    response
        .json()
        .await
        .map_err(|e| format!("The backend sent something unreadable: {e}"))
}

/// Calls from the server, newest first, with their summary and numbers: the whole team's for an
/// admin, the rep's own otherwise (the server decides). Transcripts come separately.
#[tauri::command]
pub async fn backend_team_calls() -> Result<Value, String> {
    call_backend(reqwest::Method::GET, "/calls?limit=200", None).await
}

/// What was said on a call, for an admin or the rep who made it.
#[tauri::command]
pub async fn backend_call_transcript(call_id: String) -> Result<Value, String> {
    let mut url = reqwest::Url::parse("http://backend/calls").expect("static url");
    url.path_segments_mut()
        .expect("http url")
        .push(&call_id)
        .push("transcript");
    call_backend(reqwest::Method::GET, url.path(), None).await
}

/// Each rep's numbers over the last `days` days (admins only).
#[tauri::command]
pub async fn backend_rep_stats(days: u32) -> Result<Value, String> {
    call_backend(
        reqwest::Method::GET,
        &format!("/insights/reps?days={}", days.clamp(1, 365)),
        None,
    )
    .await
}

/// The prospect's last few summarized calls with anyone in the workspace, as text for the coach's
/// prompt; empty when there are none. Found by CRM person when the rep picked one, else by name.
#[tauri::command]
pub async fn backend_previous_calls(
    crm_person_id: Option<String>,
    crm_company_id: Option<String>,
    prospect: String,
    company: String,
) -> Result<String, String> {
    let mut url = reqwest::Url::parse("http://backend/calls/previous").expect("static url");
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("prospect", prospect.trim());
        query.append_pair("company", company.trim());
        if let Some(id) = crm_person_id.as_deref().filter(|id| !id.is_empty()) {
            query.append_pair("crm_person_id", id);
        }
        // Earlier calls with anyone else at the same company count too.
        if let Some(id) = crm_company_id.as_deref().filter(|id| !id.is_empty()) {
            query.append_pair("crm_company_id", id);
        }
    }
    let path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
    let calls = call_backend(reqwest::Method::GET, &path, None).await?;
    Ok(previous_calls_prompt(&calls))
}

/// One block per call, newest first, keeping only what the summary actually found: the parts that
/// say where the deal was left (open commitments, objections, the agreed next step).
fn previous_calls_prompt(calls: &Value) -> String {
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("Not discussed"))
            .map(str::to_string)
    };
    let list = |value: &Value, item: &dyn Fn(&Value) -> Option<String>| {
        let items: Vec<String> = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(item)
            .collect();
        (!items.is_empty()).then(|| items.join("; "))
    };
    let blocks: Vec<String> = calls
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|call| {
            let summary = call.get("summary").filter(|s| s.is_object())?;
            let date = call["created_at"].as_str().and_then(|at| at.get(..10));
            let by = text(&call["user_email"]);
            let heading = match (date, by) {
                (Some(date), Some(by)) => format!("Call on {date} (rep: {by})"),
                (Some(date), None) => format!("Call on {date}"),
                (None, Some(by)) => format!("Call (rep: {by})"),
                (None, None) => "Call".to_string(),
            };
            let qualification = &summary["qualification"];
            let fields = [
                ("Overview", text(&summary["overview"])),
                ("Their needs", list(&summary["prospect"]["needs"], &text)),
                ("Pain points", list(&summary["pain_points"], &text)),
                (
                    "Objections",
                    list(&summary["objections"], &|o| {
                        let objection = text(&o["objection"])?;
                        Some(match text(&o["response"]) {
                            Some(response) => format!("{objection} (we answered: {response})"),
                            None => objection,
                        })
                    }),
                ),
                ("Budget", text(&qualification["budget"])),
                ("Timeline", text(&qualification["timeline"])),
                ("Decision process", text(&qualification["decision_process"])),
                (
                    "Commitments",
                    list(&summary["commitments"], &|c| {
                        let who = if c["owner"] == "us" { "we" } else { "they" };
                        Some(format!("{who}: {}", text(&c["action"])?))
                    }),
                ),
                ("Agreed next step", text(&summary["next_step"])),
            ];
            let lines: Vec<String> = fields
                .into_iter()
                .filter_map(|(label, value)| Some(format!("- {label}: {}", value?)))
                .collect();
            (!lines.is_empty()).then(|| format!("{heading}\n{}", lines.join("\n")))
        })
        .collect();
    blocks.join("\n\n")
}

/// Sends a request to the backend with the stored token, returning the body as JSON.
pub(crate) async fn call_backend(
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = base_url().ok_or("No backend URL is set (SIDEKICK_BACKEND_URL)")?;
    let session = session().ok_or("Sign in to the backend first")?;
    let mut request = client()?
        .request(method, format!("{url}{path}"))
        .bearer_auth(&session.token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    read_backend_answer(request).await
}

/// Sends a file to the backend as a form upload, with the same sign-in and errors as
/// `call_backend`.
pub(crate) async fn upload_to_backend(
    path: &str,
    form: reqwest::multipart::Form,
) -> Result<Value, String> {
    let url = base_url().ok_or("No backend URL is set (SIDEKICK_BACKEND_URL)")?;
    let session = session().ok_or("Sign in to the backend first")?;
    let request = client()?
        .post(format!("{url}{path}"))
        .bearer_auth(&session.token)
        .multipart(form);
    read_backend_answer(request).await
}

async fn read_backend_answer(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let response = request
        .send()
        .await
        .map_err(|e| format!("The backend didn't answer: {e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        save_session(None)?;
        return Err("The sign-in expired; sign in again".into());
    }
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("detail")
                    .map(|d| d.as_str().unwrap_or_default().to_string())
            })
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("The backend refused that ({status})"));
        return Err(detail);
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| format!("The backend sent something unreadable: {e}"))
}

/// Where Claude requests should go: the backend, when signed in, so the laptop never holds the
/// Anthropic key and the backend can enforce the user's spend limit.
pub fn llm_route() -> Option<(String, String)> {
    Some((base_url()?, session()?.token))
}

/// Does the workspace have a CRM, and at what address? For the Send to CRM panel.
pub async fn crm_workspace_status() -> Result<(bool, String), String> {
    let body = call_backend(reqwest::Method::GET, "/crm/status", None).await?;
    let base = body
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    if !base.is_empty() {
        *CRM_BASE_URL.lock().expect("crm base url lock poisoned") = Some(base.clone());
    }
    Ok((
        body.get("configured")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        base,
    ))
}

/// The workspace CRM's own address, for linking to a record after a write.
pub fn crm_base_url() -> Option<String> {
    CRM_BASE_URL
        .lock()
        .expect("crm base url lock poisoned")
        .clone()
}

/// The short-lived Deepgram key, if one has been fetched and is still fresh.
pub fn cached_stt_key() -> Option<String> {
    let cached = STT_KEY.lock().expect("stt key lock poisoned");
    cached
        .as_ref()
        .filter(|(_, at)| at.elapsed() < STT_KEY_MAX_AGE)
        .map(|(key, _)| key.clone())
}

/// Asks the backend for a Deepgram key for this person's calls. Best-effort: when it fails the
/// app falls back to a key in the environment, which is how a machine without a backend works.
pub async fn refresh_stt_key() -> Result<(), String> {
    if base_url().is_none() || session().is_none() {
        return Ok(());
    }
    let body = call_backend(reqwest::Method::POST, "/stt/key", None).await?;
    let key = body
        .get("api_key")
        .and_then(Value::as_str)
        .ok_or("The backend didn't return a transcription key")?;
    *STT_KEY.lock().expect("stt key lock poisoned") = Some((key.to_string(), Instant::now()));
    Ok(())
}

/// A key older than this is replaced in the background. Keys last 4 h and calls are capped at
/// 3 h, so a call started on a key under an hour old can't outlive it.
const STT_KEY_REFRESH_AGE: Duration = Duration::from_secs(45 * 60);
/// How often the background refresh checks the key's age.
const STT_KEY_CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);

fn stt_key_is_fresh() -> bool {
    STT_KEY
        .lock()
        .expect("stt key lock poisoned")
        .as_ref()
        .is_some_and(|(_, at)| at.elapsed() < STT_KEY_REFRESH_AGE)
}

async fn refresh_stt_key_if_old() {
    if session().is_some() && !stt_key_is_fresh() {
        if let Err(problem) = refresh_stt_key().await {
            eprintln!("No transcription key from the backend: {problem}");
        }
    }
}

/// Keeps the transcription key young for as long as the app runs. The app lives in the tray all
/// day, and a key fetched only at sign-in went stale after 2 h, after which an installed copy
/// could not start a call at all.
pub fn spawn_stt_key_refresher() {
    tauri::async_runtime::spawn(async {
        loop {
            refresh_stt_key_if_old().await;
            tokio::time::sleep(STT_KEY_CHECK_INTERVAL).await;
        }
    });
}

/// Called when the start dialog opens. Opens the connection to the backend now, so the reads the
/// dialog makes and the call's first hint don't each wait for a TLS handshake, and replaces an
/// ageing transcription key before the call needs it.
#[tauri::command]
pub async fn warm_connections() {
    let (Some(url), Some(_)) = (base_url(), session()) else {
        return;
    };
    let connect = async {
        if let Ok(client) = client() {
            let _ = client.get(format!("{url}/health")).send().await;
        }
    };
    tokio::join!(connect, refresh_stt_key_if_old());
}

/// Fetches a transcription key now; the Settings screen reports what went wrong.
#[tauri::command]
pub async fn backend_refresh_stt_key() -> Result<Value, String> {
    refresh_stt_key().await?;
    Ok(json!({ "have_key": cached_stt_key().is_some() }))
}

/// Refreshes who we are from the backend: a session saved by an older build, or a role changed
/// since signing in, would otherwise be stale until the next sign-in.
#[tauri::command]
pub async fn backend_refresh_account() -> Result<Value, String> {
    let Some(mut stored) = session() else {
        return Ok(backend_status());
    };
    let me = call_backend(reqwest::Method::GET, "/me", None).await?;
    if let Some(email) = me.get("email").and_then(Value::as_str) {
        stored.email = email.to_string();
    }
    if let Some(workspace) = me.get("workspace_name").and_then(Value::as_str) {
        stored.workspace_name = workspace.to_string();
    }
    stored.is_admin = me.get("is_admin").and_then(Value::as_bool).unwrap_or(false);
    save_session(Some(&stored))?;
    if let Err(problem) = refresh_stt_key().await {
        eprintln!("No transcription key from the backend: {problem}");
    }
    Ok(backend_status())
}

/// Everyone in the workspace (admins only, enforced by the backend).
#[tauri::command]
pub async fn list_members() -> Result<Value, String> {
    call_backend(reqwest::Method::GET, "/users", None).await
}

/// Adds a teammate; the reply carries the one-time password to pass on to them.
#[tauri::command]
pub async fn add_member(
    email: String,
    name: String,
    is_admin: bool,
    monthly_spend_limit_usd: Option<f64>,
) -> Result<Value, String> {
    call_backend(
        reqwest::Method::POST,
        "/users",
        Some(json!({
            "email": email,
            "name": name,
            "is_admin": is_admin,
            "monthly_spend_limit_usd": monthly_spend_limit_usd,
        })),
    )
    .await
}

/// Changes a teammate: name, admin rights, whether they can sign in, or their spend limit.
#[tauri::command]
pub async fn update_member(member_id: String, changes: Value) -> Result<Value, String> {
    call_backend(
        reqwest::Method::PATCH,
        &format!("/users/{member_id}"),
        Some(changes),
    )
    .await
}

/// Gives a teammate a new one-time password, shown once.
#[tauri::command]
pub async fn reset_member_password(member_id: String) -> Result<Value, String> {
    call_backend(
        reqwest::Method::POST,
        &format!("/users/{member_id}/password"),
        None,
    )
    .await
}

/// Removes a teammate who has no calls; the backend refuses if they do.
#[tauri::command]
pub async fn remove_member(member_id: String) -> Result<Value, String> {
    call_backend(
        reqwest::Method::DELETE,
        &format!("/users/{member_id}"),
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_calls_keep_what_the_summary_found() {
        let calls = json!([
            {
                "created_at": "2026-09-20T10:15:00Z",
                "user_email": "rep@example.com",
                "summary": {
                    "overview": "Wants to cut their OpenAI bill.",
                    "prospect": { "who": "CTO", "needs": ["Lower LLM spend"], "current_setup": "Not discussed" },
                    "pain_points": [],
                    "objections": [{ "objection": "Consultants are expensive", "response": "" }],
                    "qualification": { "budget": "Not discussed", "timeline": "Before the Q4 demo", "decision_process": "not discussed" },
                    "commitments": [
                        { "owner": "us", "action": "Send the audit proposal" },
                        { "owner": "prospect", "action": "Share last month's bill" }
                    ],
                    "next_step": "Review the proposal on Monday",
                    "sentiment": "positive"
                }
            },
            { "created_at": "2026-09-01T09:00:00Z", "summary": null }
        ]);
        assert_eq!(
            previous_calls_prompt(&calls),
            "Call on 2026-09-20 (rep: rep@example.com)\n\
             - Overview: Wants to cut their OpenAI bill.\n\
             - Their needs: Lower LLM spend\n\
             - Objections: Consultants are expensive\n\
             - Timeline: Before the Q4 demo\n\
             - Commitments: we: Send the audit proposal; they: Share last month's bill\n\
             - Agreed next step: Review the proposal on Monday"
        );
        assert_eq!(previous_calls_prompt(&json!([])), "");
    }
}
