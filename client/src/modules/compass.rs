//! A horizontal compass ribbon, centered on the current view direction.
use super::settings::{Detail, Value};
use super::{key_or_warn, Module, Param};
use crate::{api, config};
use hudhook::imgui::Ui;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static WIDTH: AtomicU32 = AtomicU32::new(400f32.to_bits());
static Y: AtomicI32 = AtomicI32::new(10);
static X: AtomicI32 = AtomicI32::new(0);

fn delta(bearing: f32, yaw: f32) -> f32 {
    (bearing - yaw + 180.).rem_euclid(360.) - 180.
}
fn label(bearing: i32) -> &'static str {
    match bearing.rem_euclid(360) {
        0 => "S",
        45 => "SW",
        90 => "W",
        135 => "NW",
        180 => "N",
        225 => "NE",
        270 => "E",
        315 => "SE",
        _ => "",
    }
}
pub fn draw(ui: &Ui) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let Some(player) = api::game().ok().and_then(|g| g.player().ok()) else {
        return;
    };
    let yaw = player.rotation.yaw;
    if !yaw.is_finite() {
        return;
    }
    let size = ui.io().display_size;
    let width = f32::from_bits(WIDTH.load(Ordering::Relaxed)).min((size[0] - 20.).max(1.));
    let left = ((size[0] - width - 20.) * 0.5 + X.load(Ordering::Relaxed) as f32)
        .clamp(0., (size[0] - width - 20.).max(0.));
    let (cx, top) = (
        left + (width + 20.) * 0.5,
        (Y.load(Ordering::Relaxed) as f32).clamp(0., (size[1] - 55.).max(0.)),
    );
    crate::hud_editor::measure("compass", [width + 20., 55.]);
    let draw = ui.get_background_draw_list();
    crate::theme::panel(
        &draw,
        [cx - width * 0.5 - 10., top],
        [cx + width * 0.5 + 10., top + 55.],
    );
    for bearing in (0..360).step_by(15) {
        let d = delta(bearing as f32, yaw);
        if d.abs() > 90. {
            continue;
        }
        let x = cx + d / 180. * width;
        let text = label(bearing);
        let color = if bearing == 180 {
            crate::theme::DANGER
        } else {
            crate::theme::TEXT
        };
        draw.add_line(
            [x, top + 25.],
            [x, top + if text.is_empty() { 30. } else { 34. }],
            color,
        )
        .build();
        if !text.is_empty() {
            let text_size = ui.calc_text_size(text);
            draw.add_text([x - text_size[0] * 0.5, top + 5.], color, text);
        }
    }
    // Waypoints of this dimension, as markers at their bearing.
    let here = [player.position.x, player.position.z];
    for (name, target) in super::waypoints::on_compass() {
        let d = delta(super::waypoints::bearing(here, target), yaw);
        if !d.is_finite() || d.abs() > 90. {
            continue;
        }
        let x = cx + d / 180. * width;
        let color = crate::theme::ACCENT;
        draw.add_triangle(
            [x - 4., top + 37.],
            [x + 4., top + 37.],
            [x, top + 30.],
            color,
        )
        .filled(true)
        .build();
        let initial: String = name.chars().take(3).collect();
        let text_size = ui.calc_text_size(&initial);
        draw.add_text([x - text_size[0] * 0.5, top + 38.], color, &initial);
    }
    draw.add_line([cx, top + 20.], [cx, top + 36.], crate::theme::ACCENT)
        .thickness(2.)
        .build();
    let value = format!("{:.0}", yaw.rem_euclid(360.));
    draw.add_text(
        [cx - ui.calc_text_size(&value)[0] * 0.5, top + 38.],
        crate::theme::TEXT,
        value,
    );
}
pub struct Compass {
    key: u16,
    enabled: bool,
}
impl Compass {
    pub fn new(cfg: &config::Compass) -> Self {
        let width = if cfg.width.is_finite() {
            cfg.width.clamp(180., 800.)
        } else {
            400.
        };
        WIDTH.store((width as f32).to_bits(), Ordering::Relaxed);
        Y.store(cfg.y, Ordering::Relaxed);
        X.store(cfg.x, Ordering::Relaxed);
        Self {
            key: key_or_warn("compass", &cfg.key),
            enabled: false,
        }
    }
}
impl Module for Compass {
    fn name(&self) -> &str {
        "compass"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::integer(
                "x",
                "X offset",
                "Pixels from the centered position.",
                X.load(Ordering::Relaxed),
                -7680,
                7680,
            ),
            Detail::integer(
                "y",
                "Y offset",
                "Distance from the top of the screen in pixels.",
                Y.load(Ordering::Relaxed),
                0,
                4320,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "x" => X.store(value.number()? as i32, Ordering::Relaxed),
            "y" => Y.store(value.number()? as i32, Ordering::Relaxed),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        ACTIVE.store(on, Ordering::Release);
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "width",
            value: f32::from_bits(WIDTH.load(Ordering::Relaxed)) as f64,
            min: 180.,
            max: 800.,
        })
    }
    fn set_param(&mut self, value: f64) {
        if value.is_finite() {
            WIDTH.store(
                (value.clamp(180., 800.) as f32).to_bits(),
                Ordering::Relaxed,
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headings_follow_minecraft_and_wrap_without_jumping() {
        assert_eq!(
            (label(0), label(90), label(180), label(-90)),
            ("S", "W", "N", "E")
        );
        assert_eq!(delta(180., 179.), 1.);
        assert_eq!(delta(180., -179.), -1.);
        assert_eq!(delta(0., 359.), 1.);
        assert_eq!(delta(270., 0.), -90.);
    }
}
