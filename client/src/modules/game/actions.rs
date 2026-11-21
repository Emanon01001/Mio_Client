//! Native actions for the analyzed builds (`layout::BUILDS`). Called only by the
//! native input callbacks and input tick; no saved entity/component is dereferenced.
use crate::signatures::{
    ATTACK_DOWN_SIGNATURE, ATTACK_UP_SIGNATURE, INPUT_TICK_SIGNATURE, MOUSE_FEED_SIGNATURE,
    MOVE_ALLOWED_CALL_SIGNATURE, MOVE_ALLOWED_SIGNATURE, MOVE_SIGNATURE, SPRINT_TOGGLE_SIGNATURE,
    TRACK_DOWN_SIGNATURE, TRACK_UP_SIGNATURE, USE_ALLOWED_SIGNATURE,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::entities::{actor_flags, actor_jump_state, actor_on_ground};
use super::mouse::{mouse_feed_hook, BLOCKED_MOTION, MOUSE_FEED};
use super::{actions_build, inventory, patch, read, readable};
use crate::{gui, input};
use hudhook::mh::{MH_CreateHook, MH_EnableHook, MH_Initialize, MH_STATUS};
use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::Sleep;

type MoveFn = unsafe extern "system" fn(usize) -> usize;

// Stateless std::function invokers: RCX and RDX are unused; R8 is ClientInstance.
type AttackFn = unsafe extern "system" fn(*const c_void, *const u8, usize);
struct Native {
    movement: MoveFn,
}
type InputTickFn = unsafe extern "system" fn(usize);
static DOWN: AtomicUsize = AtomicUsize::new(0);
static UP: AtomicUsize = AtomicUsize::new(0);
static INPUT_TICK: AtomicUsize = AtomicUsize::new(0);
static TRACK_DOWN: AtomicUsize = AtomicUsize::new(0);
static TRACK_UP: AtomicUsize = AtomicUsize::new(0);
static SPRINT_TOGGLE: AtomicUsize = AtomicUsize::new(0);
static LOCAL_FLAGS: AtomicUsize = AtomicUsize::new(0);
static SPRINTING: AtomicBool = AtomicBool::new(false);
static TARGETS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
pub(super) static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicBool = AtomicBool::new(false);
static PRESSES: AtomicU64 = AtomicU64::new(0);
static BLOCKED_PRESSES: AtomicU64 = AtomicU64::new(0);
static ATTACKS: AtomicU64 = AtomicU64::new(0);
static SPRINT_WRITES: AtomicU64 = AtomicU64::new(0);
static JUMP_WRITES: AtomicU64 = AtomicU64::new(0);
static INVENTORY_WRITES: AtomicU64 = AtomicU64::new(0);
static INVENTORY_ACCEPTED: AtomicU64 = AtomicU64::new(0);
static INVENTORY_REJECTED: AtomicU64 = AtomicU64::new(0);
static MOVE_ALLOWED: AtomicUsize = AtomicUsize::new(0);
static USE_ALLOWED: AtomicUsize = AtomicUsize::new(0);
static USE_ALLOWED_TARGET: AtomicUsize = AtomicUsize::new(0);
static INVENTORY_USES: AtomicU64 = AtomicU64::new(0);
static INVENTORY_USE_OVERRIDES: AtomicU64 = AtomicU64::new(0);
thread_local! {
    // Scoped to a synchronous AutoFish use call, never to native input sampling
    // as a whole. Other threads and UI clicks retain the vanilla permission.
    static INVENTORY_USE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn with_use_scope<T>(instance: usize, f: impl FnOnce() -> T) -> T {
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            INVENTORY_USE.set(self.0);
        }
    }
    let _restore = Restore(INVENTORY_USE.replace(instance));
    f()
}
unsafe extern "system" fn use_allowed_hook(instance: usize) -> u8 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: MoveAllowedFn = std::mem::transmute(USE_ALLOWED.load(Ordering::Acquire));
    let result = original(instance);
    let allowed = if gui::captures_input() || super::freecam::blocking() {
        0
    } else {
        use_permission(result, instance)
    };
    if allowed != result {
        INVENTORY_USE_OVERRIDES.fetch_add(1, Ordering::Relaxed);
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    allowed
}
fn use_permission(result: u8, instance: usize) -> u8 {
    if result == 0 && instance != 0 && INVENTORY_USE.get() == instance {
        1
    } else {
        result
    }
}
/// Only AutoFish may borrow the native use permission while in its own inventory.
/// Button-up still runs without an exception on pause/off to release owned input.
pub(super) unsafe fn fishing_use(instance: usize, down: bool) {
    let own_inventory = inventory::own_inventory_for(instance, GetTickCount64())
        && !gui::DORMANT.load(Ordering::Acquire)
        && !gui::MENU_OPEN.load(Ordering::Acquire)
        && !crate::api::blocked()
        && input::is_foreground();
    let verified = own_inventory
        && read::<usize>(read::<usize>(instance) + 0x4d8)
            == USE_ALLOWED_TARGET.load(Ordering::Acquire);
    if verified && down {
        INVENTORY_USES.fetch_add(1, Ordering::Relaxed);
    }
    with_use_scope(if verified { instance } else { 0 }, || {
        super::items::use_button(instance, down);
    });
}
pub(super) fn inventory_use_counts() -> (u64, u64) {
    (
        INVENTORY_USES.load(Ordering::Relaxed),
        INVENTORY_USE_OVERRIDES.load(Ordering::Relaxed),
    )
}
static MOVE_ALLOWED_CALLER: AtomicUsize = AtomicUsize::new(0);
static MOVE_ALLOWED_HITS: AtomicU64 = AtomicU64::new(0);
static MOVE_ALLOWED_OVERRIDES: AtomicU64 = AtomicU64::new(0);
static WALK_WRITES: AtomicU64 = AtomicU64::new(0);
static SNEAK_WRITES: AtomicU64 = AtomicU64::new(0);
static SAMPLES: AtomicU64 = AtomicU64::new(0);
static LAST_SAMPLE: AtomicU64 = AtomicU64::new(0);
static FLAGS: AtomicU32 = AtomicU32::new(0);
static GATE: AtomicU32 = AtomicU32::new(0);
/// Movement buttons released because the overlay began taking the input.
static CAPTURE_RELEASES: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn down_hook(closure: *const c_void, argument: *const u8, instance: usize) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    if gui::captures_input() || inventory::screen_open() || super::freecam::blocking() {
        // Raw/native input can bypass the overlay's WndProc filter. Do not
        // invoke vanilla attack or start the autoclicker for a console click.
        // Leave an earlier accepted hold for the input tick to release.
        BLOCKED_PRESSES.fetch_add(1, Ordering::Relaxed);
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    HELD.store(true, Ordering::Release);
    PRESSES.fetch_add(1, Ordering::Relaxed);
    CLICK_EPOCH.fetch_add(1, Ordering::Release);
    let now = GetTickCount64();
    if (super::item_assist::tool_requested(now) || super::durability_guard::requested(now))
        && input_gate(instance) == 0
    {
        let vt = read::<usize>(instance);
        let get_player: unsafe extern "system" fn(usize) -> usize =
            std::mem::transmute(read::<usize>(vt + 0xf0));
        let player = get_player(instance);
        super::durability_guard::sample(
            player,
            super::item_assist::idle() && !super::fishing::using_item(),
            now,
            true,
        );
        super::item_assist::before_attack(instance, player, now);
    }
    let original: AttackFn = std::mem::transmute(DOWN.load(Ordering::Acquire));
    original(closure, argument, instance);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}

