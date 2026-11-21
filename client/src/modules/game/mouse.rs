//! Suppress native mouse turning and wheel input while the overlay captures input.
use super::{actions::IN_FLIGHT, read};
use crate::gui;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub(super) static MOUSE_FEED: AtomicUsize = AtomicUsize::new(0);
pub(super) static MOUSE_REBASE: AtomicUsize = AtomicUsize::new(0);
pub(super) static BLOCKED_MOTION: AtomicU64 = AtomicU64::new(0);
// MouseDevice::feed routes button 4 into its wheel event queue in this build.
const WHEEL_BUTTON: u8 = 4;

// Native MouseDevice::feed: absolute position and raw relative motion both
// reach this boundary. ImGui receives its cursor position through WndProc.
pub(super) type MouseFeedFn = unsafe extern "system" fn(usize, u8, u8, i16, i16, i16, i16, u8);
pub(super) unsafe extern "system" fn mouse_feed_hook(
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
