use agent_dash_core::paths;
use agent_dash_core::protocol::{ClientRequest, ServerEvent};
use std::io::{self, BufRead, Write};
use std::os::unix::net::UnixStream;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    match subcommand {
        "status" | "list" => cmd_status(),
        "watch" => cmd_watch(),
        "approve" => {
            let id = args.get(2).expect("usage: agentctl approve <request_id>");
            cmd_permission_response(id, "allow");
        }
        "approve-similar" => {
            let id = args
                .get(2)
                .expect("usage: agentctl approve-similar <request_id>");
            cmd_permission_response(id, "allow_similar");
        }
        "deny" => {
            let id = args.get(2).expect("usage: agentctl deny <request_id>");
            cmd_permission_response(id, "deny");
        }
        "messages" => {
            let session_id = args.get(2).expect("usage: agentctl messages <session_id> [format] [limit]");
            let format = args.get(3).map(|s| s.as_str()).unwrap_or("structured");
            let limit: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
            cmd_messages(session_id, format, limit);
        }
        "sessions" => {
            let project = args.get(2).expect("usage: agentctl sessions <project_name>");
            cmd_sessions(project);
        }
        "watch-messages" => {
            let session_id = args.get(2).expect("usage: agentctl watch-messages <session_id> [format]");
            let format = args.get(3).map(|s| s.as_str()).unwrap_or("structured");
            cmd_watch_messages(session_id, format);
        }
        _ => {
            eprintln!("usage: agentctl <status|list|watch|messages|sessions|watch-messages|approve|approve-similar|deny>");
            std::process::exit(1);
        }
    }
}

/// Connect to the daemon's client socket. Prints an error and exits if the
/// daemon is not running.
fn connect() -> UnixStream {
    let socket_name = paths::client_socket_name();
    UnixStream::connect(&socket_name).unwrap_or_else(|e| {
        eprintln!(
            "Failed to connect to agent-dashd at {}: {}",
            socket_name, e
        );
        eprintln!("Is the daemon running?");
        std::process::exit(1);
    })
}

/// Serialize a ClientRequest as JSON + newline and write it to the stream.
fn send_request(conn: &mut UnixStream, request: &ClientRequest) {
    let mut line = serde_json::to_string(request).unwrap();
    line.push('\n');
    conn.write_all(line.as_bytes()).unwrap();
}

/// Print current sessions in a table format, or "No active sessions." if empty.
fn cmd_status() {
    let mut conn = connect();
    send_request(&mut conn, &ClientRequest::GetState);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&line) else {
            continue;
        };
        if let ServerEvent::StateUpdate { sessions } = event {
            if sessions.is_empty() {
                println!("No active sessions.");
                return;
            }
            for s in &sessions {
                let tool_info = s
                    .active_tool
                    .as_ref()
                    .map(|t| format!(" [{}:{}]", t.name, truncate(&t.detail, 40)))
                    .unwrap_or_default();
                println!(
                    "{:<12} {:<10} {:<10} {}{}",
                    truncate(&s.project_name, 12),
                    s.branch,
                    s.status,
                    truncate(&s.session_id, 8),
                    tool_info,
                );
            }
            return;
        }
    }
}

/// Subscribe to the daemon event stream and print each line as raw JSON.
/// Exits when the connection closes or on ctrl-c.
fn cmd_watch() {
    let mut conn = connect();
    send_request(&mut conn, &ClientRequest::Subscribe);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        println!("{}", line);
    }
}

/// Send a permission decision (allow, allow_similar, deny) for a given
/// request_id.
fn cmd_permission_response(request_id: &str, decision: &str) {
    let mut conn = connect();
    let req = ClientRequest::PermissionResponse {
        request_id: request_id.to_string(),
        session_id: String::new(), // daemon looks up by request_id
        decision: decision.to_string(),
    };
    send_request(&mut conn, &req);
    println!("Sent {} for {}", decision, request_id);
}

/// Truncate a string to at most `max` bytes, respecting UTF-8 boundaries.
fn truncate(s: &str, max: usize) -> &str {
    s.get(..max).unwrap_or(s)
}

/// Fetch and print last N messages for a session.
fn cmd_messages(session_id: &str, format: &str, limit: usize) {
    let mut conn = connect();
    let req = ClientRequest::GetMessages {
        session_id: session_id.to_string(),
        format: Some(format.to_string()),
        limit: Some(limit),
    };
    send_request(&mut conn, &req);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&line) else {
            continue;
        };
        if let ServerEvent::Messages { messages, .. } = event {
            if messages.is_empty() {
                println!("No messages found.");
                return;
            }
            for msg in &messages {
                println!("--- {} ---", msg.role);
                match &msg.content {
                    agent_dash_core::protocol::ChatContent::Structured(blocks) => {
                        for block in blocks {
                            match block {
                                agent_dash_core::protocol::ContentBlock::Text { text } => {
                                    println!("{text}");
                                }
                                agent_dash_core::protocol::ContentBlock::ToolUse {
                                    name, detail, ..
                                } => {
                                    println!("> {name}: {detail}");
                                }
                                agent_dash_core::protocol::ContentBlock::ToolResult {
                                    output, ..
                                } => {
                                    if let Some(out) = output {
                                        let display = truncate(out, 200);
                                        println!("> result: {display}");
                                    }
                                }
                            }
                        }
                    }
                    agent_dash_core::protocol::ChatContent::Rendered(text) => {
                        println!("{text}");
                    }
                }
            }
            return;
        }
    }
}

/// List all sessions for a project.
fn cmd_sessions(project: &str) {
    let mut conn = connect();
    let req = ClientRequest::ListSessions {
        project: project.to_string(),
    };
    send_request(&mut conn, &req);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&line) else {
            continue;
        };
        if let ServerEvent::SessionList {
            project, sessions, ..
        } = event
        {
            if sessions.is_empty() {
                println!("No sessions found for project '{project}'.");
                return;
            }
            for s in &sessions {
                let main_marker = if s.main { " (main)" } else { "" };
                println!("{}{main_marker}", truncate(&s.session_id, 8));
            }
            return;
        }
    }
}

/// Subscribe to live messages for a session.
fn cmd_watch_messages(session_id: &str, format: &str) {
    let mut conn = connect();
    let req = ClientRequest::WatchSession {
        session_id: session_id.to_string(),
        format: Some(format.to_string()),
    };
    send_request(&mut conn, &req);

    let reader = io::BufReader::new(&conn);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        println!("{line}");
    }
}
