# Message Fetching & Streaming API — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `get_messages`, `watch_session`, `unwatch_session`, and `list_sessions` protocol requests to the daemon so clients can fetch and stream formatted conversation content.

**Architecture:** New `messages` module in agent-dashd parses JSONL into structured messages with markdown/HTML rendering via comrak. New `watcher` module uses the `notify` crate to watch JSONL files for changes and push new messages to subscribed clients. Both plug into the existing main-loop channel pattern.

**Tech Stack:** Rust, tokio, notify (file watching), comrak (GFM markdown-to-HTML)

**Design doc:** `docs/plans/2026-02-15-message-api-design.md`

---

### Task 1: Add Dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root, line 10-16)
- Modify: `crates/agent-dashd/Cargo.toml` (line 6-13)

**Step 1: Add notify and comrak to workspace dependencies**

In `Cargo.toml` (workspace root), add to `[workspace.dependencies]`:

```toml
notify = "7"
comrak = { version = "0.36", default-features = false }
```

**Step 2: Add notify and comrak to agent-dashd dependencies**

In `crates/agent-dashd/Cargo.toml`, add to `[dependencies]`:

```toml
notify = { workspace = true }
comrak = { workspace = true }
```

**Step 3: Verify it compiles**

Run: `cargo build -p agent-dashd 2>&1`
Expected: compiles with no errors (warnings OK)

**Step 4: Commit**

```bash
git add Cargo.toml crates/agent-dashd/Cargo.toml Cargo.lock
git commit -m "build: add notify and comrak dependencies"
```

---

### Task 2: Add Protocol Types

Add the new request/response types to the shared protocol crate so both daemon and clients can use them.

**Files:**
- Modify: `crates/agent-dash-core/src/protocol.rs` (lines 39-82)

**Step 1: Write tests for new request deserialization**

Add these tests to the existing `mod tests` block in `crates/agent-dash-core/src/protocol.rs`:

```rust
#[test]
fn deserialize_get_messages() {
    let json = r#"{"method":"get_messages","session_id":"s1","format":"html","limit":20}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    match req {
        ClientRequest::GetMessages { session_id, format, limit } => {
            assert_eq!(session_id, "s1");
            assert_eq!(format.as_deref(), Some("html"));
            assert_eq!(limit, Some(20));
        }
        _ => panic!("expected GetMessages"),
    }
}

#[test]
fn deserialize_get_messages_defaults() {
    let json = r#"{"method":"get_messages","session_id":"s1"}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    match req {
        ClientRequest::GetMessages { format, limit, .. } => {
            assert!(format.is_none());
            assert!(limit.is_none());
        }
        _ => panic!("expected GetMessages"),
    }
}

#[test]
fn deserialize_watch_session() {
    let json = r#"{"method":"watch_session","session_id":"s1","format":"markdown"}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    assert!(matches!(req, ClientRequest::WatchSession { .. }));
}

#[test]
fn deserialize_unwatch_session() {
    let json = r#"{"method":"unwatch_session","session_id":"s1"}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    assert!(matches!(req, ClientRequest::UnwatchSession { .. }));
}

#[test]
fn deserialize_list_sessions() {
    let json = r#"{"method":"list_sessions","project":"traider"}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    match req {
        ClientRequest::ListSessions { project } => assert_eq!(project, "traider"),
        _ => panic!("expected ListSessions"),
    }
}

#[test]
fn serialize_messages_event() {
    let msg = ChatMessage {
        role: "assistant".into(),
        content: ChatContent::Structured(vec![
            ContentBlock::Text { text: "hello".into() },
        ]),
    };
    let event = ServerEvent::Messages {
        session_id: "s1".into(),
        messages: vec![msg],
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"event\":\"messages\""));
    assert!(json.contains("\"role\":\"assistant\""));
}

#[test]
fn serialize_message_event() {
    let msg = ChatMessage {
        role: "user".into(),
        content: ChatContent::Rendered("hello".into()),
    };
    let event = ServerEvent::Message {
        session_id: "s1".into(),
        message: msg,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"event\":\"message\""));
}

#[test]
fn serialize_session_list() {
    let entry = SessionListEntry {
        session_id: "abc".into(),
        main: true,
        modified: 1000,
    };
    let event = ServerEvent::SessionList {
        project: "traider".into(),
        sessions: vec![entry],
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"main\":true"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dash-core 2>&1`
Expected: compilation errors — the new types don't exist yet.

**Step 3: Add the new variants to ClientRequest**

In `crates/agent-dash-core/src/protocol.rs`, add these variants to the `ClientRequest` enum (after `PermissionRequest`):

```rust
#[serde(rename = "get_messages")]
GetMessages {
    session_id: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
},
#[serde(rename = "watch_session")]
WatchSession {
    session_id: String,
    #[serde(default)]
    format: Option<String>,
},
#[serde(rename = "unwatch_session")]
UnwatchSession {
    session_id: String,
},
#[serde(rename = "list_sessions")]
ListSessions {
    project: String,
},
```

