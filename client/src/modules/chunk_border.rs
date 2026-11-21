//! Chunk borders: outlines the 16x16 chunk the camera is in. Drawing only;
//! nothing in the game is modified.

use std::sync::atomic::{AtomicBool, Ordering};

use hudhook::imgui::Ui;

use super::view::segment;
use super::{game, key_or_warn, Module};
use crate::{config, log};

const CHUNK: i32 = 16;
/// How far above and below the camera the border is drawn (blocks).
const REACH: i32 = 32;
/// Spacing of the thin vertical lines along each side, and of the rings.
const SIDE_STEP: i32 = 4;
const RING_STEP: i32 = 8;

const CORNER: [f32; 4] = [0.3, 0.5, 1.0, 0.9];
const GRID: [f32; 4] = [1.0, 0.9, 0.2, 0.55];
/// The same lines around a slime chunk (Overworld only).
const SLIME_CORNER: [f32; 4] = [0.3, 1.0, 0.45, 0.95];
const SLIME_GRID: [f32; 4] = [0.45, 0.95, 0.5, 0.6];

/// Whether the block column (x, z) of the Overworld is in a slime chunk;
/// false in other dimensions or while the dimension is unknown.
pub(crate) fn slime_here(x: f32, z: f32) -> bool {
    matches!(game::world::realm(), Some(game::world::Realm::Overworld))
        && super::slime::at_block(x.floor() as i32, z.floor() as i32)
}

static ACTIVE: AtomicBool = AtomicBool::new(false);

type Line = ([f32; 3], [f32; 3], bool);

/// The lowest multiple of `step` that is >= `value`.
fn ceil_to(value: i32, step: i32) -> i32 {
    value.div_euclid(step) * step + if value.rem_euclid(step) == 0 { 0 } else { step }
}

/// World-space lines outlining the chunk containing `origin`; the bool marks
/// the chunk's corner pillars.
fn lines(origin: [f32; 3]) -> Vec<Line> {
    let floor = |v: f32| v.floor() as i32;
    let (x0, z0) = (
        floor(origin[0]).div_euclid(CHUNK) * CHUNK,
        floor(origin[2]).div_euclid(CHUNK) * CHUNK,
    );
    let (x1, z1) = (x0 + CHUNK, z0 + CHUNK);
    let (bottom, top) = (floor(origin[1]) - REACH, floor(origin[1]) + REACH);
    let point = |x: i32, y: i32, z: i32| [x as f32, y as f32, z as f32];
    let mut out = Vec::new();

    // Pillars: walk the square's perimeter; the four corners are highlighted.
    for i in (0..CHUNK).step_by(SIDE_STEP as usize) {
        for (x, z) in [(x0 + i, z0), (x1, z0 + i), (x1 - i, z1), (x0, z1 - i)] {
            out.push((point(x, bottom, z), point(x, top, z), i == 0));
        }
    }
    // Rings at fixed heights.
    let mut y = ceil_to(bottom, RING_STEP);
    while y <= top {
        let corners = [(x0, z0), (x1, z0), (x1, z1), (x0, z1)];
        for (i, &(ax, az)) in corners.iter().enumerate() {
            let (bx, bz) = corners[(i + 1) % 4];
            out.push((point(ax, y, az), point(bx, y, bz), false));
        }
        y += RING_STEP;
    }
    out
}

/// Draws the border for this frame. Called from the overlay's render loop.
pub fn draw(ui: &Ui) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let Some(camera) = game::camera() else {
        return;
    };
    let size = ui.io().display_size;
    let draw_list = ui.get_background_draw_list();
    let (corner_color, grid_color) = if slime_here(camera.origin[0], camera.origin[2]) {
        (SLIME_CORNER, SLIME_GRID)
    } else {
        (CORNER, GRID)
    };
    for (a, b, corner) in lines(camera.origin) {
        let Some((a, b)) = segment(&camera, size, a, b) else {
            continue;
        };
        let (color, thickness) = if corner {
            (corner_color, 2.0)
        } else {
            (grid_color, 1.0)
        };
        draw_list.add_line(a, b, color).thickness(thickness).build();
    }
}

