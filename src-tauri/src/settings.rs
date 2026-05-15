use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;
use tracing::warn;

const STORE_FILE: &str = "settings.json";
const SETTINGS_KEY: &str = "settings";

/// Persisted application settings.
/// All fields map to camelCase for the frontend protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Whether UI sounds are enabled (maps to soundEnabled on the frontend).
    #[serde(default)]
    pub sound_enabled: bool,

    /// When true the watcher tracks all sessions, not just the active one.
    /// Maps to watchAllSessions on the frontend.
    #[serde(default = "default_watch_all_sessions")]
    pub watch_all_sessions: bool,

    /// Always show agent name labels above characters.
    #[serde(default)]
    pub always_show_labels: bool,

    /// Keep the window floating above all other windows.
    #[serde(default)]
    pub always_on_top: bool,

    /// Visual theme ("dark" | "light").
    #[serde(default = "default_theme")]
    pub theme: String,

    /// List of external directories to scan for custom asset packs (furniture, sprites).
    #[serde(default)]
    pub external_asset_directories: Vec<String>,

    /// Maximum number of sessions to display (default 20, max 100).
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,

    /// Whether Claude hooks are enabled.
    #[serde(default = "default_hooks_enabled")]
    pub hooks_enabled: bool,

    /// Maximum context window (tokens) used to compute the token health bar ratio.
    /// Maps to `defaultContextWindowMax` on the frontend. Defaults to 200_000.
    #[serde(default = "default_context_window_max")]
    pub default_context_window_max: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound_enabled: false,
            watch_all_sessions: default_watch_all_sessions(),
            always_show_labels: false,
            always_on_top: false,
            theme: default_theme(),
            external_asset_directories: Vec::new(),
            max_sessions: default_max_sessions(),
            hooks_enabled: default_hooks_enabled(),
            default_context_window_max: default_context_window_max(),
        }
    }
}

fn default_context_window_max() -> u32 {
    200_000
}

fn default_watch_all_sessions() -> bool {
    true
}

fn default_theme() -> String {
    "dark".to_owned()
}

fn default_max_sessions() -> usize {
    20
}

fn default_hooks_enabled() -> bool {
    true
}

/// Tauri command — returns the current settings (or defaults if not yet stored).
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<Settings, String> {
    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to open store: {e}"))?;

    let settings = store
        .get(SETTINGS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    Ok(settings)
}

/// Tauri command — persists updated settings (partial merge).
/// Applies alwaysOnTop immediately to the main window when the setting changes.
#[tauri::command]
pub fn set_settings(app: AppHandle, settings: serde_json::Value) -> Result<(), String> {
    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to open store: {e}"))?;

    let mut current: serde_json::Value = store
        .get(SETTINGS_KEY)
        .unwrap_or_else(|| serde_json::to_value(Settings::default()).unwrap());

    if let (Some(obj), Some(patch)) = (current.as_object_mut(), settings.as_object()) {
        for (k, v) in patch {
            obj.insert(k.clone(), v.clone());
        }
    }

    let validated: Settings = serde_json::from_value(current.clone()).map_err(|e| e.to_string())?;
    store.set(SETTINGS_KEY, current);

    if let Err(e) = store.save() {
        warn!("Failed to flush settings to disk: {e}");
    }

    // Apply alwaysOnTop immediately so the user sees the effect without restart.
    if let Some(win) = app.get_webview_window("main") {
        if let Err(e) = win.set_always_on_top(validated.always_on_top) {
            warn!("Failed to apply always_on_top={}: {e}", validated.always_on_top);
        }
    }

    Ok(())
}
