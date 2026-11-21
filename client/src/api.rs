//! Native host for `mcbe-sdk`. Use [`game`] from modules. Other injected Rust
//! DLLs can depend on `client` with `default-features = false` and attach a Runtime.
pub use crate::config::Offsets;
pub use mcbe_sdk::*;
#[cfg(feature = "entrypoint")]
mod diagnostics;
mod observations;
mod subscriptions;
#[cfg(feature = "entrypoint")]
pub(crate) use diagnostics::command as observation_command;

use crate::{modules::game as native, native_chat};
use mcbe_sdk::bridge::{Bridge, Pending, World};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

struct Connection {
    game: Game,
    bridge: Bridge,
    observations: Arc<observations::NativeObservations>,
}
static CONNECTION: OnceLock<Connection> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);
static STOPPING: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static INPUT_READY: AtomicBool = AtomicBool::new(false);

fn clock() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}
pub fn game() -> Result<Game> {
    if STOPPING.load(Ordering::Acquire) {
        return Err(Error::Detached);
    }
    CONNECTION
        .get()
        .map(|c| c.game.clone())
        .ok_or(Error::Detached)
}

/// Keeps native hooks alive. One attachment per DLL lifetime.
/// Call `shutdown()` before unloading; if it returns false, keep the DLL mapped.
pub struct Runtime {
    sampling: bool,
    stopped: bool,
}
impl Runtime {
    /// # Safety
    /// Run on a worker thread inside Minecraft.Windows.exe, never DllMain.
    /// Keep the DLL mapped until `shutdown()` reports true, including on error.
    pub unsafe fn attach(offsets: &Offsets) -> Result<Self> {
        if STARTED.swap(true, Ordering::AcqRel) {
            return Err(Error::Unavailable(
                "SDK runtime already attached in this DLL".into(),
            ));
        }
        let (game, bridge) = Bridge::with_clock(clock);
        let observations = Arc::new(observations::NativeObservations::new());
        let _ = CONNECTION.set(Connection {
            game,
            bridge,
            observations,
        });
        native::init(offsets);
        if let Err(error) = native::acquire() {
            CONNECTION.get().unwrap().bridge.close();
            return Err(Error::Unavailable(error));
        }
        let connection = CONNECTION.get().unwrap();
        connection
            .bridge
            .set_observation_backend(connection.observations.clone());
        let actions = native::actions::available();
        if let Err(error) = &actions {
            crate::log!("SDK actions unavailable: {error}");
        }
        INPUT_READY.store(actions.is_ok(), Ordering::Release);
        CONNECTION.get().unwrap().bridge.commands_available(actions);
        let chat = native_chat::start();
        if let Err(error) = &chat {
            crate::log!("SDK chat unavailable: {error}");
        }
        CONNECTION.get().unwrap().bridge.chat_available(chat);
        Ok(Self {
            sampling: true,
            stopped: false,
        })
    }
    pub fn pause(&self, paused: bool) {
        PAUSED.store(paused, Ordering::Release);
    }
    pub fn shutdown(&mut self) -> bool {
        if self.stopped {
            return true;
        }
        STOPPING.store(true, Ordering::Release);
        #[cfg(feature = "entrypoint")]
        diagnostics::clear();
        if let Some(c) = CONNECTION.get() {
            c.bridge.close();
        }
        let observations_ok = CONNECTION.get().is_none_or(|c| c.observations.close());
        let actions_ok = native::actions::shutdown();
        let chat_ok = native_chat::shutdown();
        let sampling_ok = if self.sampling {
            match native::release() {
                Ok(()) => {
                    self.sampling = false;
                    true
                }
                Err(_) => false,
            }
        } else {
            true
        };
        self.stopped = observations_ok && actions_ok && chat_ok && sampling_ok;
        self.stopped
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub(crate) fn blocked() -> bool {
    STOPPING.load(Ordering::Acquire) || PAUSED.load(Ordering::Acquire)
}
pub(crate) fn input_ready() -> bool {
    INPUT_READY.load(Ordering::Acquire)
}
pub(crate) fn frame(world: Option<World>, player: Option<Player>, active: bool) -> Vec<Pending> {
    let Some(c) = CONNECTION.get() else {
        return Vec::new();
    };
    c.bridge
        .publish_and_drain(world, player, active && !blocked())
}
pub(crate) fn chat(direction: ChatDirection, sender: Option<String>, text: String) {
    if let Some(c) = CONNECTION.get() {
        c.bridge.push_chat(direction, sender, text);
    }
}
