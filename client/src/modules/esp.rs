//! ESP: outlines every entity's bounding box in the overlay, through walls.
//!
//! The boxes and the camera come from `game.rs`; this module only projects
//! them onto the screen. Nothing in the game is modified.

use super::settings::{Detail, Value};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use hudhook::imgui::Ui;

use super::entity_kind::Kind;
use super::game::{self, Aabb, Camera};
use super::view::{dot, project};
use super::{key_or_warn, Module};
use crate::{config, log};

const THICKNESS: f32 = 1.5;
/// Boxes lower than this (dropped items, arrows, orbs) count as "small".
const SMALL_HEIGHT: f32 = 0.3;

/// Draw settings, shared with the render thread.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SHOW_SMALL: AtomicBool = AtomicBool::new(false);
static TRACERS: AtomicBool = AtomicBool::new(false);
/// Which kinds get a tracer line (bits of `TRACER_KINDS`), while tracers are on.
static TRACER_MASK: AtomicU32 = AtomicU32::new(0b01111);
/// Tracer switches: option id, label, and the entity kind each covers.
const TRACER_KINDS: [(&str, &str, Kind); 5] = [
    ("tracer_players", "線: プレイヤー", Kind::Player),
    ("tracer_hostile", "線: 敵対モブ", Kind::Hostile),
    ("tracer_neutral", "線: 中立モブ", Kind::Neutral),
    ("tracer_passive", "線: 友好モブ（味方）", Kind::Passive),
    ("tracer_other", "線: その他（アイテムなど）", Kind::Other),
];

/// Whether an entity of `kind` gets a tracer under `mask`.
fn traced(mask: u32, kind: Kind) -> bool {
    TRACER_KINDS
        .iter()
        .position(|&(_, _, k)| k == kind)
        .is_some_and(|bit| mask & (1 << bit) != 0)
}
/// f32 bits; 0.0 = unlimited.
static MAX_DISTANCE: AtomicU32 = AtomicU32::new(0);
/// Boxes drawn in the last frame, for the status line.
static DRAWN: AtomicUsize = AtomicUsize::new(0);
static FILTER: AtomicU32 = AtomicU32::new(15);
static COLORS: [AtomicU32; 4] = [
    AtomicU32::new(0x40c8ff),
    AtomicU32::new(0x50e664),
    AtomicU32::new(0xff4040),
    AtomicU32::new(0xc8c8c8),
];
const GROUPS: [(&str, &str); 4] = [
    ("players", "Players"),
    ("passive", "Passive / neutral"),
    ("hostile", "Hostile"),
    ("other", "Other / unknown"),
];
const COMPONENTS: [[&str; 3]; 4] = [
    ["player_r", "player_g", "player_b"],
    ["passive_r", "passive_g", "passive_b"],
    ["hostile_r", "hostile_g", "hostile_b"],
    ["other_r", "other_g", "other_b"],
];
/// ESP's colour group of an actor: players, passive and neutral mobs,
/// hostile mobs, others (see `entity_kind`).
fn category(name: &str, actor_type: u32) -> usize {
    match super::entity_kind::kind(name, actor_type) {
        Kind::Player => 0,
        Kind::Passive | Kind::Neutral => 1,
        Kind::Hostile => 2,
        Kind::Other => 3,
    }
}

/// The box's 8 corners on screen, or None if any is behind the camera.
fn corners(camera: &Camera, size: [f32; 2], aabb: &Aabb) -> Option<[[f32; 2]; 8]> {
    let mut out = [[0.0; 2]; 8];
    for (i, corner) in out.iter_mut().enumerate() {
        let pick = |axis: usize| aabb[if i >> axis & 1 == 1 { axis + 3 } else { axis }];
        *corner = project(camera, size, [pick(0), pick(1), pick(2)])?;
    }
    Some(out)
}

/// A conservative screen-space rejection, including stroke/antialias margin.
/// Keep boxes enclosing the viewport and let the renderer handle nonfinite data.
fn outside(points: &[[f32; 2]; 8], size: [f32; 2]) -> bool {
    (0..2).any(|axis| {
        points.iter().all(|p| p[axis] < -THICKNESS - 2.)
            || points.iter().all(|p| p[axis] > size[axis] + THICKNESS + 2.)
    })
}

