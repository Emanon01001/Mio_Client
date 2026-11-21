//! A read-only panel of the local player's current attributes.
use super::{
    game::actors,
    key_or_warn,
    radar::origin,
    settings::{Detail, Value},
    Module,
};
use crate::{config, log};
use hudhook::imgui::Ui;

pub struct Vitals {
    key: u16,
    enabled: bool,
    x: i32,
    y: i32,
    food: bool,
    experience: bool,
}
impl Vitals {
    pub fn new(c: &config::Vitals) -> Self {
        Self {
            key: key_or_warn("vitals", &c.key),
            enabled: false,
            x: c.x,
            y: c.y,
            food: c.food,
            experience: c.experience,
        }
    }
}
impl Module for Vitals {
    fn name(&self) -> &str {
        "vitals"
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
        match actors::use_vitals(on) {
            Ok(()) => self.enabled = on,
            Err(e) => log!("vitals: {e}"),
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::x_offset(self.x, "Negative: from right."),
            Detail::y_offset(self.y, "Negative: from bottom."),
            Detail::boolean(
                "food",
                "Food details",
                "Show hunger, saturation and exhaustion.",
                self.food,
            ),
            Detail::boolean(
                "experience",
                "Experience",
                "Show level and progress to the next level.",
                self.experience,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "x" => self.x = v.number()? as i32,
            "y" => self.y = v.number()? as i32,
            "food" => self.food = v.boolean()?,
            "experience" => self.experience = v.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        actors::local_vitals().map_or_else(
            || "（データなし）".into(),
            |v| format!("HP {:.1}/{:.1}", v.health, v.max_health),
        )
    }
    fn draw(&mut self, ui: &Ui) {
        let Some(v) = actors::local_vitals() else {
            return;
        };
        let mut lines = vec![format!(
            "体力 {:.1}/{:.1}   衝撃吸収 {:.1}",
            v.health, v.max_health, v.absorption
        )];
        if self.food {
            lines.push(format!(
                "空腹値 {:.1}/20   隠し満腹度 {:.1}",
                v.hunger, v.saturation
            ));
            lines.push(format!("消耗度 {:.2}", v.exhaustion));
        }
        if self.experience {
            lines.push(format!(
                "レベル {:.0}   経験値 {:.1}%",
                v.level,
                v.experience * 100.
            ));
        }
        let width = lines
            .iter()
            .map(|s| ui.calc_text_size(s)[0])
            .fold(180., f32::max)
            + 24.;
        let line = ui.text_line_height_with_spacing();
        let height = lines.len() as f32 * line + 28.;
        crate::hud_editor::measure("vitals", [width, height]);
        let screen = ui.io().display_size;
        let x = origin(self.x, screen[0], width);
        let y = origin(self.y, screen[1], height);
        let d = ui.get_background_draw_list();
        crate::theme::panel(&d, [x, y], [x + width, y + height]);
        for (i, s) in lines.iter().enumerate() {
            d.add_text([x + 12., y + 8. + i as f32 * line], crate::theme::TEXT, s);
        }
        let bar_y = y + height - 11.;
        crate::theme::progress(
            &d,
            [x + 12., bar_y],
            [width - 24., 4.],
            v.health / v.max_health,
            crate::theme::SUCCESS,
        );
    }
}
