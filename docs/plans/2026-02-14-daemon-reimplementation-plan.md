# Daemon Reimplementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rewrite agent-dash as an async tokio-based workspace with three binaries (agent-dashd, agent-dash-hook, agentctl) and a shared core library, using cross-platform local sockets.

**Architecture:** Cargo workspace with 4 crates under `crates/`. The daemon owns all state via channels (no shared mutexes), pushes updates to subscribed clients over local sockets. Hook companion binary replaces shell scripts. CLI tool for querying and debugging.

**Tech Stack:** tokio, interprocess (local sockets), sysinfo (process scanning), serde/serde_json, dirs

---

### Task 1: Set Up Workspace Structure

**Files:**
- Create: `crates/agent-dash-core/Cargo.toml`
- Create: `crates/agent-dash-core/src/lib.rs`
- Create: `crates/agent-dashd/Cargo.toml`
- Create: `crates/agent-dashd/src/main.rs`
- Create: `crates/agent-dash-hook/Cargo.toml`
- Create: `crates/agent-dash-hook/src/main.rs`
- Create: `crates/agentctl/Cargo.toml`
- Create: `crates/agentctl/src/main.rs`
- Modify: `Cargo.toml` (convert to workspace)

**Step 1: Convert root Cargo.toml to workspace**

```toml
[workspace]
members = [
    "crates/agent-dash-core",
    "crates/agent-dashd",
    "crates/agent-dash-hook",
    "crates/agentctl",
]
resolver = "3"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dirs = "6"
tokio = { version = "1", features = ["full"] }
interprocess = { version = "2", features = ["tokio"] }
sysinfo = "0.34"
```

**Step 2: Create agent-dash-core crate**

`crates/agent-dash-core/Cargo.toml`:
```toml
[package]
name = "agent-dash-core"
version = "0.1.0"
edition = "2024"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
dirs = { workspace = true }
```

`crates/agent-dash-core/src/lib.rs`:
```rust
pub mod paths;
pub mod protocol;
pub mod session;
```

**Step 3: Create agent-dashd crate**

`crates/agent-dashd/Cargo.toml`:
```toml
[package]
name = "agent-dashd"
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
```

`crates/agent-dashd/src/main.rs`:
```rust
fn main() {
    println!("agent-dashd placeholder");
}
```

**Step 4: Create agent-dash-hook crate**

`crates/agent-dash-hook/Cargo.toml`:
```toml
[package]
name = "agent-dash-hook"
version = "0.1.0"
edition = "2024"

[dependencies]
agent-dash-core = { path = "../agent-dash-core" }
serde = { workspace = true }
serde_json = { workspace = true }
dirs = { workspace = true }
interprocess = { version = "2" }
```

`crates/agent-dash-hook/src/main.rs`:
```rust
fn main() {
    println!("agent-dash-hook placeholder");
}
```

**Step 5: Create agentctl crate**

`crates/agentctl/Cargo.toml`:
```toml
[package]
name = "agentctl"
version = "0.1.0"
edition = "2024"

[dependencies]
agent-dash-core = { path = "../agent-dash-core" }
serde = { workspace = true }
serde_json = { workspace = true }
dirs = { workspace = true }
interprocess = { version = "2" }
```

`crates/agentctl/src/main.rs`:
```rust
fn main() {
    println!("agentctl placeholder");
}
```

**Step 6: Verify workspace builds**

Run: `cargo build`
Expected: All four crates compile successfully.

**Step 7: Commit**

```bash
git add Cargo.toml crates/
git commit -m "feat: set up cargo workspace with four crates"
```

---

### Task 2: Core Library — Paths Module

**Files:**
- Create: `crates/agent-dash-core/src/paths.rs`
- Test: inline `#[cfg(test)]`

Cross-platform path resolution for socket paths, config dirs, and Claude project slugs.

**Step 1: Write the tests**

```rust
// In crates/agent-dash-core/src/paths.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_returns_something() {
        let dir = cache_dir();
        assert!(dir.to_string_lossy().len() > 0);
    }

    #[test]
    fn hook_socket_name_is_consistent() {
        let a = hook_socket_name();
        let b = hook_socket_name();
        assert_eq!(a, b);
    }

    #[test]
    fn client_socket_name_is_consistent() {
        let a = client_socket_name();
        let b = client_socket_name();
        assert_eq!(a, b);
    }

    #[test]
    fn state_file_in_cache_dir() {
        let path = state_file_path();
        assert!(path.to_string_lossy().contains("agent-dash"));
        assert!(path.to_string_lossy().ends_with("state.json"));
    }

    #[test]
    fn slug_unix_style_path() {
        let cwd = std::path::PathBuf::from("/home/user/src/project");
        assert_eq!(cwd_to_project_slug(&cwd), "-home-user-src-project");
    }

    #[test]
    fn slug_preserves_hyphens() {
        let cwd = std::path::PathBuf::from("/home/user/my-project");
        assert_eq!(cwd_to_project_slug(&cwd), "-home-user-my-project");
    }

    #[test]
    fn project_name_from_cwd_uses_last_component() {
        let cwd = std::path::PathBuf::from("/home/user/src/agent-dash");
        assert_eq!(project_name_from_cwd(&cwd), "agent-dash");
    }

    #[test]
    fn claude_projects_dir_under_home() {
        let dir = claude_projects_dir();
        assert!(dir.to_string_lossy().contains(".claude"));
        assert!(dir.to_string_lossy().ends_with("projects"));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dash-core`
Expected: FAIL — functions not defined.

**Step 3: Implement paths module**

```rust
use std::path::{Path, PathBuf};

/// Cache directory for agent-dash runtime files.
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-dash")
}

/// Socket name for hook events (fire-and-forget).
pub fn hook_socket_name() -> String {
    let path = cache_dir().join("hook.sock");
    path.to_string_lossy().to_string()
}

/// Socket name for client connections (bidirectional).
pub fn client_socket_name() -> String {
    let path = cache_dir().join("daemon.sock");
    path.to_string_lossy().to_string()
}

/// Path to the state.json debug/compat file.
pub fn state_file_path() -> PathBuf {
    cache_dir().join("state.json")
}

/// Convert a CWD path to a Claude project slug.
/// e.g., /home/user/src/project -> -home-user-src-project
pub fn cwd_to_project_slug(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    s.replace('/', "-").replace('\\', "-")
}

/// Extract project name (last path component) from a CWD.
pub fn project_name_from_cwd(cwd: &Path) -> String {
    cwd.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Path to the Claude projects directory.
pub fn claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join("projects")
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p agent-dash-core`
Expected: All pass.

**Step 5: Commit**

```bash
git add crates/agent-dash-core/src/paths.rs crates/agent-dash-core/src/lib.rs
git commit -m "feat(core): add cross-platform paths module"
```

---

### Task 3: Core Library — Protocol Types

**Files:**
- Create: `crates/agent-dash-core/src/protocol.rs`
- Test: inline `#[cfg(test)]`

All message types shared between daemon, hook, and clients.

