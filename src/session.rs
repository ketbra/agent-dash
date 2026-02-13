use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;

/// Status of a Claude Code session, ordered by priority (Red > Yellow > Green > Grey).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Needs user input (permission prompt or AskUserQuestion)
    NeedsInput,
    /// Actively working (JSONL recently modified)
    Working,
    /// Idle at prompt
    Idle,
    /// Process ended, session will be removed soon
    Ended,
}

impl SessionStatus {
    /// Sort priority: lower number = higher priority (floats to top).
    pub fn sort_key(&self) -> u8 {
        match self {
            SessionStatus::NeedsInput => 0,
            SessionStatus::Working => 1,
            SessionStatus::Idle => 2,
            SessionStatus::Ended => 3,
        }
    }
}

/// A pending permission request from the hook IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub session_id: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub timestamp: u64,
}

/// Info about why a session needs input.
#[derive(Debug, Clone)]
pub enum InputReason {
    Permission(PermissionRequest),
    Question { text: String },
}

/// A monitored Claude Code session.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub pid: i32,
    pub pty: PathBuf,
    pub cwd: PathBuf,
    pub project_name: String,
    pub branch: String,
    pub status: SessionStatus,
    pub input_reason: Option<InputReason>,
    pub jsonl_path: PathBuf,
    pub last_jsonl_modified: Option<std::time::SystemTime>,
    /// Unix epoch seconds when the status last changed.
    pub last_status_change: u64,
    /// When the process was first detected as ended (for fade-out delay).
    pub ended_at: Option<Instant>,
}

// ---------------------------------------------------------------------------
// Serializable types written by the daemon for the GNOME extension to read.
// ---------------------------------------------------------------------------

/// Top-level state written to the JSON file.
#[derive(Debug, Clone, Serialize)]
pub struct DashState {
    pub sessions: Vec<DashSession>,
}

/// A single session in the JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct DashSession {
    pub session_id: String,
    pub project_name: String,
    pub branch: String,
    pub status: String,
    pub last_status_change: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_reason: Option<DashInputReason>,
}

/// Why a session needs input (permission prompt or question).
#[derive(Debug, Clone, Serialize)]
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

impl Session {
    /// Convert this session into the serializable `DashSession` form.
    pub fn to_dash_session(&self) -> DashSession {
        let status = match self.status {
            SessionStatus::NeedsInput => "needs_input",
            SessionStatus::Working => "working",
            SessionStatus::Idle => "idle",
            SessionStatus::Ended => "ended",
        }
        .to_string();

        let input_reason = self.input_reason.as_ref().map(|ir| match ir {
            InputReason::Permission(req) => DashInputReason {
                reason_type: "permission".into(),
                tool: Some(req.tool.clone()),
                command: req.input.get("command").and_then(|v| v.as_str()).map(String::from),
                detail: Some(format!("{}", req.input)),
                text: None,
            },
            InputReason::Question { text } => DashInputReason {
                reason_type: "question".into(),
                tool: None,
                command: None,
                detail: None,
                text: Some(text.clone()),
            },
        });

        DashSession {
            session_id: self.session_id.clone(),
            project_name: self.project_name.clone(),
            branch: self.branch.clone(),
            status,
            last_status_change: self.last_status_change,
            input_reason,
        }
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
    fn permission_request_deserialize() {
        let json = r#"{"session_id":"abc","tool":"Bash","input":{"command":"ls"},"timestamp":123}"#;
        let req: PermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.tool, "Bash");
        assert_eq!(req.session_id, "abc");
    }

    #[test]
    fn dash_session_serialize() {
        let s = Session {
            session_id: "abc".into(),
            pid: 1,
            pty: PathBuf::from("/dev/pts/0"),
            cwd: PathBuf::from("/home/user/project"),
            project_name: "project".into(),
            branch: "feat".into(),
            status: SessionStatus::Working,
            input_reason: None,
            jsonl_path: PathBuf::new(),
            last_jsonl_modified: None,
            last_status_change: 1000,
            ended_at: None,
        };
        let ds = s.to_dash_session();
        let json = serde_json::to_string(&ds).unwrap();
        assert!(json.contains("\"status\":\"working\""));
        assert!(json.contains("\"project_name\":\"project\""));
        assert!(!json.contains("input_reason"));
    }
}
