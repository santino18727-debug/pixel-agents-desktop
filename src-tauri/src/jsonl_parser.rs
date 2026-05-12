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
}

/// A parsed line enriched with its session context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedLine {
    pub session_id: String,
    pub event: AgentEvent,
}

/// Attempt to parse one JSONL line into a `ParsedLine`.
///
/// Never panics — unknown or malformed lines become `AgentEvent::Raw`.
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
        return AgentEvent::Raw {
            raw: line.to_owned(),
        };
    };

    // Try to extract from assistant/user message wrapper first
    let content_block = extract_content_block(&v);

    if let Some(block) = content_block {
        return block;
    }

    // Try system record
    if let Some(subtype) = v.get("subtype").and_then(Value::as_str) {
        return AgentEvent::System {
            subtype: subtype.to_owned(),
            data: v.clone(),
        };
    }

    AgentEvent::Raw {
        raw: line.to_owned(),
    }
}

fn extract_content_block(v: &Value) -> Option<AgentEvent> {
    let role = v.get("role").and_then(Value::as_str)?;

    match role {
        "assistant" => {
            // Content can be a string or array of blocks
            let content = v.get("message")?.get("content")?;
            extract_from_content(content)
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
            // Take the first meaningful block
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
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(AgentEvent::ToolResult {
                id,
                content,
                is_error,
            })
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
        let result = parse_line("sess-1", "not json at all");
        assert!(result.is_some());
        let parsed = result.unwrap();
        assert!(matches!(parsed.event, AgentEvent::Raw { .. }));
    }

    #[test]
    fn parse_tool_use_block() {
        let line = r#"{"role":"assistant","message":{"content":[{"type":"tool_use","id":"tu_001","name":"Read","input":{"file_path":"/tmp/foo"}}]}}"#;
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
        let line = r#"{"role":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_001","content":"ok","is_error":false}]}}"#;
        let parsed = parse_line("sess-1", line).expect("should parse");
        assert!(matches!(parsed.event, AgentEvent::ToolResult { .. }));
    }

    #[test]
    fn parse_text_block() {
        let line = r#"{"role":"assistant","message":{"content":[{"type":"text","text":"Hello!"}]}}"#;
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
    fn unknown_json_becomes_raw() {
        let line = r#"{"totally":"unknown","structure":true}"#;
        let parsed = parse_line("sess-1", line).unwrap();
        assert!(matches!(parsed.event, AgentEvent::Raw { .. }));
    }
}