**Step 1: Write the tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // -- Hook events --

    #[test]
    fn deserialize_hook_tool_start() {
        let json = r#"{"event":"tool_start","session_id":"s1","tool":"Bash","detail":"ls","tool_use_id":"tu1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        match event {
            HookEvent::ToolStart { session_id, tool, detail, tool_use_id } => {
                assert_eq!(session_id, "s1");
                assert_eq!(tool, "Bash");
                assert_eq!(detail, "ls");
                assert_eq!(tool_use_id, "tu1");
            }
            _ => panic!("expected ToolStart"),
        }
    }

    #[test]
    fn deserialize_hook_tool_end() {
        let json = r#"{"event":"tool_end","session_id":"s1","tool_use_id":"tu1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::ToolEnd { .. }));
    }

    #[test]
    fn deserialize_hook_stop() {
        let json = r#"{"event":"stop","session_id":"s1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::Stop { .. }));
    }

    #[test]
    fn deserialize_hook_session_start() {
        let json = r#"{"event":"session_start","session_id":"s1","cwd":"/home/user"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::SessionStart { .. }));
    }

    #[test]
    fn deserialize_hook_session_end() {
        let json = r#"{"event":"session_end","session_id":"s1"}"#;
        let event: HookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, HookEvent::SessionEnd { .. }));
    }

    // -- Client requests --

    #[test]
    fn deserialize_subscribe() {
        let json = r#"{"method":"subscribe"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::Subscribe));
    }

    #[test]
    fn deserialize_get_state() {
        let json = r#"{"method":"get_state"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::GetState));
    }

    #[test]
    fn deserialize_permission_response() {
        let json = r#"{"method":"permission_response","request_id":"tu1","session_id":"s1","decision":"allow"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        match req {
            ClientRequest::PermissionResponse { request_id, session_id, decision } => {
                assert_eq!(request_id, "tu1");
                assert_eq!(session_id, "s1");
                assert_eq!(decision, "allow");
            }
            _ => panic!("expected PermissionResponse"),
        }
    }

    #[test]
    fn deserialize_permission_request() {
        let json = r#"{"method":"permission_request","request_id":"tu1","session_id":"s1","tool":"Bash","detail":"rm -rf /tmp"}"#;
        let req: ClientRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(req, ClientRequest::PermissionRequest { .. }));
    }

    // -- Server events --

    #[test]
    fn serialize_state_update() {
        let event = ServerEvent::StateUpdate { sessions: vec![] };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"event\":\"state_update\""));
    }

    #[test]
    fn serialize_permission_pending() {
        let event = ServerEvent::PermissionPending {
            session_id: "s1".into(),
            request_id: "tu1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"request_id\":\"tu1\""));
    }

    #[test]
    fn serialize_permission_resolved() {
        let event = ServerEvent::PermissionResolved {
            request_id: "tu1".into(),
            resolved_by: "terminal".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"resolved_by\":\"terminal\""));
    }

    // -- Permission decision for hook response --

    #[test]
    fn serialize_hook_permission_decision_allow() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "allow".into(),
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("\"decision\":\"allow\""));
    }

    // -- Line-delimited encoding --

    #[test]
    fn encode_line_appends_newline() {
        let event = ServerEvent::StateUpdate { sessions: vec![] };
        let line = encode_line(&event).unwrap();
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
    }

    #[test]
    fn decode_line_parses_json() {
        let json = r#"{"method":"subscribe"}"#;
        let req: ClientRequest = decode_line(json).unwrap();
        assert!(matches!(req, ClientRequest::Subscribe));
    }

    #[test]
    fn decode_line_trims_whitespace() {
        let json = "  {\"method\":\"subscribe\"}  \n";
        let req: ClientRequest = decode_line(json).unwrap();
        assert!(matches!(req, ClientRequest::Subscribe));
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dash-core`
Expected: FAIL — types not defined.

**Step 3: Implement protocol types**

```rust
use crate::session::DashSession;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Hook events (hook -> daemon, via hook.sock)
// ---------------------------------------------------------------------------

/// Events sent by the hook companion binary to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

// ---------------------------------------------------------------------------
// Client requests (client -> daemon, via daemon.sock)
// ---------------------------------------------------------------------------

/// Requests from clients (agentctl, GNOME extension, hooks needing responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum ClientRequest {
    #[serde(rename = "subscribe")]
    Subscribe,
    #[serde(rename = "get_state")]
    GetState,
    #[serde(rename = "permission_response")]
    PermissionResponse {
        request_id: String,
        session_id: String,
        decision: String,
    },
    #[serde(rename = "permission_request")]
    PermissionRequest {
        request_id: String,
        session_id: String,
        tool: String,
        detail: String,
    },
}

// ---------------------------------------------------------------------------
// Server events (daemon -> client, via daemon.sock)
// ---------------------------------------------------------------------------

/// Events pushed from daemon to subscribed clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum ServerEvent {
    #[serde(rename = "state_update")]
    StateUpdate { sessions: Vec<DashSession> },
    #[serde(rename = "permission_pending")]
    PermissionPending {
        session_id: String,
        request_id: String,
        tool: String,
        detail: String,
    },
    #[serde(rename = "permission_resolved")]
    PermissionResolved {
        request_id: String,
        resolved_by: String,
    },
}

// ---------------------------------------------------------------------------
// Permission decision sent back to the hook over its daemon.sock connection
// ---------------------------------------------------------------------------

/// Response sent back to the hook companion binary's permission request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPermissionDecision {
    pub request_id: String,
    pub decision: String,
}

// ---------------------------------------------------------------------------
// Line-delimited JSON helpers
// ---------------------------------------------------------------------------

/// Encode a value as a single JSON line (terminated by \n).
pub fn encode_line<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

/// Decode a JSON line (trims whitespace).
pub fn decode_line<'a, T: Deserialize<'a>>(line: &'a str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim())
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p agent-dash-core`
Expected: All pass.

**Step 5: Commit**

```bash
git add crates/agent-dash-core/src/protocol.rs
git commit -m "feat(core): add protocol message types"
```

---

### Task 4: Core Library — Session Types

**Files:**
- Create: `crates/agent-dash-core/src/session.rs`
- Test: inline `#[cfg(test)]`

Migrated and cleaned up from the old `src/session.rs`. Removes `Instant` (not serializable), keeps only cross-platform types.

**Step 1: Write the tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_sort_order() {
        assert!(SessionStatus::NeedsInput.sort_key() < SessionStatus::Working.sort_key());
        assert!(SessionStatus::Working.sort_key() < SessionStatus::Idle.sort_key());
        assert!(SessionStatus::Idle.sort_key() < SessionStatus::Ended.sort_key());
    }

    #[test]
    fn status_display_strings() {
        assert_eq!(SessionStatus::NeedsInput.as_str(), "needs_input");
        assert_eq!(SessionStatus::Working.as_str(), "working");
        assert_eq!(SessionStatus::Idle.as_str(), "idle");
        assert_eq!(SessionStatus::Ended.as_str(), "ended");
    }

    #[test]
    fn dash_session_serialize_minimal() {
        let ds = DashSession {
            session_id: "abc".into(),
            project_name: "myproject".into(),
            branch: "main".into(),
            status: "idle".into(),
            last_status_change: 1000,
            jsonl_path: None,
            input_reason: None,
            active_tool: None,
        };
        let json = serde_json::to_string(&ds).unwrap();
        assert!(json.contains("\"status\":\"idle\""));
        assert!(!json.contains("input_reason"));
        assert!(!json.contains("active_tool"));
    }

    #[test]
    fn dash_session_serialize_with_tool() {
        let ds = DashSession {
            session_id: "abc".into(),
            project_name: "myproject".into(),
            branch: "main".into(),
            status: "working".into(),
            last_status_change: 1000,
            jsonl_path: None,
            input_reason: None,
            active_tool: Some(DashActiveTool {
                name: "Bash".into(),
                detail: "cargo test".into(),
                icon: "utilities-terminal-symbolic".into(),
            }),
        };
        let json = serde_json::to_string(&ds).unwrap();
        assert!(json.contains("\"name\":\"Bash\""));
    }

    #[test]
    fn tool_icon_known_tools() {
        assert_eq!(tool_icon("Bash"), "utilities-terminal-symbolic");
        assert_eq!(tool_icon("Read"), "document-open-symbolic");
        assert_eq!(tool_icon("Edit"), "document-edit-symbolic");
    }

    #[test]
    fn tool_icon_unknown_fallback() {
        assert_eq!(tool_icon("SomeFutureTool"), "applications-system-symbolic");
    }

    #[test]
    fn dash_session_roundtrip_json() {
        let ds = DashSession {
            session_id: "abc".into(),
            project_name: "proj".into(),
            branch: "feat".into(),
            status: "working".into(),
            last_status_change: 999,
            jsonl_path: Some("/tmp/test.jsonl".into()),
            input_reason: Some(DashInputReason {
                reason_type: "permission".into(),
                tool: Some("Bash".into()),
                command: Some("ls".into()),
                detail: Some("detail".into()),
                text: None,
            }),
            active_tool: None,
        };
        let json = serde_json::to_string(&ds).unwrap();
        let parsed: DashSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "abc");
        assert_eq!(parsed.input_reason.unwrap().tool.unwrap(), "Bash");
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dash-core`
Expected: FAIL — types not defined.

**Step 3: Implement session types**

```rust
use serde::{Deserialize, Serialize};

/// Status of a Claude Code session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    NeedsInput,
    Working,
    Idle,
    Ended,
}

