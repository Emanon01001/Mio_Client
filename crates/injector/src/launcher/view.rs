use super::{brand, theme::*, Action, App, Target};
use eframe::egui::{self, Align, Color32, RichText, Stroke};

fn text(ui: &mut egui::Ui, value: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(value).color(MUTED).size(12.)).wrap());
}
fn badge(ui: &mut egui::Ui, value: &str, color: Color32) {
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.12))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(value).size(12.).color(color));
        });
}
fn logo(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(52., 52.), egui::Sense::hover());
    let bg = brand::LOGO_BG;
    ui.painter()
        .rect_filled(rect, 13., Color32::from_rgb(bg[0], bg[1], bg[2]));
    let point = |p: [f32; 2]| rect.min + egui::vec2(p[0] * rect.width(), p[1] * rect.height());
    for segment in brand::MARK.windows(2) {
        ui.painter().line_segment(
            [point(segment[0]), point(segment[1])],
            Stroke::new(rect.width() * brand::MARK_WIDTH, TEXT),
        );
    }
    ui.painter()
        .circle_filled(point(brand::DOT), rect.width() * brand::DOT_RADIUS, GOOD);
}

impl App {
    pub(super) fn draw(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(24))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.header(ui);
                        ui.add_space(12.);
                        self.connection(ui);
                        ui.add_space(4.);
                        self.file(ui);
                        ui.add_space(6.);
                        self.actions(ui);
                        self.feedback(ui);
                        self.advanced(ui);
                    });
            });
    }
    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            logo(ui);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.;
                ui.label(RichText::new(brand::LAUNCHER_NAME).size(26.).strong());
                text(ui, brand::TAGLINE_EN);
            });
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                badge(ui, concat!("v", env!("CARGO_PKG_VERSION")), MUTED);
            });
        });
    }
    fn connection(&mut self, ui: &mut egui::Ui) {
        let (color, state, heading, hint) = match self.target {
            Target::NotRunning => (
                MUTED,
                "Not running",
                "Launch Minecraft to get started",
                "Injection becomes available once Minecraft is running.",
            ),
            Target::Running { loaded: false, .. } if !self.dll_exists => (
                WARN,
                "Not injected",
                "Select a client DLL",
                "Minecraft is running. Choose a DLL in the field below.",
            ),
            Target::Running { loaded: false, .. } => (
                ACCENT,
                "Ready",
                "Ready for Mio Client",
                "Minecraft is running. Click Inject Client to connect.",
            ),
            Target::Running { loaded: true, .. } => (
                GOOD,
                "Injected",
                "Mio Client is connected",
                "Use the client in game. Click Eject below to unload it.",
            ),
        };
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(9., 9.), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 4., color);
                text(ui, "Connection status");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    badge(ui, state, color);
                });
            });
            ui.add_space(6.);
            ui.label(RichText::new(heading).size(21.).strong());
            text(ui, hint);
            ui.add_space(4.);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(RichText::new(self.process.trim()).size(12.).color(MUTED))
                        .truncate(),
                )
                .on_hover_text(&self.process);
                if let Target::Running { pid, .. } = self.target {
                    badge(ui, &format!("PID {pid}"), MUTED);
                }
            });
        });
    }
    fn file(&mut self, ui: &mut egui::Ui) {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Client DLL").strong());
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    badge(
                        ui,
                        if self.dll_exists {
                            "DLL found"
                        } else {
                            "DLL not found"
                        },
                        if self.dll_exists { GOOD } else { WARN },
                    );
                });
            });
            ui.add_space(2.);
            ui.add_enabled_ui(self.pending.is_none(), |ui| {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_sized([96., 36.], egui::Button::new("Browse…"))
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_title("Select the Mio Client DLL")
                                .add_filter("DLL", &["dll"])
                                .pick_file()
                            {
                                self.dll = path.to_string_lossy().into_owned();
                                self.dll_exists = path.is_file();
                                self.message = None;
                            }
                        }
                        let edit = egui::TextEdit::singleline(&mut self.dll)
                            .font(egui::TextStyle::Body)
                            .margin(egui::vec2(10., 8.))
                            .hint_text("Path to Mio_Client.dll")
                            .desired_width(f32::INFINITY);
                        if ui.add_sized([ui.available_width(), 36.], edit).changed() {
                            self.dll_exists = std::path::Path::new(self.dll.trim()).is_file();
                            self.message = None;
                        }
                    });
                });
            });
            text(ui, "Settings and profiles use the same folder as this DLL.");
        });
    }
    fn actions(&mut self, ui: &mut egui::Ui) {
        let width = ((ui.available_width() - 20.) / 3.).max(1.);
        let busy = self.pending.is_some();
        let primary = if self.target == Target::NotRunning {
            Action::Launch
        } else {
            Action::Inject
        };
        ui.horizontal(|ui| {
            for (action, title) in [
                (Action::Launch, "Launch Game"),
                (Action::Inject, "Inject Client"),
                (Action::Eject, "Eject"),
            ] {
                let enabled = self.target.allows(action, busy, self.dll_exists);
                let highlight = action == primary && enabled;
                let button = egui::Button::new(RichText::new(title).strong().color(if highlight {
                    BG
                } else {
                    TEXT
                }))
                .fill(if highlight { ACCENT } else { SURFACE })
                .stroke(Stroke::new(1., if highlight { ACCENT } else { BORDER }))
                .corner_radius(10);
                let response = ui.add_enabled(enabled, |ui: &mut egui::Ui| {
                    ui.add_sized([width, 46.], button)
                });
                if response.clicked() {
                    self.start(action, ui.ctx());
                }
            }
        });
    }
    fn feedback(&self, ui: &mut egui::Ui) {
        if let Some(job) = &self.pending {
            card().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(18.).color(ACCENT));
                    ui.label(match job.action {
                        Action::Launch => "Requesting Minecraft launch…",
                        Action::Inject => "Injecting Mio Client…",
                        Action::Eject => "Requesting client eject…",
                    });
                });
            });
        } else if let Some(message) = &self.message {
            let (color, heading, body) = match message {
                Ok(value) => (GOOD, "Request completed", value),
                Err(value) => (ERROR, "Operation failed", value),
            };
            card()
                .stroke(Stroke::new(1., color.gamma_multiply(0.45)))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(heading).color(color).strong());
                    ui.add(egui::Label::new(body).wrap());
                });
        }
    }
    fn advanced(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(RichText::new("Advanced settings").color(MUTED)).show(
            ui,
            |ui| {
                ui.add_enabled_ui(self.pending.is_none(), |ui| {
                    text(ui, "Target process");
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.process)
                            .desired_width(f32::INFINITY)
                            .margin(egui::vec2(10., 8.)),
                    );
                    if response.changed() {
                        self.message = None;
                        self.target = Target::NotRunning;
                    }
                    if response.lost_focus() {
                        self.refresh();
                    }
                });
            },
        );
        ui.horizontal(|ui| {
            if ui.small_button("Refresh status").clicked() {
                self.refresh();
            }
            text(ui, "Status refreshes automatically every second");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    #[test]
    fn all_target_and_feedback_states_fit_small_windows_and_high_dpi() {
        for (size, scale) in [([620., 580.], 1.), ([720., 660.], 1.5), ([900., 760.], 2.)] {
            let ctx = egui::Context::default();
            super::super::theme::install(&ctx);
            let mut app = App {
                process: "Minecraft.Windows.exe".into(),
                dll: "D:/日本語のフォルダー/クライアント/Mio_Client.dll".into(),
                dll_exists: true,
                target: Target::NotRunning,
                polled_at: Instant::now(),
                message: None,
                pending: None,
            };
            for case in 0usize..8 {
                app.target = match case {
                    0 => Target::NotRunning,
                    1 | 2 => Target::Running {
                        pid: 12345,
                        loaded: false,
                    },
                    _ => Target::Running {
                        pid: 12345,
                        loaded: true,
                    },
                };
                app.dll_exists = case != 1;
                app.message = match case {
                    3 => Some(Ok("Mio Client injected. PID 12345".into())),
                    4 => Some(Err(format!(
                        "Could not load the DLL. {}",
                        "日本語の長いファイルパス/".repeat(15)
                    ))),
                    _ => None,
                };
                let (_tx, rx) = std::sync::mpsc::channel();
                app.pending = (case >= 5).then_some(super::super::Pending {
                    action: [Action::Launch, Action::Inject, Action::Eject][case.saturating_sub(5)],
                    result: rx,
                });
                for frame in 0..2 {
                    let mut input = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                        time: Some((case * 2 + frame) as f64 / 60.),
                        ..Default::default()
                    };
                    input
                        .viewports
                        .get_mut(&egui::ViewportId::ROOT)
                        .unwrap()
                        .native_pixels_per_point = Some(scale);
                    let output = ctx.run_ui(input, |ui| {
                        app.draw(ui);
                        assert!(
                            ui.min_rect().right() <= size[0] + 1.,
                            "horizontal overflow: case={case} size={size:?} rect={:?}",
                            ui.min_rect()
                        );
                    });
                    let meshes = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
                    assert!(!meshes.is_empty());
                    for mesh in meshes {
                        if let egui::epaint::Primitive::Mesh(mesh) = mesh.primitive {
                            assert!(mesh.vertices.iter().all(|v| v.pos.is_finite()));
                        }
                    }
                    output.drop_without_applying_deltas();
                }
            }
        }
    }
}
