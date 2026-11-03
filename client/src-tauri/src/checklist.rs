//! Ticks off the opening hints as the call covers them, so the rep's checklist shows only what's
//! still to do. It runs beside the coach and never in its way: after someone finishes speaking, a
//! small request of its own asks which open items are now covered. No hint waits for it.

use crate::audio::Channel;
use crate::capture::emit_for_session;
use crate::coach::CoachInput;
use crate::llm::{LlmProvider, LlmRequest, Role, Turn, Usage};
use crate::trigger::Line;
use serde::Deserialize;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tokio::sync::mpsc::{UnboundedReceiver, WeakUnboundedSender};

pub enum ChecklistInput {
    /// The opening hints, once the coach has them: (id, text).
    Items(Vec<(String, String)>),
    /// A final transcript line, with the rep as "me".
    Line(Line),
}

/// Quiet after the latest line before checking, so one check covers a whole turn rather than
/// every fragment of it.
const CHECK_AFTER: Duration = Duration::from_millis(1500);
/// Lines of the call the check sees: enough for a question and its answer.
const CONTEXT_LINES: usize = 8;
const CHECK_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_TOKENS: u32 = 50;

const SYSTEM_PROMPT: &str = r#"You keep a sales rep's checklist during a live call.

Each message lists the checklist items still open, numbered, and the latest lines of the call. "me" is the rep, "them" is the prospect. Lines come from speech recognition and may contain misheard words.

Reply with the numbers of the items the call has now covered: the rep asked that question or raised that point, in any words, or the prospect already gave that answer. Only items that are clearly covered; when unsure, leave the item open. Most of the time nothing new is covered: reply with an empty list.

Respond with JSON: {"covered": [numbers]}"#;

static SCHEMA: LazyLock<serde_json::Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "covered": { "type": "array", "items": { "type": "integer" } }
        },
        "required": ["covered"],
        "additionalProperties": false
    })
});

#[derive(Deserialize)]
struct Covered {
    covered: Vec<i64>,
}

