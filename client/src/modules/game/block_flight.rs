//! Projectile trajectories: the path a shot or thrown projectile would take
//! from the player's eye along the view, up to the first block or entity it
//! would hit. Simulated tick by tick on the game thread, where blocks can be
//! read. Read only.
//!
//! The physics are the vanilla per-tick model (move, then drag, then
//! gravity) with Java Edition's constants; Bedrock's are close but not
//! guaranteed identical, so the path is an estimate. Inaccuracy (random
//! spread) and the shooter's own velocity are not modelled.

use super::super::{shared_boxes, Aabb};
use super::{binding, block_name, ray_steps, GetBlock, World, FLUIDS, GET_BLOCK};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

/// Ticks simulated at most (10 seconds of flight).
const MAX_TICKS: usize = 200;
static INTERVAL_MS: AtomicU64 = AtomicU64::new(16);
static TICKS: AtomicUsize = AtomicUsize::new(MAX_TICKS);
static ENTITIES: AtomicBool = AtomicBool::new(true);
static REVISION: AtomicU64 = AtomicU64::new(0);
/// A published path older than this is not drawn.
const MAX_AGE_MS: u64 = 250;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);
/// The held projectile's physics, set from the client thread.
static WANTED: Mutex<Option<Physics>> = Mutex::new(None);
static FLIGHT: Mutex<Option<(u64, u64, Arc<Flight>)>> = Mutex::new(None);
struct Cache {
    world: World,
    names: HashMap<usize, bool>,
}
static NAMES: Mutex<Option<Cache>> = Mutex::new(None);

pub fn configure(ticks: usize, entities: bool, hz: u32) {
    let changed = (TICKS.swap(ticks.clamp(20, MAX_TICKS), Ordering::Relaxed)
        != ticks.clamp(20, MAX_TICKS))
        | (ENTITIES.swap(entities, Ordering::Relaxed) != entities)
        | (INTERVAL_MS.swap(1000 / hz.clamp(10, 60) as u64, Ordering::Relaxed)
            != 1000 / hz.clamp(10, 60) as u64);
    if changed {
        REVISION.fetch_add(1, Ordering::AcqRel);
    }
}

/// How a projectile moves, per game tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Physics {
    /// Launch speed, blocks per tick.
    pub speed: f32,
    pub gravity: f32,
    /// Velocity kept each tick in air.
    pub drag: f32,
    /// Added to the view pitch at launch (degrees; negative is upward).
    pub pitch_offset: f32,
    /// How far entity boxes are grown for hit tests.
    pub inflate: f32,
}

/// Fraction of full power after drawing a bow for `seconds`; vanilla's
/// curve reaches full power after one second.
pub fn bow_power(seconds: f32) -> f32 {
    if !seconds.is_finite() || seconds <= 0. {
        return 1.;
    }
    let t = seconds.min(1.);
    ((t * t + 2. * t) / 3.).clamp(0.1, 1.)
}

/// The physics of the projectile an item launches, or None for items that
/// launch nothing. `bow_seconds` is how long the bow has been drawn (0 when
/// not drawing: the full-power path is shown).
pub fn physics(item: &str, bow_seconds: f32) -> Option<Physics> {
    let item = item.strip_prefix("minecraft:").unwrap_or(item);
    let (speed, gravity, pitch_offset, inflate) = match item {
        "bow" => (3. * bow_power(bow_seconds), 0.05, 0., 0.3),
        "crossbow" => (3.15, 0.05, 0., 0.3),
        "trident" => (2.5, 0.05, 0., 0.3),
        "snowball" | "egg" | "brown_egg" | "blue_egg" | "ender_pearl" => (1.5, 0.03, 0., 0.),
        "splash_potion" | "lingering_potion" => (0.5, 0.05, -20., 0.),
        "experience_bottle" => (0.7, 0.07, -20., 0.),
        _ => return None,
    };
    Some(Physics {
        speed,
        gravity,
        drag: 0.99,
        pitch_offset,
        inflate,
    })
}

