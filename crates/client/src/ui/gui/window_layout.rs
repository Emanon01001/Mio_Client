//! Remembers the size and position of the ClickGUI and console windows
//! across restarts (`Mio_Client_windows.toml` next to the configuration).
//!
//! The render thread only records changes (never blocking: a busy lock skips
//! the frame); the client worker writes the file once a change has settled
//! and again at shutdown, through a temporary file and a rename.
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub(crate) const FILE_NAME: &str = "Mio_Client_windows.toml";
/// A change is written once the window has been left alone this long (ms),
/// so dragging a corner does not write every frame.
const SETTLE_MS: u64 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Rect {
    pub position: [f32; 2],
    pub size: [f32; 2],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Layout {
    menu: Option<Rect>,
    console: Option<Rect>,
}
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Window {
    Menu,
    Console,
}
struct State {
    current: Layout,
    written: Layout,
    changed_at: Option<u64>,
}
static STATE: Mutex<State> = Mutex::new(State {
    current: Layout {
        menu: None,
        console: None,
    },
    written: Layout {
        menu: None,
        console: None,
    },
    changed_at: None,
});

impl Layout {
    fn slot(&mut self, window: Window) -> &mut Option<Rect> {
        match window {
            Window::Menu => &mut self.menu,
            Window::Console => &mut self.console,
        }
    }
}

fn now() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

fn sane(rect: Rect) -> bool {
    rect.position
        .iter()
        .chain(&rect.size)
        .all(|v| v.is_finite() && v.abs() < 32768.)
        && rect.size.iter().all(|v| *v > 0.)
}

/// Reads the saved layout; a missing or unreadable file keeps the defaults.
pub(crate) fn load(dir: &Path) {
    let layout = std::fs::read_to_string(dir.join(FILE_NAME))
        .ok()
        .and_then(|text| toml::from_str::<Layout>(&text).ok())
        .unwrap_or_default();
    let keep = |r: Option<Rect>| r.filter(|r| sane(*r));
    let layout = Layout {
        menu: keep(layout.menu),
        console: keep(layout.console),
    };
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.current = layout;
    state.written = layout;
    state.changed_at = None;
}

/// Where `window` opens: the saved rectangle fitted to `screen`, or the
/// given default. `min` is the window's minimum size.
pub(crate) fn initial(window: Window, screen: [f32; 2], default: Rect, min: [f32; 2]) -> Rect {
    let saved = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .current
        .slot(window)
        .to_owned();
    match saved {
        Some(saved) => fit(saved, screen, min),
        None => default,
    }
}

/// Keeps a saved rectangle on a (possibly smaller) screen.
fn fit(rect: Rect, screen: [f32; 2], min: [f32; 2]) -> Rect {
    let size: [f32; 2] =
        std::array::from_fn(|k| rect.size[k].max(min[k]).min(screen[k].max(min[k])));
    let position =
        std::array::from_fn(|k| rect.position[k].clamp(0., (screen[k] - size[k]).max(0.)));
    Rect { position, size }
}

/// Records the window's current rectangle (render thread, every frame).
pub(crate) fn observe(window: Window, position: [f32; 2], size: [f32; 2]) {
    let rect = Rect { position, size };
    if !sane(rect) {
        return;
    }
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    let slot = state.current.slot(window);
    if *slot != Some(rect) {
        *slot = Some(rect);
        state.changed_at = Some(now());
    }
}

/// Writes a settled change (client worker). `force` writes any unsaved
/// change at once (shutdown).
pub(crate) fn poll(dir: &Path, force: bool) {
    let layout = {
        let Ok(mut state) = STATE.try_lock() else {
            return;
        };
        let due = state
            .changed_at
            .is_some_and(|at| force || now().saturating_sub(at) >= SETTLE_MS);
        if !due || state.current == state.written {
            if due {
                state.changed_at = None;
            }
            return;
        }
        state.changed_at = None;
        state.current
    };
    match write(dir, &layout) {
        Ok(()) => STATE.lock().unwrap_or_else(|e| e.into_inner()).written = layout,
        Err(e) => crate::log!("ウィンドウの大きさを保存できません: {e}"),
    }
}

fn write(dir: &Path, layout: &Layout) -> Result<(), String> {
    let text = toml::to_string(layout).map_err(|e| e.to_string())?;
    let path = dir.join(FILE_NAME);
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_windows_are_fitted_to_a_smaller_screen() {
        let min = [420., 380.];
        let saved = Rect {
            position: [1500., 900.],
            size: [800., 700.],
        };
        // Fits as saved on a large screen.
        assert_eq!(fit(saved, [2560., 1600.], min), saved);
        // Pulled back on screen, shrunk to it, never below the minimum.
        let small = fit(saved, [1280., 720.], min);
        assert_eq!(small.size, [800., 700.]);
        assert_eq!(small.position, [480., 20.]);
        let tiny = fit(saved, [300., 200.], min);
        assert_eq!((tiny.size, tiny.position), ([420., 380.], [0., 0.]));
        assert!(!sane(Rect {
            position: [f32::NAN, 0.],
            size: [1., 1.]
        }));
        assert!(!sane(Rect {
            position: [0., 0.],
            size: [0., 10.]
        }));
    }

    #[test]
    fn changes_are_written_after_settling_and_restored_on_the_next_start() {
        // Menu rendering tests record into the same state.
        let _serial = super::super::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "mio-window-layout-{}-{}",
            std::process::id(),
            now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        load(&dir);
        let screen = [1920., 1080.];
        let default = Rect {
            position: [40., 40.],
            size: [500., 600.],
        };
        assert_eq!(
            initial(Window::Menu, screen, default, [420., 380.]),
            default
        );
        observe(Window::Menu, [100., 50.], [640., 720.]);
        observe(Window::Console, [800., 40.], [600., 400.]);
        // Not settled yet: nothing written.
        poll(&dir, false);
        assert!(!dir.join(FILE_NAME).exists());
        // Shutdown writes at once; a restart restores it.
        poll(&dir, true);
        load(&dir);
        assert_eq!(
            initial(Window::Menu, screen, default, [420., 380.]),
            Rect {
                position: [100., 50.],
                size: [640., 720.]
            }
        );
        assert_eq!(
            initial(Window::Console, screen, default, [300., 200.]).size,
            [600., 400.]
        );
        // A broken file keeps the defaults.
        std::fs::write(dir.join(FILE_NAME), "menu = 5").unwrap();
        load(&dir);
        assert_eq!(
            initial(Window::Menu, screen, default, [420., 380.]),
            default
        );
        let _ = std::fs::remove_dir_all(&dir);
        load(&std::env::temp_dir().join("mio-window-layout-none"));
    }
}
