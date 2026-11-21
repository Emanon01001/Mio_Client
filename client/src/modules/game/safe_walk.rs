//! Grounded edge probes and ordinary sneak demand, evaluated on native input ticks.
use super::{blocks, entities::actor_on_ground, layout, memory::ReadBatch, read};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Mutex,
};
static UNTIL: AtomicU64 = AtomicU64::new(0);
static MARGIN: AtomicU32 = AtomicU32::new(0x3df5c28f); // 0.12
static ON: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicU32 = AtomicU32::new(0);
#[derive(Default)]
struct Probe {
    world: (usize, u32, usize),
    positions: [[i32; 3]; 4],
    at: u64,
    risk: bool,
}
static PROBE: Mutex<Option<Probe>> = Mutex::new(None);
pub fn request(on: bool, now: u64, margin: f32) {
    MARGIN.store(margin.clamp(0.05, 0.4).to_bits(), Ordering::Relaxed);
    UNTIL.store(
        if on { now.saturating_add(100) } else { 0 },
        Ordering::Release,
    );
    ON.store(on, Ordering::Release);
}
pub(super) fn enabled() -> bool {
    ON.load(Ordering::Acquire)
}
fn allowed(on: bool, now: u64, until: u64, grounded: bool, jumping: bool) -> bool {
    on && now < until && grounded && !jumping
}
fn positions(bounds: [f32; 6], velocity: [f32; 3], margin: f32) -> Option<[[i32; 3]; 4]> {
    if !bounds
        .iter()
        .chain(velocity.iter())
        .chain([margin].iter())
        .all(|v| v.is_finite())
        || bounds.iter().any(|v| v.abs() > 3.0e7)
        || bounds[3] <= bounds[0]
        || bounds[5] <= bounds[2]
        || bounds[3] - bounds[0] > 4.
        || bounds[5] - bounds[2] > 4.
    {
        return None;
    }
    let dx = (velocity[0] * 2.).clamp(-0.6, 0.6);
    let dz = (velocity[2] * 2.).clamp(-0.6, 0.6);
    let y = (bounds[1] - 0.01).floor() as i32;
    Some(std::array::from_fn(|i| {
        [
            (if i & 1 == 0 {
                bounds[0] - margin + dx
            } else {
                bounds[3] + margin + dx
            })
            .floor() as i32,
            y,
            (if i & 2 == 0 {
                bounds[2] - margin + dz
            } else {
                bounds[5] + margin + dz
            })
            .floor() as i32,
        ]
    }))
}
pub(super) unsafe fn sneak(instance: usize, flags: u32, now: u64) -> bool {
    if !enabled() || now >= UNTIL.load(Ordering::Acquire) {
        STATUS.store(0, Ordering::Relaxed);
        return false;
    }
    let Some((player, world, bounds, velocity)) = (|| {
        let l = layout::layout().ok()?;
        let get: unsafe extern "system" fn(usize) -> usize = {
            let mut m = ReadBatch::new();
            if !m.readable(instance, 8) {
                return None;
            }
            let vt = read::<usize>(instance);
            if !m.readable(vt, l.vt_local_player + 8) {
                return None;
            }
            std::mem::transmute(read::<usize>(vt + l.vt_local_player))
        };
        let player = get(instance);
        let mut m = ReadBatch::new();
        if !m.readable(player, l.actor_state.max(l.actor_aabb).max(0x1f0) + 8) {
            return None;
        }
        let world = (
            read::<usize>(player + 0x10),
            read::<u32>(player + 0x18),
            read::<usize>(player + 0x1f0),
        );
        let shape = read::<usize>(player + l.actor_aabb);
        let state = read::<usize>(player + l.actor_state);
        if !m.readable(shape, 24) || !m.readable(state, 36) {
            return None;
        }
        Some((
            player,
            world,
            read::<[f32; 6]>(shape),
            read::<[f32; 3]>(state + 24),
        ))
    })() else {
        STATUS.store(3, Ordering::Relaxed);
        return false;
    };
    if !allowed(
        enabled(),
        now,
        UNTIL.load(Ordering::Acquire),
        actor_on_ground(player),
        flags & (1 << 7) != 0,
    ) {
        STATUS.store(0, Ordering::Relaxed);
        return false;
    }
    let Some(positions) = positions(
        bounds,
        velocity,
        f32::from_bits(MARGIN.load(Ordering::Relaxed)),
    ) else {
        STATUS.store(3, Ordering::Relaxed);
        return false;
    };
    let Ok(mut cache) = PROBE.try_lock() else {
        return false;
    };
    if let Some(p) = cache
        .as_ref()
        .filter(|p| p.world == world && p.positions == positions && now.saturating_sub(p.at) < 20)
    {
        return p.risk;
    }
    // Unknown/unloaded support is conservative: ask vanilla sneak to constrain
    // the edge rather than treating a pointer/name failure as solid ground.
    let result = blocks::edge_unsupported(player, &positions);
    let risk = result.unwrap_or(true);
    *cache = Some(Probe {
        world,
        positions,
        at: now,
        risk,
    });
    STATUS.store(
        if result.is_none() {
            3
        } else if risk {
            2
        } else {
            1
        },
        Ordering::Relaxed,
    );
    risk
}
pub fn status() -> String {
    crate::localization::text(match STATUS.load(Ordering::Relaxed) {
        1 => "ground supported",
        2 => "edge: sneaking",
        3 => "support uncertain",
        _ => "waiting for ground / jump override",
    })
    .into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probes_account_for_negative_coordinates_velocity_and_floor_height() {
        let b = [0.2, 64., 0.2, 0.8, 65.8, 0.8];
        assert_eq!(positions(b, [0.; 3], 0.12).unwrap(), [[0, 63, 0]; 4]);
        let p = positions(b, [0.3, 0., 0.], 0.12).unwrap();
        assert_eq!((p[0], p[1]), ([0, 63, 0], [1, 63, 0]));
        let p = positions([-0.8, 63.5, -0.8, -0.2, 65., -0.2], [0.; 3], 0.12).unwrap();
        assert_eq!(p, [[-1, 63, -1]; 4]);
        assert!(positions([f32::NAN; 6], [0.; 3], 0.12).is_none());
        assert!(positions(b, [f32::INFINITY; 3], 0.12).is_none());
    }
    #[test]
    fn pause_expiry_airborne_and_manual_jump_release_demand() {
        assert!(allowed(true, 100, 200, true, false));
        assert!(!allowed(false, 100, 200, true, false));
        assert!(!allowed(true, 200, 200, true, false));
        assert!(!allowed(true, 100, 200, false, false));
        assert!(!allowed(true, 100, 200, true, true));
    }
}