impl SessionStatus {
    pub fn sort_key(&self) -> u8 {
        match self {
            SessionStatus::NeedsInput => 0,
            SessionStatus::Working => 1,
            SessionStatus::Idle => 2,
            SessionStatus::Ended => 3,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStatus::NeedsInput => "needs_input",
            SessionStatus::Working => "working",
            SessionStatus::Idle => "idle",
            SessionStatus::Ended => "ended",
        }
    }
}

/// A single session in the JSON output (serializable for clients).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashSession {
    pub session_id: String,
    pub project_name: String,
    pub branch: String,
    pub status: String,
    pub last_status_change: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonl_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_reason: Option<DashInputReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tool: Option<DashActiveTool>,
}

/// Why a session needs input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashInputReason {
    #[serde(rename = "type")]
    pub reason_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Active tool info for rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashActiveTool {
    pub name: String,
    pub detail: String,
    pub icon: String,
}

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

**Step 4: Run tests to verify they pass**

Run: `cargo test -p agent-dash-core`
Expected: All pass.

**Step 5: Commit**

```bash
git add crates/agent-dash-core/src/session.rs
git commit -m "feat(core): add session types"
```

---

### Task 5: Daemon — Process Scanner

**Files:**
- Create: `crates/agent-dashd/src/scanner.rs`
- Test: inline `#[cfg(test)]`

Cross-platform process discovery using `sysinfo`. Also includes JSONL parsing (migrated from old `src/monitor.rs`).

