//! One loaded target cell, using the sky/block nibble layout read by the
//! verified getBrightness function. No floor/radius scan or chunk creation.
use super::{binding, GET_BLOCK};
use crate::modules::game::{self, layout::image, memory::ReadBatch, read};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
static ON: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);
static SAMPLE: Mutex<Option<(u64, DebugLight)>> = Mutex::new(None);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DebugLight {
    pub owner: (usize, u32),
    pub position: [i32; 3],
    pub sky: u8,
    pub block: u8,
}
pub(crate) fn set_debug_active(on: bool) -> Result<(), String> {
    if on {
        super::available()?;
        if game::build_name().is_none() {
            return Err("光量の直接読取は照合済みビルドでのみ利用できます".into());
        }
    }
    ON.store(on, Ordering::Release);
    LAST.store(0, Ordering::Relaxed);
    if !on {
        *SAMPLE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    Ok(())
}
pub(crate) fn debug_snapshot(owner: (usize, u32), position: [i32; 3]) -> Option<DebugLight> {
    if !ON.load(Ordering::Acquire) {
        return None;
    }
    let sample = SAMPLE.lock().ok()?;
    let (at, value) = (*sample)?;
    (value.owner == owner
        && value.position == position
        && unsafe { GetTickCount64() }.saturating_sub(at) < 500)
        .then_some(value)
}
unsafe fn nibble(m: &mut ReadBatch, at: usize, index: usize) -> Option<u8> {
    let byte = at.checked_add(index / 2)?;
    m.readable(byte, 1)
        .then(|| (read::<u8>(byte) >> ((index & 1) * 4)) & 15)
}
unsafe fn chunk_light(m: &mut ReadBatch, chunk: usize, min: i32, p: [i32; 3]) -> Option<(u8, u8)> {
    if !m.readable(chunk, 0x151a) {
        return None;
    }
    let (begin, end) = (read::<usize>(chunk + 0x1c8), read::<usize>(chunk + 0x1d0));
    if end < begin || (end - begin) % 0x68 != 0 || end - begin > 64 * 0x68 {
        return None;
    }
    let section = p[1].checked_sub(min)?.div_euclid(16);
    if section < 0 {
        return None;
    }
    let section = section as usize;
    if section >= (end - begin) / 0x68 {
        let sky = read::<u8>(chunk + 0x1518);
        let block = read::<u8>(chunk + 0x1519);
        return (sky <= 15 && block <= 15).then_some((sky, block));
    }
    let at = begin.checked_add(section * 0x68)?;
    if !m.readable(at, 0x68) {
        return None;
    }
    let index = ((p[0].rem_euclid(16) as usize) << 8)
        | ((p[2].rem_euclid(16) as usize) << 4)
        | ((p[1] - min).rem_euclid(16) as usize);
    let sky_ptr = read::<usize>(at + 8);
    let block_ptr = read::<usize>(at + 0x10);
    let sky = if sky_ptr == 0 {
        if read::<u8>(at + 0x1c) == 0 {
            0
        } else {
            15
        }
    } else {
        nibble(m, sky_ptr, index)?
    };
    let block = if block_ptr == 0 {
        0
    } else {
        nibble(m, block_ptr, index)?
    };
    Some((sky, block))
}
pub(in crate::modules::game) unsafe fn sample_debug(player: usize) {
    if !ON.load(Ordering::Acquire) {
        return;
    }
    let now = GetTickCount64();
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < 100 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let value = (|| {
        let ray = game::blocks::look_ray()?;
        let owner = (read(player + 0x10), read(player + 0x18));
        if ray.owner != owner {
            return None;
        }
        let p = ray.block?.position;
        let get = *GET_BLOCK.get()?.as_ref().ok()?;
        let (_, source, min, max) = binding(player, get)?;
        if p[1] < min || p[1] >= max {
            return None;
        }
        let mut m = ReadBatch::new();
        let vt = read::<usize>(source);
        let (base, end) = image();
        if vt < base || vt.checked_add(0x150)? > end || !m.readable(vt + 0x148, 8) {
            return None;
        }
        let getter = read::<usize>(vt + 0x148);
        if !(base..end).contains(&getter) {
            return None;
        }
        let loaded: unsafe extern "system" fn(usize, *const [i32; 2]) -> usize =
            std::mem::transmute(getter);
        let chunk = loaded(source, &[p[0].div_euclid(16), p[2].div_euclid(16)]);
        let (sky, block) = chunk_light(&mut m, chunk, min, p)?;
        Some(DebugLight {
            owner,
            position: p,
            sky,
            block,
        })
    })();
    if let Ok(mut out) = SAMPLE.try_lock() {
        *out = value.map(|v| (now, v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sky_and_block_nibbles_stay_separate_at_negative_and_section_boundaries() {
        let mut chunk = vec![0u8; 0x1520];
        let mut sections = vec![0u8; 4 * 0x68];
        let sky = vec![0x93u8; 2048];
        let block = vec![0x27u8; 2048];
        unsafe {
            let root = chunk.as_mut_ptr() as usize;
            let base = sections.as_mut_ptr() as usize;
            (root as *mut u8)
                .add(0x1c8)
                .cast::<usize>()
                .write_unaligned(base);
            (root as *mut u8)
                .add(0x1d0)
                .cast::<usize>()
                .write_unaligned(base + sections.len());
            for i in 0..4 {
                (base as *mut u8)
                    .add(i * 0x68 + 8)
                    .cast::<usize>()
                    .write_unaligned(sky.as_ptr() as usize);
                (base as *mut u8)
                    .add(i * 0x68 + 0x10)
                    .cast::<usize>()
                    .write_unaligned(block.as_ptr() as usize);
            }
            for p in [[-1, -64, 16], [-1, -63, 16], [16, -2, -1], [16, -1, -1]] {
                let expected = if (p[1] + 64) & 1 == 0 { (3, 7) } else { (9, 2) };
                assert_eq!(
                    chunk_light(&mut ReadBatch::new(), root, -64, p),
                    Some(expected)
                );
            }
            chunk[0x1518] = 15;
            assert_eq!(
                chunk_light(&mut ReadBatch::new(), root, -64, [0, 0, 0]),
                Some((15, 0))
            );
            chunk[0x1519] = 16;
            assert_eq!(
                chunk_light(&mut ReadBatch::new(), root, -64, [0, 0, 0]),
                None
            );
            assert_eq!(chunk_light(&mut ReadBatch::new(), 0, -64, [0, 0, 0]), None);
        }
    }
}
