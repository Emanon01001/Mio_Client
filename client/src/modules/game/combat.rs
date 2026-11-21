//! Targeted native attacks, performed only inside the native input update.
use super::{
    actions_build, entities,
    layout::{image, layout},
    memory::ReadBatch,
    patch, read, readable,
};
use crate::modules::view::{dot, ray_box};
use crate::signatures::{ATTACK_SIGNATURE, DERIVED_ATTACK_SIGNATURE, SWING_SIGNATURE};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Mutex, OnceLock,
};

const OWNER: u32 = 0x85B9_3800;
const ACTOR_TYPE: u32 = 0x4F6B_A419;
const GAME_TYPE: u32 = 0x88D3_EDDF;
const MAX_ENTITIES: usize = 4096;
/// Bound work and stack storage independently of the number of loaded actors.
pub(crate) const MAX_TARGETS: usize = 16;
const MODE: usize = 0xAC8;

const SWING_VTABLE_OFFSET: usize = 0x368;
// GameMode vtable+0x78 is the attack wrapper: it supplies the internal boolean
// and forwards this Vec3 pointer. Vtable+0x70 is actor interaction, not attack.
type Attack = unsafe extern "system" fn(usize, usize, *const [f32; 3]) -> u8;
type Swing = unsafe extern "system" fn(usize, u32, u8) -> u8;
static ATTACK: OnceLock<Result<usize, String>> = OnceLock::new();
static DERIVED_ATTACK: OnceLock<Result<usize, String>> = OnceLock::new();
static SWING: OnceLock<Result<usize, String>> = OnceLock::new();
static REQUESTED: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(true);
static UNTIL: AtomicU64 = AtomicU64::new(0);
static RANGE: AtomicU32 = AtomicU32::new(3f32.to_bits());
static INTERVAL: AtomicU64 = AtomicU64::new(125);
static PLAYERS: AtomicBool = AtomicBool::new(true);
static MOBS: AtomicBool = AtomicBool::new(true);
static FOV: AtomicU32 = AtomicU32::new(360f32.to_bits());
static LINE_OF_SIGHT: AtomicBool = AtomicBool::new(false);
static HOLD_ONLY: AtomicBool = AtomicBool::new(false);
static TARGET_LIMIT: AtomicUsize = AtomicUsize::new(8);
static ROUND_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static ROUND_ACCEPTED: AtomicUsize = AtomicUsize::new(0);
static TRIGGER_ON: AtomicBool = AtomicBool::new(false);
static TRIGGER_UNTIL: AtomicU64 = AtomicU64::new(0);
static TRIGGER_RANGE: AtomicU32 = AtomicU32::new(3f32.to_bits());
static TRIGGER_INTERVAL: AtomicU64 = AtomicU64::new(125);
static TRIGGER_PLAYERS: AtomicBool = AtomicBool::new(false);
static TRIGGER_MOBS: AtomicBool = AtomicBool::new(true);
static DRIVER: AtomicUsize = AtomicUsize::new(0);
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static ACCEPTED: AtomicU64 = AtomicU64::new(0);
static SWINGS: AtomicU64 = AtomicU64::new(0);
static TARGET: AtomicU32 = AtomicU32::new(u32::MAX);
static CANDIDATES: AtomicUsize = AtomicUsize::new(0);
static SCANNED: AtomicUsize = AtomicUsize::new(0);
static QUERIES: AtomicUsize = AtomicUsize::new(0);
static SCAN_US: AtomicU64 = AtomicU64::new(0);
/// Only owned scalar values reach the render thread, never an Actor pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetSnapshot {
    pub entity: u32,
    pub actor_type: u32,
    pub distance: f32,
    pub attacks: u64,
    pub accepted: u64,
    pub swings: u64,
    pub at_ms: u64,
}
impl TargetSnapshot {
    fn current(self, entity: u32, now: u64) -> bool {
        self.entity == entity && now.saturating_sub(self.at_ms) < 1000
    }
}
static TARGET_VIEW: Mutex<Option<TargetSnapshot>> = Mutex::new(None);
pub fn target_snapshot() -> Option<TargetSnapshot> {
    if !REQUESTED.load(Ordering::Acquire) && !TRIGGER_ON.load(Ordering::Acquire) {
        return None;
    }
    let snapshot = (*TARGET_VIEW.try_lock().ok()?)?;
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    snapshot
        .current(TARGET.load(Ordering::Relaxed), now)
        .then_some(snapshot)
}
static STATE: Mutex<Clock> = Mutex::new(Clock {
    world: None,
    next: 0,
});

