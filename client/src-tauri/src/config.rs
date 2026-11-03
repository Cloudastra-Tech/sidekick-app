//! Runtime settings. Lookup order: environment variables, then settings saved from the
//! Settings screen (`<app data>/settings.json`), then a local `.env` file.

use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::OnceLock;

const SETTINGS_FILE: &str = "settings.json";
static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Called once at startup so settings saved from the app can be found.
pub fn init(app_data_dir: PathBuf) {
    let _ = APP_DATA_DIR.set(app_data_dir);
}

/// Called once at startup with where the files shipped inside the app (the default playbook)
/// were installed.
pub fn init_resources(resource_dir: PathBuf) {
    let _ = RESOURCE_DIR.set(resource_dir);
}

/// Where the app keeps settings, secrets, calls and metrics.
pub fn app_data_dir() -> Option<PathBuf> {
    APP_DATA_DIR.get().cloned()
}

pub fn settings_path() -> Option<PathBuf> {
    Some(APP_DATA_DIR.get()?.join(SETTINGS_FILE))
}

/// Settings saved from the app, as `{ "KEY": "value" }`.
pub fn saved_settings() -> Map<String, Value> {
    settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

/// Where a setting's value comes from, for the Settings screen.
pub fn setting_source(key: &str) -> &'static str {
    if std::env::var(key).is_ok() {
        "environment"
    } else if saved_settings().contains_key(key) {
        "app"
    } else if dotenv_value(key).is_some() {
        "env_file"
    } else {
        "default"
    }
}

pub fn env_or_dotenv(key: &str) -> Option<String> {
    if let Ok(value) = std::env::var(key) {
        return Some(value);
    }
    if let Some(value) = saved_settings().get(key).and_then(Value::as_str) {
        return Some(value.to_string());
    }
    dotenv_value(key)
}

fn dotenv_value(key: &str) -> Option<String> {
    // Dev builds also read the crate's own `.env`, wherever they were started from.
    let dev_env =
        cfg!(debug_assertions).then(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/.env")));

    std::iter::once(PathBuf::from(".env"))
        .chain(dev_env)
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .flat_map(|contents| {
            contents
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .filter_map(|line| line.trim().split_once('='))
                .filter(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
                .collect::<Vec<_>>()
        })
        .find(|v| !v.is_empty())
}

/// Parses an optional numeric setting; unset means `None`, malformed is an error.
pub fn env_f64(key: &str) -> Result<Option<f64>, String> {
    env_or_dotenv(key)
        .map(|v| {
            v.parse()
                .map_err(|_| format!("{key} must be a number, got {v:?}"))
        })
        .transpose()
}

/// Resolves a file shipped next to the crate (e.g. the playbook). In dev builds the
/// working directory varies, so fall back to the crate directory.
/// Installed apps don't start in the crate directory (a Mac app starts in `/`), so look in the
/// files shipped with the app too.
pub fn resolve_path(relative: &str) -> PathBuf {
    let path = PathBuf::from(relative);
    if path.exists() || path.is_absolute() {
        return path;
    }
    if let Some(shipped) = RESOURCE_DIR
        .get()
        .map(|dir| dir.join(relative))
        .filter(|p| p.exists())
    {
        return shipped;
    }
    #[cfg(debug_assertions)]
    return PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    #[cfg(not(debug_assertions))]
    path
}
