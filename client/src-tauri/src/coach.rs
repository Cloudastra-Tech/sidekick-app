//! Live coaching loop: turns triggers into hints via the LLM and records latency and cost
//! for every call, so Phase 2 numbers are measured rather than estimated.
//!
//! Prompt shape (prefix-cache friendly): a static system prompt + playbook, then an
//! append-only conversation where each user turn holds only the lines that are new since
//! the previous trigger. Every request therefore extends the previous request's prefix.

use crate::audio::hearing::AudioReport;
use crate::audio::Channel;
use crate::call_summary;
use crate::capture::emit_for_session;
use crate::checklist::ChecklistInput;
use crate::config::{env_f64, env_or_dotenv, resolve_path};
use crate::journal::{CurrentJournal, Journal};
use crate::llm::{LlmProvider, LlmRequest, Role, Turn, Usage};
use crate::trigger::{Line, Trigger, TriggerReason, TriggerTiming};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, WeakUnboundedSender};
use tokio::sync::oneshot;

const DEFAULT_PLAYBOOK_PATH: &str = "playbook/sales.md";

/// The playbook file in use (`PLAYBOOK_PATH`, or the default).
///
/// The default is the copy shipped with the app, copied into the app data folder the first
/// time: edits from Settings go there, since the installed app's own files are read-only (and
/// changing them would break the Mac app's signature).
pub fn playbook_path() -> PathBuf {
    if let Some(path) = env_or_dotenv("PLAYBOOK_PATH") {
        return resolve_path(&path);
    }
    let shipped = resolve_path(DEFAULT_PLAYBOOK_PATH);
    let Some(own) = crate::config::app_data_dir().map(|d| d.join(DEFAULT_PLAYBOOK_PATH)) else {
        return shipped;
    };
    if !own.exists() {
        let copied = own
            .parent()
            .is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
            && std::fs::copy(&shipped, &own).is_ok();
        if !copied {
            return shipped;
        }
    }
    own
}

const SYSTEM_PROMPT: &str = r#"You are a real-time coach for a sales rep during a live call.

How the conversation works:
- Each user message contains only the transcript lines that are NEW since the previous message. Earlier messages are the call so far.
- "me" is the sales rep. "them" is the prospect.
- Lines come from live speech recognition and may contain misheard words. The prospect may speak Hindi, English, or a mix; always write hints in English.

Respond with JSON: {"hints": [{"type": string, "urgency": string, "text": string}]}
- hints: during the call, at most one hint. [] when nothing useful applies, or when you already gave essentially the same hint earlier in this call. Prefer no hint over a generic one.
- When the trigger is the rep asking ("the rep asked ..."), always give exactly one hint, never []: answer their question if they typed one, otherwise the best thing to say next.
- text: one sentence of at most 15 words that the rep can say or do right now. The rep glances at it mid-conversation, so shorter is better. Prefer a question to ask over a fact to recite.
- type: "question" (a question to ask), "objection" (handling pushback), "buying_signal" (move toward a next step), "reminder" (a call goal not yet covered), "fact" (a playbook fact that answers them).
- urgency: "high" (act on it now), "medium", or "low".

Make every hint about this prospect. A hint that would fit any sales call is a bad hint:
- Tie each hint to something specific: what they just said (their words, numbers, names), and the notes about this call at the end of this prompt: the CRM record, earlier calls (open commitments, objections they raised before, the agreed next step), and the rep's goals for today.
- Returning prospect (the notes include earlier calls): build on that history. When they repeat an earlier objection or topic, connect it ("Last time you said..."); chase what was left open.
- First meeting (no earlier calls): build on the CRM record and today's goals. Discovery questions are welcome, but ask the sharp one a top sales rep would ask right here, built from the prospect's own words and situation, never a textbook question.
- Answering a direct question or objection: use the playbook's facts, framed for this prospect (their size, their problem, what they just said).
- At a pause: if one of today's goals or an open commitment hasn't come up yet, remind the rep of it.
- Personal means using their words and the notes. Never make a hint sound personal with a number, comparison, or claim that isn't in the notes, the transcript, or the playbook.
- If you can't make the hint specific to this prospect, give no hint.

Only use facts from the playbook below. Never invent prices, numbers, clients, or capabilities, and never promise a result the playbook doesn't state ("halve your bill", "cut costs by 40%"). If the playbook marks something [FILL IN], do not state it.

Writing the hint text:
- Count the words before answering. 15 words is the hard limit; 6 to 12 is the target.
- Write the exact words the rep says next, in the first person, ready to read out as written: a question to ask them, or a sentence to say to them. Never an instruction to the rep ("Ask...", "Propose...", "Show...", "Remind them..."): the rep has no time to turn an instruction into words.
- One idea per hint. No lists, no "and also", no second sentence.
- Plain spoken English. No jargon the prospect didn't use first, no marketing adjectives.
- Don't repeat what the prospect just said back to the rep; the rep heard it.
- Don't coach the rep on tone or manners ("be empathetic", "build rapport"). Give words or an action.
- A question hint is a question the rep can read out as written.
- A fact hint states one playbook fact that answers what the prospect just asked or doubted.

The opening message ("Trigger: call starting") arrives before anyone has spoken. Answer it with up to 3 hints, most important first: the lines the rep will say in this call, each one a question or sentence to read out as written.
- Build them from the notes about this call at the end of this prompt: the rep's agenda and goals, what the CRM knows, and earlier calls with this prospect. Pick up what was left open: commitments either side made, objections not yet settled, the agreed next step, and what the rep wants from today.
- Each hint names something specific to this prospect: a person, number, date, commitment, problem, or goal from those notes. Nothing that would fit any sales call.
- With few or no notes, give fewer hints or none. Never pad the list with generic discovery questions.
- Same writing rules as live hints: one idea each, at most 15 words.

When to show nothing (hints: []):
- Small talk, greetings, scheduling logistics, or audio checks ("can you hear me?").
- The rep is already doing the right thing: asking the question you would suggest, or answering the objection well.
- The new lines are only "me" speaking and the prospect hasn't reacted yet, unless there was a long pause and a call goal is still uncovered.
- The lines are too garbled by speech recognition to understand.
- Your hint would be the same idea as one you already gave in this call, even in different words.

Keep the call moving: once you've suggested an offer, a next step, or a question, don't suggest it again unless the prospect brings it back. The next hint answers what they said since, so the rep always has something new to say.
- The only hint you can think of would fit any call.

Choosing urgency:
- high: the prospect raised an objection, asked a direct question the rep must answer, or showed a buying signal. The moment passes in seconds.
- medium: a useful discovery question or a call goal that is still uncovered.
- low: a reminder that can wait for a natural pause.

Examples. The prospects and notes in them are made up, to show how a hint uses the notes; take facts only from the playbook and from the real notes at the end of this prompt.

Suppose the notes say: Priya, Head of Ops at Acme Logistics. Earlier call: she worried outside teams cost too much; budgets are approved in January; we promised her a logistics case study. Today's goal: book a pilot.

Trigger: prospect finished speaking
New lines:
them: Honestly we're not sure this is the right time, budgets are frozen until next quarter.
Good: {"hints": [{"type": "objection", "urgency": "high", "text": "Could a small pilot now be ready for your January budget round?"}]}
Bad (fits any call): "What would need to be true to start next quarter?"

Trigger: keyword "expensive"
New lines:
them: I'll be honest, outside teams have always been expensive for us.
Good: {"hints": [{"type": "objection", "urgency": "high", "text": "You raised cost last time too; a small pilot caps the spend."}]}
Bad (a playbook fact that ignores her history): "We're cheaper than most agencies and have no lock-in."

Trigger: keyword "timeline"
New lines:
them: Our timeline is pretty tight, the launch is in six weeks.
Good: {"hints": [{"type": "buying_signal", "urgency": "high", "text": "Could we start the pilot this week, so it fits your six-week launch?"}]}

Trigger: pause in conversation
New lines:
me: So that's a quick overview of how we usually work.
Good: {"hints": [{"type": "reminder", "urgency": "medium", "text": "I have the logistics case study I promised you; shall I send it today?"}]}
Bad (fits any call): "Who else needs to be involved in the decision?"
Bad (an instruction, not words to say): "Remind her about the case study we promised."

Trigger: prospect finished speaking
New lines:
me: What does your current setup look like?
them: We have a small team, three developers, mostly working on the backend.
Good: {"hints": [{"type": "question", "urgency": "medium", "text": "With three backend developers, who would own the pilot day to day?"}]}
Bad (repeats the prospect back): "They have three backend developers on a small team."
Bad (textbook): "Where does the team get stuck most today?"

Trigger: prospect finished speaking
New lines:
them: Sorry, you broke up for a second there. Can you repeat that?
Good: {"hints": []}

Now suppose a first meeting, no earlier calls. The CRM says: Ananya, founder of a 12-person edtech; web form: "need an MVP of our parents' app before the March school term". Today's goal: scope the MVP.

Trigger: prospect finished speaking
New lines:
them: Right now parents just get WhatsApp messages from teachers, it's a mess.
Good: {"hints": [{"type": "question", "urgency": "medium", "text": "Which WhatsApp message should the app replace first by March?"}]}
Bad (textbook): "Can you walk me through your current process?"

Trigger: keyword "price"
New lines:
them: What would something like this cost us, roughly?
Good: a fact hint with the playbook's actual pricing, framed for her MVP, or if the playbook has no price for this, {"hints": [{"type": "question", "urgency": "high", "text": "Which parent features must be in the March MVP? Then I can quote properly."}]}
Bad (invented): "It's usually around ten thousand dollars." Never state a number the playbook doesn't give."#;

pub enum CoachInput {
    Trigger(Trigger),
    /// A window painted this hint: the end of the audio-to-screen latency.
    HintPainted {
        hint_id: String,
        at: Instant,
    },
    /// The checklist asked which opening hints the call has covered, beside the coach.
    ChecklistCheck {
        covered: Vec<String>,
        usage: Option<Usage>,
        ms: u64,
        error: Option<String>,
    },
    SttUsage {
        channel: Channel,
        audio_seconds: f64,
    },
    /// Process CPU percent (of total machine capacity) over the last sampling interval.
    CpuSample(f64),
    /// How long after its audio was captured a transcript result arrived from STT.
    SttLatency {
        ms: u64,
        is_final: bool,
    },
    /// A final transcript line: counted per capture channel (so an empty channel is visible
    /// in logs) and kept for the post-call summary under `speaker` (the prospect in solo test).
    FinalTranscript {
        channel: Channel,
        speaker: Channel,
        text: String,
    },
    /// A capture or STT stream failed. Logged so failures aren't only visible in the UI.
    StreamError {
        channel: Channel,
        stage: &'static str,
        message: String,
    },
    /// The speaker or headset system audio is captured from, read as the call starts.
    OutputDevice(Option<String>),
    /// The rep was told the prospect can't be heard (`rep_speech_seconds` of talk with nothing
    /// from them), or that they're heard again (`None`).
    ProspectNotHeard {
        rep_speech_seconds: Option<f64>,
    },
    /// Each side's levels over the call, once capture has ended.
    Audio(AudioReport),
}

