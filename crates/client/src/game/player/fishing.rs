//! Observe the native catchable-window event; use actions run only on input ticks.
//! No network packet construction, guessed vertical-motion trigger, or key events.
use crate::game::{
    actors::{self, type_hash, UNIQUE_ID},
    blocks,
    entities::{component_pools, PoolView},
    inventory, items,
    layout::image,
    memory::ReadBatch,
    metadata, patch, read,
};
use crate::signatures::FISH_EVENT_SIGNATURE;
use hudhook::mh::{MH_CreateHook, MH_EnableHook, MH_Initialize, MH_STATUS};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
};
use windows_sys::Win32::System::{SystemInformation::GetTickCount64, Threading::Sleep};

const HOOK_FLAG: u32 = type_hash("FishingHookFlagComponent");
const BITE_EVENT: u8 = 13;
type EventFn = unsafe extern "system" fn(usize, u8, i32);
static TARGET: OnceLock<Result<usize, String>> = OnceLock::new();
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static ENABLED: AtomicUsize = AtomicUsize::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static USING_ITEM: AtomicBool = AtomicBool::new(false);
static UNTIL: AtomicU64 = AtomicU64::new(0);
static BITES: AtomicU64 = AtomicU64::new(0);
static REELS: AtomicU64 = AtomicU64::new(0);
static CASTS: AtomicU64 = AtomicU64::new(0);
static EVENTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static OWNER_FAILURES: AtomicU64 = AtomicU64::new(0);
static RECAST: AtomicUsize = AtomicUsize::new(1);
static DELAY: AtomicU64 = AtomicU64::new(800);
static RESERVE: AtomicUsize = AtomicUsize::new(5);
static CLEAR_VIEW: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
struct World(usize, u32, usize);
#[derive(Clone, Copy)]
struct Bite {
    world: World,
    hook: u32,
    at: u64,
}
static BITE: Mutex<Option<Bite>> = Mutex::new(None);
static LOCAL: Mutex<Option<(World, u64)>> = Mutex::new(None);
static STATE: Mutex<State> = Mutex::new(State {
    phase: Phase::Waiting,
    pressed: None,
    next_sample: 0,
    manual_until: 0,
    status: "cast a fishing rod manually",
    last_playing: "no active sample yet",
});
#[derive(Default)]
enum Phase {
    #[default]
    Waiting,
    Recast {
        world: World,
        slot: usize,
        hook: u32,
        after: u64,
        expires: u64,
    },
}
struct Pressed {
    instance: usize,
    world: World,
    release: u64,
}
#[derive(Debug, PartialEq, Eq)]
enum UseChange {
    Keep,
    Release,
    Handoff,
}
impl Pressed {
    fn change(
        &self,
        instance: usize,
        world: Option<World>,
        manual: bool,
        allowed: bool,
        now: u64,
    ) -> UseChange {
        if manual || instance != self.instance {
            UseChange::Handoff
        } else if !allowed || world != Some(self.world) || now >= self.release {
            UseChange::Release
        } else {
            UseChange::Keep
        }
    }
}
pub(in crate::game) fn using_item() -> bool {
    USING_ITEM.load(Ordering::Acquire)
}
struct State {
    phase: Phase,
    pressed: Option<Pressed>,
    next_sample: u64,
    manual_until: u64,
    status: &'static str,
    last_playing: &'static str,
}
impl State {
    fn playing(&mut self, status: &'static str) {
        self.status = status;
        self.last_playing = status;
    }
}

