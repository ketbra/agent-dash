# GNOME Extension Pivot Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the egui overlay with a GNOME Shell extension side panel + Rust monitoring daemon.

**Architecture:** The Rust binary becomes a headless daemon (`agent-dashd`) that monitors Claude sessions and writes state to `~/.cache/agent-dash/state.json`. A GNOME Shell extension reads that file every 2 seconds and renders a side panel with colored session pills, expandable permission prompts, and Allow/Similar/Deny buttons. The extension writes permission responses to the IPC directory for the hook to read.

**Tech Stack:** Rust (daemon), GNOME Shell Extension (JavaScript/GJS), St widgets, GLib

---

### Task 1: Add serializable state output types

**Files:**
- Modify: `src/session.rs`

Add Serialize-friendly types for the daemon's JSON output. These mirror the internal types but are JSON-safe (no `Instant`, no `PathBuf`).

**Step 1: Add the DashState and DashSession types**

Add after the existing `Session` impl block, before `#[cfg(test)]`:

```rust
/// JSON-serializable state for the GNOME extension to read.
#[derive(Debug, Clone, Serialize)]
pub struct DashState {
    pub sessions: Vec<DashSession>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashSession {
    pub session_id: String,
    pub project_name: String,
    pub branch: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_reason: Option<DashInputReason>,
}

#[derive(Debug, Clone, Serialize)]
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

impl Session {
    /// Convert to JSON-serializable form.
    pub fn to_dash_session(&self) -> DashSession {
        let status = match self.status {
            SessionStatus::NeedsInput => "needs_input",
            SessionStatus::Working => "working",
            SessionStatus::Idle => "idle",
            SessionStatus::Ended => "ended",
        };
        let input_reason = self.input_reason.as_ref().map(|r| match r {
            InputReason::Permission(req) => {
                let command = req.input.get("command")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let detail = if command.is_none() {
                    Some(format!("{:?}", req.input))
                } else {
                    None
                };
                DashInputReason {
                    reason_type: "permission".to_string(),
                    tool: Some(req.tool.clone()),
                    command,
                    detail,
                    text: None,
                }
            }
            InputReason::Question { text } => DashInputReason {
                reason_type: "question".to_string(),
                tool: None,
                command: None,
                detail: None,
                text: Some(text.clone()),
            },
        });
        DashSession {
            session_id: self.session_id.clone(),
            project_name: self.project_name.clone(),
            branch: self.branch.clone(),
            status: status.to_string(),
            input_reason,
        }
    }
}
```

**Step 2: Add test for serialization**

Add to the existing `mod tests`:

```rust
    #[test]
    fn dash_session_serialize() {
        let s = Session {
            session_id: "abc".into(),
            pid: 1,
            pty: PathBuf::from("/dev/pts/0"),
            cwd: PathBuf::from("/home/user/project"),
            project_name: "project".into(),
            branch: "feat".into(),
            status: SessionStatus::Working,
            input_reason: None,
            jsonl_path: PathBuf::new(),
            last_jsonl_modified: None,
            ended_at: None,
        };
        let ds = s.to_dash_session();
        let json = serde_json::to_string(&ds).unwrap();
        assert!(json.contains("\"status\":\"working\""));
        assert!(json.contains("\"project_name\":\"project\""));
        assert!(!json.contains("input_reason"));
    }
```

**Step 3: Run tests**

Run: `cargo test`
Expected: all 16 tests pass (15 existing + 1 new)

**Step 4: Commit**

```bash
git add src/session.rs
git commit -m "feat: add serializable DashState types for daemon output"
```

---

### Task 2: Convert main.rs to headless daemon

**Files:**
- Modify: `src/main.rs`
- Delete: `src/app.rs`
- Modify: `Cargo.toml` (remove eframe/egui deps)

**Step 1: Remove eframe and egui from Cargo.toml**

