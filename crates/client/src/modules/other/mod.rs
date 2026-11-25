//! Tools that fit no other category: chat logging/sending, join notices,
//! skin export, profiling, Discord presence and user patches.
pub mod chat_log;
pub mod chat_repeat;
pub mod discord_rpc;
pub mod join_notify;
pub mod patch;
pub(crate) mod performance_profiler;
pub(crate) mod skin_stealer;
