//! Silence gate: stops sending audio to STT while nobody is speaking on a stream, since
//! streamed STT is billed by the second. A real mic is never digitally silent (room noise,
//! hiss), so without this a call bills both streams for its whole length.
//!
//! Energy-based voice activity detection. The noise floor is the quietest chunk of the last
//! few seconds (speech always has short dips between words), so it follows the room without
//! drifting into speech. Gated chunks become
//! digital silence, which the STT layer doesn't send (it sends Finalize/KeepAlive instead).
//! To avoid clipping the start of speech, the last few gated chunks are released when
//! speech begins, and the gate stays open briefly after speech ends.

use std::collections::VecDeque;

/// Level above the noise floor that counts as speech.
const SPEECH_ABOVE_FLOOR_DB: f64 = 9.0;
/// Quietest level that can count as speech, whatever the floor.
const MIN_SPEECH_DB: f64 = -55.0;
/// Chunks (100 ms each) the noise floor looks back over.
const FLOOR_WINDOW_CHUNKS: usize = 50;
/// Below this a chunk is digital silence (stream start-up, loopback gap fill), not room
/// noise, and doesn't lower the floor.
const DIGITAL_SILENCE_DB: f64 = -85.0;
/// Chunks kept open after the last speech chunk (100 ms each): trailing soft syllables.
const HANGOVER_CHUNKS: u32 = 8;
/// Chunks held while gated and released when speech starts: leading consonants.
const PRE_ROLL_CHUNKS: usize = 3;

/// Tells speech from the room's noise, one chunk at a time, against a floor that follows the
/// room. Shared by the gate and by the check that the prospect can be heard.
pub struct SpeechDetector {
    /// Recent chunk levels (dBFS), excluding digital silence.
    recent_db: VecDeque<f64>,
}

impl Default for SpeechDetector {
    fn default() -> Self {
        Self {
            recent_db: VecDeque::with_capacity(FLOOR_WINDOW_CHUNKS),
        }
    }
}

impl SpeechDetector {
    /// The quietest recent chunk; with no history yet, low enough that anything above the
    /// minimum speech level counts.
    fn floor_db(&self) -> f64 {
        if self.recent_db.is_empty() {
            return MIN_SPEECH_DB - SPEECH_ABOVE_FLOOR_DB;
        }
        self.recent_db.iter().cloned().fold(f64::INFINITY, f64::min)
    }

    /// Whether a chunk at `level` dBFS (see [`level_db`]) is speech, and adds it to the floor.
    pub fn is_speech(&mut self, level: f64) -> bool {
        if level > DIGITAL_SILENCE_DB {
            if self.recent_db.len() == FLOOR_WINDOW_CHUNKS {
                self.recent_db.pop_front();
            }
            self.recent_db.push_back(level);
        }
        level >= MIN_SPEECH_DB && level >= self.floor_db() + SPEECH_ABOVE_FLOOR_DB
    }
}

/// Below this a chunk is digital silence: nothing is playing at all.
pub fn is_digital_silence(level: f64) -> bool {
    level <= DIGITAL_SILENCE_DB
}

pub struct SilenceGate {
    speech: SpeechDetector,
    hangover: u32,
    held: VecDeque<Vec<i16>>,
    /// Whether the gate is passing audio (speech, or its hangover).
    open: bool,
    /// The last `process` call closed the gate.
    closed_now: bool,
}

impl Default for SilenceGate {
    fn default() -> Self {
        Self {
            speech: SpeechDetector::default(),
            hangover: 0,
            held: VecDeque::with_capacity(PRE_ROLL_CHUNKS),
            open: false,
            closed_now: false,
        }
    }
}

pub fn level_db(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return -120.0;
    }
    let mean_square = samples
        .iter()
        .map(|&s| {
            let v = f64::from(s) / f64::from(i16::MAX);
            v * v
        })
        .sum::<f64>()
        / samples.len() as f64;
    10.0 * mean_square.max(1e-12).log10()
}

impl SilenceGate {
    /// Takes one captured chunk and returns what to send, in order: nothing changes while
    /// speech continues; while gated, a silent chunk of the same length; at speech onset,
    /// the held pre-roll chunks followed by this one.
    pub fn process(&mut self, samples: Vec<i16>) -> Vec<Vec<i16>> {
        self.closed_now = false;
        let speech = self.speech.is_speech(level_db(&samples));

        if speech {
            let was_gated = self.hangover == 0;
            self.hangover = HANGOVER_CHUNKS;
            self.open = true;
            if was_gated {
                let mut out: Vec<Vec<i16>> = self.held.drain(..).collect();
                out.push(samples);
                return out;
            }
            return vec![samples];
        }
        if self.hangover > 0 {
            self.hangover -= 1;
            return vec![samples];
        }
        let silent = vec![0; samples.len()];
        self.closed_now = std::mem::take(&mut self.open);
        if self.held.len() == PRE_ROLL_CHUNKS {
            self.held.pop_front();
        }
        self.held.push_back(samples);
        vec![silent]
    }

    /// Whether the last chunk processed closed the gate: speech followed by the whole hangover
    /// without any. The last chunk it returned is then the first silent one.
    pub fn just_closed(&self) -> bool {
        self.closed_now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = 1600;

    /// Deterministic noise at roughly `db` dBFS.
    fn noise(db: f64, seed: &mut u32) -> Vec<i16> {
        let amplitude = 10f64.powf(db / 20.0) * f64::from(i16::MAX) * 3f64.sqrt();
        (0..CHUNK)
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let unit = f64::from(*seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0;
                (unit * amplitude) as i16
            })
            .collect()
    }

    fn sent(out: &[Vec<i16>]) -> usize {
        out.iter().filter(|c| c.iter().any(|&s| s != 0)).count()
    }

