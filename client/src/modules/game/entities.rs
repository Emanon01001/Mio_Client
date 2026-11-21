//! Entity-pool enumeration, generation validation, and owned AABB snapshots.
use super::memory::ReadBatch;
use super::{layout::Layout, read, readable, span};
use crate::log;
use std::sync::atomic::{AtomicUsize, Ordering};

/// An axis-aligned box: min x, y, z then max x, y, z.
pub type Aabb = [f32; 6];

/// node of the pool map = { next, type hash, pool pointer, control block }
const POOL_NODE_SIZE: usize = 0x20;
const AABB_SIZE: usize = 0x20;
/// Low bits of an entity id; all ones marks the null entity index.
const ENTITY_INDEX_MASK: u32 = 0x3_FFFF;
const ENTITY_VERSION_MASK: u32 = !ENTITY_INDEX_MASK;
/// Upper bound on boxes copied per snapshot.
const MAX_BOXES: usize = 2048;

/// Registry the cached pool belongs to, and that pool (0 = not found).
static POOL_REGISTRY: AtomicUsize = AtomicUsize::new(0);
static AABB_POOL: AtomicUsize = AtomicUsize::new(0);

/// Whether a pool's pages hold the component at `component`.
unsafe fn pool_holds(memory: &mut ReadBatch, l: &Layout, pool: usize, component: usize) -> bool {
    let Some((pages, pages_end)) = pool
        .checked_add(l.pool_pages)
        .and_then(|at| memory.span(at, 8))
    else {
        return false;
    };
    let Some(page_bytes) = l.pool_page_len.checked_mul(AABB_SIZE) else {
        return false;
    };
    (pages..pages_end).step_by(8).take(4096).any(|slot| {
        let page: usize = read(slot);
        page != 0
            && page
                .checked_add(page_bytes)
                .is_some_and(|end| (page..end).contains(&component))
            && (component - page) & (AABB_SIZE - 1) == 0
    })
}

/// Finds the bounding-box pool of `registry`: the one that stores the
/// player's own box. Looked up once per registry.
unsafe fn aabb_pool(
    memory: &mut ReadBatch,
    l: &Layout,
    registry: usize,
    own: usize,
) -> Option<usize> {
    let previous_registry = POOL_REGISTRY.load(Ordering::Relaxed);
    let previous_pool = AABB_POOL.load(Ordering::Relaxed);
    if previous_registry == registry
        && previous_pool != 0
        && pool_holds(memory, l, previous_pool, own)
    {
        return Some(previous_pool);
    }
    let pool = memory
        .span(registry + l.registry_pools, POOL_NODE_SIZE)
        .and_then(|(begin, end)| {
            (begin..end)
                .step_by(POOL_NODE_SIZE)
                .take(8192)
                .map(|node| read::<usize>(node + 16))
                .find(|&pool| pool_holds(memory, l, pool, own))
        })
        .unwrap_or(0);
    if pool == 0 && (previous_registry != registry || previous_pool != 0) {
        log!("esp: entity pool not found - check [offsets] for this game version");
    }
    AABB_POOL.store(pool, Ordering::Relaxed);
    POOL_REGISTRY.store(registry, Ordering::Relaxed);
    (pool != 0).then_some(pool)
}

/// The native in-place deletion policy stores a free-list link in the index
/// bits and all-ones in the version bits. Its old component bytes remain.
fn live_entity(entity: u32) -> bool {
    entity & ENTITY_INDEX_MASK != ENTITY_INDEX_MASK
        && entity & ENTITY_VERSION_MASK != ENTITY_VERSION_MASK
}

/// Confirm both generation and dense slot through the pool's sparse mapping.
/// A recycled entity index must never resurrect a deleted entity's box.
unsafe fn pool_slot(sparse: (usize, usize), entity: u32) -> Option<usize> {
    pool_slot_checked(sparse, entity, readable)
}
unsafe fn pool_slot_checked(
    sparse: (usize, usize),
    entity: u32,
    mut check: impl FnMut(usize, usize) -> bool,
) -> Option<usize> {
    if !live_entity(entity) {
        return None;
    }
    let index = (entity & ENTITY_INDEX_MASK) as usize;
    let slot = sparse.0.checked_add((index / 2048) * 8)?;
    if slot >= sparse.1 {
        return None;
    }
    let page: usize = read(slot);
    let entry = page.checked_add(index % 2048 * 4)?;
    if !check(entry, 4) {
        return None;
    }
    let packed: u32 = read(entry);
    ((packed ^ (entity & ENTITY_VERSION_MASK)) < ENTITY_INDEX_MASK)
        .then_some((packed & ENTITY_INDEX_MASK) as usize)
}

