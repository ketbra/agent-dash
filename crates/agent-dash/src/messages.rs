//! Parse Claude Code JSONL transcript files into `ChatMessage` structs.
//!
//! Supports three output formats:
//! - `"structured"` — returns `ChatContent::Structured(Vec<ContentBlock>)`
//! - `"markdown"`   — returns `ChatContent::Rendered(String)` with markdown
//! - `"html"`       — returns `ChatContent::Rendered(String)` with HTML via comrak

use std::fs::File;
use std::io::{Read as _, Seek, SeekFrom};
use std::path::Path;

use agent_dash_core::protocol::{ChatContent, ChatMessage, ContentBlock};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse JSONL lines into `ChatMessage`s.
///
/// `format` must be `"structured"`, `"markdown"`, or `"html"`.
/// Lines that don't represent user/assistant messages are silently skipped.
pub fn parse_lines(lines: &[String], format: &str) -> Vec<ChatMessage> {
    lines
        .iter()
        .filter_map(|line| parse_one_line(line, format))
        .collect()
}

/// Read the tail of a JSONL file and return the last `limit` conversation
/// messages.
pub fn read_messages(path: &Path, limit: usize, format: &str) -> Vec<ChatMessage> {
    let lines = read_tail_lines(path);
    let all = parse_lines(&lines, format);
    if all.len() > limit {
        all[all.len() - limit..].to_vec()
    } else {
        all
    }
}

/// Read new lines from a JSONL file starting at byte `offset`.
///
/// Returns the parsed messages and the new byte offset (for the next call).
pub fn read_new_messages(path: &Path, offset: u64, format: &str) -> (Vec<ChatMessage>, u64) {
    let Ok(mut file) = File::open(path) else {
        return (vec![], offset);
    };

    let Ok(metadata) = file.metadata() else {
        return (vec![], offset);
    };

    let file_len = metadata.len();
    if file_len <= offset {
        return (vec![], offset);
    }

    if file.seek(SeekFrom::Start(offset)).is_err() {
        return (vec![], offset);
    }

    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return (vec![], offset);
    }

    let lines: Vec<String> = buf
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();

    let messages = parse_lines(&lines, format);
    (messages, file_len)
}

// ---------------------------------------------------------------------------
// Internal: per-line parsing
// ---------------------------------------------------------------------------

/// Try to parse a single JSONL line into a ChatMessage.
fn parse_one_line(line: &str, format: &str) -> Option<ChatMessage> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let v: Value = serde_json::from_str(trimmed).ok()?;
    let obj = v.as_object()?;

    let msg_type = obj.get("type")?.as_str()?;
    let role = match msg_type {
        "user" | "human" => "user",
        "assistant" => "assistant",
        _ => return None, // skip file-history-snapshot, system, etc.
    };

    let message = obj.get("message")?;
    let content_val = message.get("content")?;

    let blocks = parse_content_blocks(role, content_val);
    if blocks.is_empty() {
        return None;
    }

    let content = match format {
        "markdown" => ChatContent::Rendered(render_markdown(&blocks)),
        "html" => ChatContent::Rendered(render_html(role, &blocks)),
        _ => ChatContent::Structured(blocks), // "structured" or anything else
    };

    Some(ChatMessage {
        role: role.to_string(),
        content,
    })
}

// ---------------------------------------------------------------------------
// Content block parsing
// ---------------------------------------------------------------------------

/// Convert a raw JSON `content` value into a `Vec<ContentBlock>`.
///
/// Content can be:
/// - a plain string (user messages)
/// - an array of objects with `type` fields (assistant/user tool results)
fn parse_content_blocks(role: &str, content_val: &Value) -> Vec<ContentBlock> {
    match content_val {
        Value::String(s) => {
            if s.is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text { text: s.clone() }]
            }
        }
        Value::Array(arr) => arr.iter().filter_map(|item| parse_one_block(role, item)).collect(),
        _ => vec![],
    }
}

