use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tracing::{error, info, warn};

use crate::error::MutexExt;
use crate::jsonl_parser::{parse_line, AgentEvent};
use crate::session_map;
use crate::session_registry::{scan_projects, SessionRegistry};

/// Per-file byte offset for tail-reading.
type TailOffsets = Arc<Mutex<HashMap<PathBuf, u64>>>;

/// Map from session_id string to frontend agent ID (usize).
pub type SessionAgentMap = Arc<Mutex<HashMap<String, usize>>>;

/// Map from agent_id to cancellation flag for waiting/permission timers.
type TimerCancelMap = Arc<Mutex<HashMap<usize, Arc<AtomicBool>>>>;

/// Cache of session_id -> last modified Unix seconds, updated incrementally.
/// Used by the expiry thread instead of a full WalkDir scan every 60 seconds.
type ModifiedCache = Arc<Mutex<HashMap<String, u64>>>;

const WAITING_DELAY_MS: u64 = 2000;
const PERMISSION_DELAY_MS: u64 = 5000;

/// Create a new, empty SessionAgentMap.
/// Called from lib.rs so the map can be shared with hooks_server.
pub fn new_session_agent_map() -> SessionAgentMap {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn start_watcher(
    app: AppHandle,
    registry: SessionRegistry,
    session_to_agent: SessionAgentMap,
) -> crate::error::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| {
        crate::error::AppError::Settings("Cannot resolve home directory".to_owned())
    })?;

    let projects_root = home.join(".claude").join("projects");

    if !projects_root.exists() {
        warn!("~/.claude/projects does not exist - watcher not started");
        return Ok(());
    }

    let offsets: TailOffsets = Arc::new(Mutex::new(HashMap::new()));
    seed_offsets(&projects_root, &offsets);

    let next_agent_id: Arc<Mutex<usize>> = Arc::new(Mutex::new(1));
    let timer_cancel_map: TimerCancelMap = Arc::new(Mutex::new(HashMap::new()));
    let modified_cache: ModifiedCache = Arc::new(Mutex::new(HashMap::new()));

    {
        if let Ok(sessions) = scan_projects() {
            let main_sessions: Vec<_> = sessions.iter().filter(|s| !s.is_subagent).cloned().collect();
            let sub_sessions: Vec<_> = sessions.iter().filter(|s| s.is_subagent).cloned().collect();

            // Load persisted session → agent_id mapping so IDs are stable across restarts.
            let persisted_map = session_map::load();
            let mut next_sequential: usize = persisted_map.values().copied().max().unwrap_or(0) + 1;

            let mut map = session_to_agent.lock_or_recover();
            let mut next_id = next_agent_id.lock_or_recover();
            let mut cache = modified_cache.lock_or_recover();
            let mut cancel_map = timer_cancel_map.lock_or_recover();

            for session in &main_sessions {
                let agent_id = if let Some(&existing) = persisted_map.get(&session.session_id) {
                    existing
                } else {
                    let id = next_sequential;
                    next_sequential += 1;
                    id
                };
                map.insert(session.session_id.clone(), agent_id);
                cache.insert(session.session_id.clone(), session.modified_secs);
                cancel_map.insert(agent_id, Arc::new(AtomicBool::new(false)));
            }

            // Register sub-agents with IDs matching the vscode-shim bootstrap formula:
            // subId = mainSessions.length + subIdx + 1  (same as vscode-shim.ts line ~144).
            // This ensures Rust events (agentToolStart, agentStatus…) carry the same ID
            // as the character the frontend created at startup.
            let main_count = main_sessions.len();
            for (sub_idx, sub) in sub_sessions.iter().enumerate() {
                let sub_id = main_count + sub_idx + 1;
                map.insert(sub.session_id.clone(), sub_id);
                cache.insert(sub.session_id.clone(), sub.modified_secs);
            }

            // next_id must be above every assigned ID so new live sessions don't collide.
            *next_id = next_sequential.max(main_count + sub_sessions.len() + 1);

            // Persist the updated map so new sessions are stable on next launch.
            session_map::save(&map);
        }
    }

    let offsets_watcher = Arc::clone(&offsets);
    let app_watcher = app.clone();
    let session_to_agent_watcher = Arc::clone(&session_to_agent);
    let next_agent_id_watcher = Arc::clone(&next_agent_id);
    let timer_cancel_map_watcher = Arc::clone(&timer_cancel_map);
    let modified_cache_watcher = Arc::clone(&modified_cache);

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
                        &timer_cancel_map_watcher,
                        &modified_cache_watcher,
                    );
                }
            }
        }
    });

    // Session expiry monitor: poll every 60s, emit agentClosed for sessions idle > 24h.
    // Uses the in-memory modified_cache instead of a full WalkDir scan each tick.
    {
        let app_expiry = app.clone();
        let s2a_expiry = Arc::clone(&session_to_agent);
        let cache_expiry = Arc::clone(&modified_cache);
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

                // Read active session IDs from the cache (no disk I/O).
                let active_ids: std::collections::HashSet<String> = {
                    let cache = cache_expiry.lock_or_recover();
                    cache
                        .iter()
                        .filter(|(_, &ts)| ts >= cutoff)
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                let mut map = s2a_expiry.lock_or_recover();
                let expired: Vec<(String, usize)> = map
                    .iter()
                    .filter(|(sid, _)| !active_ids.contains(*sid))
                    .map(|(k, &v)| (k.clone(), v))
                    .collect();
                let expired_ids: Vec<String> = expired.iter().map(|(id, _)| id.clone()).collect();
                for (session_id, agent_id) in &expired {
                    map.remove(session_id);
                    let msg = serde_json::json!({ "type": "agentClosed", "id": agent_id });
                    if let Err(e) = app_expiry.emit("agent-event", &msg) {
                        warn!("Failed to emit agentClosed for {session_id}: {e}");
                    }
                }
                // Persist expiry: remove stale entries from session-map.json (I2).
                drop(map);
                session_map::remove_expired(&expired_ids);
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
    let mut map = offsets.lock_or_recover();
    for session in sessions {
        let path = PathBuf::from(&session.jsonl_path);
        if let Ok(meta) = std::fs::metadata(&path) {
            map.insert(path, meta.len());
        }
    }
}

fn translate_to_frontend_messages(event: &AgentEvent, agent_id: usize) -> Vec<serde_json::Value> {
    match event {
        AgentEvent::ToolUse { id, tool, input } => {
            vec![json!({
                "type": "agentToolStart",
                "id": agent_id,
                "toolId": id,
                "status": format!("Using {}...", tool),
                "toolName": tool,
                "toolInput": input,
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
            // F1: Only emit agentToolsClear immediately.
            // The waiting and permission timers are handled separately in process_jsonl_file.
            vec![json!({
                "type": "agentToolsClear",
                "id": agent_id,
            })]
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

/// Cancel any pending timer for the given agent and reset the flag for fresh timers.
fn cancel_timer(timer_cancel_map: &TimerCancelMap, agent_id: usize) {
    let mut map = timer_cancel_map.lock_or_recover();
    if let Some(flag) = map.get(&agent_id) {
        flag.store(true, Ordering::SeqCst);
    }
    // Reset with a fresh flag so subsequent timers for this agent start clean.
    map.insert(agent_id, Arc::new(AtomicBool::new(false)));
}

/// Spawn waiting (2s) and permission (5s) timers for the given agent.
/// Both timers share the same cancellation flag; if a new ToolUse arrives
/// before either fires, cancel_timer() sets the flag to true.
fn spawn_timers(app: &AppHandle, agent_id: usize, timer_cancel_map: &TimerCancelMap) {
    let cancelled = {
        let mut map = timer_cancel_map.lock_or_recover();
        let flag = Arc::new(AtomicBool::new(false));
        map.insert(agent_id, Arc::clone(&flag));
        flag
    };

    // Waiting timer: after WAITING_DELAY_MS, emit agentStatus waiting.
    {
        let app_w = app.clone();
        let cancelled_w = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(WAITING_DELAY_MS));
            if !cancelled_w.load(Ordering::SeqCst) {
                let msg = json!({ "type": "agentStatus", "id": agent_id, "status": "waiting" });
                if let Err(e) = app_w.emit("agent-event", &msg) {
                    warn!("Failed to emit agentStatus waiting for agent {agent_id}: {e}");
                }
            }
        });
    }

    // Permission timer: after PERMISSION_DELAY_MS, emit agentToolPermission.
    {
        let app_p = app.clone();
        let cancelled_p = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(PERMISSION_DELAY_MS));
            if !cancelled_p.load(Ordering::SeqCst) {
                let msg = json!({ "type": "agentToolPermission", "id": agent_id });
                if let Err(e) = app_p.emit("agent-event", &msg) {
                    warn!("Failed to emit agentToolPermission for agent {agent_id}: {e}");
                }
            }
        });
    }
}

/// Returns the byte length of each complete (newline-terminated) line in `raw`.
/// Handles both LF (`\n`) and CRLF (`\r\n`) terminators. Incomplete trailing
/// lines (not ending in `\n`) are excluded.
fn count_line_byte_lengths(raw: &[u8]) -> Vec<usize> {
    let mut lengths = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let start = i;
        while i < raw.len() && raw[i] != b'\n' {
            i += 1;
        }
        if i < raw.len() {
            i += 1; // consume \n
            lengths.push(i - start);
        }
    }
    lengths
}

