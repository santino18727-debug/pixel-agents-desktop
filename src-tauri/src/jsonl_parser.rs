use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Parsed representation of a single JSONL agent event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    ToolUse {
        id: String,
        tool: String,
        input: Value,
    },
    ToolResult {
        id: String,
        content: Value,
        is_error: bool,
    },
    Text {
        content: String,
    },
    System {
        subtype: String,
        data: Value,
    },
    Raw {
        raw: String,
    },
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// Emitted when a subagent JSONL contains an init record linking it to a parent session.
    /// Format: {"subtype": "init", "session_id": "...", "parent_session_id": "..."}
    SubagentInit {
        session_id: String,
        parent_session_id: Option<String>,
    },
}

/// A parsed line enriched with its session context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedLine {
    pub session_id: String,
    pub event: AgentEvent,
}

/// Attempt to parse one JSONL line into a ParsedLine.
/// Never panics -- unknown or malformed lines become AgentEvent::Raw.
pub fn parse_line(session_id: &str, line: &str) -> Option<ParsedLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let event = parse_event(trimmed);
    Some(ParsedLine {
        session_id: session_id.to_owned(),
        event,
    })
}

fn parse_event(line: &str) -> AgentEvent {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return AgentEvent::Raw { raw: line.to_owned() };
    };

    let content_block = extract_content_block(&v);
    if let Some(block) = content_block {
        return block;
    }

    if let Some(subtype) = v.get("subtype").and_then(Value::as_str) {
        // P3: detect subagent init records that link a subagent to its parent session.
        // Format: {"subtype": "init", "session_id": "...", "parent_session_id": "..."}
        if subtype == "init" {
            let sid = v
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let parent_sid = v
                .get("parent_session_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            // Only emit SubagentInit when we have at least one session id field,
            // distinguishing it from other "init" subtypes (e.g. Claude Code system init).
            if sid.is_some() || parent_sid.is_some() {
                return AgentEvent::SubagentInit {
                    session_id: sid.unwrap_or_default(),
                    parent_session_id: parent_sid,
                };
            }
        }
        return AgentEvent::System {
            subtype: subtype.to_owned(),
            data: v.clone(),
        };
    }

    AgentEvent::Raw { raw: line.to_owned() }
}

/// Extract a content block from a Claude Code JSONL envelope.
///
/// Real Claude Code format observed in ~/.claude/projects/:
///   { "type": "assistant", "message": { "role": "assistant", "content": [...] }, ... }
///
/// FIX: The previous implementation matched v.get("role") at root, but Claude Code
/// places "role" inside "message", not at the envelope root. The root field is "type".
fn extract_content_block(v: &Value) -> Option<AgentEvent> {
    let msg_type = v.get("type").and_then(Value::as_str)?;

    match msg_type {
        "assistant" => {
            let message = v.get("message")?;
            // Prefer tool/text content over token usage so agentToolStart is never
            // dropped in favour of a TokenUsage event from the same message (C3).
            let content = message.get("content");
            if let Some(c) = content {
                if let Some(event) = extract_from_content(c) {
                    return Some(event);
                }
            }
            // No actionable content -- fall back to token usage if present.
            if let Some(usage) = message.get("usage") {
                let input = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                let output = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                if input > 0 || output > 0 {
                    return Some(AgentEvent::TokenUsage {
                        input_tokens: input,
                        output_tokens: output,
                    });
                }
            }
            None
        }
        "user" => {
            let content = v.get("message")?.get("content")?;
            extract_from_content(content)
        }
        _ => None,
    }
}

fn extract_from_content(content: &Value) -> Option<AgentEvent> {
    match content {
        Value::String(s) => Some(AgentEvent::Text { content: s.clone() }),
        Value::Array(arr) => {
            for block in arr {
                if let Some(event) = extract_from_block(block) {
                    return Some(event);
                }
            }
            None
        }
        _ => None,
    }
}

