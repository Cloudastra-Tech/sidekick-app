//! Event-driven trigger engine: decides *when* to ask the coaching model for a hint.
//!
//! There is no timer-based polling. A trigger needs a real event (turn end or silence) *and* new
//! remote-side speech since the last trigger, so nothing is ever re-sent for a conversation that
//! hasn't moved. Each trigger carries only the transcript lines added since the previous one — the
//! fresh, uncached part of the model prompt.
//!
//! A hint answers a whole turn, not half a sentence: speech-to-text calls a turn over after any
//! 300 ms pause, so the hint waits until the prospect has stayed quiet a little longer
//! (`TURN_SETTLE`, more after an unfinished sentence). A trigger word heard on the way names the
//! hint, and fires with the rest of the turn.

use crate::audio::Channel;
use crate::stt::Transcript;
use serde::Serialize;
use std::time::{Duration, Instant};

/// Quiet after the prospect's last word before the fallback trigger fires, counted from when
/// that word was captured (not from when its transcript arrived, which is an STT delay later).
pub const SILENCE_THRESHOLD: Duration = Duration::from_secs(2);

/// Quiet after the last word of a turn that ended on a full stop or question mark before its hint
/// fires. Speech-to-text reports the end about this long after the word anyway, so it adds
/// little delay, and a prospect who carries on after a short breath isn't cut off.
pub const TURN_SETTLE: Duration = Duration::from_millis(700);

/// The same, after a turn that stopped mid-sentence ("so our bill,"): people pause there to think.
pub const TURN_SETTLE_UNFINISHED: Duration = Duration::from_millis(1500);

