use crate::modules::{game, key_or_warn, Module, Param, Tick};
use crate::{config, log};

const MIN_CPS: f64 = 1.0;
const MAX_CPS: f64 = 20.0;

/// Repeats native attack while the game's attack action is held.
pub struct AutoClicker {
    key: u16,
    enabled: bool,
    cps: f64,
    unavailable: Option<String>,
    /// Why no input is being sent right now, if so.
    paused: Option<&'static str>,
}

fn clamp_cps(cps: f64) -> f64 {
    if cps.is_finite() {
        cps.clamp(MIN_CPS, MAX_CPS)
    } else {
        MIN_CPS
    }
}

impl AutoClicker {
    pub fn new(cfg: &config::AutoClicker) -> Self {
        Self {
            key: key_or_warn("autoclicker", &cfg.key),
            enabled: false,
            cps: clamp_cps(cfg.cps),
            unavailable: game::actions::available().err(),
            paused: None,
        }
    }

    fn interval_ms(&self) -> u64 {
        (1000.0 / self.cps).round() as u64
    }
}

impl Module for AutoClicker {
    fn name(&self) -> &str {
        "autoclicker"
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
            log!("autoclickerを利用できません: {e}");
            return;
        }
        if !on {
            game::actions::click(0, 0);
        }
        let result = if on { game::acquire() } else { game::release() };
        if let Err(e) = result {
            log!("autoclicker: {e}");
            return;
        }
        self.enabled = on;
    }

    fn tick(&mut self, tick: &Tick) {
        self.paused = tick.paused;
        let active = self.enabled && tick.active;
        game::actions::click(if active { self.interval_ms() } else { 0 }, tick.now_ms);
    }

    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "cps",
            value: self.cps,
            min: MIN_CPS,
            max: MAX_CPS,
        })
    }

    fn set_param(&mut self, value: f64) {
        self.cps = clamp_cps(value);
    }

    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        match self.paused {
            Some(reason) if self.enabled => format!("{:.1}回/秒（一時停止: {reason}）", self.cps),
            _ => format!("{:.1}回/秒（{}）", self.cps, game::actions::status(false)),
        }
    }
}
