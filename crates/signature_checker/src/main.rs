#![windows_subsystem = "windows"]
#[path = "../../shared/branding.rs"]
mod brand;
mod gui;
#[path = "../../injector/src/launcher/theme.rs"]
mod theme;

fn main() -> eframe::Result {
    let options = mio_signature_check::cli::parse(std::env::args_os().skip(1));
    eframe::run_native(
        "Mio Signature Checker",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1040., 780.])
                .with_min_inner_size([760., 560.])
                .with_icon(theme::icon()),
            ..Default::default()
        },
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            cc.egui_ctx
                .send_viewport_cmd(eframe::egui::ViewportCommand::SetTheme(
                    eframe::egui::SystemTheme::Dark,
                ));
            Ok(Box::new(gui::App::new(options)))
        }),
    )
}
