mod ipc;
mod monitor;
mod session;
mod socket;

use monitor::SessionMonitor;
use session::DashState;
use std::path::PathBuf;
use std::time::Duration;

/// Path to the state file the GNOME extension reads.
fn state_file_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("agent-dash")
        .join("state.json")
}

/// Write state atomically (tmp + rename).
fn write_state(state: &DashState) -> std::io::Result<()> {
    let path = state_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn main() {
    eprintln!("agent-dashd starting, writing to {:?}", state_file_path());
    let mut monitor = SessionMonitor::new();

    loop {
        monitor.refresh();
        let sessions: Vec<_> = monitor
            .sessions()
            .map(|s| s.to_dash_session())
            .collect();
        let state = DashState { sessions };
        if let Err(e) = write_state(&state) {
            eprintln!("error writing state: {}", e);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
