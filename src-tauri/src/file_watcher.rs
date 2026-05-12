use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tracing::{error, info, warn};

use crate::jsonl_parser::{parse_line, AgentEvent};
use crate::session_registry::{scan_projects, SessionRegistry};

/// Per-file byte offset for tail-reading.
type TailOffsets = Arc<Mutex<HashMap<PathBuf, u64>>>;

/// Map from session_id string to frontend agent ID (usize).
type SessionAgentMap = Arc<Mutex<HashMap<String, usize>>>;

pub fn start_watcher(app: AppHandle, registry: SessionRegistry) -> crate::error::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| {
        crate::error::AppError::Settings("Cannot resolve home directory".to_owned())
    })?;

    let projects_root = home.join(".claude").join("projects");

    if !projects_root.exists() {
        warn!("~/.claude/projects does not exist — watcher not started");
        return Ok(());
    }

    let offsets: TailOffsets = Arc::new(Mutex::new(HashMap::new()));
    seed_offsets(&projects_root, &offsets);

    let session_to_agent: SessionAgentMap = Arc::new(Mutex::new(HashMap::new()));
    let next_agent_id: Arc<Mutex<usize>> = Arc::new(Mutex::new(1));

    {
        if let Ok(sessions) = scan_projects() {
            let main_sessions: Vec<_> = sessions
                .into_iter()
                .filter(|s| !s.is_subagent)
                .collect();
            let mut map = session_to_agent.lock().unwrap_or_else(|e| e.into_inner());
            let mut next_id = next_agent_id.lock().unwrap_or_else(|e| e.into_inner());
            for (idx, session) in main_sessions.iter().enumerate() {
                let agent_id = idx + 1;
                map.insert(session.session_id.clone(), agent_id);
            }
            *next_id = main_sessions.len() + 1;
        }
    }

    let offsets_watcher = Arc::clone(&offsets);
    let app_watcher = app.clone();
    let session_to_agent_watcher = Arc::clone(&session_to_agent);
    let next_agent_id_watcher = Arc::clone(&next_agent_id);

    let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<DebouncedEvent>, Vec<notify::Error>>>();

    let mut debouncer = new_debouncer(Duration::from_millis(50), None, tx)
        .map_err(crate::error::AppError::Watcher)?;

    debouncer
        .watcher()
        .watch(&projects_root, RecursiveMode::Recursive)
        .map_err(crate::error::AppError::Watcher)?;

    std::thread::spawn(move || {
        let _debouncer = debouncer;

        for batch in rx {
            let events = match batch {
                Ok(evs) => evs,
                Err(errs) => {
                    for e in errs {
                        error!("Watcher error: {e}");
                    }
                    continue;
                }
            };

            for de in events {
                let path = de.event.paths.into_iter().next().unwrap_or_default();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    process_jsonl_file(
                        &path,
                        &offsets_watcher,
                        &app_watcher,
                        &registry,
                        &session_to_agent_watcher,
                        &next_agent_id_watcher,
                    );
                }
            }
        }
    });

    // Session expiry monitor: poll every 60s, emit agentClosed for sessions idle > 24h
    {
        let app_expiry = app.clone();
        let s2a_expiry = Arc::clone(&session_to_agent);
        std::thread::spawn(move || {
            const POLL_INTERVAL: Duration = Duration::from_secs(60);
            const MAX_AGE_SECS: u64 = 24 * 3600;
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let cutoff = now_secs.saturating_sub(MAX_AGE_SECS);
                let active_ids: std::collections::HashSet<String> = scan_projects()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| s.modified_secs >= cutoff)
                    .map(|s| s.session_id)
                    .collect();
                let mut map = s2a_expiry.lock().unwrap_or_else(|e| e.into_inner());
                let expired: Vec<(String, usize)> = map
                    .iter()
                    .filter(|(sid, _)| !active_ids.contains(*sid))
                    .map(|(k, &v)| (k.clone(), v))
                    .collect();
                for (session_id, agent_id) in expired {
                    map.remove(&session_id);
                    let msg = serde_json::json!({ "type": "agentClosed", "id": agent_id });
                    if let Err(e) = app_expiry.emit("agent-event", &msg) {
                        warn!("Failed to emit agentClosed for {session_id}: {e}");
                    }
                }
            }
        });
    }
    info!("File watcher started on {}", projects_root.display());
    Ok(())
}

