//! Game-speed timer: hooks `ntdll!RtlQueryPerformanceCounter` (the function
//! every QueryPerformanceCounter call ends up in) and scales elapsed time.
//!
//! Uses the MinHook build bundled with hudhook, so both share one instance.

use std::ffi::c_void;
use std::sync::atomic::{fence, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use hudhook::mh::{MH_CreateHook, MH_DisableHook, MH_EnableHook, MH_Initialize, MH_STATUS};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use super::{key_or_warn, Module, Param};
use crate::config;
use crate::log;

const MIN_SPEED: f64 = 0.1;
const MAX_SPEED: f64 = 10.0;

type QpcFn = unsafe extern "system" fn(*mut i64) -> i32;

struct Clock {
    speed: f64,
    base_real: i64,
    base_fake: i64,
}

impl Clock {
    fn fake_at(&self, real: i64) -> i64 {
        self.base_fake + ((real - self.base_real) as f64 * self.speed) as i64
    }
}

// The clock, published with a sequence lock: the hook runs on every thread's
// QueryPerformanceCounter and must not block. Readers take a consistent copy
// and retry in the rare case a speed change was being written meanwhile; a
// Mutex here measured 0.25-2 us per call with 4-16 threads calling at once,
// against about 25 ns (`bench::qpc_hook_cost`).
//
// `SEQ` is odd while a change is being written. A writer marks it odd before
// reading the real counter it rebases at, so every reading taken with the old
// clock precedes that counter value and none taken later goes backwards.
static SEQ: AtomicU64 = AtomicU64::new(0);
static SPEED: AtomicU64 = AtomicU64::new(0x3FF0_0000_0000_0000); // 1.0
static BASE_REAL: AtomicI64 = AtomicI64::new(0);
static BASE_FAKE: AtomicI64 = AtomicI64::new(0);
/// Serializes writers (the client thread in practice).
static WRITER: Mutex<()> = Mutex::new(());

fn load_clock() -> Clock {
    Clock {
        speed: f64::from_bits(SPEED.load(Ordering::Relaxed)),
        base_real: BASE_REAL.load(Ordering::Relaxed),
        base_fake: BASE_FAKE.load(Ordering::Relaxed),
    }
}

/// The fake counter at the reading `real` returns, with `real` called while a
/// consistent copy of the clock is held (called again on a retry).
fn fake_now(mut real: impl FnMut() -> i64) -> i64 {
    let mut tries = 0u32;
    loop {
        let before = SEQ.load(Ordering::Acquire);
        if before & 1 == 0 {
            let clock = load_clock();
            let now = real();
            fence(Ordering::Acquire);
            if SEQ.load(Ordering::Relaxed) == before {
                return clock.fake_at(now);
            }
        }
        tries += 1;
        if tries.is_multiple_of(64) {
            // The writer may have been preempted inside its few instructions.
            std::thread::yield_now();
        } else {
            std::hint::spin_loop();
        }
    }
}

/// Replaces the clock with `next(current clock, real counter now)`, where the
/// counter is read by `real` after readers were told to wait; nothing in
/// between may call the hooked counter. None from `real` changes nothing.
fn rebase(real: impl FnOnce() -> Option<i64>, next: impl FnOnce(&Clock, i64) -> Clock) {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let seq = SEQ.load(Ordering::Relaxed);
    SEQ.store(seq.wrapping_add(1), Ordering::Relaxed);
    fence(Ordering::SeqCst);
    let changed = real().map(|now| next(&load_clock(), now));
    if let Some(clock) = changed {
        SPEED.store(clock.speed.to_bits(), Ordering::Relaxed);
        BASE_REAL.store(clock.base_real, Ordering::Relaxed);
        BASE_FAKE.store(clock.base_fake, Ordering::Relaxed);
    }
    SEQ.store(seq.wrapping_add(2), Ordering::Release);
}

/// Address of the hooked function while the hook is enabled (0 = disabled).
static TARGET: AtomicUsize = AtomicUsize::new(0);
/// Trampoline to the original function; non-zero once the hook was created.
/// Never cleared: a thread may still be inside `detour` after disabling.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

unsafe fn real_now() -> Option<i64> {
    let original = ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return None;
    }
    let original: QpcFn = std::mem::transmute(original);
    let mut real = 0i64;
    original(&mut real);
    Some(real)
}

/// A raw profiling counter and its source identity. Discard a span if Timer's
/// trampoline was first installed between its endpoints; never mix fake/real time.
pub(crate) fn profile_counter() -> Option<(i64, usize)> {
    use windows_sys::Win32::System::Performance::QueryPerformanceCounter;
    let source = ORIGINAL.load(Ordering::Acquire);
    let mut ticks = 0;
    let success = unsafe {
        if source == 0 {
            QueryPerformanceCounter(&mut ticks)
        } else {
            let counter: QpcFn = std::mem::transmute(source);
            counter(&mut ticks)
        }
    };
    (success != 0 && ORIGINAL.load(Ordering::Acquire) == source).then_some((ticks, source))
}