**Step 4: Add the new variants to ServerEvent and supporting types**

Add these variants to the `ServerEvent` enum (after `PermissionResolved`):

```rust
#[serde(rename = "messages")]
Messages {
    session_id: String,
    messages: Vec<ChatMessage>,
},
#[serde(rename = "message")]
Message {
    session_id: String,
    message: ChatMessage,
},
#[serde(rename = "session_list")]
SessionList {
    project: String,
    sessions: Vec<SessionListEntry>,
},
```

Add these new types before the `ServerEvent` enum (after the `HookPermissionDecision` struct, before the line-delimited JSON helpers section):

```rust
// ---------------------------------------------------------------------------
// Chat message types (for message API)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: ChatContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatContent {
    /// Structured format: array of typed content blocks.
    Structured(Vec<ContentBlock>),
    /// Rendered format (markdown or html): single string.
    Rendered(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        detail: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListEntry {
    pub session_id: String,
    pub main: bool,
    pub modified: u64,
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p agent-dash-core 2>&1`
Expected: all tests pass including the new ones.

**Step 6: Commit**

```bash
git add crates/agent-dash-core/src/protocol.rs
git commit -m "feat(protocol): add message API request/response types"
```

---

### Task 3: Message Parser Module

New module that parses JSONL lines into `ChatMessage` structs, with rendering to markdown and HTML.

**Files:**
- Create: `crates/agent-dashd/src/messages.rs`
- Modify: `crates/agent-dashd/src/lib.rs` (add `pub mod messages;`)

**Step 1: Write tests**

Create `crates/agent-dashd/src/messages.rs` with the test module first:

```rust
use agent_dash_core::protocol::{ChatContent, ChatMessage, ContentBlock};
use serde::Deserialize;
use std::path::Path;

/// A raw JSONL entry. We only care about assistant and user types.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum RawEntry {
    #[serde(rename = "assistant")]
    Assistant {
        message: Option<RawMessage>,
    },
    #[serde(rename = "user")]
    User {
        message: Option<RawMessage>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    content: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_assistant_text_only() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello world"}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "assistant");
        match &msgs[0].content {
            ChatContent::Structured(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Hello world"));
            }
            _ => panic!("expected structured"),
        }
    }

    #[test]
    fn parse_user_text() {
        let line = r#"{"type":"user","message":{"content":"What is 2+2?"}}"#;
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn parse_user_tool_result() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"ok"}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Structured(blocks) => {
                assert!(matches!(&blocks[0], ContentBlock::ToolResult { .. }));
            }
            _ => panic!("expected structured"),
        }
    }

    #[test]
    fn parse_tool_use() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Structured(blocks) => {
                match &blocks[0] {
                    ContentBlock::ToolUse { name, detail, .. } => {
                        assert_eq!(name, "Bash");
                        assert_eq!(detail, "ls");
                    }
                    _ => panic!("expected ToolUse"),
                }
            }
            _ => panic!("expected structured"),
        }
    }

    #[test]
    fn parse_skips_other_types() {
        let lines = vec![
            r#"{"type":"file-history-snapshot"}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#.to_string(),
        ];
        let msgs = parse_lines(&lines, "structured");
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn format_markdown() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"**bold**"},{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls -la"}}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "markdown");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Rendered(text) => {
                assert!(text.contains("**bold**"));
                assert!(text.contains("Bash"));
                assert!(text.contains("ls -la"));
            }
            _ => panic!("expected rendered"),
        }
    }

    #[test]
    fn format_html() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"**bold**"}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "html");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Rendered(html) => {
                assert!(html.contains("<strong>bold</strong>"));
            }
            _ => panic!("expected rendered"),
        }
    }

    #[test]
    fn read_tail_messages_from_file() {
        let dir = std::env::temp_dir().join("agent-dash-test-messages");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.jsonl");
        let content = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"msg1"}]}}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"msg2"}]}}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"msg3"}]}}"#, "\n",
        );
        std::fs::write(&path, content).unwrap();
        let msgs = read_messages(&path, 2, "structured");
        assert_eq!(msgs.len(), 2);
        // Should return the LAST 2 messages
        match &msgs[0].content {
            ChatContent::Structured(blocks) => {
                assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "msg2"));
            }
            _ => panic!("expected structured"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dashd messages 2>&1`
Expected: compilation errors — functions don't exist yet.

**Step 3: Implement the module**

Add `pub mod messages;` to `crates/agent-dashd/src/lib.rs`.

Then implement the functions in `crates/agent-dashd/src/messages.rs` above the test module:

```rust
use agent_dash_core::protocol::{ChatContent, ChatMessage, ContentBlock};
use serde::Deserialize;
use std::path::Path;

/// A raw JSONL entry. We only care about assistant and user types.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum RawEntry {
    #[serde(rename = "assistant")]
    Assistant { message: Option<RawMessage> },
    #[serde(rename = "user")]
    User { message: Option<RawMessage> },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    content: Option<serde_json::Value>,
}

/// Parse a list of JSONL lines into ChatMessages in the given format.
/// `format` is one of "structured", "markdown", "html".
pub fn parse_lines(lines: &[String], format: &str) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    for line in lines {
        let Ok(entry) = serde_json::from_str::<RawEntry>(line) else {
            continue;
        };

        let (role, raw_msg) = match entry {
            RawEntry::Assistant { message } => ("assistant", message),
            RawEntry::User { message } => ("user", message),
            RawEntry::Other => continue,
        };

        let Some(raw) = raw_msg else { continue };
        let Some(content_val) = raw.content else { continue };

        let blocks = parse_content_blocks(role, &content_val);
        if blocks.is_empty() {
            continue;
        }

        let content = match format {
            "markdown" => ChatContent::Rendered(render_markdown(&blocks)),
            "html" => ChatContent::Rendered(render_html(&blocks)),
            _ => ChatContent::Structured(blocks),
        };

        messages.push(ChatMessage {
            role: role.to_string(),
            content,
        });
    }

    messages
}

/// Read the last `limit` conversation messages from a JSONL file.
pub fn read_messages(path: &Path, limit: usize, format: &str) -> Vec<ChatMessage> {
    let lines = read_tail_lines(path);
    let all = parse_lines(&lines, format);
    let start = all.len().saturating_sub(limit);
    all[start..].to_vec()
}

/// Read new lines from a JSONL file starting at byte `offset`.
/// Returns the parsed messages and the new offset.
pub fn read_new_messages(path: &Path, offset: u64, format: &str) -> (Vec<ChatMessage>, u64) {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return (vec![], offset);
    };
    let Ok(file_len) = file.seek(SeekFrom::End(0)) else {
        return (vec![], offset);
    };

    if file_len <= offset {
        return (vec![], offset);
    }

    let _ = file.seek(SeekFrom::Start(offset));
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);

    let lines: Vec<String> = buf.lines().filter(|l| !l.is_empty()).map(String::from).collect();
    let messages = parse_lines(&lines, format);

    (messages, file_len)
}

/// Extract tool detail string from a tool_use input object.
fn extract_tool_detail(name: &str, input: &serde_json::Value) -> String {
    let detail = match name {
        "Bash" => input.get("command").and_then(|v| v.as_str()),
        "Read" | "Edit" | "Write" => input.get("file_path").and_then(|v| v.as_str()),
        "Grep" => input.get("pattern").and_then(|v| v.as_str()),
        "Glob" => input.get("pattern").and_then(|v| v.as_str()),
        "WebFetch" => input.get("url").and_then(|v| v.as_str()),
        "WebSearch" => input.get("query").and_then(|v| v.as_str()),
        "Task" => input.get("description").and_then(|v| v.as_str()),
        _ => None,
    };
    detail.unwrap_or("").to_string()
}

/// Parse raw JSON content into ContentBlocks.
fn parse_content_blocks(role: &str, content: &serde_json::Value) -> Vec<ContentBlock> {
    // User messages can be a plain string.
    if let Some(text) = content.as_str() {
        if !text.trim().is_empty() {
            return vec![ContentBlock::Text {
                text: text.to_string(),
            }];
        }
        return vec![];
    }

    let Some(arr) = content.as_array() else {
        return vec![];
    };

    let mut blocks = Vec::new();

    for block in arr {
        let Some(block_type) = block.get("type").and_then(|t| t.as_str()) else {
            continue;
        };

        match block_type {
            "text" => {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    if !text.trim().is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            "tool_use" => {
                let name = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let input = block.get("input").cloned();
                let detail = input
                    .as_ref()
                    .map(|i| extract_tool_detail(&name, i))
                    .unwrap_or_default();
                blocks.push(ContentBlock::ToolUse {
                    name,
                    detail,
                    input,
                });
            }
            "tool_result" if role == "user" => {
                // Try to extract the tool name — not present in the JSONL,
                // so we leave it as "tool_result" for now.
                let output = match block.get("content") {
                    Some(serde_json::Value::String(s)) => Some(s.clone()),
                    Some(serde_json::Value::Array(arr)) => {
                        // Extract text from content blocks.
                        let texts: Vec<&str> = arr
                            .iter()
                            .filter_map(|b| {
                                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                                    b.get("text").and_then(|t| t.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if texts.is_empty() {
                            None
                        } else {
                            Some(texts.join("\n"))
                        }
                    }
                    _ => None,
                };
                blocks.push(ContentBlock::ToolResult {
                    name: "tool_result".into(),
                    output,
                });
            }
            _ => {}
        }
    }

    blocks
}

/// Render content blocks as markdown text.
fn render_markdown(blocks: &[ContentBlock]) -> String {
    let mut parts = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text { text } => parts.push(text.clone()),
            ContentBlock::ToolUse { name, detail, .. } => {
                if detail.is_empty() {
                    parts.push(format!("> **{name}**"));
                } else {
                    parts.push(format!("> **{name}**: `{detail}`"));
                }
            }
            ContentBlock::ToolResult { output, .. } => {
                if let Some(out) = output {
                    let truncated = if out.len() > 500 {
                        format!("{}...", &out[..500])
                    } else {
                        out.clone()
                    };
                    parts.push(format!("> ```\n> {}\n> ```", truncated.replace('\n', "\n> ")));
                }
            }
        }
    }

    parts.join("\n\n")
}

/// Render content blocks as HTML via comrak.
fn render_html(blocks: &[ContentBlock]) -> String {
    let md = render_markdown(blocks);
    comrak::markdown_to_html(&md, &comrak::Options::default())
}

/// Read lines from the tail of a file (last 128KB).
fn read_tail_lines(path: &Path) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return vec![];
    };
    let Ok(file_len) = file.seek(SeekFrom::End(0)) else {
        return vec![];
    };
    if file_len == 0 {
        return vec![];
    }

    let read_size = file_len.min(128 * 1024);
    let _ = file.seek(SeekFrom::End(-(read_size as i64)));
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);
    buf.lines().filter(|l| !l.is_empty()).map(String::from).collect()
}
```