unsafe extern "system" fn up_hook(closure: *const c_void, argument: *const u8, instance: usize) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    HELD.store(false, Ordering::Release);
    CLICK_EPOCH.fetch_add(1, Ordering::Release);
    let original: AttackFn = std::mem::transmute(UP.load(Ordering::Acquire));
    original(closure, argument, instance);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}

unsafe fn release_attack(instance: usize, active: bool, sdk_release: Option<usize>) {
    // WndProc may swallow the physical button-up while the overlay is open.
    // Release an accepted hold once on the game thread, including block mining.
    let held = !active && HELD.swap(false, Ordering::AcqRel);
    let sdk = sdk_release == Some(instance) && !HELD.load(Ordering::Acquire);
    if held || sdk {
        CLICK_EPOCH.fetch_add(1, Ordering::Release);
        let up: AttackFn = std::mem::transmute(UP.load(Ordering::Acquire));
        up(std::ptr::null(), std::ptr::null(), instance);
    }
}

unsafe extern "system" fn tick_hook(interface: usize) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    // This native vtable method receives the secondary interface at CI+0x88.
    sample(interface - 0x88);
    let original: InputTickFn = std::mem::transmute(INPUT_TICK.load(Ordering::Acquire));
    original(interface);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}

type MoveAllowedFn = unsafe extern "system" fn(usize) -> u8;
fn movement_permission(original: u8, inventory_context: bool, movement_caller: bool) -> u8 {
    if original == 0 && inventory_context && movement_caller {
        1
    } else {
        original
    }
}
unsafe extern "system" fn move_allowed_hook(receiver: usize) -> u8 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: MoveAllowedFn = std::mem::transmute(MOVE_ALLOWED.load(Ordering::Acquire));
    let result = original(receiver);
    let mut allowed = result;
    if super::freecam::blocking() {
        allowed = 0;
    } else if result == 0
        && inventory::active()
        && readable(receiver, 16)
        && inventory::native_move_allowed(read(receiver + 8), GetTickCount64())
    {
        MOVE_ALLOWED_HITS.fetch_add(1, Ordering::Relaxed);
        let mut trace = [std::ptr::null_mut(); 4];
        let count = RtlCaptureStackBackTrace(
            0,
            trace.len() as u32,
            trace.as_mut_ptr(),
            std::ptr::null_mut(),
        ) as usize;
        let expected = MOVE_ALLOWED_CALLER.load(Ordering::Acquire);
        let caller = expected != 0
            && trace[..count.min(trace.len())]
                .iter()
                .any(|p| *p as usize == expected);
        allowed = movement_permission(result, true, caller);
        if allowed != result {
            MOVE_ALLOWED_OVERRIDES.fetch_add(1, Ordering::Relaxed);
        }
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    allowed
}

// Vanilla sprint input goes through MoveInputComponent before intent, request,
// prediction and PlayerAuthInput generation. Never force ActorDataFlagComponent.
type TrackerFn = unsafe extern "system" fn(*const c_void, *const u8, usize);
type ToggleFn = unsafe extern "system" fn(*const c_void, usize);

