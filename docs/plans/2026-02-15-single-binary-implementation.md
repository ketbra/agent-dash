# Single Binary with PTY Wrapper — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Collapse three binaries (agent-dashd, agent-dash-hook, agentctl) into a single `agent-dash` binary with PTY-based prompt injection via `portable-pty`.

**Architecture:** A single crate produces one binary. Subcommands select runtime mode: `run` (PTY wrapper), `daemon` (background state manager), `hook` (Claude Code hook handler), or top-level CLI commands (status, messages, etc.). The wrapper connects to the daemon as a client, registers itself, and accepts prompt injection over the same connection.

**Tech Stack:** Rust, tokio, portable-pty 0.9, clap 4, crossterm (raw mode), interprocess (sockets), sysinfo, notify, comrak.

**Design Doc:** `docs/plans/2026-02-15-single-binary-design.md`

---

### Task 1: Scaffold the new crate with dependencies and CLI skeleton

**Files:**
- Create: `crates/agent-dash/Cargo.toml`
- Create: `crates/agent-dash/src/main.rs`
- Modify: `Cargo.toml` (workspace root, add member + new deps)

**Context:** This creates the new binary crate that will eventually replace all three existing binaries. We use `clap` for subcommand parsing and add `portable-pty` and `crossterm` as new workspace deps. The main.rs just parses args and prints stubs — real implementations come in later tasks.

**Step 1: Add new workspace dependencies**

In the workspace root `Cargo.toml`, add these to `[workspace.dependencies]`:

```toml
clap = { version = "4", features = ["derive"] }
portable-pty = "0.9"
crossterm = "0.28"
```

And add `"crates/agent-dash"` to the `members` list.

**Step 2: Create `crates/agent-dash/Cargo.toml`**

```toml
[package]
name = "agent-dash"
version = "0.1.0"
edition = "2024"

[dependencies]
agent-dash-core = { path = "../agent-dash-core" }
serde = { workspace = true }
serde_json = { workspace = true }
dirs = { workspace = true }
tokio = { workspace = true }
interprocess = { workspace = true }
sysinfo = { workspace = true }
notify = { workspace = true }
comrak = { workspace = true }
clap = { workspace = true }
portable-pty = { workspace = true }
crossterm = { workspace = true }
```

**Step 3: Create `crates/agent-dash/src/main.rs` with clap CLI skeleton**

Define all subcommands with clap derive macros:

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent-dash", about = "Agent dashboard and PTY wrapper")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Wrap and run an agent in a PTY
    Run {
        /// Agent to run (e.g., claude, codex)
        agent: String,
        /// Arguments to pass through to the agent
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage the daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Show all active sessions
    Status,
    /// Fetch messages from a session
    Messages {
        session_id: String,
        #[arg(default_value = "structured")]
        format: String,
        #[arg(default_value_t = 20)]
        limit: usize,
    },
    /// List JSONL sessions for a project
    Sessions {
        project: String,
    },
    /// Stream new messages from a session
    Watch {
        session_id: String,
        #[arg(default_value = "structured")]
        format: String,
    },
    /// Inject a prompt into a wrapped session
    Inject {
        session_id: String,
        text: String,
    },
    /// Handle a Claude Code hook event (called by hooks config)
    Hook {
        /// Hook event type (tool-start, tool-end, stop, session-start, session-end, permission)
        event_type: String,
    },
    /// Install hooks and check dependencies
    Setup {
        /// What to set up
        #[arg(default_value = "all")]
        target: String,
    },
    /// Subscribe to raw daemon event stream
    WatchEvents,
    /// Respond to a permission request
    Approve {
        request_id: String,
    },
    /// Respond to a permission request with allow-similar
    ApproveSimilar {
        request_id: String,
    },
    /// Deny a permission request
    Deny {
        request_id: String,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon
    Start,
    /// Stop the daemon
    Stop,
    /// Show daemon status
    Status,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Status) | None => {
            println!("status: not yet implemented");
        }
        Some(cmd) => {
            println!("command not yet implemented: {:?}", std::any::type_name_of_val(&cmd));
        }
    }
}
```

**Step 4: Verify it compiles**

Run: `cargo build -p agent-dash`
Expected: Clean build with no errors.

**Step 5: Verify help works**

Run: `cargo run -p agent-dash -- --help`
Expected: Shows help text with all subcommands listed.

**Step 6: Commit**

```bash
git add crates/agent-dash/ Cargo.toml
git commit -m "feat: scaffold agent-dash single binary crate with clap CLI"
```

---

### Task 2: Agent profiles

**Files:**
- Create: `crates/agent-dash/src/agents/mod.rs`
- Create: `crates/agent-dash/src/agents/claude.rs`
- Modify: `crates/agent-dash/src/main.rs` (add `mod agents;`)

**Context:** Agent profiles are simple structs that describe how to find and launch a supported agent. For now only Claude Code is supported. The profile is used by the PTY wrapper (Task 7) to construct the command. Keep it dead simple — a const struct, no traits.

**Step 1: Create `crates/agent-dash/src/agents/mod.rs`**

```rust
pub mod claude;

