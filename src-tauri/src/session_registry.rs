use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::Result;

/// Metadata for a discovered Claude Code session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub project_dir: String,
    pub is_subagent: bool,
    pub jsonl_path: String,
}

/// Shared, thread-safe session registry.
pub type SessionRegistry = Arc<Mutex<Vec<SessionMeta>>>;

pub fn new_registry() -> SessionRegistry {
    Arc::new(Mutex::new(Vec::new()))
}

/// Walk `~/.claude/projects/` and collect all `.jsonl` session files.
///
/// Sub-agent sessions live under `<project_dir>/subagents/`.
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

    Ok(sessions)
}

fn build_session_meta(projects_root: &PathBuf, jsonl_path: &std::path::Path) -> Option<SessionMeta> {
    let session_id = jsonl_path.file_stem()?.to_str()?.to_owned();

    // Determine if this is a sub-agent by checking whether `subagents` appears
    // in the path components between the projects root and the file.
    let rel = jsonl_path.strip_prefix(projects_root).ok()?;
    let components: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    // components[0] = project dirname, components[1] = file or "subagents", ...
    let is_subagent = components.contains(&"subagents");

    let project_dir = if let Some(first) = components.first() {
        projects_root.join(first).to_string_lossy().into_owned()
    } else {
        return None;
    };

    Some(SessionMeta {
        session_id,
        project_dir,
        is_subagent,
        jsonl_path: jsonl_path.to_string_lossy().into_owned(),
    })
}

/// Tauri command — returns the current snapshot of known sessions.
#[tauri::command]
pub fn list_sessions(registry: tauri::State<'_, SessionRegistry>) -> Vec<SessionMeta> {
    registry.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
