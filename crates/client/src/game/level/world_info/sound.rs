//! Read-only FMOD counters; no audio hooks, playback, or mixer changes.
use crate::game::{build_name, layout::image, memory::ReadBatch, patch, read};
use crate::signatures::{
    SOUND_DEBUG_SIGNATURE, SOUND_ENGINE_STATS_SIGNATURE, SOUND_INIT_SIGNATURE,
};
use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

type Query = unsafe extern "system" fn(usize, *mut i32, *mut i32) -> i32;
struct Native {
    stats: usize,
    init: usize,
    query: Query,
}
static NATIVE: OnceLock<Option<Native>> = OnceLock::new();
pub(super) fn prepare() {
    NATIVE.get_or_init(|| {
        build_name()?;
        let stats = patch::find(SOUND_DEBUG_SIGNATURE).ok()?;
        let init = patch::find(SOUND_INIT_SIGNATURE).ok()?;
        patch::find(SOUND_ENGINE_STATS_SIGNATURE).ok()?;
        let module = unsafe { GetModuleHandleW(windows_sys::core::w!("fmod.dll")) };
        if module.is_null() {
            return None;
        }
        let entry = unsafe {
            GetProcAddress(
                module,
                c"?getChannelsPlaying@System@FMOD@@QEAA?AW4FMOD_RESULT@@PEAH0@Z"
                    .as_ptr()
                    .cast(),
            )
        }?;
        Some(Native {
            stats,
            init,
            query: unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, Query>(entry)
            },
        })
    });
}
unsafe fn pointer(m: &mut ReadBatch, at: usize, size: usize) -> Option<usize> {
    if !m.readable(at, 8) {
        return None;
    }
    let value = read::<usize>(at);
    m.readable(value, size).then_some(value)
}
pub(super) unsafe fn sample(client: usize) -> Option<(u32, u32)> {
    let n = NATIVE.get()?.as_ref()?;
    let mut m = ReadBatch::new();
    // The final SoundSystem vtable validates this exact known-build chain;
    // none of the intermediate pointers are kept between callbacks.
    let game = pointer(&mut m, client.checked_add(0x1a8)?, 0x760)?;
    let engine = pointer(&mut m, game + 0x758, 0x20)?;
    let system = pointer(&mut m, engine + 0x18, 0x58)?;
    let vt = read::<usize>(system);
    let (base, end) = image();
    if vt < base
        || vt.checked_add(0x2b8)? > end
        || read::<usize>(vt + 0x210) != n.stats
        || read::<usize>(vt + 0x2b0) != n.init
    {
        return None;
    }
    // This class's init entry uses the verified thread-safe flags, so a
    // counter query may run on the game callback (never on the render thread).
    let fmod = pointer(&mut m, system + 0x50, 8)?;
    let (mut total, mut real) = (-1, -1);
    if (n.query)(fmod, &mut total, &mut real) != 0 {
        return None;
    }
    valid_counts(total, real)
}
fn valid_counts(total: i32, real: i32) -> Option<(u32, u32)> {
    (0 <= real && real <= total && total <= 100000).then_some((total as u32, real as u32))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sound_counter_errors_and_inconsistent_values_are_not_displayed() {
        assert_eq!(valid_counts(0, 0), Some((0, 0)));
        assert_eq!(valid_counts(35, 12), Some((35, 12)));
        for (a, b) in [(-1, 0), (1, -1), (1, 2), (100001, 1)] {
            assert_eq!(valid_counts(a, b), None);
        }
    }
}