Replace the `[dependencies]` section:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
procfs = "0.17"
dirs = "6"
```

**Step 2: Delete `src/app.rs`**

Remove the file entirely.

**Step 3: Replace `src/main.rs` with daemon loop**

```rust
mod ipc;
mod monitor;
mod session;

use monitor::SessionMonitor;
use session::DashState;
use std::path::PathBuf;
use std::time::Duration;

/// Path to the state file the GNOME extension reads.
fn state_file_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-dash")
        .join("state.json")
}

/// Write state atomically (tmp + rename).
fn write_state(state: &DashState) -> std::io::Result<()> {
    let path = state_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn main() {
    eprintln!("agent-dashd starting, writing to {:?}", state_file_path());
    let mut monitor = SessionMonitor::new();

    loop {
        monitor.refresh();
        let sessions: Vec<_> = monitor
            .sorted_sessions()
            .iter()
            .map(|s| s.to_dash_session())
            .collect();
        let state = DashState { sessions };
        if let Err(e) = write_state(&state) {
            eprintln!("error writing state: {}", e);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
```

**Step 4: Verify it compiles and tests pass**

Run: `cargo test && cargo check`
Expected: all tests pass, compiles clean

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: convert to headless daemon writing state.json"
```

---

### Task 3: Create GNOME Shell extension skeleton

**Files:**
- Create: `extension/metadata.json`
- Create: `extension/extension.js`
- Create: `extension/stylesheet.css`

**Step 1: Create metadata.json**

```json
{
  "uuid": "agent-dash@mfeinber",
  "name": "Agent Dash",
  "description": "Side panel showing Claude Code session status with permission controls",
  "version": 1,
  "shell-version": ["49"],
  "url": ""
}
```

**Step 2: Create stylesheet.css**

```css
.agent-dash-panel {
    background-color: rgba(30, 30, 30, 0.92);
    padding: 8px 6px;
}

.agent-dash-pill {
    background-color: rgba(50, 50, 50, 0.85);
    border-radius: 6px;
    padding: 6px 10px;
    margin: 2px 0;
}

.agent-dash-label-red {
    color: #ff5050;
    font-size: 11pt;
    font-weight: bold;
}

.agent-dash-label-yellow {
    color: #ffc832;
    font-size: 11pt;
}

.agent-dash-label-green {
    color: #50c850;
    font-size: 11pt;
}

.agent-dash-label-grey {
    color: #808080;
    font-size: 11pt;
}

.agent-dash-detail {
    color: #c8c8c8;
    font-size: 10pt;
    padding: 4px 0;
}

.agent-dash-button {
    border-radius: 4px;
    padding: 4px 12px;
    margin: 2px 4px 2px 0;
    font-size: 10pt;
}

.agent-dash-allow {
    background-color: rgba(80, 200, 80, 0.3);
    color: #50c850;
}

.agent-dash-deny {
    background-color: rgba(255, 80, 80, 0.3);
    color: #ff5050;
}

.agent-dash-similar {
    background-color: rgba(255, 200, 50, 0.3);
    color: #ffc832;
}

.agent-dash-empty {
    color: #808080;
    font-size: 10pt;
    padding: 12px 8px;
}
```

**Step 3: Create extension.js**

```javascript
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import St from 'gi://St';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

const PANEL_WIDTH = 220;
const REFRESH_INTERVAL_SECONDS = 2;
const STATE_FILE = GLib.build_filenamev([
    GLib.get_home_dir(), '.cache', 'agent-dash', 'state.json'
]);
const IPC_BASE = GLib.build_filenamev([
    GLib.get_home_dir(), '.cache', 'agent-dash', 'sessions'
]);

export default class AgentDashExtension extends Extension {
    enable() {
        this._panel = new St.BoxLayout({
            vertical: true,
            style_class: 'agent-dash-panel',
            reactive: true,
            x_expand: false,
            y_expand: true,
        });
        this._panel.set_width(PANEL_WIDTH);

        // Position on left edge, below top bar
        const monitor = Main.layoutManager.primaryMonitor;
        const topBarHeight = Main.panel.height || 32;
        this._panel.set_position(0, topBarHeight);
        this._panel.set_height(monitor.height - topBarHeight);

        Main.layoutManager.addChrome(this._panel, {
            affectsStruts: true,
            affectsInputRegion: true,
            trackFullscreen: true,
        });

        this._expandedSession = null;
        this._refresh();
        this._timeoutId = GLib.timeout_add_seconds(
            GLib.PRIORITY_DEFAULT,
            REFRESH_INTERVAL_SECONDS,
            () => {
                this._refresh();
                return GLib.SOURCE_CONTINUE;
            }
        );
    }

    disable() {
        if (this._timeoutId) {
            GLib.source_remove(this._timeoutId);
            this._timeoutId = null;
        }
        if (this._panel) {
            Main.layoutManager.removeChrome(this._panel);
            this._panel.destroy();
            this._panel = null;
        }
        this._expandedSession = null;
    }

    _refresh() {
        if (!this._panel) return;

        let data;
        try {
            const [ok, contents] = GLib.file_get_contents(STATE_FILE);
            if (!ok) return;
            const decoder = new TextDecoder();
            data = JSON.parse(decoder.decode(contents));
        } catch (e) {
            return; // File doesn't exist yet or is being written
        }

        // Preserve scroll position and expanded state
        this._panel.destroy_all_children();

        const sessions = data.sessions || [];
        if (sessions.length === 0) {
            const empty = new St.Label({
                text: 'No active Claude sessions',
                style_class: 'agent-dash-empty',
            });
            this._panel.add_child(empty);
            return;
        }

        for (const session of sessions) {
            this._addSessionPill(session);
        }
    }

    _addSessionPill(session) {
        const pill = new St.BoxLayout({
            vertical: true,
            style_class: 'agent-dash-pill',
            reactive: true,
        });

        // Status dot + label
        const dots = {
            needs_input: '\u{1F534}',
            working: '\u{1F7E1}',
            idle: '\u{1F7E2}',
            ended: '\u{26AA}',
        };
        const styleClasses = {
            needs_input: 'agent-dash-label-red',
            working: 'agent-dash-label-yellow',
            idle: 'agent-dash-label-green',
            ended: 'agent-dash-label-grey',
        };
        const dot = dots[session.status] || '\u{26AA}';
        const branch = (!session.branch || session.branch === 'main')
            ? '' : ` (${session.branch})`;
        const labelText = `${dot} ${session.project_name}${branch}`;
        const styleClass = styleClasses[session.status] || 'agent-dash-label-grey';

        const label = new St.Label({
            text: labelText,
            style_class: styleClass,
            reactive: true,
        });
        pill.add_child(label);

        const isExpanded = this._expandedSession === session.session_id;

        // Click to expand/collapse
        pill.connect('button-press-event', () => {
            if (isExpanded) {
                this._expandedSession = null;
            } else if (session.input_reason) {
                this._expandedSession = session.session_id;
            }
            this._refresh();
            return true; // consume event
        });

        // Expanded detail
        if (isExpanded && session.input_reason) {
            const reason = session.input_reason;
            if (reason.type === 'permission') {
                const detailText = reason.command
                    ? `${reason.tool}: ${reason.command}`
                    : `${reason.tool}: ${reason.detail || '?'}`;
                const detail = new St.Label({
                    text: detailText,
                    style_class: 'agent-dash-detail',
                });
                pill.add_child(detail);

                const buttonBox = new St.BoxLayout({vertical: false});

                const allowBtn = new St.Button({
                    label: 'Allow',
                    style_class: 'agent-dash-button agent-dash-allow',
                });
                allowBtn.connect('clicked', () => {
                    this._writePermissionResponse(session.session_id, 'allow', null);
                    this._expandedSession = null;
                    this._refresh();
                });

                const similarBtn = new St.Button({
                    label: 'Similar',
                    style_class: 'agent-dash-button agent-dash-similar',
                });
                similarBtn.connect('clicked', () => {
                    this._writePermissionResponse(session.session_id, 'allow', null);
                    this._expandedSession = null;
                    this._refresh();
                });

                const denyBtn = new St.Button({
                    label: 'Deny',
                    style_class: 'agent-dash-button agent-dash-deny',
                });
                denyBtn.connect('clicked', () => {
                    this._writePermissionResponse(session.session_id, 'deny',
                        'Denied from dashboard');
                    this._expandedSession = null;
                    this._refresh();
                });

                buttonBox.add_child(allowBtn);
                buttonBox.add_child(similarBtn);
                buttonBox.add_child(denyBtn);
                pill.add_child(buttonBox);
            } else if (reason.type === 'question') {
                const questionLabel = new St.Label({
                    text: reason.text || 'Agent has a question',
                    style_class: 'agent-dash-detail',
                });
                pill.add_child(questionLabel);
            }
        }

        this._panel.add_child(pill);
    }

    _writePermissionResponse(sessionId, behavior, message) {
        try {
            const dir = GLib.build_filenamev([IPC_BASE, sessionId]);
            GLib.mkdir_with_parents(dir, 0o755);

            const response = {decision: {behavior}};
            if (message) response.decision.message = message;

            const path = GLib.build_filenamev([dir, 'permission-response.json']);
            const tmpPath = GLib.build_filenamev([dir, 'permission-response.tmp']);

            const json = JSON.stringify(response);
            GLib.file_set_contents(tmpPath, json);

            const tmpFile = Gio.File.new_for_path(tmpPath);
            const destFile = Gio.File.new_for_path(path);
            tmpFile.move(destFile, Gio.FileCopyFlags.OVERWRITE, null, null);
        } catch (e) {
            console.error('agent-dash: error writing permission response:', e);
        }
    }
}
```

**Step 3: Commit**

```bash
git add extension/
git commit -m "feat: add GNOME Shell extension for side panel"
```

---

### Task 4: Install and test the extension

**Step 1: Symlink the extension into GNOME's extensions directory**

```bash
mkdir -p ~/.local/share/gnome-shell/extensions/
ln -sf /home/mfeinber/src/rust/agent-dash/extension \
       ~/.local/share/gnome-shell/extensions/agent-dash@mfeinber
```

**Step 2: Enable the extension**

```bash
gnome-extensions enable agent-dash@mfeinber
```

Note: On Wayland, you need to log out and back in (or press Alt+F2 and type `r` if on X11) for the extension to load. Alternatively, restart GNOME Shell:

```bash
# Only works on X11, on Wayland you must log out/in
busctl --user call org.gnome.Shell /org/gnome/Shell org.gnome.Shell Eval s 'Meta.restart("Restarting...")'
```

**Step 3: Start the daemon**

```bash
cargo run --release
```

The daemon writes `~/.cache/agent-dash/state.json` every 2 seconds. The extension reads it and shows the side panel.

**Step 4: Verify**

- Side panel should appear on the left edge
- Maximized windows should not cover it (strut reservation)
- Session pills should appear with correct colors
- If no sessions: "No active Claude sessions"

**Step 5: Commit any fixups**

---

### Task 5: Clean up old egui code

**Step 1: Remove old files and dependencies**

- Delete `src/app.rs` (if not already done in Task 2)
- Remove the `.gitignore` entry for egui if any
- Verify `Cargo.toml` has no egui/eframe dependencies

**Step 2: Run tests**

Run: `cargo test`
Expected: all tests pass

**Step 3: Commit**

```bash
git add -A
git commit -m "chore: remove egui overlay code, project is now daemon + extension"
```
