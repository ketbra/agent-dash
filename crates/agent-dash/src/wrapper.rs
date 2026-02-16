use crate::agents::AgentProfile;
use agent_dash_core::paths;
use agent_dash_core::protocol::{ClientRequest, ServerEvent, encode_line};
use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
        let req = ClientRequest::RegisterWrapper {
            session_id: session_id.clone(),
            agent: profile.name.to_string(),
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

    // --- PTY reader thread: reads agent output, writes to stdout ---
    let running_reader = running.clone();
    let reader_handle = {
        let mut pty_reader = pty_reader;
        std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            let mut buf = [0u8; 4096];
            loop {
                match pty_reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdout.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = stdout.flush();
                    }
                    Err(_) => break,
                }
            }
            running_reader.store(false, Ordering::Relaxed);
        })
    };

    // --- Daemon listener thread: listens for inject_prompt commands ---
    if let Some(conn) = daemon_conn {
        let write_tx_inject = write_tx.clone();
        let running_daemon = running.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(&conn);
            for line in reader.lines() {
                if !running_daemon.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<ServerEvent>(&line) {
                    if let ServerEvent::InjectPrompt { text } = event {
                        // Send text first, then Enter separately after a
                        // short delay so the TUI processes them as distinct
                        // input events.
                        if write_tx_inject.send(text.into_bytes()).is_err() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        if write_tx_inject.send(vec![b'\r']).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    // --- Resize thread: periodically checks terminal size ---
    {
        let master = pair.master;
        let running_resize = running.clone();
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