pub fn available() -> Result<(), String> {
    TARGET
        .get_or_init(|| {
            if !crate::game::actions_build() {
                return Err(format!("autofish requires {}", crate::game::SUPPORTED));
            }
            metadata::available()?;
            inventory::available()?;
            items::available()?;
            crate::game::actions::available()?;
            let target = patch::find(FISH_EVENT_SIGNATURE)?;
            let Some(hook_vtable) = crate::game::layout::addresses().hook_vtable else {
                return Err(format!(
                    "autofish requires {}",
                    crate::game::layout::SUPPORTED
                ));
            };
            let slot = image().0 + hook_vtable + 0x220;
            if unsafe { read::<usize>(slot) } != target {
                return Err("FishingHook event vtable mismatch".into());
            }
            Ok(target)
        })
        .clone()
        .map(|_| ())
}
pub fn enable(on: bool) -> Result<(), String> {
    if !on {
        return if stop_observing() {
            Ok(())
        } else {
            Err("fishing event hook is still executing; keep DLL loaded".into())
        };
    }
    available()?;
    let target = TARGET.get().unwrap().as_ref().map_err(Clone::clone)?;
    unsafe {
        match MH_Initialize() {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            e => return Err(format!("MH_Initialize: {e:?}")),
        }
        if ORIGINAL.load(Ordering::Acquire) == 0 {
            let mut original = std::ptr::null_mut();
            MH_CreateHook(*target as *mut c_void, detour as *mut c_void, &mut original)
                .ok()
                .map_err(|e| format!("fish hook: {e:?}"))?;
            ORIGINAL.store(original as usize, Ordering::Release);
        }
        MH_EnableHook(*target as *mut c_void)
            .ok()
            .map_err(|e| format!("enable fish hook: {e:?}"))?;
    }
    ENABLED.store(1, Ordering::Release);
    Ok(())
}
pub fn request(on: bool, now: u64) {
    UNTIL.store(
        if on { now.saturating_add(150) } else { 0 },
        Ordering::Release,
    );
}
pub fn configure(recast: bool, delay: u64, reserve: u16, clear_view: bool) {
    RECAST.store(usize::from(recast), Ordering::Relaxed);
    DELAY.store(delay.clamp(400, 4000), Ordering::Relaxed);
    RESERVE.store(reserve as usize, Ordering::Relaxed);
    CLEAR_VIEW.store(clear_view, Ordering::Relaxed);
}

