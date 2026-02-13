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
    /// When the process was first detected as ended (for fade-out delay).
    pub ended_at: Option<Instant>,
}

impl Session {
    pub fn label(&self) -> String {
        if self.branch.is_empty() || self.branch == "main" {
            self.project_name.clone()
        } else {
            format!("{} ({})", self.project_name, self.branch)
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
    fn label_without_branch() {
        let s = Session {
            session_id: "abc".into(),
            pid: 1,
            pty: PathBuf::from("/dev/pts/0"),
            cwd: PathBuf::from("/home/user/project"),
            project_name: "project".into(),
            branch: "main".into(),
            status: SessionStatus::Idle,
            input_reason: None,
            jsonl_path: PathBuf::new(),
            last_jsonl_modified: None,
            ended_at: None,
        };
        assert_eq!(s.label(), "project");
    }

    #[test]
    fn label_with_branch() {
        let s = Session {
            session_id: "abc".into(),
            pid: 1,
            pty: PathBuf::from("/dev/pts/0"),
            cwd: PathBuf::from("/home/user/project"),
            project_name: "project".into(),
            branch: "feature-x".into(),
            status: SessionStatus::Idle,
            input_reason: None,
            jsonl_path: PathBuf::new(),
            last_jsonl_modified: None,
            ended_at: None,
        };
        assert_eq!(s.label(), "project (feature-x)");
    }

    #[test]
    fn permission_request_deserialize() {
        let json = r#"{"session_id":"abc","tool":"Bash","input":{"command":"ls"},"timestamp":123}"#;
        let req: PermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.tool, "Bash");
        assert_eq!(req.session_id, "abc");
    }
}
