//! The Settings screen: non-secret settings saved to `<app data>/settings.json`, the
//! playbook file, and whether the API keys are configured. Keys themselves aren't shown or
//! stored here; they stay in the environment or `.env`.

use crate::config::{env_or_dotenv, saved_settings, setting_source, settings_path};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fs;

pub const LANGUAGES: [&str; 4] = ["en", "en-IN", "multi", "hi"];

/// The settings the screen edits, with their defaults.
const EDITABLE: [(&str, &str); 6] = [
    ("DEEPGRAM_LANGUAGE", "en"),
    ("DEEPGRAM_MIP_OPT_OUT", "on"),
    ("CALL_DETECTION", "on"),
    ("CALL_MAX_MINUTES", "180"),
    ("CALL_AUTO_STOP_SILENCE_MINUTES", "15"),
    ("CALL_MAX_COST_USD", "5"),
];

/// Smallest playbook the screen will save; an empty playbook would leave hints with no facts.
const MIN_PLAYBOOK_CHARS: usize = 200;

#[derive(Deserialize)]
pub struct SettingsInput {
    language: String,
    max_call_minutes: f64,
    auto_stop_silence_minutes: f64,
    max_call_cost_usd: f64,
    call_detection: bool,
    deepgram_mip_opt_out: bool,
}

fn validate(input: &SettingsInput) -> Result<(), String> {
    if !LANGUAGES.contains(&input.language.as_str()) {
        return Err(format!(
            "Unknown transcription language {:?}",
            input.language
        ));
    }
    for (label, minutes) in [
        ("Maximum call length", input.max_call_minutes),
        ("Auto-stop after silence", input.auto_stop_silence_minutes),
    ] {
        if !(minutes.is_finite() && minutes > 0.0 && minutes <= 24.0 * 60.0) {
            return Err(format!(
                "{label} must be more than 0 and at most 1440 minutes"
            ));
        }
    }
    if !(input.max_call_cost_usd.is_finite()
        && input.max_call_cost_usd > 0.0
        && input.max_call_cost_usd <= 1000.0)
    {
        return Err("Spend limit per call must be more than $0 and at most $1000".into());
    }
    Ok(())
}

#[tauri::command]
pub fn get_settings() -> Value {
    let setting = |key: &str, default: &str| {
        json!({
            "value": env_or_dotenv(key).unwrap_or_else(|| default.to_string()),
            "source": setting_source(key),
        })
    };
    let values: Map<String, Value> = EDITABLE
        .iter()
        .map(|(key, default)| (key.to_string(), setting(key, default)))
        .collect();
    let key_set = |key: &str| env_or_dotenv(key).is_some_and(|v| !v.trim().is_empty());
    let playbook = crate::coach::playbook_path();
    json!({
        "values": values,
        "keys": {
            "deepgram": key_set("DEEPGRAM_API_KEY"),
            "anthropic": key_set("ANTHROPIC_API_KEY"),
        },
        "playbook": {
            "path": playbook.display().to_string(),
            "text": fs::read_to_string(&playbook).ok(),
        },
        "settings_file": settings_path().map(|p| p.display().to_string()),
    })
}

/// Saves the editable settings; they apply from the next call.
#[tauri::command]
pub fn save_settings(input: SettingsInput) -> Result<(), String> {
    validate(&input)?;
    let path = settings_path().ok_or("The app data folder isn't available")?;
    let mut saved = saved_settings();
    saved.insert("DEEPGRAM_LANGUAGE".into(), json!(input.language));
    saved.insert(
        "CALL_DETECTION".into(),
        json!(if input.call_detection { "on" } else { "off" }),
    );
    saved.insert(
        "DEEPGRAM_MIP_OPT_OUT".into(),
        json!(if input.deepgram_mip_opt_out {
            "on"
        } else {
            "off"
        }),
    );
    saved.insert(
        "CALL_MAX_MINUTES".into(),
        json!(input.max_call_minutes.to_string()),
    );
    saved.insert(
        "CALL_AUTO_STOP_SILENCE_MINUTES".into(),
        json!(input.auto_stop_silence_minutes.to_string()),
    );
    saved.insert(
        "CALL_MAX_COST_USD".into(),
        json!(input.max_call_cost_usd.to_string()),
    );
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(saved)).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Could not save settings: {e}"))
}

/// Setting holding the WASAPI endpoint ID of the mic to capture; unset means the default.
pub const MIC_DEVICE_SETTING: &str = "MIC_DEVICE_ID";

/// Microphones Windows can record from, for the mic picker.
#[tauri::command]
pub fn list_mics() -> Result<Value, String> {
    #[cfg(windows)]
    let devices = crate::audio::wasapi_capture::list_input_devices()?;
    #[cfg(target_os = "macos")]
    let devices = crate::audio::coreaudio_capture::list_input_devices()?;
    #[cfg(not(any(windows, target_os = "macos")))]
    let devices: Vec<Value> = Vec::new();
    Ok(json!({
        "devices": devices,
        "chosen": env_or_dotenv(MIC_DEVICE_SETTING),
    }))
}

/// Saves the mic to record from on the next call; `None` goes back to the Windows default.
#[tauri::command]
pub fn set_mic(id: Option<String>) -> Result<(), String> {
    let path = settings_path().ok_or("The app data folder isn't available")?;
    let mut saved = saved_settings();
    let key = MIC_DEVICE_SETTING;
    match id.filter(|id| !id.trim().is_empty()) {
        Some(id) => saved.insert(key.into(), json!(id)),
        None => saved.remove(key),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(saved)).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Could not save the mic: {e}"))
}

/// Replaces the playbook file; the next call uses it.
#[tauri::command]
pub fn save_playbook(text: String) -> Result<(), String> {
    if text.trim().chars().count() < MIN_PLAYBOOK_CHARS {
        return Err(format!(
            "The playbook needs at least {MIN_PLAYBOOK_CHARS} characters; hints only use facts from it"
        ));
    }
    let path = crate::coach::playbook_path();
    fs::write(&path, text).map_err(|e| format!("Could not save {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(language: &str, max: f64, silence: f64) -> SettingsInput {
        SettingsInput {
            language: language.into(),
            max_call_minutes: max,
            auto_stop_silence_minutes: silence,
            max_call_cost_usd: 5.0,
            call_detection: true,
            deepgram_mip_opt_out: true,
        }
    }

    #[test]
    fn accepts_known_languages_and_sensible_limits() {
        assert!(validate(&input("en-IN", 180.0, 15.0)).is_ok());
        assert!(validate(&input("multi", 0.5, 0.25)).is_ok());
    }

    #[test]
    fn rejects_unknown_languages_and_bad_limits() {
        assert!(validate(&input("fr", 180.0, 15.0)).is_err());
        assert!(validate(&input("en", 0.0, 15.0)).is_err());
        assert!(validate(&input("en", 180.0, f64::NAN)).is_err());
        assert!(validate(&input("en", 5000.0, 15.0)).is_err());
    }
}