unsafe extern "system" fn detour(hook: usize, event: u8, data: i32) {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let now = GetTickCount64();
    if (12..=14).contains(&event) {
        EVENTS[(event - 12) as usize].fetch_add(1, Ordering::Relaxed);
    }
    if event == BITE_EVENT && now < UNTIL.load(Ordering::Acquire) {
        let own = LOCAL.try_lock().ok().and_then(|v| *v);
        if let Some((world, uid)) = own {
            let found = {
                let mut m = ReadBatch::new();
                if m.readable(hook, 0x1f8)
                    && crate::game::layout::addresses()
                        .hook_vtable
                        .is_some_and(|vt| read::<usize>(hook) == image().0 + vt)
                    && read::<usize>(hook + 0x10) == world.0
                    && read::<usize>(hook + 0x1f0) == world.2
                {
                    let entity = read::<u32>(hook + 0x18);
                    let [meta, flag] =
                        component_pools(&mut m, world.0, [metadata::HASH, HOOK_FLAG]);
                    let meta = meta.and_then(|p| metadata::View::new(&mut m, p));
                    let live = flag
                        .and_then(|p| PoolView::new(&mut m, p, 0))
                        .is_some_and(|p| p.contains(&mut m, entity));
                    (live && meta.and_then(|p| p.owner(&mut m, entity)) == Some(uid))
                        .then_some(entity)
                } else {
                    None
                }
            };
            if let Some(hook) = found {
                BITES.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut slot) = BITE.try_lock() {
                    *slot = Some(Bite {
                        world,
                        hook,
                        at: now,
                    });
                }
            } else {
                OWNER_FAILURES.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let original: EventFn = std::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    original(hook, event, data);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}
unsafe fn world_uid(player: usize) -> Option<(World, u64)> {
    let mut m = ReadBatch::new();
    if !m.readable(player, 0x1f8) {
        return None;
    }
    let world = World(
        read(player + 0x10),
        read(player + 0x18),
        read(player + 0x1f0),
    );
    let [pool] = component_pools(&mut m, world.0, [UNIQUE_ID]);
    let pool = PoolView::new(&mut m, pool?, 8)?;
    let at = pool.component(&mut m, world.1)?;
    let uid = read::<u64>(at);
    (uid != 0 && uid != u64::MAX).then_some((world, uid))
}
unsafe fn own_hooks(world: World, uid: u64) -> Option<Vec<u32>> {
    let mut m = ReadBatch::new();
    let [hooks, meta] = component_pools(&mut m, world.0, [HOOK_FLAG, metadata::HASH]);
    let Some(hooks) = hooks else {
        return Some(Vec::new());
    };
    let hooks = PoolView::new(&mut m, hooks, 0)?;
    if hooks.entities().len() / 4 > 128 {
        return None;
    }
    let meta = metadata::View::new(&mut m, meta?)?;
    let mut found = Vec::new();
    for at in hooks.entities().step_by(4).take(128) {
        let entity = read::<u32>(at);
        if hooks.contains(&mut m, entity) && meta.owner(&mut m, entity) == Some(uid) {
            found.push(entity);
        }
    }
    Some(found)
}
fn valid_bite(b: Bite, world: World, now: u64, hooks: &[u32]) -> bool {
    b.world == world && now.saturating_sub(b.at) <= 500 && hooks.contains(&b.hook)
}
/// `busy` serializes with AutoEat/AutoTool. Caller already checked game focus/UI.
pub(in crate::game) unsafe fn sample(
    instance: usize,
    player: usize,
    active: bool,
    busy: bool,
    now: u64,
) -> bool {
    if ENABLED.load(Ordering::Acquire) == 0 && !using_item() {
        return false;
    }
    let Ok(mut state) = STATE.try_lock() else {
        return true;
    };
    // Right-clicks in inventory belong to slot manipulation, not world use.
    let manual = !inventory::own_inventory_for(instance, now) && crate::input::key_down(2);
    let identity = if ENABLED.load(Ordering::Acquire) != 0 {
        world_uid(player)
    } else {
        None
    };
    if let Ok(mut local) = LOCAL.try_lock() {
        *local = identity;
    }
    let world = identity.map(|(w, _)| w);
    if let Some(press) = state.pressed.take() {
        USING_ITEM.store(false, Ordering::Release);
        match press.change(
            instance,
            world,
            manual,
            active && now < UNTIL.load(Ordering::Acquire),
            now,
        ) {
            UseChange::Release => crate::game::actions::fishing_use(instance, false),
            UseChange::Keep => {
                state.pressed = Some(press);
                USING_ITEM.store(true, Ordering::Release);
            }
            UseChange::Handoff => {}
        }
    }
    let requested = active && !busy && now < UNTIL.load(Ordering::Acquire);
    if manual {
        state.manual_until = now.saturating_add(1500);
    }
    if !requested || manual || now < state.manual_until {
        state.phase = Phase::Waiting;
        if let Ok(mut b) = BITE.try_lock() {
            *b = None;
        }
        state.status = if manual {
            "manual use"
        } else {
            "paused / waiting"
        };
        if active && (manual || busy) {
            state.last_playing = if manual {
                "manual right button"
            } else {
                "mining / another item helper"
            };
        }
        return state.pressed.is_some();
    }
    if state.pressed.is_some() {
        return true;
    }
    if now < state.next_sample {
        return false;
    }
    state.next_sample = now.saturating_add(50);
    let Some((world, uid)) = identity else {
        state.phase = Phase::Waiting;
        state.playing("player identity unavailable");
        return false;
    };
    let Some(inv) =
        items::snapshot().filter(|inv| (inv.world.0, inv.world.1) == (world.0, world.1))
    else {
        state.playing("inventory snapshot missing or stale");
        return false;
    };
    let rod = inv
        .slots
        .get(inv.selected)
        .and_then(Option::as_ref)
        .filter(|i| {
            i.name == "fishing_rod" && usize::from(i.remaining()) > RESERVE.load(Ordering::Relaxed)
        });
    if rod.is_none() || items::selected(player) != Some(inv.selected) {
        state.phase = Phase::Waiting;
        state.playing("select a fishing rod with enough durability");
        return false;
    }
    let Some(hooks) = own_hooks(world, uid) else {
        state.playing("bobber metadata unavailable / enumeration limit");
        return false;
    };
    let bite = BITE.try_lock().ok().and_then(|mut b| b.take());
    let mut cast = false;
    let reel = bite.filter(|b| valid_bite(*b, world, now, &hooks));
    if let Some(b) = reel {
        state.phase = if RECAST.load(Ordering::Relaxed) != 0 {
            Phase::Recast {
                world,
                slot: inv.selected,
                hook: b.hook,
                after: now.saturating_add(DELAY.load(Ordering::Relaxed)),
                expires: now.saturating_add(6000),
            }
        } else {
            Phase::Waiting
        };
        state.playing("reeling in");
    } else if let Phase::Recast {
        world: old,
        slot,
        hook,
        after,
        expires,
    } = state.phase
    {
        if RECAST.load(Ordering::Relaxed) == 0
            || old != world
            || slot != inv.selected
            || now > expires
        {
            state.phase = Phase::Waiting;
            state.playing("recast cancelled");
        } else if now >= after
            && !hooks.contains(&hook)
            && hooks.is_empty()
            && (!CLEAR_VIEW.load(Ordering::Relaxed) || clear_cast_view())
        {
            // Preserve the user's deliberate view: do not turn or switch slots.
            state.phase = Phase::Waiting;
            cast = true;
            state.playing("casting again");
        } else if now >= after {
            state.playing(if hooks.is_empty() {
                "recast blocked: solid block / nearby living actor"
            } else {
                "waiting for old bobber to disappear"
            });
        }
    } else {
        state.playing("waiting for own bobber bite");
    }
    if reel.is_some() || cast {
        crate::game::actions::fishing_use(instance, true);
        state.pressed = Some(Pressed {
            instance,
            world,
            release: now.saturating_add(60),
        });
        USING_ITEM.store(true, Ordering::Release);
        if cast {
            CASTS.fetch_add(1, Ordering::Relaxed);
        } else {
            REELS.fetch_add(1, Ordering::Relaxed);
        }
        return true;
    }
    false
}
pub fn status() -> String {
    let current = STATE.try_lock().map_or("sampling", |s| s.status);
    format!(
        "{} / 回収{}回・再投{}回",
        crate::localization::text(current),
        REELS.load(Ordering::Relaxed),
        CASTS.load(Ordering::Relaxed)
    )
}
pub fn diagnostic() -> String {
    let (inventory_uses, gate_overrides) = crate::game::actions::inventory_use_counts();
    let (status, last) = STATE
        .try_lock()
        .map_or(("sampling", "sampling"), |s| (s.status, s.last_playing));
    format!(
        "autofish: {status}; last playing={last}; own bites={} reel requests={} cast requests={}; events[12,13,14]={:?}, owner/layout rejects={}; inventory uses={inventory_uses} gate overrides={gate_overrides}",
        BITES.load(Ordering::Relaxed),
        REELS.load(Ordering::Relaxed),
        CASTS.load(Ordering::Relaxed),
        EVENTS.each_ref().map(|n|n.load(Ordering::Relaxed)),
        OWNER_FAILURES.load(Ordering::Relaxed)
    )
}
fn clear_cast_view() -> bool {
    if blocks::look() != Some(None) {
        return false;
    }
    let Some(player) = crate::api::game().ok().and_then(|g| g.player().ok()) else {
        return false;
    };
    actors::actors().is_some_and(|actors| {
        !actors.values().any(|a| {
            !a.local
                && (a.name.as_ref() == "player" || a.health.is_some())
                && a.bounds.is_some_and(|b| {
                    (0..3)
                        .map(|k| {
                            ([player.position.x, player.position.y, player.position.z][k]
                                - (b[k] + b[k + 3]) * 0.5)
                                .powi(2)
                        })
                        .sum::<f32>()
                        < 36.
                })
        })
    })
}
fn stop_observing() -> bool {
    request(false, 0);
    if ENABLED.load(Ordering::Acquire) != 0 {
        let Some(Ok(target)) = TARGET.get() else {
            return false;
        };
        if !crate::hooks::disable(*target) {
            return false;
        }
        ENABLED.store(0, Ordering::Release);
    }
    if !crate::hooks::drain(&IN_FLIGHT, 1) {
        return false;
    }
    *LOCAL.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *BITE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    STATE.lock().unwrap_or_else(|e| e.into_inner()).phase = Phase::Waiting;
    true
}
pub fn shutdown() -> bool {
    if !stop_observing() {
        return false;
    }
    for _ in 0..100 {
        if STATE.try_lock().is_ok_and(|s| s.pressed.is_none()) {
            return true;
        }
        unsafe {
            Sleep(1);
        }
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bites_require_same_world_live_generation_and_fresh_event() {
        let world = World(10, 1, 20);
        let b = Bite {
            world,
            hook: 0x40002,
            at: 100,
        };
        assert!(valid_bite(b, world, 200, &[0x40002]));
        assert!(!valid_bite(b, World(11, 1, 20), 200, &[0x40002]));
        assert!(!valid_bite(b, World(10, 1, 30), 200, &[0x40002]));
        assert!(!valid_bite(b, world, 601, &[0x40002]));
        assert!(!valid_bite(b, world, 200, &[0x80002]));
    }
    #[test]
    fn use_release_preserves_manual_input_and_cleans_up_on_pause_off_or_world_change() {
        let world = World(10, 1, 20);
        let p = Pressed {
            instance: 30,
            world,
            release: 200,
        };
        assert_eq!(p.change(30, Some(world), false, true, 100), UseChange::Keep);
        assert_eq!(
            p.change(30, Some(world), false, true, 200),
            UseChange::Release
        );
        assert_eq!(
            p.change(30, Some(world), false, false, 100),
            UseChange::Release
        );
        assert_eq!(p.change(30, None, false, true, 100), UseChange::Release);
        assert_eq!(
            p.change(30, Some(World(10, 2, 20)), false, true, 100),
            UseChange::Release
        );
        assert_eq!(
            p.change(30, Some(world), true, false, 100),
            UseChange::Handoff
        );
        assert_eq!(
            p.change(40, Some(world), false, false, 100),
            UseChange::Handoff
        );
    }
}
