# Hook-Driven Session Status Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace unreliable JSONL mtime status heuristic with Claude Code hooks reporting session activity via Unix socket, and show tool-specific icons with tooltips in the GNOME extension.

**Architecture:** Daemon gets a socket listener thread that receives hook events and stores them in `Arc<Mutex<HookState>>`. The main loop's `/proc` scan provides session discovery/liveness, hook state provides activity status. A single bash hook script forwards Claude Code lifecycle events to the socket.

**Tech Stack:** Rust std `UnixListener`, serde_json, GNOME `St.Icon`, `socat` for hook→socket IPC

**Design doc:** `docs/plans/2026-02-13-hook-driven-status-design.md`

---

### Task 1: HookEvent and HookState types

**Files:**
- Create: `src/socket.rs`
- Modify: `src/main.rs:1` (add `mod socket;`)

**Step 1: Write the failing test**

In `src/socket.rs`:

```rust
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// An event received from a Claude Code hook via the Unix socket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event")]
pub enum HookEvent {
    #[serde(rename = "tool_start")]
    ToolStart {
        session_id: String,
        tool: String,
        detail: String,
        tool_use_id: String,
    },
    #[serde(rename = "tool_end")]
    ToolEnd {
        session_id: String,
        tool_use_id: String,
    },
    #[serde(rename = "stop")]
    Stop { session_id: String },
    #[serde(rename = "session_start")]
    SessionStart {
        session_id: String,
        #[serde(default)]
        cwd: Option<String>,
    },
    #[serde(rename = "session_end")]
    SessionEnd { session_id: String },
}

/// Per-session state derived from hook events.
#[derive(Debug, Clone)]
pub struct HookSessionData {
    /// The tool currently in use, if any.
    pub active_tool: Option<ActiveToolData>,
    /// true if the last event was `stop` (Claude finished responding).
    pub is_idle: bool,
    /// When the last event was received.
    pub last_event: Instant,
}

/// Info about the currently active tool.
#[derive(Debug, Clone)]
pub struct ActiveToolData {
    pub tool: String,
    pub detail: String,
    pub tool_use_id: String,
}

/// Shared state updated by the socket listener thread, read by the main loop.
pub type HookState = Arc<Mutex<HashMap<String, HookSessionData>>>;

/// Create a new empty HookState.
pub fn new_hook_state() -> HookState {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Apply a HookEvent to the shared state.
pub fn apply_event(state: &HookState, event: HookEvent) {
    let mut map = state.lock().unwrap();
    match event {
        HookEvent::ToolStart {
            session_id,
            tool,
            detail,
            tool_use_id,
        } => {
            let entry = map.entry(session_id).or_insert_with(|| HookSessionData {
                active_tool: None,
                is_idle: false,
                last_event: Instant::now(),
            });
            entry.active_tool = Some(ActiveToolData {
                tool,
                detail,
                tool_use_id,
            });
            entry.is_idle = false;
            entry.last_event = Instant::now();
        }
        HookEvent::ToolEnd {
            session_id,
            tool_use_id,
        } => {
            if let Some(entry) = map.get_mut(&session_id) {
                // Only clear if it matches the current tool
                if entry
                    .active_tool
                    .as_ref()
                    .is_some_and(|t| t.tool_use_id == tool_use_id)
                {
                    entry.active_tool = None;
                }
                entry.last_event = Instant::now();
            }
        }
        HookEvent::Stop { session_id } => {
            let entry = map.entry(session_id).or_insert_with(|| HookSessionData {
                active_tool: None,
                is_idle: true,
                last_event: Instant::now(),
            });
            entry.active_tool = None;
            entry.is_idle = true;
            entry.last_event = Instant::now();
        }
        HookEvent::SessionStart { session_id, .. } => {
            map.insert(
                session_id,
                HookSessionData {
                    active_tool: None,
                    is_idle: false,
                    last_event: Instant::now(),
                },
            );
        }
        HookEvent::SessionEnd { session_id } => {
            map.remove(&session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_tool_start() {
        let json = r#"{"event":"tool_start","session_id":"abc","tool":"Bash","detail":"cargo test","tool_use_id":"t1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::ToolStart { session_id, tool, detail, tool_use_id } => {
                assert_eq!(session_id, "abc");
                assert_eq!(tool, "Bash");
                assert_eq!(detail, "cargo test");
                assert_eq!(tool_use_id, "t1");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_deserialize_tool_end() {
        let json = r#"{"event":"tool_end","session_id":"abc","tool_use_id":"t1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::ToolEnd { .. }));
    }

    #[test]
    fn test_deserialize_stop() {
        let json = r#"{"event":"stop","session_id":"abc"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::Stop { .. }));
    }

    #[test]
    fn test_deserialize_session_start() {
        let json = r#"{"event":"session_start","session_id":"abc","cwd":"/home/user/project"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::SessionStart { .. }));
    }

    #[test]
    fn test_deserialize_session_end() {
        let json = r#"{"event":"session_end","session_id":"abc"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::SessionEnd { .. }));
    }

    #[test]
    fn test_apply_tool_start() {
        let state = new_hook_state();
        apply_event(&state, HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "t1".into(),
        });
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(!data.is_idle);
        assert_eq!(data.active_tool.as_ref().unwrap().tool, "Bash");
    }

    #[test]
    fn test_apply_tool_end_clears_active_tool() {
        let state = new_hook_state();
        apply_event(&state, HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "t1".into(),
        });
        apply_event(&state, HookEvent::ToolEnd {
            session_id: "s1".into(),
            tool_use_id: "t1".into(),
        });
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(data.active_tool.is_none());
    }

    #[test]
    fn test_apply_tool_end_wrong_id_keeps_tool() {
        let state = new_hook_state();
        apply_event(&state, HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "t1".into(),
        });
        apply_event(&state, HookEvent::ToolEnd {
            session_id: "s1".into(),
            tool_use_id: "t_other".into(),
        });
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(data.active_tool.is_some());
    }

    #[test]
    fn test_apply_stop_sets_idle() {
        let state = new_hook_state();
        apply_event(&state, HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "t1".into(),
        });
        apply_event(&state, HookEvent::Stop {
            session_id: "s1".into(),
        });
        let map = state.lock().unwrap();
        let data = map.get("s1").unwrap();
        assert!(data.is_idle);
        assert!(data.active_tool.is_none());
    }

    #[test]
    fn test_apply_session_end_removes() {
        let state = new_hook_state();
        apply_event(&state, HookEvent::SessionStart {
            session_id: "s1".into(),
            cwd: None,
        });
        assert!(state.lock().unwrap().contains_key("s1"));
        apply_event(&state, HookEvent::SessionEnd {
            session_id: "s1".into(),
        });
        assert!(!state.lock().unwrap().contains_key("s1"));
    }

    #[test]
    fn test_tool_start_clears_idle() {
        let state = new_hook_state();
        apply_event(&state, HookEvent::Stop {
            session_id: "s1".into(),
        });
        assert!(state.lock().unwrap().get("s1").unwrap().is_idle);
        apply_event(&state, HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Read".into(),
            detail: "file.rs".into(),
            tool_use_id: "t2".into(),
        });
        assert!(!state.lock().unwrap().get("s1").unwrap().is_idle);
    }
}
```

