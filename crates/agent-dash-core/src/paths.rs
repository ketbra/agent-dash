use std::path::{Path, PathBuf};

/// Cache directory for agent-dash runtime files.
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-dash")
}

/// Socket name for hook events (fire-and-forget).
pub fn hook_socket_name() -> String {
    let path = cache_dir().join("hook.sock");
    path.to_string_lossy().to_string()
}

/// Socket name for client connections (bidirectional).
pub fn client_socket_name() -> String {
    let path = cache_dir().join("daemon.sock");
    path.to_string_lossy().to_string()
}

/// Path to the state.json debug/compat file.
pub fn state_file_path() -> PathBuf {
    cache_dir().join("state.json")
}

/// Path to the daemon PID file.
pub fn pid_file_path() -> PathBuf {
    cache_dir().join("daemon.pid")
}

/// Convert a CWD path to a Claude project slug.
/// e.g., /home/user/src/project -> -home-user-src-project
pub fn cwd_to_project_slug(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    s.replace('/', "-").replace('\\', "-")
}

/// Extract project name (last path component) from a CWD.
pub fn project_name_from_cwd(cwd: &Path) -> String {
    cwd.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Config directory for agent-dash settings.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-dash")
}

/// Path to the relay pairing config file.
pub fn relay_config_path() -> PathBuf {
    config_dir().join("relay.json")
}

/// Path to the Claude projects directory.
pub fn claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude")
        .join("projects")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_returns_something() {
        let dir = cache_dir();
        assert!(dir.to_string_lossy().len() > 0);
    }

    #[test]
    fn hook_socket_name_is_consistent() {
        let a = hook_socket_name();
        let b = hook_socket_name();
        assert_eq!(a, b);
    }

    #[test]
    fn client_socket_name_is_consistent() {
        let a = client_socket_name();
        let b = client_socket_name();
        assert_eq!(a, b);
    }

    #[test]
    fn state_file_in_cache_dir() {
        let path = state_file_path();
        assert!(path.to_string_lossy().contains("agent-dash"));
        assert!(path.to_string_lossy().ends_with("state.json"));
    }

    #[test]
    fn slug_unix_style_path() {
        let cwd = std::path::PathBuf::from("/home/user/src/project");
        assert_eq!(cwd_to_project_slug(&cwd), "-home-user-src-project");
    }

    #[test]
    fn slug_preserves_hyphens() {
        let cwd = std::path::PathBuf::from("/home/user/my-project");
        assert_eq!(cwd_to_project_slug(&cwd), "-home-user-my-project");
    }

    #[test]
    fn project_name_from_cwd_uses_last_component() {
        let cwd = std::path::PathBuf::from("/home/user/src/agent-dash");
        assert_eq!(project_name_from_cwd(&cwd), "agent-dash");
    }

    #[test]
    fn claude_projects_dir_under_home() {
        let dir = claude_projects_dir();
        assert!(dir.to_string_lossy().contains(".claude"));
        assert!(dir.to_string_lossy().ends_with("projects"));
    }
}
