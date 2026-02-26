# Web Interface Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an embedded web dashboard to agent-dashd for viewing sessions, reading conversations, injecting prompts, and responding to permission requests.

**Architecture:** An axum HTTP server runs inside the daemon process, sharing the same `client_tx` channel. A single multiplexed WebSocket endpoint handles all real-time communication. Vanilla HTML/JS/CSS is embedded in the binary via `include_str!`.

**Tech Stack:** axum (HTTP + WebSocket), tokio (existing), vanilla JS frontend, no build step.

**Design doc:** `docs/plans/2026-02-25-web-interface-design.md`

---

### Task 1: Add axum dependency and web module skeleton

**Files:**
- Modify: `Cargo.toml` (workspace root — add axum + tower to workspace deps)
- Modify: `crates/agent-dash/Cargo.toml` (add axum + tower-http)
- Create: `crates/agent-dash/src/web.rs`
- Modify: `crates/agent-dash/src/main.rs` (add `mod web;`)

**Step 1: Add dependencies**

In workspace root `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
axum = { version = "0.8", features = ["ws"] }
tower-http = { version = "0.6", features = ["cors"] }
```

In `crates/agent-dash/Cargo.toml`, add to `[dependencies]`:

```toml
axum = { workspace = true }
tower-http = { workspace = true }
```

**Step 2: Create minimal web module**

Create `crates/agent-dash/src/web.rs`:

```rust
use crate::client_listener::ClientMessage;
use axum::{extract::ws, response::Html, routing::get, Router};
use tokio::sync::mpsc;

/// Start the web server on the given port. Pass 0 to disable.
pub async fn run(port: u16, _client_tx: mpsc::Sender<ClientMessage>) {
    if port == 0 {
        return;
    }

    let app = Router::new()
        .route("/", get(index_handler));

    let addr = format!("127.0.0.1:{port}");
    eprintln!("  web interface: http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind web server");
    axum::serve(listener, app).await.expect("web server error");
}

async fn index_handler() -> Html<&'static str> {
    Html("<h1>agent-dash</h1>")
}
```

**Step 3: Register module in main.rs**

Add `mod web;` after the existing module declarations in `crates/agent-dash/src/main.rs`.

**Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: compiles with no new errors (web module unused for now, that's fine)

**Step 5: Commit**

```
feat: add axum dependency and web module skeleton
```

---

### Task 2: Wire web server into daemon startup with --web-port flag

**Files:**
- Modify: `crates/agent-dash/src/main.rs` (add --web-port to DaemonAction::Start)
- Modify: `crates/agent-dash/src/daemon.rs` (accept port param, spawn web listener)

**Step 1: Add --web-port flag to DaemonAction::Start**

In `crates/agent-dash/src/main.rs`, change:

```rust
#[derive(Debug, Subcommand)]
enum DaemonAction {
    /// Start the daemon
    Start,
```

To:

```rust
#[derive(Debug, Subcommand)]
enum DaemonAction {
    /// Start the daemon
    Start {
        /// Port for web interface (0 to disable)
        #[arg(long, default_value = "3131")]
        web_port: u16,
    },
```

Update the match arm in `main()` from:

```rust
DaemonAction::Start => {
    daemon::run().await;
}
```

To:

```rust
DaemonAction::Start { web_port } => {
    daemon::run(web_port).await;
}
```

**Step 2: Update daemon::run() to accept port and spawn web server**

Change `daemon::run()` signature from:

```rust
pub async fn run() {
```

To:

```rust
pub async fn run(web_port: u16) {
```

After the existing `tokio::spawn(client_listener::run(client_tx));` line (line 33), add:

```rust
tokio::spawn(crate::web::run(web_port, client_tx.clone()));
```

Note: `client_tx` must be cloned since it's also used later in the function. The existing `client_tx` is already a `mpsc::Sender` which is `Clone`.

**Step 3: Verify it compiles and runs**

Run: `cargo check --workspace`
Expected: compiles clean

Run: `cargo run -- daemon start --web-port 3131` (manual test — Ctrl-C to stop)
Expected: prints `web interface: http://127.0.0.1:3131` alongside socket paths

**Step 4: Commit**

```
feat: wire web server into daemon with --web-port flag
```

---

### Task 3: Embed static assets and serve HTML/JS/CSS

**Files:**
- Create: `crates/agent-dash/web/index.html`
- Create: `crates/agent-dash/web/style.css`
- Create: `crates/agent-dash/web/app.js`
- Modify: `crates/agent-dash/src/web.rs` (serve embedded assets)

**Step 1: Create placeholder HTML**

Create `crates/agent-dash/web/index.html`:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>agent-dash</title>
  <link rel="stylesheet" href="/style.css">
</head>
<body>
  <div id="app">
    <aside id="sidebar">
      <h2>Sessions</h2>
      <ul id="session-list"></ul>
    </aside>
    <main id="main">
      <div id="permission-banner" class="hidden"></div>
      <div id="messages"></div>
      <form id="prompt-form" class="hidden">
        <input type="text" id="prompt-input" placeholder="Send a prompt..." autocomplete="off">
        <button type="submit">Send</button>
      </form>
    </main>
  </div>
  <script src="/app.js"></script>
</body>
</html>
```

**Step 2: Create placeholder CSS**

Create `crates/agent-dash/web/style.css`:

```css
* { margin: 0; padding: 0; box-sizing: border-box; }

:root {
  --bg: #1a1b26;
  --bg-sidebar: #16161e;
  --bg-surface: #24283b;
  --text: #c0caf5;
  --text-dim: #565f89;
  --accent: #7aa2f7;
  --green: #9ece6a;
  --yellow: #e0af68;
  --red: #f7768e;
  --border: #292e42;
  --font-mono: 'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace;
  --font-sans: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
}

html, body { height: 100%; background: var(--bg); color: var(--text); font-family: var(--font-sans); }

#app {
  display: flex;
  height: 100vh;
}

#sidebar {
  width: 280px;
  min-width: 280px;
  background: var(--bg-sidebar);
  border-right: 1px solid var(--border);
  overflow-y: auto;
  padding: 16px;
}

#sidebar h2 {
  font-size: 14px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--text-dim);
  margin-bottom: 12px;
}

#session-list {
  list-style: none;
}

#session-list li {
  padding: 10px 12px;
  border-radius: 6px;
  cursor: pointer;
  margin-bottom: 4px;
  font-size: 13px;
}

#session-list li:hover { background: var(--bg-surface); }
#session-list li.active { background: var(--bg-surface); border-left: 3px solid var(--accent); }

.session-project { font-weight: 600; color: var(--text); }
.session-branch { color: var(--text-dim); font-size: 12px; }
.session-meta { display: flex; align-items: center; gap: 6px; margin-top: 4px; }

.status-dot {
  width: 8px; height: 8px; border-radius: 50%; display: inline-block;
}
.status-dot.working { background: var(--green); }
.status-dot.needs_input { background: var(--yellow); }
.status-dot.idle { background: var(--text-dim); }
.status-dot.ended { background: var(--red); }

.badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 10px;
  background: var(--yellow);
  color: var(--bg);
  font-weight: 600;
}

#main {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

#messages {
  flex: 1;
  overflow-y: auto;
  padding: 16px 24px;
  font-family: var(--font-mono);
  font-size: 13px;
  line-height: 1.6;
}

#messages .msg-user { color: var(--accent); margin-bottom: 12px; }
#messages .msg-assistant { color: var(--text); margin-bottom: 12px; }
#messages strong { color: var(--yellow); }
#messages code { background: var(--bg-surface); padding: 2px 5px; border-radius: 3px; font-size: 12px; }
#messages pre {
  background: var(--bg-surface);
  padding: 12px;
  border-radius: 6px;
  overflow-x: auto;
  margin: 8px 0;
}
#messages pre code { background: none; padding: 0; }

#prompt-form {
  display: flex;
  padding: 12px 24px;
  border-top: 1px solid var(--border);
  background: var(--bg-sidebar);
}

