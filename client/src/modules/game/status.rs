//! Local effects and air supply: read once on the game thread, share owned values.
use super::{
    acquire,
    actors::type_hash,
    entities::{actor_jump_state, component_pools, PoolView},
    known_build,
    memory::ReadBatch,
    metadata, patch, read, release,
};
use crate::signatures::EFFECT_LAYOUT_SIGNATURE;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, OnceLock,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

const EFFECTS: u32 = type_hash("MobEffectsComponent");
const STRIDE: usize = 0xa8;
const MAX_EFFECTS: usize = 64;
const INTERVAL: u64 = 100;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    pub id: u32,
    /// Zero-based amplifier: 0 = level I.
    pub amplifier: u32,
    /// Remaining game ticks. None is the game's -1 infinite sentinel.
    pub ticks: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Air {
    pub current: i16,
    pub max: i16,
    pub underwater: bool,
}
#[derive(Clone)]
pub struct Reading {
    pub effects: Option<Arc<[Effect]>>,
    pub air: Option<Air>,
}
struct Snapshot {
    at: u64,
    owner: (usize, u32),
    reading: Reading,
}
static USERS: AtomicUsize = AtomicUsize::new(0);
static LAST: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static AVAILABLE: OnceLock<Result<(), String>> = OnceLock::new();

pub fn available() -> Result<(), String> {
    AVAILABLE
        .get_or_init(|| {
            if !known_build() {
                return Err(format!(
                    "status HUD requires {}",
                    crate::modules::game::SUPPORTED
                ));
            }
            let at = patch::find(EFFECT_LAYOUT_SIGNATURE)?;
            if unsafe { read::<u32>(at + 3) } != STRIDE as u32
                || unsafe { read::<u8>(at + 17) } != 38
                || unsafe { read::<u8>(at + 36) } != 0x20
            {
                return Err("unknown effect data layout".into());
            }
            metadata::available()
        })
        .clone()
}

unsafe fn effects(m: &mut ReadBatch, component: usize) -> Option<Arc<[Effect]>> {
    if !m.readable(component, 24) {
        return None;
    }
    let (begin, end, cap): (usize, usize, usize) =
        (read(component), read(component + 8), read(component + 16));
    if begin == 0 && end == 0 && cap == 0 {
        return Some(Arc::from([]));
    }
    let bytes = end.checked_sub(begin)?;
    if begin == 0 || end > cap || bytes % STRIDE != 0 || bytes / STRIDE > MAX_EFFECTS {
        return None;
    }
    if bytes > 0 && !m.readable(begin, bytes) {
        return None;
    }
    let mut out = Vec::new();
    for index in 0..bytes / STRIDE {
        let at = begin + index * STRIDE;
        let id: u32 = read(at);
        // Unused vector slots retain arbitrary old bytes after the zero id.
        if id == 0 {
            continue;
        }
        let duration: i32 = read(at + 4);
        let amplifier: i32 = read(at + 0x20);
        if id > 38 || id as usize != index || duration < -1 || !(0..=255).contains(&amplifier) {
            return None;
        }
        if duration == 0 {
            continue;
        }
        out.push(Effect {
            id,
            amplifier: amplifier as u32,
            ticks: if duration == -1 {
                None
            } else {
                Some(duration as u32)
            },
        });
    }
    Some(Arc::from(out))
}

