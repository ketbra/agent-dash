# Agent Dash Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a frameless overlay that monitors Claude Code sessions with bidirectional permission prompt handling.

**Architecture:** egui/eframe GUI polls process list and watches JSONL session files to determine status. A global PermissionRequest hook writes pending prompts to IPC files; the dashboard reads them and writes responses back. Three layers: data types (session.rs), monitoring logic (monitor.rs + ipc.rs), GUI (app.rs + main.rs).

**Tech Stack:** Rust (edition 2024), egui/eframe, procfs, notify, serde/serde_json

---

### Task 1: Project dependencies and skeleton modules

**Files:**
- Modify: `Cargo.toml`
- Create: `src/session.rs`
- Create: `src/monitor.rs`
- Create: `src/ipc.rs`
- Create: `src/app.rs`
- Modify: `src/main.rs`

**Step 1: Add dependencies to Cargo.toml**

```toml
[package]
name = "agent-dash"
version = "0.1.0"
edition = "2024"

[dependencies]
eframe = "0.31"
egui = "0.31"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
procfs = "0.17"
notify = "8"
dirs = "6"
```

**Step 2: Create empty module files and wire up main.rs**

Create `src/session.rs`, `src/monitor.rs`, `src/ipc.rs`, `src/app.rs` as empty files.

Update `src/main.rs`:
```rust
mod app;
mod ipc;
mod monitor;
mod session;

fn main() {
    println!("agent-dash starting");
}
```

**Step 3: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors (warnings about unused modules are fine)

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add dependencies and skeleton module structure"
```

---

### Task 2: Session data types

**Files:**
- Modify: `src/session.rs`

**Step 1: Write tests for SessionStatus ordering and Session struct**

Add to `src/session.rs`:
```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;

/// Status of a Claude Code session, ordered by priority (Red > Yellow > Green > Grey).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Needs user input (permission prompt or AskUserQuestion)
    NeedsInput,
    /// Actively working (JSONL recently modified)
    Working,
    /// Idle at prompt
    Idle,
    /// Process ended, session will be removed soon
    Ended,
}

impl SessionStatus {
    /// Sort priority: lower number = higher priority (floats to top).
    pub fn sort_key(&self) -> u8 {
        match self {
            SessionStatus::NeedsInput => 0,
            SessionStatus::Working => 1,
            SessionStatus::Idle => 2,
            SessionStatus::Ended => 3,
        }
    }
}

/// A pending permission request from the hook IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub session_id: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub timestamp: u64,
}

/// Info about why a session needs input.
#[derive(Debug, Clone)]
pub enum InputReason {
    Permission(PermissionRequest),
    Question { text: String },
}

/// A monitored Claude Code session.
#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub pid: i32,
    pub pty: PathBuf,
    pub cwd: PathBuf,
    pub project_name: String,
    pub branch: String,
    pub status: SessionStatus,
    pub input_reason: Option<InputReason>,
    pub jsonl_path: PathBuf,
    pub last_jsonl_modified: Option<std::time::SystemTime>,
    /// When the process was first detected as ended (for fade-out delay).
    pub ended_at: Option<Instant>,
}

impl Session {
    pub fn label(&self) -> String {
        if self.branch.is_empty() || self.branch == "main" {
            self.project_name.clone()
        } else {
            format!("{} ({})", self.project_name, self.branch)
        }
    }
}

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
    fn label_without_branch() {
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
            ended_at: None,
        };
        assert_eq!(s.label(), "project");
    }

    #[test]
    fn label_with_branch() {
        let s = Session {
            session_id: "abc".into(),
            pid: 1,
            pty: PathBuf::from("/dev/pts/0"),
            cwd: PathBuf::from("/home/user/project"),
            project_name: "project".into(),
            branch: "feature-x".into(),
            status: SessionStatus::Idle,
            input_reason: None,
            jsonl_path: PathBuf::new(),
            last_jsonl_modified: None,
            ended_at: None,
        };
        assert_eq!(s.label(), "project (feature-x)");
    }

    #[test]
    fn permission_request_deserialize() {
        let json = r#"{"session_id":"abc","tool":"Bash","input":{"command":"ls"},"timestamp":123}"#;
        let req: PermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.tool, "Bash");
        assert_eq!(req.session_id, "abc");
    }
}
```

**Step 2: Run tests**

Run: `cargo test --lib session`
Expected: all 4 tests pass

**Step 3: Commit**

```bash
git add src/session.rs
git commit -m "feat: add session data types with status ordering"
```

---

### Task 3: Process discovery

**Files:**
- Modify: `src/monitor.rs`

**Step 1: Implement process scanning**

Add to `src/monitor.rs`:
```rust
use crate::session::SessionStatus;
use procfs::process::{FDTarget, Process};
use std::collections::HashMap;
use std::path::PathBuf;

