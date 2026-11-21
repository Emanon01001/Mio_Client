//! Minimap terrain: the top visible block of each column around the player,
//! read a little at a time on the game thread with the same budget as the
//! block scan (about 1 ms every 16 ms). Read only; never loads chunks.
//!
//! Columns are kept in a square grid indexed by world position modulo its
//! side. Fresh columns are reused; unknown and nearby columns expire sooner.
//! Incomplete columns resume at the next sample. Changed cave heights invalidate
//! their layer, and unchanged maps retain the same published Arc.
//!
//! Surface columns read once are also remembered (`Memory`, by chunk, per
//! level and dimension) until the client is unloaded, and fill the map where
//! nothing can be read now (unloaded chunks, columns not read yet). Explicit
//! archives retain immutable chunk handles while later samples keep updating.

use super::super::terrain_session as sessions;
use super::{binding, block_name, GetBlock, World, GET_BLOCK};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

/// The farthest a map reaches from the player, in blocks.
pub const MAX_RADIUS: i32 = 64;
pub const MIN_RADIUS: i32 = 8;
const SIDE: i32 = 2 * MAX_RADIUS + 1;
const INTERVAL_MS: u64 = 16;
const BUDGET: Duration = Duration::from_millis(1);
const MAX_READS: usize = 4096;
const PUBLISH_MS: u64 = 150;
/// How far above a column's last known surface its next search starts, so
/// a column that grew (a tree, a building) is found again.
const MARGIN: i32 = 16;
/// How far down a column is searched before it is left empty.
const DEPTH: i32 = 64;

/// Features using the surface (the minimap, the world map): counted, so one
/// turning off does not stop it for the other.
static USERS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The native dimension the surface was last read in (0: none yet).
static DIMENSION: AtomicUsize = AtomicUsize::new(0);
static RADIUS: AtomicI32 = AtomicI32::new(32);
static LAST: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<Option<State>> = Mutex::new(None);
static SNAPSHOT: Mutex<Option<Arc<Map>>> = Mutex::new(None);

/// One column as drawn: the height and colour of its top visible block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Column {
    pub height: i16,
    pub color: [u8; 3],
    /// False until read, and for columns with nothing within reach.
    pub known: bool,
    /// Not read now: taken from an earlier visit (`Memory`).
    pub remembered: bool,
}

/// Chunks remembered at most (about 2 KB each: 8192 chunks are 16 MB, a
/// square of about 1.4 km). Beyond it the chunks farthest from the player go.
const MAX_CHUNKS: usize = 8192;
/// Fresh reads compared with the memory before it is shown again after the
/// world was bound anew (see `Memory::store`).
const CHECKS: u32 = 32;

type Chunk = Arc<[Column; 256]>;

/// An owned, immutable terrain snapshot; no native pointer is dereferenced by
/// storage. The owner token is session-local and is never written to disk.
pub(crate) struct SavedTerrain {
    pub owner: (usize, usize, u64),
    pub chunks: Vec<SavedChunk>,
}
#[derive(Clone)]
pub(crate) struct SavedChunk {
    pub position: [i32; 2],
    pub columns: Arc<[Column; 256]>,
}
static MEMORY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// The remembered surface of one dimension.
#[derive(Default)]
struct Dimension {
    chunks: HashMap<(i32, i32), Chunk>,
    /// Shown on the map. False after the world is bound anew, until fresh
    /// reads agree with it: a new level can reuse a freed level's addresses.
    trusted: bool,
    agree: u32,
    disagree: u32,
    storage_name: Option<String>,
}

/// Surface columns read before, per level (the player's entity registry)
/// and dimension (its native object), kept while the client is loaded.
#[derive(Default)]
struct Memory {
    registry: usize,
    dimensions: HashMap<usize, Dimension>,
    stable_id: Option<Arc<str>>,
    session: u64,
}

impl Memory {
    fn bind_auto(
        &mut self,
        registry: usize,
        dimension: usize,
        identity: Option<&sessions::Identity>,
        name: Option<&str>,
        session: u64,
    ) {
        let key = identity.map(|i| Arc::clone(&i.id));
        if self.stable_id != key || self.session != session {
            self.dimensions.clear();
            changed();
        }
        self.stable_id = key;
        self.session = session;
        if self
            .dimensions
            .get(&dimension)
            .is_some_and(|d| d.storage_name.as_deref() != name)
        {
            self.dimensions.remove(&dimension);
            changed();
        }
        self.bind(registry, dimension);
        let d = self.dimensions.get_mut(&dimension).unwrap();
        d.storage_name = name.map(str::to_owned);
        if identity.is_some() {
            d.trusted = true;
        }
    }
    /// The world was bound anew: forget other levels, and check this
    /// dimension's memory against fresh reads before showing it again.
    fn bind(&mut self, registry: usize, dimension: usize) {
        MEMORY_EPOCH.fetch_add(1, Ordering::Relaxed);
        if self.registry != registry {
            self.registry = registry;
            self.dimensions.clear();
            changed();
        }
        let d = self.dimensions.entry(dimension).or_default();
        if d.trusted && !d.chunks.is_empty() {
            changed();
        }
        d.trusted = d.chunks.is_empty();
        d.agree = 0;
        d.disagree = 0;
    }

