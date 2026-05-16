use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tracing::warn;

/// Metadata describing a discovered sprite pack on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpritePackInfo {
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub path: String,
    pub character_count: usize,
}

/// Raw manifest as stored on disk (loose schema).
#[derive(Debug, Deserialize)]
struct SpritePackManifest {
    name: Option<String>,
    version: Option<String>,
    author: Option<String>,
    description: Option<String>,
    #[serde(default)]
    characters: Vec<Value>,
}

/// Returns ~/.pixel-agents/sprites/ as a PathBuf, or None if home dir cannot be resolved.
fn sprite_packs_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".pixel-agents").join("sprites"))
}

/// Returns true if `child` resolves (after symlink-following canonicalization)
/// to a path inside `root`. Used to defend against malicious sprite packs that
/// embed symlinks pointing at arbitrary files like ~/.ssh/id_rsa, which a
/// frontend canvas pixel-inspection could otherwise exfiltrate.
fn path_is_within(root: &Path, child: &Path) -> bool {
    match (root.canonicalize(), child.canonicalize()) {
        (Ok(r), Ok(c)) => c.starts_with(&r),
        _ => false,
    }
}

/// Validates a manifest and returns SpritePackInfo if all required fields are present
/// and at least one character_N.png exists on disk.
fn validate_pack(dir: &Path) -> Option<SpritePackInfo> {
    let manifest_path = dir.join("manifest.json");
    if !manifest_path.is_file() {
        return None;
    }
    // Defense in depth: ensure the manifest itself is not a symlink pointing
    // outside the pack directory.
    if !path_is_within(dir, &manifest_path) {
        warn!(
            "validate_pack: rejected manifest path traversal in {}",
            dir.display()
        );
        return None;
    }
    let content = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: SpritePackManifest = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            warn!(
                "scan_sprite_packs: corrupt manifest {}: {e}",
                manifest_path.display()
            );
            return None;
        }
    };
    let name = manifest.name?.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let version = manifest.version?.trim().to_owned();
    if version.is_empty() {
        return None;
    }

    // Count character_N.png files on disk (N=0..63 to be generous).
    // Reject any entries that resolve outside the pack dir (symlink traversal).
    let mut count = 0usize;
    for n in 0..64 {
        let p = dir.join(format!("character_{n}.png"));
        if p.is_file() {
            if path_is_within(dir, &p) {
                count += 1;
            } else {
                warn!(
                    "validate_pack: pack {}: rejected path traversal {:?}",
                    dir.display(),
                    p
                );
            }
        }
    }
    if count == 0 && manifest.characters.is_empty() {
        warn!(
            "scan_sprite_packs: pack {} has no character_N.png and empty manifest characters",
            dir.display()
        );
        return None;
    }

    Some(SpritePackInfo {
        name,
        version,
        author: manifest.author,
        description: manifest.description,
        path: dir.to_string_lossy().into_owned(),
        character_count: count.max(manifest.characters.len()),
    })
}

/// Scans ~/.pixel-agents/sprites/ for sprite packs and returns the list of valid ones.
pub fn scan_sprite_packs() -> Vec<SpritePackInfo> {
    let Some(root) = sprite_packs_root() else {
        return Vec::new();
    };
    if !root.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    match std::fs::read_dir(&root) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(info) = validate_pack(&path) {
                        out.push(info);
                    }
                }
            }
        }
        Err(e) => warn!("scan_sprite_packs: cannot read {}: {e}", root.display()),
    }
    out
}

/// Loads a sprite pack by name and returns metadata + list of character PNG paths
/// the frontend can consume. Returns Err if the pack is missing or invalid.
///
/// NOTE: The built-in `characters.json` ships decoded pixel arrays, not PNG paths.
/// Reconstructing those arrays from arbitrary PNGs is out of scope for this MVP, so
/// this command surfaces the raw PNG paths for the frontend to handle (or ignore
/// gracefully with a TODO).
pub fn load_sprite_pack_impl(name: &str) -> Result<Value, String> {
    let root = sprite_packs_root().ok_or_else(|| "Cannot resolve home directory".to_owned())?;
    if !root.is_dir() {
        return Err(format!("Sprite packs directory not found: {}", root.display()));
    }

    // Find the pack whose manifest.json declares this name.
    let entries = std::fs::read_dir(&root).map_err(|e| format!("Cannot read sprite packs: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(info) = validate_pack(&path) {
            if info.name == name {
                let mut characters = Vec::new();
                for n in 0..64 {
                    let p = path.join(format!("character_{n}.png"));
                    if p.is_file() {
                        if !path_is_within(&path, &p) {
                            warn!(
                                "load_sprite_pack: pack {}: rejected path traversal {:?}",
                                path.display(),
                                p
                            );
                            continue;
                        }
                        characters.push(json!({
                            "id": format!("character_{n}"),
                            "path": p.to_string_lossy(),
                        }));
                    }
                }
                return Ok(json!({
                    "info": info,
                    "characters": characters,
                }));
            }
        }
    }
    Err(format!("Sprite pack not found: {name}"))
}

/// Tauri command — returns the list of valid sprite packs in ~/.pixel-agents/sprites/.
#[tauri::command]
pub fn list_sprite_packs() -> Vec<SpritePackInfo> {
    scan_sprite_packs()
}

/// Tauri command — loads a sprite pack by name (returns info + character PNG paths).
#[tauri::command]
pub fn load_sprite_pack(name: String) -> Result<Value, String> {
    load_sprite_pack_impl(&name)
}

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
                        let entry_path = entry.path();
                        let manifest_path = entry_path.join("manifest.json");
                        if manifest_path.is_file() {
                            if !path_is_within(&entry_path, &manifest_path) {
                                warn!(
                                    "scan_external_assets: rejected path traversal {:?}",
                                    manifest_path
                                );
                                continue;
                            }
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
                if !path_is_within(dir, &sprite_path) {
                    warn!(
                        "scan_external_assets: rejected sprite path traversal {:?}",
                        sprite_path
                    );
                    continue;
                }
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
