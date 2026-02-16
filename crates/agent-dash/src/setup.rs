use serde_json::{Map, Value, json};
use std::fs;
use std::path::PathBuf;

/// The hook events we install, mapping Claude Code event names to agent-dash subcommands.
const HOOK_ENTRIES: &[(&str, &str)] = &[
    ("PreToolUse", "agent-dash hook pre-tool-use"),
    ("PostToolUse", "agent-dash hook post-tool-use"),
    ("PermissionRequest", "agent-dash hook permission"),
    ("Stop", "agent-dash hook stop"),
    ("SessionStart", "agent-dash hook session-start"),
    ("SessionEnd", "agent-dash hook session-end"),
];

/// Install agent-dash hooks into Claude Code's settings.json.
///
/// If `project_level` is true, writes to `.claude/settings.json` relative to CWD.
/// Otherwise writes to `~/.claude/settings.json`.
///
/// Returns `Ok(true)` if changes were made, `Ok(false)` if already up to date.
pub fn install_hooks(project_level: bool) -> Result<bool, String> {
    let path = settings_path(project_level)?;

    let mut settings = read_settings(&path)?;

    let changed = merge_hooks(&mut settings);

    if changed {
        write_settings(&path, &settings)?;
    }

    Ok(changed)
}

/// Quick check whether agent-dash hooks are already installed in user-level settings.
pub fn hooks_installed() -> bool {
    let path = match settings_path(false) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let settings = match read_settings(&path) {
        Ok(s) => s,
        Err(_) => return false,
    };

    has_agent_dash_hook(&settings, "PreToolUse")
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Determine the settings.json path.
fn settings_path(project_level: bool) -> Result<PathBuf, String> {
    if project_level {
        let mut path = std::env::current_dir().map_err(|e| format!("cannot get CWD: {e}"))?;
        path.push(".claude");
        path.push("settings.json");
        Ok(path)
    } else {
        let home = dirs::home_dir().ok_or_else(|| "cannot determine home directory".to_string())?;
        Ok(home.join(".claude").join("settings.json"))
    }
}

/// Read and parse the settings file, returning an empty object if the file does not exist.
fn read_settings(path: &PathBuf) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let contents =
        fs::read_to_string(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))
}

/// Write settings back to disk, creating parent directories as needed.
fn write_settings(path: &PathBuf, settings: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let pretty = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("failed to serialize settings: {e}"))?;
    fs::write(path, pretty.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

/// Merge agent-dash hook entries into the settings JSON object.
/// Returns `true` if any changes were made.
fn merge_hooks(settings: &mut Value) -> bool {
    let root = match settings.as_object_mut() {
        Some(obj) => obj,
        None => return false,
    };

    // Ensure "hooks" key exists as an object.
    if !root.contains_key("hooks") {
        root.insert("hooks".to_string(), json!({}));
    }

    let hooks = match root.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        Some(obj) => obj,
        None => return false,
    };

    let mut changed = false;

    // Clean up invalid event names from previous buggy versions.
    if cleanup_invalid_event(hooks, "NotificationStart") {
        changed = true;
    }

    for &(event_name, command) in HOOK_ENTRIES {
        if merge_single_hook(hooks, event_name, command) {
            changed = true;
        }
    }

    changed
}

/// Remove an invalid hook event key if it only contains agent-dash entries.
/// If the key has non-agent-dash entries mixed in, leave it alone.
fn cleanup_invalid_event(hooks: &mut Map<String, Value>, event_name: &str) -> bool {
    let should_remove = hooks
        .get(event_name)
        .and_then(|v| v.as_array())
        .map(|entries| entries.iter().all(|e| is_agent_dash_entry(e)))
        .unwrap_or(false);

    if should_remove {
        hooks.remove(event_name);
        true
    } else {
        false
    }
}

