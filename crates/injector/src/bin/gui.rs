//! Alternate GUI entry point for Mio Launcher.
#![windows_subsystem = "windows"]
#[path = "../launcher/mod.rs"]
mod launcher;
fn main() -> eframe::Result {
    launcher::run()
}