unsafe extern "system" fn detour(out: *mut i64) -> i32 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: QpcFn = std::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    let mut result = 0;
    let fake = fake_now(|| {
        let mut real = 0i64;
        result = original(&mut real);
        real
    });
    if !out.is_null() {
        *out = fake;
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}

fn install() -> Result<(), String> {
    if TARGET.load(Ordering::Acquire) != 0 {
        return Ok(());
    }
    unsafe {
        let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        let module = GetModuleHandleW(ntdll.as_ptr());
        let target = GetProcAddress(module, c"RtlQueryPerformanceCounter".as_ptr().cast())
            .ok_or("RtlQueryPerformanceCounter not found")? as *mut c_void;

        // Start the fake clock exactly at the real one so nothing jumps.
        // (The hook is disabled here, so calling the target directly is real.)
        let direct: QpcFn = std::mem::transmute(target);
        rebase(
            || {
                let mut now = 0i64;
                direct(&mut now);
                Some(now)
            },
            |_, now| Clock {
                speed: 1.0,
                base_real: now,
                base_fake: now,
            },
        );

        if ORIGINAL.load(Ordering::Acquire) == 0 {
            match MH_Initialize() {
                MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
                e => return Err(format!("MH_Initialize: {e:?}")),
            }
            let mut trampoline = std::ptr::null_mut();
            MH_CreateHook(target, detour as *mut c_void, &mut trampoline)
                .ok()
                .map_err(|e| format!("MH_CreateHook: {e:?}"))?;
            ORIGINAL.store(trampoline as usize, Ordering::Release);
        }
        MH_EnableHook(target)
            .ok()
            .map_err(|e| format!("MH_EnableHook: {e:?}"))?;
        TARGET.store(target as usize, Ordering::Release);
    }
    Ok(())
}

/// Changes speed without a discontinuity: rebases both clocks at "now".
fn set_speed(speed: f64) {
    if TARGET.load(Ordering::Acquire) == 0 {
        return;
    }
    // The trampoline reads the real counter without going through the hook.
    rebase(
        || unsafe { real_now() },
        |clock, now| Clock {
            speed,
            base_real: now,
            base_fake: clock.fake_at(now),
        },
    );
}

/// Disables the hook if that cannot make the game's clock run backwards.
///
/// After running fast, the fake clock is ahead of the real one; unhooking
/// would make time jump back, so the hook (and the DLL) must stay in place at
/// 1.0x. Returns true if the DLL may be unloaded.
pub fn uninstall_if_safe() -> bool {
    let target = TARGET.load(Ordering::Acquire);
    if target == 0 {
        return IN_FLIGHT.load(Ordering::SeqCst) == 0;
    }
    set_speed(1.0);
    let ahead = match unsafe { real_now() } {
        Some(real) => load_clock().fake_at(real) > real,
        None => false,
    };
    if ahead {
        return false;
    }
    if unsafe { MH_DisableHook(target as *mut c_void) }
        .ok()
        .is_err()
    {
        return false;
    }
    TARGET.store(0, Ordering::Release);
    crate::hooks::drain(&IN_FLIGHT, 1)
}

fn clamp_speed(speed: f64) -> f64 {
    if speed.is_finite() {
        speed.clamp(MIN_SPEED, MAX_SPEED)
    } else {
        1.0
    }
}

pub struct Timer {
    key: u16,
    enabled: bool,
    speed: f64,
}

impl Timer {
    pub fn new(cfg: &config::Timer) -> Self {
        Self {
            key: key_or_warn("timer", &cfg.key),
            enabled: false,
            speed: clamp_speed(cfg.speed),
        }
    }
}