#[allow(clippy::too_many_arguments)]
fn process_jsonl_file(
    path: &PathBuf,
    offsets: &TailOffsets,
    app: &AppHandle,
    registry: &SessionRegistry,
    session_to_agent: &SessionAgentMap,
    next_agent_id: &Arc<Mutex<usize>>,
    timer_cancel_map: &TimerCancelMap,
    modified_cache: &ModifiedCache,
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

    let mut offsets_guard = offsets.lock_or_recover();
    let offset = offsets_guard.entry(path.clone()).or_insert(0);

    if current_len < *offset {
        *offset = 0;
        if let Ok(sessions) = scan_projects() {
            let mut reg = registry.lock_or_recover();
            *reg = sessions;
        }
    }

    let start = *offset;
    drop(offsets_guard);

    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }

    let mut raw = Vec::new();
    if file.take(512 * 1024).read_to_end(&mut raw).is_err() {
        return;
    }

    if raw.is_empty() {
        return;
    }

    let buf = String::from_utf8_lossy(&raw);

    let ends_with_newline = buf.ends_with('\n');
    let lines: Vec<&str> = buf.lines().collect();

    let complete_lines = if ends_with_newline {
        &lines[..]
    } else if lines.len() > 1 {
        &lines[..lines.len() - 1]
    } else {
        return;
    };

    // Count actual bytes per line from raw buffer (handles CRLF and LF correctly).
    let line_byte_lengths = count_line_byte_lengths(&raw);
    let processed_bytes: u64 = line_byte_lengths
        .iter()
        .take(complete_lines.len())
        .sum::<usize>() as u64;

    {
        let mut offsets_guard = offsets.lock_or_recover();
        let offset = offsets_guard.entry(path.clone()).or_insert(start);
        *offset = start + processed_bytes;
    }

    // Update the modified_secs cache for this session so the expiry thread
    // doesn't need to do a WalkDir scan.
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(d) = mtime.duration_since(std::time::SystemTime::UNIX_EPOCH) {
                let mut cache = modified_cache.lock_or_recover();
                cache.insert(session_id.clone(), d.as_secs());
            }
        }
    }

    // Detect if this path is a sub-agent:
    // Layout: <projects_root>/<project>/<parent_uuid>/subagents/<child_uuid>.jsonl
    let is_subagent_path = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("subagents");

    // Derive parent session_id from path for sub-agents.
    let path_parent_sid: Option<String> = if is_subagent_path {
        path.parent()
            .and_then(|p| p.parent())
            .and_then(|pp| pp.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_owned)
    } else {
        None
    };

    let agent_id = {
        let mut map = session_to_agent.lock_or_recover();
        if let Some(&id) = map.get(&session_id) {
            id
        } else {
            let mut next_id = next_agent_id.lock_or_recover();
            let new_id = *next_id;
            *next_id += 1;
            map.insert(session_id.clone(), new_id);
            // Persist updated map so new sessions survive restarts.
            session_map::save(&map);

            let folder_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_owned();

            // Build agentCreated payload, enriched for sub-agents.
            let created_msg = if is_subagent_path {
                // Resolve the parent's numeric agent_id from the session map.
                let parent_agent_id = path_parent_sid
                    .as_deref()
                    .and_then(|psid| map.get(psid).copied());
                if let Some(paid) = parent_agent_id {
                    json!({
                        "type": "agentCreated",
                        "id": new_id,
                        "folderName": folder_name,
                        "isTeammate": true,
                        "parentAgentId": paid,
                    })
                } else {
                    // Parent not yet registered — emit without parentAgentId
                    // (agentTeamInfo will link them when SubagentInit is processed).
                    json!({
                        "type": "agentCreated",
                        "id": new_id,
                        "folderName": folder_name,
                        "isTeammate": true,
                    })
                }
            } else {
                json!({
                    "type": "agentCreated",
                    "id": new_id,
                    "folderName": folder_name,
                })
            };

            if let Err(e) = app.emit("agent-event", &created_msg) {
                warn!("Failed to emit agentCreated: {e}");
            }

            new_id
        }
    };

    for line in complete_lines {
        if let Some(parsed) = parse_line(&session_id, line) {
            // P3: handle SubagentInit - emit agentTeamInfo linking sub-agent to parent.
            if let AgentEvent::SubagentInit { parent_session_id, .. } = &parsed.event {
                let effective_parent = parent_session_id
                    .as_deref()
                    .or(path_parent_sid.as_deref());
                if let Some(parent_sid) = effective_parent {
                    let parent_agent_id = session_to_agent
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(parent_sid)
                        .copied();
                    if let Some(lead_id) = parent_agent_id {
                        let team_msg = json!({
                            "type": "agentTeamInfo",
                            "id": agent_id,
                            "isTeamLead": false,
                            "leadAgentId": lead_id,
                        });
                        if let Err(e) = app.emit("agent-event", &team_msg) {
                            warn!("Failed to emit agentTeamInfo: {e}");
                        }
                    }
                }
                continue; // SubagentInit has no other frontend messages
            }

            // F1: On ToolUse, cancel any pending waiting/permission timers for this agent.
            if matches!(parsed.event, AgentEvent::ToolUse { .. }) {
                cancel_timer(timer_cancel_map, agent_id);
            }

            let messages = translate_to_frontend_messages(&parsed.event, agent_id);
            for msg in messages {
                if let Err(e) = app.emit("agent-event", &msg) {
                    warn!("Failed to emit agent-event: {e}");
                }
            }

            // F1: On turn_duration, spawn waiting and permission timers.
            if matches!(
                parsed.event,
                AgentEvent::System { ref subtype, .. } if subtype == "turn_duration"
            ) {
                spawn_timers(app, agent_id, timer_cancel_map);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_line_bytes_counted_correctly() {
        let raw = b"line1\r\nline2\r\nline3\r\n";
        let lengths = count_line_byte_lengths(raw);
        assert_eq!(lengths, vec![7, 7, 7]);
        assert_eq!(lengths.iter().sum::<usize>(), raw.len());
    }

    #[test]
    fn lf_only_line_bytes_unchanged() {
        let raw = b"line1\nline2\nline3\n";
        let lengths = count_line_byte_lengths(raw);
        assert_eq!(lengths, vec![6, 6, 6]);
        assert_eq!(lengths.iter().sum::<usize>(), raw.len());
    }

    #[test]
    fn incomplete_final_line_not_counted() {
        let raw = b"line1\nincomplete";
        let lengths = count_line_byte_lengths(raw);
        assert_eq!(lengths, vec![6]);
    }

    #[test]
    fn empty_raw_yields_no_lengths() {
        assert!(count_line_byte_lengths(b"").is_empty());
    }
}