pub(super) unsafe fn sample(player: usize, now: u64) {
    if USERS.load(Ordering::Relaxed) == 0
        || now.saturating_sub(LAST.load(Ordering::Relaxed)) < INTERVAL
    {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let mut m = ReadBatch::new();
    let reading = (|| {
        if !m.readable(player, 0x1c) {
            return None;
        }
        let registry: usize = read(player + 0x10);
        let entity: u32 = read(player + 0x18);
        let [effect_pool, meta_pool] = component_pools(&mut m, registry, [EFFECTS, metadata::HASH]);
        let effect_data = effect_pool
            .and_then(|p| PoolView::new(&mut m, p, 24))
            .and_then(|v| v.component(&mut m, entity))
            .and_then(|c| effects(&mut m, c));
        let air = meta_pool
            .and_then(|p| metadata::View::new(&mut m, p))
            .and_then(|v| v.air(&mut m, entity))
            .map(|(current, max)| Air {
                current,
                max,
                underwater: actor_jump_state(player)[1],
            });
        Some((
            (registry, entity),
            Reading {
                effects: effect_data,
                air,
            },
        ))
    })();
    if let Ok(mut slot) = SNAPSHOT.try_lock() {
        *slot = reading.map(|(owner, reading)| Snapshot {
            at: now,
            owner,
            reading,
        });
    }
}
pub fn use_status(on: bool) -> Result<(), String> {
    if on {
        available()?;
        acquire()?;
        USERS.fetch_add(1, Ordering::Relaxed);
    } else {
        release()?;
        let previous = USERS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .unwrap_or(0);
        if previous <= 1 {
            *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            LAST.store(0, Ordering::Relaxed);
        }
    }
    Ok(())
}
pub fn reading() -> Option<Reading> {
    if USERS.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let slot = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = slot.as_ref()?;
    (unsafe { GetTickCount64() }.saturating_sub(snapshot.at) <= 1000)
        .then(|| snapshot.reading.clone())
}
pub(crate) fn reading_for_local(owner: (usize, u32)) -> Option<(u64, Reading)> {
    if USERS.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let guard = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    let value = guard.as_ref()?;
    (value.owner == owner && unsafe { GetTickCount64() }.saturating_sub(value.at) <= 1000)
        .then(|| (value.at, value.reading.clone()))
}
pub fn diagnostic() -> String {
    let Some(r) = reading() else {
        return "status: no snapshot (enable effects or air in a world)".into();
    };
    let air = r.air.map_or_else(
        || "unavailable".into(),
        |a| format!("{}/{} ticks, underwater {}", a.current, a.max, a.underwater),
    );
    let mut text = format!("status: air {air}; sampling 10 Hz");
    if let Some(effects) = r.effects {
        text.push_str(&format!("\neffects: {} active", effects.len()));
        for e in effects.iter() {
            text.push_str(&format!(
                "\n  ID {} level {} ticks {:?}",
                e.id,
                e.amplifier + 1,
                e.ticks
            ));
        }
    } else {
        text.push_str("\neffects: unavailable");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_effect_slots_expiration_infinity_and_layout_validation() {
        let mut data = vec![0u64; 9 * STRIDE / 8];
        let at = data.as_mut_ptr() as usize + 8 * STRIDE;
        unsafe {
            (at as *mut u32).write_unaligned(8);
            ((at + 4) as *mut i32).write_unaligned(7959);
        }
        let mut header = [
            data.as_ptr() as usize,
            data.as_ptr() as usize + 9 * STRIDE,
            data.as_ptr() as usize + 9 * STRIDE,
        ];
        let mut m = ReadBatch::new();
        unsafe {
            assert_eq!(
                effects(&mut m, header.as_ptr() as usize).unwrap().as_ref(),
                &[Effect {
                    id: 8,
                    amplifier: 0,
                    ticks: Some(7959)
                }]
            );
            ((at + 4) as *mut i32).write_unaligned(-1);
            assert_eq!(
                effects(&mut m, header.as_ptr() as usize).unwrap()[0].ticks,
                None
            );
            ((at + 4) as *mut i32).write_unaligned(0);
            assert!(effects(&mut m, header.as_ptr() as usize)
                .unwrap()
                .is_empty());
            ((at + 4) as *mut i32).write_unaligned(-2);
            assert!(effects(&mut m, header.as_ptr() as usize).is_none());
            header[1] -= 8;
            assert!(effects(&mut m, header.as_ptr() as usize).is_none());
            assert!(effects(&mut m, 0).is_none());
        }
    }
}
