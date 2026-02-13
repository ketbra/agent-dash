use crate::ipc;
use crate::session::{InputReason, Session, SessionStatus};
use procfs::process::FDTarget;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

/// A parsed JSONL message (we only care about a few fields).
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum JournalEntry {
    #[serde(rename = "assistant")]
    Assistant {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        #[serde(rename = "gitBranch")]
        git_branch: Option<String>,
        message: Option<AssistantMessage>,
    },
    #[serde(rename = "user")]
    User {
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        #[serde(rename = "gitBranch")]
        git_branch: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
    content: Option<Vec<ContentBlock>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "tool_use")]
    ToolUse {
        name: String,
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

/// Info extracted from the tail of a JSONL session file.
#[derive(Debug, Clone)]
pub struct JsonlStatus {
    pub session_id: String,
    pub git_branch: String,
    pub has_pending_question: bool,
    pub question_text: Option<String>,
}

/// Read the last N lines of a file (avoids reading the entire file).
fn read_tail_lines(path: &std::path::Path, max_lines: usize) -> Vec<String> {
    use std::io::{BufRead, BufReader};
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
    let start = all_lines.len().saturating_sub(max_lines);
    all_lines[start..].to_vec()
}

/// Parse the tail of a JSONL file to extract session status.
pub fn parse_jsonl_status(path: &std::path::Path) -> Option<JsonlStatus> {
    let lines = read_tail_lines(path, 20);
    let mut session_id = String::new();
    let mut git_branch = String::new();
    let mut last_was_assistant_with_ask = false;
    let mut question_text: Option<String> = None;
    let mut last_was_user = false;

    for line in &lines {
        let Ok(entry) = serde_json::from_str::<JournalEntry>(line) else {
            continue;
        };
        match entry {
            JournalEntry::Assistant {
                session_id: sid,
                git_branch: gb,
                message,
            } => {
                if let Some(sid) = sid {
                    session_id = sid;
                }
                if let Some(gb) = gb {
                    git_branch = gb;
                }
                last_was_user = false;
                last_was_assistant_with_ask = false;
                question_text = None;
                if let Some(msg) = message {
                    if let Some(content) = msg.content {
                        for block in content {
                            if let ContentBlock::ToolUse { name, input } = block {
                                if name == "AskUserQuestion" {
                                    last_was_assistant_with_ask = true;
                                    // Try to extract the question text
                                    if let Some(qs) = input.get("questions") {
                                        if let Some(arr) = qs.as_array() {
                                            if let Some(first) = arr.first() {
                                                if let Some(q) = first.get("question") {
                                                    question_text =
                                                        q.as_str().map(|s| s.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            JournalEntry::User { session_id: sid, git_branch: gb } => {
                if let Some(sid) = sid {
                    session_id = sid;
                }
                if let Some(gb) = gb {
                    git_branch = gb;
                }
                last_was_user = true;
                last_was_assistant_with_ask = false;
                question_text = None;
            }
            JournalEntry::Other => {}
        }
    }

    if session_id.is_empty() {
        return None;
    }

    Some(JsonlStatus {
        session_id,
        git_branch,
        has_pending_question: last_was_assistant_with_ask && !last_was_user,
        question_text,
    })
}

pub struct SessionMonitor {
    pub sessions: HashMap<String, Session>,
    claude_projects_dir: PathBuf,
}

impl SessionMonitor {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let claude_projects_dir = home.join(".claude").join("projects");
        Self {
            sessions: HashMap::new(),
            claude_projects_dir,
        }
    }

    /// Full refresh: scan processes, parse JSONL, check IPC.
    pub fn refresh(&mut self) {
        let processes = scan_claude_processes();
        let pending_perms = ipc::scan_pending_permissions();
        let perm_map: HashMap<String, _> = pending_perms
            .into_iter()
            .map(|p| (p.session_id.clone(), p))
            .collect();

        // Track which sessions are still alive
        let mut seen_sessions: HashMap<String, Session> = HashMap::new();

        for (_pid, proc_info) in &processes {
            let slug = cwd_to_project_slug(&proc_info.cwd);
            let project_dir = self.claude_projects_dir.join(&slug);
            let Some(jsonl_path) = find_latest_jsonl(&project_dir) else {
                continue;
            };
            let Some(jsonl_status) = parse_jsonl_status(&jsonl_path) else {
                continue;
            };

            let session_id = jsonl_status.session_id.clone();
            // Skip if we already processed this session (dedup for subagents)
            if seen_sessions.contains_key(&session_id) {
                continue;
            }

            let last_modified = std::fs::metadata(&jsonl_path)
                .ok()
                .and_then(|m| m.modified().ok());

            let recently_modified = last_modified.is_some_and(|t| {
                t.elapsed().unwrap_or(Duration::from_secs(999)) < Duration::from_secs(5)
            });

            // Determine status
            let (status, input_reason) = if let Some(perm) = perm_map.get(&session_id) {
                (
                    SessionStatus::NeedsInput,
                    Some(InputReason::Permission(perm.clone())),
                )
            } else if jsonl_status.has_pending_question {
                (
                    SessionStatus::NeedsInput,
                    Some(InputReason::Question {
                        text: jsonl_status
                            .question_text
                            .unwrap_or_else(|| "Agent has a question".to_string()),
                    }),
                )
            } else if recently_modified {
                (SessionStatus::Working, None)
            } else {
                (SessionStatus::Idle, None)
            };

            let project_name = project_name_from_cwd(&proc_info.cwd);

            seen_sessions.insert(
                session_id.clone(),
                Session {
                    session_id,
                    pid: proc_info.pid,
                    pty: proc_info.pty.clone(),
                    cwd: proc_info.cwd.clone(),
                    project_name,
                    branch: jsonl_status.git_branch,
                    status,
                    input_reason,
                    jsonl_path,
                    last_jsonl_modified: last_modified,
                    ended_at: None,
                },
            );
        }

        // Handle sessions that disappeared: mark as Ended
        for (sid, existing) in &self.sessions {
            if !seen_sessions.contains_key(sid) && existing.status != SessionStatus::Ended {
                let mut ended = existing.clone();
                ended.status = SessionStatus::Ended;
                ended.ended_at = Some(existing.ended_at.unwrap_or_else(Instant::now));
                seen_sessions.insert(sid.clone(), ended);
            }
        }

        // Remove sessions that have been ended for >5 seconds
        seen_sessions.retain(|_, s| {
            if let Some(ended_at) = s.ended_at {
                ended_at.elapsed() < Duration::from_secs(5)
            } else {
                true
            }
        });

        self.sessions = seen_sessions;
    }

    /// Get sessions sorted by status priority (red first, then yellow, then green).
    pub fn sorted_sessions(&self) -> Vec<&Session> {
        let mut sessions: Vec<&Session> = self.sessions.values().collect();
        sessions.sort_by_key(|s| s.status.sort_key());
        sessions
    }
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

    #[test]
    fn test_parse_jsonl_working_session() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-parse");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}
{"type":"user","sessionId":"abc-123","gitBranch":"main","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}
{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file":"foo.rs"}}]}}
"#;
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert_eq!(status.session_id, "abc-123");
        assert_eq!(status.git_branch, "main");
        assert!(!status.has_pending_question);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_parse_jsonl_pending_question() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-question");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"feat","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"Which approach?","header":"Approach","options":[{"label":"A"},{"label":"B"}]}]}}]}}
"#;
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert_eq!(status.session_id, "abc-123");
        assert_eq!(status.git_branch, "feat");
        assert!(status.has_pending_question);
        assert_eq!(status.question_text.as_deref(), Some("Which approach?"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_parse_jsonl_answered_question() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-answered");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"Which?"}]}}]}}
{"type":"user","sessionId":"abc-123","gitBranch":"main","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"Option A"}]}}
"#;
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert!(!status.has_pending_question);
        std::fs::remove_dir_all(&dir).ok();
    }
}