/// Info about a discovered claude process.
#[derive(Debug, Clone)]
pub struct ClaudeProcess {
    pub pid: i32,
    pub cwd: PathBuf,
    pub pty: PathBuf,
}

/// Scan /proc for running claude processes.
/// Returns a map of PID -> ClaudeProcess.
pub fn scan_claude_processes() -> HashMap<i32, ClaudeProcess> {
    let mut result = HashMap::new();
    let Ok(all) = procfs::process::all_processes() else {
        return result;
    };
    for proc_entry in all {
        let Ok(proc) = proc_entry else { continue };
        let Ok(cmdline) = proc.cmdline() else { continue };
        // Match processes whose first arg is "claude" (the binary name)
        let is_claude = cmdline.first().is_some_and(|arg| {
            arg == "claude" || arg.ends_with("/claude")
        });
        if !is_claude {
            continue;
        }
        let Ok(cwd) = proc.cwd() else { continue };
        // Read fd 0 (stdin) to find the PTY
        let pty = match proc.fd_from_fd(0) {
            Ok(fd_info) => match fd_info.target {
                FDTarget::Path(p) => p,
                _ => continue,
            },
            Err(_) => continue,
        };
        let pid = proc.pid();
        result.insert(pid, ClaudeProcess { pid, cwd, pty });
    }
    result
}

/// Convert a CWD path to a Claude project slug.
/// e.g., /home/user/src/traider -> -home-user-src-traider
pub fn cwd_to_project_slug(cwd: &std::path::Path) -> String {
    cwd.to_string_lossy().replace('/', "-")
}

/// Extract the project name (last path component) from a CWD.
pub fn project_name_from_cwd(cwd: &std::path::Path) -> String {
    cwd.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Find the most recently modified .jsonl file in a directory.
pub fn find_latest_jsonl(dir: &std::path::Path) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "jsonl")
        })
        .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cwd_to_slug() {
        let cwd = PathBuf::from("/home/user/src/traider");
        assert_eq!(cwd_to_project_slug(&cwd), "-home-user-src-traider");
    }

    #[test]
    fn test_project_name() {
        assert_eq!(
            project_name_from_cwd(&PathBuf::from("/home/user/src/traider")),
            "traider"
        );
    }

    #[test]
    fn test_project_name_worktree() {
        assert_eq!(
            project_name_from_cwd(&PathBuf::from("/home/user/src/traider/.worktrees/backtesting")),
            "backtesting"
        );
    }

    #[test]
    fn test_find_latest_jsonl_empty_dir() {
        let dir = std::env::temp_dir().join("agent-dash-test-empty");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_latest_jsonl(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_find_latest_jsonl() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.jsonl"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(dir.join("new.jsonl"), "{}").unwrap();
        let latest = find_latest_jsonl(&dir).unwrap();
        assert_eq!(latest.file_name().unwrap(), "new.jsonl");
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

**Step 2: Run tests**

Run: `cargo test --lib monitor`
Expected: all 5 tests pass

**Step 3: Commit**

```bash
git add src/monitor.rs
git commit -m "feat: add process discovery and project slug helpers"
```

---

### Task 4: JSONL parsing for status detection

**Files:**
- Modify: `src/monitor.rs` (add JSONL parsing functions)

**Step 1: Add JSONL parsing and status detection**

Append to `src/monitor.rs`:
```rust
/// A parsed JSONL message (we only care about a few fields).
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

/// Read the last N lines of a file (avoids reading the entire file).
fn read_tail_lines(path: &std::path::Path, max_lines: usize) -> Vec<String> {
    use std::io::{BufRead, BufReader};
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
    let start = all_lines.len().saturating_sub(max_lines);
    all_lines[start..].to_vec()
}

/// Parse the tail of a JSONL file to extract session status.
pub fn parse_jsonl_status(path: &std::path::Path) -> Option<JsonlStatus> {
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
                if let Some(sid) = sid {
                    session_id = sid;
                }
                if let Some(gb) = gb {
                    git_branch = gb;
                }
                last_was_user = false;
                last_was_assistant_with_ask = false;
                question_text = None;
                if let Some(msg) = message {
                    if let Some(content) = msg.content {
                        for block in content {
                            if let ContentBlock::ToolUse { name, input } = block {
                                if name == "AskUserQuestion" {
                                    last_was_assistant_with_ask = true;
                                    // Try to extract the question text
                                    if let Some(qs) = input.get("questions") {
                                        if let Some(arr) = qs.as_array() {
                                            if let Some(first) = arr.first() {
                                                if let Some(q) = first.get("question") {
                                                    question_text =
                                                        q.as_str().map(|s| s.to_string());
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
                if let Some(sid) = sid {
                    session_id = sid;
                }
                if let Some(gb) = gb {
                    git_branch = gb;
                }
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

Add `use serde::Deserialize;` to the top of monitor.rs imports.

**Step 2: Add tests for JSONL parsing**

Append to the `mod tests` block in `src/monitor.rs`:
```rust
    #[test]
    fn test_parse_jsonl_working_session() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-parse");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}
{"type":"user","sessionId":"abc-123","gitBranch":"main","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}
{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file":"foo.rs"}}]}}
"#;
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert_eq!(status.session_id, "abc-123");
        assert_eq!(status.git_branch, "main");
        assert!(!status.has_pending_question);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_parse_jsonl_pending_question() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-question");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"feat","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"Which approach?","header":"Approach","options":[{"label":"A"},{"label":"B"}]}]}}]}}