**Step 2: Add `mod socket;` to main.rs**

Add `mod socket;` after line 3 in `src/main.rs`.

**Step 3: Run tests to verify they pass**

Run: `cargo test --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: All tests pass (including new socket tests)

**Step 4: Commit**

```bash
git add src/socket.rs src/main.rs
git commit -m "feat: add HookEvent and HookState types for socket IPC"
```

---

### Task 2: Unix socket listener

**Files:**
- Modify: `src/socket.rs` (add `start_listener` function)

**Step 1: Write the socket listener function**

Add to `src/socket.rs`, after `apply_event`:

```rust
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

/// Socket path for the daemon.
pub fn socket_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-dash")
        .join("daemon.sock")
}

/// Start the Unix socket listener in a new thread.
/// Returns a JoinHandle. The listener updates `state` on each event.
pub fn start_listener(state: HookState) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let path = socket_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // Remove stale socket file from previous run
        std::fs::remove_file(&path).ok();

        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("agent-dash: failed to bind socket {:?}: {}", path, e);
                return;
            }
        };
        eprintln!("agent-dash: listening on {:?}", path);

        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("agent-dash: socket accept error: {}", e);
                    continue;
                }
            };
            let mut buf = String::new();
            if stream.read_to_string(&mut buf).is_err() {
                continue;
            }
            let buf = buf.trim();
            if buf.is_empty() {
                continue;
            }
            match serde_json::from_str::<HookEvent>(buf) {
                Ok(event) => apply_event(&state, event),
                Err(e) => {
                    eprintln!("agent-dash: invalid hook event: {}", e);
                }
            }
        }
    })
}
```

**Step 2: Write integration test for the socket listener**

Add to the `tests` module in `src/socket.rs`:

```rust
#[test]
fn test_socket_roundtrip() {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    let state = new_hook_state();
    let sock_dir = std::env::temp_dir().join("agent-dash-test-sock");
    std::fs::create_dir_all(&sock_dir).unwrap();
    let sock_path = sock_dir.join("test.sock");
    std::fs::remove_file(&sock_path).ok();

    let listener = UnixListener::bind(&sock_path).unwrap();
    let state_clone = state.clone();
    let handle = std::thread::spawn(move || {
        // Accept one connection
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).unwrap();
        let event: HookEvent = serde_json::from_str(buf.trim()).unwrap();
        apply_event(&state_clone, event);
    });

    // Give listener time to start
    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut stream = UnixStream::connect(&sock_path).unwrap();
    let msg = r#"{"event":"tool_start","session_id":"s1","tool":"Bash","detail":"ls","tool_use_id":"t1"}"#;
    stream.write_all(msg.as_bytes()).unwrap();
    drop(stream); // close so read_to_string returns

    handle.join().unwrap();

    let map = state.lock().unwrap();
    assert!(map.contains_key("s1"));
    assert_eq!(map.get("s1").unwrap().active_tool.as_ref().unwrap().tool, "Bash");

    std::fs::remove_file(&sock_path).ok();
    std::fs::remove_dir_all(&sock_dir).ok();
}
```

**Step 3: Run tests**

Run: `cargo test --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: All tests pass

