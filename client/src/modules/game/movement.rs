//! Fly controls and native velocity units.
use super::{layout::Layout, read, readable};
use crate::input;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// --- Fly -------------------------------------------------------------------------

/// StateVectorComponent = { pos, prev pos, velocity }, 3 floats each.
pub(super) const STATE_VELOCITY: usize = 0x18;
/// Game ticks per second; velocity is in blocks per tick.
pub(super) const TICKS_PER_SECOND: f32 = 20.0;

static FLY: AtomicBool = AtomicBool::new(false);
/// Whether fly may read the keyboard (game focused, no menu open).
static FLY_INPUT: AtomicBool = AtomicBool::new(false);
/// f32 bits, blocks per second.
static FLY_SPEED: AtomicU32 = AtomicU32::new(0);

pub fn set_fly(on: bool) {
    FLY.store(on, Ordering::Relaxed);
}

pub fn set_fly_input(active: bool) {
    FLY_INPUT.store(active, Ordering::Relaxed);
}

pub fn set_fly_speed(blocks_per_second: f32) {
    FLY_SPEED.store(blocks_per_second.to_bits(), Ordering::Relaxed);
}

/// Sets the player's velocity from the movement keys: W/A/S/D along the view
/// direction, Space up, Shift down, and hovering when nothing is held.
unsafe fn apply_fly(l: &Layout, instance: usize, state: usize) {
    let held = |vk: u16| FLY_INPUT.load(Ordering::Relaxed) && input::key_down(vk);
    let axis = |plus: u16, minus: u16| held(plus) as i32 as f32 - held(minus) as i32 as f32;
    let (forward, strafe, lift) = (axis(0x57, 0x53), axis(0x44, 0x41), axis(0x20, 0x10));

    let mut velocity = [0.0f32, lift, 0.0];
    if (forward != 0.0 || strafe != 0.0) && readable(instance + l.view_matrix, 48) {
        let m: [f32; 12] = read(instance + l.view_matrix);
        // right = m[0..3], back = m[8..11]; keep the motion horizontal.
        let x = strafe * m[0] - forward * m[8];
        let z = strafe * m[2] - forward * m[10];
        let length = (x * x + z * z).sqrt();
        if length.is_finite() && length > 1.0e-3 {
            velocity[0] = x / length;
            velocity[2] = z / length;
        }
    }
    let speed = f32::from_bits(FLY_SPEED.load(Ordering::Relaxed)) / TICKS_PER_SECOND;
    if speed.is_finite() {
        ((state + STATE_VELOCITY) as *mut [f32; 3]).write_unaligned(velocity.map(|v| v * speed));
    }
}

pub(super) unsafe fn apply_movement(l: &Layout, instance: usize, state: usize) {
    if super::freecam::blocking() {
        return;
    }
    if FLY.load(Ordering::Relaxed) {
        apply_fly(l, instance, state);
    }
}