/// Ensure a single hook event array contains an agent-dash entry with the given command
/// in the correct nested format. Consolidates duplicate agent-dash entries and migrates
/// flat-format entries to nested format.
///
/// Returns `true` if any modification was made.
fn merge_single_hook(hooks: &mut Map<String, Value>, event_name: &str, command: &str) -> bool {
    // Ensure the event key exists as an array.
    if !hooks.contains_key(event_name) {
        hooks.insert(event_name.to_string(), json!([]));
    }

    let entries = match hooks.get_mut(event_name).and_then(|v| v.as_array_mut()) {
        Some(arr) => arr,
        None => return false,
    };

    let target = make_hook_entry(command);

    // Find all agent-dash entries (old or new format, old or new binary name).
    let agent_dash_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| is_agent_dash_entry(entry))
        .map(|(i, _)| i)
        .collect();

    match agent_dash_indices.len() {
        0 => {
            // No agent-dash entry found — append.
            entries.push(target);
            true
        }
        1 => {
            let idx = agent_dash_indices[0];
            if entries[idx] == target {
                false // Already correct format and command.
            } else {
                entries[idx] = target;
                true
            }
        }
        _ => {
            // Multiple agent-dash entries (e.g. old agent-dash-hook + new agent-dash hook).
            // Replace first, remove rest (in reverse order to preserve indices).
            let first_idx = agent_dash_indices[0];
            entries[first_idx] = target;
            for &idx in agent_dash_indices[1..].iter().rev() {
                entries.remove(idx);
            }
            true
        }
    }
}

/// Build a correctly-formatted hook entry for Claude Code's settings.
fn make_hook_entry(command: &str) -> Value {
    json!({"hooks": [{"type": "command", "command": command}]})
}

/// Check whether a hook entry belongs to agent-dash (either the old `agent-dash-hook`
/// binary or the new `agent-dash hook` subcommand, in either flat or nested format).
fn is_agent_dash_entry(entry: &Value) -> bool {
    // Check nested format: {"hooks": [{"command": "agent-dash ...", ...}]}
    if let Some(hooks_arr) = entry.get("hooks").and_then(|v| v.as_array()) {
        for hook in hooks_arr {
            if let Some(cmd) = hook.get("command").and_then(|v| v.as_str()) {
                if is_agent_dash_command(cmd) {
                    return true;
                }
            }
        }
    }

    // Check flat format: {"type": "command", "command": "agent-dash ..."}
    if let Some(cmd) = entry.get("command").and_then(|v| v.as_str()) {
        if is_agent_dash_command(cmd) {
            return true;
        }
    }

    false
}

/// Check if a command string belongs to agent-dash.
fn is_agent_dash_command(cmd: &str) -> bool {
    cmd.starts_with("agent-dash hook") || cmd.contains("agent-dash-hook")
}