**Step 4: Run tests**

Run: `cargo test -p agent-dashd messages 2>&1`
Expected: all tests pass.

**Step 5: Commit**

```bash
git add crates/agent-dashd/src/messages.rs crates/agent-dashd/src/lib.rs
git commit -m "feat(daemon): add message parser with structured/markdown/html output"
```

---

### Task 4: File Watcher Module

New module that watches JSONL files using the `notify` crate and sends change events to the main loop.

**Files:**
- Create: `crates/agent-dashd/src/watcher.rs`
- Modify: `crates/agent-dashd/src/lib.rs` (add `pub mod watcher;`)

**Step 1: Write tests**

Create `crates/agent-dashd/src/watcher.rs` with a test that validates the core watcher logic:

```rust
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Event sent from the watcher to the main loop.
#[derive(Debug)]
pub struct FileChanged {
    pub session_id: String,
    pub path: PathBuf,
}

/// Manages file watches for session JSONL files.
pub struct SessionWatcher {
    _watcher: RecommendedWatcher,
    /// Maps watched file paths to session IDs.
    path_to_session: HashMap<PathBuf, String>,
    tx: mpsc::Sender<FileChanged>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn watcher_detects_file_change() {
        let dir = std::env::temp_dir().join("agent-dash-test-watcher");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.jsonl");
        std::fs::write(&path, "initial\n").unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let mut watcher = SessionWatcher::new(tx).unwrap();
        watcher.watch("s1", &path).unwrap();

        // Give the watcher time to register.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Append to trigger a notify event.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "new line").unwrap();

        // We should receive a FileChanged event.
        let event = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx.recv(),
        ).await;
        assert!(event.is_ok());
        let event = event.unwrap().unwrap();
        assert_eq!(event.session_id, "s1");

        watcher.unwatch("s1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn unwatch_removes_watch() {
        let dir = std::env::temp_dir().join("agent-dash-test-watcher-remove");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.jsonl");
        std::fs::write(&path, "initial\n").unwrap();

        let (tx, _rx) = mpsc::channel(16);
        let mut watcher = SessionWatcher::new(tx).unwrap();
        watcher.watch("s1", &path).unwrap();
        assert!(watcher.is_watching("s1"));
        watcher.unwatch("s1");
        assert!(!watcher.is_watching("s1"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dashd watcher 2>&1`
Expected: compilation errors — `new`, `watch`, `unwatch`, `is_watching` don't exist yet.

**Step 3: Implement SessionWatcher**

Replace the placeholder struct impls in `crates/agent-dashd/src/watcher.rs` with:

