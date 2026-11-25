//! Movement/death events copied on the input thread. No disk I/O in hooks.
use crate::game::{actors, entities::actor_on_ground, memory::ReadBatch, read};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
static STATS: AtomicBool = AtomicBool::new(false);
static DEATH: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, Default)]
pub struct Totals {
    pub distance: f64,
    pub max_speed: f32,
    pub jumps: u64,
    pub elapsed_ms: u64,
}
#[derive(Clone, Debug)]
pub struct Death {
    pub position: [i32; 3],
    pub dimension: Option<String>,
}
#[derive(Clone, Copy)]
struct Frame {
    at: u64,
    world: (usize, u32),
    dimension: usize,
    position: [f32; 3],
    ground: bool,
    health: Option<f32>,
}
struct State {
    previous: Option<Frame>,
    totals: Totals,
    death: Option<Death>,
}
static STATE: Mutex<State> = Mutex::new(State {
    previous: None,
    totals: Totals {
        distance: 0.,
        max_speed: 0.,
        jumps: 0,
        elapsed_ms: 0,
    },
    death: None,
});
pub fn set_stats(on: bool) {
    STATS.store(on, Ordering::Relaxed);
    STATE.lock().unwrap_or_else(|e| e.into_inner()).previous = None;
}
pub fn set_death(on: bool) {
    DEATH.store(on, Ordering::Relaxed);
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.previous = None;
    s.death = None;
}
pub fn reset() {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.totals = Totals::default();
    s.previous = None;
}
pub fn totals() -> Totals {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).totals
}
pub fn take_death() -> Option<Death> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).death.take()
}
fn advance(s: &mut State, f: Frame, stats: bool, death: bool, dimension: Option<String>) {
    if let Some(old) = s.previous {
        if old.world == f.world
            && old.dimension == f.dimension
            && f.at > old.at
            && f.at - old.at <= 500
        {
            let dt = f.at - old.at;
            let dx = f.position[0] - old.position[0];
            let dz = f.position[2] - old.position[2];
            let distance = (dx * dx + dz * dz).sqrt();
            let speed = distance * 1000. / dt as f32;
            // Discontinuities (teleport/respawn/world change) are not travel.
            if stats && speed <= 60. && (f.position[1] - old.position[1]).abs() < 8. {
                s.totals.distance += distance as f64;
                s.totals.max_speed = s.totals.max_speed.max(speed);
                s.totals.elapsed_ms += dt;
                if old.ground && !f.ground && f.position[1] > old.position[1] + 0.02 {
                    s.totals.jumps += 1;
                }
            }
            if death && old.health.is_some_and(|h| h > 0.) && f.health == Some(0.) {
                s.death = Some(Death {
                    position: f.position.map(|v| v.floor() as i32),
                    dimension,
                });
            }
        }
    }
    s.previous = Some(f);
}
pub(in crate::game) unsafe fn sample(player: usize, now: u64) {
    let stats = STATS.load(Ordering::Relaxed);
    let death = DEATH.load(Ordering::Relaxed);
    if !crate::game::known_build() || !stats && !death {
        return;
    }
    let Ok(mut s) = STATE.try_lock() else {
        return;
    };
    if s.previous.is_some_and(|f| now.saturating_sub(f.at) < 50) {
        return;
    }
    let mut m = ReadBatch::new();
    if !m.readable(player, 0x250) {
        s.previous = None;
        return;
    }
    let state = read::<usize>(player + 0x240);
    let bounds = read::<usize>(player + 0x248);
    if !m.readable(state, 12) || !m.readable(bounds, 24) {
        s.previous = None;
        return;
    }
    let p = read::<[f32; 3]>(state);
    let position = [p[0], read::<f32>(bounds + 4), p[2]];
    if !position.iter().all(|v| v.is_finite() && v.abs() < 3.0e7) {
        s.previous = None;
        return;
    }
    let f = Frame {
        at: now,
        world: (read(player + 0x10), read(player + 0x18)),
        dimension: read(player + 0x1f0),
        position,
        ground: actor_on_ground(player),
        health: if death {
            actors::own_health(player).map(|h| h.0)
        } else {
            None
        },
    };
    let dimension = if death
        && f.health == Some(0.)
        && s.previous.is_some_and(|p| p.health.is_some_and(|h| h > 0.))
    {
        crate::game::world::read_dimension(player).map(|(n, r)| match r {
            crate::game::world::Realm::Overworld => "overworld".into(),
            crate::game::world::Realm::Nether => "nether".into(),
            crate::game::world::Realm::End => "end".into(),
            _ => n.to_lowercase(),
        })
    } else {
        None
    };
    advance(&mut s, f, stats, death, dimension);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn travel_death_and_discontinuities() {
        let mut s = State {
            previous: None,
            totals: Totals::default(),
            death: None,
        };
        let mut f = Frame {
            at: 100,
            world: (1, 1),
            dimension: 1,
            position: [0.; 3],
            ground: true,
            health: Some(20.),
        };
        advance(&mut s, f, true, true, None);
        f.at += 100;
        f.position = [0.4, 0.4, 0.];
        f.ground = false;
        advance(&mut s, f, true, true, None);
        assert_eq!(s.totals.jumps, 1);
        assert!((s.totals.distance - 0.4).abs() < 0.001);
        f.at += 100;
        f.position[0] = 100.;
        advance(&mut s, f, true, true, None);
        assert!(s.totals.max_speed < 5.);
        f.at += 100;
        f.health = Some(0.);
        advance(&mut s, f, true, true, Some("nether".into()));
        assert_eq!(s.death.take().unwrap().dimension.as_deref(), Some("nether"));
        f.at += 100;
        advance(&mut s, f, true, true, None);
        assert!(s.death.is_none());
        f.at += 100;
        f.world = (2, 1);
        f.health = Some(20.);
        advance(&mut s, f, true, true, None);
        assert_eq!(s.totals.jumps, 1);
    }
}