/// Deepgram's listed rates include its Model Improvement Program discount; opting out pays
/// double unless an explicit opt-out rate is set.
fn stt_price_per_minute(listed: Option<f64>, opt_out: Option<f64>, opted_out: bool) -> Option<f64> {
    if opted_out {
        opt_out.or(listed.map(|p| p * 2.0))
    } else {
        listed
    }
}

#[derive(Clone, Copy, Default)]
struct Prices {
    input_per_mtok: Option<f64>,
    cache_write_per_mtok: Option<f64>,
    cache_read_per_mtok: Option<f64>,
    output_per_mtok: Option<f64>,
    stt_per_minute: Option<f64>,
}

impl Prices {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            input_per_mtok: env_f64("LLM_PRICE_INPUT_PER_MTOK")?,
            cache_write_per_mtok: env_f64("LLM_PRICE_CACHE_WRITE_PER_MTOK")?,
            cache_read_per_mtok: env_f64("LLM_PRICE_CACHE_READ_PER_MTOK")?,
            output_per_mtok: env_f64("LLM_PRICE_OUTPUT_PER_MTOK")?,
            stt_per_minute: stt_price_per_minute(
                env_f64("STT_PRICE_PER_MINUTE")?,
                env_f64("STT_PRICE_PER_MINUTE_OPT_OUT")?,
                crate::stt::deepgram_mip_opt_out(),
            ),
        })
    }

    fn llm_cost(&self, usage: &Usage) -> Option<f64> {
        let uncached = usage
            .input_tokens
            .saturating_sub(usage.cache_read_tokens + usage.cache_write_tokens)
            as f64;
        Some(
            (uncached * self.input_per_mtok?
                + usage.cache_write_tokens as f64 * self.cache_write_per_mtok?
                + usage.cache_read_tokens as f64 * self.cache_read_per_mtok?
                + usage.output_tokens as f64 * self.output_per_mtok?)
                / 1_000_000.0,
        )
    }
}

/// What the rep entered before the call. Every field is optional.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CallBrief {
    #[serde(default)]
    pub prospect: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub agenda: String,
    /// The CRM records this call belongs to, picked in the start dialog. They travel with the
    /// call so the write-back lands on that deal rather than on whatever the names match.
    #[serde(default)]
    pub crm_person_id: Option<String>,
    #[serde(default)]
    pub crm_company_id: Option<String>,
    #[serde(default)]
    pub crm_opportunity_id: Option<String>,
    /// What the CRM already knows about them, read once before the call. It sits in the cached
    /// prefix, so the first hint already knows where the deal stands.
    #[serde(default)]
    pub crm_context: String,
    /// Summaries of earlier calls with this prospect, from anyone in the workspace, read from the
    /// backend before the call. Also cached, and what the opening hints are mostly built from.
    #[serde(default)]
    pub past_calls: String,
    /// The files and notes the team keeps about their company, read before the call. Cached
    /// with the rest; left out of the saved call, which would otherwise carry pages of it.
    #[serde(default, skip_serializing)]
    pub client_files: String,
}

/// Longest agenda sent to the models; keeps a pasted document from bloating every request.
const MAX_AGENDA_CHARS: usize = 2000;
const MAX_NAME_CHARS: usize = 120;
/// The CRM history is cached, so it's paid for once per call; still, a deal with years of notes
/// shouldn't crowd out the playbook.
const MAX_CRM_CONTEXT_CHARS: usize = 3000;
/// About three calls' summaries; a longer history keeps the newest calls.
const MAX_PAST_CALLS_CHARS: usize = 4000;
/// The server caps a client at 18,000 words; this is that much text with some room. It is the
/// bulk of the cached prefix when present (about 24k tokens at the cap).
const MAX_CLIENT_FILES_CHARS: usize = 130_000;

impl CallBrief {
    fn clipped(text: &str, max: usize) -> String {
        text.trim().chars().take(max).collect()
    }

    fn prospect(&self) -> String {
        Self::clipped(&self.prospect, MAX_NAME_CHARS)
    }

    fn company(&self) -> String {
        Self::clipped(&self.company, MAX_NAME_CHARS)
    }

    fn agenda(&self) -> String {
        Self::clipped(&self.agenda, MAX_AGENDA_CHARS)
    }

    fn crm_context(&self) -> String {
        Self::clipped(&self.crm_context, MAX_CRM_CONTEXT_CHARS)
    }

    fn past_calls(&self) -> String {
        Self::clipped(&self.past_calls, MAX_PAST_CALLS_CHARS)
    }

    /// The brief as prompt text, or `None` when nothing was entered.
    pub fn to_prompt(&self) -> Option<String> {
        let fields = [
            ("Prospect", self.prospect()),
            ("Company", self.company()),
            ("Agenda and goals for this meeting", self.agenda()),
        ];
        let mut lines: Vec<String> = fields
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(label, value)| format!("- {label}: {value}"))
            .collect();
        let history = self.crm_context();
        if !history.is_empty() {
            lines.push(format!("\nFrom the CRM, before this call:\n{history}"));
        }
        let past_calls = self.past_calls();
        if !past_calls.is_empty() {
            lines.push(format!(
                "\nEarlier calls with them, newest first:\n\n{past_calls}"
            ));
        }
        let files = Self::clipped(&self.client_files, MAX_CLIENT_FILES_CHARS);
        if !files.is_empty() {
            lines.push(format!(
                "\nFiles and notes the team keeps about their company, newest first. Use them \
                 for names, needs, numbers and history when they fit what is being said:\n\n{files}"
            ));
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// A title for the saved call, e.g. "Call with Priya (Acme)".
    pub fn title(&self) -> String {
        match (self.prospect(), self.company()) {
            (p, c) if p.is_empty() && c.is_empty() => "Call summary".into(),
            (p, c) if c.is_empty() => format!("Call with {p}"),
            (p, c) if p.is_empty() => format!("Call with {c}"),
            (p, c) => format!("Call with {p} ({c})"),
        }
    }
}

/// Everything the coach needs, loaded once when capture starts so config or playbook
/// problems surface before recording begins. Playbook edits apply on the next call.
pub struct CoachSettings {
    provider: LlmProvider,
    /// The fixed part of the system prompt: instructions and playbook.
    base_system: String,
    /// `base_system` plus this call's notes: what every hint request sends.
    system: String,
    brief: CallBrief,
    prices: Prices,
    summary_provider: LlmProvider,
    playbook: String,
    summary_input_per_mtok: Option<f64>,
    summary_output_per_mtok: Option<f64>,
}

impl CoachSettings {
    /// The playbook the coach was given, for the lists kept in it (trigger words).
    pub fn playbook(&self) -> &str {
        &self.playbook
    }

    /// The live-hint model, for the checklist's own requests.
    pub fn provider(&self) -> LlmProvider {
        self.provider.clone()
    }

    /// Transcription price per stream-minute, if configured.
    pub fn stt_price_per_minute(&self) -> Option<f64> {
        self.prices.stt_per_minute
    }

    pub fn from_env() -> Result<Self, String> {
        let provider = LlmProvider::from_env()?;
        let path = playbook_path();
        let playbook = fs::read_to_string(&path)
            .map_err(|e| format!("Could not read playbook {}: {e}", path.display()))?;
        let base_system = format!("{SYSTEM_PROMPT}\n\n---\n\n{playbook}");
        Ok(Self {
            provider,
            system: base_system.clone(),
            base_system,
            brief: CallBrief::default(),
            prices: Prices::from_env()?,
            summary_provider: LlmProvider::summary_from_env()?,
            playbook,
            summary_input_per_mtok: env_f64("SUMMARY_PRICE_INPUT_PER_MTOK")?,
            summary_output_per_mtok: env_f64("SUMMARY_PRICE_OUTPUT_PER_MTOK")?,
        })
    }

    /// Adds what the rep entered before the call. It goes at the end of the system prompt,
    /// so it stays in the cached prefix for the whole call.
    pub fn with_brief(mut self, brief: CallBrief) -> Self {
        self.brief = brief;
        self.rebuild_system();
        self
    }

    /// Adds what the CRM and earlier calls say, read while capture was already starting.
    fn add_history(&mut self, history: CallHistory) {
        self.brief.crm_context = history.crm_context;
        self.brief.past_calls = history.past_calls;
        self.brief.client_files = history.client_files;
        self.rebuild_system();
    }

    fn rebuild_system(&mut self) {
        self.system = self.base_system.clone();
        if let Some(text) = self.brief.to_prompt() {
            self.system.push_str(&format!(
                "\n\n---\n\n# This call\n\nNotes from before the call: what the rep entered, \
                 and what the CRM and earlier call summaries say about this prospect. Use them \
                 to tailor hints (names, goals, open commitments, topics to cover). They are \
                 notes, not verified facts: don't state them to the prospect as facts.\n\n{text}"
            ));
        }
    }
}

/// The prospect's history, read while the call starts: capture doesn't wait for it, only the
/// opening hints do.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CallHistory {
    #[serde(default)]
    pub crm_context: String,
    #[serde(default)]
    pub past_calls: String,
    #[serde(default)]
    pub client_files: String,
}

/// Longest the opening waits for the history. The start dialog gives the reads 4 s from
/// Confirm; this adds a margin for the hand-over.
const HISTORY_WAIT: Duration = Duration::from_millis(4500);

/// The model's answer: no hints, one during the call, or up to three when it opens.
#[derive(Deserialize)]
struct HintsJson {
    hints: Vec<HintJson>,
}

#[derive(Deserialize)]
struct HintJson {
    #[serde(rename = "type")]
    kind: Option<String>,
    urgency: Option<String>,
    text: Option<String>,
}