/// Resolve several component types with one walk of the registry's pool map.
pub(super) unsafe fn component_pools<const N: usize>(
    memory: &mut ReadBatch,
    registry: usize,
    hashes: [u32; N],
) -> [Option<usize>; N] {
    let mut found = [None; N];
    let Some((begin, end)) = registry
        .checked_add(0x68)
        .and_then(|p| memory.span(p, POOL_NODE_SIZE))
    else {
        return found;
    };
    for node in (begin..end).step_by(POOL_NODE_SIZE).take(8192) {
        let hash = read::<u32>(node + 8);
        for (i, wanted) in hashes.iter().enumerate() {
            if hash == *wanted {
                let pool = read::<usize>(node + 16);
                found[i] = memory.readable(pool, 0x60).then_some(pool);
            }
        }
        if found.iter().all(Option::is_some) {
            break;
        }
    }
    found
}

/// Metadata belongs to this enumeration only. Entity IDs and generations are
/// still checked on every access; component contents are never cached.
pub(super) struct PoolView {
    sparse: (usize, usize),
    dense: (usize, usize),
    pages: (usize, usize),
    size: usize,
}
impl PoolView {
    pub unsafe fn new(memory: &mut ReadBatch, pool: usize, size: usize) -> Option<Self> {
        Some(Self {
            sparse: memory.span(pool.checked_add(8)?, 8)?,
            dense: memory.span(pool.checked_add(0x20)?, 4)?,
            pages: if size == 0 {
                (0, 0)
            } else {
                memory.span(pool.checked_add(0x50)?, 8)?
            },
            size,
        })
    }
    pub fn entities(&self) -> std::ops::Range<usize> {
        self.dense.0..self.dense.1
    }
    unsafe fn slot(&self, memory: &mut ReadBatch, entity: u32) -> Option<usize> {
        let slot = pool_slot_checked(self.sparse, entity, |p, n| memory.readable(p, n))?;
        let dense = self.dense.0.checked_add(slot.checked_mul(4)?)?;
        (dense < self.dense.1 && read::<u32>(dense) == entity).then_some(slot)
    }
    pub unsafe fn contains(&self, memory: &mut ReadBatch, entity: u32) -> bool {
        self.slot(memory, entity).is_some()
    }
    pub unsafe fn component(&self, memory: &mut ReadBatch, entity: u32) -> Option<usize> {
        if self.size == 0 {
            return None;
        }
        let slot = self.slot(memory, entity)?;
        let page_slot = self.pages.0.checked_add((slot / 128).checked_mul(8)?)?;
        if page_slot >= self.pages.1 {
            return None;
        }
        let data = read::<usize>(page_slot).checked_add((slot % 128).checked_mul(self.size)?)?;
        memory.readable(data, self.size).then_some(data)
    }
}

/// Resolve components afresh on the game thread; never retain native pointers
/// between updates. Related pools are found together by `component_pools`.
#[cfg(test)]
pub(super) unsafe fn component(pool: usize, entity: u32, size: usize) -> Option<usize> {
    let slot = pool_slot(span(pool + 8, 8)?, entity)?;
    let (entities, end) = span(pool + 0x20, 4)?;
    let dense = entities.checked_add(slot.checked_mul(4)?)?;
    if dense >= end || read::<u32>(dense) != entity {
        return None;
    }
    let (pages, end) = span(pool + 0x50, 8)?;
    let page_slot = pages.checked_add((slot / 128).checked_mul(8)?)?;
    if page_slot >= end {
        return None;
    }
    let data = read::<usize>(page_slot).checked_add((slot % 128).checked_mul(size)?)?;
    readable(data, size).then_some(data)
}