**Step 4: Commit**

```bash
git add src/socket.rs
git commit -m "feat: add Unix socket listener for hook events"
```

---

### Task 3: ActiveTool type and DashSession update

**Files:**
- Modify: `src/session.rs`

**Step 1: Add ActiveTool struct and update DashSession**

In `src/session.rs`, add after `DashInputReason` (after line 102):

```rust
/// Info about the currently active tool (for the extension to render icons).
#[derive(Debug, Clone, Serialize)]
pub struct DashActiveTool {
    pub name: String,
    pub detail: String,
    pub icon: String,
}
```

Add the `active_tool` field to `DashSession` (after line 86):

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tool: Option<DashActiveTool>,
```

Add the icon mapping function after `DashActiveTool`:

```rust
/// Map a Claude tool name to a GNOME symbolic icon name.
pub fn tool_icon(tool_name: &str) -> &'static str {
    match tool_name {
        "Bash" => "utilities-terminal-symbolic",
        "Read" => "document-open-symbolic",
        "Edit" => "document-edit-symbolic",
        "Write" => "document-new-symbolic",
        "Grep" => "edit-find-symbolic",
        "Glob" => "folder-saved-search-symbolic",
        "WebFetch" => "web-browser-symbolic",
        "WebSearch" => "system-search-symbolic",
        "Task" => "system-run-symbolic",
        _ => "applications-system-symbolic",
    }
}
```

**Step 2: Update `to_dash_session()` to populate `active_tool`**

The `Session` struct needs an `active_tool` field. Add to `Session` (after line 64):

```rust
    /// Active tool info from hook events (if any).
    pub active_tool: Option<(String, String)>, // (tool_name, detail)
```

Update `to_dash_session()` to include it. After line 138 (before the closing `}`):

```rust
            active_tool: self.active_tool.as_ref().map(|(name, detail)| DashActiveTool {
                icon: tool_icon(name).to_string(),
                name: name.clone(),
                detail: detail.clone(),
            }),
```

**Step 3: Add tests**

```rust
#[test]
fn test_tool_icon_mapping() {
    assert_eq!(tool_icon("Bash"), "utilities-terminal-symbolic");
    assert_eq!(tool_icon("Read"), "document-open-symbolic");
    assert_eq!(tool_icon("Edit"), "document-edit-symbolic");
    assert_eq!(tool_icon("UnknownTool"), "applications-system-symbolic");
}