/// A hint ready to show.
#[derive(Clone, Serialize)]
struct ShownHint {
    kind: String,
    urgency: String,
    text: String,
}

impl HintsJson {
    /// The hints worth showing: parsed, with text, and no more than `max`.
    fn shown(text: &str, max: usize) -> Option<Vec<ShownHint>> {
        let parsed = serde_json::from_str::<Self>(text).ok()?;
        Some(
            parsed
                .hints
                .into_iter()
                .filter_map(|hint| {
                    let text = hint.text?.trim().to_string();
                    (!text.is_empty()).then(|| ShownHint {
                        kind: hint.kind.unwrap_or_else(|| "fact".into()),
                        urgency: hint.urgency.unwrap_or_else(|| "medium".into()),
                        text,
                    })
                })
                .take(max)
                .collect(),
        )
    }
}

/// Words that carry no meaning of their own, left out when comparing hints.
const FILLER_WORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "in", "on", "at", "for", "with", "by", "from", "and", "or", "so",
    "is", "are", "was", "be", "it", "its", "this", "that", "these", "those", "there", "i", "me",
    "my", "we", "us", "our", "you", "your", "they", "them", "their", "do", "does", "can", "could",
    "would", "will", "should", "what", "what's", "how", "who", "which", "when", "if", "else",
    "any", "some", "just", "now", "then",
];

/// Whether two hints say the same thing: most of their meaningful words are shared. The prompt
/// already asks the model not to repeat itself; this catches the times it does anyway.
fn similar_hint(a: &str, b: &str) -> bool {
    let words = |text: &str| -> std::collections::HashSet<String> {
        text.split(|c: char| !crate::trigger::is_word_char(c))
            .map(str::to_lowercase)
            .filter(|w| !w.is_empty() && !FILLER_WORDS.contains(&w.as_str()))
            .collect()
    };
    let (a, b) = (words(a), words(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let shared = a.intersection(&b).count();
    // Over half of all the words between them.
    shared * 2 > a.union(&b).count()
}

/// The earlier hint this one repeats, if any.
fn repeat_of<'a>(earlier: &'a [String], text: &str) -> Option<&'a str> {
    earlier
        .iter()
        .find(|shown| similar_hint(text, shown))
        .map(String::as_str)
}

/// The first turn of every call, sent as capture starts. It asks for the opening hints and, as
/// the first request of the call, writes the prompt cache that every live hint then reads.
const OPENING_TURN: &str = "Trigger: call starting\nNew lines: none yet";
const MAX_OPENING_HINTS: usize = 3;

#[derive(Clone, Serialize)]
struct OpeningHint {
    id: String,
    #[serde(flatten)]
    hint: ShownHint,
}

#[derive(Clone, Serialize)]
struct HintPayload {
    /// Unique across calls: `<metrics file stem>#<n>`, so a rating can be traced back to the
    /// model call that produced the hint.
    id: String,
    #[serde(flatten)]
    hint: ShownHint,
    reason: TriggerReason,
    /// Audio of the prospect's speech captured → hint ready. The Phase 2 latency metric.
    speech_to_hint_ms: Option<u64>,
}

#[derive(Default)]
struct SessionStats {
    triggers: u64,
    llm_calls: u64,
    hints_shown: u64,
    /// Hints the model gave that repeated an earlier one, and so weren't shown.
    repeats_hidden: u64,
    errors: u64,
    usage: Usage,
    llm_cost: Option<f64>,
    speech_to_hint_ms: Vec<u64>,
    /// The prospect's last word to the hint's first words appearing (hints stream in).
    speech_to_first_words_ms: Vec<u64>,
    /// Keyword and turn-end triggers only: the ones expected to meet the 3 s target.
    responsive_speech_to_hint_ms: Vec<u64>,
    /// Silence triggers wait SILENCE_THRESHOLD by design before firing.
    silence_speech_to_hint_ms: Vec<u64>,
    cpu_percent_samples: Vec<f64>,
    llm_ms: Vec<u64>,
    /// The opening request: how long it took and how many hints it gave.
    opening_ms: Option<u64>,
    opening_hints: usize,
    /// Capture start to the opening hints painted on screen.
    start_to_opening_screen_ms: Option<u64>,
    /// The prospect's last word to the hint painted on screen: the Phase 2 metric, measured to
    /// the end (speech_to_hint stops when Claude answers).
    speech_to_screen_ms: Vec<u64>,
    /// Hint ready in Rust to painted: events, the WebView, React, the compositor.
    emit_to_paint_ms: Vec<u64>,
    /// Checklist checks run beside the coach (their tokens and cost are in the totals).
    checklist_checks: u64,
    /// Claude requests sent for hints, retries and hedges included. Above `llm_calls + errors`
    /// means some were sent twice, and those extra ones are billed but not in the cost.
    claude_requests_sent: u64,
    /// Capture to STT result: interim results (words appearing) and final ones (a line settles).
    stt_interim_ms: Vec<u64>,
    stt_final_ms: Vec<u64>,
    stt_seconds_me: f64,
    stt_seconds_them: f64,
    final_transcripts_me: u64,
    final_transcripts_them: u64,
    stream_errors: Vec<String>,
    /// The speaker or headset system audio was captured from, and each side's levels.
    output_device: Option<String>,
    audio: Option<AudioReport>,
}

impl SessionStats {
    /// Counts one hint request's tokens and cost, adds the cost to the call's spend, and returns it.
    fn add_llm_call(&mut self, prices: &Prices, usage: &Usage, spend: &AtomicU64) -> Option<f64> {
        self.llm_calls += 1;
        self.add_usage(prices, usage, spend)
    }

