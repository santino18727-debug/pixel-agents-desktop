use serde::{Deserialize, Serialize};
use tauri::AppHandle;
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound_enabled: false,
            watch_all_sessions: default_watch_all_sessions(),
            always_show_labels: false,
            always_on_top: false,
            theme: default_theme(),
        }
    }
}

fn default_watch_all_sessions() -> bool {
    true
}

fn default_theme() -> String {
    "dark".to_owned()
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

/// Tauri command — persists updated settings.
#[tauri::command]
pub fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let store = app
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to open store: {e}"))?;

    let value = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    store.set(SETTINGS_KEY, value);

    if let Err(e) = store.save() {
        warn!("Failed to flush settings to disk: {e}");
    }

    Ok(())
}