#[test]
fn test_dash_session_with_active_tool() {
    let s = Session {
        session_id: "abc".into(),
        pid: 1,
        pty: PathBuf::from("/dev/pts/0"),
        cwd: PathBuf::from("/home/user/project"),
        project_name: "project".into(),
        branch: "main".into(),
        status: SessionStatus::Working,
        input_reason: None,
        jsonl_path: PathBuf::new(),
        last_jsonl_modified: None,
        last_status_change: 1000,
        last_seen: Instant::now(),
        ended_at: None,
        active_tool: Some(("Bash".into(), "cargo test".into())),
    };
    let ds = s.to_dash_session();
    let json = serde_json::to_string(&ds).unwrap();
    assert!(json.contains("\"icon\":\"utilities-terminal-symbolic\""));
    assert!(json.contains("\"name\":\"Bash\""));
    assert!(json.contains("\"detail\":\"cargo test\""));
}

#[test]
fn test_dash_session_without_active_tool() {
    let s = Session {
        session_id: "abc".into(),
        pid: 1,
        pty: PathBuf::from("/dev/pts/0"),
        cwd: PathBuf::from("/home/user/project"),
        project_name: "project".into(),
        branch: "main".into(),
        status: SessionStatus::Idle,
        input_reason: None,
        jsonl_path: PathBuf::new(),
        last_jsonl_modified: None,
        last_status_change: 1000,
        last_seen: Instant::now(),
        ended_at: None,
        active_tool: None,
    };
    let ds = s.to_dash_session();
    let json = serde_json::to_string(&ds).unwrap();
    assert!(!json.contains("active_tool"));
}
```

**Step 4: Fix existing tests**

The existing `dash_session_serialize` test creates a `Session` and needs the new `active_tool: None` field added.

**Step 5: Run tests**

Run: `cargo test --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: All tests pass

**Step 6: Commit**

```bash
git add src/session.rs
git commit -m "feat: add ActiveTool type and icon mapping to DashSession"
```

---

### Task 4: Integrate hook state into monitor refresh()

**Files:**
- Modify: `src/monitor.rs`

**Step 1: Update `SessionMonitor` to accept `HookState`**

Change the `SessionMonitor` struct to hold a reference to the hook state:

```rust
use crate::socket::HookState;

pub struct SessionMonitor {
    pub sessions: HashMap<String, Session>,
    claude_projects_dir: PathBuf,
    hook_state: HookState,
}
```

Update `new()` to accept `HookState`:

```rust
pub fn new(hook_state: HookState) -> Self {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let claude_projects_dir = home.join(".claude").join("projects");
    Self {
        sessions: HashMap::new(),
        claude_projects_dir,
        hook_state,
    }
}
```

**Step 2: Replace JSONL mtime status with hook-based status**

In `refresh()`, replace the status determination block (lines 278-297). Remove the `recently_modified` check (lines 270-276). Replace with:

```rust
// Determine status from hook state (authoritative for activity)
let hook_map = self.hook_state.lock().unwrap();
let hook_data = hook_map.get(&session_id);

let (status, input_reason, active_tool) = if let Some(perm) = perm_map.get(&session_id) {
    (
        SessionStatus::NeedsInput,
        Some(InputReason::Permission(perm.clone())),
        None,
    )
} else if jsonl_status.has_pending_question {
    (
        SessionStatus::NeedsInput,
        Some(InputReason::Question {
            text: jsonl_status
                .question_text
                .unwrap_or_else(|| "Agent has a question".to_string()),
        }),
        None,
    )
} else if let Some(hd) = hook_data {
    if hd.is_idle {
        (SessionStatus::Idle, None, None)
    } else {
        let tool = hd.active_tool.as_ref().map(|t| (t.tool.clone(), t.detail.clone()));
        (SessionStatus::Working, None, tool)
    }
} else {
    // No hook data yet — default to Working (safe assumption)
    (SessionStatus::Working, None, None)
};
drop(hook_map); // release lock before rest of loop body
```

Then pass `active_tool` into the `Session` struct in the `seen_sessions.insert()` call:

```rust
active_tool,
```

**Step 3: Remove the grace period block**

