//! Native pre-attack selection: no retained slot ownership or synthetic input.
use crate::game::{items, read, readable};
use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        Mutex,
    },
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
pub(in crate::game) static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static UNTIL: AtomicU64 = AtomicU64::new(0);
static RESERVE: AtomicU32 = AtomicU32::new(5);
static AXES: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static STATUS: Mutex<&str> = Mutex::new("待機中");
thread_local! {static AUTOMATIC:Cell<bool>=const{Cell::new(false)};}
pub(in crate::game) fn automated<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            AUTOMATIC.set(self.0);
        }
    }
    let old = AUTOMATIC.replace(true);
    let _restore = Restore(old);
    f()
}
pub fn available() -> Result<(), String> {
    items::available()?;
    attack_available()
}
pub(in crate::game) fn attack_available() -> Result<(), String> {
    crate::game::actions::available()?;
    if ORIGINAL.load(Ordering::Acquire) == 0 {
        Err("攻撃前のフックを準備できません".into())
    } else {
        Ok(())
    }
}
pub fn request(on: bool, now: u64, reserve: u32, axes: bool) {
    RESERVE.store(reserve.min(u16::MAX as u32), Ordering::Relaxed);
    AXES.store(axes, Ordering::Relaxed);
    UNTIL.store(
        if on { now.saturating_add(150) } else { 0 },
        Ordering::Release,
    );
}
pub fn status() -> String {
    format!(
        "{} / 切替{}回",
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()),
        SWITCHES.load(Ordering::Relaxed)
    )
}
fn rank(item: &items::Item, reserve: u32, axes: bool) -> Option<(u8, u8)> {
    if item.count == 0 || item.max_damage == 0 || item.remaining() as u32 <= reserve {
        return None;
    }
    let (material, family) = item.name.rsplit_once('_')?;
    let family = match family {
        "sword" => 2,
        "axe" if axes => 1,
        _ => return None,
    };
    let material = match material {
        "wooden" | "golden" => 1,
        "stone" => 2,
        "iron" => 3,
        "diamond" => 4,
        "netherite" => 5,
        _ => return None,
    };
    Some((family, material))
}
fn choose(inv: &items::Inventory, selected: usize, reserve: u32, axes: bool) -> Option<usize> {
    let current = inv
        .slots
        .get(selected)
        .and_then(Option::as_ref)
        .and_then(|i| rank(i, reserve, axes));
    let mut best = current;
    let mut choice = None;
    for (slot, item) in inv.slots.iter().take(9).enumerate() {
        let Some(value) = item.as_ref().and_then(|i| rank(i, reserve, axes)) else {
            continue;
        };
        if best.is_none_or(|b| value > b) {
            best = Some(value);
            choice = Some(slot);
        }
    }
    choice
}
unsafe fn select(player: usize, now: u64) {
    if now >= UNTIL.load(Ordering::Acquire)
        || !crate::game::item_assist::idle()
        || crate::game::fishing::using_item()
        || crate::game::durability_guard::busy()
    {
        return;
    }
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BUSY.store(false, Ordering::Release);
        }
    }
    let _reset = Reset;
    let Some(inv) = items::snapshot() else {
        return;
    };
    if !readable(player, 0x1c) || inv.world != (read(player + 0x10), read(player + 0x18)) {
        return;
    }
    let Some(selected) = items::selected(player) else {
        return;
    };
    let reserve = RESERVE
        .load(Ordering::Relaxed)
        .max(crate::game::durability_guard::reserve(now));
    let axes = AXES.load(Ordering::Relaxed);
    let Some(slot) = choose(&inv, selected, reserve, axes) else {
        return;
    };
    let Some(candidate) = items::hotbar_item(player, slot) else {
        return;
    };
    if inv.slots.get(slot).and_then(Option::as_ref) != Some(&candidate)
        || rank(&candidate, reserve, axes).is_none()
    {
        return;
    }
    let current = items::hotbar_item(player, selected).and_then(|i| rank(&i, reserve, axes));
    if current.is_some_and(|r| rank(&candidate, reserve, axes).is_none_or(|c| c <= r)) {
        return;
    }
    if items::select_tool(player, slot) {
        SWITCHES.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut status) = STATUS.try_lock() {
            *status = "武器へ切替済み";
        }
    }
}
/// Verified shared GameMode attack helper: (mode, target, attack flag, Vec3*).
pub(in crate::game) unsafe extern "system" fn attack_hook(
    mode: usize,
    target: usize,
    flag: u8,
    point: *const [f32; 3],
) -> u8 {
    crate::game::actions::IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: unsafe extern "system" fn(usize, usize, u8, *const [f32; 3]) -> u8 =
        std::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    let now = GetTickCount64();
    let automatic = AUTOMATIC.get();
    let interested =
        now < UNTIL.load(Ordering::Acquire) || (automatic && crate::game::friends::active());
    if interested && readable(mode, 16) {
        let player = read::<usize>(mode + 8);
        if player != 0
            && player == crate::game::PLAYER.load(Ordering::Acquire)
            && crate::game::fresh(crate::game::SAMPLED_AT.load(Ordering::Acquire))
        {
            if automatic && crate::game::friends::excludes_actor(player, target) {
                crate::game::actions::IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
                return 0;
            }
            if !crate::gui::captures_input()
                && !crate::game::inventory::screen_open()
                && crate::input::is_foreground()
            {
                select(player, now);
            }
        }
    }
    let result = original(mode, target, flag, point);
    crate::game::actions::IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn item(name: &str, remaining: u16) -> Option<items::Item> {
        Some(items::Item {
            name: name.into(),
            count: 1,
            damage: 100 - remaining,
            max_damage: 100,
        })
    }
    #[test]
    fn weapon_selection_is_hotbar_only_preserves_ties_and_respects_durability() {
        let mut inv = items::Inventory {
            selected: 0,
            slots: vec![None; 36],
            armor: vec![],
            world: (1, 2),
        };
        inv.slots[0] = item("bread", 100);
        inv.slots[1] = item("iron_sword", 50);
        inv.slots[2] = item("diamond_sword", 5);
        inv.slots[3] = item("netherite_axe", 100);
        inv.slots[10] = item("netherite_sword", 100);
        assert_eq!(choose(&inv, 0, 5, false), Some(1));
        assert_eq!(choose(&inv, 0, 5, true), Some(1));
        assert_eq!(choose(&inv, 1, 5, true), None);
        inv.slots[1] = None;
        assert_eq!(choose(&inv, 0, 5, false), None);
        assert_eq!(choose(&inv, 0, 5, true), Some(3));
        inv.slots[2] = item("diamond_sword", 6);
        assert_eq!(choose(&inv, 0, 5, false), Some(2));
    }
    #[test]
    fn automatic_scope_restores_nested_and_panicking_calls() {
        assert!(!AUTOMATIC.get());
        automated(|| {
            assert!(AUTOMATIC.get());
            automated(|| assert!(AUTOMATIC.get()));
            assert!(AUTOMATIC.get());
        });
        assert!(!AUTOMATIC.get());
        let _ = std::panic::catch_unwind(|| automated(|| panic!("test")));
        assert!(!AUTOMATIC.get());
    }
}
