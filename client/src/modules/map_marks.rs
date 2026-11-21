//! Marks shared by the minimap and the world map: saved places (waypoints
//! and death points), arrows to places beyond the map's edge, and slime
//! chunk tints. Drawing only.

use std::collections::HashMap;

use hudhook::imgui::DrawListMut;

use super::slime;

pub const WAYPOINT: [f32; 4] = [0.3, 0.9, 1., 1.];
pub const DEATH: [f32; 4] = [0.95, 0.35, 0.75, 1.];
const OUTLINE: [f32; 4] = [0., 0., 0., 0.9];
const SLIME_FILL: [f32; 4] = [0.35, 1., 0.45, 0.2];
const SLIME_EDGE: [f32; 4] = [0.35, 1., 0.45, 0.55];

/// A diamond at `p` (screen), `size` pixels from centre to tip.
pub fn place(draw: &DrawListMut, p: [f32; 2], size: f32, death: bool) {
    let color = if death { DEATH } else { WAYPOINT };
    let tips = |s: f32| {
        [
            [p[0], p[1] - s],
            [p[0] + s, p[1]],
            [p[0], p[1] + s],
            [p[0] - s, p[1]],
        ]
    };
    let [a, b, c, d] = tips(size + 1.5);
    draw.add_triangle(a, b, c, OUTLINE).filled(true).build();
    draw.add_triangle(a, c, d, OUTLINE).filled(true).build();
    let [a, b, c, d] = tips(size);
    draw.add_triangle(a, b, c, color).filled(true).build();
    draw.add_triangle(a, c, d, color).filled(true).build();
}

/// Where the line from `from` (inside the rectangle `min..max`) towards
/// `to` (outside it) meets the rectangle shrunk by `inset`, and the line's
/// unit direction. None when `to` is inside or equals `from`.
pub fn edge_point(
    from: [f32; 2],
    to: [f32; 2],
    min: [f32; 2],
    max: [f32; 2],
    inset: f32,
) -> Option<([f32; 2], [f32; 2])> {
    let inside = |p: [f32; 2]| (0..2).all(|k| p[k] >= min[k] && p[k] <= max[k]);
    let d = [to[0] - from[0], to[1] - from[1]];
    let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
    // NaN lengths fail `is_finite`.
    if inside(to) || !length.is_finite() || length <= 1e-3 {
        return None;
    }
    let unit = [d[0] / length, d[1] / length];
    // The nearest of the four inset edges along the direction.
    let t = (0..2)
        .filter(|&k| unit[k] != 0.)
        .map(|k| {
            let edge = if unit[k] > 0. {
                max[k] - inset
            } else {
                min[k] + inset
            };
            (edge - from[k]) / unit[k]
        })
        .fold(f32::INFINITY, f32::min)
        .max(0.);
    Some(([from[0] + unit[0] * t, from[1] + unit[1] * t], unit))
}

/// A small arrow at `p` pointing along `unit`.
pub fn arrow(draw: &DrawListMut, p: [f32; 2], unit: [f32; 2], death: bool) {
    let color = if death { DEATH } else { WAYPOINT };
    let side = [-unit[1], unit[0]];
    let at = |a: f32, b: f32| {
        [
            p[0] + unit[0] * a + side[0] * b,
            p[1] + unit[1] * a + side[1] * b,
        ]
    };
    draw.add_triangle(at(6.5, 0.), at(-4.5, 5.5), at(-4.5, -5.5), OUTLINE)
        .filled(true)
        .build();
    draw.add_triangle(at(5., 0.), at(-3.5, 4.), at(-3.5, -4.), color)
        .filled(true)
        .build();
}

/// Slime chunks remembered by chunk position, so maps do not rerun the
/// generator for every chunk every frame. Bounded: cleared when large.
#[derive(Default)]
pub struct SlimeCache(HashMap<(i32, i32), bool>);

impl SlimeCache {
    pub fn get(&mut self, chunk: (i32, i32)) -> bool {
        if self.0.len() >= 65_536 {
            self.0.clear();
        }
        *self
            .0
            .entry(chunk)
            .or_insert_with(|| slime::is_slime_chunk(chunk.0, chunk.1))
    }
}

/// Tints the slime chunks that overlap the world area `from..to` (block
/// X/Z), with `to_screen` mapping world X/Z to the screen.
pub fn slime_chunks(
    draw: &DrawListMut,
    cache: &mut SlimeCache,
    from: [f32; 2],
    to: [f32; 2],
    to_screen: impl Fn(f32, f32) -> [f32; 2],
) {
    let chunk = |v: f32| (v.floor() as i32).div_euclid(16);
    for cz in chunk(from[1])..=chunk(to[1]) {
        for cx in chunk(from[0])..=chunk(to[0]) {
            if !cache.get((cx, cz)) {
                continue;
            }
            let a = to_screen((cx * 16) as f32, (cz * 16) as f32);
            let b = to_screen((cx * 16 + 16) as f32, (cz * 16 + 16) as f32);
            draw.add_rect(a, b, SLIME_FILL).filled(true).build();
            draw.add_rect(a, b, SLIME_EDGE).build();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_arrows_sit_on_the_inset_border_towards_the_place() {
        let (min, max) = ([0., 0.], [100., 100.]);
        // Straight east and north-west of the centre.
        let (p, u) = edge_point([50., 50.], [500., 50.], min, max, 6.).unwrap();
        assert_eq!((p, u), ([94., 50.], [1., 0.]));
        let (p, _) = edge_point([50., 50.], [-50., -50.], min, max, 6.).unwrap();
        assert!(
            (p[0] - 6.).abs() < 1e-4 && (p[1] - 6.).abs() < 1e-4,
            "{p:?}"
        );
        // Steeper than the diagonal: meets the top edge first.
        let (p, _) = edge_point([50., 50.], [60., -400.], min, max, 6.).unwrap();
        assert!(
            (p[1] - 6.).abs() < 1e-4 && p[0] > 50. && p[0] < 52.,
            "{p:?}"
        );
        // Inside, or no direction: no arrow.
        assert_eq!(edge_point([50., 50.], [70., 20.], min, max, 6.), None);
        assert_eq!(edge_point([50., 50.], [50., 50.], min, max, 6.), None);
    }

    #[test]
    fn slime_cache_agrees_with_the_generator() {
        let mut cache = SlimeCache::default();
        for x in -20..20 {
            for z in -20..20 {
                assert_eq!(cache.get((x, z)), slime::is_slime_chunk(x, z));
                assert_eq!(cache.get((x, z)), slime::is_slime_chunk(x, z));
            }
        }
    }
}
