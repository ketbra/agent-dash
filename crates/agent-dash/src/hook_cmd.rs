use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use agent_dash_core::paths;
use agent_dash_core::protocol::{
    ClientRequest, HookEnvelope, HookEvent, HookPermissionDecision, encode_line,
};

pub fn run(event_type: &str) {
    // Normalize underscores to hyphens.
    let event_type = event_type.replace('_', "-");

    // Read all of stdin as JSON.
    let mut input_buf = String::new();
    if std::io::stdin().read_to_string(&mut input_buf).is_err() {
        return;
    }

    let input: serde_json::Value = match serde_json::from_str(&input_buf) {
        Ok(v) => v,
        Err(_) => return, // Invalid JSON — exit silently.
    };

    // Extract session_id — exit silently if empty or missing.
    let session_id = input
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if session_id.is_empty() {
        return;
    }

    match event_type.as_str() {
        "pre-tool-use" | "tool-start" => handle_tool_start(&input, session_id),
        "post-tool-use" | "tool-end" => handle_tool_end(&input, session_id),
        "stop" => handle_stop(session_id),
        "session-start" => handle_session_start(&input, session_id),
        "session-end" => handle_session_end(session_id),
        "permission" => handle_permission(&input, session_id),
        _ => {} // Unknown subcommand — exit silently.
    }
}

// ---------------------------------------------------------------------------
// Fire-and-forget handlers (send to hook.sock)
// ---------------------------------------------------------------------------

fn handle_tool_start(input: &serde_json::Value, session_id: &str) {
    let tool_name = input
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_use_id = input
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let detail = extract_tool_detail(input, tool_name);

    let event = HookEvent::ToolStart {
        session_id: session_id.to_string(),
        tool: tool_name.to_string(),
        detail,
        tool_use_id: tool_use_id.to_string(),
    };
    send_hook_event(&event);
}

fn handle_tool_end(input: &serde_json::Value, session_id: &str) {
    let tool_use_id = input
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let event = HookEvent::ToolEnd {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
    };
    send_hook_event(&event);
}

fn handle_stop(session_id: &str) {
    let event = HookEvent::Stop {
        session_id: session_id.to_string(),
    };
    send_hook_event(&event);
}

fn handle_session_start(input: &serde_json::Value, session_id: &str) {
    let cwd = input
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let event = HookEvent::SessionStart {
        session_id: session_id.to_string(),
        cwd,
    };
    send_hook_event(&event);
}

fn handle_session_end(session_id: &str) {
    let event = HookEvent::SessionEnd {
        session_id: session_id.to_string(),
    };
    send_hook_event(&event);
}

// ---------------------------------------------------------------------------
// Permission handler (bidirectional via daemon.sock)
// ---------------------------------------------------------------------------

fn handle_permission(input: &serde_json::Value, session_id: &str) {
    let tool_name = input
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // PermissionRequest hooks don't include tool_use_id; generate a
    // unique request_id so the daemon can track it.
    let request_id = input
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("perm-{}-{}", std::process::id(), timestamp_ms()));

    let detail = extract_tool_detail(input, tool_name);

    let suggestions: Vec<serde_json::Value> = input
        .get("permission_suggestions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let request = ClientRequest::PermissionRequest {
        request_id,
        session_id: session_id.to_string(),
        tool: tool_name.to_string(),
        detail,
        suggestions,
    };

    let sock_path = paths::client_socket_name();
    if !Path::new(&sock_path).exists() {
        // Daemon not running — exit silently (fall through to terminal prompt).
        return;
    }

    let mut stream = match UnixStream::connect(&sock_path) {
        Ok(s) => s,
        Err(_) => return, // Connection failed — exit silently.
    };

    // Send the request as a JSON line.
    let line = match encode_line(&request) {
        Ok(l) => l,
        Err(_) => return,
    };
    if stream.write_all(line.as_bytes()).is_err() {
        return;
    }
    if stream.flush().is_err() {
        return;
    }

    // Set read timeout to 120 seconds.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));

    // Read response line containing HookPermissionDecision.
    let reader = BufReader::new(&stream);
    let mut response_line = String::new();
    if reader.take(65536).read_line(&mut response_line).is_err() {
        // Timeout or read error — exit silently (fall through to terminal prompt).
        return;
    }

    if response_line.trim().is_empty() {
        return;
    }

    let decision: HookPermissionDecision = match serde_json::from_str(response_line.trim()) {
        Ok(d) => d,
        Err(_) => return,
    };

    // Translate the decision to Claude's hook response format.
    let output = translate_permission_decision(&decision, tool_name);
    println!("{output}");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Millisecond timestamp for generating unique IDs.
