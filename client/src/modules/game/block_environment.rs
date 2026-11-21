//! Optional F3-only biome and two-layer fluid reads on the game callback.
use super::{binding, block_name, ray_steps, GetBlock, Look, LookRay, FLUIDS, GET_BLOCK};
use crate::modules::game::{build_name, layout::image, memory::ReadBatch, patch, read};
use crate::signatures::{
    BIOME_NAME_FIELD_SIGNATURE, GET_BIOME_SIGNATURE, GET_EXTRA_BLOCK_SIGNATURE,
};
use std::borrow::Cow;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex, OnceLock,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

struct Native {
    biome: usize,
    extra: usize,
}
static NATIVE: OnceLock<Option<Native>> = OnceLock::new();
pub(crate) fn prepare() {
    NATIVE.get_or_init(|| {
        build_name()?;
        super::available().ok()?;
        patch::find(BIOME_NAME_FIELD_SIGNATURE).ok()?;
        Some(Native {
            biome: patch::find(GET_BIOME_SIGNATURE).ok()?,
            extra: patch::find(GET_EXTRA_BLOCK_SIGNATURE).ok()?,
        })
    });
}
unsafe fn source(player: usize) -> Option<(usize, i32, i32, GetBlock, usize)> {
    let getter = *GET_BLOCK.get()?.as_ref().ok()?;
    let (world, source, min, max) = binding(player, getter)?;
    Some((
        source,
        min,
        max,
        std::mem::transmute::<usize, GetBlock>(getter),
        world.4,
    ))
}
unsafe fn loaded(m: &mut ReadBatch, source: usize, position: [i32; 3]) -> bool {
    let vt = read::<usize>(source);
    let (base, end) = image();
    if vt < base || vt.checked_add(0x150).is_none_or(|v| v > end) {
        return false;
    }
    let entry = read::<usize>(vt + 0x148);
    if !(base..end).contains(&entry) {
        return false;
    }
    let get: unsafe extern "system" fn(usize, *const [i32; 2]) -> usize =
        std::mem::transmute(entry);
    let chunk = get(
        source,
        &[position[0].div_euclid(16), position[2].div_euclid(16)],
    );
    m.readable(chunk, 8)
}
pub(crate) unsafe fn biome(player: usize, position: [i32; 3]) -> Option<String> {
    let n = NATIVE.get()?.as_ref()?;
    let (source, min, max, _, _) = source(player)?;
    let mut m = ReadBatch::new();
    if position[1] < min || position[1] >= max || !loaded(&mut m, source, position) {
        return None;
    }
    let get: unsafe extern "system" fn(usize, *const [i32; 3]) -> usize =
        std::mem::transmute(n.biome);
    let biome = get(source, &position);
    if !m.readable(biome, 0x1b8) {
        return None;
    }
    m.string(biome + 0x198, 128).filter(|s| valid_identifier(s))
}
fn valid_identifier(s: &str) -> bool {
    let Some((namespace, name)) = s.split_once(':') else {
        return false;
    };
    !namespace.is_empty()
        && !name.is_empty()
        && [namespace, name].iter().all(|part| {
            part.bytes().all(|b| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'_' | b'.' | b'/' | b'-')
            })
        })
}

