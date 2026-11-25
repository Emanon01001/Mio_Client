//! Mio Client: an injected utility client for Minecraft Bedrock (Windows).
//!
//! The default `entrypoint` feature starts the built-in client on a worker thread.
//! Disable it to embed the native [`api`] runtime in another injected Rust DLL.
// The library name gives the DLL its file name, `Mio_Client.dll`.
#![allow(non_snake_case)]

pub mod api;
mod app;
#[path = "../../shared/branding.rs"]
mod brand;
#[cfg(feature = "entrypoint")]
mod client;
#[path = "../../shared/compatibility.rs"]
mod compatibility;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
mod game;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
mod modules;
mod platform;
#[path = "../../shared/skin_layout_checks.rs"]
mod skin_layout_checks;
mod ui;

// Every module keeps its short crate path (`crate::config`, `crate::gui`, ...)
// wherever its file lives: `app/` (settings and services), `ui/` (overlay),
// `platform/` (Windows and process integration), `game/` (game data) and
// `modules/` (features).
#[cfg(feature = "entrypoint")]
use app::binds;
use app::{config, console, localization, notify, profiles, text};
use game::signatures;
use platform::{clipboard, hooks, input, native_chat, net};
use ui::{gui, hud_editor, theme};

#[cfg(feature = "entrypoint")]
mod entrypoint {
    use crate::client;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{CloseHandle, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{
        DisableThreadLibraryCalls, FreeLibraryAndExitThread,
    };
    use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
    use windows_sys::Win32::System::Threading::CreateThread;

    /// # Safety
    /// Called by the Windows loader only.
    #[no_mangle]
    pub unsafe extern "system" fn DllMain(
        hmodule: HMODULE,
        reason: u32,
        _reserved: *mut c_void,
    ) -> BOOL {
        if reason == DLL_PROCESS_ATTACH {
            DisableThreadLibraryCalls(hmodule);
            let thread = CreateThread(null(), 0, Some(main_thread), hmodule, 0, null_mut());
            if !thread.is_null() {
                CloseHandle(thread);
            }
        }
        1
    }

    unsafe extern "system" fn main_thread(param: *mut c_void) -> u32 {
        // A panic must never unwind into the game. On panic we also stay loaded,
        // since hooks may still point into this DLL.
        let unload = std::panic::catch_unwind(|| client::run(param as HMODULE)).unwrap_or(false);
        if unload {
            FreeLibraryAndExitThread(param as HMODULE, 0);
        }
        0
    }
}
