//! Sending a finished call to the CRM.
//!
//! Twenty is the first destination: a call lands on an Opportunity record (`opportunityCustom`),
//! which is the table the team works in. People and companies are only used to find or link that
//! record, never edited. The payload below is deliberately about the call, not about Twenty, so a
//! second destination (our own backend, roadmap Phase 4) can send the same thing later.
//!
//! Rules that matter:
//! - A call is sent under its own ID, so pressing Send twice updates one record instead of
//!   creating two.
//! - Nothing is created in the CRM unless the call names a prospect or company we can match.
//! - The workspace sits behind Cloudflare, which rejects requests without a browser user agent.
//! - Signed in to a workspace, every CRM call goes through the backend, which holds the CRM key.
//!   That is what lets an installed copy write to the CRM at all: only a dev machine has a key
//!   of its own.

use crate::config::env_or_dotenv;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// The CRM sits behind Cloudflare, which can take tens of seconds on a cold connection; a
/// too-short limit turns that into a failed send rather than a slow one.
const TIMEOUT: Duration = Duration::from_secs(45);
/// Cloudflare in front of the workspace answers 403 (error 1010) without this.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// The app's one client, so the searches a rep types don't each pay for their own TLS handshake,
/// and the reads made while the start dialog is open warm the connection the first hint uses.
fn http() -> Result<reqwest::Client, String> {
    crate::backend::client()
}

/// A finished call, in the app's own terms.
#[derive(Clone, Debug, Deserialize)]
pub struct CallForCrm {
    /// Identifies the call; re-sending the same one updates its record.
    pub call_id: String,
    pub prospect: String,
    pub company: String,
    /// The written summary, as Markdown: goes in the record and in the note.
    pub summary: String,
    /// The same summary in its parts, so each one lands in its own CRM field.
    #[serde(default)]
    pub details: Option<Value>,
    pub next_step: String,
    /// "positive", "neutral" or "negative".
    pub sentiment: String,
    pub duration_minutes: f64,
    /// The records picked before the call started. With these there is no guessing by name: the
    /// call lands on the deal the rep chose, even if two people share a name.
    #[serde(default)]
    pub crm_person_id: Option<String>,
    #[serde(default)]
    pub crm_company_id: Option<String>,
    #[serde(default)]
    pub crm_opportunity_id: Option<String>,
}

/// Someone in the CRM, as the start-call dialog lists them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrmPerson {
    pub id: String,
    pub name: String,
    pub email: String,
    /// Their job title and phone: without these, two people of the same name and no company on
    /// record are two identical rows in the list.
    pub job_title: String,
    pub phone: String,
    pub company_id: Option<String>,
    pub company_name: String,
    /// Their deals, so the call can be attached to the right one. The people list leaves them
    /// out; they are read once someone is picked.
    #[serde(default)]
    pub opportunities: Vec<CrmDeal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrmDeal {
    pub id: String,
    pub name: String,
    pub stage: String,
    pub last_call_at: Option<String>,
}

/// What the CRM already knows about this prospect, as prompt text for the coach.
#[derive(Clone, Debug, Serialize)]
pub struct CrmContext {
    /// Their deal, what was agreed, and what earlier calls covered.
    pub summary: String,
    /// How many past notes went into it, so the dialog can say so.
    pub notes: usize,
}

