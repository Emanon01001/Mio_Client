use super::{
    game::{self, actors, blocks, item_assist as native, items},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log};
pub struct ItemAssist {
    cfg: config::ItemAssist,
    key: u16,
    enabled: bool,
    eat: bool,
    unavailable: Option<String>,
}
impl ItemAssist {
    pub fn new(c: &config::ItemAssist, eat: bool) -> Self {
        let mut cfg = c.clone();
        cfg.hunger = cfg.hunger.clamp(1, 19);
        cfg.min_durability = cfg.min_durability.clamp(0, 100);
        Self {
            cfg,
            key: key_or_warn("item assist", &c.key),
            enabled: false,
            eat,
            unavailable: items::available()
                .and_then(|_| blocks::available())
                .and_then(|_| game::actions::available())
                .err(),
        }
    }
}
impl Module for ItemAssist {
    fn name(&self) -> &str {
        if self.eat {
            "autoeat"
        } else {
            "autotool"
        }
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
            log!("{} unavailable: {e}", self.name());
            return;
        }
        if on {
            if let Err(e) = items::use_items(true) {
                log!("{}: {e}", self.name());
                return;
            }
            if let Err(e) = actors::use_actors(true) {
                let _ = items::use_items(false);
                log!("{}: {e}", self.name());
                return;
            }
            blocks::set_look(true);
        } else {
            native::request(self.eat, false, 0);
            blocks::set_look(false);
            let _ = items::use_items(false);
            let _ = actors::use_actors(false);
        }
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            if self.eat {
                Detail::integer(
                    "hunger",
                    "Eat at hunger <=",
                    "Ordinary hotbar food only. Look into air, away from mobs.",
                    self.cfg.hunger,
                    1,
                    19,
                )
            } else {
                Detail::integer(
                    "min_durability",
                    "Reserve durability",
                    "Skip worn tools. Vanilla block/tool families, base material speed.",
                    self.cfg.min_durability,
                    0,
                    100,
                )
            },
            Detail::boolean(
                "restore",
                "Restore original slot",
                "Manual selection always takes priority.",
                self.cfg.restore,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "hunger" => self.cfg.hunger = (v.number()? as i32).clamp(1, 19),
            "min_durability" => self.cfg.min_durability = (v.number()? as i32).clamp(0, 100),
            "restore" => self.cfg.restore = v.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, t: &Tick) {
        if self.eat {
            native::configure_eat(self.cfg.hunger as u32, self.cfg.restore);
        } else {
            native::configure_tool(self.cfg.min_durability as u32, self.cfg.restore);
        }
        native::request(
            self.eat,
            self.enabled && t.active && !game::inventory::screen_open(),
            t.now_ms,
        );
    }
    fn status(&self) -> String {
        self.unavailable.clone().unwrap_or_else(native::status)
    }
}
