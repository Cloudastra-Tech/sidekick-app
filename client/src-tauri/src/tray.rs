use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Sidekick", true, None::<&str>)?;
    let overlay = MenuItem::with_id(
        app,
        "overlay",
        "Show or hide hints (Ctrl+Shift+H)",
        true,
        None::<&str>,
    )?;
    let move_overlay = MenuItem::with_id(
        app,
        "move-overlay",
        "Click-through hints (Ctrl+Shift+L)",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &overlay, &move_overlay, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Sidekick — Live Call Coach")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "overlay" => crate::overlay::toggle_visible(app),
            "move-overlay" => crate::overlay::toggle_click_through(app),
            "quit" => quit_after_saving(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Longest Quit waits for a call's summary. The transcript is saved within seconds of the
/// call ending, so only the summary is at risk past this.
const QUIT_WAIT: Duration = Duration::from_secs(90);
const QUIT_POLL: Duration = Duration::from_millis(250);
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Quits, first ending a running call and letting its transcript and summary be saved.
/// Choosing Quit again while waiting quits immediately.
fn quit_after_saving(app: &AppHandle) {
    if QUITTING.swap(true, Ordering::Relaxed) {
        app.exit(0);
        return;
    }
    crate::capture::end_call(app, None, None);
    let summaries = app.state::<crate::coach::SummaryState>();
    if summaries.is_settled() {
        app.exit(0);
        return;
    }

    for label in ["main", crate::overlay::LABEL] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some("Sidekick: saving the call summary, then quitting"));
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        while started.elapsed() < QUIT_WAIT
            && !app.state::<crate::coach::SummaryState>().is_settled()
        {
            tokio::time::sleep(QUIT_POLL).await;
        }
        app.exit(0);
    });
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