/// Describes a supported agent binary.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    /// Short name used in CLI (e.g., "claude")
    pub name: &'static str,
    /// Binary name to find in $PATH
    pub binary: &'static str,
    /// Human-readable name
    pub display_name: &'static str,
    /// How to install the agent if it's not found
    pub install_hint: &'static str,
}

/// Look up an agent profile by name. Returns None if unknown.
pub fn lookup(name: &str) -> Option<&'static AgentProfile> {
    match name {
        "claude" => Some(&claude::PROFILE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_claude() {
        let profile = lookup("claude").unwrap();
        assert_eq!(profile.name, "claude");
        assert_eq!(profile.binary, "claude");
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("unknown-agent").is_none());
    }
}
```

**Step 2: Create `crates/agent-dash/src/agents/claude.rs`**

```rust
use super::AgentProfile;

pub const PROFILE: AgentProfile = AgentProfile {
    name: "claude",
    binary: "claude",
    display_name: "Claude Code",
    install_hint: "curl -fsSL https://claude.ai/install.sh | bash",
};
```

**Step 3: Add `mod agents;` to main.rs**

Add `mod agents;` near the top of `crates/agent-dash/src/main.rs`.

**Step 4: Run tests**

Run: `cargo test -p agent-dash`
Expected: 2 tests pass (lookup_claude, lookup_unknown_returns_none).

**Step 5: Commit**

```bash
git add crates/agent-dash/src/agents/
git commit -m "feat: add agent profile system with Claude Code profile"
```

---

### Task 3: Protocol additions for wrapper support

**Files:**
- Modify: `crates/agent-dash-core/src/protocol.rs`

**Context:** The daemon needs new request/response types so wrappers can register themselves and clients can inject prompts. These are additions to the existing `ClientRequest` and `ServerEvent` enums — don't modify existing variants.

**Step 1: Add new `ClientRequest` variants**

After the existing `ListSessions` variant in the `ClientRequest` enum, add:

```rust
    #[serde(rename = "register_wrapper")]
    RegisterWrapper {
        session_id: String,
        agent: String,
    },
    #[serde(rename = "unregister_wrapper")]
    UnregisterWrapper {
        session_id: String,
    },
    #[serde(rename = "send_prompt")]
    SendPrompt {
        session_id: String,
        text: String,
    },
```

**Step 2: Add new `ServerEvent` variants**

After the existing `SessionList` variant in the `ServerEvent` enum, add:

```rust
    #[serde(rename = "prompt_sent")]
    PromptSent {
        session_id: String,
    },
    #[serde(rename = "inject_prompt")]
    InjectPrompt {
        text: String,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
    },
```

**Step 3: Add tests**

Add these test functions in the existing `mod tests` block:

```rust
    #[test]
    fn deserialize_register_wrapper() {
        let json = r#"{"method":"register_wrapper","session_id":"s1","agent":"claude"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::RegisterWrapper { session_id, agent } => {
                assert_eq!(session_id, "s1");
                assert_eq!(agent, "claude");
            }
            _ => panic!("expected RegisterWrapper"),
        }
    }

    #[test]
    fn deserialize_unregister_wrapper() {
        let json = r#"{"method":"unregister_wrapper","session_id":"s1"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::UnregisterWrapper { .. }));
    }

    #[test]
    fn deserialize_send_prompt() {
        let json = r#"{"method":"send_prompt","session_id":"s1","text":"fix the tests"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::SendPrompt { session_id, text } => {
                assert_eq!(session_id, "s1");
                assert_eq!(text, "fix the tests");
            }
            _ => panic!("expected SendPrompt"),
        }
    }

    #[test]
    fn serialize_prompt_sent() {
        let event = ServerEvent::PromptSent { session_id: "s1".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"prompt_sent\""));
    }

    #[test]
    fn serialize_inject_prompt() {
        let event = ServerEvent::InjectPrompt { text: "hello".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"inject_prompt\""));
        assert!(json.contains("\"text\":\"hello\""));
    }

    #[test]
    fn serialize_error() {
        let event = ServerEvent::Error { message: "not wrapped".into() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"error\""));
    }
