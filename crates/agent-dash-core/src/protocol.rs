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

// ---------------------------------------------------------------------------
// Client requests (client -> daemon, via daemon.sock)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ClientRequest {
    #[serde(rename = "subscribe")]
    Subscribe,
    #[serde(rename = "get_state")]
    GetState,
    #[serde(rename = "permission_response")]
    PermissionResponse {
        request_id: String,
        session_id: String,
        decision: String,
    },
    #[serde(rename = "permission_request")]
    PermissionRequest {
        request_id: String,
        session_id: String,
        tool: String,
        detail: String,
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
    },
    #[serde(rename = "permission_resolved")]
    PermissionResolved {
        request_id: String,
        resolved_by: String,
    },
}

// ---------------------------------------------------------------------------
// Permission decision sent back to the hook
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPermissionDecision {
    pub request_id: String,
    pub decision: String,
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
        assert!(matches!(req, ClientRequest::GetState));
    }

    #[test]
    fn deserialize_permission_response() {
        let json = r#"{"method":"permission_response","request_id":"tu1","session_id":"s1","decision":"allow"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::PermissionResponse { request_id, session_id, decision } => {
                assert_eq!(request_id, "tu1");
                assert_eq!(session_id, "s1");
                assert_eq!(decision, "allow");
            }
            _ => panic!("expected PermissionResponse"),
        }
    }

    #[test]
    fn deserialize_permission_request() {
        let json = r#"{"method":"permission_request","request_id":"tu1","session_id":"s1","tool":"Bash","detail":"rm -rf /tmp"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::PermissionRequest { .. }));
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
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"request_id\":\"tu1\""));
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
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("\"decision\":\"allow\""));
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
}
