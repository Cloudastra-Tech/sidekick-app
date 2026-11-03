//! Past calls saved under `<app data>/calls/`: `call-<ms>.md` (readable transcript and
//! summary) and, for calls since call history was added, `call-<ms>.json` (the same data,
//! structured, for the history screen).

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub fn calls_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("calls"))
}

#[derive(Serialize)]
pub struct CallListItem {
    id: String,
    title: String,
    /// When the call's file was created (the call's end), from its ID.
    ended_at_unix_ms: Option<u64>,
    /// `ready`, `failed`, `skipped`, or `transcript_only` for older calls without a summary.
    status: String,
    /// `live`, `solo_test`, or `demo`; unknown for calls saved before history existed.
    mode: Option<String>,
}

/// IDs are file stems like `call-1789650670473`; anything else is rejected, so an ID can't
/// point outside the calls folder.
fn checked_id(id: &str) -> Result<&str, String> {
    let digits = id.strip_prefix("call-").unwrap_or("");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("Not a call ID: {id:?}"));
    }
    Ok(id)
}

fn read_record(dir: &Path, id: &str) -> Option<Value> {
    let text = fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

fn list_item(dir: &Path, id: String) -> CallListItem {
    let ended_at_unix_ms = id.strip_prefix("call-").and_then(|ms| ms.parse().ok());
    if let Some(record) = read_record(dir, &id) {
        return CallListItem {
            title: record["title"].as_str().unwrap_or("Call").to_string(),
            status: record["call"]["status"]
                .as_str()
                .unwrap_or("unknown")
                .to_string(),
            mode: record["mode"].as_str().map(str::to_string),
            ended_at_unix_ms,
            id,
        };
    }
    let markdown = fs::read_to_string(dir.join(format!("{id}.md"))).unwrap_or_default();
    let title = markdown
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# "))
        .unwrap_or("Call")
        .to_string();
    let status = if markdown.contains("Summary not written yet") {
        "transcript_only"
    } else {
        "ready"
    };
    CallListItem {
        id,
        title,
        ended_at_unix_ms,
        status: status.into(),
        mode: None,
    }
}

/// IDs of saved calls: one per `call-<ms>.md`. A call still in progress only has a
/// `.journal.jsonl`, so it isn't listed until it's saved (or recovered).
fn call_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".md")?;
            checked_id(id).ok().map(str::to_string)
        })
        .collect()
}

/// Every saved call, newest first.
#[tauri::command]
pub fn list_calls(app: AppHandle) -> Result<Vec<CallListItem>, String> {
    let dir = calls_dir(&app)?;
    let mut items: Vec<CallListItem> = call_ids(&dir)
        .into_iter()
        .map(|id| list_item(&dir, id))
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.ended_at_unix_ms));
    Ok(items)
}

/// `{ id, record, markdown }`: the structured record when there is one, and the Markdown file.
#[tauri::command]
pub fn get_call(app: AppHandle, id: String) -> Result<Value, String> {
    let id = checked_id(&id)?;
    let dir = calls_dir(&app)?;
    let markdown = fs::read_to_string(dir.join(format!("{id}.md"))).ok();
    let record = read_record(&dir, id);
    if markdown.is_none() && record.is_none() {
        return Err("This call's files are gone".into());
    }
    Ok(serde_json::json!({ "id": id, "record": record, "markdown": markdown }))
}

/// Deletes a call's saved transcript and summary.
#[tauri::command]
pub fn delete_call(app: AppHandle, id: String) -> Result<(), String> {
    let id = checked_id(&id)?;
    let dir = calls_dir(&app)?;
    for extension in ["md", "json"] {
        let path = dir.join(format!("{id}.{extension}"));
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| format!("Could not delete {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_call_ids_are_accepted() {
        assert!(checked_id("call-1789650670473").is_ok());
        for bad in [
            "call-",
            "call-12a",
            "../consent_log",
            "call-1/../../x",
            "session-1",
        ] {
            assert!(checked_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn journals_of_calls_in_progress_are_not_listed() {
        let dir = std::env::temp_dir().join(format!("sidekick-history-ids-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("call-1.md"), "# Call\n").unwrap();
        fs::write(dir.join("call-1.json"), "{}").unwrap();
        fs::write(dir.join("call-2.journal.jsonl"), "{}\n").unwrap();

        assert_eq!(call_ids(&dir), vec!["call-1".to_string()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lists_structured_and_markdown_only_calls() {
        let dir = std::env::temp_dir().join(format!("sidekick-history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("call-100.md"),
            "# Call transcript\n\nSummary not written yet.\n",
        )
        .unwrap();
        fs::write(dir.join("call-200.md"), "# Call with Priya\n").unwrap();
        fs::write(
            dir.join("call-200.json"),
            r#"{"title":"Call with Priya","mode":"live","call":{"status":"ready"}}"#,
        )
        .unwrap();

        let old = list_item(&dir, "call-100".into());
        assert_eq!(
            (old.title.as_str(), old.status.as_str()),
            ("Call transcript", "transcript_only")
        );
        assert_eq!(old.ended_at_unix_ms, Some(100));
        let new = list_item(&dir, "call-200".into());
        assert_eq!(
            (new.title.as_str(), new.status.as_str()),
            ("Call with Priya", "ready")
        );
        assert_eq!(new.mode.as_deref(), Some("live"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