#[repr(C)]
struct ButtonCapture {
    unused: usize,
    bit: i32,
    padding: u32,
}
const SPRINT_CAPTURE: ButtonCapture = ButtonCapture {
    unused: 0,
    bit: 8,
    padding: 0,
};
const JUMP_CAPTURE: ButtonCapture = ButtonCapture {
    unused: 0,
    bit: 7,
    padding: 0,
};
const FORWARD_CAPTURE: ButtonCapture = ButtonCapture {
    unused: 0,
    bit: 13,
    padding: 0,
};
const SNEAK_CAPTURE: ButtonCapture = ButtonCapture {
    unused: 0,
    bit: 0,
    padding: 0,
};
const BACK_CAPTURE: ButtonCapture = ButtonCapture {
    unused: 0,
    bit: 14,
    padding: 0,
};
const LEFT_CAPTURE: ButtonCapture = ButtonCapture {
    unused: 0,
    bit: 15,
    padding: 0,
};
const RIGHT_CAPTURE: ButtonCapture = ButtonCapture {
    unused: 0,
    bit: 16,
    padding: 0,
};
#[derive(Clone, Copy)]
enum MovementButton {
    Backward,
    Left,
    Right,
    Forward,
    Sneak,
    Sprint,
    Jump,
}
impl MovementButton {
    fn capture(self) -> &'static ButtonCapture {
        match self {
            Self::Backward => &BACK_CAPTURE,
            Self::Left => &LEFT_CAPTURE,
            Self::Right => &RIGHT_CAPTURE,
            Self::Forward => &FORWARD_CAPTURE,
            Self::Sneak => &SNEAK_CAPTURE,
            Self::Sprint => &SPRINT_CAPTURE,
            Self::Jump => &JUMP_CAPTURE,
        }
    }
    fn mask(self) -> u32 {
        1 << self.capture().bit
    }
    fn enabled(self) -> bool {
        let helper = match self {
            Self::Forward => WALK.load(Ordering::Acquire),
            Self::Sneak => SNEAK.load(Ordering::Acquire) || super::safe_walk::enabled(),
            Self::Sprint => SPRINT.load(Ordering::Acquire),
            Self::Jump => {
                (SPRINT.load(Ordering::Acquire) && SPRINT_JUMP.load(Ordering::Acquire))
                    || AUTO_JUMP.load(Ordering::Acquire)
                    || AUTO_SWIM.load(Ordering::Acquire)
            }
            _ => false,
        };
        helper || inventory::requested(self.capture().bit)
    }
    unsafe fn automatic(self, instance: usize, flags: u32, now: u64) -> bool {
        let helper = inventory::helpers_allowed()
            && match self {
                Self::Forward => {
                    request_active(&WALK, &WALK_UNTIL, now)
                        && wants_walk(flags, WALK_PAUSE_BACK.load(Ordering::Acquire))
                }
                Self::Sneak => {
                    (request_active(&SNEAK, &SNEAK_UNTIL, now)
                        && wants_sneak(flags, SNEAK_FORWARD_ONLY.load(Ordering::Acquire)))
                        || super::safe_walk::sneak(instance, flags, now)
                }
                Self::Sprint => request_active(&SPRINT, &SPRINT_UNTIL, now) && wants_sprint(flags),
                Self::Jump => automatic_jump(instance, flags, now),
                _ => false,
            };
        helper || inventory::requested(self.capture().bit)
    }
    fn counter(self) -> &'static AtomicU64 {
        match self {
            Self::Backward | Self::Left | Self::Right => &INVENTORY_WRITES,
            Self::Forward => &WALK_WRITES,
            Self::Sneak => &SNEAK_WRITES,
            Self::Sprint => &SPRINT_WRITES,
            Self::Jump => &JUMP_WRITES,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct JumpDemand {
    ground: bool,
    water: bool,
}
impl JumpDemand {
    fn active(&self) -> bool {
        self.ground || self.water
    }
    fn wants_input(&self, grounded: bool, underwater: bool) -> bool {
        (self.ground && grounded) || (self.water && underwater)
    }
}
fn jump_demand(
    flags: u32,
    sprint: bool,
    jump: bool,
    swim: bool,
    jump_forward: bool,
    swim_forward: bool,
) -> JumpDemand {
    // Sneak is always a deliberate override, including AutoSneak.
    let intent = |forward_only: bool| flags & 1 == 0 && (!forward_only || wants_sprint(flags));
    JumpDemand {
        ground: (sprint && wants_sprint(flags)) || (jump && intent(jump_forward)),
        water: swim && intent(swim_forward),
    }
}
fn request_active(enabled: &AtomicBool, until: &AtomicU64, now: u64) -> bool {
    enabled.load(Ordering::Acquire) && now <= until.load(Ordering::Acquire)
}
unsafe fn automatic_jump(instance: usize, flags: u32, now: u64) -> bool {
    let demand = jump_demand(
        flags,
        request_active(&SPRINT, &SPRINT_UNTIL, now) && SPRINT_JUMP.load(Ordering::Acquire),
        request_active(&AUTO_JUMP, &AUTO_JUMP_UNTIL, now),
        request_active(&AUTO_SWIM, &AUTO_SWIM_UNTIL, now),
        JUMP_FORWARD_ONLY.load(Ordering::Acquire),
        SWIM_FORWARD_ONLY.load(Ordering::Acquire),
    );
    if !demand.active() {
        return false;
    }
    let vtable: usize = read(instance);
    let local_player: unsafe extern "system" fn(usize) -> usize =
        std::mem::transmute(read::<usize>(vtable + 0xF0));
    let player = local_player(instance);
    let [grounded, underwater] = if demand.water {
        actor_jump_state(player)
    } else {
        [actor_on_ground(player), false]
    };
    GROUNDED.store(grounded, Ordering::Relaxed);
    UNDERWATER.store(underwater, Ordering::Relaxed);
    demand.wants_input(grounded, underwater)
}

#[derive(Default)]
struct InputHold {
    instance: usize,
    component: usize,
    manual: bool,
    owned: bool,
}
impl InputHold {
    fn inventory_ownership(&mut self, pressed: bool, managed: bool, handoff: bool) {
        if managed {
            self.manual = false;
            self.owned = pressed;
        } else if self.owned && handoff {
            self.manual = true;
            self.owned = false;
        }
    }
}

#[derive(Debug, PartialEq)]
enum ButtonChange {
    None,
    Press,
    Release,
}
fn button_change(auto: bool, manual: bool, owned: bool, pressed: bool) -> ButtonChange {
    if auto && !pressed {
        ButtonChange::Press
    } else if !auto && !manual && owned && pressed {
        ButtonChange::Release
    } else {
        ButtonChange::None
    }
}

unsafe fn input_gate(instance: usize) -> u32 {
    if gui::DORMANT.load(Ordering::Relaxed) || crate::api::blocked() {
        return 1;
    }
    // The overlay menu or the world map takes the input (`captures_input`).
    if gui::MENU_OPEN.load(Ordering::Relaxed) || gui::MAP_OPEN.load(Ordering::Relaxed) {
        return 2;
    }
    if !input::is_foreground() {
        return 3;
    }
    if super::freecam::blocking() {
        return 5;
    }
    if inventory::screen_open() {
        return 4;
    }
    let vtable: usize = read(instance);
    let blocked: unsafe extern "system" fn(usize) -> u8 =
        std::mem::transmute(read::<usize>(vtable + 0x518));
    if blocked(instance) != 0 {
        4
    } else {
        0
    }
}
fn movement_gate(base: u32, inventory_active: bool) -> u32 {
    if base == 4 && inventory_active {
        0
    } else {
        base
    }
}
fn fishing_active(base: u32, own_inventory: bool) -> bool {
    base == 0 || (base == 4 && own_inventory)
}

unsafe fn reconcile_button(
    native: &Native,
    instance: usize,
    hold: &mut InputHold,
    button: MovementButton,
    manual: Option<bool>,
    toggle: bool,
) {
    // Disabled helpers need no extra native lookups in the normal input path.
    // Still observe physical events and release any automatic input we own.
    if matches!(
        button,
        MovementButton::Backward
            | MovementButton::Left
            | MovementButton::Right
            | MovementButton::Forward
            | MovementButton::Sneak
    ) && !button.enabled()
        && !hold.owned
        && manual.is_none()
    {
        return;
    }
    let component = (native.movement)(instance);
    if !readable(component, 0x64) {
        *hold = InputHold::default();
        return;
    }
    let flags: u32 = read(component + 0x10);
    let mask = button.mask();
    let changed = (hold.instance, hold.component) != (instance, component);
    if changed {
        *hold = InputHold {
            instance,
            component,
            manual: flags & mask != 0,
            owned: false,
        };
    }
    if let Some(manual) = manual {
        hold.manual = manual;
    }
    if toggle && !changed {
        hold.manual = if hold.owned || button.enabled() {
            !hold.manual
        } else {
            flags & mask != 0
        };
    } else if manual.is_none() && !hold.owned && !button.enabled() {
        hold.manual = flags & mask != 0;
    }
    // Adopt existing movement inside the inventory; hand a still-held key
    // back to vanilla when the inventory closes.
    hold.inventory_ownership(
        flags & mask != 0,
        inventory::managed(button.capture().bit),
        inventory::handoff(button.capture().bit),
    );
    FLAGS.store(flags, Ordering::Relaxed);
    let now = GetTickCount64();
    let base_gate = input_gate(instance);
    let gate = movement_gate(base_gate, inventory::active());
    if gate != 0 {
        hold.manual = false;
    }
    GATE.store(gate, Ordering::Release);
    let auto = gate == 0 && button.automatic(instance, flags, now);
    // Ground jumping and underwater floating share one owned input. Removing
    // one request must not release another request or the player's Space hold.
    // On screens the native input reset owns manual input. Release only ours.
    let manual = hold.manual && gate == 0;
    let change = button_change(auto, manual, hold.owned, flags & mask != 0);
    let target = match change {
        ButtonChange::Press => TRACK_DOWN.load(Ordering::Acquire),
        ButtonChange::Release => TRACK_UP.load(Ordering::Acquire),
        ButtonChange::None => 0,
    };
    if target != 0 {
        let callback: TrackerFn = std::mem::transmute(target);
        callback(
            (button.capture() as *const ButtonCapture).cast(),
            std::ptr::null(),
            instance,
        );
        let after: u32 = read(component + 0x10);
        if inventory::active() && change == ButtonChange::Press {
            if after & mask != 0 {
                INVENTORY_ACCEPTED.fetch_add(1, Ordering::Relaxed);
            } else {
                INVENTORY_REJECTED.fetch_add(1, Ordering::Relaxed);
            }
        }
        if change == ButtonChange::Press && after & mask != 0 {
            hold.owned = !manual;
        }
        if change == ButtonChange::Release && after & mask == 0 {
            hold.owned = false;
        }
        button.counter().fetch_add(1, Ordering::Relaxed);
    }
    if manual || read::<u32>(component + 0x10) & mask == 0 {
        hold.owned = false;
    }
}

unsafe fn reconcile_movement(
    native: &Native,
    instance: usize,
    state: &mut State,
    manual: Option<(i32, bool)>,
) {
    for (button, hold) in state.movement() {
        let event = manual.and_then(|(bit, down)| (bit == button.capture().bit).then_some(down));
        reconcile_button(native, instance, hold, button, event, false);
    }
}

/// Releases every movement button the game still has down, through the
/// game's own button-up tracker (what a key release would do). Called while the
/// overlay takes the input: its WndProc filter keeps the key-up a
/// player makes meanwhile from the game, and unlike game screens the overlay
/// gets no native input reset, so a held W or Shift would otherwise stay down
/// after the menu closes. Returns how many buttons were released.
unsafe fn release_held_movement(native: &Native, instance: usize, state: &mut State) -> u32 {
    let component = (native.movement)(instance);
    if !readable(component, 0x64) {
        return 0;
    }
    let up = TRACK_UP.load(Ordering::Acquire);
    if up == 0 {
        return 0;
    }
    let callback: TrackerFn = std::mem::transmute(up);
    let mut released = 0;
    for (button, hold) in state.movement() {
        hold.manual = false;
        if read::<u32>(component + 0x10) & button.mask() == 0 {
            continue;
        }
        callback(
            (button.capture() as *const ButtonCapture).cast(),
            std::ptr::null(),
            instance,
        );
        if read::<u32>(component + 0x10) & button.mask() == 0 {
            hold.owned = false;
            released += 1;
        }
    }
    CAPTURE_RELEASES.fetch_add(released as u64, Ordering::Relaxed);
    released
}

unsafe fn tracker_event(closure: *const c_void, instance: usize, down: bool) {
    let Some(Ok(native)) = NATIVE.get() else {
        return;
    };
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    let bit = read::<i32>(closure as usize + 8);
    reconcile_movement(native, instance, &mut state, Some((bit, down)));
}
unsafe extern "system" fn track_down_hook(
    closure: *const c_void,
    argument: *const u8,
    instance: usize,
) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: TrackerFn = std::mem::transmute(TRACK_DOWN.load(Ordering::Acquire));
    if !super::freecam::blocking() {
        original(closure, argument, instance);
    }
    tracker_event(closure, instance, true);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}
