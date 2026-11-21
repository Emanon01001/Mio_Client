//! Read the current screen on the native input thread; publish only scalar state.
use super::{actions_build, layout::image, memory::ReadBatch, patch, read, readable};
use crate::{
    gui, input,
    signatures::{SCREEN_MANAGER_SIGNATURE, SCREEN_NAME_SIGNATURE, TOP_SCREEN_SIGNATURE},
};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    OnceLock,
};

pub const BITS: [i32; 6] = [13, 14, 15, 16, 7, 0];
static KEYS: [AtomicU32; 6] = [
    AtomicU32::new(0x57),
    AtomicU32::new(0x53),
    AtomicU32::new(0x41),
    AtomicU32::new(0x44),
    AtomicU32::new(0x20),
    AtomicU32::new(0xA0),
];
static ON: AtomicBool = AtomicBool::new(false);
static UNTIL: AtomicU64 = AtomicU64::new(0);
static AT: AtomicU64 = AtomicU64::new(0);
static INSTANCE: AtomicUsize = AtomicUsize::new(0);
static KIND: AtomicU32 = AtomicU32::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicU32 = AtomicU32::new(0);
static PHYSICAL: AtomicU32 = AtomicU32::new(0);
static HANDOFF: AtomicBool = AtomicBool::new(false);
static JUMP: AtomicBool = AtomicBool::new(true);
static SNEAK: AtomicBool = AtomicBool::new(false);
static AUTOMATIC: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
enum Screen {
    Unknown,
    Hud,
    Inventory,
    Chat,
}
fn classify(name: &[u8]) -> Screen {
    match name {
        b"hud_screen" => Screen::Hud,
        b"inventory_screen" | b"creative_inventory_screen" => Screen::Inventory,
        b"chat_screen" => Screen::Chat,
        _ => Screen::Unknown,
    }
}
struct Reader {
    manager_offset: usize,
    manager_getter: usize,
    top_getter: usize,
    name_body: usize,
}
static READER: OnceLock<Result<Reader, String>> = OnceLock::new();
pub fn available() -> Result<(), String> {
    READER
        .get_or_init(|| {
            if !actions_build() {
                return Err(format!(
                    "inventorymove requires {}",
                    crate::modules::game::SUPPORTED
                ));
            }
            let manager_getter = patch::find(SCREEN_MANAGER_SIGNATURE)?;
            let manager_offset = unsafe { read::<u32>(manager_getter + 10) as usize };
            if manager_offset != 0x5E8 {
                return Err("unexpected screen manager layout".into());
            }
            Ok(Reader {
                manager_offset,
                manager_getter,
                top_getter: patch::find(TOP_SCREEN_SIGNATURE)?,
                name_body: patch::find(SCREEN_NAME_SIGNATURE)?,
            })
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
impl Reader {
    unsafe fn screen(&self, instance: usize) -> Screen {
        self.read_screen(instance).unwrap_or(Screen::Unknown)
    }
    unsafe fn read_screen(&self, instance: usize) -> Option<Screen> {
        let mut m = ReadBatch::new();
        if !m.readable(instance, self.manager_offset + 8) {
            return None;
        }
        let vt: usize = read(instance);
        if !m.readable(vt, 0x740) || read::<usize>(vt + 0x738) != self.manager_getter {
            return None;
        }
        let manager: usize = read(instance + self.manager_offset);
        if !m.readable(manager, 0x30) {
            return None;
        }
        let vt: usize = read(manager);
        if !m.readable(vt, 0x168) || read::<usize>(vt + 0x160) != self.top_getter {
            return None;
        }
        // Each stack element is 32 bytes; the native getter loads end-24.
        let (begin, end) = m.span(manager + 0x18, 32)?;
        if end <= begin || end - begin > 64 * 32 {
            return None;
        }
        let screen: usize = read(end - 24);
        if !m.readable(screen, 0x48) {
            return None;
        }
        let vt: usize = read(screen);
        if !m.readable(vt, 0x1C0) {
            return None;
        }
        let getter: usize = read(vt + 0x1B8);
        let (image_begin, image_end) = image();
        if getter < image_begin || getter.checked_add(9)? > image_end {
            return None;
        }
        if read::<[u8; 5]>(getter) != [0x48, 0x8B, 0x49, 0x40, 0xE9] {
            return None;
        }
        if (getter + 9).checked_add_signed(read::<i32>(getter + 5) as isize)? != self.name_body {
            return None;
        }
        let facade: usize = read(screen + 0x40);
        if !m.readable(facade, 0x58) {
            return None;
        }
        let holder: usize = read(facade + 0x50);
        if !m.readable(holder, 0x18) {
            return None;
        }
        let model: usize = read(holder + 8);
        if !m.readable(model, 0x40) {
            return None;
        }
        screen_name(&mut m, model + 0x20)
    }
}
unsafe fn screen_name(m: &mut ReadBatch, address: usize) -> Option<Screen> {
    if !m.readable(address, 32) {
        return None;
    }
    let length: usize = read(address + 16);
    let capacity: usize = read(address + 24);
    if length == 0 || length > 64 || length > capacity {
        return None;
    }
    let data = if capacity < 16 {
        address
    } else {
        read(address)
    };
    if !m.readable(data, length) {
        return None;
    }
    Some(classify(std::slice::from_raw_parts(
        data as *const u8,
        length,
    )))
}

pub fn request(on: bool, now: u64) {
    UNTIL.store(now.saturating_add(100), Ordering::Release);
    ON.store(on, Ordering::Release);
}
pub fn set_key(index: usize, key: u16) {
    KEYS[index].store(key as u32, Ordering::Release);
}
pub fn configure(jump: bool, sneak: bool, automatic: bool) {
    JUMP.store(jump, Ordering::Release);
    SNEAK.store(sneak, Ordering::Release);
    AUTOMATIC.store(automatic, Ordering::Release);
}
fn fresh(now: u64) -> bool {
    now.saturating_sub(AT.load(Ordering::Acquire)) <= 100
}
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}
pub fn native_move_allowed(instance: usize, now: u64) -> bool {
    instance != 0
        && instance == INSTANCE.load(Ordering::Acquire)
        && ON.load(Ordering::Acquire)
        && now <= UNTIL.load(Ordering::Acquire)
        && fresh(now)
        && active()
        && !gui::captures_input()
        && !crate::api::blocked()
        && input::is_foreground()
}
pub fn requested(bit: i32) -> bool {
    REQUESTED.load(Ordering::Acquire) & (1 << bit) != 0
}
pub fn managed(bit: i32) -> bool {
    active()
        && match bit {
            13..=16 => true,
            7 => JUMP.load(Ordering::Acquire),
            0 => SNEAK.load(Ordering::Acquire),
            _ => false,
        }
}
pub fn handoff(bit: i32) -> bool {
    HANDOFF.load(Ordering::Acquire) && PHYSICAL.load(Ordering::Acquire) & (1 << bit) != 0
}
pub fn screen_open() -> bool {
    READER.get().is_some_and(Result::is_ok) && KIND.load(Ordering::Acquire) != Screen::Hud as u32
}
/// Fresh own-inventory classification, independent of InventoryMove's toggle.
/// Chat, containers, pause screens and unknown layouts deliberately fail closed.
pub fn own_inventory_open(now: u64) -> bool {
    INSTANCE.load(Ordering::Acquire) != 0
        && fresh_inventory(
            KIND.load(Ordering::Acquire),
            AT.load(Ordering::Acquire),
            now,
        )
}
/// ChatRepeat may submit normal chat while this exact screen is on top.
/// This grants no movement, attack or inventory-use permission.
pub(crate) fn chat_open(now: u64) -> bool {
    INSTANCE.load(Ordering::Acquire) != 0
        && fresh_chat(
            KIND.load(Ordering::Acquire),
            AT.load(Ordering::Acquire),
            now,
        )
}
fn fresh_chat(kind: u32, at: u64, now: u64) -> bool {
    kind == Screen::Chat as u32 && at != 0 && now.saturating_sub(at) <= 100
}
pub(crate) fn chat_open_for(instance: usize, now: u64) -> bool {
    instance != 0 && instance == INSTANCE.load(Ordering::Acquire) && chat_open(now)
}
fn fresh_inventory(kind: u32, at: u64, now: u64) -> bool {
    kind == Screen::Inventory as u32 && at <= now && now - at <= 100
}
pub(super) fn own_inventory_for(instance: usize, now: u64) -> bool {
    instance != 0 && instance == INSTANCE.load(Ordering::Acquire) && own_inventory_open(now)
}
pub fn helpers_allowed() -> bool {
    !screen_open() || (active() && AUTOMATIC.load(Ordering::Acquire))
}
pub fn movement_active(normal: bool, focused: bool, now: u64) -> bool {
    (normal && !screen_open())
        || (ON.load(Ordering::Acquire)
            && now <= UNTIL.load(Ordering::Acquire)
            && focused
            && !gui::captures_input()
            && !crate::api::blocked()
            && fresh(now)
            && active()
            && AUTOMATIC.load(Ordering::Acquire))
}
/// Called before any movement reconciliation, including feature-OFF cleanup.
pub(super) unsafe fn sample(instance: usize, player: usize, now: u64) {
    let was_active = active();
    let on = ON.load(Ordering::Acquire) && now <= UNTIL.load(Ordering::Acquire);
    let kind = if readable(player, 0x1C) {
        READER
            .get()
            .and_then(|r| r.as_ref().ok())
            .map_or(Screen::Unknown, |r| r.screen(instance))
    } else {
        Screen::Unknown
    };
    INSTANCE.store(
        if readable(player, 0x1C) { instance } else { 0 },
        Ordering::Release,
    );
    let permitted = on
        && kind == Screen::Inventory
        && !gui::captures_input()
        && input::is_foreground()
        && !crate::api::blocked();
    let mut physical = 0;
    if permitted || (was_active && kind == Screen::Hud) {
        for (i, bit) in BITS.into_iter().enumerate() {
            if input::key_down(KEYS[i].load(Ordering::Acquire) as u16) {
                physical |= 1 << bit;
            }
        }
    }
    let mut wanted = if permitted { physical } else { 0 };
    if !JUMP.load(Ordering::Acquire) {
        wanted &= !(1 << 7);
    }
    if !SNEAK.load(Ordering::Acquire) {
        wanted &= !1;
    }
    PHYSICAL.store(physical, Ordering::Release);
    HANDOFF.store(was_active && kind == Screen::Hud, Ordering::Release);
    REQUESTED.store(wanted, Ordering::Release);
    KIND.store(kind as u32, Ordering::Release);
    ACTIVE.store(permitted, Ordering::Release);
    AT.store(now, Ordering::Release);
}
pub fn status() -> String {
    crate::localization::text(if active() {
        "movement in inventory"
    } else {
        "waiting for inventory"
    })
    .into()
}
pub fn diagnostic() -> String {
    format!(
        "inventory on={} screen={} active={} requested=0x{:X} age={}ms",
        ON.load(Ordering::Acquire),
        KIND.load(Ordering::Acquire),
        active(),
        REQUESTED.load(Ordering::Acquire),
        unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
            .saturating_sub(AT.load(Ordering::Acquire))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chat_repeat_requires_a_fresh_exact_chat_screen_without_inventory_permissions() {
        assert!(fresh_chat(classify(b"chat_screen") as u32, 100, 150));
        assert!(fresh_chat(classify(b"chat_screen") as u32, 151, 150));
        assert!(!fresh_chat(Screen::Chat as u32, 100, 201));
        assert!(!fresh_chat(Screen::Chat as u32, 0, 50));
        assert!(!fresh_inventory(Screen::Chat as u32, 100, 150));
        for name in [
            b"hud_screen".as_slice(),
            b"pause_screen",
            b"inventory_screen",
            b"chest_screen",
            b"",
        ] {
            assert!(!fresh_chat(classify(name) as u32, 100, 150));
        }
    }
    #[test]
    fn only_fresh_own_inventory_allows_background_fishing() {
        for name in [b"inventory_screen".as_slice(), b"creative_inventory_screen"] {
            assert!(fresh_inventory(classify(name) as u32, 100, 150));
        }
        for name in [
            b"hud_screen".as_slice(),
            b"chat_screen",
            b"pause_screen",
            b"chest_screen",
            b"",
        ] {
            assert!(!fresh_inventory(classify(name) as u32, 100, 150));
        }
        assert!(fresh_inventory(Screen::Inventory as u32, 100, 200));
        assert!(!fresh_inventory(Screen::Inventory as u32, 100, 201));
        assert!(!fresh_inventory(Screen::Inventory as u32, 200, 100));
    }
    #[test]
    fn screen_name_handles_inline_heap_and_invalid_metadata() {
        let mut text = [0usize; 4];
        text[2] = 10;
        text[3] = 15;
        unsafe {
            std::ptr::copy_nonoverlapping(
                b"hud_screen".as_ptr(),
                text.as_mut_ptr().cast::<u8>(),
                10,
            );
            let address = text.as_mut_ptr() as usize;
            assert_eq!(
                screen_name(&mut ReadBatch::new(), address),
                Some(Screen::Hud)
            );
            (address as *mut usize)
                .add(0)
                .write(b"inventory_screen".as_ptr() as usize);
            (address as *mut usize).add(2).write(16);
            (address as *mut usize).add(3).write(31);
            assert_eq!(
                screen_name(&mut ReadBatch::new(), address),
                Some(Screen::Inventory)
            );
            (address as *mut usize).add(2).write(65);
            assert_eq!(screen_name(&mut ReadBatch::new(), address), None);
            (address as *mut usize).add(2).write(32);
            assert_eq!(screen_name(&mut ReadBatch::new(), address), None);
            (address as *mut usize).add(2).write(16);
            (address as *mut usize).write(0);
            assert_eq!(screen_name(&mut ReadBatch::new(), address), None);
        }
    }
    #[test]
    fn only_inventory_screens_are_allowed_not_chat_or_other_overlays() {
        assert_eq!(classify(b"inventory_screen"), Screen::Inventory);
        assert_eq!(classify(b"creative_inventory_screen"), Screen::Inventory);
        assert_eq!(classify(b"hud_screen"), Screen::Hud);
        assert_eq!(classify(b"chat_screen"), Screen::Chat);
        for name in [
            b"pause_screen".as_slice(),
            b"chest_screen",
            b"inventory_search_screen",
            b"",
        ] {
            assert_eq!(classify(name), Screen::Unknown);
        }
    }
}