```rust
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Event sent from the watcher to the main loop.
#[derive(Debug)]
pub struct FileChanged {
    pub session_id: String,
    pub path: PathBuf,
}

/// Manages file watches for session JSONL files.
pub struct SessionWatcher {
    _watcher: RecommendedWatcher,
    /// Maps session ID to watched path.
    session_to_path: HashMap<String, PathBuf>,
    /// Maps watched file paths to session IDs.
    path_to_session: HashMap<PathBuf, String>,
    tx: mpsc::Sender<FileChanged>,
}

impl SessionWatcher {
    /// Create a new watcher that sends FileChanged events on the given channel.
    pub fn new(tx: mpsc::Sender<FileChanged>) -> notify::Result<Self> {
        let tx_clone = tx.clone();
        // We need a second map reference for the callback — use a channel-based
        // approach: the callback sends raw paths, and we map them in the main struct.
        // But notify callbacks are Fn, not async, so we use a std mpsc internally.
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<PathBuf>();

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    for path in event.paths {
                        let _ = raw_tx.send(path);
                    }
                }
            }
        })?;

        let mut sw = Self {
            _watcher: watcher,
            session_to_path: HashMap::new(),
            path_to_session: HashMap::new(),
            tx,
        };

        // Spawn a task to bridge the sync notify callback to our async channel.
        let path_to_session = sw.path_to_session.clone();
        let tx_bridge = sw.tx.clone();
        // We can't share the HashMap with the thread, so we use a different
        // approach: store the mapping and have a bridge task.
        // Actually, we'll use a simpler design — the bridge forwards raw paths,
        // and the main loop does the mapping. Let's keep it clean.

        // Instead: just forward the raw path and let the caller map it.
        // We'll store a "raw" channel and provide a recv method.
        // Let me restructure...

        drop(sw);
        drop(path_to_session);
        drop(tx_bridge);

        Self::new_internal(tx, raw_rx)
    }

    fn new_internal(
        tx: mpsc::Sender<FileChanged>,
        raw_rx: std::sync::mpsc::Receiver<PathBuf>,
    ) -> notify::Result<Self> {
        // Recreate the watcher since we dropped the previous one.
        let (raw_tx, _) = std::sync::mpsc::channel::<PathBuf>();
        // This design is getting tangled. Let's use a cleaner approach.
        todo!()
    }
}
```

**Wait — the above is getting tangled.** Let me simplify. The cleanest approach: the `SessionWatcher` owns the `notify` watcher and a path→session map. Since `notify`'s callback is `Fn + Send`, we can't share the HashMap directly. Instead, we use a two-stage channel design:

1. `notify` callback sends raw `PathBuf` events to a `std::sync::mpsc` channel.
2. A tokio task reads from that channel, maps paths to session IDs using a shared `Arc<Mutex<HashMap>>`, and forwards `FileChanged` events to the main loop.

Here's the clean implementation:

```rust
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Event sent from the watcher to the main loop.
#[derive(Debug)]
pub struct FileChanged {
    pub session_id: String,
    pub path: PathBuf,
}

/// Manages file watches for session JSONL files.
pub struct SessionWatcher {
    watcher: RecommendedWatcher,
    session_to_path: HashMap<String, PathBuf>,
    path_to_session: Arc<Mutex<HashMap<PathBuf, String>>>,
}

impl SessionWatcher {
    /// Create a new watcher. Spawns a background tokio task that bridges
    /// notify's sync callback to the async `tx` channel.
    pub fn new(tx: mpsc::Sender<FileChanged>) -> notify::Result<Self> {
        let path_to_session: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let map_clone = path_to_session.clone();

        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<PathBuf>();

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                    for path in event.paths {
                        let _ = raw_tx.send(path);
                    }
                }
            }
        })?;

        // Bridge task: map raw paths to session IDs and forward.
        tokio::spawn(async move {
            while let Some(path) = raw_rx.recv().await {
                let session_id = {
                    let map = map_clone.lock().unwrap();
                    map.get(&path).cloned()
                };
                if let Some(session_id) = session_id {
                    let _ = tx.send(FileChanged {
                        session_id,
                        path,
                    }).await;
                }
            }
        });

        Ok(Self {
            watcher,
            session_to_path: HashMap::new(),
            path_to_session,
        })
    }

    /// Start watching a JSONL file for a session.
    pub fn watch(&mut self, session_id: &str, path: &PathBuf) -> notify::Result<()> {
        self.watcher.watch(path, RecursiveMode::NonRecursive)?;
        self.session_to_path
            .insert(session_id.to_string(), path.clone());
        self.path_to_session
            .lock()
            .unwrap()
            .insert(path.clone(), session_id.to_string());
        Ok(())
    }

    /// Stop watching a session's JSONL file.
    pub fn unwatch(&mut self, session_id: &str) {
        if let Some(path) = self.session_to_path.remove(session_id) {
            let _ = self.watcher.unwatch(&path);
            self.path_to_session.lock().unwrap().remove(&path);
        }
    }

    /// Check if a session is currently being watched.
    pub fn is_watching(&self, session_id: &str) -> bool {
        self.session_to_path.contains_key(session_id)
    }
}
```

**Step 4: Add to lib.rs and run tests**

Add `pub mod watcher;` to `crates/agent-dashd/src/lib.rs`.

Run: `cargo test -p agent-dashd watcher 2>&1`
Expected: all tests pass.

