//! Native edge assistance, independent of the always-sneak movement helper.
use crate::modules::{
    game::{self, safe_walk as native},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log};
pub struct SafeWalk {
    key: u16,
    enabled: bool,
    margin: f64,
}
impl SafeWalk {
    pub fn new(c: &config::SafeWalk) -> Self {
        Self {
            key: key_or_warn("safewalk", &c.key),
            enabled: false,
            margin: if c.margin.is_finite() {
                c.margin.clamp(0.05, 0.4)
            } else {
                0.12
            },
        }
    }
}
impl Module for SafeWalk {
    fn name(&self) -> &str {
        "safewalk"
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
        if on {
            if let Err(e) = game::blocks::available()
                .and_then(|_| game::actions::available())
                .and_then(|_| game::acquire())
            {
                log!("safewalk: {e}");
                return;
            }
        } else {
            native::request(false, 0, self.margin as f32);
            if let Err(e) = game::release() {
                log!("safewalk: {e}");
                return;
            }
        }
        self.enabled = on;
    }
    fn tick(&mut self, t: &Tick) {
        native::request(
            self.enabled && game::inventory::movement_active(t.active, t.focused, t.now_ms),
            t.now_ms,
            self.margin as f32,
        );
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::number("margin", "Edge margin (blocks)", "Sneak near unsupported feet. Jump manually to leave an edge. Unverified block shapes are treated conservatively.",self.margin,0.05,0.4)]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        if id != "margin" {
            return Err(format!("不明な設定項目: {id}"));
        }
        self.margin = v.number()?.clamp(0.05, 0.4);
        Ok(())
    }
    fn status(&self) -> String {
        native::status()
    }
}
