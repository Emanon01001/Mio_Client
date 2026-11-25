//! Desktop launcher. Native operations stay on a worker; the GUI observes
//! the game until the user presses a launch / inject / eject button.
mod theme;
mod view;
use eframe::egui;
use injector::{default_dll, eject, find_process, inject, is_loaded, launch, DEFAULT_PROCESS};
#[path = "../../../shared/branding.rs"]
mod brand;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};
const POLL_INTERVAL: Duration = Duration::from_secs(1);
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Launch,
    Inject,
    Eject,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    NotRunning,
    Running { pid: u32, loaded: bool },
}
impl Target {
    fn poll(process: &str) -> Self {
        match find_process(process.trim()) {
            Ok(pid) => Self::Running {
                pid,
                loaded: is_loaded(pid),
            },
            Err(_) => Self::NotRunning,
        }
    }
    fn allows(self, action: Action, busy: bool, dll_exists: bool) -> bool {
        if busy {
            return false;
        }
        match action {
            Action::Launch => self == Self::NotRunning,
            Action::Inject => dll_exists && matches!(self, Self::Running { loaded: false, .. }),
            Action::Eject => matches!(self, Self::Running { loaded: true, .. }),
        }
    }
}
struct Pending {
    action: Action,
    result: Receiver<Result<String, String>>,
}
struct App {
    process: String,
    dll: String,
    dll_exists: bool,
    target: Target,
    polled_at: Instant,
    message: Option<Result<String, String>>,
    pending: Option<Pending>,
}
impl App {
    fn new() -> Self {
        let dll = default_dll().to_string_lossy().into_owned();
        let dll_exists = std::path::Path::new(&dll).is_file();
        Self {
            process: DEFAULT_PROCESS.into(),
            dll,
            dll_exists,
            target: Target::poll(DEFAULT_PROCESS),
            polled_at: Instant::now(),
            message: None,
            pending: None,
        }
    }
    fn refresh(&mut self) {
        self.target = Target::poll(&self.process);
        self.dll_exists = std::path::Path::new(self.dll.trim()).is_file();
        self.polled_at = Instant::now();
    }
    fn start(&mut self, action: Action, ctx: &egui::Context) {
        if !self
            .target
            .allows(action, self.pending.is_some(), self.dll_exists)
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(Pending { action, result: rx });
        self.message = None;
        let process = self.process.trim().to_string();
        let dll = PathBuf::from(self.dll.trim());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = match action {
                Action::Launch => launch().map(|()| "Minecraft launch requested. The status will update when the game starts.".into()),
                Action::Inject | Action::Eject => find_process(&process).and_then(|pid| match action {
                    Action::Inject if is_loaded(pid) => Err("Mio Client is already loaded.".into()),
                    Action::Inject => inject(pid, &dll).map(|()| format!("Mio Client injected. PID {pid}")),
                    Action::Eject => eject(pid).map(|()| format!("Eject requested for PID {pid}. The status will update when the client finishes unloading.")),
                    Action::Launch => unreachable!(),
                }),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(job) = &self.pending {
            match job.result.try_recv() {
                Ok(result) => {
                    self.message = Some(result);
                    self.pending = None;
                    self.refresh();
                }
                Err(TryRecvError::Disconnected) => {
                    self.message = Some(Err(
                        "The operation was interrupted. Refresh the status and try again.".into(),
                    ));
                    self.pending = None;
                    self.refresh();
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.polled_at.elapsed() >= POLL_INTERVAL {
            self.refresh();
        }
        ctx.request_repaint_after(if self.pending.is_some() {
            Duration::from_millis(50)
        } else {
            POLL_INTERVAL
        });
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}
pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720., 660.])
            .with_icon(theme::icon())
            .with_min_inner_size([620., 580.])
            .with_resizable(true)
            .with_maximize_button(false),
        ..Default::default()
    };
    eframe::run_native(
        brand::LAUNCHER_NAME,
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            Ok(Box::new(App::new()))
        }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn action_availability_respects_target_worker_and_file_state() {
        let ready = Target::Running {
            pid: 42,
            loaded: false,
        };
        let loaded = Target::Running {
            pid: 42,
            loaded: true,
        };
        assert!(Target::NotRunning.allows(Action::Launch, false, false));
        assert!(!Target::NotRunning.allows(Action::Inject, false, true));
        assert!(ready.allows(Action::Inject, false, true));
        assert!(!ready.allows(Action::Inject, false, false));
        assert!(!loaded.allows(Action::Inject, false, true));
        assert!(loaded.allows(Action::Eject, false, false));
        for target in [Target::NotRunning, ready, loaded] {
            for action in [Action::Launch, Action::Inject, Action::Eject] {
                assert!(!target.allows(action, true, true));
            }
        }
    }
}
