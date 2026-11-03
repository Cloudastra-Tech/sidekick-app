//! Keeping installed copies up to date.
//!
//! The app asks the workspace it is signed in to whether a newer build exists, so installers are
//! never on a public URL. That alone wouldn't be safe enough — anyone who could answer for the
//! server could hand over any file — so Tauri checks each build's signature against the release
//! public key baked into the app, and refuses anything else.
//!
//! An update is never installed during a call: the check simply reports what's available, and the
//! install happens when the user agrees.

use crate::backend;
use serde::Serialize;
use serde_json::json;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The update found by the last check, waiting for the user to say yes.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Serialize)]
pub struct UpdateCheck {
    pub available: bool,
    pub current_version: String,
    pub version: Option<String>,
    pub notes: Option<String>,
}

/// Asks the workspace whether there's a newer build. `None` version means this one is current.
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<UpdateCheck, String> {
    let current_version = app.package_info().version.to_string();
    let (url, token) =
        backend::llm_route().ok_or("Sign in to your workspace to check for updates")?;
    let endpoint = format!("{url}/updates/latest?target={{{{target}}}}")
        .parse()
        .map_err(|e| format!("That workspace address isn't usable for updates: {e}"))?;

    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| e.to_string())?
        .header("Authorization", format!("Bearer {token}"))
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;

    match updater.check().await {
        Ok(Some(update)) => {
            let found = UpdateCheck {
                available: true,
                current_version,
                version: Some(update.version.clone()),
                notes: update.body.clone(),
            };
            *pending.0.lock().expect("update lock poisoned") = Some(update);
            Ok(found)
        }
        Ok(None) => Ok(UpdateCheck {
            available: false,
            current_version,
            version: None,
            notes: None,
        }),
        Err(e) => Err(format!("Couldn't check for updates: {e}")),
    }
}

/// Downloads and installs the update found by the last check, then restarts the app.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    let update = pending
        .0
        .lock()
        .expect("update lock poisoned")
        .take()
        .ok_or("Check for an update first")?;
    if app.state::<crate::capture::CaptureState>().is_running() {
        return Err("A call is running; end it before updating".into());
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("The update didn't install: {e}"))?;
    app.restart();
}

/// The running version, for the About dialog and bug reports.
#[tauri::command]
pub fn app_version(app: AppHandle) -> serde_json::Value {
    json!({ "version": app.package_info().version.to_string() })
}