pub struct ChunkBorder {
    key: u16,
    /// Why the game hook is unavailable, if it is.
    unavailable: Option<String>,
    enabled: bool,
}

impl ChunkBorder {
    pub fn new(cfg: &config::Toggle) -> Self {
        Self {
            key: key_or_warn("chunkborder", &cfg.key),
            unavailable: game::available().err(),
            enabled: false,
        }
    }
}

impl Module for ChunkBorder {
    fn name(&self) -> &str {
        "chunkborder"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if let Some(e) = &self.unavailable {
            log!("chunkborderを利用できません: {e}");
            return;
        }
        if on == self.enabled {
            return;
        }
        if let Err(e) = game::use_camera(on) {
            log!("chunkborder: {e}");
            return;
        }
        ACTIVE.store(on, Ordering::Relaxed);
        self.enabled = on;
    }

    fn status(&self) -> String {
        match &self.unavailable {
            Some(e) => format!("({e})"),
            None => match game::camera() {
                Some(c) if slime_here(c.origin[0], c.origin[2]) => "スライムチャンク".into(),
                _ => String::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_the_chunk_containing_the_camera_including_negative_coordinates() {
        for (origin, x0, z0) in [
            ([5.5, 70.0, 20.2], 0.0, 16.0),
            ([-0.5, 70.0, -16.5], -16.0, -32.0),
            ([16.0, 70.0, -16.0], 16.0, -16.0),
        ] {
            let lines = lines(origin);
            let corners: Vec<_> = lines.iter().filter(|l| l.2).collect();
            assert_eq!(corners.len(), 4, "{origin:?}");
            let mut xz: Vec<(f32, f32)> = corners.iter().map(|l| (l.0[0], l.0[2])).collect();
            xz.sort_by(|a, b| a.partial_cmp(b).unwrap());
            assert_eq!(
                xz,
                vec![
                    (x0, z0),
                    (x0, z0 + 16.0),
                    (x0 + 16.0, z0),
                    (x0 + 16.0, z0 + 16.0)
                ],
                "{origin:?}"
            );
            // Every line stays on the chunk's boundary.
            for (a, b, _) in &lines {
                for p in [a, b] {
                    let on_x = p[0] == x0 || p[0] == x0 + 16.0;
                    let on_z = p[2] == z0 || p[2] == z0 + 16.0;
                    assert!(on_x || on_z, "{p:?} is inside the chunk");
                    assert!((x0..=x0 + 16.0).contains(&p[0]) && (z0..=z0 + 16.0).contains(&p[2]));
                }
            }
        }
    }

    #[test]
    fn pillars_and_rings_cover_the_vertical_reach() {
        let lines = lines([0.5, 70.9, 0.5]);
        // 4 pillars per step of 4 along a side of 16 = 16 pillars.
        let pillars: Vec<_> = lines.iter().filter(|l| l.0[1] != l.1[1]).collect();
        assert_eq!(pillars.len(), 16);
        assert!(pillars.iter().all(|l| l.0[1] == 38.0 && l.1[1] == 102.0));
        // Rings at multiples of 8 within 38..=102: 40, 48, ..., 96 = 8 rings.
        let rings: Vec<_> = lines.iter().filter(|l| l.0[1] == l.1[1]).collect();
        assert_eq!(rings.len(), 8 * 4);
        assert!(rings.iter().all(|l| l.0[1] % 8.0 == 0.0));
    }

    #[test]
    fn ceil_to_rounds_up_for_negative_values_too() {
        assert_eq!(ceil_to(38, 8), 40);
        assert_eq!(ceil_to(40, 8), 40);
        assert_eq!(ceil_to(-70, 8), -64);
        assert_eq!(ceil_to(-64, 8), -64);
    }
}