**Step 5: Commit**

```bash
git add crates/agent-dashd/src/watcher.rs crates/agent-dashd/src/lib.rs
git commit -m "feat(daemon): add notify-based JSONL file watcher"
```

---

### Task 5: Wire Up ClientMessage Variants and Client Listener

Add the new `ClientMessage` variants and handle them in the client listener's connection handler.

**Files:**
- Modify: `crates/agent-dashd/src/client_listener.rs` (lines 11-34 and 106-168)

**Step 1: Add new ClientMessage variants**

Add these variants to the `ClientMessage` enum in `crates/agent-dashd/src/client_listener.rs`:

```rust
/// Client requests last N messages from a session.
GetMessages {
    session_id: String,
    format: String,
    limit: usize,
    reply: oneshot::Sender<String>,
},
/// Client wants to stream new messages from a session.
WatchSession {
    session_id: String,
    format: String,
    tx: mpsc::Sender<String>,
},
/// Client stops streaming messages from a session.
UnwatchSession {
    session_id: String,
},
/// Client requests all sessions for a project.
ListSessions {
    project: String,
    reply: oneshot::Sender<String>,
},
```

**Step 2: Handle new requests in handle_client_connection**

Add these match arms after the existing `ClientRequest::PermissionRequest` arm in the `handle_client_connection` function:

```rust
ClientRequest::GetMessages {
    session_id,
    format,
    limit,
} => {
    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = tx
        .send(ClientMessage::GetMessages {
            session_id,
            format: format.unwrap_or_else(|| "structured".into()),
            limit: limit.unwrap_or(50),
            reply: reply_tx,
        })
        .await;
    if let Ok(json) = reply_rx.await {
        let _ = writer.write_all(json.as_bytes()).await;
    }
}
ClientRequest::WatchSession {
    session_id,
    format,
} => {
    let (sub_tx, mut sub_rx) = mpsc::channel::<String>(64);
    let _ = tx
        .send(ClientMessage::WatchSession {
            session_id: session_id.clone(),
            format: format.unwrap_or_else(|| "structured".into()),
            tx: sub_tx,
        })
        .await;

    // Stream messages until disconnect.
    while let Some(msg) = sub_rx.recv().await {
        if writer.write_all(msg.as_bytes()).await.is_err() {
            break;
        }
    }

    // Clean up the watch on disconnect.
    let _ = tx
        .send(ClientMessage::UnwatchSession {
            session_id,
        })
        .await;
    return;
}
ClientRequest::UnwatchSession { session_id } => {
    let _ = tx
        .send(ClientMessage::UnwatchSession { session_id })
        .await;
}
ClientRequest::ListSessions { project } => {
    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = tx
        .send(ClientMessage::ListSessions {
            project,
            reply: reply_tx,
        })
        .await;
    if let Ok(json) = reply_rx.await {
        let _ = writer.write_all(json.as_bytes()).await;
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo build -p agent-dashd 2>&1`
Expected: compilation errors in `main.rs` about unhandled `ClientMessage` variants (expected — we handle those in Task 6).

**Step 4: Commit**

```bash
git add crates/agent-dashd/src/client_listener.rs
git commit -m "feat(daemon): add message API client message types and handling"
```

---

### Task 6: Wire Up Main Loop

Handle the new `ClientMessage` variants and `FileChanged` events in the main loop.

**Files:**
- Modify: `crates/agent-dashd/src/main.rs`

**Step 1: Add watcher state and FileChanged channel**

Near the top of `main()`, after the existing channel declarations (around line 28), add:

```rust
let (watch_tx, mut watch_rx) = mpsc::channel::<agent_dashd::watcher::FileChanged>(256);
let mut session_watcher = agent_dashd::watcher::SessionWatcher::new(watch_tx)
    .expect("failed to create file watcher");

// Per-session message subscribers: session_id -> Vec<(format, tx)>
let mut message_subscribers: HashMap<String, Vec<(String, mpsc::Sender<String>)>> =
    HashMap::new();
```

**Step 2: Add FileChanged handler in the select! loop**

Add a new arm to the `tokio::select!` block (after the scan interval arm):

```rust
// --- File change events (for watch_session subscribers) ---
Some(changed) = watch_rx.recv() => {
    if let Some(subs) = message_subscribers.get(&changed.session_id) {
        if !subs.is_empty() {
            // Read new content for each format requested.
            let session_offset = state.sessions.get(&changed.session_id)
                .and_then(|s| s.watch_offset);

            let offset = session_offset.unwrap_or(0);
            // Group subscribers by format to avoid redundant parsing.
            let mut by_format: HashMap<String, Vec<&mpsc::Sender<String>>> = HashMap::new();
            for (fmt, tx) in subs {
                by_format.entry(fmt.clone()).or_default().push(tx);
            }

            for (fmt, senders) in &by_format {
                let (msgs, new_offset) = messages::read_new_messages(
                    &changed.path, offset, fmt,
                );
                // Update stored offset.
                if let Some(session) = state.sessions.get_mut(&changed.session_id) {
                    session.watch_offset = Some(new_offset);
                }
                for msg in &msgs {
                    let event = ServerEvent::Message {
                        session_id: changed.session_id.clone(),
                        message: msg.clone(),
                    };
                    if let Ok(line) = protocol::encode_line(&event) {
                        for tx in senders {
                            let _ = tx.try_send(line.clone());
                        }
                    }
                }
            }
        }
    }
}
```