    /// Live: records the default communications mic for `GATE_SECS` (default 20) and prints
    /// the level and whether the gate sent each second. Play or speak something part-way
    /// through. `cargo test mic_gate_live -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn mic_gate_live() {
        use crate::audio::{CaptureEvent, Channel, LoopbackTarget};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};
        use std::time::{Duration, Instant};

        let secs: u64 = std::env::var("GATE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<CaptureEvent>();
        let handle = crate::audio::wasapi_capture::spawn(
            Channel::Me,
            LoopbackTarget::DefaultOutput,
            stop.clone(),
            tx,
        )
        .unwrap();
        let mut gate = SilenceGate::default();
        let (mut total, mut sent_chunks) = (0usize, 0usize);
        let mut second = (Vec::new(), 0usize);
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            let Ok(CaptureEvent::Chunk(chunk)) = rx.recv_timeout(Duration::from_millis(500)) else {
                continue;
            };
            second.0.push(level_db(&chunk.samples));
            let out = gate.process(chunk.samples);
            let n = sent(&out);
            second.1 += n.min(1);
            sent_chunks += n;
            total += 1;
            if second.0.len() == 10 {
                let peak = second.0.iter().cloned().fold(f64::MIN, f64::max);
                let quietest = second.0.iter().cloned().fold(f64::MAX, f64::min);
                println!(
                    "{:>2}s  quietest {:>6.1}  peak {:>6.1} dBFS  floor {:>6.1}  sent {:>2}/10  {}",
                    total / 10,
                    quietest,
                    peak,
                    gate.speech.floor_db(),
                    second.1,
                    "#".repeat(second.1)
                );
                second = (Vec::new(), 0);
            }
        }
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        println!(
            "sent {sent_chunks} of {total} chunks ({:.0}%)",
            100.0 * sent_chunks as f64 / total.max(1) as f64
        );
    }

    #[test]
    fn closing_is_reported_once_after_the_hangover() {
        let mut gate = SilenceGate::default();
        let mut seed = 1;
        for _ in 0..20 {
            gate.process(noise(-60.0, &mut seed));
        }
        assert!(!gate.just_closed(), "never opened");
        gate.process(noise(-25.0, &mut seed));
        let closed_at: Vec<usize> = (0..(HANGOVER_CHUNKS as usize + 5))
            .filter(|_| {
                gate.process(noise(-60.0, &mut seed));
                gate.just_closed()
            })
            .collect();
        assert_eq!(closed_at.len(), 1);
    }

    #[test]
    fn room_noise_is_not_sent() {
        let mut gate = SilenceGate::default();
        let mut seed = 1;
        let mut real = 0;
        for i in 0..100 {
            let out = gate.process(noise(-62.0, &mut seed));
            // The first chunk sets the floor; nothing after it is speech.
            if i > 0 {
                real += sent(&out);
            }
        }
        assert_eq!(real, 0);
    }

    #[test]
    fn speech_is_sent_with_pre_roll_and_hangover() {
        let mut gate = SilenceGate::default();
        let mut seed = 7;
        for _ in 0..20 {
            gate.process(noise(-60.0, &mut seed));
        }
        // Speech starts: the 3 held chunks come out before it.
        let onset = gate.process(noise(-25.0, &mut seed));
        assert_eq!(onset.len(), PRE_ROLL_CHUNKS + 1);
        assert_eq!(sent(&onset), PRE_ROLL_CHUNKS + 1);
        for _ in 0..10 {
            assert_eq!(sent(&gate.process(noise(-25.0, &mut seed))), 1);
        }
        // Speech stops: sent for the hangover, then gated.
        let after: Vec<usize> = (0..12)
            .map(|_| sent(&gate.process(noise(-60.0, &mut seed))))
            .collect();
        assert_eq!(after.iter().sum::<usize>(), HANGOVER_CHUNKS as usize);
        assert_eq!(after[HANGOVER_CHUNKS as usize..], [0, 0, 0, 0]);
    }

    #[test]
    fn quiet_speech_over_a_quiet_room_is_sent() {
        let mut gate = SilenceGate::default();
        let mut seed = 3;
        for _ in 0..20 {
            gate.process(noise(-70.0, &mut seed));
        }
        assert!(sent(&gate.process(noise(-45.0, &mut seed))) > 0);
    }

    #[test]
    fn steady_noise_stops_counting_as_speech_within_the_floor_window() {
        let mut gate = SilenceGate::default();
        let mut seed = 5;
        for _ in 0..10 {
            gate.process(noise(-65.0, &mut seed));
        }
        // A fan turns on: sent at first, gated once the window only holds fan noise.
        let sent_per_chunk: Vec<usize> = (0..100)
            .map(|_| sent(&gate.process(noise(-45.0, &mut seed))))
            .collect();
        assert_eq!(sent_per_chunk[0], 1 + PRE_ROLL_CHUNKS);
        assert_eq!(
            sent_per_chunk[FLOOR_WINDOW_CHUNKS + HANGOVER_CHUNKS as usize..]
                .iter()
                .sum::<usize>(),
            0
        );
    }

    #[test]
    fn digital_silence_at_start_up_does_not_lower_the_floor() {
        let mut gate = SilenceGate::default();
        let mut seed = 9;
        for _ in 0..5 {
            gate.process(vec![0; CHUNK]);
        }
        // Room noise right after: the first few chunks may pass, then it's gated.
        let later: usize = (0..40)
            .map(|i| {
                let n = sent(&gate.process(noise(-57.0, &mut seed)));
                if i >= 20 {
                    n
                } else {
                    0
                }
            })
            .sum();
        assert_eq!(later, 0);
    }
}
