use super::{
    game::{self, combat},
    key_or_warn,
    settings::{Detail, Value},
    Module, Param, Tick,
};
use crate::{config, log};
pub struct TriggerBot {
    key: u16,
    enabled: bool,
    range: f64,
    cps: f64,
    players: bool,
    mobs: bool,
    unavailable: Option<String>,
    paused: Option<&'static str>,
}
impl TriggerBot {
    pub fn new(c: &config::TriggerBot) -> Self {
        let range = if c.range.is_finite() {
            c.range.clamp(1., 6.)
        } else {
            3.
        };
        let cps = if c.cps.is_finite() {
            c.cps.clamp(1., 20.)
        } else {
            8.
        };
        combat::configure_trigger(range, cps, c.players, c.mobs);
        Self {
            key: key_or_warn("triggerbot", &c.key),
            enabled: false,
            range,
            cps,
            players: c.players,
            mobs: c.mobs,
            unavailable: combat::available()
                .and_then(|_| game::blocks::available())
                .err(),
            paused: None,
        }
    }
    fn configure(&self) {
        combat::configure_trigger(self.range, self.cps, self.players, self.mobs);
    }
}
impl Module for TriggerBot {
    fn name(&self) -> &str {
        "triggerbot"
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
            log!("TriggerBotを利用できません: {e}");
            return;
        }
        if let Err(e) = if on { game::acquire() } else { game::release() } {
            log!("TriggerBot: {e}");
            return;
        }
        self.enabled = on;
        if !on {
            combat::request_trigger(false, 0);
        }
    }
    fn tick(&mut self, t: &Tick) {
        self.paused = t.paused;
        combat::request_trigger(self.enabled && t.active, t.now_ms);
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "range",
            value: self.range,
            min: 1.,
            max: 6.,
        })
    }
    fn set_param(&mut self, value: f64) {
        if value.is_finite() {
            self.range = value.clamp(1., 6.);
            self.configure();
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::number(
                "cps",
                "Attacks per second",
                "Maximum attack rate; the game still applies its damage timing.",
                self.cps,
                1.,
                20.,
            ),
            Detail::boolean(
                "players",
                "Target players",
                "Allow player entities as targets.",
                self.players,
            ),
            Detail::boolean("mobs", "Target mobs", "Allow mobs as targets.", self.mobs),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "cps" => self.cps = v.number()?.clamp(1., 20.),
            "players" => self.players = v.boolean()?,
            "mobs" => self.mobs = v.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.configure();
        Ok(())
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        if let Some(reason) = self.paused.filter(|_| self.enabled) {
            return format!("一時停止: {reason}");
        }
        format!(
            "照準先のみ / {:.1}ブロック / {:.1}回/秒",
            self.range, self.cps
        )
    }
}