fn timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Extract a human-readable detail string from the tool input, based on tool name.
fn extract_tool_detail(input: &serde_json::Value, tool_name: &str) -> String {
    let tool_input = input.get("tool_input");

    let detail = match tool_name {
        "Bash" => {
            let cmd = tool_input
                .and_then(|ti| ti.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let desc = tool_input
                .and_then(|ti| ti.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let combined = if desc.is_empty() {
                cmd.to_string()
            } else {
                format!("{cmd}\n{desc}")
            };
            // Truncate to 500 characters (char-boundary safe).
            if combined.chars().count() > 500 {
                combined.chars().take(500).collect()
            } else {
                combined
            }
        }
        "Read" | "Edit" | "Write" => tool_input
            .and_then(|ti| ti.get("file_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Grep" | "Glob" => tool_input
            .and_then(|ti| ti.get("pattern"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebFetch" => tool_input
            .and_then(|ti| ti.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebSearch" => tool_input
            .and_then(|ti| ti.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        name if name.starts_with("Task") => tool_input
            .and_then(|ti| ti.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => tool_name.to_string(),
    };

    detail
}

/// Send a fire-and-forget event to hook.sock. Exits silently on any error.
/// Wraps the event in a HookEnvelope that includes wrapper_id if the
/// AGENT_DASH_WRAPPER_ID env var is set (i.e., running inside `agent-dash run`).
fn send_hook_event(event: &HookEvent) {
    let sock_path = paths::hook_socket_name();
    if !Path::new(&sock_path).exists() {
        return; // Daemon not running.
    }

    let mut stream = match UnixStream::connect(&sock_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    let envelope = HookEnvelope {
        event: event.clone(),
        wrapper_id: std::env::var("AGENT_DASH_WRAPPER_ID").ok(),
    };

    let line = match encode_line(&envelope) {
        Ok(l) => l,
        Err(_) => return,
    };

    let _ = stream.write_all(line.as_bytes());
    // Connection drops when stream goes out of scope — fire and forget.
}

/// Translate a HookPermissionDecision into Claude's hook response JSON format.
fn translate_permission_decision(decision: &HookPermissionDecision, tool_name: &str) -> String {
    match decision.decision.as_str() {
        "deny" => {
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "deny",
                        "message": "Denied from agent-dash"
                    }
                }
            })
            .to_string()
        }
        "allow_similar" => {
            // Legacy path for old agentctl clients that send "allow_similar".
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {
                        "behavior": "allow",
                        "updatedPermissions": [
                            {"tool": tool_name, "permission": "allow"}
                        ]
                    }
                }
            })
            .to_string()
        }
        _ => {
            // Default to allow. If a suggestion is attached, translate it to
            // updatedPermissions so Claude Code applies the "always allow" rule.
            let updated = decision
                .suggestion
                .as_ref()
                .map(|s| build_updated_permissions(s))
                .unwrap_or_default();

            if updated.is_empty() {
                serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PermissionRequest",
                        "decision": {
                            "behavior": "allow"
                        }
                    }
                })
                .to_string()
            } else {
                serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PermissionRequest",
                        "decision": {
                            "behavior": "allow",
                            "updatedPermissions": updated
                        }
                    }
                })
                .to_string()
            }
        }
    }
}

