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
use tracing::{debug, error, info, warn};

use crate::error::MutexExt;
use crate::jsonl_parser::{parse_line, AgentEvent};
use crate::session_map;
use crate::session_registry::{scan_projects, SessionRegistry};

/// Per-file byte offset for tail-reading.
type TailOffsets = Arc<Mutex<HashMap<PathBuf, u64>>>;

/// Shared, thread-safe mapping from a Claude Code `session_id` (string UUID)
/// to the corresponding frontend agent ID (`usize`). Used by both the file
/// watcher and the hooks HTTP server so a session resolves to the same agent
/// regardless of ingestion path.
pub type SessionAgentMap = Arc<Mutex<HashMap<String, usize>>>;

/// Map from agent_id to cancellation flag for waiting/permission timers.
type TimerCancelMap = Arc<Mutex<HashMap<usize, Arc<AtomicBool>>>>;

/// Cache of session_id -> last modified Unix seconds, updated incrementally.
/// Used by the expiry thread instead of a full WalkDir scan every 60 seconds.
type ModifiedCache = Arc<Mutex<HashMap<String, u64>>>;

const WAITING_DELAY_MS: u64 = 2000;
const PERMISSION_DELAY_MS: u64 = 5000;

/// Number of seconds of JSONL history to replay at startup so an agent that
/// emitted events shortly before the app launched does not appear idle.
/// Configurable via the `PIXEL_AGENTS_REPLAY_SECS` env var (for debugging).
const REPLAY_WINDOW_SECS: u64 = 30;

/// Hard ceiling on how far back we will scan inside a single JSONL while
/// looking for the replay cutoff (safety against gigantic files).
const REPLAY_MAX_SCAN_BYTES: u64 = 1024 * 1024;