```

**Step 4: Run tests**

Run: `cargo test -p agent-dash-core`
Expected: All existing + 6 new tests pass.

**Step 5: Commit**

```bash
git add crates/agent-dash-core/src/protocol.rs
git commit -m "feat(protocol): add wrapper registration and prompt injection types"
```

---

### Task 4: Move daemon modules into new crate

**Files:**
- Create: `crates/agent-dash/src/state.rs` (copy from agent-dashd)
- Create: `crates/agent-dash/src/scanner.rs` (copy from agent-dashd)
- Create: `crates/agent-dash/src/messages.rs` (copy from agent-dashd)
- Create: `crates/agent-dash/src/watcher.rs` (copy from agent-dashd)
- Create: `crates/agent-dash/src/hook_listener.rs` (copy from agent-dashd)
- Create: `crates/agent-dash/src/client_listener.rs` (copy from agent-dashd)
- Create: `crates/agent-dash/src/daemon.rs` (adapted from agent-dashd/src/main.rs)
- Modify: `crates/agent-dash/src/main.rs` (add mod declarations, wire daemon start)

**Context:** This is the largest task — migrating all daemon modules from `agent-dashd` into the new `agent-dash` crate. The code is essentially a copy, but internal crate references change from `agent_dashd::*` to `crate::*`. The daemon's `main()` becomes a `pub async fn run()` in `daemon.rs` so the CLI can call it.

**Step 1: Copy all 6 daemon modules**

Copy these files from `crates/agent-dashd/src/` to `crates/agent-dash/src/`, with NO changes needed since they all reference `agent_dash_core::*` (external crate) not `agent_dashd::*`:
- `state.rs`
- `scanner.rs`
- `messages.rs`
- `watcher.rs`
- `hook_listener.rs`
- `client_listener.rs`

These modules use `agent_dash_core::*` for external types and `crate::` for nothing (they don't cross-reference each other through the crate root). They should compile as-is.

**Step 2: Create `daemon.rs`**

Adapt `crates/agent-dashd/src/main.rs` into `crates/agent-dash/src/daemon.rs`. The key change: replace `fn main()` with `pub async fn run()` (the `#[tokio::main]` moves to the CLI dispatch). Replace `agent_dashd::` references with `crate::`. The full content is essentially the existing main.rs body wrapped in `pub async fn run()`, using `crate::client_listener`, `crate::hook_listener`, etc.

The function signature:

```rust
pub async fn run() {
    // ... exact same body as current agent-dashd main(), with crate:: imports
}
```

**Step 3: Add module declarations to main.rs**

Add these to the top of `crates/agent-dash/src/main.rs`:

```rust
mod agents;
mod client_listener;
mod daemon;
mod hook_listener;
mod messages;
mod scanner;
mod state;
mod watcher;
```

**Step 4: Wire up the `daemon start` subcommand**

In the `main()` match arm for `Commands::Daemon { action: DaemonAction::Start }`:

```rust
Some(Commands::Daemon { action }) => match action {
    DaemonAction::Start => {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(daemon::run());
    }
    _ => {
        println!("not yet implemented");
    }
},
```

Note: Since `main()` is sync (clap parsing), we create the tokio runtime manually for the daemon subcommand. Alternatively, mark `main()` as `#[tokio::main]` and `.await` the daemon. Choose whichever approach compiles cleanly.

**Step 5: Verify it compiles and tests pass**

Run: `cargo build -p agent-dash`
Run: `cargo test -p agent-dash`
Expected: All tests from the moved modules pass (state tests, scanner tests, messages tests, watcher tests).

**Step 6: Commit**

```bash
git add crates/agent-dash/src/
git commit -m "feat: move daemon modules into agent-dash crate"
```

---

### Task 5: Move hook command into new crate

**Files:**
- Create: `crates/agent-dash/src/hook_cmd.rs`
- Modify: `crates/agent-dash/src/main.rs` (add mod, wire hook subcommand)

**Context:** The hook command reads JSON from stdin, extracts session/tool info, and sends events to the daemon. It's currently the `agent-dash-hook` binary. Move it into a module with a `pub fn run(event_type: &str)` entry point.

**Step 1: Create `hook_cmd.rs`**

Copy the contents of `crates/agent-dash-hook/src/main.rs` into `crates/agent-dash/src/hook_cmd.rs`. Make these changes:
- Rename `fn main()` to `pub fn run(event_type: &str)`
- Remove the args parsing at the top (the event_type comes from the clap CLI now)
- The `subcommand` variable becomes the `event_type` parameter
- Keep all helper functions (`extract_tool_detail`, `send_hook_event`, `handle_tool_start`, etc.) as-is
- Keep all existing tests

**Step 2: Add mod declaration and wire CLI**

In `main.rs`, add `mod hook_cmd;` and wire the match arm:

```rust
Some(Commands::Hook { event_type }) => {
    hook_cmd::run(&event_type);
}
```

**Step 3: Run tests**

Run: `cargo test -p agent-dash -- hook`
Expected: All existing hook tests pass (extract_tool_detail_*, translate_*).

**Step 4: Commit**

```bash
git add crates/agent-dash/src/hook_cmd.rs crates/agent-dash/src/main.rs
git commit -m "feat: move hook command into agent-dash binary"
```

---

### Task 6: Move CLI commands into new crate

**Files:**
- Create: `crates/agent-dash/src/cli.rs`
- Modify: `crates/agent-dash/src/main.rs` (add mod, wire all CLI subcommands)

**Context:** The agentctl binary's commands (status, watch, messages, sessions, approve/deny, etc.) move into `cli.rs`. Each becomes a public function called from the clap dispatch.

**Step 1: Create `cli.rs`**

Copy the helper functions and command implementations from `crates/agentctl/src/main.rs` into `crates/agent-dash/src/cli.rs`:
- `fn connect() -> UnixStream` (no change)
- `fn send_request(...)` (no change)
- `fn truncate(...)` (no change)
- `pub fn cmd_status()` (make pub)
- `pub fn cmd_watch()` (make pub)
- `pub fn cmd_permission_response(...)` (make pub)
- `pub fn cmd_messages(...)` (make pub)
- `pub fn cmd_sessions(...)` (make pub)
- `pub fn cmd_watch_messages(...)` (make pub)

Add a new function for the inject command:

