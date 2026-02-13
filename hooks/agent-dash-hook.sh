#!/usr/bin/env bash
# agent-dash-hook.sh — Forward Claude Code hook events to the agent-dash daemon
# Reads hook context JSON from stdin, sends a message to the daemon Unix socket.

set -euo pipefail

EVENT="${1:-}"
SOCKET="${XDG_CACHE_HOME:-$HOME/.cache}/agent-dash/daemon.sock"
INPUT=$(cat)

# Extract session_id from the hook's JSON input
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')

# Exit silently if session_id is empty
if [ -z "$SESSION_ID" ]; then
    exit 0
fi

# Exit silently if socket doesn't exist (daemon not running)
if [ ! -S "$SOCKET" ]; then
    exit 0
fi

build_message() {
    case "$EVENT" in
        tool_start)
            TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty')
            TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty')

            # Extract a detail field based on tool type
            case "$TOOL_NAME" in
                Bash)
                    DETAIL=$(echo "$INPUT" | jq -r '.tool_input.command // empty' | head -c 200)
                    ;;
                Read|Edit|Write)
                    DETAIL=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')
                    ;;
                Grep|Glob)
                    DETAIL=$(echo "$INPUT" | jq -r '.tool_input.pattern // empty')
                    ;;
                WebFetch)
                    DETAIL=$(echo "$INPUT" | jq -r '.tool_input.url // empty')
                    ;;
                WebSearch)
                    DETAIL=$(echo "$INPUT" | jq -r '.tool_input.query // empty')
                    ;;
                Task*)
                    DETAIL=$(echo "$INPUT" | jq -r '.tool_input.description // empty')
                    ;;
                *)
                    DETAIL="$TOOL_NAME"
                    ;;
            esac

            jq -n -c \
                --arg event "$EVENT" \
                --arg sid "$SESSION_ID" \
                --arg tool "$TOOL_NAME" \
                --arg tuid "$TOOL_USE_ID" \
                --arg detail "$DETAIL" \
                '{event: $event, session_id: $sid, tool: $tool, tool_use_id: $tuid, detail: $detail}'
            ;;
        tool_end)
            TOOL_USE_ID=$(echo "$INPUT" | jq -r '.tool_use_id // empty')

            jq -n -c \
                --arg event "$EVENT" \
                --arg sid "$SESSION_ID" \
                --arg tuid "$TOOL_USE_ID" \
                '{event: $event, session_id: $sid, tool_use_id: $tuid}'
            ;;
        *)
            jq -n -c \
                --arg event "$EVENT" \
                --arg sid "$SESSION_ID" \
                '{event: $event, session_id: $sid}'
            ;;
    esac
}

# Fire-and-forget: send to daemon socket, silently ignore errors
MSG=$(build_message)
echo "$MSG" | ncat -U --send-only "$SOCKET" 2>/dev/null || true

exit 0
