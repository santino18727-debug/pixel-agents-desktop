use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
use tauri::{AppHandle, Emitter};
use tracing::{error, info, warn};

use crate::jsonl_parser::parse_line;
use crate::session_registry::{scan_projects, SessionRegistry};

/// Per-file byte offset for tail-reading.
type TailOffsets = Arc<Mutex<HashMap<PathBuf, u64>>>;

/// Start the filesystem watcher on `~/.claude/projects/` using a 50 ms debounce.
///
/// Any new JSONL lines discovered are emitted to the frontend as `"agent-event"`.
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

    // Seed offsets so we don't replay history on startup
    seed_offsets(&projects_root, &offsets);

    let offsets_watcher = Arc::clone(&offsets);
    let app_watcher = app.clone();

    let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<DebouncedEvent>, Vec<notify::Error>>>();

    let mut debouncer = new_debouncer(Duration::from_millis(50), None, tx)
        .map_err(crate::error::AppError::Watcher)?;

    debouncer
        .watcher()
        .watch(&projects_root, RecursiveMode::Recursive)
        .map_err(crate::error::AppError::Watcher)?;

    // Keep the debouncer alive by moving it into the spawned thread.
    std::thread::spawn(move || {
        let _debouncer = debouncer; // keep alive

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
                    process_jsonl_file(&path, &offsets_watcher, &app_watcher, &registry);
                }
            }
        }
    });

    info!("File watcher started on {}", projects_root.display());
    Ok(())
}

/// Seed tail offsets for all existing JSONL files so we don't replay on startup.
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

/// Read any new lines from a JSONL file since last read and emit events.
fn process_jsonl_file(
    path: &PathBuf,
    offsets: &TailOffsets,
    app: &AppHandle,
    registry: &SessionRegistry,
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

    // File truncated (e.g. /clear created a new file) — reset offset
    if current_len < *offset {
        *offset = 0;

        // Refresh registry after /clear
        if let Ok(sessions) = scan_projects() {
            let mut reg = registry.lock().unwrap_or_else(|e| e.into_inner());
            *reg = sessions;
        }
    }

    let start = *offset;
    drop(offsets_guard); // release lock before I/O

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

    // Update offset (handle partial last line)
    let ends_with_newline = buf.ends_with('\n');
    let lines: Vec<&str> = buf.lines().collect();

    let complete_lines = if ends_with_newline {
        &lines[..]
    } else if lines.len() > 1 {
        &lines[..lines.len() - 1]
    } else {
        return; // Only a partial line — wait for more data
    };

    let processed_bytes: u64 = complete_lines
        .iter()
        .map(|l| l.len() as u64 + 1) // +1 for newline
        .sum();

    {
        let mut offsets_guard = offsets.lock().unwrap_or_else(|e| e.into_inner());
        let offset = offsets_guard.entry(path.clone()).or_insert(start);
        *offset = start + processed_bytes;
    }

    for line in complete_lines {
        if let Some(parsed) = parse_line(&session_id, line) {
            if let Err(e) = app.emit("agent-event", &parsed) {
                warn!("Failed to emit agent-event: {e}");
            }
        }
    }
}
