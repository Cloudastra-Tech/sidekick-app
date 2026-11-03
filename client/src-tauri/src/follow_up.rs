//! Follow-up email after a call, drafted on request from the call's summary: the step every rep
//! does next. A separate request (not part of the summary), so calls that need no email don't
//! pay for one. Uses only what the summary says, so it can't promise what wasn't agreed.

use crate::coach::CallBrief;
use crate::llm::{LlmProvider, LlmRequest, Role, Turn, Usage};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

const MAX_TOKENS: u32 = 1200;
const TIMEOUT: Duration = Duration::from_secs(60);

const SYSTEM_PROMPT: &str = r#"You write the follow-up email a sales rep sends the prospect right after a call.

Use only what the call summary says. Never invent prices, dates, names, features, or commitments; if something wasn't agreed, don't promise it.

The email:
- Thanks them briefly and recaps what matters to them (their needs, in their words).
- Restates the agreed next step, with its date or time if there is one, and what each side committed to.
- Answers an open objection only if the summary records how it was answered.
- Is under 150 words, plain text, friendly and direct; no bullet-point walls, no sales clichés.
- Ends with the rep's name when it's given.

Respond with JSON: {"subject": string, "body": string}. The subject is short and specific to this call."#;

static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["subject", "body"],
        "properties": { "subject": { "type": "string" }, "body": { "type": "string" } }
    })
});

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FollowUp {
    pub subject: String,
    pub body: String,
}

/// What the model is given: the summary and who the call was with. Not the transcript.
fn prompt(summary: &Value, brief: &CallBrief, rep_name: Option<&str>) -> String {
    let mut lines = Vec::new();
    let mut add = |label: &str, value: &str| {
        let value = value.trim();
        if !value.is_empty() {
            lines.push(format!("{label}: {value}"));
        }
    };
    add("Prospect", &brief.prospect);
    add("Company", &brief.company);
    add("Rep's name", rep_name.unwrap_or_default());
    let summary = serde_json::to_string_pretty(summary).unwrap_or_default();
    format!("{}\n\nCall summary:\n{summary}", lines.join("\n"))
        .trim_start()
        .to_string()
}

/// Reads the model's answer.
fn parse(text: &str) -> Result<FollowUp, String> {
    let answer: Value =
        serde_json::from_str(text).map_err(|e| format!("Unexpected follow-up answer: {e}"))?;
    let field = |name: &str| answer[name].as_str().unwrap_or_default().trim().to_string();
    let (subject, body) = (field("subject"), field("body"));
    if body.is_empty() {
        return Err("The follow-up email came back empty".into());
    }
    Ok(FollowUp { subject, body })
}

async fn draft(
    client: &reqwest::Client,
    provider: &LlmProvider,
    summary: &Value,
    brief: &CallBrief,
    rep_name: Option<&str>,
) -> Result<(FollowUp, Usage), String> {
    let turns = [Turn {
        role: Role::User,
        text: prompt(summary, brief, rep_name),
    }];
    let response = provider
        .generate(
            client,
            &LlmRequest {
                system: SYSTEM_PROMPT,
                turns: &turns,
                schema: &SCHEMA,
                max_tokens: MAX_TOKENS,
                cache: false,
                effort: Some("low"),
                temperature: None,
                timeout: TIMEOUT,
                hedge_after: None,
            },
        )
        .await?;
    Ok((parse(&response.text)?, response.usage))
}

#[derive(Serialize)]
pub struct FollowUpDraft {
    #[serde(flatten)]
    email: FollowUp,
    /// From the summary model's prices (`SUMMARY_PRICE_*`), when they're set.
    cost_usd: Option<f64>,
}