/// Copies every entity's box except the player's own into `out`.
pub(super) unsafe fn collect_boxes(
    l: &Layout,
    player: usize,
    own: usize,
    out: &mut Vec<(u32, Aabb)>,
) {
    out.clear();
    let mut memory = ReadBatch::new();
    if l.pool_page_len == 0 || !memory.readable(player + l.actor_registry, 8) {
        return;
    }
    let registry: usize = read(player + l.actor_registry);
    let Some(pool) = aabb_pool(&mut memory, l, registry, own) else {
        return;
    };
    let (Some((entities, entities_end)), Some((pages, pages_end)), Some(sparse)) = (
        memory.span(pool + l.pool_entities, 4),
        memory.span(pool + l.pool_pages, 8),
        memory.span(pool + 8, 8),
    ) else {
        return;
    };
    let count = ((entities_end - entities) / 4).min(MAX_BOXES);
    let page_count = (pages_end - pages) / 8;
    // Pages are checked once each, not per component.
    let mut checked_page = usize::MAX;
    let mut page = 0;
    for i in 0..count {
        let page_index = i / l.pool_page_len;
        if page_index >= page_count {
            break;
        }
        let entity: u32 = read(entities + i * 4);
        if pool_slot_checked(sparse, entity, |p, n| memory.readable(p, n)) != Some(i) {
            continue;
        }
        if page_index != checked_page {
            page = read(pages + page_index * 8);
            if !memory.readable(page, l.pool_page_len * AABB_SIZE) {
                break;
            }
            checked_page = page_index;
        }
        let component = page + i % l.pool_page_len * AABB_SIZE;
        if component == own {
            continue;
        }
        let aabb: Aabb = read(component);
        let sane = aabb.iter().all(|v| v.is_finite())
            && (0..3).all(|k| aabb[k + 3] > aabb[k] && aabb[k + 3] - aabb[k] < 64.0);
        if sane {
            out.push((entity, aabb));
        }
    }
}

// FNV-1a type IDs verified against the exact build's type-info getters.
const ON_GROUND: u32 = 0xC290_78A0;
const HEAD_IN_WATER: u32 = 0x8971_8A63;

/// Flags are determined by current entity membership, not component bytes.
unsafe fn actor_flag_membership<const N: usize>(player: usize, hashes: [u32; N]) -> [bool; N] {
    let mut memory = ReadBatch::new();
    if !memory.readable(player, 0x1C) {
        return [false; N];
    }
    let registry: usize = read(player + 0x10);
    let entity: u32 = read(player + 0x18);
    component_pools(&mut memory, registry, hashes).map(|pool| {
        pool.and_then(|pool| PoolView::new(&mut memory, pool, 0))
            .is_some_and(|view| view.contains(&mut memory, entity))
    })
}
pub(super) unsafe fn actor_on_ground(player: usize) -> bool {
    actor_flag_membership(player, [ON_GROUND])[0]
}
/// One registry scan for simultaneous grounded jumping and underwater floating.
pub(super) unsafe fn actor_jump_state(player: usize) -> [bool; 2] {
    actor_flag_membership(player, [ON_GROUND, HEAD_IN_WATER])
}

