//! Crash-safe call journal. Every final transcript line is appended to
//! `<app data>/calls/call-<start ms>.journal.jsonl` as the call happens, so a crash or a killed
//! app never loses the conversation. The first line is a header describing the call. When the
//! call ends normally its `.md`/`.json` are written and the journal is removed; a journal still
//! there at the next launch is turned into a recovered call.

use crate::audio::Channel;
use crate::call_summary;
use crate::trigger::Line;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// One transcript line as it is journaled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalLine {
    pub channel: Channel,
    pub text: String,
    pub at_unix_ms: u64,
}

/// What a journal file holds.
#[derive(Debug)]
pub struct JournalContents {
    pub header: Value,
    pub lines: Vec<JournalLine>,
}

/// The journal of the call in progress. Writing failures never interrupt the call.
pub struct Journal {
    file: Option<File>,
    path: Option<PathBuf>,
}

impl Journal {
    /// Creates `call-<started_unix_ms>.journal.jsonl` in `dir` and writes the header.
    pub fn open(dir: &Path, started_unix_ms: u128, header: &Value) -> Self {
        let path = dir.join(format!("call-{started_unix_ms}.journal.jsonl"));
        let file = fs::create_dir_all(dir)
            .and_then(|()| OpenOptions::new().create(true).append(true).open(&path))
            .ok();
        let mut journal = Self {
            file,
            path: Some(path),
        };
        journal.write_line(&header.to_string());
        journal
    }