impl Module for Timer {
    fn name(&self) -> &str {
        "timer"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if on {
            if let Err(e) = install() {
                log!("timer: hook failed: {e}");
                return;
            }
            set_speed(self.speed);
        } else {
            set_speed(1.0);
        }
        self.enabled = on;
    }

    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "speed",
            value: self.speed,
            min: MIN_SPEED,
            max: 5.0,
        })
    }

    fn set_param(&mut self, value: f64) {
        self.speed = clamp_speed(value);
        if self.enabled {
            set_speed(self.speed);
        }
    }

    fn status(&self) -> String {
        format!("{:.2}x", self.speed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Threading::Sleep;

    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(count: *mut i64) -> i32;
    }

    fn qpc() -> i64 {
        let mut t = 0;
        unsafe { QueryPerformanceCounter(&mut t) };
        t
    }

    /// The clock is process-wide; tests that change it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    #[ignore = "manual isolated-process regression; installs the process-local Timer hook"]
    fn profiler_counter_bypasses_active_timer_hook() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let before = profile_counter().unwrap();
        install().unwrap();
        set_speed(8.);
        let first = profile_counter().unwrap();
        let fake_first = qpc();
        unsafe {
            Sleep(30);
        }
        let last = profile_counter().unwrap();
        let fake_last = qpc();
        set_speed(1.);
        // Keep the offset clock installed until this test process exits; the
        // production unload policy also forbids jumping a clock backwards.
        assert_ne!(before.1, first.1);
        assert_eq!(first.1, last.1);
        assert!(last.0 > first.0);
        let ratio = (fake_last - fake_first) as f64 / (last.0 - first.0) as f64;
        assert!((ratio - 8.).abs() < 0.5, "fake/raw counter ratio: {ratio}");
        eprintln!("Timer=8x: scaled/raw ratio={ratio:.3}; profiler used the raw trampoline");
    }

    fn start_clock(speed: f64) {
        rebase(
            || Some(qpc()),
            |_, now| Clock {
                speed,
                base_real: now,
                base_fake: now,
            },
        );
    }

    fn change_speed(speed: f64) {
        rebase(
            || Some(qpc()),
            |clock, now| Clock {
                speed,
                base_real: now,
                base_fake: clock.fake_at(now),
            },
        );
    }

    /// Readings never go backwards on any thread, and readings taken one
    /// after another (across threads) never decrease, while the speed keeps
    /// changing between 0.1x and 10x.
    #[test]
    fn readings_stay_monotonic_while_the_speed_changes() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start_clock(1.0);
        let stop = std::sync::atomic::AtomicBool::new(false);
        // The largest reading returned so far by any thread.
        let latest = AtomicI64::new(i64::MIN);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    let mut last = i64::MIN;
                    let mut reads = 0u64;
                    while !stop.load(Ordering::Relaxed) || reads < 10_000 {
                        // Anything published before this call started must
                        // not exceed what it returns.
                        let floor = latest.load(Ordering::SeqCst);
                        let now = fake_now(qpc);
                        assert!(now >= last, "went back on one thread: {last} -> {now}");
                        assert!(now >= floor, "earlier reading {floor} > {now}");
                        latest.fetch_max(now, Ordering::SeqCst);
                        last = now;
                        reads += 1;
                    }
                });
            }
            for i in 0..2_000 {
                change_speed(if i % 2 == 0 { 10.0 } else { 0.1 });
                std::hint::spin_loop();
            }
            stop.store(true, Ordering::Relaxed);
        });
        start_clock(1.0);
    }

    #[test]
    fn a_speed_change_keeps_the_clock_continuous_and_scales_what_follows() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        rebase(
            || Some(1_000),
            |_, now| Clock {
                speed: 1.0,
                base_real: now,
                base_fake: now,
            },
        );
        assert_eq!(fake_now(|| 1_500), 1_500);
        rebase(
            || Some(2_000),
            |clock, now| Clock {
                speed: 2.0,
                base_real: now,
                base_fake: clock.fake_at(now),
            },
        );
        assert_eq!(fake_now(|| 2_000), 2_000);
        assert_eq!(fake_now(|| 2_100), 2_200);
        // Without a real reading nothing changes.
        rebase(|| None, |_, _| unreachable!());
        assert_eq!(fake_now(|| 2_100), 2_200);
        start_clock(1.0);
    }

    /// ns per call of the hook's clock arithmetic with `threads` threads
    /// calling it at once, against a Mutex (the implementation before) and
    /// QueryPerformanceCounter alone.
    #[test]
    #[ignore = "manual: cargo test -p client --release --lib qpc_hook_cost -- --ignored --nocapture"]
    fn qpc_hook_cost() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        static LOCKED: Mutex<Clock> = Mutex::new(Clock {
            speed: 1.5,
            base_real: 0,
            base_fake: 0,
        });
        fn locked() -> i64 {
            let real = qpc();
            LOCKED.lock().unwrap().fake_at(real)
        }
        fn sequence_lock() -> i64 {
            fake_now(qpc)
        }
        start_clock(1.5);
        const CALLS: usize = 500_000;
        for threads in [1, 4, 8, 16] {
            for (name, f) in [
                ("QPC alone", qpc as fn() -> i64),
                ("Mutex", locked as fn() -> i64),
                ("seqlock", sequence_lock as fn() -> i64),
            ] {
                let start = std::time::Instant::now();
                std::thread::scope(|s| {
                    for _ in 0..threads {
                        s.spawn(|| {
                            for _ in 0..CALLS {
                                std::hint::black_box(f());
                            }
                        });
                    }
                });
                let ns = start.elapsed().as_nanos() as f64 / CALLS as f64;
                println!("{threads:>2} threads, {name:<9}: {ns:.1} ns per call (wall, per thread)");
            }
        }
        start_clock(1.0);
    }
}
