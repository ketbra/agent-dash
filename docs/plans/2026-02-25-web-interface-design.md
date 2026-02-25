# Web Interface for Agent Dash

## Overview

Add a web-based dashboard to agent-dashd for viewing active sessions, reading conversations, injecting prompts, and responding to permission requests. The web server is embedded in the daemon process, shares the same internal channel architecture, and serves a vanilla JS frontend with no build step.

## Architecture

The daemon gains a new `web.rs` module that runs an axum HTTP server alongside the existing Unix socket listeners. It shares the same `mpsc::Sender<ClientMessage>` channel, so web requests flow through the identical state management path as CLI and Unix socket clients.

```
Browser <-> HTTP/WebSocket (axum, port 3131)
                    |
                    |-- GET /           -> static HTML/JS/CSS (embedded in binary)
                    |-- GET /api/state  -> one-shot JSON state snapshot
                    \-- WS  /ws        -> bidirectional events
                            |
                      client_tx channel (shared with Unix socket listener)
                            |
                      daemon main loop (unchanged)
```

The web server is started as another `tokio::spawn` alongside the hook and client listeners. Static assets (HTML, JS, CSS) are embedded in the binary using `include_str!`.

## WebSocket Protocol

The WebSocket at `/ws` uses a **single multiplexed connection** rather than the Unix socket's "one connection per mode" pattern. The handler:

- Auto-subscribes on connect — pushes `StateUpdate` events whenever daemon state changes
- Accepts any `ClientRequest` at any time (get_state, watch_session, unwatch_session, send_prompt, permission_response)
- Forwards `watch_session`/`unwatch_session` for per-session message streaming
- Forwards `permission_response` and `send_prompt` as fire-and-forget actions
- Sends all `ServerEvent` types to the client (StateUpdate, Message, PermissionPending, PermissionResolved, PromptSent, Error)

The web module internally manages multiple daemon channel subscriptions (state subscriber + message watcher) on behalf of the single WebSocket connection.

Messages are JSON, one per WebSocket text frame (no newline delimiter needed since WebSocket has its own framing).

## Frontend UI

A single-page application with three areas:

### Session List (left sidebar)
- Shows active main sessions
- Each entry: project name, branch, status indicator (color dot — green for working, yellow for needs_input, gray for idle), subagent count badge if > 0
- Permission badge when a session has a pending request
- Clicking selects a session; highlighted state

### Conversation View (main area)
- Shows message history for the selected session, rendered as HTML (using daemon's existing `html` format from the messages module)
- Auto-scrolls to bottom on new messages
- Tool use blocks displayed with tool name and detail
- Tool results shown collapsed by default
- Live-updates via `watch_session` when selected; `unwatch_session` when switching away

### Prompt Input (bottom of conversation)
- Text input for prompt injection via `send_prompt`
- Disabled when no session selected or session is a subagent

### Permission Banner (top of conversation, conditional)
- Appears when selected session has a pending permission request
- Shows tool name, detail/command
- Allow and Deny buttons
- Suggestion options (e.g., "Always allow this tool") as secondary actions

### Styling
- Dark theme with terminal-friendly aesthetic
- Monospace fonts for code blocks
- No external CSS frameworks — vanilla CSS

## Configuration

The `daemon start` subcommand gains a `--web-port <PORT>` flag:
- Default: `3131`
- `--web-port 0` disables the web server entirely

On startup, the daemon prints the web URL:
```
agent-dashd starting
  hook socket:   ~/.cache/agent-dash/hook.sock
  client socket: ~/.cache/agent-dash/daemon.sock
  web interface: http://localhost:3131
```

The server binds to `127.0.0.1` only (localhost). No authentication in v1.

## Dependencies

- `axum` — HTTP server with built-in WebSocket support, built on tokio/hyper
- `tower-http` — middleware (may be needed for static serving)

Both integrate naturally with the existing tokio runtime.

## Static Assets

Three files embedded via `include_str!`:
- `web/index.html` — page structure
- `web/app.js` — all client logic (WebSocket, DOM manipulation, state management)
- `web/style.css` — dark theme styling

No build step, no npm, no bundler.

## Data Flow Examples

### Page Load
1. Browser loads `GET /` → receives index.html (which loads app.js + style.css)
2. JS opens WebSocket to `/ws`
3. Server auto-subscribes, immediately sends `StateUpdate` with current sessions
4. JS renders session list

### Select Session
1. User clicks session in sidebar
2. JS sends `{"method":"get_messages","session_id":"...","format":"html","limit":50}`
3. Server responds with `Messages` event containing history
4. JS sends `{"method":"watch_session","session_id":"...","format":"html"}`
5. Server streams `Message` events as conversation progresses
6. When switching away, JS sends `{"method":"unwatch_session","session_id":"..."}`

### Permission Response
1. `StateUpdate` arrives with session status `needs_input` and `input_reason.reason_type == "permission"`
2. JS shows permission banner with tool/detail
3. User clicks Allow
4. JS sends `{"method":"permission_response","request_id":"...","session_id":"...","decision":"allow"}`
5. Server forwards to daemon; `PermissionResolved` event clears the banner

### Prompt Injection
1. User types in prompt input, presses Enter
2. JS sends `{"method":"send_prompt","session_id":"...","text":"..."}`
3. Server responds with `PromptSent` event confirming delivery

## Scope

### In v1
- Embedded axum web server with `--web-port` flag
- WebSocket with multiplexed subscriptions
- Session list with real-time status updates
- Conversation view with HTML-rendered messages and live streaming
- Prompt injection input
- Permission request UI with Allow/Deny and suggestions
- Dark theme, no build step, embedded assets

### Not in v1
- Authentication / TLS
- Subagent tree view (count shown, not expandable)
- Session history / search
- Mobile-responsive layout
- Customizable themes