/// Sales-call phrases worth an immediate hint when the other side says them.
/// Matched as whole words/phrases, case-insensitive.
const DEFAULT_KEYWORDS: &[&str] = &[
    "price",
    "prices",
    "pricing",
    "cost",
    "costs",
    "expensive",
    "budget",
    "discount",
    "competitor",
    "competitors",
    "contract",
    "timeline",
    "decision",
    "not interested",
    "think about it",
    "send me an email",
    "already using",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerReason {
    Keyword {
        keyword: String,
    },
    TurnEnd,
    Silence,
    /// The rep asked for help: the hotkey or button (`None`), or a typed question.
    Asked {
        question: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    pub channel: Channel,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Trigger {
    pub reason: TriggerReason,
    /// Final transcript lines (both channels, in order) since the previous trigger.
    pub lines: Vec<Line>,
    /// Latency markers for the remote speech this trigger responds to.
    #[serde(skip)]
    pub timing: TriggerTiming,
}

#[derive(Clone, Copy, Debug)]
pub struct TriggerTiming {
    /// When the audio of the latest remote final transcript was captured.
    pub speech_end_at: Option<Instant>,
    /// When that final transcript arrived from STT.
    pub transcript_at: Instant,
    pub fired_at: Instant,
}

impl TriggerTiming {
    /// Of two triggers answered by one request, the timing of the speech that waited longest.
    pub fn earliest(self, other: Self) -> Self {
        let earlier = |a: Option<Instant>, b: Option<Instant>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Self {
            speech_end_at: earlier(self.speech_end_at, other.speech_end_at),
            transcript_at: self.transcript_at.min(other.transcript_at),
            fired_at: self.fired_at.min(other.fired_at),
        }
    }
}

pub struct TriggerEngine {
    /// Normalized (see `normalize`) keyword phrases.
    keywords: Vec<String>,
    pending: Vec<Line>,
    /// Whether `pending` contains anything the remote side said.
    pending_from_them: bool,
    /// When the prospect's latest recognised speech was captured. Only their speech counts: the
    /// rep answering, or the prospect's voice echoing into the mic, must not hold back a hint
    /// about what the prospect just said.
    last_them_speech_at: Option<Instant>,
    /// (audio end, arrival) of the latest remote final transcript.
    last_them_final: Option<(Option<Instant>, Instant)>,
    /// Speech-to-text said the prospect's turn is over, and they haven't spoken since.
    turn_over: bool,
    /// Whether their latest line ended a sentence (a full stop, question mark, ...).
    sentence_finished: bool,
    /// The first trigger word heard since the last hint.
    keyword: Option<String>,
}

impl TriggerEngine {
    pub fn new<I, S>(keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            keywords: keywords
                .into_iter()
                .map(|k| normalize(k.as_ref()))
                .filter(|k| !k.trim().is_empty())
                .collect(),
            pending: Vec::new(),
            pending_from_them: false,
            last_them_speech_at: None,
            last_them_final: None,
            turn_over: false,
            sentence_finished: false,
            keyword: None,
        }
    }

    /// The built-in English list; live calls use the playbook's (`keywords_from_playbook`).
    #[cfg(test)]
    pub fn with_default_keywords() -> Self {
        Self::new(DEFAULT_KEYWORDS)
    }

    pub fn on_transcript(&mut self, transcript: &Transcript, now: Instant) -> Option<Trigger> {
        // Any recognised speech from the prospect, interim or final, restarts the silence clock
        // from when that speech was captured.
        if transcript.channel == Channel::Them {
            let spoken_at = transcript.audio_end_at.unwrap_or(now);
            self.last_them_speech_at = Some(
                self.last_them_speech_at
                    .map_or(spoken_at, |last| last.max(spoken_at)),
            );
        }
        if !transcript.is_final {
            // They're still talking: whatever ended before, the turn goes on.
            if transcript.channel == Channel::Them {
                self.turn_over = false;
            }
            return None;
        }
        self.pending.push(Line {
            channel: transcript.channel,
            text: transcript.text.clone(),
        });
        // Hints respond to the other side; our own speech is context only.
        if transcript.channel != Channel::Them {
            return None;
        }
        self.pending_from_them = true;
        self.last_them_final = Some((transcript.audio_end_at, now));
        self.sentence_finished = ends_sentence(&transcript.text);
        if self.keyword.is_none() {
            self.keyword = self.match_keyword(&transcript.text);
        }
        self.turn_over = transcript.speech_final;
        self.on_tick(now)
    }

    /// The speech-to-text provider says the prospect finished, with no new words (an empty
    /// final, a Finalize answer, an UtteranceEnd): end the turn now instead of waiting for the
    /// silence fallback.
    pub fn on_turn_end(&mut self, now: Instant) -> Option<Trigger> {
        if !self.pending_from_them {
            return None;
        }
        self.turn_over = true;
        self.on_tick(now)
    }

    /// When the next trigger is due, if one is waiting: a short while after a turn ends, or the
    /// silence fallback. Counted from when the prospect's last word was captured.
    pub fn deadline(&self) -> Option<Instant> {
        if !self.pending_from_them {
            return None;
        }
        let wait = match (self.turn_over, self.sentence_finished) {
            (true, true) => TURN_SETTLE,
            (true, false) => TURN_SETTLE_UNFINISHED,
            (false, _) => SILENCE_THRESHOLD,
        };
        self.last_them_speech_at.map(|at| at + wait)
    }

    /// Call at the deadline (and periodically as a backstop); fires once per turn or silence gap
    /// that follows new remote speech.
    pub fn on_tick(&mut self, now: Instant) -> Option<Trigger> {
        let due = self.deadline()?;
        if now < due {
            return None;
        }
        let reason = match self.keyword.take() {
            Some(keyword) => TriggerReason::Keyword { keyword },
            None if self.turn_over => TriggerReason::TurnEnd,
            None => TriggerReason::Silence,
        };
        Some(self.fire(reason, now))
    }

    fn match_keyword(&self, text: &str) -> Option<String> {
        let text = normalize(text);
        self.keywords
            .iter()
            .find(|k| text.contains(k.as_str()))
            .map(|k| k.trim().to_string())
    }

    /// The rep asked for a hint. Fires at once, with whatever was said since the last trigger
    /// (possibly nothing): asking doesn't wait for the prospect.
    pub fn ask(&mut self, question: Option<String>, now: Instant) -> Trigger {
        self.fire(TriggerReason::Asked { question }, now)
    }

    fn fire(&mut self, reason: TriggerReason, now: Instant) -> Trigger {
        self.pending_from_them = false;
        self.turn_over = false;
        self.keyword = None;
        let (speech_end_at, transcript_at) = self.last_them_final.unwrap_or((None, now));
        Trigger {
            reason,
            lines: std::mem::take(&mut self.pending),
            timing: TriggerTiming {
                speech_end_at,
                transcript_at,
                fired_at: now,
            },
        }
    }
}

/// Letters, digits and apostrophes, plus what holds Indic words together: Devanagari's virama
/// (्) and nukta, which aren't letters to Unicode, and the zero-width (non-)joiners. Without them
/// "क्या" would split into "क" and "या".
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
        || c == '\''
        || ('\u{0900}'..='\u{097F}').contains(&c)
        || matches!(c, '\u{200C}' | '\u{200D}')
}

/// Whether a line ends a sentence, by the punctuation speech-to-text added (Hindi's danda too).
fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .ends_with(['.', '?', '!', '\u{0964}', '\u{0965}'])
}

