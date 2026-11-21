//! Read loaded blocks on the game thread. No world writes or forced chunk loads.
use super::{known_build, layout::image, memory, patch, read, readable};
use crate::signatures::GET_BLOCK_SIGNATURE;
#[path = "block_environment.rs"]
pub(crate) mod environment;
/// Projectile trajectories (blocks read along the flight path).
#[path = "block_flight.rs"]
pub mod flight;
#[path = "block_lighting.rs"]
pub mod lighting;
#[path = "block_scan.rs"]
mod scan;
/// Minimap terrain (the top block of each column around the player).
#[path = "block_surface.rs"]
pub mod surface;
use crate::log;
use scan::Scan;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

const MAX_MATCHES: usize = 4096;
const MAX_READS: usize = 2048;
const SAMPLE_INTERVAL_MS: u64 = 16;
static LAST_SAMPLE: AtomicU64 = AtomicU64::new(0);
type GetBlock = unsafe extern "system" fn(usize, *const [i32; 3]) -> usize;
static GET_BLOCK: OnceLock<Result<usize, String>> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static RADIUS: AtomicI32 = AtomicI32::new(24);
static ORES: AtomicBool = AtomicBool::new(true);
static CHESTS: AtomicBool = AtomicBool::new(true);
static STATE: Mutex<Option<State>> = Mutex::new(None);
struct Published {
    at: u64,
    options: Settings,
    blocks: Arc<Vec<Found>>,
}
static SNAPSHOT: Mutex<Option<Published>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Coal,
    Iron,
    Copper,
    Gold,
    Redstone,
    Lapis,
    Diamond,
    Emerald,
    Quartz,
    Debris,
    Chest,
    TrappedChest,
    EnderChest,
    Barrel,
    /// Any colour of shulker box.
    ShulkerBox,
    /// Matched one of the user's own block names (`set_custom`).
    Custom,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Coal => "石炭鉱石",
            Self::Iron => "鉄鉱石",
            Self::Copper => "銅鉱石",
            Self::Gold => "金鉱石",
            Self::Redstone => "レッドストーン鉱石",
            Self::Lapis => "ラピスラズリ鉱石",
            Self::Diamond => "ダイヤモンド鉱石",
            Self::Emerald => "エメラルド鉱石",
            Self::Quartz => "ネザークォーツ鉱石",
            Self::Debris => "古代の残骸",
            Self::Chest => "チェスト",
            Self::TrappedChest => "トラップチェスト",
            Self::EnderChest => "エンダーチェスト",
            Self::Barrel => "樽",
            Self::ShulkerBox => "シュルカーボックス",
            Self::Custom => "指定ブロック",
        }
    }
    pub fn color(self) -> [f32; 4] {
        let rgb = match self {
            Self::Custom => [1., 0.3, 0.9],
            Self::Coal => [0.5, 0.5, 0.5],
            Self::Iron => [0.9, 0.7, 0.6],
            Self::Copper => [0.9, 0.45, 0.2],
            Self::Gold => [1., 0.85, 0.1],
            Self::Redstone | Self::TrappedChest => [1., 0.2, 0.2],
            Self::Lapis => [0.2, 0.4, 1.],
            Self::Diamond => [0., 1., 1.],
            Self::Emerald => [0.1, 1., 0.3],
            Self::Quartz => [1., 0.95, 0.9],
            Self::Debris => [0.65, 0.4, 0.3],
            Self::Chest | Self::Barrel => [1., 0.6, 0.1],
            Self::EnderChest => [0.7, 0.3, 1.],
            Self::ShulkerBox => [1., 0.55, 0.75],
        };
        [rgb[0], rgb[1], rgb[2], 0.9]
    }
    fn storage(self) -> bool {
        matches!(
            self,
            Self::Chest | Self::TrappedChest | Self::EnderChest | Self::Barrel | Self::ShulkerBox
        )
    }
}
fn classify(name: &str) -> Option<Kind> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    Some(match name.strip_prefix("deepslate_").unwrap_or(name) {
        "coal_ore" => Kind::Coal,
        "iron_ore" => Kind::Iron,
        "copper_ore" => Kind::Copper,
        "gold_ore" | "nether_gold_ore" => Kind::Gold,
        "redstone_ore" | "lit_redstone_ore" | "lit_deepslate_redstone_ore" => Kind::Redstone,
        "lapis_ore" => Kind::Lapis,
        "diamond_ore" => Kind::Diamond,
        "emerald_ore" => Kind::Emerald,
        "quartz_ore" => Kind::Quartz,
        "ancient_debris" => Kind::Debris,
        "chest" => Kind::Chest,
        "trapped_chest" => Kind::TrappedChest,
        "ender_chest" => Kind::EnderChest,
        "barrel" => Kind::Barrel,
        // The 16 dyed boxes (`white_shulker_box`...), `undyed_shulker_box`,
        // and the older single `shulker_box` id.
        n if n == "shulker_box" || n.ends_with("_shulker_box") => Kind::ShulkerBox,
        _ => return None,
    })
}
// --- User-chosen block names ---------------------------------------------------------

/// How many custom names may be watched at once.
pub const MAX_CUSTOM: usize = 32;
static CUSTOM: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Bumped on every change, so a running scan restarts with the new list.
static CUSTOM_VERSION: AtomicU32 = AtomicU32::new(0);

/// Cleans up a block name typed by the user: lower case, no `minecraft:`
/// prefix. `*` is allowed at either end as a wildcard. None if it is not a
/// plausible block name.
pub fn normalize_pattern(text: &str) -> Option<String> {
    let text = text.trim().to_ascii_lowercase();
    let text = text.strip_prefix("minecraft:").unwrap_or(&text);
    let core = text.trim_matches('*');
    let valid = !core.is_empty()
        && text.len() <= 64
        && core
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | ':' | '.'));
    valid.then(|| text.to_string())
}

/// Whether a block's name matches a normalized pattern: exactly, or by the
/// part before/after/between `*` wildcards at the ends.
fn pattern_matches(pattern: &str, name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    let core = pattern.trim_matches('*');
    match (pattern.starts_with('*'), pattern.ends_with('*')) {
        (false, false) => name == core,
        (true, false) => name.ends_with(core),
        (false, true) => name.starts_with(core),
        (true, true) => name.contains(core),
    }
}