```rust
/// Send a prompt to a wrapped session.
pub fn cmd_inject(session_id: &str, text: &str) {
    let mut conn = connect();
    let req = ClientRequest::SendPrompt {
        session_id: session_id.to_string(),
        text: text.to_string(),
    };
    send_request(&mut conn, &req);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&line) else {
            continue;
        };
        match event {
            ServerEvent::PromptSent { session_id } => {
                println!("Prompt sent to {}", truncate(&session_id, 8));
                return;
            }
            ServerEvent::Error { message } => {
                eprintln!("Error: {message}");
                std::process::exit(1);
            }
            _ => {}
        }
    }
}
```

**Step 2: Wire all CLI subcommands in main.rs**

Update the match in `main()`:

```rust
match cli.command {
    Some(Commands::Run { agent, args }) => {
        println!("run: not yet implemented");
    }
    Some(Commands::Daemon { action }) => match action {
        DaemonAction::Start => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(daemon::run());
        }
        DaemonAction::Stop => println!("daemon stop: not yet implemented"),
        DaemonAction::Status => println!("daemon status: not yet implemented"),
    },
    Some(Commands::Status) | None => cli::cmd_status(),
    Some(Commands::Messages { session_id, format, limit }) => {
        cli::cmd_messages(&session_id, &format, limit);
    }
    Some(Commands::Sessions { project }) => cli::cmd_sessions(&project),
    Some(Commands::Watch { session_id, format }) => {
        cli::cmd_watch_messages(&session_id, &format);
    }
    Some(Commands::WatchEvents) => cli::cmd_watch(),
    Some(Commands::Inject { session_id, text }) => {
        cli::cmd_inject(&session_id, &text);
    }
    Some(Commands::Hook { event_type }) => hook_cmd::run(&event_type),
    Some(Commands::Setup { target }) => println!("setup: not yet implemented"),
    Some(Commands::Approve { request_id }) => {
        cli::cmd_permission_response(&request_id, "allow");
    }
    Some(Commands::ApproveSimilar { request_id }) => {
        cli::cmd_permission_response(&request_id, "allow_similar");
    }
    Some(Commands::Deny { request_id }) => {
        cli::cmd_permission_response(&request_id, "deny");
    }
}
```

**Step 3: Verify it compiles and the daemon commands work**

Run: `cargo build -p agent-dash`
Run: `cargo run -p agent-dash -- --help`
Expected: Clean build, help shows all subcommands.

**Step 4: Commit**

```bash
git add crates/agent-dash/src/cli.rs crates/agent-dash/src/main.rs
git commit -m "feat: move CLI commands into agent-dash binary"
```

---

### Task 7: PTY wrapper implementation

**Files:**
- Create: `crates/agent-dash/src/wrapper.rs`
- Modify: `crates/agent-dash/src/main.rs` (add mod, wire `run` subcommand)

**Context:** This is the core new feature. The wrapper spawns an agent inside a PTY, bridges stdin/stdout transparently, connects to the daemon, registers as a wrapper, and listens for prompt injection commands. Uses `portable-pty` for PTY management and `crossterm` for raw terminal mode.

**Step 1: Create `wrapper.rs`**

