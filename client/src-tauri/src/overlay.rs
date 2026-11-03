//! Hint overlay window: Anarlog-style floating bar in the top-right corner that shows live hints
//! and the transcript during a call. It is clickable (stop, expand/collapse, drag by the hover
//! handle). Global shortcuts show or hide it (Ctrl+Shift+H), toggle click-through
//! (Ctrl+Shift+L) for when it sits over something that needs the mouse, and ask the coach
//! "what should I say?" (Ctrl+Shift+Space).

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow};
use tauri_plugin_global_shortcut::{Code, Modifiers, ShortcutState};

pub const LABEL: &str = "overlay";
/// Gap from the screen edges, in logical pixels.
const MARGIN: f64 = 24.0;

/// Window sizes in logical pixels. Expanded matches Anarlog's panel; compact is the bar plus
/// room for the latest hint (see `ui/src/components/overlay/layout.ts`).
const WIDTH: f64 = 360.0;
const EXPANDED_HEIGHT: f64 = 430.0;
const COMPACT_HEIGHT: f64 = 185.0;
/// Corner radius of the overlay window, in logical pixels; the page's corners match it.
const CORNER_RADIUS: f64 = 18.0;

pub const SHORTCUT_SHOW_HIDE: &str = "ctrl+shift+h";
pub const SHORTCUT_CLICK_THROUGH: &str = "ctrl+shift+l";
/// "What should I say?": a hint now, during a call.
pub const SHORTCUT_ASK: &str = "ctrl+shift+space";

#[derive(Default)]
pub struct OverlayState {
    click_through: AtomicBool,
    /// Placed in the top-right corner once; after that, keep wherever the user dragged it.
    placed: AtomicBool,
}

#[derive(Clone, Serialize)]
struct ModePayload {
    click_through: bool,
}

/// Global shortcuts for the overlay. Registration can fail if another app already owns a
/// shortcut, so the caller treats an error as non-fatal.
pub fn shortcuts_plugin(
) -> Result<tauri::plugin::TauriPlugin<tauri::Wry>, tauri_plugin_global_shortcut::Error> {
    Ok(tauri_plugin_global_shortcut::Builder::new()
        .with_shortcuts([SHORTCUT_SHOW_HIDE, SHORTCUT_CLICK_THROUGH, SHORTCUT_ASK])?
        .with_handler(|app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let ctrl_shift = Modifiers::CONTROL | Modifiers::SHIFT;
            if shortcut.matches(ctrl_shift, Code::KeyH) {
                toggle_visible(app);
            } else if shortcut.matches(ctrl_shift, Code::KeyL) {
                toggle_click_through(app);
            } else if shortcut.matches(ctrl_shift, Code::Space) {
                // Outside a call there's nothing to ask about; the shortcut does nothing.
                let _ = crate::capture::ask(app, None);
            }
        })
        .build())
}

/// Frosted glass behind the overlay: Windows blurs whatever is under the window, and the page
/// adds a light or dark tint on top. Each effect lags while dragging on some builds, so pick the
/// one that doesn't: Acrylic lags on Windows 10 and 11 build 22000, Blur on 11 build 22621+.
pub fn apply_glass(app: &AppHandle) {
    use tauri::window::{Effect, EffectsBuilder};
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    #[cfg(target_os = "macos")]
    let effects = {
        EffectsBuilder::new()
            .effect(Effect::HudWindow)
            .state(tauri::window::EffectState::Active)
            .radius(CORNER_RADIUS)
            .build()
    };
    #[cfg(not(target_os = "macos"))]
    let effects = {
        let build = crate::call_detect::windows_build().unwrap_or(0);
        let effect = if build >= 22621 {
            Effect::Acrylic
        } else {
            Effect::Blur
        };
        EffectsBuilder::new()
            .effect(effect)
            .color(tauri::window::Color(18, 18, 22, 60))
            .build()
    };
    if let Err(e) = window.set_effects(effects) {
        eprintln!("Overlay glass effect unavailable: {e}");
    }
    round_corners(&window);
}

/// Clips the window, blur included, to a rounded rectangle. Windows 10 has no rounded-corner
/// setting for windows, and Windows 11's is fixed at 8px, so this uses a window region on both.
/// Reapplied whenever the window changes size.
#[cfg(windows)]
fn round_corners(window: &WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
    let (Ok(handle), Ok(size), Ok(scale)) =
        (window.hwnd(), window.outer_size(), window.scale_factor())
    else {
        return;
    };
    let diameter = (CORNER_RADIUS * 2.0 * scale).round() as i32;
    // SAFETY: the handle belongs to this live window; Windows owns the region once it is set.
    unsafe {
        let region = CreateRoundRectRgn(
            0,
            0,
            size.width as i32 + 1,
            size.height as i32 + 1,
            diameter,
            diameter,
        );
        SetWindowRgn(HWND(handle.0 as _), Some(region), true);
    }
}

#[cfg(not(windows))]
fn round_corners(_window: &WebviewWindow) {}

pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let state = app.state::<OverlayState>();
    if !state.placed.swap(true, Ordering::Relaxed) {
        place_top_right(&window);
    }
    let _ = window.set_ignore_cursor_events(state.click_through.load(Ordering::Relaxed));
    round_corners(&window);
    let _ = window.show();
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
}

pub fn toggle_visible(app: &AppHandle) {
    let visible = app
        .get_webview_window(LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide(app);
    } else {
        show(app);
    }
}

pub fn toggle_click_through(app: &AppHandle) {
    let state = app.state::<OverlayState>();
    let click_through = !state.click_through.load(Ordering::Relaxed);
    state.click_through.store(click_through, Ordering::Relaxed);
    let _ = app.emit_to(LABEL, "overlay-mode", ModePayload { click_through });
    show(app);
}

/// Resizes the overlay between the compact bar and the expanded panel, keeping its top-right
/// corner where it is.
#[tauri::command]
pub fn overlay_set_expanded(app: AppHandle, expanded: bool) -> Result<(), String> {
    let window = app
        .get_webview_window(LABEL)
        .ok_or("Overlay window not found")?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let old_width = window.outer_size().map_err(|e| e.to_string())?.width as i32;

    let height = if expanded {
        EXPANDED_HEIGHT
    } else {
        COMPACT_HEIGHT
    };
    window
        .set_size(LogicalSize::new(WIDTH, height))
        .map_err(|e| e.to_string())?;
    round_corners(&window);
    let new_width = (WIDTH * scale).round() as i32;
    let _ = window.set_position(PhysicalPosition::new(
        position.x + old_width - new_width,
        position.y,
    ));
    Ok(())
}

fn place_top_right(window: &WebviewWindow) {
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let area = monitor.work_area();
    let margin = (MARGIN * monitor.scale_factor()).round() as i32;
    let x = area.position.x + area.size.width as i32 - size.width as i32 - margin;
    let y = area.position.y + margin;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}
