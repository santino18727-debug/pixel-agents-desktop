use std::collections::HashMap;
use std::path::PathBuf;

const SESSION_MAP_FILE: &str = "session-map.json";

fn session_map_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".pixel-agents").join(SESSION_MAP_FILE))
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
pub fn save(map: &HashMap<String, usize>) {
    let path = match session_map_path() {
        Some(p) => p,
        None => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(&path, json);
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
