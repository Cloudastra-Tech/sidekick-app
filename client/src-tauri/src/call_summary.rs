//! Post-call summary: the full transcript goes to Claude Sonnet 5 once the call ends and
//! comes back as a structured summary (needs, objections, qualification, next steps).

use crate::audio::Channel;
use crate::llm::{LlmProvider, LlmRequest, Role, Turn, Usage};
use crate::trigger::Line;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

/// Below this many final lines there is nothing worth summarizing.
pub const MIN_LINES: usize = 2;

const MAX_TOKENS: u32 = 8000;
const TIMEOUT: Duration = Duration::from_secs(90);

const SYSTEM_PROMPT: &str = r#"You write post-call summaries for a sales rep.

The transcript comes from live speech recognition: "me" is the sales rep, "them" is the prospect side (possibly several people). Words may be misheard; infer meaning where it's clear, and don't invent details that weren't said.

Keep every field short and specific, using the prospect's own numbers, names, and dates. When something wasn't covered, write "Not discussed". Use the playbook only to understand what the rep's company sells, never as a source of facts about this call."#;

static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    let text = || json!({ "type": "string" });
    let list = || json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["overview", "prospect", "pain_points", "objections", "qualification",
                     "commitments", "next_step", "sentiment"],
        "properties": {
            "overview": text(),
            "prospect": {
                "type": "object",
                "additionalProperties": false,
                "required": ["who", "needs", "current_setup"],
                "properties": { "who": text(), "needs": list(), "current_setup": text() }
            },
            "pain_points": list(),
            "objections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["objection", "response"],
                    "properties": { "objection": text(), "response": text() }
                }
            },
            "qualification": {
                "type": "object",
                "additionalProperties": false,
                "required": ["budget", "timeline", "decision_process"],
                "properties": {
                    "budget": text(), "timeline": text(), "decision_process": text()
                }
            },
            "commitments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["owner", "action"],
                    "properties": {
                        "owner": { "type": "string", "enum": ["us", "prospect"] },
                        "action": text()
                    }
                }
            },
            "next_step": text(),
            "sentiment": { "type": "string", "enum": ["positive", "neutral", "negative"] }
        }
    })
});

pub struct CallSummary {
    pub summary: Value,
    pub usage: Usage,
}

pub async fn generate(
    client: &reqwest::Client,
    provider: &LlmProvider,
    playbook: &str,
    // Notes the rep entered before the call.
    brief: Option<&str>,
    transcript: &[Line],
) -> Result<CallSummary, String> {
    let mut system = format!("{SYSTEM_PROMPT}\n\n--- Playbook (context only) ---\n\n{playbook}");
    if let Some(brief) = brief {
        system.push_str(&format!(
            "\n\n--- Rep's notes from before the call (use for names and context; the \
             transcript is the source of what was said) ---\n\n{brief}"
        ));
    }
    let turns = [Turn {
        role: Role::User,
        text: format!("Transcript:\n{}", format_transcript(transcript)),
    }];
    let request = LlmRequest {
        system: &system,
        turns: &turns,
        schema: &SCHEMA,
        max_tokens: MAX_TOKENS,
        cache: false,
        effort: Some("low"),
        temperature: None,
        timeout: TIMEOUT,
        hedge_after: None,
    };

    // In testing, some responses came back schema-valid but empty (blank strings and
    // lists), so an empty overview is treated as a failed attempt and retried once.
    // Network errors and rate limits / server errors are retried with a short backoff; the
    // transcript is already on disk, so this only affects whether a summary gets written.
    let mut usage = Usage::default();
    for _ in 0..ATTEMPTS {
        let response = generate_with_retries(client, provider, &request).await?;
        usage.input_tokens += response.usage.input_tokens;
        usage.output_tokens += response.usage.output_tokens;
        let summary: Value = serde_json::from_str(&response.text)
            .map_err(|e| format!("Call summary was not valid JSON: {e}"))?;
        if is_substantive(&summary) {
            return Ok(CallSummary { summary, usage });
        }
    }
    Err(format!(
        "Call summary came back empty after {ATTEMPTS} attempts"
    ))
}

const ATTEMPTS: usize = 2;
const TRANSIENT_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(6)];

async fn generate_with_retries(
    client: &reqwest::Client,
    provider: &LlmProvider,
    request: &LlmRequest<'_>,
) -> Result<crate::llm::LlmResponse, String> {
    let mut delays = TRANSIENT_RETRY_DELAYS.iter();
    loop {
        match provider.generate(client, request).await {
            Err(error) if is_transient(&error) => match delays.next() {
                Some(delay) => tokio::time::sleep(*delay).await,
                None => return Err(error),
            },
            result => return result,
        }
    }
}

/// Connection failures, timeouts, rate limits, and server errors are worth retrying;
/// bad requests and refusals are not.
fn is_transient(error: &str) -> bool {
    error.starts_with("Claude request failed")
        || error.starts_with("Claude HTTP 429")
        || error.starts_with("Claude HTTP 5")
}

fn is_substantive(summary: &Value) -> bool {
    summary["overview"]
        .as_str()
        .is_some_and(|overview| overview.trim().len() >= 20)
}

