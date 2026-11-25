//! Fullbright: reports a high gamma to the game's renderer by hooking the
//! gamma option's getter. The saved brightness setting is not changed.
//!
//! Only the Simple/Fancy renderers use that value; under Vibrant Visuals the
//! getter is still called but the result has no visible effect.

use crate::game::visual;
use crate::modules::{key_or_warn, Module, Param};
use crate::{config, log};

const MIN_GAMMA: f64 = 1.0;
const MAX_GAMMA: f64 = 25.0;

pub struct Fullbright {
    key: u16,
    enabled: bool,
    gamma: f64,
    /// Why the game function could not be found, if so.
    unavailable: Option<String>,
}

fn clamp_gamma(gamma: f64) -> f64 {
    if gamma.is_finite() {
        gamma.clamp(MIN_GAMMA, MAX_GAMMA)
    } else {
        10.0
    }
}

impl Fullbright {
    pub fn new(cfg: &config::Fullbright) -> Self {
        let gamma = clamp_gamma(cfg.gamma);
        visual::set_gamma(gamma as f32);
        Self {
            key: key_or_warn("fullbright", &cfg.key),
            enabled: false,
            gamma,
            unavailable: visual::gamma_available().err(),
        }
    }
}

impl Module for Fullbright {
    fn name(&self) -> &str {
        "fullbright"
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
            log!("fullbrightを利用できません: {e}");
            return;
        }
        if let Err(e) = visual::set_gamma_hook(on) {
            log!("fullbright: {e}");
            return;
        }
        self.enabled = on;
    }

    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "gamma",
            value: self.gamma,
            min: MIN_GAMMA,
            max: MAX_GAMMA,
        })
    }

    fn set_param(&mut self, value: f64) {
        self.gamma = clamp_gamma(value);
        visual::set_gamma(self.gamma as f32);
    }

    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        format!("ガンマ値 {:.1}", self.gamma)
    }
}
