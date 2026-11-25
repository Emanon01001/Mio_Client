//! View-relative entity radar using the existing owned ESP snapshots.
use crate::modules::entity_kind;
use crate::modules::settings::{Detail, Value};
use crate::modules::{game, key_or_warn, Module, Param};
use crate::{api, config, log};
use hudhook::imgui::Ui;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
mod projection;
use super::origin;
use projection::Projection;

#[cfg(test)]
fn point(delta: [f32; 2], yaw: f32, range: f32, half: f32) -> Option<[f32; 2]> {
    Projection::new(yaw, range, half)?.point(delta)
}
pub struct Radar {
    key: u16,
    enabled: bool,
    range: f64,
    size: f32,
    x: i32,
    y: i32,
    boxes: Option<std::sync::Arc<Vec<(u32, game::Aabb)>>>,
    last_read: u64,
    last_position: Option<[f32; 3]>,
    /// Actor facts are subscribed (to colour marks by entity kind).
    actors: bool,
}
impl Radar {
    pub fn new(cfg: &config::Radar) -> Self {
        Self {
            key: key_or_warn("radar", &cfg.key),
            enabled: false,
            range: if cfg.range.is_finite() {
                cfg.range.clamp(8., 128.)
            } else {
                48.
            },
            size: if cfg.size.is_finite() {
                cfg.size.clamp(120., 360.)
            } else {
                180.
            },
            x: cfg.x,
            y: cfg.y,
            boxes: None,
            last_read: 0,
            last_position: None,
            actors: false,
        }
    }
}
impl Module for Radar {
    fn name(&self) -> &str {
        "radar"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::number(
                "size",
                "Size (pixels)",
                "Width and height of the radar.",
                self.size as f64,
                100.,
                400.,
            ),
            Detail::x_offset(self.x, "Positive: from left. Negative: from right."),
            Detail::y_offset(self.y, "Positive: from top. Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "size" => self.size = value.number()?.clamp(100., 400.) as f32,
            "x" => self.x = value.number()? as i32,
            "y" => self.y = value.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        if let Err(e) = game::use_entities(on) {
            log!("radar: {e}");
            return;
        }
        // Without actor facts the marks are still drawn, in one colour.
        if on {
            self.actors = match game::actors::use_actors(true) {
                Ok(()) => true,
                Err(e) => {
                    log!("radar: 種類での色分けはできません: {e}");
                    false
                }
            };
        } else if std::mem::take(&mut self.actors) {
            let _ = game::actors::use_actors(false);
        }
        self.enabled = on;
        self.boxes = None;
        self.last_read = 0;
        self.last_position = None;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "range",
            value: self.range,
            min: 8.,
            max: 128.,
        })
    }
    fn set_param(&mut self, v: f64) {
        if v.is_finite() {
            self.range = v.clamp(8., 128.);
        }
    }
    fn draw(&mut self, ui: &Ui) {
        let Some(player) = api::game().ok().and_then(|g| g.player().ok()) else {
            self.boxes = None;
            self.last_read = 0;
            self.last_position = None;
            return;
        };
        if game::box_count().is_none() {
            self.boxes = None;
            self.last_read = 0;
            return;
        }
        let position: [f32; 3] = player.position.into();
        let now = unsafe { GetTickCount64() };
        let moved = self.last_position.is_some_and(|p| {
            (0..3).map(|k| (p[k] - position[k]).powi(2)).sum::<f32>() > (self.range as f32).powi(2)
        });
        if self.last_read == 0 || now.saturating_sub(self.last_read) >= 50 || moved {
            self.boxes = game::shared_boxes();
            self.last_read = now;
            self.last_position = Some(position);
        }
        let screen = ui.io().display_size;
        let size = self
            .size
            .min((screen[0] - 10.).max(1.))
            .min((screen[1] - 10.).max(1.));
        let left = origin(self.x, screen[0], size);
        let top = origin(self.y, screen[1], size);
        let center = [left + size * 0.5, top + size * 0.5];
        crate::hud_editor::measure("radar", [size, size]);
        let half = (size * 0.5 - 14.).max(1.);
        let Some(projection) = Projection::new(player.rotation.yaw, self.range as f32, half) else {
            return;
        };
        let draw = ui.get_background_draw_list();
        crate::theme::panel(&draw, [left, top], [left + size, top + size]);
        draw.add_circle(center, half, [0.57, 0.64, 0.74, 0.35])
            .build();
        draw.add_line(
            [center[0] - half, center[1]],
            [center[0] + half, center[1]],
            crate::theme::BORDER,
        )
        .build();
        draw.add_line(
            [center[0], center[1] - half],
            [center[0], center[1] + half],
            crate::theme::BORDER,
        )
        .build();
        let facts = game::actors::actors();
        for (id, a) in self.boxes.as_deref().into_iter().flatten() {
            if a[4] - a[1] < 0.3 {
                continue;
            }
            let world = [
                (a[0] + a[3]) * 0.5,
                (a[1] + a[4]) * 0.5,
                (a[2] + a[5]) * 0.5,
            ];
            if let Some(p) = projection.point([world[0] - position[0], world[2] - position[2]]) {
                let color = entity_kind::mark(entity_kind::of(facts.as_deref(), *id));
                draw.add_circle([center[0] + p[0], center[1] + p[1]], 2.5, color)
                    .filled(true)
                    .build();
            }
        }
        // Cardinal markers rotate with the view; the player always faces up.
        let markers = Projection::new(player.rotation.yaw, 1., half).unwrap();
        for (world, label) in [
            ([0., -1.], "N"),
            ([1., 0.], "E"),
            ([0., 1.], "S"),
            ([-1., 0.], "W"),
        ] {
            let p = markers.point(world).unwrap();
            let text = ui.calc_text_size(label);
            draw.add_text(
                [
                    center[0] + p[0] - text[0] * 0.5,
                    center[1] + p[1] - text[1] * 0.5,
                ],
                [1., 1., 1., 0.9],
                label,
            );
        }
        draw.add_triangle(
            [center[0], center[1] - 5.],
            [center[0] - 4., center[1] + 4.],
            [center[0] + 4., center[1] + 4.],
            crate::theme::TEXT,
        )
        .filled(true)
        .build();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radar_places_forward_up_and_right_right_for_minecraft_yaw() {
        assert_eq!(point([0., 10.], 0., 20., 100.), Some([0., -50.]));
        assert_eq!(point([-10., 0.], 0., 20., 100.), Some([50., 0.]));
        let p = point([-10., 0.], 90., 20., 100.).unwrap();
        assert!(p[0].abs() < 0.001 && (p[1] + 50.).abs() < 0.001);
        assert!(point([21., 0.], 0., 20., 100.).is_none());
        assert!(point([f32::NAN, 0.], 0., 20., 100.).is_none());
        assert_eq!(origin(-20, 1920., 180.), 1720.);
    }
}
