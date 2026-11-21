//! Custom field of view: draws the world with a chosen FOV, beyond the range
//! of the game's own slider. Shares the field-of-view hook with zoom, which
//! still applies on top. The held item keeps its normal size.
//!
//! The game's dynamic FOV changes (sprinting, speed effects) are replaced by
//! the fixed value while this is on. The saved setting is not changed.

use super::game::visual;
use super::{key_or_warn, Module, Param};
use crate::{config, log};

pub struct Fov {
    key: u16,
    enabled: bool,
    degrees: f64,
    /// Why the game function could not be found, if so.
    unavailable: Option<String>,
}

fn clamp_degrees(degrees: f64) -> f64 {
    if degrees.is_finite() {
        degrees.clamp(visual::FOV_RANGE.0 as f64, visual::FOV_RANGE.1 as f64)
    } else {
        110.0
    }
}

impl Fov {
    pub fn new(cfg: &config::Fov) -> Self {
        Self {
            key: key_or_warn("fov", &cfg.key),
            enabled: false,
            degrees: clamp_degrees(cfg.degrees),
            unavailable: visual::zoom_available().err(),
        }
    }
}

impl Module for Fov {
    fn name(&self) -> &str {
        "fov"
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
            log!("fovを利用できません: {e}");
            return;
        }
        if let Err(e) = visual::set_fov_hook(on, self.degrees as f32) {
            log!("fov: {e}");
            return;
        }
        self.enabled = on;
    }

    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "degrees",
            value: self.degrees,
            min: visual::FOV_RANGE.0 as f64,
            max: visual::FOV_RANGE.1 as f64,
        })
    }

    fn set_param(&mut self, value: f64) {
        self.degrees = clamp_degrees(value);
        visual::set_fov_override(self.degrees as f32);
    }

    fn status(&self) -> String {
        match &self.unavailable {
            Some(e) => format!("({e})"),
            None => format!("{:.0}度", self.degrees),
        }
    }
}
