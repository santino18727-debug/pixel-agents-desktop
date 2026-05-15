//! Native OS notifications for agent state changes.
//!
//! Emits notifications when an agent is waiting on the user or requires
//! permission, gated on the `notifications_enabled` setting and skipped
//! when the main window is already focused.

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_store::StoreExt;
use tracing::warn;

use crate::settings::Settings;

const STORE_FILE: &str = "settings.json";
const SETTINGS_KEY: &str = "settings";

fn notifications_enabled(app: &AppHandle) -> bool {
    let store = match app.store(STORE_FILE) {
        Ok(s) => s,
        Err(_) => return true, // default
    };
    let settings: Settings = store
        .get(SETTINGS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    settings.notifications_enabled
}

fn main_window_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false)
}

/// Send a native notification unless disabled or the main window already has focus.
pub fn notify_agent(app: &AppHandle, title: &str, body: &str) {
    if !notifications_enabled(app) {
        return;
    }
    if main_window_focused(app) {
        return;
    }
    if let Err(e) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
    {
        warn!("Failed to show notification: {e}");
    }
}

/// Convenience helpers used by the file watcher's timer threads.
pub fn notify_waiting(app: &AppHandle, agent_id: usize) {
    notify_agent(
        app,
        "Pixel Agents",
        &format!("Agent #{agent_id} is waiting"),
    );
}

pub fn notify_permission(app: &AppHandle, agent_id: usize) {
    notify_agent(
        app,
        "Pixel Agents",
        &format!("Agent #{agent_id} needs your attention"),
    );
}
