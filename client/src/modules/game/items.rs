//! Game-thread item snapshots. Layouts verified against this exact executable;
//! render/worker threads receive owned strings and numbers only.
use super::{
    actors::type_hash,
    entities::{component_pools, PoolView},
    known_build,
    memory::ReadBatch,
    patch, read,
};
use crate::signatures::*;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub count: u8,
    pub damage: u16,
    pub max_damage: u16,
}
impl Item {
    pub fn remaining(&self) -> u16 {
        self.max_damage.saturating_sub(self.damage)
    }
}
#[derive(Clone, Debug)]
pub struct Inventory {
    pub selected: usize,
    pub slots: Vec<Option<Item>>,
    pub armor: Vec<Option<Item>>,
    pub world: (usize, u32),
}
struct Native {
    damage: usize,
    max: usize,
    component_max: usize,
    inventory_get: usize,
    armor_get: usize,
    pub select: usize,
    proxy_select: usize,
    pub down: usize,
    pub up: usize,
}
static NATIVE: OnceLock<Result<Native, String>> = OnceLock::new();
static USERS: AtomicUsize = AtomicUsize::new(0);
static LAST: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT: Mutex<Option<(u64, Arc<Inventory>)>> = Mutex::new(None);
pub fn available() -> Result<(), String> {
    NATIVE
        .get_or_init(|| {
            if !known_build() {
                return Err(format!("items require {}", crate::modules::game::SUPPORTED));
            }
            Ok(Native {
                damage: patch::find(ITEM_DAMAGE_SIGNATURE)?,
                max: patch::find(ITEM_MAX_DAMAGE_SIGNATURE)?,
                component_max: patch::find(COMPONENT_MAX_DAMAGE_SIGNATURE)?,
                inventory_get: patch::find(INVENTORY_ITEM_SIGNATURE)?,
                armor_get: patch::find(ARMOR_ITEM_SIGNATURE)?,
                select: patch::find(SELECT_SLOT_SIGNATURE)?,
                proxy_select: patch::find(PROXY_SELECT_SIGNATURE)?,
                down: patch::find(USE_DOWN_SIGNATURE)?,
                up: patch::find(USE_UP_SIGNATURE)?,
            })
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
pub fn use_items(on: bool) -> Result<(), String> {
    if on {
        available()?;
        super::acquire()?;
        USERS.fetch_add(1, Ordering::Relaxed);
    } else {
        super::release()?;
        let _ = USERS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
        if USERS.load(Ordering::Relaxed) == 0 {
            *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            LAST.store(0, Ordering::Relaxed);
        }
    }
    Ok(())
}
pub fn snapshot() -> Option<Arc<Inventory>> {
    if USERS.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let s = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    s.as_ref()
        .filter(|(at, _)| {
            unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
                .saturating_sub(*at)
                < 500
        })
        .map(|(_, s)| Arc::clone(s))
}
pub(crate) fn reading_for_local(owner: (usize, u32)) -> Option<(u64, Arc<Inventory>)> {
    if USERS.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let guard = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    let (at, value) = guard.as_ref()?;
    (value.world == owner
        && unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
            .saturating_sub(*at)
            < 500)
        .then(|| (*at, Arc::clone(value)))
}
unsafe fn stack(address: usize, native: &Native) -> Option<Item> {
    let (name, count, item, max_getter, fixed_max) = {
        let mut m = ReadBatch::new();
        if !m.readable(address, 0x98) {
            return None;
        }
        let weak = read::<usize>(address + 8);
        let count = read::<u8>(address + 0x22);
        if count == 0 || read::<u8>(address + 0x23) != 1 || !m.readable(weak, 8) {
            return None;
        }
        let item = read::<usize>(weak);
        if !m.readable(item, 0x158) {
            return None;
        }
        let vt = read::<usize>(item);
        if !m.readable(vt, 0x128) {
            return None;
        }
        let max_getter = read::<usize>(vt + 0x120);
        if max_getter != native.max && max_getter != native.component_max {
            return None;
        }
        let name = m.string(item + 0xD8, 96)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        {
            return None;
        }
        let max_damage = read::<u16>(item + 0x150);
        let tag = read::<usize>(address + 0x10);
        if tag != 0 && !m.readable(tag, 0x20) {
            return None;
        }
        // Region checks do not survive the native call below.
        (name, count, item, max_getter, max_damage)
    };
    let max_damage = if max_getter == native.max {
        fixed_max
    } else {
        let getter: unsafe extern "system" fn(usize) -> u16 = std::mem::transmute(max_getter);
        getter(item)
    };
    let damage = if max_damage == 0 {
        0
    } else {
        let f: unsafe extern "system" fn(usize) -> u16 = std::mem::transmute(native.damage);
        f(address)
    };
    Some(Item {
        name,
        count,
        damage,
        max_damage,
    })
}
unsafe fn collect(player: usize, n: &Native) -> Option<Inventory> {
    let (world, selected, begin, armor) = {
        let mut m = ReadBatch::new();
        if !m.readable(player, 0x5e8) {
            return None;
        }
        let world = (read(player + 0x10), read(player + 0x18));
        let proxy = read::<usize>(player + 0x5e0);
        if !m.readable(proxy, 0xc0) {
            return None;
        }
        let selected = read::<u32>(proxy + 0x10) as usize;
        if selected > 8 {
            return None;
        }
        let container = read::<usize>(proxy + 0xb8);
        if !container_kind(&mut m, container, n.inventory_get) {
            return None;
        }
        let (begin, end) = m.span(container.checked_add(0x198)?, 0x98)?;
        if end - begin != 36 * 0x98 {
            return None;
        }
        let armor = (|| {
            let pool = component_pools(&mut m, world.0, [type_hash("ActorEquipmentComponent")])[0]?;
            let c = PoolView::new(&mut m, pool, 16)?.component(&mut m, world.1)?;
            let container = read::<usize>(c + 8);
            if !container_kind(&mut m, container, n.armor_get) || read::<u8>(container + 8) != 0x12
            {
                return None;
            }
            let (a, b) = m.span(container.checked_add(0x1a0)?, 0x98)?;
            ((b - a) == 5 * 0x98).then_some(a)
        })();
        (world, selected, begin, armor)
    };
    let slots = (0..36).map(|i| stack(begin + i * 0x98, n)).collect();
    let armor = (0..4)
        .map(|i| armor.and_then(|a| stack(a + i * 0x98, n)))
        .collect();
    Some(Inventory {
        selected,
        slots,
        armor,
        world,
    })
}
unsafe fn container_kind(m: &mut ReadBatch, container: usize, get: usize) -> bool {
    if !m.readable(container, 0x1b8) {
        return false;
    }
    let vt = read::<usize>(container);
    m.readable(vt, 0x40) && read::<usize>(vt + 0x38) == get
}
pub fn diagnostic() -> String {
    let Some(s) = snapshot() else {
        let reason=available().err().unwrap_or_else(||{
            let users=USERS.load(Ordering::Relaxed);
            if users==0{return "items: disabled (enable armorhud/inventoryhud/autotool/autoeat)".into();}
            let at=LAST.load(Ordering::Relaxed);
            if at==0{return format!("items: waiting for first sample; users={users}");}
            let age=unsafe{windows_sys::Win32::System::SystemInformation::GetTickCount64()}.saturating_sub(at);
            format!("items: no live snapshot; users={users} last_sample_age={age}ms (paused/stale or read rejected)")
        });
        // The game may stop input/HUD sampling on the pause screen. Preserve
        // the last mining decision even when live inventory data has expired.
        return format!("{reason}\n{}", super::item_assist::tool_diagnostic());
    };
    let describe = |slot: usize, i: &Item| {
        format!(
            "{}:{} x{} durability={}/{}",
            slot + 1,
            i.name,
            i.count,
            i.remaining(),
            i.max_damage
        )
    };
    let hotbar = s
        .slots
        .iter()
        .take(9)
        .enumerate()
        .filter_map(|(slot, i)| i.as_ref().map(|i| describe(slot, i)))
        .collect::<Vec<_>>()
        .join(", ");
    let armor = s
        .armor
        .iter()
        .enumerate()
        .filter_map(|(slot, i)| i.as_ref().map(|i| describe(slot, i)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "items: selected={} hotbar=[{}] armor=[{}] automation={}\n{}",
        s.selected + 1,
        hotbar,
        armor,
        super::item_assist::status(),
        super::item_assist::tool_diagnostic()
    )
}
pub(super) unsafe fn sample(player: usize, now: u64) {
    if USERS.load(Ordering::Relaxed) == 0 || now.saturating_sub(LAST.load(Ordering::Relaxed)) < 100
    {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let Some(Ok(n)) = NATIVE.get() else {
        return;
    };
    let found = collect(player, n).map(|s| (now, Arc::new(s)));
    if let Ok(mut s) = SNAPSHOT.try_lock() {
        *s = found;
    }
}
pub(super) unsafe fn select(instance: usize, slot: usize) {
    if slot < 9 && super::actions_build() {
        if let Some(Ok(n)) = NATIVE.get() {
            let f: unsafe extern "system" fn(usize, i32) = std::mem::transmute(n.select);
            f(instance, slot as i32 + 1);
        }
    }
}
pub(super) unsafe fn selected(player: usize) -> Option<usize> {
    let mut m = ReadBatch::new();
    if !m.readable(player, 0x5e8) {
        return None;
    }
    let proxy = read::<usize>(player + 0x5e0);
    if !m.readable(proxy, 0x18) {
        return None;
    }
    let slot = read::<u32>(proxy + 0x10) as usize;
    (slot < 9).then_some(slot)
}
/// Fresh single-slot validation before a durability-driven selection. The native
/// Item getter runs after the read-only container traversal has ended.
pub(super) unsafe fn hotbar_item(player: usize, slot: usize) -> Option<Item> {
    if slot >= 9 {
        return None;
    }
    inventory_item(player, slot)
}
pub(super) unsafe fn inventory_item(player: usize, slot: usize) -> Option<Item> {
    let n = NATIVE.get()?.as_ref().ok()?;
    stack(hotbar_address(player, slot, n)?, n)
}
unsafe fn hotbar_address(player: usize, slot: usize, n: &Native) -> Option<usize> {
    if slot >= 36 {
        return None;
    }
    let address = {
        let mut m = ReadBatch::new();
        if !m.readable(player, 0x5e8) {
            return None;
        }
        let proxy = read::<usize>(player + 0x5e0);
        if !m.readable(proxy, 0xc0) {
            return None;
        }
        let container = read::<usize>(proxy + 0xb8);
        if !container_kind(&mut m, container, n.inventory_get) {
            return None;
        }
        let (begin, end) = m.span(container.checked_add(0x198)?, 0x98)?;
        if end - begin != 36 * 0x98 {
            return None;
        }
        begin + slot * 0x98
    };
    Some(address)
}
pub(super) unsafe fn fresh_hotbar_empty(player: usize, slot: usize) -> bool {
    if slot >= 9 {
        return false;
    }
    let Some(Ok(n)) = NATIVE.get() else {
        return false;
    };
    let Some(address) = hotbar_address(player, slot, n) else {
        return false;
    };
    readable_stack_empty(address)
}
unsafe fn readable_stack_empty(address: usize) -> bool {
    let mut m = ReadBatch::new();
    m.readable(address, 0x98) && read::<u8>(address + 0x22) == 0
}
/// Game-thread hotbar selection with the same proxy method used by vanilla's
/// hotbar handler. The caller already checked gameplay/menu/focus permission.
pub(super) unsafe fn select_tool(player: usize, slot: usize) -> bool {
    // Selecting acts in the game: not on read-only trust.
    let Some(Ok(n)) = NATIVE.get().filter(|_| super::actions_build()) else {
        return false;
    };
    select_in(n, player, slot)
}
unsafe fn select_in(n: &Native, player: usize, slot: usize) -> bool {
    if slot >= 9 {
        return false;
    }
    let proxy = {
        let mut m = ReadBatch::new();
        if !m.readable(player, 0x5e8) {
            return false;
        }
        let proxy = read::<usize>(player + 0x5e0);
        if !m.readable(proxy, 0xe8) {
            return false;
        }
        let container = read::<usize>(proxy + 0xb8);
        if !container_kind(&mut m, container, n.inventory_get) {
            return false;
        }
        if !m
            .span(container + 0x198, 0x98)
            .is_some_and(|(begin, end)| end - begin == 36 * 0x98)
        {
            return false;
        }
        if !(0..9).contains(&read::<u32>(proxy + 0x10)) {
            return false;
        }
        proxy
    };
    let select: unsafe extern "system" fn(usize, u32, u8) -> u8 =
        std::mem::transmute(n.proxy_select);
    select(proxy, slot as u32, 0) != 0 && selected(player) == Some(slot)
}
pub(super) unsafe fn use_button(instance: usize, down: bool) {
    if let Some(Ok(n)) = NATIVE.get() {
        let f: unsafe extern "system" fn(usize, usize, usize) =
            std::mem::transmute(if down { n.down } else { n.up });
        f(0, 0, instance);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_slot_confirmation_rejects_unreadable_and_nonempty_unknown_items() {
        let mut stack = [0u8; 0x98];
        unsafe {
            assert!(readable_stack_empty(stack.as_ptr() as usize));
            stack[0x22] = 1;
            // A nonempty item with an unsupported getter is not an empty slot.
            assert!(!readable_stack_empty(stack.as_ptr() as usize));
            assert!(!readable_stack_empty(0));
        }
    }
    static COMPONENT_ITEM: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "system" fn component_max(item: usize) -> u16 {
        COMPONENT_ITEM.store(item, Ordering::Relaxed);
        384
    }
    unsafe extern "system" fn component_damage(_: usize) -> u16 {
        7
    }
    #[test]
    fn inventory_uses_152_byte_slots_and_count_byte_34() {
        unsafe extern "system" fn select_proxy(proxy: usize, slot: u32, flag: u8) -> u8 {
            ((proxy + 0x10) as *mut u32).write_unaligned(slot);
            ((proxy + 0xb0) as *mut u8).write(flag);
            1
        }
        unsafe extern "system" fn rejected_proxy(_: usize, _: u32, _: u8) -> u8 {
            1
        }
        let mut n = Native {
            damage: 0,
            max: 0x5678,
            component_max: component_max as *const () as usize,
            inventory_get: 0x1234,
            armor_get: 0,
            select: 0,
            proxy_select: rejected_proxy as *const () as usize,
            down: 0,
            up: 0,
        };
        let mut item_vt = [0usize; 37];
        item_vt[36] = n.max;
        let mut item = [0usize; 44];
        item[0] = item_vt.as_ptr() as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                b"paper".as_ptr(),
                (item.as_mut_ptr() as *mut u8).add(0xd8),
                5,
            );
        }
        item[(0xd8 + 16) / 8] = 5;
        item[(0xd8 + 24) / 8] = 15;
        let weak = [item.as_ptr() as usize, 0];
        let mut slots = vec![[0usize; 19]; 36];
        slots[35][1] = weak.as_ptr() as usize;
        slots[35][4] = (64usize << 16) | (1 << 24);
        let mut cvt = [0usize; 8];
        cvt[7] = n.inventory_get;
        let mut container = [0usize; 55];
        container[0] = cvt.as_ptr() as usize;
        container[0x198 / 8] = slots.as_ptr() as usize;
        container[0x1a0 / 8] = container[0x198 / 8] + 36 * 0x98;
        let mut proxy = [0usize; 30];
        proxy[2] = 8;
        proxy[0xb8 / 8] = container.as_ptr() as usize;
        let mut player = [0usize; 190];
        player[0x5e0 / 8] = proxy.as_ptr() as usize;
        player[3] = 0x40012;
        let s = unsafe { collect(player.as_ptr() as usize, &n) }.unwrap();
        assert_eq!(s.selected, 8);
        assert_eq!(s.slots.len(), 36);
        assert!(s.slots[..35].iter().all(Option::is_none));
        let last = s.slots[35].as_ref().unwrap();
        assert_eq!(last.name, "paper");
        assert_eq!(last.count, 64);
        // ComponentItem rods keep the base maximum at zero. The validated
        // getter must run with Item*, not ItemStack*, before reading Damage.
        item_vt[36] = n.component_max;
        n.damage = component_damage as *const () as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                b"fishing_rod".as_ptr(),
                item.as_mut_ptr().cast::<u8>().add(0xd8),
                11,
            );
        }
        item[(0xd8 + 16) / 8] = 11;
        slots[35][4] = (1usize << 16) | (1 << 24);
        let rod = unsafe { stack(slots[35].as_ptr() as usize, &n) }.unwrap();
        assert_eq!(rod.name, "fishing_rod");
        assert_eq!((rod.max_damage, rod.damage, rod.remaining()), (384, 7, 377));
        assert_eq!(
            COMPONENT_ITEM.load(Ordering::Relaxed),
            item.as_ptr() as usize
        );
        unsafe {
            item_vt.as_mut_ptr().add(36).write(0xdeadbeef);
        }
        assert!(unsafe { stack(slots[35].as_ptr() as usize, &n) }.is_none());
        let player_address = player.as_ptr() as usize;
        // A claimed success without a changed selection must be rejected.
        assert!(!unsafe { select_in(&n, player_address, 0) });
        assert_eq!(unsafe { selected(player_address) }, Some(8));
        n.proxy_select = select_proxy as *const () as usize;
        assert!(!unsafe { select_in(&n, player_address, 9) });
        assert_eq!(unsafe { selected(player_address) }, Some(8));
        assert!(unsafe { select_in(&n, player_address, 0) });
        assert_eq!(unsafe { selected(player_address) }, Some(0));
        assert_eq!(unsafe { read::<u8>(proxy.as_ptr() as usize + 0xb0) }, 0);
        unsafe {
            let end = container.as_mut_ptr().add(0x1a0 / 8);
            end.write(end.read() - 1);
        }
        assert!(unsafe { collect(player.as_ptr() as usize, &n) }.is_none());
        assert!(!unsafe { select_in(&n, player_address, 8) });
        unsafe {
            let end = container.as_mut_ptr().add(0x1a0 / 8);
            end.write(end.read() + 1);
            cvt.as_mut_ptr().add(7).write(0x4321);
        }
        assert!(unsafe { collect(player.as_ptr() as usize, &n) }.is_none());
    }
}