/// Map a permission suggestion object to Claude's `updatedPermissions` array.
fn build_updated_permissions(suggestion: &serde_json::Value) -> Vec<serde_json::Value> {
    let suggestion_type = suggestion
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match suggestion_type {
        "toolAlwaysAllow" => {
            if let Some(tool) = suggestion.get("tool").and_then(|v| v.as_str()) {
                vec![serde_json::json!({"tool": tool, "permission": "allow"})]
            } else {
                vec![]
            }
        }
        _ => vec![], // Unknown suggestion type — fall back to plain allow.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tool_detail_bash_truncates() {
        let long_cmd = "x".repeat(600);
        let input = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": long_cmd}
        });
        let detail = extract_tool_detail(&input, "Bash");
        assert_eq!(detail.len(), 500);
    }

    #[test]
    fn extract_tool_detail_bash_short() {
        let input = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "ls -la"}
        });
        let detail = extract_tool_detail(&input, "Bash");
        assert_eq!(detail, "ls -la");
    }

    #[test]
    fn extract_tool_detail_bash_with_description() {
        let input = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "git push", "description": "Push to remote"}
        });
        let detail = extract_tool_detail(&input, "Bash");
        assert_eq!(detail, "git push\nPush to remote");
    }

    #[test]
    fn extract_tool_detail_read_file_path() {
        let input = serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/test.txt"}
        });
        let detail = extract_tool_detail(&input, "Read");
        assert_eq!(detail, "/tmp/test.txt");
    }

    #[test]
    fn extract_tool_detail_edit_file_path() {
        let input = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": {"file_path": "/tmp/edit.rs"}
        });
        let detail = extract_tool_detail(&input, "Edit");
        assert_eq!(detail, "/tmp/edit.rs");
    }

    #[test]
    fn extract_tool_detail_write_file_path() {
        let input = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/out.txt"}
        });
        let detail = extract_tool_detail(&input, "Write");
        assert_eq!(detail, "/tmp/out.txt");
    }

    #[test]
    fn extract_tool_detail_grep_pattern() {
        let input = serde_json::json!({
            "tool_name": "Grep",
            "tool_input": {"pattern": "fn main"}
        });
        let detail = extract_tool_detail(&input, "Grep");
        assert_eq!(detail, "fn main");
    }

    #[test]
    fn extract_tool_detail_glob_pattern() {
        let input = serde_json::json!({
            "tool_name": "Glob",
            "tool_input": {"pattern": "**/*.rs"}
        });
        let detail = extract_tool_detail(&input, "Glob");
        assert_eq!(detail, "**/*.rs");
    }

    #[test]
    fn extract_tool_detail_webfetch_url() {
        let input = serde_json::json!({
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://example.com"}
        });
        let detail = extract_tool_detail(&input, "WebFetch");
        assert_eq!(detail, "https://example.com");
    }

    #[test]
    fn extract_tool_detail_websearch_query() {
        let input = serde_json::json!({
            "tool_name": "WebSearch",
            "tool_input": {"query": "rust unix socket"}
        });
        let detail = extract_tool_detail(&input, "WebSearch");
        assert_eq!(detail, "rust unix socket");
    }

    #[test]
    fn extract_tool_detail_task_description() {
        let input = serde_json::json!({
            "tool_name": "TaskStart",
            "tool_input": {"description": "Run tests"}
        });
        let detail = extract_tool_detail(&input, "TaskStart");
        assert_eq!(detail, "Run tests");
    }

    #[test]
    fn extract_tool_detail_unknown_tool_returns_name() {
        let input = serde_json::json!({
            "tool_name": "SomeFutureTool",
            "tool_input": {}
        });
        let detail = extract_tool_detail(&input, "SomeFutureTool");
        assert_eq!(detail, "SomeFutureTool");
    }

    #[test]
    fn extract_tool_detail_missing_tool_input() {
        let input = serde_json::json!({
            "tool_name": "Bash"
        });
        let detail = extract_tool_detail(&input, "Bash");
        assert_eq!(detail, "");
    }

    #[test]
    fn translate_allow() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "allow".into(),
            suggestion: None,
        };
        let output = translate_permission_decision(&decision, "Bash");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            parsed["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
        assert!(parsed["hookSpecificOutput"]["decision"]
            .get("message")
            .is_none());
        assert!(parsed["hookSpecificOutput"]["decision"]
            .get("updatedPermissions")
            .is_none());
    }

    #[test]
    fn translate_deny() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "deny".into(),
            suggestion: None,
        };
        let output = translate_permission_decision(&decision, "Bash");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            parsed["hookSpecificOutput"]["decision"]["behavior"],
            "deny"
        );
        assert_eq!(
            parsed["hookSpecificOutput"]["decision"]["message"],
            "Denied from agent-dash"
        );
    }

    #[test]
    fn translate_allow_similar() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "allow_similar".into(),
            suggestion: None,
        };
        let output = translate_permission_decision(&decision, "Bash");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            parsed["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
        let perms = &parsed["hookSpecificOutput"]["decision"]["updatedPermissions"];
        assert_eq!(perms[0]["tool"], "Bash");
        assert_eq!(perms[0]["permission"], "allow");
    }

    #[test]
    fn translate_allow_with_tool_always_allow_suggestion() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "allow".into(),
            suggestion: Some(serde_json::json!({"type": "toolAlwaysAllow", "tool": "Read"})),
        };
        let output = translate_permission_decision(&decision, "Bash");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            parsed["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
        let perms = &parsed["hookSpecificOutput"]["decision"]["updatedPermissions"];
        assert_eq!(perms[0]["tool"], "Read");
        assert_eq!(perms[0]["permission"], "allow");
    }

    #[test]
    fn translate_allow_with_unknown_suggestion_type() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "allow".into(),
            suggestion: Some(serde_json::json!({"type": "someFutureThing"})),
        };
        let output = translate_permission_decision(&decision, "Bash");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            parsed["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
        // Unknown type -> no updatedPermissions
        assert!(parsed["hookSpecificOutput"]["decision"]
            .get("updatedPermissions")
            .is_none());
    }

    #[test]
    fn timestamp_ms_returns_nonzero() {
        let ts = timestamp_ms();
        assert!(ts > 0, "timestamp should be > 0, got {ts}");
    }

    #[test]
    fn request_id_fallback_has_expected_format() {
        // Simulate the fallback request_id generation logic used in handle_permission.
        let pid = std::process::id();
        let ts = timestamp_ms();
        let request_id = format!("perm-{pid}-{ts}");
        assert!(request_id.starts_with("perm-"));
        // Should have three parts: perm, pid, timestamp
        let parts: Vec<&str> = request_id.splitn(3, '-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "perm");
        assert!(parts[1].parse::<u32>().is_ok(), "PID should be numeric");
        assert!(parts[2].parse::<u128>().is_ok(), "timestamp should be numeric");
    }

    #[test]
    fn request_id_uses_tool_use_id_when_present() {
        let input = serde_json::json!({
            "tool_use_id": "tu-abc-123"
        });
        let request_id = input
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("perm-{}-{}", std::process::id(), timestamp_ms()));
        assert_eq!(request_id, "tu-abc-123");
    }

    #[test]
    fn request_id_falls_back_when_tool_use_id_empty() {
        let input = serde_json::json!({
            "tool_use_id": ""
        });
        let request_id = input
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("perm-{}-{}", std::process::id(), timestamp_ms()));
        assert!(request_id.starts_with("perm-"));
    }

    #[test]
    fn request_id_falls_back_when_tool_use_id_missing() {
        let input = serde_json::json!({});
        let request_id = input
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("perm-{}-{}", std::process::id(), timestamp_ms()));
        assert!(request_id.starts_with("perm-"));
    }

    #[test]
    fn translate_unknown_decision_defaults_to_allow() {
        let decision = HookPermissionDecision {
            request_id: "tu1".into(),
            decision: "something_unknown".into(),
            suggestion: None,
        };
        let output = translate_permission_decision(&decision, "Bash");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            parsed["hookSpecificOutput"]["decision"]["behavior"],
            "allow"
        );
    }

    #[test]
    fn build_updated_permissions_tool_always_allow() {
        let suggestion = serde_json::json!({"type": "toolAlwaysAllow", "tool": "Bash"});
        let perms = build_updated_permissions(&suggestion);
        assert_eq!(perms.len(), 1);
        assert_eq!(perms[0]["tool"], "Bash");
        assert_eq!(perms[0]["permission"], "allow");
    }

    #[test]
    fn build_updated_permissions_unknown_type() {
        let suggestion = serde_json::json!({"type": "unknownType"});
        let perms = build_updated_permissions(&suggestion);
        assert!(perms.is_empty());
    }

    #[test]
    fn build_updated_permissions_missing_tool() {
        let suggestion = serde_json::json!({"type": "toolAlwaysAllow"});
        let perms = build_updated_permissions(&suggestion);
        assert!(perms.is_empty());
    }
}
