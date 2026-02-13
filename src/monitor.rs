use procfs::process::{FDTarget, Process};
use std::collections::HashMap;
use std::path::PathBuf;

/// Info about a discovered claude process.
#[derive(Debug, Clone)]
pub struct ClaudeProcess {
    pub pid: i32,
    pub cwd: PathBuf,
    pub pty: PathBuf,
}

/// Scan /proc for running claude processes.
/// Returns a map of PID -> ClaudeProcess.
pub fn scan_claude_processes() -> HashMap<i32, ClaudeProcess> {
    let mut result = HashMap::new();
    let Ok(all) = procfs::process::all_processes() else {
        return result;
    };
    for proc_entry in all {
        let Ok(proc) = proc_entry else { continue };
        let Ok(cmdline) = proc.cmdline() else { continue };
        // Match processes whose first arg is "claude" (the binary name)
        let is_claude = cmdline.first().is_some_and(|arg| {
            arg == "claude" || arg.ends_with("/claude")
        });
        if !is_claude {
            continue;
        }
        let Ok(cwd) = proc.cwd() else { continue };
        // Read fd 0 (stdin) to find the PTY
        let pty = match proc.fd_from_fd(0) {
            Ok(fd_info) => match fd_info.target {
                FDTarget::Path(p) => p,
                _ => continue,
            },
            Err(_) => continue,
        };
        let pid = proc.pid();
        result.insert(pid, ClaudeProcess { pid, cwd, pty });
    }
    result
}

/// Convert a CWD path to a Claude project slug.
/// e.g., /home/user/src/traider -> -home-user-src-traider
pub fn cwd_to_project_slug(cwd: &std::path::Path) -> String {
    cwd.to_string_lossy().replace('/', "-")
}

/// Extract the project name (last path component) from a CWD.
pub fn project_name_from_cwd(cwd: &std::path::Path) -> String {
    cwd.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Find the most recently modified .jsonl file in a directory.
pub fn find_latest_jsonl(dir: &std::path::Path) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "jsonl")
        })
        .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cwd_to_slug() {
        let cwd = PathBuf::from("/home/user/src/traider");
        assert_eq!(cwd_to_project_slug(&cwd), "-home-user-src-traider");
    }

    #[test]
    fn test_project_name() {
        assert_eq!(
            project_name_from_cwd(&PathBuf::from("/home/user/src/traider")),
            "traider"
        );
    }

    #[test]
    fn test_project_name_worktree() {
        assert_eq!(
            project_name_from_cwd(&PathBuf::from("/home/user/src/traider/.worktrees/backtesting")),
            "backtesting"
        );
    }

    #[test]
    fn test_find_latest_jsonl_empty_dir() {
        let dir = std::env::temp_dir().join("agent-dash-test-empty");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_latest_jsonl(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_find_latest_jsonl() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.jsonl"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(dir.join("new.jsonl"), "{}").unwrap();
        let latest = find_latest_jsonl(&dir).unwrap();
        assert_eq!(latest.file_name().unwrap(), "new.jsonl");
        std::fs::remove_dir_all(&dir).ok();
    }
}