/// Check whether a particular hook event array has an agent-dash entry.
fn has_agent_dash_hook(settings: &Value, event_name: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|h| h.get(event_name))
        .and_then(|arr| arr.as_array())
        .map(|entries| entries.iter().any(|entry| is_agent_dash_entry(entry)))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_merge_into_empty_settings() {
        let mut settings = json!({});
        let changed = merge_hooks(&mut settings);
        assert!(changed, "should report changes on empty settings");

        // Verify all hook events were created in nested format.
        let hooks = settings.get("hooks").unwrap().as_object().unwrap();
        for &(event_name, command) in HOOK_ENTRIES {
            let arr = hooks.get(event_name).unwrap().as_array().unwrap();
            assert_eq!(arr.len(), 1, "event {event_name} should have exactly one entry");
            let inner = arr[0]["hooks"].as_array().unwrap();
            assert_eq!(inner.len(), 1);
            assert_eq!(inner[0]["type"], "command");
            assert_eq!(inner[0]["command"], command);
        }
    }

    #[test]
    fn test_preserves_existing_hooks() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "my-linter check"}]}
                ],
                "PostToolUse": [
                    {"hooks": [{"type": "command", "command": "my-formatter format"}]}
                ]
            }
        });

        let changed = merge_hooks(&mut settings);
        assert!(changed, "should report changes when adding agent-dash hooks");

        let hooks = settings.get("hooks").unwrap().as_object().unwrap();

        // PreToolUse should have 2 entries: the original linter + our hook.
        let pre = hooks.get("PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["hooks"][0]["command"], "my-linter check");
        assert_eq!(pre[1]["hooks"][0]["command"], "agent-dash hook pre-tool-use");

        // PostToolUse should have 2 entries: the original formatter + our hook.
        let post = hooks.get("PostToolUse").unwrap().as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert_eq!(post[0]["hooks"][0]["command"], "my-formatter format");
        assert_eq!(post[1]["hooks"][0]["command"], "agent-dash hook post-tool-use");
    }

    #[test]
    fn test_idempotent_no_changes() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook pre-tool-use"}]}
                ],
                "PostToolUse": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook post-tool-use"}]}
                ],
                "PermissionRequest": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook permission"}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook stop"}]}
                ],
                "SessionStart": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook session-start"}]}
                ],
                "SessionEnd": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook session-end"}]}
                ]
            }
        });

        let changed = merge_hooks(&mut settings);
        assert!(!changed, "should report no changes when hooks are already installed");
    }

    #[test]
    fn test_updates_stale_command() {
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook old-command"}]}
                ]
            }
        });

        let changed = merge_hooks(&mut settings);
        assert!(changed, "should report changes when updating stale command");

        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["hooks"][0]["command"], "agent-dash hook pre-tool-use");
    }

    #[test]
    fn test_migrates_flat_format_to_nested() {
        // Simulate entries written by the old buggy setup.rs (flat format).
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "agent-dash hook pre-tool-use"}
                ],
                "PostToolUse": [
                    {"type": "command", "command": "agent-dash hook post-tool-use"}
                ]
            }
        });

        let changed = merge_hooks(&mut settings);
        assert!(changed, "should report changes when migrating flat format");

        // Verify entries are now in nested format.
        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert!(pre[0].get("hooks").is_some(), "should be in nested format");
        assert_eq!(pre[0]["hooks"][0]["command"], "agent-dash hook pre-tool-use");
    }

    #[test]
    fn test_consolidates_duplicate_entries() {
        // Simulate settings with both old agent-dash-hook and new agent-dash hook entries.
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "/usr/bin/agent-dash-hook tool-start"}]},
                    {"type": "command", "command": "agent-dash hook pre-tool-use"}
                ]
            }
        });

        let changed = merge_hooks(&mut settings);
        assert!(changed, "should report changes when consolidating duplicates");

        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1, "should consolidate to a single entry");
        assert_eq!(pre[0]["hooks"][0]["command"], "agent-dash hook pre-tool-use");
    }

    #[test]
    fn test_cleans_up_notification_start() {
        // NotificationStart was an invalid event name we used before.
        let mut settings = json!({
            "hooks": {
                "NotificationStart": [
                    {"type": "command", "command": "agent-dash hook session-start"}
                ]
            }
        });

        let changed = merge_hooks(&mut settings);
        assert!(changed);

        // NotificationStart should be removed.
        assert!(
            settings["hooks"].get("NotificationStart").is_none(),
            "NotificationStart should be removed"
        );

        // SessionStart should exist with correct entry.
        let ss = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 1);
        assert_eq!(ss[0]["hooks"][0]["command"], "agent-dash hook session-start");
    }

    #[test]
    fn test_cleanup_preserves_non_agent_dash_entries() {
        // If NotificationStart has non-agent-dash entries, don't remove it.
        let mut settings = json!({
            "hooks": {
                "NotificationStart": [
                    {"hooks": [{"type": "command", "command": "some-other-tool notify"}]},
                    {"type": "command", "command": "agent-dash hook session-start"}
                ]
            }
        });

        merge_hooks(&mut settings);

        // NotificationStart should still exist (has non-agent-dash entry).
        assert!(
            settings["hooks"].get("NotificationStart").is_some(),
            "NotificationStart should be kept when it has non-agent-dash entries"
        );
    }

    #[test]
    fn test_hooks_installed_check_nested() {
        let settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "agent-dash hook pre-tool-use"}]}
                ]
            }
        });
        assert!(has_agent_dash_hook(&settings, "PreToolUse"));
    }

    #[test]
    fn test_hooks_installed_check_flat() {
        // Old flat format should still be detected as "installed".
        let settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "agent-dash hook pre-tool-use"}
                ]
            }
        });
        assert!(has_agent_dash_hook(&settings, "PreToolUse"));
    }

    #[test]
    fn test_hooks_installed_check_old_binary() {
        // Old agent-dash-hook binary should also be detected.
        let settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "/usr/bin/agent-dash-hook tool-start"}]}
                ]
            }
        });
        assert!(has_agent_dash_hook(&settings, "PreToolUse"));
    }

    #[test]
    fn test_hooks_not_installed_check() {
        let settings = json!({});
        assert!(!has_agent_dash_hook(&settings, "PreToolUse"));
    }

    #[test]
    fn test_hooks_not_installed_different_hook() {
        let settings = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "some-other-tool check"}]}
                ]
            }
        });
        assert!(!has_agent_dash_hook(&settings, "PreToolUse"));
    }

    #[test]
    fn test_preserves_non_hooks_keys() {
        let mut settings = json!({
            "theme": "dark",
            "hooks": {}
        });

        let changed = merge_hooks(&mut settings);
        assert!(changed);

        // The "theme" key should still be present.
        assert_eq!(settings["theme"], "dark");
    }
}
