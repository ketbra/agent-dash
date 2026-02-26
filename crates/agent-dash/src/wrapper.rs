use crate::agents::AgentProfile;
use agent_dash_core::paths;
use agent_dash_core::protocol::{ClientRequest, ServerEvent, encode_line};
use base64::Engine as _;
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

    let width = screen.size().1;
    let rows = screen.size().0;

    // Scan all rows for a prompt line. Claude Code uses "❯" (U+276F) as its
    // prompt character, sometimes preceded by a space. Look for a row that
    // starts with an "❯" prompt and contains dim text after non-dim user text.
    for r in (0..rows).rev() {
        // Find prompt character in first few columns.
        let mut prompt_col = None;
        for c in 0..4u16 {
            if let Some(cell) = screen.cell(r, c) {
                let ch = cell.contents();
                if ch == "❯" || ch == ">" {
                    prompt_col = Some(c);
                    break;
                }
            }
        }
        let Some(pcol) = prompt_col else {
            continue;
        };

        // Collect ALL dim text on this row after the prompt character.
        // Spaces and punctuation between dim segments may not be dim
        // themselves, so we track the last dim position and fill gaps
        // with whatever characters are in between.
        let mut dim_parts: Vec<(u16, String)> = Vec::new(); // (col, text)
        let mut found_non_dim_content = false;
        for c in (pcol + 1)..width {
            let Some(cell) = screen.cell(r, c) else {
                break;
            };
            let contents = cell.contents();
            if contents.is_empty() {
                continue;
            }
            if cell.dim() {
                dim_parts.push((c, contents.to_string()));
            } else {
                found_non_dim_content = true;
            }
        }

        if !dim_parts.is_empty() {
            // Build suggestion from dim segments. If there are small gaps
            // (1-2 cols) between consecutive dim cells, fill them with
            // the actual cell contents (spaces, punctuation).
            let mut text = dim_parts[0].1.clone();
            for i in 1..dim_parts.len() {
                let gap = dim_parts[i].0 - dim_parts[i - 1].0;
                if gap > 3 {
                    break; // large gap = separate dim region, stop
                }
                // Fill gap with actual cell contents.
                for g in (dim_parts[i - 1].0 + 1)..dim_parts[i].0 {
                    if let Some(cell) = screen.cell(r, g) {
                        let ch = cell.contents();
                        text.push_str(if ch.is_empty() { " " } else { ch });
                    }
                }
                text.push_str(&dim_parts[i].1);
            }

            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        // Found a prompt line but no dim text - no suggestion right now.
        if found_non_dim_content || pcol < width.saturating_sub(1) {
            return None;
        }
    }

    None
}

/// Detect thinking status text from the VT100 screen.
///
/// Claude Code displays a line like "· Pouncing… (thinking)" while processing.
/// We scan rows bottom-up for a line starting with `·` (U+00B7 middle dot) or
/// `•` (U+2022 bullet), and extract the full line text.
/// Characters used in Claude Code's animated spinner.
/// Two variants exist (Ghostty vs default terminal) plus a reduced-motion
/// fallback; we match the union of all possible spinner frame characters.
fn is_spinner_char(s: &str) -> bool {
    // Note: ● (U+25CF) is intentionally excluded — Claude Code uses it for
    // completed-action indicators (e.g. "● Pushed successfully.") which would
    // false-positive when the truncated text contains "…".
    matches!(
        s,
        "·"  // U+00B7 Middle Dot
        | "✢" // U+2722 Four Teardrop-Spoked Asterisk
        | "*"  // U+002A Asterisk
        | "✳" // U+2733 Eight Spoked Asterisk (Ghostty)
        | "✶" // U+2736 Six Pointed Black Star
        | "✻" // U+273B Teardrop-Spoked Asterisk
        | "✽" // U+273D Heavy Teardrop-Spoked Asterisk
    )
}

fn detect_thinking_text(screen: &vt100::Screen) -> Option<String> {
    if screen.alternate_screen() {
        return None;
    }

    let rows = screen.size().0;
    let width = screen.size().1;

    for r in (0..rows).rev() {
        let Some(cell) = screen.cell(r, 0) else {
            continue;
        };
        let first_char = cell.contents();
        if !is_spinner_char(first_char) {
            continue;
        }

        // Found a row starting with a spinner character — extract the full line.
        let mut text = String::new();
        for c in 0..width {
            let Some(cell) = screen.cell(r, c) else {
                break;
            };
            let ch = cell.contents();
            if ch.is_empty() {
                text.push(' ');
            } else {
                text.push_str(ch);
            }
        }
        let trimmed = text.trim();
        // The thinking status line always contains "…" (U+2026) as part of
        // the active form (e.g. "Garnishing…").  Completed response lines
        // also start with a spinner char (● green dot) but lack the ellipsis.
        if !trimmed.is_empty() && trimmed.contains('\u{2026}') {
            return Some(trimmed.to_string());
        }
    }

    None
}