    /// Counts any model request's tokens and cost into the call's totals and spend.
    fn add_usage(&mut self, prices: &Prices, usage: &Usage, spend: &AtomicU64) -> Option<f64> {
        let cost = prices.llm_cost(usage);
        self.usage.input_tokens += usage.input_tokens;
        self.usage.cache_read_tokens += usage.cache_read_tokens;
        self.usage.cache_write_tokens += usage.cache_write_tokens;
        self.usage.output_tokens += usage.output_tokens;
        self.llm_cost = self.llm_cost.zip(cost).map(|(a, b)| a + b);
        if let Some(cost) = cost {
            spend.fetch_add((cost * 1_000_000.0).round() as u64, Ordering::Relaxed);
        }
        cost
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: AppHandle,
    session_id: u64,
    mut settings: CoachSettings,
    mut inputs: UnboundedReceiver<CoachInput>,
    session_started: Instant,
    // "live", "solo_test", or "demo"
    mode: &'static str,
    // Hint spend so far in millionths of a dollar, for the call's spend limit.
    llm_spend: Arc<AtomicU64>,
    // The CRM and earlier calls, when the start dialog is still reading them.
    call_history: Option<oneshot::Receiver<CallHistory>>,
    // Where the opening hints go, so the checklist can tick them off as the call covers them.
    checklist: Option<UnboundedSender<ChecklistInput>>,
) {
    // The app's shared client: the reads the start dialog just made left it a warm connection.
    let client = crate::backend::client().unwrap_or_else(|_| reqwest::Client::new());
    let mut log = MetricsLog::open(&app);
    let hint_id_prefix = log
        .stem()
        .unwrap_or_else(|| format!("session-{}", unix_ms()));
    let mut history: Vec<Turn> = Vec::new();
    let mut stats = SessionStats {
        llm_cost: Some(0.0),
        ..Default::default()
    };
    // Lines from a failed request, re-sent with the next trigger so context isn't lost.
    let mut carried: Vec<Line> = Vec::new();
    // Every final line of the call, in order, for the post-call summary.
    let mut transcript: Vec<Line> = Vec::new();
    // The same lines on disk as they're said, so a crash can't lose the call.
    let started_unix_ms = unix_ms();
    let mut journal = crate::history::calls_dir(&app).ok().map(|dir| {
        Journal::open(
            &dir,
            started_unix_ms,
            &json!({
                "kind": "start",
                "started_at_unix_ms": started_unix_ms,
                "title": settings.brief.title(),
                "mode": mode,
                "brief": settings.brief,
            }),
        )
    });
    let current_journal = app.state::<CurrentJournal>();
    current_journal.set(
        journal
            .as_ref()
            .and_then(|j| j.path())
            .map(Path::to_path_buf),
    );

    // Hints emitted and not yet reported painted: when, and the speech they answer.
    let mut emitted: HashMap<String, Emitted> = HashMap::new();
    // Every hint shown in this call, opening ones too, so a repeat isn't shown again.
    let mut shown_texts: Vec<String> = Vec::new();

    // The opening needs the prospect's history; capture is already running while it arrives.
    let history_wait_ms = match call_history {
        Some(call_history) => {
            let waiting = Instant::now();
            if let Ok(Ok(found)) = tokio::time::timeout(HISTORY_WAIT, call_history).await {
                settings.add_history(found);
            }
            Some(waiting.elapsed().as_millis() as u64)
        }
        None => None,
    };

    let requests_before = crate::llm::REQUESTS_SENT.load(Ordering::Relaxed);

    // Opening hints, while the rep is still saying hello. Triggers that arrive meanwhile wait. A
    // call stopped while its history was being read gets none: nobody would see them.
    let still_running = app
        .state::<crate::capture::CaptureState>()
        .is_running_call(session_id);
    history.push(Turn {
        role: Role::User,
        text: OPENING_TURN.into(),
    });
    let request_started = Instant::now();
    let result = if still_running || mode == "demo" {
        settings
            .provider
            .generate(&client, &opening_request(&settings.system, &history))
            .await
    } else {
        Err("The call ended before the opening hints".into())
    };
    let llm_ms = request_started.elapsed().as_millis() as u64;
    match result {
        Ok(response) => {
            history.push(Turn {
                role: Role::Model,
                text: response.text.clone(),
            });
            let shown = HintsJson::shown(&response.text, MAX_OPENING_HINTS);
            let cost = stats.add_llm_call(&settings.prices, &response.usage, &llm_spend);
            let hints: Vec<OpeningHint> = shown
                .iter()
                .flatten()
                .enumerate()
                .map(|(i, hint)| OpeningHint {
                    id: format!("{hint_id_prefix}#open-{}", i + 1),
                    hint: hint.clone(),
                })
                .collect();
            stats.opening_ms = Some(llm_ms);
            stats.opening_hints = hints.len();
            let since_start = request_started.saturating_duration_since(session_started);
            log.write(json!({
                "kind": "opening_hints",
                "at_unix_ms": unix_ms(),
                "hints": hints.iter().map(|h| &h.hint).collect::<Vec<_>>(),
                "raw_response": if shown.is_none() { Some(&response.text) } else { None },
                "latency_ms": {
                    "history_wait": history_wait_ms,
                    "since_start": since_start.as_millis() as u64,
                    "llm": llm_ms,
                },
                "tokens": {
                    "input": response.usage.input_tokens,
                    "cache_read": response.usage.cache_read_tokens,
                    "cache_write": response.usage.cache_write_tokens,
                    "output": response.usage.output_tokens,
                },
                "cost_usd": cost,
            }));
            let now = Instant::now();
            for hint in &hints {
                emitted.insert(
                    hint.id.clone(),
                    Emitted {
                        at: now,
                        speech_end_at: None,
                        opening: true,
                    },
                );
            }
            if let Some(checklist) = &checklist {
                let items = hints.iter().map(|h| (h.id.clone(), h.hint.text.clone()));
                let _ = checklist.send(ChecklistInput::Items(items.collect()));
            }
            shown_texts.extend(hints.iter().map(|h| h.hint.text.clone()));
            emit_for_session(&app, session_id, "opening-hints", hints);
        }
        Err(message) => {
            // Live hints still work without the opening; the first one just writes the cache.
            history.pop();
            stats.errors += 1;
            log.write(json!({
                "kind": "llm_error",
                "at_unix_ms": unix_ms(),
                "reason": "opening",
                "history_wait_ms": history_wait_ms,
                "llm_ms": llm_ms,
                "error": message,
            }));
        }
    }

    // The checklist has everything it needs from the coach; let go of it, so it can finish when
    // the call does.
    drop(checklist);

    // The timing of lines carried over from a failed request: the prospect has been waiting
    // since then, and the next hint's latency must say so.
    let mut carried_timing: Option<TriggerTiming> = None;
    while let Some(input) = inputs.recv().await {
        let mut record = Record {
            stats: &mut stats,
            log: &mut log,
            transcript: &mut transcript,
            journal: &mut journal,
            emitted: &mut emitted,
            session_started,
            prices: settings.prices,
            spend: &llm_spend,
        };
        let Some(mut trigger) = record.take_trigger(input) else {
            continue;
        };
        record.stats.triggers += 1;
        // Whether any of the merged triggers is one the 3 s target applies to (not a silence).
        let mut responsive = trigger.reason != TriggerReason::Silence;
        // Triggers that queued while the previous request was in flight are merged, so the model
        // answers the latest state of the call instead of a backlog. The latency is counted from
        // the earliest of them: that speech has waited the longest.
        while let Ok(next) = inputs.try_recv() {
            if let Some(next) = record.take_trigger(next) {
                record.stats.triggers += 1;
                responsive |= next.reason != TriggerReason::Silence;
                trigger.lines.extend(next.lines);
                trigger.reason = next.reason;
                trigger.timing = trigger.timing.earliest(next.timing);
            }
        }
        trigger.lines.splice(0..0, carried.drain(..));
        if let Some(earlier) = carried_timing.take() {
            trigger.timing = trigger.timing.earliest(earlier);
        }

        history.push(Turn {
            role: Role::User,
            text: format_turn(&trigger),
        });
        let request_started = Instant::now();
        // Numbered up front so the windows can tie the streamed words to the finished hint
        // (`add_llm_call` counts this call once it succeeds).
        let hint_id = format!("{hint_id_prefix}#{}", stats.llm_calls + 1);
        let mut draft = HintStream::new(hint_id.clone());
        let result = settings
            .provider
            .stream(
                &client,
                &hint_request(&settings.system, &history),
                &mut |piece| {
                    if let Some(delta) = draft.on_piece(piece) {
                        emit_for_session(&app, session_id, "hint-delta", delta);
                    }
                },
            )
            .await;
        let done = Instant::now();
        // Words that were on screen but won't become a hint must come off again.
        let discard_draft = |app: &AppHandle| {
            if draft.first_words_at.is_some() {
                emit_for_session(app, session_id, "hint-discarded", json!({ "id": hint_id }));
            }
        };
        let timing = trigger.timing;
        let ms = |from: Instant, to: Instant| to.saturating_duration_since(from).as_millis() as u64;
        let speech_to_hint_ms = timing.speech_end_at.map(|at| ms(at, done));
        let llm_ms = ms(request_started, done);

        let response = match result {
            Ok(response) => response,
            Err(message) => {
                stats.errors += 1;
                history.pop();
                carried = trigger.lines;
                carried_timing = Some(trigger.timing);
                log.write(json!({
                    "kind": "llm_error",
                    "at_unix_ms": unix_ms(),
                    "reason": trigger.reason,
                    "llm_ms": llm_ms,
                    "error": message,
                }));
                discard_draft(&app);
                emit_for_session(&app, session_id, "coach-error", message);
                continue;
            }
        };

        history.push(Turn {
            role: Role::Model,
            text: response.text.clone(),
        });
        let parsed = HintsJson::shown(&response.text, 1);
        let answer = parsed.as_ref().and_then(|hints| hints.first());
        // Said before in this call: not shown again, so the screen keeps what it has. The model's
        // answer stays in the history, so it knows it made that point.
        let repeated = answer
            .and_then(|hint| repeat_of(&shown_texts, &hint.text))
            .map(str::to_string);
        let shown = answer.filter(|_| repeated.is_none());
        if repeated.is_some() {
            stats.repeats_hidden += 1;
        }
        let cost = stats.add_llm_call(&settings.prices, &response.usage, &llm_spend);
        stats.llm_ms.push(llm_ms);
        if let (Some(spoken), Some(first)) = (timing.speech_end_at, draft.first_words_at) {
            stats.speech_to_first_words_ms.push(ms(spoken, first));
        }
        if let Some(latency) = speech_to_hint_ms {
            stats.speech_to_hint_ms.push(latency);
            if responsive {
                stats.responsive_speech_to_hint_ms.push(latency);
            } else {
                stats.silence_speech_to_hint_ms.push(latency);
            }
        }

        log.write(json!({
            "kind": "llm_call",
            "at_unix_ms": unix_ms(),
            "reason": trigger.reason,
            "new_lines": trigger.lines.len(),
            "history_turns": history.len(),
            "shown": shown.is_some(),
            "hint_id": shown.is_some().then_some(&hint_id),
            "hint": shown.map(|hint| &hint.text),
            "repeat_of": repeated,
            "repeated_hint": repeated.is_some().then(|| answer.map(|hint| &hint.text)).flatten(),
            "raw_response": if parsed.is_none() { Some(&response.text) } else { None },
            "latency_ms": {
                "speech_to_hint": speech_to_hint_ms,
                "stt": timing.speech_end_at.map(|at| ms(at, timing.transcript_at)),
                "trigger_wait": ms(timing.transcript_at, timing.fired_at),
                "queue": ms(timing.fired_at, request_started),
                "llm": llm_ms,
            },
            "tokens": {
                "input": response.usage.input_tokens,
                "cache_read": response.usage.cache_read_tokens,
                "cache_write": response.usage.cache_write_tokens,
                "output": response.usage.output_tokens,
            },
            "cost_usd": cost,
        }));

        if shown.is_none() {
            discard_draft(&app);
        }
        if let Some(hint) = shown {
            stats.hints_shown += 1;
            shown_texts.push(hint.text.clone());
            emitted.insert(
                hint_id.clone(),
                Emitted {
                    at: Instant::now(),
                    speech_end_at: timing.speech_end_at,
                    opening: false,
                },
            );
            emit_for_session(
                &app,
                session_id,
                "hint",
                HintPayload {
                    id: hint_id,
                    hint: hint.clone(),
                    reason: trigger.reason,
                    speech_to_hint_ms,
                },
            );
        }
    }

    stats.claude_requests_sent =
        crate::llm::REQUESTS_SENT.load(Ordering::Relaxed) - requests_before;
    let summary = summarize(
        &stats,
        &settings,
        session_started.elapsed().as_secs_f64(),
        mode,
        &log,
    );
    log.write(summary.clone());
    // Stored as well as emitted: an event is delivered once, and the UI can miss it, so
    // the UI also polls `last_summaries` after Stop.
    store_summaries(
        &app,
        session_id,
        Some(summary.clone()),
        Some(json!({ "status": "pending" })),
    );
    emit_for_session(&app, session_id, "session-summary", summary.clone());

    let call_file = CallFile::create(&app);
    let call = summarize_call(
        &client,
        &settings,
        &transcript,
        call_file.as_ref(),
        &mut log,
    )
    .await;
    // The structured copy of the call, for the history screen.
    if let Some(file) = &call_file {
        file.write_record(&json!({
            "version": 1,
            "title": settings.brief.title(),
            "brief": settings.brief,
            "mode": mode,
            "call": call,
            "stats": summary,
            "transcript": transcript,
        }));
        // Saved: the journal has done its job. Without a call file it stays, to be recovered.
        if let Some(journal) = journal {
            journal.finish();
        }
    }
    current_journal.set(None);
    store_summaries(&app, session_id, Some(summary), Some(call));
}

/// Writes the post-call summary with the summary model, saves it as Markdown, and returns
/// the status object the UI renders.
async fn summarize_call(
    client: &reqwest::Client,
    settings: &CoachSettings,
    transcript: &[Line],
    call_file: Option<&CallFile>,
    log: &mut MetricsLog,
) -> serde_json::Value {
    // Save the transcript before asking for a summary, so a failed or skipped summary never
    // loses the call. The same file gets the summary added once it's written.
    let transcript_saved_to = call_file.and_then(|file| {
        file.write(&call_summary::transcript_markdown(
            &settings.brief.title(),
            transcript,
        ))
    });
    if !transcript.is_empty() && transcript_saved_to.is_none() {
        log.write(json!({
            "kind": "transcript_save_error",
            "at_unix_ms": unix_ms(),
            "transcript_lines": transcript.len(),
        }));
    }

    if transcript.len() < call_summary::MIN_LINES {
        return json!({
            "status": "skipped",
            "reason": "Not enough conversation to summarize.",
            "file": transcript_saved_to,
        });
    }
    let started = Instant::now();
    let result = call_summary::generate(
        client,
        &settings.summary_provider,
        &settings.playbook,
        settings.brief.to_prompt().as_deref(),
        transcript,
    )
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(call) => {
            let cost = settings
                .summary_input_per_mtok
                .zip(settings.summary_output_per_mtok)
                .map(|(input, output)| {
                    (call.usage.input_tokens as f64 * input
                        + call.usage.output_tokens as f64 * output)
                        / 1_000_000.0
                });
            let markdown =
                call_summary::to_markdown(&call.summary, &settings.brief.title(), transcript);
            let saved_to = call_file
                .and_then(|file| file.write(&markdown))
                .or(transcript_saved_to);
            log.write(json!({
                "kind": "call_summary",
                "at_unix_ms": unix_ms(),
                "model": settings.summary_provider.model_name(),
                "elapsed_ms": elapsed_ms,
                "transcript_lines": transcript.len(),
                "tokens": { "input": call.usage.input_tokens, "output": call.usage.output_tokens },
                "cost_usd": cost,
                "file": saved_to,
                "summary": call.summary,
            }));
            json!({
                "status": "ready",
                "summary": call.summary,
                "file": saved_to,
                "cost_usd": cost,
                "elapsed_ms": elapsed_ms,
            })
        }
        Err(message) => {
            log.write(json!({
                "kind": "call_summary_error",
                "at_unix_ms": unix_ms(),
                "elapsed_ms": elapsed_ms,
                "error": message,
                "transcript_file": transcript_saved_to,
            }));
            json!({ "status": "failed", "error": message, "file": transcript_saved_to })
        }
    }
}

