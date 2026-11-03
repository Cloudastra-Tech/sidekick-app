//! Whether the prospect can be heard. When the call's sound plays on a different speaker or
//! headset than the one captured, the prospect's side stays silent, nothing triggers a hint,
//! and the rep only finds out after the call. This watches both sides as audio arrives and says
//! when the rep has been talking for a while with nothing from the prospect, and when the
//! prospect is heard again. It also keeps each side's levels for the call's log.
//!
//! It only reads levels the capture pump already has, beside the hint path, so it adds no
//! latency to hints.

use super::gate::{is_digital_silence, level_db, SpeechDetector};
use super::{duration_of, Channel};
use serde::Serialize;
use std::time::Duration;

/// Rep speech with no prospect sound before the first warning, while the prospect hasn't been
/// heard at all yet: a call nearly always starts with both sides saying hello.
pub const NEVER_HEARD_AFTER: Duration = Duration::from_secs(20);
/// The same once the prospect has been heard: the rep may be presenting, so wait longer.
pub const NOT_HEARD_AGAIN_AFTER: Duration = Duration::from_secs(60);

/// A change worth telling the rep about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HearingChange {
    /// The rep has talked for `rep_speech` with nothing from the prospect's side.
    NotHeard {
        rep_speech: Duration,
        /// Whether the prospect was heard earlier in the call.
        heard_before: bool,
    },
    /// The prospect is heard again after a `NotHeard`.
    Heard,
}

#[derive(Default)]
struct ChannelLevels {
    detector: SpeechDetector,
    total: Duration,
    speech: Duration,
    /// Time with nothing playing at all (digital silence), as opposed to quiet room noise.
    digital_silence: Duration,
    /// Sum and count of levels (dBFS) of the chunks that weren't digital silence.
    level_sum: f64,
    level_chunks: u64,
    peak_db: Option<f64>,
}

impl ChannelLevels {
    /// Adds a chunk; returns whether it was speech.
    fn add(&mut self, samples: &[i16]) -> bool {
        let level = level_db(samples);
        let length = duration_of(samples.len());
        self.total += length;
        if is_digital_silence(level) {
            self.digital_silence += length;
        } else {
            self.level_sum += level;
            self.level_chunks += 1;
            self.peak_db = Some(self.peak_db.map_or(level, |peak| peak.max(level)));
        }
        let speech = self.detector.is_speech(level);
        if speech {
            self.speech += length;
        }
        speech
    }

    fn report(&self) -> ChannelReport {
        let round = |value: f64| (value * 10.0).round() / 10.0;
        let total = self.total.as_secs_f64();
        ChannelReport {
            speech_seconds: round(self.speech.as_secs_f64()),
            silent_share: (total > 0.0)
                .then(|| (self.digital_silence.as_secs_f64() / total * 100.0).round() / 100.0),
            avg_db: (self.level_chunks > 0)
                .then(|| round(self.level_sum / self.level_chunks as f64)),
            peak_db: self.peak_db.map(round),
        }
    }
}

/// One side's audio over the call, for the call's log.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChannelReport {
    /// Time that sounded like speech.
    pub speech_seconds: f64,
    /// Share of the call (0–1) with nothing playing at all; near 1 on the prospect's side means
    /// the captured device played nothing.
    pub silent_share: Option<f64>,
    /// Average and loudest level in dBFS, over the time something played.
    pub avg_db: Option<f64>,
    pub peak_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AudioReport {
    /// Times the rep was told the prospect couldn't be heard.
    pub prospect_warnings: u32,
    pub me: ChannelReport,
    pub them: ChannelReport,
}

pub struct HearingCheck {
    /// Off in a solo test, where the mic plays the prospect and the call audio is unused.
    warn: bool,
    me: ChannelLevels,
    them: ChannelLevels,
    heard: bool,
    rep_speech_since_heard: Duration,
    warning: bool,
    warnings: u32,
}

impl HearingCheck {
    pub fn new(warn: bool) -> Self {
        Self {
            warn,
            me: ChannelLevels::default(),
            them: ChannelLevels::default(),
            heard: false,
            rep_speech_since_heard: Duration::ZERO,
            warning: false,
            warnings: 0,
        }
    }

    /// Takes one captured chunk (not while the call is paused).
    pub fn on_chunk(&mut self, channel: Channel, samples: &[i16]) -> Option<HearingChange> {
        match channel {
            Channel::Them => {
                if !self.them.add(samples) {
                    return None;
                }
                self.heard = true;
                self.rep_speech_since_heard = Duration::ZERO;
                std::mem::take(&mut self.warning).then_some(HearingChange::Heard)
            }
            Channel::Me => {
                if !self.me.add(samples) {
                    return None;
                }
                self.rep_speech_since_heard += duration_of(samples.len());
                let limit = if self.heard {
                    NOT_HEARD_AGAIN_AFTER
                } else {
                    NEVER_HEARD_AFTER
                };
                if !self.warn || self.warning || self.rep_speech_since_heard < limit {
                    return None;
                }
                self.warning = true;
                self.warnings += 1;
                Some(HearingChange::NotHeard {
                    rep_speech: self.rep_speech_since_heard,
                    heard_before: self.heard,
                })
            }
        }
    }

