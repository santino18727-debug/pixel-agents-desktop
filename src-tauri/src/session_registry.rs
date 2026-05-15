use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::Result;

/// Metadata for a discovered Claude Code session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub project_dir: String,
    /// Human-readable project folder name, decoded from Claude Code encoding.
    /// Claude Code encodes path separators as  in the directory name stored
    /// under ~/.claude/projects/ (e.g.  -> ).
    pub folder_name: String,
    pub is_subagent: bool,
    /// For sub-agent sessions, the session_id of the parent (derived from path).
    /// None for top-level sessions.
    pub parent_session_id: Option<String>,
    pub jsonl_path: String,
    /// Unix timestamp (seconds) of last modification, or 0 if unavailable.
    pub modified_secs: u64,
    /// Resolved frontend agent ID from the persisted session map.
    /// None if the session has not been seen by the file watcher yet.
    pub agent_id: Option<usize>,
}

/// Shared, thread-safe session registry.
pub type SessionRegistry = Arc<Mutex<Vec<SessionMeta>>>;

pub fn new_registry() -> SessionRegistry {
    Arc::new(Mutex::new(Vec::new()))
}

/// Decode a Claude Code project directory name to a human-readable folder name.
///
/// Claude Code stores sessions under ~/.claude/projects/<encoded-path>/ where
/// the path separators (/ and \) are replaced by . For example:
///     ->  
///     ->  
///
/// Strategy: split on  (the separator), take the last non-empty segment.
/// Trade-off: folder names that legitimately contain  will be truncated.
pub fn decode_folder_name(encoded: &str) -> String {
    // Split on the double-dash separator used by Claude Code for path components
    let parts: Vec<&str> = encoded.split("--").collect();
    // The last part is the actual folder name (rightmost path component)
    parts
        .into_iter()
        .rev()
        .find(|s| !s.is_empty())
        .unwrap_or(encoded)
        .to_owned()
}

/// Walk  and collect all  session files.
///
/// Sub-agent sessions live under .
/// Returns an error only if the home directory cannot be resolved.
pub fn scan_projects() -> Result<Vec<SessionMeta>> {
    let home = dirs::home_dir().ok_or_else(|| {
        crate::error::AppError::Settings("Cannot resolve home directory".to_owned())
    })?;

    let projects_root = home.join(".claude").join("projects");

    if !projects_root.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in WalkDir::new(&projects_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        let Some(meta) = build_session_meta(&projects_root, path) else {
            continue;
        };

        sessions.push(meta);
    }

    // Sort by most recently modified first
    sessions.sort_by(|a, b| b.modified_secs.cmp(&a.modified_secs));

    Ok(sessions)
}

fn build_session_meta(projects_root: &PathBuf, jsonl_path: &std::path::Path) -> Option<SessionMeta> {
    let session_id = jsonl_path.file_stem()?.to_str()?.to_owned();

    // Determine if this is a sub-agent by checking whether  appears
    // in the path components between the projects root and the file.
    let rel = jsonl_path.strip_prefix(projects_root).ok()?;
    let components: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    // components[0] = project dirname, components[1] = file or "subagents", ...
    let is_subagent = components.contains(&"subagents");

    // For sub-agents, derive parent session_id from path:
    // path layout: <project>/<parent_uuid>/subagents/<child_uuid>.jsonl
    // so the parent uuid is the component just before "subagents".
    let parent_session_id: Option<String> = if is_subagent {
        components
            .iter()
            .position(|&c| c == "subagents")
            .and_then(|idx| if idx > 0 { components.get(idx - 1).copied() } else { None })
            .map(str::to_owned)
    } else {
        None
    };

    let encoded_dir_name = components.first()?.to_string();

    let project_dir = projects_root
        .join(&encoded_dir_name)
        .to_string_lossy()
        .into_owned();

    // Decode the Claude Code encoded directory name to a human-readable folder name
    let folder_name = decode_folder_name(&encoded_dir_name);

    // Get file modification time as Unix seconds
    let modified_secs = std::fs::metadata(jsonl_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Some(SessionMeta {
        session_id,
        project_dir,
        folder_name,
        is_subagent,
        parent_session_id,
        jsonl_path: jsonl_path.to_string_lossy().into_owned(),
        modified_secs,
        agent_id: None,
    })
}

/// Tauri command -- returns sessions modified within the last  hours.
/// Defaults to 24h. Returns at most  sessions (default 20).
/// Populates agent_id from the shared session-agent map so the frontend
/// uses the same IDs as the file watcher (avoids idle-forever mismatch).
#[tauri::command]
pub fn list_sessions(
    registry: tauri::State<'_, SessionRegistry>,
    agent_map: tauri::State<
        '_,
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    >,
    max_age_hours: Option<u64>,
    limit: Option<usize>,
) -> Vec<SessionMeta> {
    let max_age = max_age_hours.unwrap_or(24);
    let limit = limit.unwrap_or(20);

    let cutoff = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(max_age * 3600))
        .unwrap_or(0);

    let map = agent_map.lock().unwrap_or_else(|e| e.into_inner());

    registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|s| s.modified_secs >= cutoff)
        .take(limit)
        .map(|s| {
            let mut s = s.clone();
            s.agent_id = map.get(&s.session_id).copied();
            s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_folder_name_windows_single_level() {
        // Windows encoding: C:\Dev\pixel-agents-desktop -> C--Dev-pixel-agents-desktop
        // Claude Code uses -- for :\ and - for subsequent \
        // Best output: strip drive prefix, show path remainder.
        assert_eq!(decode_folder_name("C--Dev-pixel-agents-desktop"), "Dev-pixel-agents-desktop");
    }

    #[test]
    fn decode_folder_name_deep_path_double_dash() {
        // Some systems encode all \ as --; we still take the last segment.
        assert_eq!(decode_folder_name("C--Dev--projects--my-app"), "my-app");
    }

    #[test]
    fn decode_folder_name_single_segment() {
        // No double-dash: return as-is
        assert_eq!(decode_folder_name("myapp"), "myapp");
    }

    #[test]
    fn decode_folder_name_station_math() {
        // C:\Dev\station-math-app -> C--Dev-station-math-app
        assert_eq!(decode_folder_name("C--Dev-station-math-app"), "Dev-station-math-app");
    }
}
