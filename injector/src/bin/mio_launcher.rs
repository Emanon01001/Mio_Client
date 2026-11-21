//! Mio Launcher; the legacy executable name remains a compatible entrypoint.
#![windows_subsystem = "windows"]
#[path = "../launcher/mod.rs"]
mod launcher;
fn main() -> eframe::Result {
    launcher::run()
}