pub async fn run(
    app: AppHandle,
    session_id: u64,
    provider: LlmProvider,
    mut inputs: UnboundedReceiver<ChecklistInput>,
    // Weak: the coach finishes when the call does, whatever this is still doing.
    coach: WeakUnboundedSender<CoachInput>,
) {
    let client = crate::backend::client().unwrap_or_else(|_| reqwest::Client::new());
    // Items still open, in the order they were shown.
    let mut open: Vec<(String, String)> = Vec::new();
    let mut recent: VecDeque<Line> = VecDeque::with_capacity(CONTEXT_LINES);
    let mut heard_since_check = false;
    let mut due: Option<Instant> = None;

    loop {
        let wait = async move {
            match due {
                Some(due) => tokio::time::sleep_until(due.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            input = inputs.recv() => match input {
                None => break,
                Some(ChecklistInput::Items(items)) => {
                    open = items;
                    if heard_since_check && !open.is_empty() {
                        due = Some(Instant::now() + CHECK_AFTER);
                    }
                }
                Some(ChecklistInput::Line(line)) => {
                    if recent.len() == CONTEXT_LINES {
                        recent.pop_front();
                    }
                    recent.push_back(line);
                    heard_since_check = true;
                    if !open.is_empty() {
                        due = Some(Instant::now() + CHECK_AFTER);
                    }
                }
            },
            _ = wait => {
                due = None;
                heard_since_check = false;
                let started = Instant::now();
                let result = check(&client, &provider, &open, &recent).await;
                let ms = started.elapsed().as_millis() as u64;
                let (covered, usage, error) = match result {
                    Ok((numbers, usage)) => (take_covered(&mut open, &numbers), Some(usage), None),
                    Err(message) => (Vec::new(), None, Some(message)),
                };
                if !covered.is_empty() {
                    emit_for_session(&app, session_id, "opening-covered", &covered);
                }
                if let Some(coach) = coach.upgrade() {
                    let _ = coach.send(CoachInput::ChecklistCheck { covered, usage, ms, error });
                }
            }
        }
    }
}

async fn check(
    client: &reqwest::Client,
    provider: &LlmProvider,
    open: &[(String, String)],
    recent: &VecDeque<Line>,
) -> Result<(Vec<i64>, Usage), String> {
    let turns = [Turn {
        role: Role::User,
        text: prompt(open, recent),
    }];
    let request = LlmRequest {
        system: SYSTEM_PROMPT,
        turns: &turns,
        schema: &SCHEMA,
        max_tokens: MAX_TOKENS,
        // Under the model's minimum cacheable length; caching would only add a write charge.
        cache: false,
        effort: None,
        temperature: None,
        timeout: CHECK_TIMEOUT,
        hedge_after: None,
    };
    let response = provider.generate(client, &request).await?;
    let parsed: Covered = serde_json::from_str(&response.text)
        .map_err(|e| format!("Checklist answer was not valid JSON: {e}"))?;
    Ok((parsed.covered, response.usage))
}

fn prompt(open: &[(String, String)], recent: &VecDeque<Line>) -> String {
    let items: Vec<String> = open
        .iter()
        .enumerate()
        .map(|(i, (_, text))| format!("{}. {text}", i + 1))
        .collect();
    let lines: Vec<String> = recent
        .iter()
        .map(|line| {
            let who = match line.channel {
                Channel::Me => "me",
                Channel::Them => "them",
            };
            format!("{who}: {}", line.text)
        })
        .collect();
    format!(
        "Open items:\n{}\n\nLatest lines:\n{}",
        items.join("\n"),
        lines.join("\n")
    )
}

/// Removes the items the answer numbered (1-based, as shown) and returns their ids. Numbers out
/// of range are ignored.
fn take_covered(open: &mut Vec<(String, String)>, numbers: &[i64]) -> Vec<String> {
    let mut indexes: Vec<usize> = numbers
        .iter()
        .filter_map(|&n| usize::try_from(n).ok()?.checked_sub(1))
        .filter(|&i| i < open.len())
        .collect();
    indexes.sort_unstable();
    indexes.dedup();
    // Removed from the back, so the indexes still to remove don't shift.
    let mut ids: Vec<String> = indexes
        .into_iter()
        .rev()
        .map(|i| open.remove(i).0)
        .collect();
    ids.reverse();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<(String, String)> {
        ["Ask about the budget", "Send the case study", "Book a demo"]
            .iter()
            .enumerate()
            .map(|(i, text)| (format!("s#open-{}", i + 1), text.to_string()))
            .collect()
    }

    #[test]
    fn covered_numbers_remove_those_items_and_ignore_nonsense() {
        let mut open = items();
        let covered = take_covered(&mut open, &[3, 1, 1, 0, 9, -2]);
        assert_eq!(covered, ["s#open-1", "s#open-3"]);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].1, "Send the case study");
        assert!(take_covered(&mut open, &[]).is_empty());
    }

    #[test]
    fn the_prompt_numbers_open_items_and_labels_speakers() {
        let recent: VecDeque<Line> = [
            (Channel::Me, "What budget do you have in mind?"),
            (Channel::Them, "Around ten lakh."),
        ]
        .into_iter()
        .map(|(channel, text)| Line {
            channel,
            text: text.into(),
        })
        .collect();
        let text = prompt(&items(), &recent);
        assert!(text.contains("1. Ask about the budget\n2. Send the case study"));
        assert!(text.ends_with("me: What budget do you have in mind?\nthem: Around ten lakh."));
    }

    /// Live: one check against the configured provider.
    /// `cargo test checklist_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn checklist_live() {
        let provider = LlmProvider::from_env().expect("provider");
        let recent: VecDeque<Line> = [
            (Channel::Them, "Hi, good to talk again."),
            (
                Channel::Me,
                "Likewise. Before anything, what budget are you working with this quarter?",
            ),
            (Channel::Them, "We set aside about ten lakh for this."),
        ]
        .into_iter()
        .map(|(channel, text)| Line {
            channel,
            text: text.into(),
        })
        .collect();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let started = Instant::now();
        let (covered, usage) = runtime
            .block_on(check(&reqwest::Client::new(), &provider, &items(), &recent))
            .expect("check");
        println!(
            "{:?} in {} ms, {} in / {} out tokens",
            covered,
            started.elapsed().as_millis(),
            usage.input_tokens,
            usage.output_tokens
        );
        assert_eq!(covered, [1]);
    }
}
