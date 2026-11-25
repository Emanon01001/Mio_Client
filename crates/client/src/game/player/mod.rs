//! The local player: inventory and items, status, mining, telemetry,
//! fishing, safe walking, and movement/mouse control (`control`).
pub(super) mod control;
pub mod durability_guard;
pub mod fishing;
pub mod inventory;
pub(super) mod inventory_transfer;
pub mod item_assist;
pub mod items;
pub mod mining;
pub mod safe_walk;
pub mod status;
pub mod telemetry;