"#;
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert_eq!(status.session_id, "abc-123");
        assert_eq!(status.git_branch, "feat");
        assert!(status.has_pending_question);
        assert_eq!(status.question_text.as_deref(), Some("Which approach?"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_parse_jsonl_answered_question() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-answered");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"Which?"}]}}]}}
{"type":"user","sessionId":"abc-123","gitBranch":"main","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"Option A"}]}}
"#;
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert!(!status.has_pending_question);
        std::fs::remove_dir_all(&dir).ok();
    }
```

**Step 2: Run tests**

Run: `cargo test --lib monitor`
Expected: all 8 tests pass

**Step 3: Commit**

```bash
git add src/monitor.rs
git commit -m "feat: add JSONL parsing for session status detection"
```

---

### Task 5: IPC for permission prompts

**Files:**
- Modify: `src/ipc.rs`

**Step 1: Implement IPC read/write functions**

Add to `src/ipc.rs`:
```rust
use crate::session::PermissionRequest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The response the dashboard writes for the hook to read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionResponse {
    pub decision: PermissionDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDecision {
    pub behavior: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Base directory for IPC files.
pub fn ipc_base_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-dash")
        .join("sessions")
}

/// Path to the pending permission file for a session.
pub fn pending_permission_path(session_id: &str) -> PathBuf {
    ipc_base_dir().join(session_id).join("pending-permission.json")
}

/// Path to the permission response file for a session.
pub fn permission_response_path(session_id: &str) -> PathBuf {
    ipc_base_dir().join(session_id).join("permission-response.json")
}

/// Read a pending permission request (if one exists).
pub fn read_pending_permission(session_id: &str) -> Option<PermissionRequest> {
    let path = pending_permission_path(session_id);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write a permission response for the hook to read.
pub fn write_permission_response(session_id: &str, response: &PermissionResponse) -> std::io::Result<()> {
    let path = permission_response_path(session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    // Write atomically: write to temp file then rename
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Scan the IPC directory for all pending permission requests.
pub fn scan_pending_permissions() -> Vec<PermissionRequest> {
    let base = ipc_base_dir();
    let Ok(entries) = std::fs::read_dir(&base) else {
        return vec![];
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let session_id = e.file_name().to_string_lossy().to_string();
            read_pending_permission(&session_id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_roundtrip() {
        let test_id = "test-session-roundtrip";
        let base = ipc_base_dir().join(test_id);
        std::fs::create_dir_all(&base).unwrap();

        // Write a pending permission
        let req = PermissionRequest {
            session_id: test_id.to_string(),
            tool: "Bash".to_string(),
            input: serde_json::json!({"command": "cargo build"}),
            timestamp: 12345,
        };
        let req_path = pending_permission_path(test_id);
        std::fs::write(&req_path, serde_json::to_string(&req).unwrap()).unwrap();

        // Read it back
        let read = read_pending_permission(test_id).unwrap();
        assert_eq!(read.tool, "Bash");
        assert_eq!(read.session_id, test_id);

        // Write a response
        let resp = PermissionResponse {
            decision: PermissionDecision {
                behavior: "allow".to_string(),
                message: None,
            },
        };
        write_permission_response(test_id, &resp).unwrap();

        // Verify response file exists and is valid
        let resp_path = permission_response_path(test_id);
        let content = std::fs::read_to_string(&resp_path).unwrap();
        let parsed: PermissionResponse = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.decision.behavior, "allow");

        // Cleanup
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn test_read_nonexistent_permission() {
        assert!(read_pending_permission("nonexistent-session-xyz").is_none());
    }

    #[test]
    fn test_deny_response_with_message() {
        let test_id = "test-session-deny";
        let base = ipc_base_dir().join(test_id);
        std::fs::create_dir_all(&base).unwrap();

        let resp = PermissionResponse {
            decision: PermissionDecision {
                behavior: "deny".to_string(),
                message: Some("User denied from dashboard".to_string()),
            },
        };
        write_permission_response(test_id, &resp).unwrap();

        let content = std::fs::read_to_string(permission_response_path(test_id)).unwrap();
        let parsed: PermissionResponse = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.decision.behavior, "deny");
        assert_eq!(
            parsed.decision.message.as_deref(),
            Some("User denied from dashboard")
        );

        std::fs::remove_dir_all(&base).ok();
    }
}
```

**Step 2: Run tests**

Run: `cargo test --lib ipc`
Expected: all 3 tests pass

**Step 3: Commit**

```bash
git add src/ipc.rs
git commit -m "feat: add IPC protocol for permission prompt bridge"
```

---

### Task 6: Session monitor (combines process + JSONL + IPC)

**Files:**
- Modify: `src/monitor.rs` (add `SessionMonitor` struct)

**Step 1: Add the SessionMonitor that ties everything together**

Append to `src/monitor.rs`:
```rust
use crate::ipc;
use crate::session::{InputReason, Session, SessionStatus};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

pub struct SessionMonitor {
    pub sessions: HashMap<String, Session>,
    claude_projects_dir: PathBuf,
}

impl SessionMonitor {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let claude_projects_dir = home.join(".claude").join("projects");
        Self {
            sessions: HashMap::new(),
            claude_projects_dir,
        }
    }

    /// Full refresh: scan processes, parse JSONL, check IPC.
    pub fn refresh(&mut self) {
        let processes = scan_claude_processes();
        let pending_perms = ipc::scan_pending_permissions();
        let perm_map: HashMap<String, _> = pending_perms
            .into_iter()
            .map(|p| (p.session_id.clone(), p))
            .collect();

        // Track which sessions are still alive
        let mut seen_sessions: HashMap<String, Session> = HashMap::new();

        for (_pid, proc_info) in &processes {
            let slug = cwd_to_project_slug(&proc_info.cwd);
            let project_dir = self.claude_projects_dir.join(&slug);
            let Some(jsonl_path) = find_latest_jsonl(&project_dir) else {
                continue;
            };
            let Some(jsonl_status) = parse_jsonl_status(&jsonl_path) else {
                continue;
            };

            let session_id = jsonl_status.session_id.clone();
            // Skip if we already processed this session (dedup for subagents)
            if seen_sessions.contains_key(&session_id) {
                continue;
            }

            let last_modified = std::fs::metadata(&jsonl_path)
                .ok()
                .and_then(|m| m.modified().ok());

            let recently_modified = last_modified.is_some_and(|t| {
                t.elapsed().unwrap_or(Duration::from_secs(999)) < Duration::from_secs(5)
            });

            // Determine status
            let (status, input_reason) = if let Some(perm) = perm_map.get(&session_id) {
                (
                    SessionStatus::NeedsInput,
                    Some(InputReason::Permission(perm.clone())),
                )
            } else if jsonl_status.has_pending_question {
                (
                    SessionStatus::NeedsInput,
                    Some(InputReason::Question {
                        text: jsonl_status
                            .question_text
                            .unwrap_or_else(|| "Agent has a question".to_string()),
                    }),
                )
            } else if recently_modified {
                (SessionStatus::Working, None)
            } else {
                (SessionStatus::Idle, None)
            };

            let project_name = project_name_from_cwd(&proc_info.cwd);

            seen_sessions.insert(
                session_id.clone(),
                Session {
                    session_id,
                    pid: proc_info.pid,
                    pty: proc_info.pty.clone(),
                    cwd: proc_info.cwd.clone(),
                    project_name,
                    branch: jsonl_status.git_branch,
                    status,
                    input_reason,
                    jsonl_path,
                    last_jsonl_modified: last_modified,
                    ended_at: None,
                },
            );
        }

        // Handle sessions that disappeared: mark as Ended
        for (sid, existing) in &self.sessions {
            if !seen_sessions.contains_key(sid) && existing.status != SessionStatus::Ended {
                let mut ended = existing.clone();
                ended.status = SessionStatus::Ended;
                ended.ended_at = Some(existing.ended_at.unwrap_or_else(Instant::now));
                seen_sessions.insert(sid.clone(), ended);
            }
        }

        // Remove sessions that have been ended for >5 seconds
        seen_sessions.retain(|_, s| {
            if let Some(ended_at) = s.ended_at {
                ended_at.elapsed() < Duration::from_secs(5)
            } else {
                true
            }
        });

        self.sessions = seen_sessions;
    }

    /// Get sessions sorted by status priority (red first, then yellow, then green).
    pub fn sorted_sessions(&self) -> Vec<&Session> {
        let mut sessions: Vec<&Session> = self.sessions.values().collect();
        sessions.sort_by_key(|s| s.status.sort_key());
        sessions
    }
}
```

**Step 2: Run all tests to make sure nothing broke**

Run: `cargo test --lib`
Expected: all tests pass

**Step 3: Commit**

```bash
git add src/monitor.rs
git commit -m "feat: add SessionMonitor combining process, JSONL, and IPC"
```

---

### Task 7: GUI app shell with frameless window

**Files:**
- Modify: `src/main.rs`
- Modify: `src/app.rs`

**Step 1: Implement the eframe app shell**

Replace `src/main.rs`:
```rust
mod app;
mod ipc;
mod monitor;
mod session;

use app::AgentDashApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_inner_size([260.0, 200.0])
            .with_min_inner_size([260.0, 60.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Agent Dash",
        options,
        Box::new(|cc| Ok(Box::new(AgentDashApp::new(cc)))),
    )
}
```

Write `src/app.rs`:
```rust
use crate::ipc::{self, PermissionDecision, PermissionResponse};
use crate::monitor::SessionMonitor;
use crate::session::{InputReason, SessionStatus};
use eframe::egui;
use std::time::{Duration, Instant};

pub struct AgentDashApp {
    monitor: SessionMonitor,
    last_refresh: Instant,
    expanded_session: Option<String>,
    dragging: bool,
    drag_offset: egui::Pos2,
}

impl AgentDashApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            monitor: SessionMonitor::new(),
            last_refresh: Instant::now() - Duration::from_secs(10), // force immediate refresh
            expanded_session: None,
            dragging: false,
            drag_offset: egui::Pos2::ZERO,
        }
    }
}

impl eframe::App for AgentDashApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Refresh session data every 2 seconds
        if self.last_refresh.elapsed() > Duration::from_secs(2) {
            self.monitor.refresh();
            self.last_refresh = Instant::now();
        }

        // Request repaint periodically to keep status up to date
        ctx.request_repaint_after(Duration::from_secs(1));

        // Handle window dragging
        let panel_frame = egui::Frame::none()
            .fill(egui::Color32::from_rgba_unmultiplied(30, 30, 30, 217)) // ~0.85 alpha
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::same(8));

        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                // Drag handling: drag anywhere on the panel
                let resp = ui.interact(
                    ui.max_rect(),
                    ui.id().with("drag"),
                    egui::Sense::drag(),
                );
                if resp.drag_started() {
                    self.dragging = true;
                }
                if resp.dragged() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if resp.drag_stopped() {
                    self.dragging = false;
                }

                let sessions = self.monitor.sorted_sessions();

                if sessions.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_gray(128),
                        "No active Claude sessions",
                    );
                    return;
                }

                for session in sessions {
                    let (color, dot) = match session.status {
                        SessionStatus::NeedsInput => (egui::Color32::from_rgb(255, 80, 80), "\u{1F534}"),
                        SessionStatus::Working => (egui::Color32::from_rgb(255, 200, 50), "\u{1F7E1}"),
                        SessionStatus::Idle => (egui::Color32::from_rgb(80, 200, 80), "\u{1F7E2}"),
                        SessionStatus::Ended => (egui::Color32::from_gray(128), "\u{26AA}"),
                    };

                    let label_text = format!("{} {}", dot, session.label());
                    let is_expanded = self.expanded_session.as_ref() == Some(&session.session_id);

                    // Pill background
                    let pill_frame = egui::Frame::none()
                        .fill(egui::Color32::from_rgba_unmultiplied(50, 50, 50, 200))
                        .rounding(egui::Rounding::same(6.0))
                        .inner_margin(egui::Margin::same(6));

                    pill_frame.show(ui, |ui| {
                        let label_resp = ui.add(
                            egui::Label::new(
                                egui::RichText::new(&label_text)
                                    .color(color)
                                    .size(13.0),
                            )
                            .sense(egui::Sense::click()),
                        );

                        if label_resp.clicked() {
                            if is_expanded {
                                self.expanded_session = None;
                            } else if session.input_reason.is_some() {
                                self.expanded_session = Some(session.session_id.clone());
                            }
                            // For non-input sessions, clicking could focus terminal (future)
                        }

                        // Expanded section
                        if is_expanded {
                            if let Some(reason) = &session.input_reason {
                                ui.separator();
                                match reason {
                                    InputReason::Permission(req) => {
                                        // Show tool + command
                                        let detail = if let Some(cmd) = req.input.get("command") {
                                            format!("{}: {}", req.tool, cmd.as_str().unwrap_or("?"))
                                        } else {
                                            format!("{}: {:?}", req.tool, req.input)
                                        };
                                        ui.label(
                                            egui::RichText::new(&detail)
                                                .size(11.0)
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        ui.horizontal(|ui| {
                                            if ui
                                                .button(egui::RichText::new("Allow").size(11.0))
                                                .clicked()
                                            {
                                                let _ = ipc::write_permission_response(
                                                    &session.session_id,
                                                    &PermissionResponse {
                                                        decision: PermissionDecision {
                                                            behavior: "allow".to_string(),
                                                            message: None,
                                                        },
                                                    },
                                                );
                                                self.expanded_session = None;
                                            }
                                            if ui
                                                .button(egui::RichText::new("Similar").size(11.0))
                                                .clicked()
                                            {
                                                let _ = ipc::write_permission_response(
                                                    &session.session_id,
                                                    &PermissionResponse {
                                                        decision: PermissionDecision {
                                                            behavior: "allow".to_string(),
                                                            // updatedPermissions would go here
                                                            message: None,
                                                        },
                                                    },
                                                );
                                                self.expanded_session = None;
                                            }
                                            if ui
                                                .button(egui::RichText::new("Deny").size(11.0))
                                                .clicked()
                                            {
                                                let _ = ipc::write_permission_response(
                                                    &session.session_id,
                                                    &PermissionResponse {
                                                        decision: PermissionDecision {
                                                            behavior: "deny".to_string(),
                                                            message: Some(
                                                                "Denied from dashboard".into(),
                                                            ),
                                                        },
                                                    },
                                                );
                                                self.expanded_session = None;
                                            }
                                        });
                                    }
                                    InputReason::Question { text } => {
                                        ui.label(
                                            egui::RichText::new(text)
                                                .size(11.0)
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        if ui
                                            .button(
                                                egui::RichText::new("Go to terminal").size(11.0),
                                            )
                                            .clicked()
                                        {
                                            // TODO: focus terminal window
                                            self.expanded_session = None;
                                        }
                                    }
                                }
                            }
                        }
                    });

                    ui.add_space(2.0);
                }
            });
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add src/main.rs src/app.rs
git commit -m "feat: add frameless overlay GUI with pill stack rendering"
```

---

### Task 8: Hook script

**Files:**
- Create: `hooks/permission-bridge.sh`

**Step 1: Write the hook script**

```bash
#!/usr/bin/env bash
# permission-bridge.sh — PermissionRequest hook for agent-dash
# Reads tool info from stdin, writes to IPC dir, polls for response.

