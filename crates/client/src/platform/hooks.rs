//! Shared MinHook teardown steps: disabling a hook and waiting until no call
//! is inside a detour, so the DLL may be unloaded.
use hudhook::mh::{MH_DisableHook, MH_STATUS};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::System::Threading::Sleep;

/// Disables the hook at `target`; one already disabled counts as done.
/// False if MinHook refuses (the caller must then keep the DLL mapped).
pub(crate) fn disable(target: usize) -> bool {
    matches!(
        unsafe { MH_DisableHook(target as *mut c_void) },
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_DISABLED
    )
}

/// Waits, checking 100 times `sleep_ms` apart and once more at the end, until
/// `in_flight` (calls currently inside a detour) is zero. False if a call is
/// still inside: the DLL must stay mapped.
pub(crate) fn drain(in_flight: &AtomicUsize, sleep_ms: u32) -> bool {
    for _ in 0..100 {
        if in_flight.load(Ordering::SeqCst) == 0 {
            return true;
        }
        unsafe { Sleep(sleep_ms) };
    }
    in_flight.load(Ordering::SeqCst) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drain_returns_once_calls_leave_and_reports_a_stuck_call() {
        let idle = AtomicUsize::new(0);
        assert!(drain(&idle, 1));
        let stuck = AtomicUsize::new(1);
        assert!(!drain(&stuck, 0));
        // A call leaving while waiting is noticed.
        let leaving = std::sync::Arc::new(AtomicUsize::new(1));
        let other = std::sync::Arc::clone(&leaving);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            other.store(0, Ordering::SeqCst);
        });
        assert!(drain(&leaving, 2));
        thread.join().unwrap();
    }
}
