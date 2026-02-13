# Agent Dash — Design Document

A frameless, always-on-top overlay that monitors Claude Code sessions and enables
bidirectional interaction with permission prompts.

## Architecture

Three components:

1. **Dashboard GUI** (Rust, egui/eframe) — Frameless, always-on-top, draggable overlay.
   Polls for session state every 2 seconds. Renders a vertical pill stack.
   Writes permission responses to IPC files when the user clicks allow/deny.

2. **Global PermissionRequest Hook** (shell script) — Installed in
   `~/.claude/settings.json`. When Claude hits a permission prompt, the hook writes
   request details to `~/.cache/agent-dash/sessions/<session-id>/pending-permission.json`,
   then polls for a response file. The dashboard picks it up, shows buttons, user clicks,
   dashboard writes the response, hook reads it and returns the decision to Claude.

3. **Session Monitor** (built into the GUI) — Watches `~/.claude/projects/` for JSONL
   changes using inotify. Cross-references with running `claude` processes via `/proc`.
   Parses the last few JSONL lines to determine status.

## IPC Protocol for Permission Prompts

### Directory structure

```
~/.cache/agent-dash/
  sessions/
    <session-id>/
      pending-permission.json   # Written by hook, read by dashboard
      permission-response.json  # Written by dashboard, read by hook
```

### Flow

1. Claude triggers a permission prompt (e.g., wants to run `cargo build`)
2. PermissionRequest hook fires, receives tool name + input on stdin
3. Hook writes `pending-permission.json`:
   ```json
   {
     "session_id": "9e13b797-...",
     "tool": "Bash",
     "input": {"command": "cargo build"},
     "timestamp": 1707...
   }
   ```
4. Hook enters a poll loop (checking every 200ms) for `permission-response.json`
5. Dashboard detects the pending file, turns that session's pill red, expands it to show
   "Bash: `cargo build`" with Allow / Allow Similar / Deny buttons
6. User clicks a button, dashboard writes `permission-response.json`:
   ```json
   {"decision": {"behavior": "allow"}}
   ```
   or with persistent rule:
   ```json
   {"decision": {"behavior": "allow", "updatedPermissions": [...]}}
   ```
   or deny:
   ```json
   {"decision": {"behavior": "deny", "message": "User denied from dashboard"}}
   ```
7. Hook reads it, deletes both files, returns the decision to Claude
8. Pill goes back to yellow

### Timeout

If no response within 120 seconds, the hook exits without output, letting Claude's
normal permission prompt appear in the terminal as a fallback.

## GUI Layout

### Window properties

- Frameless, always-on-top, transparent background
- ~250px wide, height grows/shrinks with session count
- Draggable by clicking anywhere on the background
- Semi-transparent dark background (rgba ~30, 30, 30, 0.85)
- Rounded corners on the overall container

### Pill layout

```
+------------------------------+
| * traider (main)             |  <- green: idle/done
| * traider (backtesting)      |  <- yellow: working
| * agent-dash (main)          |  <- red: needs input
|  +------------------------+  |
|  | Bash: cargo build      |  |  <- expanded detail
|  | [Allow] [Similar] [Deny]| |  <- action buttons
|  +------------------------+  |
+------------------------------+
```

### Behavior

- Red sessions float to the top of the list automatically
- Then yellow, then green at the bottom
- Clicking a red pill with a pending permission expands it inline to show the tool
  name + command and the three buttons (Allow, Allow Similar, Deny)
- Clicking a red pill with an AskUserQuestion expands to show the question text
  and a "Go to terminal" button that focuses the relevant terminal window
- Clicking a green/yellow pill focuses the relevant terminal window
- Sessions appear when a `claude` process is detected; disappear ~5s after the
  process exits (fade out)

### Focusing the terminal window

On Wayland/GNOME, use gdbus or the xdg-activation protocol to raise the window
containing the target PTY. Fallback: highlight which terminal to look for.

## Session Discovery & Status Monitoring

### Finding active sessions

- Poll `/proc` every 2 seconds for processes named `claude`
- For each PID, read `/proc/<pid>/fd/0` for the PTY, `/proc/<pid>/cwd` for CWD
- Map CWD to a project slug (e.g., `/home/user/src/traider` -> `-home-user-src-traider`)
- Find the most recent JSONL file in `~/.claude/projects/<slug>/`

### Status detection

| Condition | Status |
|-----------|--------|
| Pending permission file exists in IPC dir | Red: permission prompt |
| Last assistant message has AskUserQuestion with no subsequent user reply | Red: question pending |
| JSONL modified within last 5s AND process running | Yellow: working |
| Process running but JSONL quiet >5s | Green: idle at prompt |
| Process not running | Grey: ended (fade out, remove after 5s) |

### Labels

- Project folder name: last component of CWD path
- Branch: from `gitBranch` field in JSONL messages
- Combined: `traider (backtesting)`

### Deduplication

Multiple processes can point to the same project+branch (subagents). Group by
session ID from the JSONL, showing one pill per session.

## Technology & Crate Choices

| Crate | Purpose |
|-------|---------|
| eframe / egui | GUI framework, frameless window, rendering |
| notify | File watching (wraps inotify on Linux) |
| serde / serde_json | Parse JSONL and IPC JSON |
| procfs | Process discovery via /proc |

Single-threaded egui event loop with a background thread for file watching and
process polling. No async runtime, no HTTP server, no database.

## Project Structure

```
src/
  main.rs        - eframe app setup, frameless window config
  app.rs         - egui rendering, pill stack layout, drag handling
  monitor.rs     - session discovery, process scanning, JSONL parsing
  ipc.rs         - read/write permission IPC files
  session.rs     - session state struct, status enum
hooks/
  permission-bridge.sh  - the PermissionRequest hook script
```

The hook script is installed globally via `~/.claude/settings.json`.
