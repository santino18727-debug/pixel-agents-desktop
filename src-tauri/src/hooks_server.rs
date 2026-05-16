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

/// Constant-time string comparison to prevent timing attacks against the
/// hook token. Always scans the full length of the shorter input even when
/// lengths differ, so an attacker cannot use timing to learn the secret
/// byte-by-byte.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Returns true if the Host header value is one of the expected local origins.
/// This blocks DNS rebinding attacks where a remote page resolves an attacker
/// domain to 127.0.0.1 to reach our localhost-bound server.
fn is_valid_host(host: Option<&str>) -> bool {
    matches!(
        host,
        Some("127.0.0.1:17317") | Some("localhost:17317")
    )
}

/// Start the hooks HTTP server. Blocks the calling thread.
/// session_agent_map is shared with the file watcher so session_id -> agent_id
/// resolution is consistent across both ingestion paths.
/// expected_token must be present in the X-Hook-Token request header; requests
/// without it are rejected with 401 to prevent local process injection attacks.
pub fn start(app: AppHandle, session_agent_map: SessionAgentMap, expected_token: String) {
    let listener = match TcpListener::bind(BIND_ADDR) {
        Ok(l) => l,
        Err(e) => {
            // Log at error level — hooks won't work, user should know.
            tracing::error!(
                "hooks_server: failed to bind {BIND_ADDR}: {e}. \
                 Check that no other process is using port 17317."
            );
            return;
        }
    };
    info!("hooks_server: listening on {BIND_ADDR}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let app2 = app.clone();
                let map2 = Arc::clone(&session_agent_map);
                let token = expected_token.clone();
                std::thread::spawn(move || {
                    handle_connection(stream, &app2, &map2, &token);
                });
            }
            Err(e) => warn!("hooks_server: accept error: {e}"),
        }
    }
}

/// Handle one incoming HTTP connection.
/// Takes ownership of the stream to avoid try_clone and the associated panic risk.
fn handle_connection(
    stream: std::net::TcpStream,
    app: &AppHandle,
    session_agent_map: &SessionAgentMap,
    expected_token: &str,
) {
    // 30-second read timeout prevents slow-loris style hangs.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));

    // Clone the stream for writing; if this fails, log and bail — never panic.
    let write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            warn!("hooks_server: stream clone failed: {e}");
            return;
        }
    };

    let mut writer = std::io::BufWriter::new(write_stream);
    let mut reader = BufReader::new(stream);

    // Parse HTTP request line.
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }

    // Read headers until blank line, collecting Content-Length, X-Hook-Token and Host.
    let mut content_length: usize = 0;
    let mut received_token = String::new();
    let mut host_header: Option<String> = None;
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
        } else if let Some(rest) = lower.strip_prefix("host:") {
            host_header = Some(rest.trim().to_owned());
        }
    }

    // Reject requests with an invalid Host header to defend against DNS rebinding.
    if !is_valid_host(host_header.as_deref()) {
        warn!(
            "hooks_server: rejected request with invalid Host: {host_header:?}"
        );
        let _ = writer.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        return;
    }

    // Reject requests with missing or wrong token.
    // Constant-time comparison to prevent timing attacks.
    if !constant_time_eq(&received_token, expected_token) {
        warn!("hooks_server: rejected request — invalid or missing X-Hook-Token");
        let _ = writer.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
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
    // Body is valid JSON: "ok" (4 bytes with quotes).
    let response = if is_post_hook {
        "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nContent-Type: application/json\r\n\r\n\"ok\""
    } else {
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"
    };
    let _ = writer.write_all(response.as_bytes());
    let _ = writer.flush();

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
            let tool_input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
            Some(json!({
                "type": "agentToolStart",
                "id": agent_id,
                "toolId": format!("hook-{}", session_id),
                "status": format!("Using {}...", tool),
                "toolName": tool,
                "toolInput": tool_input,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_equal_strings() {
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("abcd", "abc"));
    }

    #[test]
    fn constant_time_eq_same_length_diff() {
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("aaaaa", "bbbbb"));
    }

    #[test]
    fn is_valid_host_accepts_expected() {
        assert!(is_valid_host(Some("127.0.0.1:17317")));
        assert!(is_valid_host(Some("localhost:17317")));
    }

    #[test]
    fn is_valid_host_rejects_others() {
        assert!(!is_valid_host(None));
        assert!(!is_valid_host(Some("evil.com:17317")));
        assert!(!is_valid_host(Some("127.0.0.1:80")));
        assert!(!is_valid_host(Some("0.0.0.0:17317")));
    }
}

