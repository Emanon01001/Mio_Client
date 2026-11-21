//! Bounded game-thread light queries. Only loaded chunks and owned cells are published.
#[path = "block_debug_light.rs"]
mod debug;
use super::super::{layout::image, patch, read, readable};
use super::{binding, block_name, full_support, GetBlock, World, GET_BLOCK};
use crate::signatures::BLOCK_LIGHT_SIGNATURE;
pub(in crate::modules::game) use debug::sample_debug;
pub(crate) use debug::{debug_snapshot, set_debug_active};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
type Brightness = unsafe extern "system" fn(usize, *mut u8, *const [i32; 3], u8, bool) -> usize;
type LoadedChunk = unsafe extern "system" fn(usize, *const [i32; 2]) -> usize;
static GET_LIGHT: OnceLock<Result<usize, String>> = OnceLock::new();
static ON: AtomicBool = AtomicBool::new(false);
static RADIUS: AtomicI32 = AtomicI32::new(8);
static LAST: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<Option<State>> = Mutex::new(None);
static SNAPSHOT: Mutex<Option<(u64, Arc<Vec<Cell>>)>> = Mutex::new(None);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub position: [i32; 3],
    pub block: u8,
    pub effective: u8,
}
struct State {
    world: World,
    center: [i32; 3],
    radius: i32,
    cursor: usize,
    cells: Vec<Option<Cell>>,
    order: Vec<[i32; 2]>,
    names: HashMap<usize, Option<String>>,
    cycle_at: u64,
    published: u64,
}
fn nearby(radius: i32) -> Vec<[i32; 2]> {
    let mut order = Vec::new();
    for z in -radius..=radius {
        for x in -radius..=radius {
            if x * x + z * z <= radius * radius {
                order.push([x, z]);
            }
        }
    }
    order.sort_by_key(|[x, z]| (x * x + z * z, *x, *z));
    order
}
fn recenter(s: &mut State, center: [i32; 3]) {
    // A single step retains the overlapping floor instead of blanking the
    // entire overlay. Changed columns are revalidated by the bounded scan.
    let previous: HashMap<_, _> = s
        .cells
        .iter()
        .flatten()
        .filter(|c| (c.position[1] - center[1]).abs() <= 3)
        .map(|c| ([c.position[0], c.position[2]], *c))
        .collect();
    for (cell, [dx, dz]) in s.cells.iter_mut().zip(&s.order) {
        *cell = previous.get(&[center[0] + dx, center[2] + dz]).copied();
    }
    s.center = center;
    s.cursor = 0;
    s.published = 0;
}
pub fn available() -> Result<(), String> {
    super::available()?;
    GET_LIGHT
        .get_or_init(|| patch::find(BLOCK_LIGHT_SIGNATURE))
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
pub fn configure(radius: i32) {
    RADIUS.store(radius.clamp(4, 12), Ordering::Relaxed);
}
pub fn set_active(on: bool) {
    ON.store(on, Ordering::Release);
    LAST.store(0, Ordering::Relaxed);
    if !on {
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
pub fn snapshot() -> Option<Arc<Vec<Cell>>> {
    if !ON.load(Ordering::Acquire) {
        return None;
    }
    let g = SNAPSHOT.lock().ok()?;
    let (at, cells) = g.as_ref()?;
    (unsafe { GetTickCount64() }.saturating_sub(*at) < 1500).then(|| Arc::clone(cells))
}
/// Two air cells above a recognized full floor. This is a lighting candidate,
/// not a guarantee: biome, mob type, player distance and floor rules still apply.
unsafe fn floor(
    get: GetBlock,
    source: usize,
    air: usize,
    p: [i32; 3],
    names: &mut HashMap<usize, Option<String>>,
) -> bool {
    let block = get(source, &[p[0], p[1] - 1, p[2]]);
    if block == 0 || block == air {
        return false;
    }
    if names.len() > 2048 {
        names.clear();
    }
    let name = names.entry(block).or_insert_with(|| block_name(block));
    name.as_deref().is_some_and(full_support)
        && get(source, &p) == air
        && get(source, &[p[0], p[1] + 1, p[2]]) == air
}
pub(in crate::modules::game) unsafe fn sample(player: usize, center: [i32; 3]) {
    if !ON.load(Ordering::Acquire) {
        return;
    }
    let now = GetTickCount64();
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < 32 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let Some(Ok(&get)) = GET_BLOCK.get().map(|v| v.as_ref()) else {
        return;
    };
    let Some(Ok(&light)) = GET_LIGHT.get().map(|v| v.as_ref()) else {
        return;
    };
    let Some((world, source, min, max)) = binding(player, get) else {
        if let Ok(mut g) = SNAPSHOT.try_lock() {
            *g = None;
        }
        return;
    };
    if !readable(world.2 + 0x1a9, 1) {
        return;
    }
    let dark = read::<u8>(world.2 + 0x1a9);
    if dark > 15 {
        return;
    }
    let vt = read::<usize>(source);
    let (base, end) = image();
    if vt < base || vt + 0x150 > end {
        return;
    }
    let chunk = read::<usize>(vt + 0x148);
    if chunk < base || chunk >= end {
        return;
    }
    let get: GetBlock = std::mem::transmute(get);
    let light: Brightness = std::mem::transmute(light);
    let chunk: LoadedChunk = std::mem::transmute(chunk);
    let Ok(mut guard) = STATE.try_lock() else {
        return;
    };
    let radius = RADIUS.load(Ordering::Relaxed);
    if guard
        .as_ref()
        .is_none_or(|s| s.world != world || s.radius != radius)
    {
        let order = nearby(radius);
        *guard = Some(State {
            world,
            center,
            radius,
            cursor: 0,
            cells: vec![None; order.len()],
            order,
            names: HashMap::new(),
            cycle_at: 0,
            published: 0,
        });
        if let Ok(mut g) = SNAPSHOT.try_lock() {
            *g = None;
        }
    }
    let s = guard.as_mut().unwrap();
    if s.center != center {
        recenter(s, center);
    }
    let total = s.order.len();
    if s.cursor == total {
        if now.saturating_sub(s.cycle_at) < 500 {
            return;
        }
        s.cursor = 0;
    }
    let start = Instant::now();
    let mut profile = crate::modules::performance_profiler::metrics::Scope::new(
        crate::modules::performance_profiler::metrics::Stage::LightScan,
    );
    let mut columns = 0;
    while s.cursor < total && columns < 16 {
        if columns > 0 && start.elapsed() >= Duration::from_micros(500) {
            break;
        }
        let i = s.cursor;
        s.cursor += 1;
        columns += 1;
        let x = center[0] + s.order[i][0];
        let z = center[2] + s.order[i][1];
        s.cells[i] = None;
        if chunk(source, &[x.div_euclid(16), z.div_euclid(16)]) == 0 {
            continue;
        }
        for y in ((center[1] - 3).max(min + 1)..=(center[1] + 2).min(max - 2)).rev() {
            let p = [x, y, z];
            if !floor(get, source, world.4, p, &mut s.names) {
                continue;
            }
            let (mut block, mut effective) = (255, 255);
            light(source, &mut block, &p, 15, true);
            light(source, &mut effective, &p, dark, true);
            if block <= 15 && effective <= 15 {
                s.cells[i] = Some(Cell {
                    position: p,
                    block,
                    effective,
                });
            }
            break;
        }
    }
    if s.cursor == total {
        s.cycle_at = now;
    }
    profile.units(columns);
    if s.cursor == total || now.saturating_sub(s.published) >= 150 {
        if let Ok(mut g) = SNAPSHOT.try_lock() {
            *g = Some((now, Arc::new(s.cells.iter().flatten().copied().collect())));
            s.published = now;
        }
    }
}
pub fn diagnostic() -> String {
    snapshot()
        .map(|s| format!("LightOverlay cells={} sample={:?}", s.len(), s.first()))
        .unwrap_or_else(|| "LightOverlay: no current scan".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn walking_preserves_overlapping_cells_and_a_large_move_drops_them() {
        let order = nearby(4);
        let cells = order
            .iter()
            .map(|[x, z]| {
                Some(Cell {
                    position: [*x, 64, *z],
                    block: 0,
                    effective: 4,
                })
            })
            .collect();
        let mut state = State {
            world: (1, 2, 3, 4, 5),
            center: [0, 64, 0],
            radius: 4,
            cursor: order.len(),
            cells,
            order,
            names: HashMap::new(),
            cycle_at: 500,
            published: 500,
        };
        recenter(&mut state, [1, 64, 0]);
        let kept = state.cells.iter().flatten().count();
        assert!(kept > 30 && kept < state.cells.len());
        assert_eq!(state.cursor, 0);
        for (cell, [dx, dz]) in state.cells.iter().zip(&state.order) {
            if let Some(c) = cell {
                assert_eq!([c.position[0], c.position[2]], [1 + dx, *dz]);
            }
        }
        recenter(&mut state, [100, 64, 100]);
        assert!(state.cells.iter().all(Option::is_none));
    }
    #[test]
    fn scan_starts_at_the_feet_and_covers_each_nearby_column_once() {
        for r in [4, 8, 12] {
            let order = nearby(r);
            assert_eq!(order[0], [0, 0]);
            assert_eq!(
                order.iter().collect::<std::collections::HashSet<_>>().len(),
                order.len()
            );
            assert!(order
                .windows(2)
                .all(|v| v[0][0].pow(2) + v[0][1].pow(2) <= v[1][0].pow(2) + v[1][1].pow(2)));
            for x in -r..=r {
                for z in -r..=r {
                    assert_eq!(order.contains(&[x, z]), x * x + z * z <= r * r);
                }
            }
        }
    }
}