unsafe extern "system" fn track_up_hook(
    closure: *const c_void,
    argument: *const u8,
    instance: usize,
) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: TrackerFn = std::mem::transmute(TRACK_UP.load(Ordering::Acquire));
    original(closure, argument, instance);
    tracker_event(closure, instance, false);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}
unsafe extern "system" fn sprint_toggle_hook(closure: *const c_void, instance: usize) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: ToggleFn = std::mem::transmute(SPRINT_TOGGLE.load(Ordering::Acquire));
    if !super::freecam::blocking() {
        original(closure, instance);
    }
    if let Some(Ok(native)) = NATIVE.get() {
        if let Ok(mut state) = STATE.try_lock() {
            reconcile_button(
                native,
                instance,
                &mut state.sprint,
                MovementButton::Sprint,
                None,
                true,
            );
        }
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}

#[allow(clippy::too_many_arguments)] // Separately verified native callback entries.
fn install_hooks(
    down: usize,
    up: usize,
    tick: usize,
    tracker_down: usize,
    tracker_up: usize,
    toggle: usize,
    mouse_feed: usize,
    move_allowed: usize,
    use_allowed: usize,
    attack_helper: Option<usize>,
    camera_sync: Option<usize>,
    render_camera: Option<usize>,
) -> Result<(), String> {
    let mut targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        match MH_Initialize() {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            e => return Err(format!("native input MH_Initialize: {e:?}")),
        }
        for (target, detour, original) in [
            (down, down_hook as *mut c_void, &DOWN),
            (up, up_hook as *mut c_void, &UP),
            (tick, tick_hook as *mut c_void, &INPUT_TICK),
            (tracker_down, track_down_hook as *mut c_void, &TRACK_DOWN),
            (tracker_up, track_up_hook as *mut c_void, &TRACK_UP),
            (toggle, sprint_toggle_hook as *mut c_void, &SPRINT_TOGGLE),
            (mouse_feed, mouse_feed_hook as *mut c_void, &MOUSE_FEED),
            (use_allowed, use_allowed_hook as *mut c_void, &USE_ALLOWED),
            (
                move_allowed,
                move_allowed_hook as *mut c_void,
                &MOVE_ALLOWED,
            ),
        ]
        .into_iter()
        .chain(attack_helper.map(|target| {
            (
                target,
                super::weapon::attack_hook as *mut c_void,
                &super::weapon::ORIGINAL,
            )
        }))
        .chain(camera_sync.map(|target| {
            (
                target,
                super::freelook::sync_hook as *mut c_void,
                &super::freelook::ORIGINAL,
            )
        }))
        .chain(render_camera.map(|target| {
            (
                target,
                super::freecam::render_hook as *mut c_void,
                &super::freecam::RENDER_ORIGINAL,
            )
        })) {
            let mut trampoline = std::ptr::null_mut();
            MH_CreateHook(target as *mut c_void, detour, &mut trampoline)
                .ok()
                .map_err(|e| format!("native input MH_CreateHook: {e:?}"))?;
            original.store(trampoline as usize, Ordering::Release);
            targets.push(target);
        }
        for &target in targets.iter() {
            MH_EnableHook(target as *mut c_void)
                .ok()
                .map_err(|e| format!("native input MH_EnableHook: {e:?}"))?;
        }
    }
    Ok(())
}
static NATIVE: OnceLock<Result<Native, String>> = OnceLock::new();
static SPRINT: AtomicBool = AtomicBool::new(false);
static SPRINT_JUMP: AtomicBool = AtomicBool::new(true);
static SPRINT_UNTIL: AtomicU64 = AtomicU64::new(0);
static WALK: AtomicBool = AtomicBool::new(false);
static WALK_UNTIL: AtomicU64 = AtomicU64::new(0);
static WALK_PAUSE_BACK: AtomicBool = AtomicBool::new(true);
static SNEAK: AtomicBool = AtomicBool::new(false);
static SNEAK_UNTIL: AtomicU64 = AtomicU64::new(0);
static SNEAK_FORWARD_ONLY: AtomicBool = AtomicBool::new(false);
static AUTO_JUMP: AtomicBool = AtomicBool::new(false);
static AUTO_JUMP_UNTIL: AtomicU64 = AtomicU64::new(0);
static JUMP_FORWARD_ONLY: AtomicBool = AtomicBool::new(true);
static AUTO_SWIM: AtomicBool = AtomicBool::new(false);
static AUTO_SWIM_UNTIL: AtomicU64 = AtomicU64::new(0);
static SWIM_FORWARD_ONLY: AtomicBool = AtomicBool::new(false);
static GROUNDED: AtomicBool = AtomicBool::new(false);
static UNDERWATER: AtomicBool = AtomicBool::new(false);
static CLICK_INTERVAL: AtomicU64 = AtomicU64::new(0);
static CLICK_UNTIL: AtomicU64 = AtomicU64::new(0);
static CLICK_EPOCH: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<State> = Mutex::new(State {
    captured: false,
    sdk_release: None,
    backward: InputHold {
        instance: 0,
        component: 0,
        manual: false,
        owned: false,
    },
    left: InputHold {
        instance: 0,
        component: 0,
        manual: false,
        owned: false,
    },
    right: InputHold {
        instance: 0,
        component: 0,
        manual: false,
        owned: false,
    },
    forward: InputHold {
        instance: 0,
        component: 0,
        manual: false,
        owned: false,
    },
    sneak: InputHold {
        instance: 0,
        component: 0,
        manual: false,
        owned: false,
    },
    jump: InputHold {
        instance: 0,
        component: 0,
        manual: false,
        owned: false,
    },
    sprint: InputHold {
        instance: 0,
        component: 0,
        manual: false,
        owned: false,
    },
    click: ClickClock { next: 0, epoch: 0 },
});

