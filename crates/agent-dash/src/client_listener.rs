use agent_dash_core::paths;
use agent_dash_core::protocol::{self, ClientRequest, HookPermissionDecision, ImageAttachment, ServerEvent};
use interprocess::local_socket::{
    tokio::prelude::*,
    GenericFilePath, ListenerOptions,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Messages from client connections to the main state manager.
pub enum ClientMessage {
    /// Client wants to subscribe to state updates.
    Subscribe {
        tx: mpsc::Sender<String>,
    },
    /// Client requests current state snapshot.
    GetState {
        include_subagents: bool,
        reply: oneshot::Sender<String>,
    },
    /// Client sent a permission response (allow/deny/allow_similar).
    PermissionResponse {
        request_id: String,
        session_id: String,
        decision: String,
        suggestion: Option<serde_json::Value>,
    },
    /// Hook binary sent a permission request (needs response back).
    PermissionRequest {
        request_id: String,
        session_id: String,
        tool: String,
        detail: String,
        suggestions: Vec<serde_json::Value>,
        reply: oneshot::Sender<HookPermissionDecision>,
    },
    /// Client requests last N messages from a session.
    GetMessages {
        session_id: String,
        format: String,
        limit: usize,
        reply: oneshot::Sender<String>,
    },
    /// Client wants to stream new messages from a session.
    WatchSession {
        session_id: String,
        format: String,
        tx: mpsc::Sender<String>,
    },
    /// Client stops streaming messages from a session.
    UnwatchSession {
        session_id: String,
    },
    /// Client requests all sessions for a project.
    ListSessions {
        project: String,
        reply: oneshot::Sender<String>,
    },
    /// Wrapper registering itself.
    RegisterWrapper {
        session_id: String,
        agent: String,
        cwd: Option<String>,
        branch: Option<String>,
        project_name: Option<String>,
        real_session_id: Option<String>,
        wrapper_tx: mpsc::Sender<ServerEvent>,
    },
    /// Wrapper unregistering.
    UnregisterWrapper {
        session_id: String,
    },
    /// Client requesting prompt injection.
    SendPrompt {
        session_id: String,
        text: String,
        images: Vec<ImageAttachment>,
        reply: oneshot::Sender<String>,
    },
    /// Wrapper updating prompt suggestion for a session.
    UpdateSuggestion {
        session_id: String,
        suggestion: Option<String>,
    },
    /// Wrapper updating thinking text for a session.
    UpdateThinkingText {
        session_id: String,
        thinking_text: Option<String>,
    },
    /// Client wants to watch raw terminal output for a session.
    WatchTerminal {
        session_id: String,
        tx: mpsc::Sender<String>,
    },
    /// Client stops watching terminal output for a session.
    UnwatchTerminal {
        session_id: String,
    },
    /// Wrapper forwarding raw terminal output (base64-encoded).
    TerminalOutput {
        session_id: String,
        data: String,
    },
    /// Client requests creation of a new headless session.
    CreateSession {
        agent: Option<String>,
        cwd: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
        reply: oneshot::Sender<String>,
    },
    /// Client sending raw terminal input (base64-encoded).
    TerminalInput {
        session_id: String,
        data: String,
    },
    /// Client requesting terminal resize for a session.
    TerminalResize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
}

/// Run the client listener. Accepts persistent bidirectional connections on
/// daemon.sock.
pub async fn run(tx: mpsc::Sender<ClientMessage>) {
    let name = paths::client_socket_name();

    // Ensure parent directory exists.
    let path = std::path::Path::new(&name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Remove stale socket file.
    let _ = std::fs::remove_file(&name);

    let listener = match ListenerOptions::new()
        .name(
            name.as_str()
                .to_fs_name::<GenericFilePath>()
                .expect("invalid socket path"),
        )
        .create_tokio()
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!("agent-dashd: failed to bind client socket: {e}");
            return;
        }
    };

    eprintln!("agent-dashd: client listener on {name}");

    loop {
        match listener.accept().await {
            Ok(conn) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_client_connection(conn, tx).await;
                });
            }
            Err(e) => {
                eprintln!("agent-dashd: client accept error: {e}");
            }
        }
    }
}