static TARGET: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);
type Fluid = (u64, (usize, u32), Option<Look>);
static FLUID: Mutex<Option<Fluid>> = Mutex::new(None);
pub(crate) fn diagnostic() -> String {
    let g = FLUID.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some((at, _, value)) => format!(
            "DebugHUD fluid age={}ms: {value:?}",
            unsafe { GetTickCount64() }.saturating_sub(*at)
        ),
        None => "DebugHUD fluid: no sample".into(),
    }
}
pub(crate) fn set_target(on: bool) {
    if on {
        prepare();
    }
    TARGET.store(on, Ordering::Release);
    LAST.store(0, Ordering::Relaxed);
    if !on {
        *FLUID.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}
pub(crate) fn fluid(owner: (usize, u32)) -> Option<Option<Look>> {
    if !TARGET.load(Ordering::Acquire) {
        return None;
    }
    let g = FLUID.lock().ok()?;
    let (at, token, value) = g.as_ref()?;
    (*token == owner && unsafe { GetTickCount64() }.saturating_sub(*at) < 500)
        .then(|| value.clone())
}
fn trace_fluid(
    ray: &LookRay,
    mut cell: impl FnMut([i32; 3]) -> Option<(Cow<'static, str>, Cow<'static, str>)>,
) -> Option<Option<Look>> {
    for (position, distance) in ray_steps(ray.origin, ray.forward, ray.reach) {
        let (primary, extra) = cell(position)?;
        for name in [&primary, &extra] {
            if FLUIDS.contains(&name.as_ref()) {
                return Some(Some(Look {
                    name: name.to_string(),
                    position,
                    distance,
                }));
            }
        }
        if primary != "air" {
            return Some(None);
        }
    }
    Some(None)
}
pub(in crate::modules::game) unsafe fn sample_target(player: usize, now: u64) {
    if !TARGET.load(Ordering::Acquire) || now.saturating_sub(LAST.load(Ordering::Relaxed)) < 100 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let value = (|| {
        let n = NATIVE.get()?.as_ref()?;
        let ray = super::look_ray()?;
        let owner = (read(player + 0x10), read(player + 0x18));
        if ray.owner != owner {
            return None;
        }
        let (source, min, max, get, air) = source(player)?;
        if read::<usize>(read::<usize>(source) + 0x20) != n.extra {
            return None;
        }
        let extra: GetBlock = std::mem::transmute(n.extra);
        let mut m = ReadBatch::new();
        let mut last_chunk = None;
        let result = trace_fluid(&ray, |position| {
            if position[1] < min || position[1] >= max {
                return None;
            }
            let chunk = [position[0].div_euclid(16), position[2].div_euclid(16)];
            if last_chunk != Some(chunk) {
                if !loaded(&mut m, source, position) {
                    return None;
                }
                last_chunk = Some(chunk);
            }
            let name = |block| {
                if block == air {
                    Some(Cow::Borrowed("air"))
                } else {
                    block_name(block).map(Cow::Owned)
                }
            };
            Some((
                name(get(source, &position))?,
                name(extra(source, &position))?,
            ))
        })?;
        Some((now, owner, result))
    })();
    if let Ok(mut g) = FLUID.try_lock() {
        *g = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ray() -> LookRay {
        LookRay {
            owner: (0, 0),
            origin: [0.5, 0.5, 0.5],
            forward: [1., 0., 0.],
            reach: 6.,
            block: None,
        }
    }
    #[test]
    fn fluid_trace_checks_secondary_water_and_stops_at_occluders_and_unloaded_cells() {
        let r = ray();
        let water = trace_fluid(&r, |p| {
            Some((
                if p[0] == 2 { "oak_stairs" } else { "air" }.into(),
                if p[0] == 2 { "water" } else { "air" }.into(),
            ))
        })
        .unwrap()
        .unwrap();
        assert_eq!(water.position, [2, 0, 0]);
        assert_eq!(water.distance, 1.5);
        assert_eq!(
            trace_fluid(&r, |p| Some((
                if p[0] == 1 { "stone" } else { "water" }.into(),
                "air".into()
            )))
            .unwrap()
            .unwrap()
            .position,
            [0, 0, 0]
        );
        assert_eq!(
            trace_fluid(&r, |p| Some((
                if p[0] == 1 { "stone" } else { "air" }.into(),
                "air".into()
            ))),
            Some(None)
        );
        assert_eq!(trace_fluid(&r, |_| None), None);
        assert_eq!(
            trace_fluid(&r, |p| Some((
                match p[0] {
                    1 => "stone",
                    2 => "water",
                    _ => "air",
                }
                .into(),
                "air".into()
            ))),
            Some(None)
        );
    }
    #[test]
    fn biome_names_accept_namespaces_and_reject_corrupt_or_control_text() {
        assert!(valid_identifier("minecraft:plains"));
        assert!(valid_identifier("custom:mountains/high_2"));
        for s in [
            "plains",
            ":plains",
            "minecraft:",
            "minecraft:plains\n",
            "minecraft:Plains",
            "a:b:c",
        ] {
            assert!(!valid_identifier(s));
        }
    }
}
