//! Fixed-size frustum clipping, translucent tiles and non-overlapping number badges.
use crate::modules::{
    game::{blocks::lighting::Cell, Camera},
    radar::origin,
    view::{dot, project, segment},
};
use crate::{config, theme};
use hudhook::imgui::Ui;
type Color = [f32; 4];
fn color(block: u8, effective: u8, threshold: i32) -> Option<Color> {
    if block as i32 > threshold {
        None
    } else if effective as i32 <= threshold {
        Some(theme::DANGER)
    } else {
        Some(theme::WARNING)
    }
}
#[derive(Clone, Copy)]
struct Polygon {
    points: [[f32; 3]; 12],
    len: usize,
}
impl Polygon {
    fn clip(self, plane: impl Fn([f32; 3]) -> f32) -> Self {
        let mut out = Self {
            points: [[0.; 3]; 12],
            len: 0,
        };
        if self.len == 0 {
            return out;
        }
        let mut a = self.points[self.len - 1];
        let mut da = plane(a);
        let mut push = |p| {
            if out.len < out.points.len() {
                out.points[out.len] = p;
                out.len += 1;
            }
        };
        for &b in &self.points[..self.len] {
            let db = plane(b);
            if (da >= 0.) != (db >= 0.) {
                let t = da / (da - db);
                push(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
            }
            if db >= 0. {
                push(b);
            }
            a = b;
            da = db;
        }
        out
    }
}
fn project_floor(camera: &Camera, size: [f32; 2], corners: [[f32; 3]; 4]) -> Polygon {
    let mut p = Polygon {
        points: [[0.; 3]; 12],
        len: 4,
    };
    for (out, point) in p.points.iter_mut().zip(corners) {
        let rel = std::array::from_fn(|i| point[i] - camera.origin[i]);
        *out = [
            dot(rel, camera.right) * camera.scale[0],
            dot(rel, camera.up) * camera.scale[1],
            -dot(rel, camera.back),
        ];
    }
    if p.points[..4].iter().flatten().any(|v| !v.is_finite())
        || size.iter().any(|v| !v.is_finite() || *v <= 0.)
    {
        p.len = 0;
        return p;
    }
    p = p
        .clip(|v| v[2] - 0.1)
        .clip(|v| v[0] + v[2])
        .clip(|v| v[2] - v[0])
        .clip(|v| v[1] + v[2])
        .clip(|v| v[2] - v[1]);
    for point in &mut p.points[..p.len] {
        *point = [
            (size[0] * 0.5 * (1. + point[0] / point[2])).clamp(0., size[0]),
            (size[1] * 0.5 * (1. - point[1] / point[2])).clamp(0., size[1]),
            0.,
        ];
    }
    p
}
#[derive(Clone, Copy, Debug)]
struct Rect {
    min: [f32; 2],
    max: [f32; 2],
}
impl Rect {
    fn overlaps(self, other: Self) -> bool {
        (0..2).all(|i| self.min[i] < other.max[i] + 3. && self.max[i] > other.min[i] - 3.)
    }
    fn fits(self, size: [f32; 2]) -> bool {
        (0..2).all(|i| self.min[i] >= 0. && self.max[i] <= size[i])
    }
}
struct Badge {
    bounds: Rect,
    text: String,
    color: Color,
}
pub(super) fn draw(
    ui: &Ui,
    camera: &Camera,
    cells: &[Cell],
    feet: [f32; 3],
    cfg: &config::LightOverlay,
) {
    let screen = ui.io().display_size;
    let draw = ui.get_background_draw_list();
    let mut badges: Vec<Badge> = Vec::with_capacity(cfg.max_labels as usize);
    let mut counts = [0usize; 2];
    for cell in cells {
        let Some(mut tint) = color(cell.block, cell.effective, cfg.threshold) else {
            continue;
        };
        let [x, y, z] = cell.position.map(|v| v as f32);
        let d2 = (x + 0.5 - feet[0]).powi(2) + (z + 0.5 - feet[2]).powi(2);
        if d2 > (cfg.radius + 1).pow(2) as f32 {
            continue;
        }
        counts[(cell.effective as i32 > cfg.threshold) as usize] += 1;
        let fade = (1. - (d2.sqrt() / cfg.radius as f32).powi(4)).clamp(0.2, 1.);
        tint[3] *= fade;
        let corners = [
            [x + 0.06, y + 0.028, z + 0.06],
            [x + 0.94, y + 0.028, z + 0.06],
            [x + 0.94, y + 0.028, z + 0.94],
            [x + 0.06, y + 0.028, z + 0.94],
        ];
        let polygon = project_floor(camera, screen, corners);
        if polygon.len < 3 {
            continue;
        }
        if cfg.filled {
            let mut fill = tint;
            fill[3] *= cfg.opacity as f32;
            let at = |i: usize| [polygon.points[i][0], polygon.points[i][1]];
            for i in 1..polygon.len - 1 {
                draw.add_triangle(at(0), at(i), at(i + 1), fill)
                    .filled(true)
                    .build();
            }
        }
        for i in 0..4 {
            let a = corners[i];
            let b = corners[(i + 1) % 4];
            if let Some((a, b)) = segment(camera, screen, a, b) {
                draw.add_line(a, b, [tint[0], tint[1], tint[2], tint[3] * 0.35])
                    .thickness(1.)
                    .build();
            }
            for j in [(i + 1) % 4, (i + 3) % 4] {
                let b =
                    std::array::from_fn(|k| corners[i][k] + (corners[j][k] - corners[i][k]) * 0.23);
                if let Some((a, b)) = segment(camera, screen, corners[i], b) {
                    draw.add_line(a, b, [0., 0., 0., tint[3] * 0.5])
                        .thickness(cfg.line_width as f32 + 1.5)
                        .build();
                    draw.add_line(a, b, tint)
                        .thickness(cfg.line_width as f32)
                        .build();
                }
            }
        }
        let center = [x + 0.5, y + 0.04, z + 0.5];
        let distance = (0..3)
            .map(|i| (center[i] - camera.origin[i]).powi(2))
            .sum::<f32>();
        if !cfg.labels
            || badges.len() >= cfg.max_labels as usize
            || distance > (cfg.label_range as f32).powi(2)
        {
            continue;
        }
        let Some(p) = project(camera, screen, center) else {
            continue;
        };
        let text = format!("{} | {}", cell.block, cell.effective);
        let size = ui.calc_text_size(&text);
        let bounds = Rect {
            min: [p[0] - size[0] * 0.5 - 6., p[1] - size[1] * 0.5 - 3.],
            max: [p[0] + size[0] * 0.5 + 6., p[1] + size[1] * 0.5 + 3.],
        };
        if !bounds.fits(screen) || badges.iter().any(|b| b.bounds.overlaps(bounds)) {
            continue;
        }
        badges.push(Badge {
            bounds,
            text,
            color: tint,
        });
    }
    for badge in badges {
        let r = badge.bounds;
        draw.add_rect(r.min, r.max, [0.025, 0.035, 0.05, 0.94])
            .rounding(4.)
            .filled(true)
            .build();
        draw.add_rect(
            r.min,
            r.max,
            [badge.color[0], badge.color[1], badge.color[2], 0.55],
        )
        .rounding(4.)
        .build();
        draw.add_text([r.min[0] + 6., r.min[1] + 3.], theme::TEXT, badge.text);
    }
    drop(draw);
    if cfg.legend {
        legend(ui, cfg, counts);
    }
}
fn legend(ui: &Ui, cfg: &config::LightOverlay, counts: [usize; 2]) {
    let a = format!("現在暗い {}", counts[0]);
    let b = format!("空の光のみ {}", counts[1]);
    let note = "数値: ブロック光 | 現在の明るさ";
    let h = ui.text_line_height();
    let w = (ui.calc_text_size(&a)[0] + ui.calc_text_size(&b)[0] + 56.)
        .max(ui.calc_text_size(note)[0] + 24.)
        .max(240.);
    let size = [w, h * 3. + 28.];
    crate::hud_editor::measure("lightoverlay", size);
    let screen = ui.io().display_size;
    let left = origin(cfg.x, screen[0], w);
    let top = origin(cfg.y, screen[1], size[1]);
    let draw = ui.get_background_draw_list();
    theme::panel(&draw, [left, top], [left + w, top + size[1]]);
    draw.add_text([left + 12., top + 8.], theme::TEXT, "LightOverlay");
    let y = top + 10. + h;
    draw.add_circle([left + 16., y + h * 0.5], 3., theme::DANGER)
        .filled(true)
        .build();
    draw.add_text([left + 25., y], theme::TEXT, &a);
    let x = left + 34. + ui.calc_text_size(&a)[0];
    draw.add_circle([x, y + h * 0.5], 3., theme::WARNING)
        .filled(true)
        .build();
    draw.add_text([x + 9., y], theme::TEXT, b);
    draw.add_text([left + 12., top + 12. + h * 2.], theme::MUTED, note);
}
#[cfg(test)]
pub(super) fn preview(ui: &Ui) {
    let camera = Camera {
        origin: [0., 2.5, 4.],
        right: [1., 0., 0.],
        up: [0., 0.94, -0.342],
        back: [0., 0.342, 0.94],
        scale: [0.9, 1.2],
    };
    let mut cells = Vec::new();
    for z in -5..=3 {
        for x in -5..=5 {
            cells.push(Cell {
                position: [x, 0, z],
                block: 0,
                effective: if x < 0 { 0 } else { 8 },
            });
        }
    }
    draw(
        ui,
        &camera,
        &cells,
        [0., 0., 0.],
        &config::LightOverlay {
            y: 20,
            ..Default::default()
        },
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn color_uses_both_actual_light_levels() {
        assert_eq!(color(1, 1, 0), None);
        assert_eq!(color(0, 0, 0), Some(theme::DANGER));
        assert_eq!(color(0, 15, 0), Some(theme::WARNING));
    }
    #[test]
    fn floor_polygons_clip_to_viewport_and_handle_near_plane() {
        let camera = Camera {
            origin: [0.; 3],
            right: [1., 0., 0.],
            up: [0., 1., 0.],
            back: [0., 0., 1.],
            scale: [1.; 2],
        };
        for z in [-1., -0.05, 1.] {
            let p = project_floor(
                &camera,
                [800., 600.],
                [
                    [-10., -0.5, z],
                    [10., -0.5, z],
                    [10., -0.5, z - 2.],
                    [-10., -0.5, z - 2.],
                ],
            );
            assert!(p.len <= 12);
            assert!(p.points[..p.len]
                .iter()
                .all(|p| p.iter().all(|v| v.is_finite())
                    && (0. ..=800.).contains(&p[0])
                    && (0. ..=600.).contains(&p[1])));
        }
        assert_eq!(
            project_floor(&camera, [800., 600.], [[0., 0., 1.]; 4]).len,
            0
        );
    }
    #[test]
    fn labels_respect_padding_and_screen_bounds() {
        let r = Rect {
            min: [10., 10.],
            max: [50., 30.],
        };
        assert!(r.overlaps(Rect {
            min: [52., 20.],
            max: [70., 40.]
        }));
        assert!(!r.overlaps(Rect {
            min: [60., 20.],
            max: [70., 40.]
        }));
        assert!(r.fits([100., 100.]));
        assert!(!r.fits([40., 100.]));
    }
}
