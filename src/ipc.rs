use crate::session::PermissionRequest;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
/// Ignores requests older than 150 seconds (the hook times out at 120s).
pub fn scan_pending_permissions() -> Vec<PermissionRequest> {
    let base = ipc_base_dir();
    let Ok(entries) = std::fs::read_dir(&base) else {
        return vec![];
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let session_id = e.file_name().to_string_lossy().to_string();
            let req = read_pending_permission(&session_id)?;
            // Ignore stale requests (hook process likely dead)
            if now.saturating_sub(req.timestamp) > 150 {
                return None;
            }
            Some(req)
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
