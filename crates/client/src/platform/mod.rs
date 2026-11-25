//! Integration with Windows and the game process outside the game's data:
//! MinHook teardown, key state, the clipboard, the server address and the
//! native chat boundaries.
pub(crate) mod clipboard;
pub(crate) mod hooks;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod input;
pub(crate) mod native_chat;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod net;
