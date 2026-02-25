# Wrapper-Driven Sessions Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace process-scanner-based session discovery with wrapper registration as the authoritative source of truth, hide subagents by default, and add wrapper reconnect on daemon restart.

**Architecture:** The wrapper (`agent-dash run`) becomes the single lifecycle signal for sessions. RegisterWrapper = birth, UnregisterWrapper/disconnect = death. Hook events provide real-time status. Subagents are detected by seeing new session_ids arrive with a known wrapper_id but no matching RegisterWrapper.

**Tech Stack:** Rust, tokio, serde, interprocess (Unix sockets), portable-pty

---

### Task 1: Update protocol types (RegisterWrapper, GetState, DashSession)

Add new fields to protocol and session types. All new fields have defaults so existing serialized messages remain backwards compatible.

**Files:**
- Modify: `crates/agent-dash-core/src/protocol.rs:97-111`
- Modify: `crates/agent-dash-core/src/session.rs:39-52`
- Test: `crates/agent-dash-core/src/protocol.rs` (inline tests)
- Test: `crates/agent-dash-core/src/session.rs` (inline tests)

**Step 1: Write failing tests for new RegisterWrapper fields**

Add to the existing test module in `crates/agent-dash-core/src/protocol.rs`:

```rust
#[test]
fn deserialize_register_wrapper_with_metadata() {
    let json = r#"{"method":"register_wrapper","session_id":"wrap-1","agent":"claude","cwd":"/home/user/project","branch":"main","project_name":"project"}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    match req {
        ClientRequest::RegisterWrapper { session_id, agent, cwd, branch, project_name, .. } => {
            assert_eq!(session_id, "wrap-1");
            assert_eq!(agent, "claude");
            assert_eq!(cwd.as_deref(), Some("/home/user/project"));
            assert_eq!(branch.as_deref(), Some("main"));
            assert_eq!(project_name.as_deref(), Some("project"));
        }
        _ => panic!("expected RegisterWrapper"),
    }
}

#[test]
fn deserialize_register_wrapper_backwards_compat() {
    // Old-style RegisterWrapper without new fields should still parse.
    let json = r#"{"method":"register_wrapper","session_id":"s1","agent":"claude"}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    match req {
        ClientRequest::RegisterWrapper { cwd, branch, project_name, real_session_id, .. } => {
            assert!(cwd.is_none());
            assert!(branch.is_none());
            assert!(project_name.is_none());
            assert!(real_session_id.is_none());
        }
        _ => panic!("expected RegisterWrapper"),
    }
}

#[test]
fn deserialize_get_state_with_include_subagents() {
    let json = r#"{"method":"get_state","include_subagents":true}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    match req {
        ClientRequest::GetState { include_subagents } => {
            assert!(include_subagents);
        }
        _ => panic!("expected GetState"),
    }
}

#[test]
fn deserialize_get_state_backwards_compat() {
    let json = r#"{"method":"get_state"}"#;
    let req: ClientRequest = serde_json::from_str(json).unwrap();
    match req {
        ClientRequest::GetState { include_subagents } => {
            assert!(!include_subagents);
        }
        _ => panic!("expected GetState"),
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dash-core`
Expected: FAIL — fields don't exist yet

**Step 3: Update RegisterWrapper and GetState in protocol.rs**

In `crates/agent-dash-core/src/protocol.rs`, change:

```rust
#[serde(rename = "register_wrapper")]
RegisterWrapper {
    session_id: String,
    agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    real_session_id: Option<String>,
},
```

Change GetState from unit variant to struct:

```rust
#[serde(rename = "get_state")]
GetState {
    #[serde(default)]
    include_subagents: bool,
},
```

**Step 4: Add subagent_count to DashSession in session.rs**

In `crates/agent-dash-core/src/session.rs`, add to `DashSession`:

```rust
#[serde(default, skip_serializing_if = "is_zero")]
pub subagent_count: usize,
```

Add helper above the struct:

```rust
fn is_zero(v: &usize) -> bool { *v == 0 }
```

**Step 5: Fix compilation errors from GetState change**