```rust
use crate::agents::AgentProfile;
use agent_dash_core::paths;
use agent_dash_core::protocol::{ClientRequest, ServerEvent, encode_line};
use crossterm::terminal;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Run an agent inside a PTY wrapper.
/// This function blocks until the child process exits.
pub fn run(profile: &AgentProfile, args: &[String]) -> i32 {
    // Check that the agent binary exists in PATH.
    if which(profile.binary).is_none() {
        eprintln!(
            "{} not found in PATH. Install it with:\n  {}",
            profile.display_name, profile.install_hint
        );
        return 1;
    }

    // Get current terminal size.
    let (cols, rows) = terminal::size().unwrap_or((80, 24));

    // Create the PTY.
    let pty_system = NativePtySystem::default();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open pty");

    // Build the command.
    let mut cmd = CommandBuilder::new(profile.binary);
    for arg in args {
        cmd.arg(arg);
    }

    // Spawn the agent.
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn agent");
    // Drop the slave — the child now owns its end.
    drop(pair.slave);

    // Get reader and writer for the master side.
    let mut pty_reader = pair.master.try_clone_reader().expect("failed to clone pty reader");
    let mut pty_writer = pair.master.take_writer().expect("failed to take pty writer");

    // Try to connect to the daemon and register.
    let daemon_conn = try_connect_daemon();
    let session_id = format!("wrap-{}", std::process::id());

    if let Some(ref mut conn) = daemon_conn.as_ref() {
        let req = ClientRequest::RegisterWrapper {
            session_id: session_id.clone(),
            agent: profile.name.to_string(),
        };
        let _ = send_to_daemon(conn, &req);
    }

    // Enter raw mode so keypresses flow immediately.
    let _ = terminal::enable_raw_mode();
    let running = Arc::new(AtomicBool::new(true));

    // --- Thread 1: stdin -> pty (user typing) ---
    let running_stdin = running.clone();
    let mut pty_writer_stdin = pair.master.take_writer().unwrap_or_else(|_| {
        // take_writer() can only be called once; for the second writer,
        // we need try_clone_writer(). Since portable-pty doesn't expose this,
        // we'll use a shared writer approach via a channel instead.
        // For now, this thread uses the pty_writer directly.
        unreachable!("writer already taken");
    });

    // Actually, portable-pty's take_writer() can only be called once.
    // We need a different approach: use a channel to multiplex stdin and
    // injected prompts into a single writer thread.

    // Channel for data to write to the PTY.
    let (write_tx, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();

    // Writer thread: reads from channel, writes to PTY.
    let running_writer = running.clone();
    let writer_handle = std::thread::spawn(move || {
        while running_writer.load(Ordering::Relaxed) {
            match write_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(data) => {
                    if pty_writer.write_all(&data).is_err() {
                        break;
                    }
                    let _ = pty_writer.flush();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    // stdin reader thread: reads user input, sends to writer channel.
    let write_tx_stdin = write_tx.clone();
    let running_stdin2 = running.clone();
    let stdin_handle = std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 1024];
        while running_stdin2.load(Ordering::Relaxed) {
            match stdin.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if write_tx_stdin.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // PTY reader thread: reads agent output, writes to stdout.
    let running_reader = running.clone();
    let reader_handle = std::thread::spawn(move || {
        let mut stdout = std::io::stdout().lock();
        let mut buf = [0u8; 4096];
        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) => break, // PTY closed
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = stdout.flush();
                }
                Err(_) => break,
            }
        }
    });

    // Daemon listener thread: listens for inject_prompt commands.
    let write_tx_inject = write_tx.clone();
    let running_daemon = running.clone();
    if let Some(conn) = daemon_conn {
        std::thread::spawn(move || {
            let reader = BufReader::new(&conn);
            for line in reader.lines() {
                if !running_daemon.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<ServerEvent>(&line) {
                    if let ServerEvent::InjectPrompt { text } = event {
                        let mut data = text.into_bytes();
                        data.push(b'\n');
                        let _ = write_tx_inject.send(data);
                    }
                }
            }
        });
    }

    // Handle SIGWINCH for terminal resize.
    #[cfg(unix)]
    {
        let master = pair.master;
        let running_sig = running.clone();
        std::thread::spawn(move || {
            use std::sync::mpsc as std_mpsc;
            // Use signal_hook or poll terminal size periodically.
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if !running_sig.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok((new_cols, new_rows)) = terminal::size() {
                    let _ = master.resize(PtySize {
                        rows: new_rows,
                        cols: new_cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        });
    }

    // Wait for the child process to exit.
    let status = child.wait().expect("failed to wait on child");
    running.store(false, Ordering::Relaxed);

    // Restore terminal.
    let _ = terminal::disable_raw_mode();

    // Unregister from daemon.
    if let Ok(mut conn) = UnixStream::connect(paths::client_socket_name()) {
        let req = ClientRequest::UnregisterWrapper {
            session_id: session_id.clone(),
        };
        let _ = send_to_daemon(&conn, &req);
    }

    // Wait for threads.
    drop(write_tx); // Signals writer thread to stop.
    let _ = writer_handle.join();
    let _ = reader_handle.join();
    // stdin_handle may block on read — don't wait.

    status.exit_code() as i32
}

/// Try to connect to the daemon. Returns None if daemon is not running.
fn try_connect_daemon() -> Option<UnixStream> {
    let sock = paths::client_socket_name();
    UnixStream::connect(&sock).ok()
}

/// Send a request to the daemon connection.
fn send_to_daemon(conn: &UnixStream, req: &ClientRequest) -> Result<(), std::io::Error> {
    let line = encode_line(req).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    (&*conn).write_all(line.as_bytes())?;
    (&*conn).flush()
}

/// Check if a binary exists in PATH.
fn which(binary: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full_path = dir.join(binary);
            if full_path.is_file() {
                Some(full_path)
            } else {
                None
            }
        })
    })
}
```

Note: The above is a starting point. The implementer should adjust based on `portable-pty` 0.9's actual API (e.g., `NativePtySystem::default()` may need to be `native_pty_system()`). Check docs at https://docs.rs/portable-pty/0.9.0/portable_pty/.

The `take_writer()` can only be called once. The solution above uses a channel to multiplex stdin and injected prompts into a single writer thread. This is the correct pattern.

**Step 2: Wire the `run` subcommand in main.rs**

```rust
Some(Commands::Run { agent, args }) => {
    let profile = agents::lookup(&agent).unwrap_or_else(|| {
        eprintln!("Unknown agent: {agent}");
        eprintln!("Supported agents: claude");
        std::process::exit(1);
    });
    let exit_code = wrapper::run(profile, &args);
    std::process::exit(exit_code);
}
```

**Step 3: Add `mod wrapper;` to main.rs**

**Step 4: Verify it compiles**

Run: `cargo build -p agent-dash`
Expected: Clean build. Manual testing of `cargo run -p agent-dash -- run claude` should launch Claude Code inside the PTY wrapper (requires Claude Code installed).

**Step 5: Commit**

```bash
git add crates/agent-dash/src/wrapper.rs crates/agent-dash/src/main.rs
git commit -m "feat: implement PTY wrapper for agent sessions"
```