/// One Markdown file per call under `<app data>/calls/`: first the transcript, then the
/// transcript with its summary.
struct CallFile {
    path: PathBuf,
}

impl CallFile {
    fn create(app: &AppHandle) -> Option<Self> {
        let dir = crate::history::calls_dir(app).ok()?;
        fs::create_dir_all(&dir).ok()?;
        Some(Self {
            path: dir.join(format!("call-{}.md", unix_ms())),
        })
    }

    /// Replaces the file's contents; returns the path, or `None` if writing failed.
    fn write(&self, markdown: &str) -> Option<String> {
        fs::write(&self.path, markdown).ok()?;
        Some(self.path.display().to_string())
    }

    /// Writes the structured record next to the Markdown file (`.json`). Best effort: the
    /// Markdown file already holds everything.
    fn write_record(&self, record: &serde_json::Value) {
        let _ = fs::write(self.path.with_extension("json"), record.to_string());
    }
}

/// Stores a call's summaries, unless a newer call has started (its summaries win).
fn store_summaries(
    app: &AppHandle,
    session_id: u64,
    session: Option<serde_json::Value>,
    call: Option<serde_json::Value>,
) {
    let state = app.state::<SummaryState>();
    let mut summaries = state.0.lock().expect("summary lock poisoned");
    if summaries.session_id == session_id {
        summaries.session = session;
        summaries.call = call;
    }
}

#[derive(Default)]
pub struct Summaries {
    session_id: u64,
    session: Option<serde_json::Value>,
    call: Option<serde_json::Value>,
}

/// Summaries of the most recently started call.
#[derive(Default)]
pub struct SummaryState(pub std::sync::Mutex<Summaries>);

/// The running call's coach, for inputs that come from the windows. Weak, so it never keeps a
/// finished call's coach waiting for more input.
#[derive(Default)]
pub struct CoachInbox(std::sync::Mutex<Option<WeakUnboundedSender<CoachInput>>>);

impl CoachInbox {
    pub fn set(&self, coach: &UnboundedSender<CoachInput>) {
        *self.0.lock().expect("coach inbox lock poisoned") = Some(coach.downgrade());
    }
}

/// A window painted a hint (after its first frame on screen): the end of the latency measured
/// from the prospect's last word.
#[tauri::command]
pub fn hint_painted(inbox: tauri::State<'_, CoachInbox>, hint_id: String) {
    let at = Instant::now();
    let coach = inbox.0.lock().expect("coach inbox lock poisoned").clone();
    if let Some(coach) = coach.and_then(|weak| weak.upgrade()) {
        let _ = coach.send(CoachInput::HintPainted { hint_id, at });
    }
}

/// The prospect's history for a call that started before the start dialog finished reading it.
#[derive(Default)]
pub struct PendingHistory(std::sync::Mutex<Option<(u64, oneshot::Sender<CallHistory>)>>);

impl PendingHistory {
    /// A call is starting without its history; the opening waits on the returned receiver.
    pub fn expect(&self, session_id: u64) -> oneshot::Receiver<CallHistory> {
        let (sender, receiver) = oneshot::channel();
        *self.0.lock().expect("pending history lock poisoned") = Some((session_id, sender));
        receiver
    }
}

/// Hands the CRM record and earlier calls to the call that is starting. Ignored for any other
/// call, and once the opening stopped waiting.
#[tauri::command]
pub fn provide_call_history(
    pending: tauri::State<'_, PendingHistory>,
    session_id: u64,
    history: CallHistory,
) {
    let mut pending = pending.0.lock().expect("pending history lock poisoned");
    if pending.as_ref().is_some_and(|(id, _)| *id == session_id) {
        if let Some((_, sender)) = pending.take() {
            let _ = sender.send(history);
        }
    }
}

impl SummaryState {
    /// A new call started: forget the previous call's summaries.
    pub fn begin(&self, session_id: u64) {
        *self.0.lock().expect("summary lock poisoned") = Summaries {
            session_id,
            ..Summaries::default()
        };
    }

    /// False while the latest call's transcript and summary are still being saved.
    pub fn is_settled(&self) -> bool {
        let summaries = self.0.lock().expect("summary lock poisoned");
        summaries.session_id == 0
            || summaries
                .call
                .as_ref()
                .is_some_and(|call| call["status"] != "pending")
    }
}

/// `{ session_id, session, call }`: which call these belong to, the metrics summary, and the
/// post-call summary status (`pending`, `ready`, `failed`, or `skipped`). Either summary is
/// null until available.
#[tauri::command]
pub fn last_summaries(state: tauri::State<'_, SummaryState>) -> serde_json::Value {
    let summaries = state.0.lock().expect("summary lock poisoned");
    json!({
        "session_id": summaries.session_id,
        "session": summaries.session,
        "call": summaries.call,
    })
}

const HINT_RATINGS_FILE: &str = "hint_ratings.jsonl";

#[derive(Clone, Serialize)]
struct HintRatedPayload {
    hint_id: String,
    rating: Option<&'static str>,
}

/// Records the rep's rating of a hint (`"up"`, `"down"`, or `null` to clear it) in
/// `metrics/hint_ratings.jsonl`; the latest line per hint wins. Every window is told.
#[tauri::command]
pub fn rate_hint(
    app: AppHandle,
    hint_id: String,
    text: String,
    rating: Option<String>,
) -> Result<(), String> {
    let rating = match rating.as_deref() {
        Some("up") => Some("up"),
        Some("down") => Some("down"),
        None => None,
        Some(other) => return Err(format!("Unknown rating {other:?}")),
    };
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("metrics");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(HINT_RATINGS_FILE))
        .map_err(|e| format!("Could not save the rating: {e}"))?;
    let record = json!({
        "kind": "hint_rating",
        "at_unix_ms": unix_ms(),
        "hint_id": hint_id,
        "rating": rating,
        "text": text,
    });
    writeln!(file, "{record}").map_err(|e| format!("Could not save the rating: {e}"))?;
    let _ = app.emit("hint-rated", HintRatedPayload { hint_id, rating });
    Ok(())
}

const HINT_MAX_TOKENS: u32 = 300;
/// Below the default 1.0: replaying a call at the default, the same turn came back as a short hint
/// one time and a 20-word one the next. Steadier wording keeps to the rules more often.
const HINT_TEMPERATURE: f32 = 0.3;
/// A live hint older than this is no use mid-conversation; giving up frees the coach for the
/// next trigger, and the lines go out again with it.
const HINT_TIMEOUT: Duration = Duration::from_secs(8);
/// Warm live hints take 1-2.5 s; one still unanswered at 4 s is probably stuck, so a copy goes out
/// on a fresh connection.
const HINT_HEDGE_AFTER: Duration = Duration::from_secs(4);
/// The opening writes the whole prompt cache, so it runs slower (2.5-4.6 s measured). Live
/// triggers wait behind it, so a stuck opening must give up well before a live hint would. No
/// hedge: a copy sent once the opening looks stuck couldn't write the cache in the time left.
const OPENING_TIMEOUT: Duration = Duration::from_secs(7);

