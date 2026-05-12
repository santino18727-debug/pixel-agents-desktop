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

use crate::error::MutexExt;
use crate::file_watcher::SessionAgentMap;

const BIND_ADDR: &str = "127.0.0.1:17317";

/// Start the hooks HTTP server. Blocks the calling thread.
/// session_agent_map is shared with the file watcher so session_id -> agent_id
/// resolution is consistent across both ingestion paths.
/// expected_token must be present in the X-Hook-Token request header; requests
/// without it are rejected with 401 to prevent local process injection attacks.
pub fn start(app: AppHandle, session_agent_map: SessionAgentMap, expected_token: String) {
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
                let token = expected_token.clone();
                std::thread::spawn(move || {
                    handle_connection(&mut stream, &app2, &map2, &token);
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
    expected_token: &str,
) {
    // 30-second read timeout prevents slow-loris style hangs.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));

    let mut reader = BufReader::new(stream.try_clone().unwrap_or_else(|_| {
        panic!("hooks_server: stream clone failed");
    }));

    // Parse HTTP request line.
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }

    // Read headers until blank line, collecting Content-Length and X-Hook-Token.
    let mut content_length: usize = 0;
    let mut received_token = String::new();
    loop {
        let mut header_line = String::new();
        if reader.read_line(&mut header_line).is_err() {
            break;
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            break; // end of headers
        }
        let lower = trimmed.to_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            // Cap body size at 1 MiB to prevent memory exhaustion.
            content_length = rest.trim().parse().unwrap_or(0).min(1024 * 1024);
        } else if let Some(rest) = lower.strip_prefix("x-hook-token:") {
            received_token = rest.trim().to_owned();
        }
    }

    // Reject requests with missing or wrong token.
    if received_token != expected_token {
        warn!("hooks_server: rejected request — invalid or missing X-Hook-Token");
        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
        return;
    }

    // Read body.
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        use std::io::Read;
        let _ = reader.read_exact(&mut body);
    }

    // Only handle POST /hook.
    let is_post_hook = request_line.starts_with("POST /hook");

    // Write HTTP response with proper CRLF line endings (RFC 7230).
    let response = if is_post_hook {
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\nok"
    } else {
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"
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
        let map = session_agent_map.lock_or_recover();
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