    /// Records a column just read at (x, z). Returns true when the memory of
    /// `dimension` has just become trusted (the map should be published).
    fn store(&mut self, dimension: usize, x: i32, z: i32, column: Column) -> bool {
        let d = self.dimensions.entry(dimension).or_default();
        let key = (x.div_euclid(16), z.div_euclid(16));
        let index = (z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize;
        let mut trusted_now = false;
        if !d.trusted {
            if let Some(old) = d.chunks.get(&key).map(|c| c[index]).filter(|c| c.known) {
                if old.color == column.color && old.height.abs_diff(column.height) <= 1 {
                    d.agree += 1;
                } else {
                    d.disagree += 1;
                }
                if d.agree + d.disagree >= CHECKS {
                    // Mostly the same terrain: the same level. Otherwise the
                    // memory belongs to another level and is dropped.
                    if d.agree * 4 < (d.agree + d.disagree) * 3 {
                        d.chunks.clear();
                    }
                    d.trusted = true;
                    trusted_now = true;
                    changed();
                }
            }
        }
        let chunk = d
            .chunks
            .entry(key)
            .or_insert_with(|| Arc::new([Column::default(); 256]));
        let column = Column {
            remembered: false,
            ..column
        };
        if chunk[index] != column {
            Arc::make_mut(chunk)[index] = column;
            changed();
        }
        trusted_now
    }

    /// The remembered column at (x, z), if any and trusted.
    fn get(&self, dimension: usize, x: i32, z: i32) -> Option<Column> {
        let d = self.dimensions.get(&dimension).filter(|d| d.trusted)?;
        let chunk = d.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        Some(chunk[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize])
            .filter(|c| c.known)
            .map(|c| Column {
                remembered: true,
                ..c
            })
    }

    fn chunks(&self) -> usize {
        self.dimensions.values().map(|d| d.chunks.len()).sum()
    }

    /// Keeps at most `MAX_CHUNKS`: other dimensions go first, then the
    /// chunks of `dimension` farthest from (x, z), down to 7/8 of the limit.
    fn trim(&mut self, dimension: usize, x: i32, z: i32) {
        if self.chunks() <= MAX_CHUNKS {
            return;
        }
        self.dimensions
            .retain(|&k, d| k == dimension && !d.chunks.is_empty());
        let Some(d) = self.dimensions.get_mut(&dimension) else {
            return;
        };
        let keep = MAX_CHUNKS * 7 / 8;
        if d.chunks.len() <= keep {
            return;
        }
        let center = (x.div_euclid(16), z.div_euclid(16));
        let distance = |&(cx, cz): &(i32, i32)| {
            let (dx, dz) = ((cx - center.0) as i64, (cz - center.1) as i64);
            dx * dx + dz * dz
        };
        let mut far: Vec<(i32, i32)> = d.chunks.keys().copied().collect();
        far.sort_unstable_by_key(|k| std::cmp::Reverse(distance(k)));
        for key in &far[..far.len() - keep] {
            d.chunks.remove(key);
            changed();
        }
    }
}

static MEMORY: Mutex<Option<Memory>> = Mutex::new(None);
/// Bumped whenever what `remembered_area` can return changes (a column
/// differs, chunks or trust come or go), so a map built from it knows
/// whether building again would give anything new.
static REVISION: AtomicU64 = AtomicU64::new(0);

fn changed() {
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// The memory's revision (see `REVISION`).
pub fn memory_revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}
static REMEMBERED: AtomicUsize = AtomicUsize::new(0);
static DISPLAY_CURRENT: AtomicBool = AtomicBool::new(true);
fn display_ready() -> bool {
    !sessions::running() || DISPLAY_CURRENT.load(Ordering::Acquire)
}

/// For benchmarks: fills the memory of dimension 1 with `side` x `side`
/// chunks of varied terrain around the origin, trusted and current.
#[cfg(test)]
pub(crate) fn fill_memory_for_bench(side: i32) {
    let mut m = Memory::default();
    m.bind(1, 1);
    let half = side / 2;
    for cz in -half..side - half {
        for cx in -half..side - half {
            for i in 0..256 {
                let (x, z) = (cx * 16 + i % 16, cz * 16 + i / 16);
                let h = 64 + ((x / 7 + z / 5) % 4) as i16;
                let c = [(x / 9 % 5 * 40) as u8, 120, (z / 11 % 3 * 60) as u8];
                m.store(
                    1,
                    x,
                    z,
                    Column {
                        height: h,
                        color: c,
                        known: true,
                        remembered: false,
                    },
                );
            }
        }
    }
    *MEMORY.lock().unwrap_or_else(|e| e.into_inner()) = Some(m);
    DIMENSION.store(1, Ordering::Relaxed);
}

/// How many chunks of terrain are remembered (all dimensions).
pub fn remembered_chunks() -> usize {
    REMEMBERED.load(Ordering::Relaxed)
}

/// Drops the remembered terrain. Called when the client is unloaded (static
/// memory is never freed otherwise) and from the minimap's settings.
pub fn forget() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *MEMORY.lock().unwrap_or_else(|e| e.into_inner()) = None;
    MEMORY_EPOCH.fetch_add(1, Ordering::Relaxed);
    changed();
    REMEMBERED.store(0, Ordering::Relaxed);
    DIMENSION.store(0, Ordering::Relaxed);
    LAST.store(0, Ordering::Relaxed);
    // Rebind next sample: keeping the old State would leave MEMORY=None
    // indefinitely while the player remains in the same world.
    *state = None;
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn context_for(state: &State, memory: &Memory) -> Option<sessions::Context> {
    let identity = state.identity.as_ref()?;
    let dimension = state.storage_dimension.as_deref()?;
    if memory.registry != state.world.0
        || memory.stable_id.as_deref() != Some(identity.id.as_ref())
        || memory.session != state.session
    {
        return None;
    }
    Some(sessions::Context {
        owner: (
            memory.registry,
            state.world.2,
            MEMORY_EPOCH.load(Ordering::Relaxed),
        ),
        entry: sessions::Entry {
            id: Arc::clone(&identity.id),
            dimension: dimension.into(),
            registry: memory.registry,
            session: state.session,
        },
        name: Arc::clone(&identity.name),
        revision: memory_revision(),
        center: [state.center[0], state.center[2]],
    })
}
fn owned_session(state: &State, memory: &Memory) -> Option<sessions::Snapshot> {
    let context = context_for(state, memory)?;
    let data = memory
        .dimensions
        .get(&state.world.2)
        .filter(|d| d.trusted && !d.chunks.is_empty())?;
    let chunks = data
        .chunks
        .iter()
        .map(|(&(x, z), c)| SavedChunk {
            position: [x, z],
            columns: Arc::clone(c),
        })
        .collect();
    Some(sessions::Snapshot { context, chunks })
}
pub(crate) fn auto_context() -> Option<sessions::Context> {
    if !display_ready() {
        return None;
    }
    let state = STATE.try_lock().ok()?;
    let state = state.as_ref()?;
    let memory = MEMORY.try_lock().ok()?;
    context_for(state, memory.as_ref()?)
}
pub(crate) fn auto_snapshot(owner: sessions::Owner) -> Option<sessions::Snapshot> {
    let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let memory = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = owned_session(state.as_ref()?, memory.as_ref()?)?;
    (snapshot.context.owner == owner).then_some(snapshot)
}
/// Retain the old world's last columns before its binding disappears.
pub(crate) fn finish_absent(now: u64) {
    if !sessions::absent(now) {
        return;
    }
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    if !sessions::absent(now) {
        return;
    }
    let Ok(mut memory) = MEMORY.try_lock() else {
        return;
    };
    if let (Some(state), Some(memory)) = (state.as_ref(), memory.as_ref()) {
        if let Some(snapshot) = owned_session(state, memory) {
            if !sessions::retire(snapshot) {
                return;
            }
        }
    }
    *state = None;
    *memory = None;
    MEMORY_EPOCH.fetch_add(1, Ordering::Relaxed);
    DIMENSION.store(0, Ordering::Relaxed);
    REMEMBERED.store(0, Ordering::Relaxed);
    DISPLAY_CURRENT.store(false, Ordering::Release);
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    changed();
}
/// Freeze attribution and enqueue the final snapshot under the same locks
/// used by producers, so no last sample can slip between capture and stop.
pub(crate) fn finish_auto() -> Result<(), String> {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let memory = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    if let (Some(s), Some(m)) = (state.as_ref(), memory.as_ref()) {
        if let Some(snapshot) = owned_session(s, m) {
            if !sessions::retire(snapshot) {
                return Err("最終保存の待機列が使用中です".into());
            }
        }
    }
    sessions::set_running(false);
    if let Some(state) = state.as_mut() {
        state.identity = None;
    }
    Ok(())
}

fn handoff(state: Option<&State>, memory: Option<&Memory>) -> bool {
    if !sessions::running() {
        return true;
    }
    match state.zip(memory).and_then(|(s, m)| owned_session(s, m)) {
        Some(snapshot) => sessions::retire(snapshot),
        None => true,
    }
}

/// Clone chunk handles only. Encoding and filesystem work run on the client
/// worker; subsequent native samples copy just the chunks they actually edit.
pub(crate) fn save_snapshot() -> Result<SavedTerrain, String> {
    if !display_ready() {
        return Err("ワールドを切り替えています".into());
    }
    let guard = MEMORY
        .try_lock()
        .map_err(|_| "地形を更新中です。もう一度お試しください")?;
    let memory = guard.as_ref().ok_or("地形データがありません")?;
    let dimension = DIMENSION.load(Ordering::Relaxed);
    let data = memory
        .dimensions
        .get(&dimension)
        .filter(|d| d.trusted)
        .ok_or("ワールドの地形を確認中です。少し待ってください")?;
    if data.chunks.is_empty() {
        return Err("保存する地形がありません".into());
    }
    Ok(SavedTerrain {
        owner: (
            memory.registry,
            dimension,
            MEMORY_EPOCH.load(Ordering::Relaxed),
        ),
        chunks: data
            .chunks
            .iter()
            .map(|(&(x, z), columns)| SavedChunk {
                position: [x, z],
                columns: Arc::clone(columns),
            })
            .collect(),
    })
}

/// Read the live binding even when there are no remembered chunks yet.
pub(crate) fn storage_context() -> Result<ArchiveContext, String> {
    let state = STATE
        .try_lock()
        .map_err(|_| "地形を更新中です。もう一度お試しください")?;
    let state = state
        .as_ref()
        .ok_or("MinimapまたはWorldMapで地形を取得してください")?;
    let owner = (
        state.world.0,
        state.world.2,
        MEMORY_EPOCH.load(Ordering::Relaxed),
    );
    let dimension = state
        .storage_dimension
        .clone()
        .ok_or("ディメンション情報を取得できません")?;
    Ok(ArchiveContext {
        owner,
        dimension,
        world_id: state.identity.as_ref().map(|i| i.id.to_string()),
    })
}
pub(crate) struct ArchiveContext {
    pub owner: sessions::Owner,
    pub dimension: String,
    pub world_id: Option<String>,
}

/// Merge only unknown columns: live observations always win over disk data.
/// A load that finishes after a world/dimension change cannot affect it.
pub(crate) fn restore(
    owner: (usize, usize, u64),
    chunks: Vec<SavedChunk>,
) -> Result<usize, String> {
    restore_inner(owner, chunks, None)
}
pub(crate) fn restore_auto(
    context: &sessions::Context,
    chunks: Vec<SavedChunk>,
) -> Result<usize, String> {
    restore_inner(context.owner, chunks, Some(context))
}
fn restore_inner(
    owner: sessions::Owner,
    mut chunks: Vec<SavedChunk>,
    expected: Option<&sessions::Context>,
) -> Result<usize, String> {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = state
        .as_mut()
        .ok_or("ワールドから離れたため読み込みを中止しました")?;
    let mut memory = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    let memory = memory
        .as_mut()
        .ok_or("地形が消去されたため読み込みを中止しました")?;
    if let Some(expected) = expected {
        if context_for(state, memory).is_none_or(|c| c.entry != expected.entry) {
            return Err("自動復元先のワールドが変わりました".into());
        }
    }
    if owner
        != (
            state.world.0,
            state.world.2,
            MEMORY_EPOCH.load(Ordering::Relaxed),
        )
        || memory.registry != owner.0
        || DIMENSION.load(Ordering::Relaxed) != owner.1
    {
        return Err("ワールドまたはディメンションが変わったため読み込みを中止しました".into());
    }
    let existing = memory.chunks();
    let data = memory
        .dimensions
        .get_mut(&owner.1)
        .filter(|d| d.trusted)
        .ok_or("ワールドの地形を確認中です")?;
    if expected.is_some() {
        let center = (
            state.center[0].div_euclid(16),
            state.center[2].div_euclid(16),
        );
        chunks.sort_unstable_by_key(|c| {
            let dx = c.position[0] as i64 - center.0 as i64;
            let dz = c.position[1] as i64 - center.1 as i64;
            (dx * dx + dz * dz, c.position)
        });
        let mut room = MAX_CHUNKS.saturating_sub(existing);
        chunks.retain(|c| {
            if data.chunks.contains_key(&(c.position[0], c.position[1])) {
                true
            } else if room != 0 {
                room -= 1;
                true
            } else {
                false
            }
        });
    }
    let extra = chunks
        .iter()
        .filter(|c| !data.chunks.contains_key(&(c.position[0], c.position[1])))
        .count();
    if memory_chunk_limit(existing, extra).is_none() {
        return Err("読み込み後の地形が8192チャンクの上限を超えます".into());
    }
    let mut added = 0;
    for chunk in chunks {
        let key = (chunk.position[0], chunk.position[1]);
        match data.chunks.entry(key) {
            std::collections::hash_map::Entry::Vacant(e) => {
                added += chunk.columns.iter().filter(|c| c.known).count();
                e.insert(chunk.columns);
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                for (i, c) in chunk.columns.iter().enumerate().filter(|(_, c)| c.known) {
                    if !e.get()[i].known {
                        Arc::make_mut(e.get_mut())[i] = *c;
                        added += 1;
                    }
                }
            }
        }
    }
    if added != 0 {
        changed();
        state.dirty = true;
        state.published = 0;
        REMEMBERED.store(memory.chunks(), Ordering::Relaxed);
    }
    Ok(added)
}
fn memory_chunk_limit(existing: usize, extra: usize) -> Option<usize> {
    existing.checked_add(extra).filter(|&n| n <= MAX_CHUNKS)
}

/// The columns within `radius` of `center` (x, z), row by row from the
/// north-west corner: index `(dz + radius) * side + (dx + radius)`.
#[derive(Debug)]
pub struct Map {
    pub center: [i32; 2],
    pub radius: i32,
    pub columns: Vec<Column>,
}

impl Map {
    pub fn side(&self) -> usize {
        (2 * self.radius + 1) as usize
    }
    /// The column at offset (dx, dz) from the center, if inside the map.
    pub fn at(&self, dx: i32, dz: i32) -> Option<Column> {
        if !(-self.radius..=self.radius).contains(&dx)
            || !(-self.radius..=self.radius).contains(&dz)
        {
            return None;
        }
        let side = self.side();
        self.columns
            .get((dz + self.radius) as usize * side + (dx + self.radius) as usize)
            .copied()
    }
}

#[derive(Clone, Copy, Default)]
struct Cell {
    x: i32,
    z: i32,
    at: u64,
    column: Column,
}

struct State {
    world: World,
    identity: Option<sessions::Identity>,
    session: u64,
    storage_dimension: Option<String>,
    cells: Vec<Cell>,
    /// Colour of each block state seen, or None for see-through blocks.
    colors: HashMap<usize, Option<[u8; 3]>>,
    cursor: usize,
    published: u64,
    order: Vec<(i32, i32)>,
    radius: i32,
    center: [i32; 3],
    /// Where searches without a hint of their own start: above the highest
    /// surface known on the map (None: nothing known yet).
    sky: Option<i32>,
    pending: Option<Pending>,
    dirty: bool,
    idle_until: u64,
}
#[derive(Clone, Copy)]
struct Pending {
    x: i32,
    z: i32,
    y: i32,
    bottom: i32,
}
/// None means budget exhausted: retain the current y and resume next sample.
fn scan_column(
    p: &mut Pending,
    reads: &mut usize,
    mut yield_now: impl FnMut() -> bool,
    mut color: impl FnMut([i32; 3]) -> Option<Option<[u8; 3]>>,
) -> Option<Column> {
    while p.y >= p.bottom {
        if *reads >= MAX_READS || ((*reads).is_multiple_of(16) && yield_now()) {
            return None;
        }
        *reads += 1;
        match color([p.x, p.y, p.z]) {
            None => return Some(Column::default()),
            Some(None) => p.y -= 1,
            Some(Some(color)) => {
                return Some(Column {
                    height: p.y as i16,
                    color,
                    known: true,
                    remembered: false,
                })
            }
        }
    }
    Some(Column::default())
}
fn fresh_cell(cell: &Cell, x: i32, z: i32, now: u64, near: bool) -> bool {
    cell.at != 0
        && cell.x == x
        && cell.z == z
        && now.saturating_sub(cell.at)
            < if !cell.column.known {
                500
            } else if near {
                1000
            } else {
                5000
            }
}

/// Where a column's search for its surface starts and ends, within the
/// world's height `min..max`: above its own last surface if known (searching
/// `DEPTH` blocks down); otherwise above the highest surface known nearby
/// (`sky`), or at the top of the world, searching down to the bottom.
fn search(own: Option<i16>, sky: Option<i32>, min: i32, max: i32) -> (i32, i32) {
    match (own, sky) {
        (Some(height), _) => {
            let top = (height as i32 + MARGIN).clamp(min, max - 1);
            (top, (top - DEPTH).max(min))
        }
        (None, Some(sky)) => (sky.clamp(min, max - 1), min),
        (None, None) => (max - 1, min),
    }
}

impl State {
    fn new(world: World) -> Self {
        Self {
            world,
            identity: None,
            session: sessions::session(),
            storage_dimension: None,
            cells: vec![Cell::default(); (SIDE * SIDE) as usize],
            colors: HashMap::new(),
            cursor: 0,
            published: 0,
            order: Vec::new(),
            radius: 0,
            center: [i32::MIN; 3],
            sky: None,
            pending: None,
            dirty: true,
            idle_until: 0,
        }
    }
}

fn slot(x: i32, z: i32) -> usize {
    (z.rem_euclid(SIDE) * SIDE + x.rem_euclid(SIDE)) as usize
}

/// Every offset of the largest map, nearest first.
fn order() -> &'static [(i32, i32)] {
    static ORDER: OnceLock<Vec<(i32, i32)>> = OnceLock::new();
    ORDER.get_or_init(|| {
        let mut all: Vec<(i32, i32)> = (-MAX_RADIUS..=MAX_RADIUS)
            .flat_map(|dz| (-MAX_RADIUS..=MAX_RADIUS).map(move |dx| (dx, dz)))
            .collect();
        all.sort_by_key(|&(dx, dz)| (dx * dx + dz * dz, dz, dx));
        all
    })
}

