use crate::agents::AgentProfile;
use agent_dash_core::paths;
use agent_dash_core::protocol::{ClientRequest, ServerEvent, encode_line};
use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Packs (cols, rows) into a single AtomicU32 for lock-free sharing
/// between the resize thread and the reader thread.
struct AtomicTermSize(AtomicU32);

impl AtomicTermSize {
    fn new(cols: u16, rows: u16) -> Self {
        Self(AtomicU32::new(((cols as u32) << 16) | rows as u32))
    }

    fn store(&self, cols: u16, rows: u16) {
        self.0
            .store(((cols as u32) << 16) | rows as u32, Ordering::Relaxed);
    }

    fn load(&self) -> (u16, u16) {
        let v = self.0.load(Ordering::Relaxed);
        ((v >> 16) as u16, v as u16)
    }
}

/// Detect prompt suggestion (dim text after cursor) on the current prompt line.
///
/// Returns `Some(text)` if dim text is found after the cursor on a line
/// starting with `> `, or `None` otherwise.
fn detect_suggestion(screen: &vt100::Screen) -> Option<String> {
    if screen.alternate_screen() {
        return None;
    }

    let (cursor_row, cursor_col) = screen.cursor_position();
    let width = screen.size().1;

    // Check if cursor row starts with "> "
    let cell0 = screen.cell(cursor_row, 0)?;
    let cell1 = screen.cell(cursor_row, 1)?;
    if cell0.contents() != ">" || cell1.contents() != " " {
        return None;
    }

    // Scan from cursor_col forward for dim text
    let mut text = String::new();
    for c in cursor_col..width {
        let Some(cell) = screen.cell(cursor_row, c) else {
            break;
        };
        let contents = cell.contents();
        if contents.is_empty() || contents == " " && text.is_empty() {
            // Skip leading spaces, but break on empty cells
            if contents.is_empty() {
                break;
            }
            continue;
        }
        if cell.dim() {
            text.push_str(&contents);
        } else if !text.is_empty() {
            break; // non-dim after dim = end
        }
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Ensure the daemon is running. If not, fork it as a background process.
/// Returns true if the daemon is available, false if couldn't start.
fn ensure_daemon() -> bool {
    let sock = paths::client_socket_name();

    // Try connecting — if it works, daemon is already running.
    if UnixStream::connect(&sock).is_ok() {
        return true;
    }

    eprintln!("Starting agent-dash daemon...");

    // Get path to our own binary.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    // Fork the daemon as a background process.
    let child = std::process::Command::new(&exe)
        .arg("daemon")
        .arg("start")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn();

    if child.is_err() {
        eprintln!("Failed to start daemon");
        return false;
    }

    // Wait up to 2 seconds for the socket to appear.
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if UnixStream::connect(&sock).is_ok() {
            return true;
        }
    }

    eprintln!("Daemon did not start within 2 seconds");
    false
}

/// Run an agent inside a PTY wrapper.
/// Blocks until the child process exits. Returns the exit code.
pub fn run(profile: &AgentProfile, args: &[String]) -> i32 {
    // Check that the agent binary exists in PATH.
    if which(profile.binary).is_none() {
        eprintln!(
            "{} not found in PATH. Install it with:\n  {}",
            profile.display_name, profile.install_hint
        );
        return 1;
    }

    // Ensure daemon is running (auto-start if needed).
    ensure_daemon();

    // Check if hooks are installed.
    if !crate::setup::hooks_installed() {
        eprintln!("agent-dash hooks not installed. Run `agent-dash setup hooks` to install.");
    }

    // Get current terminal size.
    let (cols, rows) = terminal::size().unwrap_or((80, 24));

    // Create the PTY.
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open pty");

    // Generate wrapper session ID for daemon registration.
    let session_id = format!("wrap-{}", std::process::id());

    // Build the command with the current working directory.
    let mut cmd = CommandBuilder::new(profile.binary);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }
    // Pass wrapper ID so hooks can link the real session ID back to us.
    cmd.env("AGENT_DASH_WRAPPER_ID", &session_id);
    for arg in args {
        cmd.arg(arg);
    }

    // Spawn the agent inside the PTY.
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn agent");
    // Drop the slave -- the child owns it now.
    drop(pair.slave);

    // Get reader and writer for the master side.
    // IMPORTANT: take_writer() can only be called once, so we do it here
    // before moving pair.master to the resize thread.
    let pty_reader = pair
        .master
        .try_clone_reader()
        .expect("failed to clone pty reader");
    let pty_writer = pair
        .master
        .take_writer()
        .expect("failed to take pty writer");

    // Try to connect to the daemon and register.
    let daemon_conn = try_connect_daemon();

    if let Some(ref conn) = daemon_conn {
        let cwd = std::env::current_dir().ok();
        let branch = cwd.as_ref().map(|d| git_branch(d)).unwrap_or_default();
        let project_name = cwd
            .as_ref()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let req = ClientRequest::RegisterWrapper {
            session_id: session_id.clone(),
            agent: profile.name.to_string(),
            cwd: cwd.map(|d| d.to_string_lossy().to_string()),
            branch: Some(branch),
            project_name: Some(project_name),
            real_session_id: None,
        };
        let _ = send_to_daemon(conn, &req);
    }

    // Enter raw mode so keypresses flow immediately.
    let _ = terminal::enable_raw_mode();
    let running = Arc::new(AtomicBool::new(true));

    // Channel for multiplexing stdin + injected prompts into the PTY writer.
    let (write_tx, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();

    // --- Writer thread: reads from channel, writes to PTY ---
    let running_writer = running.clone();
    let writer_handle = {
        let mut pty_writer = pty_writer;
        std::thread::spawn(move || {
            while running_writer.load(Ordering::Relaxed) {
                match write_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(data) => {
                        if pty_writer.write_all(&data).is_err() {
                            break;
                        }
                        let _ = pty_writer.flush();
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
    };

    // --- Stdin thread: reads user input, sends to writer channel ---
    let write_tx_stdin = write_tx.clone();
    let running_stdin = running.clone();
    let stdin_handle = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        while running_stdin.load(Ordering::Relaxed) {
            match stdin.lock().read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if write_tx_stdin.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Shared terminal size for vt100 parser.
    let term_size = Arc::new(AtomicTermSize::new(cols, rows));

    // Channel for sending detected suggestions to the daemon.
    let (suggest_tx, suggest_rx) = std::sync::mpsc::sync_channel::<Option<String>>(4);

    // --- PTY reader thread: reads agent output, writes to stdout ---
    let running_reader = running.clone();
    let term_size_reader = term_size.clone();
    let reader_handle = {
        let mut pty_reader = pty_reader;
        std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            let mut buf = [0u8; 4096];
            let mut parser = vt100::Parser::new(rows, cols, 0);
            let mut last_suggestion: Option<String> = None;
            let mut last_check = Instant::now();
            let debounce = std::time::Duration::from_millis(100);
            let mut last_parser_size = (cols, rows);

            loop {
                match pty_reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdout.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = stdout.flush();

                        // Update parser size if terminal was resized.
                        let (cur_cols, cur_rows) = term_size_reader.load();
                        if (cur_cols, cur_rows) != last_parser_size {
                            parser.screen_mut().set_size(cur_rows, cur_cols);
                            last_parser_size = (cur_cols, cur_rows);
                        }

                        // Feed bytes to vt100 parser.
                        parser.process(&buf[..n]);

                        // Debounced suggestion detection.
                        let now = Instant::now();
                        if now.duration_since(last_check) >= debounce {
                            last_check = now;
                            let suggestion = detect_suggestion(parser.screen());
                            if suggestion != last_suggestion {
                                last_suggestion = suggestion.clone();
                                let _ = suggest_tx.try_send(suggestion);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            running_reader.store(false, Ordering::Relaxed);
        })
    };

    // --- Suggestion sender thread: drains suggestion channel, sends to daemon ---
    {
        let running_suggest = running.clone();
        let session_id_suggest = session_id.clone();
        std::thread::spawn(move || {
            // Open a dedicated connection for suggestion updates.
            let Some(conn) = try_connect_daemon() else {
                return;
            };
            while running_suggest.load(Ordering::Relaxed) {
                match suggest_rx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(suggestion) => {
                        let req = ClientRequest::UpdateSuggestion {
                            session_id: session_id_suggest.clone(),
                            suggestion,
                        };
                        if send_to_daemon(&conn, &req).is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }

    // --- Daemon listener thread: listens for inject_prompt commands ---
    // Reconnects automatically if the daemon restarts.
    if let Some(conn) = daemon_conn {
        let write_tx_inject = write_tx.clone();
        let running_daemon = running.clone();
        let session_id_daemon = session_id.clone();
        let agent_name = profile.name.to_string();
        std::thread::spawn(move || {
            let mut conn = conn;
            loop {
                // Read events from daemon using a cloned stream so the
                // borrow does not prevent reassigning `conn` later.
                {
                    let stream_clone = match conn.try_clone() {
                        Ok(c) => c,
                        Err(_) => break,
                    };
                    let reader = BufReader::new(stream_clone);
                    for line in reader.lines() {
                        if !running_daemon.load(Ordering::Relaxed) {
                            return;
                        }
                        let Ok(line) = line else { break };
                        if let Ok(event) = serde_json::from_str::<ServerEvent>(&line) {
                            if let ServerEvent::InjectPrompt { text } = event {
                                if write_tx_inject.send(text.into_bytes()).is_err() {
                                    return;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(50));
                                if write_tx_inject.send(vec![b'\r']).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }

                // Daemon disconnected — reconnect loop.
                if !running_daemon.load(Ordering::Relaxed) {
                    return;
                }
                eprintln!("agent-dash: daemon disconnected, reconnecting...");
                let mut delay = std::time::Duration::from_secs(1);
                let max_delay = std::time::Duration::from_secs(10);
                loop {
                    if !running_daemon.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(delay);
                    if let Some(new_conn) = try_connect_daemon() {
                        // Re-register with metadata.
                        let cwd = std::env::current_dir().ok();
                        let branch =
                            cwd.as_ref().map(|d| git_branch(d)).unwrap_or_default();
                        let pname = cwd
                            .as_ref()
                            .and_then(|d| d.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        let req = ClientRequest::RegisterWrapper {
                            session_id: session_id_daemon.clone(),
                            agent: agent_name.clone(),
                            cwd: cwd.map(|d| d.to_string_lossy().to_string()),
                            branch: Some(branch),
                            project_name: Some(pname),
                            real_session_id: None,
                        };
                        if send_to_daemon(&new_conn, &req).is_ok() {
                            eprintln!("agent-dash: reconnected to daemon");
                            conn = new_conn;
                            break;
                        }
                    }
                    delay = (delay * 2).min(max_delay);
                }
            }
        });
    }

    // --- Resize thread: periodically checks terminal size ---
    {
        let master = pair.master;
        let running_resize = running.clone();
        let term_size_resize = term_size;
        std::thread::spawn(move || {
            let mut last_size = (cols, rows);
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if !running_resize.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok((new_cols, new_rows)) = terminal::size() {
                    if (new_cols, new_rows) != last_size {
                        let _ = master.resize(PtySize {
                            rows: new_rows,
                            cols: new_cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                        term_size_resize.store(new_cols, new_rows);
                        last_size = (new_cols, new_rows);
                    }
                }
            }
        });
    }

    // Wait for the child process to exit.
    let status = child.wait().expect("failed to wait on child");
    running.store(false, Ordering::Relaxed);

    // Restore terminal.
    let _ = terminal::disable_raw_mode();

    // Unregister from daemon.
    if let Some(conn) = try_connect_daemon() {
        let req = ClientRequest::UnregisterWrapper {
            session_id: session_id.clone(),
        };
        let _ = send_to_daemon(&conn, &req);
    }

    // Clean up threads.
    drop(write_tx);
    let _ = writer_handle.join();
    let _ = reader_handle.join();
    // stdin_handle may block on read -- don't wait indefinitely.
    let _ = stdin_handle;

    // Return the child's exit code.
    if status.success() {
        0
    } else {
        status.exit_code() as i32
    }
}

/// Extract git branch from a directory. Returns empty string on failure.
fn git_branch(dir: &std::path::Path) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8(out.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

/// Try to connect to the daemon. Returns None if not running.
fn try_connect_daemon() -> Option<UnixStream> {
    UnixStream::connect(paths::client_socket_name()).ok()
}

/// Send a request to the daemon connection.
fn send_to_daemon(
    conn: &UnixStream,
    req: &ClientRequest,
) -> Result<(), std::io::Error> {
    let line =
        encode_line(req).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    (&*conn).write_all(line.as_bytes())?;
    (&*conn).flush()
}

/// Check if a binary exists in PATH.
fn which(binary: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full_path = dir.join(binary);
            if full_path.is_file() {
                Some(full_path)
            } else {
                None
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a parser, write VT100 sequences, return detected suggestion.
    fn suggestion_from(sequences: &[u8]) -> Option<String> {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(sequences);
        detect_suggestion(parser.screen())
    }

    #[test]
    fn detect_dim_suggestion_on_prompt_line() {
        // Simulate: "> " then dim text, then move cursor back to col 3 (1-indexed)
        // \x1b[2m = SGR dim on, \x1b[0m = SGR reset
        // \x1b[1;3H = move cursor to row 1, col 3 (0-indexed: row 0, col 2)
        let seq = b"> \x1b[2mcheck the tests\x1b[0m\x1b[1;3H";

        let result = suggestion_from(seq);
        assert_eq!(result, Some("check the tests".to_string()));
    }

    #[test]
    fn no_suggestion_on_non_prompt_line() {
        // Line without "> " prefix
        let result = suggestion_from(b"$ \x1b[2msome text\x1b[0m");
        assert_eq!(result, None);
    }

    #[test]
    fn no_suggestion_on_alternate_screen() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        // Enter alternate screen
        parser.process(b"\x1b[?1049h");
        // Write prompt with dim text
        parser.process(b"> \x1b[2msuggestion\x1b[0m\x1b[1;3H");
        let result = detect_suggestion(parser.screen());
        assert_eq!(result, None);
    }

    #[test]
    fn no_suggestion_when_no_dim_text() {
        let input = b"> hello world";
        let result = suggestion_from(input);
        // Cursor ends at col 13, no dim text after it
        assert_eq!(result, None);
    }

    #[test]
    fn atomic_term_size_round_trip() {
        let ats = AtomicTermSize::new(120, 40);
        assert_eq!(ats.load(), (120, 40));
        ats.store(200, 50);
        assert_eq!(ats.load(), (200, 50));
    }
}
