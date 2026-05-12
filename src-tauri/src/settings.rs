use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;
use tracing::warn;

const STORE_FILE: &str = "settings.json";
const SETTINGS_KEY: &str = "settings";

/// Persisted application settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_watch_all")]
    pub watch_all: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub always_on_top: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            watch_all: default_watch_all(),
            theme: default_theme(),
            always_on_top: false,
        }
    }
}

fn default_watch_all() -> bool {
    false
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