---

### Task 8: Daemon wrapper support

**Files:**
- Modify: `crates/agent-dash/src/client_listener.rs`
- Modify: `crates/agent-dash/src/daemon.rs`
- Modify: `crates/agent-dash/src/state.rs`

**Context:** The daemon needs to handle wrapper registration, track which sessions are wrapped, and route `send_prompt` requests to the right wrapper. This wires up the protocol types from Task 3.

**Step 1: Add `wrapped` field to `InternalSession` in `state.rs`**

Add to the `InternalSession` struct:

```rust
    pub wrapped: bool,
    pub agent: Option<String>,
```

Initialize both in `ensure_session`: `wrapped: false`, `agent: None`.

**Step 2: Add new `ClientMessage` variants in `client_listener.rs`**

Add to the `ClientMessage` enum:

```rust
    /// Wrapper registering itself.
    RegisterWrapper {
        session_id: String,
        agent: String,
        prompt_tx: mpsc::Sender<String>,
    },
    /// Wrapper unregistering.
    UnregisterWrapper {
        session_id: String,
    },
    /// Client requesting prompt injection.
    SendPrompt {
        session_id: String,
        text: String,
        reply: oneshot::Sender<String>,
    },
```

**Step 3: Handle new request types in `handle_client_connection`**

Add match arms for the new `ClientRequest` variants:

```rust
ClientRequest::RegisterWrapper { session_id, agent } => {
    // Create a channel for prompt injection.
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(16);
    let _ = tx
        .send(ClientMessage::RegisterWrapper {
            session_id: session_id.clone(),
            agent,
            prompt_tx,
        })
        .await;

    // Listen for prompts and send them back to the wrapper.
    while let Some(text) = prompt_rx.recv().await {
        let event = ServerEvent::InjectPrompt { text };
        if let Ok(line) = protocol::encode_line(&event) {
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    }

    // When the prompt channel closes or the connection breaks,
    // unregister the wrapper.
    let _ = tx
        .send(ClientMessage::UnregisterWrapper { session_id })
        .await;
    return;
}
ClientRequest::UnregisterWrapper { session_id } => {
    let _ = tx
        .send(ClientMessage::UnregisterWrapper { session_id })
        .await;
}
ClientRequest::SendPrompt { session_id, text } => {
    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = tx
        .send(ClientMessage::SendPrompt {
            session_id,
            text,
            reply: reply_tx,
        })
        .await;
    if let Ok(json) = reply_rx.await {
        let _ = writer.write_all(json.as_bytes()).await;
    }
}
```

**Step 4: Handle new messages in daemon.rs main loop**

Add to the `ClientMessage` match in the `tokio::select!` loop:

```rust
ClientMessage::RegisterWrapper {
    session_id,
    agent,
    prompt_tx,
} => {
    state.ensure_session(&session_id);
    if let Some(session) = state.sessions.get_mut(&session_id) {
        session.wrapped = true;
        session.agent = Some(agent);
    }
    wrapper_channels.insert(session_id, prompt_tx);
    state_dirty = true;
    broadcast_state(&mut subscribers, &state);
}
ClientMessage::UnregisterWrapper { session_id } => {
    wrapper_channels.remove(&session_id);
    if let Some(session) = state.sessions.get_mut(&session_id) {
        session.wrapped = false;
        session.agent = None;
    }
    state_dirty = true;
    broadcast_state(&mut subscribers, &state);
}
ClientMessage::SendPrompt {
    session_id,
    text,
    reply,
} => {
    let response = if let Some(prompt_tx) = wrapper_channels.get(&session_id) {
        if prompt_tx.try_send(text).is_ok() {
            ServerEvent::PromptSent { session_id }
        } else {
            ServerEvent::Error {
                message: "wrapper channel full or closed".into(),
            }
        }
    } else {
        ServerEvent::Error {
            message: "session is not wrapped".into(),
        }
    };
    let _ = reply.send(protocol::encode_line(&response).unwrap_or_default());
}
```

Add `wrapper_channels` to the daemon state setup:

```rust
let mut wrapper_channels: HashMap<String, mpsc::Sender<String>> = HashMap::new();
```

**Step 5: Verify it compiles**

Run: `cargo build -p agent-dash`
Run: `cargo test -p agent-dash`
Expected: Clean build, all existing tests pass.

**Step 6: Commit**

```bash
git add crates/agent-dash/src/client_listener.rs crates/agent-dash/src/daemon.rs crates/agent-dash/src/state.rs
git commit -m "feat(daemon): add wrapper registration and prompt routing"
```

---

### Task 9: Hook setup command

**Files:**
- Create: `crates/agent-dash/src/setup.rs`
- Modify: `crates/agent-dash/src/main.rs` (add mod, wire setup subcommand)

**Context:** `agent-dash setup hooks` installs the hook configuration into `~/.claude/settings.json`. It reads existing settings, merges in agent-dash hook entries, and writes back. Must be idempotent.