/// Draws the boxes for this frame. Called from the overlay's render loop.
pub fn draw(ui: &Ui) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let (Some(camera), Some(boxes)) = (game::camera(), game::shared_boxes()) else {
        DRAWN.store(0, Ordering::Relaxed);
        return;
    };
    let size = ui.io().display_size;
    let show_small = SHOW_SMALL.load(Ordering::Relaxed);
    let tracers = TRACERS.load(Ordering::Relaxed);
    let tracer_mask = TRACER_MASK.load(Ordering::Relaxed);
    let max_distance = f32::from_bits(MAX_DISTANCE.load(Ordering::Relaxed));
    let draw_list = ui.get_background_draw_list();
    let mut drawn = 0;
    let facts = game::actors::actors();
    let filter = FILTER.load(Ordering::Relaxed);
    let colors = COLORS.each_ref().map(|rgb| {
        let rgb = rgb.load(Ordering::Relaxed);
        [
            ((rgb >> 16) & 255) as f32 / 255.,
            ((rgb >> 8) & 255) as f32 / 255.,
            (rgb & 255) as f32 / 255.,
            0.9,
        ]
    });
    for (id, aabb) in boxes.iter() {
        if !show_small && aabb[4] - aabb[1] < SMALL_HEIGHT {
            continue;
        }
        if max_distance > 0.0 {
            let center = std::array::from_fn(|k| (aabb[k] + aabb[k + 3]) * 0.5 - camera.origin[k]);
            if dot(center, center) > max_distance * max_distance {
                continue;
            }
        }
        let actor = facts.as_ref().and_then(|m| m.get(id));
        let group = actor.map_or(3, |a| category(&a.name, a.actor_type));
        let kind = actor.map_or(Kind::Other, |a| {
            super::entity_kind::kind(&a.name, a.actor_type)
        });
        if filter & (1 << group) == 0 {
            continue;
        }
        let color = colors[group];
        let Some(points) = corners(&camera, size, aabb) else {
            continue;
        };
        if !outside(&points, size) {
            for (a, b) in super::view::BOX_EDGES {
                draw_list
                    .add_line(points[a], points[b], color)
                    .thickness(THICKNESS)
                    .build();
            }
        }
        if tracers && traced(tracer_mask, kind) {
            // A line from the bottom centre of the screen to the box's centre.
            let center = [
                (aabb[0] + aabb[3]) * 0.5,
                (aabb[1] + aabb[4]) * 0.5,
                (aabb[2] + aabb[5]) * 0.5,
            ];
            if let Some(point) = project(&camera, size, center) {
                draw_list
                    .add_line([size[0] * 0.5, size[1]], point, color)
                    .thickness(1.0)
                    .build();
            }
        }
        drawn += 1;
    }
    DRAWN.store(drawn, Ordering::Relaxed);
}

pub struct Esp {
    key: u16,
    /// Why the game hook is unavailable, if it is.
    unavailable: Option<String>,
    enabled: bool,
}

