# agent-dash

Monitor and interact with multiple Claude Code sessions from any interface.

agent-dash is a local daemon that tracks all running Claude Code sessions via hooks, exposes their state over Unix sockets, and provides a unified CLI for querying sessions, reading chat history, approving permissions, and injecting prompts. Clients include a GNOME Shell extension and an encrypted WebSocket relay for mobile access.

## Building

```bash
cargo build --workspace --release
```

Binaries:
- `target/release/agent-dash` — unified CLI and daemon
- `target/release/agent-dash-relay` — WebSocket relay server (runs on a VPS)

## Quick start

```bash
# Install hooks into Claude Code's settings.json
agent-dash setup

# Start the daemon
agent-dash daemon start

# Open a Claude session and see it appear
agent-dash status
```

## Architecture

```
Claude Code ──hooks──> agent-dash hook ──> hook.sock ──> agent-dashd
                                                            │
                                          daemon.sock <─────┘
                                              │
                              ┌───────────────┼───────────────┐
                              │               │               │
                          CLI (agent-dash)  GNOME ext    relay connector
                                                              │
                                                         relay server (WSS)
                                                              │
                                                         phone app (future)
```

The daemon creates two local sockets:
- `~/.cache/agent-dash/hook.sock` — receives hook events (fire-and-forget)
- `~/.cache/agent-dash/daemon.sock` — client connections (bidirectional, line-delimited JSON)

## CLI reference

### Session monitoring

```bash
# Show all active sessions (default command)
agent-dash status

# Stream raw daemon events as JSON
agent-dash watch-events
agent-dash watch-events | jq .
```

### Chat history

```bash
# Fetch last 20 messages from a session
agent-dash messages <session_id>

# Fetch with options
agent-dash messages <session_id> json --limit 50

# Stream new messages as they arrive
agent-dash watch <session_id>

# List all JSONL sessions for a project
agent-dash sessions <project>
```

### Permission management

```bash
# Approve a single permission request
agent-dash approve <request_id>

# Approve this and similar future requests
agent-dash approve-similar <request_id>

# Deny a permission request
agent-dash deny <request_id>
```

### Agent wrapping

```bash
# Run Claude in a PTY wrapper (enables prompt injection)
agent-dash run claude

# Inject a prompt into a wrapped session
agent-dash inject <session_id> "fix the failing tests"
```

### Daemon management

```bash
agent-dash daemon start
```

### Hook setup

```bash
# Install hooks into ~/.claude/settings.json
agent-dash setup
```

This registers `agent-dash hook` as the handler for all Claude Code hook events (PreToolUse, PostToolUse, PermissionRequest, Stop, SessionStart, SessionEnd).

### Remote relay

See [docs/relay.md](docs/relay.md) for full relay documentation.

```bash
# Generate keypair and display QR code for phone pairing
agent-dash relay pair wss://my-relay.example.com

# Start forwarding daemon events through the relay
agent-dash relay connect

# Show relay connection status
agent-dash relay status

# Remove pairing config and keys
agent-dash relay unpair
```

## GNOME Shell extension

The `extension/` directory contains a GNOME Shell extension that connects to `daemon.sock` and provides a side panel showing all active Claude Code sessions with:

- Real-time session status and active tool display
- Click-to-expand chat history with markdown rendering
- Inline permission approval/denial controls
- Notification sounds on session completion or permission prompts

Install by symlinking into your GNOME extensions directory:

```bash
ln -s "$(pwd)/extension" ~/.local/share/gnome-shell/extensions/agent-dash@mfeinber
```

## Running tests

```bash
cargo test --workspace
```

## Project structure

```
crates/
  agent-dash/          Unified CLI + daemon binary
  agent-dash-core/     Shared types: protocol, relay protocol, session, paths
  agent-dash-relay/    WebSocket relay server (deployed separately)
extension/             GNOME Shell extension
docs/plans/            Design documents
```
