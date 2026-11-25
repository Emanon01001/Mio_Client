//! Local player controls: fly controls and native velocity units, and
//! suppressing native mouse turning and wheel input while the overlay captures
//! input.
use crate::game::{actions::IN_FLIGHT, layout::Layout, read, readable};
use crate::{gui, input};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

// --- Fly -------------------------------------------------------------------------

/// StateVectorComponent = { pos, prev pos, velocity }, 3 floats each.
pub(in crate::game) const STATE_VELOCITY: usize = 0x18;
/// Game ticks per second; velocity is in blocks per tick.
pub(in crate::game) const TICKS_PER_SECOND: f32 = 20.0;

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

pub(in crate::game) unsafe fn apply_movement(l: &Layout, instance: usize, state: usize) {
    if crate::game::freecam::blocking() {
        return;
    }
    if FLY.load(Ordering::Relaxed) {
        apply_fly(l, instance, state);
    }
}

// ---- Mouse ---------------------------------------------------------------

pub(in crate::game) static MOUSE_FEED: AtomicUsize = AtomicUsize::new(0);
pub(in crate::game) static MOUSE_REBASE: AtomicUsize = AtomicUsize::new(0);
pub(in crate::game) static BLOCKED_MOTION: AtomicU64 = AtomicU64::new(0);
// MouseDevice::feed routes button 4 into its wheel event queue in this build.
const WHEEL_BUTTON: u8 = 4;

// Native MouseDevice::feed: absolute position and raw relative motion both
// reach this boundary. ImGui receives its cursor position through WndProc.
pub(in crate::game) type MouseFeedFn =
    unsafe extern "system" fn(usize, u8, u8, i16, i16, i16, i16, u8);
pub(in crate::game) unsafe extern "system" fn mouse_feed_hook(
    mouse: usize,
    button: u8,
    down: u8,
    x: i16,
    y: i16,
    mut dx: i16,
    mut dy: i16,
    motion: u8,
) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    if gui::captures_input() {
        // Keep the game's absolute position steady as well as its raw deltas:
        // the legacy mouse path derives turning from the absolute positions.
        let position: [i16; 2] = read(mouse + 4);
        // Discard any relative movement already waiting for the input update.
        ((mouse + 8) as *mut [i16; 2]).write_unaligned([0, 0]);
        ((mouse + 12) as *mut [i16; 2]).write_unaligned(position);
        MOUSE_REBASE.store(mouse, Ordering::Release);
        BLOCKED_MOTION.fetch_add(1, Ordering::Relaxed);
        // MouseDevice::feed queues even zero-delta and button events. These
        // still reach native screens, so freeze alone is insufficient.
        let begin = read::<usize>(mouse + 0x18);
        let end = read::<usize>(mouse + 0x20);
        let capacity = read::<usize>(mouse + 0x28);
        if begin <= end
            && end <= capacity
            && (end - begin).is_multiple_of(0x14)
            && capacity - begin <= 1024 * 1024
        {
            ((mouse + 0x20) as *mut usize).write_unaligned(begin);
        }
        ((mouse + 0x10) as *mut [u8; 7]).write_unaligned([0; 7]);
        ((mouse + 0x30) as *mut i32).write_unaligned(0);
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        return;
    } else if button != WHEEL_BUTTON
        && MOUSE_REBASE
            .compare_exchange(mouse, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        // Seed both positions from the first post-menu sample. Otherwise the
        // legacy path turns by the distance the UI cursor moved while open.
        let position = [x, y];
        ((mouse + 4) as *mut [i16; 2]).write_unaligned(position);
        ((mouse + 8) as *mut [i16; 2]).write_unaligned([0, 0]);
        ((mouse + 12) as *mut [i16; 2]).write_unaligned(position);
        dx = 0;
        dy = 0;
    }
    let original: MouseFeedFn = std::mem::transmute(MOUSE_FEED.load(Ordering::Acquire));
    original(mouse, button, down, x, y, dx, dy, motion);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}
