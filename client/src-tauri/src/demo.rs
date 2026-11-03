//! Demo call: plays a scripted sales call as transcripts, with no mic, capture, or STT
//! (so no Deepgram usage). Triggers, live hints, call stats, and the post-call summary all
//! run for real on the scripted lines.

use crate::audio::Channel;
use crate::stt::{SttEvent, Transcript};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

/// Roughly conversational speaking speed, used to pace each line.
const WORDS_PER_SECOND: f64 = 2.6;
/// How often a partial ("interim") transcript is shown while a line is being "spoken".
const INTERIM_EVERY: Duration = Duration::from_millis(700);
const LEAD_IN: Duration = Duration::from_secs(1);

struct ScriptLine {
    channel: Channel,
    text: &'static str,
    /// Silence after the line. Over 2 s lets a silence trigger fire.
    pause_after_ms: u64,
}

const fn line(channel: Channel, text: &'static str, pause_after_ms: u64) -> ScriptLine {
    ScriptLine {
        channel,
        text,
        pause_after_ms,
    }
}

const SCRIPT: &[ScriptLine] = &[
    line(Channel::Me, "Hi, thanks for making the time today. What prompted you to reach out?", 1200),
    line(Channel::Them, "We run a customer support chatbot on OpenAI, and our bill has gone up to around six thousand dollars a month.", 2800),
    line(Channel::Me, "That's a lot. Is most of that coming from the model calls themselves?", 1200),
    line(Channel::Them, "Mostly, yes. And honestly, the agencies we've talked to are too expensive for a company our size.", 3000),
    line(Channel::Me, "Understood. Our senior AI engineers start at eighteen dollars an hour, with no long-term commitment.", 1200),
    line(Channel::Them, "Okay, that's more reasonable. But we already have three developers in house.", 3000),
    line(Channel::Me, "That works well. We usually work alongside in-house teams rather than replacing them.", 1200),
    line(Channel::Them, "We have a board meeting in six weeks, and I need to show the costs coming down by then.", 3000),
    line(Channel::Me, "A short architecture audit would show exactly where the spend is going. Who else would be involved in the decision?", 1200),
    line(Channel::Them, "Our CTO would need to sign off. Can you just send me an email with some details?", 3000),
    line(Channel::Me, "Sure, I'll send it today. Could we set up a thirty minute call with your CTO on Thursday?", 1200),
    line(Channel::Them, "Thursday afternoon works. Let's do that.", 1500),
];

/// Plays the script into `events` until it ends or `stop` is set, holding while `paused` is
/// set. Dropping `events` on return is what ends the session, exactly like the STT streams
/// closing on a live call.
pub async fn run(
    events: UnboundedSender<SttEvent>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) {
    if !pace(LEAD_IN, &stop, &paused).await {
        return;
    }

    for script_line in SCRIPT {
        let words: Vec<&str> = script_line.text.split_whitespace().collect();
        let speaking = Duration::from_secs_f64(words.len() as f64 / WORDS_PER_SECOND);
        let steps = (speaking.as_millis() / INTERIM_EVERY.as_millis()).max(1) as usize;

        for step in 1..steps {
            if !pace(INTERIM_EVERY, &stop, &paused).await {
                return;
            }
            let partial = words[..words.len() * step / steps].join(" ");
            send(&events, script_line.channel, partial, false);
        }
        let rest = speaking.saturating_sub(INTERIM_EVERY * (steps as u32 - 1));
        if !pace(rest, &stop, &paused).await {
            return;
        }
        send(
            &events,
            script_line.channel,
            script_line.text.to_string(),
            true,
        );

        if !pace(
            Duration::from_millis(script_line.pause_after_ms),
            &stop,
            &paused,
        )
        .await
        {
            return;
        }
    }
}

/// Waits `duration`, then for as long as the call is paused. Returns false once stopped.
async fn pace(duration: Duration, stop: &AtomicBool, paused: &AtomicBool) -> bool {
    tokio::time::sleep(duration).await;
    while paused.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    !stop.load(Ordering::Relaxed)
}

fn send(events: &UnboundedSender<SttEvent>, channel: Channel, text: String, is_final: bool) {
    let _ = events.send(SttEvent::Transcript(Transcript {
        channel,
        text,
        is_final,
        speech_final: is_final,
        // No real STT: the line "finishes" as it is delivered, so measured latency covers
        // triggers and the model only.
        audio_end_at: is_final.then(Instant::now),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn pausing_holds_the_script_until_resumed() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (tx, mut rx) = unbounded_channel();
            let stop = Arc::new(AtomicBool::new(false));
            let paused = Arc::new(AtomicBool::new(true));
            let demo = tokio::spawn(run(tx, stop.clone(), paused.clone()));
            tokio::time::sleep(LEAD_IN + Duration::from_millis(1500)).await;
            assert!(rx.try_recv().is_err(), "nothing should play while paused");

            paused.store(false, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(1500)).await;
            assert!(
                rx.try_recv().is_ok(),
                "the script should continue after resuming"
            );

            stop.store(true, Ordering::Relaxed);
            tokio::time::timeout(Duration::from_secs(3), demo)
                .await
                .expect("demo should stop")
                .unwrap();
        });
    }

    #[test]
    fn stopping_ends_the_demo_early_and_closes_the_stream() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (tx, mut rx) = unbounded_channel();
            let stop = Arc::new(AtomicBool::new(false));
            let demo = tokio::spawn(run(tx, stop.clone(), Arc::new(AtomicBool::new(false))));
            tokio::time::sleep(LEAD_IN + Duration::from_millis(1500)).await;
            stop.store(true, Ordering::Relaxed);
            tokio::time::timeout(Duration::from_secs(3), demo)
                .await
                .expect("demo should stop promptly")
                .unwrap();

            let mut received = 0;
            while let Some(SttEvent::Transcript(t)) = rx.recv().await {
                assert_eq!(t.channel, Channel::Me, "only the first line had started");
                received += 1;
            }
            assert!(received > 0, "some partial text should have been sent");
        });
    }
}
