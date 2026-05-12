use serde_json::{json, Value};
use std::path::Path;
use tracing::warn;

/// Tauri command — scans external asset directories for custom furniture and sprites.
///
/// For each directory in /c/Dev, looks for:
///   - furniture/{ID}/manifest.json   -> catalog entries
///   - char_N.png (N=0..9)            -> custom character sprites
///
/// Returns { catalog: [...], sprites: {}, characters: [...] }
#[tauri::command]
pub fn scan_external_assets(dirs: Vec<String>) -> Value {
    let mut catalog: Vec<Value> = Vec::new();
    let mut sprites: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut characters: Vec<Value> = Vec::new();

    for dir_str in &dirs {
        let dir = Path::new(dir_str);
        if !dir.is_dir() {
            warn!("scan_external_assets: directory not found: {dir_str}");
            continue;
        }

        // Scan furniture/{ID}/manifest.json
        let furniture_dir = dir.join("furniture");
        if furniture_dir.is_dir() {
            match std::fs::read_dir(&furniture_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let manifest_path = entry.path().join("manifest.json");
                        if manifest_path.is_file() {
                            match std::fs::read_to_string(&manifest_path) {
                                Ok(content) => match serde_json::from_str::<Value>(&content) {
                                    Ok(manifest) => catalog.push(manifest),
                                    Err(e) => warn!(
                                        "scan_external_assets: corrupt manifest {}: {e}",
                                        manifest_path.display()
                                    ),
                                },
                                Err(e) => warn!(
                                    "scan_external_assets: cannot read {}: {e}",
                                    manifest_path.display()
                                ),
                            }
                        }
                    }
                }
                Err(e) => warn!(
                    "scan_external_assets: cannot read furniture dir {}: {e}",
                    furniture_dir.display()
                ),
            }
        }

        // Scan char_N.png (N=0..9) for custom character sprites.
        for n in 0..=9 {
            let sprite_name = format!("char_{n}.png");
            let sprite_path = dir.join(&sprite_name);
            if sprite_path.is_file() {
                let key = format!("char_{n}");
                sprites.insert(key.clone(), json!(sprite_path.to_string_lossy()));
                characters.push(json!({ "id": key, "path": sprite_path.to_string_lossy() }));
            }
        }
    }

    json!({
        "catalog": catalog,
        "sprites": sprites,
        "characters": characters,
    })
}