    /// Appends one final line, flushed to disk before returning.
    pub fn append(&mut self, channel: Channel, text: &str) {
        let line = JournalLine {
            channel,
            text: text.to_string(),
            at_unix_ms: now_unix_ms(),
        };
        if let Ok(json) = serde_json::to_string(&line) {
            self.write_line(&json);
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The call was saved normally: the journal is no longer needed.
    pub fn finish(mut self) {
        self.file = None;
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }

    /// Lines are few (one per spoken sentence), so each is synced to disk: a power cut, not
    /// just a crash, keeps everything up to the last line.
    fn write_line(&mut self, line: &str) {
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{line}").and_then(|()| file.sync_data());
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Reads a journal. A torn last line (the app died mid-write) is skipped.
pub fn read(path: &Path) -> Option<JournalContents> {
    let text = fs::read_to_string(path).ok()?;
    let mut rows = text.lines();
    let header: Value = serde_json::from_str(rows.next()?).ok()?;
    let lines = rows
        .filter_map(|row| serde_json::from_str::<JournalLine>(row).ok())
        .collect();
    Some(JournalContents { header, lines })
}

/// Turns journals left by calls that never finished (crash, killed app, power cut) into normal
/// saved calls: `call-<start ms>.md` with the transcript, and a `.json` record marked as failed
/// with the reason. Returns the IDs recovered. A journal whose call was saved anyway, or that
/// holds no speech, is just removed.
pub fn recover(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let journals: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().ends_with(JOURNAL_SUFFIX))
        .collect();
    journals
        .into_iter()
        .filter_map(|path| recover_one(dir, &path))
        .collect()
}

const JOURNAL_SUFFIX: &str = ".journal.jsonl";
const RECOVERED_ERROR: &str =
    "Sidekick closed before this call ended, so there's no summary. The transcript was recovered.";

fn recover_one(dir: &Path, journal: &Path) -> Option<String> {
    let name = journal.file_name()?.to_str()?;
    let id = name.strip_suffix(JOURNAL_SUFFIX)?.to_string();
    let already_saved = dir.join(format!("{id}.md")).exists();
    let contents = read(journal).filter(|c| !c.lines.is_empty());
    let Some(contents) = contents.filter(|_| !already_saved) else {
        // Saved normally after all, or nothing was said: nothing to recover.
        let _ = fs::remove_file(journal);
        return None;
    };
    if !write_recovered(dir, &id, &contents) {
        // Keep the journal to try again at the next launch.
        return None;
    }
    let _ = fs::remove_file(journal);
    Some(id)
}

fn write_recovered(dir: &Path, id: &str, contents: &JournalContents) -> bool {
    let header = &contents.header;
    let title = format!("{} (recovered)", header["title"].as_str().unwrap_or("Call"));
    let transcript: Vec<Line> = contents
        .lines
        .iter()
        .map(|line| Line {
            channel: line.channel,
            text: line.text.clone(),
        })
        .collect();
    let markdown_path = dir.join(format!("{id}.md"));
    let record = json!({
        "version": 1,
        "title": title,
        "brief": header.get("brief").cloned().unwrap_or(Value::Null),
        "mode": header.get("mode").cloned().unwrap_or(Value::Null),
        "call": {
            "status": "failed",
            "error": RECOVERED_ERROR,
            "file": markdown_path.display().to_string(),
        },
        "stats": Value::Null,
        "transcript": transcript,
    });
    fs::write(
        &markdown_path,
        call_summary::transcript_markdown(&title, &transcript),
    )
    .is_ok()
        && fs::write(dir.join(format!("{id}.json")), record.to_string()).is_ok()
}

/// The transcript so far of the call in progress, for a window that reloaded mid-call.
#[derive(Debug, Serialize)]
pub struct LiveTranscript {
    pub started_at_unix_ms: u64,
    pub lines: Vec<JournalLine>,
}

fn live_transcript_from(path: Option<&Path>) -> Option<LiveTranscript> {
    let contents = read(path?)?;
    Some(LiveTranscript {
        started_at_unix_ms: contents.header["started_at_unix_ms"].as_u64().unwrap_or(0),
        lines: contents.lines,
    })
}

/// Where the running call's journal is, if a call is running.
#[derive(Default)]
pub struct CurrentJournal(Mutex<Option<PathBuf>>);

impl CurrentJournal {
    pub fn set(&self, path: Option<PathBuf>) {
        *self.0.lock().expect("journal lock poisoned") = path;
    }
}

/// The transcript so far of the call in progress, or `None` when no call is running.
#[tauri::command]
pub fn live_transcript(current: tauri::State<'_, CurrentJournal>) -> Option<LiveTranscript> {
    let path = current.0.lock().expect("journal lock poisoned").clone();
    live_transcript_from(path.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sidekick-journal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lines_written_during_the_call_can_be_read_back() {
        let dir = temp_dir("roundtrip");
        let header = json!({ "kind": "start", "title": "Call with Asha", "mode": "live" });
        let mut journal = Journal::open(&dir, 1_700_000_000_000, &header);
        journal.append(Channel::Them, "What does it cost?");
        journal.append(Channel::Me, "It depends on scope.");

        let path = journal.path().unwrap().to_path_buf();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "call-1700000000000.journal.jsonl"
        );
        let contents = read(&path).unwrap();
        assert_eq!(contents.header["title"], "Call with Asha");
        let texts: Vec<(Channel, &str)> = contents
            .lines
            .iter()
            .map(|l| (l.channel, l.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            vec![
                (Channel::Them, "What does it cost?"),
                (Channel::Me, "It depends on scope.")
            ]
        );
        assert!(contents.lines.iter().all(|l| l.at_unix_ms > 0));
    }

    #[test]
    fn a_line_torn_by_a_crash_is_skipped_and_earlier_lines_kept() {
        let dir = temp_dir("torn");
        let mut journal = Journal::open(&dir, 1, &json!({ "kind": "start" }));
        journal.append(Channel::Them, "First line");
        let path = journal.path().unwrap().to_path_buf();
        drop(journal);
        // The app died halfway through writing the next line.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, b"{\"channel\":\"me\",\"te").unwrap();

        let contents = read(&path).unwrap();
        assert_eq!(contents.lines.len(), 1);
        assert_eq!(contents.lines[0].text, "First line");
    }

    #[test]
    fn finishing_removes_the_journal() {
        let dir = temp_dir("finish");
        let mut journal = Journal::open(&dir, 2, &json!({ "kind": "start" }));
        journal.append(Channel::Me, "Hello");
        let path = journal.path().unwrap().to_path_buf();
        journal.finish();
        assert!(!path.exists());
    }

    #[test]
    fn an_unfinished_call_is_recovered_into_history() {
        let dir = temp_dir("recover");
        let header = json!({
            "kind": "start",
            "title": "Call with Asha",
            "mode": "live",
            "brief": { "prospect": "Asha" },
        });
        let mut journal = Journal::open(&dir, 1_700_000_000_000, &header);
        journal.append(Channel::Them, "What does it cost?");
        journal.append(Channel::Me, "It depends on scope.");
        let path = journal.path().unwrap().to_path_buf();
        drop(journal); // the app died: no finish()

        assert_eq!(recover(&dir), vec!["call-1700000000000".to_string()]);

        assert!(!path.exists(), "the journal is consumed");
        let markdown = std::fs::read_to_string(dir.join("call-1700000000000.md")).unwrap();
        assert!(markdown.starts_with("# Call with Asha (recovered)"));
        assert!(markdown.contains("What does it cost?"));
        let record: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("call-1700000000000.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["title"], "Call with Asha (recovered)");
        assert_eq!(record["mode"], "live");
        assert_eq!(record["brief"]["prospect"], "Asha");
        assert_eq!(record["call"]["status"], "failed");
        assert!(record["call"]["error"].as_str().unwrap().contains("closed"));
        assert_eq!(record["transcript"][1]["text"], "It depends on scope.");
        assert_eq!(record["transcript"][1]["channel"], "me");
    }

    #[test]
    fn a_journal_with_no_speech_is_removed_without_a_call() {
        let dir = temp_dir("empty");
        let journal = Journal::open(&dir, 5, &json!({ "kind": "start", "title": "Call" }));
        let path = journal.path().unwrap().to_path_buf();
        drop(journal);

        assert!(recover(&dir).is_empty());
        assert!(!path.exists());
        assert!(!dir.join("call-5.md").exists());
    }

    #[test]
    fn a_call_saved_anyway_is_not_duplicated() {
        let dir = temp_dir("saved");
        let mut journal = Journal::open(&dir, 7, &json!({ "kind": "start", "title": "Call" }));
        journal.append(Channel::Them, "Hello");
        let path = journal.path().unwrap().to_path_buf();
        drop(journal);
        std::fs::write(dir.join("call-7.md"), "# Call\n\nsaved normally").unwrap();

        assert!(recover(&dir).is_empty());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("call-7.md")).unwrap(),
            "# Call\n\nsaved normally"
        );
    }

    #[test]
    fn a_reloaded_window_gets_the_transcript_so_far() {
        let dir = temp_dir("live");
        let header = json!({ "kind": "start", "started_at_unix_ms": 1_700_000_000_000u64 });
        let mut journal = Journal::open(&dir, 1_700_000_000_000, &header);
        journal.append(Channel::Them, "Hi there");

        let live = live_transcript_from(journal.path()).unwrap();
        assert_eq!(live.started_at_unix_ms, 1_700_000_000_000);
        assert_eq!(live.lines.len(), 1);
        assert_eq!(live.lines[0].text, "Hi there");

        assert!(live_transcript_from(None).is_none(), "no call running");
    }
}