**Step 1: Create `setup.rs`**

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Install agent-dash hooks into Claude Code settings.
/// Returns Ok(true) if changes were made, Ok(false) if already up to date.
pub fn install_hooks(project_level: bool) -> Result<bool, String> {
    let settings_path = if project_level {
        PathBuf::from(".claude/settings.json")
    } else {
        dirs::home_dir()
            .ok_or_else(|| "cannot determine home directory".to_string())?
            .join(".claude")
            .join("settings.json")
    };

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content =
            std::fs::read_to_string(&settings_path).map_err(|e| format!("read error: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("parse error: {e}"))?
    } else {
        serde_json::json!({})
    };

    let hooks = settings
        .as_object_mut()
        .ok_or_else(|| "settings is not an object".to_string())?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_obj = hooks
        .as_object_mut()
        .ok_or_else(|| "hooks is not an object".to_string())?;

    let hook_events = [
        ("PreToolUse", "pre-tool-use"),
        ("PostToolUse", "post-tool-use"),
        ("Stop", "stop"),
        ("NotificationStart", "session-start"),
        ("SessionEnd", "session-end"),
    ];

    let mut changed = false;

    for (event_name, cmd_arg) in &hook_events {
        let command = format!("agent-dash hook {cmd_arg}");
        let new_entry = serde_json::json!({
            "type": "command",
            "command": command
        });

        let entries = hooks_obj
            .entry(event_name.to_string())
            .or_insert_with(|| serde_json::json!([]));

        let arr = entries
            .as_array_mut()
            .ok_or_else(|| format!("{event_name} is not an array"))?;

        // Check if agent-dash hook is already installed.
        let already_installed = arr.iter().any(|entry| {
            entry
                .get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.starts_with("agent-dash hook"))
        });

        if already_installed {
            // Update the existing entry in case the command changed.
            for entry in arr.iter_mut() {
                if entry
                    .get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.starts_with("agent-dash hook"))
                {
                    if *entry != new_entry {
                        *entry = new_entry.clone();
                        changed = true;
                    }
                }
            }
        } else {
            arr.push(new_entry);
            changed = true;
        }
    }

    if changed {
        // Ensure parent directory exists.
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir error: {e}"))?;
        }
        let json = serde_json::to_string_pretty(&settings)
            .map_err(|e| format!("serialize error: {e}"))?;
        std::fs::write(&settings_path, json).map_err(|e| format!("write error: {e}"))?;
    }

    Ok(changed)
}