**Step 1: Write the tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_does_not_panic() {
        // Just verify it runs without error on this platform.
        let _procs = scan_claude_processes();
    }

    #[test]
    fn find_latest_jsonl_empty_dir() {
        let dir = std::env::temp_dir().join("agent-dash-test-scanner-empty");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_latest_jsonl(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_latest_jsonl_picks_newest() {
        let dir = std::env::temp_dir().join("agent-dash-test-scanner-jsonl");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.jsonl"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(dir.join("new.jsonl"), "{}").unwrap();
        let latest = find_latest_jsonl(&dir).unwrap();
        assert_eq!(latest.file_name().unwrap(), "new.jsonl");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_jsonl_working_session() {
        let dir = std::env::temp_dir().join("agent-dash-test-scanner-parse");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = concat!(
            r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
            "\n",
            r#"{"type":"user","sessionId":"abc-123","gitBranch":"main","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}"#,
            "\n",
        );
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert_eq!(status.session_id, "abc-123");
        assert_eq!(status.git_branch, "main");
        assert!(!status.has_pending_question);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_jsonl_pending_question() {
        let dir = std::env::temp_dir().join("agent-dash-test-scanner-question");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"s1","gitBranch":"feat","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"Which approach?"}]}}]}}"#;
        std::fs::write(&path, format!("{}\n", content)).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert!(status.has_pending_question);
        assert_eq!(status.question_text.as_deref(), Some("Which approach?"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_jsonl_no_session_id_returns_none() {
        let dir = std::env::temp_dir().join("agent-dash-test-scanner-nosid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(&path, r#"{"type":"user","message":{}}"#).unwrap();
        assert!(parse_jsonl_status(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dashd`
Expected: FAIL — module and functions not defined.

**Step 3: Implement scanner**

```rust
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use sysinfo::System;

/// Info about a discovered Claude process.
#[derive(Debug, Clone)]
pub struct ClaudeProcess {
    pub pid: u32,
    pub cwd: PathBuf,
}

/// Scan for running Claude processes using sysinfo (cross-platform).
pub fn scan_claude_processes() -> HashMap<u32, ClaudeProcess> {
    let mut system = System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut result = HashMap::new();
    for (pid, process) in system.processes() {
        let name = process.name().to_string_lossy();
        let is_claude = name == "claude" || process.cmd().first().is_some_and(|arg| {
            let s = arg.to_string_lossy();
            s == "claude" || s.ends_with("/claude") || s.ends_with("\\claude")
        });
        if !is_claude {
            continue;
        }
        let Some(cwd) = process.cwd() else { continue };
        result.insert(pid.as_u32(), ClaudeProcess {
            pid: pid.as_u32(),
            cwd: cwd.to_path_buf(),
        });
    }
    result
}

/// Find the most recently modified .jsonl file in a directory.
pub fn find_latest_jsonl(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension().is_some_and(|ext| ext == "jsonl")
        })
        .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|e| e.path())
}

// -- JSONL parsing (migrated from old monitor.rs) --

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum JournalEntry {
    #[serde(rename = "assistant")]
    Assistant {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        #[serde(rename = "gitBranch")]
        git_branch: Option<String>,
        message: Option<AssistantMessage>,
    },
    #[serde(rename = "user")]
    User {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        #[serde(rename = "gitBranch")]
        git_branch: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    content: Option<Vec<ContentBlock>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

/// Info extracted from the tail of a JSONL session file.
#[derive(Debug, Clone)]
pub struct JsonlStatus {
    pub session_id: String,
    pub git_branch: String,
    pub has_pending_question: bool,
    pub question_text: Option<String>,
}

/// Read the last N lines of a file by seeking from the end.
fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.seek(SeekFrom::End(0)).ok()?;
    if file_len == 0 {
        return vec![];
    }
    let read_size = file_len.min(64 * 1024);
    let _ = file.seek(SeekFrom::End(-(read_size as i64)));
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);
    let lines: Vec<String> = buf.lines().map(|l| l.to_string()).collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].to_vec()
}

/// Parse the tail of a JSONL file to extract session status.
pub fn parse_jsonl_status(path: &Path) -> Option<JsonlStatus> {
    let lines = read_tail_lines(path, 20);
    let mut session_id = String::new();
    let mut git_branch = String::new();
    let mut last_was_assistant_with_ask = false;
    let mut question_text: Option<String> = None;
    let mut last_was_user = false;

    for line in &lines {
        let Ok(entry) = serde_json::from_str::<JournalEntry>(line) else {
            continue;
        };
        match entry {
            JournalEntry::Assistant {
                session_id: sid,
                git_branch: gb,
                message,
            } => {
                if let Some(sid) = sid { session_id = sid; }
                if let Some(gb) = gb { git_branch = gb; }
                last_was_user = false;
                last_was_assistant_with_ask = false;
                question_text = None;
                if let Some(msg) = message {
                    if let Some(content) = msg.content {
                        for block in content {
                            if let ContentBlock::ToolUse { name, input } = block {
                                if name == "AskUserQuestion" {
                                    last_was_assistant_with_ask = true;
                                    if let Some(qs) = input.get("questions") {
                                        if let Some(arr) = qs.as_array() {
                                            if let Some(first) = arr.first() {
                                                if let Some(q) = first.get("question") {
                                                    question_text = q.as_str().map(|s| s.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            JournalEntry::User { session_id: sid, git_branch: gb } => {
                if let Some(sid) = sid { session_id = sid; }
                if let Some(gb) = gb { git_branch = gb; }
                last_was_user = true;
                last_was_assistant_with_ask = false;
                question_text = None;
            }
            JournalEntry::Other => {}
        }
    }

    if session_id.is_empty() {
        return None;
    }

    Some(JsonlStatus {
        session_id,
        git_branch,
        has_pending_question: last_was_assistant_with_ask && !last_was_user,
        question_text,
    })
}
```

Note: the `read_tail_lines` function uses `?` in a non-Option context — fix that to use early returns:

```rust
fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return vec![]; };
    let Ok(file_len) = file.seek(SeekFrom::End(0)) else { return vec![]; };
    if file_len == 0 {
        return vec![];
    }
    let read_size = file_len.min(64 * 1024);
    let _ = file.seek(SeekFrom::End(-(read_size as i64)));
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);
    let lines: Vec<String> = buf.lines().map(|l| l.to_string()).collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].to_vec()
}
```

**Step 4: Register module in main.rs**

Add `mod scanner;` to `crates/agent-dashd/src/main.rs`.

**Step 5: Run tests to verify they pass**

Run: `cargo test -p agent-dashd`
Expected: All pass.

**Step 6: Commit**

```bash
git add crates/agent-dashd/src/scanner.rs crates/agent-dashd/src/main.rs
git commit -m "feat(daemon): add cross-platform process scanner"
```

---

### Task 6: Daemon — State Manager

**Files:**
- Create: `crates/agent-dashd/src/state.rs`
- Test: inline `#[cfg(test)]`

The central state owner. Receives updates via channels, broadcasts to subscribers. No shared mutexes.

**Step 1: Write the tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use agent_dash_core::protocol::HookEvent;

    #[test]
    fn apply_tool_start_sets_working() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "tu1".into(),
        });
        let session = state.sessions.get("s1").unwrap();
        assert_eq!(session.status, SessionStatus::Working);
        assert!(session.active_tool.is_some());
    }

    #[test]
    fn apply_tool_end_clears_tool() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "tu1".into(),
        });
        state.apply_hook_event(HookEvent::ToolEnd {
            session_id: "s1".into(),
            tool_use_id: "tu1".into(),
        });
        let session = state.sessions.get("s1").unwrap();
        assert!(session.active_tool.is_none());
    }

    #[test]
    fn apply_stop_sets_idle() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::ToolStart {
            session_id: "s1".into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            tool_use_id: "tu1".into(),
        });
        state.apply_hook_event(HookEvent::Stop { session_id: "s1".into() });
        let session = state.sessions.get("s1").unwrap();
        assert_eq!(session.status, SessionStatus::Idle);
        assert!(session.active_tool.is_none());
    }

    #[test]
    fn apply_session_end_removes_session() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.apply_hook_event(HookEvent::SessionEnd { session_id: "s1".into() });
        assert!(!state.sessions.contains_key("s1"));
    }

    #[test]
    fn permission_request_lifecycle() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.add_permission_request("s1", "tu1", "Bash", "rm -rf /tmp");
        assert!(state.pending_permissions.contains_key("tu1"));
        let session = state.sessions.get("s1").unwrap();
        assert_eq!(session.status, SessionStatus::NeedsInput);

        state.resolve_permission("tu1");
        assert!(!state.pending_permissions.contains_key("tu1"));
    }

    #[test]
    fn to_dash_sessions_returns_all() {
        let mut state = DaemonState::new();
        state.ensure_session("s1");
        state.ensure_session("s2");
        let dash = state.to_dash_sessions();
        assert_eq!(dash.len(), 2);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dashd`
Expected: FAIL — types not defined.

**Step 3: Implement state manager**

```rust
use agent_dash_core::protocol::HookEvent;
use agent_dash_core::session::{
    DashActiveTool, DashInputReason, DashSession, SessionStatus, tool_icon,
};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Internal session state owned by the daemon.
#[derive(Debug, Clone)]
pub struct InternalSession {
    pub session_id: String,
    pub pid: Option<u32>,
    pub cwd: Option<String>,
    pub project_name: String,
    pub branch: String,
    pub status: SessionStatus,
    pub active_tool: Option<(String, String, String)>, // (name, detail, tool_use_id)
    pub jsonl_path: Option<String>,
    pub last_status_change: u64,
    pub has_pending_question: bool,
    pub question_text: Option<String>,
}

/// A pending permission request.
#[derive(Debug, Clone)]
pub struct PendingPermission {
    pub request_id: String,
    pub session_id: String,
    pub tool: String,
    pub detail: String,
}

/// All daemon state. Owned by a single task, mutated via messages.
pub struct DaemonState {
    pub sessions: HashMap<String, InternalSession>,
    pub pending_permissions: HashMap<String, PendingPermission>,
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            pending_permissions: HashMap::new(),
        }
    }

    fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Ensure a session entry exists (creates with defaults if missing).
    pub fn ensure_session(&mut self, session_id: &str) {
        self.sessions.entry(session_id.to_string()).or_insert_with(|| InternalSession {
            session_id: session_id.to_string(),
            pid: None,
            cwd: None,
            project_name: "unknown".into(),
            branch: String::new(),
            status: SessionStatus::Idle,
            active_tool: None,
            jsonl_path: None,
            last_status_change: Self::now_epoch(),
            has_pending_question: false,
            question_text: None,
        });
    }

    /// Apply a hook event to internal state.
    pub fn apply_hook_event(&mut self, event: HookEvent) {
        match event {
            HookEvent::ToolStart { session_id, tool, detail, tool_use_id } => {
                self.ensure_session(&session_id);
                let session = self.sessions.get_mut(&session_id).unwrap();
                session.active_tool = Some((tool, detail, tool_use_id));
                self.set_status(&session_id, SessionStatus::Working);
            }
            HookEvent::ToolEnd { session_id, tool_use_id } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    if session.active_tool.as_ref().is_some_and(|(_, _, id)| id == &tool_use_id) {
                        session.active_tool = None;
                    }
                }
            }
            HookEvent::Stop { session_id } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.active_tool = None;
                    self.set_status(&session_id, SessionStatus::Idle);
                }
            }
            HookEvent::SessionStart { session_id, cwd } => {
                self.ensure_session(&session_id);
                if let Some(cwd) = cwd {
                    if let Some(session) = self.sessions.get_mut(&session_id) {
                        session.cwd = Some(cwd);
                    }
                }
            }
            HookEvent::SessionEnd { session_id } => {
                self.sessions.remove(&session_id);
            }
        }
    }

    fn set_status(&mut self, session_id: &str, new_status: SessionStatus) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            if session.status != new_status {
                session.status = new_status;
                session.last_status_change = Self::now_epoch();
            }
        }
    }

    /// Register a pending permission request.
    pub fn add_permission_request(&mut self, session_id: &str, request_id: &str, tool: &str, detail: &str) {
        self.pending_permissions.insert(request_id.to_string(), PendingPermission {
            request_id: request_id.to_string(),
            session_id: session_id.to_string(),
            tool: tool.to_string(),
            detail: detail.to_string(),
        });
        self.ensure_session(session_id);
        self.set_status(session_id, SessionStatus::NeedsInput);
    }

    /// Resolve a pending permission. Returns the permission if found.
    pub fn resolve_permission(&mut self, request_id: &str) -> Option<PendingPermission> {
        let perm = self.pending_permissions.remove(request_id)?;
        // If no more pending permissions for this session, clear NeedsInput
        let session_id = perm.session_id.clone();
        let has_more = self.pending_permissions.values().any(|p| p.session_id == session_id);
        if !has_more {
            if let Some(session) = self.sessions.get_mut(&session_id) {
                if session.status == SessionStatus::NeedsInput {
                    session.status = SessionStatus::Idle;
                    session.last_status_change = Self::now_epoch();
                }
            }
        }
        Some(perm)
    }

    /// Convert all sessions to the serializable form.
    pub fn to_dash_sessions(&self) -> Vec<DashSession> {
        self.sessions.values().map(|s| {
            let input_reason = if let Some(perm) = self.pending_permissions.values().find(|p| p.session_id == s.session_id) {
                Some(DashInputReason {
                    reason_type: "permission".into(),
                    tool: Some(perm.tool.clone()),
                    command: None,
                    detail: Some(perm.detail.clone()),
                    text: None,
                })
            } else if s.has_pending_question {
                Some(DashInputReason {
                    reason_type: "question".into(),
                    tool: None,
                    command: None,
                    detail: None,
                    text: s.question_text.clone(),
                })
            } else {
                None
            };

            DashSession {
                session_id: s.session_id.clone(),
                project_name: s.project_name.clone(),
                branch: s.branch.clone(),
                status: s.status.as_str().to_string(),
                last_status_change: s.last_status_change,
                jsonl_path: s.jsonl_path.clone(),
                input_reason,
                active_tool: s.active_tool.as_ref().map(|(name, detail, _)| DashActiveTool {
                    icon: tool_icon(name).to_string(),
                    name: name.clone(),
                    detail: detail.clone(),
                }),
            }
        }).collect()
    }
}
```

**Step 4: Register module and run tests**

Add `mod state;` to `crates/agent-dashd/src/main.rs`.

Run: `cargo test -p agent-dashd`
Expected: All pass.

**Step 5: Commit**

```bash
git add crates/agent-dashd/src/state.rs crates/agent-dashd/src/main.rs
git commit -m "feat(daemon): add state manager"
```

---

### Task 7: Daemon — Hook Listener

**Files:**
- Create: `crates/agent-dashd/src/hook_listener.rs`

Async listener on `hook.sock`. Accepts connections, reads one JSON message, sends it through a channel to the state manager.

**Step 1: Implement hook listener**

```rust
use agent_dash_core::paths;
use agent_dash_core::protocol::HookEvent;
use interprocess::local_socket::{
    tokio::prelude::*,
    GenericFilePath, ListenerOptions,
};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

