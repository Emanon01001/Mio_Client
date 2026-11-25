use crate::modules::{
    game::{self, durability_guard as native, items},
    key_or_warn,
    settings::{Detail, Value},
    Module, Param, Tick,
};
use crate::{config, log};
pub struct DurabilityGuard {
    key: u16,
    enabled: bool,
    reserve: u32,
    empty: bool,
}
impl DurabilityGuard {
    pub fn new(c: &config::DurabilityGuard) -> Self {
        Self {
            key: key_or_warn("durabilityguard", &c.key),
            enabled: false,
            reserve: c.reserve.clamp(1, native::MAX_RESERVE as i32) as u32,
            empty: c.empty_slot,
        }
    }
}
impl Module for DurabilityGuard {
    fn name(&self) -> &str {
        "durabilityguard"
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
            if let Err(e) = game::actions::available()
                .and_then(|_| native::available())
                .and_then(|_| items::use_items(true))
            {
                log!("durabilityguard: {e}");
                return;
            }
        } else {
            native::request(false, 0, self.reserve, self.empty);
            if let Err(e) = items::use_items(false) {
                log!("durabilityguard: {e}");
                return;
            }
        }
        self.enabled = on;
    }
    fn tick(&mut self, t: &Tick) {
        native::request(
            self.enabled && t.active && !game::inventory::screen_open(),
            t.now_ms,
            self.reserve,
            self.empty,
        );
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::integer("reserve", "Remaining durability threshold", "Switch below this remaining durability to the same tool family. Hotbar first, then inventory. Ctrl+click for an exact number.",self.reserve as i32,1,native::MAX_RESERVE as i32),
        Detail::boolean("empty_slot","Use empty slot if no spare","If no spare or empty slot is available, keep the selection and show a warning in this feature's status.",self.empty)]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "reserve" => self.reserve = (v.number()? as u32).clamp(1, native::MAX_RESERVE),
            "empty_slot" => self.empty = v.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        native::status()
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "remaining durability",
            value: self.reserve as f64,
            min: 1.,
            max: native::MAX_RESERVE as f64,
        })
    }
    fn set_param(&mut self, value: f64) {
        if value.is_finite() {
            self.reserve = (value as u32).clamp(1, native::MAX_RESERVE);
        }
    }
    fn draw_settings(&mut self, ui: &hudhook::imgui::Ui) {
        if ui.collapsing_header(
            crate::localization::text("Diagnostics"),
            hudhook::imgui::TreeNodeFlags::empty(),
        ) {
            ui.text_wrapped(native::diagnostic());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn high_durability_tools_keep_configured_units_across_menu_and_console() {
        let mut guard = DurabilityGuard::new(&config::DurabilityGuard {
            reserve: 2000,
            ..Default::default()
        });
        assert_eq!(guard.reserve, 2000);
        guard.set_option("reserve", &Value::Number(2030.)).unwrap();
        assert_eq!(guard.param().unwrap().value, 2030.);
        guard.set_param(10.);
        assert_eq!(guard.reserve, 10);
        guard.set_param(f64::NAN);
        assert_eq!(guard.reserve, 10);
        guard
            .set_option("reserve", &Value::Number(100000.))
            .unwrap();
        assert_eq!(guard.reserve, native::MAX_RESERVE);
    }
}