The `GetState` variant is used in several places as a unit variant. Update all match arms:
- `crates/agent-dash-core/src/protocol.rs`: test `deserialize_get_state` — update match
- `crates/agent-dash/src/cli.rs:35`: `ClientRequest::GetState` → `ClientRequest::GetState { include_subagents: false }`
- `crates/agent-dash/src/client_listener.rs:163`: match arm for `ClientRequest::GetState` → `ClientRequest::GetState { include_subagents }`
- `crates/agent-dash/src/daemon.rs:149-156`: match arm for `ClientMessage::GetState` — pass through include_subagents (for now, ignore it; Task 4 uses it)

Update `ClientMessage::GetState` in `client_listener.rs` to include the field:

```rust
GetState {
    include_subagents: bool,
    reply: oneshot::Sender<String>,
},
```

And thread it through in the handler.

**Step 6: Run tests to verify they pass**

Run: `cargo test -p agent-dash-core && cargo test -p agent-dash`
Expected: PASS

**Step 7: Commit**

```bash
git add crates/agent-dash-core/src/protocol.rs crates/agent-dash-core/src/session.rs \
       crates/agent-dash/src/cli.rs crates/agent-dash/src/client_listener.rs \
       crates/agent-dash/src/daemon.rs
git commit -m "feat: add metadata fields to RegisterWrapper, include_subagents to GetState, subagent_count to DashSession"
```

---

### Task 2: Update InternalSession data model

Remove scanner-derived fields (`pid`, `wrapped`), add wrapper-driven fields (`parent_wrapper_id`, `is_main`).

**Files:**
- Modify: `crates/agent-dash/src/state.rs:9-25`
- Modify: `crates/agent-dash/src/daemon.rs` (all references to removed fields)
- Test: `crates/agent-dash/src/state.rs` (inline tests)
- Test: `crates/agent-dash/src/daemon.rs` (inline tests)

**Step 1: Write failing test for new InternalSession fields**

Add to `crates/agent-dash/src/state.rs` tests:

```rust
#[test]
fn ensure_session_defaults_not_main() {
    let mut state = DaemonState::new();
    state.ensure_session("s1");
    let session = state.sessions.get("s1").unwrap();
    assert!(!session.is_main);
    assert!(session.parent_wrapper_id.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p agent-dash -- ensure_session_defaults_not_main`
Expected: FAIL — field doesn't exist

**Step 3: Update InternalSession struct**

In `crates/agent-dash/src/state.rs`, replace the struct:

```rust
#[derive(Debug, Clone)]
pub struct InternalSession {
    pub session_id: String,
    pub cwd: Option<String>,
    pub project_name: String,
    pub branch: String,
    pub status: SessionStatus,
    pub active_tool: Option<(String, String, String)>, // (name, detail, tool_use_id)
    pub jsonl_path: Option<String>,
    pub last_status_change: u64,
    pub has_pending_question: bool,
    pub question_text: Option<String>,
    pub watch_offset: Option<u64>,
    pub is_main: bool,
    pub parent_wrapper_id: Option<String>,
    pub agent: Option<String>,
}
```

**Step 4: Update ensure_session defaults**

Update the `or_insert_with` closure in `ensure_session`:

```rust
.or_insert_with(|| InternalSession {
    session_id: session_id.to_string(),
    cwd: None,
    project_name: String::new(),
    branch: String::new(),
    status: SessionStatus::Idle,
    active_tool: None,
    jsonl_path: None,
    last_status_change: now_epoch_secs(),
    has_pending_question: false,
    question_text: None,
    watch_offset: None,
    is_main: false,
    parent_wrapper_id: None,
    agent: None,
});
```

**Step 5: Fix all compilation errors from removed fields**

Update `make_session` helper in `crates/agent-dash/src/daemon.rs` tests:

```rust
fn make_session(id: &str) -> InternalSession {
    InternalSession {
        session_id: id.to_string(),
        cwd: None,
        project_name: String::new(),
        branch: String::new(),
        status: SessionStatus::Idle,
        active_tool: None,
        jsonl_path: None,
        last_status_change: 0,
        has_pending_question: false,
        question_text: None,
        watch_offset: None,
        is_main: false,
        parent_wrapper_id: None,
        agent: None,
    }
}
```