static HINT_SCHEMA: LazyLock<serde_json::Value> = LazyLock::new(|| {
    // One schema for the opening and live hints: the schema is part of what the prompt cache
    // matches, so a second one would make the first live hint miss the cache the opening wrote.
    json!({
        "type": "object",
        "properties": {
            "hints": {
                "type": "array",
                "items": {
                    "type": "object",
                    // `text` first: Claude writes fields in this order, and the words are what
                    // stream to the screen.
                    "properties": {
                        "text": { "type": "string" },
                        "type": {
                            "type": "string",
                            "enum": ["question", "objection", "buying_signal", "reminder", "fact"]
                        },
                        "urgency": { "type": "string", "enum": ["low", "medium", "high"] }
                    },
                    "required": ["text", "type", "urgency"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["hints"],
        "additionalProperties": false
    })
});

/// The words of a hint so far, sent to the windows while Claude writes it.
#[derive(Clone, Debug, PartialEq, Serialize)]
struct HintDelta {
    id: String,
    text: String,
}

/// Turns a live hint's streamed pieces into growing drafts for the windows.
struct HintStream {
    id: String,
    field: crate::llm::partial_json::TextField,
    first_words_at: Option<Instant>,
}

impl HintStream {
    fn new(id: String) -> Self {
        Self {
            id,
            field: Default::default(),
            first_words_at: None,
        }
    }

    /// The draft to show after this piece, if it added words.
    fn on_piece(&mut self, piece: &str) -> Option<HintDelta> {
        let text = self.field.push(piece)?.to_string();
        self.first_words_at.get_or_insert_with(Instant::now);
        Some(HintDelta {
            id: self.id.clone(),
            text,
        })
    }
}

/// A live-hint request: cached, short, and fast-failing.
fn hint_request<'a>(system: &'a str, turns: &'a [Turn]) -> LlmRequest<'a> {
    LlmRequest {
        system,
        turns,
        schema: &HINT_SCHEMA,
        max_tokens: HINT_MAX_TOKENS,
        cache: true,
        effort: None,
        temperature: Some(HINT_TEMPERATURE),
        timeout: HINT_TIMEOUT,
        hedge_after: Some(HINT_HEDGE_AFTER),
    }
}

/// The opening request: the same prompt and schema as live hints (so it writes the cache they
/// read), with its own, longer limits.
fn opening_request<'a>(system: &'a str, turns: &'a [Turn]) -> LlmRequest<'a> {
    LlmRequest {
        timeout: OPENING_TIMEOUT,
        hedge_after: None,
        ..hint_request(system, turns)
    }
}

/// A hint sent to the windows, waiting for one of them to report it painted.
struct Emitted {
    at: Instant,
    /// The prospect's last word the hint answers; `None` for the opening hints.
    speech_end_at: Option<Instant>,
    opening: bool,
}

/// What the coach loop records non-trigger inputs into.
struct Record<'a> {
    stats: &'a mut SessionStats,
    log: &'a mut MetricsLog,
    transcript: &'a mut Vec<Line>,
    journal: &'a mut Option<Journal>,
    emitted: &'a mut HashMap<String, Emitted>,
    session_started: Instant,
    prices: Prices,
    spend: &'a AtomicU64,
}

impl Record<'_> {
    /// Returns the trigger, or records a non-trigger input and returns `None`.
    fn take_trigger(&mut self, input: CoachInput) -> Option<Trigger> {
        let (stats, log, transcript) = (&mut *self.stats, &mut *self.log, &mut *self.transcript);
        match input {
            CoachInput::Trigger(trigger) => Some(trigger),
            CoachInput::HintPainted { hint_id, at } => {
                // The first window to paint it counts; a hint painted twice is reported once.
                let sent = self.emitted.remove(&hint_id)?;
                let ms = |from: Instant| at.saturating_duration_since(from).as_millis() as u64;
                let emit_to_paint = ms(sent.at);
                stats.emit_to_paint_ms.push(emit_to_paint);
                let total = if sent.opening {
                    let since_start = ms(self.session_started);
                    stats.start_to_opening_screen_ms.get_or_insert(since_start);
                    json!({ "start_to_screen": since_start })
                } else {
                    let speech_to_screen = sent.speech_end_at.map(ms);
                    stats.speech_to_screen_ms.extend(speech_to_screen);
                    json!({ "speech_to_screen": speech_to_screen })
                };
                log.write(json!({
                    "kind": "hint_painted",
                    "at_unix_ms": unix_ms(),
                    "hint_id": hint_id,
                    "latency_ms": { "emit_to_paint": emit_to_paint, "total": total },
                }));
                None
            }
            CoachInput::ChecklistCheck {
                covered,
                usage,
                ms,
                error,
            } => {
                stats.checklist_checks += 1;
                let cost = usage
                    .as_ref()
                    .and_then(|usage| stats.add_usage(&self.prices, usage, self.spend));
                log.write(json!({
                "kind": "checklist_check",
                "at_unix_ms": unix_ms(),
                "covered": covered,
                "ms": ms,
                "tokens": usage.map(|u| json!({ "input": u.input_tokens, "output": u.output_tokens })),
                "cost_usd": cost,
                "error": error,
            }));
                None
            }
            CoachInput::CpuSample(percent) => {
                stats.cpu_percent_samples.push(percent);
                None
            }
            CoachInput::SttLatency { ms, is_final } => {
                if is_final {
                    stats.stt_final_ms.push(ms);
                } else {
                    stats.stt_interim_ms.push(ms);
                }
                None
            }
            CoachInput::FinalTranscript {
                channel,
                speaker,
                text,
            } => {
                match channel {
                    Channel::Me => stats.final_transcripts_me += 1,
                    Channel::Them => stats.final_transcripts_them += 1,
                }
                if let Some(journal) = self.journal.as_mut() {
                    journal.append(speaker, &text);
                }
                transcript.push(Line {
                    channel: speaker,
                    text,
                });
                None
            }
            CoachInput::StreamError {
                channel,
                stage,
                message,
            } => {
                log.write(json!({
                    "kind": "stream_error",
                    "at_unix_ms": unix_ms(),
                    "channel": channel,
                    "stage": stage,
                    "error": message,
                }));
                stats
                    .stream_errors
                    .push(format!("{channel:?} {stage}: {message}"));
                None
            }
            CoachInput::OutputDevice(name) => {
                stats.output_device = name;
                None
            }
            CoachInput::ProspectNotHeard { rep_speech_seconds } => {
                log.write(json!({
                    "kind": if rep_speech_seconds.is_some() { "prospect_not_heard" } else { "prospect_heard_again" },
                    "at_unix_ms": unix_ms(),
                    "rep_speech_seconds": rep_speech_seconds,
                    "output_device": stats.output_device,
                }));
                None
            }
            CoachInput::Audio(report) => {
                stats.audio = Some(report);
                None
            }
            CoachInput::SttUsage {
                channel,
                audio_seconds,
            } => {
                match channel {
                    Channel::Me => stats.stt_seconds_me += audio_seconds,
                    Channel::Them => stats.stt_seconds_them += audio_seconds,
                }
                None
            }
        }
    }
}

fn format_turn(trigger: &Trigger) -> String {
    let reason = match &trigger.reason {
        TriggerReason::Keyword { keyword } => format!("keyword \"{keyword}\""),
        TriggerReason::TurnEnd => "prospect finished speaking".into(),
        TriggerReason::Silence => "pause in conversation".into(),
        TriggerReason::Asked { question: None } => "the rep asked what to say now".into(),
        TriggerReason::Asked {
            question: Some(question),
        } => format!("the rep asked: \"{question}\""),
    };
    let lines: Vec<String> = trigger
        .lines
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
        "Trigger: {reason}\nNew lines:\n{}\n\n{TURN_REMINDER}",
        lines.join("\n")
    )
}

/// Closes every live turn. The model weighs the newest message most, and over a long call it
/// drifts from rules stated only at the top: hints grew past 15 words and made up numbers.
const TURN_REMINDER: &str = "(Reply rules: at most one hint; one sentence of at most 15 words \
     the rep says as written; only facts from the playbook and the notes, no numbers or results \
     they don't state; nothing you already suggested.)";

fn summarize(
    stats: &SessionStats,
    settings: &CoachSettings,
    duration_seconds: f64,
    mode: &str,
    log: &MetricsLog,
) -> serde_json::Value {
    let hours = duration_seconds / 3600.0;
    let stt_minutes = (stats.stt_seconds_me + stats.stt_seconds_them) / 60.0;
    let stt_cost = settings.prices.stt_per_minute.map(|p| p * stt_minutes);
    let llm_cost = stats.llm_cost;
    let per_hour = |cost: Option<f64>| cost.filter(|_| hours > 0.0).map(|c| c / hours);
    let total = llm_cost.zip(stt_cost).map(|(a, b)| a + b);
    let cache_ratio = (stats.usage.input_tokens > 0)
        .then(|| stats.usage.cache_read_tokens as f64 / stats.usage.input_tokens as f64);

    json!({
        "kind": "session_summary",
        "at_unix_ms": unix_ms(),
        "model": settings.provider.model_name(),
        "duration_minutes": duration_seconds / 60.0,
        "mode": mode,
        "triggers": stats.triggers,
        "llm_calls": stats.llm_calls,
        "claude_requests_sent": stats.claude_requests_sent,
        "checklist_checks": stats.checklist_checks,
        "hints_shown": stats.hints_shown,
        "repeats_hidden": stats.repeats_hidden,
        "opening_hints": stats.opening_hints,
        "errors": stats.errors,
        "cpu_percent": cpu_summary(&stats.cpu_percent_samples),
        "latency_ms": {
            "speech_to_hint_p50": percentile(&stats.speech_to_hint_ms, 0.5),
            "speech_to_hint_p90": percentile(&stats.speech_to_hint_ms, 0.9),
            "speech_to_first_words_p50": percentile(&stats.speech_to_first_words_ms, 0.5),
            "speech_to_first_words_p90": percentile(&stats.speech_to_first_words_ms, 0.9),
            "keyword_turn_end_p90": percentile(&stats.responsive_speech_to_hint_ms, 0.9),
            "silence_p90": percentile(&stats.silence_speech_to_hint_ms, 0.9),
            "llm_p50": percentile(&stats.llm_ms, 0.5),
            "llm_p90": percentile(&stats.llm_ms, 0.9),
            "opening_hints": stats.opening_ms,
            "speech_to_screen_p50": percentile(&stats.speech_to_screen_ms, 0.5),
            "speech_to_screen_p90": percentile(&stats.speech_to_screen_ms, 0.9),
            "emit_to_paint_p50": percentile(&stats.emit_to_paint_ms, 0.5),
            "emit_to_paint_p90": percentile(&stats.emit_to_paint_ms, 0.9),
            "start_to_opening_screen": stats.start_to_opening_screen_ms,
            "stt_interim_p50": percentile(&stats.stt_interim_ms, 0.5),
            "stt_interim_p90": percentile(&stats.stt_interim_ms, 0.9),
            "stt_final_p50": percentile(&stats.stt_final_ms, 0.5),
            "stt_final_p90": percentile(&stats.stt_final_ms, 0.9),
        },
        "tokens": {
            "input": stats.usage.input_tokens,
            "cache_read": stats.usage.cache_read_tokens,
            "cache_write": stats.usage.cache_write_tokens,
            "output": stats.usage.output_tokens,
            "cache_ratio": cache_ratio,
        },
        "stt_mip_opt_out": crate::stt::deepgram_mip_opt_out(),
        "stt_minutes": { "me": stats.stt_seconds_me / 60.0, "them": stats.stt_seconds_them / 60.0 },
        "final_transcripts": { "me": stats.final_transcripts_me, "them": stats.final_transcripts_them },
        "stream_errors": stats.stream_errors,
        "audio": {
            "output_device": stats.output_device,
            "prospect_warnings": stats.audio.as_ref().map(|a| a.prospect_warnings),
            "me": stats.audio.as_ref().map(|a| &a.me),
            "them": stats.audio.as_ref().map(|a| &a.them),
        },
        "cost_usd": { "llm": llm_cost, "stt": stt_cost, "total": total },
        "cost_per_call_hour_usd": {
            "llm": per_hour(llm_cost),
            "stt": per_hour(stt_cost),
            "total": per_hour(total),
        },
        "metrics_file": log.path.as_ref().map(|p| p.display().to_string()),
    })
}

/// Average, p90, and max of the per-second CPU samples; null when none were taken.
fn cpu_summary(samples: &[f64]) -> serde_json::Value {
    if samples.is_empty() {
        return serde_json::Value::Null;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p90_rank = ((0.9 * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    json!({
        "avg": sorted.iter().sum::<f64>() / sorted.len() as f64,
        "p90": sorted[p90_rank - 1],
        "max": sorted[sorted.len() - 1],
        "note": "Rust core process only; WebView2 UI processes are excluded",
    })
}

fn percentile(values: &[u64], p: f64) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// One JSONL file per session under `<app data>/metrics/`. Logging failures never
/// interrupt coaching.
struct MetricsLog {
    file: Option<File>,
    path: Option<PathBuf>,
}

impl MetricsLog {
    fn open(app: &AppHandle) -> Self {
        let opened = app.path().app_data_dir().ok().and_then(|dir| {
            let dir = dir.join("metrics");
            fs::create_dir_all(&dir).ok()?;
            let path = dir.join(format!("session-{}.jsonl", unix_ms()));
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()?;
            Some((file, path))
        });
        match opened {
            Some((file, path)) => Self {
                file: Some(file),
                path: Some(path),
            },
            None => Self {
                file: None,
                path: None,
            },
        }
    }

    fn stem(&self) -> Option<String> {
        Some(
            self.path
                .as_ref()?
                .file_stem()?
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn write(&mut self, record: serde_json::Value) {
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{record}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_hint_in_other_words_is_a_repeat() {
        for (a, b) in [
            (
                "Who else needs to be involved in the decision?",
                "Who else is involved in this decision?",
            ),
            (
                "Would an architecture audit be a low-risk first step?",
                "An architecture audit could be the low-risk first step for you.",
            ),
            (
                "Who signs off on the budget besides you?",
                "Who else signs off on budget?",
            ),
            ("Kitna budget hai aapka?", "Aapka budget kitna hai?"),
        ] {
            assert!(similar_hint(a, b), "{a:?} vs {b:?}");
            assert!(similar_hint(b, a), "{b:?} vs {a:?}");
        }
    }

    #[test]
    fn a_hint_already_shown_is_found_among_the_earlier_ones() {
        let earlier = vec![
            "What's your monthly LLM bill running today?".to_string(),
            "Could we start the audit this week, before your board meeting?".to_string(),
        ];
        assert_eq!(
            repeat_of(
                &earlier,
                "Could we start the audit this week, ahead of the board meeting?"
            ),
            Some(earlier[1].as_str())
        );
        assert_eq!(
            repeat_of(&earlier, "Who else signs off besides Meera?"),
            None
        );
        assert_eq!(repeat_of(&[], "Anything"), None);
    }

    #[test]
    fn different_hints_on_one_topic_are_not_repeats() {
        for (a, b) in [
            (
                "Could a small pilot now be ready for your January budget round?",
                "Could we start the pilot this week, so it fits your six-week launch?",
            ),
            (
                "What's your monthly OpenAI spend?",
                "Which part of the OpenAI spend grows fastest?",
            ),
            ("Is that okay?", "Does Friday work?"),
            ("", ""),
        ] {
            assert!(!similar_hint(a, b), "{a:?} vs {b:?}");
        }
    }
    use crate::trigger::TriggerTiming;

    /// Live check against the configured provider: a scripted call, one request per
    /// trigger, printing hints, latency, and cache hits. `SCRIPT_ROUNDS=3` replays the script
    /// to grow the conversation past the model's minimum cacheable prompt.
    /// `cargo test scripted_call -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scripted_call_against_live_provider() {
        let mut settings = CoachSettings::from_env().expect("coach settings");
        // `SCRIPT_BRIEF=1` adds pre-call notes and an earlier call, to check hints use them;
        // `SCRIPT_BRIEF=first` is a first meeting: only the CRM record and the agenda.
        let script_brief = std::env::var("SCRIPT_BRIEF").ok();
        if script_brief.as_deref() == Some("first") {
            settings = settings.with_brief(CallBrief {
                prospect: "Rahul Mehta".into(),
                company: "FinStack".into(),
                agenda: "First call. Goals: understand their AI setup and what it costs them, \
                         qualify budget and timeline, book a technical deep-dive."
                    .into(),
                crm_context:
                    "- Person: Rahul Mehta, CTO at FinStack (fintech lending, 40 people)\n\
                     - Source: website form, message: \"Our LLM costs are growing faster than \
                     our users\"\n\
                     - Deal: FinStack AI cost audit, stage Discovery"
                        .into(),
                ..CallBrief::default()
            });
        } else if script_brief.is_some() {
            settings = settings.with_brief(CallBrief {
                prospect: "Rahul".into(),
                company: "FinStack".into(),
                agenda: "Second call. Last time they asked for an AI Architecture Audit                          proposal. Goal: find out who signs off and lock a start date."
                    .into(),
                past_calls: "Call on 2026-09-18 (rep: rep@example.com)\n\
                     - Overview: Their chatbot prototype's OpenAI bill tripled in two months.\n\
                     - Objections: Consultants are expensive for a startup our size\n\
                     - Decision process: Rahul decides with his co-founder Meera\n\
                     - Commitments: we: Send the audit proposal; they: Share last month's bill\n\
                     - Agreed next step: Review the proposal this week"
                    .into(),
                ..CallBrief::default()
            });
        }
        let script: &[(TriggerReason, &[(Channel, &str)])] = &[
            (
                TriggerReason::TurnEnd,
                &[
                    (
                        Channel::Me,
                        "Thanks for taking the time. What made you reach out?",
                    ),
                    (
                        Channel::Them,
                        "We built a chatbot prototype but our OpenAI bill is getting out of hand.",
                    ),
                ],
            ),
            (
                TriggerReason::Keyword {
                    keyword: "expensive".into(),
                },
                &[
                    (Channel::Me, "We could look at optimizing that for you."),
                    (
                        Channel::Them,
                        "Maybe, but consultants are usually expensive for a startup our size.",
                    ),
                ],
            ),
            (
                TriggerReason::Silence,
                &[(
                    Channel::Them,
                    "We also have two developers in-house already.",
                )],
            ),
            (
                TriggerReason::Keyword {
                    keyword: "timeline".into(),
                },
                &[(
                    Channel::Them,
                    "Our timeline is tight, we're demoing to investors next month.",
                )],
            ),
            (
                TriggerReason::TurnEnd,
                &[
                    (Channel::Me, "Understood."),
                    (
                        Channel::Them,
                        "Okay. Can you send me an email with some details?",
                    ),
                ],
            ),
        ];

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let rounds: usize = std::env::var("SCRIPT_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let mut history = Vec::new();
        // The opening turn first, as a real call sends it, then the script.
        let triggers = script
            .iter()
            .cycle()
            .take(script.len() * rounds)
            .map(|(reason, lines)| {
                let now = Instant::now();
                let trigger = Trigger {
                    reason: reason.clone(),
                    lines: lines
                        .iter()
                        .map(|(channel, text)| Line {
                            channel: *channel,
                            text: text.to_string(),
                        })
                        .collect(),
                    timing: TriggerTiming {
                        speech_end_at: None,
                        transcript_at: now,
                        fired_at: now,
                    },
                };
                format_turn(&trigger)
            });
        for turn in std::iter::once(OPENING_TURN.to_string()).chain(triggers) {
            history.push(Turn {
                role: Role::User,
                text: turn,
            });
            let started = Instant::now();
            let response = runtime
                .block_on(
                    settings
                        .provider
                        .generate(&client, &hint_request(&settings.system, &history)),
                )
                .expect("llm call");
            println!(
                "{:>5} ms | in {} (cache read {}, write {}) out {} | {}",
                started.elapsed().as_millis(),
                response.usage.input_tokens,
                response.usage.cache_read_tokens,
                response.usage.cache_write_tokens,
                response.usage.output_tokens,
                response.text.replace('\n', " "),
            );
            assert!(HintsJson::shown(&response.text, MAX_OPENING_HINTS).is_some());
            history.push(Turn {
                role: Role::Model,
                text: response.text,
            });
        }
    }

    /// Full live chain on real hardware: loopback capture → Deepgram → trigger engine →
    /// Claude, printing measured speech-to-hint latency per hint. Play speech through
    /// the speakers while it runs: `cargo test live_speech_to_hint -- --ignored --nocapture`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_speech_to_hint() {
        use crate::audio::{wasapi_capture, CaptureEvent, LoopbackTarget};
        use crate::stt::{SttEvent, SttProvider};
        use crate::trigger::TriggerEngine;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        let settings = CoachSettings::from_env().expect("coach settings");
        let stt = SttProvider::from_env().expect("stt settings");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();

        let stop = Arc::new(AtomicBool::new(false));
        let (capture_tx, capture_rx) = mpsc::channel();
        let capture = wasapi_capture::spawn(
            Channel::Them,
            LoopbackTarget::DefaultOutput,
            stop.clone(),
            capture_tx,
        )
        .unwrap();
        let (audio_tx, audio_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        match &stt {
            SttProvider::Deepgram {
                api_key,
                language,
                mip_opt_out,
                keyterms,
            } => {
                runtime.spawn(crate::stt::deepgram_run_for_tests(
                    api_key.clone(),
                    language.clone(),
                    *mip_opt_out,
                    keyterms.clone(),
                    Channel::Them,
                    audio_rx,
                    events_tx,
                ));
            }
        }
        let forward = std::thread::spawn(move || {
            for event in capture_rx {
                if let CaptureEvent::Chunk(chunk) = event {
                    let _ = audio_tx.send(crate::audio::SttAudio {
                        samples: chunk.samples,
                        captured_at: chunk.captured_at,
                        ends_speech: false,
                    });
                }
            }
        });

        let client = reqwest::Client::new();
        let latencies = runtime.block_on(async {
            let mut engine = TriggerEngine::with_default_keywords();
            let mut history = Vec::new();
            let mut latencies = Vec::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(26);
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            loop {
                let trigger = tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    event = events_rx.recv() => match event {
                        Some(SttEvent::Transcript(t)) => engine.on_transcript(&t, Instant::now()),
                        Some(SttEvent::Failed { message, .. }) => panic!("stt: {message}"),
                        _ => None,
                    },
                    _ = tick.tick() => engine.on_tick(Instant::now()),
                };
                let Some(trigger) = trigger else { continue };
                history.push(Turn {
                    role: Role::User,
                    text: format_turn(&trigger),
                });
                let response = settings
                    .provider
                    .generate(&client, &hint_request(&settings.system, &history))
                    .await
                    .expect("llm call");
                let done = Instant::now();
                let timing = trigger.timing;
                let speech_to_hint = timing
                    .speech_end_at
                    .map(|at| done.duration_since(at).as_millis());
                let stt_lag = timing
                    .speech_end_at
                    .map(|at| timing.transcript_at.duration_since(at).as_millis());
                println!(
                    "{:?}: speech→hint {:?} ms (stt {:?}, trigger wait {} ms, llm {} ms) | {}",
                    trigger.reason,
                    speech_to_hint,
                    stt_lag,
                    timing
                        .fired_at
                        .duration_since(timing.transcript_at)
                        .as_millis(),
                    done.duration_since(timing.fired_at).as_millis(),
                    response.text,
                );
                if let Some(ms) = speech_to_hint {
                    latencies.push(ms as u64);
                }
                history.push(Turn {
                    role: Role::Model,
                    text: response.text,
                });
            }
            latencies
        });
        stop.store(true, Ordering::Relaxed);
        capture.join().unwrap();
        forward.join().unwrap();
        println!(
            "speech→hint p50 {:?} ms, p90 {:?} ms over {} hints",
            percentile(&latencies, 0.5),
            percentile(&latencies, 0.9),
            latencies.len()
        );
        assert!(!latencies.is_empty(), "no hints produced");
    }

    #[test]
    fn opted_out_stt_pays_double_unless_a_rate_is_set() {
        assert_eq!(
            stt_price_per_minute(Some(0.0048), None, false),
            Some(0.0048)
        );
        assert_eq!(stt_price_per_minute(Some(0.0048), None, true), Some(0.0096));
        assert_eq!(
            stt_price_per_minute(Some(0.0048), Some(0.0077), true),
            Some(0.0077)
        );
        assert_eq!(stt_price_per_minute(None, None, true), None);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = [100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];
        assert_eq!(percentile(&values, 0.5), Some(500));
        assert_eq!(percentile(&values, 0.9), Some(900));
        assert_eq!(percentile(&[42], 0.9), Some(42));
        assert_eq!(percentile(&[], 0.9), None);
    }

    #[test]
    fn call_brief_skips_empty_fields_and_titles_the_call() {
        let empty = CallBrief::default();
        assert_eq!(empty.to_prompt(), None);
        assert_eq!(empty.title(), "Call summary");

        let brief = CallBrief {
            prospect: " Priya ".into(),
            company: "Acme".into(),
            agenda: String::new(),
            ..CallBrief::default()
        };
        assert_eq!(
            brief.to_prompt().as_deref(),
            Some(
                "- Prospect: Priya
- Company: Acme"
            )
        );
        assert_eq!(brief.title(), "Call with Priya (Acme)");
        let company_only = CallBrief {
            company: "Acme".into(),
            ..CallBrief::default()
        };
        assert_eq!(company_only.title(), "Call with Acme");
    }

    #[test]
    fn crm_history_goes_into_the_brief_the_coach_caches() {
        let brief = CallBrief {
            prospect: "Priya".into(),
            crm_person_id: Some("p-1".into()),
            crm_context: "- Deal: Acme audit\n- Agreed next step: send pricing".into(),
            ..CallBrief::default()
        };
        let prompt = brief.to_prompt().expect("a brief with a prospect");
        assert!(prompt.starts_with("- Prospect: Priya"), "{prompt}");
        assert!(
            prompt.contains("From the CRM, before this call:"),
            "{prompt}"
        );
        assert!(
            prompt.contains("- Agreed next step: send pricing"),
            "{prompt}"
        );

        // A CRM that said nothing leaves the brief exactly as it was.
        let with_files = CallBrief {
            company: "FinStack".into(),
            client_files: format!("### Note\n\nMeera owns the budget. {}", "x".repeat(200_000)),
            ..CallBrief::default()
        };
        let prompt = with_files.to_prompt().expect("a brief with files");
        assert!(prompt.contains("Files and notes the team keeps about their company"));
        assert!(prompt.contains("Meera owns the budget."));
        assert!(
            prompt.len() < MAX_CLIENT_FILES_CHARS + 1000,
            "files are clipped"
        );
        assert!(
            !serde_json::to_string(&with_files)
                .unwrap()
                .contains("Meera"),
            "the saved call doesn't carry the files"
        );

        let quiet = CallBrief {
            prospect: "Priya".into(),
            crm_context: "   ".into(),
            ..CallBrief::default()
        };
        assert_eq!(quiet.to_prompt().as_deref(), Some("- Prospect: Priya"));
    }

    #[test]
    fn hints_without_text_are_dropped_and_capped() {
        let reply = r#"{"hints": [
            {"type": "question", "urgency": "high", "text": " Did you share the bill? "},
            {"type": "reminder", "urgency": "low", "text": "  "},
            {"type": "fact", "urgency": "medium", "text": "Second"}
        ]}"#;
        let shown = HintsJson::shown(reply, 3).unwrap();
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].text, "Did you share the bill?");
        assert_eq!(HintsJson::shown(reply, 1).unwrap().len(), 1);
        assert!(HintsJson::shown(r#"{"hints": []}"#, 1).unwrap().is_empty());
        assert!(HintsJson::shown("not json", 1).is_none());
    }

    #[test]
    fn long_agendas_are_clipped() {
        let brief = CallBrief {
            agenda: "x".repeat(MAX_AGENDA_CHARS + 500),
            ..CallBrief::default()
        };
        let prompt = brief.to_prompt().unwrap();
        assert_eq!(
            prompt.len(),
            "- Agenda and goals for this meeting: ".len() + MAX_AGENDA_CHARS
        );
    }

    #[test]
    fn llm_cost_bills_cache_reads_and_writes_at_their_rates() {
        // Haiku 4.5: $1 input, 1.25x cache write, 0.1x cache read, $5 output.
        let prices = Prices {
            input_per_mtok: Some(1.00),
            cache_write_per_mtok: Some(1.25),
            cache_read_per_mtok: Some(0.10),
            output_per_mtok: Some(5.00),
            stt_per_minute: None,
        };
        let usage = Usage {
            input_tokens: 3_000_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 1_000_000,
            output_tokens: 100_000,
        };
        let cost = prices.llm_cost(&usage).unwrap();
        assert!((cost - (1.00 + 1.25 + 0.10 + 0.50)).abs() < 1e-9);
        assert!(Prices::default().llm_cost(&usage).is_none());
    }
}

#[cfg(test)]
mod demo_pipeline_tests {
    use super::*;
    use crate::stt::SttEvent;
    use crate::trigger::TriggerEngine;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    /// Live: runs the demo script through triggers, Haiku hints, and the Sonnet summary,
    /// without the UI. `cargo test live_demo_pipeline -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_demo_pipeline() {
        let settings = CoachSettings::from_env().expect("coach settings");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(crate::demo::run(
                tx,
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
            ));
            let client = reqwest::Client::new();
            let mut engine = TriggerEngine::with_default_keywords();
            let mut history = Vec::new();
            let mut transcript = Vec::new();
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            loop {
                let trigger = tokio::select! {
                    event = rx.recv() => match event {
                        Some(SttEvent::Transcript(t)) => {
                            if t.is_final {
                                transcript.push(Line { channel: t.channel, text: t.text.clone() });
                            }
                            engine.on_transcript(&t, Instant::now())
                        }
                        Some(_) => None,
                        None => break,
                    },
                    _ = tick.tick() => engine.on_tick(Instant::now()),
                };
                let Some(trigger) = trigger else { continue };
                history.push(Turn {
                    role: Role::User,
                    text: format_turn(&trigger),
                });
                let started = Instant::now();
                let response = settings
                    .provider
                    .generate(&client, &hint_request(&settings.system, &history))
                    .await
                    .expect("hint");
                let hint = HintsJson::shown(&response.text, 1).expect("hints JSON");
                println!(
                    "{:?} → {} ms, cache read {} | {}",
                    trigger.reason,
                    started.elapsed().as_millis(),
                    response.usage.cache_read_tokens,
                    hint.first().map_or("(no hint)", |hint| hint.text.as_str())
                );
                history.push(Turn {
                    role: Role::Model,
                    text: response.text,
                });
            }
            let call = call_summary::generate(
                &client,
                &settings.summary_provider,
                &settings.playbook,
                None,
                &transcript,
            )
            .await
            .expect("summary");
            println!(
                "\n{}",
                call_summary::to_markdown(&call.summary, "Call summary", &[])
            );
        });
    }

    #[test]
    fn the_hint_text_is_written_first_so_it_streams_first() {
        let fields: Vec<&String> = HINT_SCHEMA["properties"]["hints"]["items"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(fields[0], "text", "{fields:?}");
    }

    #[test]
    fn a_streamed_hint_grows_in_the_windows() {
        let mut stream = HintStream::new("session-1#3".into());
        assert_eq!(stream.on_piece(r#"{"hints":[{"te"#), None);
        assert!(stream.first_words_at.is_none());
        assert_eq!(
            stream.on_piece(r#"xt":"Ask what"#),
            Some(HintDelta {
                id: "session-1#3".into(),
                text: "Ask what".into()
            })
        );
        let first = stream.first_words_at.expect("first words time");
        assert_eq!(
            stream
                .on_piece(r#" budget they have","type":"question""#)
                .map(|d| d.text),
            Some("Ask what budget they have".into())
        );
        assert_eq!(
            stream.first_words_at,
            Some(first),
            "first words time doesn't move"
        );
        assert_eq!(stream.on_piece(r#","urgency":"high"}]}"#), None);
    }

    #[test]
    fn the_reps_question_reaches_the_model() {
        let mut engine = crate::trigger::TriggerEngine::with_default_keywords();
        let typed = engine.ask(
            Some("How do I answer the AWS point?".into()),
            Instant::now(),
        );
        assert!(format_turn(&typed)
            .starts_with("Trigger: the rep asked: \"How do I answer the AWS point?\"\n"));
        let hotkey = engine.ask(None, Instant::now());
        assert!(format_turn(&hotkey).starts_with("Trigger: the rep asked what to say now\n"));
        assert!(
            SYSTEM_PROMPT.contains("When the trigger is the rep asking"),
            "the model is told to always answer the rep"
        );
    }
}

#[cfg(test)]
#[path = "coach_eval.rs"]
mod eval;
