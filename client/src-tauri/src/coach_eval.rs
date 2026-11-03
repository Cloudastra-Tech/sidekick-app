//! Hint quality check against the real model: replays a realistic discovery call (English with
//! Hindi mixed in) through the live-hint prompt, the way a call sends it, and prints every hint,
//! how many turns got none, repeats, and how long each took.
//!
//! Signed in: point `EVAL_APP_DATA` at the app's data folder and requests go through the backend,
//! as in the app. `EVAL_FRAGMENTS=1` splits the prospect's turns at commas and full stops, the way
//! a 300 ms pause cut them before turns had to settle.
//!
//! `EVAL_APP_DATA="$HOME/Library/Application Support/com.sidekick.callcoach" \
//!   cargo test hint_quality_eval -- --ignored --nocapture`

use super::*;

/// (rep's line, prospect's turn). A first meeting: the CRM knows only the web form.
const CALL: &[(&str, &str)] = &[
    (
        "Hi Rahul, thanks for making time. What made you fill in the form?",
        "Haan, so basically our OpenAI bill has gone crazy, last month it was around eleven thousand dollars.",
    ),
    (
        "Eleven thousand, okay. What's driving most of that?",
        "Mostly the support chatbot, it answers loan questions for our customers, and the prompts are huge because we send the whole policy document every time.",
    ),
    (
        "Got it. How many conversations a month?",
        "Around two lakh conversations, and it is growing every month, so the bill will only go up.",
    ),
    (
        "And who built the chatbot?",
        "Hamare do developers hain, they built it in three months, but they are not AI specialists, they are backend people.",
    ),
    (
        "Makes sense.",
        "Honestly we tried caching once but it broke some answers, so we turned it off.",
    ),
    (
        "What broke exactly?",
        "Customers were getting old interest rates, the policy changed but the cached answer didn't.",
    ),
    (
        "That's a common trap.",
        "Yes. So what would you guys do differently? Aur kitna time lagega?",
    ),
    (
        "We'd usually start by looking at the whole setup.",
        "Okay but I need to be careful, consultants are expensive for a startup our size.",
    ),
    (
        "Understood.",
        "We have a board meeting in six weeks, and I want to show them the cost is under control.",
    ),
    (
        "Six weeks, okay.",
        "Also we're on AWS, and our CTO wants everything to stay in our own account, no data leaving.",
    ),
    (
        "That's fine, you'd keep everything.",
        "Achha. And can you show me some similar work, any fintech client?",
    ),
    (
        "Yes, we've done fintech before.",
        "Okay. Mujhe apne co-founder Meera se baat karni padegi, she handles the budget.",
    ),
    (
        "Sure.",
        "So what is the next step from your side?",
    ),
    (
        "",
        "Hello? Sorry, my network dropped for a second. Can you hear me?",
    ),
    (
        "Yes, I can hear you.",
        "Okay good. Send me something on email and I will discuss with Meera.",
    ),
];

fn brief() -> CallBrief {
    CallBrief {
        prospect: "Rahul Mehta".into(),
        company: "FinStack".into(),
        agenda: "First call. Understand their AI costs, qualify budget and timeline, book the \
                 next step."
            .into(),
        crm_context:
            "- Person: Rahul Mehta, Head of Engineering at FinStack (lending, 40 people)\n\
                      - Source: website form: \"Our LLM costs are growing faster than our users\""
                .into(),
        ..CallBrief::default()
    }
}