/// Combined screen update sent from the reader thread to the sender thread.
#[derive(Clone, Debug, PartialEq)]
struct ScreenUpdate {
    suggestion: Option<String>,
    thinking_text: Option<String>,
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

/// Options for running a wrapper session.
pub struct RunOptions {
    /// Run without a controlling terminal (for daemon-spawned sessions).
    pub headless: bool,
    /// Override PTY columns (defaults to terminal width or 120 in headless mode).
    pub cols: Option<u16>,
    /// Override PTY rows (defaults to terminal height or 36 in headless mode).
    pub rows: Option<u16>,
    /// Override session ID (defaults to "wrap-{pid}").
    pub session_id: Option<String>,
    /// Override working directory.
    pub cwd: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            headless: false,
            cols: None,
            rows: None,
            session_id: None,
            cwd: None,
        }
    }
}

/// Run an agent inside a PTY wrapper.
/// Blocks until the child process exits. Returns the exit code.
pub fn run(profile: &AgentProfile, args: &[String], opts: &RunOptions) -> i32 {
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

    // Determine PTY size.
    let (cols, rows) = if opts.headless {
        (opts.cols.unwrap_or(120), opts.rows.unwrap_or(36))
    } else {
        let (tc, tr) = terminal::size().unwrap_or((80, 24));
        (opts.cols.unwrap_or(tc), opts.rows.unwrap_or(tr))
    };

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
    let session_id = opts
        .session_id
        .clone()
        .unwrap_or_else(|| format!("wrap-{}", std::process::id()));

    // Build the command with the working directory.
    let mut cmd = CommandBuilder::new(profile.binary);
    if let Some(ref cwd) = opts.cwd {
        cmd.cwd(std::path::Path::new(cwd));
    } else if let Ok(cwd) = std::env::current_dir() {
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
        let effective_cwd = opts
            .cwd
            .as_ref()
            .map(|c| std::path::PathBuf::from(c))
            .or_else(|| std::env::current_dir().ok());
        let branch = effective_cwd
            .as_ref()
            .map(|d| git_branch(d))
            .unwrap_or_default();
        let project_name = effective_cwd
            .as_ref()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let req = ClientRequest::RegisterWrapper {
            session_id: session_id.clone(),
            agent: profile.name.to_string(),
            cwd: effective_cwd.map(|d| d.to_string_lossy().to_string()),
            branch: Some(branch),
            project_name: Some(project_name),
            real_session_id: None,
        };
        let _ = send_to_daemon(conn, &req);
    }

    // Enter raw mode so keypresses flow immediately (skip in headless mode).
    if !opts.headless {
        let _ = terminal::enable_raw_mode();
    }
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

    // --- Stdin thread: reads user input, sends to writer channel (skip in headless mode) ---
    let stdin_handle = if !opts.headless {
        let write_tx_stdin = write_tx.clone();
        let running_stdin = running.clone();
        Some(std::thread::spawn(move || {
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
        }))
    } else {
        None
    };

    // Shared terminal size for vt100 parser.
    let term_size = Arc::new(AtomicTermSize::new(cols, rows));

    // Channel for sending detected screen updates (suggestion + thinking text) to the daemon.
    let (screen_tx, screen_rx) = std::sync::mpsc::sync_channel::<ScreenUpdate>(4);

    // Channel for forwarding raw terminal output to the daemon (for web viewers).
    let (term_data_tx, term_data_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);

    // --- PTY reader thread: reads agent output, writes to stdout (or discards in headless) ---
    let running_reader = running.clone();
    let term_size_reader = term_size.clone();
    let headless = opts.headless;
    let reader_handle = {
        let mut pty_reader = pty_reader;
        std::thread::spawn(move || {
            let mut stdout = if !headless { Some(std::io::stdout()) } else { None };
            let mut buf = [0u8; 4096];
            let mut parser = vt100::Parser::new(rows, cols, 0);
            let mut last_update = ScreenUpdate { suggestion: None, thinking_text: None };
            let mut last_check = Instant::now();
            let debounce = std::time::Duration::from_millis(100);
            let mut last_parser_size = (cols, rows);

            loop {
                match pty_reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // Write to stdout only when not headless.
                        if let Some(ref mut out) = stdout {
                            if out.write_all(&buf[..n]).is_err() {
                                break;
                            }
                            let _ = out.flush();
                        }

                        // Forward raw bytes to terminal data channel for web viewers.
                        let _ = term_data_tx.try_send(buf[..n].to_vec());

                        // Update parser size if terminal was resized.
                        let (cur_cols, cur_rows) = term_size_reader.load();
                        if (cur_cols, cur_rows) != last_parser_size {
                            parser.screen_mut().set_size(cur_rows, cur_cols);
                            last_parser_size = (cur_cols, cur_rows);
                        }

                        // Feed bytes to vt100 parser.
                        parser.process(&buf[..n]);

                        // Debounced screen update detection.
                        let now = Instant::now();
                        if now.duration_since(last_check) >= debounce {
                            last_check = now;
                            let update = ScreenUpdate {
                                suggestion: detect_suggestion(parser.screen()),
                                thinking_text: detect_thinking_text(parser.screen()),
                            };
                            if update != last_update {
                                last_update = update.clone();
                                let _ = screen_tx.try_send(update);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            running_reader.store(false, Ordering::Relaxed);
        })
    };

    // --- Screen update sender thread: drains screen update channel, sends to daemon ---
    {
        let running_suggest = running.clone();
        let session_id_suggest = session_id.clone();
        std::thread::spawn(move || {
            // Open a dedicated connection for screen updates.
            let Some(conn) = try_connect_daemon() else {
                return;
            };
            let mut last_suggestion: Option<String> = None;
            let mut last_thinking: Option<String> = None;
            while running_suggest.load(Ordering::Relaxed) {
                match screen_rx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(update) => {
                        if update.suggestion != last_suggestion {
                            last_suggestion = update.suggestion.clone();
                            let req = ClientRequest::UpdateSuggestion {
                                session_id: session_id_suggest.clone(),
                                suggestion: update.suggestion,
                            };
                            if send_to_daemon(&conn, &req).is_err() {
                                break;
                            }
                        }
                        if update.thinking_text != last_thinking {
                            last_thinking = update.thinking_text.clone();
                            let req = ClientRequest::UpdateThinkingText {
                                session_id: session_id_suggest.clone(),
                                thinking_text: update.thinking_text,
                            };
                            if send_to_daemon(&conn, &req).is_err() {
                                break;
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }

    // --- Terminal data sender thread: forwards raw PTY output to daemon ---
    {
        let running_term = running.clone();
        let session_id_term = session_id.clone();
        std::thread::spawn(move || {
            let Some(conn) = try_connect_daemon() else {
                return;
            };
            let engine = base64::engine::general_purpose::STANDARD;
            while running_term.load(Ordering::Relaxed) {
                match term_data_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(data) => {
                        // Batch: drain any additional pending data to reduce message count.
                        let mut combined = data;
                        while let Ok(more) = term_data_rx.try_recv() {
                            combined.extend_from_slice(&more);
                            if combined.len() > 16384 {
                                break;
                            }
                        }
                        let b64 = engine.encode(&combined);
                        let req = ClientRequest::TerminalOutput {
                            session_id: session_id_term.clone(),
                            data: b64,
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

    // --- Daemon listener thread: listens for inject_prompt and terminal_write commands ---
    // Reconnects automatically if the daemon restarts.
    if let Some(conn) = daemon_conn {
        let write_tx_inject = write_tx.clone();
        let running_daemon = running.clone();
        let session_id_daemon = session_id.clone();
        let agent_name = profile.name.to_string();
        std::thread::spawn(move || {
            let engine = base64::engine::general_purpose::STANDARD;
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
                            match event {
                                ServerEvent::InjectPrompt { text } => {
                                    if write_tx_inject.send(text.into_bytes()).is_err() {
                                        return;
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                    if write_tx_inject.send(vec![b'\r']).is_err() {
                                        return;
                                    }
                                }
                                ServerEvent::TerminalWrite { data } => {
                                    if let Ok(bytes) = engine.decode(&data) {
                                        if write_tx_inject.send(bytes).is_err() {
                                            return;
                                        }
                                    }
                                }
                                _ => {}
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

    // --- Resize thread: periodically checks terminal size (skip in headless mode) ---
    if !headless {
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
    } else {
        // In headless mode, we still need to keep pair.master alive to
        // prevent the PTY from closing.
        let _master = pair.master;
        let running_headless = running.clone();
        std::thread::spawn(move || {
            while running_headless.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            drop(_master);
        });
    }

    // Wait for the child process to exit.
    let status = child.wait().expect("failed to wait on child");
    running.store(false, Ordering::Relaxed);

    // Restore terminal (skip in headless mode).
    if !headless {
        let _ = terminal::disable_raw_mode();
    }

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
    fn detect_dim_suggestion_with_gt_prompt() {
        // "> " then dim text
        let seq = b"> \x1b[2mcheck the tests\x1b[0m\x1b[1;3H";
        let result = suggestion_from(seq);
        assert_eq!(result, Some("check the tests".to_string()));
    }

    #[test]
    fn detect_dim_suggestion_with_heavy_angle_prompt() {
        // Claude Code uses "❯" (U+276F) as prompt
        let mut seq = Vec::new();
        seq.extend_from_slice("❯ ".as_bytes());
        seq.extend_from_slice(b"\x1b[2mrun the tests\x1b[0m");
        let result = suggestion_from(&seq);
        assert_eq!(result, Some("run the tests".to_string()));
    }

    #[test]
    fn no_suggestion_on_non_prompt_line() {
        let result = suggestion_from(b"$ \x1b[2msome text\x1b[0m");
        assert_eq!(result, None);
    }

    #[test]
    fn no_suggestion_on_alternate_screen() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[?1049h");
        parser.process(b"> \x1b[2msuggestion\x1b[0m\x1b[1;3H");
        let result = detect_suggestion(parser.screen());
        assert_eq!(result, None);
    }

    #[test]
    fn no_suggestion_when_no_dim_text() {
        let input = b"> hello world";
        let result = suggestion_from(input);
        assert_eq!(result, None);
    }

    #[test]
    fn detect_suggestion_with_non_dim_gaps() {
        // Simulate: prompt, then dim "Try", non-dim quote, dim text, non-dim quote
        // ❯ Try "refactor monitor.rs"
        // where quotes are not dim but the words are
        let mut seq = Vec::new();
        seq.extend_from_slice("❯ ".as_bytes());
        seq.extend_from_slice(b"\x1b[2mTry \x1b[0m\"\x1b[2mrefactor monitor.rs\x1b[0m\"");
        let result = suggestion_from(&seq);
        // Trailing non-dim quote is not included — that's fine.
        assert_eq!(result, Some("Try \"refactor monitor.rs".to_string()));
    }

    /// Helper: create a parser, write VT100 sequences, return detected thinking text.
    fn thinking_from(sequences: &[u8]) -> Option<String> {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(sequences);
        detect_thinking_text(parser.screen())
    }

    #[test]
    fn detect_thinking_middle_dot() {
        let result = thinking_from("· Pouncing\u{2026} (thinking)".as_bytes());
        assert_eq!(result, Some("· Pouncing\u{2026} (thinking)".to_string()));
    }

    #[test]
    fn detect_thinking_asterisk() {
        let result = thinking_from("* Garnishing\u{2026} (1m 30s)".as_bytes());
        assert_eq!(result, Some("* Garnishing\u{2026} (1m 30s)".to_string()));
    }

    #[test]
    fn detect_thinking_star() {
        let result = thinking_from("\u{2736} Working\u{2026}".as_bytes());
        assert_eq!(result, Some("\u{2736} Working\u{2026}".to_string()));
    }

    #[test]
    fn detect_thinking_teardrop() {
        let result = thinking_from("\u{2722} Thinking\u{2026}".as_bytes());
        assert_eq!(result, Some("\u{2722} Thinking\u{2026}".to_string()));
    }

    #[test]
    fn no_thinking_on_black_circle() {
        // ● (U+25CF) is used for completed-action indicators, not matched as spinner.
        let result = thinking_from("\u{25CF} Working\u{2026}".as_bytes());
        assert_eq!(result, None);
    }

    #[test]
    fn no_thinking_on_completed_response() {
        // Completed response lines start with ● but lack the ellipsis.
        let result = thinking_from("● Pushed both commits to origin/main.".as_bytes());
        assert_eq!(result, None);
    }

    #[test]
    fn no_thinking_on_normal_text() {
        let result = thinking_from(b"Hello world");
        assert_eq!(result, None);
    }

    #[test]
    fn no_thinking_on_alternate_screen() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[?1049h");
        parser.process("· Thinking\u{2026}".as_bytes());
        let result = detect_thinking_text(parser.screen());
        assert_eq!(result, None);
    }

    #[test]
    fn detect_thinking_not_prompt_line() {
        // Make sure a prompt line with ">" doesn't match as thinking
        let result = thinking_from(b"> some prompt text");
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