#prompt-input {
  flex: 1;
  padding: 10px 14px;
  background: var(--bg-surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text);
  font-family: var(--font-mono);
  font-size: 13px;
  outline: none;
}

#prompt-input:focus { border-color: var(--accent); }

#prompt-form button {
  margin-left: 8px;
  padding: 10px 20px;
  background: var(--accent);
  color: var(--bg);
  border: none;
  border-radius: 6px;
  font-weight: 600;
  cursor: pointer;
}

#prompt-form button:hover { opacity: 0.9; }

#permission-banner {
  padding: 12px 24px;
  background: #2a2040;
  border-bottom: 1px solid var(--yellow);
  display: flex;
  align-items: center;
  gap: 12px;
}

#permission-banner .perm-info { flex: 1; }
#permission-banner .perm-tool { font-weight: 600; color: var(--yellow); }
#permission-banner .perm-detail { font-family: var(--font-mono); font-size: 12px; color: var(--text-dim); margin-top: 2px; }

#permission-banner button {
  padding: 6px 16px;
  border: none;
  border-radius: 4px;
  font-weight: 600;
  cursor: pointer;
}
#permission-banner .btn-allow { background: var(--green); color: var(--bg); }
#permission-banner .btn-deny { background: var(--red); color: var(--bg); }
#permission-banner .btn-suggestion { background: var(--bg-surface); color: var(--text-dim); font-size: 12px; }

.hidden { display: none !important; }

#empty-state {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-dim);
  font-size: 15px;
}
```

**Step 3: Create placeholder JS**

Create `crates/agent-dash/web/app.js`:

```javascript
// agent-dash web interface
(function() {
  'use strict';
  console.log('agent-dash web UI loaded');
})();
```

**Step 4: Serve embedded assets in web.rs**

Update `crates/agent-dash/src/web.rs`:

```rust
use crate::client_listener::ClientMessage;
use axum::{response::Html, routing::get, Router};
use axum::response::{IntoResponse, Response};
use axum::http::{header, StatusCode};
use tokio::sync::mpsc;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const STYLE_CSS: &str = include_str!("../web/style.css");

pub async fn run(port: u16, _client_tx: mpsc::Sender<ClientMessage>) {
    if port == 0 {
        return;
    }

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(js_handler))
        .route("/style.css", get(css_handler));

    let addr = format!("127.0.0.1:{port}");
    eprintln!("  web interface: http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind web server");
    axum::serve(listener, app).await.expect("web server error");
}

async fn index_handler() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn js_handler() -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/javascript")], APP_JS).into_response()
}

async fn css_handler() -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/css")], STYLE_CSS).into_response()
}
```

**Step 5: Verify it compiles**

Run: `cargo check --workspace`

**Step 6: Commit**

```
feat: embed static web assets (HTML, CSS, JS) and serve them
```

---

### Task 4: WebSocket endpoint with auto-subscribe and request handling

This is the core backend task. The WebSocket handler manages a single multiplexed connection per browser tab.

**Files:**
- Modify: `crates/agent-dash/src/web.rs` (add WebSocket handler)

**Step 1: Implement the WebSocket handler**

Replace the full `web.rs` with:

```rust
use crate::client_listener::ClientMessage;
use agent_dash_core::protocol::{self, ClientRequest, ServerEvent};
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

pub async fn run(port: u16, client_tx: mpsc::Sender<ClientMessage>) {
    if port == 0 {
        return;
    }

    let state = AppState { client_tx };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(js_handler))
        .route("/style.css", get(css_handler))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    eprintln!("  web interface: http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind web server");
    axum::serve(listener, app).await.expect("web server error");
}

async fn index_handler() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn js_handler() -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/javascript")], APP_JS).into_response()
}

async fn css_handler() -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/css")], STYLE_CSS).into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| handle_ws(socket, state.client_tx))
}