fn extract_from_block(block: &Value) -> Option<AgentEvent> {
    let block_type = block.get("type").and_then(Value::as_str)?;

    match block_type {
        "tool_use" => {
            let id = block.get("id")?.as_str()?.to_owned();
            let tool = block.get("name")?.as_str()?.to_owned();
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            Some(AgentEvent::ToolUse { id, tool, input })
        }
        "tool_result" => {
            let id = block.get("tool_use_id")?.as_str()?.to_owned();
            let content = block.get("content").cloned().unwrap_or(Value::Null);
            let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            Some(AgentEvent::ToolResult { id, content, is_error })
        }
        "text" => {
            let content = block.get("text")?.as_str()?.to_owned();
            Some(AgentEvent::Text { content })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_line_returns_none() {
        assert!(parse_line("sess-1", "").is_none());
        assert!(parse_line("sess-1", "   ").is_none());
    }

    #[test]
    fn parse_malformed_json_returns_raw() {
        let parsed = parse_line("sess-1", "not json at all").unwrap();
        assert!(matches!(parsed.event, AgentEvent::Raw { .. }));
    }

    // All tests below use the real Claude Code envelope format:
    // { "type": "assistant"|"user", "message": { "content": [...] }, ... }

    #[test]
    fn parse_tool_use_block() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_001","name":"Read","input":{"file_path":"/tmp/foo"}}]},"uuid":"abc"}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        match parsed.event {
            AgentEvent::ToolUse { id, tool, .. } => {
                assert_eq!(id, "tu_001");
                assert_eq!(tool, "Read");
            }
            other => panic!("Expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_result_block() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_001","content":"ok","is_error":false}]},"uuid":"abc"}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        assert!(matches!(parsed.event, AgentEvent::ToolResult { .. }));
    }

    #[test]
    fn parse_text_block() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hello!"}]},"uuid":"abc"}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        match parsed.event {
            AgentEvent::Text { content } => assert_eq!(content, "Hello!"),
            other => panic!("Expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parse_system_subtype() {
        let line = r#"{"subtype":"turn_duration","duration_ms":1234}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        match parsed.event {
            AgentEvent::System { subtype, .. } => assert_eq!(subtype, "turn_duration"),
            other => panic!("Expected System, got {other:?}"),
        }
    }

    #[test]
    fn session_id_propagated() {
        let line = r#"{"subtype":"turn_duration","duration_ms":0}"#;
        let parsed = parse_line("my-session", line).unwrap();
        assert_eq!(parsed.session_id, "my-session");
    }

    #[test]
    fn queue_operation_becomes_raw() {
        // type=queue-operation is not assistant/user -> Raw
        let line = r#"{"type":"queue-operation","operation":"enqueue","sessionId":"s"}"#;
        let parsed = parse_line("sess-1", line).unwrap();
        assert!(matches!(parsed.event, AgentEvent::Raw { .. }));
    }

    #[test]
    fn attachment_line_becomes_raw() {
        let line = r#"{"type":"attachment","attachment":{"type":"deferred_tools_delta"},"uuid":"abc"}"#;
        let parsed = parse_line("sess-1", line).unwrap();
        assert!(matches!(parsed.event, AgentEvent::Raw { .. }));
    }

    #[test]
    fn real_user_plain_string_content() {
        // Content can be a plain string (not array) in user messages
        let line = r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"Bonjour"},"uuid":"b0b2","sessionId":"abc"}"#;
        let parsed = parse_line("abc", line).expect("should parse");
        match parsed.event {
            AgentEvent::Text { content } => assert_eq!(content, "Bonjour"),
            other => panic!("Expected Text for string content, got {other:?}"),
        }
    }

    #[test]
    fn parse_subagent_init_with_parent() {
        let line = r#"{"subtype":"init","session_id":"sub-abc","parent_session_id":"parent-xyz"}"#;
        let parsed = parse_line("sub-abc", line).expect("should parse");
        match parsed.event {
            AgentEvent::SubagentInit { session_id, parent_session_id } => {
                assert_eq!(session_id, "sub-abc");
                assert_eq!(parent_session_id, Some("parent-xyz".to_owned()));
            }
            other => panic!("Expected SubagentInit, got {other:?}"),
        }
    }

    #[test]
    fn parse_subagent_init_without_parent() {
        let line = r#"{"subtype":"init","session_id":"sub-abc"}"#;
        let parsed = parse_line("sub-abc", line).expect("should parse");
        match parsed.event {
            AgentEvent::SubagentInit { session_id, parent_session_id } => {
                assert_eq!(session_id, "sub-abc");
                assert_eq!(parent_session_id, None);
            }
            other => panic!("Expected SubagentInit, got {other:?}"),
        }
    }

    #[test]
    fn parse_generic_init_without_session_fields_stays_system() {
        // An init subtype without session_id or parent_session_id stays as System
        let line = r#"{"subtype":"init","some_other":"field"}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        match parsed.event {
            AgentEvent::System { subtype, .. } => assert_eq!(subtype, "init"),
            other => panic!("Expected System, got {other:?}"),
        }
    }

    #[test]
    fn tool_use_takes_priority_over_token_usage_in_same_message() {
        // C3: when an assistant message contains both usage and tool_use content,
        // ToolUse must win -- never silently dropped in favour of TokenUsage.
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_c3","name":"Read","input":{"file_path":"/tmp/x"}}],"usage":{"input_tokens":100,"output_tokens":50}},"uuid":"abc"}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        match parsed.event {
            AgentEvent::ToolUse { id, tool, .. } => {
                assert_eq!(id, "tu_c3");
                assert_eq!(tool, "Read");
            }
            other => panic!("Expected ToolUse (not TokenUsage), got {other:?}"),
        }
    }

    #[test]
    fn token_usage_emitted_when_no_content() {
        // TokenUsage should be returned when usage is present but content is empty/absent.
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[],"usage":{"input_tokens":200,"output_tokens":80}},"uuid":"abc"}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        match parsed.event {
            AgentEvent::TokenUsage { input_tokens, output_tokens } => {
                assert_eq!(input_tokens, 200);
                assert_eq!(output_tokens, 80);
            }
            other => panic!("Expected TokenUsage, got {other:?}"),
        }
    }
}