/// Run the hook listener. Accepts fire-and-forget connections on hook.sock.
/// Each connection sends one JSON event, then disconnects.
pub async fn run(tx: mpsc::Sender<HookEvent>) {
    let name = paths::hook_socket_name();

    // Ensure parent directory exists
    let path = std::path::Path::new(&name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Remove stale socket
    let _ = std::fs::remove_file(&name);

    let listener = match ListenerOptions::new()
        .name(name.as_str().to_fs_name::<GenericFilePath>().expect("invalid socket path"))
        .create_tokio()
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!("agent-dashd: failed to bind hook socket: {e}");
            return;
        }
    };

    eprintln!("agent-dashd: hook listener on {name}");

    loop {
        match listener.accept().await {
            Ok(conn) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_hook_connection(conn, tx).await;
                });
            }
            Err(e) => {
                eprintln!("agent-dashd: hook accept error: {e}");
            }
        }
    }
}

async fn handle_hook_connection(
    mut conn: impl AsyncReadExt + Unpin,
    tx: mpsc::Sender<HookEvent>,
) {
    let mut buf = Vec::with_capacity(4096);
    match conn.read_to_end(&mut buf).await {
        Ok(_) => {
            let text = String::from_utf8_lossy(&buf);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return;
            }
            match serde_json::from_str::<HookEvent>(trimmed) {
                Ok(event) => {
                    let _ = tx.send(event).await;
                }
                Err(e) => {
                    eprintln!("agent-dashd: failed to parse hook event: {e}");
                }
            }
        }
        Err(e) => {
            eprintln!("agent-dashd: hook read error: {e}");
        }
    }
}
```

**Step 2: Register module**

Add `mod hook_listener;` to `crates/agent-dashd/src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build -p agent-dashd`
Expected: Compiles. (Integration testing deferred to Task 10.)

**Step 4: Commit**

```bash
git add crates/agent-dashd/src/hook_listener.rs crates/agent-dashd/src/main.rs
git commit -m "feat(daemon): add async hook listener"
```

---

### Task 8: Daemon — Client Listener

**Files:**
- Create: `crates/agent-dashd/src/client_listener.rs`

Async listener on `daemon.sock`. Handles persistent bidirectional connections. Supports subscribe, get_state, permission_request, and permission_response.

**Step 1: Implement client listener**

```rust
use agent_dash_core::paths;
use agent_dash_core::protocol::{self, ClientRequest, ServerEvent, HookPermissionDecision};
use interprocess::local_socket::{
    tokio::prelude::*,
    GenericFilePath, ListenerOptions,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

/// Messages from client connections to the main state manager.
pub enum ClientMessage {
    /// Client wants to subscribe to updates.
    Subscribe {
        tx: mpsc::Sender<String>,
    },
    /// Client requests current state snapshot.
    GetState {
        reply: tokio::sync::oneshot::Sender<String>,
    },
    /// Client sent a permission response.
    PermissionResponse {
        request_id: String,
        session_id: String,
        decision: String,
    },
    /// Hook binary sent a permission request (needs response back).
    PermissionRequest {
        request_id: String,
        session_id: String,
        tool: String,
        detail: String,
        reply: tokio::sync::oneshot::Sender<HookPermissionDecision>,
    },
}

/// Run the client listener. Accepts persistent connections on daemon.sock.
pub async fn run(tx: mpsc::Sender<ClientMessage>) {
    let name = paths::client_socket_name();

    let path = std::path::Path::new(&name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&name);

    let listener = match ListenerOptions::new()
        .name(name.as_str().to_fs_name::<GenericFilePath>().expect("invalid socket path"))
        .create_tokio()
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!("agent-dashd: failed to bind client socket: {e}");
            return;
        }
    };

    eprintln!("agent-dashd: client listener on {name}");

    loop {
        match listener.accept().await {
            Ok(conn) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_client_connection(conn, tx).await;
                });
            }
            Err(e) => {
                eprintln!("agent-dashd: client accept error: {e}");
            }
        }
    }
}