pub fn available() -> Result<(), String> {
    super::available()?;
    NATIVE
        .get_or_init(|| {
            if !actions_build() {
                return Err(format!(
                    "native actions require {}",
                    crate::modules::game::SUPPORTED
                ));
            }
            unsafe {
                let movement = std::mem::transmute::<usize, MoveFn>(patch::find(MOVE_SIGNATURE)?);
                let down = patch::find(ATTACK_DOWN_SIGNATURE)?;
                let up = patch::find(ATTACK_UP_SIGNATURE)?;
                let tick = patch::find(INPUT_TICK_SIGNATURE)?;
                let tracker_down = patch::find(TRACK_DOWN_SIGNATURE)?;
                let tracker_up = patch::find(TRACK_UP_SIGNATURE)?;
                let mouse_feed = patch::find(MOUSE_FEED_SIGNATURE)?;
                let move_allowed = patch::find(MOVE_ALLOWED_SIGNATURE)?;
                let use_allowed = patch::find(USE_ALLOWED_SIGNATURE)?;
                USE_ALLOWED_TARGET.store(use_allowed, Ordering::Release);
                MOVE_ALLOWED_CALLER.store(
                    patch::find(MOVE_ALLOWED_CALL_SIGNATURE)? + 43,
                    Ordering::Release,
                );
                // Unique toggle-bit operation at entry+0x107 (known-build only).
                let toggle = patch::find(SPRINT_TOGGLE_SIGNATURE)? - 0x107;
                let attack_helper = super::combat::attack_helper().ok();
                let camera_sync = super::freelook::resolve().ok();
                // Freecam's render camera hook, only with the camera sync.
                let render_camera = camera_sync.and(super::freecam::render_target().ok());
                if std::slice::from_raw_parts(toggle as *const u8, 5)
                    != [0x56, 0x48, 0x83, 0xEC, 0x20]
                {
                    return Err("sprint toggle entry validation failed".into());
                }
                install_hooks(
                    down,
                    up,
                    tick,
                    tracker_down,
                    tracker_up,
                    toggle,
                    mouse_feed,
                    move_allowed,
                    use_allowed,
                    attack_helper,
                    camera_sync,
                    render_camera,
                )?;
                Ok(Native { movement })
            }
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

pub fn set_sprint_jump(on: bool) {
    SPRINT_JUMP.store(on, Ordering::Release);
}

pub fn sprint(active: bool, now: u64) {
    SPRINT_UNTIL.store(now.saturating_add(100), Ordering::Release);
    SPRINT.store(active, Ordering::Release);
}

pub fn set_walk_pause_back(on: bool) {
    WALK_PAUSE_BACK.store(on, Ordering::Release);
}
pub fn set_sneak_forward_only(on: bool) {
    SNEAK_FORWARD_ONLY.store(on, Ordering::Release);
}
pub fn walk(active: bool, now: u64) {
    WALK_UNTIL.store(now.saturating_add(100), Ordering::Release);
    WALK.store(active, Ordering::Release);
}
pub fn sneak(active: bool, now: u64) {
    SNEAK_UNTIL.store(now.saturating_add(100), Ordering::Release);
    SNEAK.store(active, Ordering::Release);
}

pub fn set_jump_forward_only(on: bool) {
    JUMP_FORWARD_ONLY.store(on, Ordering::Release);
}
pub fn set_swim_forward_only(on: bool) {
    SWIM_FORWARD_ONLY.store(on, Ordering::Release);
}
pub fn auto_jump(active: bool, now: u64) {
    AUTO_JUMP_UNTIL.store(now.saturating_add(100), Ordering::Release);
    AUTO_JUMP.store(active, Ordering::Release);
}
pub fn auto_swim(active: bool, now: u64) {
    AUTO_SWIM_UNTIL.store(now.saturating_add(100), Ordering::Release);
    AUTO_SWIM.store(active, Ordering::Release);
}

pub fn click(interval: u64, now: u64) {
    CLICK_UNTIL.store(now.saturating_add(100), Ordering::Release);
    if CLICK_INTERVAL.swap(interval, Ordering::AcqRel) != interval {
        CLICK_EPOCH.fetch_add(1, Ordering::Release);
    }
}

const FORWARD_BITS: u32 = (1 << 13) | (1 << 9) | (1 << 10);
const BACK_BITS: u32 = (1 << 14) | (1 << 11) | (1 << 12);

fn wants_walk(flags: u32, pause_back: bool) -> bool {
    !pause_back || flags & BACK_BITS == 0
}
fn wants_sneak(flags: u32, forward_only: bool) -> bool {
    !forward_only || flags & FORWARD_BITS != 0
}

fn wants_sprint(flags: u32) -> bool {
    flags & FORWARD_BITS != 0 && flags & (BACK_BITS | 1) == 0
}

struct State {
    captured: bool,
    sdk_release: Option<usize>,
    backward: InputHold,
    left: InputHold,
    right: InputHold,
    forward: InputHold,
    sneak: InputHold,
    jump: InputHold,
    sprint: InputHold,
    click: ClickClock,
}

impl State {
    /// Every movement button with its hold, in reconciliation order.
    fn movement(&mut self) -> [(MovementButton, &mut InputHold); 7] {
        [
            (MovementButton::Backward, &mut self.backward),
            (MovementButton::Left, &mut self.left),
            (MovementButton::Right, &mut self.right),
            (MovementButton::Forward, &mut self.forward),
            (MovementButton::Sneak, &mut self.sneak),
            (MovementButton::Sprint, &mut self.sprint),
            (MovementButton::Jump, &mut self.jump),
        ]
    }
    /// Whether any movement input is still held by the client.
    fn owns_movement(&self) -> bool {
        [
            &self.backward,
            &self.left,
            &self.right,
            &self.forward,
            &self.sneak,
            &self.sprint,
            &self.jump,
        ]
        .iter()
        .any(|hold| hold.owned)
    }
}

struct ClickClock {
    next: u64,
    epoch: u64,
}

impl ClickClock {
    fn due(&mut self, now: u64, interval: u64, epoch: u64) -> bool {
        if interval == 0 {
            self.next = 0;
            self.epoch = epoch;
            return false;
        }
        if self.next == 0 || self.epoch != epoch {
            self.epoch = epoch;
            self.next = now.saturating_add(interval);
            return false; // Physical press already performed the first attack.
        }
        if now < self.next {
            return false;
        }
        self.next = now.saturating_add(interval);
        true // Never catch up with a burst after a stalled frame.
    }
}

pub(super) unsafe fn sample(instance: usize) {
    let Some(Ok(native)) = NATIVE.get() else {
        return;
    };
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    let now = GetTickCount64();
    SAMPLES.fetch_add(1, Ordering::Relaxed);
    LAST_SAMPLE.store(now, Ordering::Release);
    let vtable: usize = read(instance);
    // A fresh native lookup checks both entity generation and component presence.
    let component = (native.movement)(instance);
    let valid = readable(component, 0x64);
    let local_player: unsafe extern "system" fn(usize) -> usize =
        std::mem::transmute(read::<usize>(vtable + 0xF0));
    let player = local_player(instance);
    super::freecam::observe_player(player, now);
    inventory::sample(instance, player, now);
    super::terrain_session::observe_presence(player != 0, now);
    super::items::sample(player, now);
    super::telemetry::sample(player, now);
    let gate = input_gate(instance);
    GATE.store(gate, Ordering::Release);
    let active = gate == 0;
    let capture = gui::captures_input() || super::freecam::blocking();
    if capture && !state.captured {
        super::items::use_button(instance, false);
    }
    if capture {
        // Held movement keys would otherwise stay down (see the function).
        // Every tick, not only on the edge: a key-down that slipped in just
        // before the filter took over still gets its release. Nothing is
        // called while no button is down, so this is one read per button.
        release_held_movement(native, instance, &mut state);
    }
    state.captured = capture;
    release_attack(instance, active, state.sdk_release.take());
    let local_flags = if player != 0 {
        actor_flags(player).unwrap_or(0)
    } else {
        0
    };
    LOCAL_FLAGS.store(local_flags, Ordering::Release);
    SPRINTING.store(
        local_flags != 0 && read::<u64>(local_flags) & 8 != 0,
        Ordering::Release,
    );
    reconcile_movement(native, instance, &mut state, None);
    for pending in super::sdk_frame(instance, player, active) {
        let result = match pending.command {
            mcbe_sdk::bridge::Command::Click => {
                let up: AttackFn = std::mem::transmute(UP.load(Ordering::Acquire));
                let down: AttackFn = std::mem::transmute(DOWN.load(Ordering::Acquire));
                up(std::ptr::null(), std::ptr::null(), instance);
                down(std::ptr::null(), std::ptr::null(), instance);
                if !HELD.load(Ordering::Acquire) {
                    state.sdk_release = Some(instance);
                }
                Ok(())
            }
            mcbe_sdk::bridge::Command::SetVelocity(velocity) => {
                super::sdk_velocity(player, velocity)
            }
        };
        pending.complete(result);
    }
    let eating = super::item_assist::sample(
        instance,
        player,
        active && !super::fishing::using_item(),
        HELD.load(Ordering::Acquire),
        now,
    );
    let fishing = super::fishing::sample(
        instance,
        player,
        fishing_active(gate, inventory::own_inventory_for(instance, now)),
        eating || HELD.load(Ordering::Acquire),
        now,
    );
    super::durability_guard::sample(
        player,
        active && !eating && !fishing && super::item_assist::idle(),
        now,
        false,
    );
    let aura_owns_attack = super::combat::sample(
        instance,
        player,
        active && !eating && !fishing,
        HELD.load(Ordering::Acquire),
        now,
    );
    let interval = if active
        && !eating
        && !fishing
        && !aura_owns_attack
        && valid
        && HELD.load(Ordering::Acquire)
        && now <= CLICK_UNTIL.load(Ordering::Acquire)
    {
        CLICK_INTERVAL.load(Ordering::Acquire)
    } else {
        0
    };
    if state
        .click
        .due(now, interval, CLICK_EPOCH.load(Ordering::Acquire))
    {
        // Same release/press handlers as button.destroy_or_attack. They resolve
        // the current crosshair hit and apply the game's normal interaction rules.
        let up: AttackFn = std::mem::transmute(UP.load(Ordering::Acquire));
        let down: AttackFn = std::mem::transmute(DOWN.load(Ordering::Acquire));
        super::weapon::automated(|| {
            up(std::ptr::null(), std::ptr::null(), instance);
            down(std::ptr::null(), std::ptr::null(), instance);
        });
        ATTACKS.fetch_add(1, Ordering::Relaxed);
    }
}

/// A fresh, unmodified world-input gate; InventoryMove exceptions do not apply.
pub(crate) fn world_input_active(now: u64) -> bool {
    let at = LAST_SAMPLE.load(Ordering::Acquire);
    recent_world_input(now, at, GATE.load(Ordering::Acquire))
}
fn recent_world_input(now: u64, at: u64, gate: u32) -> bool {
    at != 0 && now.saturating_sub(at) <= 150 && gate == 0
}

/// Keep the hook/DLL alive if a suspended game thread has not removed our flag.
pub fn shutdown() -> bool {
    if !super::fishing::shutdown() {
        return false;
    }
    super::item_assist::request(true, false, 0);
    super::item_assist::request(false, false, 0);
    super::combat::request(false, 0);
    super::weapon::request(false, 0, 5, false);
    super::friends::enable(false);
    super::freelook::request(false, 0);
    super::freecam::request(false, 0, 10., 3.);
    super::safe_walk::request(false, 0, 0.12);
    super::durability_guard::request(false, 0, 10, true);
    sprint(false, 0);
    walk(false, 0);
    sneak(false, 0);
    auto_jump(false, 0);
    auto_swim(false, 0);
    inventory::request(false, 0);
    click(0, 0);
    for _ in 0..100 {
        // Synchronize with a sample that may already have read SPRINT=true.
        // An atomic snapshot alone could miss its not-yet-published write.
        if STATE.try_lock().is_ok_and(|state| {
            super::item_assist::idle()
                && !state.owns_movement()
                && state.sdk_release.is_none()
                && super::freelook::restored()
        }) {
            let targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
            return targets.iter().all(|&target| crate::hooks::disable(target))
                && crate::hooks::drain(&IN_FLIGHT, 2);
        }
        unsafe { Sleep(2) };
    }
    false
}

pub fn diagnostic() -> String {
    let age = unsafe { GetTickCount64() }.saturating_sub(LAST_SAMPLE.load(Ordering::Acquire));
    format!("input ticks={} age={}ms gate={} flags=0x{:X} held={} presses={} blocked_presses={} blocked_motion={} attacks={} sprint_inputs={} jump_inputs={} walk_inputs={} sneak_inputs={} inventory_inputs={} grounded={} underwater={} local_flags=0x{:X} sprinting={}",
        SAMPLES.load(Ordering::Relaxed), age, GATE.load(Ordering::Acquire), FLAGS.load(Ordering::Relaxed),
        HELD.load(Ordering::Acquire), PRESSES.load(Ordering::Relaxed), BLOCKED_PRESSES.load(Ordering::Relaxed), BLOCKED_MOTION.load(Ordering::Relaxed), ATTACKS.load(Ordering::Relaxed),
        SPRINT_WRITES.load(Ordering::Relaxed), JUMP_WRITES.load(Ordering::Relaxed), WALK_WRITES.load(Ordering::Relaxed), SNEAK_WRITES.load(Ordering::Relaxed), INVENTORY_WRITES.load(Ordering::Relaxed), GROUNDED.load(Ordering::Relaxed), UNDERWATER.load(Ordering::Relaxed), LOCAL_FLAGS.load(Ordering::Acquire), SPRINTING.load(Ordering::Acquire))
}
pub fn inventory_diagnostic() -> String {
    format!(
        "{} accepted={} rejected={} movement_gate_hits={} movement_gate_allowed={} overlay_releases={}",
        inventory::diagnostic(),
        INVENTORY_ACCEPTED.load(Ordering::Relaxed),
        INVENTORY_REJECTED.load(Ordering::Relaxed),
        MOVE_ALLOWED_HITS.load(Ordering::Relaxed),
        MOVE_ALLOWED_OVERRIDES.load(Ordering::Relaxed),
        CAPTURE_RELEASES.load(Ordering::Relaxed)
    )
}

/// Why the input helpers are not acting: no recent native input tick, or the
/// input gate (stopping, menu, focus, native screen). None while active.
fn paused_reason() -> Option<&'static str> {
    if unsafe { GetTickCount64() }.saturating_sub(LAST_SAMPLE.load(Ordering::Acquire)) > 1000 {
        return Some("waiting for native input tick");
    }
    match GATE.load(Ordering::Acquire) {
        1 => Some("stopping"),
        2 => Some("paused: menu open"),
        3 => Some("paused: game not focused"),
        4 => Some("paused: native screen"),
        _ => None,
    }
}

pub fn assist_status(walking: bool) -> String {
    if let Some(reason) = paused_reason() {
        return reason.into();
    }
    let flags = FLAGS.load(Ordering::Acquire);
    if walking && !wants_walk(flags, WALK_PAUSE_BACK.load(Ordering::Acquire)) {
        "paused: manual backward".into()
    } else if !walking && !wants_sneak(flags, SNEAK_FORWARD_ONLY.load(Ordering::Acquire)) {
        "waiting for forward movement".into()
    } else if flags & if walking { 1 << 13 } else { 1 } != 0 {
        if walking { "walking" } else { "sneaking" }.into()
    } else {
        "waiting for movement input".into()
    }
}

pub fn jump_assist_status(swimming: bool) -> String {
    if let Some(reason) = paused_reason() {
        return reason.into();
    }
    let flags = FLAGS.load(Ordering::Acquire);
    if flags & 1 != 0 {
        return "paused: sneak input".into();
    }
    let forward_only = if swimming {
        SWIM_FORWARD_ONLY.load(Ordering::Acquire)
    } else {
        JUMP_FORWARD_ONLY.load(Ordering::Acquire)
    };
    if forward_only && !wants_sprint(flags) {
        return "waiting for forward movement".into();
    }
    if swimming {
        if UNDERWATER.load(Ordering::Acquire) {
            "floating upward"
        } else {
            "waiting for submerged head"
        }
    } else if GROUNDED.load(Ordering::Acquire) {
        "jumping on ground"
    } else {
        "waiting for landing"
    }
    .into()
}

pub fn status(sprint: bool) -> String {
    if let Some(reason) = paused_reason() {
        return crate::localization::text(reason).into();
    }
    if sprint {
        let text = if !SPRINTING.load(Ordering::Acquire) {
            "waiting for forward movement"
        } else if SPRINT_JUMP.load(Ordering::Acquire) {
            "sprinting + jump"
        } else {
            "sprinting"
        };
        crate::localization::text(text).into()
    } else if !HELD.load(Ordering::Acquire) {
        format!(
            "左ボタンを押すと攻撃（{}回）",
            ATTACKS.load(Ordering::Relaxed)
        )
    } else {
        format!("攻撃{}回", ATTACKS.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests;