pub fn set_radius(radius: i32) {
    RADIUS.store(radius.clamp(MIN_RADIUS, MAX_RADIUS), Ordering::Relaxed);
}

/// One user starts (`true`) or stops (`false`) using the surface; reading
/// runs while any user is on. Each `true` needs one matching `false`.
pub fn set_active(active: bool) {
    let users = if active {
        USERS.fetch_add(1, Ordering::AcqRel) + 1
    } else {
        USERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
            .map_or(0, |n| n - 1)
    };
    ACTIVE.store(users > 0, Ordering::Release);
    LAST.store(0, Ordering::Relaxed);
    if users == 0 {
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The remembered surface of the dimension last read in, sampled on a grid
/// of `cols` x `rows` cells of `step` blocks whose first cell starts at
/// block (x, z): each cell holds the column at its centre (default: none),
/// row by row. Empty cells while the memory is not trusted.
pub fn remembered_area(x: i32, z: i32, step: i32, cols: usize, rows: usize) -> Vec<Column> {
    let mut out = vec![Column::default(); cols * rows];
    if !display_ready() {
        return out;
    }
    let step = step.max(1);
    let guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(d) = guard
        .as_ref()
        .and_then(|m| m.dimensions.get(&DIMENSION.load(Ordering::Relaxed)))
        .filter(|d| d.trusted)
    else {
        return out;
    };
    // Neighbouring cells usually share a chunk: look each up once in a row.
    let mut last: Option<((i32, i32), Option<&Chunk>)> = None;
    for row in 0..rows {
        let bz = z + row as i32 * step + step / 2;
        for col in 0..cols {
            let bx = x + col as i32 * step + step / 2;
            let key = (bx.div_euclid(16), bz.div_euclid(16));
            let chunk = match last {
                Some((k, chunk)) if k == key => chunk,
                _ => {
                    let chunk = d.chunks.get(&key);
                    last = Some((key, chunk));
                    chunk
                }
            };
            if let Some(chunk) = chunk {
                let c = chunk[(bz.rem_euclid(16) * 16 + bx.rem_euclid(16)) as usize];
                if c.known {
                    out[row * cols + col] = c;
                }
            }
        }
    }
    out
}

/// The latest map, if the feature is on and the player is in a world.
pub fn snapshot() -> Option<Arc<Map>> {
    if !display_ready() {
        return None;
    }
    if !ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The colour a block shows on the map, or None for blocks that are seen
/// through from above (air, plants without a collision box, torches...).
pub(crate) fn color(name: &str) -> Option<[u8; 3]> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    const SEE_THROUGH: &[&str] = &[
        "air",
        "light_block",
        "structure_void",
        "barrier",
        "short_grass",
        "tall_grass",
        "tallgrass",
        "fern",
        "large_fern",
        "double_plant",
        "deadbush",
        "dead_bush",
        "red_flower",
        "yellow_flower",
        "dandelion",
        "poppy",
        "blue_orchid",
        "allium",
        "azure_bluet",
        "oxeye_daisy",
        "cornflower",
        "lily_of_the_valley",
        "wither_rose",
        "sunflower",
        "lilac",
        "rose_bush",
        "peony",
        "torchflower",
        "pink_petals",
        "vine",
        "glow_lichen",
        "torch",
        "soul_torch",
        "redstone_torch",
        "unlit_redstone_torch",
        "redstone_wire",
        "rail",
        "golden_rail",
        "detector_rail",
        "activator_rail",
        "lever",
        "tripwire",
        "trip_wire",
        "tripwire_hook",
        "web",
        "cobweb",
        "fire",
        "soul_fire",
        "reeds",
        "sugar_cane",
        "brown_mushroom",
        "red_mushroom",
        "sweet_berry_bush",
        "wheat",
        "carrots",
        "potatoes",
        "beetroot",
        "nether_sprouts",
        "crimson_roots",
        "warped_roots",
        "crimson_fungus",
        "warped_fungus",
        "hanging_roots",
        "cave_vines",
        "string",
        "ladder",
    ];
    if SEE_THROUGH.contains(&name)
        || [
            "_sapling",
            "_button",
            "_pressure_plate",
            "_sign",
            "_banner",
            "_tulip",
        ]
        .iter()
        .any(|s| name.ends_with(s))
    {
        return None;
    }
    const DYES: [(&str, [u8; 3]); 16] = [
        ("light_blue", [100, 160, 210]),
        ("light_gray", [150, 150, 145]),
        ("white", [225, 228, 230]),
        ("orange", [220, 115, 30]),
        ("magenta", [170, 60, 160]),
        ("yellow", [235, 190, 45]),
        ("lime", [110, 175, 35]),
        ("pink", [215, 120, 150]),
        ("gray", [65, 70, 75]),
        ("cyan", [25, 125, 135]),
        ("purple", [110, 40, 160]),
        ("blue", [50, 55, 145]),
        ("brown", [100, 65, 40]),
        ("green", [75, 95, 35]),
        ("red", [145, 35, 35]),
        ("black", [25, 25, 30]),
    ];
    let has = |part: &str| name.contains(part);
    Some(match name {
        n if n.ends_with("water")
            || n == "kelp"
            || n.contains("seagrass")
            || n == "bubble_column" =>
        {
            [52, 92, 180]
        }
        n if n.ends_with("lava") || n.starts_with("magma") => [210, 90, 20],
        "ice" | "packed_ice" | "blue_ice" | "frosted_ice" => [150, 180, 230],
        "snow" | "snow_layer" | "powder_snow" | "snow_block" => [240, 242, 245],
        "grass_block" | "grass" => [92, 140, 60],
        "grass_path" | "dirt_path" => [150, 125, 75],
        "waterlily" | "lily_pad" => [40, 110, 40],
        "sand" | "suspicious_sand" => [214, 200, 150],
        "red_sand" => [190, 105, 50],
        "gravel" | "suspicious_gravel" => [130, 125, 120],
        "dirt" | "coarse_dirt" | "rooted_dirt" | "farmland" => [125, 90, 60],
        "podzol" => [100, 70, 40],
        "mycelium" => [110, 95, 110],
        "mud" | "packed_mud" => [70, 60, 55],
        "clay" => [160, 165, 175],
        "moss_block" | "moss_carpet" => [90, 120, 50],
        "cactus" => [80, 130, 50],
        "bamboo" => [100, 140, 40],
        "pumpkin" | "carved_pumpkin" | "lit_pumpkin" => [215, 125, 30],
        "melon_block" | "melon" => [110, 150, 40],
        "netherrack" => [110, 45, 45],
        "crimson_nylium" | "nether_wart_block" | "shroomlight" => [140, 25, 30],
        "warped_nylium" | "warped_wart_block" => [30, 120, 110],
        "soul_sand" | "soul_soil" => [85, 65, 50],
        "basalt" | "polished_basalt" | "smooth_basalt" | "blackstone" => [55, 55, 60],
        "end_stone" | "end_bricks" => [220, 220, 160],
        "obsidian" | "crying_obsidian" => [25, 20, 40],
        "bedrock" => [60, 60, 60],
        "glowstone" => [230, 200, 120],
        "glass" => [180, 210, 220],
        n if n.contains("deepslate") || n == "tuff" => [80, 80, 85],
        _ if has("leaves") => [50, 105, 40],
        n if n.ends_with("_log") || n == "log" || n == "log2" || n.ends_with("_wood") => {
            [105, 80, 50]
        }
        n if n.ends_with("_stem") || n.ends_with("_hyphae") => [90, 50, 70],
        n if n.ends_with("planks") => [160, 130, 80],
        n if has("sandstone") => {
            if n.starts_with("red_") {
                [180, 100, 50]
            } else {
                [210, 195, 145]
            }
        }
        n if has("terracotta") || has("hardened_clay") => DYES
            .iter()
            .find(|(dye, _)| n.starts_with(dye))
            .map_or([150, 90, 65], |&(_, c)| c.map(|v| (v as u16 * 3 / 4) as u8)),
        n if has("wool") || has("concrete") || has("carpet") || has("stained_glass") => DYES
            .iter()
            .find(|(dye, _)| n.starts_with(dye))
            .map_or([200, 200, 200], |&(_, c)| c),
        n if has("stone")
            || has("cobble")
            || n.ends_with("_ore")
            || has("andesite")
            || has("diorite")
            || has("granite")
            || has("calcite")
            || has("brick") =>
        {
            [125, 125, 125]
        }
        _ => [140, 140, 140],
    })
}

/// Only called inside the HUD hook, while the local player/dimension are live.
pub(in crate::modules::game) unsafe fn sample(player: usize, center: [i32; 3]) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let now = GetTickCount64();
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < INTERVAL_MS {
        return;
    }
    let Some(Ok(&getter)) = GET_BLOCK.get().map(|r| r.as_ref()) else {
        return;
    };
    let Ok(mut guard) = STATE.try_lock() else {
        return;
    };
    // A concurrent accessor may have sampled between the first check and
    // acquiring the state. Keep the same interval without duplicate scans.
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < INTERVAL_MS {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let mut profile = crate::modules::performance_profiler::metrics::Scope::new(
        crate::modules::performance_profiler::metrics::Stage::SurfaceScan,
    );
    let Some((world, source, min, max)) = binding(player, getter) else {
        DISPLAY_CURRENT.store(false, Ordering::Release);
        let Ok(memory) = MEMORY.try_lock() else {
            return;
        };
        if !handoff(guard.as_ref(), memory.as_ref()) {
            return;
        }
        *guard = None;
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    };
    let automatic = sessions::running();
    let identity = automatic
        .then(|| super::super::world_info::identity(player))
        .flatten();
    let session = sessions::session();
    let dimension = if automatic {
        super::super::world::storage_dimension(player)
    } else {
        None
    };
    let rebinding = guard.as_ref().is_none_or(|s| {
        s.world != world
            || automatic
                && (s.session != session
                    || s.identity.as_ref().map(|i| (&i.id, i.level))
                        != identity.as_ref().map(|i| (&i.id, i.level))
                    || s.storage_dimension != dimension)
    });
    if rebinding {
        DISPLAY_CURRENT.store(false, Ordering::Release);
        let Ok(mut memory) = MEMORY.try_lock() else {
            return;
        };
        if !handoff(guard.as_ref(), memory.as_ref()) {
            return;
        }
        // The layout is right only if the registry's air block is "air".
        if block_name(world.4).as_deref() != Some("air") {
            *guard = None;
            *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return;
        }
        let mut next = State::new(world);
        next.identity = identity.clone();
        next.session = session;
        next.storage_dimension = dimension.clone();
        let memory = memory.get_or_insert_with(Memory::default);
        if automatic {
            memory.bind_auto(
                world.0,
                world.2,
                identity.as_ref(),
                dimension.as_deref(),
                session,
            );
        } else {
            memory.bind(world.0, world.2);
        }
        *guard = Some(next);
        DIMENSION.store(world.2, Ordering::Relaxed);
    }
    let state = guard.as_mut().unwrap();
    if automatic {
        state.identity = identity;
    }
    // The dimension's text can become available after its native binding.
    // Retry until it is known, then reuse only for this exact live binding.
    if state.storage_dimension.is_none() {
        state.storage_dimension = super::super::world::storage_dimension(player);
    }
    DISPLAY_CURRENT.store(true, Ordering::Release);
    let get: GetBlock = std::mem::transmute(getter);
    let color_at = |state: &mut State, position: [i32; 3]| -> Option<Option<[u8; 3]>> {
        if position[1] < min || position[1] >= max {
            return Some(None);
        }
        let block = get(source, &position);
        if block == 0 {
            return None;
        }
        if block == world.4 {
            return Some(None);
        }
        if let Some(&known) = state.colors.get(&block) {
            return Some(known);
        }
        let shown = color(&block_name(block)?);
        if state.colors.len() >= 16384 {
            state.colors.clear();
        }
        state.colors.insert(block, shown);
        Some(shown)
    };

    // Always the surface, whatever the player's height: the map does not
    // depend on y, only on the column (x, z).
    let radius = RADIUS.load(Ordering::Relaxed).clamp(MIN_RADIUS, MAX_RADIUS);
    if state.radius != radius {
        state.order = order()
            .iter()
            .copied()
            .filter(|&(x, z)| x.abs() <= radius && z.abs() <= radius)
            .collect();
        state.radius = radius;
        state.cursor = 0;
        state.pending = None;
        state.dirty = true;
        state.idle_until = 0;
    }
    if state.center[0] != center[0] || state.center[2] != center[2] {
        state.cursor = 0;
        state.pending = None;
        state.idle_until = 0;
        state.dirty = true;
        state.published = 0;
        state.center = center;
    }

    // Only this thread writes the memory; `forget` may hold it briefly.
    let mut memory = MEMORY.try_lock().ok();
    let start = Instant::now();
    let mut reads = 0;
    let mut visited = 0;
    while now >= state.idle_until && reads < MAX_READS && visited < state.order.len() {
        if start.elapsed() >= BUDGET {
            break;
        }
        let mut pending = if let Some(p) = state.pending.take() {
            p
        } else {
            let (dx, dz) = state.order[state.cursor];
            state.cursor = (state.cursor + 1) % state.order.len();
            visited += 1;
            let (x, z) = (center[0] + dx, center[2] + dz);
            let cell = state.cells[slot(x, z)];
            if fresh_cell(&cell, x, z, now, dx * dx + dz * dz <= 64) {
                continue;
            }
            let own = (cell.at != 0 && cell.x == x && cell.z == z && cell.column.known)
                .then_some(cell.column.height)
                .or_else(|| {
                    let memory = memory.as_ref()?.as_ref()?;
                    Some(memory.get(state.world.2, x, z)?.height)
                });
            let (top, bottom) = search(own, state.sky, min, max);
            Pending {
                x,
                z,
                y: top,
                bottom,
            }
        };
        let Some(column) = scan_column(
            &mut pending,
            &mut reads,
            || start.elapsed() >= BUDGET,
            |p| color_at(state, p),
        ) else {
            state.pending = Some(pending);
            break;
        };
        let (x, z) = (pending.x, pending.z);
        if column.known {
            let above = column.height as i32 + MARGIN;
            state.sky = Some(state.sky.map_or(above, |sky| sky.max(above)));
            if let Some(memory) = memory.as_mut().and_then(|m| m.as_mut()) {
                state.dirty |= memory.store(state.world.2, x, z, column);
            }
        }
        let old = state.cells[slot(x, z)];
        if old.x != x || old.z != z || old.column != column {
            state.dirty = true;
        }
        state.cells[slot(x, z)] = Cell {
            x,
            z,
            at: now,
            column,
        };
    }
    if visited == state.order.len() && reads == 0 {
        state.idle_until = now.saturating_add(100);
    }
    profile.units(reads);

    if state.dirty && now.saturating_sub(state.published) >= PUBLISH_MS {
        state.dirty = false;
        state.published = now;
        let side = 2 * radius + 1;
        let mut columns = Vec::with_capacity((side * side) as usize);
        // The map falls back to the memory where nothing was read.
        let mut recall = memory.as_mut().and_then(|m| m.as_mut());
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let (x, z) = (center[0] + dx, center[2] + dz);
                let cell = state.cells[slot(x, z)];
                let read = (cell.at != 0 && cell.x == x && cell.z == z)
                    .then_some(cell.column)
                    .filter(|c| c.known);
                columns.push(
                    read.or_else(|| recall.as_ref()?.get(state.world.2, x, z))
                        .unwrap_or_default(),
                );
            }
        }
        if let Some(memory) = recall.as_mut() {
            memory.trim(state.world.2, center[0], center[2]);
            REMEMBERED.store(memory.chunks(), Ordering::Relaxed);
        }
        // Lower the start of unhinted searches again after leaving high ground.
        if let Some(highest) = columns.iter().filter(|c| c.known).map(|c| c.height).max() {
            state.sky = Some(highest as i32 + MARGIN);
        }
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(Map {
            center: [center[0], center[2]],
            radius,
            columns,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that use the shared statics run one at a time.
    static SURFACE_TEST: Mutex<()> = Mutex::new(());

    #[test]
    fn automatic_binding_separates_world_ids_dimensions_and_reused_native_addresses() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let identity = |id: &str| sessions::Identity {
            id: id.into(),
            name: "same name".into(),
            level: 3,
        };
        let mut memory = Memory::default();
        memory.bind_auto(1, 10, Some(&identity("A")), Some("minecraft:overworld"), 1);
        memory.store(10, 0, 0, read(64, [1, 2, 3]));
        let mut state = State::new((1, 42, 10, 11, 12));
        state.session = 1;
        state.identity = Some(identity("A"));
        state.storage_dimension = Some("minecraft:overworld".into());
        let final_a = owned_session(&state, &memory).unwrap();
        memory.bind_auto(1, 10, Some(&identity("B")), Some("minecraft:overworld"), 2);
        assert_eq!(
            memory.chunks(),
            0,
            "same registry address may belong to another world"
        );
        assert_eq!(final_a.context.entry.id.as_ref(), "A");
        assert_eq!(
            final_a.chunks[0].columns[0].height, 64,
            "handoff retains the old terrain"
        );
        memory.store(10, 0, 0, read(90, [4, 5, 6]));
        memory.bind_auto(1, 10, Some(&identity("B")), Some("minecraft:nether"), 2);
        assert_eq!(
            memory.chunks(),
            0,
            "same dimension pointer cannot alias another dimension ID"
        );
        memory.store(10, 0, 0, read(30, [4, 5, 6]));
        memory.bind_auto(1, 10, Some(&identity("B")), Some("minecraft:nether"), 3);
        assert_eq!(
            memory.chunks(),
            0,
            "new visit must await its archive rather than trust native address reuse"
        );
        memory.bind_auto(1, 10, None, Some("minecraft:nether"), 3);
        memory.store(10, 0, 0, read(70, [1, 2, 3]));
        state.identity = None;
        assert!(
            owned_session(&state, &memory).is_none(),
            "unknown IDs are never automatically saved"
        );
    }

    #[test]
    fn automatic_restore_rejects_a_foreign_world_even_when_owner_addresses_match() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let identity = sessions::Identity {
            id: "A".into(),
            name: "A".into(),
            level: 3,
        };
        let mut memory = Memory::default();
        memory.bind_auto(1, 10, Some(&identity), Some("minecraft:overworld"), 1);
        memory.store(10, 0, 0, read(80, [1, 2, 3]));
        let mut state = State::new((1, 42, 10, 11, 12));
        state.session = 1;
        state.identity = Some(identity);
        state.storage_dimension = Some("minecraft:overworld".into());
        let mut c = context_for(&state, &memory).unwrap();
        *STATE.lock().unwrap() = Some(state);
        *MEMORY.lock().unwrap() = Some(memory);
        DIMENSION.store(10, Ordering::Relaxed);
        let mut columns = [Column::default(); 256];
        columns[1] = read(64, [4, 5, 6]);
        let chunks = vec![SavedChunk {
            position: [0, 0],
            columns: Arc::new(columns),
        }];
        c.entry.id = "B".into();
        assert!(restore_auto(&c, chunks.clone()).is_err());
        c.entry.id = "A".into();
        c.entry.session += 1;
        assert!(restore_auto(&c, chunks.clone()).is_err());
        c.entry.session -= 1;
        assert_eq!(restore_auto(&c, chunks).unwrap(), 1);
        assert_eq!(
            MEMORY
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .get(10, 0, 0)
                .unwrap()
                .height,
            80
        );
        forget();
    }

    #[test]
    fn clearing_memory_rearms_world_binding_and_invalidates_pending_imports() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut memory = Memory::default();
        memory.bind(1, 10);
        memory.store(10, 0, 0, read(64, [1, 2, 3]));
        *MEMORY.lock().unwrap() = Some(memory);
        *STATE.lock().unwrap() = Some(State::new((1, 42, 10, 11, 12)));
        DIMENSION.store(10, Ordering::Relaxed);
        LAST.store(100, Ordering::Relaxed);
        let snapshot = save_snapshot().unwrap();
        forget();
        assert!(STATE.lock().unwrap().is_none());
        assert!(MEMORY.lock().unwrap().is_none());
        assert_eq!(LAST.load(Ordering::Relaxed), 0);
        assert_eq!(DIMENSION.load(Ordering::Relaxed), 0);
        assert!(restore(snapshot.owner, snapshot.chunks).is_err());
    }

    #[test]
    fn exports_retain_immutable_chunks_and_imports_keep_live_columns_and_owner_boundaries() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut memory = Memory::default();
        memory.bind(1, 10);
        memory.store(10, -1, -1, read(64, [1, 2, 3]));
        *MEMORY.lock().unwrap() = Some(memory);
        let mut state = State::new((1, 42, 10, 11, 12));
        state.storage_dimension = Some("minecraft:overworld".into());
        *STATE.lock().unwrap() = Some(state);
        DIMENSION.store(10, Ordering::Relaxed);
        let before = save_snapshot().unwrap();
        assert_eq!(storage_context().unwrap().owner, before.owner);
        assert_eq!(before.chunks[0].columns[255].height, 64);
        MEMORY
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .store(10, -1, -1, read(70, [4, 5, 6]));
        assert_eq!(
            before.chunks[0].columns[255].height, 64,
            "export cannot change while the game samples"
        );
        let current = save_snapshot().unwrap();
        assert!(!Arc::ptr_eq(
            &before.chunks[0].columns,
            &current.chunks[0].columns
        ));
        let mut incoming = [Column::default(); 256];
        incoming[0] = read(60, [7, 8, 9]);
        incoming[255] = read(64, [1, 2, 3]);
        let added = restore(
            before.owner,
            vec![SavedChunk {
                position: [-1, -1],
                columns: Arc::new(incoming),
            }],
        )
        .unwrap();
        assert_eq!(added, 1);
        {
            let memory = MEMORY.lock().unwrap();
            let memory = memory.as_ref().unwrap();
            assert_eq!(memory.get(10, -1, -1).unwrap().height, 70);
            assert_eq!(memory.get(10, -16, -16).unwrap().height, 60);
        }
        MEMORY_EPOCH.fetch_add(1, Ordering::Relaxed);
        assert!(restore(before.owner, before.chunks).is_err());
        DIMENSION.store(11, Ordering::Relaxed);
        assert!(restore(current.owner, current.chunks).is_err());
        forget();
        *STATE.lock().unwrap() = None;
    }

    #[test]
    fn column_reads_resume_and_never_exceed_the_read_budget() {
        let mut pending = Pending {
            x: 1,
            z: 2,
            y: 64,
            bottom: 0,
        };
        let mut visited = Vec::new();
        let mut reads = MAX_READS - 3;
        assert!(scan_column(
            &mut pending,
            &mut reads,
            || false,
            |p| {
                visited.push(p[1]);
                Some(None)
            }
        )
        .is_none());
        assert_eq!(reads, MAX_READS);
        assert_eq!(pending.y, 61);
        assert_eq!(visited, [64, 63, 62]);
        let mut reads = 0;
        let got = scan_column(
            &mut pending,
            &mut reads,
            || false,
            |p| Some((p[1] == 60).then_some([80, 90, 100])),
        )
        .unwrap();
        assert_eq!(reads, 2);
        assert_eq!(
            got,
            Column {
                height: 60,
                color: [80, 90, 100],
                known: true,
                remembered: false,
            }
        );
        let mut reads = 0;
        let start = pending.y;
        assert!(scan_column(
            &mut pending,
            &mut reads,
            || true,
            |_| panic!("must yield before reading")
        )
        .is_none());
        assert_eq!(pending.y, start);
        assert_eq!(reads, 0);
    }
    #[test]
    fn cell_cache_expires_unknown_columns_sooner() {
        let c = Cell {
            x: -1,
            z: 2,
            at: 100,
            column: Column {
                height: 64,
                color: [1, 2, 3],
                known: true,
                remembered: false,
            },
        };
        assert!(fresh_cell(&c, -1, 2, 1099, true));
        assert!(!fresh_cell(&c, -1, 2, 1100, true));
        assert!(fresh_cell(&c, -1, 2, 5099, false));
        assert!(!fresh_cell(&c, -1, 2, 5100, false));
        assert!(!fresh_cell(&c, 128, 2, 101, false));
        let unknown = Cell {
            column: Column::default(),
            ..c
        };
        assert!(fresh_cell(&unknown, -1, 2, 599, false));
        assert!(!fresh_cell(&unknown, -1, 2, 600, false));
    }

    #[test]
    fn searches_start_above_the_surface_whatever_the_player_height() {
        // Its own last surface: from 16 above, 64 blocks down.
        assert_eq!(search(Some(70), Some(200), -64, 320), (86, 22));
        assert_eq!(search(Some(310), None, -64, 320), (319, 255));
        assert_eq!(search(Some(-60), None, -64, 320), (-44, -64));
        // Otherwise from above the highest known surface, or the top of the
        // world, down to the bottom.
        assert_eq!(search(None, Some(96), -64, 320), (96, -64));
        assert_eq!(search(None, Some(999), -64, 320), (319, -64));
        assert_eq!(search(None, None, 0, 128), (127, 0));
    }

    #[test]
    fn plants_and_air_are_seen_through_and_terrain_has_a_colour() {
        for name in [
            "air",
            "minecraft:short_grass",
            "tallgrass",
            "oak_sapling",
            "stone_button",
            "torch",
            "red_tulip",
        ] {
            assert_eq!(color(name), None, "{name}");
        }
        assert_eq!(color("grass_block"), Some([92, 140, 60]));
        assert_eq!(color("water"), Some([52, 92, 180]));
        assert_eq!(color("flowing_water"), Some([52, 92, 180]));
        assert_eq!(color("oak_leaves"), Some([50, 105, 40]));
        assert_eq!(color("leaves2"), Some([50, 105, 40]));
        assert_eq!(color("deepslate_diamond_ore"), Some([80, 80, 85]));
        assert_eq!(color("red_wool"), Some([145, 35, 35]));
        assert_eq!(color("light_blue_concrete"), Some([100, 160, 210]));
        assert_eq!(color("grass_path"), Some([150, 125, 75]));
        assert_eq!(color("modded:thing"), Some([140, 140, 140]));
    }

    #[test]
    fn nearest_columns_come_first_and_every_offset_once() {
        let order = order();
        assert_eq!(order.len(), (SIDE * SIDE) as usize);
        assert_eq!(order[0], (0, 0));
        assert!(order[1..5].iter().all(|&(x, z)| x.abs() + z.abs() == 1));
        let mut seen = std::collections::HashSet::new();
        assert!(order.iter().all(|&o| seen.insert(o)));
        let d = |&(x, z): &(i32, i32)| x * x + z * z;
        assert!(order.windows(2).all(|w| d(&w[0]) <= d(&w[1])));
    }

    fn read(height: i16, color: [u8; 3]) -> Column {
        Column {
            height,
            color,
            known: true,
            remembered: false,
        }
    }

    #[test]
    fn memory_returns_read_columns_marked_remembered_across_negative_chunks() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = Memory::default();
        m.bind(1, 10);
        for (x, z) in [(-1, -1), (-16, 15), (0, 0), (17, -33)] {
            assert!(!m.store(10, x, z, read(x as i16, [1, 2, 3])));
        }
        for (x, z) in [(-1, -1), (-16, 15), (0, 0), (17, -33)] {
            let c = m.get(10, x, z).unwrap();
            assert_eq!((c.height, c.known, c.remembered), (x as i16, true, true));
        }
        // Unread columns of a stored chunk, other chunks and dimensions: none.
        assert_eq!(m.get(10, -2, -1), None);
        assert_eq!(m.get(10, 100, 100), None);
        assert_eq!(m.get(11, 0, 0), None);
        assert_eq!(m.chunks(), 4);
    }

    #[test]
    fn memory_is_hidden_after_rebinding_until_fresh_reads_agree() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = Memory::default();
        m.bind(1, 10);
        for x in 0..64 {
            m.store(10, x, 0, read(64, [9, 9, 9]));
        }
        m.bind(1, 10);
        assert_eq!(m.get(10, 63, 0), None, "hidden until checked");
        // 24 of 32 agree (heights within 1 count as the same): trusted.
        let mut trusted = false;
        for x in 0..CHECKS as i32 {
            let height = if x < 8 { 70 } else { 65 };
            trusted |= m.store(10, x, 0, read(height, [9, 9, 9]));
        }
        assert!(trusted);
        assert_eq!(m.get(10, 63, 0).map(|c| c.height), Some(64));
    }

    #[test]
    fn memory_of_another_level_is_dropped() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = Memory::default();
        m.bind(1, 10);
        for x in 0..64 {
            m.store(10, x, 0, read(64, [9, 9, 9]));
        }
        // The same addresses again, but different terrain: dropped.
        m.bind(1, 10);
        for x in 0..CHECKS as i32 {
            m.store(10, x, 0, read(64, [1, 1, 1]));
        }
        assert_eq!(m.get(10, 63, 0), None);
        assert_eq!(m.get(10, 0, 0), None);
        // The read that decided is remembered, and so are later ones.
        assert_eq!(m.get(10, 31, 0).map(|c| c.color), Some([1, 1, 1]));
        m.store(10, 0, 0, read(64, [1, 1, 1]));
        assert_eq!(m.get(10, 0, 0).map(|c| c.color), Some([1, 1, 1]));
        // A different registry is another level: everything goes.
        m.bind(2, 10);
        assert_eq!(m.chunks(), 0);
    }

    #[test]
    fn memory_keeps_the_chunks_nearest_the_player() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = Memory::default();
        m.bind(1, 10);
        m.bind(1, 11);
        m.store(11, 0, 0, read(64, [1, 1, 1]));
        for i in 0..MAX_CHUNKS as i32 - 1 {
            m.store(10, (i % 128) * 16, (i / 128) * 16, read(64, [2, 2, 2]));
        }
        m.trim(10, 0, 0);
        assert_eq!(m.chunks(), MAX_CHUNKS, "at the limit nothing goes");
        m.store(10, -16, 0, read(64, [2, 2, 2]));
        m.trim(10, 0, 0);
        assert_eq!(m.chunks(), MAX_CHUNKS * 7 / 8);
        assert_eq!(m.get(11, 0, 0), None, "other dimensions go first");
        assert!(m.get(10, 0, 0).is_some() && m.get(10, -16, 0).is_some());
        assert_eq!(m.get(10, 126 * 16, 63 * 16), None, "the farthest go");
    }

    #[test]
    fn remembered_area_samples_cell_centres_of_the_current_dimension() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = Memory::default();
        m.bind(1, 10);
        for x in -20..20 {
            for z in -20..20 {
                m.store(10, x, z, read((x * 100 + z) as i16, [1, 1, 1]));
            }
        }
        *MEMORY.lock().unwrap() = Some(m);
        DIMENSION.store(10, Ordering::Relaxed);
        // 4-block cells from (-8, -8): centres at -6, -2, 2.
        let grid = remembered_area(-8, -8, 4, 3, 2);
        let heights: Vec<i16> = grid.iter().map(|c| c.height).collect();
        assert_eq!(heights, [-606, -206, 194, -602, -202, 198]);
        assert!(grid.iter().all(|c| c.known && !c.remembered));
        // Outside the memory, another dimension, no memory: empty cells.
        assert!(!remembered_area(1000, 0, 1, 2, 1).iter().any(|c| c.known));
        DIMENSION.store(11, Ordering::Relaxed);
        assert!(!remembered_area(-8, -8, 4, 3, 2).iter().any(|c| c.known));
        forget();
        DIMENSION.store(10, Ordering::Relaxed);
        assert_eq!(remembered_area(-8, -8, 4, 3, 2).len(), 6);
        assert!(!remembered_area(-8, -8, 4, 3, 2).iter().any(|c| c.known));
    }

    #[test]
    fn the_revision_moves_only_when_remembered_terrain_changes() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = Memory::default();
        m.bind(1, 10);
        let start = memory_revision();
        m.store(10, 3, 4, read(64, [1, 2, 3]));
        let after_new = memory_revision();
        assert!(after_new > start, "a new column");
        m.store(10, 3, 4, read(64, [1, 2, 3]));
        assert_eq!(memory_revision(), after_new, "the same column again");
        m.store(10, 3, 4, read(65, [1, 2, 3]));
        assert!(memory_revision() > after_new, "a changed column");
        let before = memory_revision();
        m.bind(2, 10);
        assert!(memory_revision() > before, "another level drops it all");
    }

    #[test]
    fn surface_users_are_counted() {
        let _serial = SURFACE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        set_active(true);
        set_active(true);
        set_active(false);
        assert!(ACTIVE.load(Ordering::Acquire), "one user left");
        set_active(false);
        assert!(!ACTIVE.load(Ordering::Acquire));
        set_active(false);
        assert_eq!(USERS.load(Ordering::Acquire), 0, "never below zero");
    }

    #[test]
    fn grid_slots_wrap_with_world_position() {
        assert_eq!(slot(0, 0), slot(SIDE, -SIDE));
        assert_ne!(slot(0, 0), slot(1, 0));
        assert!(slot(-1, -1) < (SIDE * SIDE) as usize);
        let map = Map {
            center: [10, 20],
            radius: 1,
            columns: (0..9)
                .map(|i| Column {
                    height: i,
                    color: [0; 3],
                    known: true,
                    remembered: false,
                })
                .collect(),
        };
        assert_eq!(map.at(-1, -1).unwrap().height, 0);
        assert_eq!(map.at(1, -1).unwrap().height, 2);
        assert_eq!(map.at(0, 1).unwrap().height, 7);
        assert_eq!(map.at(2, 0), None);
    }
}