set -euo pipefail

IPC_BASE="${XDG_CACHE_HOME:-$HOME/.cache}/agent-dash/sessions"
INPUT=$(cat)

# Extract fields from the hook's JSON input
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')
TOOL=$(echo "$INPUT" | jq -r '.tool // empty')
TOOL_INPUT=$(echo "$INPUT" | jq -c '.tool_input // {}')

if [ -z "$SESSION_ID" ]; then
    # No session ID — can't bridge, fall through to normal prompt
    exit 0
fi

SESSION_DIR="$IPC_BASE/$SESSION_ID"
mkdir -p "$SESSION_DIR"

PENDING="$SESSION_DIR/pending-permission.json"
RESPONSE="$SESSION_DIR/permission-response.json"

# Clean up any stale response file
rm -f "$RESPONSE"

# Write the pending permission request
TIMESTAMP=$(date +%s)
jq -n \
    --arg sid "$SESSION_ID" \
    --arg tool "$TOOL" \
    --argjson input "$TOOL_INPUT" \
    --arg ts "$TIMESTAMP" \
    '{session_id: $sid, tool: $tool, input: $input, timestamp: ($ts | tonumber)}' \
    > "$PENDING"

# Poll for response (200ms intervals, 120s timeout = 600 iterations)
for i in $(seq 1 600); do
    if [ -f "$RESPONSE" ]; then
        # Read the response and format it for Claude's hook protocol
        DECISION=$(cat "$RESPONSE")
        rm -f "$PENDING" "$RESPONSE"

        BEHAVIOR=$(echo "$DECISION" | jq -r '.decision.behavior // "allow"')
        MESSAGE=$(echo "$DECISION" | jq -r '.decision.message // empty')

        if [ "$BEHAVIOR" = "deny" ]; then
            jq -n \
                --arg msg "${MESSAGE:-Denied from dashboard}" \
                '{
                    "hookSpecificOutput": {
                        "hookEventName": "PermissionRequest",
                        "decision": {
                            "behavior": "deny",
                            "message": $msg
                        }
                    }
                }'
        else
            jq -n '{
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "allow"
                    }
                }
            }'
        fi
        exit 0
    fi
    sleep 0.2
done

# Timeout — clean up and fall through to normal prompt
rm -f "$PENDING"
exit 0
```

**Step 2: Make it executable**

Run: `chmod +x hooks/permission-bridge.sh`

**Step 3: Commit**

```bash
git add hooks/permission-bridge.sh
git commit -m "feat: add PermissionRequest hook script for dashboard bridge"
```

---

### Task 9: Integration test — run the app

**Step 1: Run cargo test for all unit tests**

Run: `cargo test`
Expected: all tests pass

**Step 2: Run the app**

Run: `cargo run`
Expected: A frameless overlay window appears on screen. If no claude sessions are running, it shows "No active Claude sessions". If sessions exist, they appear as colored pills.

**Step 3: Commit any fixups needed**

---

### Task 10: Install the hook globally

**Step 1: Show the user how to install the hook**

The user needs to add this to their `~/.claude/settings.json`:
```json
{
  "hooks": {
    "PermissionRequest": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "/home/mfeinber/src/rust/agent-dash/hooks/permission-bridge.sh"
          }
        ]
      }
    ]
  }
}
```

**Step 2: Test end-to-end with a real Claude session**

Start a Claude session in another terminal, trigger a permission prompt, verify the dashboard shows it as red with Allow/Deny buttons.