/// Positions tick by tick from `origin` along `direction` (need not be unit
/// length), starting with `origin`.
#[cfg(test)]
pub fn path(
    origin: [f32; 3],
    direction: [f32; 3],
    physics: Physics,
    ticks: usize,
) -> Vec<[f32; 3]> {
    flight_path(origin, direction, physics, ticks).collect()
}
fn flight_path(
    origin: [f32; 3],
    direction: [f32; 3],
    physics: Physics,
    ticks: usize,
) -> impl Iterator<Item = [f32; 3]> {
    let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();
    let valid = length.is_finite()
        && length > 1e-6
        && origin.iter().all(|v| v.is_finite() && v.abs() < 3e7)
        && [
            physics.speed,
            physics.gravity,
            physics.drag,
            physics.pitch_offset,
            physics.inflate,
        ]
        .iter()
        .all(|v| v.is_finite())
        && physics.speed > 0.
        && physics.gravity >= 0.
        && (0. ..=1.).contains(&physics.drag)
        && physics.inflate >= 0.;
    let d = if valid {
        direction.map(|v| v / length)
    } else {
        [0.; 3]
    };
    // Re-aim by the pitch offset: Minecraft pitch is positive downward.
    let pitch = (-d[1]).clamp(-1., 1.).asin() + physics.pitch_offset.to_radians();
    let horizontal = (d[0] * d[0] + d[2] * d[2]).sqrt();
    let (hx, hz) = if horizontal > 1e-6 {
        (d[0] / horizontal, d[2] / horizontal)
    } else {
        (0., 1.)
    };
    let mut velocity = [
        hx * pitch.cos() * physics.speed,
        -pitch.sin() * physics.speed,
        hz * pitch.cos() * physics.speed,
    ];
    let mut position = origin;
    let mut index = 0;
    std::iter::from_fn(move || {
        if !valid
            || index > ticks.min(MAX_TICKS)
            || !position.iter().all(|v| v.is_finite() && v.abs() < 3e7)
        {
            return None;
        }
        let point = position;
        index += 1;
        for k in 0..3 {
            position[k] += velocity[k];
            velocity[k] *= physics.drag;
        }
        velocity[1] -= physics.gravity;
        Some(point)
    })
}

/// What the flight ends on.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Block([i32; 3]),
    Entity(Aabb),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub point: [f32; 3],
    pub target: Target,
}

/// A path cut where it first hits something.
#[derive(Clone, Debug, PartialEq)]
pub struct Flight {
    pub points: Vec<[f32; 3]>,
    pub hit: Option<Hit>,
}

/// Blocks a projectile flies through: air, fluids and blocks without a
/// collision box (the ones the minimap sees through).
fn passable(name: &str) -> bool {
    if name == "barrier" {
        return false;
    }
    FLUIDS.contains(&name)
        || matches!(
            name,
            "kelp" | "seagrass" | "tall_seagrass" | "bubble_column"
        )
        || super::surface::color(name).is_none()
}