**Step 3: Handle new ClientMessage variants**

Add these to the `Some(msg) = client_rx.recv()` match block (after the existing `PermissionRequest` arm):

```rust
ClientMessage::GetMessages {
    session_id,
    format,
    limit,
    reply,
} => {
    let response = if let Some(session) = state.sessions.get(&session_id) {
        if let Some(ref jsonl) = session.jsonl_path {
            let path = std::path::PathBuf::from(jsonl);
            let msgs = messages::read_messages(&path, limit, &format);
            let event = ServerEvent::Messages {
                session_id,
                messages: msgs,
            };
            protocol::encode_line(&event).unwrap_or_default()
        } else {
            protocol::encode_line(&ServerEvent::Messages {
                session_id,
                messages: vec![],
            })
            .unwrap_or_default()
        }
    } else {
        protocol::encode_line(&ServerEvent::Messages {
            session_id,
            messages: vec![],
        })
        .unwrap_or_default()
    };
    let _ = reply.send(response);
}
ClientMessage::WatchSession {
    session_id,
    format,
    tx,
} => {
    // Start file watch if we have a JSONL path.
    if let Some(session) = state.sessions.get_mut(&session_id) {
        if let Some(ref jsonl) = session.jsonl_path {
            let path = std::path::PathBuf::from(jsonl);
            // Record current file size as offset so we only send new msgs.
            let file_len = std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(0);
            session.watch_offset = Some(file_len);
            let _ = session_watcher.watch(&session_id, &path);
        }
    }
    message_subscribers
        .entry(session_id)
        .or_default()
        .push((format, tx));
}
ClientMessage::UnwatchSession { session_id } => {
    message_subscribers.remove(&session_id);
    session_watcher.unwatch(&session_id);
    if let Some(session) = state.sessions.get_mut(&session_id) {
        session.watch_offset = None;
    }
}
ClientMessage::ListSessions { project, reply } => {
    let projects_dir = paths::claude_projects_dir();
    let slug = project.replace('/', "-").replace('\\', "-");
    let project_dir = projects_dir.join(&slug);

    let mut entries = Vec::new();
    if let Ok(dir_entries) = std::fs::read_dir(&project_dir) {
        // Find the main session's JSONL path from state.
        let main_jsonl: Option<String> = state
            .sessions
            .values()
            .find(|s| s.project_name == project)
            .and_then(|s| s.jsonl_path.clone());

        for entry in dir_entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            // Parse session ID from the file.
            let session_id = scanner::parse_jsonl_status(&path)
                .map(|s| s.session_id)
                .unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                });

            let is_main = main_jsonl
                .as_ref()
                .map(|p| p == &path.to_string_lossy().to_string())
                .unwrap_or(false);

            entries.push(agent_dash_core::protocol::SessionListEntry {
                session_id,
                main: is_main,
                modified,
            });
        }
    }

    entries.sort_by(|a, b| b.modified.cmp(&a.modified));

    let event = ServerEvent::SessionList {
        project,
        sessions: entries,
    };
    let _ = reply.send(protocol::encode_line(&event).unwrap_or_default());
}
```

**Step 4: Add `watch_offset` field to InternalSession**

In `crates/agent-dashd/src/state.rs`, add to the `InternalSession` struct:

```rust
pub watch_offset: Option<u64>,
```

And in the `ensure_session` default initialization, add:

```rust
watch_offset: None,
```

**Step 5: Add messages import to main.rs**

At the top of `crates/agent-dashd/src/main.rs`, add:

```rust
use agent_dashd::messages;
```

**Step 6: Verify it compiles and tests pass**

Run: `cargo build -p agent-dashd 2>&1`
Expected: clean compilation.

Run: `cargo test --workspace 2>&1`
Expected: all tests pass.

**Step 7: Commit**

```bash
git add crates/agent-dashd/src/main.rs crates/agent-dashd/src/state.rs
git commit -m "feat(daemon): wire up message API in main loop"
```

---

### Task 7: agentctl Subcommands

Add `messages` and `sessions` subcommands to agentctl for manual testing.

**Files:**
- Modify: `crates/agentctl/src/main.rs`

**Step 1: Add `messages` subcommand**