impl Esp {
    pub fn new(cfg: &config::Esp) -> Self {
        FILTER.store(
            u32::from(cfg.players)
                | (u32::from(cfg.passive) << 1)
                | (u32::from(cfg.hostile) << 2)
                | (u32::from(cfg.other) << 3),
            Ordering::Relaxed,
        );
        for (i, c) in [
            cfg.player_color,
            cfg.passive_color,
            cfg.hostile_color,
            cfg.other_color,
        ]
        .iter()
        .enumerate()
        {
            COLORS[i].store(
                (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32,
                Ordering::Relaxed,
            );
        }
        SHOW_SMALL.store(cfg.show_small, Ordering::Relaxed);
        TRACERS.store(cfg.tracers, Ordering::Relaxed);
        let mask = [
            cfg.tracer_players,
            cfg.tracer_hostile,
            cfg.tracer_neutral,
            cfg.tracer_passive,
            cfg.tracer_other,
        ]
        .iter()
        .enumerate()
        .fold(0, |m, (bit, &on)| m | (u32::from(on) << bit));
        TRACER_MASK.store(mask, Ordering::Relaxed);
        let max_distance = if cfg.max_distance.is_finite() {
            cfg.max_distance.max(0.0)
        } else {
            0.0
        };
        MAX_DISTANCE.store(max_distance.to_bits(), Ordering::Relaxed);
        Self {
            key: key_or_warn("esp", &cfg.key),
            unavailable: game::available().err(),
            enabled: false,
        }
    }
}

impl Module for Esp {
    fn name(&self) -> &str {
        "esp"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn options(&self) -> Vec<Detail> {
        let mut details = vec![
            Detail::boolean(
                "show_small",
                "Small entities",
                "Include dropped items, arrows and experience orbs.",
                SHOW_SMALL.load(Ordering::Relaxed),
            ),
            Detail::boolean(
                "tracers",
                "Tracers",
                "Draw lines from the bottom of the screen to targets.",
                TRACERS.load(Ordering::Relaxed),
            ),
            Detail::number(
                "max_distance",
                "Max distance (0 = unlimited)",
                "Hide entity outlines beyond this distance in blocks.",
                f32::from_bits(MAX_DISTANCE.load(Ordering::Relaxed)) as f64,
                0.,
                512.,
            ),
        ];
        // Which kinds get a line (while tracers are on).
        let mask = TRACER_MASK.load(Ordering::Relaxed);
        for (bit, &(id, label, _)) in TRACER_KINDS.iter().enumerate() {
            details.push(Detail::boolean(
                id,
                label,
                "トレーサーがONのとき、この種類の対象へ線を引きます。",
                mask & (1 << bit) != 0,
            ));
        }
        for (i, (id, label)) in GROUPS.iter().enumerate() {
            details.push(Detail::boolean(
                id,
                label,
                "Classified by vanilla identifier. Unknown/add-on entities use Other.",
                FILTER.load(Ordering::Relaxed) & (1 << i) != 0,
            ));
            let c = COLORS[i].load(Ordering::Relaxed);
            for (j, id) in COMPONENTS[i].iter().enumerate() {
                details.push(Detail::integer(
                    id,
                    id,
                    "RGB channel (0-255).",
                    ((c >> (16 - j * 8)) & 255) as i32,
                    0,
                    255,
                ));
            }
        }
        details
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        for (i, (group, _)) in GROUPS.iter().enumerate() {
            if id == *group {
                let bit = 1 << i;
                if value.boolean()? {
                    FILTER.fetch_or(bit, Ordering::Relaxed);
                } else {
                    FILTER.fetch_and(!bit, Ordering::Relaxed);
                }
                return Ok(());
            }
            for (j, component) in COMPONENTS[i].iter().enumerate() {
                if id == *component {
                    let shift = 16 - j * 8;
                    let v = (value.number()?.clamp(0., 255.) as u32) << shift;
                    let _ = COLORS[i].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                        Some((c & !(255 << shift)) | v)
                    });
                    return Ok(());
                }
            }
        }
        match id {
            "show_small" => SHOW_SMALL.store(value.boolean()?, Ordering::Relaxed),
            "tracers" => TRACERS.store(value.boolean()?, Ordering::Relaxed),
            id if TRACER_KINDS.iter().any(|&(k, _, _)| k == id) => {
                let bit = TRACER_KINDS.iter().position(|&(k, _, _)| k == id).unwrap();
                let on = value.boolean()?;
                let _ = TRACER_MASK.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |m| {
                    Some(if on { m | (1 << bit) } else { m & !(1 << bit) })
                });
            }
            "max_distance" => MAX_DISTANCE.store(
                (value.number()?.clamp(0., 512.) as f32).to_bits(),
                Ordering::Relaxed,
            ),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if let Some(e) = &self.unavailable {
            log!("espを利用できません: {e}");
            return;
        }
        if on == self.enabled {
            return;
        }
        if on {
            if let Err(e) = game::actors::use_actors(true) {
                log!("esp: {e}");
                return;
            }
        }
        if let Err(e) = game::use_scene(on) {
            if on {
                let _ = game::actors::use_actors(false);
            }
            log!("esp: {e}");
            return;
        }
        if !on {
            let _ = game::actors::use_actors(false);
        }
        ACTIVE.store(on, Ordering::Relaxed);
        self.enabled = on;
    }

    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        if !self.enabled {
            return String::new();
        }
        format!("{}個の枠", DRAWN.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracers_follow_the_switch_for_each_kind() {
        // Players, hostile, neutral and passive on; other off (the default).
        let mask = 0b01111;
        for kind in [Kind::Player, Kind::Hostile, Kind::Neutral, Kind::Passive] {
            assert!(traced(mask, kind), "{kind:?}");
        }
        assert!(!traced(mask, Kind::Other));
        // Only hostile mobs.
        assert!(traced(0b00010, Kind::Hostile));
        assert!(!traced(0b00010, Kind::Neutral));
        assert!(!traced(0b00010, Kind::Player));
        assert!(!traced(0, Kind::Hostile));
        // Each switch is its own option, and toggling one leaves the others.
        let mut esp = Esp::new(&config::Esp::default());
        let ids: Vec<&str> = esp.options().iter().map(|d| d.id).collect();
        for (id, _, _) in TRACER_KINDS {
            assert!(ids.contains(&id), "{id}");
        }
        esp.set_option("tracer_neutral", &Value::Bool(false))
            .unwrap();
        let now = TRACER_MASK.load(Ordering::Relaxed);
        assert!(!traced(now, Kind::Neutral));
        assert!(traced(now, Kind::Hostile) && traced(now, Kind::Passive));
        esp.set_option("tracer_neutral", &Value::Bool(true))
            .unwrap();
        assert!(traced(TRACER_MASK.load(Ordering::Relaxed), Kind::Neutral));
    }
    #[test]
    fn screen_rejection_keeps_border_crossings_enclosing_boxes_and_tracer_targets() {
        let size = [200., 100.];
        assert!(!outside(&[[100., 50.]; 8], size));
        assert!(outside(&[[-5., 50.]; 8], size));
        assert!(!outside(&[[-3., 50.]; 8], size));
        assert!(outside(&[[205., 50.]; 8], size));
        let mut enclosing = [[-20., -20.]; 8];
        enclosing[7] = [220., 120.];
        assert!(!outside(&enclosing, size));
        let mut crossing = [[-20., 50.]; 8];
        crossing[7] = [20., 50.];
        assert!(!outside(&crossing, size));
    }
    #[test]
    fn known_and_unknown_categories() {
        assert_eq!(category("minecraft:player", 0), 0);
        assert_eq!(category("cow", 0), 1);
        assert_eq!(category("creeper", 0), 2);
        assert_eq!(category("custom_beast", 0), 3);
        assert_eq!(category("item", 0), 3);
    }
}
