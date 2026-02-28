use crate::session::DashSession;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Hook events (hook -> daemon, via hook.sock)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum HookEvent {
    #[serde(rename = "tool_start")]
    ToolStart {
        session_id: String,
        tool: String,
        detail: String,
        tool_use_id: String,
    },
    #[serde(rename = "tool_end")]
    ToolEnd {
        session_id: String,
        tool_use_id: String,
    },
    #[serde(rename = "stop")]
    Stop { session_id: String },
    #[serde(rename = "session_start")]
    SessionStart {
        session_id: String,
        #[serde(default)]
        cwd: Option<String>,
    },
    #[serde(rename = "session_end")]
    SessionEnd { session_id: String },
}

/// Envelope wrapping a HookEvent with optional context that applies to all
/// event types. The `wrapper_id` is set when the hook runs inside a session
/// launched via `agent-dash run`, allowing the daemon to link the real
/// session_id to the wrapper's prompt channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEnvelope {
    #[serde(flatten)]
    pub event: HookEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Image attachment (for prompt injection with images)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub mime_type: String,
    pub data: String, // base64-encoded
}

// ---------------------------------------------------------------------------
// Client requests (client -> daemon, via daemon.sock)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ClientRequest {
    #[serde(rename = "subscribe")]
    Subscribe,
    #[serde(rename = "get_state")]
    GetState {
        #[serde(default)]
        include_subagents: bool,
    },
    #[serde(rename = "permission_response")]
    PermissionResponse {
        request_id: String,
        session_id: String,
        decision: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suggestion: Option<serde_json::Value>,
    },
    #[serde(rename = "permission_request")]
    PermissionRequest {
        request_id: String,
        session_id: String,
        tool: String,
        detail: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        suggestions: Vec<serde_json::Value>,
    },
    #[serde(rename = "get_messages")]
    GetMessages {
        session_id: String,
        #[serde(default)]
        format: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    #[serde(rename = "watch_session")]
    WatchSession {
        session_id: String,
        #[serde(default)]
        format: Option<String>,
    },
    #[serde(rename = "unwatch_session")]
    UnwatchSession {
        session_id: String,
    },
    #[serde(rename = "list_sessions")]
    ListSessions {
        project: String,
    },
    #[serde(rename = "register_wrapper")]
    RegisterWrapper {
        session_id: String,
        agent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        real_session_id: Option<String>,
    },
    #[serde(rename = "unregister_wrapper")]
    UnregisterWrapper {
        session_id: String,
    },
    #[serde(rename = "send_prompt")]
    SendPrompt {
        session_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageAttachment>,
    },
    #[serde(rename = "update_suggestion")]
    UpdateSuggestion {
        session_id: String,
        suggestion: Option<String>,
    },
    #[serde(rename = "update_thinking_text")]
    UpdateThinkingText {
        session_id: String,
        thinking_text: Option<String>,
    },
    #[serde(rename = "watch_terminal")]
    WatchTerminal {
        session_id: String,
    },
    #[serde(rename = "unwatch_terminal")]
    UnwatchTerminal {
        session_id: String,
    },
    #[serde(rename = "terminal_output")]
    TerminalOutput {
        session_id: String,
        data: String, // base64-encoded raw PTY bytes
    },
    #[serde(rename = "create_session")]
    CreateSession {
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        cols: Option<u16>,
        #[serde(default)]
        rows: Option<u16>,
    },
    #[serde(rename = "terminal_input")]
    TerminalInput {
        session_id: String,
        data: String, // base64-encoded raw bytes
    },
    #[serde(rename = "terminal_resize")]
    TerminalResize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
}

