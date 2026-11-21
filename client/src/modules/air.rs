//! Current/max air from typed metadata, plus the native head-in-water flag.
use super::{
    game::status,
    key_or_warn,
    radar::origin,
    settings::{Detail, Value},
    Module,
};
use crate::{config, log};
use hudhook::imgui::Ui;
pub struct Air {
    cfg: config::Air,
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
}
impl Air {
    pub fn new(c: &config::Air) -> Self {
        Self {
            cfg: c.clone(),
            key: key_or_warn("air", &c.key),
            enabled: false,
            unavailable: status::available().err(),
        }
    }
}
impl Module for Air {
    fn name(&self) -> &str {
        "air"
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
            log!("airを利用できません: {e}");
            return;
        }
        if let Err(e) = status::use_status(on) {
            log!("air: {e}");
            return;
        }
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::x_offset(self.cfg.x, "Negative: from right."),
            Detail::y_offset(self.cfg.y, "Negative: from bottom."),
            Detail::boolean(
                "only_underwater",
                "Hide when full on land",
                "Show while the head is underwater or air is still refilling.",
                self.cfg.only_underwater,
            ),
            Detail::boolean(
                "ticks",
                "Raw ticks",
                "Also show the actual current/maximum air counters.",
                self.cfg.ticks,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            "only_underwater" => self.cfg.only_underwater = v.boolean()?,
            "ticks" => self.cfg.ticks = v.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        status::reading().and_then(|r| r.air).map_or_else(
            || "（データなし）".into(),
            |a| format!("{}/{}", a.current.max(0), a.max),
        )
    }
    fn draw(&mut self, ui: &Ui) {
        if !super::hud_view() {
            return;
        }
        let Some(a) = status::reading().and_then(|r| r.air) else {
            return;
        };
        if self.cfg.only_underwater && !a.underwater && a.current >= a.max {
            return;
        }
        let ratio = (a.current as f32 / a.max as f32).clamp(0., 1.);
        let text = if self.cfg.ticks {
            format!("酸素 {:.0}%   {}/{} tick", ratio * 100., a.current, a.max)
        } else {
            format!("酸素 {:.0}%", ratio * 100.)
        };
        let width = ui.calc_text_size(&text)[0].max(160.) + 20.;
        let height = ui.text_line_height() + 30.;
        crate::hud_editor::measure("air", [width, height]);
        let screen = ui.io().display_size;
        let x = origin(self.cfg.x, screen[0], width);
        let y = origin(self.cfg.y, screen[1], height);
        let d = ui.get_background_draw_list();
        crate::theme::panel(&d, [x, y], [x + width, y + height]);
        d.add_text([x + 10., y + 8.], crate::theme::TEXT, text);
        let by = y + height - 12.;
        crate::theme::progress(
            &d,
            [x + 10., by],
            [width - 20., 5.],
            ratio,
            if ratio <= 0.2 {
                crate::theme::DANGER
            } else {
                crate::theme::ACCENT
            },
        );
    }
}
