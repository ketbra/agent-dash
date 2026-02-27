# Terminal Resize from Web UI

## Problem

The PTY is created at a fixed size (120x36 for headless/web sessions, or the host terminal size for CLI sessions). The xterm.js fit addon resizes the browser-side terminal to match available space, but no resize message is sent back to the server. The resize polling thread is skipped in headless mode. The PTY and browser terminal are permanently out of sync after creation.

## Decisions

- **Resize the real PTY** from the browser, not display-only scaling. The web UI should behave like a real terminal.
- **Debounced on window resize + immediate on view/session switch.** Covers all cases without being chatty.
- **Last writer wins** for multi-client scenarios. Standard terminal multiplexer behavior.

## Design

### Protocol Changes (protocol.rs)

Add `TerminalResize` to both directions:

**ClientRequest** (web client to daemon):
```rust
TerminalResize {
    session_id: String,
    cols: u16,
    rows: u16,
}
```

**ServerEvent** (daemon to wrapper):
```rust
TerminalResize {
    cols: u16,
    rows: u16,
}
```

Fire-and-forget like `TerminalInput` — no response needed.

### Browser (app.js)

Add `sendTerminalSize()` that reads `terminalInstance.cols` / `terminalInstance.rows` and sends `terminal_resize` over the WebSocket.

Call it in three places:
1. After `fitAddon.fit()` in `setViewMode('terminal')` — immediate on view switch
2. After `fitAddon.fit()` in the window resize handler — debounced at 150ms
3. In `selectSession()` when in terminal mode — after re-watching

Also send after the initial `loadXterm()` completes so web-initiated sessions get correct size immediately.

### Daemon (client_listener.rs + daemon.rs)

- `client_listener.rs`: Parse `terminal_resize` method, emit `ClientRequest::TerminalResize`
- `daemon.rs`: Route to the wrapper's channel as `ServerEvent::TerminalResize`

Pure pass-through, same pattern as `TerminalInput`.

### Wrapper (wrapper.rs)

On receiving `ServerEvent::TerminalResize`:
1. Call `master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })`
2. Update `AtomicTermSize` so the vt100 parser picks up new dimensions

Same logic as the existing interactive resize thread, just triggered by a network message.

## Files Changed

| File | Change |
|------|--------|
| `crates/agent-dash-core/src/protocol.rs` | Add `TerminalResize` to `ClientRequest` and `ServerEvent` |
| `crates/agent-dash/web/app.js` | Add `sendTerminalSize()`, debounced resize, call on view/session switch |
| `crates/agent-dash/src/client_listener.rs` | Parse `terminal_resize`, emit `ClientRequest::TerminalResize` |
| `crates/agent-dash/src/daemon.rs` | Route `TerminalResize` from client to wrapper |
| `crates/agent-dash/src/wrapper.rs` | Handle `TerminalResize`: call `master.resize()` + update `AtomicTermSize` |

## What's NOT Changing

- Interactive resize thread continues working for CLI-attached sessions
- `CreateSession` initial size unchanged — browser corrects on first fit
- No new dependencies