/// Check if hooks are installed in the user-level settings.
pub fn hooks_installed() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let settings_path = home.join(".claude").join("settings.json");
    let Ok(content) = std::fs::read_to_string(&settings_path) else {
        return false;
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    settings
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(|arr| arr.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|e| {
                e.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.starts_with("agent-dash hook"))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hooks_creates_settings_file() {
        let dir = std::env::temp_dir().join("agent-dash-test-setup");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let settings_path = dir.join("settings.json");

        // Manually test the merge logic with a temp file.
        let mut settings = serde_json::json!({});
        let hooks = settings
            .as_object_mut()
            .unwrap()
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}));
        let hooks_obj = hooks.as_object_mut().unwrap();

        let entries = hooks_obj
            .entry("PreToolUse")
            .or_insert_with(|| serde_json::json!([]));
        let arr = entries.as_array_mut().unwrap();
        arr.push(serde_json::json!({"type": "command", "command": "agent-dash hook pre-tool-use"}));

        // Verify the structure.
        let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["command"]
            .as_str()
            .unwrap()
            .starts_with("agent-dash hook"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn install_hooks_preserves_existing_hooks() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "my-custom-hook pre-tool-use"}
                ]
            }
        });

        let hooks_obj = settings["hooks"].as_object_mut().unwrap();
        let arr = hooks_obj
            .get_mut("PreToolUse")
            .unwrap()
            .as_array_mut()
            .unwrap();
        arr.push(serde_json::json!({
            "type": "command",
            "command": "agent-dash hook pre-tool-use"
        }));

        let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(
            arr[0]["command"].as_str().unwrap(),
            "my-custom-hook pre-tool-use"
        );
        assert!(arr[1]["command"]
            .as_str()
            .unwrap()
            .starts_with("agent-dash hook"));
    }

    #[test]
    fn install_hooks_idempotent() {
        let mut settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "agent-dash hook pre-tool-use"}
                ]
            }
        });

        let arr = settings["hooks"]["PreToolUse"].as_array().unwrap();
        let already = arr.iter().any(|e| {
            e.get("command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.starts_with("agent-dash hook"))
        });
        assert!(already);
    }
}
```

**Step 2: Wire the setup subcommand in main.rs**

```rust
Some(Commands::Setup { target }) => {
    match target.as_str() {
        "hooks" | "all" => {
            match setup::install_hooks(false) {
                Ok(true) => println!("Hooks installed successfully."),
                Ok(false) => println!("Hooks already up to date."),
                Err(e) => {
                    eprintln!("Failed to install hooks: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("Unknown setup target: {target}");
            eprintln!("Available: hooks, all");
            std::process::exit(1);
        }
    }
}
```

Add `mod setup;` to main.rs.

**Step 3: Run tests**

Run: `cargo test -p agent-dash -- setup`
Expected: 3 tests pass.

**Step 4: Commit**

```bash
git add crates/agent-dash/src/setup.rs crates/agent-dash/src/main.rs
git commit -m "feat: add hook setup command"
```

---

### Task 10: Daemon auto-start in `run` command

**Files:**
- Modify: `crates/agent-dash/src/wrapper.rs`

**Context:** When `agent-dash run claude` starts, it should check if the daemon is running. If not, fork `agent-dash daemon start` as a background process and wait for the socket to become available.

**Step 1: Add `ensure_daemon()` function to `wrapper.rs`**

```rust
/// Ensure the daemon is running. If not, fork it as a background process.
/// Returns true if the daemon is available, false if it couldn't be started.
fn ensure_daemon() -> bool {
    let sock = paths::client_socket_name();

    // Try connecting — if it works, daemon is already running.
    if UnixStream::connect(&sock).is_ok() {
        return true;
    }

    eprintln!("Starting agent-dash daemon...");

    // Get path to our own binary.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    // Fork the daemon as a background process.
    let child = std::process::Command::new(&exe)
        .arg("daemon")
        .arg("start")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();

    if child.is_err() {
        eprintln!("Failed to start daemon");
        return false;
    }

    // Wait up to 2 seconds for the socket to appear.
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if UnixStream::connect(&sock).is_ok() {
            return true;
        }
    }

    eprintln!("Daemon did not start within 2 seconds");
    false
}
```

**Step 2: Call `ensure_daemon()` at the start of `run()`**

At the beginning of the `run()` function, before PTY setup:

```rust
pub fn run(profile: &AgentProfile, args: &[String]) -> i32 {
    // Check agent binary.
    if which(profile.binary).is_none() { ... }

    // Ensure daemon is running.
    ensure_daemon();

    // Check if hooks are installed — offer to install on first run.
    if !crate::setup::hooks_installed() {
        eprintln!("agent-dash hooks not installed. Run `agent-dash setup hooks` to install.");
    }

    // ... rest of PTY setup
}
```

**Step 3: Verify it compiles**

Run: `cargo build -p agent-dash`
Expected: Clean build.

**Step 4: Commit**

```bash
git add crates/agent-dash/src/wrapper.rs
git commit -m "feat: auto-start daemon when running PTY wrapper"
```

---

### Task 11: Remove old crates and clean up workspace

**Files:**
- Remove: `crates/agent-dashd/` (entire directory)
- Remove: `crates/agent-dash-hook/` (entire directory)
- Remove: `crates/agentctl/` (entire directory)
- Modify: `Cargo.toml` (workspace root — remove old members)

**Context:** All code has been migrated to the `agent-dash` crate. The old crates are now dead code. Remove them from the workspace.

**Step 1: Remove old crate directories**

Delete:
- `crates/agent-dashd/`
- `crates/agent-dash-hook/`
- `crates/agentctl/`

**Step 2: Update workspace `Cargo.toml`**

Change the members list to:

```toml
[workspace]
members = [
    "crates/agent-dash-core",
    "crates/agent-dash",
]
```

**Step 3: Verify everything still works**

Run: `cargo build --workspace`
Run: `cargo test --workspace`
Expected: Clean build, all tests pass. Only `agent-dash-core` and `agent-dash` remain.

**Step 4: Verify CLI**

Run: `cargo run -p agent-dash -- --help`
Run: `cargo run -p agent-dash -- status`
Run: `cargo run -p agent-dash -- daemon start` (in background, then ctrl-c)
Expected: All subcommands dispatch correctly.

**Step 5: Commit**

```bash
git rm -r crates/agent-dashd crates/agent-dash-hook crates/agentctl
git add Cargo.toml
git commit -m "refactor: remove old crates, single binary is the only entry point"
```

---

### Task 12: Integration testing

**Files:** No new files — manual testing.

**Context:** Verify the full pipeline works end-to-end: daemon auto-start, PTY wrapper, hook events, prompt injection.

**Step 1: Build release binary**

Run: `cargo build --workspace --release`
Expected: Clean build.

**Step 2: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 3: Test daemon start/stop**

```bash
cargo run -p agent-dash --release -- daemon start &
sleep 1
cargo run -p agent-dash --release -- status
# Should show "No active sessions." or similar
kill %1
```

**Step 4: Test hook setup**

```bash
cargo run -p agent-dash --release -- setup hooks
# Should print "Hooks installed successfully." or "Hooks already up to date."
# Verify ~/.claude/settings.json contains agent-dash hook entries.
```

**Step 5: Test PTY wrapper** (requires Claude Code installed)

```bash
cargo run -p agent-dash --release -- run claude
# Should launch Claude Code inside the PTY, look identical to running `claude` directly.
# Type a prompt, verify it works. Ctrl-C to exit.
```

**Step 6: Test prompt injection** (requires Claude Code running in wrapper)

In one terminal:
```bash
cargo run -p agent-dash --release -- run claude
```

In another terminal:
```bash
cargo run -p agent-dash --release -- status
# Note the session_id of the wrapped session
cargo run -p agent-dash --release -- inject <session_id> "what is 2+2?"
# Should see the prompt appear in the Claude Code TUI in the first terminal.
```

**Step 7: Commit integration test results**

If any fixes were needed during integration testing, commit them.
