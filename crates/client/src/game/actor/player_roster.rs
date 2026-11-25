//! Copies the level roster and joins it to loaded actors on the game callback.
use crate::game::{
    actors, build_name,
    entities::{component_pools, PoolView},
    layout::image,
    memory::ReadBatch,
    patch, read,
};
use crate::signatures::{LEVEL_DATA_SIGNATURE, PLAYER_ROSTER_WALK_SIGNATURE};
use crate::{gui, input, modules::chat_log};
use std::collections::{hash_map::Entry, HashMap};
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Arc, Mutex, OnceLock,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

const LEVEL_FIELD: usize = 0x200;
const LEVEL_DATA_SLOT: usize = 0x568;
const ROSTER_SLOT: usize = 0x9f8;
const ROSTER_FIELD: usize = 0x4e8;
const ROSTER_GETTER: [u8; 8] = [0x48, 0x8b, 0x81, 0xe8, 4, 0, 0, 0xc3];
const ACTOR_ID_FIELD: usize = 0x20;
const NAME_FIELD: usize = 0x38;
const MAX_ENTRIES: usize = 1024;
const INTERVAL_MS: u64 = 350;

static HOLD_KEY: AtomicU16 = AtomicU16::new(0);
static LEVEL_DATA: OnceLock<Result<usize, String>> = OnceLock::new();
static STATE: Mutex<Option<Reading>> = Mutex::new(None);
static SAMPLING: Mutex<()> = Mutex::new(());

struct Reading {
    owner: (usize, usize),
    at: u64,
    roster: Option<Arc<Roster>>,
}

pub(crate) struct Roster {
    pub players: Vec<Player>,
    pub loaded: Option<usize>,
}

pub(crate) struct Player {
    pub name: String,
    pub local: bool,
    pub loaded: bool,
    pub distance: Option<String>,
    pub health: Option<String>,
}