fn seed_offsets(_projects_root: &PathBuf, offsets: &TailOffsets) {
    let Ok(sessions) = scan_projects() else {
        return;
    };
    let mut map = offsets.lock().unwrap_or_else(|e| e.into_inner());
    for session in sessions {
        let path = PathBuf::from(&session.jsonl_path);
        if let Ok(meta) = std::fs::metadata(&path) {
            map.insert(path, meta.len());
        }
    }
}

fn translate_to_frontend_messages(event: &AgentEvent, agent_id: usize) -> Vec<serde_json::Value> {
    match event {
        AgentEvent::ToolUse { id, tool, .. } => {
            vec![json!({
                "type": "agentToolStart",
                "id": agent_id,
                "toolId": id,
                "status": format!("Using {}...", tool),
                "toolName": tool,
            })]
        }
        AgentEvent::ToolResult { id, .. } => {
            vec![json!({
                "type": "agentToolDone",
                "id": agent_id,
                "toolId": id,
            })]
        }
        AgentEvent::Text { .. } => {
            vec![json!({
                "type": "agentStatus",
                "id": agent_id,
                "status": "active",
            })]
        }
        AgentEvent::System { subtype, .. } if subtype == "turn_duration" => {
            // P3: emit "waiting" (not "idle") so frontend shows the user-attention bubble.
            vec![
                json!({
                    "type": "agentToolsClear",
                    "id": agent_id,
                }),
                json!({
                    "type": "agentStatus",
                    "id": agent_id,
                    "status": "waiting",
                }),
            ]
        }
        // P2: emit token usage so frontend can display consumption per agent.
        AgentEvent::TokenUsage { input_tokens, output_tokens } => {
            vec![json!({
                "type": "agentTokenUsage",
                "id": agent_id,
                "inputTokens": input_tokens,
                "outputTokens": output_tokens,
            })]
        }
        _ => vec![],
    }
}

fn process_jsonl_file(
    path: &PathBuf,
    offsets: &TailOffsets,
    app: &AppHandle,
    registry: &SessionRegistry,
    session_to_agent: &SessionAgentMap,
    next_agent_id: &Arc<Mutex<usize>>,
) {
    let session_id = match path.file_stem().and_then(|s| s.to_str()) {
        Some(id) => id.to_owned(),
        None => return,
    };

    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };

    let current_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            warn!("Cannot stat {}: {e}", path.display());
            return;
        }
    };

    let mut offsets_guard = offsets.lock().unwrap_or_else(|e| e.into_inner());
    let offset = offsets_guard.entry(path.clone()).or_insert(0);

    if current_len < *offset {
        *offset = 0;
        if let Ok(sessions) = scan_projects() {
            let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
            *reg = sessions;
        }
    }

    let start = *offset;
    drop(offsets_guard);

    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }

    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return;
    }

    let bytes_read = buf.len() as u64;
    if bytes_read == 0 {
        return;
    }

    let ends_with_newline = buf.ends_with('\n');
    let lines: Vec<&str> = buf.lines().collect();

    let complete_lines = if ends_with_newline {
        &lines[..]
    } else if lines.len() > 1 {
        &lines[..lines.len() - 1]
    } else {
        return;
    };

    let processed_bytes: u64 = complete_lines
        .iter()
        .map(|l| l.len() as u64 + 1)
        .sum();

    {
        let mut offsets_guard = offsets.lock().unwrap_or_else(|e| e.into_inner());
        let offset = offsets_guard.entry(path.clone()).or_insert(start);
        *offset = start + processed_bytes;
    }

    let agent_id = {
        let mut map = session_to_agent.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = map.get(&session_id) {
            id
        } else {
            let mut next_id = next_agent_id.lock().unwrap_or_else(|e| e.into_inner());
            let new_id = *next_id;
            *next_id += 1;
            map.insert(session_id.clone(), new_id);

            let folder_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_owned();

            let created_msg = json!({
                "type": "agentCreated",
                "id": new_id,
                "folderName": folder_name,
            });

            if let Err(e) = app.emit("agent-event", &created_msg) {
                warn!("Failed to emit agentCreated: {e}");
            }

            new_id
        }
    };

    for line in complete_lines {
        if let Some(parsed) = parse_line(&session_id, line) {
            let messages = translate_to_frontend_messages(&parsed.event, agent_id);
            for msg in messages {
                if let Err(e) = app.emit("agent-event", &msg) {
                    warn!("Failed to emit agent-event: {e}");
                }
            }
        }
    }
}
