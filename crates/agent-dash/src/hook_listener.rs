use agent_dash_core::paths;
use agent_dash_core::protocol::HookEvent;
use interprocess::local_socket::{
    tokio::prelude::*,
    GenericFilePath, ListenerOptions,
};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

/// Run the hook listener. Accepts fire-and-forget connections on hook.sock.
pub async fn run(tx: mpsc::Sender<HookEvent>) {
    let name = paths::hook_socket_name();

    // Ensure parent directory exists
    let path = std::path::Path::new(&name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Remove stale socket
    let _ = std::fs::remove_file(&name);

    let listener = match ListenerOptions::new()
        .name(name.as_str().to_fs_name::<GenericFilePath>().expect("invalid socket path"))
        .create_tokio()
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!("agent-dashd: failed to bind hook socket: {e}");
            return;
        }
    };

    eprintln!("agent-dashd: hook listener on {name}");

    loop {
        match listener.accept().await {
            Ok(conn) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_hook_connection(conn, tx).await;
                });
            }
            Err(e) => {
                eprintln!("agent-dashd: hook accept error: {e}");
            }
        }
    }
}

async fn handle_hook_connection(
    mut conn: impl AsyncReadExt + Unpin,
    tx: mpsc::Sender<HookEvent>,
) {
    let mut buf = Vec::with_capacity(4096);
    match conn.read_to_end(&mut buf).await {
        Ok(_) => {
            let text = String::from_utf8_lossy(&buf);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return;
            }
            match serde_json::from_str::<HookEvent>(trimmed) {
                Ok(event) => {
                    let _ = tx.send(event).await;
                }
                Err(e) => {
                    eprintln!("agent-dashd: failed to parse hook event: {e}");
                }
            }
        }
        Err(e) => {
            eprintln!("agent-dashd: hook read error: {e}");
        }
    }
}
