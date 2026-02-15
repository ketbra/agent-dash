use crate::agents::AgentProfile;
use agent_dash_core::paths;
use agent_dash_core::protocol::{ClientRequest, ServerEvent, encode_line};
use crossterm::terminal;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

    // Build the command.
    let mut cmd = CommandBuilder::new(profile.binary);
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
    let session_id = format!("wrap-{}", std::process::id());
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
                        let mut data = text.into_bytes();
                        data.push(b'\n');
                        if write_tx_inject.send(data).is_err() {
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
fn try_connect_daemon() -> Option<std::os::unix::net::UnixStream> {
    std::os::unix::net::UnixStream::connect(paths::client_socket_name()).ok()
}

/// Send a request to the daemon connection.
fn send_to_daemon(
    conn: &std::os::unix::net::UnixStream,
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
