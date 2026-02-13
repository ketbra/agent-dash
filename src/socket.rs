use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// An event sent by Claude Code hooks via the Unix socket.
#[derive(Debug, Clone, Deserialize)]
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
        cwd: Option<String>,
    },
    #[serde(rename = "session_end")]
    SessionEnd { session_id: String },
}

/// Data about a currently active tool invocation.
#[derive(Debug, Clone)]
pub struct ActiveToolData {
    pub tool: String,
    pub detail: String,
    pub tool_use_id: String,
}

/// Per-session state tracked from hook events.
#[derive(Debug, Clone)]
pub struct HookSessionData {
    pub active_tool: Option<ActiveToolData>,
    pub is_idle: bool,
    pub last_event: Instant,
}

/// Shared hook state: a map from session_id to per-session data.
pub type HookState = Arc<Mutex<HashMap<String, HookSessionData>>>;

/// Create a new, empty hook state.
pub fn new_hook_state() -> HookState {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Apply a hook event to the shared state.
pub fn apply_event(state: &HookState, event: HookEvent) {
    let mut map = state.lock().unwrap();
    match event {
        HookEvent::ToolStart {
            session_id,
            tool,
            detail,
            tool_use_id,
        } => {
            let entry = map.entry(session_id).or_insert_with(|| HookSessionData {
                active_tool: None,
                is_idle: false,
                last_event: Instant::now(),
            });
            entry.active_tool = Some(ActiveToolData {
                tool,
                detail,
                tool_use_id,
            });
            entry.is_idle = false;
            entry.last_event = Instant::now();
        }
        HookEvent::ToolEnd {
            session_id,
            tool_use_id,
        } => {
            if let Some(entry) = map.get_mut(&session_id) {
                if entry
                    .active_tool
                    .as_ref()
                    .is_some_and(|t| t.tool_use_id == tool_use_id)
                {
                    entry.active_tool = None;
                }
                entry.last_event = Instant::now();
            }
        }
        HookEvent::Stop { session_id } => {
            if let Some(entry) = map.get_mut(&session_id) {
                entry.active_tool = None;
                entry.is_idle = true;
                entry.last_event = Instant::now();
            }
        }
        HookEvent::SessionStart {
            session_id,
            cwd: _,
        } => {
            map.insert(
                session_id,
                HookSessionData {
                    active_tool: None,
                    is_idle: false,
                    last_event: Instant::now(),
                },
            );
        }
        HookEvent::SessionEnd { session_id } => {
            map.remove(&session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Deserialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_tool_start() {
        let json = r#"{
            "event": "tool_start",
            "session_id": "sess-1",
            "tool": "Bash",
            "detail": "ls -la",
            "tool_use_id": "tu-001"
        }"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::ToolStart {
                session_id,
                tool,
                detail,
                tool_use_id,
            } => {
                assert_eq!(session_id, "sess-1");
                assert_eq!(tool, "Bash");
                assert_eq!(detail, "ls -la");
                assert_eq!(tool_use_id, "tu-001");
            }
            _ => panic!("expected ToolStart"),
        }
    }

    #[test]
    fn deserialize_tool_end() {
        let json = r#"{
            "event": "tool_end",
            "session_id": "sess-1",
            "tool_use_id": "tu-001"
        }"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::ToolEnd {
                session_id,
                tool_use_id,
            } => {
                assert_eq!(session_id, "sess-1");
                assert_eq!(tool_use_id, "tu-001");
            }
            _ => panic!("expected ToolEnd"),
        }
    }

    #[test]
    fn deserialize_stop() {
        let json = r#"{"event": "stop", "session_id": "sess-1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::Stop { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            _ => panic!("expected Stop"),
        }
    }

    #[test]
    fn deserialize_session_start() {
        let json = r#"{
            "event": "session_start",
            "session_id": "sess-2",
            "cwd": "/home/user/project"
        }"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::SessionStart { session_id, cwd } => {
                assert_eq!(session_id, "sess-2");
                assert_eq!(cwd.as_deref(), Some("/home/user/project"));
            }
            _ => panic!("expected SessionStart"),
        }
    }

    #[test]
    fn deserialize_session_start_without_cwd() {
        let json = r#"{"event": "session_start", "session_id": "sess-3"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::SessionStart { session_id, cwd } => {
                assert_eq!(session_id, "sess-3");
                assert!(cwd.is_none());
            }
            _ => panic!("expected SessionStart"),
        }
    }

    #[test]
    fn deserialize_session_end() {
        let json = r#"{"event": "session_end", "session_id": "sess-1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::SessionEnd { session_id } => {
                assert_eq!(session_id, "sess-1");
            }
            _ => panic!("expected SessionEnd"),
        }
    }

    // -----------------------------------------------------------------------
    // apply_event tests
    // -----------------------------------------------------------------------

    #[test]
    fn apply_tool_start() {
        let state = new_hook_state();
        apply_event(
            &state,
            HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: None,
            },
        );
        apply_event(
            &state,
            HookEvent::ToolStart {
                session_id: "s1".into(),
                tool: "Bash".into(),
                detail: "cargo build".into(),
                tool_use_id: "tu-1".into(),
            },
        );
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        let tool = data.active_tool.as_ref().unwrap();
        assert_eq!(tool.tool, "Bash");
        assert_eq!(tool.detail, "cargo build");
        assert_eq!(tool.tool_use_id, "tu-1");
        assert!(!data.is_idle);
    }

    #[test]
    fn apply_tool_end_matching_id() {
        let state = new_hook_state();
        apply_event(
            &state,
            HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: None,
            },
        );
        apply_event(
            &state,
            HookEvent::ToolStart {
                session_id: "s1".into(),
                tool: "Bash".into(),
                detail: "ls".into(),
                tool_use_id: "tu-1".into(),
            },
        );
        apply_event(
            &state,
            HookEvent::ToolEnd {
                session_id: "s1".into(),
                tool_use_id: "tu-1".into(),
            },
        );
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(data.active_tool.is_none());
    }

    #[test]
    fn apply_tool_end_non_matching_id() {
        let state = new_hook_state();
        apply_event(
            &state,
            HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: None,
            },
        );
        apply_event(
            &state,
            HookEvent::ToolStart {
                session_id: "s1".into(),
                tool: "Bash".into(),
                detail: "ls".into(),
                tool_use_id: "tu-1".into(),
            },
        );
        // End with a different tool_use_id -- should NOT clear
        apply_event(
            &state,
            HookEvent::ToolEnd {
                session_id: "s1".into(),
                tool_use_id: "tu-OTHER".into(),
            },
        );
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(data.active_tool.is_some());
        assert_eq!(data.active_tool.as_ref().unwrap().tool_use_id, "tu-1");
    }

    #[test]
    fn apply_stop() {
        let state = new_hook_state();
        apply_event(
            &state,
            HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: None,
            },
        );
        apply_event(
            &state,
            HookEvent::ToolStart {
                session_id: "s1".into(),
                tool: "Bash".into(),
                detail: "cargo test".into(),
                tool_use_id: "tu-1".into(),
            },
        );
        apply_event(
            &state,
            HookEvent::Stop {
                session_id: "s1".into(),
            },
        );
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(data.active_tool.is_none());
        assert!(data.is_idle);
    }

    #[test]
    fn apply_session_start() {
        let state = new_hook_state();
        apply_event(
            &state,
            HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: Some("/home/user/project".into()),
            },
        );
        let map = state.lock().unwrap();
        assert!(map.contains_key("s1"));
        let data = map.get("s1").unwrap();
        assert!(data.active_tool.is_none());
        assert!(!data.is_idle);
    }

    #[test]
    fn apply_session_end() {
        let state = new_hook_state();
        apply_event(
            &state,
            HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: None,
            },
        );
        assert!(state.lock().unwrap().contains_key("s1"));
        apply_event(
            &state,
            HookEvent::SessionEnd {
                session_id: "s1".into(),
            },
        );
        assert!(!state.lock().unwrap().contains_key("s1"));
    }

    #[test]
    fn tool_start_clears_idle_flag() {
        let state = new_hook_state();
        apply_event(
            &state,
            HookEvent::SessionStart {
                session_id: "s1".into(),
                cwd: None,
            },
        );
        // First make it idle via Stop
        apply_event(
            &state,
            HookEvent::Stop {
                session_id: "s1".into(),
            },
        );
        assert!(state.lock().unwrap().get("s1").unwrap().is_idle);

        // Now a tool start should clear the idle flag
        apply_event(
            &state,
            HookEvent::ToolStart {
                session_id: "s1".into(),
                tool: "Read".into(),
                detail: "foo.rs".into(),
                tool_use_id: "tu-2".into(),
            },
        );
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(!data.is_idle);
        assert!(data.active_tool.is_some());
    }
}
