//! Remaining-durability protection: same-family hotbar selection or a balanced
//! native inventory swap. No fabricated packets or synthetic keys.
use super::{inventory_transfer, items, read, readable};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Mutex,
};
static UNTIL: AtomicU64 = AtomicU64::new(0);
static RESERVE: AtomicU32 = AtomicU32::new(10);
static EMPTY: AtomicBool = AtomicBool::new(true);
pub const MAX_RESERVE: u32 = u16::MAX as u32;
static STATE: Mutex<State> = Mutex::new(State {
    next: 0,
    status: "waiting for items",
    switches: 0,
    trace: Trace::new(),
    pending: None,
});
struct State {
    next: u64,
    status: &'static str,
    switches: u64,
    trace: Trace,
    pending: Option<Pending>,
}
struct Pending {
    world: (usize, u32),
    source: usize,
    destination: usize,
    old: items::Item,
    replacement: items::Item,
    at: u64,
}
pub(super) fn busy() -> bool {
    STATE.try_lock().map_or(true, |s| s.pending.is_some())
}
pub fn available() -> Result<(), String> {
    inventory_transfer::available()
}
struct Trace {
    at: u64,
    reason: &'static str,
    selected: Option<usize>,
    name: String,
    remaining: u16,
    maximum: u16,
    reserve: u32,
    attempted: u64,
    target: Option<usize>,
    actual: Option<usize>,
}
impl Trace {
    const fn new() -> Self {
        Self {
            at: 0,
            reason: "no active sample yet",
            selected: None,
            name: String::new(),
            remaining: 0,
            maximum: 0,
            reserve: 0,
            attempted: 0,
            target: None,
            actual: None,
        }
    }
    fn observe(&mut self, selected: usize, item: &items::Item, reserve: u32) {
        self.selected = Some(selected);
        self.name.clone_from(&item.name);
        self.remaining = item.remaining();
        self.maximum = item.max_damage;
        self.reserve = reserve;
    }
}
impl State {
    fn playing(&mut self, reason: &'static str, now: u64) {
        self.status = reason;
        self.trace.reason = reason;
        self.trace.at = now;
    }
}
pub fn request(on: bool, now: u64, reserve: u32, empty: bool) {
    RESERVE.store(reserve.clamp(1, MAX_RESERVE), Ordering::Relaxed);
    EMPTY.store(empty, Ordering::Relaxed);
    UNTIL.store(
        if on { now.saturating_add(150) } else { 0 },
        Ordering::Release,
    );
}
pub(super) fn requested(now: u64) -> bool {
    now < UNTIL.load(Ordering::Acquire)
}
pub(super) fn reserve(now: u64) -> u32 {
    if requested(now) {
        RESERVE.load(Ordering::Relaxed)
    } else {
        0
    }
}
fn family(name: &str) -> Option<&str> {
    for (suffix, kind) in [
        ("_pickaxe", "pickaxe"),
        ("_axe", "axe"),
        ("_shovel", "shovel"),
        ("_hoe", "hoe"),
        ("_sword", "sword"),
    ] {
        if name.ends_with(suffix) {
            return Some(kind);
        }
    }
    matches!(
        name,
        "bow" | "crossbow" | "fishing_rod" | "shears" | "flint_and_steel" | "brush" | "mace"
    )
    .then_some(name)
}
fn protected(item: &items::Item) -> bool {
    item.max_damage > 0 && family(&item.name).is_some()
}
fn candidate(current: &items::Item, other: &items::Item, threshold: u32) -> bool {
    other.count > 0
        && protected(other)
        && family(&other.name) == family(&current.name)
        && other.remaining() as u32 >= threshold
}
fn material(name: &str) -> u8 {
    match name.split('_').next().unwrap_or("") {
        "netherite" => 6,
        "diamond" => 5,
        "iron" => 4,
        "stone" => 3,
        "wooden" => 2,
        "golden" => 1,
        _ => 0,
    }
}
pub(super) fn worn(item: &items::Item, reserve: u32) -> bool {
    protected(item) && (item.remaining() as u32) < reserve
}
pub(super) fn tool_reserve(now: u64) -> u32 {
    reserve(now).saturating_sub(1)
}
fn choose(
    slots: &[Option<items::Item>],
    selected: usize,
    current: &items::Item,
    reserve: u32,
    empty: bool,
) -> Option<(usize, bool)> {
    if selected >= 9 || selected >= slots.len() || !worn(current, reserve) {
        return None;
    }
    slots
        .iter()
        .take(36)
        .enumerate()
        .filter_map(|(i, item)| {
            let item = item.as_ref()?;
            (i != selected && candidate(current, item, reserve)).then_some((i, item))
        })
        .max_by_key(|&(i, item)| {
            (
                i < 9,
                item.name == current.name,
                material(&item.name),
                item.remaining(),
                std::cmp::Reverse(i),
            )
        })
        .map(|(i, _)| (i, false))
        .or_else(|| {
            empty
                .then(|| {
                    slots
                        .iter()
                        .take(9)
                        .position(Option::is_none)
                        .map(|i| (i, true))
                })
                .flatten()
        })
}
pub(super) unsafe fn restore_allowed(player: usize, slot: usize, now: u64) -> bool {
    !requested(now)
        || items::hotbar_item(player, slot).map_or_else(
            || items::fresh_hotbar_empty(player, slot),
            |i| !worn(&i, reserve(now)),
        )
}
/// Actions calls this only while other item helpers do not own the hotbar.
pub(super) unsafe fn sample(player: usize, active: bool, now: u64, force: bool) {
    if !requested(now) {
        return;
    }
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    if !active {
        state.status = "paused: input gate / another item helper";
        return;
    }
    if crate::input::key_down(2) {
        state.status = "paused: manual right button";
        return;
    }
    if let Some(p) = state.pending.take() {
        if !readable(player, 0x1c) || p.world != (read(player + 0x10), read(player + 0x18)) {
            state.playing("inventory swap world changed", now);
            state.next = now.saturating_add(1000);
            return;
        }
        if now.saturating_sub(p.at) < 250 {
            state.pending = Some(p);
            return;
        }
        let source = items::inventory_item(player, p.source);
        let target = items::inventory_item(player, p.destination);
        let retained = source.as_ref().is_some_and(|i| i == &p.old)
            && target.as_ref().is_some_and(|i| {
                i.name == p.replacement.name
                    && i.max_damage == p.replacement.max_damage
                    && i.count == p.replacement.count
                    && i.damage >= p.replacement.damage
            });
        state.trace.actual = items::selected(player);
        if retained {
            state.switches += 1;
            state.playing("inventory tool swapped and rechecked", now);
        } else {
            if now.saturating_sub(p.at) < 2000 {
                state.pending = Some(p);
                state.playing("waiting for native inventory response", now);
                return;
            }
            state.playing("inventory swap not retained / slots changed", now);
            state.next = now.saturating_add(3000);
            return;
        }
    }
    if !force && now < state.next {
        return;
    }
    state.next = now.saturating_add(50);
    let Some(inv) = items::snapshot() else {
        state.playing("waiting for items", now);
        return;
    };
    if !readable(player, 0x1c) || inv.world != (read(player + 0x10), read(player + 0x18)) {
        state.playing("waiting for current world", now);
        return;
    }
    let Some(selected) = items::selected(player) else {
        state.playing("selection unavailable", now);
        return;
    };
    let Some(current) = items::hotbar_item(player, selected) else {
        state.playing("selected slot empty or item read rejected", now);
        return;
    };
    let reserve = reserve(now);
    state.trace.observe(selected, &current, reserve);
    if !worn(&current, reserve) {
        state.playing(
            if protected(&current) {
                "remaining durability is not below threshold"
            } else {
                "held item is not a protected tool"
            },
            now,
        );
        return;
    }
    // Use the snapshot only to choose a candidate, never to authorize a switch.
    let slots = &inv.slots;
    if selected >= slots.len() {
        state.playing("incomplete inventory", now);
        return;
    }
    let Some((mut slot, empty)) = choose(
        slots,
        selected,
        &current,
        reserve,
        EMPTY.load(Ordering::Relaxed),
    ) else {
        state.playing("worn: no usable same-family tool", now);
        return;
    };
    // None is ambiguous (empty or invalid). Confirm the candidate's validated
    // native count; a read rejection must not authorize an empty fallback.
    let valid = if empty {
        let found = slots
            .iter()
            .take(9)
            .enumerate()
            .find(|(i, item)| item.is_none() && items::fresh_hotbar_empty(player, *i))
            .map(|(i, _)| i);
        if let Some(found) = found {
            slot = found;
            true
        } else {
            false
        }
    } else {
        items::inventory_item(player, slot).is_some_and(|i| candidate(&current, &i, reserve))
    };
    if !valid || items::selected(player) != Some(selected) {
        state.playing("inventory changed / candidate read rejected; retrying", now);
        return;
    }
    if slot >= 9 {
        let Some(replacement) =
            items::inventory_item(player, slot).filter(|i| candidate(&current, i, reserve))
        else {
            return;
        };
        state.trace.attempted = now;
        state.trace.target = Some(slot);
        match inventory_transfer::swap(player, slot, selected) {
            Ok(()) => {
                state.pending = Some(Pending {
                    world: inv.world,
                    source: slot,
                    destination: selected,
                    old: current,
                    replacement,
                    at: now,
                });
                state.playing("inventory swap requested; waiting to recheck", now);
            }
            Err(reason) => {
                state.playing(reason, now);
                state.next = now.saturating_add(1000);
            }
        }
        return;
    }
    let accepted = items::select_tool(player, slot);
    state.trace.attempted = now;
    state.trace.target = Some(slot);
    state.trace.actual = items::selected(player);
    if accepted {
        state.switches += 1;
        state.playing(
            if empty {
                "worn tool: selected empty slot"
            } else {
                "switched to spare"
            },
            now,
        );
        state.next = now.saturating_add(250);
    } else {
        state.playing("slot selection rejected", now);
    }
}
pub fn status() -> String {
    STATE.try_lock().map_or_else(
        |_| "耐久度を確認中".into(),
        |s| {
            format!(
                "{} / 切り替え{}回",
                crate::localization::text(s.status),
                s.switches
            )
        },
    )
}
pub fn diagnostic() -> String {
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    let Ok(s) = STATE.try_lock() else {
        return "durabilityguard: sampling".into();
    };
    let t = &s.trace;
    format!("durabilityguard: {}; requested={} remaining durability threshold={} (strictly below), empty fallback={}; switches={}\nlast active: {} age={}ms slot={:?} item={} remaining={}/{} checked threshold={}\nlast selection: age={}ms source={:?} actual hotbar={:?}",
        s.status,requested(now),RESERVE.load(Ordering::Relaxed),EMPTY.load(Ordering::Relaxed),s.switches,
        t.reason,now.saturating_sub(t.at),t.selected.map(|s|s+1),t.name,t.remaining,t.maximum,t.reserve,
        now.saturating_sub(t.attempted),t.target.map(|s|s+1),t.actual.map(|s|s+1))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn tool(name: &str, remaining: u16) -> items::Item {
        items::Item {
            name: name.into(),
            count: 1,
            damage: 100 - remaining,
            max_damage: 100,
        }
    }
    #[test]
    fn spare_selection_respects_identity_wear_hotbar_and_deterministic_ties() {
        let mut slots = vec![None; 36];
        slots[0] = Some(tool("iron_pickaxe", 9));
        slots[1] = Some(tool("diamond_pickaxe", 100));
        slots[2] = Some(tool("iron_pickaxe", 20));
        slots[3] = Some(tool("iron_pickaxe", 80));
        slots[4] = Some(tool("iron_pickaxe", 80));
        slots[10] = Some(tool("iron_pickaxe", 100));
        let current = slots[0].clone().unwrap();
        assert_eq!(choose(&slots, 0, &current, 10, true), Some((3, false)));
        assert_eq!(choose(&slots, 0, &current, 9, true), None);
        slots[2] = None;
        slots[3] = None;
        slots[4] = None;
        assert_eq!(choose(&slots, 0, &current, 10, true), Some((1, false)));
        slots[1] = None;
        assert_eq!(choose(&slots, 0, &current, 10, false), Some((10, false)));
        slots[10] = None;
        assert_eq!(choose(&slots, 0, &current, 10, true), Some((1, true)));
        assert_eq!(choose(&slots, 0, &current, 10, false), None);
        for slot in slots.iter_mut().take(9).skip(2) {
            *slot = Some(tool("iron_pickaxe", 9));
        }
        slots[1] = Some(tool("diamond_axe", 100));
        assert_eq!(choose(&slots, 0, &current, 10, true), None);
        assert!(choose(&slots, 100, &current, 10, true).is_none());
        assert!(choose(&slots, 10, &current, 10, true).is_none());
    }
    #[test]
    fn ordinary_items_and_equipment_are_never_selected_for_protection() {
        assert!(!protected(&tool("netherite_chestplate", 1)));
        assert!(!protected(&tool("bread", 1)));
        assert!(protected(&tool("fishing_rod", 1)));
        assert!(protected(&tool("netherite_sword", 1)));
    }
    #[test]
    fn remaining_durability_is_strict_and_families_do_not_cross() {
        let current = tool("wooden_hoe", 9);
        assert!(worn(&current, 10));
        assert!(!worn(&tool("wooden_hoe", 10), 10));
        assert!(candidate(&current, &tool("diamond_hoe", 10), 10));
        assert!(!candidate(&current, &tool("diamond_axe", 100), 10));
        assert!(!candidate(&current, &tool("diamond_hoe", 9), 10));
        let mut slots = vec![None; 36];
        slots[0] = Some(current.clone());
        slots[10] = Some(tool("diamond_hoe", 100));
        slots[2] = Some(tool("stone_hoe", 50));
        assert_eq!(choose(&slots, 0, &current, 10, false), Some((2, false)));
        slots[2] = None;
        assert_eq!(choose(&slots, 0, &current, 10, false), Some((10, false)));
    }
}