/// Lowercases, turns punctuation into spaces, and pads with spaces so `contains` on two
/// normalized strings only matches whole words ("cost" does not match "costume").
fn normalize(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !is_word_char(c))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    format!(" {} ", words.join(" "))
}

/// The playbook's "Trigger words" section, or the built-in English list when it has none.
pub fn keywords_from_playbook(playbook: &str) -> Vec<String> {
    let words = crate::playbook::section_items(playbook, "trigger words");
    if words.is_empty() {
        DEFAULT_KEYWORDS.iter().map(|w| w.to_string()).collect()
    } else {
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(channel: Channel, text: &str, is_final: bool, speech_final: bool) -> Transcript {
        Transcript {
            channel,
            text: text.into(),
            is_final,
            speech_final,
            audio_end_at: None,
        }
    }

    /// The prospect's speech, captured at `at` (arrival time is passed separately).
    fn said_at(text: &str, is_final: bool, speech_final: bool, at: Instant) -> Transcript {
        Transcript {
            audio_end_at: Some(at),
            ..said(Channel::Them, text, is_final, speech_final)
        }
    }

    #[test]
    fn a_finished_sentence_ends_the_turn_soon_after_its_last_word() {
        let mut engine = TriggerEngine::with_default_keywords();
        let spoken = Instant::now();
        let arrived = spoken + Duration::from_millis(500);
        let line = said_at("What's the pricing, roughly?", true, true, spoken);
        assert!(engine.on_transcript(&line, arrived).is_none());
        assert_eq!(engine.deadline(), Some(spoken + TURN_SETTLE));
        assert!(engine
            .on_tick(spoken + TURN_SETTLE - Duration::from_millis(1))
            .is_none());
        let trigger = engine.on_tick(spoken + TURN_SETTLE).unwrap();
        assert_eq!(
            trigger.reason,
            TriggerReason::Keyword {
                keyword: "pricing".into()
            }
        );
    }

    #[test]
    fn a_turn_end_that_arrives_late_fires_at_once() {
        let mut engine = TriggerEngine::with_default_keywords();
        let spoken = Instant::now();
        let line = said_at("We use AWS.", true, true, spoken);
        let trigger = engine.on_transcript(&line, spoken + TURN_SETTLE).unwrap();
        assert_eq!(trigger.reason, TriggerReason::TurnEnd);
    }

    #[test]
    fn a_short_pause_mid_sentence_does_not_end_the_turn() {
        let mut engine = TriggerEngine::with_default_keywords();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        // A 300 ms pause after "so basically our bill," makes Deepgram call the turn over.
        engine.on_transcript(&said_at("So basically our bill,", true, true, t0), ms(450));
        assert_eq!(engine.deadline(), Some(t0 + TURN_SETTLE_UNFINISHED));
        // They carry on.
        engine.on_transcript(&said_at("has", false, false, ms(900)), ms(1300));
        assert!(engine.on_tick(t0 + TURN_SETTLE_UNFINISHED).is_none());
        engine.on_transcript(
            &said_at("has doubled since March.", true, true, ms(2000)),
            ms(2450),
        );
        let trigger = engine.on_tick(ms(2000) + TURN_SETTLE).unwrap();
        assert_eq!(trigger.reason, TriggerReason::TurnEnd);
        let lines: Vec<_> = trigger.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            lines,
            ["So basically our bill,", "has doubled since March."]
        );
    }

    #[test]
    fn a_keyword_mid_sentence_waits_for_the_end_of_the_turn() {
        let mut engine = TriggerEngine::with_default_keywords();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let first = said_at("Honestly the cost is not really", true, false, t0);
        assert!(engine.on_transcript(&first, ms(400)).is_none());
        let rest = said_at("the problem, it's the timing.", true, true, ms(1500));
        assert!(engine.on_transcript(&rest, ms(1900)).is_none());
        let trigger = engine.on_tick(ms(1500) + TURN_SETTLE).unwrap();
        assert_eq!(
            trigger.reason,
            TriggerReason::Keyword {
                keyword: "cost".into()
            },
            "the first trigger word names the hint"
        );
        assert_eq!(trigger.lines.len(), 2, "the hint sees the whole turn");
    }

    #[test]
    fn keywords_match_whole_words_and_phrases_only() {
        assert!(!fires(DEFAULT_KEYWORDS, "Nice costume."));
        assert!(fires(DEFAULT_KEYWORDS, "Honestly, we're NOT interested."));
    }

    #[test]
    fn interim_results_and_own_speech_do_not_fire() {
        let mut engine = TriggerEngine::with_default_keywords();
        let now = Instant::now();
        let interim = said(Channel::Them, "what about the price", false, false);
        assert!(engine.on_transcript(&interim, now).is_none());
        let mine = said(Channel::Me, "Our price is fair.", true, true);
        assert!(engine.on_transcript(&mine, now).is_none());
    }

    #[test]
    fn turn_end_fires_and_carries_lines_since_last_trigger() {
        let mut engine = TriggerEngine::with_default_keywords();
        let now = Instant::now();
        engine.on_transcript(&said(Channel::Me, "How's the rollout?", true, true), now);
        let late = now + TURN_SETTLE;
        let trigger = engine
            .on_transcript(&said_at("Going slowly.", true, true, now), late)
            .unwrap();
        assert_eq!(trigger.reason, TriggerReason::TurnEnd);
        assert_eq!(trigger.lines.len(), 2);
        assert_eq!(trigger.lines[0].channel, Channel::Me);

        let later = late + TURN_SETTLE;
        let next = engine
            .on_transcript(&said_at("Team is small.", true, true, late), later)
            .unwrap();
        assert_eq!(next.lines.len(), 1, "earlier lines must not be re-sent");
    }

    #[test]
    fn silence_fires_once_after_new_remote_speech() {
        let mut engine = TriggerEngine::with_default_keywords();
        let start = Instant::now();
        engine.on_transcript(&said(Channel::Them, "Let me check.", true, false), start);

        assert!(engine
            .on_tick(start + Duration::from_millis(1900))
            .is_none());
        let trigger = engine.on_tick(start + Duration::from_millis(2100)).unwrap();
        assert_eq!(trigger.reason, TriggerReason::Silence);
        assert!(engine.on_tick(start + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn nothing_new_from_them_means_no_trigger() {
        let mut engine = TriggerEngine::with_default_keywords();
        let start = Instant::now();
        assert!(engine.on_tick(start + Duration::from_secs(5)).is_none());
        engine.on_transcript(&said(Channel::Me, "Just me talking.", true, true), start);
        assert!(engine.on_tick(start + Duration::from_secs(5)).is_none());
    }

    #[test]
    fn prospect_speech_resets_the_silence_clock() {
        let mut engine = TriggerEngine::with_default_keywords();
        let start = Instant::now();
        engine.on_transcript(&said(Channel::Them, "So anyway.", true, false), start);
        let later = start + Duration::from_millis(1500);
        engine.on_transcript(&said(Channel::Them, "and", false, false), later);
        assert!(engine
            .on_tick(start + Duration::from_millis(2500))
            .is_none());
        assert!(engine.on_tick(later + SILENCE_THRESHOLD).is_some());
    }

    #[test]
    fn the_reps_answer_does_not_hold_back_the_hint() {
        let mut engine = TriggerEngine::with_default_keywords();
        let start = Instant::now();
        engine.on_transcript(
            &said(Channel::Them, "We tried that before.", true, false),
            start,
        );
        // The rep starts answering, or the prospect's voice echoes into the mic.
        for ms in [300, 800, 1400, 1900] {
            let t = start + Duration::from_millis(ms);
            engine.on_transcript(&said(Channel::Me, "right, so", false, false), t);
        }
        assert_eq!(engine.deadline(), Some(start + SILENCE_THRESHOLD));
        assert!(engine.on_tick(start + SILENCE_THRESHOLD).is_some());
    }

    #[test]
    fn silence_counts_from_when_the_speech_was_captured() {
        let mut engine = TriggerEngine::with_default_keywords();
        let spoken = Instant::now();
        // The final arrives 1.2 s after the words it covers.
        let arrived = spoken + Duration::from_millis(1200);
        let mut line = said(Channel::Them, "Let me think.", true, false);
        line.audio_end_at = Some(spoken);
        engine.on_transcript(&line, arrived);
        assert_eq!(engine.deadline(), Some(spoken + SILENCE_THRESHOLD));
        assert!(engine
            .on_tick(arrived + Duration::from_millis(799))
            .is_none());
        assert!(engine.on_tick(spoken + SILENCE_THRESHOLD).is_some());
        assert_eq!(engine.deadline(), None, "fires once");
    }

    #[test]
    fn a_turn_end_signal_fires_only_for_new_prospect_speech() {
        let mut engine = TriggerEngine::with_default_keywords();
        let now = Instant::now();
        assert!(engine.on_turn_end(now).is_none());
        engine.on_transcript(&said(Channel::Them, "That's our setup.", true, false), now);
        // Deepgram's UtteranceEnd comes a second after the last word.
        let ended = now + Duration::from_secs(1);
        let trigger = engine.on_turn_end(ended).unwrap();
        assert_eq!(trigger.reason, TriggerReason::TurnEnd);
        assert_eq!(trigger.lines.len(), 1);
        assert!(engine.on_turn_end(ended).is_none(), "nothing new since");
        assert!(engine.on_tick(now + Duration::from_secs(10)).is_none());
    }

    fn fires(keywords: &[&str], said_text: &str) -> bool {
        let mut engine = TriggerEngine::new(keywords);
        let now = Instant::now();
        engine.on_transcript(&said(Channel::Them, said_text, true, true), now);
        engine
            .on_tick(now + SILENCE_THRESHOLD)
            .is_some_and(|t| matches!(t.reason, TriggerReason::Keyword { .. }))
    }

    #[test]
    fn hindi_words_match_whole_words_with_their_vowel_signs() {
        // "कम" (less) is not "कमी" (shortage): the vowel sign is part of the word.
        assert!(fires(&["कम"], "थोड़ा कम कर दीजिए"));
        assert!(!fires(&["कम"], "इसमें क्या कमी है"));
        assert!(fires(&["बाद में"], "हम बाद में बात करेंगे"));
        assert!(fires(&["महंगा"], "यह तो बहुत महंगा है।"));
    }

    #[test]
    fn a_hindi_half_letter_joins_its_word() {
        // The virama in "क्या" (kya) joins "क" to the rest of the word, so "क" alone isn't in it.
        assert!(!fires(&["क"], "आप क्या सोचते हैं"));
        assert!(fires(&["क्या"], "आप क्या सोचते हैं"));
        assert!(fires(&["डिस्काउंट"], "कोई डिस्काउंट मिलेगा?"));
    }

    #[test]
    fn romanized_hinglish_matches_like_english() {
        assert!(fires(&["kitna"], "Ye kitna hoga?"));
        assert!(fires(&["sochna padega"], "Humein sochna padega, thoda."));
        assert!(!fires(&["kitna"], "kitnaa"));
    }

    #[test]
    fn trigger_words_come_from_the_playbook_section() {
        let playbook = "## 9. Trigger words\nNotes are skipped.\n- pricing, kitna\n- कितना\n";
        assert_eq!(
            keywords_from_playbook(playbook),
            vec!["pricing", "kitna", "कितना"]
        );
    }

    #[test]
    fn without_a_section_the_built_in_words_are_used() {
        assert_eq!(
            keywords_from_playbook("# Playbook\n## Objections\n- Too expensive\n"),
            DEFAULT_KEYWORDS
        );
    }

    #[test]
    fn the_shipped_playbook_has_english_and_hinglish_trigger_words() {
        let words = keywords_from_playbook(include_str!("../playbook/sales.md"));
        for word in [
            "pricing",
            "not interested",
            "kitna",
            "कितना",
            "महंगा",
            "baad mein",
        ] {
            assert!(words.iter().any(|w| w == word), "{word} missing: {words:?}");
        }
    }

    #[test]
    fn the_rep_can_ask_at_any_time_with_what_was_said_since_the_last_hint() {
        let mut engine = TriggerEngine::with_default_keywords();
        let now = Instant::now();
        assert!(engine
            .on_transcript(
                &said(Channel::Them, "We run on AWS today", true, false),
                now
            )
            .is_none());
        let asked = engine.ask(Some("How do I answer the AWS point?".into()), now);
        assert_eq!(
            asked.reason,
            TriggerReason::Asked {
                question: Some("How do I answer the AWS point?".into())
            }
        );
        assert_eq!(
            asked.lines.len(),
            1,
            "carries the lines since the last trigger"
        );
        // Nothing new was said since: asking still works, with no lines.
        let again = engine.ask(None, now);
        assert_eq!(again.reason, TriggerReason::Asked { question: None });
        assert!(again.lines.is_empty());
        // The lines went with the question: no silence trigger repeats them.
        assert!(engine.on_tick(now + SILENCE_THRESHOLD * 2).is_none());
    }
}
