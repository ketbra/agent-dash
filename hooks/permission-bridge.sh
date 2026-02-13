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