async fn handle_ws(socket: WebSocket, client_tx: mpsc::Sender<ClientMessage>) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Channel for daemon events → WebSocket. Both the state subscriber and
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

    let watched_clone = watched_session.clone();
    let client_tx_clone = client_tx.clone();
    let event_tx_clone = event_tx.clone();

    // Forward daemon events → WebSocket.
    use futures_util::SinkExt;
    let send_task = tokio::spawn(async move {
        while let Some(msg) = event_rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Read WebSocket messages → dispatch to daemon.
    use futures_util::StreamExt;
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
            ClientRequest::SendPrompt { session_id, text } => {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = client_tx_clone
                    .send(ClientMessage::SendPrompt {
                        session_id,
                        text,
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
            // Ignore wrapper-only requests from the web UI.
            _ => {}
        }
    }

    // Clean up: unwatch any session and abort the send task.
    let watched = watched_session.lock().await;
    if let Some(session_id) = watched.as_ref() {
        let _ = client_tx
            .send(ClientMessage::UnwatchSession {
                session_id: session_id.clone(),
            })
            .await;
    }
    send_task.abort();
}
```

**Step 2: Add futures-util if not already in scope**

`futures-util` is already in `crates/agent-dash/Cargo.toml` as a dependency. Verify it's there; if not, add `futures-util = "0.3"`.

**Step 3: Verify it compiles**

Run: `cargo check --workspace`

**Step 4: Commit**

```
feat: add WebSocket endpoint with auto-subscribe and request dispatch
```

---

### Task 5: Frontend JS — WebSocket connection and session list

**Files:**
- Modify: `crates/agent-dash/web/app.js`

**Step 1: Implement the full app.js**

Replace `crates/agent-dash/web/app.js` with:

```javascript
// agent-dash web interface
(function () {
  'use strict';

  // --- State ---
  let ws = null;
  let sessions = [];
  let selectedSessionId = null;
  let pendingPermissions = {}; // session_id -> permission info

  // --- DOM refs ---
  const sessionList = document.getElementById('session-list');
  const messagesEl = document.getElementById('messages');
  const promptForm = document.getElementById('prompt-form');
  const promptInput = document.getElementById('prompt-input');
  const permBanner = document.getElementById('permission-banner');

  // --- WebSocket ---
  function connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/ws`);

    ws.onopen = function () {
      console.log('WebSocket connected');
    };

    ws.onmessage = function (e) {
      const data = JSON.parse(e.data);
      handleEvent(data);
    };

    ws.onclose = function () {
      console.log('WebSocket closed, reconnecting in 2s...');
      setTimeout(connect, 2000);
    };

    ws.onerror = function () {
      ws.close();
    };
  }

  function send(msg) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }

  // --- Event handling ---
  function handleEvent(data) {
    switch (data.event) {
      case 'state_update':
        sessions = data.sessions || [];
        renderSessions();
        updatePermissions();
        break;
      case 'messages':
        renderMessages(data.messages || []);
        break;
      case 'message':
        appendMessage(data.message);
        break;
      case 'permission_pending':
        pendingPermissions[data.session_id] = data;
        updatePermissions();
        renderSessions();
        break;
      case 'permission_resolved':
        // Remove resolved permission
        for (const sid in pendingPermissions) {
          if (pendingPermissions[sid].request_id === data.request_id) {
            delete pendingPermissions[sid];
          }
        }
        updatePermissions();
        renderSessions();
        break;
      case 'prompt_sent':
        // Could show a confirmation toast
        break;
      case 'error':
        console.error('Server error:', data.message);
        break;
    }
  }

  // --- Session list ---
  function renderSessions() {
    sessionList.innerHTML = '';
    if (sessions.length === 0) {
      sessionList.innerHTML = '<li style="color:var(--text-dim);padding:8px">No active sessions</li>';
      return;
    }
    sessions.forEach(function (s) {
      const li = document.createElement('li');
      if (s.session_id === selectedSessionId) li.className = 'active';

      const hasPerm = pendingPermissions[s.session_id];
      const subagentBadge = s.subagent_count > 0
        ? ' <span style="color:var(--text-dim);font-size:11px">(+' + s.subagent_count + ')</span>'
        : '';
      const permBadge = hasPerm
        ? ' <span class="badge">!</span>'
        : '';

      li.innerHTML =
        '<div class="session-project">' + escapeHtml(s.project_name) + subagentBadge + permBadge + '</div>' +
        '<div class="session-branch">' + escapeHtml(s.branch || '') + '</div>' +
        '<div class="session-meta">' +
        '<span class="status-dot ' + (s.status || 'idle') + '"></span>' +
        '<span style="font-size:11px;color:var(--text-dim)">' + escapeHtml(s.status || 'idle') + '</span>' +
        (s.active_tool ? ' <span style="font-size:11px;color:var(--text-dim)">• ' + escapeHtml(s.active_tool.name) + '</span>' : '') +
        '</div>';

      li.onclick = function () { selectSession(s.session_id); };
      sessionList.appendChild(li);
    });
  }

  function selectSession(id) {
    if (selectedSessionId === id) return;

    // Unwatch previous
    if (selectedSessionId) {
      send({ method: 'unwatch_session', session_id: selectedSessionId });
    }

    selectedSessionId = id;
    renderSessions();
    messagesEl.innerHTML = '<div id="empty-state">Loading...</div>';
    promptForm.classList.remove('hidden');
    updatePermissions();

    // Fetch history then start watching
    send({ method: 'get_messages', session_id: id, format: 'html', limit: 100 });
    send({ method: 'watch_session', session_id: id, format: 'html' });
  }

  // --- Messages ---
  function renderMessages(msgs) {
    messagesEl.innerHTML = '';
    if (msgs.length === 0) {
      messagesEl.innerHTML = '<div id="empty-state">No messages yet</div>';
      return;
    }
    msgs.forEach(function (m) { appendMessage(m); });
  }

  function appendMessage(msg) {
    // Remove empty state if present
    const empty = messagesEl.querySelector('#empty-state');
    if (empty) empty.remove();

    const div = document.createElement('div');
    div.className = msg.role === 'user' ? 'msg-user' : 'msg-assistant';

    if (typeof msg.content === 'string') {
      div.innerHTML = msg.content;
    } else if (Array.isArray(msg.content)) {
      // Structured content — render blocks
      msg.content.forEach(function (block) {
        if (block.type === 'text') {
          const p = document.createElement('div');
          p.innerHTML = escapeHtml(block.text);
          div.appendChild(p);
        } else if (block.type === 'tool_use') {
          const t = document.createElement('div');
          t.innerHTML = '<strong>' + escapeHtml(block.name) + '</strong>: <code>' + escapeHtml(block.detail || '') + '</code>';
          div.appendChild(t);
        } else if (block.type === 'tool_result') {
          const t = document.createElement('div');
          t.style.color = 'var(--text-dim)';
          t.style.fontSize = '12px';
          t.textContent = '↳ ' + (block.output || '(no output)');
          div.appendChild(t);
        }
      });
    }

    messagesEl.appendChild(div);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  // --- Prompt injection ---
  promptForm.onsubmit = function (e) {
    e.preventDefault();
    const text = promptInput.value.trim();
    if (!text || !selectedSessionId) return;
    send({ method: 'send_prompt', session_id: selectedSessionId, text: text });
    promptInput.value = '';
  };

  // --- Permission UI ---
  function updatePermissions() {
    if (!selectedSessionId || !pendingPermissions[selectedSessionId]) {
      permBanner.classList.add('hidden');
      return;
    }

    const perm = pendingPermissions[selectedSessionId];
    permBanner.classList.remove('hidden');

    let html =
      '<div class="perm-info">' +
      '<div class="perm-tool">' + escapeHtml(perm.tool) + '</div>' +
      '<div class="perm-detail">' + escapeHtml(perm.detail) + '</div>' +
      '</div>' +
      '<button class="btn-allow" onclick="window._permAllow()">Allow</button>' +
      '<button class="btn-deny" onclick="window._permDeny()">Deny</button>';

    if (perm.suggestions && perm.suggestions.length > 0) {
      perm.suggestions.forEach(function (s, i) {
        const label = s.type === 'toolAlwaysAllow' ? 'Always allow ' + (s.tool || 'this tool') : 'Allow similar';
        html += ' <button class="btn-suggestion" onclick="window._permSuggest(' + i + ')">' + escapeHtml(label) + '</button>';
      });
    }

    permBanner.innerHTML = html;

    window._permAllow = function () {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'allow'
      });
    };

    window._permDeny = function () {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'deny'
      });
    };

    window._permSuggest = function (i) {
      send({
        method: 'permission_response',
        request_id: perm.request_id,
        session_id: perm.session_id,
        decision: 'allow',
        suggestion: perm.suggestions[i]
      });
    };
  }

  // --- Utilities ---
  function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }

  // --- Init ---
  messagesEl.innerHTML = '<div id="empty-state">Select a session to view</div>';
  connect();
})();
```

**Step 2: Verify it compiles** (include_str! will pick up the new file)

Run: `cargo check --workspace`

**Step 3: Commit**

```
feat: add web frontend with session list, messages, prompt input, permissions
```

---

### Task 6: Smoke test the full web interface

**Files:** None (testing only)

**Step 1: Build release binary**

Run: `cargo build --release`

**Step 2: Start daemon**

Run: `./target/release/agent-dash daemon start --web-port 3131`

Expected output includes:
```
  web interface: http://127.0.0.1:3131
```

**Step 3: Verify static assets load**

Run: `curl -s http://localhost:3131/ | head -5`
Expected: HTML starting with `<!DOCTYPE html>`

Run: `curl -s http://localhost:3131/style.css | head -3`
Expected: CSS starting with `* { margin: 0;`

Run: `curl -s http://localhost:3131/app.js | head -3`
Expected: JS starting with `// agent-dash web interface`

**Step 4: Test WebSocket with Python**

```python
import asyncio, websockets, json

async def test():
    async with websockets.connect("ws://localhost:3131/ws") as ws:
        # Should receive auto-subscribed state update
        msg = await asyncio.wait_for(ws.recv(), timeout=3)
        data = json.loads(msg)
        print(f"Received: {data['event']}")
        assert data['event'] == 'state_update'
        print(f"Sessions: {len(data['sessions'])}")

        # Request state explicitly
        await ws.send(json.dumps({"method": "get_state"}))
        msg = await asyncio.wait_for(ws.recv(), timeout=3)
        data = json.loads(msg)
        print(f"GetState response: {data['event']}")
        print("PASS")

asyncio.run(test())
```

**Step 5: Open in browser**

Navigate to `http://localhost:3131` in a browser. Verify:
- Dark theme loads
- Session list shows active sessions (if any running via `agent-dash run`)
- Clicking a session loads its conversation
- Prompt input appears at bottom

**Step 6: Commit (if any fixes were needed)**

```
fix: address issues found in web interface smoke test
```

---

### Task 7: Polish and edge cases

**Files:**
- Possibly: `crates/agent-dash/web/app.js`, `web/style.css`, `web/index.html`
- Possibly: `crates/agent-dash/src/web.rs`

**Step 1: Handle edge cases**

Review and fix:
- WebSocket reconnect: JS already has auto-reconnect on close
- Empty state: show "No active sessions" when list is empty (already in JS)
- Session disappears: if selected session goes away in a state update, clear the conversation view
- Large messages: ensure CSS handles long lines with `overflow-x: auto` on `pre` blocks

**Step 2: Add session-gone detection to JS**

In the `state_update` handler in `app.js`, after updating sessions, add:

```javascript
// If selected session is gone, deselect
if (selectedSessionId && !sessions.find(function(s) { return s.session_id === selectedSessionId; })) {
  selectedSessionId = null;
  messagesEl.innerHTML = '<div id="empty-state">Session ended</div>';
  promptForm.classList.add('hidden');
  permBanner.classList.add('hidden');
}
```

**Step 3: Run tests**

Run: `cargo test --workspace`
Expected: all existing tests pass (no protocol changes)

**Step 4: Final commit**

```
fix: handle session disappearance and UI edge cases
```
