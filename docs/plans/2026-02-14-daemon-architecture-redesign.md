# Daemon Architecture Redesign

## Motivation

The current agent-dash is a synchronous, single-threaded daemon tightly coupled
to a GNOME Shell extension. The daemon writes `state.json` every second; the
extension polls it. Permission handling uses file-based IPC with polling. This
architecture has caused blocking issues and limits the project to a single UI.

The goal is a robust, async, cross-platform daemon with a well-defined protocol
that any client can connect to: GNOME extension, CLI tool, Emacs, or anything
else.

## Architecture Overview

Three binaries plus a shared library, organized as a Cargo workspace:

| Binary | Purpose |
|--------|---------|
| `agent-dashd` | Async daemon — owns all state, pushes updates to clients |
| `agent-dash-hook` | Hook companion — invoked by Claude's hook system, sends events to daemon |
| `agentctl` | CLI tool — query state, stream events, respond to permission prompts |

All three share types and helpers from `agent-dash-core`.

## Daemon Tasks

The daemon runs on tokio with these concurrent tasks:

- **Hook listener** — Accepts fire-and-forget events on `hook.sock` (tool_start,
  tool_end, stop, session lifecycle)
- **Client listener** — Accepts persistent bidirectional connections on
  `daemon.sock` (UI clients, agentctl, permission bridge)
- **Process scanner** — Runs every 5 seconds via `sysinfo`, discovers and prunes
  sessions cross-platform
- **State manager** — Owns all session state, merges updates from hooks and
  scans, broadcasts diffs to subscribed clients via channels
- **State file writer** — Subscribes to state changes, writes `state.json`
  debounced (500ms) for debugging and simple script consumption

### State Manager

The state manager is the single owner of all session state. No shared mutexes —
only message passing via tokio channels.

It runs a single task that `select!`s over:

- Hook event channel
- Process scan channel
- Client request channel
- Debounce timer for state.json writes

It broadcasts to subscribers only when state actually changes. A process scan
that finds the same set of processes does not emit a duplicate update.

#### Session State Merging

Priority order (same as today):

1. Permission request pending → `NeedsInput`
2. Hook says idle → `Idle`
3. Hook has active tool → `Working` + tool info
4. No hook data → default to `Idle`

Sessions are pruned when the process disappears from the scan and no hook
activity has been seen for 30 seconds (handles brief process restarts).

## Communication

### Transport

Cross-platform local sockets via the `interprocess` crate (tokio integration).
Uses Unix domain sockets on Linux/macOS and named pipes on Windows.

Two socket paths:

- `hook.sock` — Hook companion connects here. Fire-and-forget: send one JSON
  message, disconnect. Keeps hooks fast and non-blocking.
- `daemon.sock` — Clients connect here. Persistent, bidirectional.

### Protocol

Line-delimited JSON (`\n`-terminated). No framing beyond newline separation.

Optional `id` field on requests enables request/response correlation if needed
in the future (natural evolution toward JSON-RPC 2.0).

#### Hook → Daemon (via hook.sock)

```json
{"event":"tool_start","session_id":"abc-123","tool":"Bash","detail":"cargo test","tool_use_id":"toolu_01ABC"}
{"event":"tool_end","session_id":"abc-123","tool_use_id":"toolu_01ABC"}
{"event":"stop","session_id":"abc-123"}
{"event":"session_start","session_id":"abc-123"}
{"event":"session_end","session_id":"abc-123"}
```

#### Client → Daemon (via daemon.sock)

```json
{"method":"subscribe"}
{"method":"get_state"}
{"method":"permission_response","request_id":"toolu_01ABC","session_id":"abc-123","decision":"allow"}
{"method":"permission_response","request_id":"toolu_01ABC","session_id":"abc-123","decision":"allow_similar"}
{"method":"permission_response","request_id":"toolu_01ABC","session_id":"abc-123","decision":"deny"}
```

#### Daemon → Client (via daemon.sock)

```json
{"event":"state_update","sessions":[...]}
{"event":"permission_pending","session_id":"abc-123","request_id":"toolu_01ABC","tool":"Bash","detail":"rm -rf /tmp/foo"}
{"event":"permission_resolved","request_id":"toolu_01ABC","resolved_by":"terminal"}
```

### Permission Flow

1. `agent-dash-hook permission` connects to `daemon.sock` (not hook.sock,
   because it needs a response)