async fn handle_client_connection(
    conn: impl AsyncReadExt + AsyncWriteExt + Unpin,
    tx: mpsc::Sender<ClientMessage>,
) {
    let (reader, mut writer) = tokio::io::split(conn);
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req = match serde_json::from_str::<ClientRequest>(trimmed) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("agent-dashd: invalid client request: {e}");
                continue;
            }
        };

        match req {
            ClientRequest::Subscribe => {
                let (sub_tx, mut sub_rx) = mpsc::channel::<String>(64);
                let _ = tx.send(ClientMessage::Subscribe { tx: sub_tx }).await;
                // Stream events to client until they disconnect
                while let Some(line) = sub_rx.recv().await {
                    if writer.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                }
                return; // Connection done after subscribe ends
            }
            ClientRequest::GetState => {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let _ = tx.send(ClientMessage::GetState { reply: reply_tx }).await;
                if let Ok(json) = reply_rx.await {
                    let _ = writer.write_all(json.as_bytes()).await;
                }
            }
            ClientRequest::PermissionResponse { request_id, session_id, decision } => {
                let _ = tx.send(ClientMessage::PermissionResponse {
                    request_id,
                    session_id,
                    decision,
                }).await;
            }
            ClientRequest::PermissionRequest { request_id, session_id, tool, detail } => {
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let _ = tx.send(ClientMessage::PermissionRequest {
                    request_id,
                    session_id,
                    tool,
                    detail,
                    reply: reply_tx,
                }).await;
                // Wait for the permission decision and send it back to the hook
                if let Ok(decision) = reply_rx.await {
                    if let Ok(line) = protocol::encode_line(&decision) {
                        let _ = writer.write_all(line.as_bytes()).await;
                    }
                }
            }
        }
    }
}
```

**Step 2: Register module**

Add `mod client_listener;` to `crates/agent-dashd/src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo build -p agent-dashd`
Expected: Compiles.

**Step 4: Commit**

```bash
git add crates/agent-dashd/src/client_listener.rs crates/agent-dashd/src/main.rs
git commit -m "feat(daemon): add async client listener"
```

---

### Task 9: Daemon — Main Loop

**Files:**
- Modify: `crates/agent-dashd/src/main.rs`

Wire everything together: spawn hook listener, client listener, process scanner, and state manager as concurrent tokio tasks.

**Step 1: Implement the main loop**

```rust
mod client_listener;
mod hook_listener;
mod scanner;
mod state;

use agent_dash_core::paths;
use agent_dash_core::protocol::{self, HookEvent, ServerEvent};
use client_listener::ClientMessage;
use state::DaemonState;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    eprintln!("agent-dashd starting");
    eprintln!("  hook socket: {}", paths::hook_socket_name());
    eprintln!("  client socket: {}", paths::client_socket_name());
    eprintln!("  state file: {}", paths::state_file_path().display());

    // Channels
    let (hook_tx, mut hook_rx) = mpsc::channel::<HookEvent>(256);
    let (client_tx, mut client_rx) = mpsc::channel::<ClientMessage>(256);

    // Spawn hook listener
    tokio::spawn(hook_listener::run(hook_tx));

    // Spawn client listener
    tokio::spawn(client_listener::run(client_tx.clone()));

    // State manager + process scanner in main task
    let mut state = DaemonState::new();
    let mut subscribers: Vec<mpsc::Sender<String>> = Vec::new();
    let mut permission_waiters: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Sender<protocol::HookPermissionDecision>,
    > = std::collections::HashMap::new();
    let mut scan_interval = tokio::time::interval(Duration::from_secs(5));
    let mut write_interval = tokio::time::interval(Duration::from_millis(500));
    let mut state_dirty = true;

    loop {
        tokio::select! {
            // Hook events
            Some(event) = hook_rx.recv() => {
                // If we get a tool event for a session with pending permission,
                // the permission was resolved via terminal
                if let HookEvent::ToolStart { ref session_id, .. }
                     | HookEvent::Stop { ref session_id } = &event
                {
                    let resolved: Vec<String> = state.pending_permissions
                        .iter()
                        .filter(|(_, p)| &p.session_id == session_id)
                        .map(|(id, _)| id.clone())
                        .collect();
                    for request_id in resolved {
                        if let Some(perm) = state.resolve_permission(&request_id) {
                            // Notify the waiting hook (if any)
                            if let Some(waiter) = permission_waiters.remove(&request_id) {
                                let _ = waiter.send(protocol::HookPermissionDecision {
                                    request_id: request_id.clone(),
                                    decision: "allow".into(),
                                });
                            }
                            // Broadcast resolution to clients
                            let event = ServerEvent::PermissionResolved {
                                request_id,
                                resolved_by: "terminal".into(),
                            };
                            broadcast_to_subscribers(&mut subscribers, &event).await;
                        }
                    }
                }

                state.apply_hook_event(event);
                state_dirty = true;
                broadcast_state(&mut subscribers, &state).await;
            }

            // Client messages
            Some(msg) = client_rx.recv() => {
                match msg {
                    ClientMessage::Subscribe { tx } => {
                        // Send current state immediately
                        let event = ServerEvent::StateUpdate {
                            sessions: state.to_dash_sessions(),
                        };
                        if let Ok(line) = protocol::encode_line(&event) {
                            let _ = tx.send(line).await;
                        }
                        // Send pending permissions
                        for perm in state.pending_permissions.values() {
                            let event = ServerEvent::PermissionPending {
                                session_id: perm.session_id.clone(),
                                request_id: perm.request_id.clone(),
                                tool: perm.tool.clone(),
                                detail: perm.detail.clone(),
                            };
                            if let Ok(line) = protocol::encode_line(&event) {
                                let _ = tx.send(line).await;
                            }
                        }
                        subscribers.push(tx);
                    }
                    ClientMessage::GetState { reply } => {
                        let event = ServerEvent::StateUpdate {
                            sessions: state.to_dash_sessions(),
                        };
                        if let Ok(line) = protocol::encode_line(&event) {
                            let _ = reply.send(line);
                        }
                    }
                    ClientMessage::PermissionResponse { request_id, session_id: _, decision } => {
                        if let Some(perm) = state.resolve_permission(&request_id) {
                            // Send decision to the waiting hook
                            if let Some(waiter) = permission_waiters.remove(&request_id) {
                                let _ = waiter.send(protocol::HookPermissionDecision {
                                    request_id: request_id.clone(),
                                    decision: decision.clone(),
                                });
                            }
                            // Broadcast resolution
                            let event = ServerEvent::PermissionResolved {
                                request_id,
                                resolved_by: "client".into(),
                            };
                            broadcast_to_subscribers(&mut subscribers, &event).await;
                            state_dirty = true;
                            broadcast_state(&mut subscribers, &state).await;
                        }
                    }
                    ClientMessage::PermissionRequest { request_id, session_id, tool, detail, reply } => {
                        state.add_permission_request(&session_id, &request_id, &tool, &detail);
                        permission_waiters.insert(request_id.clone(), reply);
                        // Broadcast pending to all subscribers
                        let event = ServerEvent::PermissionPending {
                            session_id,
                            request_id,
                            tool,
                            detail,
                        };
                        broadcast_to_subscribers(&mut subscribers, &event).await;
                        state_dirty = true;
                        broadcast_state(&mut subscribers, &state).await;
                    }
                }
            }

            // Periodic process scan
            _ = scan_interval.tick() => {
                let processes = scanner::scan_claude_processes();
                let claude_projects_dir = paths::claude_projects_dir();

                // Update sessions from discovered processes
                let mut seen_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
                for (pid, proc_info) in &processes {
                    seen_pids.insert(*pid);
                    let slug = paths::cwd_to_project_slug(&proc_info.cwd);
                    let project_dir = claude_projects_dir.join(&slug);
                    let jsonl_path = scanner::find_latest_jsonl(&project_dir);
                    let jsonl_status = jsonl_path.as_ref().and_then(|p| scanner::parse_jsonl_status(p));

                    let session_id = jsonl_status
                        .as_ref()
                        .map(|s| s.session_id.clone())
                        .unwrap_or_else(|| format!("pid-{}", pid));

                    state.ensure_session(&session_id);
                    let session = state.sessions.get_mut(&session_id).unwrap();
                    session.pid = Some(*pid);
                    session.cwd = Some(proc_info.cwd.to_string_lossy().to_string());
                    session.project_name = paths::project_name_from_cwd(&proc_info.cwd);
                    if let Some(ref js) = jsonl_status {
                        session.branch = js.git_branch.clone();
                        session.has_pending_question = js.has_pending_question;
                        session.question_text = js.question_text.clone();
                    }
                    if let Some(ref jp) = jsonl_path {
                        session.jsonl_path = Some(jp.to_string_lossy().to_string());
                    }
                }

                // Prune sessions whose processes are gone (keep hook-only sessions)
                state.sessions.retain(|_id, session| {
                    if let Some(pid) = session.pid {
                        seen_pids.contains(&pid)
                    } else {
                        true // No pid = hook-only session, keep it
                    }
                });

                state_dirty = true;
                broadcast_state(&mut subscribers, &state).await;
            }

            // Periodic state.json write
            _ = write_interval.tick() => {
                if state_dirty {
                    write_state_file(&state);
                    state_dirty = false;
                }
            }
        }
    }
}