Add this match arm to the `main()` match block and the usage string:

```rust
"messages" => {
    let session_id = args.get(2).expect("usage: agentctl messages <session_id> [format] [limit]");
    let format = args.get(3).map(|s| s.as_str()).unwrap_or("structured");
    let limit: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
    cmd_messages(session_id, format, limit);
}
"sessions" => {
    let project = args.get(2).expect("usage: agentctl sessions <project_name>");
    cmd_sessions(project);
}
"watch-messages" => {
    let session_id = args.get(2).expect("usage: agentctl watch-messages <session_id> [format]");
    let format = args.get(3).map(|s| s.as_str()).unwrap_or("structured");
    cmd_watch_messages(session_id, format);
}
```

Update the usage line:

```rust
eprintln!("usage: agentctl <status|list|watch|messages|sessions|watch-messages|approve|approve-similar|deny>");
```

**Step 2: Implement the command functions**

Add these functions to `crates/agentctl/src/main.rs`:

```rust
/// Fetch and print last N messages for a session.
fn cmd_messages(session_id: &str, format: &str, limit: usize) {
    let mut conn = connect();
    let req = ClientRequest::GetMessages {
        session_id: session_id.to_string(),
        format: Some(format.to_string()),
        limit: Some(limit),
    };
    send_request(&mut conn, &req);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&line) else {
            continue;
        };
        if let ServerEvent::Messages { messages, .. } = event {
            if messages.is_empty() {
                println!("No messages found.");
                return;
            }
            for msg in &messages {
                println!("--- {} ---", msg.role);
                match &msg.content {
                    agent_dash_core::protocol::ChatContent::Structured(blocks) => {
                        for block in blocks {
                            match block {
                                agent_dash_core::protocol::ContentBlock::Text { text } => {
                                    println!("{text}");
                                }
                                agent_dash_core::protocol::ContentBlock::ToolUse {
                                    name, detail, ..
                                } => {
                                    println!("> {name}: {detail}");
                                }
                                agent_dash_core::protocol::ContentBlock::ToolResult {
                                    output, ..
                                } => {
                                    if let Some(out) = output {
                                        let display = truncate(out, 200);
                                        println!("> result: {display}");
                                    }
                                }
                            }
                        }
                    }
                    agent_dash_core::protocol::ChatContent::Rendered(text) => {
                        println!("{text}");
                    }
                }
            }
            return;
        }
    }
}

/// List all sessions for a project.
fn cmd_sessions(project: &str) {
    let mut conn = connect();
    let req = ClientRequest::ListSessions {
        project: project.to_string(),
    };
    send_request(&mut conn, &req);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&line) else {
            continue;
        };
        if let ServerEvent::SessionList {
            project, sessions, ..
        } = event
        {
            if sessions.is_empty() {
                println!("No sessions found for project '{project}'.");
                return;
            }
            for s in &sessions {
                let main_marker = if s.main { " (main)" } else { "" };
                println!("{}{main_marker}", truncate(&s.session_id, 8));
            }
            return;
        }
    }
}

/// Subscribe to live messages for a session.
fn cmd_watch_messages(session_id: &str, format: &str) {
    let mut conn = connect();
    let req = ClientRequest::WatchSession {
        session_id: session_id.to_string(),
        format: Some(format.to_string()),
    };
    send_request(&mut conn, &req);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        println!("{line}");
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo build --workspace 2>&1`
Expected: clean compilation.

**Step 4: Commit**

```bash
git add crates/agentctl/src/main.rs
git commit -m "feat(agentctl): add messages, sessions, and watch-messages commands"
```

---

### Task 8: Integration Test

End-to-end verification that the full pipeline works.

**Step 1: Build release binaries**

Run: `cargo build --workspace --release 2>&1`
Expected: clean build.

**Step 2: Run all unit tests**

Run: `cargo test --workspace 2>&1`
Expected: all tests pass.

**Step 3: Manual integration test**

Start the daemon:
```bash
cargo run -p agent-dashd --release &
```

Test `list` to verify existing functionality still works:
```bash
cargo run -p agentctl --release -- list
```

Test `messages` with a known session ID (get one from `list` output):
```bash
cargo run -p agentctl --release -- messages <session_id> structured 5
cargo run -p agentctl --release -- messages <session_id> markdown 5
cargo run -p agentctl --release -- messages <session_id> html 5
```

Test `sessions` with a project name:
```bash
cargo run -p agentctl --release -- sessions agent-dash
```

Test `watch-messages` (in another terminal, trigger activity, verify messages stream):
```bash
cargo run -p agentctl --release -- watch-messages <session_id> structured
```

**Step 4: Commit any fixes**

If integration testing reveals issues, fix and commit incrementally.

**Step 5: Final commit**

```bash
git add -A
git commit -m "test: verify message API integration"
```
