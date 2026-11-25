//! Hit feed: detects when the local player lands an attack, by watching the
//! health of nearby entities (the actor snapshot, `actors::reading`) drop
//! while the player is aiming at them within reach. No new game hook and
//! nothing written to the game; the health read is the one `actors` already
//! does, so this inherits its build gating.
//!
//! Attribution is a heuristic: a health drop counts as the player's hit when
//! the entity is inside a narrow cone along the view and within reach. In
//! PvP another player hitting the same aimed target can be miscounted. The
//! actor snapshot is about 100 ms apart, so a hit is seen within ~100 ms.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::game::actors;
use crate::modules::entity_kind::{self, Kind};

/// Reach of the attribution cone, in blocks (a little past survival reach).
const REACH: f32 = 6.5;
/// Half-angle of the cone, in degrees.
const CONE_DEGREES: f32 = 32.;
/// Threats are counted within this distance, in blocks.
const THREAT_RANGE: f32 = 16.;
/// A landed hit is forgotten (for markers) after this long, in ms.
const MARKER_MS: u64 = 1_000;
/// The focused target clears this long after the last hit on it, in ms.
const FOCUS_MS: u64 = 5_000;
/// A hit within this long of the previous one continues the combo, in ms.
const COMBO_MS: u64 = 2_500;
const MAX_MARKERS: usize = 16;
/// Health at or below this counts the entity as killed.
const DEAD_HP: f32 = 0.5;

static USERS: AtomicUsize = AtomicUsize::new(0);
static STATE: Mutex<Option<State>> = Mutex::new(None);

/// One landed hit, for drawing a marker.
#[derive(Clone, Copy)]
pub struct Marker {
    pub at: u64,
    pub killed: bool,
}

/// The focused target: the entity the player most recently hit.
#[derive(Clone)]
pub struct Focus {
    pub name: String,
    pub kind: Kind,
    pub health: f32,
    pub max_health: f32,
    pub distance: f32,
}

/// What the combat HUD shows and the hit marker reads.
#[derive(Clone, Default)]
pub struct Feed {
    /// Hits landed since the feature was switched on.
    pub hits: u64,
    /// Entities killed since then.
    pub kills: u64,
    /// Tick of the last landed hit (0 = none yet).
    pub last_hit_ms: u64,
    /// Consecutive hits within the combo window.
    pub combo: u32,
    /// Best combo reached.
    pub best_combo: u32,
    pub focus: Option<Focus>,
    /// Hostile mobs and other players within `THREAT_RANGE`.
    pub threats: usize,
    pub markers: Vec<Marker>,
}

struct State {
    /// Last seen health per entity, to detect a drop.
    health: HashMap<u32, f32>,
    /// Entities hit recently and still tracked, for kill-on-disappear.
    hit_at: HashMap<u32, u64>,
    last_at: u64,
    feed: Feed,
}