fn format_transcript(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| {
            let who = match line.channel {
                Channel::Me => "me",
                Channel::Them => "them",
            };
            format!("{who}: {}", line.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The transcript on its own, saved as soon as the call ends.
pub fn transcript_markdown(title: &str, transcript: &[Line]) -> String {
    format!(
        "# {title}\n\nSummary not written yet.\n\n## Transcript\n```\n{}\n```\n",
        format_transcript(transcript)
    )
}

/// Renders the structured summary as a Markdown document for saving and sharing.
pub fn to_markdown(summary: &Value, title: &str, transcript: &[Line]) -> String {
    let text = |v: &Value| v.as_str().unwrap_or("Not discussed").to_string();
    let bullets = |v: &Value| -> String {
        let items: Vec<String> = v
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| format!("- {s}"))
            .collect();
        if items.is_empty() {
            "- Not discussed".into()
        } else {
            items.join("\n")
        }
    };

    let objections: Vec<String> = summary["objections"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| format!("- **{}** → {}", text(&o["objection"]), text(&o["response"])))
        .collect();
    let commitments: Vec<String> = summary["commitments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let owner = if c["owner"] == "prospect" {
                "Prospect"
            } else {
                "Us"
            };
            format!("- [ ] **{owner}:** {}", text(&c["action"]))
        })
        .collect();
    let or_none = |items: Vec<String>| {
        if items.is_empty() {
            "- None".to_string()
        } else {
            items.join("\n")
        }
    };

    format!(
        "# {title}\n\n\
         **Sentiment:** {sentiment}\n\n\
         ## Overview\n{overview}\n\n\
         ## Prospect\n**Who:** {who}\n\n**Current setup:** {setup}\n\n**Needs:**\n{needs}\n\n\
         ## Pain points\n{pains}\n\n\
         ## Objections\n{objections}\n\n\
         ## Qualification\n| Budget | Timeline | Decision process |\n|---|---|---|\n| {budget} | {timeline} | {decision} |\n\n\
         ## Commitments\n{commitments}\n\n\
         ## Next step\n{next_step}\n\n\
         ## Transcript\n```\n{transcript}\n```\n",
        sentiment = text(&summary["sentiment"]),
        overview = text(&summary["overview"]),
        who = text(&summary["prospect"]["who"]),
        setup = text(&summary["prospect"]["current_setup"]),
        needs = bullets(&summary["prospect"]["needs"]),
        pains = bullets(&summary["pain_points"]),
        objections = or_none(objections),
        budget = text(&summary["qualification"]["budget"]),
        timeline = text(&summary["qualification"]["timeline"]),
        decision = text(&summary["qualification"]["decision_process"]),
        commitments = or_none(commitments),
        next_step = text(&summary["next_step"]),
        transcript = format_transcript(transcript),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_errors_are_retried_and_others_are_not() {
        assert!(is_transient("Claude request failed: error sending request"));
        assert!(is_transient(
            "Claude HTTP 429 Too Many Requests: rate limited"
        ));
        assert!(is_transient(
            "Claude HTTP 529 <unknown status code>: overloaded"
        ));
        assert!(!is_transient("Claude HTTP 400 Bad Request: invalid schema"));
        assert!(!is_transient("Claude declined to answer this request"));
    }

    #[test]
    fn transcript_markdown_includes_every_line() {
        let lines = [
            Line {
                channel: Channel::Me,
                text: "Hello".into(),
            },
            Line {
                channel: Channel::Them,
                text: "Too expensive.".into(),
            },
        ];
        let md = transcript_markdown("Call transcript", &lines);
        assert!(md.contains("me: Hello"));
        assert!(md.contains("them: Too expensive."));
    }

    #[test]
    fn markdown_renders_all_sections_and_handles_empty_lists() {
        let summary = json!({
            "overview": "Prospect wants lower AI costs.",
            "prospect": { "who": "Startup founder", "needs": ["Cut OpenAI bill"], "current_setup": "Chatbot on OpenAI" },
            "pain_points": [],
            "objections": [{ "objection": "Too expensive", "response": "From $18/hour" }],
            "qualification": { "budget": "Not discussed", "timeline": "Next month", "decision_process": "Co-founder" },
            "commitments": [{ "owner": "us", "action": "Send email" }],
            "next_step": "Call Thursday",
            "sentiment": "positive"
        });
        let lines = [Line {
            channel: Channel::Them,
            text: "Too expensive.".into(),
        }];
        let md = to_markdown(&summary, "Call summary", &lines);
        for expected in [
            "# Call summary",
            "**Sentiment:** positive",
            "- Cut OpenAI bill",
            "## Pain points\n- Not discussed",
            "- **Too expensive** → From $18/hour",
            "| Not discussed | Next month | Co-founder |",
            "- [ ] **Us:** Send email",
            "them: Too expensive.",
        ] {
            assert!(md.contains(expected), "missing {expected:?} in:\n{md}");
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Live: `cargo test live_call_summary -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_call_summary() {
        let provider = LlmProvider::summary_from_env().expect("summary provider");
        let said = |channel, text: &str| Line {
            channel,
            text: text.into(),
        };
        let transcript = [
            said(Channel::Me, "What made you reach out?"),
            said(
                Channel::Them,
                "Our OpenAI bill is around four thousand dollars a month.",
            ),
            said(
                Channel::Them,
                "Honestly consultants are expensive for a startup like us.",
            ),
            said(
                Channel::Me,
                "Senior AI engineering starts at eighteen dollars an hour, no lock-in.",
            ),
            said(
                Channel::Them,
                "Our investor demo is next month. I need to check with my co-founder.",
            ),
            said(
                Channel::Me,
                "Can we do a call Thursday with your co-founder?",
            ),
            said(
                Channel::Them,
                "Thursday works, send me an email with details.",
            ),
        ];
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let started = std::time::Instant::now();
        let call = runtime
            .block_on(generate(
                &reqwest::Client::new(),
                &provider,
                "Acme Cloud sells cloud engineering.",
                None,
                &transcript,
            ))
            .expect("summary");
        println!(
            "{} ms, in {} out {}\nRAW {}\n{}",
            started.elapsed().as_millis(),
            call.usage.input_tokens,
            call.usage.output_tokens,
            call.summary,
            to_markdown(&call.summary, "Call summary", &transcript)
        );
    }
}
