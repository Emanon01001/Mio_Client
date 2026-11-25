//! HitMarker: a brief X over the crosshair when the player lands an attack,
//! turning red (and larger) on a kill, like the classic feedback mark. The
//! hits come from `game::hits`, which watches entity health; nothing is
//! written to the game.

use hudhook::imgui::Ui;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

use crate::game::{self, hits};
use crate::modules::settings::{Detail, Value};
use crate::modules::{key_or_warn, Module};
use crate::{config, log};

/// How long a marker is shown, in ms.
const SHOW_MS: u64 = 320;
const HIT_COLOR: [f32; 4] = [1., 1., 1., 1.];
const KILL_COLOR: [f32; 4] = [1., 0.35, 0.3, 1.];

pub struct HitMarker {
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    size: f32,
    thickness: f32,
}

impl HitMarker {
    pub fn new(cfg: &config::HitMarker) -> Self {
        Self {
            key: key_or_warn("hitmarker", &cfg.key),
            enabled: false,
            unavailable: game::available().err(),
            size: if cfg.size.is_finite() {
                cfg.size.clamp(4., 40.)
            } else {
                10.
            },
            thickness: if cfg.thickness.is_finite() {
                cfg.thickness.clamp(1., 6.)
            } else {
                2.
            },
        }
    }
}

impl Module for HitMarker {
    fn name(&self) -> &str {
        "hitmarker"
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
        if let Err(e) = hits::set_active(on) {
            log!("hitmarker: {e}");
            return;
        }
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::number(
                "size",
                "大きさ",
                "照準からXの先までの長さ（ピクセル）です。",
                self.size as f64,
                4.,
                40.,
            ),
            Detail::number(
                "thickness",
                "線の太さ",
                "Xの線の太さ（ピクセル）です。",
                self.thickness as f64,
                1.,
                6.,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "size" => self.size = value.number()?.clamp(4., 40.) as f32,
            "thickness" => self.thickness = value.number()?.clamp(1., 6.) as f32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        String::new()
    }
    fn draw(&mut self, ui: &Ui) {
        if !crate::modules::hud_view() {
            return;
        }
        let now = unsafe { GetTickCount64() };
        let Some(feed) = hits::feed(now) else {
            return;
        };
        // The newest marker still within its show time.
        let Some(marker) = feed
            .markers
            .iter()
            .filter(|m| now.saturating_sub(m.at) <= SHOW_MS)
            .max_by_key(|m| m.at)
        else {
            return;
        };
        self.paint(ui, now.saturating_sub(marker.at), marker.killed);
    }
}

impl HitMarker {
    fn paint(&self, ui: &Ui, age_ms: u64, killed: bool) {
        let screen = ui.io().display_size;
        let center = [screen[0] * 0.5, screen[1] * 0.5];
        // Fade out and grow slightly over the marker's life.
        let t = (age_ms as f32 / SHOW_MS as f32).clamp(0., 1.);
        let alpha = 1. - t;
        let grow = if killed { 1.5 } else { 1. } * (1. + t * 0.3);
        let reach = self.size * grow;
        let gap = reach * 0.35;
        let mut color = if killed { KILL_COLOR } else { HIT_COLOR };
        color[3] = alpha;
        let draw = ui.get_background_draw_list();
        // Four diagonal strokes from a gap near the centre outwards.
        for (sx, sy) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
            let a = [center[0] + sx * gap, center[1] + sy * gap];
            let b = [center[0] + sx * reach, center[1] + sy * reach];
            // A dark backing stroke, then the colour, for contrast.
            draw.add_line(a, b, [0., 0., 0., alpha * 0.6])
                .thickness(self.thickness + 1.5)
                .build();
            draw.add_line(a, b, color).thickness(self.thickness).build();
        }
    }
}

/// A fresh hit marker over the centre, for the headless UI preview.
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let marker = HitMarker::new(&config::HitMarker::default());
    marker.paint(ui, 40, false);
}
