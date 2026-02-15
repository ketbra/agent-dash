# agent-dash

Monitor and interact with multiple Claude Code sessions from any interface.

## Building

```bash
cargo build --workspace --release
```

Binaries are placed in `target/release/`:
- `agent-dashd` — the daemon
- `agent-dash-hook` — hook companion (invoked by Claude's hook system)
- `agentctl` — CLI query/debug tool

## Running the daemon

```bash
cargo run -p agent-dashd --release
```

Or run the built binary directly:

```bash
./target/release/agent-dashd
```

The daemon creates two local sockets and a state file:
- `~/.cache/agent-dash/hook.sock` — receives hook events (fire-and-forget)
- `~/.cache/agent-dash/daemon.sock` — client connections (bidirectional)
- `~/.cache/agent-dash/state.json` — state snapshot for legacy clients

## Using agentctl

```bash
# Show active sessions
cargo run -p agentctl --release

# Same thing
cargo run -p agentctl --release -- status

# Stream all events as JSON (pipe to jq for formatting)
cargo run -p agentctl --release -- watch
cargo run -p agentctl --release -- watch | jq .

# Respond to permission prompts
cargo run -p agentctl --release -- approve <request_id>
cargo run -p agentctl --release -- approve-similar <request_id>
cargo run -p agentctl --release -- deny <request_id>
```

Or use the built binary:

```bash
agentctl status
agentctl watch
agentctl watch | jq .
```

## Installing the hook binary

```bash
cargo build -p agent-dash-hook --release
cp target/release/agent-dash-hook ~/.local/bin/
```

Configure in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [{"hooks": [{"type": "command", "command": "agent-dash-hook tool-start"}]}],
    "PostToolUse": [{"hooks": [{"type": "command", "command": "agent-dash-hook tool-end"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "agent-dash-hook stop"}]}],
    "SessionStart": [{"hooks": [{"type": "command", "command": "agent-dash-hook session-start"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "agent-dash-hook session-end"}]}],
    "PermissionRequest": [{"matcher": "*", "hooks": [{"type": "command", "command": "agent-dash-hook permission"}]}]
  }
}
```

## Running tests

```bash
cargo test --workspace
```

## Project structure

```
crates/
  agent-dash-core/   Shared types: protocol, session, paths
  agent-dashd/       Async tokio daemon
  agent-dash-hook/   Hook companion binary
  agentctl/          CLI tool
extension/           GNOME Shell extension (legacy client)
```