In `crates/agent-dash/src/daemon.rs`, update:
- `RegisterWrapper` handler (~line 332): replace `session.wrapped = true;` with `session.is_main = true;`
- `UnregisterWrapper` handler (~line 341): replace `session.wrapped = false;` with `session.is_main = false;`
- Hook handler wrapper_id block (~line 79): remove `session.wrapped = true;` (subagents are NOT main)
- Scanner block (~line 386-462): leave for now (deleted in Task 5)

**Step 6: Run all tests**

Run: `cargo test -p agent-dash`
Expected: PASS

**Step 7: Commit**

```bash
git add crates/agent-dash/src/state.rs crates/agent-dash/src/daemon.rs
git commit -m "refactor: replace pid/wrapped with is_main/parent_wrapper_id on InternalSession"
```

---

### Task 3: Extract JSONL utilities from scanner

The `ListSessions` handler uses `scanner::parse_jsonl_status`. Move the JSONL parsing functions to a standalone module before deleting scanner.

**Files:**
- Create: `crates/agent-dash/src/jsonl.rs`
- Modify: `crates/agent-dash/src/main.rs` (add `mod jsonl`)
- Modify: `crates/agent-dash/src/daemon.rs:301` (use `jsonl::parse_jsonl_status`)
- Delete references to scanner in daemon's ListSessions handler

**Step 1: Create jsonl.rs with functions moved from scanner.rs**

Create `crates/agent-dash/src/jsonl.rs` containing:
- `JournalEntry` enum (private)
- `AssistantMessage` struct (private)
- `ContentBlock` enum (private)
- `JsonlStatus` struct (public)
- `read_tail_lines` function (private)
- `parse_jsonl_status` function (public)
- `find_latest_jsonl` function (public)
- All corresponding tests from scanner.rs (except `scan_does_not_panic`)

Copy these directly from `scanner.rs` lines 47-315 (everything except `ClaudeProcess`, `scan_claude_processes`, and the `scan_does_not_panic` test).

**Step 2: Add `mod jsonl` to main.rs**

In `crates/agent-dash/src/main.rs`, add:
```rust
mod jsonl;
```

**Step 3: Update daemon.rs ListSessions handler**

In `crates/agent-dash/src/daemon.rs`, line 301, change:
```rust
let Some(status) = scanner::parse_jsonl_status(&path) else {
```
to:
```rust
let Some(status) = crate::jsonl::parse_jsonl_status(&path) else {
```

**Step 4: Run tests**

Run: `cargo test -p agent-dash`
Expected: PASS (both old scanner tests and new jsonl tests)

**Step 5: Commit**

```bash
git add crates/agent-dash/src/jsonl.rs crates/agent-dash/src/main.rs crates/agent-dash/src/daemon.rs
git commit -m "refactor: extract JSONL parsing from scanner into jsonl module"
```

---

### Task 4: Delete scanner and remove sysinfo dependency

**Files:**
- Delete: `crates/agent-dash/src/scanner.rs`
- Modify: `crates/agent-dash/src/main.rs` (remove `mod scanner`)
- Modify: `crates/agent-dash/src/daemon.rs` (remove scan_interval, scanner import, scan block)
- Modify: `crates/agent-dash/Cargo.toml` (remove sysinfo)
- Modify: `Cargo.toml` (remove sysinfo from workspace deps)

**Step 1: Remove `mod scanner` from main.rs**

Delete `mod scanner;` from `crates/agent-dash/src/main.rs`.

**Step 2: Remove scanner import from daemon.rs**

Delete `use crate::scanner;` from `crates/agent-dash/src/daemon.rs` line 7.

**Step 3: Remove scan_interval and scan block from daemon.rs**

In `crates/agent-dash/src/daemon.rs`:
- Remove `let mut scan_interval = ...` (~line 48)
- Remove `scan_interval.set_missed_tick_behavior(...)` (~line 53)
- Remove the entire `_ = scan_interval.tick() => { ... }` arm (~lines 386-463) from the select loop

