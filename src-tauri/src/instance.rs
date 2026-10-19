//! Behavior for a second application launch. The Tauri single-instance plugin
//! owns the process-wide lease and calls this function in the original process.

use tauri::Manager;

pub fn focus_existing(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        log::info!("second launch detected while using browser fallback mode");
        return;
    };

    if let Err(error) = window.unminimize() {
        log::warn!("could not restore the existing window: {error}");
    }
    if let Err(error) = window.show() {
        log::warn!("could not show the existing window: {error}");
    }
    if let Err(error) = window.set_focus() {
        log::warn!("could not focus the existing window: {error}");
    }
}
