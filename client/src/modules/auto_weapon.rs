use super::{
    game::{self, items, weapon},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log};
pub struct AutoWeapon {
    key: u16,
    enabled: bool,
    reserve: u32,
    axes: bool,
}
impl AutoWeapon {
    pub fn new(c: &config::AutoWeapon) -> Self {
        Self {
            key: key_or_warn("autoweapon", &c.key),
            enabled: false,
            reserve: c.reserve.clamp(0, u16::MAX as i32) as u32,
            axes: c.axes,
        }
    }
}
impl Module for AutoWeapon {
    fn name(&self) -> &str {
        "autoweapon"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        if let Err(e) = if on {
            weapon::available().and_then(|_| items::use_items(true))
        } else {
            items::use_items(false)
        } {
            log!("AutoWeapon: {e}");
            return;
        }
        self.enabled = on;
        if !on {
            weapon::request(false, 0, self.reserve, self.axes);
        }
    }
    fn tick(&mut self, t: &Tick) {
        weapon::request(
            self.enabled && t.active && !game::inventory::screen_open(),
            t.now_ms,
            self.reserve,
            self.axes,
        );
    }
    fn options(&self) -> Vec<Detail> {
        vec![
        Detail::integer("reserve","残り耐久の下限","残り耐久がこの値以下の武器は選択しません。DurabilityGuardが有効なら、その設定値も下限に使います。",self.reserve as i32,0,u16::MAX as i32),
        Detail::boolean("axes","剣がない場合は斧を選ぶ","ホットバー内の通常素材の剣を優先し、剣がない場合だけ斧を使います。エンチャントの攻撃力は比較しません。",self.axes)]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "reserve" => self.reserve = v.number()?.clamp(0., u16::MAX as f64) as u32,
            "axes" => self.axes = v.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        weapon::status()
    }
}