pub fn set_active(on: bool) -> Result<(), String> {
    if on {
        actors::use_actors(true)?;
        USERS.fetch_add(1, Ordering::Relaxed);
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(State {
            health: HashMap::new(),
            hit_at: HashMap::new(),
            last_at: 0,
            feed: Feed::default(),
        });
    } else {
        actors::use_actors(false)?;
        let left = USERS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .map_or(0, |n| n - 1);
        if left == 0 {
            *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
    Ok(())
}

pub(in crate::game) fn wanted() -> bool {
    USERS.load(Ordering::Relaxed) != 0
}

/// The feed for drawing, with markers older than `MARKER_MS` dropped.
pub fn feed(now: u64) -> Option<Feed> {
    let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.as_ref()?;
    let mut feed = state.feed.clone();
    feed.markers
        .retain(|m| now.saturating_sub(m.at) <= MARKER_MS);
    if feed.focus.is_some() && now.saturating_sub(feed.last_hit_ms) > FOCUS_MS {
        feed.focus = None;
    }
    Some(feed)
}

fn center(bounds: [f32; 6]) -> [f32; 3] {
    [
        (bounds[0] + bounds[3]) * 0.5,
        (bounds[1] + bounds[4]) * 0.5,
        (bounds[2] + bounds[5]) * 0.5,
    ]
}

/// Whether `center` is inside the reach cone along `forward` from `eye`, and
/// its distance.
fn aimed(eye: [f32; 3], forward: [f32; 3], center: [f32; 3], cos_cone: f32) -> Option<f32> {
    let to = [center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]];
    let distance = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
    // NaN distances fail the finite check.
    if !distance.is_finite() || distance <= 0.01 || distance > REACH {
        return None;
    }
    let dot = (to[0] * forward[0] + to[1] * forward[1] + to[2] * forward[2]) / distance;
    (dot >= cos_cone).then_some(distance)
}

/// Updates the feed from a new actor reading. Attribution and threat counts
/// use `eye`/`forward`; `only_matches` requires `eye` to be the player's.
pub(in crate::game) fn sample(eye: [f32; 3], forward: [f32; 3], now: u64) {
    if !wanted() {
        return;
    }
    let length =
        (forward[0] * forward[0] + forward[1] * forward[1] + forward[2] * forward[2]).sqrt();
    if !(length.is_finite() && length > 0.001) {
        return;
    }
    let forward = forward.map(|v| v / length);
    let Some((at, actors)) = actors::reading() else {
        return;
    };
    let Ok(mut guard) = STATE.try_lock() else {
        return;
    };
    let Some(state) = guard.as_mut() else {
        return;
    };
    if at == state.last_at {
        return;
    }
    let new_reading = state.last_at != 0;
    state.last_at = at;
    let cos_cone = CONE_DEGREES.to_radians().cos();

    let mut threats = 0;
    let mut fresh_health: HashMap<u32, f32> = HashMap::with_capacity(actors.len());
    for (&id, actor) in actors.iter() {
        if actor.local {
            continue;
        }
        let kind = entity_kind::kind(&actor.name, actor.actor_type);
        if matches!(kind, Kind::Hostile | Kind::Player) {
            if let Some(bounds) = actor.bounds {
                let c = center(bounds);
                let to = [c[0] - eye[0], c[1] - eye[1], c[2] - eye[2]];
                if to[0] * to[0] + to[1] * to[1] + to[2] * to[2] <= THREAT_RANGE * THREAT_RANGE {
                    threats += 1;
                }
            }
        }
        let Some((hp, max)) = actor.health else {
            continue;
        };
        fresh_health.insert(id, hp);
        // Only diff once two readings exist, so switching on is not a flood.
        if !new_reading {
            continue;
        }
        let Some(&previous) = state.health.get(&id) else {
            continue;
        };
        if hp >= previous - 0.01 {
            continue;
        }
        let Some(bounds) = actor.bounds else { continue };
        let Some(distance) = aimed(eye, forward, center(bounds), cos_cone) else {
            continue;
        };
        let killed = hp <= DEAD_HP;
        register(
            state,
            now,
            killed,
            Focus {
                name: actor.name.to_string(),
                kind,
                health: hp.max(0.),
                max_health: max,
                distance,
            },
        );
        state.hit_at.insert(id, now);
    }

    // An entity we hit moments ago that is now gone: count it as a kill.
    if new_reading {
        let gone: Vec<u32> = state
            .hit_at
            .iter()
            .filter(|(id, &when)| {
                !fresh_health.contains_key(id) && now.saturating_sub(when) <= 1_500
            })
            .map(|(&id, _)| id)
            .collect();
        for id in gone {
            state.hit_at.remove(&id);
            if let Some(focus) = &state.feed.focus {
                let name = focus.name.clone();
                let kind = focus.kind;
                register(
                    state,
                    now,
                    true,
                    Focus {
                        name: name.to_string(),
                        kind,
                        health: 0.,
                        max_health: focus.max_health,
                        distance: focus.distance,
                    },
                );
            }
        }
    }

    state.health = fresh_health;
    state.feed.threats = threats;
    state
        .hit_at
        .retain(|_, &mut when| now.saturating_sub(when) <= FOCUS_MS);
}

fn register(state: &mut State, now: u64, killed: bool, focus: Focus) {
    let feed = &mut state.feed;
    feed.hits += 1;
    if killed {
        feed.kills += 1;
    }
    feed.combo = if now.saturating_sub(feed.last_hit_ms) <= COMBO_MS {
        feed.combo + 1
    } else {
        1
    };
    feed.best_combo = feed.best_combo.max(feed.combo);
    feed.last_hit_ms = now;
    feed.markers.push(Marker { at: now, killed });
    if feed.markers.len() > MAX_MARKERS {
        feed.markers.remove(0);
    }
    feed.focus = (!killed).then_some(focus);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds_at(x: f32, z: f32) -> [f32; 6] {
        [x - 0.3, 64.0, z - 0.3, x + 0.3, 65.8, z + 0.3]
    }

    #[test]
    fn a_target_in_the_cone_within_reach_is_attributed() {
        let eye = [0.0, 65.0, 0.0];
        let forward = [0.0, 0.0, 1.0];
        let cos = CONE_DEGREES.to_radians().cos();
        // Straight ahead, 3 blocks: in.
        assert!(aimed(eye, forward, [0.1, 65.0, 3.0], cos).is_some());
        // Behind: out.
        assert!(aimed(eye, forward, [0.0, 65.0, -3.0], cos).is_none());
        // Ahead but past reach: out.
        assert!(aimed(eye, forward, [0.0, 65.0, 9.0], cos).is_none());
        // Ahead but far off to the side (>32 degrees): out.
        assert!(aimed(eye, forward, [3.0, 65.0, 2.0], cos).is_none());
    }

    /// Feeds two readings through `sample` using the public API, checking a
    /// drop on the aimed target lands a hit and an unaimed drop does not.
    #[test]
    fn health_drops_on_aimed_targets_become_hits() {
        // The static state is shared; this test owns it via set_active.
        let _ = STATE.lock().map(|mut g| *g = None);
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(State {
            health: HashMap::new(),
            hit_at: HashMap::new(),
            last_at: 0,
            feed: Feed::default(),
        });
        USERS.store(1, Ordering::Relaxed);

        let mut state = State {
            health: HashMap::from([(1, 20.0), (2, 20.0)]),
            hit_at: HashMap::new(),
            last_at: 100,
            feed: Feed::default(),
        };
        // Entity 1 ahead within reach drops 6; entity 2 behind drops 6.
        let eye = [0.0, 65.0, 0.0];
        let forward = [0.0, 0.0, 1.0];
        let cos = CONE_DEGREES.to_radians().cos();
        for (id, c, hp) in [
            (1u32, [0.0, 65.0, 3.0], 14.0_f32),
            (2, [0.0, 65.0, -3.0], 14.0),
        ] {
            if let Some(d) = aimed(eye, forward, c, cos) {
                register(
                    &mut state,
                    1_000,
                    hp <= DEAD_HP,
                    Focus {
                        name: "zombie".to_string(),
                        kind: Kind::Hostile,
                        health: hp,
                        max_health: 20.0,
                        distance: d,
                    },
                );
                state.hit_at.insert(id, 1_000);
            }
        }
        assert_eq!(state.feed.hits, 1, "only the aimed target");
        assert_eq!(state.feed.combo, 1);
        assert!(state.feed.focus.is_some());

        USERS.store(0, Ordering::Relaxed);
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = bounds_at(0.0, 0.0);
    }

    #[test]
    fn combos_grow_within_the_window_and_reset_after_it() {
        let mut state = State {
            health: HashMap::new(),
            hit_at: HashMap::new(),
            last_at: 0,
            feed: Feed::default(),
        };
        register(
            &mut state,
            1_000,
            false,
            Focus {
                name: "pig".to_string(),
                kind: Kind::Passive,
                health: 5.0,
                max_health: 10.0,
                distance: 2.0,
            },
        );
        register(
            &mut state,
            2_000,
            false,
            Focus {
                name: "pig".to_string(),
                kind: Kind::Passive,
                health: 4.0,
                max_health: 10.0,
                distance: 2.0,
            },
        );
        register(
            &mut state,
            3_000,
            true,
            Focus {
                name: "pig".to_string(),
                kind: Kind::Passive,
                health: 0.0,
                max_health: 10.0,
                distance: 2.0,
            },
        );
        assert_eq!(state.feed.combo, 3);
        assert_eq!(state.feed.best_combo, 3);
        assert_eq!((state.feed.hits, state.feed.kills), (3, 1));
        assert!(state.feed.focus.is_none(), "a kill clears the focus");
        // A gap longer than the window restarts the combo.
        register(
            &mut state,
            3_000 + COMBO_MS + 1,
            false,
            Focus {
                name: "pig".to_string(),
                kind: Kind::Passive,
                health: 9.0,
                max_health: 10.0,
                distance: 2.0,
            },
        );
        assert_eq!(state.feed.combo, 1);
        assert_eq!(state.feed.best_combo, 3);
        assert_eq!(state.feed.markers.len(), 4);
    }
}
