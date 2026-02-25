# Wrapper-Driven Session Discovery

## Motivation

The current session discovery relies on a process scanner that polls `sysinfo`
every 5 seconds, finds `claude` processes, groups them by CWD slug, and parses
JSONL files for metadata. This approach is unreliable:

- **Stale sessions**: Dead/zombie processes linger in scan results
- **Race conditions**: Scanner is inherently racy with process lifecycle
- **Subagent noise**: Subagent processes appear as separate sessions
- **Deduplication hacks**: Grouping by slug and picking lowest PID is fragile
- **Polling waste**: 5-second scan interval means delayed discovery and cleanup

With the new requirement that all agents are started through `agent-dash run`,
the wrapper registration becomes the authoritative source of truth for session
lifecycle. We can replace the entire scanner with explicit lifecycle signals.

## Design

### Core Idea

Sessions exist if and only if a wrapper registered them. The process scanner is
deleted entirely. Session lifecycle is driven by:

- **RegisterWrapper** (birth) — wrapper sends metadata at startup
- **Hook events** (status) — real-time updates from Claude's hook system
- **UnregisterWrapper / disconnect** (death) — wrapper cleans up on exit

### Session Types

**Main session**: Created by `agent-dash run claude`. Has a registered wrapper,
a PTY prompt channel, and is visible to clients by default.

**Subagent session**: Spawned by Claude inside the PTY (via Task tool).
Inherits `AGENT_DASH_WRAPPER_ID` from the parent environment. Fires hook events
with the parent's wrapper_id. Tracked by the daemon but hidden from clients by
default.

### Main Session Lifecycle

1. User runs `agent-dash run claude` in a project directory
2. Wrapper extracts metadata (CWD, git branch, project name)
3. Wrapper sends `RegisterWrapper { wrapper_id, agent, cwd, branch, project_name }`
4. Daemon creates the session — immediately visible to clients
5. Claude boots inside the PTY, fires `SessionStart { session_id, cwd }` with
   `wrapper_id` in the hook envelope
6. Daemon links the real `session_id` to the wrapper session (enables JSONL
   path derivation, message streaming)
7. Hook events (`ToolStart`, `ToolEnd`, `Stop`) update status in real-time
8. On exit: wrapper sends `UnregisterWrapper` — session removed. If wrapper
   crashes, daemon detects the broken socket and cleans up.

### Subagent Lifecycle

1. Claude spawns a subagent (via Task tool) inside the PTY
2. Subagent inherits `AGENT_DASH_WRAPPER_ID` from the environment
3. Subagent fires `SessionStart { session_id, cwd }` with the parent's
   `wrapper_id`
4. Daemon sees a new `session_id` with an existing `wrapper_id` but no matching
   `RegisterWrapper` — marks it as a subagent linked to the parent
5. Subagent hooks update its status independently
6. `SessionEnd` or parent wrapper exit — subagent cleaned up

### Visibility Rules

- **Default**: `GetState` and `Subscribe` return only main sessions
- **Subagent count**: Each main session includes a `subagent_count` field
- **Permission bubbling**: Subagent permission requests are broadcast to clients
  with both the subagent's `session_id` and its parent's `wrapper_id`, so
  clients can display them in context
- **Explicit listing**: `GetState { include_subagents: true }` returns
  everything for debugging or advanced views

### Prompt Injection

- `SendPrompt` only works on main sessions (those with a registered wrapper
  holding a PTY prompt channel)
- Attempting to prompt a subagent returns `Error: "cannot inject prompt into subagent"`
- `PermissionResponse` works for any session (main or subagent) since the
  permission protocol is bidirectional over the daemon socket

### Daemon Restart Recovery

The wrapper detects daemon disconnect (EOF/error on its read loop) and enters
a reconnect loop:

1. Retry with backoff: 1s, 2s, 4s, capped at 10s
2. On reconnect, re-send `RegisterWrapper` with full metadata
3. Include `real_session_id` if the link was already established, so the daemon
   can immediately reconstruct the full session state

Subagents re-appear naturally when they fire their next hook event. Idle
subagents stay invisible until active — acceptable since they don't need
attention.

## Protocol Changes

### RegisterWrapper (enhanced)

```rust
RegisterWrapper {
    session_id: String,             // "wrap-{pid}"
    agent: String,                  // "claude"
    cwd: String,                    // working directory
    branch: String,                 // git branch at startup
    project_name: String,           // derived from CWD
    real_session_id: Option<String>, // included on reconnect
}
```

### GetState (enhanced)

```rust
GetState {
    include_subagents: bool,  // default false
}
```

### StateUpdate (enhanced)

`DashSession` gains:

```rust
subagent_count: usize,  // number of active subagents under this session
```

## Data Model Changes

### InternalSession

- Add `parent_wrapper_id: Option<String>` — set for subagents, None for main
- Add `is_main: bool` — true if session has a registered wrapper
- Remove `pid: Option<u32>` — no longer needed without scanner
- Remove `wrapped: bool` — redundant (all sessions are wrapped by definition)
- Keep `jsonl_path: Option<String>` — derived from `session_id` + project slug
  once `SessionStart` links the real session_id

## What Gets Removed

### Deleted code

- `scanner.rs` — entire process scanning module
- 5-second scan interval in `daemon.rs`
- PID-based deduplication logic (slug grouping, lowest-PID-wins)
- `sysinfo` crate dependency

### Deleted concepts

- "Discovered" sessions — sessions appear by registering, not by being found
- PID tracking — daemon doesn't need to know PIDs
- JSONL scanning for discovery — JSONL is still read for `GetMessages` /
  `WatchSession`, located by deriving path from `session_id` + project slug

### Kept as-is

- Hook listener (`hook.sock`) — fire-and-forget events
- Client listener (`daemon.sock`) — subscriptions, permissions, prompts, messages
- JSONL message reading (`messages.rs`, `watcher.rs`) — for `GetMessages` / `WatchSession`
- `state.json` writing — GNOME extension compatibility
- PTY wrapper logic — core of `agent-dash run`
- Permission flow — unchanged, subagent permissions now have parent context
