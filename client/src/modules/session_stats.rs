use super::{
    game::{self, telemetry},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, input, log};
pub struct SessionStats {
    cfg: config::SessionStats,
    key: u16,
    reset: u16,
    held: bool,
    enabled: bool,
}
impl SessionStats {
    pub fn new(c: &config::SessionStats) -> Self {
        Self {
            cfg: c.clone(),
            key: key_or_warn("sessionstats", &c.key),
            reset: key_or_warn("プレイ統計をリセットしました", &c.reset_key),
            held: false,
            enabled: false,
        }
    }
}
impl Module for SessionStats {
    fn name(&self) -> &str {
        "sessionstats"
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
        if on && !game::known_build() {
            log!("sessionstats: 対応するゲームビルドが必要です");
            return;
        }
        let result = if on { game::acquire() } else { game::release() };
        if let Err(e) = result {
            log!("sessionstats: {e}");
            return;
        }
        telemetry::set_stats(on);
        self.enabled = on;
        self.held = false;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::x_offset(self.cfg.x, "Negative: from right."),
            Detail::y_offset(self.cfg.y, "Negative: from bottom."),
            Detail::key(
                "reset_key",
                "Reset statistics",
                "Works only in the world view.",
                self.reset,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            "reset_key" => self.reset = v.key()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, t: &Tick) {
        let down = input::key_down(self.reset);
        if self.enabled && t.active && !game::inventory::screen_open() && down && !self.held {
            telemetry::reset();
        }
        self.held = down;
    }
    fn status(&self) -> String {
        let s = telemetry::totals();
        format!("{:.1}m / ジャンプ{}回", s.distance, s.jumps)
    }
    fn draw(&mut self, ui: &hudhook::imgui::Ui) {
        if !super::hud_view() {
            return;
        }
        let s = telemetry::totals();
        let sec = s.elapsed_ms / 1000;
        super::item_hud::panel(
            ui,
            "sessionstats",
            self.cfg.x,
            self.cfg.y,
            &[
                format!(
                    "プレイ時間 {:02}:{:02}:{:02}",
                    sec / 3600,
                    (sec / 60) % 60,
                    sec % 60
                ),
                format!("水平移動距離 {:.1} m", s.distance),
                format!("最高速度 {:.2}ブロック/秒", s.max_speed),
                format!("ジャンプ {}回", s.jumps),
            ],
        );
    }
}
