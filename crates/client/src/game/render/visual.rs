//! Hooks on the game's view getters: field of view, gamma, and optional mouse
//! sensitivity while zooming. See analysis/notes/visual.md and analysis/notes/actors.md.
//!
//! These only change a value the game is reading; nothing is written to the
//! game's memory apart from the hooks themselves.

use crate::signatures::{
    gamma_signature, sensitivity_signature, FOV_SIGNATURE, GAMMA_GETTER_SHAPE, GAMMA_ID_AT,
    GAMMA_NAME_ANCHOR, SENSITIVITY_GETTER_SHAPE, SENSITIVITY_ID_AT, SENSITIVITY_NAME,
    SENSITIVITY_NAME_ANCHOR,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use hudhook::mh::{MH_CreateHook, MH_DisableHook, MH_EnableHook, MH_Initialize, MH_STATUS};
use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::Sleep;

use crate::game::layout::image;
use crate::game::patch;

// --- Gamma option resolution ---------------------------------------------------

/// How far after the anchor the id is looked for.
const GAMMA_ID_WINDOW: usize = 0x200;

use crate::compatibility::option_id_after;

fn find_gamma() -> Result<usize, String> {
    let anchor = patch::find(GAMMA_NAME_ANCHOR).map_err(|e| format!("gamma option name: {e}"))?;
    // The anchor is a match inside the mapped image's code, so the bytes after
    // it are readable up to the image's end. (`memory::readable` is for data:
    // it rejects execute-read pages.)
    let (_, image_end) = image();
    let window = GAMMA_ID_WINDOW.min(image_end.saturating_sub(anchor));
    let code = unsafe { std::slice::from_raw_parts(anchor as *const u8, window) };
    let id = option_id_after(code).ok_or("gamma option id not found")?;
    patch::find_filled(
        GAMMA_GETTER_SHAPE,
        GAMMA_ID_AT,
        &id.to_le_bytes(),
        &gamma_signature(id),
    )
    .map_err(|e| format!("gamma getter (option 0x{id:X}): {e}"))
}

fn find_sensitivity() -> Result<usize, String> {
    let (base, end) = image();
    let mut options = Vec::new();
    for anchor in patch::find_all(SENSITIVITY_NAME_ANCHOR, 256)? {
        if anchor + 17 > end {
            continue;
        }
        let displacement = unsafe { ((anchor + 3) as *const i32).read_unaligned() };
        let Some(name) = (anchor + 7).checked_add_signed(displacement as isize) else {
            continue;
        };
        if name < base || name + 16 > end {
            continue;
        }
        let text = unsafe { std::slice::from_raw_parts(name as *const u8, 16) };
        // The anchor constructs the last character ('2') as an immediate.
        if text != &SENSITIVITY_NAME[..16] {
            continue;
        }
        let code =
            unsafe { std::slice::from_raw_parts(anchor as *const u8, 0x200.min(end - anchor)) };
        if let Some(id) = option_id_after(code) {
            options.push(id);
        }
    }
    if options.len() != 1 {
        return Err(format!("sensitivity name: {} registrations", options.len()));
    }
    patch::find_filled(
        SENSITIVITY_GETTER_SHAPE,
        SENSITIVITY_ID_AT,
        &options[0].to_le_bytes(),
        &sensitivity_signature(options[0]),
    )
    .map_err(|e| format!("sensitivity getter (option 0x{:X}): {e}", options[0]))
}

// --- Hook plumbing -----------------------------------------------------------------

/// One MinHook hook that can be switched on and off.
struct Hook {
    /// The hooked function, once resolved.
    target: OnceLock<Result<usize, String>>,
    /// Trampoline to the original; non-zero once the hook was created.
    original: AtomicUsize,
    enabled: AtomicBool,
    /// Threads currently inside the detour; disabling waits for it to drain.
    in_flight: AtomicUsize,
    /// Serializes enable/disable.
    switching: Mutex<()>,
}

impl Hook {
    const fn new() -> Self {
        Self {
            target: OnceLock::new(),
            original: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            switching: Mutex::new(()),
        }
    }

    fn target(&self, find: fn() -> Result<usize, String>) -> Result<usize, String> {
        self.target.get_or_init(find).clone()
    }

    fn set(
        &self,
        on: bool,
        find: fn() -> Result<usize, String>,
        detour: *mut c_void,
    ) -> Result<(), String> {
        let _guard = self.switching.lock().unwrap_or_else(|e| e.into_inner());
        if on == self.enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        let target = self.target(find)? as *mut c_void;
        unsafe {
            if on {
                if self.original.load(Ordering::Acquire) == 0 {
                    match MH_Initialize() {
                        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
                        e => return Err(format!("MH_Initialize: {e:?}")),
                    }
                    let mut trampoline = std::ptr::null_mut();
                    MH_CreateHook(target, detour, &mut trampoline)
                        .ok()
                        .map_err(|e| format!("MH_CreateHook: {e:?}"))?;
                    self.original.store(trampoline as usize, Ordering::Release);
                }
                MH_EnableHook(target)
                    .ok()
                    .map_err(|e| format!("MH_EnableHook: {e:?}"))?;
            } else {
                match MH_DisableHook(target) {
                    MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_DISABLED => {}
                    e => return Err(format!("MH_DisableHook: {e:?}")),
                }
            }
        }
        self.enabled.store(on, Ordering::Release);
        Ok(())
    }

    /// True once no thread is inside the detour (waits briefly for that).
    fn drained(&self) -> bool {
        for _ in 0..100 {
            if self.in_flight.load(Ordering::SeqCst) == 0 {
                return true;
            }
            unsafe { Sleep(1) };
        }
        false
    }
}

/// Counts a detour call for as long as it lives.
struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn enter(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn load_f32(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

fn store_f32(cell: &AtomicU32, value: f32) {
    cell.store(value.to_bits(), Ordering::Relaxed);
}

// --- Zoom ----------------------------------------------------------------------------

/// An immutable curve between input/configuration changes. Readers evaluate
/// elapsed time from one origin; they never write a partially advanced curve.
#[derive(Clone, Copy)]
struct ZoomState {
    from: f32,
    target: f32,
    started_us: u64,
    tau_ms: f32,
    factor: f32,
    held: bool,
}
impl ZoomState {
    const fn new() -> Self {
        Self {
            from: 1.,
            target: 1.,
            started_us: 0,
            tau_ms: 200. / 3.,
            factor: 4.,
            held: false,
        }
    }
    fn value(&self, now: u64) -> f32 {
        ease(
            self.from,
            self.target,
            now.saturating_sub(self.started_us) as f32 / 1000.,
            self.tau_ms,
        )
    }
    fn retarget(&mut self, target: f32, now: u64) {
        self.from = self.value(now);
        self.target = target;
        self.started_us = now;
    }
    fn hold(&mut self, held: bool, now: u64) {
        if held != self.held {
            self.retarget(if held { self.factor } else { 1. }, now);
            self.held = held;
        }
    }
    fn factor(&mut self, factor: f32, now: u64) {
        if factor != self.factor {
            self.factor = factor;
            if self.held {
                self.retarget(factor, now);
            }
        }
    }
    fn smoothing(&mut self, tau_ms: f32, now: u64) {
        self.retarget(self.target, now);
        self.tau_ms = tau_ms;
    }
    fn reset(&mut self, now: u64) {
        self.from = 1.;
        self.target = 1.;
        self.started_us = now;
        self.held = false;
    }
}
static ZOOM: Mutex<ZoomState> = Mutex::new(ZoomState::new());
/// The narrowest field of view zoom may produce (degrees).
const MIN_FOV: f32 = 1.0;

type FovFn = unsafe extern "system" fn(*mut c_void, f32, u8) -> f32;

static FOV: Hook = Hook::new();
/// Zoom is wanted right now (the hold key is down).
static ZOOMING: AtomicBool = AtomicBool::new(false);
static ZOOM_HOLD_KEY: AtomicU32 = AtomicU32::new(0x43); // C
static ZOOM_INPUT_AT: AtomicU64 = AtomicU64::new(0);
const INPUT_MAX_AGE_MS: u64 = 250;
/// Diagnostics: calls seen, and the last field of view the game computed.
static FOV_CALLS: AtomicU64 = AtomicU64::new(0);
static FOV_LAST: AtomicU32 = AtomicU32::new(0);
static FOV_RETURNED: AtomicU32 = AtomicU32::new(0);
static FOV_AT: AtomicU64 = AtomicU64::new(0);
static ZOOM_APPLIED: AtomicU32 = AtomicU32::new(0x3F80_0000);

/// Fine-grained monotonic time from Timer's unscaled counter. Wall-clock
/// synchronization must not restart/freeze a zoom transition.
fn precise_us() -> u64 {
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    static LAST: AtomicU64 = AtomicU64::new(0);
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut value = 0;
        unsafe {
            QueryPerformanceFrequency(&mut value);
        }
        value.max(0) as u64
    });
    // On a transient counter-source change, retain the QPC timeline instead
    // of mixing GetTickCount64's different epoch into the curve.
    let now = crate::modules::timer::profile_counter()
        .or_else(crate::modules::timer::profile_counter)
        .filter(|(ticks, _)| *ticks >= 0 && frequency != 0)
        .map(|(ticks, _)| (ticks as u128 * 1_000_000 / frequency as u128) as u64)
        .unwrap_or_else(|| LAST.load(Ordering::Relaxed));
    LAST.fetch_max(now, Ordering::Relaxed).max(now)
}

/// Moves the magnification `current` toward `target`, covering most of the
/// gap in a few time constants `tau_ms` however often it is called. Eases
/// the logarithm of the magnification, so the view closes in at an even
/// pace (4x is as far from 2x as 2x is from 1x).
fn ease(current: f32, target: f32, elapsed_ms: f32, tau_ms: f32) -> f32 {
    // NaN fails every `is_finite`/`>` test here, so broken input jumps to the target.
    let usable = current.is_finite() && current > 0. && tau_ms.is_finite() && tau_ms > 0.;
    if !usable || !target.is_finite() {
        return target;
    }
    let blend = 1.0 - (-elapsed_ms.max(0.0) / tau_ms).exp();
    let (from, to) = (current.ln(), target.max(f32::MIN_POSITIVE).ln());
    let next = (from + (to - from) * blend).exp();
    // Land exactly, so an idle zoom costs nothing.
    if (next - target).abs() < 0.001 * target.max(1.) {
        target
    } else {
        next
    }
}

/// How far the eased zoom is toward `factor`: 0 (none) to 1 (fully in).
fn progress(current: f32, factor: f32) -> f32 {
    let zoomed = factor.is_finite() && factor > 1. && current.is_finite() && current > 1.;
    if !zoomed {
        return 0.;
    }
    (current.ln() / factor.ln()).clamp(0., 1.)
}

/// The field of view to report for the game's `fov` at magnification `factor`.
fn zoomed(fov: f32, factor: f32) -> f32 {
    if factor.is_nan() || factor <= 1.0 || !fov.is_finite() {
        return fov;
    }
    (fov / factor).max(MIN_FOV.min(fov))
}

unsafe extern "system" fn fov_detour(this: *mut c_void, partial: f32, apply_effects: u8) -> f32 {
    let _in_flight = InFlight::enter(&FOV.in_flight);
    let original: FovFn = std::mem::transmute(FOV.original.load(Ordering::Acquire));
    let fov = original(this, partial, apply_effects);
    // Without effects this is the fixed FOV of the held item; leave it alone.
    if apply_effects == 0 {
        return fov;
    }
    FOV_CALLS.fetch_add(1, Ordering::Relaxed);
    store_f32(&FOV_LAST, fov);
    let fov = overridden(fov, load_f32(&FOV_OVERRIDE));

    let factor = zoom_values().0;
    let returned = zoomed(fov, factor);
    store_f32(&ZOOM_APPLIED, factor);
    store_f32(&FOV_RETURNED, returned);
    FOV_AT.store(GetTickCount64(), Ordering::Relaxed);
    returned
}

fn live_hold(requested: bool, key_down: bool, input_age_ms: u64) -> bool {
    requested && key_down && input_age_ms <= INPUT_MAX_AGE_MS
}
/// A release is visible even if the worker is delayed behind menu/render work.
/// Only the worker can request activation; callbacks may revoke a stale hold.
fn zoom_values() -> (f32, f32) {
    let requested = ZOOMING.load(Ordering::Acquire);
    let held = live_hold(
        requested,
        requested
            && !crate::gui::captures_input()
            && crate::input::is_foreground()
            && crate::input::key_down(ZOOM_HOLD_KEY.load(Ordering::Relaxed) as u16),
        unsafe { GetTickCount64() }.saturating_sub(ZOOM_INPUT_AT.load(Ordering::Acquire)),
    );
    let now = precise_us();
    let state = {
        let mut state = ZOOM.lock().unwrap_or_else(|e| e.into_inner());
        state.hold(held && ZOOMING.load(Ordering::Acquire), now);
        *state
    };
    (state.value(now), state.factor)
}

/// Whether zoom can work in this game build.
pub fn zoom_available() -> Result<(), String> {
    FOV.target(|| patch::find(FOV_SIGNATURE).map_err(|e| format!("getFov: {e}")))
        .map(|_| ())
}

/// Features currently using the field-of-view hook (zoom, custom FOV).
static FOV_USERS: Mutex<usize> = Mutex::new(0);

/// Adds or removes one user of the field-of-view hook; the hook is installed
/// while there is at least one.
fn fov_hook(on: bool) -> Result<(), String> {
    let mut users = FOV_USERS.lock().unwrap_or_else(|e| e.into_inner());
    let (first, last) = (on && *users == 0, !on && *users == 1);
    if first || last {
        FOV.set(
            on,
            || patch::find(FOV_SIGNATURE).map_err(|e| format!("getFov: {e}")),
            fov_detour as *mut c_void,
        )?;
    }
    if on {
        *users += 1;
    } else {
        *users = users.saturating_sub(1);
    }
    Ok(())
}

/// Starts or stops zoom's use of the field-of-view hook.
pub fn set_zoom_hook(on: bool) -> Result<(), String> {
    if !on {
        ZOOMING.store(false, Ordering::Relaxed);
    }
    fov_hook(on)?;
    if !on {
        ZOOM.lock()
            .unwrap_or_else(|e| e.into_inner())
            .reset(precise_us());
    }
    Ok(())
}

// --- Custom field of view ------------------------------------------------------------

/// The range a custom field of view may take (degrees).
pub const FOV_RANGE: (f32, f32) = (30.0, 170.0);
/// f32 bits: the field of view to report instead of the game's; 0 = off.
static FOV_OVERRIDE: AtomicU32 = AtomicU32::new(0);

/// The field of view to use given the game's own and the override (0 = none).
fn overridden(fov: f32, custom: f32) -> f32 {
    if custom.is_finite() && custom > 0.0 && fov.is_finite() {
        custom.clamp(FOV_RANGE.0, FOV_RANGE.1)
    } else {
        fov
    }
}

/// Starts or stops the custom field of view. While on, the world is drawn with
/// `degrees` instead of the game's value (so sprint/speed FOV effects are gone);
/// zoom still applies on top.
pub fn set_fov_hook(on: bool, degrees: f32) -> Result<(), String> {
    if on {
        fov_hook(true)?;
        store_f32(&FOV_OVERRIDE, degrees);
    } else {
        store_f32(&FOV_OVERRIDE, 0.0);
        fov_hook(false)?;
    }
    Ok(())
}

/// Changes the custom field of view while it is on.
pub fn set_fov_override(degrees: f32) {
    if load_f32(&FOV_OVERRIDE) != 0.0 {
        store_f32(&FOV_OVERRIDE, degrees);
    }
}

/// Zooms in while `on` (eased); needs the hook installed.
pub fn set_zooming(on: bool) {
    ZOOM_INPUT_AT.store(unsafe { GetTickCount64() }, Ordering::Release);
    if ZOOMING.swap(on, Ordering::AcqRel) != on {
        ZOOM.lock()
            .unwrap_or_else(|e| e.into_inner())
            .hold(on, precise_us());
    }
}
pub fn set_zoom_hold_key(key: u16) {
    if ZOOM_HOLD_KEY.swap(u32::from(key), Ordering::AcqRel) != u32::from(key) {
        set_zooming(false);
    }
}

/// How long a zoom in or out takes, in ms (0 = instant).
pub fn set_zoom_smoothing(duration_ms: f32) {
    let tau = if duration_ms.is_finite() {
        duration_ms.clamp(0., 1000.) / 3.
    } else {
        200. / 3.
    };
    ZOOM.lock()
        .unwrap_or_else(|e| e.into_inner())
        .smoothing(tau, precise_us());
}

/// Whether zoom is wanted right now (the hold key is down while playing).
pub fn zooming() -> bool {
    ZOOM.lock().unwrap_or_else(|e| e.into_inner()).held
}

pub fn set_zoom_factor(factor: f32) {
    let factor = if factor.is_finite() {
        factor.clamp(1.5, 30.)
    } else {
        4.
    };
    ZOOM.lock()
        .unwrap_or_else(|e| e.into_inner())
        .factor(factor, precise_us());
}

/// (calls seen, the game's last field of view, current magnification).
pub fn zoom_stats() -> (u64, f32, f32) {
    (
        FOV_CALLS.load(Ordering::Relaxed),
        load_f32(&FOV_LAST),
        zoom_values().0,
    )
}

/// Separate the curve's computed value from what the game last received.
/// A diagnostic read must not be mistaken for a real getFov invocation.
pub fn diagnostic() -> String {
    let current = zoom_values().0;
    let state = *ZOOM.lock().unwrap_or_else(|e| e.into_inner());
    let now = unsafe { GetTickCount64() };
    let key = ZOOM_HOLD_KEY.load(Ordering::Relaxed) as u16;
    format!("Zoom key={} down={} requested={} held={} current={current:.6} target={:.3} from={:.3} smoothing_ms={:.0}\nworker_age_ms={} getFov_calls={} getFov_age_ms={} original_fov={:.4} returned_fov={:.4} applied_zoom={:.6}\ngetGamma_calls={} original_gamma={:.4}",
        crate::input::key_name(key), crate::input::key_down(key), ZOOMING.load(Ordering::Acquire), state.held,
        state.target, state.from, state.tau_ms*3., now.saturating_sub(ZOOM_INPUT_AT.load(Ordering::Acquire)),
        FOV_CALLS.load(Ordering::Relaxed), now.saturating_sub(FOV_AT.load(Ordering::Relaxed)),
        load_f32(&FOV_LAST), load_f32(&FOV_RETURNED), load_f32(&ZOOM_APPLIED),
        GAMMA_CALLS.load(Ordering::Relaxed), load_f32(&GAMMA_LAST))
}

// --- Optional zoom sensitivity --------------------------------------------------------

type SensitivityFn = unsafe extern "system" fn(*mut c_void, u32) -> f32;
static SENSITIVITY: Hook = Hook::new();
static SENSITIVITY_SCALE: AtomicU32 = AtomicU32::new(0x3E80_0000); // 0.25

fn scaled_sensitivity(original: f32, mode: u32, zooming: bool, scale: f32) -> f32 {
    // Mouse = 1; touch/controller settings pass through unchanged. This scales
    // the option value, not a claimed exact screen-space/angular turn rate.
    if zooming
        && mode == 1
        && original.is_finite()
        && original >= 0.
        && scale.is_finite()
        && (0.01..=1.).contains(&scale)
    {
        original * scale
    } else {
        original
    }
}

unsafe extern "system" fn sensitivity_detour(options: *mut c_void, mode: u32) -> f32 {
    let _in_flight = InFlight::enter(&SENSITIVITY.in_flight);
    let original: SensitivityFn = std::mem::transmute(SENSITIVITY.original.load(Ordering::Acquire));
    // Follow the eased zoom, so the sensitivity changes with the view
    // instead of jumping when the key is pressed.
    let (current, factor) = zoom_values();
    let done = progress(current, factor);
    let scale = 1. + (load_f32(&SENSITIVITY_SCALE) - 1.) * done;
    scaled_sensitivity(original(options, mode), mode, done > 0., scale)
}

pub fn set_zoom_sensitivity(on: bool) -> Result<(), String> {
    SENSITIVITY.set(on, find_sensitivity, sensitivity_detour as *mut c_void)
}

pub fn set_sensitivity_scale(value: f32) {
    store_f32(&SENSITIVITY_SCALE, value.clamp(0.01, 1.));
}

// --- Fullbright ------------------------------------------------------------------------

type GammaFn = unsafe extern "system" fn(*mut c_void) -> f32;

static GAMMA: Hook = Hook::new();
/// f32 bits: the gamma to report while the hook is on.
static GAMMA_VALUE: AtomicU32 = AtomicU32::new(0x4120_0000); // 10.0
/// Diagnostics: calls seen, and the last gamma the game itself returned.
static GAMMA_CALLS: AtomicU64 = AtomicU64::new(0);
static GAMMA_LAST: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn gamma_detour(options: *mut c_void) -> f32 {
    let _in_flight = InFlight::enter(&GAMMA.in_flight);
    let original: GammaFn = std::mem::transmute(GAMMA.original.load(Ordering::Acquire));
    let gamma = original(options);
    GAMMA_CALLS.fetch_add(1, Ordering::Relaxed);
    store_f32(&GAMMA_LAST, gamma);
    load_f32(&GAMMA_VALUE)
}

/// Whether fullbright can work in this game build.
pub fn gamma_available() -> Result<(), String> {
    GAMMA.target(find_gamma).map(|_| ())
}

/// Installs or removes the gamma hook.
pub fn set_gamma_hook(on: bool) -> Result<(), String> {
    GAMMA.set(on, find_gamma, gamma_detour as *mut c_void)
}

pub fn set_gamma(value: f32) {
    store_f32(&GAMMA_VALUE, value);
}

/// (calls seen, the game's own last gamma).
pub fn gamma_stats() -> (u64, f32) {
    (GAMMA_CALLS.load(Ordering::Relaxed), load_f32(&GAMMA_LAST))
}

/// Removes all view hooks. Returns false if a thread is still inside a detour, in
/// which case the DLL must stay loaded.
pub fn shutdown() -> bool {
    // Whatever the user count says, the hook itself must go.
    ZOOMING.store(false, Ordering::Relaxed);
    store_f32(&FOV_OVERRIDE, 0.0);
    ZOOM.lock()
        .unwrap_or_else(|e| e.into_inner())
        .reset(precise_us());
    let fov_off = {
        let mut users = FOV_USERS.lock().unwrap_or_else(|e| e.into_inner());
        *users = 0;
        FOV.set(
            false,
            || patch::find(FOV_SIGNATURE).map_err(|e| format!("getFov: {e}")),
            fov_detour as *mut c_void,
        )
        .is_ok()
    };
    let disabled = fov_off & set_gamma_hook(false).is_ok() & set_zoom_sensitivity(false).is_ok();
    disabled && FOV.drained() && GAMMA.drained() && SENSITIVITY.drained()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_completes_without_getfov_or_sensitivity_callbacks() {
        let mut state = ZoomState::new();
        state.hold(true, 10_000);
        assert!((state.value(610_000) - 4.).abs() < 0.01);
        state.hold(false, 610_000);
        // No callback advances a shared "current" value in this interval.
        assert_eq!(state.value(1_410_000), 1.);
        assert_eq!(state.value(10_000_000), 1.);
        assert_eq!(zoomed(70., state.value(1_410_000)), 70.);
    }

    #[test]
    fn zoom_readers_do_not_rewrite_the_transition_at_any_call_rate() {
        let mut state = ZoomState::new();
        state.hold(true, 0);
        state.hold(false, 600_000);
        let expected = state.value(1_400_000);
        std::thread::scope(|scope| {
            for offset in [0, 1, 100, 1000] {
                let state = &state;
                scope.spawn(move || {
                    for n in 0..20_000 {
                        std::hint::black_box(state.value(600_000 + n * 40 + offset));
                    }
                    assert_eq!(state.value(1_400_000), 1.);
                });
            }
        });
        assert_eq!(state.started_us, 600_000);
        assert_eq!(state.value(1_400_000), expected);
        assert_eq!(expected, 1.);
    }

    #[test]
    fn a_delayed_reader_cannot_overwrite_a_newer_release() {
        let state = std::sync::Arc::new(Mutex::new(ZoomState::new()));
        state.lock().unwrap().hold(true, 0);
        let ready = std::sync::Barrier::new(2);
        let released = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let state = &state;
            let ready = &ready;
            let released = &released;
            scope.spawn(move || {
                let old = *state.lock().unwrap();
                ready.wait();
                released.wait();
                // This reader finishes late, after the worker released zoom.
                assert!((old.value(600_000) - 4.).abs() < 0.01);
            });
            ready.wait();
            state.lock().unwrap().hold(false, 600_000);
            released.wait();
        });
        assert_eq!(state.lock().unwrap().value(1_400_000), 1.);
        assert!(!state.lock().unwrap().held);
    }

    #[test]
    fn rapid_repress_configuration_changes_and_disable_are_continuous() {
        let mut state = ZoomState::new();
        state.hold(true, 0);
        let before = state.value(50_000);
        state.hold(false, 50_000);
        assert_eq!(state.value(50_000), before);
        let before = state.value(70_000);
        state.hold(true, 70_000);
        assert_eq!(state.value(70_000), before);
        state.factor(8., 70_000);
        assert_eq!(state.value(70_000), before);
        state.smoothing(0., 70_000);
        assert_eq!(state.value(70_000), 8.);
        state.hold(false, 80_000);
        assert_eq!(state.value(80_000), 1.);
        state.hold(true, 90_000);
        state.reset(90_000);
        assert_eq!(state.value(90_000), 1.);
        assert!(!state.held);
    }

    #[test]
    fn released_key_and_expired_worker_input_revoke_a_hold() {
        assert!(live_hold(true, true, 0));
        assert!(live_hold(true, true, INPUT_MAX_AGE_MS));
        assert!(!live_hold(true, true, INPUT_MAX_AGE_MS + 1));
        assert!(!live_hold(true, false, 0));
        assert!(!live_hold(false, true, 0));
    }

    #[test]
    fn zoom_sensitivity_only_changes_the_mouse_option_while_held() {
        assert_eq!(scaled_sensitivity(0.5, 1, true, 0.25), 0.125);
        for (mode, held, scale) in [
            (1, false, 0.25),
            (2, true, 0.25),
            (3, true, 0.25),
            (1, true, f32::NAN),
            (1, true, 2.),
        ] {
            assert_eq!(scaled_sensitivity(0.5, mode, held, scale), 0.5);
        }
        assert_eq!(scaled_sensitivity(-1., 1, true, 0.25), -1.);
        assert!(sensitivity_signature(0x183)
            .parse::<sigscan::Pattern>()
            .is_ok());
        assert!(SENSITIVITY_NAME_ANCHOR.parse::<sigscan::Pattern>().is_ok());
    }

    #[test]
    fn zoom_divides_the_field_of_view_and_leaves_it_alone_at_1x() {
        assert_eq!(zoomed(70.0, 1.0), 70.0);
        assert_eq!(zoomed(70.0, 4.0), 17.5);
        assert_eq!(zoomed(110.0, 2.0), 55.0);
        // Never below the floor, never wider than the game asked for.
        assert_eq!(zoomed(70.0, 1000.0), MIN_FOV);
        assert_eq!(zoomed(0.5, 4.0), 0.5);
        // Nonsense factors do nothing.
        assert_eq!(zoomed(70.0, 0.5), 70.0);
        assert_eq!(zoomed(70.0, f32::NAN), 70.0);
        assert!(zoomed(f32::NAN, 4.0).is_nan());
    }

    #[test]
    fn custom_fov_replaces_the_games_value_within_its_range_and_zoom_stacks() {
        // Off: the game's value passes through.
        assert_eq!(overridden(110.0, 0.0), 110.0);
        assert_eq!(overridden(110.0, f32::NAN), 110.0);
        assert_eq!(overridden(110.0, -5.0), 110.0);
        // On: the custom value wins, clamped to the supported range.
        assert_eq!(overridden(110.0, 140.0), 140.0);
        assert_eq!(overridden(121.0, 60.0), 60.0);
        assert_eq!(overridden(70.0, 1.0), FOV_RANGE.0);
        assert_eq!(overridden(70.0, 500.0), FOV_RANGE.1);
        assert!(overridden(f32::NAN, 90.0).is_nan());
        // Zoom divides whatever field of view is in effect.
        assert_eq!(zoomed(overridden(110.0, 140.0), 4.0), 35.0);
        // Outside the game the hook cannot be installed, and nothing is left on.
        assert!(set_fov_hook(true, 120.0).is_err());
        assert_eq!(load_f32(&FOV_OVERRIDE), 0.0);
        assert_eq!(*FOV_USERS.lock().unwrap(), 0);
        set_fov_override(120.0);
        assert_eq!(load_f32(&FOV_OVERRIDE), 0.0);
        assert!(set_fov_hook(false, 0.0).is_ok());
    }

    #[test]
    fn easing_converges_independently_of_call_rate() {
        let tau = 200. / 3.;
        // One 200 ms step and forty 5 ms steps end in the same place.
        let coarse = ease(1.0, 4.0, 200.0, tau);
        let fine = (0..40).fold(1.0, |current, _| ease(current, 4.0, 5.0, tau));
        assert!((coarse - fine).abs() < 0.01, "{coarse} vs {fine}");
        assert!(coarse > 3.5 && coarse < 4.0, "{coarse}");
        // It lands exactly and then stays.
        let landed = (0..200).fold(1.0, |current, _| ease(current, 4.0, 16.0, tau));
        assert_eq!(landed, 4.0);
        assert_eq!(ease(4.0, 4.0, 16.0, tau), 4.0);
        // Out again, back to exactly 1.
        let out = (0..200).fold(4.0, |current, _| ease(current, 1.0, 16.0, tau));
        assert_eq!(out, 1.0);
        // No time passed: no movement. Broken state or no smoothing: jump.
        assert_eq!(ease(2.0, 4.0, 0.0, tau), 2.0);
        assert_eq!(ease(f32::NAN, 4.0, 16.0, tau), 4.0);
        assert_eq!(ease(1.0, 4.0, 1.0, 0.0), 4.0);
    }

    #[test]
    fn zoom_eases_at_an_even_pace_and_sensitivity_follows_it() {
        let tau = 50.;
        // Halfway in time constants, the logarithm is halfway: 1x -> 4x
        // passes 2x (not 2.5x, as easing the magnification itself would).
        let half = (0.5f32).ln().abs() * tau; // elapsed where blend = 0.5
        let mid = ease(1.0, 4.0, half, tau);
        assert!((mid - 2.0).abs() < 0.01, "{mid}");
        assert_eq!(progress(1.0, 4.0), 0.);
        assert!((progress(2.0, 4.0) - 0.5).abs() < 1e-5);
        assert_eq!(progress(4.0, 4.0), 1.);
        assert_eq!(progress(8.0, 4.0), 1.);
        assert_eq!(progress(2.0, 1.0), 0.);
    }

    /// With the real clock, stepped as the hook steps it (several calls a
    /// frame): held -> in, released -> back to exactly 1x. Local state: the
    /// shared zoom statics are also written by other tests' module ticks.
    #[test]
    fn zoom_returns_to_1x_after_release_in_real_time() {
        let mut state = ZoomState::new();
        state.hold(true, precise_us());
        let run = |state: &ZoomState, ms: u64| {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(ms) {
                for _ in 0..3 {
                    std::hint::black_box(state.value(precise_us()));
                }
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            state.value(precise_us())
        };
        let held = run(&state, 600);
        assert!((held - 4.0).abs() < 0.01, "held: {held}");
        state.hold(false, precise_us());
        assert_eq!(run(&state, 800), 1.0, "released");
    }

    #[test]
    fn the_precise_clock_moves_finer_than_the_tick_count() {
        let mut before = precise_us();
        let mut smallest = u64::MAX;
        for _ in 0..10_000 {
            let next = precise_us();
            assert!(next >= before);
            if next > before {
                smallest = smallest.min(next - before);
            }
            before = next;
        }
        // Check resolution across many readings, not one scheduler-dependent gap.
        assert!(smallest < 5_000, "minimum clock step: {smallest}");
    }

    /// The registration code of the analyzed build (1.26.6301.0), from the
    /// `movabs rax, "gfx_gamm"` anchor's neighbourhood down to the call.
    #[test]
    fn option_id_is_read_from_the_registration_code() {
        let code = [
            0x64, 0x24, 0x20, 0xC7, 0x44, 0x24, 0x30, 0x02, 0x00, 0x00,
            0x00, // mov [rsp+30], 2
            0x48, 0x8B, 0x8D, 0x40, 0x02, 0x00, 0x00, // mov rcx, [rbp+240]
            0xBA, 0x32, 0x00, 0x00, 0x00, // mov edx, 32
            0x41, 0xB8, 0x01, 0x00, 0x00, 0x00, // mov r8d, 1
            0x41, 0xB9, 0x10, 0x00, 0x00, 0x00, // mov r9d, 10
            0xE8, 0xA8, 0x6B, 0xE9, 0xFA, // call
        ];
        assert_eq!(option_id_after(&code), Some(0x32));
        // The FOV option's registration uses `xor r8d, r8d` in between.
        let fov = [
            0x48, 0x89, 0xD9, // mov rcx, rbx
            0xBA, 0x2F, 0x00, 0x00, 0x00, // mov edx, 2F
            0x45, 0x31, 0xC0, // xor r8d, r8d
            0x41, 0xB9, 0x10, 0x00, 0x00, 0x00, // mov r9d, 10
            0xE8, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(option_id_after(&fov), Some(0x2F));
        // A `mov edx` that is not followed by the constructor call is skipped.
        let other = [
            0xBA, 0x20, 0x00, 0x00, 0x00, 0xFF, 0x15, 0, 0, 0, 0, 0x90, 0x90, 0x90,
        ];
        assert_eq!(option_id_after(&other), None);
        assert_eq!(option_id_after(&[]), None);
    }

    #[test]
    fn gamma_signature_embeds_the_id_and_parses() {
        let signature = gamma_signature(0x32);
        assert!(signature.contains("41 B8 32 00 00 00 FF 15"), "{signature}");
        assert!(signature.parse::<sigscan::Pattern>().is_ok());
        assert!(gamma_signature(0x1A3).contains("41 B8 A3 01 00 00 FF 15"));
        assert!(FOV_SIGNATURE.parse::<sigscan::Pattern>().is_ok());
        assert!(GAMMA_NAME_ANCHOR.parse::<sigscan::Pattern>().is_ok());
    }
}
