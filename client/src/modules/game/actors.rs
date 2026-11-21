//! Per-entity facts read from the game's entity components: what each entity
//! is, its health, and the local player's hunger and experience. Read-only,
//! on the game thread. See analysis/notes/actors.md for how the layouts were found.
use super::entities::{component_pools, PoolView};
use super::memory::ReadBatch;
use super::{acquire, known_build, read, release};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

/// FNV-1a (32-bit) of a component's type name: the id its pool is registered
/// under in the entity registry.
pub const fn type_hash(name: &str) -> u32 {
    let bytes = name.as_bytes();
    let mut hash = 0x811C_9DC5u32;
    let mut i = 0;
    while i < bytes.len() {
        hash = (hash ^ bytes[i] as u32).wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

pub(super) const ATTRIBUTES: u32 = type_hash("AttributesComponent");
const IDENTIFIER: u32 = type_hash("ActorDefinitionIdentifierComponent");
const ACTOR_TYPE: u32 = type_hash("ActorTypeComponent");
pub(super) const AABB: u32 = type_hash("AABBShapeComponent");
pub(super) const UNIQUE_ID: u32 = type_hash("ActorUniqueIDComponent");

/// AttributesComponent = { ids: vector<u32>, values: vector<AttributeInstance>, ... }
pub(super) const ATTRIBUTES_SIZE: usize = 0x50;
const ATTRIBUTE_VALUES: usize = 0x18;
/// One AttributeInstance, and its current { min, max, value } floats.
const ATTRIBUTE_STRIDE: usize = 0x88;
const ATTRIBUTE_CURRENT: usize = 0x74;
const MAX_ATTRIBUTES: usize = 64;
/// ActorDefinitionIdentifierComponent = { namespace, identifier, ... } strings.
const IDENTIFIER_SIZE: usize = 0xB0;
const IDENTIFIER_NAME: usize = 0x20;

/// Attribute ids in the analyzed build.
pub mod attribute {
    pub const HUNGER: u32 = 1;
    pub const SATURATION: u32 = 2;
    pub const EXHAUSTION: u32 = 3;
    pub const LEVEL: u32 = 4;
    pub const EXPERIENCE: u32 = 5;
    pub const HEALTH: u32 = 6;
    pub const ABSORPTION: u32 = 13;
}

/// The snapshot is refreshed this often while someone uses it.
const INTERVAL_MS: u64 = 100;
/// A snapshot older than this is not reported (the HUD hook stopped).
const MAX_AGE_MS: u64 = 1000;
/// Upper bound on entities described per snapshot.
const MAX_ACTORS: usize = 1024;

/// What one entity is.
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    /// The identifier without its namespace: `pig`, `zombie`, `player`, `item`.
    pub name: Arc<str>,
    /// The current displayed name, when the actor metadata has one.
    pub display_name: Option<Arc<str>>,
    /// The game's numeric actor type (low byte: the entity kind).
    pub actor_type: u32,
    /// Current and maximum health, for entities that have health.
    pub health: Option<(f32, f32)>,
    /// A copied world-space hitbox, never a pointer into the game.
    pub bounds: Option<super::Aabb>,
    pub local: bool,
}

/// The local player's own numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vitals {
    pub health: f32,
    pub max_health: f32,
    pub absorption: f32,
    pub hunger: f32,
    /// The hidden buffer that is used up before hunger starts to drop.
    pub saturation: f32,
    pub exhaustion: f32,
    pub level: f32,
    /// Progress to the next level, 0-1.
    pub experience: f32,
}

struct Snapshot {
    at: u64,
    owner: (usize, u32),
    actors: Arc<HashMap<u32, Actor>>,
    vitals: Option<Vitals>,
    surrounding: bool,
}
impl Snapshot {
    fn for_owner(&self, owner: (usize, u32)) -> Option<Arc<HashMap<u32, Actor>>> {
        (self.surrounding && self.owner == owner).then(|| Arc::clone(&self.actors))
    }
}

