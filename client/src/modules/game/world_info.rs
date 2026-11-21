//! Read-only world clock registry and interpolated dimension weather.
use super::{
    known_build,
    layout::image,
    memory::ReadBatch,
    patch, read,
    world::{self, Realm},
};
use crate::signatures::{CLOCK_HANDLE_SIGNATURE, LEVEL_TIME_SIGNATURE, WEATHER_FIELDS_SIGNATURE};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Mutex, OnceLock,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
mod metadata;
mod sound;
pub(crate) use metadata::details;
pub(crate) use metadata::{identity, prepare_identity};
#[derive(Clone, Copy, Debug)]
pub struct Info {
    pub ticks: i32,
    pub rain: f32,
    pub thunder: f32,
    pub realm: Realm,
}
struct Native {
    time: usize,
    clock_handle: usize,
    key: usize,
}
static NATIVE: OnceLock<Result<Native, String>> = OnceLock::new();
static ON: AtomicBool = AtomicBool::new(false);
static SUBSCRIBERS: AtomicUsize = AtomicUsize::new(0);
static CLOCK_SUBSCRIBERS: AtomicUsize = AtomicUsize::new(0);
fn wanted() -> bool {
    ON.load(Ordering::Acquire)
        || SUBSCRIBERS.load(Ordering::Acquire) != 0
        || CLOCK_SUBSCRIBERS.load(Ordering::Acquire) != 0
}
pub(crate) struct Subscription {
    details: bool,
}
pub(crate) fn subscribe() -> Result<Subscription, String> {
    available()?;
    metadata::prepare();
    SUBSCRIBERS.fetch_add(1, Ordering::AcqRel);
    LAST.store(0, Ordering::Relaxed);
    Ok(Subscription { details: true })
}
/// Clock/weather consumers must not enable the F3 biome/sound/name samplers.
pub(crate) fn subscribe_clock() -> Result<Subscription, String> {
    available()?;
    CLOCK_SUBSCRIBERS.fetch_add(1, Ordering::AcqRel);
    LAST.store(0, Ordering::Relaxed);
    Ok(Subscription { details: false })
}
impl Drop for Subscription {
    fn drop(&mut self) {
        let users = if self.details {
            &SUBSCRIBERS
        } else {
            &CLOCK_SUBSCRIBERS
        };
        let _ = users.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
        if self.details && SUBSCRIBERS.load(Ordering::Acquire) == 0 {
            metadata::clear();
        }
        if !wanted() {
            *INFO.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
}
static LAST: AtomicU64 = AtomicU64::new(0);
struct WorldReading {
    at: u64,
    owner: (usize, u32),
    info: Info,
}
static INFO: Mutex<Option<WorldReading>> = Mutex::new(None);
pub fn available() -> Result<(), String> {
    native().map(|_| ())
}
fn native() -> Result<&'static Native, String> {
    NATIVE
        .get_or_init(|| {
            if !known_build() {
                return Err("WorldInfoには解析済みビルドが必要です".into());
            }
            let time = patch::find(LEVEL_TIME_SIGNATURE)?;
            let clock_handle = patch::find(CLOCK_HANDLE_SIGNATURE)?;
            patch::find(WEATHER_FIELDS_SIGNATURE)?;
            unsafe {
                let at = time + 0x5c;
                if std::slice::from_raw_parts(at as *const u8, 3) != [0x48, 0x8d, 0x15] {
                    return Err("WorldClock key instruction mismatch".into());
                }
                let key = (at + 7)
                    .checked_add_signed(read::<i32>(at + 3) as isize)
                    .ok_or("WorldClock key address overflow")?;
                let (base, end) = image();
                if key < base || key + 8 > end {
                    return Err("WorldClock key outside image".into());
                }
                Ok(Native {
                    time,
                    clock_handle,
                    key,
                })
            }
        })
        .as_ref()
        .map_err(Clone::clone)
}
pub fn set_active(on: bool) {
    ON.store(on, Ordering::Release);
    LAST.store(0, Ordering::Relaxed);
    if !wanted() {
        *INFO.lock().unwrap_or_else(|e| e.into_inner()) = None;
        metadata::clear();
    }
}
pub fn info() -> Option<Info> {
    if !wanted() {
        return None;
    }
    let guard = INFO.lock().unwrap_or_else(|e| e.into_inner());
    let v = guard.as_ref()?;
    (unsafe { GetTickCount64() }.saturating_sub(v.at) < 2000).then_some(v.info)
}
pub(crate) fn reading_for_local(owner: (usize, u32)) -> Option<(u64, Info)> {
    if !wanted() {
        return None;
    }
    let guard = INFO.lock().unwrap_or_else(|e| e.into_inner());
    let value = guard.as_ref()?;
    (value.owner == owner && unsafe { GetTickCount64() }.saturating_sub(value.at) <= 1000)
        .then_some((value.at, value.info))
}
unsafe fn valid_handle(m: &mut ReadBatch, holder: usize) -> bool {
    if !m.readable(holder, 0x18) {
        return false;
    }
    let flag = read::<usize>(holder + 8);
    m.readable(flag, 1) && read::<u8>(flag) == 1
}
unsafe fn read_info(player: usize, n: &Native) -> Option<Info> {
    let mut m = ReadBatch::new();
    if !m.readable(player, 0x208) {
        return None;
    }
    let level = read::<usize>(player + 0x200);
    if !m.readable(level, 8) {
        return None;
    }
    let vt = read::<usize>(level);
    let (base, end) = image();
    if vt < base || vt + 0xc98 > end || read::<usize>(vt + 0x420) != n.time {
        return None;
    }
    let accessor = read::<usize>(vt + 0xc90);
    if accessor < base
        || accessor + 8 > end
        || std::slice::from_raw_parts(accessor as *const u8, 3) != [0x48, 0x8b, 0x81]
        || read::<u8>(accessor + 7) != 0xc3
    {
        return None;
    }
    let offset = read::<u32>(accessor + 3) as usize;
    if !(0x100..=0x2000).contains(&offset) || !m.readable(level + offset, 8) {
        return None;
    }
    let manager = read::<usize>(level + offset);
    if !m.readable(manager, 0x60) {
        return None;
    }
    let mv = read::<usize>(manager);
    if mv < base
        || mv + 0x20 > end
        || read::<usize>(mv + 0x18) != n.clock_handle
        || !valid_handle(&mut m, read(manager + 0x50))
    {
        return None;
    }
    let root = read::<usize>(manager + 0x58);
    let (b, e) = m.span(root + 8, 8)?;
    let (v, ve) = m.span(root + 0x20, 16)?;
    let count = (e - b) / 8;
    if count == 0 || count > 64 || ve - v != count * 16 {
        return None;
    }
    let key = read::<u64>(n.key);
    let mut lo = 0;
    let mut hi = count;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if read::<u64>(b + mid * 8) < key {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == count || read::<u64>(b + lo * 8) != key {
        return None;
    }
    let entry = v + lo * 16;
    if !valid_handle(&mut m, read(entry)) {
        return None;
    }
    let clock = read::<usize>(entry + 8);
    if !m.readable(clock, 0x34) || read::<u64>(clock) != key {
        return None;
    }
    let ticks = read::<i32>(clock + 0x30);
    let dim = read::<usize>(player + 0x1f0);
    if !m.readable(dim, 0x1f0) {
        return None;
    }
    let weather = read::<usize>(dim + 0x1e8);
    if !m.readable(weather, 0x4c) {
        return None;
    }
    let rain = read::<f32>(weather + 0x3c);
    let thunder = read::<f32>(weather + 0x48);
    if !rain.is_finite()
        || !thunder.is_finite()
        || !(0. ..=1.).contains(&rain)
        || !(0. ..=1.).contains(&thunder)
    {
        return None;
    }
    Some(Info {
        ticks,
        rain,
        thunder,
        realm: world::realm().unwrap_or(Realm::Other),
    })
}
pub(super) unsafe fn sample(player: usize, client: usize, position: [i32; 3], now: u64) {
    if !wanted() || now.saturating_sub(LAST.load(Ordering::Relaxed)) < 200 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    metadata::sample(player, client, position, now);
    let value = native().ok().and_then(|n| read_info(player, n));
    if let Ok(mut guard) = INFO.try_lock() {
        *guard = value.map(|info| WorldReading {
            at: now,
            owner: (read(player + 0x10), read(player + 0x18)),
            info,
        });
    }
}
pub fn clock(ticks: i32) -> (i64, u32, u32) {
    let ticks = ticks as i64;
    let day = ticks.div_euclid(24000) + 1;
    let minutes = (ticks.rem_euclid(24000) * 1440 / 24000 + 360) % 1440;
    (day, (minutes / 60) as u32, (minutes % 60) as u32)
}
pub fn diagnostic() -> String {
    let world = match info() {
        Some(v) => format!(
            "WorldInfo ticks={} time={:?} rain={:.3} thunder={:.3} realm={:?}",
            v.ticks,
            clock(v.ticks),
            v.rain,
            v.thunder,
            v.realm
        ),
        None => "WorldInfo: no current reading".into(),
    };
    format!(
        "{world}\n{}\n{}",
        metadata::diagnostic(),
        super::blocks::environment::diagnostic()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn releasing_debug_clock_keeps_manual_world_info_enabled() {
        // Model a successful subscription without needing a game executable.
        set_active(true);
        SUBSCRIBERS.fetch_add(1, Ordering::AcqRel);
        let first = Subscription { details: true };
        SUBSCRIBERS.fetch_add(1, Ordering::AcqRel);
        let second = Subscription { details: true };
        drop(first);
        assert!(wanted());
        drop(second);
        assert!(wanted());
        set_active(false);
        assert!(!wanted());

        SUBSCRIBERS.fetch_add(1, Ordering::AcqRel);
        let debug = Subscription { details: true };
        set_active(false);
        assert!(wanted());
        drop(debug);
        assert!(!wanted());

        // An SDK clock observer must neither request F3 details nor be stopped
        // when a DebugHUD observer is dropped.
        CLOCK_SUBSCRIBERS.fetch_add(1, Ordering::AcqRel);
        let sdk = Subscription { details: false };
        assert!(wanted());
        assert_eq!(SUBSCRIBERS.load(Ordering::Acquire), 0);
        SUBSCRIBERS.fetch_add(1, Ordering::AcqRel);
        drop(Subscription { details: true });
        assert!(wanted());
        assert_eq!(CLOCK_SUBSCRIBERS.load(Ordering::Acquire), 1);
        drop(sdk);
        assert!(!wanted());
    }
    #[test]
    fn clock_handles_minecraft_dawn_midnight_day_wrap_and_negative_time() {
        assert_eq!(clock(0), (1, 6, 0));
        assert_eq!(clock(6000), (1, 12, 0));
        assert_eq!(clock(12000), (1, 18, 0));
        assert_eq!(clock(18000), (1, 0, 0));
        assert_eq!(clock(24000), (2, 6, 0));
        assert_eq!(clock(-1), (0, 5, 59));
        for t in [i32::MIN, i32::MAX, -24000, -18000, 23999] {
            let (_, h, m) = clock(t);
            assert!(h < 24 && m < 60);
        }
    }
}
