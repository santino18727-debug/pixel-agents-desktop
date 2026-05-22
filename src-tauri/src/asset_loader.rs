use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Maximum manifest size (1 MiB). Anything larger is rejected to avoid OOM.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// Maximum number of declared characters in a manifest. Matches the on-disk scan bound.
const MAX_MANIFEST_CHARACTERS: usize = 64;
/// Maximum number of character_N.png files scanned on disk.
const MAX_CHARACTERS: usize = 64;

/// Metadata describing a discovered sprite pack on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpritePackInfo {
    /// Folder name on disk — stable unique identifier used to load the pack.
    pub folder_name: String,
    /// Display name from manifest.json (may collide between packs).
    pub display_name: String,
    /// Legacy `name` field kept for backward compatibility with the frontend.
    /// Equals `display_name`. Persist `folder_name` if you need a stable key.
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

/// Reads the first 8 bytes of a file and checks the PNG magic signature.
/// Rejects anything that isn't a real PNG (SVG, HTML, JS, EXE renamed to .png, etc.).
fn is_valid_png(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut header = [0u8; 8];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    header == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
}

/// Validates a manifest and returns SpritePackInfo if all required fields are present
/// and at least one character_N.png exists on disk.
///
/// `folder_name` is the on-disk directory name (already validated against traversal
/// by the caller via `read_dir`). It becomes the stable unique identifier — pack
/// loading uses the folder name, not the manifest's `name` field, to defeat
/// pack-name squatting where two packs declare the same `name`.
fn validate_pack(dir: &Path, folder_name: &str) -> Option<SpritePackInfo> {
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

    // Cap manifest size before reading — a 10 GiB manifest would OOM the app.
    let metadata = std::fs::metadata(&manifest_path).ok()?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        warn!(
            "validate_pack: manifest too large ({} bytes, max {}): {}",
            metadata.len(),
            MAX_MANIFEST_BYTES,
            manifest_path.display()
        );
        return None;
    }

    let content = std::fs::read_to_string(&manifest_path).ok()?;
    let mut manifest: SpritePackManifest = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            warn!(
                "scan_sprite_packs: corrupt manifest {}: {e}",
                manifest_path.display()
            );
            return None;
        }
    };

    // Cap declared characters to avoid a manifest claiming 1M virtual characters.
    if manifest.characters.len() > MAX_MANIFEST_CHARACTERS {
        warn!(
            "validate_pack: manifest declares {} characters, truncating to {}: {}",
            manifest.characters.len(),
            MAX_MANIFEST_CHARACTERS,
            manifest_path.display()
        );
        manifest.characters.truncate(MAX_MANIFEST_CHARACTERS);
    }

    let display_name = manifest.name?.trim().to_owned();
    if display_name.is_empty() {
        return None;
    }
    let version = manifest.version?.trim().to_owned();
    if version.is_empty() {
        return None;
    }

    // Count character_N.png files on disk (N=0..63 to be generous).
    // Reject any entries that resolve outside the pack dir (symlink traversal)
    // or that are not real PNGs (magic-byte mismatch).
    let mut count = 0usize;
    for n in 0..MAX_CHARACTERS {
        let p = dir.join(format!("character_{n}.png"));
        if p.is_file() {
            if !path_is_within(dir, &p) {
                warn!(
                    "validate_pack: pack {}: rejected path traversal {:?}",
                    dir.display(),
                    p
                );
                continue;
            }
            if !is_valid_png(&p) {
                warn!(
                    "validate_pack: pack {}: rejected non-PNG file {:?}",
                    dir.display(),
                    p
                );
                continue;
            }
            count += 1;
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
        folder_name: folder_name.to_owned(),
        display_name: display_name.clone(),
        name: display_name,
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
                    let folder_name = match entry.file_name().into_string() {
                        Ok(s) => s,
                        Err(_) => {
                            warn!(
                                "scan_sprite_packs: skipping non-UTF8 folder name in {}",
                                root.display()
                            );
                            continue;
                        }
                    };
                    if let Some(info) = validate_pack(&path, &folder_name) {
                        out.push(info);
                    }
                }
            }
        }
        Err(e) => warn!("scan_sprite_packs: cannot read {}: {e}", root.display()),
    }

    // Warn on display_name collisions so users can spot pack-name squatting.
    // We keep all packs (folder_name is the unique key) but log loudly.
    let mut seen_names = std::collections::HashSet::new();
    for pack in &out {
        if !seen_names.insert(pack.display_name.clone()) {
            warn!(
                "scan_sprite_packs: display name collision '{}' (folder '{}') — \
                 packs are uniquely identified by folder name, but the UI may be confusing",
                pack.display_name, pack.folder_name
            );
        }
    }

    out
}