/// Modules currently using the snapshot.
static USERS: AtomicUsize = AtomicUsize::new(0);
static VITAL_USERS: AtomicUsize = AtomicUsize::new(0);
static LAST_SAMPLE: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
// The HUD accessor may be evaluated concurrently. Never duplicate a scan or
// make a game thread wait for another producer's native enumeration.
static SAMPLING: Mutex<()> = Mutex::new(());
/// Entity id (which includes its generation) -> name, so each name is read once.
struct CachedName {
    identifier: Arc<str>,
    display: Option<Arc<str>>,
}
type NameCache = (usize, HashMap<u32, CachedName>);
static NAMES: LazyLock<Mutex<NameCache>> = LazyLock::new(|| Mutex::new((0, HashMap::new())));

/// The current { min, max, value } of attribute `id` in an attribute map.
unsafe fn attribute(memory: &mut ReadBatch, component: usize, id: u32) -> Option<[f32; 3]> {
    let (keys, values, count) = attribute_table(memory, component)?;
    let index = (0..count).find(|&i| read::<u32>(keys + i * 4) == id)?;
    let current: [f32; 3] = read(values + index * ATTRIBUTE_STRIDE + ATTRIBUTE_CURRENT);
    current.iter().all(|v| v.is_finite()).then_some(current)
}
unsafe fn attribute_table(
    memory: &mut ReadBatch,
    component: usize,
) -> Option<(usize, usize, usize)> {
    let (keys, keys_end) = memory.span(component, 4)?;
    let (values, values_end) = memory.span(component.checked_add(ATTRIBUTE_VALUES)?, 8)?;
    let count = (keys_end - keys) / 4;
    // The layout is only known for this exact element size.
    if count == 0 || count > MAX_ATTRIBUTES || values_end - values != count * ATTRIBUTE_STRIDE {
        return None;
    }
    Some((keys, values, count))
}

/// Current and maximum health from an attribute map.
pub(super) unsafe fn health(memory: &mut ReadBatch, component: usize) -> Option<(f32, f32)> {
    let [_, max, value] = attribute(memory, component, attribute::HEALTH)?;
    (max > 0.0 && value >= 0.0 && value <= max).then_some((value, max))
}

/// Local health sampled even while a death screen prevents the HUD callback.
pub(super) unsafe fn own_health(player: usize) -> Option<(f32, f32)> {
    let mut m = ReadBatch::new();
    if !m.readable(player, 0x1c) {
        return None;
    }
    let registry = read(player + 0x10);
    let entity = read(player + 0x18);
    let pool = component_pools(&mut m, registry, [ATTRIBUTES])[0]?;
    let component = PoolView::new(&mut m, pool, ATTRIBUTES_SIZE)?.component(&mut m, entity)?;
    health(&mut m, component)
}

unsafe fn vitals(memory: &mut ReadBatch, component: usize) -> Option<Vitals> {
    let (keys, values, count) = attribute_table(memory, component)?;
    let mut found = [None; 14];
    for i in 0..count {
        let id = read::<u32>(keys + i * 4) as usize;
        if !matches!(id, 1..=6 | 13) || found[id].is_some() {
            continue;
        }
        let current: [f32; 3] = read(values + i * ATTRIBUTE_STRIDE + ATTRIBUTE_CURRENT);
        if current.iter().any(|v| !v.is_finite()) {
            return None;
        }
        found[id] = Some(current);
    }
    let [_, max_health, health] = found[attribute::HEALTH as usize]?;
    if max_health <= 0. || health < 0. || health > max_health {
        return None;
    }
    let value = |id: u32| found[id as usize].map(|v| v[2]);
    Some(Vitals {
        health,
        max_health,
        absorption: value(attribute::ABSORPTION)?,
        hunger: value(attribute::HUNGER)?,
        saturation: value(attribute::SATURATION)?,
        exhaustion: value(attribute::EXHAUSTION)?,
        level: value(attribute::LEVEL)?,
        experience: value(attribute::EXPERIENCE)?,
    })
}

