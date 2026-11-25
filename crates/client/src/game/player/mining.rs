//! Read-only GameMode mining state. Only sampled on the existing game HUD hook.
use crate::game::{blocks, known_build, memory::ReadBatch, patch, read};
use crate::signatures::{
    MINING_CONTINUE_SIGNATURE, MINING_PROGRESS_SIGNATURE, MINING_STOP_SIGNATURE,
    MINING_TARGET_SIGNATURE,
};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Mutex, OnceLock,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
const MODE: usize = 0xAC8; // Same verified LocalPlayer->GameMode as combat.
struct Native {
    stop: usize,
    continuation: usize,
    progress: usize,
    position: usize,
    active: usize,
}
static NATIVE: OnceLock<Result<Native, String>> = OnceLock::new();
static USERS: AtomicUsize = AtomicUsize::new(0);
static LAST: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT: Mutex<Option<(u64, Reading)>> = Mutex::new(None);
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub name: String,
    pub position: [i32; 3],
    pub progress: f32,
}
pub fn available() -> Result<(), String> {
    NATIVE
        .get_or_init(|| {
            if !known_build() {
                return Err("採掘進捗の取得には対応するゲームビルドが必要です".into());
            }
            blocks::available()?;
            let stop = patch::find(MINING_STOP_SIGNATURE)?;
            let add = patch::find(MINING_PROGRESS_SIGNATURE)?;
            let target = patch::find(MINING_TARGET_SIGNATURE)?;
            let continuation = patch::find(MINING_CONTINUE_SIGNATURE)?;
            let progress = unsafe { read::<u8>(stop + 19) } as usize;
            let position = unsafe { read::<u8>(target + 4) } as usize;
            let active = unsafe { read::<u32>(target + 41) } as usize;
            if progress != 0x24
                || position != 0x10
                || active != 0xB4A
                || unsafe { read::<u8>(stop + 35) } as usize != progress
                || unsafe { read::<u8>(add + 8) } as usize != progress
                || unsafe { read::<u8>(add + 13) } as usize != progress
                || unsafe { read::<u8>(target + 16) } as usize != position + 4
                || unsafe { read::<u8>(target + 28) } as usize != position + 8
                || !(continuation..continuation + 0x1000).contains(&add)
                || !(continuation..continuation + 0x1000).contains(&target)
            {
                return Err("採掘進捗の命令配置が一致しません".into());
            }
            Ok(Native {
                stop,
                continuation,
                progress,
                position,
                active,
            })
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
pub fn use_mining(on: bool) -> Result<(), String> {
    if on {
        available()?;
        crate::game::acquire()?;
        USERS.fetch_add(1, Ordering::Relaxed);
    } else {
        crate::game::release()?;
        let left = USERS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1))
            .unwrap_or(0);
        if left <= 1 {
            LAST.store(0, Ordering::Relaxed);
            *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
    Ok(())
}
fn progress(value: f32, active: u8) -> Option<f32> {
    (active == 1 && value.is_finite() && (0. ..=2.).contains(&value)).then(|| value.min(1.))
}
pub(in crate::game) unsafe fn sample(player: usize, now: u64) {
    if USERS.load(Ordering::Relaxed) == 0 || now.saturating_sub(LAST.load(Ordering::Relaxed)) < 50 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let reading = (|| {
        let n = NATIVE.get()?.as_ref().ok()?;
        let (value, position) = {
            let mut m = ReadBatch::new();
            if !m.readable(player, MODE + 8) || !m.readable(player + n.active, 1) {
                return None;
            }
            let mode = read::<usize>(player + MODE);
            if !m.readable(mode, 0x28) || read::<usize>(mode + 8) != player {
                return None;
            }
            let vt = read::<usize>(mode);
            if !m.readable(vt, 0x80)
                || read::<usize>(vt + 0x20) != n.stop
                || read::<usize>(vt + 0x18) != n.continuation
            {
                return None;
            }
            let value = progress(read(mode + n.progress), read(player + n.active))?;
            let position: [i32; 3] = read(mode + n.position);
            if position
                .iter()
                .any(|&v| !(-30_000_000..=30_000_000).contains(&v))
            {
                return None;
            }
            (value, position)
        }; // No protection cache survives this native block getter.
        let name = blocks::name_at(player, position)?;
        Some((
            now,
            Reading {
                name,
                position,
                progress: value,
            },
        ))
    })();
    if let Ok(mut slot) = SNAPSHOT.try_lock() {
        *slot = reading;
    }
}
pub fn snapshot() -> Option<Reading> {
    if USERS.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let slot = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref()
        .filter(|(at, _)| unsafe { GetTickCount64() }.saturating_sub(*at) < 250)
        .map(|(_, r)| r.clone())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_progress_requires_active_finite_values_and_clamps_completion() {
        assert_eq!(progress(0., 1), Some(0.));
        assert_eq!(progress(0.45, 1), Some(0.45));
        assert_eq!(progress(1.2, 1), Some(1.));
        for v in [f32::NAN, f32::INFINITY, -0.1, 3.] {
            assert_eq!(progress(v, 1), None);
        }
        for active in [0, 2, 255] {
            assert_eq!(progress(0.5, active), None);
        }
    }
}
