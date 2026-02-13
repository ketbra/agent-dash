# Hook-Driven Session Status via Unix Socket

**Date:** 2026-02-13
**Status:** Draft

## Problem

The current status detection relies on JSONL file modification time — if the file
hasn't been modified in 3 seconds, the session is considered `Idle`. But Claude often
pauses for more than 3 seconds while thinking between tool calls, causing false
`Working → Idle` transitions and premature completion sounds.

The 10-second grace period for "transient /proc failures" was papering over a separate
bug: the `/proc` scan finds the process, but downstream JSONL resolution
(`find_latest_jsonl` or `parse_jsonl_status` returning `None`) silently drops the
session via `continue` statements.

## Design

Two distinct problems, each with a natural solution:

| Problem | Solution |
|---------|----------|
| **Session discovery & liveness** | `/proc` scanning (keep — it works) |
| **Session activity status** | Claude Code hooks via Unix socket (new) |

### Architecture

```
┌──────────────────────────────────────────────────────┐
│                     agent-dashd                      │
│                                                      │
│  ┌───────────────┐       ┌────────────────────────┐  │
│  │ Main Thread    │       │ Socket Thread           │  │
│  │                │       │                         │  │
│  │ loop {         │       │ listen daemon.sock      │  │
│  │   scan /proc   │       │ on connect:             │  │
│  │   read hook    │◄──────│   parse JSON event      │  │
│  │     state      │       │   update HookState      │  │
│  │   merge        │       │     (behind Mutex)      │  │
│  │   write        │       │                         │  │
│  │   state.json   │       │                         │  │
│  │   sleep 1s     │       │                         │  │
│  │ }              │       │                         │  │
│  └───────────────┘       └────────────────────────┘  │
│          │                         │                  │
│          └─────────┬───────────────┘                  │
│                    │                                  │
│        Arc<Mutex<HookState>>                          │
│        {                                              │
│          sessions: HashMap<String, HookSessionData>   │
│        }                                              │
└──────────────────────────────────────────────────────┘
         ▲                              ▲
         │ /proc (liveness)             │ Unix socket (status)
         │                              │
    ┌────┘                     ┌────────┘
    │                          │
 Linux kernel          Hook scripts in
                       Claude Code sessions
```

### Hook Event Schema

Hook scripts send JSON messages to the daemon via Unix socket at
`~/.cache/agent-dash/daemon.sock`. Each message is a single JSON object.

```json
// PreToolUse → tool_start
{
  "event": "tool_start",
  "session_id": "abc-123",
  "tool": "Bash",
  "detail": "cargo test --release",
  "tool_use_id": "toolu_01ABC"
}

// PostToolUse → tool_end
{
  "event": "tool_end",
  "session_id": "abc-123",
  "tool_use_id": "toolu_01ABC"
}

// Stop → stop (Claude finished responding)
{
  "event": "stop",
  "session_id": "abc-123"
}

// SessionStart → session_start
{
  "event": "session_start",
  "session_id": "abc-123",
  "cwd": "/home/user/src/project"
}

// SessionEnd → session_end
{
  "event": "session_end",
  "session_id": "abc-123"
}
```

### Hook Script

A single script at `~/.local/bin/agent-dash-hook.sh` handles all events. It reads
JSON from stdin (provided by Claude Code), extracts relevant fields, and sends them
to the daemon socket.

```bash
#!/bin/bash
SOCK="${XDG_CACHE_HOME:-$HOME/.cache}/agent-dash/daemon.sock"

# Bail fast if daemon isn't running
[ -S "$SOCK" ] || exit 0

INPUT=$(cat)
EVENT="$1"
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id')

case "$EVENT" in
  tool_start)
    TOOL=$(echo "$INPUT" | jq -r '.tool_name')
    TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id')
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
    TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id')
    MSG=$(jq -nc --arg e "$EVENT" --arg s "$SESSION_ID" --arg tid "$TOOL_USE_ID" \
      '{event:$e, session_id:$s, tool_use_id:$tid}')
    ;;
  *)
    MSG=$(jq -nc --arg e "$EVENT" --arg s "$SESSION_ID" \
      '{event:$e, session_id:$s}')
    ;;
esac

echo "$MSG" | curl -s --unix-socket "$SOCK" \
  -X POST -d @- http://localhost/event &>/dev/null &
```

### Claude Code Settings