/// Splits a turn the way short pauses did: at commas and full stops.
fn fragments(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        current.push(c);
        if matches!(c, ',' | '.' | '?') && current.trim().len() > 12 {
            parts.push(current.trim().to_string());
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

fn trigger(lines: Vec<(Channel, String)>) -> Trigger {
    let now = Instant::now();
    Trigger {
        reason: TriggerReason::TurnEnd,
        lines: lines
            .into_iter()
            .map(|(channel, text)| Line { channel, text })
            .collect(),
        timing: TriggerTiming {
            speech_end_at: None,
            transcript_at: now,
            fired_at: now,
        },
    }
}

#[test]
#[ignore]
fn hint_quality_eval() {
    if let Ok(dir) = std::env::var("EVAL_APP_DATA") {
        crate::config::init(PathBuf::from(dir));
    }
    let settings = CoachSettings::from_env()
        .expect("coach settings")
        .with_brief(brief());
    let split = std::env::var("EVAL_FRAGMENTS").is_ok_and(|v| v == "1");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = reqwest::Client::new();

    let mut triggers = Vec::new();
    for (me, them) in CALL {
        let mut lines = Vec::new();
        if !me.is_empty() {
            lines.push((Channel::Me, me.to_string()));
        }
        if split {
            for (i, part) in fragments(them).into_iter().enumerate() {
                if i > 0 {
                    triggers.push(trigger(std::mem::take(&mut lines)));
                }
                lines.push((Channel::Them, part));
            }
        } else {
            lines.push((Channel::Them, them.to_string()));
        }
        triggers.push(trigger(lines));
    }

    let mut history = vec![Turn {
        role: Role::User,
        text: OPENING_TURN.into(),
    }];
    let opening = runtime
        .block_on(
            settings
                .provider
                .generate(&client, &opening_request(&settings.system, &history)),
        )
        .expect("opening");
    let mut shown: Vec<String> = HintsJson::shown(&opening.text, MAX_OPENING_HINTS)
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.text)
        .collect();
    println!("OPENING: {shown:?}\n");
    history.push(Turn {
        role: Role::Model,
        text: opening.text,
    });

    let (mut none, mut repeats, mut long) = (0, 0, 0);
    let (mut first_words, mut totals) = (Vec::new(), Vec::new());
    for trigger in &triggers {
        history.push(Turn {
            role: Role::User,
            text: format_turn(trigger),
        });
        let started = Instant::now();
        let mut draft = HintStream::new("eval".into());
        let response = runtime
            .block_on(settings.provider.stream(
                &client,
                &hint_request(&settings.system, &history),
                &mut |piece| {
                    draft.on_piece(piece);
                },
            ))
            .expect("hint");
        let total = started.elapsed().as_millis() as u64;
        totals.push(total);
        if let Some(at) = draft.first_words_at {
            first_words.push(at.duration_since(started).as_millis() as u64);
        }
        let hint = HintsJson::shown(&response.text, 1)
            .and_then(|hints| hints.into_iter().next())
            .map(|h| h.text);
        let said: Vec<_> = trigger.lines.iter().map(|l| l.text.as_str()).collect();
        let shown_text = match &hint {
            None => {
                none += 1;
                "—".to_string()
            }
            Some(text) => {
                let repeat = shown.iter().find(|earlier| similar_hint(text, earlier));
                if let Some(earlier) = repeat {
                    repeats += 1;
                    format!("{text}   [REPEAT of: {earlier}]")
                } else {
                    text.clone()
                }
            }
        };
        println!(
            "{total:>5} ms | cache read {:>5} | them: {}\n         => {shown_text}",
            response.usage.cache_read_tokens,
            said.last().unwrap_or(&"")
        );
        if let Some(text) = hint {
            if text.split_whitespace().count() > 15 {
                long += 1;
            }
            shown.push(text);
        }
        history.push(Turn {
            role: Role::Model,
            text: response.text,
        });
    }
    let pct = |v: &mut Vec<u64>, p: f64| {
        v.sort_unstable();
        v.get(((v.len() as f64 - 1.0) * p).round() as usize)
            .copied()
    };
    println!(
        "\n{} triggers | no hint {none} | repeats {repeats} | over 15 words {long} | first words p50 {:?} p90 {:?} | \
         total p50 {:?} p90 {:?}",
        triggers.len(),
        pct(&mut first_words, 0.5),
        pct(&mut first_words, 0.9),
        pct(&mut totals, 0.5),
        pct(&mut totals, 0.9),
    );
}