/// A plausible entity identifier: short, lower-case words.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | ':'))
}

unsafe fn collect(
    player: usize,
    surrounding: bool,
) -> Option<(HashMap<u32, Actor>, Option<Vitals>)> {
    let mut memory = ReadBatch::new();
    if !memory.readable(player, 0x1C) {
        return None;
    }
    let (registry, own): (usize, u32) = (read(player + 0x10), read(player + 0x18));
    if !surrounding {
        // Local health/hunger/XP needs one component lookup, never an entity
        // enumeration, identifier strings, metadata, types or hitboxes.
        let own_vitals = component_pools(&mut memory, registry, [ATTRIBUTES])[0]
            .and_then(|pool| PoolView::new(&mut memory, pool, ATTRIBUTES_SIZE))
            .and_then(|attributes| attributes.component(&mut memory, own))
            .and_then(|map| vitals(&mut memory, map));
        return Some((HashMap::new(), own_vitals));
    }
    let [identifiers, attributes, types, boxes, meta] = component_pools(
        &mut memory,
        registry,
        [
            IDENTIFIER,
            ATTRIBUTES,
            ACTOR_TYPE,
            AABB,
            super::metadata::HASH,
        ],
    );
    let identifiers = PoolView::new(&mut memory, identifiers?, IDENTIFIER_SIZE)?;
    let attributes = match attributes {
        Some(pool) => PoolView::new(&mut memory, pool, ATTRIBUTES_SIZE),
        None => None,
    };
    let types = match types {
        Some(pool) => PoolView::new(&mut memory, pool, 4),
        None => None,
    };
    let boxes = boxes.and_then(|pool| PoolView::new(&mut memory, pool, 0x20));
    let meta = meta.and_then(|pool| super::metadata::View::new(&mut memory, pool));
    // Read the local player directly; it can be beyond the enumeration limit.
    let own_vitals = attributes
        .as_ref()
        .and_then(|view| view.component(&mut memory, own))
        .and_then(|map| vitals(&mut memory, map));

    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if names.0 != registry || names.1.len() > 8192 {
        *names = (registry, HashMap::new());
    }
    let mut actors = HashMap::new();
    for slot in identifiers.entities().step_by(4).take(8192) {
        if actors.len() == MAX_ACTORS {
            break;
        }
        let entity: u32 = read(slot);
        // Rejects tombstones and recycled ids, as the game's own lookup does.
        let Some(identifier) = identifiers.component(&mut memory, entity) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(entry) = names.1.entry(entity) {
            let Some(text) = memory
                .string(identifier + IDENTIFIER_NAME, 48)
                .filter(|text| valid_name(text))
            else {
                continue;
            };
            entry.insert(CachedName {
                identifier: text.into(),
                display: None,
            });
        }
        let cached = names.1.get_mut(&entity).unwrap();
        let name = Arc::clone(&cached.identifier);
        let display_name = meta
            .as_ref()
            .and_then(|v| v.name_reusing(&mut memory, entity, cached.display.as_ref()));
        cached.display = display_name.clone();
        let map = match &attributes {
            Some(view) => view.component(&mut memory, entity),
            None => None,
        };
        let health = match map {
            Some(map) => health(&mut memory, map),
            None => None,
        };
        let bounds = boxes
            .as_ref()
            .and_then(|view| view.component(&mut memory, entity))
            .map(|at| read::<super::Aabb>(at))
            .filter(valid_bounds);
        let actor_type = match &types {
            Some(view) => view
                .component(&mut memory, entity)
                .map_or(0, |at| read::<u32>(at)),
            None => 0,
        };
        actors.insert(
            entity,
            Actor {
                name,
                display_name,
                actor_type,
                health,
                bounds,
                local: entity == own,
            },
        );
    }
    names.1.retain(|entity, _| actors.contains_key(entity));
    Some((actors, own_vitals))
}

