# Design: Single Binary with PTY Wrapper

## Context

The project currently produces three binaries (`agent-dashd`, `agent-dash-hook`, `agentctl`) plus a shared library (`agent-dash-core`). Users must install and coordinate multiple executables. More importantly, there is no way to inject prompts into a running Claude Code session — the CLI has no programmatic input mechanism, and the Agent SDK requires API keys rather than a subscription.

This design collapses everything into a single binary that wraps Claude Code in a pseudo-terminal, enabling prompt injection, output capture, and a zero-friction setup experience.

## Architecture Overview

The binary (`agent-dash`) has four runtime modes selected by subcommand:

- **Wrapper mode** (`agent-dash run claude`) — Spawns an agent inside a PTY via `portable-pty`. User I/O passes through transparently. The wrapper registers with the daemon and accepts prompt injection commands.
- **Daemon mode** (`agent-dash daemon start`) — Background process that owns all state. Same channel-based `tokio::select!` architecture as today. Gains the ability to route prompt injection requests to registered wrappers.
- **CLI mode** (`agent-dash status`, `messages`, etc.) — Connects to the daemon, sends a request, prints the response, exits.
- **Hook mode** (`agent-dash hook pre-tool-use`) — Reads Claude Code's hook environment variables, sends a JSON event to the daemon, exits.

The daemon auto-starts when the first `run` command can't find it running. It auto-exits after an idle timeout (5 minutes with no sessions).

## PTY Wrapper Data Flow

When the user runs `agent-dash run claude`:

1. **Daemon check** — Try connecting to `daemon.sock`. If it fails, fork `agent-dash daemon start` as a background process. Wait up to 2 seconds for the socket to become available.

2. **PTY setup** — Call `native_pty_system().openpty(PtySize)` with the current terminal's dimensions (queried via `TIOCGWINSZ`). Build a `CommandBuilder` for the agent binary with pass-through args and environment.

3. **Spawn** — `pair.slave.spawn_command(cmd)` launches the agent inside the PTY.

4. **Register with daemon** — Send `register_wrapper` over the daemon connection. The daemon creates a prompt channel and associates it with this session.

5. **I/O bridge** — Three concurrent tasks:
   - **stdin -> pty**: Read user's real stdin in raw mode, write to `pair.master.take_writer()`.
   - **pty -> stdout**: Read from `pair.master.try_clone_reader()`, write to real stdout.
   - **daemon -> pty**: Listen on the daemon connection for `inject_prompt` commands. Write the prompt text + newline to the PTY writer (identical to the user typing it).

6. **Resize** — Handle `SIGWINCH`, call `pair.master.resize(new_size)` so the agent's TUI reflows.

7. **Cleanup** — When the child exits, send `unregister_wrapper`, restore terminal mode, exit with the child's exit code.

Prompt injection is writing to the same PTY writer that stdin uses. No special mechanism needed.

## Daemon Changes

The daemon gains wrapper registration and prompt routing. Everything else stays the same.

### New State

```rust
struct RegisteredWrapper {
    session_id: String,
    agent: String,
    prompt_tx: mpsc::Sender<String>,
}

// Added to DaemonState:
registered_wrappers: HashMap<String, RegisteredWrapper>  // keyed by session_id
```

### New Protocol Messages

```
// Wrapper -> Daemon
{"request": "register_wrapper", "session_id": "...", "agent": "claude"}
{"request": "unregister_wrapper", "session_id": "..."}

// Any client -> Daemon
{"request": "send_prompt", "session_id": "...", "text": "fix the tests"}

// Daemon -> Client (response)
{"event": "prompt_sent", "session_id": "..."}
{"event": "error", "message": "session not wrapped"}

// Daemon -> Wrapper
{"event": "inject_prompt", "text": "fix the tests"}
```

### Unchanged

- Hook listener (`hook.sock`) — still needed for unwrapped sessions
- Client listener (`daemon.sock`) — wrappers connect here like any other client
- Process scanner — still discovers unwrapped sessions
- File watcher, message API, state management — unchanged

### Wrapped vs Unwrapped Sessions

When both a wrapper and a process scan find the same session, the daemon prefers the wrapper's metadata. A `wrapped: bool` field on `InternalSession` tells clients which capabilities are available. Unwrapped sessions still appear in status/messages but cannot receive prompt injection.

