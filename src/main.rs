mod app;
mod ipc;
mod monitor;
mod session;

use app::AgentDashApp;

fn main() -> eframe::Result<()> {
    // Force X11/XWayland backend — GNOME Wayland doesn't support always-on-top from clients.
    // Must unset WAYLAND_DISPLAY or winit ignores the backend override.
    // SAFETY: called before any threads are spawned
    unsafe {
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        std::env::remove_var("WAYLAND_DISPLAY");
    }

    // Spawn a background thread that sets _NET_WM_STATE_ABOVE via xprop on all our windows.
    // winit's with_always_on_top() doesn't reliably set this on XWayland.
    std::thread::spawn(|| {
        use std::process::Command;
        // Wait for windows to appear, then set ABOVE on all matching windows
        std::thread::sleep(std::time::Duration::from_secs(1));
        let output = Command::new("xprop")
            .args(["-root", "_NET_CLIENT_LIST"])
            .output();
        let Ok(output) = output else { return };
        let list = String::from_utf8_lossy(&output.stdout);
        for wid in list.split(|c: char| !c.is_ascii_hexdigit() && c != 'x')
            .filter(|s| s.starts_with("0x"))
        {
            let name_output = Command::new("xprop")
                .args(["-id", wid, "WM_NAME"])
                .output();
            if let Ok(name_output) = name_output {
                let name = String::from_utf8_lossy(&name_output.stdout);
                if name.contains("Agent Dash") {
                    let _ = Command::new("xprop")
                        .args([
                            "-id", wid,
                            "-f", "_NET_WM_STATE", "32a",
                            "-set", "_NET_WM_STATE", "_NET_WM_STATE_ABOVE",
                        ])
                        .status();
                }
            }
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_inner_size([260.0, 200.0])
            .with_min_inner_size([260.0, 60.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Agent Dash",
        options,
        Box::new(|cc| Ok(Box::new(AgentDashApp::new(cc)))),
    )
}