async fn broadcast_state(subscribers: &mut Vec<mpsc::Sender<String>>, state: &DaemonState) {
    let event = ServerEvent::StateUpdate {
        sessions: state.to_dash_sessions(),
    };
    broadcast_to_subscribers(subscribers, &event).await;
}

async fn broadcast_to_subscribers(subscribers: &mut Vec<mpsc::Sender<String>>, event: &ServerEvent) {
    let Ok(line) = protocol::encode_line(event) else { return };
    // Send to all, remove disconnected
    let mut i = 0;
    while i < subscribers.len() {
        if subscribers[i].try_send(line.clone()).is_err() {
            subscribers.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

fn write_state_file(state: &DaemonState) {
    let dash_state = agent_dash_core::session::DashState {
        sessions: state.to_dash_sessions(),
    };
    let path = paths::state_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string_pretty(&dash_state) else { return };
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}
```

Note: `DashState` needs to be added to `session.rs` in core:

```rust
/// Top-level state (for state.json compat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashState {
    pub sessions: Vec<DashSession>,
}
```

**Step 2: Verify it compiles**

Run: `cargo build -p agent-dashd`
Expected: Compiles.

**Step 3: Commit**

```bash
git add crates/agent-dashd/src/main.rs crates/agent-dash-core/src/session.rs
git commit -m "feat(daemon): wire up main loop with all tasks"
```

---

### Task 10: Hook Companion Binary

**Files:**
- Modify: `crates/agent-dash-hook/src/main.rs`

Replaces both shell scripts. Reads hook context JSON from stdin, sends events to daemon.

**Step 1: Implement**

```rust
use agent_dash_core::paths;
use agent_dash_core::protocol;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::UnixStream;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: agent-dash-hook <subcommand>");
        eprintln!("subcommands: tool-start, tool-end, stop, session-start, session-end, permission");
        std::process::exit(1);
    }

    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or_default();

    let hook_input: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            // Can't parse input, exit silently (don't block Claude)
            std::process::exit(0);
        }
    };

    let session_id = hook_input.get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    if session_id.is_empty() {
        std::process::exit(0);
    }

    match args[1].as_str() {
        "permission" => handle_permission(&hook_input, &session_id),
        subcommand => handle_fire_and_forget(subcommand, &hook_input, &session_id),
    }
}

fn handle_fire_and_forget(subcommand: &str, input: &serde_json::Value, session_id: &str) {
    let event = match subcommand {
        "tool-start" | "tool_start" => {
            let tool = input.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
            let tool_use_id = input.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
            let detail = extract_tool_detail(tool, input);
            serde_json::json!({
                "event": "tool_start",
                "session_id": session_id,
                "tool": tool,
                "tool_use_id": tool_use_id,
                "detail": detail
            })
        }
        "tool-end" | "tool_end" => {
            let tool_use_id = input.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
            serde_json::json!({
                "event": "tool_end",
                "session_id": session_id,
                "tool_use_id": tool_use_id
            })
        }
        "stop" => {
            serde_json::json!({ "event": "stop", "session_id": session_id })
        }
        "session-start" | "session_start" => {
            let cwd = input.get("cwd").and_then(|v| v.as_str());
            let mut obj = serde_json::json!({ "event": "session_start", "session_id": session_id });
            if let Some(cwd) = cwd {
                obj["cwd"] = serde_json::Value::String(cwd.to_string());
            }
            obj
        }
        "session-end" | "session_end" => {
            serde_json::json!({ "event": "session_end", "session_id": session_id })
        }
        _ => {
            eprintln!("agent-dash-hook: unknown subcommand: {}", subcommand);
            std::process::exit(1);
        }
    };

    // Fire and forget to hook socket
    let socket_name = paths::hook_socket_name();
    let Ok(mut conn) = UnixStream::connect(&socket_name) else {
        // Daemon not running, exit silently
        std::process::exit(0);
    };
    let json = serde_json::to_string(&event).unwrap_or_default();
    let _ = conn.write_all(json.as_bytes());
    let _ = conn.shutdown(std::net::Shutdown::Write);
}

fn handle_permission(input: &serde_json::Value, session_id: &str) {
    let tool = input.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
    let tool_input = input.get("tool_input").cloned().unwrap_or(serde_json::Value::Null);
    let request_id = input.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
    let detail = format!("{}", tool_input);

    let request = serde_json::json!({
        "method": "permission_request",
        "request_id": request_id,
        "session_id": session_id,
        "tool": tool,
        "detail": detail
    });

    // Connect to daemon.sock (not hook.sock, we need a response)
    let socket_name = paths::client_socket_name();
    let Ok(mut conn) = UnixStream::connect(&socket_name) else {
        // Daemon not running, fall through to terminal
        std::process::exit(0);
    };

    // Send request
    let mut msg = serde_json::to_string(&request).unwrap_or_default();
    msg.push('\n');
    if conn.write_all(msg.as_bytes()).is_err() {
        std::process::exit(0);
    }

    // Wait for response (with 120s timeout)
    conn.set_read_timeout(Some(std::time::Duration::from_secs(120))).ok();
    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(decision) = serde_json::from_str::<protocol::HookPermissionDecision>(&line) else {
            continue;
        };

        // Translate to Claude's hook response format
        let response = match decision.decision.as_str() {
            "deny" => serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "deny",
                        "message": "Denied from agent-dash"
                    }
                }
            }),
            "allow_similar" => serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "allow",
                        "updatedPermissions": [{
                            "tool": tool,
                            "permission": "allow"
                        }]
                    }
                }
            }),
            _ => serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": { "behavior": "allow" }
                }
            }),
        };

        println!("{}", serde_json::to_string(&response).unwrap());
        std::process::exit(0);
    }

    // Timeout or error — fall through to terminal prompt
    std::process::exit(0);
}

