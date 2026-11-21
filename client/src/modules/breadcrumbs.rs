//! Breadcrumbs: draws the path the player has walked, to find the way back.
//! Drawing only; positions come from the SDK's player snapshot.

use std::collections::VecDeque;

use hudhook::imgui::Ui;

use super::view::segment;
use super::{game, key_or_warn, Module, Param, Tick};
use crate::{api, config, log};

const MIN_POINTS: f64 = 50.0;
const MAX_POINTS: f64 = 3000.0;
/// The position is sampled this often (ms).
const SAMPLE_MS: u64 = 100;
/// A new point is added once the player has moved this far (blocks).
const MIN_STEP: f32 = 0.4;
/// A jump longer than this is a teleport, respawn or dimension change: the
/// old path no longer leads here, so it is dropped.
const TELEPORT: f32 = 48.0;
/// The line is drawn this far above the feet.
const LIFT: f32 = 0.15;

const COLOR: [f32; 4] = [0.3, 0.9, 1.0, 0.85];

/// The recorded path: the newest `max` points, and how far it has gone.
struct Trail {
    points: VecDeque<[f32; 3]>,
    max: usize,
    /// Total distance walked since the trail was started (blocks).
    walked: f32,
}

impl Trail {
    fn new(max: usize) -> Self {
        Self {
            points: VecDeque::new(),
            max,
            walked: 0.0,
        }
    }

    fn clear(&mut self) {
        self.points.clear();
        self.walked = 0.0;
    }

    fn set_max(&mut self, max: usize) {
        self.max = max;
        while self.points.len() > self.max {
            self.points.pop_front();
        }
    }

    /// Records a position if the player has moved since the last point.
    fn push(&mut self, position: [f32; 3]) {
        if !position.iter().all(|v| v.is_finite()) {
            return;
        }
        if let Some(last) = self.points.back() {
            let distance = (0..3)
                .map(|k| (position[k] - last[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            if distance < MIN_STEP {
                return;
            }
            if distance > TELEPORT {
                self.points.clear();
            } else {
                self.walked += distance;
            }
        }
        self.points.push_back(position);
        while self.points.len() > self.max {
            self.points.pop_front();
        }
    }
}

pub struct Breadcrumbs {
    key: u16,
    /// Why the game hook is unavailable, if it is.
    unavailable: Option<String>,
    enabled: bool,
    trail: Trail,
    sampled_at: u64,
}

fn clamp_points(points: f64) -> usize {
    if points.is_finite() {
        points.clamp(MIN_POINTS, MAX_POINTS).round() as usize
    } else {
        600
    }
}

impl Breadcrumbs {
    pub fn new(cfg: &config::Breadcrumbs) -> Self {
        Self {
            key: key_or_warn("breadcrumbs", &cfg.key),
            unavailable: game::available().err(),
            enabled: false,
            trail: Trail::new(clamp_points(cfg.points)),
            sampled_at: 0,
        }
    }
}

impl Module for Breadcrumbs {
    fn name(&self) -> &str {
        "breadcrumbs"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        if let Some(e) = &self.unavailable {
            log!("breadcrumbsを利用できません: {e}");
            return;
        }
        // Drawing needs the camera.
        if let Err(e) = game::use_camera(on) {
            log!("breadcrumbs: {e}");
            return;
        }
        // Each time it is switched on the path starts from here.
        self.trail.clear();
        self.enabled = on;
    }

    fn tick(&mut self, tick: &Tick) {
        if !self.enabled || tick.now_ms.saturating_sub(self.sampled_at) < SAMPLE_MS {
            return;
        }
        self.sampled_at = tick.now_ms;
        if let Some(player) = api::game().ok().and_then(|g| g.player().ok()) {
            let p = player.position;
            self.trail.push([p.x, p.y, p.z]);
        }
    }

    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "points",
            value: self.trail.max as f64,
            min: MIN_POINTS,
            max: MAX_POINTS,
        })
    }

    fn set_param(&mut self, value: f64) {
        self.trail.set_max(clamp_points(value));
    }

    fn status(&self) -> String {
        match &self.unavailable {
            Some(e) => format!("({e})"),
            None => format!("{:.0} m", self.trail.walked),
        }
    }

    fn draw(&mut self, ui: &Ui) {
        if self.trail.points.len() < 2 {
            return;
        }
        let Some(camera) = game::camera() else {
            return;
        };
        let size = ui.io().display_size;
        let draw_list = ui.get_background_draw_list();
        let lifted = |p: &[f32; 3]| [p[0], p[1] + LIFT, p[2]];
        let mut previous = lifted(&self.trail.points[0]);
        for point in self.trail.points.iter().skip(1) {
            let current = lifted(point);
            if let Some((a, b)) = segment(&camera, size, previous, current) {
                draw_list.add_line(a, b, COLOR).thickness(2.0).build();
            }
            previous = current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_movement_but_not_standing_still() {
        let mut trail = Trail::new(100);
        trail.push([0.0, 64.0, 0.0]);
        trail.push([0.1, 64.0, 0.0]); // jitter: ignored
        trail.push([0.0, 64.0, 0.0]);
        assert_eq!(trail.points.len(), 1);
        trail.push([1.0, 64.0, 0.0]);
        trail.push([1.0, 64.0, 2.0]);
        assert_eq!(trail.points.len(), 3);
        assert!((trail.walked - 3.0).abs() < 1e-5);
        trail.push([f32::NAN, 0.0, 0.0]);
        assert_eq!(trail.points.len(), 3);
    }

    #[test]
    fn keeps_only_the_newest_points_but_the_whole_distance() {
        let mut trail = Trail::new(5);
        for i in 0..20 {
            trail.push([i as f32, 64.0, 0.0]);
        }
        assert_eq!(trail.points.len(), 5);
        assert_eq!(trail.points.front(), Some(&[15.0, 64.0, 0.0]));
        assert_eq!(trail.points.back(), Some(&[19.0, 64.0, 0.0]));
        assert!((trail.walked - 19.0).abs() < 1e-4);
        trail.set_max(2);
        assert_eq!(trail.points.len(), 2);
        assert_eq!(trail.points.front(), Some(&[18.0, 64.0, 0.0]));
    }

    #[test]
    fn a_teleport_starts_a_new_path_without_a_line_across_the_world() {
        let mut trail = Trail::new(100);
        trail.push([0.0, 64.0, 0.0]);
        trail.push([5.0, 64.0, 0.0]);
        trail.push([1000.0, 70.0, -400.0]);
        assert_eq!(trail.points.len(), 1);
        assert_eq!(trail.points[0], [1000.0, 70.0, -400.0]);
        // The jump itself is not counted as walking.
        assert!((trail.walked - 5.0).abs() < 1e-5);
        trail.clear();
        assert!(trail.points.is_empty() && trail.walked == 0.0);
    }

    #[test]
    fn point_limit_is_clamped() {
        assert_eq!(clamp_points(1.0), 50);
        assert_eq!(clamp_points(1.0e9), 3000);
        assert_eq!(clamp_points(600.4), 600);
        assert_eq!(clamp_points(f64::NAN), 600);
    }
}