2. Sends `permission_request` with `request_id` (Claude's `tool_use_id`)
3. Daemon broadcasts `permission_pending` to all connected clients
4. Any source resolves it — terminal approval, GNOME button, `agentctl approve`,
   Emacs, etc.
5. Daemon sends response back to the hook connection
6. Daemon broadcasts `permission_resolved` (with `request_id`) to all clients
7. All clients clear that specific prompt from their UI

If the terminal resolves it (Claude proceeds without the daemon), the daemon
detects this from subsequent hook events and broadcasts `permission_resolved`
with `resolved_by: "terminal"`.

## Hook Companion Binary

`agent-dash-hook` replaces the shell scripts (`agent-dash-hook.sh`,
`permission-bridge.sh`). A single Rust binary with subcommands:

```
agent-dash-hook tool-start --session <id> --tool <name> --detail <text> --tool-use-id <id>
agent-dash-hook tool-end --session <id> --tool-use-id <id>
agent-dash-hook stop --session <id>
agent-dash-hook session-start --session <id>
agent-dash-hook session-end --session <id>
agent-dash-hook permission --session <id> --request-id <id> --tool <name> --detail <text>
```

Fire-and-forget subcommands connect to `hook.sock`, send, and exit.

The `permission` subcommand connects to `daemon.sock`, sends the request, waits
for the response, prints the decision in Claude's hook response format, and
exits.

Cross-platform: no dependency on `ncat`, `socat`, or any external tool.

## CLI Tool

`agentctl` subcommands:

```
agentctl status                     # Print current sessions, disconnect
agentctl list                       # Terse session listing
agentctl watch                      # Subscribe and stream events as JSON (ctrl-c to stop)
agentctl approve <request_id>       # Approve a pending permission
agentctl approve-similar <request_id>  # Approve similar future commands
agentctl deny <request_id>          # Deny a pending permission
```

`watch` prints raw JSON events, one per line, for piping to `jq` or debugging.

## Cross-Platform Support

| Concern | Solution |
|---------|----------|
| Local sockets | `interprocess` crate (Unix sockets on Linux/macOS, named pipes on Windows) |
| Process discovery | `sysinfo` crate (replaces Linux-only `procfs`) |
| Config paths | `dirs` crate (already used) |
| Claude project slugs | Same format on all platforms; path separator handling in slug generation |
| Hook binary | Rust binary, no shell/ncat dependency |

## Dependencies

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime |
| `interprocess` | Cross-platform local sockets with tokio integration |
| `sysinfo` | Cross-platform process discovery |
| `serde` + `serde_json` | JSON serialization |
| `dirs` | Platform-specific config/cache paths |

Dropped: `procfs` (Linux-only, replaced by `sysinfo`).

## Project Structure

```
agent-dash/
├── Cargo.toml                      # Workspace definition
├── crates/
│   ├── agent-dashd/                # Daemon binary
│   │   └── src/
│   │       ├── main.rs             # Tokio runtime, task spawning
│   │       ├── state.rs            # State manager task
│   │       ├── scanner.rs          # Process scanner (sysinfo)
│   │       ├── hook_listener.rs    # Hook socket listener
│   │       └── client_listener.rs  # Client socket listener
│   ├── agent-dash-hook/            # Hook companion binary
│   │   └── src/
│   │       └── main.rs             # Subcommand dispatch, socket send
│   ├── agentctl/                   # CLI query/debug tool
│   │   └── src/
│   │       └── main.rs             # Subcommand dispatch
│   └── agent-dash-core/            # Shared library
│       └── src/
│           ├── lib.rs
│           ├── protocol.rs         # Message types (serde structs)
│           ├── session.rs          # Session/status types
│           ├── paths.rs            # Cross-platform socket/config paths
│           └── connection.rs       # Socket connect/read/write helpers
├── extension/                      # GNOME extension (unchanged for now)
└── docs/plans/
```

## Migration Path

1. Build the new daemon, hook binary, and agentctl from scratch in the
   `crates/` workspace
2. Keep `state.json` output so the GNOME extension continues working unmodified
3. Update Claude hook config to point to `agent-dash-hook` binary instead of
   shell scripts
4. Migrate the GNOME extension from polling `state.json` to subscribing via
   `daemon.sock` when ready
5. Remove old `src/` top-level source files and shell scripts