Added to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [{"hooks": [{"type": "command", "command": "agent-dash-hook.sh tool_start"}]}],
    "PostToolUse": [{"hooks": [{"type": "command", "command": "agent-dash-hook.sh tool_end"}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "agent-dash-hook.sh stop"}]}],
    "SessionStart": [{"hooks": [{"type": "command", "command": "agent-dash-hook.sh session_start"}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "agent-dash-hook.sh session_end"}]}]
  }
}
```

### Merge Logic

The main loop combines `/proc` discovery with hook-reported status:

1. `/proc` scan finds sessions (PID, CWD, PTY)
2. For each discovered session, look up `HookState` by session_id
3. Status determination:
   - `tool_start` with no `tool_end` → `Working` with active tool info
   - `stop` event received → `Idle`
   - Permission IPC present → `NeedsInput` (existing mechanism)
   - No hook data yet → default to `Working`
4. If `/proc` says process is gone → `Ended`, regardless of hook state

`/proc` is authoritative for **existence**. Hooks are authoritative for **activity**.

### State.json Schema

New `active_tool` field added to session objects:

```json
{
  "sessions": [
    {
      "session_id": "abc-123",
      "project_name": "agent-dash",
      "branch": "main",
      "status": "working",
      "input_reason": null,
      "detail": null,
      "last_status_change": 1707840000,
      "active_tool": {
        "name": "Bash",
        "detail": "cargo test --release",
        "icon": "utilities-terminal-symbolic"
      }
    }
  ]
}
```

`active_tool` is `null` when not working or when no hook data is available.

### Tool Icon Mapping

The daemon resolves icon names from tool names:

| Tool | GNOME Icon | Tooltip source |
|------|------------|----------------|
| Bash | `utilities-terminal-symbolic` | command |
| Read | `document-open-symbolic` | file_path |
| Edit | `document-edit-symbolic` | file_path |
| Write | `document-new-symbolic` | file_path |
| Grep | `edit-find-symbolic` | pattern |
| Glob | `folder-saved-search-symbolic` | pattern |
| WebFetch | `web-browser-symbolic` | url |
| WebSearch | `system-search-symbolic` | query |
| Task | `system-run-symbolic` | description |
| *(other)* | `applications-system-symbolic` | tool name |

All standard GNOME symbolic icons — no custom assets needed.

### Extension Changes

- When `active_tool` is present, render a `St.Icon` with the icon name instead of
  the colored status dot
- Add CSS `@keyframes pulse` animation on the tool icon to indicate activity
- Add hover tooltip (`St.Label`) showing `active_tool.detail`, truncated to ~80 chars
- Sound transition logic unchanged but now triggers on accurate status changes

## Edge Cases

**Daemon not running when hooks fire:**
Hook script checks `[ -S "$SOCK" ] || exit 0` and exits silently. When daemon starts
later, `/proc` discovers sessions and shows them as `Working` until the next hook event.

**Daemon restarts while sessions are active:**
`HookState` starts empty. `/proc` scan rediscovers sessions immediately. They show as
`Working` (default) until the next hook event corrects the status.

**Hook fires before `/proc` finds the session:**
Store in `HookState` anyway. The next `/proc` scan (within 1s) matches it up.

**Subagents:**
Share the parent's `session_id`. Hook events from subagent tool use update the same
session status, which is correct — the session is still working.

**`jq` not installed:**
Hook script fails silently. Daemon falls back to showing `Working` for all discovered
sessions. Note `jq` as an install dependency.

**Socket buffer:**
Each connection sends ~200 bytes and disconnects. Even under heavy tool use, this
is not a concern.

## Implementation Order

1. Socket listener + `HookState` struct (new `src/socket.rs`)
2. Hook script + `~/.claude/settings.json` configuration
3. Merge logic in `refresh()` — hook status replaces JSONL mtime heuristic
4. `active_tool` field in `DashSession` and state.json output
5. Extension: tool icon rendering + hover tooltip
6. Extension: CSS pulse animation
7. Remove dead code (JSONL mtime heuristic, grace period)
8. End-to-end testing with live sessions

## What Gets Removed

- The 3-second JSONL mtime heuristic (`monitor.rs` lines 270-276)
- The 10-second grace period for "transient /proc failures" (`monitor.rs` lines 337-340)
- `read_tail_lines()` and `parse_jsonl_status()` can be removed once hooks fully
  replace session_id/branch extraction (may keep initially)

## What Stays Unchanged

- `/proc` scanning for session discovery and liveness
- Permission bridge IPC flow (`permission-bridge.sh`, `pending-permission.json`)
- `state.json` atomic write pattern
- Extension 1-second refresh polling
- Mute toggle and sound theme
