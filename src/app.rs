use crate::ipc::{self, PermissionDecision, PermissionResponse};
use crate::monitor::SessionMonitor;
use crate::session::{InputReason, SessionStatus};
use eframe::egui;
use std::time::{Duration, Instant};

pub struct AgentDashApp {
    monitor: SessionMonitor,
    last_refresh: Instant,
    expanded_session: Option<String>,
    dragging: bool,
}

impl AgentDashApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            monitor: SessionMonitor::new(),
            last_refresh: Instant::now() - Duration::from_secs(10), // force immediate refresh
            expanded_session: None,
            dragging: false,
        }
    }
}

impl eframe::App for AgentDashApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Refresh session data every 2 seconds
        if self.last_refresh.elapsed() > Duration::from_secs(2) {
            self.monitor.refresh();
            self.last_refresh = Instant::now();
        }

        // Request repaint periodically to keep status up to date
        ctx.request_repaint_after(Duration::from_secs(1));

        // Handle window dragging
        let panel_frame = egui::Frame::NONE
            .fill(egui::Color32::from_rgba_unmultiplied(30, 30, 30, 217)) // ~0.85 alpha
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(8));

        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| {
                // Drag handling: drag anywhere on the panel
                let resp = ui.interact(
                    ui.max_rect(),
                    ui.id().with("drag"),
                    egui::Sense::drag(),
                );
                if resp.drag_started() {
                    self.dragging = true;
                }
                if resp.dragged() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if resp.drag_stopped() {
                    self.dragging = false;
                }

                let sessions = self.monitor.sorted_sessions();

                if sessions.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_gray(128),
                        "No active Claude sessions",
                    );
                    return;
                }

                for session in sessions {
                    let (color, dot) = match session.status {
                        SessionStatus::NeedsInput => (egui::Color32::from_rgb(255, 80, 80), "\u{1F534}"),
                        SessionStatus::Working => (egui::Color32::from_rgb(255, 200, 50), "\u{1F7E1}"),
                        SessionStatus::Idle => (egui::Color32::from_rgb(80, 200, 80), "\u{1F7E2}"),
                        SessionStatus::Ended => (egui::Color32::from_gray(128), "\u{26AA}"),
                    };

                    let label_text = format!("{} {}", dot, session.label());
                    let is_expanded = self.expanded_session.as_ref() == Some(&session.session_id);

                    // Pill background
                    let pill_frame = egui::Frame::NONE
                        .fill(egui::Color32::from_rgba_unmultiplied(50, 50, 50, 200))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(6));

                    pill_frame.show(ui, |ui| {
                        let label_resp = ui.add(
                            egui::Label::new(
                                egui::RichText::new(&label_text)
                                    .color(color)
                                    .size(13.0),
                            )
                            .sense(egui::Sense::click()),
                        );

                        if label_resp.clicked() {
                            if is_expanded {
                                self.expanded_session = None;
                            } else if session.input_reason.is_some() {
                                self.expanded_session = Some(session.session_id.clone());
                            }
                            // For non-input sessions, clicking could focus terminal (future)
                        }

                        // Expanded section
                        if is_expanded {
                            if let Some(reason) = &session.input_reason {
                                ui.separator();
                                match reason {
                                    InputReason::Permission(req) => {
                                        // Show tool + command
                                        let detail = if let Some(cmd) = req.input.get("command") {
                                            format!("{}: {}", req.tool, cmd.as_str().unwrap_or("?"))
                                        } else {
                                            format!("{}: {:?}", req.tool, req.input)
                                        };
                                        ui.label(
                                            egui::RichText::new(&detail)
                                                .size(11.0)
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        ui.horizontal(|ui| {
                                            if ui
                                                .button(egui::RichText::new("Allow").size(11.0))
                                                .clicked()
                                            {
                                                let _ = ipc::write_permission_response(
                                                    &session.session_id,
                                                    &PermissionResponse {
                                                        decision: PermissionDecision {
                                                            behavior: "allow".to_string(),
                                                            message: None,
                                                        },
                                                    },
                                                );
                                                self.expanded_session = None;
                                            }
                                            if ui
                                                .button(egui::RichText::new("Similar").size(11.0))
                                                .clicked()
                                            {
                                                let _ = ipc::write_permission_response(
                                                    &session.session_id,
                                                    &PermissionResponse {
                                                        decision: PermissionDecision {
                                                            behavior: "allow".to_string(),
                                                            // updatedPermissions would go here
                                                            message: None,
                                                        },
                                                    },
                                                );
                                                self.expanded_session = None;
                                            }
                                            if ui
                                                .button(egui::RichText::new("Deny").size(11.0))
                                                .clicked()
                                            {
                                                let _ = ipc::write_permission_response(
                                                    &session.session_id,
                                                    &PermissionResponse {
                                                        decision: PermissionDecision {
                                                            behavior: "deny".to_string(),
                                                            message: Some(
                                                                "Denied from dashboard".into(),
                                                            ),
                                                        },
                                                    },
                                                );
                                                self.expanded_session = None;
                                            }
                                        });
                                    }
                                    InputReason::Question { text } => {
                                        ui.label(
                                            egui::RichText::new(text)
                                                .size(11.0)
                                                .color(egui::Color32::from_gray(200)),
                                        );
                                        if ui
                                            .button(
                                                egui::RichText::new("Go to terminal").size(11.0),
                                            )
                                            .clicked()
                                        {
                                            // TODO: focus terminal window
                                            self.expanded_session = None;
                                        }
                                    }
                                }
                            }
                        }
                    });

                    ui.add_space(2.0);
                }
            });
    }
}