**Step 4: Delete scanner.rs**

Delete `crates/agent-dash/src/scanner.rs`.

**Step 5: Remove sysinfo from Cargo.toml files**

In `crates/agent-dash/Cargo.toml`, remove the line:
```toml
sysinfo = { workspace = true }
```

In `Cargo.toml` (workspace root), remove:
```toml
sysinfo = "0.34"
```

**Step 6: Run tests and build**

Run: `cargo build -p agent-dash && cargo test -p agent-dash`
Expected: PASS — everything compiles, no scanner references remain

**Step 7: Commit**

```bash
git add -A
git commit -m "feat: remove process scanner and sysinfo dependency

Session lifecycle is now driven entirely by wrapper registration
and hook events. No more polling."
```

---

### Task 5: Subagent-aware state filtering

Update `to_dash_sessions` to filter by `is_main` and count subagents.

**Files:**
- Modify: `crates/agent-dash/src/state.rs:188-244`
- Test: `crates/agent-dash/src/state.rs` (inline tests)

**Step 1: Write failing tests**

Add to `crates/agent-dash/src/state.rs` tests:

```rust
#[test]
fn to_dash_sessions_filters_subagents() {
    let mut state = DaemonState::new();
    state.ensure_session("main-1");
    state.sessions.get_mut("main-1").unwrap().is_main = true;
    state.sessions.get_mut("main-1").unwrap().project_name = "proj".into();
    state.ensure_session("sub-1");
    state.sessions.get_mut("sub-1").unwrap().parent_wrapper_id = Some("main-1".into());

    let dash = state.to_dash_sessions();
    assert_eq!(dash.len(), 1);
    assert_eq!(dash[0].session_id, "main-1");
}

#[test]
fn to_dash_sessions_counts_subagents() {
    let mut state = DaemonState::new();
    state.ensure_session("main-1");
    state.sessions.get_mut("main-1").unwrap().is_main = true;
    state.ensure_session("sub-1");
    state.sessions.get_mut("sub-1").unwrap().parent_wrapper_id = Some("main-1".into());
    state.ensure_session("sub-2");
    state.sessions.get_mut("sub-2").unwrap().parent_wrapper_id = Some("main-1".into());

    let dash = state.to_dash_sessions();
    assert_eq!(dash.len(), 1);
    assert_eq!(dash[0].subagent_count, 2);
}

#[test]
fn to_all_dash_sessions_includes_subagents() {
    let mut state = DaemonState::new();
    state.ensure_session("main-1");
    state.sessions.get_mut("main-1").unwrap().is_main = true;
    state.ensure_session("sub-1");
    state.sessions.get_mut("sub-1").unwrap().parent_wrapper_id = Some("main-1".into());

    let dash = state.to_all_dash_sessions();
    assert_eq!(dash.len(), 2);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p agent-dash -- to_dash_sessions_filters`
Expected: FAIL

**Step 3: Implement filtering in to_dash_sessions**

In `crates/agent-dash/src/state.rs`, update `to_dash_sessions`:

```rust
/// Convert main sessions to the serializable `DashSession` form.
/// Subagents are excluded; their count is included on the parent.
pub fn to_dash_sessions(&self) -> Vec<DashSession> {
    self.build_dash_sessions(false)
}

/// Convert ALL sessions (including subagents) to `DashSession` form.
pub fn to_all_dash_sessions(&self) -> Vec<DashSession> {
    self.build_dash_sessions(true)
}

fn build_dash_sessions(&self, include_subagents: bool) -> Vec<DashSession> {
    // Count subagents per parent wrapper.
    let mut subagent_counts: HashMap<String, usize> = HashMap::new();
    for s in self.sessions.values() {
        if let Some(ref parent) = s.parent_wrapper_id {
            *subagent_counts.entry(parent.clone()).or_default() += 1;
        }
    }

    let mut sessions: Vec<DashSession> = self
        .sessions
        .values()
        .filter(|s| include_subagents || s.is_main)
        .map(|s| {
            // ... existing mapping logic ...
            // Add subagent_count from the counts map:
            let subagent_count = subagent_counts.get(&s.session_id).copied().unwrap_or(0);
            // Include subagent_count in the DashSession construction
        })
        .collect();

    sessions.sort_by(|a, b| {
        a.status.cmp(&b.status).then(a.session_id.cmp(&b.session_id))
    });

    sessions
}
```

