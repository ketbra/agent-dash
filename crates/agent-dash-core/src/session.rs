use serde::{Deserialize, Serialize};

/// Status of a Claude Code session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    NeedsInput,
    Working,
    Idle,
    Ended,
}

impl SessionStatus {
    pub fn sort_key(&self) -> u8 {
        match self {
            SessionStatus::NeedsInput => 0,
            SessionStatus::Working => 1,
            SessionStatus::Idle => 2,
            SessionStatus::Ended => 3,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStatus::NeedsInput => "needs_input",
            SessionStatus::Working => "working",
            SessionStatus::Idle => "idle",
            SessionStatus::Ended => "ended",
        }
    }
}

/// Top-level state (for state.json compat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashState {
    pub sessions: Vec<DashSession>,
}

fn is_zero(v: &usize) -> bool { *v == 0 }

/// A single session in the JSON output (serializable for clients).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashSession {
    pub session_id: String,
    pub project_name: String,
    pub branch: String,
    pub status: String,
    pub last_status_change: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonl_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_reason: Option<DashInputReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tool: Option<DashActiveTool>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub subagent_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_suggestion: Option<String>,
}

/// Why a session needs input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashInputReason {
    #[serde(rename = "type")]
    pub reason_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Active tool info for rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashActiveTool {
    pub name: String,
    pub detail: String,
    pub icon: String,
}

/// Map a Claude tool name to a GNOME symbolic icon name.
pub fn tool_icon(tool_name: &str) -> &'static str {
    match tool_name {
        "Bash" => "utilities-terminal-symbolic",
        "Read" => "document-open-symbolic",
        "Edit" => "document-edit-symbolic",
        "Write" => "document-new-symbolic",
        "Grep" => "edit-find-symbolic",
        "Glob" => "folder-saved-search-symbolic",
        "WebFetch" => "web-browser-symbolic",
        "WebSearch" => "system-search-symbolic",
        "Task" => "system-run-symbolic",
        _ => "applications-system-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_sort_order() {
        assert!(SessionStatus::NeedsInput.sort_key() < SessionStatus::Working.sort_key());
        assert!(SessionStatus::Working.sort_key() < SessionStatus::Idle.sort_key());
        assert!(SessionStatus::Idle.sort_key() < SessionStatus::Ended.sort_key());
    }

    #[test]
    fn status_display_strings() {
        assert_eq!(SessionStatus::NeedsInput.as_str(), "needs_input");
        assert_eq!(SessionStatus::Working.as_str(), "working");
        assert_eq!(SessionStatus::Idle.as_str(), "idle");
        assert_eq!(SessionStatus::Ended.as_str(), "ended");
    }

    #[test]
    fn dash_session_serialize_minimal() {
        let ds = DashSession {
            session_id: "abc".into(),
            project_name: "myproject".into(),
            branch: "main".into(),
            status: "idle".into(),
            last_status_change: 1000,
            jsonl_path: None,
            input_reason: None,
            active_tool: None,
            subagent_count: 0,
            prompt_suggestion: None,
        };
        let json = serde_json::to_string(&ds).unwrap();
        assert!(json.contains("\"status\":\"idle\""));
        assert!(!json.contains("input_reason"));
        assert!(!json.contains("active_tool"));
        assert!(!json.contains("subagent_count"));
    }

    #[test]
    fn dash_session_serialize_with_tool() {
        let ds = DashSession {
            session_id: "abc".into(),
            project_name: "myproject".into(),
            branch: "main".into(),
            status: "working".into(),
            last_status_change: 1000,
            jsonl_path: None,
            input_reason: None,
            active_tool: Some(DashActiveTool {
                name: "Bash".into(),
                detail: "cargo test".into(),
                icon: "utilities-terminal-symbolic".into(),
            }),
            subagent_count: 0,
            prompt_suggestion: None,
        };
        let json = serde_json::to_string(&ds).unwrap();
        assert!(json.contains("\"name\":\"Bash\""));
    }

    #[test]
    fn tool_icon_known_tools() {
        assert_eq!(tool_icon("Bash"), "utilities-terminal-symbolic");
        assert_eq!(tool_icon("Read"), "document-open-symbolic");
        assert_eq!(tool_icon("Edit"), "document-edit-symbolic");
    }

    #[test]
    fn tool_icon_unknown_fallback() {
        assert_eq!(tool_icon("SomeFutureTool"), "applications-system-symbolic");
    }

    #[test]
    fn dash_session_roundtrip_json() {
        let ds = DashSession {
            session_id: "abc".into(),
            project_name: "proj".into(),
            branch: "feat".into(),
            status: "working".into(),
            last_status_change: 999,
            jsonl_path: Some("/tmp/test.jsonl".into()),
            input_reason: Some(DashInputReason {
                reason_type: "permission".into(),
                tool: Some("Bash".into()),
                command: Some("ls".into()),
                detail: Some("detail".into()),
                text: None,
            }),
            active_tool: None,
            subagent_count: 0,
            prompt_suggestion: None,
        };
        let json = serde_json::to_string(&ds).unwrap();
        let parsed: DashSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "abc");
        assert_eq!(parsed.input_reason.unwrap().tool.unwrap(), "Bash");
    }
}