// ---------------------------------------------------------------------------
// Server events (daemon -> client, via daemon.sock)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum ServerEvent {
    #[serde(rename = "state_update")]
    StateUpdate { sessions: Vec<DashSession> },
    #[serde(rename = "permission_pending")]
    PermissionPending {
        session_id: String,
        request_id: String,
        tool: String,
        detail: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        suggestions: Vec<serde_json::Value>,
    },
    #[serde(rename = "permission_resolved")]
    PermissionResolved {
        request_id: String,
        resolved_by: String,
    },
    #[serde(rename = "messages")]
    Messages {
        session_id: String,
        messages: Vec<ChatMessage>,
    },
    #[serde(rename = "message")]
    Message {
        session_id: String,
        message: ChatMessage,
    },
    #[serde(rename = "session_list")]
    SessionList {
        project: String,
        sessions: Vec<SessionListEntry>,
    },
    #[serde(rename = "prompt_sent")]
    PromptSent {
        session_id: String,
    },
    #[serde(rename = "inject_prompt")]
    InjectPrompt {
        text: String,
    },
    #[serde(rename = "terminal_data")]
    TerminalData {
        session_id: String,
        data: String, // base64-encoded raw PTY bytes
    },
    #[serde(rename = "terminal_write")]
    TerminalWrite {
        data: String, // base64-encoded raw bytes to write to PTY
    },
    #[serde(rename = "terminal_resize")]
    TerminalResize {
        cols: u16,
        rows: u16,
    },
    #[serde(rename = "session_created")]
    SessionCreated {
        session_id: String,
    },
    /// Sent to a wrapper when a new terminal watcher connects, telling it to
    /// force a redraw so the child TUI re-renders at the correct dimensions.
    #[serde(rename = "force_redraw")]
    ForceRedraw,
    #[serde(rename = "error")]
    Error {
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Permission decision sent back to the hook
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPermissionDecision {
    pub request_id: String,
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Chat message types (for message API)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: ChatContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatContent {
    Structured(Vec<ContentBlock>),
    Rendered(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListEntry {
    pub session_id: String,
    pub main: bool,
    pub modified: u64,
}

// ---------------------------------------------------------------------------
// Line-delimited JSON helpers
// ---------------------------------------------------------------------------

pub fn encode_line<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

pub fn decode_line<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Hook events --

    #[test]
    fn deserialize_hook_tool_start() {
        let json = r#"{"event":"tool_start","session_id":"s1","tool":"Bash","detail":"ls","tool_use_id":"tu1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::ToolStart { session_id, tool, detail, tool_use_id } => {
                assert_eq!(session_id, "s1");
                assert_eq!(tool, "Bash");
                assert_eq!(detail, "ls");
                assert_eq!(tool_use_id, "tu1");
            }
            _ => panic!("expected ToolStart"),
        }
    }

    #[test]
    fn deserialize_hook_tool_end() {
        let json = r#"{"event":"tool_end","session_id":"s1","tool_use_id":"tu1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::ToolEnd { .. }));
    }

    #[test]
    fn deserialize_hook_stop() {
        let json = r#"{"event":"stop","session_id":"s1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::Stop { .. }));
    }

    #[test]
    fn deserialize_hook_session_start() {
        let json = r#"{"event":"session_start","session_id":"s1","cwd":"/home/user"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::SessionStart { .. }));
    }

    #[test]
    fn deserialize_hook_session_end() {
        let json = r#"{"event":"session_end","session_id":"s1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::SessionEnd { .. }));
    }

    // -- Client requests --

    #[test]
    fn deserialize_subscribe() {
        let json = r#"{"method":"subscribe"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::Subscribe));
    }

    #[test]
    fn deserialize_get_state() {
        let json = r#"{"method":"get_state"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::GetState { .. }));
    }

    #[test]
    fn deserialize_permission_response() {
        let json = r#"{"method":"permission_response","request_id":"tu1","session_id":"s1","decision":"allow"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::PermissionResponse { request_id, session_id, decision, suggestion } => {
                assert_eq!(request_id, "tu1");
                assert_eq!(session_id, "s1");
                assert_eq!(decision, "allow");
                assert!(suggestion.is_none());
            }
            _ => panic!("expected PermissionResponse"),
        }
    }

    #[test]
    fn deserialize_permission_response_with_suggestion() {
        let json = r#"{"method":"permission_response","request_id":"tu1","session_id":"s1","decision":"allow","suggestion":{"type":"toolAlwaysAllow","tool":"Bash"}}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::PermissionResponse { suggestion, .. } => {
                let s = suggestion.unwrap();
                assert_eq!(s["type"], "toolAlwaysAllow");
                assert_eq!(s["tool"], "Bash");
            }
            _ => panic!("expected PermissionResponse"),
        }
    }

    #[test]
    fn deserialize_permission_request() {
        let json = r#"{"method":"permission_request","request_id":"tu1","session_id":"s1","tool":"Bash","detail":"rm -rf /tmp"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::PermissionRequest { suggestions, .. } => {
                assert!(suggestions.is_empty());
            }
            _ => panic!("expected PermissionRequest"),
        }
    }

    #[test]
    fn deserialize_permission_request_with_suggestions() {
        let json = r#"{"method":"permission_request","request_id":"tu1","session_id":"s1","tool":"Bash","detail":"rm -rf /tmp","suggestions":[{"type":"toolAlwaysAllow","tool":"Bash"}]}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::PermissionRequest { suggestions, .. } => {
                assert_eq!(suggestions.len(), 1);
                assert_eq!(suggestions[0]["type"], "toolAlwaysAllow");
            }
            _ => panic!("expected PermissionRequest"),
        }
    }

    // -- Server events --

    #[test]
    fn serialize_state_update() {
        let event = ServerEvent::StateUpdate { sessions: vec![] };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"state_update\""));
    }

    #[test]
    fn serialize_permission_pending() {
        let event = ServerEvent::PermissionPending {
            session_id: "s1".into(),
            request_id: "tu1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            suggestions: vec![],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"request_id\":\"tu1\""));
        // Empty suggestions should be omitted
        assert!(!json.contains("suggestions"));
    }

    #[test]
    fn serialize_permission_pending_with_suggestions() {
        let suggestion = serde_json::json!({"type": "toolAlwaysAllow", "tool": "Bash"});
        let event = ServerEvent::PermissionPending {
            session_id: "s1".into(),
            request_id: "tu1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            suggestions: vec![suggestion],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"suggestions\""));
        assert!(json.contains("toolAlwaysAllow"));
    }

    #[test]
    fn serialize_permission_resolved() {
        let event = ServerEvent::PermissionResolved {
            request_id: "tu1".into(),
            resolved_by: "terminal".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"resolved_by\":\"terminal\""));
    }

    // -- Permission decision for hook response --

    #[test]
    fn serialize_hook_permission_decision_allow() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "allow".into(),
            suggestion: None,
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("\"decision\":\"allow\""));
        // suggestion: None should be omitted
        assert!(!json.contains("suggestion"));
    }

    #[test]
    fn serialize_hook_permission_decision_with_suggestion() {
        let suggestion = serde_json::json!({"type": "toolAlwaysAllow", "tool": "Bash"});
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "allow".into(),
            suggestion: Some(suggestion),
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("\"suggestion\""));
        assert!(json.contains("toolAlwaysAllow"));
    }

    // -- Line-delimited encoding --

    #[test]
    fn encode_line_appends_newline() {
        let event = ServerEvent::StateUpdate { sessions: vec![] };
        let line = encode_line(&event).unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
    }

    #[test]
    fn decode_line_parses_json() {
        let json = r#"{"method":"subscribe"}"#;
        let req: ClientRequest = decode_line(json).unwrap();
        assert!(matches!(req, ClientRequest::Subscribe));
    }

    #[test]
    fn decode_line_trims_whitespace() {
        let json = "  {\"method\":\"subscribe\"}  \n";
        let req: ClientRequest = decode_line(json).unwrap();
        assert!(matches!(req, ClientRequest::Subscribe));
    }

    // -- New message/session request types --

    #[test]
    fn deserialize_get_messages() {
        let json = r#"{"method":"get_messages","session_id":"s1","format":"html","limit":20}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::GetMessages { session_id, format, limit } => {
                assert_eq!(session_id, "s1");
                assert_eq!(format.as_deref(), Some("html"));
                assert_eq!(limit, Some(20));
            }
            _ => panic!("expected GetMessages"),
        }
    }

    #[test]
    fn deserialize_get_messages_defaults() {
        let json = r#"{"method":"get_messages","session_id":"s1"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::GetMessages { format, limit, .. } => {
                assert!(format.is_none());
                assert!(limit.is_none());
            }
            _ => panic!("expected GetMessages"),
        }
    }

    #[test]
    fn deserialize_watch_session() {
        let json = r#"{"method":"watch_session","session_id":"s1","format":"markdown"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::WatchSession { .. }));
    }

    #[test]
    fn deserialize_unwatch_session() {
        let json = r#"{"method":"unwatch_session","session_id":"s1"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::UnwatchSession { .. }));
    }

    #[test]
    fn deserialize_list_sessions() {
        let json = r#"{"method":"list_sessions","project":"traider"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::ListSessions { project } => assert_eq!(project, "traider"),
            _ => panic!("expected ListSessions"),
        }
    }

    // -- New message/session server events --

    #[test]
    fn serialize_messages_event() {
        let msg = ChatMessage {
            role: "assistant".into(),
            content: ChatContent::Structured(vec![
                ContentBlock::Text { text: "hello".into() },
            ]),
        };
        let event = ServerEvent::Messages {
            session_id: "s1".into(),
            messages: vec![msg],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"messages\""));
        assert!(json.contains("\"role\":\"assistant\""));
    }

    #[test]
    fn serialize_message_event() {
        let msg = ChatMessage {
            role: "user".into(),
            content: ChatContent::Rendered("hello".into()),
        };
        let event = ServerEvent::Message {
            session_id: "s1".into(),
            message: msg,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"message\""));
    }

    #[test]
    fn serialize_session_list() {
        let entry = SessionListEntry {
            session_id: "abc".into(),
            main: true,
            modified: 1000,
        };
        let event = ServerEvent::SessionList {
            project: "traider".into(),
            sessions: vec![entry],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"main\":true"));
    }

    // -- Wrapper registration and prompt injection --

    #[test]
    fn deserialize_register_wrapper() {
        let json = r#"{"method":"register_wrapper","session_id":"s1","agent":"claude"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::RegisterWrapper { session_id, agent, .. } => {
                assert_eq!(session_id, "s1");
                assert_eq!(agent, "claude");
            }
            _ => panic!("expected RegisterWrapper"),
        }
    }

    #[test]
    fn deserialize_unregister_wrapper() {
        let json = r#"{"method":"unregister_wrapper","session_id":"s1"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::UnregisterWrapper { .. }));
    }

    #[test]
    fn deserialize_send_prompt() {
        let json = r#"{"method":"send_prompt","session_id":"s1","text":"fix the tests"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::SendPrompt { session_id, text, images } => {
                assert_eq!(session_id, "s1");
                assert_eq!(text, "fix the tests");
                assert!(images.is_empty());
            }
            _ => panic!("expected SendPrompt"),
        }
    }

    #[test]
    fn deserialize_send_prompt_with_images() {
        let json = r#"{"method":"send_prompt","session_id":"s1","text":"look at this","images":[{"mime_type":"image/png","data":"iVBOR..."}]}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::SendPrompt { images, .. } => {
                assert_eq!(images.len(), 1);
                assert_eq!(images[0].mime_type, "image/png");
            }
            _ => panic!("expected SendPrompt"),
        }
    }

    #[test]
    fn serialize_prompt_sent() {
        let event = ServerEvent::PromptSent { session_id: "s1".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"prompt_sent\""));
    }

    #[test]
    fn serialize_inject_prompt() {
        let event = ServerEvent::InjectPrompt { text: "hello".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"inject_prompt\""));
        assert!(json.contains("\"text\":\"hello\""));
    }

    #[test]
    fn serialize_error() {
        let event = ServerEvent::Error { message: "not wrapped".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"error\""));
    }

    // -- RegisterWrapper with metadata --

    #[test]
    fn deserialize_register_wrapper_with_metadata() {
        let json = r#"{"method":"register_wrapper","session_id":"wrap-1","agent":"claude","cwd":"/home/user/project","branch":"main","project_name":"project"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::RegisterWrapper { session_id, agent, cwd, branch, project_name, .. } => {
                assert_eq!(session_id, "wrap-1");
                assert_eq!(agent, "claude");
                assert_eq!(cwd.as_deref(), Some("/home/user/project"));
                assert_eq!(branch.as_deref(), Some("main"));
                assert_eq!(project_name.as_deref(), Some("project"));
            }
            _ => panic!("expected RegisterWrapper"),
        }
    }

    #[test]
    fn deserialize_register_wrapper_backwards_compat() {
        let json = r#"{"method":"register_wrapper","session_id":"s1","agent":"claude"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::RegisterWrapper { cwd, branch, project_name, real_session_id, .. } => {
                assert!(cwd.is_none());
                assert!(branch.is_none());
                assert!(project_name.is_none());
                assert!(real_session_id.is_none());
            }
            _ => panic!("expected RegisterWrapper"),
        }
    }

    #[test]
    fn deserialize_get_state_with_include_subagents() {
        let json = r#"{"method":"get_state","include_subagents":true}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::GetState { include_subagents } => {
                assert!(include_subagents);
            }
            _ => panic!("expected GetState"),
        }
    }

    #[test]
    fn deserialize_get_state_backwards_compat() {
        let json = r#"{"method":"get_state"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::GetState { include_subagents } => {
                assert!(!include_subagents);
            }
            _ => panic!("expected GetState"),
        }
    }

    // -- HookEnvelope round-trip (flattened serde) --

    #[test]
    fn deserialize_update_thinking_text() {
        let json = r#"{"method":"update_thinking_text","session_id":"s1","thinking_text":"· Pouncing… (thinking)"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::UpdateThinkingText { session_id, thinking_text } => {
                assert_eq!(session_id, "s1");
                assert_eq!(thinking_text.as_deref(), Some("· Pouncing… (thinking)"));
            }
            _ => panic!("expected UpdateThinkingText"),
        }
    }

    #[test]
    fn deserialize_update_thinking_text_null() {
        let json = r#"{"method":"update_thinking_text","session_id":"s1","thinking_text":null}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::UpdateThinkingText { thinking_text, .. } => {
                assert!(thinking_text.is_none());
            }
            _ => panic!("expected UpdateThinkingText"),
        }
    }

    #[test]
    fn hook_envelope_round_trip_without_wrapper_id() {
        let envelope = HookEnvelope {
            event: HookEvent::ToolStart {
                session_id: "s1".into(),
                tool: "Bash".into(),
                detail: "ls".into(),
                tool_use_id: "tu1".into(),
            },
            wrapper_id: None,
        };
        let json = serde_json::to_string(&envelope).unwrap();
        // wrapper_id should be absent (skip_serializing_if = "Option::is_none")
        assert!(!json.contains("wrapper_id"));
        // Round-trip back
        let decoded: HookEnvelope = serde_json::from_str(&json).unwrap();
        assert!(decoded.wrapper_id.is_none());
        match decoded.event {
            HookEvent::ToolStart { session_id, tool, .. } => {
                assert_eq!(session_id, "s1");
                assert_eq!(tool, "Bash");
            }
            _ => panic!("expected ToolStart"),
        }
    }

    #[test]
    fn hook_envelope_round_trip_with_wrapper_id() {
        let envelope = HookEnvelope {
            event: HookEvent::Stop { session_id: "s1".into() },
            wrapper_id: Some("wrap-123".into()),
        };
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("\"wrapper_id\":\"wrap-123\""));
        let decoded: HookEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.wrapper_id.as_deref(), Some("wrap-123"));
        assert!(matches!(decoded.event, HookEvent::Stop { .. }));
    }

    #[test]
    fn hook_envelope_encode_decode_line() {
        let envelope = HookEnvelope {
            event: HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: Some("/home/user".into()),
            },
            wrapper_id: Some("wrap-456".into()),
        };
        let line = encode_line(&envelope).unwrap();
        assert!(line.ends_with('\n'));
        let decoded: HookEnvelope = decode_line(&line).unwrap();
        assert_eq!(decoded.wrapper_id.as_deref(), Some("wrap-456"));
        match decoded.event {
            HookEvent::SessionStart { session_id, cwd } => {
                assert_eq!(session_id, "s1");
                assert_eq!(cwd.as_deref(), Some("/home/user"));
            }
            _ => panic!("expected SessionStart"),
        }
    }

    #[test]
    fn hook_envelope_deserialize_without_wrapper_id_field() {
        // Simulate a JSON payload that omits wrapper_id entirely.
        let json = r#"{"event":"stop","session_id":"s1"}"#;
        let decoded: HookEnvelope = serde_json::from_str(json).unwrap();
        assert!(decoded.wrapper_id.is_none());
        assert!(matches!(decoded.event, HookEvent::Stop { .. }));
    }

    // -- CreateSession / TerminalInput --

    #[test]
    fn deserialize_create_session_defaults() {
        let json = r#"{"method":"create_session"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::CreateSession { agent, cwd, cols, rows } => {
                assert!(agent.is_none());
                assert!(cwd.is_none());
                assert!(cols.is_none());
                assert!(rows.is_none());
            }
            _ => panic!("expected CreateSession"),
        }
    }

    #[test]
    fn deserialize_create_session_with_options() {
        let json = r#"{"method":"create_session","agent":"claude","cwd":"/home/user/project","cols":120,"rows":36}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::CreateSession { agent, cwd, cols, rows } => {
                assert_eq!(agent.as_deref(), Some("claude"));
                assert_eq!(cwd.as_deref(), Some("/home/user/project"));
                assert_eq!(cols, Some(120));
                assert_eq!(rows, Some(36));
            }
            _ => panic!("expected CreateSession"),
        }
    }

    #[test]
    fn deserialize_terminal_input() {
        let json = r#"{"method":"terminal_input","session_id":"s1","data":"aGVsbG8="}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::TerminalInput { session_id, data } => {
                assert_eq!(session_id, "s1");
                assert_eq!(data, "aGVsbG8=");
            }
            _ => panic!("expected TerminalInput"),
        }
    }

    #[test]
    fn deserialize_terminal_resize() {
        let json = r#"{"method":"terminal_resize","session_id":"s1","cols":120,"rows":40}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::TerminalResize { session_id, cols, rows } => {
                assert_eq!(session_id, "s1");
                assert_eq!(cols, 120);
                assert_eq!(rows, 40);
            }
            _ => panic!("expected TerminalResize"),
        }
    }

    #[test]
    fn serialize_terminal_resize_event() {
        let event = ServerEvent::TerminalResize { cols: 80, rows: 24 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"terminal_resize\""));
        assert!(json.contains("\"cols\":80"));
        assert!(json.contains("\"rows\":24"));
    }

    #[test]
    fn serialize_session_created() {
        let event = ServerEvent::SessionCreated { session_id: "web-123".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"session_created\""));
        assert!(json.contains("\"session_id\":\"web-123\""));
    }

    #[test]
    fn serialize_terminal_write() {
        let event = ServerEvent::TerminalWrite { data: "aGVsbG8=".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"terminal_write\""));
        assert!(json.contains("\"data\":\"aGVsbG8=\""));
    }
}
