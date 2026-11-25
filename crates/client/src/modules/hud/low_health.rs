//! Low-health notifications with a recovery dead band and per-episode notices.
use crate::modules::{
    game::actors,
    key_or_warn,
    settings::{Detail, Value},
    Module, Param, Tick,
};
use crate::{api, config, log, notify};
use hudhook::imgui::Ui;

#[derive(Default)]
struct Alarm {
    active: bool,
    notified: bool,
    last_notice: Option<u64>,
}
impl Alarm {
    fn update(&mut self, percent: Option<f32>, threshold: f32, now: u64, cooldown: u64) -> bool {
        let Some(percent) = percent.filter(|v| v.is_finite()) else {
            self.active = false;
            self.notified = false;
            return false;
        };
        if percent > threshold + 5. {
            self.active = false;
            self.notified = false;
        } else if percent <= threshold {
            self.active = true;
        }
        if self.active
            && !self.notified
            && self
                .last_notice
                .is_none_or(|last| now.saturating_sub(last) >= cooldown)
        {
            self.notified = true;
            self.last_notice = Some(now);
            return true;
        }
        false
    }
}

pub struct LowHealth {
    cfg: config::LowHealth,
    key: u16,
    enabled: bool,
    visible: bool,
    alarm: Alarm,
    sampled: u64,
    last: Option<actors::Vitals>,
}
impl LowHealth {
    pub fn new(c: &config::LowHealth) -> Self {
        let mut cfg = c.clone();
        cfg.threshold = if cfg.threshold.is_finite() {
            cfg.threshold.clamp(5., 90.)
        } else {
            30.
        };
        cfg.cooldown = if cfg.cooldown.is_finite() {
            cfg.cooldown.clamp(1., 120.)
        } else {
            10.
        };
        Self {
            key: key_or_warn("lowhealth", &cfg.key),
            cfg,
            enabled: false,
            visible: false,
            alarm: Alarm::default(),
            sampled: 0,
            last: None,
        }
    }
}
impl Module for LowHealth {
    fn name(&self) -> &str {
        "lowhealth"
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
        if let Err(e) = actors::use_vitals(on) {
            log!("lowhealth: {e}");
            return;
        }
        self.enabled = on;
        self.alarm = Alarm::default();
        self.last = None;
        self.visible = false;
        self.sampled = 0;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "threshold %",
            value: self.cfg.threshold,
            min: 5.,
            max: 90.,
        })
    }
    fn set_param(&mut self, v: f64) {
        if v.is_finite() {
            self.cfg.threshold = v.clamp(5., 90.);
            self.alarm.active = false;
            self.alarm.notified = false;
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean("notification", "Notification", "Notify once per low-health episode. Also requires GUI notifications to be enabled.", self.cfg.notification),
            Detail::boolean("border", "Red screen border", "Show a red border while health is low.", self.cfg.border),
            Detail::boolean("absorption", "Count absorption HP", "Add temporary absorption HP when comparing against the threshold.", self.cfg.absorption),
            Detail::number("cooldown", "Notice cooldown (seconds)", "Minimum time between notices; health must recover before another episode.", self.cfg.cooldown,1.,120.),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "notification" => self.cfg.notification = v.boolean()?,
            "border" => self.cfg.border = v.boolean()?,
            "absorption" => {
                self.cfg.absorption = v.boolean()?;
                self.alarm.active = false;
                self.alarm.notified = false;
            }
            "cooldown" => self.cfg.cooldown = v.number()?.clamp(1., 120.),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, tick: &Tick) {
        self.visible = self.enabled && tick.active && crate::modules::hud_view();
        if !self.visible || tick.now_ms.saturating_sub(self.sampled) < 100 {
            return;
        }
        self.sampled = tick.now_ms;
        self.last = if api::game().ok().and_then(|g| g.player().ok()).is_some() {
            actors::local_vitals().filter(|v| v.health > 0.)
        } else {
            None
        };
        let percent = self.last.map(|v| {
            (v.health
                + if self.cfg.absorption {
                    v.absorption.max(0.)
                } else {
                    0.
                })
                / v.max_health
                * 100.
        });
        if self.alarm.update(
            percent,
            self.cfg.threshold as f32,
            tick.now_ms,
            (self.cfg.cooldown * 1000.) as u64,
        ) && self.cfg.notification
        {
            if let Some(v) = self.last {
                notify::push(&format!("体力低下: {:.1}/{:.1} HP", v.health, v.max_health));
            }
        }
    }
    fn status(&self) -> String {
        self.last.map_or_else(
            || "（データなし）".into(),
            |v| {
                format!(
                    "{:.1}/{:.1} HP{}",
                    v.health,
                    v.max_health,
                    if self.alarm.active { " 低下中" } else { "" }
                )
            },
        )
    }
    fn draw(&mut self, ui: &Ui) {
        if !self.visible
            || !self.cfg.border
            || !self.alarm.active
            || !crate::modules::hud_view()
            || actors::local_vitals().is_none()
        {
            return;
        }
        let size = ui.io().display_size;
        let d = ui.get_foreground_draw_list();
        let color = [1., 0.15, 0.1, 0.8];
        let width = 6_f32.min(size[0] * 0.5).min(size[1] * 0.5);
        for (a, b) in [
            ([0., 0.], [size[0], width]),
            ([0., size[1] - width], size),
            ([0., width], [width, size[1] - width]),
            ([size[0] - width, width], [size[0], size[1] - width]),
        ] {
            d.add_rect(a, b, color).filled(true).build();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn low_health_notices_need_recovery_and_respect_cooldown() {
        let mut a = Alarm::default();
        assert!(!a.update(Some(100.), 30., 0, 10000));
        assert!(a.update(Some(30.), 30., 100, 10000));
        assert!(!a.update(Some(10.), 30., 20000, 10000)); // same episode
        assert!(!a.update(Some(32.), 30., 20001, 10000)); // recovery dead band
        assert!(a.active);
        assert!(!a.update(Some(36.), 30., 20002, 10000));
        assert!(!a.active);
        assert!(a.update(Some(20.), 30., 20003, 10000));
        a.update(Some(100.), 30., 20004, 10000);
        assert!(!a.update(Some(20.), 30., 20005, 10000));
        assert!(a.update(Some(20.), 30., 30003, 10000)); // deferred notice
        assert!(!a.update(None, 30., 40000, 10000));
        assert!(!a.active);
    }
}