/// Cuts `points` at the first block (`solid(position)`: Some(true) solid,
/// Some(false) passable, None unreadable) or entity box it enters.
#[cfg(test)]
fn trace(
    points: &[[f32; 3]],
    boxes: &[Aabb],
    inflate: f32,
    solid: impl FnMut([i32; 3]) -> Option<bool>,
) -> Flight {
    trace_path(
        points.iter().copied(),
        boxes.iter().copied(),
        inflate,
        solid,
    )
}
fn trace_path(
    points: impl IntoIterator<Item = [f32; 3]>,
    boxes: impl IntoIterator<Item = Aabb>,
    inflate: f32,
    mut solid: impl FnMut([i32; 3]) -> Option<bool>,
) -> Flight {
    let mut points = points.into_iter();
    let mut out = Vec::with_capacity(MAX_TICKS + 1);
    let Some(first) = points.next() else {
        return Flight {
            points: out,
            hit: None,
        };
    };
    out.push(first);
    let grow = |bounds: &Aabb| -> Aabb {
        std::array::from_fn(|k| {
            if k < 3 {
                bounds[k] - inflate
            } else {
                bounds[k] + inflate
            }
        })
    };
    // A box around the launch point (a ridden mount) is not a target.
    let boxes: Vec<(Aabb, Aabb)> = boxes
        .into_iter()
        .filter(|b| b.iter().all(|v| v.is_finite()) && (0..3).all(|k| b[k] < b[k + 3]))
        .map(|b| (b, grow(&b)))
        .filter(|(_, g)| !(0..3).all(|k| g[k] <= first[k] && first[k] <= g[k + 3]))
        .collect();
    let mut a = first;
    for b in points {
        let direction = std::array::from_fn(|k| b[k] - a[k]);
        let length = direction.iter().map(|v: &f32| v * v).sum::<f32>().sqrt();
        if !(length.is_finite() && length > 1e-6) {
            continue;
        }
        let unit = direction.map(|v| v / length);
        let at = |distance: f32| std::array::from_fn(|k| a[k] + unit[k] * distance);
        let mut nearest: Option<(f32, Target)> = None;
        let mut unknown = None;
        for (position, distance) in ray_steps(a, direction, length) {
            match solid(position) {
                Some(false) => continue,
                Some(true) => nearest = Some((distance, Target::Block(position))),
                // Unknown ground: end the path there without a target.
                None => {
                    unknown = Some(distance);
                }
            }
            break;
        }
        for (bounds, grown) in &boxes {
            if (0..3).any(|k| grown[k] > a[k].max(b[k]) || grown[k + 3] < a[k].min(b[k])) {
                continue;
            }
            if let Some(distance) = crate::modules::view::ray_box(a, unit, *grown, length) {
                if unknown.is_none_or(|d| distance < d)
                    && nearest.as_ref().is_none_or(|(d, _)| distance < *d)
                {
                    nearest = Some((distance, Target::Entity(*bounds)));
                }
            }
        }
        if let Some(distance) = unknown.filter(|_| nearest.is_none()) {
            out.push(at(distance));
            return Flight {
                points: out,
                hit: None,
            };
        }
        if let Some((distance, target)) = nearest {
            let point = at(distance);
            out.push(point);
            return Flight {
                points: out,
                hit: Some(Hit { point, target }),
            };
        }
        out.push(b);
        a = b;
    }
    Flight {
        points: out,
        hit: None,
    }
}

pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Release);
    REVISION.fetch_add(1, Ordering::AcqRel);
    LAST.store(0, Ordering::Relaxed);
    if !active {
        *WANTED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *FLIGHT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *NAMES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The projectile to show the path of (None: nothing held that launches one).
pub fn set_wanted(physics: Option<Physics>) {
    let mut wanted = WANTED.lock().unwrap_or_else(|e| e.into_inner());
    if *wanted != physics {
        REVISION.fetch_add(1, Ordering::AcqRel);
        *wanted = physics;
    }
    if physics.is_none() {
        *FLIGHT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Whether the game thread should simulate a path at all.
pub(in crate::modules::game) fn wanted() -> bool {
    ACTIVE.load(Ordering::Acquire) && WANTED.try_lock().map(|w| w.is_some()).unwrap_or(false)
}

/// The latest simulated flight, if recent.
pub fn flight() -> Option<Arc<Flight>> {
    if !ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let guard = FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    let (at, revision, flight) = guard.as_ref()?;
    current(*at, *revision, REVISION.load(Ordering::Acquire), unsafe {
        GetTickCount64()
    })
    .then(|| Arc::clone(flight))
}
fn current(at: u64, revision: u64, wanted_revision: u64, now: u64) -> bool {
    revision == wanted_revision && now.saturating_sub(at) < MAX_AGE_MS
}

/// Only called inside the HUD hook, while the local player/dimension are live.
pub(in crate::modules::game) unsafe fn sample(player: usize, eye: [f32; 3], forward: [f32; 3]) {
    let now = GetTickCount64();
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < INTERVAL_MS.load(Ordering::Relaxed) {
        return;
    }
    let wanted = WANTED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(physics) = *wanted else {
        return;
    };
    let revision = REVISION.load(Ordering::Acquire);
    drop(wanted);
    let Some(Ok(&getter)) = GET_BLOCK.get().map(|r| r.as_ref()) else {
        return;
    };
    let Ok(mut names) = NAMES.try_lock() else {
        return;
    };
    LAST.store(now, Ordering::Relaxed);
    let Some((world, source, min, max)) = binding(player, getter) else {
        *FLIGHT.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    };
    if names.as_ref().is_none_or(|c| c.world != world) {
        if block_name(world.4).as_deref() != Some("air") {
            *FLIGHT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            *names = None;
            return;
        }
        *names = Some(Cache {
            world,
            names: HashMap::new(),
        });
    }
    let names = &mut names.as_mut().unwrap().names;
    let get: GetBlock = std::mem::transmute(getter);
    let points = flight_path(eye, forward, physics, TICKS.load(Ordering::Relaxed));
    // Living things only: dropped items, arrows and the like are tiny.
    let boxes = ENTITIES
        .load(Ordering::Relaxed)
        .then(shared_boxes)
        .flatten();
    let boxes = boxes
        .iter()
        .flat_map(|all| all.iter())
        .map(|(_, b)| *b)
        .filter(|b| b[4] - b[1] >= 0.3);
    let mut last_block = None;
    let flight = trace_path(points, boxes, physics.inflate, |position| {
        if let Some((previous, value)) = last_block {
            if previous == position {
                return value;
            }
        }
        let value = (|| {
            if position[1] >= max {
                return Some(false);
            }
            if position[1] < min {
                return None;
            }
            let block = get(source, &position);
            if block == 0 {
                return None;
            }
            if block == world.4 {
                return Some(false);
            }
            if !names.contains_key(&block) {
                let passable = passable(&block_name(block)?);
                if names.len() >= 4096 {
                    names.clear();
                }
                names.insert(block, passable);
            }
            Some(!names[&block])
        })();
        last_block = Some((position, value));
        value
    });
    if ACTIVE.load(Ordering::Acquire) && REVISION.load(Ordering::Acquire) == revision {
        *FLIGHT.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, revision, Arc::new(flight)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_from_an_old_item_or_setting_is_never_current() {
        assert!(current(100, 1, 1, 349));
        assert!(!current(100, 1, 1, 350));
        assert!(!current(100, 1, 2, 101));
    }

    #[test]
    fn entity_before_unknown_terrain_is_hit_but_not_one_behind_or_on_the_boundary() {
        let points = [[0.5, 64.5, 0.5], [0.5, 64.5, 10.5]];
        let front = [0., 64., 2., 1., 66., 3.];
        let behind = [0., 64., 7., 1., 66., 8.];
        let tied = [0., 64., 4., 1., 66., 5.];
        let unknown = |p: [i32; 3]| (p[2] < 4).then_some(false);
        assert_eq!(
            trace(&points, &[front, behind], 0., unknown)
                .hit
                .unwrap()
                .target,
            Target::Entity(front)
        );
        for target in [behind, tied] {
            let result = trace(&points, &[target], 0., unknown);
            assert!(result.hit.is_none());
            assert!((result.points.last().unwrap()[2] - 4.).abs() < 1e-6);
        }
        assert_eq!(
            trace(&points, &[tied], 0., |p| Some(p[2] == 4))
                .hit
                .unwrap()
                .target,
            Target::Block([0, 64, 4])
        );
    }
    #[test]
    fn simulation_stops_generating_ticks_after_the_first_collision() {
        let mut generated = 0;
        let points = flight_path(
            [0.5, 64.5, 0.5],
            [0., 0., 1.],
            physics("bow", 0.).unwrap(),
            200,
        )
        .inspect(|_| generated += 1);
        let flight = trace_path(points, [], 0., |p| Some(p[2] == 1));
        assert!(flight.hit.is_some());
        assert_eq!(generated, 2);
        assert_eq!(flight.points.len(), 2);
        let bad = Physics {
            gravity: f32::NAN,
            ..physics("bow", 0.).unwrap()
        };
        assert!(path([0.; 3], [0., 0., 1.], bad, 200).is_empty());
        assert_eq!(
            path(
                [0.; 3],
                [0., 0., 1.],
                physics("bow", 0.).unwrap(),
                usize::MAX
            )
            .len(),
            201
        );
    }

    #[test]
    fn items_launch_their_own_projectiles() {
        assert_eq!(physics("bow", 0.).unwrap().speed, 3.);
        assert!((physics("bow", 0.5).unwrap().speed - 3. * (0.25 + 1.) / 3.).abs() < 1e-6);
        assert_eq!(physics("minecraft:snowball", 0.).unwrap().gravity, 0.03);
        assert_eq!(physics("splash_potion", 0.).unwrap().pitch_offset, -20.);
        assert!(physics("diamond_sword", 0.).is_none());
        assert_eq!(bow_power(2.), 1.);
        assert_eq!(bow_power(0.01), 0.1);
        assert_eq!(bow_power(f32::NAN), 1.);
    }

    #[test]
    fn a_flat_shot_moves_then_drags_then_falls_each_tick() {
        let p = Physics {
            speed: 1.,
            gravity: 0.05,
            drag: 0.99,
            pitch_offset: 0.,
            inflate: 0.,
        };
        let points = path([0., 64., 0.], [0., 0., 1.], p, 3);
        assert_eq!(points.len(), 4);
        // Tick 1: +1 forward, no drop yet; tick 2: 0.99 forward, 0.05 down.
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5);
        assert!(close(points[1], [0., 64., 1.]), "{:?}", points[1]);
        assert!(close(points[2], [0., 63.95, 1.99]), "{:?}", points[2]);
        assert!(close(
            points[3],
            [0., 63.95 - 0.05 * 0.99 - 0.05, 1.99 + 0.9801]
        ));
        // Straight up comes back down; a pitch offset raises a level throw.
        let up = path([0., 0., 0.], [0., 1., 0.], p, 100);
        assert!(up.last().unwrap()[1] < 0.);
        let raised = Physics {
            pitch_offset: -20.,
            ..p
        };
        assert!(path([0., 0., 0.], [1., 0., 0.], raised, 1)[1][1] > 0.3);
        assert!(path([0., 0., 0.], [0., 0., 0.], p, 5).is_empty());
    }

    #[test]
    fn trace_stops_at_the_nearest_block_or_entity() {
        let points = [[0.5, 64.5, 0.5], [0.5, 64.5, 10.5]];
        // A wall at z = 6.
        let wall = |p: [i32; 3]| Some(p[2] == 6);
        let flight = trace(&points, &[], 0., wall);
        let hit = flight.hit.unwrap();
        assert_eq!(hit.target, Target::Block([0, 64, 6]));
        assert!((hit.point[2] - 6.).abs() < 1e-4);
        assert_eq!(flight.points.len(), 2);
        // An entity in front of the wall is hit first.
        let mob: Aabb = [0., 64., 3., 1., 66., 4.];
        let flight = trace(&points, &[mob], 0., wall);
        assert_eq!(flight.hit.unwrap().target, Target::Entity(mob));
        // A box around the launch point (a ridden mount) is ignored.
        let mount: Aabb = [-1., 63., -1., 2., 66., 2.];
        assert_eq!(
            trace(&points, &[mount], 0., wall).hit.unwrap().target,
            Target::Block([0, 64, 6])
        );
        // Nothing in the way: the whole path, no hit.
        let open = trace(&points, &[], 0., |_| Some(false));
        assert_eq!((open.points.len(), open.hit), (2, None));
        // Unreadable ground ends the path without a target.
        let unknown = trace(&points, &[], 0., |p| (p[2] < 4).then_some(false));
        assert!(unknown.hit.is_none() && (unknown.points[1][2] - 4.).abs() < 1e-4);
    }

    #[test]
    fn projectiles_pass_through_fluids_and_plants_only() {
        for name in [
            "air",
            "water",
            "flowing_lava",
            "short_grass",
            "kelp",
            "torch",
        ] {
            assert!(passable(name), "{name}");
        }
        for name in ["stone", "oak_leaves", "glass", "grass_block", "barrier"] {
            assert!(!passable(name), "{name}");
        }
    }
}
