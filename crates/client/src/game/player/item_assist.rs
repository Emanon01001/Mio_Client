//! Serialized hotbar ownership: eating takes priority over mining. Native
//! calls execute on the input thread; requests expire if the worker stops.
use crate::game::{actors, blocks, inventory, items};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Mutex,
};
static TOOL: AtomicU64 = AtomicU64::new(0);
static EAT: AtomicU64 = AtomicU64::new(0);
static HUNGER: AtomicU32 = AtomicU32::new(16);
static MIN_DURABILITY: AtomicU32 = AtomicU32::new(5);
static RESTORE: AtomicBool = AtomicBool::new(true);
static TOOL_RESTORE: AtomicBool = AtomicBool::new(true);
static STATUS: Mutex<&str> = Mutex::new("idle");
#[derive(Clone, Default)]
struct ToolTrace {
    at: u64,
    samples: u64,
    selections: u64,
    selected: usize,
    requested: Option<usize>,
    actual: Option<usize>,
    block: Option<String>,
    reason: &'static str,
}
static TOOL_TRACE: Mutex<ToolTrace> = Mutex::new(ToolTrace {
    at: 0,
    samples: 0,
    selections: 0,
    selected: 0,
    requested: None,
    actual: None,
    block: None,
    reason: "no mining input",
});
pub fn tool_diagnostic() -> String {
    let t = TOOL_TRACE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let age = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
        .saturating_sub(t.at);
    format!("autotool: {} age={}ms samples={} selections={} selected={} block={} requested={:?} actual={:?}",t.reason,age,t.samples,t.selections,t.selected+1,t.block.as_deref().unwrap_or("-"),t.requested.map(|s|s+1),t.actual.map(|s|s+1))
}
fn tool_trace(now: u64, selected: Option<usize>, hit: Result<&blocks::Look, &'static str>) {
    if let Ok(mut t) = TOOL_TRACE.try_lock() {
        t.at = now;
        t.samples += 1;
        t.selected = selected.unwrap_or(0);
        match hit {
            Ok(h) => {
                t.block = Some(h.name.clone());
                t.reason = "checking tools";
            }
            Err(reason) => {
                t.block = None;
                t.reason = reason;
            }
        }
    }
}
struct Owned {
    world: (usize, u32),
    original: usize,
    slot: usize,
    at: u64,
    eating: bool,
    pressed: bool,
    count: u8,
}
struct State {
    owned: Option<Owned>,
    cooldown: u64,
    manual: bool,
    next_decision: u64,
}
static STATE: Mutex<State> = Mutex::new(State {
    owned: None,
    cooldown: 0,
    manual: false,
    next_decision: 0,
});
pub fn configure_eat(hunger: u32, restore: bool) {
    HUNGER.store(hunger, Ordering::Relaxed);
    RESTORE.store(restore, Ordering::Relaxed);
}
pub fn configure_tool(min: u32, restore: bool) {
    MIN_DURABILITY.store(min, Ordering::Relaxed);
    TOOL_RESTORE.store(restore, Ordering::Relaxed);
}
pub fn request(eat: bool, on: bool, now: u64) {
    (if eat { &EAT } else { &TOOL }).store(
        if on { now.saturating_add(150) } else { 0 },
        Ordering::Release,
    );
}
pub(in crate::game) fn tool_requested(now: u64) -> bool {
    now < TOOL.load(Ordering::Acquire)
}
pub fn idle() -> bool {
    STATE.try_lock().is_ok_and(|s| s.owned.is_none())
}
pub fn status() -> String {
    crate::localization::text(*STATUS.lock().unwrap_or_else(|e| e.into_inner())).to_string()
}
fn food(name: &str) -> u8 {
    match name {
        "cooked_beef" | "cooked_porkchop" => 8,
        "cooked_mutton" | "cooked_salmon" => 6,
        "bread" | "baked_potato" => 5,
        "cooked_chicken" => 6,
        "cooked_rabbit" => 5,
        "cooked_cod" => 5,
        "apple" | "carrot" | "sweet_berries" => 3,
        "melon_slice" => 2,
        "beetroot" => 1,
        _ => 0,
    }
}
fn tool_kind(block: &str) -> Option<&'static str> {
    let block = block.strip_prefix("minecraft:").unwrap_or(block);
    if [
        "dirt",
        "grass_block",
        "sand",
        "gravel",
        "snow",
        "clay",
        "mud",
        "soul_soil",
        "soul_sand",
        "mycelium",
        "podzol",
        "farmland",
        "dirt_path",
        "concrete_powder",
    ]
    .iter()
    .any(|s| block == *s || block.ends_with(&format!("_{s}")))
    {
        Some("shovel")
    } else if [
        "log",
        "wood",
        "planks",
        "stem",
        "hyphae",
        "bamboo",
        "bookshelf",
        "chest",
        "barrel",
        "pumpkin",
        "melon",
    ]
    .iter()
    .any(|s| block == *s || block.ends_with(&format!("_{s}")))
    {
        Some("axe")
    } else if [
        "stone",
        "ore",
        "deepslate",
        "netherrack",
        "obsidian",
        "ancient_debris",
        "cobblestone",
        "brick",
        "terracotta",
        "concrete",
        "basalt",
        "blackstone",
        "furnace",
        "anvil",
        "iron_block",
        "gold_block",
        "diamond_block",
        "emerald_block",
        "copper_block",
        "quartz_block",
    ]
    .iter()
    .any(|s| block.contains(s))
    {
        Some("pickaxe")
    } else {
        None
    }
}
fn score(item: &items::Item, kind: &str, min: u32, required: u8) -> u32 {
    if item.max_damage > 0 && item.remaining() as u32 <= min {
        return 0;
    }
    let Some(tier) = item.name.strip_suffix(&format!("_{kind}")) else {
        return 0;
    };
    let (harvest, speed) = match tier {
        "netherite" => (4, 9),
        "diamond" => (3, 8),
        "golden" => (0, 10),
        "iron" => (2, 6),
        "stone" => (1, 4),
        "wooden" => (0, 2),
        _ => (0, 0),
    };
    if harvest < required {
        0
    } else {
        speed
    }
}
fn best_tool(
    slots: &[Option<items::Item>],
    selected: usize,
    block: &str,
    min: u32,
) -> Option<usize> {
    let kind = tool_kind(block)?;
    let required = if kind != "pickaxe" {
        0
    } else if block.contains("obsidian") || block.contains("ancient_debris") {
        3
    } else if ["diamond", "emerald", "gold", "redstone"]
        .iter()
        .any(|s| block.contains(s))
    {
        2
    } else if ["iron_ore", "copper_ore", "lapis"]
        .iter()
        .any(|s| block.contains(s))
    {
        1
    } else {
        0
    };
    let current = slots
        .get(selected)?
        .as_ref()
        .map_or(0, |i| score(i, kind, min, required));
    slots
        .iter()
        .take(9)
        .enumerate()
        .filter_map(|(s, i)| {
            let score = score(i.as_ref()?, kind, min, required);
            (score > current).then_some((s, score))
        })
        .max_by_key(|(_, v)| *v)
        .map(|(s, _)| s)
}
unsafe fn finish(
    s: &mut State,
    instance: usize,
    player: usize,
    world: Option<(usize, u32)>,
    selected: Option<usize>,
    manual_use: bool,
) {
    if let Some(o) = s.owned.take() {
        // The input tick's CI is live even when its previous player disappeared.
        // The normal release handler tolerates no local player and clears the hold.
        if o.pressed && !manual_use {
            items::use_button(instance, false);
        }
        if world == Some(o.world)
            && !manual_use
            && (if o.eating { &RESTORE } else { &TOOL_RESTORE }).load(Ordering::Relaxed)
            && selected == Some(o.slot)
            && (o.eating
                || crate::game::durability_guard::restore_allowed(player, o.original, unsafe {
                    windows_sys::Win32::System::SystemInformation::GetTickCount64()
                }))
        {
            if o.eating {
                items::select(instance, o.original);
            } else {
                let _ = items::select_tool(player, o.original);
            }
        }
    }
}
fn ray_hits(eye: [f32; 3], direction: [f32; 3], bounds: [f32; 6], distance: f32) -> bool {
    let mut near = 0f32;
    let mut far = distance;
    for i in 0..3 {
        if direction[i].abs() < 1e-6 {
            if eye[i] < bounds[i] - 0.1 || eye[i] > bounds[i + 3] + 0.1 {
                return false;
            }
        } else {
            let a = (bounds[i] - 0.1 - eye[i]) / direction[i];
            let b = (bounds[i + 3] + 0.1 - eye[i]) / direction[i];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return false;
            }
        }
    }
    far >= 0.
}
fn mining_hit() -> Result<blocks::Look, &'static str> {
    let hit = blocks::look()
        .ok_or("no block snapshot")?
        .ok_or("aiming at air")?;
    if hit.distance > 5. {
        return Err("block beyond reach");
    }
    let p = crate::api::game()
        .and_then(|g| g.player())
        .map_err(|_| "no player snapshot")?;
    let map = actors::actors().ok_or("no actor snapshot")?;
    if map
        .values()
        .filter(|a| !a.local && a.health.is_some())
        .any(|a| {
            a.bounds.is_some_and(|b| {
                ray_hits(p.eye_position.into(), p.direction().into(), b, hit.distance)
            })
        })
    {
        Err("entity in front of block")
    } else {
        Ok(hit)
    }
}
/// Run before vanilla's first destroy action: instant/creative breaks can
/// remove the aimed block before the next input tick gets to inspect it.
pub(in crate::game) unsafe fn before_attack(instance: usize, player: usize, now: u64) {
    if now >= TOOL.load(Ordering::Acquire) || inventory::screen_open() {
        return;
    }
    // The periodic 50ms scan throttle must not delay a fresh physical press.
    // Keep `cooldown`: a manual slot change still gets its full handoff delay.
    if let Ok(mut s) = STATE.try_lock() {
        s.next_decision = 0;
    }
    let Ok(l) = crate::game::layout::layout() else {
        return;
    };
    let (eye, view) = {
        let mut m = crate::game::memory::ReadBatch::new();
        if !m.readable(player, l.actor_state + 8) || !m.readable(instance + l.view_matrix, 48) {
            return;
        }
        let state = crate::game::read::<usize>(player + l.actor_state);
        if !m.readable(state, 12) {
            return;
        }
        let eye = crate::game::read::<[f32; 3]>(state);
        let view = crate::game::read::<[f32; 12]>(instance + l.view_matrix);
        (eye, view)
    };
    blocks::refresh_look(player, eye, [-view[8], -view[9], -view[10]]);
    let _ = sample(instance, player, true, true, now);
}
fn tool_finished(requested: bool, hit: Result<&blocks::Look, &str>) -> bool {
    // Air between adjacent blocks is normal while the destroy button is held.
    // Keep the owned tool across that gap; combat and button release end it.
    !requested || matches!(hit, Err("entity in front of block"))
}
pub(in crate::game) unsafe fn sample(
    instance: usize,
    player: usize,
    active: bool,
    mining: bool,
    now: u64,
) -> bool {
    let Ok(mut s) = STATE.try_lock() else {
        return true;
    };
    if !mining {
        s.manual = false;
    }
    let requested = active
        && !inventory::screen_open()
        && (now < EAT.load(Ordering::Acquire) || mining && now < TOOL.load(Ordering::Acquire));
    if s.owned.is_none() && !requested {
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = "idle";
        return false;
    }
    let manual_use = crate::input::key_down(2);
    let inv = items::snapshot();
    let selected = items::selected(player);
    let world = if crate::game::readable(player, 0x1c) {
        Some((
            crate::game::read(player + 0x10),
            crate::game::read(player + 0x18),
        ))
    } else {
        None
    };
    let safe = active && !inventory::screen_open();
    let eat = safe && now < EAT.load(Ordering::Acquire);
    let tool = safe && now < TOOL.load(Ordering::Acquire) && mining;
    let aimed_result = if tool {
        let hit = mining_hit();
        tool_trace(now, selected, hit.as_ref().map_err(|e| *e));
        hit
    } else {
        Err("no mining input")
    };
    let aimed = aimed_result.as_ref().ok();
    let mut status = "idle";
    // Hand a worn owned tool to DurabilityGuard without restoring another worn
    // slot. AutoEat retains its exclusive ownership until its normal cleanup.
    if crate::game::durability_guard::requested(now)
        && s.owned.as_ref().is_some_and(|o| {
            !o.eating
                && selected == Some(o.slot)
                && world == Some(o.world)
                && inv
                    .as_ref()
                    .filter(|i| Some(i.world) == world)
                    .and_then(|i| i.slots.get(o.slot))
                    .and_then(Option::as_ref)
                    .is_some_and(|i| {
                        crate::game::durability_guard::worn(
                            i,
                            crate::game::durability_guard::reserve(now),
                        )
                    })
        })
    {
        s.owned = None;
        s.next_decision = now.saturating_add(50);
    }
    if let Some(o) = s.owned.as_ref() {
        let cancelled = world != Some(o.world) || selected != Some(o.slot);
        let done = if o.eating {
            !eat || manual_use
                || mining
                || now.saturating_sub(o.at) > 5000
                || actors::local_vitals().is_none_or(|v| v.hunger >= 20.)
                || inv.as_ref().is_none_or(|i| {
                    i.slots
                        .get(o.slot)
                        .and_then(Option::as_ref)
                        .is_none_or(|i| i.count != o.count)
                })
        } else {
            tool_finished(tool, aimed_result.as_ref().map_err(|e| *e))
        };
        if cancelled || done {
            let delay = if cancelled || manual_use {
                3000
            } else if o.eating {
                250
            } else {
                0
            };
            if cancelled {
                s.manual = true;
            }
            finish(&mut s, instance, player, world, selected, manual_use);
            s.cooldown = now.saturating_add(delay);
        } else if o.eating {
            if !o.pressed && now.saturating_sub(o.at) >= 120 {
                items::use_button(instance, true);
                if let Some(o) = s.owned.as_mut() {
                    o.pressed = true;
                }
            }
            status = "eating";
        } else {
            // Re-evaluate when the aim changes while the attack button stays held.
            if let (Some(inv), Some(hit)) = (
                inv.as_ref().filter(|i| Some(i.world) == world),
                aimed.as_ref(),
            ) {
                if let Some(slot) = best_tool(
                    &inv.slots,
                    o.slot,
                    &hit.name,
                    MIN_DURABILITY
                        .load(Ordering::Relaxed)
                        .max(crate::game::durability_guard::tool_reserve(now)),
                ) {
                    let accepted = items::select_tool(player, slot);
                    let actual = items::selected(player);
                    if let Ok(mut t) = TOOL_TRACE.try_lock() {
                        t.selections += 1;
                        t.requested = Some(slot);
                        t.actual = actual;
                        t.reason = if actual == Some(slot) {
                            "tool selected"
                        } else {
                            "slot selection rejected"
                        };
                    }
                    if accepted && actual == Some(slot) {
                        if let Some(o) = s.owned.as_mut() {
                            o.slot = slot;
                        }
                    }
                }
            }
            status = "mining";
        }
    }
    if s.owned.is_none() && now >= s.cooldown && now >= s.next_decision && !s.manual && !manual_use
    {
        s.next_decision = now.saturating_add(50);
        if let (Some(inv), Some(selected)) =
            (inv.as_ref().filter(|i| Some(i.world) == world), selected)
        {
            if eat
                && actors::local_vitals()
                    .is_some_and(|v| v.hunger <= HUNGER.load(Ordering::Relaxed) as f32)
                && !mining
            {
                // Only start ordinary item use looking into air, away from entities.
                let clear = blocks::look() == Some(None)
                    && actors::actors().is_some_and(|map| {
                        map.values().filter(|a| !a.local).all(|a| {
                            a.bounds.is_none_or(|b| {
                                let Some(p) = crate::api::game().and_then(|g| g.player()).ok()
                                else {
                                    return false;
                                };
                                let d = [
                                    (b[0] + b[3]) * 0.5 - p.position.x,
                                    (b[1] + b[4]) * 0.5 - p.position.y,
                                    (b[2] + b[5]) * 0.5 - p.position.z,
                                ];
                                d.iter().map(|v| v * v).sum::<f32>() > 36.
                            })
                        })
                    });
                if clear {
                    if let Some((slot, item)) = inv
                        .slots
                        .iter()
                        .take(9)
                        .enumerate()
                        .filter_map(|(s, i)| {
                            i.as_ref().filter(|i| food(&i.name) > 0).map(|i| (s, i))
                        })
                        .max_by_key(|(_, i)| food(&i.name))
                    {
                        items::select(instance, slot);
                        s.owned = Some(Owned {
                            world: inv.world,
                            original: selected,
                            slot,
                            at: now,
                            eating: true,
                            pressed: false,
                            count: item.count,
                        });
                        status = "selecting food";
                    } else {
                        status = "no ordinary food in hotbar";
                    }
                } else {
                    status = "look into air, away from mobs";
                }
            } else if tool {
                if let Some(hit) = aimed.as_ref() {
                    if let Some(slot) = best_tool(
                        &inv.slots,
                        selected,
                        &hit.name,
                        MIN_DURABILITY
                            .load(Ordering::Relaxed)
                            .max(crate::game::durability_guard::tool_reserve(now)),
                    ) {
                        let accepted = items::select_tool(player, slot);
                        let actual = items::selected(player);
                        if let Ok(mut t) = TOOL_TRACE.try_lock() {
                            t.selections += 1;
                            t.requested = Some(slot);
                            t.actual = actual;
                            t.reason = if actual == Some(slot) {
                                "tool selected"
                            } else {
                                "slot selection rejected"
                            };
                        }
                        if accepted && actual == Some(slot) {
                            s.owned = Some(Owned {
                                world: inv.world,
                                original: selected,
                                slot,
                                at: now,
                                eating: false,
                                pressed: false,
                                count: 0,
                            });
                            status = "mining";
                        } else {
                            s.cooldown = now.saturating_add(250);
                        }
                    } else if let Ok(mut t) = TOOL_TRACE.try_lock() {
                        t.reason = if tool_kind(&hit.name).is_some() {
                            "no better usable hotbar tool"
                        } else {
                            "unrecognized block family"
                        };
                    }
                }
            }
        }
    }
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = status;
    s.owned.as_ref().is_some_and(|o| o.eating)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mining_hold_keeps_tool_across_air_and_releases_for_combat_or_button_up() {
        let block = blocks::Look {
            name: "stone".into(),
            position: [0; 3],
            distance: 2.,
        };
        assert!(!tool_finished(true, Ok(&block)));
        assert!(!tool_finished(true, Err("aiming at air")));
        assert!(tool_finished(true, Err("entity in front of block")));
        assert!(tool_finished(false, Ok(&block)));
        assert!(tool_finished(false, Err("aiming at air")));
    }
    #[test]
    fn entities_in_front_of_a_block_prevent_tool_switching() {
        let b = [-0.3, 0., 2., 0.3, 2., 2.6];
        assert!(ray_hits([0., 1., 0.], [0., 0., 1.], b, 4.));
        assert!(!ray_hits([0., 1., 0.], [0., 0., 1.], b, 1.));
        assert!(!ray_hits([2., 1., 0.], [0., 0., 1.], b, 4.));
        assert!(!ray_hits([0., 1., 0.], [0., 0., -1.], b, 4.));
    }
    fn item(name: &str, left: u16) -> Option<items::Item> {
        Some(items::Item {
            name: name.into(),
            count: 1,
            damage: 100 - left,
            max_damage: 100,
        })
    }
    #[test]
    fn tool_selection_respects_wear_and_material() {
        let slots = vec![
            item("netherite_axe", 90),
            item("netherite_pickaxe", 3),
            item("iron_pickaxe", 80),
            item("netherite_shovel", 80),
        ];
        assert_eq!(best_tool(&slots, 0, "diamond_ore", 5), Some(2));
        assert_eq!(best_tool(&slots, 0, "grass_block", 5), Some(3));
        assert_eq!(best_tool(&slots, 0, "oak_log", 5), None);
        assert_eq!(best_tool(&slots, 0, "air", 5), None);
        let gold = vec![item("golden_pickaxe", 80), item("iron_pickaxe", 80)];
        assert_eq!(best_tool(&gold, 0, "minecraft:diamond_ore", 5), Some(1));
        assert_eq!(best_tool(&gold, 1, "stone", 5), Some(0));
        assert_eq!(food("rotten_flesh"), 0);
        assert_eq!(food("golden_apple"), 0);
        assert!(food("cooked_chicken") > 0);
    }
}
