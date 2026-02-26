use agent_dash_core::paths;
use agent_dash_core::protocol::{self, HookEnvelope, HookEvent, HookPermissionDecision, ImageAttachment, ServerEvent};
use agent_dash_core::session::{DashState, SessionStatus};
use base64::Engine as _;
use crate::client_listener::{self, ClientMessage};
use crate::hook_listener;
use crate::messages;
use crate::watcher;
use crate::state::DaemonState;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub async fn run(web_port: u16) {
    let hook_sock = paths::hook_socket_name();
    let client_sock = paths::client_socket_name();
    let state_path = paths::state_file_path();

    eprintln!("agent-dashd starting");
    eprintln!("  hook socket:   {hook_sock}");
    eprintln!("  client socket: {client_sock}");
    eprintln!("  state file:    {}", state_path.display());

    // Ensure runtime directory exists.
    let cache = paths::cache_dir();
    let _ = std::fs::create_dir_all(&cache);

    // Write PID file so `daemon stop` and `daemon status` can find us.
    let pid_path = paths::pid_file_path();
    let _ = std::fs::write(&pid_path, std::process::id().to_string());

    // Channels
    let (hook_tx, mut hook_rx) = mpsc::channel::<HookEnvelope>(256);
    let (client_tx, mut client_rx) = mpsc::channel::<ClientMessage>(256);

    // Spawn listeners
    tokio::spawn(hook_listener::run(hook_tx));
    tokio::spawn(client_listener::run(client_tx.clone()));
    tokio::spawn(crate::web::run(web_port, client_tx));

    // Main loop state
    let mut state = DaemonState::new();
    let mut subscribers: Vec<mpsc::Sender<String>> = Vec::new();
    let mut permission_waiters: HashMap<String, oneshot::Sender<HookPermissionDecision>> =
        HashMap::new();
    let (watch_tx, mut watch_rx) = mpsc::channel::<watcher::FileChanged>(256);
    let mut session_watcher = watcher::SessionWatcher::new(watch_tx)
        .expect("failed to create file watcher");
    let mut message_subscribers: HashMap<String, Vec<(String, mpsc::Sender<String>)>> =
        HashMap::new();
    let mut wrapper_channels: HashMap<String, mpsc::Sender<String>> = HashMap::new();

    let mut write_interval = tokio::time::interval(Duration::from_millis(500));
    let mut state_dirty = false;

    // Don't delay the first tick.
    write_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // --- Hook events ---
            Some(envelope) = hook_rx.recv() => {
                let HookEnvelope { event, wrapper_id } = envelope;

                // If this hook came from a wrapped session, alias the real
                // session_id to the wrapper's prompt channel so `inject` works.
                if let Some(ref wid) = wrapper_id {
                    let hook_session_id = match &event {
                        HookEvent::ToolStart { session_id, .. }
                        | HookEvent::ToolEnd { session_id, .. }
                        | HookEvent::Stop { session_id }
                        | HookEvent::SessionStart { session_id, .. }
                        | HookEvent::SessionEnd { session_id } => session_id,
                    };
                    if !wrapper_channels.contains_key(hook_session_id) {
                        if let Some(prompt_tx) = wrapper_channels.get(wid) {
                            wrapper_channels.insert(hook_session_id.clone(), prompt_tx.clone());
                        }
                    }
                    // Ensure the real session exists (but do NOT mark as
                    // is_main — subagent hooks should not promote to main).
                    state.ensure_session(hook_session_id);
                    if let Some(session) = state.sessions.get_mut(hook_session_id) {
                        // If hook_session_id matches wrapper_id, it's the main session.
                        // If it doesn't match and isn't already main, it's a subagent.
                        if hook_session_id != wid && !session.is_main {
                            session.parent_wrapper_id = Some(wid.clone());
                        }
                    }
                }

                // Check if a ToolStart or Stop resolves a pending permission
                // (user approved via terminal rather than through us).
                match &event {
                    HookEvent::ToolStart { session_id, .. } | HookEvent::Stop { session_id } => {
                        // Find any pending permissions for this session.
                        let pending_ids: Vec<String> = state
                            .pending_permissions
                            .iter()
                            .filter(|(_, p)| p.session_id == *session_id)
                            .map(|(id, _)| id.clone())
                            .collect();

                        for request_id in pending_ids {
                            if let Some(perm) = state.resolve_permission(&request_id) {
                                // Notify the waiting hook if present.
                                if let Some(waiter) = permission_waiters.remove(&request_id) {
                                    let _ = waiter.send(HookPermissionDecision {
                                        request_id: request_id.clone(),
                                        decision: "allow".into(),
                                        suggestion: None,
                                    });
                                }
                                // Broadcast resolution.
                                let resolved = ServerEvent::PermissionResolved {
                                    request_id: perm.request_id,
                                    resolved_by: "terminal".into(),
                                };
                                broadcast_to_subscribers(&mut subscribers, &resolved);
                            }
                        }
                    }
                    _ => {}
                }

                // Extract the session_id before consuming the event.
                let hook_sid = match &event {
                    HookEvent::ToolStart { session_id, .. }
                    | HookEvent::ToolEnd { session_id, .. }
                    | HookEvent::Stop { session_id }
                    | HookEvent::SessionStart { session_id, .. }
                    | HookEvent::SessionEnd { session_id } => session_id.clone(),
                };

                state.apply_hook_event(event);

                // Populate jsonl_path if missing (on the real session and its wrapper).
                if let Some(session) = state.sessions.get(&hook_sid) {
                    if session.jsonl_path.is_none() {
                        if let Some(path) = resolve_jsonl_path(&hook_sid, &state) {
                            let path_str = path.to_string_lossy().to_string();
                            if let Some(s) = state.sessions.get_mut(&hook_sid) {
                                s.jsonl_path = Some(path_str.clone());
                            }
                            // Also set on the wrapper session so the UI can find it.
                            if let Some(ref wid) = wrapper_id {
                                if *wid != hook_sid {
                                    if let Some(ws) = state.sessions.get_mut(wid) {
                                        if ws.jsonl_path.is_none() {
                                            ws.jsonl_path = Some(path_str);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                state_dirty = true;
                broadcast_state(&mut subscribers, &state);
            }

            // --- Client messages ---
            Some(msg) = client_rx.recv() => {
                match msg {
                    ClientMessage::Subscribe { tx } => {
                        // Send current state immediately.
                        let update = ServerEvent::StateUpdate {
                            sessions: state.to_dash_sessions(),
                        };
                        if let Ok(line) = protocol::encode_line(&update) {
                            let _ = tx.try_send(line);
                        }
                        // Send any pending permissions (using wrapper ID for display).
                        for perm in state.pending_permissions.values() {
                            let display_sid = state
                                .sessions
                                .get(&perm.session_id)
                                .and_then(|s| s.parent_wrapper_id.clone())
                                .unwrap_or_else(|| perm.session_id.clone());
                            let pending = ServerEvent::PermissionPending {
                                session_id: display_sid,
                                request_id: perm.request_id.clone(),
                                tool: perm.tool.clone(),
                                detail: perm.detail.clone(),
                                suggestions: perm.suggestions.clone(),
                            };
                            if let Ok(line) = protocol::encode_line(&pending) {
                                let _ = tx.try_send(line);
                            }
                        }
                        subscribers.push(tx);
                    }
                    ClientMessage::GetState { include_subagents, reply } => {
                        let sessions = if include_subagents {
                            state.to_all_dash_sessions()
                        } else {
                            state.to_dash_sessions()
                        };
                        let event = ServerEvent::StateUpdate {
                            sessions,
                        };
                        if let Ok(json) = protocol::encode_line(&event) {
                            let _ = reply.send(json);
                        }
                    }
                    ClientMessage::PermissionResponse {
                        request_id,
                        decision,
                        suggestion,
                        ..
                    } => {
                        if let Some(perm) = state.resolve_permission(&request_id) {
                            // Notify the waiting hook.
                            if let Some(waiter) = permission_waiters.remove(&request_id) {
                                let _ = waiter.send(HookPermissionDecision {
                                    request_id: request_id.clone(),
                                    decision: decision.clone(),
                                    suggestion,
                                });
                            }
                            // Broadcast resolution.
                            let resolved = ServerEvent::PermissionResolved {
                                request_id: perm.request_id,
                                resolved_by: "dashboard".into(),
                            };
                            broadcast_to_subscribers(&mut subscribers, &resolved);
                            state_dirty = true;
                            broadcast_state(&mut subscribers, &state);
                        }
                    }
                    ClientMessage::PermissionRequest {
                        request_id,
                        session_id,
                        tool,
                        detail,
                        suggestions,
                        reply,
                    } => {
                        state.add_permission_request(
                            &session_id,
                            &request_id,
                            &tool,
                            &detail,
                            suggestions.clone(),
                        );
                        permission_waiters.insert(request_id.clone(), reply);

                        // Broadcast permission pending. If this session is a
                        // subagent/child, use the parent wrapper_id so the UI
                        // (which shows wrapper sessions) can match it.
                        let display_session_id = state
                            .sessions
                            .get(&session_id)
                            .and_then(|s| s.parent_wrapper_id.clone())
                            .unwrap_or(session_id);
                        let pending = ServerEvent::PermissionPending {
                            session_id: display_session_id,
                            request_id,
                            tool,
                            detail,
                            suggestions,
                        };
                        broadcast_to_subscribers(&mut subscribers, &pending);
                        state_dirty = true;
                        broadcast_state(&mut subscribers, &state);
                    }
                    ClientMessage::GetMessages {
                        session_id,
                        format,
                        limit,
                        reply,
                    } => {
                        let response = if let Some(path) = resolve_jsonl_path(&session_id, &state) {
                            let msgs = messages::read_messages(&path, limit, &format);
                            let event = ServerEvent::Messages {
                                session_id,
                                messages: msgs,
                            };
                            protocol::encode_line(&event).unwrap_or_default()
                        } else {
                            protocol::encode_line(&ServerEvent::Messages {
                                session_id,
                                messages: vec![],
                            })
                            .unwrap_or_default()
                        };
                        let _ = reply.send(response);
                    }
                    ClientMessage::WatchSession {
                        session_id,
                        format,
                        tx,
                    } => {
                        // Resolve truncated session ID to canonical key.
                        let canonical = resolve_session_key(&session_id, &state)
                            .unwrap_or(session_id);
                        if let Some(session) = state.sessions.get_mut(&canonical) {
                            if let Some(ref jsonl) = session.jsonl_path {
                                let path = std::path::PathBuf::from(jsonl);
                                let file_len = std::fs::metadata(&path)
                                    .map(|m| m.len())
                                    .unwrap_or(0);
                                session.watch_offset = Some(file_len);
                                let _ = session_watcher.watch(&canonical, &path);
                            }
                        }
                        message_subscribers
                            .entry(canonical)
                            .or_default()
                            .push((format, tx));
                    }
                    ClientMessage::UnwatchSession { session_id } => {
                        let canonical = resolve_session_key(&session_id, &state)
                            .unwrap_or(session_id);
                        message_subscribers.remove(&canonical);
                        session_watcher.unwatch(&canonical);
                        if let Some(session) = state.sessions.get_mut(&canonical) {
                            session.watch_offset = None;
                        }
                    }
                    ClientMessage::ListSessions { project, reply } => {
                        let projects_dir = paths::claude_projects_dir();
                        // Find the slug for this project by looking at existing sessions.
                        let slug = state
                            .sessions
                            .values()
                            .find(|s| s.project_name == project)
                            .and_then(|s| s.cwd.as_ref())
                            .map(|cwd| paths::cwd_to_project_slug(std::path::Path::new(cwd)))
                            .unwrap_or_else(|| project.replace('/', "-").replace('\\', "-"));
                        let project_dir = projects_dir.join(&slug);

                        let mut entries = Vec::new();
                        if let Ok(dir_entries) = std::fs::read_dir(&project_dir) {
                            let main_jsonl: Option<String> = state
                                .sessions
                                .values()
                                .find(|s| s.project_name == project)
                                .and_then(|s| s.jsonl_path.clone());

                            for entry in dir_entries.flatten() {
                                let path = entry.path();
                                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                                    continue;
                                }
                                let modified = entry
                                    .metadata()
                                    .ok()
                                    .and_then(|m| m.modified().ok())
                                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);

                                // Skip JSONL files that have no parseable
                                // conversation content (e.g. metadata-only
                                // files with just file-history-snapshot).
                                let Some(status) = crate::jsonl::parse_jsonl_status(&path) else {
                                    continue;
                                };
                                let session_id = status.session_id;

                                let is_main = main_jsonl
                                    .as_ref()
                                    .is_some_and(|p| *p == path.to_string_lossy().to_string());

                                entries.push(protocol::SessionListEntry {
                                    session_id,
                                    main: is_main,
                                    modified,
                                });
                            }
                        }

                        entries.sort_by(|a, b| b.modified.cmp(&a.modified));

                        let event = ServerEvent::SessionList {
                            project,
                            sessions: entries,
                        };
                        let _ = reply.send(protocol::encode_line(&event).unwrap_or_default());
                    }
                    ClientMessage::RegisterWrapper {
                        session_id,
                        agent,
                        cwd,
                        branch,
                        project_name,
                        real_session_id,
                        prompt_tx,
                    } => {
                        state.ensure_session(&session_id);
                        if let Some(session) = state.sessions.get_mut(&session_id) {
                            session.is_main = true;
                            session.agent = Some(agent);
                            if let Some(ref c) = cwd {
                                session.cwd = Some(c.clone());
                            }
                            if let Some(ref b) = branch {
                                session.branch = b.clone();
                            }
                            if let Some(ref p) = project_name {
                                session.project_name = p.clone();
                            }
                        }
                        wrapper_channels.insert(session_id.clone(), prompt_tx);

                        // On reconnect, re-link the real session_id to this wrapper's channel
                        // and resolve the JSONL path if missing.
                        if let Some(ref real_id) = real_session_id {
                            if let Some(tx) = wrapper_channels.get(&session_id) {
                                wrapper_channels.insert(real_id.clone(), tx.clone());
                            }
                            // Populate jsonl_path on the wrapper from the real session ID.
                            let needs_path = state.sessions.get(&session_id)
                                .is_some_and(|s| s.jsonl_path.is_none());
                            if needs_path {
                                if let Some(path) = find_jsonl_on_disk(real_id) {
                                    let path_str = path.to_string_lossy().to_string();
                                    if let Some(s) = state.sessions.get_mut(&session_id) {
                                        s.jsonl_path = Some(path_str);
                                    }
                                }
                            }
                        }

                        state_dirty = true;
                        broadcast_state(&mut subscribers, &state);
                    }
                    ClientMessage::UnregisterWrapper { session_id } => {
                        wrapper_channels.remove(&session_id);
                        // Also remove channels for subagents of this wrapper.
                        let sub_ids: Vec<String> = state.sessions.iter()
                            .filter(|(_, s)| s.parent_wrapper_id.as_deref() == Some(&session_id))
                            .map(|(id, _)| id.clone())
                            .collect();
                        for id in &sub_ids {
                            wrapper_channels.remove(id);
                        }
                        state.remove_wrapper(&session_id);
                        state_dirty = true;
                        broadcast_state(&mut subscribers, &state);
                    }
                    ClientMessage::UpdateSuggestion {
                        session_id,
                        suggestion,
                    } => {
                        if let Some(session) = state.sessions.get_mut(&session_id) {
                            if session.prompt_suggestion != suggestion {
                                session.prompt_suggestion = suggestion;
                                state_dirty = true;
                                broadcast_state(&mut subscribers, &state);
                            }
                        }
                    }
                    ClientMessage::SendPrompt {
                        session_id,
                        text,
                        images,
                        reply,
                    } => {
                        // Save any attached images and augment the prompt text.
                        let text = if images.is_empty() {
                            text
                        } else {
                            match save_images_to_temp(&images) {
                                Ok(paths) => {
                                    let mut augmented = text;
                                    augmented.push_str("\n\nAttached images (use the Read tool to view them):");
                                    for p in &paths {
                                        augmented.push_str("\n- ");
                                        augmented.push_str(p);
                                    }
                                    augmented
                                }
                                Err(e) => {
                                    let _ = reply.send(
                                        protocol::encode_line(&ServerEvent::Error {
                                            message: format!("failed to save images: {e}"),
                                        })
                                        .unwrap_or_default(),
                                    );
                                    continue;
                                }
                            }
                        };

                        // Check if the target is a subagent — reject prompt
                        // injection to subagent sessions.
                        let is_subagent = resolve_session_key(&session_id, &state)
                            .and_then(|k| state.sessions.get(&k))
                            .is_some_and(|s| !s.is_main && s.parent_wrapper_id.is_some());

                        let response = if is_subagent {
                            ServerEvent::Error {
                                message: "cannot inject prompt into subagent".into(),
                            }
                        } else {
                            // Try exact match first, then prefix match (user may
                            // pass truncated session IDs from `sessions` output).
                            let prompt_tx = wrapper_channels.get(&session_id).or_else(|| {
                                let matches: Vec<_> = wrapper_channels
                                    .iter()
                                    .filter(|(k, _)| k.starts_with(&session_id))
                                    .collect();
                                if matches.len() == 1 {
                                    Some(matches[0].1)
                                } else {
                                    None
                                }
                            });
                            if let Some(prompt_tx) = prompt_tx {
                                if prompt_tx.try_send(text).is_ok() {
                                    // Immediately mark the wrapper session as Working so
                                    // the UI shows "thinking..." while Claude processes
                                    // the prompt (before the first ToolStart hook fires).
                                    state.set_status(&session_id, SessionStatus::Working);
                                    broadcast_state(&mut subscribers, &state);
                                    ServerEvent::PromptSent { session_id }
                                } else {
                                    ServerEvent::Error {
                                        message: "wrapper channel full or closed".into(),
                                    }
                                }
                            } else {
                                ServerEvent::Error {
                                    message: "session is not wrapped".into(),
                                }
                            }
                        };
                        let _ = reply.send(protocol::encode_line(&response).unwrap_or_default());
                    }
                }
            }

            // --- File change events (for watch_session subscribers) ---
            Some(changed) = watch_rx.recv() => {
                if let Some(subs) = message_subscribers.get(&changed.session_id) {
                    if !subs.is_empty() {
                        let offset = state.sessions.get(&changed.session_id)
                            .and_then(|s| s.watch_offset)
                            .unwrap_or(0);

                        let mut by_format: HashMap<&str, Vec<&mpsc::Sender<String>>> = HashMap::new();
                        for (fmt, tx) in subs {
                            by_format.entry(fmt.as_str()).or_default().push(tx);
                        }

                        for (fmt, senders) in &by_format {
                            let (msgs, new_offset) = messages::read_new_messages(
                                &changed.path, offset, fmt,
                            );
                            if let Some(session) = state.sessions.get_mut(&changed.session_id) {
                                session.watch_offset = Some(new_offset);
                            }
                            for msg in &msgs {
                                let event = ServerEvent::Message {
                                    session_id: changed.session_id.clone(),
                                    message: msg.clone(),
                                };
                                if let Ok(line) = protocol::encode_line(&event) {
                                    for tx in senders {
                                        let _ = tx.try_send(line.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                // Clean up disconnected subscribers.
                for subs in message_subscribers.values_mut() {
                    subs.retain(|(_, tx)| !tx.is_closed());
                }
                message_subscribers.retain(|_, subs| !subs.is_empty());
            }

            // --- Periodic state.json write ---
            _ = write_interval.tick() => {
                if state_dirty {
                    write_state_file(&state);
                    state_dirty = false;
                }
            }

        }
    }
}

/// Broadcast a state_update event to all subscribers.
fn broadcast_state(subscribers: &mut Vec<mpsc::Sender<String>>, state: &DaemonState) {
    let event = ServerEvent::StateUpdate {
        sessions: state.to_dash_sessions(),
    };
    broadcast_to_subscribers(subscribers, &event);
}

/// Encode and send an event to all subscribers. Removes disconnected ones.
fn broadcast_to_subscribers<T: serde::Serialize>(
    subscribers: &mut Vec<mpsc::Sender<String>>,
    event: &T,
) {
    let Ok(line) = protocol::encode_line(event) else {
        return;
    };
    subscribers.retain(|tx| tx.try_send(line.clone()).is_ok());
}

/// Resolve a possibly-truncated session ID to the canonical key in
/// daemon state. Returns `None` if no unique match is found.
fn resolve_session_key(session_id: &str, state: &DaemonState) -> Option<String> {
    // Exact match.
    if state.sessions.contains_key(session_id) {
        return Some(session_id.to_string());
    }
    // Unique prefix match.
    let matches: Vec<_> = state
        .sessions
        .keys()
        .filter(|k| k.starts_with(session_id))
        .collect();
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }
    None
}

/// Resolve a (possibly truncated) session ID to its JSONL path.
///
/// Tries in order:
/// 1. Exact match in daemon state
/// 2. Prefix match in daemon state
/// 3. Prefix match on JSONL filenames in Claude's projects directory
fn resolve_jsonl_path(session_id: &str, state: &DaemonState) -> Option<std::path::PathBuf> {
    // 1. Exact match.
    if let Some(session) = state.sessions.get(session_id) {
        if let Some(ref p) = session.jsonl_path {
            return Some(std::path::PathBuf::from(p));
        }
    }

    // 2. Prefix match on daemon state keys.
    let prefix_matches: Vec<_> = state
        .sessions
        .iter()
        .filter(|(k, _)| k.starts_with(session_id))
        .collect();
    if prefix_matches.len() == 1 {
        if let Some(ref p) = prefix_matches[0].1.jsonl_path {
            return Some(std::path::PathBuf::from(p));
        }
    }

    // 3. For wrapper sessions, check child sessions that are linked via
    //    parent_wrapper_id — their jsonl_path is the main conversation.
    for (_, child) in state.sessions.iter() {
        if child.parent_wrapper_id.as_deref() == Some(session_id) {
            if let Some(ref p) = child.jsonl_path {
                return Some(std::path::PathBuf::from(p));
            }
            // Child exists but has no jsonl_path — try filesystem for it.
            if let Some(path) = find_jsonl_on_disk(&child.session_id) {
                return Some(path);
            }
        }
    }

    // 4. Search JSONL files in Claude's projects directory by filename prefix.
    if let Some(path) = find_jsonl_on_disk(session_id) {
        return Some(path);
    }

    None
}

/// Search `~/.claude/projects/` for a JSONL file whose name starts with the given ID.
fn find_jsonl_on_disk(session_id: &str) -> Option<std::path::PathBuf> {
    let projects_dir = paths::claude_projects_dir();
    if let Ok(project_dirs) = std::fs::read_dir(&projects_dir) {
        for project_entry in project_dirs.flatten() {
            if !project_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(project_entry.path()) {
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if stem.starts_with(session_id) {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InternalSession;
    use agent_dash_core::session::SessionStatus;

    fn make_session(id: &str) -> InternalSession {
        InternalSession {
            session_id: id.to_string(),
            cwd: None,
            project_name: String::new(),
            branch: String::new(),
            status: SessionStatus::Idle,
            active_tool: None,
            jsonl_path: None,
            last_status_change: 0,
            has_pending_question: false,
            question_text: None,
            watch_offset: None,
            is_main: false,
            parent_wrapper_id: None,
            agent: None,
            prompt_suggestion: None,
        }
    }

    // -- resolve_session_key --

    #[test]
    fn resolve_session_key_exact_match() {
        let mut state = DaemonState::new();
        state.sessions.insert("abc-123".into(), make_session("abc-123"));
        assert_eq!(
            resolve_session_key("abc-123", &state),
            Some("abc-123".to_string())
        );
    }

    #[test]
    fn resolve_session_key_prefix_match() {
        let mut state = DaemonState::new();
        state
            .sessions
            .insert("abc-123-full-uuid".into(), make_session("abc-123-full-uuid"));
        assert_eq!(
            resolve_session_key("abc-123", &state),
            Some("abc-123-full-uuid".to_string())
        );
    }

    #[test]
    fn resolve_session_key_ambiguous_prefix_returns_none() {
        let mut state = DaemonState::new();
        state.sessions.insert("abc-111".into(), make_session("abc-111"));
        state.sessions.insert("abc-222".into(), make_session("abc-222"));
        assert_eq!(resolve_session_key("abc", &state), None);
    }

    #[test]
    fn resolve_session_key_no_match_returns_none() {
        let state = DaemonState::new();
        assert_eq!(resolve_session_key("xyz", &state), None);
    }

    #[test]
    fn resolve_session_key_prefers_exact_over_prefix() {
        let mut state = DaemonState::new();
        // "abc" is an exact match key AND a prefix of "abc-longer"
        state.sessions.insert("abc".into(), make_session("abc"));
        state.sessions.insert("abc-longer".into(), make_session("abc-longer"));
        assert_eq!(
            resolve_session_key("abc", &state),
            Some("abc".to_string())
        );
    }

    // -- resolve_jsonl_path --

    #[test]
    fn resolve_jsonl_path_exact_match() {
        let mut state = DaemonState::new();
        let mut session = make_session("sess-1");
        session.jsonl_path = Some("/tmp/test.jsonl".into());
        state.sessions.insert("sess-1".into(), session);

        let result = resolve_jsonl_path("sess-1", &state);
        assert_eq!(result, Some(std::path::PathBuf::from("/tmp/test.jsonl")));
    }

    #[test]
    fn resolve_jsonl_path_prefix_match() {
        let mut state = DaemonState::new();
        let mut session = make_session("sess-1-full-uuid");
        session.jsonl_path = Some("/tmp/test.jsonl".into());
        state.sessions.insert("sess-1-full-uuid".into(), session);

        let result = resolve_jsonl_path("sess-1", &state);
        assert_eq!(result, Some(std::path::PathBuf::from("/tmp/test.jsonl")));
    }

    #[test]
    fn resolve_jsonl_path_no_jsonl_returns_none() {
        let mut state = DaemonState::new();
        let session = make_session("sess-1");
        // jsonl_path is None
        state.sessions.insert("sess-1".into(), session);

        let result = resolve_jsonl_path("sess-1", &state);
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_jsonl_path_no_match_returns_none() {
        let state = DaemonState::new();
        // No filesystem match for this ID either (no real files to find).
        let result = resolve_jsonl_path("nonexistent-id-xyz-99999", &state);
        assert_eq!(result, None);
    }
}

/// Save base64-encoded images to temporary files and return their paths.
fn save_images_to_temp(images: &[ImageAttachment]) -> Result<Vec<String>, std::io::Error> {
    let dir = std::path::Path::new("/tmp/agent-dash-images");
    std::fs::create_dir_all(dir)?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let engine = base64::engine::general_purpose::STANDARD;
    let mut paths = Vec::with_capacity(images.len());

    for (i, img) in images.iter().enumerate() {
        let ext = match img.mime_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        let filename = format!("img-{ts}-{i}.{ext}");
        let path = dir.join(&filename);
        let data = engine
            .decode(&img.data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, &data)?;
        paths.push(path.to_string_lossy().to_string());
    }

    Ok(paths)
}

/// Atomically write state.json (write to .tmp then rename).
fn write_state_file(state: &DaemonState) {
    let dash = DashState {
        sessions: state.to_dash_sessions(),
    };
    let Ok(json) = serde_json::to_string_pretty(&dash) else {
        return;
    };

    let state_path = paths::state_file_path();
    let tmp_path = state_path.with_extension("json.tmp");

    if std::fs::write(&tmp_path, &json).is_ok() {
        let _ = std::fs::rename(&tmp_path, &state_path);
    }
}
