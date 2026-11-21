//! Optional F3 metadata. Owned snapshots are copied on the game callback.
use super::super::{build_name, layout::image, memory::ReadBatch, patch, read};
use crate::signatures::{
    LEVEL_DATA_SIGNATURE, LEVEL_DIFFICULTY_SIGNATURE, LEVEL_ID_FIELD_SIGNATURE,
    LEVEL_NAME_FIELD_SIGNATURE,
};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

static GETTER: OnceLock<Option<usize>> = OnceLock::new();
static DIFFICULTY: OnceLock<Option<usize>> = OnceLock::new();
static ID: OnceLock<Option<usize>> = OnceLock::new();
#[derive(Clone, Debug, Default)]
pub(crate) struct Details {
    pub name: Option<String>,
    pub difficulty: Option<u32>,
    pub world_id: Option<String>,
    pub biome: Option<String>,
    pub sounds: Option<(u32, u32)>,
}
static NAME: Mutex<Option<(u64, Details)>> = Mutex::new(None);
pub(super) fn diagnostic() -> String {
    let guard = NAME.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some((at, value)) => format!(
            "DebugHUD details age={}ms: {value:?}",
            unsafe { GetTickCount64() }.saturating_sub(*at)
        ),
        None => "DebugHUD details: no sample (enable F3 in a world)".into(),
    }
}
pub(super) fn clear() {
    *NAME.lock().unwrap_or_else(|e| e.into_inner()) = None;
}
pub(super) fn prepare() {
    let _ = prepare_identity();
    DIFFICULTY.get_or_init(|| patch::find(LEVEL_DIFFICULTY_SIGNATURE).ok());
    super::super::blocks::environment::prepare();
    super::sound::prepare();
}
pub(crate) fn prepare_identity() -> Result<(), String> {
    GETTER.get_or_init(|| {
        build_name()?;
        patch::find(LEVEL_NAME_FIELD_SIGNATURE).ok()?;
        patch::find(LEVEL_DATA_SIGNATURE).ok()
    });
    ID.get_or_init(|| patch::find(LEVEL_ID_FIELD_SIGNATURE).ok());
    if GETTER.get().copied().flatten().is_some() && ID.get().copied().flatten().is_some() {
        Ok(())
    } else {
        Err("ワールドIDの取得位置を確認できません".into())
    }
}
pub(crate) unsafe fn identity(player: usize) -> Option<super::super::terrain_session::Identity> {
    let getter = GETTER.get().copied().flatten()?;
    let mut memory = ReadBatch::new();
    if !memory.readable(player, 0x208) {
        return None;
    }
    let level = read::<usize>(player + 0x200);
    let details = read_details(&mut memory, level, getter)?;
    let id = details.world_id?;
    let name = details.name.unwrap_or_else(|| id.clone());
    Some(super::super::terrain_session::Identity {
        id: id.into(),
        name: name.into(),
        level,
    })
}
pub(crate) fn details() -> Option<Details> {
    if super::SUBSCRIBERS.load(std::sync::atomic::Ordering::Acquire) == 0 {
        return None;
    }
    let guard = NAME.lock().ok()?;
    let (at, name) = guard.as_ref()?;
    (unsafe { GetTickCount64() }.saturating_sub(*at) < 1000).then(|| name.clone())
}
unsafe fn read_details(m: &mut ReadBatch, level: usize, getter: usize) -> Option<Details> {
    if !m.readable(level, 0x98) {
        return None;
    }
    let vt = read::<usize>(level);
    let (base, end) = image();
    if vt < base || vt.checked_add(0x570)? > end || read::<usize>(vt + 0x568) != getter {
        return None;
    }
    let flag = read::<usize>(level + 0x80);
    if !m.readable(flag, 1) || read::<u8>(flag) == 0 {
        return None;
    }
    let data = read::<usize>(level + 0x90);
    let name = m
        .string(data.checked_add(0x2b0)?, 256)
        .map(|s| crate::modules::chat_log::clean(&s))
        .filter(|s| !s.is_empty());
    let difficulty = (|| {
        let expected = DIFFICULTY.get().copied().flatten()?;
        if read::<usize>(vt + 0x120) != expected || !m.readable(data.checked_add(0x3c0)?, 4) {
            return None;
        }
        let value = read::<u32>(data + 0x3c0);
        (value <= 3).then_some(value)
    })();
    let world_id = ID
        .get()
        .copied()
        .flatten()
        .and_then(|_| m.string(level + 0x260, 128))
        .filter(|s| {
            !s.is_empty()
                && s.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'_' | b'=' | b'/')
                })
        });
    Some(Details {
        name,
        difficulty,
        world_id,
        ..Default::default()
    })
}
pub(super) unsafe fn sample(player: usize, client: usize, position: [i32; 3], now: u64) {
    if super::SUBSCRIBERS.load(std::sync::atomic::Ordering::Acquire) == 0 {
        return;
    }
    let value = (|| {
        let getter = GETTER.get().copied().flatten()?;
        let mut m = ReadBatch::new();
        if !m.readable(player, 0x208) {
            return None;
        }
        let mut details = read_details(&mut m, read(player + 0x200), getter)?;
        details.biome = super::super::blocks::environment::biome(player, position);
        details.sounds = super::sound::sample(client);
        Some(details)
    })();
    if let Ok(mut guard) = NAME.try_lock() {
        *guard = value.map(|name| (now, name));
    }
}
