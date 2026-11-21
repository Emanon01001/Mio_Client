use super::{
    game::{self, world::Realm, world_info as native},
    key_or_warn,
    settings::{Detail, Value},
    Module,
};
use crate::{config, log};
use hudhook::imgui::Ui;
pub struct WorldInfo {
    key: u16,
    on: bool,
    x: i32,
    y: i32,
}
impl WorldInfo {
    pub fn new(c: &config::HudPosition) -> Self {
        Self {
            key: key_or_warn("worldinfo", &c.key),
            on: false,
            x: c.x,
            y: c.y,
        }
    }
}
impl Module for WorldInfo {
    fn name(&self) -> &str {
        "worldinfo"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.on
    }
    fn set_enabled(&mut self, on: bool) {
        if self.on == on {
            return;
        }
        if let Err(e) = if on {
            native::available().and_then(|_| game::acquire())
        } else {
            game::release()
        } {
            log!("WorldInfo: {e}");
            return;
        }
        native::set_active(on);
        self.on = on;
    }
    fn status(&self) -> String {
        native::info()
            .map(|v| {
                let (_, h, m) = native::clock(v.ticks);
                format!("{h:02}:{m:02}")
            })
            .unwrap_or_else(|| "（ワールド未接続）".into())
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::x_offset(self.x, "Negative: from right."),
            Detail::y_offset(self.y, "Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "x" => self.x = v.number()? as i32,
            "y" => self.y = v.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn draw(&mut self, ui: &Ui) {
        if !super::hud_view() {
            return;
        }
        if let Some(v) = native::info() {
            let (day, h, m) = native::clock(v.ticks);
            let weather = if v.realm != Realm::Overworld {
                "この次元に天気なし"
            } else if v.thunder > 0.01 {
                "雷雨"
            } else if v.rain > 0.01 {
                "雨・雪"
            } else {
                "晴れ"
            };
            super::item_hud::panel(
                ui,
                "worldinfo",
                self.x,
                self.y,
                &[
                    "WorldInfo".into(),
                    format!("{day}日目  {h:02}:{m:02}  {weather}"),
                    format!("雨 {:.0}% / 雷 {:.0}%", v.rain * 100., v.thunder * 100.),
                ],
            );
        }
    }
}
