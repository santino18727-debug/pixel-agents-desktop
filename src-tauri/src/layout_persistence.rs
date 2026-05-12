use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tracing::warn;

fn layout_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".pixel-agents").join("layout.json"))
}

/// Tauri command — persists the current layout to ~/.pixel-agents/layout.json.
#[tauri::command]
pub fn save_layout(layout: Value) -> Result<(), String> {
    let path = layout_path().ok_or("Cannot resolve home directory")?;

    // Create ~/.pixel-agents/ if it does not exist yet.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let json = serde_json::to_string_pretty(&layout)
        .map_err(|e| format!("Failed to serialize layout: {e}"))?;

    fs::write(&path, json).map_err(|e| format!("Failed to write layout: {e}"))?;

    Ok(())
}

/// Tauri command — loads the persisted layout from ~/.pixel-agents/layout.json.
/// Returns None if the file is absent or corrupt.
#[tauri::command]
pub fn load_layout() -> Option<Value> {
    let path = layout_path()?;

    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(&path)
        .map_err(|e| {
            warn!("Failed to read layout file: {e}");
        })
        .ok()?;

    serde_json::from_str(&content)
        .map_err(|e| {
            warn!("Layout file is corrupt, ignoring: {e}");
        })
        .ok()
}
