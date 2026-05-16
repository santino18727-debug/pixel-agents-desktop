use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tracing::{debug, warn};

const SESSION_MAP_FILE: &str = "session-map.json";

/// Global serializer for `save()` calls. Without this, two concurrent writers
/// can race on tmp-file creation / rename on Windows and produce a truncated
/// or missing destination file.
fn save_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn session_map_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    // On Windows, `dirs::home_dir()` can resolve to a OneDrive-redirected
    // path (e.g. `C:\Users\X\OneDrive`). Cloud sync may race with our atomic
    // rename and produce phantom `.tmp` files or truncated session-map.json.
    if home.to_string_lossy().contains("OneDrive") {
        warn!(
            "Home directory is inside OneDrive ({}). \
             session-map.json may conflict with cloud sync. \
             Consider configuring %USERPROFILE% outside OneDrive.",
            home.display()
        );
    }
    Some(home.join(".pixel-agents").join(SESSION_MAP_FILE))
}

/// Loads the persisted session_id → agent_id mapping from disk.
/// Returns an empty map if the file is absent or unreadable.
pub fn load() -> HashMap<String, usize> {
    let path = match session_map_path() {
        Some(p) => p,
        None => return HashMap::new(),
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persists the session_id → agent_id mapping to disk.
///
/// Atomicity: writes to `<file>.tmp` then renames over the destination so a
/// crash or concurrent reader never sees a half-written file. A global mutex
/// serializes concurrent writers (Windows can't rename onto an open handle).
pub fn save(map: &HashMap<String, usize>) {
    let path = match session_map_path() {
        Some(p) => p,
        None => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = match serde_json::to_string_pretty(map) {
        Ok(s) => s,
        Err(e) => {
            warn!("session_map: serialize failed: {e}");
            return;
        }
    };

    let _guard = save_lock().lock().unwrap_or_else(|e| e.into_inner());

    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &json) {
        warn!("session_map: write tmp failed: {e}");
        return;
    }
    // Windows: rename will fail if the destination exists in some edge cases
    // on older filesystems, but on NTFS this is atomic. Fall back to a remove
    // + rename if needed.
    match std::fs::rename(&tmp, &path) {
        Ok(()) => debug!("session_map: saved {} entries atomically", map.len()),
        Err(e) => {
            warn!("session_map: rename failed ({e}); attempting direct write");
            let _ = std::fs::remove_file(&path);
            if let Err(e2) = std::fs::rename(&tmp, &path) {
                warn!("session_map: fallback rename also failed: {e2}");
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

/// Removes expired session IDs from the persisted map.
/// Called by the expiry monitor after emitting agentClosed so that the
/// session-map.json file does not accumulate stale entries indefinitely.
pub fn remove_expired(expired_ids: &[String]) {
    if expired_ids.is_empty() {
        return;
    }
    let mut map = load();
    let before = map.len();
    for id in expired_ids {
        map.remove(id);
    }
    if map.len() != before {
        save(&map);
    }
}