/// Handle a single persistent client connection. Reads JSON lines from the
/// client, dispatches each request, and writes responses/events back.
async fn handle_client_connection(
    conn: interprocess::local_socket::tokio::Stream,
    tx: mpsc::Sender<ClientMessage>,
) {
    let (reader, mut writer) = conn.split();
    let buf_reader = BufReader::new(reader);
    let mut lines = buf_reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req = match serde_json::from_str::<ClientRequest>(trimmed) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("agent-dashd: invalid client request: {e}");
                continue;
            }
        };

        match req {
            ClientRequest::Subscribe => {
                // Create an mpsc channel for this subscriber. Send the sender
                // to the state manager so it can push events to us.
                let (sub_tx, mut sub_rx) = mpsc::channel::<String>(64);
                let _ = tx.send(ClientMessage::Subscribe { tx: sub_tx }).await;

                // Stream events to the client until they disconnect or the
                // receiver closes.
                while let Some(msg) = sub_rx.recv().await {
                    if writer.write_all(msg.as_bytes()).await.is_err() {
                        break;
                    }
                }
                // Connection is done after subscribe stream ends.
                return;
            }
            ClientRequest::GetState { include_subagents } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx.send(ClientMessage::GetState { include_subagents, reply: reply_tx }).await;
                if let Ok(json) = reply_rx.await {
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
            ClientRequest::PermissionResponse {
                request_id,
                session_id,
                decision,
                suggestion,
            } => {
                let _ = tx
                    .send(ClientMessage::PermissionResponse {
                        request_id,
                        session_id,
                        decision,
                        suggestion,
                    })
                    .await;
            }
            ClientRequest::PermissionRequest {
                request_id,
                session_id,
                tool,
                detail,
                suggestions,
            } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx
                    .send(ClientMessage::PermissionRequest {
                        request_id,
                        session_id,
                        tool,
                        detail,
                        suggestions,
                        reply: reply_tx,
                    })
                    .await;

                // Wait for the permission decision from the state manager and
                // send it back to the hook binary.
                if let Ok(decision) = reply_rx.await {
                    if let Ok(line) = protocol::encode_line(&decision) {
                        let _ = writer.write_all(line.as_bytes()).await;
                    }
                }
            }
            ClientRequest::GetMessages {
                session_id,
                format,
                limit,
            } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx
                    .send(ClientMessage::GetMessages {
                        session_id,
                        format: format.unwrap_or_else(|| "structured".into()),
                        limit: limit.unwrap_or(50),
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
            ClientRequest::WatchSession {
                session_id,
                format,
            } => {
                let (sub_tx, mut sub_rx) = mpsc::channel::<String>(64);
                let _ = tx
                    .send(ClientMessage::WatchSession {
                        session_id: session_id.clone(),
                        format: format.unwrap_or_else(|| "structured".into()),
                        tx: sub_tx,
                    })
                    .await;

                // Stream messages until disconnect.
                while let Some(msg) = sub_rx.recv().await {
                    if writer.write_all(msg.as_bytes()).await.is_err() {
                        break;
                    }
                }

                // Clean up the watch on disconnect.
                let _ = tx
                    .send(ClientMessage::UnwatchSession {
                        session_id,
                    })
                    .await;
                return;
            }
            ClientRequest::UnwatchSession { session_id } => {
                let _ = tx
                    .send(ClientMessage::UnwatchSession { session_id })
                    .await;
            }
            ClientRequest::ListSessions { project } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx
                    .send(ClientMessage::ListSessions {
                        project,
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
            ClientRequest::RegisterWrapper { session_id, agent, cwd, branch, project_name, real_session_id } => {
                // Create a channel for wrapper commands (prompt injection, terminal input, etc).
                let (wrapper_tx, mut wrapper_rx) = mpsc::channel::<ServerEvent>(16);
                let _ = tx
                    .send(ClientMessage::RegisterWrapper {
                        session_id: session_id.clone(),
                        agent,
                        cwd,
                        branch,
                        project_name,
                        real_session_id,
                        wrapper_tx,
                    })
                    .await;

                // Stream events to this wrapper while also monitoring the
                // client reader for disconnect (EOF) or explicit messages.
                loop {
                    tokio::select! {
                        event = wrapper_rx.recv() => {
                            match event {
                                Some(event) => {
                                    if let Ok(line) = protocol::encode_line(&event) {
                                        if writer.write_all(line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                None => break, // daemon dropped wrapper channel
                            }
                        }
                        line_result = lines.next_line() => {
                            match line_result {
                                Ok(Some(line)) => {
                                    // Client sent a message in wrapper mode.
                                    // Handle explicit unregister; ignore others.
                                    let trimmed = line.trim();
                                    if !trimmed.is_empty() {
                                        match serde_json::from_str::<ClientRequest>(trimmed) {
                                            Ok(ClientRequest::UnregisterWrapper { .. }) => {
                                                break;
                                            }
                                            Ok(ClientRequest::UpdateSuggestion { session_id, suggestion }) => {
                                                let _ = tx.send(ClientMessage::UpdateSuggestion { session_id, suggestion }).await;
                                            }
                                            Ok(ClientRequest::UpdateThinkingText { session_id, thinking_text }) => {
                                                let _ = tx.send(ClientMessage::UpdateThinkingText { session_id, thinking_text }).await;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                _ => break, // EOF or error — client disconnected
                            }
                        }
                    }
                }

                // Clean up on disconnect.
                let _ = tx
                    .send(ClientMessage::UnregisterWrapper { session_id })
                    .await;
                return;
            }
            ClientRequest::UnregisterWrapper { session_id } => {
                let _ = tx
                    .send(ClientMessage::UnregisterWrapper { session_id })
                    .await;
            }
            ClientRequest::UpdateSuggestion { session_id, suggestion } => {
                let _ = tx.send(ClientMessage::UpdateSuggestion { session_id, suggestion }).await;
            }
            ClientRequest::UpdateThinkingText { session_id, thinking_text } => {
                let _ = tx.send(ClientMessage::UpdateThinkingText { session_id, thinking_text }).await;
            }
            ClientRequest::TerminalOutput { session_id, data } => {
                let _ = tx.send(ClientMessage::TerminalOutput { session_id, data }).await;
            }
            ClientRequest::WatchTerminal { session_id } => {
                let (sub_tx, mut sub_rx) = mpsc::channel::<String>(64);
                let _ = tx.send(ClientMessage::WatchTerminal { session_id: session_id.clone(), tx: sub_tx }).await;
                // Stream terminal data until disconnect.
                while let Some(msg) = sub_rx.recv().await {
                    if writer.write_all(msg.as_bytes()).await.is_err() {
                        break;
                    }
                }
                let _ = tx.send(ClientMessage::UnwatchTerminal { session_id }).await;
                return;
            }
            ClientRequest::UnwatchTerminal { session_id } => {
                let _ = tx.send(ClientMessage::UnwatchTerminal { session_id }).await;
            }
            ClientRequest::SendPrompt { session_id, text, images } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx
                    .send(ClientMessage::SendPrompt {
                        session_id,
                        text,
                        images,
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
            ClientRequest::CreateSession { agent, cwd, cols, rows } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx
                    .send(ClientMessage::CreateSession {
                        agent,
                        cwd,
                        cols,
                        rows,
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
            ClientRequest::TerminalInput { session_id, data } => {
                let _ = tx
                    .send(ClientMessage::TerminalInput { session_id, data })
                    .await;
            }
            ClientRequest::TerminalResize { session_id, cols, rows } => {
                let _ = tx
                    .send(ClientMessage::TerminalResize { session_id, cols, rows })
                    .await;
            }
        }
    }
}