/// A CRM field's worth of text from a summary section; `None` when the model had nothing to say,
/// so an empty call never wipes what someone typed by hand.
fn text_of(value: Option<&Value>) -> Option<String> {
    let text = match value? {
        Value::String(s) => s.trim().to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s.trim().to_string()),
                // Objections are {objection, response}; commitments are {owner, action}.
                Value::Object(map) => {
                    let parts: Vec<String> = ["objection", "response", "owner", "action"]
                        .iter()
                        .filter_map(|key| map.get(*key).and_then(Value::as_str))
                        .map(|s| s.trim().to_string())
                        .collect();
                    (!parts.is_empty()).then(|| parts.join(" — "))
                }
                _ => None,
            })
            .filter(|line| !line.is_empty())
            .map(|line| format!("• {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    (!text.is_empty()).then_some(text)
}

/// What the app tells the user after sending.
#[derive(Clone, Debug, Serialize)]
pub struct CrmResult {
    pub record_url: String,
    pub record_name: String,
    /// "created" or "updated", so the UI can say which happened.
    pub action: &'static str,
    /// Names of the records it matched on, for the "written to" line.
    pub matched_person: Option<String>,
    pub matched_company: Option<String>,
}

struct Twenty {
    client: reqwest::Client,
    route: Route,
}

/// Straight to the CRM with a key on this machine, or through the workspace backend, which has
/// the key. Installed copies only ever have the second.
enum Route {
    Direct { base: String, key: String },
    Backend { base: String, token: String },
}

impl Twenty {
    fn from_env() -> Result<Self, String> {
        let client = http()?;

        // A key on this machine wins (that is a developer pointing at their own CRM); otherwise
        // the workspace's, through the backend.
        let local = env_or_dotenv("TWENTY_API_KEY")
            .filter(|k| !k.trim().is_empty())
            .zip(env_or_dotenv("TWENTY_BASE_URL").filter(|b| !b.trim().is_empty()));
        let route =
            match (local, crate::backend::llm_route()) {
                (Some((key, base)), _) => Route::Direct {
                    base: base.trim_end_matches('/').to_string(),
                    key,
                },
                (None, Some((base, token))) => Route::Backend { base, token },
                (None, None) => return Err(
                    "No CRM: sign in to your workspace, or set TWENTY_API_KEY and TWENTY_BASE_URL"
                        .into(),
                ),
            };
        Ok(Self { client, route })
    }

    /// Where a record lives, for the link shown after a send.
    fn record_url(&self, id: &str) -> String {
        match &self.route {
            Route::Direct { base, .. } => format!("{base}/object/opportunityCustom/{id}"),
            // The backend knows the CRM's address; the app only needs it for this link.
            Route::Backend { .. } => crate::backend::crm_base_url()
                .map(|base| format!("{base}/object/opportunityCustom/{id}"))
                .unwrap_or_default(),
        }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        self.call(method, path, &[], body).await
    }

    /// `path` is a REST path under `/rest`, e.g. `opportunityCustoms/<id>`.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        params: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value, String> {
        let path = path.trim_start_matches('/').trim_start_matches("rest/");
        match &self.route {
            Route::Direct { base, key } => {
                let request = self
                    .client
                    .request(method, format!("{base}/rest/{path}"))
                    .bearer_auth(key)
                    .query(params);
                self.send(request, body).await
            }
            Route::Backend { base, token } => {
                let params: serde_json::Map<String, Value> = params
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), json!(v)))
                    .collect();
                let request = self
                    .client
                    .post(format!("{base}/crm/request"))
                    .bearer_auth(token)
                    .json(&json!({
                        "method": method.as_str(),
                        "path": path,
                        "params": params,
                        "body": body,
                    }));
                self.send(request, None).await
            }
        }
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let mut request = request
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .timeout(TIMEOUT);
        if let Some(body) = body {
            request = request.json(&body);
        }
        // One retry: a cold connection through Cloudflare sometimes stalls, and the second
        // attempt goes through in under a second.
        let retry = request.try_clone();
        let response = match request.send().await {
            Ok(response) => response,
            Err(first) => match retry {
                Some(request) => request
                    .send()
                    .await
                    .map_err(|again| format!("CRM unreachable: {again} (first try: {first})"))?,
                None => return Err(format!("CRM unreachable: {first}")),
            },
        };
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let detail = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("messages")
                        .or(v.get("message"))
                        .map(|m| m.to_string())
                })
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(format!("CRM refused the write ({status}): {detail}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("CRM sent something unreadable: {e}"))
    }

    /// The first list in a REST response (`{"data": {"opportunityCustoms": [...]}}`).
    fn records(body: &Value) -> Vec<Value> {
        body.get("data")
            .and_then(Value::as_object)
            .and_then(|map| map.values().find_map(Value::as_array))
            .cloned()
            .unwrap_or_default()
    }

    /// The single record in a create or update response, whatever the operation is called.
    fn record(body: &Value) -> Option<Value> {
        body.get("data")
            .and_then(Value::as_object)
            .and_then(|map| map.values().find(|v| v.get("id").is_some()))
            .cloned()
    }

    async fn find(&self, collection: &str, filter: &str) -> Result<Vec<Value>, String> {
        let body = self
            .call(
                reqwest::Method::GET,
                collection,
                &[("limit", "5"), ("filter", filter)],
                None,
            )
            .await?;
        Ok(Self::records(&body))
    }

    /// Like `find`, newest first. The CRM returns records in no particular order otherwise, so
    /// "the last few notes" has to say so explicitly.
    async fn find_newest(
        &self,
        collection: &str,
        filter: &str,
        limit: &str,
    ) -> Result<Vec<Value>, String> {
        let body = self
            .call(
                reqwest::Method::GET,
                collection,
                &[
                    ("limit", limit),
                    ("filter", filter),
                    ("order_by", "createdAt[DescNullsLast]"),
                ],
                None,
            )
            .await?;
        Ok(Self::records(&body))
    }

    /// One record by id. `None` covers a record that is no longer there — deleted since the call
    /// started, or an id from a workspace this laptop no longer points at — because a call's
    /// summary must not be lost over a record that moved; the send falls back to matching by name.
    async fn get(&self, collection: &str, id: &str) -> Option<Value> {
        let body = self
            .call(
                reqwest::Method::GET,
                &format!("{collection}/{id}"),
                &[],
                None,
            )
            .await
            .ok()?;
        Self::record(&body)
    }

    /// Records whose name contains `text`; empty when there's nothing to search for.
    async fn find_by_name(&self, collection: &str, text: &str) -> Result<Vec<Value>, String> {
        let text = searchable(text);
        if text.is_empty() {
            return Ok(Vec::new());
        }
        self.find(collection, &format!("name[ilike]:%{text}%"))
            .await
    }
}

