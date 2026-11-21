use super::{
    game::{self, freelook as native},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, input, log};
pub struct FreeLook {
    key: u16,
    hold: u16,
    on: bool,
}
impl FreeLook {
    pub fn new(c: &config::FreeLook) -> Self {
        Self {
            key: key_or_warn("freelook", &c.key),
            hold: key_or_warn("freelook.hold_key", &c.hold_key),
            on: false,
        }
    }
}
impl Module for FreeLook {
    fn name(&self) -> &str {
        "freelook"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.on
    }
    fn set_enabled(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        if let Err(e) = if on {
            native::available().and_then(|_| game::acquire())
        } else {
            game::release()
        } {
            log!("FreeLook: {e}");
            return;
        }
        self.on = on;
        if !on {
            native::request(false, 0);
        }
    }
    fn tick(&mut self, t: &Tick) {
        native::request(
            self.on && t.active && !game::inventory::screen_open() && input::key_down(self.hold),
            t.now_ms,
        );
    }
    fn status(&self) -> String {
        if native::active() {
            "見回し中".into()
        } else {
            format!("{}を押して見回す", input::key_name(self.hold))
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::key(
            "hold_key",
            "見回しキー",
            "押している間はカメラだけを回し、離すと元の向きへ戻します。",
            self.hold,
        )]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        if id != "hold_key" {
            return Err(format!("不明な設定項目: {id}"));
        }
        self.hold = v.key()?;
        native::request(false, 0);
        Ok(())
    }
}
