use agent_dash_core::paths;
use agent_dash_core::protocol::{self, ClientRequest, HookPermissionDecision};
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
        reply: oneshot::Sender<String>,
    },
    /// Client sent a permission response (allow/deny/allow_similar).
    PermissionResponse {
        request_id: String,
        session_id: String,
        decision: String,
    },
    /// Hook binary sent a permission request (needs response back).
    PermissionRequest {
        request_id: String,
        session_id: String,
        tool: String,
        detail: String,
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
            ClientRequest::GetState => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx.send(ClientMessage::GetState { reply: reply_tx }).await;
                if let Ok(json) = reply_rx.await {
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
            ClientRequest::PermissionResponse {
                request_id,
                session_id,
                decision,
            } => {
                let _ = tx
                    .send(ClientMessage::PermissionResponse {
                        request_id,
                        session_id,
                        decision,
                    })
                    .await;
            }
            ClientRequest::PermissionRequest {
                request_id,
                session_id,
                tool,
                detail,
            } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx
                    .send(ClientMessage::PermissionRequest {
                        request_id,
                        session_id,
                        tool,
                        detail,
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
            // Wrapper-related requests will be handled in a future task.
            _ => {
                eprintln!("agent-dashd: unhandled client request");
            }
        }
    }
}