pub(super) fn valid_bounds(b: &super::Aabb) -> bool {
    b.iter().all(|v| v.is_finite()) && (0..3).all(|k| b[k + 3] > b[k] && b[k + 3] - b[k] < 64.0)
}

/// Only called inside the HUD hook, while the local player is live.
pub(super) unsafe fn sample(player: usize, now: u64) {
    let surrounding = USERS.load(Ordering::Relaxed) != 0;
    if (!surrounding && VITAL_USERS.load(Ordering::Relaxed) == 0)
        || now.saturating_sub(LAST_SAMPLE.load(Ordering::Relaxed)) < INTERVAL_MS
    {
        return;
    }
    let Ok(_sample) = SAMPLING.try_lock() else {
        return;
    };
    if now.saturating_sub(LAST_SAMPLE.load(Ordering::Relaxed)) < INTERVAL_MS {
        return;
    }
    LAST_SAMPLE.store(now, Ordering::Relaxed);
    // The component layouts above were taken from this build only.
    if !known_build() {
        return;
    }
    let mut scope = crate::modules::performance_profiler::metrics::Scope::new(
        crate::modules::performance_profiler::metrics::Stage::ActorSample,
    );
    // Copy native data before taking the shared snapshot lock: rendering only
    // waits for an Arc swap, never for an entire registry enumeration.
    let snapshot = collect(player, surrounding).map(|(actors, vitals)| {
        scope.units(actors.len());
        Snapshot {
            at: now,
            owner: (read(player + 0x10), read(player + 0x18)),
            actors: Arc::new(actors),
            vitals,
            surrounding,
        }
    });
    let Ok(mut slot) = SNAPSHOT.try_lock() else {
        return;
    };
    *slot = snapshot;
}