// Sparse/dense lookup with the native getter's entity-generation check.
pub(super) unsafe fn actor_flags(player: usize) -> Option<usize> {
    let registry: usize = read(player + 0x10);
    let entity: u32 = read(player + 0x18);
    let (begin, end) = span(registry + 0x68, 32)?;
    let node = (begin..end)
        .step_by(32)
        .take(8192)
        .find(|&node| read::<u32>(node + 8) == 0xC674_26F3)?;
    let pool: usize = read(node + 16);
    let index = pool_slot(span(pool + 8, 8)?, entity)?;
    let (pages, pages_end) = span(pool + 0x50, 8)?;
    let page_slot = pages + (index / 128) * 8;
    if page_slot >= pages_end {
        return None;
    }
    let data: usize = read(page_slot);
    let component = data.checked_add(index % 128 * 0x18)?;
    readable(component, 0x18).then_some(component)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_tag_requires_current_entity_membership_without_component_pages() {
        let mut dense = [0x40001u32];
        let mut sparse = [u32::MAX; 2048];
        sparse[1] = 0x40000;
        let sparse_pages = [sparse.as_mut_ptr() as usize];
        let mut pool = [0usize; 12];
        pool[1] = sparse_pages.as_ptr() as usize;
        pool[2] = pool[1] + 8;
        pool[4] = dense.as_mut_ptr() as usize;
        pool[5] = pool[4] + 4;
        // Empty tags have no component data pages.
        let mut nodes = [
            0usize,
            ON_GROUND as usize,
            pool.as_ptr() as usize,
            0,
            0,
            HEAD_IN_WATER as usize,
            pool.as_ptr() as usize,
            0,
        ];
        let mut registry = [0usize; 16];
        registry[13] = nodes.as_ptr() as usize;
        registry[14] = registry[13] + 64;
        let mut actor = [0usize; 4];
        actor[2] = registry.as_ptr() as usize;
        actor[3] = 0x40001;
        let player = actor.as_mut_ptr() as usize;
        unsafe {
            assert!(!actor_on_ground(0));
            assert_eq!(actor_jump_state(0), [false, false]);
            assert!(actor_on_ground(player));
            assert_eq!(actor_jump_state(player), [true, true]);
            (nodes.as_mut_ptr().add(1)).write(0);
            assert_eq!(actor_jump_state(player), [false, true]);
            (nodes.as_mut_ptr().add(1)).write(ON_GROUND as usize);
            (nodes.as_mut_ptr().add(5)).write(0);
            assert_eq!(actor_jump_state(player), [true, false]);
            (nodes.as_mut_ptr().add(5)).write(HEAD_IN_WATER as usize);
            (sparse_pages[0] as *mut u32).add(1).write(u32::MAX);
            assert!(!actor_on_ground(player)); // Tag removed on takeoff.
            assert_eq!(actor_jump_state(player), [false, false]);
            (sparse_pages[0] as *mut u32).add(1).write(0x40000);
            assert!(actor_on_ground(player)); // Tag restored on landing.
            (player as *mut usize).add(3).write(0x80001);
            assert!(!actor_on_ground(player)); // Recycled entity generation.
            assert_eq!(actor_jump_state(player), [false, false]);
            (player as *mut usize).add(3).write(0x40001);
            (pool[4] as *mut u32).write(0x40002);
            assert!(!actor_on_ground(player)); // Stale sparse mapping.
            assert_eq!(actor_jump_state(player), [false, false]);
        }
    }

    #[test]
    fn owner_lookup_rejects_retired_recycled_or_mismatched_dense_slots() {
        let mut entities = [0x40001u32];
        let mut sparse = [u32::MAX; 2048];
        sparse[1] = 0x40000;
        let sparse_pages = [sparse.as_mut_ptr() as usize];
        let data = [123usize; 128];
        let pages = [data.as_ptr() as usize];
        let mut pool = [0usize; 12];
        pool[1] = sparse_pages.as_ptr() as usize;
        pool[2] = pool[1] + 8;
        pool[4] = entities.as_mut_ptr() as usize;
        pool[5] = pool[4] + 4;
        pool[10] = pages.as_ptr() as usize;
        pool[11] = pool[10] + 8;
        let p = pool.as_ptr() as usize;
        unsafe {
            let mut memory = ReadBatch::new();
            let view = PoolView::new(&mut memory, p, 8).unwrap();
            assert_eq!(component(p, 0x40001, 8), Some(data.as_ptr() as usize));
            assert_eq!(
                view.component(&mut memory, 0x40001),
                component(p, 0x40001, 8)
            );
            assert_eq!(component(p, 0x80001, 8), None); // Same index, other generation.
            assert_eq!(view.component(&mut memory, 0x80001), None);
            (pool[4] as *mut u32).write(0x40002);
            assert_eq!(component(p, 0x40001, 8), None); // Sparse still points at a replaced dense slot.
            assert_eq!(view.component(&mut memory, 0x40001), None);
            (pool[4] as *mut u32).write(0x40001);
            (sparse_pages[0] as *mut u32).add(1).write(u32::MAX);
            assert_eq!(component(p, 0x40001, 8), None); // Old owner bytes still exist.
            assert!(!view.contains(&mut memory, 0x40001));
        }
    }

    #[test]
    #[ignore = "manual CPU comparison; cargo test -p client --release --lib component_batch_benchmark -- --ignored --nocapture"]
    fn component_batch_benchmark() {
        use std::{
            hint::black_box,
            time::{Duration, Instant},
        };
        const N: usize = 1024;
        let ids: Vec<u32> = (0..N as u32).map(|i| 0x40000 | i).collect();
        let mut sparse = vec![u32::MAX; 2048];
        for (i, &id) in ids.iter().enumerate() {
            sparse[i] = id;
        }
        let sparse_pages = [sparse.as_ptr() as usize];
        let data: Vec<u64> = (1..=N as u64).collect();
        let pages: Vec<usize> = data.chunks(128).map(|p| p.as_ptr() as usize).collect();
        let mut pool = [0usize; 12];
        pool[1] = sparse_pages.as_ptr() as usize;
        pool[2] = pool[1] + 8;
        pool[4] = ids.as_ptr() as usize;
        pool[5] = pool[4] + N * 4;
        pool[10] = pages.as_ptr() as usize;
        pool[11] = pool[10] + pages.len() * 8;
        let p = black_box(pool.as_ptr() as usize);
        let expected = (N * (N + 1) / 2 * 32) as u64;
        let old = || unsafe {
            let mut sum = 0u64;
            for _ in 0..32 {
                for &id in &ids {
                    let addr = component(black_box(p), black_box(id), 8).unwrap();
                    sum += black_box(read::<u64>(addr));
                }
            }
            assert_eq!(sum, expected);
        };
        let new = || unsafe {
            let mut sum = 0u64;
            for _ in 0..32 {
                let mut memory = ReadBatch::new();
                let view = PoolView::new(&mut memory, black_box(p), 8).unwrap();
                for &id in &ids {
                    let addr = view.component(&mut memory, black_box(id)).unwrap();
                    sum += black_box(read::<u64>(addr));
                }
                black_box(memory.queries());
            }
            assert_eq!(sum, expected);
        };
        let median = |f: &dyn Fn()| -> Duration {
            f();
            let mut times = Vec::new();
            for _ in 0..7 {
                let start = Instant::now();
                f();
                times.push(start.elapsed());
            }
            times.sort();
            times[3]
        };
        let before = median(&old);
        let after = median(&new);
        let mut memory = ReadBatch::new();
        unsafe {
            let view = PoolView::new(&mut memory, p, 8).unwrap();
            for &id in &ids {
                view.component(&mut memory, id).unwrap();
            }
        }
        println!("1024 component lookups x32: old {:.3}ms, new {:.3}ms, {:.2}x; new {} protection queries / scan",
            before.as_secs_f64()*1000.,after.as_secs_f64()*1000.,before.as_secs_f64()/after.as_secs_f64(),memory.queries());
        println!("Synthetic component lookup CPU comparison; excludes native attacks, rendering and FPS.");
    }

    #[test]
    fn esp_ignores_tombstones_and_recycled_sparse_entries() {
        // Deleted slots keep realistic AABB bytes. Checking dimensions alone,
        // or only the entity-index null sentinel, would draw their ghosts.
        let entities = [0x40001u32, 0x80002, 0xFFFC0003, 0x3FFFF, 0x40004, 0x40005];
        let mut sparse_page = [u32::MAX; 2048];
        sparse_page[1] = 0x40000; // Local player, dense slot 0.
        sparse_page[2] = 0x80001; // Live other entity, dense slot 1.
        sparse_page[3] = 0x40002; // Deleted dense slot, old data still readable.
        sparse_page[4] = 0x80004; // Same index, new generation.
        sparse_page[5] = 0x40001; // Same generation, different dense slot.
        let sparse_pages = [sparse_page.as_mut_ptr() as usize];
        let mut components = [[0f32; 8]; 128];
        for (i, component) in components.iter_mut().enumerate() {
            let x = i as f32 * 3.;
            *component = [x, 64., 10., x + 0.6, 65.8, 10.6, 0.6, 1.8];
        }
        let own = components.as_ptr() as usize;
        let pages = [own];
        let mut pool = [0usize; 12];
        pool[1] = sparse_pages.as_ptr() as usize;
        pool[2] = pool[1] + 8;
        pool[4] = entities.as_ptr() as usize;
        pool[5] = pool[4] + std::mem::size_of_val(&entities);
        pool[10] = pages.as_ptr() as usize;
        pool[11] = pool[10] + 8;
        let node = [0usize, 0, pool.as_ptr() as usize, 0];
        let mut registry = [0usize; 15];
        registry[13] = node.as_ptr() as usize;
        registry[14] = registry[13] + 32;
        let player = [registry.as_ptr() as usize];
        // Layout contains only usize fields; unused offsets stay zero.
        let mut layout: Layout = unsafe { std::mem::zeroed() };
        layout.registry_pools = 0x68;
        layout.pool_entities = 0x20;
        layout.pool_pages = 0x50;
        layout.pool_page_len = 128;
        let mut out = Vec::new();
        unsafe { collect_boxes(&layout, player.as_ptr() as usize, own, &mut out) };
        let expected: Aabb = components[1][..6].try_into().unwrap();
        assert_eq!(out, vec![(0x80002, expected)]);

        // Despawn without clearing the component's old coordinates.
        unsafe {
            (sparse_pages[0] as *mut u32).add(2).write(u32::MAX);
            collect_boxes(&layout, player.as_ptr() as usize, own, &mut out);
        }
        assert!(out.is_empty());
        AABB_POOL.store(0, Ordering::Relaxed);
        POOL_REGISTRY.store(0, Ordering::Relaxed);
    }
}