struct Clock {
    world: Option<(usize, u32, usize)>,
    next: u64,
}
type Chosen = (u32, u32, [f32; 3]);
#[derive(Clone, Copy)]
struct Candidate {
    chosen: Chosen,
    distance: f32,
}
/// Only IDs, positions and distances survive enumeration; never Actor pointers.
struct Targets {
    entries: [Candidate; MAX_TARGETS],
    len: usize,
    limit: usize,
}
impl Targets {
    fn new(limit: usize) -> Self {
        Self {
            entries: [Candidate {
                chosen: (0, 0, [0.; 3]),
                distance: f32::INFINITY,
            }; MAX_TARGETS],
            len: 0,
            limit: limit.clamp(1, MAX_TARGETS),
        }
    }
    fn consider(&mut self, chosen: Chosen, distance: f32) {
        if !distance.is_finite()
            || distance < 0.
            || self.entries[..self.len]
                .iter()
                .any(|c| c.chosen.0 == chosen.0)
        {
            return;
        }
        let at = self.entries[..self.len].partition_point(|c| {
            c.distance < distance || (c.distance == distance && c.chosen.0 < chosen.0)
        });
        if at >= self.limit {
            return;
        }
        self.len = (self.len + 1).min(self.limit);
        self.entries.copy_within(at..self.len - 1, at + 1);
        self.entries[at] = Candidate { chosen, distance };
    }
}
struct Selection {
    distance: f32,
    target: Option<Chosen>,
}
impl Selection {
    fn new() -> Self {
        Self {
            distance: f32::INFINITY,
            target: None,
        }
    }
    fn consider(&mut self, chosen: Chosen, distance: f32, allowed: bool, trigger: bool) {
        if !allowed && !trigger {
            return;
        }
        if distance < self.distance || (distance == self.distance && (!trigger || !allowed)) {
            self.distance = distance;
            self.target = allowed.then_some(chosen);
        }
    }
}
impl Clock {
    fn due(&mut self, world: (usize, u32, usize), now: u64, interval: u64) -> bool {
        if self.world != Some(world) {
            self.world = Some(world);
            self.next = 0;
        }
        if now < self.next {
            return false;
        }
        self.next = now.saturating_add(interval);
        true
    }
    fn reset(&mut self) {
        self.world = None;
        self.next = 0;
    }
}
const fn hash(name: &str) -> u32 {
    let b = name.as_bytes();
    let mut i = 0;
    let mut h = 2166136261u32;
    while i < b.len() {
        h = (h ^ b[i] as u32).wrapping_mul(16777619);
        i += 1;
    }
    h
}
fn base_attack() -> Result<usize, String> {
    ATTACK
        .get_or_init(|| {
            if !actions_build() {
                return Err(format!(
                    "killaura requires {}",
                    crate::modules::game::SUPPORTED
                ));
            }
            patch::find(ATTACK_SIGNATURE)
        })
        .as_ref()
        .copied()
        .map_err(Clone::clone)
}
// Resolve before installing the shared helper detour: its prologue is also
// part of ATTACK_SIGNATURE. Both verified attack wrappers tail-call it.
pub(super) fn attack_helper() -> Result<usize, String> {
    let entry = base_attack()?;
    unsafe {
        if read::<u8>(entry + 6) != 0xe9 {
            return Err("attack helper jump validation failed".into());
        }
        let helper = (entry + 11).wrapping_add_signed(read::<i32>(entry + 7) as isize);
        if helper != entry + 16 {
            return Err("attack helper target validation failed".into());
        }
        Ok(helper)
    }
}
pub fn available() -> Result<(), String> {
    super::actions::available()?;
    base_attack()?;
    SWING
        .get_or_init(|| patch::find(SWING_SIGNATURE).map_err(|e| format!("arm swing: {e}")))
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)?;
    DERIVED_ATTACK
        .get_or_init(|| patch::find(DERIVED_ATTACK_SIGNATURE))
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
pub fn configure_limits(fov: f64, line_of_sight: bool, hold_only: bool) {
    FOV.store(
        (if fov.is_finite() {
            fov.clamp(1., 360.) as f32
        } else {
            360.
        })
        .to_bits(),
        Ordering::Relaxed,
    );
    LINE_OF_SIGHT.store(line_of_sight, Ordering::Relaxed);
    HOLD_ONLY.store(hold_only, Ordering::Relaxed);
}
pub fn configure_targets(limit: usize) {
    TARGET_LIMIT.store(limit.clamp(1, MAX_TARGETS), Ordering::Relaxed);
}
/// Attack reach in blocks: 1-6, 3 when not finite.
fn reach(range: f64) -> f32 {
    if range.is_finite() {
        range.clamp(1., 6.) as f32
    } else {
        3.
    }
}
/// Milliseconds between attacks at `cps` per second: 1-20, 8 when not finite.
fn attack_interval(cps: f64) -> u64 {
    (1000.
        / if cps.is_finite() {
            cps.clamp(1., 20.)
        } else {
            8.
        })
    .round() as u64
}
pub fn configure_trigger(range: f64, cps: f64, players: bool, mobs: bool) {
    TRIGGER_RANGE.store(reach(range).to_bits(), Ordering::Relaxed);
    TRIGGER_INTERVAL.store(attack_interval(cps), Ordering::Relaxed);
    TRIGGER_PLAYERS.store(players, Ordering::Relaxed);
    TRIGGER_MOBS.store(mobs, Ordering::Relaxed);
}
pub fn request_trigger(on: bool, now: u64) {
    if on {
        TRIGGER_UNTIL.store(now.saturating_add(100), Ordering::Release);
    }
    if TRIGGER_ON.load(Ordering::Acquire) == on {
        return;
    }
    let was_on = TRIGGER_ON.swap(on, Ordering::AcqRel);
    if was_on && !on && DRIVER.load(Ordering::Acquire) == 2 {
        PAUSED.store(true, Ordering::Release);
        TARGET.store(u32::MAX, Ordering::Relaxed);
        CANDIDATES.store(0, Ordering::Relaxed);
    }
}
fn driver(active: bool, aura: bool, trigger: bool, held: bool, hold_only: bool) -> usize {
    if !active {
        0
    } else if aura && (!hold_only || held) {
        1
    } else if trigger {
        2
    } else {
        0
    }
}
fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let n = dot(v, v).sqrt();
    (n.is_finite() && n > 1e-6).then(|| v.map(|x| x / n))
}
fn within_fov(eye: [f32; 3], direction: [f32; 3], bounds: [f32; 6], degrees: f32) -> bool {
    if degrees >= 360. {
        return true;
    }
    let to = std::array::from_fn(|i| (bounds[i] + bounds[i + 3]) * 0.5 - eye[i]);
    normalized(to).is_none_or(|v| dot(direction, v) >= (degrees * 0.5).to_radians().cos() - 1e-6)
}
unsafe fn attack_entry(mode: usize, player: usize) -> Option<usize> {
    if !readable(mode, 16) || read::<usize>(mode + 8) != player {
        return None;
    }
    let vt = read::<usize>(mode);
    if !readable(vt, 0x80) {
        return None;
    }
    let entry = read::<usize>(vt + 0x78);
    let base = *ATTACK.get()?.as_ref().ok()?;
    let derived = *DERIVED_ATTACK.get()?.as_ref().ok()?;
    (entry == base || entry == derived).then_some(entry)
}
/// The component pools a target is checked with: owner, type, dead and
/// removed flags, game type and metadata.
unsafe fn target_pools(memory: &mut ReadBatch, registry: usize) -> [Option<usize>; 6] {
    entities::component_pools(
        memory,
        registry,
        [
            OWNER,
            ACTOR_TYPE,
            hash("IsDeadFlagComponent"),
            hash("ActorRemovedFlagComponent"),
            GAME_TYPE,
            super::metadata::HASH,
        ],
    )
}
pub fn configure(range: f32, cps: f64, players: bool, mobs: bool) {
    RANGE.store(reach(f64::from(range)).to_bits(), Ordering::Relaxed);
    INTERVAL.store(attack_interval(cps), Ordering::Relaxed);
    PLAYERS.store(players, Ordering::Relaxed);
    MOBS.store(mobs, Ordering::Relaxed);
}
pub fn request(on: bool, now: u64) {
    if on {
        UNTIL.store(now.saturating_add(100), Ordering::Release);
    }
    if REQUESTED.load(Ordering::Acquire) == on {
        return;
    }
    let was_on = REQUESTED.swap(on, Ordering::AcqRel);
    if was_on && !on {
        PAUSED.store(true, Ordering::Release);
        TARGET.store(u32::MAX, Ordering::Relaxed);
        CANDIDATES.store(0, Ordering::Relaxed);
    }
}
fn eligible(kind: u32, players: bool, mobs: bool) -> bool {
    kind & 0x100 != 0 && if kind & 0xFF == 63 { players } else { mobs }
}
fn hit(eye: [f32; 3], bounds: [f32; 6]) -> Option<([f32; 3], f32)> {
    if !eye.iter().chain(bounds.iter()).all(|v| v.is_finite())
        || !(0..3).all(|k| bounds[k + 3] > bounds[k] && bounds[k + 3] - bounds[k] < 64.)
    {
        return None;
    }
    let point = std::array::from_fn(|k| eye[k].clamp(bounds[k], bounds[k + 3]));
    let distance = (0..3).map(|k| (eye[k] - point[k]).powi(2)).sum();
    Some((point, distance))
}
fn aimed_hit(
    eye: [f32; 3],
    dir: [f32; 3],
    bounds: [f32; 6],
    range: f32,
    trigger: bool,
    fov: f32,
) -> Option<([f32; 3], f32)> {
    let base = hit(eye, bounds)?;
    if trigger {
        let distance = ray_box(eye, dir, bounds, range)?;
        Some((
            std::array::from_fn(|i| eye[i] + dir[i] * distance),
            distance * distance,
        ))
    } else {
        within_fov(eye, dir, bounds, fov).then_some(base)
    }
}
unsafe fn actor(
    registry: usize,
    entity: u32,
    kind: u32,
    players: bool,
    mobs: bool,
) -> Option<usize> {
    let mut memory = ReadBatch::new();
    let [owner, types, dead, removed, game_types, metadata] = target_pools(&mut memory, registry);
    let owner = entities::PoolView::new(&mut memory, owner?, 8)?;
    let types = entities::PoolView::new(&mut memory, types?, 4)?;
    if read::<u32>(types.component(&mut memory, entity)?) != kind || !eligible(kind, players, mobs)
    {
        return None;
    }
    for pool in [dead, removed].into_iter().flatten() {
        if entities::PoolView::new(&mut memory, pool, 0)?.contains(&mut memory, entity) {
            return None;
        }
    }
    let actor = read::<usize>(owner.component(&mut memory, entity)?);
    if !memory.readable(actor, 0x250) || !actor_matches(actor, registry, entity) {
        return None;
    }
    if kind & 0xff == 63 {
        let game_type = game_types
            .and_then(|p| entities::PoolView::new(&mut memory, p, 4))
            .and_then(|p| p.component(&mut memory, entity))
            .map(|p| read::<i32>(p));
        if matches!(game_type, Some(1 | 6)) {
            return None;
        }
        if let Some(names) = super::friends::snapshot() {
            let name = metadata
                .and_then(|p| super::metadata::View::new(&mut memory, p))
                .and_then(|v| v.name(&mut memory, entity));
            if super::friends::excludes(&names, name.as_deref()) {
                return None;
            }
        }
    }
    Some(actor)
}
unsafe fn actor_matches(actor: usize, registry: usize, entity: u32) -> bool {
    if read::<usize>(actor + 0x10) != registry || read::<u32>(actor + 0x18) != entity {
        return false;
    }
    let (base, end) = image();
    let vt = read::<usize>(actor);
    (base..end).contains(&vt)
}
unsafe fn same_world(player: usize, extent: usize, world: (usize, u32, usize)) -> bool {
    readable(player, extent)
        && read::<usize>(player + 0x10) == world.0
        && read::<u32>(player + 0x18) == world.1
        && read::<usize>(player + 0x1f0) == world.2
}
fn action_allowed(source: usize, now: u64) -> bool {
    let (on, until) = if source == 2 {
        (&TRIGGER_ON, &TRIGGER_UNTIL)
    } else {
        (&REQUESTED, &UNTIL)
    };
    on.load(Ordering::Acquire)
        && now <= until.load(Ordering::Acquire)
        && !crate::gui::captures_input()
        && !super::inventory::screen_open()
        && crate::input::is_foreground()
}
/// Returns true while aura owns repeated attacks, so autoclicker does not stack.
pub(super) unsafe fn sample(
    instance: usize,
    player: usize,
    active: bool,
    held: bool,
    now: u64,
) -> bool {
    // The default/off path performs no locks, pool walks or memory queries.
    if !REQUESTED.load(Ordering::Acquire) && !TRIGGER_ON.load(Ordering::Acquire) {
        return false;
    }
    let selected = driver(
        active && player != 0,
        REQUESTED.load(Ordering::Acquire) && now <= UNTIL.load(Ordering::Acquire),
        TRIGGER_ON.load(Ordering::Acquire) && now <= TRIGGER_UNTIL.load(Ordering::Acquire),
        held,
        HOLD_ONLY.load(Ordering::Relaxed),
    );
    if selected == 0 {
        PAUSED.store(true, Ordering::Release);
        TARGET.store(u32::MAX, Ordering::Relaxed);
        CANDIDATES.store(0, Ordering::Relaxed);
        return false;
    }
    let Ok(mut clock) = STATE.try_lock() else {
        return true;
    };
    let changed = DRIVER.swap(selected, Ordering::AcqRel) != selected;
    if PAUSED.swap(false, Ordering::AcqRel) || changed {
        clock.reset();
    }
    let Some(Ok(_)) = ATTACK.get().map(|r| r.as_ref()) else {
        return false;
    };
    let Some(Ok(&swing_entry)) = SWING.get().map(|r| r.as_ref()) else {
        return false;
    };
    let Ok(l) = layout() else {
        return false;
    };
    if !readable(player, l.actor_state.max(MODE) + 8) {
        return true;
    }
    let registry = read::<usize>(player + 0x10);
    let own = read::<u32>(player + 0x18);
    let dimension = read::<usize>(player + 0x1F0);
    if !clock.due(
        (registry, own, dimension),
        now,
        (if selected == 2 {
            &TRIGGER_INTERVAL
        } else {
            &INTERVAL
        })
        .load(Ordering::Relaxed),
    ) {
        return true;
    }
    TARGET.store(u32::MAX, Ordering::Relaxed);
    CANDIDATES.store(0, Ordering::Relaxed);
    ROUND_ATTEMPTS.store(0, Ordering::Relaxed);
    ROUND_ACCEPTED.store(0, Ordering::Relaxed);
    let mode = read::<usize>(player + MODE);
    if attack_entry(mode, player).is_none() {
        return true;
    }
    let player_vt = read::<usize>(player);
    if !readable(player_vt, SWING_VTABLE_OFFSET + 8)
        || read::<usize>(player_vt + SWING_VTABLE_OFFSET) != swing_entry
    {
        return true;
    }
    let state = read::<usize>(player + l.actor_state);
    if !readable(state, 12) {
        return true;
    }
    let eye = read::<[f32; 3]>(state);
    let range = f32::from_bits(
        (if selected == 2 {
            &TRIGGER_RANGE
        } else {
            &RANGE
        })
        .load(Ordering::Relaxed),
    );
    let fov = f32::from_bits(FOV.load(Ordering::Relaxed));
    let line_of_sight = selected == 2 || LINE_OF_SIGHT.load(Ordering::Relaxed);
    let direction = if selected == 2 || fov < 360. {
        if !readable(instance + l.view_matrix, 48) {
            return true;
        }
        let v = read::<[f32; 12]>(instance + l.view_matrix);
        let Some(dir) = normalized([-v[8], -v[9], -v[10]]) else {
            return true;
        };
        dir
    } else {
        [0.; 3]
    };
    let (players, mobs) = (
        (if selected == 2 {
            &TRIGGER_PLAYERS
        } else {
            &PLAYERS
        })
        .load(Ordering::Relaxed),
        (if selected == 2 { &TRIGGER_MOBS } else { &MOBS }).load(Ordering::Relaxed),
    );
    let start = std::time::Instant::now();
    let chosen = {
        // A pool view and its protection cache live only for this enumeration.
        // Drop all of them before resolving the selected Actor again and calling
        // native code, which may change the world or allocate/free memory.
        let mut memory = ReadBatch::new();
        let friends = super::friends::snapshot();
        let [owner, types, dead, removed, game_types, metadata] =
            target_pools(&mut memory, registry);
        let (Some(owner), Some(types)) = (
            owner.and_then(|p| entities::PoolView::new(&mut memory, p, 8)),
            types.and_then(|p| entities::PoolView::new(&mut memory, p, 4)),
        ) else {
            return true;
        };
        let dead = dead.and_then(|p| entities::PoolView::new(&mut memory, p, 0));
        let removed = removed.and_then(|p| entities::PoolView::new(&mut memory, p, 0));
        let game_types = game_types.and_then(|p| entities::PoolView::new(&mut memory, p, 4));
        let metadata = friends
            .as_ref()
            .and(metadata)
            .and_then(|p| super::metadata::View::new(&mut memory, p));
        let (mut selection, mut candidates, mut scanned) = (Selection::new(), 0usize, 0usize);
        let mut targets = Targets::new(if selected == 2 {
            1
        } else {
            TARGET_LIMIT.load(Ordering::Relaxed)
        });
        for slot in owner.entities().step_by(4).take(MAX_ENTITIES) {
            scanned += 1;
            let entity = read::<u32>(slot);
            if entity == own
                || dead
                    .as_ref()
                    .is_some_and(|p| p.contains(&mut memory, entity))
                || removed
                    .as_ref()
                    .is_some_and(|p| p.contains(&mut memory, entity))
            {
                continue;
            }
            let Some(kind) = types.component(&mut memory, entity).map(|p| read::<u32>(p)) else {
                continue;
            };
            let mut allowed = eligible(kind, players, mobs);
            if !allowed && selected != 2 {
                continue;
            }
            if kind & 0xFF == 63 {
                let game_type = game_types
                    .as_ref()
                    .and_then(|p| p.component(&mut memory, entity))
                    .map(|p| read::<i32>(p));
                if game_type == Some(6) {
                    continue;
                }
                if game_type == Some(1) {
                    allowed = false;
                }
                if !allowed && selected != 2 {
                    continue;
                }
            }
            let Some(target) = owner
                .component(&mut memory, entity)
                .map(|p| read::<usize>(p))
            else {
                continue;
            };
            if !memory.readable(target, 0x250) || !actor_matches(target, registry, entity) {
                continue;
            }
            let bounds = read::<usize>(target + l.actor_aabb);
            if !memory.readable(bounds, 24) {
                continue;
            }
            let Some((point, distance)) =
                aimed_hit(eye, direction, read(bounds), range, selected == 2, fov)
            else {
                continue;
            };
            if distance > range * range {
                continue;
            }
            if kind & 0xff == 63 {
                if let Some(names) = &friends {
                    let name = metadata.as_ref().and_then(|v| v.name(&mut memory, entity));
                    allowed &= !super::friends::excludes(names, name.as_deref());
                }
            }
            if selected == 2 {
                // A friend/excluded entity under the crosshair must still
                // occlude TriggerBot's target behind it.
                selection.consider((entity, kind, point), distance, allowed, true);
            } else if allowed {
                targets.consider((entity, kind, point), distance);
            }
            if allowed && distance <= range * range {
                candidates += 1;
            }
        }
        CANDIDATES.store(candidates, Ordering::Relaxed);
        SCANNED.store(scanned, Ordering::Relaxed);
        QUERIES.store(memory.queries(), Ordering::Relaxed);
        if selected == 2 {
            if let Some(chosen) = selection.target {
                targets.consider(chosen, selection.distance);
            }
        }
        targets
    };
    SCAN_US.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    let mut primary = None;
    let mut swing_attempted = false;
    for candidate in &chosen.entries[..chosen.len] {
        let (entity, kind, point) = candidate.chosen;
        let checked_point = point;
        let world = (registry, own, dimension);
        if !action_allowed(
            selected,
            windows_sys::Win32::System::SystemInformation::GetTickCount64(),
        ) || !same_world(player, l.actor_state.max(MODE) + 8, world)
        {
            break;
        }
        if line_of_sight && !super::blocks::visible(player, eye, point) {
            continue;
        }
        // Resolve the generation/owner and current bounds again immediately
        // before the call. The previous scan only retained an integer entity ID.
        if !action_allowed(
            selected,
            windows_sys::Win32::System::SystemInformation::GetTickCount64(),
        ) {
            break;
        }
        // Native attacks can remove entities or change the scene. Resolve
        // ownership, generation, live flags and Friends again for every call;
        // no pool view or memory-protection cache survives a native call.
        if !same_world(player, l.actor_state.max(MODE) + 8, world) {
            break;
        }
        let mode = read::<usize>(player + MODE);
        let Some(entry) = attack_entry(mode, player) else {
            break;
        };
        let Some(target) = actor(registry, entity, kind, players, mobs) else {
            continue;
        };
        let bounds = read::<usize>(target + l.actor_aabb);
        if !readable(bounds, 24) {
            continue;
        }
        let state = read::<usize>(player + l.actor_state);
        if !readable(state, 12) {
            break;
        }
        let current_eye = read::<[f32; 3]>(state);
        let Some((point, distance)) = aimed_hit(
            current_eye,
            direction,
            read(bounds),
            range,
            selected == 2,
            fov,
        ) else {
            continue;
        };
        if line_of_sight
            && (0..3).any(|i| {
                (point[i] - checked_point[i]).abs() > 1e-3 || (current_eye[i] - eye[i]).abs() > 1e-3
            })
        {
            continue;
        }
        if distance > range * range {
            continue;
        }
        if primary.is_none() {
            TARGET.store(entity, Ordering::Relaxed);
            primary = Some(TargetSnapshot {
                entity,
                actor_type: kind,
                distance: distance.sqrt(),
                attacks: 0,
                accepted: 0,
                swings: 0,
                at_ms: now,
            });
        }
        let attack: Attack = std::mem::transmute(entry);
        let first = ATTEMPTS.fetch_add(1, Ordering::Relaxed) == 0;
        ROUND_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        let accepted = super::weapon::automated(|| attack(mode, target, &point)) != 0;
        if accepted {
            ACCEPTED.fetch_add(1, Ordering::Relaxed);
            ROUND_ACCEPTED.fetch_add(1, Ordering::Relaxed);
            // Vanilla invokes the arm animation separately from GameMode's
            // attack transaction. Preserve its own duration/restart checks.
            if !swing_attempted {
                swing_attempted = true;
                if same_world(player, 0x250, world) && actor_matches(player, registry, own) {
                    let vt = read::<usize>(player);
                    if readable(vt, SWING_VTABLE_OFFSET + 8)
                        && read::<usize>(vt + SWING_VTABLE_OFFSET) == swing_entry
                    {
                        let swing: Swing = std::mem::transmute(swing_entry);
                        if swing(player, 4, 0) != 0 {
                            SWINGS.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        if first {
            crate::log!("combat: first native attack entity {entity:#X}, distance {:.2}, accepted={accepted}", distance.sqrt());
        }
    }
    if let Some(mut primary) = primary {
        primary.attacks = ATTEMPTS.load(Ordering::Relaxed);
        primary.accepted = ACCEPTED.load(Ordering::Relaxed);
        primary.swings = SWINGS.load(Ordering::Relaxed);
        *TARGET_VIEW.lock().unwrap_or_else(|e| e.into_inner()) = Some(primary);
    }
    true
}
pub fn diagnostic() -> String {
    let target = TARGET.load(Ordering::Relaxed);
    let source = if PAUSED.load(Ordering::Acquire) {
        "paused"
    } else {
        match DRIVER.load(Ordering::Acquire) {
            1 => "KillAura",
            2 => "TriggerBot",
            _ => "none",
        }
    };
    format!(
        "combat ({source}): {} targets, selected {}, attacks {} / accepted {} / swings {}; last scan {} actors / {} memory queries / {}us; last cycle attempts={} accepted={} limit={}",
        CANDIDATES.load(Ordering::Relaxed),
        if target == u32::MAX {
            "none".into()
        } else {
            format!("{target:#X}")
        },
        ATTEMPTS.load(Ordering::Relaxed),
        ACCEPTED.load(Ordering::Relaxed),
        SWINGS.load(Ordering::Relaxed),
        SCANNED.load(Ordering::Relaxed),
        QUERIES.load(Ordering::Relaxed),
        SCAN_US.load(Ordering::Relaxed),
        ROUND_ATTEMPTS.load(Ordering::Relaxed),
        ROUND_ACCEPTED.load(Ordering::Relaxed),
        if DRIVER.load(Ordering::Relaxed) == 2 { 1 } else { TARGET_LIMIT.load(Ordering::Relaxed) }
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_and_attack_interval_keep_their_ranges_and_defaults() {
        assert_eq!(reach(3.5), 3.5);
        assert_eq!((reach(0.2), reach(9.)), (1., 6.));
        // Non-finite falls back; a huge finite value still clamps (as f64).
        assert_eq!((reach(f64::NAN), reach(f64::INFINITY)), (3., 3.));
        assert_eq!(reach(1e300), 6.);
        assert_eq!(reach(f64::from(4.25f32)), 4.25);
        assert_eq!(attack_interval(8.), 125);
        assert_eq!((attack_interval(0.5), attack_interval(40.)), (1000, 50));
        assert_eq!(attack_interval(f64::NAN), 125);
        assert_eq!(attack_interval(12.), 83);
    }
    #[test]
    fn revalidation_rejects_removed_dead_recycled_and_changed_targets_between_attacks() {
        let mut registry = [0usize; 15];
        let mut target = [0usize; 0x250 / 8];
        target[0] = image().0;
        target[2] = registry.as_ptr() as usize;
        target[3] = 0x40001;
        let mut ids = [0x40001u32];
        let mut sparse = [u32::MAX; 2048];
        sparse[1] = 0x40000;
        let mut flags = [u32::MAX; 2048];
        let sparse_pages = [sparse.as_ptr() as usize];
        let flag_pages = [flags.as_ptr() as usize];
        let mut owners = [target.as_ptr() as usize; 128];
        let mut kinds = [0xb0bu32; 128];
        let owner_pages = [owners.as_ptr() as usize];
        let kind_pages = [kinds.as_ptr() as usize];
        let mut owner = [0usize; 12];
        owner[1] = sparse_pages.as_ptr() as usize;
        owner[2] = owner[1] + 8;
        owner[4] = ids.as_ptr() as usize;
        owner[5] = owner[4] + 4;
        owner[10] = owner_pages.as_ptr() as usize;
        owner[11] = owner[10] + 8;
        let mut types = owner;
        types[10] = kind_pages.as_ptr() as usize;
        types[11] = types[10] + 8;
        let mut dead = owner;
        dead[1] = flag_pages.as_ptr() as usize;
        dead[2] = dead[1] + 8;
        dead[10] = 0;
        dead[11] = 0;
        let mut nodes = [
            [0usize, OWNER as usize, owner.as_ptr() as usize, 0],
            [0, ACTOR_TYPE as usize, types.as_ptr() as usize, 0],
            [
                0,
                hash("IsDeadFlagComponent") as usize,
                dead.as_ptr() as usize,
                0,
            ],
        ];
        registry[13] = nodes.as_ptr() as usize;
        registry[14] = registry[13] + std::mem::size_of_val(&nodes);
        let resolve = || unsafe { actor(registry.as_ptr() as usize, 0x40001, 0xb0b, false, true) };
        assert_eq!(resolve(), Some(target.as_ptr() as usize));
        // Emulate mutations made by intervening native attacks; every lookup
        // must read these buffers again rather than reuse a cached pool view.
        unsafe {
            kinds.as_mut_ptr().write(0x13f);
            assert_eq!(resolve(), None);
            kinds.as_mut_ptr().write(0xb0b);
            owners.as_mut_ptr().write(0);
            assert_eq!(resolve(), None);
            owners.as_mut_ptr().write(target.as_ptr() as usize);
            ids.as_mut_ptr().write(0x80001);
            assert_eq!(resolve(), None);
            ids.as_mut_ptr().write(0x40001);
            flags.as_mut_ptr().add(1).write(0x40000);
            assert_eq!(resolve(), None);
            nodes[2]
                .as_mut_ptr()
                .add(1)
                .write(hash("ActorRemovedFlagComponent") as usize);
            assert_eq!(resolve(), None);
            flags.as_mut_ptr().add(1).write(u32::MAX);
            assert_eq!(resolve(), Some(target.as_ptr() as usize));
            target.as_mut_ptr().add(3).write(0x80001);
            assert_eq!(resolve(), None);
        }
    }
    #[test]
    fn a_world_change_stops_the_old_player_context() {
        let mut player = [0usize; (MODE + 8) / 8];
        player[2] = 0x1230;
        player[3] = 0x40001;
        player[0x1f0 / 8] = 0x4560;
        let world = (0x1230, 0x40001, 0x4560);
        unsafe {
            assert!(same_world(player.as_ptr() as usize, MODE + 8, world));
            player[0x1f0 / 8] = 0x5670;
            assert!(!same_world(player.as_ptr() as usize, MODE + 8, world));
            player[0x1f0 / 8] = 0x4560;
            player[3] = 0x80001;
            assert!(!same_world(player.as_ptr() as usize, MODE + 8, world));
        }
    }
    #[test]
    fn multi_target_selection_is_bounded_nearest_first_and_single_target_is_supported() {
        for limit in [0, 1, 3, 8, MAX_TARGETS, usize::MAX] {
            for reverse in [false, true] {
                let mut selection = Targets::new(limit);
                let order: Vec<_> = if reverse {
                    (0..128).rev().collect()
                } else {
                    (0..128).collect()
                };
                for entity in order {
                    selection.consider((entity, 0xb0b, [0.; 3]), entity as f32 + 0.5);
                }
                let limit = limit.clamp(1, MAX_TARGETS);
                assert_eq!(selection.len, limit);
                assert_eq!(
                    selection.entries[..selection.len]
                        .iter()
                        .map(|c| c.chosen.0)
                        .collect::<Vec<_>>(),
                    (0..limit as u32).collect::<Vec<_>>()
                );
            }
        }
    }
    #[test]
    fn multiple_targets_are_distinct_ties_are_stable_and_invalid_distances_are_rejected() {
        let mut selection = Targets::new(8);
        for entity in [4, 2, 1, 3] {
            selection.consider((entity, 0xb0b, [0.; 3]), 1.);
        }
        selection.consider((1, 0xb0b, [0.; 3]), 1.);
        for distance in [f32::NAN, f32::INFINITY, -1.] {
            selection.consider((9, 0xb0b, [0.; 3]), distance);
        }
        assert_eq!(selection.len, 4);
        assert_eq!(
            selection.entries[..selection.len]
                .iter()
                .map(|c| c.chosen.0)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }
    #[test]
    fn automation_priority_and_hold_gate_never_enable_actions_in_menus() {
        assert_eq!(driver(false, true, true, true, false), 0);
        assert_eq!(driver(true, true, true, false, false), 1);
        assert_eq!(driver(true, true, true, false, true), 2);
        assert_eq!(driver(true, true, false, false, true), 0);
        assert_eq!(driver(true, true, true, true, true), 1);
        assert_eq!(driver(true, false, true, false, false), 2);
        assert_eq!(driver(true, false, false, true, false), 0);
    }
    #[test]
    fn trigger_requires_ray_intersection_and_aura_respects_fov() {
        let eye = [0., 1., 0.];
        let dir = [0., 0., 1.];
        let front = [-0.5, 0., 3., 0.5, 2., 4.];
        let side = [2., 0., 0., 3., 2., 1.];
        let behind = [-0.5, 0., -3., 0.5, 2., -2.];
        assert_eq!(
            aimed_hit(eye, dir, front, 3., true, 360.),
            Some(([0., 1., 3.], 9.))
        );
        assert!(aimed_hit(eye, dir, front, 2.99, true, 360.).is_none());
        assert!(aimed_hit(eye, dir, side, 6., true, 360.).is_none());
        assert!(aimed_hit(eye, dir, behind, 6., true, 360.).is_none());
        assert!(aimed_hit(eye, dir, side, 6., false, 90.).is_none());
        assert!(aimed_hit(eye, dir, front, 6., false, 90.).is_some());
        assert!(aimed_hit(eye, dir, behind, 6., false, 360.).is_some());
        assert!(normalized([f32::NAN, 0., 1.]).is_none());
        assert!(ray_box(eye, [0.; 3], front, 6.).is_none());
        assert!(ray_box(eye, [f32::NAN, 0., 1.], front, 6.).is_none());
    }
    #[test]
    fn excluded_front_entity_occludes_trigger_targets_in_both_enumeration_orders() {
        let target = (1, 0xB0B, [0., 1., 3.]);
        let excluded = (2, 0x13F, [0., 1., 2.]);
        let mut selection = Selection::new();
        selection.consider(target, 9., true, true);
        selection.consider(excluded, 4., false, true);
        assert!(selection.target.is_none());
        selection.consider(target, 9., true, true);
        assert!(selection.target.is_none());
        let mut selection = Selection::new();
        selection.consider(excluded, 4., false, true);
        selection.consider(target, 9., true, true);
        assert!(selection.target.is_none());
        let mut selection = Selection::new();
        selection.consider(target, 9., true, true);
        assert_eq!(selection.target, Some(target));
        selection.consider(excluded, 9., false, true);
        assert!(selection.target.is_none());
        selection.consider(target, 9., true, true);
        assert!(selection.target.is_none());
        let mut aura = Selection::new();
        aura.consider(excluded, 4., false, false);
        aura.consider(target, 9., true, false);
        assert_eq!(aura.target, Some(target));
    }
    #[test]
    fn target_view_expires_and_never_follows_an_old_selected_entity() {
        let s = TargetSnapshot {
            entity: 1,
            actor_type: 0x13F,
            distance: 2.,
            attacks: 5,
            accepted: 5,
            swings: 2,
            at_ms: 100,
        };
        assert!(s.current(1, 200));
        assert!(!s.current(2, 200));
        assert!(!s.current(u32::MAX, 200));
        assert!(!s.current(1, 1100));
    }
    #[test]
    fn only_living_actor_types_are_targets() {
        assert!(eligible(0x13F, true, false)); // player
        assert!(!eligible(0x13F, false, true));
        assert!(eligible(0xB0B, false, true)); // cow
        assert!(!eligible(0xB0B, true, false));
        for kind in [0x40, 0x800050, 0] {
            assert!(!eligible(kind, true, true));
        }
        assert_eq!(hash("ActorOwnerComponent"), OWNER);
    }
    #[test]
    fn reach_uses_the_nearest_point_of_a_sane_box() {
        let b = [3., 0., -0.5, 4., 2., 0.5];
        assert_eq!(hit([0., 1., 0.], b), Some(([3., 1., 0.], 9.)));
        assert_eq!(hit([3.5, 1., 0.], b), Some(([3.5, 1., 0.], 0.)));
        assert!(hit([0.; 3], [0.; 6]).is_none());
        assert!(hit([f32::NAN; 3], b).is_none());
    }
    #[test]
    fn clock_never_bursts_and_resets_on_world_change_and_pause() {
        let mut c = Clock {
            world: None,
            next: 0,
        };
        let w = (1, 2, 3);
        assert!(c.due(w, 100, 125));
        assert!(!c.due(w, 200, 125));
        assert!(c.due(w, 2000, 125));
        assert!(!c.due(w, 2000, 125));
        assert!(c.due((1, 4, 3), 2001, 125));
        c.reset();
        assert!(c.due(w, 2002, 125));
    }
}
