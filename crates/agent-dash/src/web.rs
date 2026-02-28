use crate::client_listener::ClientMessage;
use agent_dash_core::protocol::{ClientRequest, ServerEvent};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::sync::{mpsc, oneshot};

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");

#[derive(Clone)]
struct AppState {
    client_tx: mpsc::Sender<ClientMessage>,
}

pub async fn run(addr: std::net::SocketAddr, client_tx: mpsc::Sender<ClientMessage>) {
    let state = AppState { client_tx };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(js_handler))
        .route("/style.css", get(css_handler))
        .route("/ws", get(ws_handler))
        .with_state(state);

    eprintln!("  web interface: http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind web server");
    axum::serve(listener, app).await.expect("web server error");
}

async fn index_handler() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn js_handler() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/javascript")],
        APP_JS,
    )
        .into_response()
}

async fn css_handler() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/css")],
        STYLE_CSS,
    )
        .into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| handle_ws(socket, state.client_tx))
}

async fn handle_ws(socket: WebSocket, client_tx: mpsc::Sender<ClientMessage>) {
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Channel for daemon events -> WebSocket. Both the state subscriber and
    // message watchers send serialized JSON through this single channel.
    let (event_tx, mut event_rx) = mpsc::channel::<String>(256);

    // Auto-subscribe for state updates.
    let _ = client_tx
        .send(ClientMessage::Subscribe {
            tx: event_tx.clone(),
        })
        .await;

    // Track which session we're watching so we can unwatch on disconnect.
    let watched_session: std::sync::Arc<tokio::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let watched_terminal: std::sync::Arc<tokio::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(None));

    let watched_clone = watched_session.clone();
    let watched_term_clone = watched_terminal.clone();
    let client_tx_clone = client_tx.clone();
    let event_tx_clone = event_tx.clone();

    // Forward daemon events -> WebSocket.
    let send_task = tokio::spawn(async move {
        while let Some(msg) = event_rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Read WebSocket messages -> dispatch to daemon.
    while let Some(Ok(msg)) = ws_rx.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            _ => continue,
        };

        let req = match serde_json::from_str::<ClientRequest>(&text) {
            Ok(r) => r,
            Err(_) => continue,
        };

        match req {
            ClientRequest::GetState { include_subagents } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = client_tx_clone
                    .send(ClientMessage::GetState {
                        include_subagents,
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = event_tx_clone.send(json).await;
                }
            }
            ClientRequest::GetMessages {
                session_id,
                format,
                limit,
            } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = client_tx_clone
                    .send(ClientMessage::GetMessages {
                        session_id,
                        format: format.unwrap_or_else(|| "html".into()),
                        limit: limit.unwrap_or(50),
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = event_tx_clone.send(json).await;
                }
            }
            ClientRequest::WatchSession { session_id, format } => {
                // Unwatch previous session if any.
                let mut watched = watched_clone.lock().await;
                if let Some(prev) = watched.take() {
                    let _ = client_tx_clone
                        .send(ClientMessage::UnwatchSession { session_id: prev })
                        .await;
                }
                *watched = Some(session_id.clone());
                drop(watched);

                let _ = client_tx_clone
                    .send(ClientMessage::WatchSession {
                        session_id,
                        format: format.unwrap_or_else(|| "html".into()),
                        tx: event_tx_clone.clone(),
                    })
                    .await;
            }
            ClientRequest::UnwatchSession { session_id } => {
                let mut watched = watched_clone.lock().await;
                if watched.as_deref() == Some(&session_id) {
                    *watched = None;
                }
                drop(watched);
                let _ = client_tx_clone
                    .send(ClientMessage::UnwatchSession { session_id })
                    .await;
            }
            ClientRequest::SendPrompt { session_id, text, images } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = client_tx_clone
                    .send(ClientMessage::SendPrompt {
                        session_id,
                        text,
                        images,
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = event_tx_clone.send(json).await;
                }
            }
            ClientRequest::PermissionResponse {
                request_id,
                session_id,
                decision,
                suggestion,
            } => {
                let _ = client_tx_clone
                    .send(ClientMessage::PermissionResponse {
                        request_id,
                        session_id,
                        decision,
                        suggestion,
                    })
                    .await;
            }
            ClientRequest::ListSessions { project } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = client_tx_clone
                    .send(ClientMessage::ListSessions {
                        project,
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = event_tx_clone.send(json).await;
                }
            }
            ClientRequest::WatchTerminal { session_id } => {
                // Unwatch previous terminal if any.
                let mut watched = watched_term_clone.lock().await;
                if let Some(prev) = watched.take() {
                    let _ = client_tx_clone
                        .send(ClientMessage::UnwatchTerminal { session_id: prev })
                        .await;
                }
                *watched = Some(session_id.clone());
                drop(watched);

                let _ = client_tx_clone
                    .send(ClientMessage::WatchTerminal {
                        session_id,
                        tx: event_tx_clone.clone(),
                    })
                    .await;
            }
            ClientRequest::UnwatchTerminal { session_id } => {
                let mut watched = watched_term_clone.lock().await;
                if watched.as_deref() == Some(&session_id) {
                    *watched = None;
                }
                drop(watched);
                let _ = client_tx_clone
                    .send(ClientMessage::UnwatchTerminal { session_id })
                    .await;
            }
            ClientRequest::CreateSession { agent, cwd, cols, rows } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = client_tx_clone
                    .send(ClientMessage::CreateSession {
                        agent,
                        cwd,
                        cols,
                        rows,
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(json) = reply_rx.await {
                    let _ = event_tx_clone.send(json).await;
                }
            }
            ClientRequest::TerminalInput { session_id, data } => {
                let _ = client_tx_clone
                    .send(ClientMessage::TerminalInput { session_id, data })
                    .await;
            }
            ClientRequest::TerminalResize { session_id, cols, rows } => {
                let _ = client_tx_clone
                    .send(ClientMessage::TerminalResize { session_id, cols, rows })
                    .await;
            }
            ClientRequest::ListDirectory { path } => {
                let dir = match path {
                    Some(ref p) if !p.is_empty() => std::path::PathBuf::from(p),
                    _ => dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/")),
                };
                let response = match std::fs::read_dir(&dir) {
                    Ok(rd) => {
                        let mut entries: Vec<String> = rd
                            .filter_map(|e| e.ok())
                            .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                            .filter_map(|e| e.file_name().into_string().ok())
                            .filter(|name| !name.starts_with('.'))
                            .collect();
                        entries.sort();
                        entries.truncate(200);
                        serde_json::to_string(&ServerEvent::DirectoryListing {
                            path: dir.to_string_lossy().into_owned(),
                            entries,
                        })
                    }
                    Err(e) => serde_json::to_string(&ServerEvent::Error {
                        message: format!("list_directory: {e}"),
                    }),
                };
                if let Ok(json) = response {
                    let _ = event_tx_clone.send(json).await;
                }
            }
            // Ignore wrapper-only requests from the web UI.
            _ => {}
        }
    }

    // Clean up: unwatch any session/terminal and abort the send task.
    let watched = watched_session.lock().await;
    if let Some(session_id) = watched.as_ref() {
        let _ = client_tx
            .send(ClientMessage::UnwatchSession {
                session_id: session_id.clone(),
            })
            .await;
    }
    drop(watched);
    let watched_term = watched_terminal.lock().await;
    if let Some(session_id) = watched_term.as_ref() {
        let _ = client_tx
            .send(ClientMessage::UnwatchTerminal {
                session_id: session_id.clone(),
            })
            .await;
    }
    drop(watched_term);
    send_task.abort();
}