## Agent Profiles

Each supported agent gets a module under `agents/`. A profile is a minimal struct:

```rust
pub struct AgentProfile {
    pub name: &'static str,
    pub binary: &'static str,
    pub display_name: &'static str,
    pub install_hint: &'static str,
    pub hook_env_session_id: &'static str,
}
```

For now, one profile:

```rust
pub const CLAUDE: AgentProfile = AgentProfile {
    name: "claude",
    binary: "claude",
    display_name: "Claude Code",
    install_hint: "curl -fsSL https://claude.ai/install.sh | bash",
    hook_env_session_id: "SESSION_ID",
};
```

No trait abstraction yet. When a second agent (Codex) arrives, add a second module and extract the trait if a pattern emerges.

## Hooks

Claude Code's hook config points back to the same binary:

```json
{
  "hooks": {
    "PreToolUse": [{"type": "command", "command": "agent-dash hook pre-tool-use"}],
    "PostToolUse": [{"type": "command", "command": "agent-dash hook post-tool-use"}],
    "Stop": [{"type": "command", "command": "agent-dash hook stop"}]
  }
}
```

Hooks fire the same way for wrapped and unwrapped sessions. The wrapper does not try to parse terminal output to detect tool use — hooks provide structured events reliably. The wrapper focuses on what only it can do: prompt injection.

### Hook Installation

`agent-dash setup hooks` merges agent-dash hook entries into `~/.claude/settings.json`:

- Reads existing settings, preserves all non-agent-dash entries
- Adds/updates agent-dash hook commands for each event type
- Idempotent (safe to run repeatedly)
- Supports `--project` flag to write to `.claude/settings.json` instead
- `agent-dash run claude` checks for hooks on first run and offers to install them if missing

## Output Streaming

Remote clients (chat UIs, Emacs) get messages via the existing JSONL-based message API (`get_messages`, `watch_session`). The wrapper captures raw terminal output but does not expose it through the protocol — it's noisy, full of ANSI escape codes, and requires a terminal emulator on the client to render.

Raw terminal streaming can be added later if needed. The architecture doesn't preclude it.

## Crate Structure

```
crates/
  agent-dash-core/          # shared types, protocol, paths (UNCHANGED)
  agent-dash/               # single binary (NEW)
    src/
      main.rs               # CLI parsing, subcommand dispatch
      daemon.rs             # daemon main loop
      wrapper.rs            # PTY wrapper
      hook_cmd.rs           # hook subcommand
      cli.rs                # status/messages/sessions commands
      setup.rs              # hook installation, dependency checks
      agents/
        mod.rs              # AgentProfile struct, lookup
        claude.rs           # Claude Code profile
      client_listener.rs    # (from agent-dashd)
      hook_listener.rs      # (from agent-dashd)
      scanner.rs            # (from agent-dashd)
      state.rs              # (from agent-dashd)
      messages.rs           # (from agent-dashd)
      watcher.rs            # (from agent-dashd)
```

Old crates (`agent-dashd`, `agent-dash-hook`, `agentctl`) are removed from the workspace.

## CLI

```
agent-dash run <agent> [-- args...]     # PTY-wrapped agent session
agent-dash status                       # list all sessions
agent-dash messages <session> [format] [limit]
agent-dash sessions <project>
agent-dash watch <session> [format]
agent-dash inject <session> <prompt>    # CLI prompt injection
agent-dash daemon start|stop|status
agent-dash hook <event-type>            # called by Claude Code hooks
agent-dash setup [hooks]                # install hooks, check deps
agent-dash update                       # self-upgrade (deferred)
```

## New Dependencies

- `portable-pty` — Cross-platform PTY management (used by wezterm). Provides `openpty()`, `spawn_command()`, `take_writer()` (for injection), `try_clone_reader()` (for output capture), and `resize()`.

## Deferred

- **Self-update** (`agent-dash update`) — GitHub releases + `self_update` crate. Build after core is working.
- **Raw terminal streaming** — Expose PTY output to remote clients. Add when a use case demands it.
- **Agent auto-install** — `agent-dash run claude` could install Claude Code if missing. Add after the basics work.