/// Create a new, empty SessionAgentMap.
/// Called from lib.rs so the map can be shared with hooks_server.
pub fn new_session_agent_map() -> SessionAgentMap {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Global shutdown flag, checked by background threads (notably the
/// expiry monitor) so they exit cleanly before the app kills the process.
/// Without this, the 60s expiry sleep can be interrupted mid-`session_map::save`,
/// truncating session-map.json.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Signal background threads to stop. Called from `lib.rs` on
/// `RunEvent::ExitRequested` (or any other shutdown hook).
pub fn signal_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
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

            // Register sub-agents — HONOR persisted_map first to avoid ID collisions.
            // Previous bug: assigning sub_id = main_count + sub_idx + 1 ignored persistence
            // and would collide with existing main agent IDs (1, 2, 3...) causing events
            // to be dispatched on wrong characters → "Idle" label stuck forever.
            // Now: same logic as main_sessions, persisted ID wins, otherwise next sequential.
            for sub in &sub_sessions {
                let sub_id = if let Some(&existing) = persisted_map.get(&sub.session_id) {
                    existing
                } else {
                    let id = next_sequential;
                    next_sequential += 1;
                    id
                };
                map.insert(sub.session_id.clone(), sub_id);
                cache.insert(sub.session_id.clone(), sub.modified_secs);
                cancel_map.insert(sub_id, Arc::new(AtomicBool::new(false)));
            }

            // next_id must be above every assigned ID so new live sessions don't collide.
            *next_id = next_sequential;

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
        let cancel_expiry = Arc::clone(&timer_cancel_map);
        std::thread::spawn(move || {
            const POLL_INTERVAL: Duration = Duration::from_secs(60);
            const SHUTDOWN_TICK: Duration = Duration::from_millis(500);
            const MAX_AGE_SECS: u64 = 24 * 3600;
            loop {
                // Sleep in 500ms ticks so we react quickly when the app exits.
                let mut slept = Duration::from_secs(0);
                while slept < POLL_INTERVAL {
                    if shutdown_requested() {
                        info!("Expiry monitor: shutdown signaled, exiting");
                        return;
                    }
                    std::thread::sleep(SHUTDOWN_TICK);
                    slept += SHUTDOWN_TICK;
                }
                if shutdown_requested() {
                    info!("Expiry monitor: shutdown signaled, exiting");
                    return;
                }
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let cutoff = now_secs.saturating_sub(MAX_AGE_SECS);

                // FIX 2: only expire sessions that are PRESENT in the cache
                // with an old timestamp. Previously we expired any session
                // missing from the active set, which incorrectly closed
                // sessions at startup that hadn't yet been cache-inserted.
                let expired_session_ids: Vec<String> = {
                    let cache = cache_expiry.lock_or_recover();
                    cache
                        .iter()
                        .filter(|(_, &ts)| ts < cutoff)
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                if expired_session_ids.is_empty() {
                    continue;
                }

                let expired: Vec<(String, usize)> = {
                    let mut map = s2a_expiry.lock_or_recover();
                    let mut out = Vec::new();
                    for sid in &expired_session_ids {
                        if let Some(aid) = map.remove(sid) {
                            out.push((sid.clone(), aid));
                        }
                    }
                    out
                };

                // FIX 5: clean up the other maps keyed by session_id / agent_id
                // so they don't grow unbounded.
                {
                    let mut cache = cache_expiry.lock_or_recover();
                    for (sid, _) in &expired {
                        cache.remove(sid);
                    }
                }
                {
                    let mut cmap = cancel_expiry.lock_or_recover();
                    for (_, aid) in &expired {
                        if let Some(flag) = cmap.remove(aid) {
                            flag.store(true, Ordering::SeqCst);
                        }
                    }
                }
                // tail_offsets is keyed by PathBuf, not session_id — we don't
                // have the path here, so we let it persist (bounded by the
                // number of distinct JSONL files seen, which is small).

                let expired_ids: Vec<String> = expired.iter().map(|(id, _)| id.clone()).collect();
                for (session_id, agent_id) in &expired {
                    let msg = serde_json::json!({ "type": "agentClosed", "id": agent_id });
                    if let Err(e) = app_expiry.emit("agent-event", &msg) {
                        warn!("Failed to emit agentClosed for {session_id}: {e}");
                    }
                }
                info!("Expiry tick: closed {} idle session(s)", expired.len());
                // Persist expiry: remove stale entries from session-map.json (I2).
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

    let replay_window = std::env::var("PIXEL_AGENTS_REPLAY_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(REPLAY_WINDOW_SECS);

    let cutoff = iso8601_cutoff(replay_window);

    let mut map = offsets.lock_or_recover();
    for session in sessions {
        let raw_path = PathBuf::from(&session.jsonl_path);
        // Fix 4: same canonicalization as process_jsonl_file so the HashMap
        // keys align (avoids the same session being seeded twice via two
        // path spellings).
        let path = dunce::canonicalize(&raw_path).unwrap_or(raw_path);
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                warn!("seed_offsets: metadata failed for {}: {e}", path.display());
                continue;
            }
        };
        let len = meta.len();
        let offset = compute_replay_offset(&path, len, cutoff.as_deref());
        if offset < len {
            info!(
                "Replay: rewound {} bytes in {}",
                len - offset,
                path.display()
            );
        }
        map.insert(path, offset);
    }
}

/// Build an ISO-8601 UTC cutoff string (`YYYY-MM-DDTHH:MM:SS.sssZ`) for
/// `now - window_secs`. Returns `None` if the system clock is before the
/// Unix epoch (shouldn't happen) — caller falls back to legacy seeding.
fn iso8601_cutoff(window_secs: u64) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let secs = now.as_secs().saturating_sub(window_secs);
    Some(format_iso8601(secs))
}

/// Format a Unix timestamp (seconds) as an ISO-8601 UTC string with
/// millisecond precision and a trailing `Z`. Pure date arithmetic — no
/// dependency on chrono.
fn format_iso8601(secs: u64) -> String {
    // Days since 1970-01-01 + time of day
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;

    // Convert days-since-epoch to (year, month, day) via Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        year, m, d, hour, minute, second
    )
}

/// Extract the `"timestamp":"…"` field from a single JSONL line, if present.
/// Returns the raw string value (without surrounding quotes) so callers can
/// compare it lexicographically against an ISO-8601 cutoff.
fn extract_timestamp(line: &str) -> Option<&str> {
    // Tolerate optional whitespace between key, colon, and value.
    let key_idx = line.find("\"timestamp\"")?;
    let rest = &line[key_idx + "\"timestamp\"".len()..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    let after_colon = after_colon.strip_prefix('"')?;
    let end = after_colon.find('"')?;
    Some(&after_colon[..end])
}

/// Find the byte offset in `path` of the first line whose timestamp is
/// `>= cutoff`. Returns `file_len` if no such line exists (i.e. everything
/// is old, or no parseable timestamp was found) so behaviour matches the
/// legacy "skip history" path.
///
/// Reads the file backwards in 4 KB chunks, capped at `REPLAY_MAX_SCAN_BYTES`.
fn compute_replay_offset(path: &PathBuf, file_len: u64, cutoff: Option<&str>) -> u64 {
    if file_len == 0 {
        return 0;
    }
    let Some(cutoff) = cutoff else {
        return file_len;
    };

    let scan_limit = file_len.min(REPLAY_MAX_SCAN_BYTES);
    let scan_start = file_len - scan_limit;

    // For small files, just load the relevant tail in one shot.
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            warn!(
                "compute_replay_offset: File::open failed for {}: {e}",
                path.display()
            );
            return file_len;
        }
    };
    if let Err(e) = file.seek(SeekFrom::Start(scan_start)) {
        warn!(
            "compute_replay_offset: seek to {scan_start} failed for {}: {e}",
            path.display()
        );
        return file_len;
    }
    let mut buf = Vec::with_capacity(scan_limit as usize);
    if let Err(e) = file.take(scan_limit).read_to_end(&mut buf) {
        warn!(
            "compute_replay_offset: read failed for {}: {e}",
            path.display()
        );
        return file_len;
    }

    // If we did not start at byte 0 of the file, the buffer's first "line"
    // is a partial line — discard it by advancing past the first newline.
    let line_search_start: usize = if scan_start == 0 {
        0
    } else {
        match buf.iter().position(|&b| b == b'\n') {
            Some(idx) => idx + 1,
            None => return file_len, // single megaline, nothing parseable
        }
    };

    // Walk forward over complete lines, recording (line_start_offset, has_recent_ts).
    // The earliest line with timestamp >= cutoff is our replay anchor.
    let mut found_anchor: Option<u64> = None;
    let mut sentinel_seen = false; // any parseable timestamp at all
    let mut cursor = line_search_start;
    while cursor < buf.len() {
        let line_start = cursor;
        let nl_rel = buf[cursor..].iter().position(|&b| b == b'\n');
        let (line_end, advance) = match nl_rel {
            Some(rel) => (cursor + rel, cursor + rel + 1),
            None => (buf.len(), buf.len()), // incomplete trailing line
        };

        // Stop if we hit an incomplete trailing line (no terminator).
        if nl_rel.is_none() {
            break;
        }

        let line_bytes = &buf[line_start..line_end];
        if let Ok(line_str) = std::str::from_utf8(line_bytes) {
            if let Some(ts) = extract_timestamp(line_str) {
                sentinel_seen = true;
                if ts >= cutoff {
                    found_anchor = Some(scan_start + line_start as u64);
                    break;
                }
            }
        }
        cursor = advance;
    }

    match found_anchor {
        Some(off) => off,
        None => {
            // If we never parsed any timestamp at all, the format may be
            // unfamiliar — degrade by replaying ~50 KB of tail rather than
            // skipping everything (graceful fallback per spec).
            if !sentinel_seen {
                file_len.saturating_sub(50 * 1024)
            } else {
                file_len
            }
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

    // Fix 4: canonicalize the path before using it as a HashMap key so that
    // junctions / symlinks / mixed casing don't produce duplicate offset
    // entries (which would double-replay the same bytes). `dunce::canonicalize`
    // resolves like `std::fs::canonicalize` but strips the Windows `\\?\` UNC
    // prefix so paths stay comparable to the keys we use elsewhere.
    let path: PathBuf = dunce::canonicalize(path).unwrap_or_else(|_| path.clone());
    let path = &path;

    // Windows MAX_PATH (260 chars): paths exceeding this limit fail to open
    // unless prefixed with `\\?\`. Sessions with deeply nested sub-agents +
    // UUID directory names can hit this. We log the failure so the issue is
    // diagnosable instead of silent.
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            warn!("Cannot open {}: {e}", path.display());
            return;
        }
    };

    let current_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            warn!("Cannot stat {}: {e}", path.display());
            return;
        }
    };

    // FIX 1 (TOCTOU): hold the `offsets` lock across the entire
    // read-and-update sequence so two rapid notify events on the same path
    // cannot read overlapping byte ranges and emit duplicate events.
    // The lock is short-lived in practice (one bounded 512 KB read) and the
    // few other call sites of `offsets` only briefly mutate the map.
    let mut offsets_guard = offsets.lock_or_recover();
    let offset_entry = offsets_guard.entry(path.clone()).or_insert(0);

    if current_len < *offset_entry {
        *offset_entry = 0;
        if let Ok(sessions) = scan_projects() {
            let mut reg = registry.lock_or_recover();
            *reg = sessions;
        }
    }

    let start = *offset_entry;

    if let Err(e) = file.seek(SeekFrom::Start(start)) {
        warn!("Cannot seek {} to {}: {e}", path.display(), start);
        return;
    }

    let mut raw = Vec::new();
    if let Err(e) = file.take(512 * 1024).read_to_end(&mut raw) {
        warn!("Cannot read {}: {e}", path.display());
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

    // Commit the new offset BEFORE dropping the guard so the next notify
    // event cannot replay these same bytes.
    *offset_entry = start + processed_bytes;
    drop(offsets_guard);
    debug!(
        "process_jsonl_file: {} consumed {} bytes (from {} to {})",
        path.display(),
        processed_bytes,
        start,
        start + processed_bytes
    );

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

    // FIX 6: do disk I/O (session_map::save) and event emission OUTSIDE the
    // session_to_agent lock so we don't block hooks_server::handle_connection
    // or list_sessions during a write.
    let (agent_id, pending_save, pending_emit): (
        usize,
        Option<HashMap<String, usize>>,
        Option<serde_json::Value>,
    ) = {
        let mut map = session_to_agent.lock_or_recover();
        if let Some(&id) = map.get(&session_id) {
            (id, None, None)
        } else {
            let mut next_id = next_agent_id.lock_or_recover();
            let new_id = *next_id;
            *next_id += 1;
            map.insert(session_id.clone(), new_id);

            let folder_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_owned();

            let created_msg = if is_subagent_path {
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

            // Snapshot the map for the save() call so we can drop the guard.
            let snapshot = map.clone();
            (new_id, Some(snapshot), Some(created_msg))
        }
        // map guard dropped here
    };

    if let Some(snapshot) = pending_save {
        debug!("process_jsonl_file: persisting session map (no lock held)");
        session_map::save(&snapshot);
    }
    if let Some(msg) = pending_emit {
        if let Err(e) = app.emit("agent-event", &msg) {
            warn!("Failed to emit agentCreated: {e}");
        }
    }

    for line in complete_lines {
        if let Some(parsed) = parse_line(&session_id, line) {
            // P3: handle SubagentInit - emit agentTeamInfo linking sub-agent to parent.
            if let AgentEvent::SubagentInit { parent_session_id, .. } = &parsed.event {
                let effective_parent = parent_session_id
                    .as_deref()
                    .or(path_parent_sid.as_deref());
                if let Some(parent_sid) = effective_parent {
                    let parent_agent_id = session_to_agent
                        .lock_or_recover()
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

    #[test]
    fn extract_timestamp_basic() {
        let line = r#"{"type":"x","timestamp":"2026-05-15T10:00:00.000Z","other":1}"#;
        assert_eq!(
            extract_timestamp(line),
            Some("2026-05-15T10:00:00.000Z")
        );
    }

    #[test]
    fn extract_timestamp_missing() {
        let line = r#"{"type":"x","other":1}"#;
        assert_eq!(extract_timestamp(line), None);
    }

    #[test]
    fn format_iso8601_known_epoch() {
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00.000Z");
        // 2020-01-01T00:00:00Z == 1_577_836_800
        assert_eq!(format_iso8601(1_577_836_800), "2020-01-01T00:00:00.000Z");
        // 2020-02-29T00:00:00Z (leap day) == 1_582_934_400
        assert_eq!(format_iso8601(1_582_934_400), "2020-02-29T00:00:00.000Z");
    }

    #[test]
    fn seed_offsets_with_replay_skips_old_lines() {
        use std::io::Write;

        // Build cutoff as "now - 30s" — same logic as production.
        let cutoff = iso8601_cutoff(30).expect("cutoff");

        // Helper: a timestamp `delta_secs` in the past, ISO-8601 UTC.
        let ts_at = |delta_secs: u64| -> String {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            format_iso8601(now.saturating_sub(delta_secs))
        };

        let old = ts_at(60); // -60s, before cutoff
        let recent_a = ts_at(10); // -10s, after cutoff
        let recent_b = ts_at(5); // -5s, after cutoff

        let line_old = format!(r#"{{"type":"a","timestamp":"{}"}}"#, old) + "\n";
        let line_a = format!(r#"{{"type":"b","timestamp":"{}"}}"#, recent_a) + "\n";
        let line_b = format!(r#"{{"type":"c","timestamp":"{}"}}"#, recent_b) + "\n";

        let dir = std::env::temp_dir().join(format!(
            "pixel-agents-replay-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("seed_test.jsonl");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(line_old.as_bytes()).unwrap();
            f.write_all(line_a.as_bytes()).unwrap();
            f.write_all(line_b.as_bytes()).unwrap();
        }
        let len = std::fs::metadata(&path).unwrap().len();
        let offset = compute_replay_offset(&path, len, Some(&cutoff));

        // Expect offset == byte length of the first (old) line — i.e. the
        // two recent lines will be replayed.
        let expected = line_old.len() as u64;
        assert_eq!(
            offset, expected,
            "offset should point at start of first recent line"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn compute_replay_offset_empty_file_returns_zero() {
        let dir = std::env::temp_dir().join(format!(
            "pixel-agents-replay-empty-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.jsonl");
        std::fs::File::create(&path).unwrap();
        assert_eq!(compute_replay_offset(&path, 0, Some("2000-01-01T00:00:00.000Z")), 0);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn compute_replay_offset_all_old_returns_file_len() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!(
            "pixel-agents-replay-allold-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("all_old.jsonl");
        let line = "{\"timestamp\":\"2000-01-01T00:00:00.000Z\"}\n";
        {
            let mut f = std::fs::File::create(&path).unwrap();
            for _ in 0..3 {
                f.write_all(line.as_bytes()).unwrap();
            }
        }
        let len = std::fs::metadata(&path).unwrap().len();
        let cutoff = iso8601_cutoff(30).unwrap();
        assert_eq!(compute_replay_offset(&path, len, Some(&cutoff)), len);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
