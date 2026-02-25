use serde::Deserialize;
use std::path::Path;

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
#[allow(dead_code)]
pub struct JsonlStatus {
    pub session_id: String,
    pub git_branch: String,
    pub has_pending_question: bool,
    pub question_text: Option<String>,
}

/// Read the last N lines of a file by seeking from the end.
fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return vec![];
    };
    let Ok(file_len) = file.seek(SeekFrom::End(0)) else {
        return vec![];
    };
    if file_len == 0 {
        return vec![];
    }
    // Read up to 64KB from the end -- more than enough for 20 JSONL lines
    let read_size = file_len.min(64 * 1024);
    let _ = file.seek(SeekFrom::End(-(read_size as i64)));
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);
    let lines: Vec<String> = buf.lines().map(|l| l.to_string()).collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].to_vec()
}

/// Parse the tail of a JSONL file to extract session status.
pub fn parse_jsonl_status(path: &Path) -> Option<JsonlStatus> {
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
            JournalEntry::User {
                session_id: sid,
                git_branch: gb,
            } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jsonl_working_session() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-parse");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = concat!(
            r#"{"type":"assistant","sessionId":"abc-123","gitBranch":"main","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
            "\n",
            r#"{"type":"user","sessionId":"abc-123","gitBranch":"main","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}"#,
            "\n",
        );
        std::fs::write(&path, content).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert_eq!(status.session_id, "abc-123");
        assert_eq!(status.git_branch, "main");
        assert!(!status.has_pending_question);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_jsonl_pending_question() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-question");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let content = r#"{"type":"assistant","sessionId":"s1","gitBranch":"feat","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{"questions":[{"question":"Which approach?"}]}}]}}"#;
        std::fs::write(&path, format!("{}\n", content)).unwrap();
        let status = parse_jsonl_status(&path).unwrap();
        assert!(status.has_pending_question);
        assert_eq!(status.question_text.as_deref(), Some("Which approach?"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_jsonl_no_session_id_returns_none() {
        let dir = std::env::temp_dir().join("agent-dash-test-jsonl-nosid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(&path, r#"{"type":"user","message":{}}"#).unwrap();
        assert!(parse_jsonl_status(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
