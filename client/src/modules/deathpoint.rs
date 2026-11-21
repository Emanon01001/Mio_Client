use super::{
    game::{self, telemetry},
    key_or_warn,
    settings::{Detail, Value},
    waypoints, Module, Tick,
};
use crate::{config, log};
pub struct Deathpoint {
    cfg: config::Deathpoint,
    key: u16,
    enabled: bool,
    last: Option<[i32; 3]>,
}
impl Deathpoint {
    pub fn new(c: &config::Deathpoint) -> Self {
        let mut cfg = c.clone();
        cfg.keep = cfg.keep.clamp(1, 16);
        Self {
            cfg,
            key: key_or_warn("deathpoint", &c.key),
            enabled: false,
            last: None,
        }
    }
}
impl Module for Deathpoint {
    fn name(&self) -> &str {
        "deathpoint"
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
            log!("deathpoint: 対応するゲームビルドが必要です");
            return;
        }
        if let Err(e) = game::use_camera(on) {
            log!("deathpoint: {e}");
            return;
        }
        telemetry::set_death(on);
        waypoints::show_deaths(on);
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::integer(
            "keep",
            "Keep death markers",
            "Automatic markers only; manual waypoints are preserved.",
            self.cfg.keep,
            1,
            16,
        )]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        if id != "keep" {
            return Err(format!("不明な設定項目: {id}"));
        }
        self.cfg.keep = (v.number()? as i32).clamp(1, 16);
        Ok(())
    }
    fn tick(&mut self, _: &Tick) {
        if self.enabled {
            if let Some(d) = telemetry::take_death() {
                match waypoints::record_death(d.position, d.dimension, self.cfg.keep as usize) {
                    Ok(name) => {
                        self.last = Some(d.position);
                        log!("死亡地点を記録: {name} {:?}", d.position);
                    }
                    Err(e) => log!("deathpoint: {e}"),
                }
            }
        }
    }
    fn status(&self) -> String {
        self.last.map_or_else(
            || "死亡地点の記録を待機中".into(),
            |[x, y, z]| format!("{x} {y} {z}"),
        )
    }
}