/// Replaces the custom block names. Invalid or duplicate entries are dropped;
/// returns the list actually in use.
pub fn set_custom(patterns: &[String]) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    for pattern in patterns.iter().filter_map(|p| normalize_pattern(p)) {
        if list.len() < MAX_CUSTOM && !list.contains(&pattern) {
            list.push(pattern);
        }
    }
    *CUSTOM.lock().unwrap_or_else(|e| e.into_inner()) = list.clone();
    CUSTOM_VERSION.fetch_add(1, Ordering::Relaxed);
    list
}

/// The custom block names in use.
pub fn custom() -> Vec<String> {
    CUSTOM.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The built-in kinds first; otherwise a match against the user's names.
fn classify_with(name: &str, custom: &[String], ores: bool, chests: bool) -> Option<Kind> {
    classify(name)
        .filter(|kind| if kind.storage() { chests } else { ores })
        .or_else(|| {
            custom
                .iter()
                .any(|pattern| pattern_matches(pattern, name))
                .then_some(Kind::Custom)
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Settings {
    radius: i32,
    ores: bool,
    chests: bool,
    /// Which revision of the custom list the scan is using.
    custom: u32,
}
fn settings() -> Settings {
    Settings {
        radius: RADIUS.load(Ordering::Relaxed),
        ores: ORES.load(Ordering::Relaxed),
        chests: CHESTS.load(Ordering::Relaxed),
        custom: CUSTOM_VERSION.load(Ordering::Relaxed),
    }
}
pub fn configure(radius: i32, ores: bool, chests: bool) {
    RADIUS.store(radius.clamp(4, 64), Ordering::Relaxed);
    ORES.store(ores, Ordering::Relaxed);
    CHESTS.store(chests, Ordering::Relaxed);
}
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Release);
    LAST_SAMPLE.store(0, Ordering::Relaxed);
    if !active {
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
pub fn available() -> Result<(), String> {
    GET_BLOCK
        .get_or_init(|| {
            if !known_build() {
                return Err(format!(
                    "blockesp requires {}",
                    crate::modules::game::SUPPORTED
                ));
            }
            patch::find(GET_BLOCK_SIGNATURE)
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub position: [i32; 3],
    pub kind: Kind,
}
type World = (usize, u32, usize, usize, usize);
struct State {
    world: World,
    options: Settings,
    scan: Scan,
    /// The custom names as of `options.custom`, copied so the scan never locks.
    custom: Vec<String>,
    kinds: HashMap<usize, Option<Kind>>,
    last_kind: Option<(usize, Option<Kind>)>,
    found: HashMap<[i32; 3], Kind>,
    snapshot: Arc<Vec<Found>>,
    dirty: bool,
    at: u64,
    published: u64,
    recheck: usize,
    announced: bool,
    last_reads: usize,
    last_us: u128,
}
impl State {
    fn new(world: World, options: Settings, center: [i32; 3]) -> Self {
        Self {
            world,
            options,
            scan: Scan::new(center, options.radius),
            custom: custom(),
            kinds: HashMap::new(),
            last_kind: None,
            found: HashMap::new(),
            snapshot: Arc::new(Vec::new()),
            dirty: false,
            at: 0,
            published: 0,
            recheck: 0,
            announced: false,
            last_reads: 0,
            last_us: 0,
        }
    }
    fn keep(&self, kind: Kind) -> bool {
        if kind == Kind::Custom {
            true
        } else if kind.storage() {
            self.options.chests
        } else {
            self.options.ores
        }
    }
    unsafe fn kind(&mut self, get: GetBlock, source: usize, position: &[i32; 3]) -> Option<Kind> {
        self.last_reads += 1;
        let block = get(source, position);
        // The getter returns null for unloaded/unavailable blocks. There is
        // no block header to validate or name to classify at this address.
        if block == 0 {
            return None;
        }
        if let Some((last, kind)) = self.last_kind {
            if last == block {
                return kind;
            }
        }
        if let Some(kind) = self.kinds.get(&block) {
            self.last_kind = Some((block, *kind));
            return *kind;
        }
        let name = block_name(block)?;
        let kind = classify_with(&name, &self.custom, self.options.ores, self.options.chests);
        if self.kinds.len() >= 16384 {
            self.kinds.clear();
        }
        self.kinds.insert(block, kind);
        self.last_kind = Some((block, kind));
        kind
    }
    fn record(&mut self, position: [i32; 3], kind: Option<Kind>) {
        if let Some(kind) = kind {
            if self.found.len() < MAX_MATCHES || self.found.contains_key(&position) {
                self.dirty |= self.found.insert(position, kind) != Some(kind);
            }
        } else {
            self.dirty |= self.found.remove(&position).is_some();
        }
    }
    fn publish(&mut self, now: u64) -> Published {
        if self.dirty {
            self.snapshot = Arc::new(
                self.found
                    .iter()
                    .map(|(&position, &kind)| Found { position, kind })
                    .collect(),
            );
            self.dirty = false;
        }
        self.published = now;
        Published {
            at: now,
            options: self.options,
            blocks: Arc::clone(&self.snapshot),
        }
    }
}
unsafe fn pointer(memory: &mut memory::ReadBatch, address: usize) -> Option<usize> {
    if !memory.readable(address, 8) {
        return None;
    }
    let p: usize = read(address);
    (p != 0).then_some(p)
}
unsafe fn block_name(block: usize) -> Option<String> {
    if block == 0 {
        return None;
    }
    // This read-only batch ends before the next native block getter call.
    let mut memory = memory::ReadBatch::new();
    let legacy = pointer(&mut memory, block.checked_add(0x68)?)?;
    memory.string(legacy.checked_add(0x98)?, 96)
}

/// Read a specific live mining target on the game thread; never load chunks.
pub(super) unsafe fn name_at(player: usize, position: [i32; 3]) -> Option<String> {
    let getter = *GET_BLOCK.get()?.as_ref().ok()?;
    let (world, source, min, max) = binding(player, getter)?;
    if position[1] < min || position[1] >= max || block_name(world.4).as_deref() != Some("air") {
        return None;
    }
    let get: GetBlock = std::mem::transmute(getter);
    let block = get(source, &position);
    if block == 0 || block == world.4 {
        return None;
    }
    block_name(block).filter(|s| s != "air")
}

/// Independent read-only line-of-sight query. Never changes Waila's settings
/// or published ray, and treats unreadable/unloaded blocks as blocked.
fn ray_visible(
    origin: [f32; 3],
    point: [f32; 3],
    mut passable: impl FnMut([i32; 3]) -> Option<bool>,
) -> bool {
    let direction = std::array::from_fn(|i| point[i] - origin[i]);
    let distance = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !distance.is_finite()
        || distance > 8.
        || !origin
            .iter()
            .chain(point.iter())
            .all(|v| v.is_finite() && v.abs() < 3e7)
    {
        return false;
    }
    if distance < 1e-5 {
        return passable(origin.map(|v| v.floor() as i32)) == Some(true);
    }
    for (position, at) in ray_steps(origin, direction, distance) {
        if at > distance + 1e-4 {
            break;
        }
        if passable(position) != Some(true) {
            return false;
        }
    }
    true
}
pub(super) unsafe fn visible(player: usize, origin: [f32; 3], point: [f32; 3]) -> bool {
    let Some(Ok(&getter)) = GET_BLOCK.get().map(|v| v.as_ref()) else {
        return false;
    };
    let Some((world, source, min, max)) = binding(player, getter) else {
        return false;
    };
    if block_name(world.4).as_deref() != Some("air") {
        return false;
    }
    let get: GetBlock = std::mem::transmute(getter);
    ray_visible(origin, point, |position| {
        if position[1] < min || position[1] >= max {
            return None;
        }
        let block = get(source, &position);
        if block == 0 {
            return None;
        }
        if block == world.4 {
            return Some(true);
        }
        block_name(block).map(|name| name == "air" || FLUIDS.contains(&name.as_str()))
    })
}

unsafe fn binding(player: usize, getter: usize) -> Option<(World, usize, i32, i32)> {
    let base = image().0;
    let addresses = super::layout::addresses();
    binding_at(
        player,
        getter,
        base.checked_add(addresses.source_vtable?)?,
        base.checked_add(addresses.air_registry?)?,
    )
}

/// Read all binding links afresh in one batch. No native calls are made while
/// its protection checks are shared; no native pointers are cached between calls.
unsafe fn binding_at(
    player: usize,
    getter: usize,
    source_vtable: usize,
    air_registry: usize,
) -> Option<(World, usize, i32, i32)> {
    let mut memory = memory::ReadBatch::new();
    let dimension = pointer(&mut memory, player.checked_add(0x1F0)?)?;
    let source = pointer(&mut memory, dimension.checked_add(0xF0)?)?;
    if !memory.readable(source, 0x40)
        || !memory.readable(player.checked_add(0x10)?, 12)
        || !memory.readable(source_vtable.checked_add(0x10)?, 8)
    {
        return None;
    }
    if read::<usize>(source) != source_vtable
        || read::<usize>(source + 0x30) != dimension
        || read::<usize>(source_vtable + 0x10) != getter
    {
        return None;
    }
    let table = pointer(&mut memory, air_registry)?;
    let table = pointer(&mut memory, table)?;
    let air = pointer(&mut memory, table.checked_add(0x240)?)?;
    let min = read::<i16>(source + 0x3A) as i32;
    let max = read::<i16>(source + 0x38) as i32;
    if min >= max || max - min > 4096 {
        return None;
    }
    let world = (
        read::<usize>(player + 0x10),
        read::<u32>(player + 0x18),
        dimension,
        source,
        air,
    );
    Some((world, source, min, max))
}

/// Only positively recognized vanilla full blocks count as confirmed support.
/// Partial/custom shapes take the conservative native-sneak path.
fn full_support(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    if name.contains(':') || name == "light_block" {
        return false;
    }
    matches!(
        name,
        "stone"
            | "dirt"
            | "grass"
            | "grass_block"
            | "cobblestone"
            | "bedrock"
            | "deepslate"
            | "netherrack"
            | "end_stone"
            | "sand"
            | "red_sand"
            | "gravel"
            | "clay"
            | "obsidian"
            | "crying_obsidian"
            | "ice"
            | "packed_ice"
            | "blue_ice"
            | "snow"
            | "snow_block"
            | "mud"
            | "mycelium"
            | "podzol"
            | "basalt"
            | "smooth_basalt"
            | "blackstone"
            | "tuff"
            | "calcite"
            | "andesite"
            | "granite"
            | "diorite"
            | "bricks"
            | "bookshelf"
            | "glass"
    ) || [
        "_ore",
        "_planks",
        "_log",
        "_wood",
        "_stem",
        "_hyphae",
        "_wool",
        "_concrete",
        "_terracotta",
        "_leaves",
        "_block",
        "_sandstone",
    ]
    .iter()
    .any(|s| name.ends_with(s))
}
pub(super) unsafe fn edge_unsupported(player: usize, positions: &[[i32; 3]; 4]) -> Option<bool> {
    let getter = *GET_BLOCK.get()?.as_ref().ok()?;
    if !readable(player, 0x1f8) {
        return None;
    }
    let (_, source, min, max) = binding(player, getter)?;
    let get: GetBlock = std::mem::transmute(getter);
    for (i, p) in positions.iter().enumerate() {
        if positions[..i].contains(p) {
            continue;
        }
        if p[1] < min || p[1] >= max {
            return Some(true);
        }
        let block = get(source, p);
        if !full_support(&block_name(block)?) {
            return Some(true);
        }
    }
    Some(false)
}

/// Only called inside the HUD hook, while the local player/dimension are live.
pub(super) unsafe fn sample(player: usize, center: [i32; 3]) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let now = GetTickCount64();
    if now.saturating_sub(LAST_SAMPLE.load(Ordering::Relaxed)) < SAMPLE_INTERVAL_MS {
        return;
    }
    let Some(Ok(&getter)) = GET_BLOCK.get().map(|r| r.as_ref()) else {
        return;
    };
    let Ok(mut guard) = STATE.try_lock() else {
        return;
    };
    if now.saturating_sub(LAST_SAMPLE.load(Ordering::Relaxed)) < SAMPLE_INTERVAL_MS {
        return;
    }
    LAST_SAMPLE.store(now, Ordering::Relaxed);
    let mut profile = crate::modules::performance_profiler::metrics::Scope::new(
        crate::modules::performance_profiler::metrics::Stage::BlockScan,
    );
    let Some((world, source, min, max)) = binding(player, getter) else {
        *guard = None;
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    };
    let options = settings();
    if guard.as_ref().is_none_or(|s| {
        s.world != world || s.options != options || now.saturating_sub(s.at) >= 1000
    }) {
        // Name/layout validation is required once per world/air identity, not
        // once per HUD accessor. Pointer and vtable checks still run each batch.
        if block_name(world.4).as_deref() != Some("air") {
            *guard = None;
            *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return;
        }
        *guard = Some(State::new(world, options, center));
    }
    let state = guard.as_mut().unwrap();
    let get: GetBlock = std::mem::transmute(getter);
    let start = Instant::now();
    state.last_reads = 0;
    // Do not finish a scan around a place the player has already left.
    if (0..3)
        .any(|k| (center[k] as i64 - state.scan.center[k] as i64).abs() > options.radius as i64)
    {
        state.scan = Scan::new(center, options.radius);
    }
    // Revisit known matches first so a mined block/opened-and-broken chest
    // disappears without waiting for the full surrounding volume to finish.
    for _ in 0..16.min(state.snapshot.len()) {
        let p = state.snapshot[state.recheck % state.snapshot.len()].position;
        state.recheck = state.recheck.wrapping_add(1);
        let kind = state.kind(get, source, &p).filter(|k| state.keep(*k));
        state.record(p, kind);
    }
    for i in 0..MAX_READS {
        if i % 64 == 0 && start.elapsed() >= Duration::from_millis(1) {
            break;
        }
        let Some(p) = state.scan.next() else {
            state.scan = Scan::new(center, options.radius);
            let previous = state.found.len();
            state.found.retain(|p, _| {
                (0..3)
                    .map(|k| (p[k] as i64 - center[k] as i64).pow(2))
                    .sum::<i64>()
                    <= (options.radius as i64).pow(2)
            });
            state.dirty |= state.found.len() != previous;
            break;
        };
        if p[1] < min || p[1] >= max {
            continue;
        }
        let kind = state.kind(get, source, &p).filter(|k| state.keep(*k));
        if let Some(kind) = kind {
            if !state.announced {
                log!(
                    "blockesp: first match {kind:?} at {} {} {}",
                    p[0],
                    p[1],
                    p[2]
                );
                state.announced = true;
            }
        }
        state.record(p, kind);
    }
    let now = GetTickCount64();
    state.at = now;
    if now.saturating_sub(state.published) >= 100 {
        *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(state.publish(now));
    }
    state.last_us = start.elapsed().as_micros();
    profile.units(state.last_reads);
}
pub fn snapshot() -> Option<Arc<Vec<Found>>> {
    if !ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let guard = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    let s = guard.as_ref()?;
    (s.options == settings() && unsafe { GetTickCount64() }.saturating_sub(s.at) < 1000)
        .then(|| Arc::clone(&s.blocks))
}
// --- The block under the crosshair ---------------------------------------------------

/// The look-at ray is re-cast this often.
const LOOK_INTERVAL_MS: u64 = 50;
/// A result older than this is not shown (the HUD hook stopped).
const LOOK_MAX_AGE_MS: u64 = 500;
/// Upper bound on blocks visited by one ray, whatever the reach.
const LOOK_MAX_STEPS: usize = 256;
/// Blocks that a ray passes through unless fluids are asked for.
const FLUIDS: [&str; 4] = ["water", "flowing_water", "lava", "flowing_lava"];

static LOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
static OUTLINE_ACTIVE: AtomicBool = AtomicBool::new(false);
static LOOK_USERS: AtomicUsize = AtomicUsize::new(0);
static LOOK_FLUIDS: AtomicBool = AtomicBool::new(false);
/// f32 bits; how far the ray goes, in blocks.
static LOOK_REACH: AtomicU32 = AtomicU32::new(0x40C0_0000); // 6.0
static LOOK_LAST: AtomicU64 = AtomicU64::new(0);

/// The first solid block along the view direction.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    pub position: [i32; 3],
    /// From the eye to where the ray enters the block.
    pub distance: f32,
}

/// Copied ray and occluder from one game-thread sample. The owner token is
/// compared with actor snapshot identity; it is never dereferenced by readers.
#[derive(Clone, Debug)]
pub(crate) struct LookRay {
    pub owner: (usize, u32),
    pub origin: [f32; 3],
    pub forward: [f32; 3],
    pub reach: f32,
    pub block: Option<Look>,
}

struct LookState {
    at: u64,
    ray: Option<LookRay>,
    /// The world whose air block was checked to really be "air".
    validated: Option<World>,
    /// Names of the block states seen along recent rays.
    names: HashMap<usize, String>,
}
static LOOK: Mutex<Option<LookState>> = Mutex::new(None);

pub fn set_look(active: bool) {
    if active {
        LOOK_USERS.fetch_add(1, Ordering::Relaxed);
    } else {
        let _ = LOOK_USERS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    }
    let active = LOOK_USERS.load(Ordering::Relaxed) != 0;
    LOOK_ACTIVE.store(active, Ordering::Release);
    LOOK_LAST.store(0, Ordering::Relaxed);
    if !active {
        *LOOK.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
pub fn set_look_reach(blocks: f32) {
    if blocks.is_finite() {
        LOOK_REACH.store(blocks.clamp(1., 64.).to_bits(), Ordering::Relaxed);
    }
}
pub fn set_look_fluids(include: bool) {
    LOOK_FLUIDS.store(include, Ordering::Relaxed);
}
pub(crate) fn set_outline(active: bool) {
    set_look(active);
    OUTLINE_ACTIVE.store(active, Ordering::Release);
}
/// Whether the game thread should cast the ray at all.
pub(super) fn look_wanted() -> bool {
    LOOK_ACTIVE.load(Ordering::Acquire)
}

/// Every block a ray passes through, in order, with the distance at which it
/// enters each one (0 for the block containing the origin). Voxel traversal
/// (Amanatides & Woo): exact, with no sampling step to tune.
fn ray_steps(
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> impl Iterator<Item = ([i32; 3], f32)> {
    let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
    let valid = length.is_finite()
        && length > 1e-6
        && origin.iter().all(|v| v.is_finite() && v.abs() < 3.0e7)
        && reach.is_finite()
        && reach >= 0.;
    let dir = if valid {
        direction.map(|v| v / length)
    } else {
        [0.; 3]
    };
    let mut block = origin.map(|v| v.floor() as i32);
    let step = dir.map(|v| if v > 0. { 1 } else { -1 });
    // Distance along the ray between successive grid planes on each axis, and
    // to the first one.
    let delta = dir.map(|v| if v != 0. { 1. / v.abs() } else { f32::INFINITY });
    let mut next: [f32; 3] = std::array::from_fn(|k| {
        let inside = origin[k] - origin[k].floor();
        if dir[k] > 0. {
            (1. - inside) * delta[k]
        } else if dir[k] < 0. {
            inside * delta[k]
        } else {
            f32::INFINITY
        }
    });
    let mut count = 0;
    let mut distance: f32 = 0.;
    std::iter::from_fn(move || {
        if !valid || count >= LOOK_MAX_STEPS || !distance.is_finite() || distance > reach {
            return None;
        }
        let item = (block, distance);
        count += 1;
        let axis = (0..3)
            .min_by(|&a, &b| next[a].total_cmp(&next[b]))
            .unwrap_or(0);
        distance = next[axis];
        block[axis] += step[axis];
        next[axis] += delta[axis];
        Some(item)
    })
}
#[cfg(test)]
fn ray_blocks(origin: [f32; 3], direction: [f32; 3], reach: f32) -> Vec<([i32; 3], f32)> {
    ray_steps(origin, direction, reach).collect()
}

/// Only called inside the HUD hook, while the local player/dimension are live.
pub(super) unsafe fn refresh_look(player: usize, eye: [f32; 3], forward: [f32; 3]) {
    if !look_wanted() {
        return;
    }
    LOOK_LAST.store(0, Ordering::Relaxed);
    sample_look(player, eye, forward);
}
pub(super) unsafe fn sample_look(player: usize, eye: [f32; 3], forward: [f32; 3]) {
    let now = GetTickCount64();
    let interval = if OUTLINE_ACTIVE.load(Ordering::Acquire) {
        16
    } else {
        LOOK_INTERVAL_MS
    };
    if now.saturating_sub(LOOK_LAST.load(Ordering::Relaxed)) < interval {
        return;
    }
    let Some(Ok(&getter)) = GET_BLOCK.get().map(|r| r.as_ref()) else {
        return;
    };
    let Ok(mut guard) = LOOK.try_lock() else {
        return;
    };
    LOOK_LAST.store(now, Ordering::Relaxed);
    let Some((world, source, min, max)) = binding(player, getter) else {
        *guard = None;
        return;
    };
    let state = guard.get_or_insert_with(|| LookState {
        at: 0,
        ray: None,
        validated: None,
        names: HashMap::new(),
    });
    if state.validated != Some(world) {
        // Same check as the block scan: the layout is right only if the
        // registry's air block really is named "air".
        if block_name(world.4).as_deref() != Some("air") {
            *guard = None;
            return;
        }
        state.validated = Some(world);
        state.names.clear();
    }
    let get: GetBlock = std::mem::transmute(getter);
    let fluids = LOOK_FLUIDS.load(Ordering::Relaxed);
    let reach = f32::from_bits(LOOK_REACH.load(Ordering::Relaxed));
    let mut result = None;
    for (position, distance) in ray_steps(eye, forward, reach) {
        if position[1] < min || position[1] >= max {
            continue;
        }
        let block = get(source, &position);
        if block == 0 || block == world.4 {
            continue;
        }
        if !state.names.contains_key(&block) {
            let Some(name) = block_name(block) else {
                continue;
            };
            if state.names.len() >= 1024 {
                state.names.clear();
            }
            state.names.insert(block, name);
        }
        let name = &state.names[&block];
        if name == "air" || (!fluids && FLUIDS.contains(&name.as_str())) {
            continue;
        }
        result = Some(Look {
            name: name.clone(),
            position,
            distance,
        });
        break;
    }
    state.ray = Some(LookRay {
        owner: (world.0, world.1),
        origin: eye,
        forward,
        reach,
        block: result,
    });
    state.at = now;
}

/// What the player is looking at: `None` when there is no reading (not in a
/// world, feature off), `Some(None)` when only air is within reach.
pub fn look() -> Option<Option<Look>> {
    look_ray().map(|ray| ray.block)
}

/// Copy the position without cloning the ray's block name every render frame.
pub(crate) fn outline_position() -> Option<[i32; 3]> {
    let guard = LOOK.try_lock().ok()?;
    let state = guard.as_ref()?;
    let now = unsafe { GetTickCount64() };
    if !OUTLINE_ACTIVE.load(Ordering::Acquire)
        || state.at == 0
        || now.saturating_sub(state.at) > 120
    {
        return None;
    }
    let block = state.ray.as_ref()?.block.as_ref()?;
    (block.distance <= 6. && !FLUIDS.contains(&block.name.as_str())).then_some(block.position)
}

pub(crate) fn look_ray() -> Option<LookRay> {
    if !LOOK_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let guard = LOOK.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.as_ref()?;
    (state.at != 0 && unsafe { GetTickCount64() }.saturating_sub(state.at) < LOOK_MAX_AGE_MS)
        .then(|| state.ray.clone())
        .flatten()
}
pub(crate) fn look_for_local(owner: (usize, u32)) -> Option<(u64, LookRay)> {
    if !LOOK_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let guard = LOOK.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.as_ref()?;
    let ray = state.ray.as_ref()?;
    (ray.owner == owner
        && state.at != 0
        && unsafe { GetTickCount64() }.saturating_sub(state.at) < LOOK_MAX_AGE_MS)
        .then(|| (state.at, ray.clone()))
}

pub fn diagnostic() -> String {
    let Ok(guard) = STATE.try_lock() else {
        return "blockesp: sampling".into();
    };
    match guard.as_ref() {
        Some(s) => format!(
            "blockesp: {} matches, {} block states; center {:?}, radius {}, cursor {}, age {}ms; last batch {} reads / {}us (min interval {}ms)",
            s.found.len(),
            s.kinds.len(),
            s.scan.center,
            s.options.radius,
            s.scan.cursor,
            unsafe { GetTickCount64() }.saturating_sub(s.at),
            s.last_reads,
            s.last_us,
            SAMPLE_INTERVAL_MS,
        ),
        None => "blockesp: disabled or waiting for a valid world".into(),
    }
}

/// Serializes tests that replace the process-wide custom block list.
#[cfg(test)]
pub(crate) static CUSTOM_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, VirtualProtect, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
        PAGE_NOACCESS, PAGE_READWRITE,
    };

    struct BindingFixture {
        allocation: *mut std::ffi::c_void,
        player: usize,
        dimension: usize,
        source: usize,
        vtable: usize,
        registry: usize,
        table: usize,
        air: usize,
    }
    impl BindingFixture {
        fn new(bytes: usize) -> Self {
            let allocation = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    bytes,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            assert!(!allocation.is_null());
            let base = allocation as usize;
            let step = bytes / 8;
            assert!(step >= 4096);
            let f = Self {
                allocation,
                vtable: base,
                registry: base + step,
                table: base + 3 * step,
                source: base + 4 * step,
                dimension: base + 5 * step,
                player: base + 6 * step,
                air: base + 7 * step,
            };
            unsafe {
                (f.vtable as *mut usize).add(2).write(0x1234);
                (f.registry as *mut usize).write(base + 2 * step);
                ((base + 2 * step) as *mut usize).write(f.table);
                ((f.table + 0x240) as *mut usize).write(f.air);
                (f.source as *mut usize).write(f.vtable);
                ((f.source + 0x30) as *mut usize).write(f.dimension);
                ((f.source + 0x38) as *mut i16).write(320);
                ((f.source + 0x3a) as *mut i16).write(-64);
                ((f.dimension + 0xf0) as *mut usize).write(f.source);
                ((f.player + 0x1f0) as *mut usize).write(f.dimension);
                ((f.player + 0x10) as *mut usize).write(0x5678);
                ((f.player + 0x18) as *mut u32).write(0x40001);
            }
            f
        }
        unsafe fn read(&self) -> Option<(World, usize, i32, i32)> {
            binding_at(self.player, 0x1234, self.vtable, self.registry)
        }
    }
    impl Drop for BindingFixture {
        fn drop(&mut self) {
            unsafe {
                VirtualFree(self.allocation, 0, MEM_RELEASE);
            }
        }
    }

    #[test]
    fn block_binding_revalidates_owner_links_vtable_getter_and_height_limits() {
        let f = BindingFixture::new(64 * 1024);
        unsafe {
            let first = f.read().unwrap();
            assert_eq!(
                first,
                (
                    (0x5678, 0x40001, f.dimension, f.source, f.air),
                    f.source,
                    -64,
                    320
                )
            );
            ((f.player + 0x18) as *mut u32).write(0x80001);
            assert_eq!(
                f.read().unwrap().0 .1,
                0x80001,
                "entity generations are never cached"
            );
            ((f.table + 0x240) as *mut usize).write(0);
            assert!(
                f.read().is_none(),
                "missing air binding must invalidate the result"
            );
            ((f.table + 0x240) as *mut usize).write(f.air);
            ((f.source + 0x30) as *mut usize).write(0);
            assert!(
                f.read().is_none(),
                "source must belong to the live dimension"
            );
            ((f.source + 0x30) as *mut usize).write(f.dimension);
            (f.source as *mut usize).write(0);
            assert!(
                f.read().is_none(),
                "source vtable identity must still match"
            );
            (f.source as *mut usize).write(f.vtable);
            ((f.vtable + 0x10) as *mut usize).write(0x9999);
            assert!(f.read().is_none(), "wrong native getter must be rejected");
            ((f.vtable + 0x10) as *mut usize).write(0x1234);
            ((f.source + 0x38) as *mut i16).write(-64);
            assert!(f.read().is_none());
            ((f.source + 0x38) as *mut i16).write(5000);
            assert!(f.read().is_none());
            ((f.source + 0x38) as *mut i16).write(320);
            assert!(f.read().is_some());
            assert!(binding_at(0, 0x1234, f.vtable, f.registry).is_none());
            assert!(binding_at(usize::MAX, 0x1234, f.vtable, f.registry).is_none());
            assert!(binding_at(f.player, 0x1234, usize::MAX, f.registry).is_none());
        }
    }

    #[test]
    fn block_binding_does_not_reuse_protection_checks_from_an_earlier_call() {
        let f = BindingFixture::new(64 * 1024);
        unsafe {
            assert!(f.read().is_some());
            let mut old = 0;
            assert_ne!(
                VirtualProtect(f.dimension as *const _, 4096, PAGE_NOACCESS, &mut old),
                0
            );
            assert!(f.read().is_none());
            assert_ne!(
                VirtualProtect(f.dimension as *const _, 4096, PAGE_READWRITE, &mut old),
                0
            );
            assert!(f.read().is_some());
        }
    }

    #[test]
    fn block_names_reject_null_links_and_keep_name_bounds() {
        let name = "minecraft:diamond_ore";
        let mut legacy = [0usize; 24];
        let mut block = [0usize; 14];
        legacy[0x98 / 8] = name.as_ptr() as usize;
        legacy[0xa8 / 8] = name.len();
        legacy[0xb0 / 8] = name.len();
        block[0x68 / 8] = legacy.as_ptr() as usize;
        unsafe {
            assert_eq!(block_name(block.as_ptr() as usize).as_deref(), Some(name));
            assert!(block_name(0).is_none());
            assert!(block_name(usize::MAX).is_none());
            block[0x68 / 8] = 0;
            assert!(block_name(block.as_ptr() as usize).is_none());
            block[0x68 / 8] = legacy.as_ptr() as usize;
            legacy.as_mut_ptr().add(0xa8 / 8).write(97);
            legacy.as_mut_ptr().add(0xb0 / 8).write(97);
            assert!(block_name(block.as_ptr() as usize).is_none());
        }
    }

    #[test]
    fn unloaded_blocks_do_not_match_or_populate_the_type_cache() {
        unsafe extern "system" fn unloaded(_: usize, _: *const [i32; 3]) -> usize {
            0
        }
        let _serial = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = State::new((1, 1, 2, 3, 4), settings(), [0; 3]);
        for _ in 0..256 {
            assert_eq!(unsafe { state.kind(unloaded, 0, &[0; 3]) }, None);
        }
        assert_eq!(state.last_reads, 256);
        assert!(state.kinds.is_empty());
    }

    /// Frozen region-query binding path for the before/after native API benchmark.
    unsafe fn region_binding(f: &BindingFixture) -> Option<(World, usize, i32, i32)> {
        unsafe fn old_pointer(a: usize) -> Option<usize> {
            if !readable(a, 8) {
                return None;
            }
            let p = read::<usize>(a);
            (p != 0).then_some(p)
        }
        let dimension = old_pointer(f.player + 0x1f0)?;
        let source = old_pointer(dimension + 0xf0)?;
        if !readable(source, 0x40)
            || read::<usize>(source) != f.vtable
            || read::<usize>(source + 0x30) != dimension
            || read::<usize>(f.vtable + 0x10) != 0x1234
        {
            return None;
        }
        let table = old_pointer(f.registry)?;
        let table = old_pointer(table)?;
        let air = old_pointer(table + 0x240)?;
        let min = read::<i16>(source + 0x3a) as i32;
        let max = read::<i16>(source + 0x38) as i32;
        if min >= max || max - min > 4096 {
            return None;
        }
        Some((
            (
                read(f.player + 0x10),
                read(f.player + 0x18),
                dimension,
                source,
                air,
            ),
            source,
            min,
            max,
        ))
    }

    #[test]
    #[ignore = "manual benchmark: native block-source validation in a large committed heap"]
    fn block_binding_cost() {
        use std::hint::black_box;
        let fixture = BindingFixture::new(256 * 1024 * 1024);
        let expected = unsafe { region_binding(&fixture) }.unwrap();
        assert_eq!(unsafe { fixture.read() }, Some(expected));
        for fast in [false, true] {
            let mut times = Vec::new();
            for _ in 0..101 {
                let start = Instant::now();
                let got = unsafe {
                    if fast {
                        fixture.read()
                    } else {
                        region_binding(&fixture)
                    }
                };
                assert_eq!(black_box(got), Some(expected));
                times.push(start.elapsed());
            }
            times.sort_unstable();
            eprintln!(
                "block binding / resident_pages={fast}: median {:?}, p90 {:?}",
                times[50], times[90]
            );
        }
    }

    #[test]
    fn combat_visibility_stops_at_walls_unknown_blocks_and_the_endpoint() {
        let origin = [0.5, 64.5, 0.5];
        let point = [0.5, 64.5, 3.];
        assert!(ray_visible(origin, point, |_| Some(true)));
        assert!(!ray_visible(origin, point, |p| Some(p[2] != 2)));
        assert!(!ray_visible(origin, point, |p| Some(p[2] != 3)));
        assert!(!ray_visible(origin, point, |p| if p[2] == 1 {
            None
        } else {
            Some(true)
        }));
        assert!(ray_visible(origin, origin, |_| Some(true)));
        assert!(!ray_visible(origin, origin, |_| Some(false)));
        assert!(!ray_visible(origin, [f32::NAN, 0., 0.], |_| Some(true)));
        assert!(!ray_visible(origin, [0.5, 64.5, 20.], |_| Some(true)));
    }
    #[test]
    fn edge_support_accepts_full_vanilla_blocks_but_not_air_fluids_or_partial_shapes() {
        for name in [
            "minecraft:stone",
            "grass_block",
            "oak_planks",
            "deepslate_diamond_ore",
            "iron_block",
        ] {
            assert!(full_support(name), "{name}");
        }
        for name in [
            "air",
            "water",
            "flowing_water",
            "lava",
            "torch",
            "short_grass",
            "snow_layer",
            "oak_slab",
            "oak_stairs",
            "custom:stone",
            "mystery",
            "light_block",
        ] {
            assert!(!full_support(name), "{name}");
        }
    }
    #[test]
    fn classifies_ores_and_storage_without_matching_ordinary_blocks() {
        assert_eq!(
            classify("minecraft:deepslate_diamond_ore"),
            Some(Kind::Diamond)
        );
        assert_eq!(classify("deepslate_lit_redstone_ore"), Some(Kind::Redstone));
        assert_eq!(classify("nether_gold_ore"), Some(Kind::Gold));
        assert_eq!(classify("ancient_debris"), Some(Kind::Debris));
        assert_eq!(classify("trapped_chest"), Some(Kind::TrappedChest));
        for name in ["air", "stone", "diamond_block", "chest_minecart", "ore"] {
            assert_eq!(classify(name), None);
        }
    }
    #[test]
    fn custom_names_match_exactly_or_by_wildcard_and_never_shadow_built_ins() {
        assert_eq!(
            normalize_pattern(" Minecraft:Mob_Spawner "),
            Some("mob_spawner".into())
        );
        assert_eq!(
            normalize_pattern("*shulker_box"),
            Some("*shulker_box".into())
        );
        for bad in ["", "*", "**", "two words", "semi;colon", &"x".repeat(65)] {
            assert_eq!(normalize_pattern(bad), None, "{bad:?}");
        }

        assert!(pattern_matches("mob_spawner", "minecraft:mob_spawner"));
        assert!(!pattern_matches("mob_spawner", "trial_spawner"));
        assert!(pattern_matches("*spawner", "trial_spawner"));
        assert!(pattern_matches("*shulker_box", "undyed_shulker_box"));
        assert!(!pattern_matches("*shulker_box", "shulker_box_item"));
        assert!(pattern_matches("portal*", "portal"));
        assert!(!pattern_matches("portal*", "end_portal"));
        assert!(pattern_matches("*amethyst*", "budding_amethyst"));
        assert!(!pattern_matches("stone", "cobblestone"));

        let custom = vec!["mob_spawner".to_string(), "*_ore".to_string()];
        assert_eq!(
            classify_with("mob_spawner", &custom, true, true),
            Some(Kind::Custom)
        );
        // A built-in kind keeps its own colour even if a pattern also matches.
        assert_eq!(
            classify_with("diamond_ore", &custom, true, true),
            Some(Kind::Diamond)
        );
        assert_eq!(
            classify_with("diamond_ore", &custom, false, false),
            Some(Kind::Custom)
        );
        assert_eq!(
            classify_with("chest", &["chest".into()], false, false),
            Some(Kind::Custom)
        );
        assert_eq!(classify_with("chest", &[], false, false), None);
        // Shulker boxes of every colour are storage, like chests.
        for name in [
            "shulker_box",
            "undyed_shulker_box",
            "minecraft:white_shulker_box",
            "light_blue_shulker_box",
            "black_shulker_box",
        ] {
            assert_eq!(
                classify_with(name, &[], false, true),
                Some(Kind::ShulkerBox),
                "{name}"
            );
            assert_eq!(classify_with(name, &[], true, false), None, "{name}");
        }
        assert_eq!(classify_with("shulker_shell", &[], true, true), None);
        assert_eq!(classify_with("stone", &custom, true, true), None);
        assert_eq!(classify_with("mob_spawner", &[], true, true), None);
    }

    #[test]
    fn custom_list_is_normalized_deduplicated_bounded_and_restarts_the_scan() {
        let _serial = CUSTOM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = settings();
        let used = set_custom(&[
            "Mob_Spawner".into(),
            "minecraft:mob_spawner".into(),
            "not a block".into(),
            "*shulker_box".into(),
        ]);
        assert_eq!(used, ["mob_spawner", "*shulker_box"]);
        assert_eq!(custom(), used);
        assert_ne!(settings().custom, before.custom);
        // A state created now scans with the new names and always keeps them.
        let state = State::new((1, 1, 2, 3, 4), settings(), [0; 3]);
        assert_eq!(state.custom, used);
        assert!(state.keep(Kind::Custom));
        let many: Vec<String> = (0..MAX_CUSTOM + 5).map(|i| format!("block_{i}")).collect();
        assert_eq!(set_custom(&many).len(), MAX_CUSTOM);
        assert!(set_custom(&[]).is_empty());
    }

    #[test]
    fn ray_visits_adjacent_blocks_in_order_with_entry_distances() {
        // Straight along +X from the middle of a block.
        let blocks = ray_blocks([0.5, 64.5, 0.5], [1., 0., 0.], 3.);
        assert_eq!(
            blocks,
            [
                ([0, 64, 0], 0.),
                ([1, 64, 0], 0.5),
                ([2, 64, 0], 1.5),
                ([3, 64, 0], 2.5)
            ]
        );
        // Along -X / -Y / -Z, across zero into negative coordinates.
        let back = ray_blocks([0.25, 0.5, 0.5], [-1., 0., 0.], 2.);
        assert_eq!(
            back.iter().map(|b| b.0).collect::<Vec<_>>(),
            [[0, 0, 0], [-1, 0, 0], [-2, 0, 0]]
        );
        assert_eq!(back[1].1, 0.25);
        let down = ray_blocks([0.5, 1.9, 0.5], [0., -2., 0.], 1.);
        assert_eq!(
            down.iter().map(|b| b.0).collect::<Vec<_>>(),
            [[0, 1, 0], [0, 0, 0]]
        );
        assert!((down[1].1 - 0.9).abs() < 1e-5);

        // A diagonal moves one axis at a time (no corner skipping) and the
        // distances never decrease or exceed the reach.
        let diagonal = ray_blocks([0.1, 0.2, 0.3], [1., 1., 1.], 10.);
        for pair in diagonal.windows(2) {
            let moved: i32 = (0..3).map(|k| (pair[1].0[k] - pair[0].0[k]).abs()).sum();
            assert_eq!(moved, 1, "{pair:?}");
            assert!(pair[1].1 >= pair[0].1);
        }
        assert!(diagonal.last().unwrap().1 <= 10.);
        assert!(diagonal.len() > 15);

        // The step count is bounded however long the ray is.
        assert_eq!(
            ray_blocks([0.5; 3], [0., 1., 0.], 1.0e6).len(),
            LOOK_MAX_STEPS
        );
        // Zero reach: only the block the eye is in.
        assert_eq!(ray_blocks([0.5; 3], [0., 0., 1.], 0.), [([0, 0, 0], 0.)]);
        // Nonsense input yields nothing instead of a wild walk.
        assert!(ray_blocks([0.5; 3], [0., 0., 0.], 5.).is_empty());
        assert!(ray_blocks([f32::NAN, 0., 0.], [1., 0., 0.], 5.).is_empty());
        assert!(ray_blocks([0.5; 3], [1., 0., 0.], f32::NAN).is_empty());
    }

    #[test]
    fn look_reports_nothing_until_enabled_and_sampled() {
        assert_eq!(look(), None);
        set_look(true);
        // Enabled but never sampled (not in the game): still no reading.
        assert_eq!(look(), None);
        set_look(false);
        set_look_reach(1.0e9);
        assert_eq!(f32::from_bits(LOOK_REACH.load(Ordering::Relaxed)), 64.);
        set_look_reach(f32::NAN);
        assert_eq!(f32::from_bits(LOOK_REACH.load(Ordering::Relaxed)), 64.);
        set_look_reach(6.);
    }

    #[test]
    fn bounded_scan_visits_each_block_once_including_negative_positions() {
        let center = [-17, -64, -1];
        let mut scan = Scan::new(center, 1);
        let mut positions = Vec::new();
        // Resume after each small budget rather than starting over.
        for _ in 0..4 {
            for _ in 0..2 {
                if let Some(p) = scan.next() {
                    positions.push(p);
                }
            }
        }
        positions.sort();
        positions.dedup();
        assert_eq!(positions.len(), 7);
        assert!(positions.contains(&center));
        assert!(positions.contains(&[-18, -64, -1]));
        assert_eq!(scan.next(), None);
        let old = State::new(
            (1, 1, 2, 3, 4),
            Settings {
                radius: 4,
                ores: true,
                chests: true,
                custom: 0,
            },
            center,
        );
        let new = State::new((1, 2, 2, 3, 4), old.options, center);
        assert!(new.snapshot.is_empty());
        assert!(new.kinds.is_empty());
    }
    #[test]
    fn unchanged_results_share_the_snapshot_and_changes_replace_it() {
        let mut state = State::new(
            (1, 1, 2, 3, 4),
            Settings {
                radius: 24,
                ores: true,
                chests: true,
                custom: 0,
            },
            [0; 3],
        );
        let p = [-1, -64, 0];
        state.record(p, Some(Kind::Chest));
        let first = state.publish(100);
        assert_eq!(
            first.blocks.as_slice(),
            &[Found {
                position: p,
                kind: Kind::Chest
            }]
        );
        state.record(p, Some(Kind::Chest));
        state.record([1; 3], None);
        let unchanged = state.publish(200);
        assert!(Arc::ptr_eq(&first.blocks, &unchanged.blocks));
        assert_eq!(unchanged.at, 200);
        state.record(p, Some(Kind::Diamond));
        let replaced = state.publish(300);
        assert!(!Arc::ptr_eq(&first.blocks, &replaced.blocks));
        assert_eq!(replaced.blocks[0].kind, Kind::Diamond);
        assert_eq!(first.blocks[0].kind, Kind::Chest);
        state.record(p, None);
        assert!(state.publish(400).blocks.is_empty());
    }
}
