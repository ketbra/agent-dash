# Design: Message Fetching & Streaming API

## Context

The daemon currently provides session metadata (project name, status, active tool) but not conversation content. The GNOME extension reads JSONL files directly for its chat popup. Other planned clients (Emacs, web UIs) would each need to reimplement JSONL parsing. Centralizing message access in the daemon gives clients a clean, format-aware API and enables real-time streaming.

## Protocol

### Requests

**`get_messages`** — Request/response. Returns the last N messages from a session.

```json
{"request": "get_messages", "session_id": "abc-123", "format": "structured", "limit": 50}
```

Response:

```json
{"event": "messages", "session_id": "abc-123", "messages": [...]}
```

**`watch_session`** — Subscription. Client receives new messages as they're written to the JSONL.

```json
{"request": "watch_session", "session_id": "abc-123", "format": "html"}
```

Pushes:

```json
{"event": "message", "session_id": "abc-123", "role": "assistant", "content": "..."}
```

**`unwatch_session`** — Stops the subscription.

```json
{"request": "unwatch_session", "session_id": "abc-123"}
```

**`list_sessions`** — Lists all JSONL sessions for a project, distinguishing main from subagent.

```json
{"request": "list_sessions", "project": "traider"}
```

Response:

```json
{"event": "session_list", "project": "traider", "sessions": [
  {"session_id": "abc-123", "main": true, "modified": 1739500000},
  {"session_id": "def-456", "main": false, "modified": 1739499800}
]}
```

### Format Parameter

Applies to both `get_messages` and `watch_session`:

- **`structured`** (default) — Typed objects with full content arrays.
- **`markdown`** — Raw markdown text, tools rendered as text blocks.
- **`html`** — Markdown rendered to HTML via comrak (GFM).

## Message Structure

### Structured Format

The canonical representation. Other formats are renderings of it.

```json
{
  "role": "assistant",
  "content": [
    {
      "type": "text",
      "text": "Here's the fix for the login bug..."
    },
    {
      "type": "tool_use",
      "name": "Bash",
      "detail": "cargo test --workspace",
      "input": {"command": "cargo test --workspace"}
    },
    {
      "type": "tool_result",
      "name": "Bash",
      "status": "success",
      "output": "running 31 tests\ntest result: ok..."
    }
  ]
}
```

Tool results from the JSONL `user` messages are attached to the preceding tool use, so clients see the full tool invocation cycle together.

### Markdown Format

Tools rendered inline as blockquotes:

```markdown
Here's the fix for the login bug...

> **Bash**: `cargo test --workspace`
> ```
> running 31 tests
> test result: ok...
> ```
```

### HTML Format

The markdown rendering piped through comrak. Code blocks produce `<pre><code class="language-rust">` etc. Full GFM support (tables, strikethrough, task lists, autolinks).

## Daemon Internals

### JSONL Watcher

Uses the `notify` crate to watch JSONL files for changes.

When a client sends `watch_session`:

1. Look up the session's `jsonl_path`.
2. If not already watching that file, start a `notify` watch.
3. Record the current file size as the read offset.
4. On write notification, read from the last offset to EOF, parse new lines, format, and push to subscribed clients.
5. When the last subscriber for a file unwatches, remove the watch.

The watcher sends events through a channel into the main `tokio::select!` loop, same pattern as hooks and clients.

### Message Parser

A new module that:

1. Parses raw JSONL lines into the structured message format (extending the existing `JournalEntry` types).
2. Pairs tool results with their preceding tool uses (matching on `tool_use_id`).
3. Renders to the requested format: `structured` returns typed objects, `markdown` flattens to text, `html` pipes markdown through comrak.

### State

```rust
struct WatchedFile {
    path: PathBuf,
    offset: u64,
    subscribers: Vec<(String, mpsc::Sender<String>)>,  // (format, tx)
}

watched_files: HashMap<String, WatchedFile>  // keyed by session_id
```

### get_messages

No file watching needed. Reads the tail of the JSONL, parses the last N entries, formats, and responds. Same parsing and rendering pipeline as the watcher.

## Main Session Default

The process scanner's slug dedup already collapses all processes for a project into one session. `list` / `GetState` shows only main sessions (one per project slug). No change needed.

For subagent discovery, `list_sessions` returns all JSONL files for a given project slug. Each entry is marked `main: true/false`. `get_messages` and `watch_session` work on any session ID — clients that want subagent content use `list_sessions` to find the IDs, then request them explicitly.

## New Dependencies

- **`notify`** — Cross-platform file watching (inotify on Linux, kqueue on macOS, ReadDirectoryChanges on Windows).
- **`comrak`** — GFM-compatible markdown to HTML rendering. Pure Rust.

## File Changes

| File | Change |
|---|---|
| `crates/agent-dash-core/src/protocol.rs` | Add `GetMessages`, `WatchSession`, `UnwatchSession`, `ListSessions` to `ClientRequest`. Add `Messages`, `Message`, `SessionList` to `ServerEvent`. |
| `crates/agent-dash-core/src/session.rs` | Add `MessageContent`, `ContentBlock` types for the structured format. |
| `crates/agent-dashd/src/messages.rs` | **New.** JSONL parsing into structured messages, markdown flattening, HTML rendering via comrak. |
| `crates/agent-dashd/src/watcher.rs` | **New.** `notify`-based file watcher. Sends events to main loop via channel. Tracks per-file offsets and subscriber lists. |
| `crates/agent-dashd/src/client_listener.rs` | Handle new request types, forward to main loop as `ClientMessage` variants. |
| `crates/agent-dashd/src/main.rs` | New arms in `tokio::select!` for watcher events. Handle `GetMessages`, `WatchSession`, `UnwatchSession`, `ListSessions`. |
| `crates/agent-dashd/src/scanner.rs` | Extract shared JSONL parsing types to `messages.rs`, scanner uses the shared types. |
| `crates/agentctl/src/main.rs` | Add `messages` and `sessions` subcommands for testing. |

No changes to `agent-dash-hook`, `agent-dash-core/paths.rs`, or the GNOME extension (it can migrate later as a separate effort).