pub(crate) fn prepare() -> Result<(), String> {
    LEVEL_DATA
        .get_or_init(|| {
            build_name().ok_or("このビルドの参加者一覧は未検証です")?;
            patch::find(PLAYER_ROSTER_WALK_SIGNATURE)?;
            patch::find(LEVEL_DATA_SIGNATURE)
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

pub(crate) fn set_hold_key(key: u16) {
    HOLD_KEY.store(key, Ordering::Release);
    clear_reading();
}

pub(in crate::game) fn clear() {
    if HOLD_KEY.load(Ordering::Relaxed) == 0 {
        return;
    }
    clear_reading();
}

fn clear_reading() {
    if let Ok(mut state) = STATE.try_lock() {
        *state = None;
    }
}

pub(crate) fn snapshot() -> Option<Arc<Roster>> {
    let state = STATE.try_lock().ok()?;
    let sample = state.as_ref()?;
    if HOLD_KEY.load(Ordering::Acquire) == 0
        || unsafe { GetTickCount64() }.saturating_sub(sample.at) > INTERVAL_MS * 3
    {
        return None;
    }
    sample.roster.clone()
}

pub(in crate::game) unsafe fn sample(player: usize, position: [f32; 3], now: u64) {
    let key = HOLD_KEY.load(Ordering::Acquire);
    if key == 0
        || !input::key_down(key)
        || gui::captures_input()
        || crate::game::inventory::screen_open()
        || !input::is_foreground()
    {
        return;
    }
    let Some(Ok(&level_data)) = LEVEL_DATA.get().map(Result::as_ref) else {
        return;
    };
    let Ok(_sampling) = SAMPLING.try_lock() else {
        return;
    };
    let level: usize = read(player + LEVEL_FIELD);
    let owner = (player, level);
    {
        let Ok(mut state) = STATE.try_lock() else {
            return;
        };
        if let Some(sample) = state.as_ref() {
            if sample.owner != owner {
                *state = None;
            } else if now.saturating_sub(sample.at) < INTERVAL_MS {
                return;
            }
        }
    }
    let mut memory = ReadBatch::new();
    let roster = level_roster(&mut memory, level, level_data)
        .and_then(|map| read_entries(&mut memory, map))
        .map(|mut entries| {
            let loaded = add_actor_details(&mut memory, player, position, &mut entries);
            Arc::new(Roster {
                players: entries.into_iter().map(|(_, row)| row).collect(),
                loaded,
            })
        });
    if let Ok(mut state) = STATE.try_lock() {
        if HOLD_KEY.load(Ordering::Acquire) == key {
            *state = Some(Reading {
                owner,
                at: now,
                roster,
            });
        }
    }
}

/// The number of players in the level roster (the local player included),
/// without reading their names; works without the hold key.
pub(in crate::game) unsafe fn count(player: usize) -> Option<usize> {
    let Some(Ok(&level_data)) = LEVEL_DATA.get().map(Result::as_ref) else {
        return None;
    };
    let mut memory = ReadBatch::new();
    if !memory.readable(player, LEVEL_FIELD + 8) {
        return None;
    }
    let map = level_roster(&mut memory, read(player + LEVEL_FIELD), level_data)?;
    if !memory.readable(map, 24) {
        return None;
    }
    let count: usize = read(map + 16);
    (count <= MAX_ENTRIES).then_some(count)
}

unsafe fn level_roster(memory: &mut ReadBatch, level: usize, level_data: usize) -> Option<usize> {
    if !memory.readable(level, ROSTER_FIELD + 8) {
        return None;
    }
    let vt: usize = read(level);
    let (base, end) = image();
    if vt < base
        || vt.checked_add(ROSTER_SLOT + 8)? > end
        || read::<usize>(vt + LEVEL_DATA_SLOT) != level_data
    {
        return None;
    }
    let getter: usize = read(vt + ROSTER_SLOT);
    if getter < base
        || getter.checked_add(ROSTER_GETTER.len())? > end
        || read::<[u8; 8]>(getter) != ROSTER_GETTER
    {
        return None;
    }
    Some(read(level + ROSTER_FIELD))
}

unsafe fn read_entries(memory: &mut ReadBatch, map: usize) -> Option<Vec<(u64, Player)>> {
    if !memory.readable(map, 24) {
        return None;
    }
    let head: usize = read(map + 8);
    let count: usize = read(map + 16);
    if count > MAX_ENTRIES || !memory.readable(head, 16) {
        return None;
    }
    let mut players = Vec::with_capacity(count);
    let mut previous = head;
    let mut node: usize = read(head);
    for _ in 0..count {
        if node == head
            || !memory.readable(node, NAME_FIELD + 32)
            || read::<usize>(node + 8) != previous
        {
            return None;
        }
        let name = memory.with_string(node + NAME_FIELD, 192, chat_log::clean)?;
        let name: String = name
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(48)
            .collect();
        if !name.is_empty() {
            players.push((
                read(node + ACTOR_ID_FIELD),
                Player {
                    name,
                    local: false,
                    loaded: false,
                    distance: None,
                    health: None,
                },
            ));
        }
        previous = node;
        node = read(node);
    }
    if node != head || read::<usize>(head + 8) != previous || read::<usize>(map + 16) != count {
        return None;
    }
    players.sort_unstable_by(|(_, a), (_, b)| {
        let (a, b) = (&a.name, &b.name);
        a.bytes()
            .map(|b| b.to_ascii_lowercase())
            .cmp(b.bytes().map(|b| b.to_ascii_lowercase()))
            .then_with(|| a.cmp(b))
    });
    Some(players)
}

unsafe fn add_actor_details(
    memory: &mut ReadBatch,
    player: usize,
    position: [f32; 3],
    players: &mut [(u64, Player)],
) -> Option<usize> {
    let (registry, own): (usize, u32) = (read(player + 0x10), read(player + 0x18));
    let [ids, boxes, attributes] = component_pools(
        memory,
        registry,
        [actors::UNIQUE_ID, actors::AABB, actors::ATTRIBUTES],
    );
    let ids = PoolView::new(memory, ids?, 8)?;
    let own_id: u64 = read(ids.component(memory, own)?);
    let mut unmatched = player_indices(players, own_id)?;
    let boxes = boxes.and_then(|pool| PoolView::new(memory, pool, 0x20));
    let attributes =
        attributes.and_then(|pool| PoolView::new(memory, pool, actors::ATTRIBUTES_SIZE));
    let mut loaded = 0;
    // Resolve the local player directly. Only join remote rows by a unique actor id.
    for entity in
        std::iter::once(own).chain(ids.entities().step_by(4).take(8192).map(|at| read(at)))
    {
        if unmatched.is_empty() {
            break;
        }
        let Some(id) = ids.component(memory, entity).map(|at| read::<u64>(at)) else {
            continue;
        };
        let Some(index) = unmatched.remove(&id).flatten() else {
            continue;
        };
        let row = &mut players[index].1;
        row.local = entity == own;
        row.loaded = true;
        loaded += 1;
        row.distance = if row.local {
            Some("0.0 m".into())
        } else {
            boxes
                .as_ref()
                .and_then(|pool| pool.component(memory, entity))
                .map(|at| read::<crate::game::Aabb>(at))
                .filter(actors::valid_bounds)
                .and_then(|bounds| actor_distance(position, bounds))
                .map(|value| format!("{value:.1} m"))
        };
        row.health = attributes
            .as_ref()
            .and_then(|pool| pool.component(memory, entity))
            .and_then(|at| actors::health(memory, at))
            .map(|(current, max)| format!("{current:.1}/{max:.1}"));
    }
    Some(loaded)
}

fn player_indices(players: &[(u64, Player)], own_id: u64) -> Option<HashMap<u64, Option<usize>>> {
    let mut indices = HashMap::with_capacity(players.len());
    for (index, (id, _)) in players
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| *id != 0 && *id != u64::MAX)
    {
        match indices.entry(*id) {
            Entry::Vacant(entry) => {
                entry.insert(Some(index));
            }
            Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }
    // Require the roster and ECS to agree on the local id before joining.
    indices.get(&own_id).copied().flatten()?;
    indices.retain(|_, index| index.is_some());
    Some(indices)
}

fn actor_distance(position: [f32; 3], bounds: crate::game::Aabb) -> Option<f32> {
    let target = [
        (bounds[0] + bounds[3]) * 0.5,
        bounds[1],
        (bounds[2] + bounds[5]) * 0.5,
    ];
    let [x, y, z] = std::array::from_fn(|i| position[i] - target[i]);
    let distance = (x * x + y * y + z * z).sqrt();
    distance.is_finite().then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_sorted_and_invalid_list_links_are_rejected() {
        let mut head = [0usize; 2];
        let mut first = [0usize; 11];
        let mut second = [0usize; 11];
        let (h, a, b) = (
            head.as_mut_ptr() as usize,
            first.as_mut_ptr() as usize,
            second.as_mut_ptr() as usize,
        );
        head.copy_from_slice(&[a, b]);
        first[..2].copy_from_slice(&[b, h]);
        second[..2].copy_from_slice(&[h, a]);
        for (node, id, name) in [(&mut first, 99, "Zulu"), (&mut second, 42, "§aAlpha")] {
            let bytes = name.as_bytes();
            let mut inline = [0u8; 8];
            inline[..bytes.len()].copy_from_slice(bytes);
            node[4] = id;
            node[7] = usize::from_le_bytes(inline);
            node[9] = bytes.len();
            node[10] = 15;
        }
        let map = [0usize, h, 2];
        let get = || unsafe { read_entries(&mut ReadBatch::new(), map.as_ptr() as usize) };
        let mut rows = get().unwrap();
        assert_eq!((rows[0].0, rows[0].1.name.as_str()), (42, "Alpha"));
        assert_eq!((rows[1].0, rows[1].1.name.as_str()), (99, "Zulu"));
        assert_eq!(player_indices(&rows, 42).unwrap().len(), 2);
        assert!(player_indices(&rows, 123).is_none());
        rows[1].0 = 42;
        assert!(player_indices(&rows, 42).is_none());
        unsafe { second.as_mut_ptr().add(1).write(h) };
        assert!(get().is_none());
        unsafe {
            second.as_mut_ptr().add(1).write(a);
            second.as_mut_ptr().write(a);
        }
        assert!(get().is_none());
    }

    #[test]
    fn distance_uses_feet_with_vertical_separation() {
        let bounds = [2.5, 4., -0.5, 3.5, 5.8, 0.5];
        assert_eq!(actor_distance([0.; 3], bounds), Some(5.));
        assert_eq!(actor_distance([f32::NAN, 0., 0.], bounds), None);
    }
}
