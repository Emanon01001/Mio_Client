use super::{
    game::{self, actors, blocks, fishing, items},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log};
pub struct AutoFish {
    cfg: config::AutoFish,
    key: u16,
    enabled: bool,
}
impl AutoFish {
    pub fn new(c: &config::AutoFish) -> Self {
        let mut cfg = c.clone();
        cfg.delay = if cfg.delay.is_finite() {
            cfg.delay.clamp(0.4, 4.)
        } else {
            0.8
        };
        cfg.reserve = cfg.reserve.clamp(0, 100);
        Self {
            cfg,
            key: key_or_warn("autofish", &c.key),
            enabled: false,
        }
    }
}
impl Module for AutoFish {
    fn name(&self) -> &str {
        "autofish"
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
            if let Err(e) = fishing::available().and_then(|_| items::use_items(true)) {
                log!("autofish: {e}");
                return;
            }
            if let Err(e) = actors::use_actors(true) {
                let _ = items::use_items(false);
                log!("autofish: {e}");
                return;
            }
            if let Err(e) = fishing::enable(true) {
                let _ = actors::use_actors(false);
                let _ = items::use_items(false);
                log!("autofish: {e}");
                return;
            }
            blocks::set_look(true);
        } else {
            if let Err(e) = fishing::enable(false) {
                log!("autofish: {e}");
                return;
            }
            blocks::set_look(false);
            let _ = actors::use_actors(false);
            let _ = items::use_items(false);
        }
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::boolean("recast","Recast after catch","First cast is manual. Recast waits for the old bobber to disappear.",self.cfg.recast),
            Detail::number("delay","Recast delay (seconds)","Works in your inventory too. Manual world use, slot changes and other menus cancel a pending cast.",self.cfg.delay,0.4,4.),
            Detail::integer("reserve","Reserve rod durability","Stop before the rod reaches this remaining durability.",self.cfg.reserve,0,100),
            Detail::boolean("clear_view","Require clear cast view","Optional: pause recasting near solid blocks or living actors. Can also pause over shallow water or vegetation.",self.cfg.clear_view)]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "recast" => self.cfg.recast = value.boolean()?,
            "delay" => self.cfg.delay = value.number()?.clamp(0.4, 4.),
            "reserve" => self.cfg.reserve = (value.number()? as i32).clamp(0, 100),
            "clear_view" => self.cfg.clear_view = value.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, t: &Tick) {
        if self.enabled {
            fishing::configure(
                self.cfg.recast,
                (self.cfg.delay * 1000.) as u64,
                self.cfg.reserve as u16,
                self.cfg.clear_view,
            );
        }
        fishing::request(
            self.enabled
                && t.focused
                && !crate::gui::captures_input()
                && !crate::gui::DORMANT.load(std::sync::atomic::Ordering::Acquire)
                && !crate::api::blocked()
                && (t.active || game::inventory::own_inventory_open(t.now_ms)),
            t.now_ms,
        );
    }
    fn status(&self) -> String {
        fishing::status()
    }
}
