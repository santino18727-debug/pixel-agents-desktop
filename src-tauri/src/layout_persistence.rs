use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::new_debouncer;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
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
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
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

/// Watches ~/.pixel-agents/layout.json for external edits (e.g. manual user edits).
/// On change, reads the file and emits "layout-changed" with the parsed Value,
/// or null if the file is absent or corrupt.
pub fn start_layout_watcher(app: AppHandle) {
    let Some(path) = layout_path() else {
        warn!("start_layout_watcher: cannot resolve layout path, watcher not started");
        return;
    };

    // Ensure parent directory exists so the watcher can be registered even if
    // layout.json hasn't been written yet.
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            warn!("start_layout_watcher: failed to create directory: {e}");
            return;
        }
    }

    let watch_dir = match path.parent() {
        Some(p) => p.to_path_buf(),
        None => {
            warn!("start_layout_watcher: layout path has no parent");
            return;
        }
    };

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();

        let mut debouncer = match new_debouncer(Duration::from_millis(500), None, tx) {
            Ok(d) => d,
            Err(e) => {
                warn!("start_layout_watcher: failed to create debouncer: {e}");
                return;
            }
        };

        if let Err(e) = debouncer
            .watcher()
            .watch(&watch_dir, RecursiveMode::NonRecursive)
        {
            warn!("start_layout_watcher: failed to watch directory: {e}");
            return;
        }

        for batch in rx {
            let events = match batch {
                Ok(evs) => evs,
                Err(errs) => {
                    for e in errs {
                        warn!("layout_watcher error: {e}");
                    }
                    continue;
                }
            };

            // Only react to events touching layout.json specifically.
            let relevant = events.iter().any(|de| {
                de.event
                    .paths
                    .iter()
                    .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("layout.json"))
            });

            if !relevant {
                continue;
            }

            let layout_value: Value = match fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or(Value::Null),
                Err(_) => Value::Null,
            };

            if let Err(e) = app.emit("layout-changed", &layout_value) {
                warn!("start_layout_watcher: failed to emit layout-changed: {e}");
            }
        }
    });
}
