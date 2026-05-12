//! Minimal HTTP server for Claude Code Hooks API.
//!
//! Listens on 127.0.0.1:17317 for POST /hook requests.
//! Translates Claude Code hook payloads into frontend agent-event emissions.
//!
//! Supported hook types:
//!   PreToolUse    : { session_id, tool_name, tool_input }
//!   PostToolUse   : { session_id, tool_name, tool_response }
//!   Stop          : { session_id }
//!   Notification  : { session_id, message }

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::file_watcher::SessionAgentMap;

const BIND_ADDR: &str = "127.0.0.1:17317";

/// Start the hooks HTTP server. Blocks the calling thread.
/// session_agent_map is shared with the file watcher so session_id -> agent_id
/// resolution is consistent across both ingestion paths.
pub fn start(app: AppHandle, session_agent_map: SessionAgentMap) {
    let listener = match TcpListener::bind(BIND_ADDR) {
        Ok(l) => l,
        Err(e) => {
            warn!("hooks_server: failed to bind {BIND_ADDR}: {e}");
            return;
        }
    };
    info!("hooks_server: listening on {BIND_ADDR}");

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let app2 = app.clone();
                let map2 = Arc::clone(&session_agent_map);
                std::thread::spawn(move || {
                    handle_connection(&mut stream, &app2, &map2);
                });
            }
            Err(e) => warn!("hooks_server: accept error: {e}"),
        }
    }
}

fn handle_connection(
    stream: &mut std::net::TcpStream,
    app: &AppHandle,
    session_agent_map: &SessionAgentMap,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap_or_else(|_| {
        // If clone fails we cannot respond; just return.
        panic!("hooks_server: stream clone failed");
    }));

    // Parse HTTP request line.
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }

    // Read headers until blank line, collecting Content-Length.
    let mut content_length: usize = 0;
    loop {
        let mut header_line = String::new();
        if reader.read_line(&mut header_line).is_err() {
            break;
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.to_lowercase().strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }

    // Read body.
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        use std::io::Read;
        let _ = reader.read_exact(&mut body);
    }

    // Only handle POST /hook.
    let is_post_hook = request_line.starts_with("POST /hook");

    // Write HTTP response.
    let response = if is_post_hook {
        "HTTP/1.1 200 OK
Content-Length: 2
Content-Type: application/json

ok"
    } else {
        "HTTP/1.1 404 Not Found
Content-Length: 0

"
    };
    let _ = stream.write_all(response.as_bytes());

    if !is_post_hook {
        return;
    }

    // Parse JSON body.
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        warn!("hooks_server: invalid JSON body");
        return;
    };

    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    let hook_type = payload
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    // Resolve session_id -> agent_id from the shared map.
    let agent_id = {
        let map = session_agent_map.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(&session_id).copied() {
            Some(id) => id,
            None => {
                warn!("hooks_server: unknown session_id {session_id:?}");
                return;
            }
        }
    };

    let msg: Option<Value> = match hook_type.as_str() {
        "PreToolUse" => {
            let tool = payload
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Some(json!({
                "type": "agentToolStart",
                "id": agent_id,
                "toolId": format!("hook-{}", session_id),
                "status": format!("Using {}...", tool),
                "toolName": tool,
            }))
        }
        "PostToolUse" => {
            let tool_id = format!("hook-{}", session_id);
            Some(json!({
                "type": "agentToolDone",
                "id": agent_id,
                "toolId": tool_id,
            }))
        }
        "Stop" => {
            Some(json!({
                "type": "agentToolsClear",
                "id": agent_id,
            }))
        }
        "Notification" => {
            let _message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("");
            // Map notification -> agentStatus active so the character animates.
            Some(json!({
                "type": "agentStatus",
                "id": agent_id,
                "status": "active",
            }))
        }
        other => {
            warn!("hooks_server: unknown hook_event_name {other:?}");
            None
        }
    };

    if let Some(msg) = msg {
        if let Err(e) = app.emit("agent-event", &msg) {
            warn!("hooks_server: failed to emit agent-event: {e}");
        }
    }
}