/// Parse a single content block from a JSON object.
fn parse_one_block(role: &str, item: &Value) -> Option<ContentBlock> {
    let obj = item.as_object()?;
    let block_type = obj.get("type")?.as_str()?;

    match block_type {
        "text" => {
            let text = obj.get("text")?.as_str()?.to_string();
            if text.is_empty() {
                None
            } else {
                Some(ContentBlock::Text { text })
            }
        }
        "tool_use" => {
            let name = obj.get("name")?.as_str()?.to_string();
            let input = obj.get("input").cloned();
            let detail = extract_tool_detail(&name, &input);
            Some(ContentBlock::ToolUse {
                name,
                detail,
                input,
            })
        }
        "tool_result" => {
            // tool_result blocks appear in user messages
            let _ = role; // acknowledged but not needed for now
            let name = tool_name_from_result(obj);
            let output = extract_tool_result_output(obj);
            Some(ContentBlock::ToolResult { name, output })
        }
        _ => None,
    }
}

/// Extract a human-readable detail string from tool input JSON.
fn extract_tool_detail(name: &str, input: &Option<Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let obj = match input.as_object() {
        Some(o) => o,
        None => return String::new(),
    };

    match name {
        "Bash" => obj
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Read" | "Edit" | "Write" => obj
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Grep" => obj
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Glob" => obj
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebFetch" => obj
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebSearch" => obj
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Task" | "Skill" => obj
            .get("description")
            .or_else(|| obj.get("skill"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "NotebookEdit" => obj
            .get("notebook_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            // Fallback: try common key names
            for key in &["file_path", "path", "command", "pattern", "query", "url"] {
                if let Some(val) = obj.get(*key).and_then(|v| v.as_str()) {
                    return val.to_string();
                }
            }
            String::new()
        }
    }
}

/// Try to extract a tool name from a tool_result block.
///
/// Claude Code JSONL doesn't always include the tool name in tool_result
/// blocks, so we fall back to "tool" if absent.
fn tool_name_from_result(obj: &serde_json::Map<String, Value>) -> String {
    obj.get("name")
        .or_else(|| obj.get("tool_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("tool")
        .to_string()
}

/// Extract the text output from a tool_result block.
///
/// The `content` field can be a plain string or an array of text blocks.
fn extract_tool_result_output(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let content = obj.get("content")?;
    match content {
        Value::String(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        Value::Array(arr) => {
            let parts: Vec<&str> = arr
                .iter()
                .filter_map(|item| {
                    if item.get("type")?.as_str()? == "text" {
                        item.get("text")?.as_str()
                    } else {
                        None
                    }
                })
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render content blocks as markdown text.
///
/// - Text blocks: passed through as-is.
/// - Tool use: `> **ToolName**: \`detail\``
/// - Tool result: output in a blockquoted code block, truncated at 500 chars.
fn render_markdown(blocks: &[ContentBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text { text } => {
                parts.push(text.clone());
            }
            ContentBlock::ToolUse { name, detail, .. } => {
                if detail.is_empty() {
                    parts.push(format!("> **{name}**"));
                } else {
                    parts.push(format!("> **{name}**: `{detail}`"));
                }
            }
            ContentBlock::ToolResult { output, .. } => {
                if let Some(output) = output {
                    let truncated = truncate_output(output, 500);
                    // Blockquoted fenced code block
                    let mut lines = String::from("> ```\n");
                    for line in truncated.lines() {
                        lines.push_str("> ");
                        lines.push_str(line);
                        lines.push('\n');
                    }
                    lines.push_str("> ```");
                    parts.push(lines);
                }
            }
        }
    }

    parts.join("\n\n")
}

/// Render content blocks as HTML with Claude Code terminal-style formatting.
///
/// - Text blocks: bullet-prefixed paragraphs (assistant) or plain text (user),
///   with inline markdown (backticks, bold, italic) rendered via comrak.
/// - Tool use: `• ToolName(detail)` with bold tool name.
/// - Tool result: box-drawing nested output with collapsible long content.
fn render_html(role: &str, blocks: &[ContentBlock]) -> String {
    const VISIBLE_LINES: usize = 3;
    const COLLAPSE_THRESHOLD: usize = 5;
    const DETAIL_MAX_LEN: usize = 80;

    let mut html = String::new();

    for block in blocks {
        match block {
            ContentBlock::Text { text } => {
                // Render inline markdown (backticks, bold, italic) via comrak,
                // then strip the outer <p> wrapper to get inline HTML.
                let rendered = comrak::markdown_to_html(text, &comrak::Options::default());
                let inline = strip_outer_p(&rendered);

                if role == "user" {
                    html.push_str(&format!(
                        "<div class=\"block-text\">{}</div>",
                        inline
                    ));
                } else {
                    html.push_str(&format!(
                        "<div class=\"block-text\">\u{2022} {}</div>",
                        inline
                    ));
                }
            }
            ContentBlock::ToolUse { name, detail, .. } => {
                let escaped_name = escape_html(name);
                if detail.is_empty() {
                    html.push_str(&format!(
                        "<div class=\"block-tool-use\">\u{2022} <strong>{}</strong></div>",
                        escaped_name
                    ));
                } else {
                    let truncated = truncate_detail(detail, DETAIL_MAX_LEN);
                    let escaped_detail = escape_html(&truncated);
                    html.push_str(&format!(
                        "<div class=\"block-tool-use\">\u{2022} <strong>{}</strong>(<span class=\"tool-detail\">{}</span>)</div>",
                        escaped_name, escaped_detail
                    ));
                }
            }
            ContentBlock::ToolResult { output, .. } => {
                if let Some(output) = output {
                    let lines: Vec<&str> = output.lines().collect();
                    let total = lines.len();

                    if total == 0 {
                        continue;
                    }

                    html.push_str("<div class=\"block-tool-result\">");

                    if total <= COLLAPSE_THRESHOLD {
                        // Show all lines with box-drawing prefix.
                        for (i, line) in lines.iter().enumerate() {
                            let prefix = if i == 0 { "\u{2514}\u{2500} " } else { "   " };
                            html.push_str(&format!(
                                "<div class=\"tool-output-line\">{}{}</div>",
                                prefix,
                                escape_html(line)
                            ));
                        }
                    } else {
                        // Show first VISIBLE_LINES, collapse the rest.
                        for (i, line) in lines[..VISIBLE_LINES].iter().enumerate() {
                            let prefix = if i == 0 { "\u{2514}\u{2500} " } else { "   " };
                            html.push_str(&format!(
                                "<div class=\"tool-output-line\">{}{}</div>",
                                prefix,
                                escape_html(line)
                            ));
                        }
                        let remaining = total - VISIBLE_LINES;
                        html.push_str(&format!(
                            "<details class=\"tool-output-collapsed\"><summary>   \u{2026} +{} lines (click to expand)</summary><pre class=\"tool-output-full\">",
                            remaining
                        ));
                        for line in &lines[VISIBLE_LINES..] {
                            html.push_str(&escape_html(line));
                            html.push('\n');
                        }
                        html.push_str("</pre></details>");
                    }

                    html.push_str("</div>");
                }
            }
        }
    }

    html
}

/// Strip the outer `<p>...</p>\n` wrapper that comrak adds around inline text,
/// leaving inner HTML (code, strong, em) intact. If the content has multiple
/// `<p>` blocks, return as-is.
fn strip_outer_p(html: &str) -> String {
    let trimmed = html.trim();
    // Only strip if there's exactly one <p> wrapper.
    if trimmed.starts_with("<p>")
        && trimmed.ends_with("</p>")
        && trimmed.matches("<p>").count() == 1
    {
        trimmed[3..trimmed.len() - 4].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Escape HTML special characters to prevent XSS and rendering issues.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// Truncate a tool detail string for display in a header line.
///
/// Replaces newlines with spaces and truncates to `max_len` chars with an
/// ellipsis if needed.
fn truncate_detail(s: &str, max_len: usize) -> String {
    let oneline: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if oneline.len() <= max_len {
        oneline
    } else {
        let mut end = max_len;
        while end > 0 && !oneline.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\u{2026}", &oneline[..end])
    }
}

/// Truncate a string to at most `max_chars` characters, appending "..." if
/// truncated.
fn truncate_output(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        // Find a char boundary at or before max_chars
        let mut end = max_chars;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

// ---------------------------------------------------------------------------
// File I/O
// ---------------------------------------------------------------------------

/// Read the last ~128KB of a file and split into non-empty lines.
fn read_tail_lines(path: &Path) -> Vec<String> {
    const TAIL_BYTES: u64 = 128 * 1024;

    let Ok(mut file) = File::open(path) else {
        return vec![];
    };

    let Ok(metadata) = file.metadata() else {
        return vec![];
    };

    let file_len = metadata.len();
    let start = file_len.saturating_sub(TAIL_BYTES);

    if start > 0 {
        if file.seek(SeekFrom::Start(start)).is_err() {
            return vec![];
        }
    }

    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return vec![];
    }

    // If we seeked into the middle of the file, drop the first (likely
    // partial) line.
    let text = if start > 0 {
        match buf.find('\n') {
            Some(idx) => &buf[idx + 1..],
            None => &buf,
        }
    } else {
        &buf
    };

    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Create a unique temporary directory for each test.
    fn test_dir() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join("agent-dashd-tests")
            .join(format!("{}_{}", std::process::id(), id));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- extract_tool_detail -------------------------------------------------

    #[test]
    fn tool_detail_bash() {
        let input = Some(serde_json::json!({"command": "ls -la"}));
        assert_eq!(extract_tool_detail("Bash", &input), "ls -la");
    }

    #[test]
    fn tool_detail_read() {
        let input = Some(serde_json::json!({"file_path": "/tmp/foo.rs"}));
        assert_eq!(extract_tool_detail("Read", &input), "/tmp/foo.rs");
    }

    #[test]
    fn tool_detail_edit() {
        let input = Some(serde_json::json!({"file_path": "/tmp/bar.rs", "old_string": "a", "new_string": "b"}));
        assert_eq!(extract_tool_detail("Edit", &input), "/tmp/bar.rs");
    }

    #[test]
    fn tool_detail_grep() {
        let input = Some(serde_json::json!({"pattern": "fn main"}));
        assert_eq!(extract_tool_detail("Grep", &input), "fn main");
    }

    #[test]
    fn tool_detail_glob() {
        let input = Some(serde_json::json!({"pattern": "**/*.rs"}));
        assert_eq!(extract_tool_detail("Glob", &input), "**/*.rs");
    }

    #[test]
    fn tool_detail_web_fetch() {
        let input = Some(serde_json::json!({"url": "https://example.com"}));
        assert_eq!(extract_tool_detail("WebFetch", &input), "https://example.com");
    }

    #[test]
    fn tool_detail_web_search() {
        let input = Some(serde_json::json!({"query": "rust async"}));
        assert_eq!(extract_tool_detail("WebSearch", &input), "rust async");
    }

    #[test]
    fn tool_detail_unknown_fallback() {
        let input = Some(serde_json::json!({"file_path": "/x/y"}));
        assert_eq!(extract_tool_detail("FutureTool", &input), "/x/y");
    }

    #[test]
    fn tool_detail_none_input() {
        assert_eq!(extract_tool_detail("Bash", &None), "");
    }

    #[test]
    fn tool_detail_empty_object() {
        let input = Some(serde_json::json!({}));
        assert_eq!(extract_tool_detail("Bash", &input), "");
    }

    // -- parse_content_blocks ------------------------------------------------

    #[test]
    fn parse_plain_string_content() {
        let val = Value::String("hello world".into());
        let blocks = parse_content_blocks("user", &val);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "hello world"));
    }

    #[test]
    fn parse_empty_string_content() {
        let val = Value::String(String::new());
        let blocks = parse_content_blocks("user", &val);
        assert!(blocks.is_empty());
    }

    #[test]
    fn parse_assistant_text_and_tool_use() {
        let val = serde_json::json!([
            {"type": "text", "text": "Let me check."},
            {"type": "tool_use", "id": "tu1", "name": "Bash", "input": {"command": "ls"}}
        ]);
        let blocks = parse_content_blocks("assistant", &val);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Let me check."));
        match &blocks[1] {
            ContentBlock::ToolUse { name, detail, .. } => {
                assert_eq!(name, "Bash");
                assert_eq!(detail, "ls");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn parse_tool_result_string_content() {
        let val = serde_json::json!([
            {"type": "tool_result", "tool_use_id": "tu1", "content": "file1.rs\nfile2.rs"}
        ]);
        let blocks = parse_content_blocks("user", &val);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolResult { name, output } => {
                assert_eq!(name, "tool"); // no name in tool_result
                assert_eq!(output.as_deref(), Some("file1.rs\nfile2.rs"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn parse_tool_result_array_content() {
        let val = serde_json::json!([
            {
                "type": "tool_result",
                "tool_use_id": "tu1",
                "content": [
                    {"type": "text", "text": "line1"},
                    {"type": "text", "text": "line2"}
                ]
            }
        ]);
        let blocks = parse_content_blocks("user", &val);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::ToolResult { output, .. } => {
                assert_eq!(output.as_deref(), Some("line1\nline2"));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn parse_skips_unknown_block_types() {
        let val = serde_json::json!([
            {"type": "image", "source": {}},
            {"type": "text", "text": "visible"}
        ]);
        let blocks = parse_content_blocks("user", &val);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "visible"));
    }

    // -- parse_one_line / parse_lines ----------------------------------------

    #[test]
    fn parse_assistant_line_structured() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello!"}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "assistant");
        match &msgs[0].content {
            ChatContent::Structured(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Hello!"));
            }
            _ => panic!("expected Structured"),
        }
    }

    #[test]
    fn parse_user_plain_string() {
        let line = r#"{"type":"user","message":{"content":"What is 2+2?"}}"#;
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn parse_skips_file_history_snapshot() {
        let line = r#"{"type":"file-history-snapshot","message":{"content":"ignored"}}"#;
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert!(msgs.is_empty());
    }

    #[test]
    fn parse_skips_invalid_json() {
        let line = "not json at all";
        let msgs = parse_lines(&[line.to_string()], "structured");
        assert!(msgs.is_empty());
    }

    #[test]
    fn parse_skips_empty_lines() {
        let msgs = parse_lines(&["  ".to_string(), "".to_string()], "structured");
        assert!(msgs.is_empty());
    }

    #[test]
    fn parse_multiple_lines() {
        let lines = vec![
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hey"}]}}"#
                .to_string(),
            r#"{"type":"file-history-snapshot","message":{"content":"x"}}"#.to_string(),
        ];
        let msgs = parse_lines(&lines, "structured");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    // -- render_markdown -----------------------------------------------------

    #[test]
    fn markdown_text_only() {
        let blocks = vec![ContentBlock::Text {
            text: "Hello world".into(),
        }];
        let md = render_markdown(&blocks);
        assert_eq!(md, "Hello world");
    }

    #[test]
    fn markdown_tool_use_with_detail() {
        let blocks = vec![ContentBlock::ToolUse {
            name: "Bash".into(),
            detail: "ls -la".into(),
            input: None,
        }];
        let md = render_markdown(&blocks);
        assert_eq!(md, "> **Bash**: `ls -la`");
    }

    #[test]
    fn markdown_tool_use_no_detail() {
        let blocks = vec![ContentBlock::ToolUse {
            name: "Bash".into(),
            detail: String::new(),
            input: None,
        }];
        let md = render_markdown(&blocks);
        assert_eq!(md, "> **Bash**");
    }

    #[test]
    fn markdown_tool_result_with_output() {
        let blocks = vec![ContentBlock::ToolResult {
            name: "Bash".into(),
            output: Some("file1.rs\nfile2.rs".into()),
        }];
        let md = render_markdown(&blocks);
        assert!(md.contains("> ```"));
        assert!(md.contains("> file1.rs"));
        assert!(md.contains("> file2.rs"));
    }

    #[test]
    fn markdown_tool_result_no_output() {
        let blocks = vec![ContentBlock::ToolResult {
            name: "Bash".into(),
            output: None,
        }];
        let md = render_markdown(&blocks);
        assert!(md.is_empty());
    }

    #[test]
    fn markdown_mixed_blocks() {
        let blocks = vec![
            ContentBlock::Text {
                text: "Let me check.".into(),
            },
            ContentBlock::ToolUse {
                name: "Bash".into(),
                detail: "ls".into(),
                input: None,
            },
        ];
        let md = render_markdown(&blocks);
        assert!(md.contains("Let me check."));
        assert!(md.contains("> **Bash**: `ls`"));
    }

    // -- render_html ---------------------------------------------------------

    #[test]
    fn html_assistant_text_has_bullet() {
        let blocks = vec![ContentBlock::Text {
            text: "Hello".into(),
        }];
        let html = render_html("assistant", &blocks);
        assert!(html.contains("block-text"));
        assert!(html.contains("\u{2022} Hello"));
    }

    #[test]
    fn html_user_text_no_bullet() {
        let blocks = vec![ContentBlock::Text {
            text: "Hello".into(),
        }];
        let html = render_html("user", &blocks);
        assert!(html.contains("block-text"));
        assert!(html.contains("Hello"));
        assert!(!html.contains("\u{2022}"));
    }

    #[test]
    fn html_renders_tool_bold() {
        let blocks = vec![ContentBlock::ToolUse {
            name: "Bash".into(),
            detail: "ls".into(),
            input: None,
        }];
        let html = render_html("assistant", &blocks);
        assert!(html.contains("<strong>Bash</strong>"));
        assert!(html.contains("tool-detail"));
        assert!(html.contains("ls"));
    }

    #[test]
    fn html_tool_use_truncates_long_detail() {
        let long_detail = "x".repeat(120);
        let blocks = vec![ContentBlock::ToolUse {
            name: "Bash".into(),
            detail: long_detail.clone(),
            input: None,
        }];
        let html = render_html("assistant", &blocks);
        assert!(html.contains("\u{2026}")); // ellipsis
        assert!(!html.contains(&long_detail));
    }

    #[test]
    fn html_tool_result_short_no_collapse() {
        let blocks = vec![ContentBlock::ToolResult {
            name: "Bash".into(),
            output: Some("line1\nline2\nline3".into()),
        }];
        let html = render_html("assistant", &blocks);
        assert!(html.contains("tool-output-line"));
        assert!(html.contains("\u{2514}\u{2500}")); // box-drawing on first line
        assert!(html.contains("line3"));
        assert!(!html.contains("<details"));
    }

    #[test]
    fn html_tool_result_long_collapses() {
        let lines: Vec<String> = (1..=10).map(|i| format!("line {}", i)).collect();
        let output = lines.join("\n");
        let blocks = vec![ContentBlock::ToolResult {
            name: "Bash".into(),
            output: Some(output),
        }];
        let html = render_html("assistant", &blocks);
        assert!(html.contains("<details"));
        assert!(html.contains("+7 lines"));
        assert!(html.contains("click to expand"));
        // First 3 lines visible, rest collapsed
        assert!(html.contains("line 1"));
        assert!(html.contains("line 3"));
    }

    #[test]
    fn html_escapes_special_chars() {
        let blocks = vec![ContentBlock::Text {
            text: "<script>alert('xss')</script>".into(),
        }];
        let html = render_html("assistant", &blocks);
        // Must not contain raw <script> tags — comrak escapes or strips them
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn html_escapes_tool_output() {
        // Tool results bypass comrak — verify our escape_html works
        let blocks = vec![ContentBlock::ToolResult {
            name: "Bash".into(),
            output: Some("<b>bold</b> & \"quoted\"".into()),
        }];
        let html = render_html("assistant", &blocks);
        assert!(!html.contains("<b>bold</b>"));
        assert!(html.contains("&lt;b&gt;bold&lt;/b&gt;"));
        assert!(html.contains("&amp;"));
    }

    #[test]
    fn html_tool_detail_newlines_replaced() {
        let blocks = vec![ContentBlock::ToolUse {
            name: "Bash".into(),
            detail: "echo hello\necho world".into(),
            input: None,
        }];
        let html = render_html("assistant", &blocks);
        assert!(html.contains("echo hello echo world"));
    }

    #[test]
    fn html_preserves_inline_code() {
        let blocks = vec![ContentBlock::Text {
            text: "Run `cargo test` now".into(),
        }];
        let html = render_html("assistant", &blocks);
        assert!(html.contains("<code>cargo test</code>"));
    }

    // -- truncate_output -----------------------------------------------------

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate_output("hello", 500), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "x".repeat(600);
        let t = truncate_output(&long, 500);
        assert!(t.ends_with("..."));
        assert_eq!(t.len(), 503); // 500 + "..."
    }

    // -- format selection in parse_lines -------------------------------------

    #[test]
    fn parse_lines_markdown_format() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "markdown");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Rendered(s) => assert_eq!(s, "hi"),
            _ => panic!("expected Rendered"),
        }
    }

    #[test]
    fn parse_lines_html_format() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        let msgs = parse_lines(&[line.to_string()], "html");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Rendered(s) => assert!(s.contains("block-text")),
            _ => panic!("expected Rendered"),
        }
    }

    // -- read_messages / read_new_messages (file-based tests) ----------------

    #[test]
    fn read_messages_from_file() {
        let dir = test_dir();
        let path = dir.join("test.jsonl");
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, r#"{{"type":"user","message":{{"content":"q1"}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"a1"}}]}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"user","message":{{"content":"q2"}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"a2"}}]}}}}"#).unwrap();
        }

        let msgs = read_messages(&path, 2, "structured");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn read_messages_limit_larger_than_total() {
        let dir = test_dir();
        let path = dir.join("test.jsonl");
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, r#"{{"type":"user","message":{{"content":"q1"}}}}"#).unwrap();
        }
        let msgs = read_messages(&path, 100, "structured");
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn read_messages_nonexistent_file() {
        let msgs = read_messages(Path::new("/nonexistent/path.jsonl"), 10, "structured");
        assert!(msgs.is_empty());
    }

    #[test]
    fn read_new_messages_incremental() {
        let dir = test_dir();
        let path = dir.join("test.jsonl");

        // Write first message
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, r#"{{"type":"user","message":{{"content":"q1"}}}}"#).unwrap();
        }

        let (msgs1, offset1) = read_new_messages(&path, 0, "structured");
        assert_eq!(msgs1.len(), 1);
        assert!(offset1 > 0);

        // Append second message
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"a1"}}]}}}}"#).unwrap();
        }

        let (msgs2, offset2) = read_new_messages(&path, offset1, "structured");
        assert_eq!(msgs2.len(), 1);
        assert_eq!(msgs2[0].role, "assistant");
        assert!(offset2 > offset1);

        // No new data
        let (msgs3, offset3) = read_new_messages(&path, offset2, "structured");
        assert!(msgs3.is_empty());
        assert_eq!(offset3, offset2);
    }

    #[test]
    fn read_new_messages_nonexistent_file() {
        let (msgs, offset) = read_new_messages(Path::new("/nonexistent.jsonl"), 0, "structured");
        assert!(msgs.is_empty());
        assert_eq!(offset, 0);
    }

    // -- read_tail_lines -----------------------------------------------------

    #[test]
    fn read_tail_lines_small_file() {
        let dir = test_dir();
        let path = dir.join("small.jsonl");
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, "line1").unwrap();
            writeln!(f, "line2").unwrap();
            writeln!(f, "line3").unwrap();
        }
        let lines = read_tail_lines(&path);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn read_tail_lines_nonexistent() {
        let lines = read_tail_lines(Path::new("/nonexistent.jsonl"));
        assert!(lines.is_empty());
    }

    #[test]
    fn read_tail_lines_skips_empty() {
        let dir = test_dir();
        let path = dir.join("gaps.jsonl");
        {
            let mut f = File::create(&path).unwrap();
            writeln!(f, "line1").unwrap();
            writeln!(f).unwrap(); // empty line
            writeln!(f, "line2").unwrap();
        }
        let lines = read_tail_lines(&path);
        assert_eq!(lines, vec!["line1", "line2"]);
    }

    // -- Integration: full pipeline ------------------------------------------

    #[test]
    fn full_pipeline_structured() {
        let lines = vec![
            r#"{"type":"user","message":{"content":"Explain closures"}}"#.to_string(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"A closure captures variables from its environment."},{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"rustc --edition 2021 example.rs"}}]}}"#.to_string(),
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"Compiling example.rs\nDone."}]}}"#.to_string(),
        ];
        let msgs = parse_lines(&lines, "structured");
        assert_eq!(msgs.len(), 3);

        // user question
        assert_eq!(msgs[0].role, "user");
        match &msgs[0].content {
            ChatContent::Structured(blocks) => {
                assert_eq!(blocks.len(), 1);
                assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Explain closures"));
            }
            _ => panic!("expected Structured"),
        }

        // assistant with text + tool_use
        assert_eq!(msgs[1].role, "assistant");
        match &msgs[1].content {
            ChatContent::Structured(blocks) => {
                assert_eq!(blocks.len(), 2);
                match &blocks[1] {
                    ContentBlock::ToolUse { name, detail, .. } => {
                        assert_eq!(name, "Bash");
                        assert_eq!(detail, "rustc --edition 2021 example.rs");
                    }
                    _ => panic!("expected ToolUse"),
                }
            }
            _ => panic!("expected Structured"),
        }

        // user tool_result
        assert_eq!(msgs[2].role, "user");
        match &msgs[2].content {
            ChatContent::Structured(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::ToolResult { output, .. } => {
                        assert_eq!(
                            output.as_deref(),
                            Some("Compiling example.rs\nDone.")
                        );
                    }
                    _ => panic!("expected ToolResult"),
                }
            }
            _ => panic!("expected Structured"),
        }
    }

    #[test]
    fn full_pipeline_markdown() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Here is the result."},{"type":"tool_use","id":"tu1","name":"Read","input":{"file_path":"/tmp/foo.rs"}}]}}"#.to_string(),
        ];
        let msgs = parse_lines(&lines, "markdown");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Rendered(md) => {
                assert!(md.contains("Here is the result."));
                assert!(md.contains("> **Read**: `/tmp/foo.rs`"));
            }
            _ => panic!("expected Rendered"),
        }
    }

    #[test]
    fn full_pipeline_html() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#
                .to_string(),
        ];
        let msgs = parse_lines(&lines, "html");
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            ChatContent::Rendered(html) => {
                assert!(html.contains("block-text"));
                assert!(html.contains("done"));
            }
            _ => panic!("expected Rendered"),
        }
    }
}
