use agent_dash_core::protocol::HookEvent;
use agent_dash_core::session::{
    DashActiveTool, DashInputReason, DashSession, SessionStatus, tool_icon,
};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Internal session state owned by the daemon.
#[derive(Debug, Clone)]
pub struct InternalSession {
    pub session_id: String,
    pub cwd: Option<String>,
    pub project_name: String,
    pub branch: String,
    pub status: SessionStatus,
    pub active_tool: Option<(String, String, String)>, // (name, detail, tool_use_id)
    pub jsonl_path: Option<String>,
    pub last_status_change: u64,
    pub has_pending_question: bool,
    pub question_text: Option<String>,
    pub watch_offset: Option<u64>,
    pub is_main: bool,
    pub parent_wrapper_id: Option<String>,
    pub agent: Option<String>,
    pub prompt_suggestion: Option<String>,
    pub thinking_text: Option<String>,
}

/// A pending permission request.
#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub request_id: String,
    pub session_id: String,
    pub tool: String,
    pub detail: String,
    pub suggestions: Vec<serde_json::Value>,
}

/// All daemon state.
pub struct DaemonState {
    pub sessions: HashMap<String, InternalSession>,
    pub pending_permissions: HashMap<String, PendingPermission>,
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl DaemonState {
    /// Create empty state.
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            pending_permissions: HashMap::new(),
        }
    }

    /// Creates a default session entry if one does not already exist.
    pub fn ensure_session(&mut self, session_id: &str) {
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(|| InternalSession {
                session_id: session_id.to_string(),
                cwd: None,
                project_name: String::new(),
                branch: String::new(),
                status: SessionStatus::Idle,
                active_tool: None,
                jsonl_path: None,
                last_status_change: now_epoch_secs(),
                has_pending_question: false,
                question_text: None,
                watch_offset: None,
                is_main: false,
                parent_wrapper_id: None,
                agent: None,
                prompt_suggestion: None,
                thinking_text: None,
            });
    }

    /// Update session status, only bumping `last_status_change` when the status
    /// actually changes.
    pub fn set_status(&mut self, session_id: &str, new_status: SessionStatus) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            if session.status != new_status {
                session.status = new_status;
                session.last_status_change = now_epoch_secs();
            }
        }
    }

    /// Apply a hook event to the state.
    pub fn apply_hook_event(&mut self, event: HookEvent) {
        match event {
            HookEvent::ToolStart {
                session_id,
                tool,
                detail,
                tool_use_id,
            } => {
                self.ensure_session(&session_id);
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.active_tool = Some((tool, detail, tool_use_id));
                }
                self.set_status(&session_id, SessionStatus::Working);
            }
            HookEvent::ToolEnd {
                session_id,
                tool_use_id,
            } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    if let Some((_, _, ref current_id)) = session.active_tool {
                        if current_id == &tool_use_id {
                            session.active_tool = None;
                        }
                    }
                }
            }
            HookEvent::Stop { session_id } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.active_tool = None;
                }
                self.set_status(&session_id, SessionStatus::Idle);
            }
            HookEvent::SessionStart { session_id, cwd } => {
                self.ensure_session(&session_id);
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    if cwd.is_some() {
                        session.cwd = cwd;
                    }
                }
            }
            HookEvent::SessionEnd { session_id } => {
                self.sessions.remove(&session_id);
                // Also clean up any pending permissions for this session.
                self.pending_permissions
                    .retain(|_, perm| perm.session_id != session_id);
            }
        }
    }

    /// Add a permission request, setting the session to NeedsInput.
    pub fn add_permission_request(
        &mut self,
        session_id: &str,
        request_id: &str,
        tool: &str,
        detail: &str,
        suggestions: Vec<serde_json::Value>,
    ) {
        self.pending_permissions.insert(
            request_id.to_string(),
            PendingPermission {
                request_id: request_id.to_string(),
                session_id: session_id.to_string(),
                tool: tool.to_string(),
                detail: detail.to_string(),
                suggestions,
            },
        );
        self.set_status(session_id, SessionStatus::NeedsInput);
        // Also mark the parent wrapper session as NeedsInput so the web UI
        // (which only shows wrapper sessions) reflects the status.
        if let Some(parent_id) = self
            .sessions
            .get(session_id)
            .and_then(|s| s.parent_wrapper_id.clone())
        {
            self.set_status(&parent_id, SessionStatus::NeedsInput);
        }
    }

    /// Resolve (remove) a permission request. If no more pending permissions
    /// remain for that session, clears NeedsInput back to Idle.
    pub fn resolve_permission(&mut self, request_id: &str) -> Option<PendingPermission> {
        let perm = self.pending_permissions.remove(request_id)?;
        let session_id = perm.session_id.clone();

        // Check if there are any remaining pending permissions for this session.
        let still_pending = self
            .pending_permissions
            .values()
            .any(|p| p.session_id == session_id);

        if !still_pending {
            // Only clear NeedsInput if the session is currently in that state.
            if let Some(session) = self.sessions.get(&session_id) {
                if session.status == SessionStatus::NeedsInput {
                    self.set_status(&session_id, SessionStatus::Idle);
                }
            }
            // Also clear the parent wrapper's NeedsInput.
            if let Some(parent_id) = self
                .sessions
                .get(&session_id)
                .and_then(|s| s.parent_wrapper_id.clone())
            {
                if let Some(parent) = self.sessions.get(&parent_id) {
                    if parent.status == SessionStatus::NeedsInput {
                        self.set_status(&parent_id, SessionStatus::Idle);
                    }
                }
            }
        }

        Some(perm)
    }

    /// Remove a wrapper session and all its subagent sessions.
    /// Also cleans up pending permissions for removed sessions.
    pub fn remove_wrapper(&mut self, wrapper_id: &str) {
        let to_remove: Vec<String> = self
            .sessions
            .iter()
            .filter(|(id, s)| {
                *id == wrapper_id
                    || s.parent_wrapper_id.as_deref() == Some(wrapper_id)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for id in &to_remove {
            self.sessions.remove(id);
            self.pending_permissions
                .retain(|_, perm| perm.session_id != *id);
        }
    }

    /// Convert main sessions to the serializable `DashSession` form.
    /// Subagents are excluded; their count is included on the parent.
    pub fn to_dash_sessions(&self) -> Vec<DashSession> {
        self.build_dash_sessions(false)
    }

    /// Convert ALL sessions (including subagents) to `DashSession` form.
    pub fn to_all_dash_sessions(&self) -> Vec<DashSession> {
        self.build_dash_sessions(true)
    }

    fn build_dash_sessions(&self, include_subagents: bool) -> Vec<DashSession> {
        // Count subagents per parent wrapper.
        let mut subagent_counts: HashMap<String, usize> = HashMap::new();
        for s in self.sessions.values() {
            if let Some(ref parent) = s.parent_wrapper_id {
                *subagent_counts.entry(parent.clone()).or_default() += 1;
            }
        }

        let mut sessions: Vec<DashSession> = self
            .sessions
            .values()
            .filter(|s| include_subagents || s.is_main)
            .map(|s| {
                // Determine input_reason from pending permissions or pending question.
                let input_reason = if s.has_pending_question {
                    Some(DashInputReason {
                        reason_type: "question".into(),
                        tool: None,
                        command: None,
                        detail: None,
                        text: s.question_text.clone(),
                    })
                } else {
                    // Find the first pending permission for this session.
                    // For wrapper (main) sessions, permissions are stored under
                    // the real child session_id, so also match children whose
                    // parent_wrapper_id points to this session.
                    self.pending_permissions
                        .values()
                        .find(|p| {
                            p.session_id == s.session_id
                                || self
                                    .sessions
                                    .get(&p.session_id)
                                    .and_then(|child| child.parent_wrapper_id.as_deref())
                                    == Some(&s.session_id)
                        })
                        .map(|p| DashInputReason {
                            reason_type: "permission".into(),
                            tool: Some(p.tool.clone()),
                            command: None,
                            detail: Some(p.detail.clone()),
                            text: None,
                        })
                };

                let active_tool = s.active_tool.as_ref().map(|(name, detail, _)| {
                    DashActiveTool {
                        name: name.clone(),
                        detail: detail.clone(),
                        icon: tool_icon(name).to_string(),
                    }
                });

                let subagent_count = subagent_counts.get(&s.session_id).copied().unwrap_or(0);

                DashSession {
                    session_id: s.session_id.clone(),
                    project_name: s.project_name.clone(),
                    branch: s.branch.clone(),
                    status: s.status.as_str().to_string(),
                    last_status_change: s.last_status_change,
                    jsonl_path: s.jsonl_path.clone(),
                    input_reason,
                    active_tool,
                    subagent_count,
                    prompt_suggestion: s.prompt_suggestion.clone(),
                    thinking_text: s.thinking_text.clone(),
                }
            })
            .collect();

        // Sort by status priority then session_id for deterministic output.
        sessions.sort_by(|a, b| {
            a.status.cmp(&b.status).then(a.session_id.cmp(&b.session_id))
        });

        sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_dash_core::protocol::HookEvent;

    #[test]
    fn apply_tool_start_sets_working() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "tu1".into(),
        });
        let session = state.sessions.get("s1").unwrap();
        assert_eq!(session.status, SessionStatus::Working);
        assert!(session.active_tool.is_some());
    }

    #[test]
    fn apply_tool_end_clears_tool() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "tu1".into(),
        });
        state.apply_hook_event(HookEvent::ToolEnd {
            session_id: "s1".into(),
            tool_use_id: "tu1".into(),
        });
        let session = state.sessions.get("s1").unwrap();
        assert!(session.active_tool.is_none());
    }

    #[test]
    fn apply_stop_sets_idle() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "tu1".into(),
        });
        state.apply_hook_event(HookEvent::Stop {
            session_id: "s1".into(),
        });
        let session = state.sessions.get("s1").unwrap();
        assert_eq!(session.status, SessionStatus::Idle);
        assert!(session.active_tool.is_none());
    }

    #[test]
    fn apply_session_end_removes_session() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::SessionEnd {
            session_id: "s1".into(),
        });
        assert!(!state.sessions.contains_key("s1"));
    }

    #[test]
    fn permission_request_lifecycle() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.add_permission_request("s1", "tu1", "Bash", "rm -rf /tmp", vec![]);
        assert!(state.pending_permissions.contains_key("tu1"));
        let session = state.sessions.get("s1").unwrap();
        assert_eq!(session.status, SessionStatus::NeedsInput);

        state.resolve_permission("tu1");
        assert!(!state.pending_permissions.contains_key("tu1"));
    }

    #[test]
    fn ensure_session_defaults_not_main() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        let session = state.sessions.get("s1").unwrap();
        assert!(!session.is_main);
        assert!(session.parent_wrapper_id.is_none());
    }

    #[test]
    fn to_dash_sessions_returns_all() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.sessions.get_mut("s1").unwrap().is_main = true;
        state.ensure_session("s2");
        state.sessions.get_mut("s2").unwrap().is_main = true;
        let dash = state.to_dash_sessions();
        assert_eq!(dash.len(), 2);
    }

    #[test]
    fn to_dash_sessions_filters_subagents() {
        let mut state = DaemonState::new();
        state.ensure_session("main-1");
        state.sessions.get_mut("main-1").unwrap().is_main = true;
        state.sessions.get_mut("main-1").unwrap().project_name = "proj".into();
        state.ensure_session("sub-1");
        state.sessions.get_mut("sub-1").unwrap().parent_wrapper_id = Some("main-1".into());

        let dash = state.to_dash_sessions();
        assert_eq!(dash.len(), 1);
        assert_eq!(dash[0].session_id, "main-1");
    }

    #[test]
    fn to_dash_sessions_counts_subagents() {
        let mut state = DaemonState::new();
        state.ensure_session("main-1");
        state.sessions.get_mut("main-1").unwrap().is_main = true;
        state.ensure_session("sub-1");
        state.sessions.get_mut("sub-1").unwrap().parent_wrapper_id = Some("main-1".into());
        state.ensure_session("sub-2");
        state.sessions.get_mut("sub-2").unwrap().parent_wrapper_id = Some("main-1".into());

        let dash = state.to_dash_sessions();
        assert_eq!(dash.len(), 1);
        assert_eq!(dash[0].subagent_count, 2);
    }

    #[test]
    fn to_all_dash_sessions_includes_subagents() {
        let mut state = DaemonState::new();
        state.ensure_session("main-1");
        state.sessions.get_mut("main-1").unwrap().is_main = true;
        state.ensure_session("sub-1");
        state.sessions.get_mut("sub-1").unwrap().parent_wrapper_id = Some("main-1".into());

        let dash = state.to_all_dash_sessions();
        assert_eq!(dash.len(), 2);
    }

    #[test]
    fn remove_wrapper_cleans_up_subagents() {
        let mut state = DaemonState::new();

        // Main session.
        state.ensure_session("wrap-1");
        state.sessions.get_mut("wrap-1").unwrap().is_main = true;

        // Two subagents.
        state.ensure_session("sub-a");
        state.sessions.get_mut("sub-a").unwrap().parent_wrapper_id = Some("wrap-1".into());
        state.ensure_session("sub-b");
        state.sessions.get_mut("sub-b").unwrap().parent_wrapper_id = Some("wrap-1".into());

        // Unrelated session.
        state.ensure_session("wrap-2");
        state.sessions.get_mut("wrap-2").unwrap().is_main = true;

        state.remove_wrapper("wrap-1");

        assert!(!state.sessions.contains_key("wrap-1"));
        assert!(!state.sessions.contains_key("sub-a"));
        assert!(!state.sessions.contains_key("sub-b"));
        assert!(state.sessions.contains_key("wrap-2"));
    }

    #[test]
    fn permission_on_child_shows_on_wrapper() {
        let mut state = DaemonState::new();

        // Wrapper (main) session.
        state.ensure_session("wrap-1");
        state.sessions.get_mut("wrap-1").unwrap().is_main = true;

        // Child session linked to wrapper.
        state.ensure_session("child-1");
        state.sessions.get_mut("child-1").unwrap().parent_wrapper_id =
            Some("wrap-1".into());

        // Permission filed under the child session_id.
        state.add_permission_request("child-1", "perm-1", "Bash", "rm -rf /tmp", vec![]);

        // The wrapper (shown in to_dash_sessions) should surface the permission.
        let sessions = state.to_dash_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "wrap-1");
        let ir = sessions[0].input_reason.as_ref().expect("should have input_reason");
        assert_eq!(ir.reason_type, "permission");
        assert_eq!(ir.tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn permission_resolve_clears_wrapper_status() {
        let mut state = DaemonState::new();

        state.ensure_session("wrap-1");
        state.sessions.get_mut("wrap-1").unwrap().is_main = true;

        state.ensure_session("child-1");
        state.sessions.get_mut("child-1").unwrap().parent_wrapper_id =
            Some("wrap-1".into());

        state.add_permission_request("child-1", "perm-1", "Bash", "ls", vec![]);

        // Both child and wrapper should be NeedsInput.
        assert_eq!(
            state.sessions.get("child-1").unwrap().status,
            SessionStatus::NeedsInput,
        );
        assert_eq!(
            state.sessions.get("wrap-1").unwrap().status,
            SessionStatus::NeedsInput,
        );

        // Resolve the permission.
        state.resolve_permission("perm-1");

        // Both should be back to Idle.
        assert_eq!(
            state.sessions.get("child-1").unwrap().status,
            SessionStatus::Idle,
        );
        assert_eq!(
            state.sessions.get("wrap-1").unwrap().status,
            SessionStatus::Idle,
        );
    }
}