Keep the existing mapping logic (input_reason, active_tool) inside the map closure, just add `subagent_count` to the `DashSession` construction.

**Step 4: Update daemon.rs to use include_subagents**

In `crates/agent-dash/src/daemon.rs`, update the `GetState` handler:

```rust
ClientMessage::GetState { include_subagents, reply } => {
    let sessions = if include_subagents {
        state.to_all_dash_sessions()
    } else {
        state.to_dash_sessions()
    };
    let event = ServerEvent::StateUpdate { sessions };
    if let Ok(json) = protocol::encode_line(&event) {
        let _ = reply.send(json);
    }
}
```

Also update the existing `to_dash_sessions_returns_all` test to set `is_main = true` on both sessions so it keeps passing.

**Step 5: Run tests**

Run: `cargo test -p agent-dash`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/agent-dash/src/state.rs crates/agent-dash/src/daemon.rs
git commit -m "feat: filter subagents from default state, add subagent_count"
```

---

### Task 6: Daemon subagent linking and enriched RegisterWrapper

Update the daemon's hook handler to detect subagents and the RegisterWrapper handler to accept metadata.

**Files:**
- Modify: `crates/agent-dash/src/daemon.rs:59-82` (hook handler)
- Modify: `crates/agent-dash/src/daemon.rs:326-338` (RegisterWrapper handler)
- Modify: `crates/agent-dash/src/client_listener.rs:59-63,275-301` (pass through fields)

**Step 1: Update ClientMessage::RegisterWrapper**

In `crates/agent-dash/src/client_listener.rs`, update:

```rust
RegisterWrapper {
    session_id: String,
    agent: String,
    cwd: Option<String>,
    branch: Option<String>,
    project_name: Option<String>,
    real_session_id: Option<String>,
    prompt_tx: mpsc::Sender<String>,
},
```

Update the handler at line 275 to pass through the new fields:

```rust
ClientRequest::RegisterWrapper { session_id, agent, cwd, branch, project_name, real_session_id } => {
    let (prompt_tx, mut prompt_rx) = mpsc::channel::<String>(16);
    let _ = tx
        .send(ClientMessage::RegisterWrapper {
            session_id: session_id.clone(),
            agent,
            cwd,
            branch,
            project_name,
            real_session_id,
            prompt_tx,
        })
        .await;
    // ... rest unchanged ...
```

**Step 2: Update RegisterWrapper handler in daemon.rs**

Replace the `RegisterWrapper` handler (~line 326):

```rust
ClientMessage::RegisterWrapper {
    session_id,
    agent,
    cwd,
    branch,
    project_name,
    real_session_id,
    prompt_tx,
} => {
    state.ensure_session(&session_id);
    if let Some(session) = state.sessions.get_mut(&session_id) {
        session.is_main = true;
        session.agent = Some(agent);
        if let Some(ref c) = cwd {
            session.cwd = Some(c.clone());
        }
        if let Some(ref b) = branch {
            session.branch = b.clone();
        }
        if let Some(ref p) = project_name {
            session.project_name = p.clone();
        }
    }
    wrapper_channels.insert(session_id.clone(), prompt_tx);

    // On reconnect, re-link the real session_id to this wrapper's channel.
    if let Some(ref real_id) = real_session_id {
        if let Some(tx) = wrapper_channels.get(&session_id) {
            wrapper_channels.insert(real_id.clone(), tx.clone());
        }
    }

    state_dirty = true;
    broadcast_state(&mut subscribers, &state);
}
```

**Step 3: Update hook handler for subagent detection**

In `crates/agent-dash/src/daemon.rs`, replace the wrapper_id block (~lines 64-82):

```rust
if let Some(ref wid) = wrapper_id {
    let hook_session_id = match &event {
        HookEvent::ToolStart { session_id, .. }
        | HookEvent::ToolEnd { session_id, .. }
        | HookEvent::Stop { session_id }
        | HookEvent::SessionStart { session_id, .. }
        | HookEvent::SessionEnd { session_id } => session_id,
    };

    // Link the real session's prompt channel to the wrapper's.
    if !wrapper_channels.contains_key(hook_session_id) {
        if let Some(prompt_tx) = wrapper_channels.get(wid) {
            wrapper_channels.insert(hook_session_id.clone(), prompt_tx.clone());
        }
    }

    state.ensure_session(hook_session_id);
    if let Some(session) = state.sessions.get_mut(hook_session_id) {
        // If this session_id matches the wrapper_id, it's the main
        // session linking up. If it doesn't match, it's a subagent.
        if hook_session_id == wid {
            // This is the wrapper's own session — already marked is_main
            // by RegisterWrapper.
        } else if !session.is_main {
            // New session under this wrapper — it's a subagent.
            session.parent_wrapper_id = Some(wid.clone());
        }
    }
}
```

**Step 4: Run tests and build**

Run: `cargo test -p agent-dash && cargo build -p agent-dash`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/agent-dash/src/daemon.rs crates/agent-dash/src/client_listener.rs
git commit -m "feat: subagent detection via wrapper_id, enriched RegisterWrapper with metadata"
```

---

### Task 7: UnregisterWrapper cleans up subagents

When a wrapper unregisters (exit or crash), remove all its subagent sessions.

**Files:**
- Modify: `crates/agent-dash/src/daemon.rs:340-348` (UnregisterWrapper handler)
- Modify: `crates/agent-dash/src/state.rs` (add cleanup method)
- Test: `crates/agent-dash/src/state.rs`

**Step 1: Write failing test**

Add to `crates/agent-dash/src/state.rs` tests:

```rust
#[test]
fn remove_wrapper_cleans_up_subagents() {
    let mut state = DaemonState::new();

    // Main session.
    state.ensure_session("wrap-1");
    state.sessions.get_mut("wrap-1").unwrap().is_main = true;

    // Two subagents.
    state.ensure_session("sub-a");
    state.sessions.get_mut("sub-a").unwrap().parent_wrapper_id = Some("wrap-1".into());
    state.ensure_session("sub-b");
    state.sessions.get_mut("sub-b").unwrap().parent_wrapper_id = Some("wrap-1".into());

    // Unrelated session.
    state.ensure_session("wrap-2");
    state.sessions.get_mut("wrap-2").unwrap().is_main = true;

    state.remove_wrapper("wrap-1");

    assert!(!state.sessions.contains_key("wrap-1"));
    assert!(!state.sessions.contains_key("sub-a"));
    assert!(!state.sessions.contains_key("sub-b"));
    assert!(state.sessions.contains_key("wrap-2"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p agent-dash -- remove_wrapper_cleans_up`
Expected: FAIL

**Step 3: Implement remove_wrapper on DaemonState**

Add to `crates/agent-dash/src/state.rs` impl block:

```rust
/// Remove a wrapper session and all its subagent sessions.
/// Also cleans up pending permissions for removed sessions.
pub fn remove_wrapper(&mut self, wrapper_id: &str) {
    // Collect session IDs to remove: the wrapper itself + subagents.
    let to_remove: Vec<String> = self
        .sessions
        .iter()
        .filter(|(id, s)| {
            *id == wrapper_id
                || s.parent_wrapper_id.as_deref() == Some(wrapper_id)
        })
        .map(|(id, _)| id.clone())
        .collect();

    for id in &to_remove {
        self.sessions.remove(id);
        self.pending_permissions
            .retain(|_, perm| perm.session_id != *id);
    }
}
```

**Step 4: Use remove_wrapper in daemon's UnregisterWrapper handler**

In `crates/agent-dash/src/daemon.rs`, replace the UnregisterWrapper handler:

```rust
ClientMessage::UnregisterWrapper { session_id } => {
    wrapper_channels.remove(&session_id);
    // Also remove channels for subagents of this wrapper.
    let sub_ids: Vec<String> = state.sessions.iter()
        .filter(|(_, s)| s.parent_wrapper_id.as_deref() == Some(&session_id))
        .map(|(id, _)| id.clone())
        .collect();
    for id in &sub_ids {
        wrapper_channels.remove(id);
    }
    state.remove_wrapper(&session_id);
    state_dirty = true;
    broadcast_state(&mut subscribers, &state);
}
```

**Step 5: Run tests**

Run: `cargo test -p agent-dash`
Expected: PASS

**Step 6: Commit**

```bash
git add crates/agent-dash/src/state.rs crates/agent-dash/src/daemon.rs
git commit -m "feat: UnregisterWrapper cleans up subagent sessions and permissions"
```

---

### Task 8: Guard SendPrompt against subagents

**Files:**
- Modify: `crates/agent-dash/src/daemon.rs:349-381` (SendPrompt handler)

**Step 1: Update SendPrompt handler**

Add a check before looking up the prompt channel. After resolving the session key, check if it's a main session:

```rust
ClientMessage::SendPrompt {
    session_id,
    text,
    reply,
} => {
    // Check if the target is a subagent.
    let resolved_key = resolve_session_key(&session_id, &state);
    if let Some(ref key) = resolved_key {
        if let Some(session) = state.sessions.get(key) {
            if !session.is_main && session.parent_wrapper_id.is_some() {
                let response = ServerEvent::Error {
                    message: "cannot inject prompt into subagent".into(),
                };
                let _ = reply.send(protocol::encode_line(&response).unwrap_or_default());
                continue; // Skip to next select arm (use appropriate control flow)
            }
        }
    }
    // ... existing prompt channel lookup logic ...
```

Note: Since this is inside a `tokio::select!` match arm, use an if/else block rather than early return. Wrap the existing logic in an else branch, or restructure as:

```rust
let response = {
    // Check subagent
    let is_subagent = resolve_session_key(&session_id, &state)
        .and_then(|k| state.sessions.get(&k))
        .is_some_and(|s| !s.is_main && s.parent_wrapper_id.is_some());

    if is_subagent {
        ServerEvent::Error {
            message: "cannot inject prompt into subagent".into(),
        }
    } else {
        // existing prompt_tx lookup logic...
    }
};
let _ = reply.send(protocol::encode_line(&response).unwrap_or_default());
```

**Step 2: Run tests and build**

Run: `cargo test -p agent-dash && cargo build -p agent-dash`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/agent-dash/src/daemon.rs
git commit -m "feat: reject prompt injection to subagent sessions"
```

---

### Task 9: Wrapper sends metadata + reconnect loop

Update the wrapper to send CWD, branch, and project_name in RegisterWrapper, and reconnect on daemon disconnect.

**Files:**
- Modify: `crates/agent-dash/src/wrapper.rs`

**Step 1: Extract metadata at startup**

Add a helper function to `wrapper.rs`:

```rust
/// Extract git branch from a directory. Returns empty string on failure.
fn git_branch(dir: &std::path::Path) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8(out.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}
```

**Step 2: Send enriched RegisterWrapper**

Update the registration block (~line 123-129):

```rust
if let Some(ref conn) = daemon_conn {
    let cwd = std::env::current_dir().ok();
    let branch = cwd.as_ref().map(|d| git_branch(d)).unwrap_or_default();
    let project_name = cwd.as_ref()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let req = ClientRequest::RegisterWrapper {
        session_id: session_id.clone(),
        agent: profile.name.to_string(),
        cwd: cwd.map(|d| d.to_string_lossy().to_string()),
        branch: Some(branch),
        project_name: Some(project_name),
        real_session_id: None,
    };
    let _ = send_to_daemon(conn, &req);
}
```

**Step 3: Add reconnect logic to daemon listener thread**

Replace the daemon listener thread (~lines 201-227) to include reconnect:

```rust
if let Some(conn) = daemon_conn {
    let write_tx_inject = write_tx.clone();
    let running_daemon = running.clone();
    let session_id_daemon = session_id.clone();
    let agent_name = profile.name.to_string();
    std::thread::spawn(move || {
        let mut conn = conn;
        loop {
            // Read events from daemon.
            let reader = BufReader::new(&conn);
            for line in reader.lines() {
                if !running_daemon.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(line) = line else { break }; // Daemon disconnected
                if let Ok(event) = serde_json::from_str::<ServerEvent>(&line) {
                    if let ServerEvent::InjectPrompt { text } = event {
                        if write_tx_inject.send(text.into_bytes()).is_err() {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        if write_tx_inject.send(vec![b'\r']).is_err() {
                            return;
                        }
                    }
                }
            }

            // Daemon disconnected — reconnect loop.
            if !running_daemon.load(Ordering::Relaxed) {
                return;
            }
            eprintln!("agent-dash: daemon disconnected, reconnecting...");
            let mut delay = Duration::from_secs(1);
            let max_delay = Duration::from_secs(10);
            loop {
                if !running_daemon.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(delay);
                if let Some(new_conn) = try_connect_daemon() {
                    // Re-register with metadata.
                    let cwd = std::env::current_dir().ok();
                    let req = ClientRequest::RegisterWrapper {
                        session_id: session_id_daemon.clone(),
                        agent: agent_name.clone(),
                        cwd: cwd.map(|d| d.to_string_lossy().to_string()),
                        branch: None, // Could re-extract but not critical
                        project_name: None,
                        real_session_id: None, // TODO: track and include
                    };
                    if send_to_daemon(&new_conn, &req).is_ok() {
                        eprintln!("agent-dash: reconnected to daemon");
                        conn = new_conn;
                        break; // Back to reading events
                    }
                }
                delay = (delay * 2).min(max_delay);
            }
        }
    });
}
```

**Step 4: Build and test**

Run: `cargo build -p agent-dash`
Expected: PASS (wrapper logic is hard to unit test; manual testing needed)

**Step 5: Commit**

```bash
git add crates/agent-dash/src/wrapper.rs
git commit -m "feat: wrapper sends metadata in RegisterWrapper, reconnects on daemon restart"
```

---

### Task 10: Update CLI status display

Show subagent count in the status output.

**Files:**
- Modify: `crates/agent-dash/src/cli.rs:33-66`

**Step 1: Update cmd_status**

In the session printing loop, add subagent info:

```rust
for s in &sessions {
    let tool_info = s
        .active_tool
        .as_ref()
        .map(|t| format!(" [{}:{}]", t.name, truncate(&t.detail, 40)))
        .unwrap_or_default();
    let sub_info = if s.subagent_count > 0 {
        format!(" (+{} subagents)", s.subagent_count)
    } else {
        String::new()
    };
    println!(
        "{:<12} {:<10} {:<10} {}{}{}",
        truncate(&s.project_name, 12),
        s.branch,
        s.status,
        truncate(&s.session_id, 8),
        tool_info,
        sub_info,
    );
}
```

**Step 2: Build**

Run: `cargo build -p agent-dash`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/agent-dash/src/cli.rs
git commit -m "feat: show subagent count in status output"
```

---

### Task 11: Update broadcast_state to respect filtering

Currently `broadcast_state` always calls `to_dash_sessions`. Subscribers should see only main sessions by default.

**Files:**
- Modify: `crates/agent-dash/src/daemon.rs:517-523`

**Step 1: Verify broadcast_state already calls to_dash_sessions**

The current `broadcast_state` calls `state.to_dash_sessions()` which now filters to main sessions only. This is already correct — subscribers see main sessions only.

No code change needed. Verify with:

Run: `cargo test -p agent-dash`
Expected: PASS

**Step 2: Commit** (skip if no changes)

---

### Task 12: Final integration test and cleanup

**Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS

**Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

**Step 3: Manual smoke test**

1. Start daemon: `agent-dash daemon start &`
2. In another terminal: `agent-dash run claude`
3. In another terminal: `agent-dash status` — should show one session with project/branch info
4. Have Claude spawn a subagent (ask it to do a Task)
5. `agent-dash status` — should show session with subagent count
6. Kill daemon, restart: `agent-dash daemon start &`
7. Check wrapper reconnects: `agent-dash status` — session should reappear

**Step 4: Final commit if any fixes needed**
