//! The client's own services: configuration and profiles, key binds, the
//! console and log, localization, notifications and display text.
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod binds;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod config;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod console;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod localization;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod notify;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod profiles;
#[cfg_attr(not(feature = "entrypoint"), allow(dead_code))]
pub(crate) mod text;