impl Twenty {
    /// This deal's note for that call, and the note target that ties it to the deal. A miss is
    /// not an error: it only means a note will be created.
    ///
    /// The match is on the marker in the body, not the title: the title is written in whoever's
    /// timezone the laptop is in, so a rep who flies somewhere and re-sends would otherwise write
    /// a second note. It reads the deal's own notes rather than filtering, because the CRM's
    /// filter syntax treats a comma as the start of the next clause and titles have commas.
    async fn note_of(&self, opportunity_id: &str, call_id: &str) -> Option<(String, Value)> {
        let targets = self
            .find_newest(
                "noteTargets",
                &format!("opportunityCustomId[eq]:{opportunity_id}"),
                // A deal with more notes than this than one call would have to be an old one, and
                // the note being looked for was written seconds ago.
                "30",
            )
            .await
            .ok()?;
        let marker = note_marker(call_id);
        for target in targets {
            let Some(note_id) = field_of(&target, "noteId") else {
                continue;
            };
            let Some(note) = self.get("notes", &note_id).await else {
                continue;
            };
            let body = note
                .get("bodyV2")
                .and_then(|body| body.get("markdown"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if body.contains(&marker) {
                return Some((note_id, target));
            }
        }
        None
    }

    /// Writes the summary as a note and hangs it on the Opportunity, the person and the company.
    async fn attach_note(
        &self,
        call_id: &str,
        title: &str,
        markdown: &str,
        opportunity_id: &str,
        person_id: Option<&str>,
        company_id: Option<&str>,
    ) -> Result<(), String> {
        // The body ends with the call's own id, so a re-send can find this note again.
        let body = format!(
            "{}

{}",
            markdown.trim_end(),
            note_marker(call_id)
        );

        // Each call keeps its own note, so a deal with five meetings reads as five notes. A
        // re-send of the same call rewrites its note instead of leaving two copies behind.
        if let Some((id, target)) = self.note_of(opportunity_id, call_id).await {
            self.request(
                reqwest::Method::PATCH,
                &format!("/rest/notes/{id}"),
                Some(json!({ "title": title, "bodyV2": { "markdown": body } })),
            )
            .await?;
            // The first send may not have known who the call was with — the rep can name them on
            // the summary screen and send again — so the note gets linked to them now.
            let mut missing = json!({});
            for (field, id) in [
                ("personsCustomId", person_id),
                ("companyCustomId", company_id),
            ] {
                if let Some(id) = id {
                    if field_of(&target, field).is_none() {
                        missing[field] = json!(id);
                    }
                }
            }
            if let (Some(target_id), false) = (field_of(&target, "id"), missing == json!({})) {
                self.request(
                    reqwest::Method::PATCH,
                    &format!("/rest/noteTargets/{target_id}"),
                    Some(missing),
                )
                .await?;
            }
            return Ok(());
        }

        let note = self
            .request(
                reqwest::Method::POST,
                "/rest/notes",
                Some(json!({ "title": title, "bodyV2": { "markdown": body } })),
            )
            .await?;
        let note_id = Self::record(&note)
            .and_then(|n| n.get("id").and_then(Value::as_str).map(str::to_string))
            .ok_or("The CRM didn't return the new note")?;

        let mut target = json!({ "noteId": note_id, "opportunityCustomId": opportunity_id });
        if let Some(id) = person_id {
            target["personsCustomId"] = json!(id);
        }
        if let Some(id) = company_id {
            target["companyCustomId"] = json!(id);
        }
        self.request(reqwest::Method::POST, "/rest/noteTargets", Some(target))
            .await?;
        Ok(())
    }
}

/// What of this text can safely go inside a CRM filter. The filter syntax reads a comma as the
/// start of the next clause and a colon as the start of a value, so "Sharma, Priya" or a pasted
/// "Priya: notes" would come back as a 400 and read to the rep as "CRM search unavailable". The
/// leading part is what people type first anyway, and searching it finds them.
fn searchable(text: &str) -> &str {
    text.trim()
        .split([',', ':', '[', ']', '(', ')'])
        .next()
        .unwrap_or("")
        .trim()
}

fn name_of(record: &Value) -> String {
    record
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether a CRM is reachable, for the Settings screen and the Send button: a key on this
/// machine, or a workspace that has one.
#[tauri::command]
pub async fn crm_status() -> Value {
    let local_key = env_or_dotenv("TWENTY_API_KEY").is_some_and(|k| !k.trim().is_empty());
    let local_base = env_or_dotenv("TWENTY_BASE_URL").unwrap_or_default();
    if local_key && !local_base.trim().is_empty() {
        return json!({ "configured": true, "base_url": local_base, "through": "this computer" });
    }
    match crate::backend::crm_workspace_status().await {
        Ok(status) => json!({
            "configured": status.0,
            "base_url": status.1,
            "through": "your workspace",
        }),
        Err(_) => json!({ "configured": false, "base_url": "", "through": "nothing" }),
    }
}

/// Sends a finished call to the CRM: updates the prospect's Opportunity, or creates one.
#[tauri::command]
pub async fn send_call_to_crm(call: CallForCrm) -> Result<CrmResult, String> {
    let twenty = Twenty::from_env()?;

    // 1. Sent before? Update that record, so a second press never duplicates the call.
    let existing = twenty
        .find(
            "opportunityCustoms",
            &format!("sidekickCallId[eq]:{}", call.call_id),
        )
        .await?;

    // A record picked before the call is the identity; names are only the fallback, for calls
    // started without a CRM lookup.
    let mut company = match call.crm_company_id.as_deref() {
        Some(id) => twenty.get("companyCustoms", id).await,
        None => None,
    };
    let mut person = match call.crm_person_id.as_deref() {
        Some(id) => twenty.get("personsCustoms", id).await,
        None => None,
    };

    // Match by name, and create what's missing, so the call lands on real records rather than
    // an Opportunity floating on its own.
    if company.is_none() {
        company = twenty
            .find_by_name("companyCustoms", &call.company)
            .await?
            .first()
            .cloned();
    }
    if company.is_none() && !call.company.trim().is_empty() {
        let created = twenty
            .request(
                reqwest::Method::POST,
                "/rest/companyCustoms",
                Some(json!({ "name": call.company.trim() })),
            )
            .await?;
        company = Twenty::record(&created);
    }

    if person.is_none() {
        person = twenty
            .find_by_name("personsCustoms", &call.prospect)
            .await?
            .first()
            .cloned();
    }
    if person.is_none() && !call.prospect.trim().is_empty() {
        let mut fields = json!({ "name": call.prospect.trim() });
        if let Some(id) = company.as_ref().and_then(|c| c.get("id")) {
            fields["companyId"] = id.clone();
        }
        let created = twenty
            .request(reqwest::Method::POST, "/rest/personsCustoms", Some(fields))
            .await?;
        person = Twenty::record(&created);
    }

    // 2. Otherwise the deal picked before the call, then the prospect's most recent one.
    let picked = match call.crm_opportunity_id.as_deref() {
        Some(id) => twenty.get("opportunityCustoms", id).await,
        None => None,
    };
    let opportunity = match existing.first().cloned().or(picked) {
        Some(found) => Some(found),
        None => {
            let mut found = None;
            for (field, record) in [("pointOfContactId", &person), ("companyId", &company)] {
                let Some(id) = record
                    .as_ref()
                    .and_then(|r| r.get("id"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let matches = twenty
                    .find("opportunityCustoms", &format!("{field}[eq]:{id}"))
                    .await?;
                if let Some(best) = newest(matches) {
                    found = Some(best);
                    break;
                }
            }
            found
        }
    };

    let mut fields = json!({
        "sidekickCallId": call.call_id,
        "lastCallAt": chrono_now(),
        "callSummary": call.summary,
        "callSentiment": call.sentiment.to_uppercase(),
        "callDurationMinutes": (call.duration_minutes * 10.0).round() / 10.0,
        "meetingStatus": "MEETING_CONDUCTED",
    });
    if !call.next_step.trim().is_empty() {
        fields["nextStep"] = json!(call.next_step);
    }
    if let Some(details) = call.details.as_ref() {
        let prospect = details.get("prospect");
        let qualification = details.get("qualification");
        let parts = [
            ("remarks", text_of(details.get("overview"))),
            (
                "callProspectNeeds",
                text_of(prospect.and_then(|p| p.get("needs"))),
            ),
            (
                "callCurrentSetup",
                text_of(prospect.and_then(|p| p.get("current_setup"))),
            ),
            ("callPainPoints", text_of(details.get("pain_points"))),
            ("callObjections", text_of(details.get("objections"))),
            (
                "callBudget",
                text_of(qualification.and_then(|q| q.get("budget"))),
            ),
            (
                "callTimeline",
                text_of(qualification.and_then(|q| q.get("timeline"))),
            ),
            (
                "callDecisionProcess",
                text_of(qualification.and_then(|q| q.get("decision_process"))),
            ),
            ("callCommitments", text_of(details.get("commitments"))),
        ];
        for (field, value) in parts {
            if let Some(value) = value {
                fields[field] = json!(value);
            }
        }
    }

    let (record, action) = match opportunity {
        Some(existing) => {
            let id = existing
                .get("id")
                .and_then(Value::as_str)
                .ok_or("The CRM record has no id")?;
            let updated = twenty
                .request(
                    reqwest::Method::PATCH,
                    &format!("/rest/opportunityCustoms/{id}"),
                    Some(fields),
                )
                .await?;
            (Twenty::record(&updated).unwrap_or(existing), "updated")
        }
        None => {
            // 3. Nothing to attach to: name the new Opportunity after whoever we know.
            let label = match (call.prospect.trim(), call.company.trim()) {
                ("", "") => return Err(
                    "This call has no prospect or company, so there's nothing to match in the CRM. \
                     Add them in the call details and send again."
                        .into(),
                ),
                (prospect, "") => prospect.to_string(),
                ("", company) => company.to_string(),
                (prospect, company) => format!("{prospect} ({company})"),
            };
            fields["name"] = json!(label);
            if let Some(id) = person.as_ref().and_then(|p| p.get("id")) {
                fields["pointOfContactId"] = id.clone();
            }
            if let Some(id) = company.as_ref().and_then(|c| c.get("id")) {
                fields["companyId"] = id.clone();
            }
            let created = twenty
                .request(
                    reqwest::Method::POST,
                    "/rest/opportunityCustoms",
                    Some(fields),
                )
                .await?;
            (
                Twenty::record(&created).ok_or("The CRM didn't return the new record")?,
                "created",
            )
        }
    };

    // The link needs the CRM's own address, which only the workspace knows.
    if matches!(twenty.route, Route::Backend { .. }) && crate::backend::crm_base_url().is_none() {
        let _ = crate::backend::crm_workspace_status().await;
    }

    let id = record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let id = id.as_str();

    // The note is what a colleague opens later; the fields are what a list view filters on.
    if let Err(problem) = twenty
        .attach_note(
            &call.call_id,
            &note_title(&call),
            &call.summary,
            id,
            person
                .as_ref()
                .and_then(|p| p.get("id"))
                .and_then(Value::as_str),
            company
                .as_ref()
                .and_then(|c| c.get("id"))
                .and_then(Value::as_str),
        )
        .await
    {
        // The record itself is written; a missing note isn't worth failing the send over.
        eprintln!("The CRM note couldn't be attached: {problem}");
    }

    Ok(CrmResult {
        record_url: twenty.record_url(id),
        record_name: name_of(&record),
        action,
        matched_person: person.as_ref().map(name_of).filter(|n| !n.is_empty()),
        matched_company: company.as_ref().map(name_of).filter(|n| !n.is_empty()),
    })
}

/// The most recently updated record.
fn newest(mut records: Vec<Value>) -> Option<Value> {
    records.sort_by(|a, b| {
        let at = |v: &Value| {
            v.get("updatedAt")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        at(b).cmp(&at(a))
    });
    records.into_iter().next()
}

/// The line at the end of a note that says which call wrote it. A re-send finds its own note by
/// this, rather than by the title: the title is written in the rep's timezone and would change
/// under them if they travelled, and then one call would leave two notes.
fn note_marker(call_id: &str) -> String {
    format!("_Sidekick {call_id}_")
}

/// The note's title: who the call was with and when it happened, e.g.
/// "Call with Priya — 21 Sep 2026, 4:19 pm". The time comes from the call's own id, so re-sending
/// the same call produces the same title — that is what keeps one note per call — while a second
/// meeting with the same person gets a note of its own.
fn note_title(call: &CallForCrm) -> String {
    let who = match (call.prospect.trim(), call.company.trim()) {
        ("", "") => "Sidekick call".to_string(),
        ("", company) => format!("Call with {company}"),
        (prospect, _) => format!("Call with {prospect}"),
    };
    match started_at_ms(&call.call_id) {
        Some(ms) => format!("{who} — {}", local_stamp(ms)),
        None => who,
    }
}

/// When the call started, from its id (`call-1789987771082`).
fn started_at_ms(call_id: &str) -> Option<i64> {
    call_id.strip_prefix("call-")?.parse().ok()
}

/// A moment in the rep's own timezone, as "21 Sep 2026, 4:19 pm". The CRM stores UTC, but this
/// string is read by people, and a note titled three hours off the meeting reads as a mistake.
fn local_stamp(epoch_ms: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = epoch_ms.div_euclid(1000);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let time = secs.rem_euclid(86_400);
    let (mut hour, mut minute) = ((time / 3600) as u16, ((time % 3600) / 60) as u16);
    let (mut year, mut month, mut day) = (year, month as u16, day as u16);

    // Windows converts UTC to whatever zone this machine is in, daylight saving included.
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::SYSTEMTIME;
        use windows::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
        let utc = SYSTEMTIME {
            wYear: year as u16,
            wMonth: month,
            wDay: day,
            wHour: hour,
            wMinute: minute,
            ..Default::default()
        };
        let mut local = SYSTEMTIME::default();
        if unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }.is_ok() {
            year = local.wYear as i64;
            month = local.wMonth;
            day = local.wDay;
            hour = local.wHour;
            minute = local.wMinute;
        }
    }

    // macOS: the C library converts to the local zone, daylight saving included.
    #[cfg(target_os = "macos")]
    {
        let t = secs as libc::time_t;
        // SAFETY: localtime_r only writes into the zeroed struct.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            year = i64::from(tm.tm_year) + 1900;
            month = (tm.tm_mon + 1) as u16;
            day = tm.tm_mday as u16;
            hour = tm.tm_hour as u16;
            minute = tm.tm_min as u16;
        }
    }

    let name = MONTHS
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("");
    let (clock, half) = match hour {
        0 => (12, "am"),
        1..=11 => (hour, "am"),
        12 => (12, "pm"),
        _ => (hour - 12, "pm"),
    };
    format!("{day} {name} {year}, {clock}:{minute:02} {half}")
}

/// Now, as the ISO 8601 timestamp the CRM expects.
fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// Days since the Unix epoch to a calendar date (Howard Hinnant's civil_from_days).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A field of the record as trimmed text, or `None` when it's empty.
fn field_of(record: &Value, key: &str) -> Option<String> {
    record
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// People in the CRM matching what the rep typed, each with their deals. This is what the
/// start-call dialog searches, so a call can be tied to real records before a word is spoken.
#[tauri::command]
pub async fn crm_search_people(query: String) -> Result<Vec<CrmPerson>, String> {
    let query = query.trim();
    if query.len() < 2 {
        return Ok(Vec::new());
    }
    let text = searchable(query);
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let twenty = std::sync::Arc::new(Twenty::from_env()?);
    // `depth=1` brings each person's company and deals along in the same answer. Fetched
    // separately they were two more rounds to a CRM behind Cloudflare, and the rep waited
    // 2–5 s per search instead of about one.
    let filter = format!("name[ilike]:%{text}%");
    let body = twenty
        .call(
            reqwest::Method::GET,
            "personsCustoms",
            &[("limit", "5"), ("filter", &filter), ("depth", "1")],
            None,
        )
        .await?;

    // A CRM whose schema names the relations differently still works, one lookup slower.
    let mut lookups = Vec::new();
    for person in Twenty::records(&body) {
        let twenty = twenty.clone();
        lookups.push(tokio::spawn(async move {
            let mut found = person_from(&person)?;
            if person.get(DEALS_RELATION).is_none() {
                let deals = twenty
                    .find(
                        "opportunityCustoms",
                        &format!("pointOfContactId[eq]:{}", found.id),
                    )
                    .await
                    .unwrap_or_default();
                found.opportunities = deals_from(&deals);
            }
            if let (Some(company_id), None) = (found.company_id.as_deref(), person.get("company")) {
                if let Some(company) = twenty.get("companyCustoms", company_id).await {
                    found.company_name = name_of(&company);
                }
            }
            Some(found)
        }));
    }

    // In the order the CRM matched them; a person whose lookup failed is simply left out.
    let mut found = Vec::new();
    for lookup in lookups {
        if let Ok(Some(person)) = lookup.await {
            found.push(person);
        }
    }
    Ok(found)
}

/// Everyone in the workspace CRM, without deals, for the start-call dialog to search as the rep
/// types. The backend holds the list in memory, so this answers in well under a second where a
/// search of the CRM itself takes one to two. With a CRM key on this machine (a developer setup)
/// there is no list, and the dialog searches the CRM instead.
#[tauri::command]
pub async fn crm_people_index() -> Result<Vec<CrmPerson>, String> {
    let Some((base, token)) = crate::backend::llm_route() else {
        return Err("The people list comes from the workspace server".into());
    };
    let response = http()?
        .get(format!("{base}/crm/people"))
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("CRM people list unreachable: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("CRM people list refused ({})", response.status()));
    }
    response
        .json::<Vec<CrmPerson>>()
        .await
        .map_err(|e| format!("CRM people list unreadable: {e}"))
}

/// The deals someone is point of contact on, newest first, read once they are picked.
#[tauri::command]
pub async fn crm_person_deals(person_id: String) -> Result<Vec<CrmDeal>, String> {
    let twenty = Twenty::from_env()?;
    let deals = twenty
        .find(
            "opportunityCustoms",
            &format!("pointOfContactId[eq]:{}", searchable(&person_id)),
        )
        .await?;
    Ok(deals_from(&deals))
}

/// The relation on a person that holds the deals they are point of contact on
/// (`opportunityCustoms.pointOfContactId`), as `depth=1` names it.
const DEALS_RELATION: &str = "championedOpportunities";

/// A person as the dialog shows them, from a record read with `depth=1`.
fn person_from(person: &Value) -> Option<CrmPerson> {
    let deals = person
        .get(DEALS_RELATION)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Some(CrmPerson {
        id: field_of(person, "id")?,
        name: name_of(person),
        email: field_of(person, "workEmail")
            .or_else(|| field_of(person, "email"))
            .unwrap_or_default(),
        job_title: field_of(person, "title").unwrap_or_default(),
        phone: field_of(person, "phone").unwrap_or_default(),
        company_id: field_of(person, "companyId"),
        // Null when the company was deleted but the person still points at it.
        company_name: person.get("company").map(name_of).unwrap_or_default(),
        opportunities: deals_from(&deals),
    })
}

/// Newest first: the dialog preselects the first deal, and the call is almost always about the
/// one that moved most recently.
fn deals_from(deals: &[Value]) -> Vec<CrmDeal> {
    let mut deals = deals.to_vec();
    deals.sort_by_key(|deal| {
        std::cmp::Reverse(
            field_of(deal, "lastCallAt")
                .or_else(|| field_of(deal, "updatedAt"))
                .unwrap_or_default(),
        )
    });
    deals
        .iter()
        .filter_map(|deal| {
            Some(CrmDeal {
                id: field_of(deal, "id")?,
                name: name_of(deal),
                stage: field_of(deal, "stage").unwrap_or_default(),
                last_call_at: field_of(deal, "lastCallAt"),
            })
        })
        .collect()
}

/// What the CRM already knows about this prospect, as prompt text. It goes in the coach's cached
/// prefix, so the first hint of the call already knows where the deal stands.
#[tauri::command]
pub async fn crm_person_context(
    person_id: String,
    opportunity_id: Option<String>,
) -> Result<CrmContext, String> {
    let twenty = Twenty::from_env()?;
    let mut lines: Vec<String> = Vec::new();

    // The deal and the note list don't depend on each other, and each read is a round trip
    // through the backend to the CRM, so they go together; then the notes, together. Read one
    // after another they took 1.6 s on a real call start.
    let deal = async {
        Ok::<_, String>(match opportunity_id.as_deref() {
            Some(id) => twenty.get("opportunityCustoms", id).await,
            None => newest(
                twenty
                    .find(
                        "opportunityCustoms",
                        &format!("pointOfContactId[eq]:{person_id}"),
                    )
                    .await?,
            ),
        })
    };
    // The last few notes: earlier calls with this person, and what colleagues wrote. Newest first
    // — the CRM returns records in no order at all otherwise, which would feed the coach whichever
    // three notes happened to come back.
    let targets_filter = format!("personsCustomId[eq]:{person_id}");
    let targets = twenty.find_newest("noteTargets", &targets_filter, "3");
    let (deal, targets) = tokio::join!(deal, targets);
    let deal = deal?;
    let targets = targets.unwrap_or_default();

    if let Some(deal) = deal.as_ref() {
        lines.push(format!("- Deal: {}", name_of(deal)));
        for (label, key) in [
            ("Stage", "stage"),
            ("Agreed next step", "nextStep"),
            ("Last call", "callSummary"),
            ("Their objections so far", "callObjections"),
            ("What they need", "callProspectNeeds"),
            ("Budget", "callBudget"),
            ("Timeline", "callTimeline"),
            ("Decision process", "callDecisionProcess"),
            ("Open commitments", "callCommitments"),
        ] {
            if let Some(value) = field_of(deal, key) {
                lines.push(format!("- {label}: {}", value.replace('\n', "; ")));
            }
        }
    }

    let note_reads = targets
        .iter()
        .filter_map(|target| field_of(target, "noteId"))
        .map(|note_id| {
            let twenty = &twenty;
            async move { twenty.get("notes", &note_id).await }
        });
    let mut notes = 0;
    for note in futures_util::future::join_all(note_reads).await {
        let body = note
            .as_ref()
            .and_then(|note| {
                note.get("bodyV2")
                    .and_then(|body| body.get("markdown"))
                    .and_then(Value::as_str)
                    .map(|text| text.trim().chars().take(600).collect::<String>())
            })
            .filter(|text| !text.is_empty());
        if let Some(body) = body {
            notes += 1;
            // The title carries the date of that meeting, so several calls read in order.
            let when = note
                .as_ref()
                .and_then(|note| field_of(note, "title"))
                .unwrap_or_else(|| "Earlier note".into());
            lines.push(format!("- {when}: {}", body.replace('\n', " ")));
        }
    }

    Ok(CrmContext {
        summary: lines.join("\n"),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call_at(call_id: &str, prospect: &str) -> CallForCrm {
        CallForCrm {
            call_id: call_id.into(),
            prospect: prospect.into(),
            company: String::new(),
            summary: "...".into(),
            details: None,
            next_step: String::new(),
            sentiment: "neutral".into(),
            duration_minutes: 1.0,
            crm_person_id: None,
            crm_company_id: None,
            crm_opportunity_id: None,
        }
    }

    #[test]
    fn each_call_gets_its_own_note_title_and_a_re_send_reuses_it() {
        let first = call_at("call-1789987771082", "Priya");
        let again = call_at("call-1789987771082", "Priya");
        assert_eq!(
            note_title(&first),
            note_title(&again),
            "a re-send must find the same note, not write a second one"
        );
        // A later meeting with the same person is a different note.
        let second_meeting = call_at("call-1790074171082", "Priya");
        assert_ne!(note_title(&first), note_title(&second_meeting));

        assert!(
            note_title(&first).starts_with("Call with Priya — "),
            "{}",
            note_title(&first)
        );
        // A call saved before ids carried a timestamp still gets a usable title.
        assert_eq!(note_title(&call_at("legacy", "Priya")), "Call with Priya");
        assert_eq!(note_title(&call_at("legacy", "")), "Sidekick call");
    }

    #[test]
    fn a_name_with_punctuation_can_still_be_searched() {
        // A comma or a colon in the filter would come back as a 400, which the picker can only
        // report as "CRM search unavailable"; the leading part still finds the person.
        assert_eq!(searchable("Sharma, Priya"), "Sharma");
        assert_eq!(searchable(" Priya: follow up "), "Priya");
        assert_eq!(searchable("Acme (India)"), "Acme");
        assert_eq!(searchable("Priya"), "Priya");
        assert_eq!(searchable(" , "), "");
    }

    #[test]
    fn a_calls_time_reads_as_a_date_people_recognise() {
        // 2026-09-21 10:49:31 UTC. The hour shifts with this machine's timezone, the date only at
        // its edges, so the test pins what is stable.
        let stamp = local_stamp(1_789_987_771_082);
        assert!(stamp.contains("Sep 2026, "), "{stamp}");
        assert!(stamp.ends_with("am") || stamp.ends_with("pm"), "{stamp}");
        // Midnight UTC reads as 12-something, never 0:00.
        let midnight = local_stamp(1_789_948_800_000);
        assert!(!midnight.contains(" 0:"), "{midnight}");
    }

    /// Writes a throwaway call to the live CRM twice, checks it left one note rather than two,
    /// then deletes everything it made. Run with
    /// `cargo test --lib -- --ignored --nocapture one_note_per_call`.
    #[tokio::test]
    #[ignore]
    async fn one_note_per_call_even_when_sent_twice() {
        let twenty = Twenty::from_env().expect("a CRM");
        let stamp = chrono_now();
        // A fresh id each run, so a previous run's records can't be mistaken for this one's.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let mut call = call_at(&format!("call-{now_ms}"), "Sidekick Note Check");
        call.summary = format!("First send at {stamp}.");
        call.company = "Sidekick Note Check Co".into();

        let first = send_call_to_crm(call.clone()).await.expect("first send");
        call.summary = format!("Second send at {stamp}, which should rewrite the same note.");
        let second = send_call_to_crm(call.clone()).await.expect("second send");
        assert_eq!(
            second.action, "updated",
            "a re-send must not make a new deal"
        );
        assert_eq!(
            first.record_url, second.record_url,
            "both sends must land on one deal"
        );

        let title = note_title(&call);
        let deal = twenty
            .find(
                "opportunityCustoms",
                &format!("sidekickCallId[eq]:{}", call.call_id),
            )
            .await
            .expect("the deal this call wrote to");
        let deal_id = deal
            .first()
            .and_then(|d| field_of(d, "id"))
            .expect("a deal");

        // Every note on the deal, so a second copy of the same call would show up here.
        let mut notes = Vec::new();
        for target in twenty
            .find_newest(
                "noteTargets",
                &format!("opportunityCustomId[eq]:{deal_id}"),
                "10",
            )
            .await
            .expect("the deal's notes")
        {
            let Some(note_id) = field_of(&target, "noteId") else {
                continue;
            };
            if let Some(note) = twenty.get("notes", &note_id).await {
                if field_of(&note, "title").as_deref() == Some(title.as_str()) {
                    notes.push(note);
                }
            }
        }
        println!("{} note(s) titled {title}", notes.len());
        let body = notes
            .first()
            .and_then(|note| note.get("bodyV2"))
            .and_then(|body| body.get("markdown"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // Clean up before asserting, so a failed run doesn't leave test records in a live CRM.
        // This also sweeps up anything an earlier run of this test left behind: everything is
        // named after the test, and nothing else in the workspace carries these names.
        let mut removed = 0;
        let mut people = twenty
            .find("personsCustoms", &format!("name[eq]:{}", call.prospect))
            .await
            .unwrap_or_default();
        let mut deals = Vec::new();
        for person in &people {
            if let Some(id) = field_of(person, "id") {
                deals.extend(
                    twenty
                        .find("opportunityCustoms", &format!("pointOfContactId[eq]:{id}"))
                        .await
                        .unwrap_or_default(),
                );
            }
        }
        for deal in &deals {
            let Some(deal_id) = field_of(deal, "id") else {
                continue;
            };
            for target in twenty
                .find_newest(
                    "noteTargets",
                    &format!("opportunityCustomId[eq]:{deal_id}"),
                    "20",
                )
                .await
                .unwrap_or_default()
            {
                for (collection, id) in [
                    ("noteTargets", field_of(&target, "id")),
                    ("notes", field_of(&target, "noteId")),
                ] {
                    if let Some(id) = id {
                        let _ = twenty
                            .request(
                                reqwest::Method::DELETE,
                                &format!("/rest/{collection}/{id}"),
                                None,
                            )
                            .await;
                        removed += 1;
                    }
                }
            }
        }
        people.extend(
            twenty
                .find("companyCustoms", &format!("name[eq]:{}", call.company))
                .await
                .unwrap_or_default(),
        );
        for (collection, records) in [("opportunityCustoms", &deals), ("personsCustoms", &people)] {
            for record in records {
                if let Some(id) = field_of(record, "id") {
                    // People and companies live in different collections; try both, one succeeds.
                    for collection in [collection, "companyCustoms"] {
                        if twenty
                            .request(
                                reqwest::Method::DELETE,
                                &format!("/rest/{collection}/{id}"),
                                None,
                            )
                            .await
                            .is_ok()
                        {
                            removed += 1;
                            break;
                        }
                    }
                }
            }
        }
        println!("deleted {removed} test record(s)");

        assert_eq!(notes.len(), 1, "two sends of one call must leave one note");
        assert!(
            body.contains("Second send"),
            "the note should hold the newer summary: {body}"
        );
    }

    /// Reads the live CRM named in `.env`: searches for a person and pulls their history, the
    /// same two calls the start-call dialog makes. Read-only, so it writes nothing.
    /// Run with `cargo test --lib -- --ignored searches_the_live_crm`.
    #[tokio::test]
    #[ignore]
    async fn searches_the_live_crm_and_reads_a_prospects_history() {
        let query = std::env::var("CRM_SEARCH").unwrap_or_else(|_| "a".into());
        let people = crm_search_people(query).await.expect("a CRM to search");
        println!("{} people", people.len());
        let Some(person) = people.into_iter().find(|p| !p.opportunities.is_empty()) else {
            println!("nobody matched with a deal; nothing more to check");
            return;
        };
        println!(
            "{} ({}) — {} deal(s)",
            person.name,
            person.company_name,
            person.opportunities.len()
        );
        let context = crm_person_context(
            person.id,
            person.opportunities.first().map(|d| d.id.clone()),
        )
        .await
        .expect("their history");
        println!("{} note(s)\n{}", context.notes, context.summary);
        assert!(
            context.summary.contains("- Deal:"),
            "a picked deal should name itself: {}",
            context.summary
        );
    }

    #[test]
    fn reads_company_and_newest_deal_from_one_depth_1_record() {
        let person = json!({
            "id": "p1", "name": "Sam", "workEmail": "sam@northwind.example", "companyId": "c1",
            "company": { "id": "c1", "name": "Northwind" },
            "championedOpportunities": [
                { "id": "old", "name": "Pilot", "stage": "WON", "updatedAt": "2026-01-01T00:00:00Z" },
                { "id": "new", "name": "Rollout", "stage": "PROPOSAL", "lastCallAt": "2026-10-01T00:00:00Z" },
            ],
        });
        let found = person_from(&person).expect("a person");
        assert_eq!(found.company_name, "Northwind");
        let ids: Vec<_> = found.opportunities.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["new", "old"]);
    }

    #[test]
    fn a_deleted_company_leaves_the_name_blank() {
        let person = json!({ "id": "p2", "name": "Kshetry & co", "companyId": "gone", "company": null,
            "championedOpportunities": [] });
        let found = person_from(&person).expect("a person");
        assert_eq!(found.company_name, "");
        assert_eq!(found.company_id.as_deref(), Some("gone"));
    }

    #[test]
    fn reads_the_record_out_of_a_rest_response() {
        let list = json!({ "data": { "opportunityCustoms": [{ "id": "a", "name": "Acme" }] } });
        assert_eq!(Twenty::records(&list).len(), 1);
        let created =
            json!({ "data": { "createOpportunityCustom": { "id": "b", "name": "New" } } });
        assert_eq!(
            Twenty::record(&created).map(|r| name_of(&r)),
            Some("New".into())
        );
    }

    #[test]
    fn picks_the_most_recently_updated() {
        let records = vec![
            json!({ "id": "old", "updatedAt": "2026-01-01T00:00:00Z" }),
            json!({ "id": "new", "updatedAt": "2026-09-01T00:00:00Z" }),
        ];
        assert_eq!(
            newest(records).and_then(|r| r["id"].as_str().map(str::to_string)),
            Some("new".into())
        );
    }

    #[test]
    fn formats_the_timestamp_the_crm_expects() {
        let now = chrono_now();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z') && now.contains('T'), "{now}");
        assert!(now.starts_with("20"), "{now}");
    }
}