/// Starts or stops one module's use of the entity snapshot; keeps the game
/// hook installed while in use.
pub fn use_actors(on: bool) -> Result<(), String> {
    use_snapshot(on, &USERS)
}
/// Subscribe only to the local player's attributes. Other subscriptions can
/// still request full actor facts; both paths share the same 100ms sample.
pub fn use_vitals(on: bool) -> Result<(), String> {
    use_snapshot(on, &VITAL_USERS)
}
fn use_snapshot(on: bool, users: &AtomicUsize) -> Result<(), String> {
    if on {
        if !known_build() {
            return Err("エンティティ属性の取得には対応するゲームビルドが必要です".into());
        }
        acquire()?;
        if users.fetch_add(1, Ordering::Relaxed) == 0 {
            LAST_SAMPLE.store(0, Ordering::Relaxed);
        }
    } else {
        release()?;
        let left = users
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .map_or(0, |n| n - 1);
        if left == 0 {
            LAST_SAMPLE.store(0, Ordering::Relaxed);
            if USERS.load(Ordering::Relaxed) == 0 && VITAL_USERS.load(Ordering::Relaxed) == 0 {
                *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        }
    }
    Ok(())
}

fn fresh<T>(surrounding: bool, read: impl FnOnce(&Snapshot) -> T) -> Option<T> {
    if USERS.load(Ordering::Relaxed) == 0
        && (surrounding || VITAL_USERS.load(Ordering::Relaxed) == 0)
    {
        return None;
    }
    let slot = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = slot.as_ref()?;
    if surrounding && !snapshot.surrounding {
        return None;
    }
    (unsafe { GetTickCount64() }.saturating_sub(snapshot.at) <= MAX_AGE_MS).then(|| read(snapshot))
}

/// The latest reading's time and entities, for callers that diff successive
/// readings (the hit feed). None until sampled, fresh, and in use.
pub fn reading() -> Option<(u64, Arc<HashMap<u32, Actor>>)> {
    fresh(true, |snapshot| (snapshot.at, Arc::clone(&snapshot.actors)))
}
pub(crate) type LocalReading = (u64, Arc<HashMap<u32, Actor>>, Option<Vitals>);
/// SDK readers must not combine the last world's facts with a new player.
pub(crate) fn reading_for_local(owner: (usize, u32)) -> Option<LocalReading> {
    fresh(true, |s| {
        s.for_owner(owner).map(|actors| (s.at, actors, s.vitals))
    })
    .flatten()
}
pub(crate) fn vitals_for_local(owner: (usize, u32)) -> Option<(u64, Option<Vitals>)> {
    fresh(false, |s| (s.owner == owner).then_some((s.at, s.vitals))).flatten()
}

/// Every described entity by id, if in a world and in use.
pub fn actors() -> Option<Arc<HashMap<u32, Actor>>> {
    fresh(true, |snapshot| Arc::clone(&snapshot.actors))
}

/// Only combine facts from the same local player/registry as a sampled ray.
pub(crate) fn actors_for_local(owner: (usize, u32)) -> Option<Arc<HashMap<u32, Actor>>> {
    fresh(true, |snapshot| snapshot.for_owner(owner)).flatten()
}

/// The local player's health, hunger and experience, if in a world and in use.
pub fn local_vitals() -> Option<Vitals> {
    fresh(false, |snapshot| snapshot.vitals).flatten()
}

pub fn diagnostic() -> String {
    let mut text = if let Some(map) = actors() {
        let health = map.values().filter(|a| a.health.is_some()).count();
        let boxes = map.values().filter(|a| a.bounds.is_some()).count();
        format!(
            "actors: {} entities, {health} with HP, {boxes} with bounds; 10 Hz",
            map.len()
        )
    } else if local_vitals().is_some() {
        "actors: local attributes only; surrounding enumeration disabled; 10 Hz".into()
    } else {
        return "actors: no snapshot (enable an entity/health HUD in a world)".into();
    };
    if let Some(v) = local_vitals() {
        text.push_str(&format!("\nlocal: HP {:.1}/{:.1}, absorption {:.1}, food {:.1}, saturation {:.1}, exhaustion {:.2}, level {:.0}, XP {:.1}%",
            v.health,v.max_health,v.absorption,v.hunger,v.saturation,v.exhaustion,v.level,v.experience*100.));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakePool {
        _sparse: Vec<u32>,
        _sparse_pages: Vec<usize>,
        _dense: Vec<u32>,
        _pages: Vec<Vec<u64>>,
        _page_pointers: Vec<usize>,
        header: [usize; 12],
    }
    impl FakePool {
        fn new(count: usize, size: usize, init: impl Fn(usize)) -> Self {
            let mut sparse = vec![u32::MAX; 2048];
            let dense: Vec<_> = (0..count).map(|i| 0x40000 | i as u32).collect();
            for (i, entry) in sparse.iter_mut().enumerate().take(count) {
                *entry = 0x40000 | i as u32;
            }
            let sparse_pages = vec![sparse.as_ptr() as usize];
            let mut pages: Vec<_> = (0..count.div_ceil(128))
                .map(|_| vec![0u64; 128 * size / 8])
                .collect();
            let page_pointers: Vec<_> = pages.iter_mut().map(|p| p.as_mut_ptr() as usize).collect();
            for i in 0..count {
                init(page_pointers[i / 128] + (i % 128) * size);
            }
            let mut header = [0; 12];
            header[1] = sparse_pages.as_ptr() as usize;
            header[2] = header[1] + sparse_pages.len() * 8;
            header[4] = dense.as_ptr() as usize;
            header[5] = header[4] + count * 4;
            header[10] = page_pointers.as_ptr() as usize;
            header[11] = header[10] + page_pointers.len() * 8;
            Self {
                _sparse: sparse,
                _sparse_pages: sparse_pages,
                _dense: dense,
                _pages: pages,
                _page_pointers: page_pointers,
                header,
            }
        }
    }
    struct Fixture {
        _attributes: Map,
        _pools: [Box<FakePool>; 2],
        _nodes: Vec<usize>,
        _registry: Box<[usize; 16]>,
        player: Box<[usize; 4]>,
    }
    impl Fixture {
        fn new(count: usize) -> Self {
            let attributes = map(
                &[
                    (1, 0., 20., 18.),
                    (2, 0., 20., 5.),
                    (3, 0., 20., 0.),
                    (4, 0., 100., 3.),
                    (5, 0., 1., 0.5),
                    (6, 0., 20., 20.),
                    (13, 0., 20., 0.),
                ],
                ATTRIBUTE_STRIDE,
            );
            let attrs = Box::new(FakePool::new(count, ATTRIBUTES_SIZE, |at| unsafe {
                std::ptr::copy_nonoverlapping(
                    attributes.component.as_ptr(),
                    at as *mut usize,
                    attributes.component.len(),
                );
            }));
            let ids = Box::new(FakePool::new(count, IDENTIFIER_SIZE, |at| unsafe {
                std::ptr::copy_nonoverlapping(
                    b"player".as_ptr(),
                    (at + IDENTIFIER_NAME) as *mut u8,
                    6,
                );
                ((at + IDENTIFIER_NAME + 16) as *mut usize).write(6);
                ((at + IDENTIFIER_NAME + 24) as *mut usize).write(15);
            }));
            let nodes = vec![
                0,
                ATTRIBUTES as usize,
                attrs.header.as_ptr() as usize,
                0,
                0,
                IDENTIFIER as usize,
                ids.header.as_ptr() as usize,
                0,
            ];
            let mut registry = Box::new([0usize; 16]);
            registry[13] = nodes.as_ptr() as usize;
            registry[14] = registry[13] + nodes.len() * 8;
            let player = Box::new([0, 0, registry.as_ptr() as usize, 0x40000]);
            Self {
                _attributes: attributes,
                _pools: [attrs, ids],
                _nodes: nodes,
                _registry: registry,
                player,
            }
        }
    }
    #[test]
    fn local_only_collection_reads_the_same_vitals_without_describing_surrounding_entities() {
        let _serial = super::super::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut fixture = Fixture::new(512);
        *NAMES.lock().unwrap() = (0, HashMap::new());
        let player = fixture.player.as_ptr() as usize;
        let (all, expected) = unsafe { collect(player, true) }.unwrap();
        assert_eq!(all.len(), 512);
        assert!(expected.is_some());
        let (local, actual) = unsafe { collect(player, false) }.unwrap();
        assert!(local.is_empty());
        assert_eq!(actual, expected);
        // Local data remains available when no identifier pool exists at all.
        unsafe {
            fixture._nodes.as_mut_ptr().add(5).write(0);
        }
        assert_eq!(unsafe { collect(player, false) }.unwrap().1, expected);
        assert!(unsafe { collect(player, true) }.is_none());
        *NAMES.lock().unwrap() = (0, HashMap::new());
    }

    #[test]
    fn local_subscription_does_not_publish_empty_entities_and_keeps_world_and_age_checks() {
        let _serial = super::super::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                USERS.store(0, Ordering::Relaxed);
                VITAL_USERS.store(0, Ordering::Relaxed);
                *SNAPSHOT.lock().unwrap() = None;
            }
        }
        let _reset = Reset;
        let owner = (0x1000, 0x40001);
        VITAL_USERS.store(1, Ordering::Relaxed);
        let now = unsafe { GetTickCount64() };
        *SNAPSHOT.lock().unwrap() = Some(Snapshot {
            at: now,
            owner,
            actors: Arc::new(HashMap::new()),
            vitals: Some(Vitals::default()),
            surrounding: false,
        });
        assert_eq!(local_vitals(), Some(Vitals::default()));
        assert_eq!(
            vitals_for_local(owner),
            Some((now, Some(Vitals::default())))
        );
        assert!(vitals_for_local((0x2000, owner.1)).is_none());
        assert!(actors().is_none() && reading_for_local(owner).is_none());
        USERS.store(1, Ordering::Relaxed);
        assert!(
            actors().is_none(),
            "a full subscriber must wait for a full sample"
        );
        SNAPSHOT.lock().unwrap().as_mut().unwrap().surrounding = true;
        assert!(actors().is_some());
        let mut state = SNAPSHOT.lock().unwrap();
        state.as_mut().unwrap().at = now.saturating_sub(MAX_AGE_MS + 1);
        drop(state);
        assert!(local_vitals().is_none() && actors().is_none());
    }

    #[test]
    #[ignore = "manual: cargo test -p client --release --lib actor_collection_cost -- --ignored --nocapture"]
    fn actor_collection_cost() {
        let _serial = super::super::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for count in [75, 512] {
            let fixture = Fixture::new(count);
            *NAMES.lock().unwrap() = (0, HashMap::new());
            for full in [true, false] {
                let mut samples = Vec::new();
                for i in 0..500 {
                    let start = std::time::Instant::now();
                    let value = unsafe { collect(fixture.player.as_ptr() as usize, full) }.unwrap();
                    std::hint::black_box(value);
                    if i >= 50 {
                        samples.push(start.elapsed());
                    }
                }
                samples.sort();
                println!(
                    "Actor facts {count} entities / full={full}: median {:?}, p90 {:?}",
                    samples[225], samples[405]
                );
            }
        }
        *NAMES.lock().unwrap() = (0, HashMap::new());
    }

    #[test]
    fn view_rays_never_use_another_registry_or_local_player_snapshot() {
        let snapshot = Snapshot {
            at: 1,
            owner: (0x1000, 0x40002),
            actors: Arc::new(HashMap::new()),
            vitals: None,
            surrounding: true,
        };
        assert!(Arc::ptr_eq(
            &snapshot.for_owner((0x1000, 0x40002)).unwrap(),
            &snapshot.actors
        ));
        assert!(snapshot.for_owner((0x2000, 0x40002)).is_none());
        assert!(snapshot.for_owner((0x1000, 2)).is_none());
        let local = Snapshot {
            surrounding: false,
            ..snapshot
        };
        assert!(
            local.for_owner((0x1000, 0x40002)).is_none(),
            "a local-only reading is not an empty world snapshot"
        );
    }

    /// A fake AttributesComponent with the given (id, min, max, value) rows.
    struct Map {
        _keys: Vec<u32>,
        _values: Vec<u64>,
        component: [usize; ATTRIBUTES_SIZE / 8],
    }

    fn map(rows: &[(u32, f32, f32, f32)], stride: usize) -> Map {
        let keys: Vec<u32> = rows.iter().map(|r| r.0).collect();
        let mut values = vec![0u64; rows.len() * stride / 8];
        for (i, &(_, min, max, value)) in rows.iter().enumerate() {
            let at = values.as_mut_ptr() as usize + i * stride + ATTRIBUTE_CURRENT;
            unsafe { (at as *mut [f32; 3]).write_unaligned([min, max, value]) };
        }
        let mut component = [0usize; ATTRIBUTES_SIZE / 8];
        component[0] = keys.as_ptr() as usize;
        component[1] = component[0] + keys.len() * 4;
        component[3] = values.as_ptr() as usize;
        component[4] = component[3] + rows.len() * stride;
        Map {
            _keys: keys,
            _values: values,
            component,
        }
    }

    #[test]
    fn type_ids_are_the_fnv1a_of_the_component_names() {
        // Ids the client already used, found independently in the game's code.
        assert_eq!(type_hash("AABBShapeComponent"), 0xBAC1_B3CF);
        assert_eq!(type_hash("StateVectorComponent"), 0x1B5D_5238);
        assert_eq!(type_hash("ActorRotationComponent"), 0x75DF_36B7);
        assert_eq!(type_hash("OnGroundFlagComponent"), 0xC290_78A0);
        assert_eq!(type_hash("ActorOwnerComponent"), 0x85B9_3800);
        assert_eq!(ACTOR_TYPE, 0x4F6B_A419);
        // The ones this module adds, as seen in the live registry.
        assert_eq!(ATTRIBUTES, 0xFD3B_0613);
        assert_eq!(IDENTIFIER, 0xDEB6_534F);
        assert_eq!(AABB, 0xBAC1_B3CF);
    }

    #[test]
    fn attributes_are_found_by_id_and_read_from_the_current_values() {
        // The local player's values as read from the game (creative mode).
        let player = map(
            &[
                (1, 0., 20., 20.),
                (2, 0., 20., 5.),
                (3, 0., 20., 0.),
                (4, 0., 24791., 3.),
                (5, 0., 1., 0.142857),
                (6, 0., 20., 17.5),
                (13, 0., 16., 4.),
            ],
            ATTRIBUTE_STRIDE,
        );
        let at = player.component.as_ptr() as usize;
        let mut memory = ReadBatch::new();
        unsafe {
            assert_eq!(attribute(&mut memory, at, 2), Some([0., 20., 5.]));
            assert_eq!(attribute(&mut memory, at, 99), None);
            assert_eq!(health(&mut memory, at), Some((17.5, 20.)));
            let v = vitals(&mut memory, at).unwrap();
            assert_eq!((v.health, v.max_health, v.absorption), (17.5, 20., 4.));
            assert_eq!((v.hunger, v.saturation, v.exhaustion), (20., 5., 0.));
            assert_eq!((v.level, v.experience), (3., 0.142857));
        }
        // A mob has health but none of the player-only attributes.
        let pig = map(
            &[(6, 0., 10., 10.), (9, 0., 3.4e38, 0.25)],
            ATTRIBUTE_STRIDE,
        );
        let at = pig.component.as_ptr() as usize;
        unsafe {
            assert_eq!(health(&mut memory, at), Some((10., 10.)));
            assert_eq!(vitals(&mut memory, at), None);
        }
    }

    #[test]
    fn unknown_layouts_and_bad_values_are_rejected() {
        let mut memory = ReadBatch::new();
        unsafe {
            // Another element size: this is not the layout that was analyzed.
            let other = map(&[(6, 0., 20., 20.)], ATTRIBUTE_STRIDE + 8);
            assert_eq!(health(&mut memory, other.component.as_ptr() as usize), None);
            // Empty map, null pointers, unreadable component.
            let empty = map(&[], ATTRIBUTE_STRIDE);
            assert_eq!(health(&mut memory, empty.component.as_ptr() as usize), None);
            let zeros = [0usize; ATTRIBUTES_SIZE / 8];
            assert_eq!(health(&mut memory, zeros.as_ptr() as usize), None);
            assert_eq!(health(&mut memory, 0), None);
            // Nonsense numbers are not shown as health.
            for (max, value) in [
                (0., 0.),
                (20., -1.),
                (20., 21.),
                (20., f32::NAN),
                (f32::INFINITY, 5.),
            ] {
                let bad = map(&[(6, 0., max, value)], ATTRIBUTE_STRIDE);
                assert_eq!(
                    health(&mut memory, bad.component.as_ptr() as usize),
                    None,
                    "{max} {value}"
                );
            }
        }
    }

    #[test]
    fn names_must_look_like_identifiers() {
        for name in [
            "pig",
            "player",
            "zombie_villager_v2",
            "xp_orb",
            "mypack:custom.mob",
        ] {
            assert!(valid_name(name), "{name}");
        }
        for name in ["", "Pig", "two words", "pig\0", &"x".repeat(49)] {
            assert!(!valid_name(name), "{name:?}");
        }
    }

    #[test]
    fn nothing_is_reported_outside_the_game() {
        let _serial = super::super::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert!(actors().is_none());
        assert!(local_vitals().is_none());
        // The game hook cannot be installed here, so no user is registered.
        assert!(use_actors(true).is_err());
        assert!(use_vitals(true).is_err());
        assert_eq!(USERS.load(Ordering::Relaxed), 0);
        assert_eq!(VITAL_USERS.load(Ordering::Relaxed), 0);
    }
}