/// Drafts the follow-up email for a finished call from its summary. Through the backend when
/// signed in, so spend limits apply.
#[tauri::command]
pub async fn draft_follow_up(
    summary: Value,
    brief: Option<CallBrief>,
    rep_name: Option<String>,
) -> Result<FollowUpDraft, String> {
    let provider = LlmProvider::summary_from_env()?;
    let client = crate::backend::client().unwrap_or_else(|_| reqwest::Client::new());
    let brief = brief.unwrap_or_default();
    let rep_name = rep_name.filter(|name| !name.trim().is_empty());
    let (email, usage) = draft(&client, &provider, &summary, &brief, rep_name.as_deref()).await?;
    let price = |key: &str| crate::config::env_f64(key).ok().flatten();
    let cost_usd = price("SUMMARY_PRICE_INPUT_PER_MTOK")
        .zip(price("SUMMARY_PRICE_OUTPUT_PER_MTOK"))
        .map(|(input, output)| {
            (usage.input_tokens as f64 * input + usage.output_tokens as f64 * output) / 1_000_000.0
        });
    Ok(FollowUpDraft { email, cost_usd })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Value {
        json!({
            "overview": "Asha wants an MVP for a logistics app.",
            "prospect": { "who": "Asha, CTO at ShipFast", "needs": ["MVP in 8 weeks", "AWS"], "current_setup": "Spreadsheets" },
            "pain_points": ["Manual dispatch"],
            "objections": [{ "objection": "Price", "response": "Phased scope" }],
            "qualification": { "budget": "Not discussed", "timeline": "Q1", "decision_process": "CTO decides" },
            "commitments": [
                { "owner": "us", "action": "Send a scoped proposal by Friday" },
                { "owner": "prospect", "action": "Share the current data model" }
            ],
            "next_step": "Proposal review call next Tuesday",
            "sentiment": "positive"
        })
    }

    #[test]
    fn the_email_is_built_from_the_summary_and_who_the_call_was_with() {
        let brief = CallBrief {
            prospect: "Asha".into(),
            company: "ShipFast".into(),
            ..Default::default()
        };
        let text = prompt(&summary(), &brief, Some("Alex"));
        for expected in [
            "Asha",
            "ShipFast",
            "Alex",
            "Proposal review call next Tuesday",
            "Send a scoped proposal by Friday",
            "Share the current data model",
            "MVP in 8 weeks",
        ] {
            assert!(text.contains(expected), "{expected} missing from:\n{text}");
        }
        assert!(!text.to_lowercase().contains("transcript"));
    }

    #[test]
    fn without_a_name_or_brief_it_still_works() {
        let text = prompt(&summary(), &CallBrief::default(), None);
        assert!(text.contains("Proposal review call next Tuesday"));
        assert!(!text.contains("Rep's name"));
    }

    #[test]
    fn reads_the_answer_and_rejects_an_empty_email() {
        assert_eq!(
            parse(r#"{"subject":" Next steps for ShipFast ","body":"Hi Asha,\n\nThanks..."}"#),
            Ok(FollowUp {
                subject: "Next steps for ShipFast".into(),
                body: "Hi Asha,\n\nThanks...".into()
            })
        );
        assert!(parse(r#"{"subject":"Hi","body":"   "}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn a_draft_comes_back_from_the_summary_model() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (sent_tx, sent_rx) = tokio::sync::oneshot::channel::<String>();
            tokio::spawn(async move {
                let (mut tcp, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let mut buf = [0u8; 65536];
                // Read headers and the whole body before answering.
                loop {
                    let n = tcp.read(&mut buf).await.unwrap();
                    data.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&data).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length: usize = text[..end]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                            .unwrap_or(0);
                        if data.len() >= end + 4 + length {
                            let _ = sent_tx.send(text);
                            break;
                        }
                    }
                }
                let body = r#"{"content":[{"type":"text","text":"{\"subject\":\"Next steps\",\"body\":\"Hi Asha, thanks for today.\"}"}],"stop_reason":"end_turn","usage":{"input_tokens":900,"output_tokens":80}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                tcp.write_all(response.as_bytes()).await.unwrap();
            });
            let provider = LlmProvider::Anthropic {
                access: crate::llm::Access::Backend { url, token: "t".into() },
                model: "claude-sonnet-5".into(),
            };
            let brief = CallBrief { prospect: "Asha".into(), ..Default::default() };
            let (email, usage) =
                draft(&reqwest::Client::new(), &provider, &summary(), &brief, Some("Alex"))
                    .await
                    .unwrap();
            assert_eq!(email.subject, "Next steps");
            assert_eq!(email.body, "Hi Asha, thanks for today.");
            assert_eq!((usage.input_tokens, usage.output_tokens), (900, 80));
            let sent = sent_rx.await.unwrap();
            assert!(sent.starts_with("POST /llm/messages "), "goes through the backend: {sent}");
            assert!(sent.contains("claude-sonnet-5"));
            assert!(sent.contains("Proposal review call next Tuesday"));
        });
    }
}
