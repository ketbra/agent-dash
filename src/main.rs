mod app;
mod ipc;
mod monitor;
mod session;

use app::AgentDashApp;

fn main() -> eframe::Result<()> {
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