/// Loads a sprite pack by **folder name** and returns metadata + list of character PNG paths
/// the frontend can consume. Returns Err if the pack is missing or invalid.
///
/// Identity is the on-disk folder name (already validated against traversal) — not the
/// `name` field from manifest.json — to defeat pack-name squatting where a malicious
/// pack `aaaa-evil/manifest.json` declares `name: "Cute Cats"` and shadows the real
/// "Cute Cats" pack purely thanks to filesystem enumeration order.
///
/// NOTE: The built-in `characters.json` ships decoded pixel arrays, not PNG paths.
/// Reconstructing those arrays from arbitrary PNGs is out of scope for this MVP, so
/// this command surfaces the raw PNG paths for the frontend to handle (or ignore
/// gracefully with a TODO).
pub fn load_sprite_pack_impl(folder_name: &str) -> Result<Value, String> {
    let root = sprite_packs_root().ok_or_else(|| "Cannot resolve home directory".to_owned())?;
    if !root.is_dir() {
        return Err(format!(
            "Sprite packs directory not found: {}",
            root.display()
        ));
    }

    // Reject path-traversal attempts in the folder name (e.g. "../../etc").
    // A simple component check is enough: folder_name must be a single, non-empty,
    // non-traversal segment.
    if folder_name.is_empty()
        || folder_name.contains('/')
        || folder_name.contains('\\')
        || folder_name == "."
        || folder_name == ".."
    {
        return Err(format!("Invalid sprite pack folder name: {folder_name}"));
    }

    let path = root.join(folder_name);
    if !path.is_dir() {
        return Err(format!("Sprite pack not found: {folder_name}"));
    }
    if !path_is_within(&root, &path) {
        return Err(format!(
            "Sprite pack rejected (path traversal): {folder_name}"
        ));
    }

    let info = validate_pack(&path, folder_name)
        .ok_or_else(|| format!("Sprite pack invalid: {folder_name}"))?;

    let mut characters = Vec::new();
    for n in 0..MAX_CHARACTERS {
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
            if !is_valid_png(&p) {
                warn!(
                    "load_sprite_pack: pack {}: rejected non-PNG file {:?}",
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
    Ok(json!({
        "info": info,
        "characters": characters,
    }))
}

/// Tauri command — returns the list of valid sprite packs in ~/.pixel-agents/sprites/.
#[tauri::command]
pub fn list_sprite_packs() -> Vec<SpritePackInfo> {
    scan_sprite_packs()
}

/// Tauri command — loads a sprite pack by **folder name** (returns info + character PNG paths).
///
/// The frontend should pass the `folderName` from `SpritePackInfo`, NOT the `name` field
/// (which is a non-unique display string vulnerable to pack-name squatting).
#[tauri::command]
pub fn load_sprite_pack(folder_name: String) -> Result<Value, String> {
    load_sprite_pack_impl(&folder_name)
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
                            // Cap manifest size to avoid OOM from a 10 GiB file.
                            match std::fs::metadata(&manifest_path) {
                                Ok(meta) if meta.len() > MAX_MANIFEST_BYTES => {
                                    warn!(
                                        "scan_external_assets: manifest too large ({} bytes): {}",
                                        meta.len(),
                                        manifest_path.display()
                                    );
                                    continue;
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    warn!(
                                        "scan_external_assets: cannot stat {}: {e}",
                                        manifest_path.display()
                                    );
                                    continue;
                                }
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
                if !is_valid_png(&sprite_path) {
                    warn!(
                        "scan_external_assets: rejected non-PNG sprite {:?}",
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