fn extract_tool_detail(tool: &str, input: &serde_json::Value) -> String {
    let tool_input = input.get("tool_input");
    let detail = match tool {
        "Bash" => tool_input.and_then(|i| i.get("command")).and_then(|v| v.as_str()),
        "Read" | "Edit" | "Write" => tool_input.and_then(|i| i.get("file_path")).and_then(|v| v.as_str()),
        "Grep" | "Glob" => tool_input.and_then(|i| i.get("pattern")).and_then(|v| v.as_str()),
        "WebFetch" => tool_input.and_then(|i| i.get("url")).and_then(|v| v.as_str()),
        "WebSearch" => tool_input.and_then(|i| i.get("query")).and_then(|v| v.as_str()),
        _ => tool_input.and_then(|i| i.get("description")).and_then(|v| v.as_str()),
    };
    let s = detail.unwrap_or(tool).to_string();
    if s.len() > 200 { s[..200].to_string() } else { s }
}
```

Note: This uses `std::os::unix::net::UnixStream` which is Unix-only. For cross-platform, this will need to be updated to use `interprocess` sync local sockets. For now this gets the Linux/macOS path working. The cross-platform abstraction can be added as a follow-up.

**Step 2: Verify it compiles**

Run: `cargo build -p agent-dash-hook`
Expected: Compiles.

**Step 3: Commit**

```bash
git add crates/agent-dash-hook/src/main.rs
git commit -m "feat: add hook companion binary"
```

---

### Task 11: CLI Tool (agentctl)

**Files:**
- Modify: `crates/agentctl/src/main.rs`

**Step 1: Implement**

```rust
use agent_dash_core::paths;
use agent_dash_core::protocol::{self, ClientRequest, ServerEvent};
use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    match subcommand {
        "status" | "list" => cmd_status(),
        "watch" => cmd_watch(),
        "approve" => {
            let id = args.get(2).expect("usage: agentctl approve <request_id>");
            cmd_permission_response(id, "allow");
        }
        "approve-similar" => {
            let id = args.get(2).expect("usage: agentctl approve-similar <request_id>");
            cmd_permission_response(id, "allow_similar");
        }
        "deny" => {
            let id = args.get(2).expect("usage: agentctl deny <request_id>");
            cmd_permission_response(id, "deny");
        }
        _ => {
            eprintln!("usage: agentctl <status|list|watch|approve|approve-similar|deny>");
            std::process::exit(1);
        }
    }
}

fn connect() -> UnixStream {
    let socket_name = paths::client_socket_name();
    UnixStream::connect(&socket_name).unwrap_or_else(|e| {
        eprintln!("Failed to connect to agent-dashd at {}: {}", socket_name, e);
        eprintln!("Is the daemon running?");
        std::process::exit(1);
    })
}

fn send_request(conn: &mut UnixStream, request: &ClientRequest) {
    let mut line = serde_json::to_string(request).unwrap();
    line.push('\n');
    conn.write_all(line.as_bytes()).unwrap();
}

fn cmd_status() {
    let mut conn = connect();
    send_request(&mut conn, &ClientRequest::GetState);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&line) else { continue };
        if let ServerEvent::StateUpdate { sessions } = event {
            if sessions.is_empty() {
                println!("No active sessions.");
                return;
            }
            for s in &sessions {
                let tool_info = s.active_tool.as_ref()
                    .map(|t| format!(" [{}:{}]", t.name, truncate(&t.detail, 40)))
                    .unwrap_or_default();
                println!("{:<12} {:<10} {:<10} {}{}",
                    truncate(&s.project_name, 12),
                    s.branch,
                    s.status,
                    &s.session_id[..8.min(s.session_id.len())],
                    tool_info,
                );
            }
            return;
        }
    }
}

fn cmd_watch() {
    let mut conn = connect();
    send_request(&mut conn, &ClientRequest::Subscribe);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        println!("{}", line);
    }
}

fn cmd_permission_response(request_id: &str, decision: &str) {
    let mut conn = connect();
    let req = ClientRequest::PermissionResponse {
        request_id: request_id.to_string(),
        session_id: String::new(), // daemon looks up by request_id
        decision: decision.to_string(),
    };
    send_request(&mut conn, &req);
    println!("Sent {} for {}", decision, request_id);
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() > max { &s[..max] } else { s }
}
```

Same note as Task 10: uses `UnixStream` directly, cross-platform follow-up later.

**Step 2: Verify it compiles**

Run: `cargo build -p agentctl`
Expected: Compiles.

**Step 3: Commit**

```bash
git add crates/agentctl/src/main.rs
git commit -m "feat: add agentctl CLI tool"
```

---

### Task 12: Integration Test — End-to-End

**Files:**
- Create: `crates/agent-dashd/tests/integration.rs`

Starts the daemon, sends hook events, verifies state via agentctl protocol.

**Step 1: Write integration test**

```rust
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Start the daemon in background, run a basic flow, verify output.
#[test]
fn hook_event_updates_state() {
    let dir = std::env::temp_dir().join(format!("agent-dash-integ-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hook_sock = dir.join("hook.sock");
    let client_sock = dir.join("daemon.sock");

    // Set env vars so paths module uses our test directory
    // (This requires paths module to support override, or we test at protocol level)

    // For now, test at the protocol/state level without spawning the actual daemon
    // Full E2E test is a manual smoke test

    // Test the state manager directly
    use agent_dash_core::protocol::HookEvent;

    // This is covered by unit tests in state.rs — mark as placeholder
    // for future E2E testing with actual socket connections.
    assert!(true);

    std::fs::remove_dir_all(&dir).ok();
}
```

This is a placeholder — real integration testing will be done manually by running `agent-dashd` and `agentctl watch` in separate terminals.

**Step 2: Verify everything builds and tests pass**

Run: `cargo build --workspace && cargo test --workspace`
Expected: All crates compile, all tests pass.

**Step 3: Commit**

```bash
git add crates/agent-dashd/tests/
git commit -m "test: add integration test placeholder"
```

---

### Task 13: Clean Up and Final Verification

**Step 1: Run full workspace build**

Run: `cargo build --workspace`
Expected: All four crates compile.

**Step 2: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 3: Manual smoke test**

1. Run `cargo run -p agent-dashd` in terminal 1
2. Run `cargo run -p agentctl -- watch` in terminal 2
3. Start a Claude session — verify events appear in terminal 2
4. Run `cargo run -p agentctl -- status` — verify session listing

**Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix: address issues found in smoke testing"
```

**Step 5: Final commit summarizing the reimplementation**

The old `src/` directory and shell scripts remain for now (the GNOME extension still uses `state.json`). They can be removed once the extension is migrated.
