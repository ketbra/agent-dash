use agent_dash_core::paths;
use agent_dash_core::protocol::{self, HookEvent, HookPermissionDecision, ServerEvent};
use agent_dash_core::session::DashState;
use agent_dashd::client_listener::{self, ClientMessage};
use agent_dashd::hook_listener;
use agent_dashd::scanner;
use agent_dashd::state::DaemonState;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

#[tokio::main]
async fn main() {
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

    // Channels
    let (hook_tx, mut hook_rx) = mpsc::channel::<HookEvent>(256);
    let (client_tx, mut client_rx) = mpsc::channel::<ClientMessage>(256);

    // Spawn listeners
    tokio::spawn(hook_listener::run(hook_tx));
    tokio::spawn(client_listener::run(client_tx));

    // Main loop state
    let mut state = DaemonState::new();
    let mut subscribers: Vec<mpsc::Sender<String>> = Vec::new();
    let mut permission_waiters: HashMap<String, oneshot::Sender<HookPermissionDecision>> =
        HashMap::new();
    let mut scan_interval = tokio::time::interval(Duration::from_secs(5));
    let mut write_interval = tokio::time::interval(Duration::from_millis(500));
    let mut state_dirty = false;

    // Don't delay the first tick.
    scan_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    write_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // --- Hook events ---
            Some(event) = hook_rx.recv() => {
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

                state.apply_hook_event(event);
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
                        // Send any pending permissions.
                        for perm in state.pending_permissions.values() {
                            let pending = ServerEvent::PermissionPending {
                                session_id: perm.session_id.clone(),
                                request_id: perm.request_id.clone(),
                                tool: perm.tool.clone(),
                                detail: perm.detail.clone(),
                            };
                            if let Ok(line) = protocol::encode_line(&pending) {
                                let _ = tx.try_send(line);
                            }
                        }
                        subscribers.push(tx);
                    }
                    ClientMessage::GetState { reply } => {
                        let event = ServerEvent::StateUpdate {
                            sessions: state.to_dash_sessions(),
                        };
                        if let Ok(json) = protocol::encode_line(&event) {
                            let _ = reply.send(json);
                        }
                    }
                    ClientMessage::PermissionResponse {
                        request_id,
                        decision,
                        ..
                    } => {
                        if let Some(perm) = state.resolve_permission(&request_id) {
                            // Notify the waiting hook.
                            if let Some(waiter) = permission_waiters.remove(&request_id) {
                                let _ = waiter.send(HookPermissionDecision {
                                    request_id: request_id.clone(),
                                    decision: decision.clone(),
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
                        reply,
                    } => {
                        state.add_permission_request(
                            &session_id,
                            &request_id,
                            &tool,
                            &detail,
                        );
                        permission_waiters.insert(request_id.clone(), reply);

                        // Broadcast permission pending.
                        let pending = ServerEvent::PermissionPending {
                            session_id,
                            request_id,
                            tool,
                            detail,
                        };
                        broadcast_to_subscribers(&mut subscribers, &pending);
                        state_dirty = true;
                        broadcast_state(&mut subscribers, &state);
                    }
                }
            }

            // --- Periodic process scan ---
            _ = scan_interval.tick() => {
                let processes = scanner::scan_claude_processes();
                let projects_dir = paths::claude_projects_dir();

                // Group processes by project slug to deduplicate subagents
                // sharing the same CWD.
                let mut slug_groups: HashMap<String, Vec<(u32, &scanner::ClaudeProcess)>> =
                    HashMap::new();
                for (pid, proc_info) in &processes {
                    let slug = paths::cwd_to_project_slug(&proc_info.cwd);
                    slug_groups.entry(slug).or_default().push((*pid, proc_info));
                }

                let mut active_session_ids: HashSet<String> = HashSet::new();

                for (slug, group) in &mut slug_groups {
                    // Sort by PID for a stable representative.
                    group.sort_by_key(|(pid, _)| *pid);
                    let (representative_pid, proc_info) = group[0];
                    let project_name = paths::project_name_from_cwd(&proc_info.cwd);

                    // Look up JSONL once per slug, not per PID.
                    let project_dir = projects_dir.join(slug.as_str());
                    let jsonl_path = scanner::find_latest_jsonl(&project_dir);

                    let (session_id, branch, has_pending_question, question_text) =
                        if let Some(ref jsonl) = jsonl_path {
                            if let Some(status) = scanner::parse_jsonl_status(jsonl) {
                                (
                                    status.session_id,
                                    status.git_branch,
                                    status.has_pending_question,
                                    status.question_text,
                                )
                            } else {
                                // No parseable session info; use slug as fallback ID.
                                (format!("scan-{slug}"), String::new(), false, None)
                            }
                        } else {
                            (format!("scan-{slug}"), String::new(), false, None)
                        };

                    active_session_ids.insert(session_id.clone());
                    state.ensure_session(&session_id);
                    if let Some(session) = state.sessions.get_mut(&session_id) {
                        session.pid = Some(representative_pid);
                        session.cwd = Some(proc_info.cwd.to_string_lossy().to_string());
                        session.project_name = project_name;
                        session.branch = branch;
                        session.has_pending_question = has_pending_question;
                        session.question_text = question_text;
                        if let Some(ref jsonl) = jsonl_path {
                            session.jsonl_path =
                                Some(jsonl.to_string_lossy().to_string());
                        }
                    }
                }

                // Prune sessions no longer active.
                // Keep hook-only sessions (pid=None) if updated within last 5 minutes.
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                state.sessions.retain(|id, session| {
                    if active_session_ids.contains(id) {
                        return true;
                    }
                    // Hook-only sessions have no PID; keep if recently active.
                    if session.pid.is_none() {
                        return now_secs.saturating_sub(session.last_status_change) < 300;
                    }
                    false
                });

                state_dirty = true;
                broadcast_state(&mut subscribers, &state);
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