    pub fn report(&self) -> AudioReport {
        AudioReport {
            prospect_warnings: self.warnings,
            me: self.me.report(),
            them: self.them.report(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = 1600; // 100 ms at 16 kHz

    fn tone(amplitude: i16) -> Vec<i16> {
        (0..CHUNK)
            .map(|i| if i % 2 == 0 { amplitude } else { -amplitude })
            .collect()
    }

    fn room_noise() -> Vec<i16> {
        tone(30)
    }

    fn silence() -> Vec<i16> {
        vec![0; CHUNK]
    }

    /// Someone talking: a 200 ms dip between words, then 800 ms of words, as real speech has.
    /// The detector needs a quiet moment before speech to know the floor.
    fn speaking(chunk: u32) -> Vec<i16> {
        if chunk % 10 < 2 {
            room_noise()
        } else {
            tone(8000)
        }
    }

    /// Feeds `seconds` of the rep talking, with the prospect's side silent.
    fn rep_talks(check: &mut HearingCheck, seconds: u32) -> Vec<HearingChange> {
        let mut changes = Vec::new();
        for chunk in 0..seconds * 10 {
            changes.extend(check.on_chunk(Channel::Me, &speaking(chunk)));
            changes.extend(check.on_chunk(Channel::Them, &silence()));
        }
        changes
    }

    fn prospect_talks(check: &mut HearingCheck, seconds: u32) -> Vec<HearingChange> {
        let mut changes = Vec::new();
        for chunk in 0..seconds * 10 {
            changes.extend(check.on_chunk(Channel::Me, &room_noise()));
            changes.extend(check.on_chunk(Channel::Them, &speaking(chunk)));
        }
        changes
    }

    /// Room noise first, so the floor is set the way a real call starts.
    fn settled(warn: bool) -> HearingCheck {
        let mut check = HearingCheck::new(warn);
        for _ in 0..20 {
            check.on_chunk(Channel::Me, &room_noise());
        }
        check
    }

    #[test]
    fn warns_once_when_the_prospect_is_never_heard() {
        let mut check = settled(true);
        // 24 s of talk is 19.2 s of speech: not yet.
        assert!(rep_talks(&mut check, 24).is_empty());
        let changes = rep_talks(&mut check, 20);
        assert_eq!(changes.len(), 1, "one warning, not one per chunk");
        assert!(matches!(
            changes[0],
            HearingChange::NotHeard { heard_before: false, rep_speech } if rep_speech >= NEVER_HEARD_AFTER
        ));
        assert_eq!(check.report().prospect_warnings, 1);
    }

    #[test]
    fn clears_when_the_prospect_is_heard() {
        let mut check = settled(true);
        assert_eq!(rep_talks(&mut check, 26).len(), 1);
        assert_eq!(prospect_talks(&mut check, 1), vec![HearingChange::Heard]);
    }

    #[test]
    fn waits_longer_once_the_prospect_was_heard() {
        let mut check = settled(true);
        prospect_talks(&mut check, 2);
        // The rep presenting for half a minute is normal.
        assert!(rep_talks(&mut check, 30).is_empty());
        let changes = rep_talks(&mut check, 50);
        assert!(matches!(
            changes[..],
            [HearingChange::NotHeard {
                heard_before: true,
                ..
            }]
        ));
    }

    #[test]
    fn prospect_speech_restarts_the_count() {
        let mut check = settled(true);
        for _ in 0..6 {
            assert!(rep_talks(&mut check, 20).is_empty());
            assert!(prospect_talks(&mut check, 2).is_empty());
        }
        assert_eq!(check.report().prospect_warnings, 0);
    }

    #[test]
    fn quiet_rep_never_warns() {
        let mut check = settled(true);
        for _ in 0..600 {
            assert_eq!(check.on_chunk(Channel::Me, &room_noise()), None);
            assert_eq!(check.on_chunk(Channel::Them, &silence()), None);
        }
    }

    #[test]
    fn solo_test_keeps_levels_but_never_warns() {
        let mut check = settled(false);
        assert!(rep_talks(&mut check, 60).is_empty());
        assert_eq!(check.report().me.speech_seconds, 48.0);
    }

    #[test]
    fn report_shows_a_silent_prospect_side() {
        let mut check = settled(true);
        rep_talks(&mut check, 10);
        let report = check.report();
        assert_eq!(report.them.silent_share, Some(1.0));
        assert_eq!(report.them.avg_db, None);
        assert_eq!(report.them.speech_seconds, 0.0);
        assert!(report.me.peak_db.is_some_and(|db| db > -20.0));
    }

    #[test]
    fn report_shows_a_heard_prospect() {
        let mut check = settled(true);
        prospect_talks(&mut check, 3);
        let them = check.report().them;
        assert_eq!(them.silent_share, Some(0.0));
        assert_eq!(them.speech_seconds, 2.4);
        assert!(them.avg_db.is_some_and(|db| db > -30.0));
    }
}