Remove lines 331-350 (the "Handle sessions not found in this scan" block with the 10-second grace period). Replace with simple ended handling:

```rust
// Handle sessions not found in this scan — mark as ended immediately
for (sid, existing) in &self.sessions {
    if seen_sessions.contains_key(sid) {
        continue;
    }
    if existing.status != SessionStatus::Ended {
        let mut ended = existing.clone();
        ended.status = SessionStatus::Ended;
        ended.ended_at = Some(existing.ended_at.unwrap_or_else(Instant::now));
        seen_sessions.insert(sid.clone(), ended);
    } else {
        // Already ended, keep for fade-out
        seen_sessions.insert(sid.clone(), existing.clone());
    }
}
```

**Step 4: Run tests**

Run: `cargo test --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: All tests pass (note: `main.rs` will need updating in Task 5 before compilation succeeds — if compiler errors occur, proceed to Task 5 first)

**Step 5: Commit**

```bash
git add src/monitor.rs
git commit -m "feat: use hook state for session status instead of JSONL mtime"
```

---

### Task 5: Wire up socket listener in main.rs

**Files:**
- Modify: `src/main.rs`

**Step 1: Update main() to start socket listener and pass hook state**

Replace the contents of `main()`:

```rust
fn main() {
    eprintln!("agent-dashd starting, writing to {:?}", state_file_path());

    let hook_state = socket::new_hook_state();
    let _listener_handle = socket::start_listener(hook_state.clone());

    let mut monitor = SessionMonitor::new(hook_state);

    loop {
        monitor.refresh();
        let sessions: Vec<_> = monitor
            .sessions()
            .map(|s| s.to_dash_session())
            .collect();
        let state = DashState { sessions };
        if let Err(e) = write_state(&state) {
            eprintln!("error writing state: {}", e);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
```

**Step 2: Build to verify compilation**

Run: `cargo build --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: Compiles successfully (warnings about unused JSONL functions are OK)

**Step 3: Run all tests**

Run: `cargo test --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: All tests pass

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire up socket listener in main loop"
```

---

### Task 6: Hook script

**Files:**
- Create: `hooks/agent-dash-hook.sh`

**Step 1: Write the hook script**

```bash
#!/usr/bin/env bash
# agent-dash-hook.sh — Forwards Claude Code hook events to the agent-dash daemon.
# Usage: agent-dash-hook.sh <event_type>
# Reads hook context JSON from stdin, extracts relevant fields, sends to daemon socket.

set -euo pipefail

SOCK="${XDG_CACHE_HOME:-$HOME/.cache}/agent-dash/daemon.sock"

# Bail fast if daemon isn't running
[ -S "$SOCK" ] || exit 0

INPUT=$(cat)
EVENT="${1:-unknown}"
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')

[ -n "$SESSION_ID" ] || exit 0

case "$EVENT" in
  tool_start)
    TOOL=$(echo "$INPUT" | jq -r '.tool_name // empty')
    TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty')
    case "$TOOL" in
      Bash)       DETAIL=$(echo "$INPUT" | jq -r '.tool_input.command // empty' | head -c 200) ;;
      Read)       DETAIL=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty') ;;
      Edit)       DETAIL=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty') ;;
      Write)      DETAIL=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty') ;;
      Grep)       DETAIL=$(echo "$INPUT" | jq -r '.tool_input.pattern // empty') ;;
      Glob)       DETAIL=$(echo "$INPUT" | jq -r '.tool_input.pattern // empty') ;;
      WebFetch)   DETAIL=$(echo "$INPUT" | jq -r '.tool_input.url // empty') ;;
      WebSearch)  DETAIL=$(echo "$INPUT" | jq -r '.tool_input.query // empty') ;;
      Task)       DETAIL=$(echo "$INPUT" | jq -r '.tool_input.description // empty') ;;
      *)          DETAIL="$TOOL" ;;
    esac
    MSG=$(jq -nc --arg e "$EVENT" --arg s "$SESSION_ID" --arg t "$TOOL" \
      --arg d "$DETAIL" --arg tid "$TOOL_USE_ID" \
      '{event:$e, session_id:$s, tool:$t, detail:$d, tool_use_id:$tid}')
    ;;
  tool_end)
    TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty')
    MSG=$(jq -nc --arg e "$EVENT" --arg s "$SESSION_ID" --arg tid "$TOOL_USE_ID" \
      '{event:$e, session_id:$s, tool_use_id:$tid}')
    ;;
  *)
    # stop, session_start, session_end
    MSG=$(jq -nc --arg e "$EVENT" --arg s "$SESSION_ID" \
      '{event:$e, session_id:$s}')
    ;;
esac

# Fire-and-forget to daemon socket
echo "$MSG" | socat -t0 - UNIX-CONNECT:"$SOCK" 2>/dev/null || true
```

**Step 2: Make it executable**

```bash
chmod +x hooks/agent-dash-hook.sh
```

**Step 3: Verify it exits cleanly when no daemon is running**

Run: `echo '{}' | /home/mfeinber/src/rust/agent-dash/hooks/agent-dash-hook.sh stop`
Expected: Exits with code 0, no output (socket doesn't exist so it bails)

**Step 4: Commit**

```bash
git add hooks/agent-dash-hook.sh
git commit -m "feat: add hook script for forwarding Claude events to daemon"
```

---

### Task 7: Update Claude Code settings

**Files:**
- Modify: `~/.claude/settings.json`

**Step 1: Add hook entries alongside existing PermissionRequest hook**

The `hooks` section should become:

```json
{
  "enabledPlugins": {
    "superpowers@superpowers-marketplace": true
  },
  "hooks": {
    "PermissionRequest": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "/home/mfeinber/src/rust/agent-dash/hooks/permission-bridge.sh"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/mfeinber/src/rust/agent-dash/hooks/agent-dash-hook.sh tool_start"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/mfeinber/src/rust/agent-dash/hooks/agent-dash-hook.sh tool_end"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/mfeinber/src/rust/agent-dash/hooks/agent-dash-hook.sh stop"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/mfeinber/src/rust/agent-dash/hooks/agent-dash-hook.sh session_start"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/mfeinber/src/rust/agent-dash/hooks/agent-dash-hook.sh session_end"
          }
        ]
      }
    ]
  }
}
```

**Step 2: Verify settings parse correctly**

Run: `jq . ~/.claude/settings.json`
Expected: Valid JSON output with all hook entries

**Step 3: No commit** (settings.json is outside the repo)

---

### Task 8: Extension — tool icons and tooltips

**Files:**
- Modify: `extension/extension.js`

**Step 1: Update `_addSessionPill()` to render tool icons**

Replace the dots/emoji section (lines 203-219) with logic that checks for `active_tool`:

```javascript
_addSessionPill(session) {
    const pill = new St.BoxLayout({
        vertical: true,
        style_class: 'agent-dash-pill',
    });

    // Header row: status indicator + label
    const headerRow = new St.BoxLayout({vertical: false});

    const styleClasses = {
        needs_input: 'agent-dash-label-red',
        working: 'agent-dash-label-yellow',
        idle: 'agent-dash-label-green',
        ended: 'agent-dash-label-grey',
    };
    const styleClass = styleClasses[session.status] || 'agent-dash-label-grey';

    // Status indicator: tool icon or colored dot
    let indicator;
    if (session.active_tool && session.status === 'working') {
        indicator = new St.Icon({
            icon_name: session.active_tool.icon,
            icon_size: 14,
            style_class: 'agent-dash-tool-icon',
            reactive: true,
        });
        // Tooltip on hover
        const tooltip = new St.Label({
            text: session.active_tool.detail
                ? session.active_tool.detail.substring(0, 80)
                : session.active_tool.name,
            style_class: 'agent-dash-tooltip',
            visible: false,
        });
        indicator.connect('enter-event', () => { tooltip.visible = true; });
        indicator.connect('leave-event', () => { tooltip.visible = false; });
        headerRow.add_child(indicator);
        headerRow.add_child(tooltip);
    } else {
        const dots = {
            needs_input: '\u{1F534}',
            working: '\u{1F7E1}',
            idle: '\u{1F7E2}',
            ended: '\u{26AA}',
        };
        const dot = dots[session.status] || '\u{26AA}';
        const dotLabel = new St.Label({text: dot + ' ', style_class: styleClass});
        headerRow.add_child(dotLabel);
    }

    // Project label
    const branch = (!session.branch || session.branch === 'main')
        ? '' : ` (${session.branch})`;
    const labelText = session.active_tool && session.status === 'working'
        ? ` ${session.project_name}${branch}`
        : `${session.project_name}${branch}`;

    const labelBtn = new St.Button({
        style_class: styleClass,
        reactive: true,
        x_expand: true,
    });
    labelBtn.set_child(new St.Label({text: labelText}));
    headerRow.add_child(labelBtn);
    pill.add_child(headerRow);

    // ... rest of expand/collapse logic stays the same ...
```

Keep the existing expand/collapse and permission button logic, just update the variable references (the `labelBtn` click handler and expanded detail section remain unchanged, referencing `session.session_id` and `session.input_reason`).

**Step 2: No automated tests** — GNOME extensions are tested manually

**Step 3: Commit**

```bash
git add extension/extension.js
git commit -m "feat: show tool icons with tooltips in extension"
```

---

### Task 9: Extension — CSS pulse animation and tooltip style

**Files:**
- Modify: `extension/stylesheet.css`

**Step 1: Add tool icon and tooltip styles**

Append to `extension/stylesheet.css`:

```css
.agent-dash-tool-icon {
    color: #ffc832;
    margin-right: 4px;
    margin-top: 2px;
}

.agent-dash-tooltip {
    background-color: rgba(20, 20, 20, 0.95);
    color: #e0e0e0;
    font-size: 9pt;
    padding: 4px 8px;
    border-radius: 4px;
    margin-left: 4px;
}
```

**Step 2: Commit**

```bash
git add extension/stylesheet.css
git commit -m "feat: add CSS for tool icons and tooltips"
```

---

### Task 10: Remove dead JSONL mtime code

**Files:**
- Modify: `src/monitor.rs`

**Step 1: Remove unused code**

Remove the following from `src/monitor.rs`:
- `read_tail_lines()` function (lines 127-147)
- `parse_jsonl_status()` function (lines 149-225) — **WAIT: still needed for session_id and branch extraction**. Keep it for now.
- Remove the `last_modified` / `recently_modified` variables if not already removed in Task 4
- Remove the `last_jsonl_modified` field usage if no longer needed

Actually, `parse_jsonl_status()` is still used for `session_id` and `git_branch` extraction in `refresh()`. Keep it. Only remove the mtime-based status check (already done in Task 4).

Remove from `Session` struct in `session.rs`:
- `last_jsonl_modified` field (line 58) — if no longer read anywhere

**Step 2: Run tests**

Run: `cargo test --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: All tests pass, no warnings about unused functions

**Step 3: Commit**

```bash
git add src/monitor.rs src/session.rs
git commit -m "refactor: remove unused JSONL mtime and grace period code"
```

---

### Task 11: Build, deploy, and end-to-end test

**Files:** None (testing only)

**Step 1: Build release binary**

Run: `cargo build --release --manifest-path /home/mfeinber/src/rust/agent-dash/Cargo.toml`
Expected: Compiles with no errors

**Step 2: Verify `socat` is installed**

Run: `which socat || sudo dnf install -y socat`
Expected: socat is available

**Step 3: Kill existing daemon and start new one**

```bash
kill $(pgrep -f "target/release/agent-dash") 2>/dev/null; sleep 1
/home/mfeinber/src/rust/agent-dash/target/release/agent-dash &
```

**Step 4: Verify socket was created**

Run: `ls -la ~/.cache/agent-dash/daemon.sock`
Expected: Socket file exists (type `s`)

**Step 5: Send a test event manually**

```bash
echo '{"event":"tool_start","session_id":"test-123","tool":"Bash","detail":"echo hello","tool_use_id":"t1"}' | socat -t0 - UNIX-CONNECT:$HOME/.cache/agent-dash/daemon.sock
```
Expected: No error. Daemon stderr shows no parse errors.

**Step 6: Verify state.json includes active_tool when a real session is running**

```bash
sleep 2 && cat ~/.cache/agent-dash/state.json | jq .
```
Expected: Sessions shown with `active_tool` field when working

**Step 7: Reload GNOME extension**

Press Alt+F2, type `r`, press Enter (or log out/in on Wayland).
Expected: Extension loads without errors, tool icons appear for working sessions.

**Step 8: No commit** — this is verification only
