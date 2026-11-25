//! Fly: while on, the movement keys set the player's velocity directly -
//! W/A/S/D along the view direction, Space up, Shift down, hover otherwise.
//!
//! The velocity is written by the shared game hook (`game.rs`).

use crate::modules::{game, key_or_warn, Module, Param, Tick};
use crate::{config, log};

/// Blocks per second.
const MIN_SPEED: f64 = 1.0;
const MAX_SPEED: f64 = 100.0;

fn clamp_speed(speed: f64) -> f64 {
    if speed.is_finite() {
        speed.clamp(MIN_SPEED, MAX_SPEED)
    } else {
        10.0
    }
}

pub struct Fly {
    key: u16,
    /// Why the game hook is unavailable, if it is.
    unavailable: Option<String>,
    enabled: bool,
    speed: f64,
}

impl Fly {
    pub fn new(cfg: &config::Fly) -> Self {
        let speed = clamp_speed(cfg.speed);
        game::set_fly_speed(speed as f32);
        Self {
            key: key_or_warn("fly", &cfg.key),
            unavailable: game::available().err(),
            enabled: false,
            speed,
        }
    }
}

impl Module for Fly {
    fn name(&self) -> &str {
        "fly"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if let Some(e) = &self.unavailable {
            log!("flyを利用できません: {e}");
            return;
        }
        if on == self.enabled {
            return;
        }
        // Stop writing velocity before the hook goes away.
        if !on {
            game::set_fly(false);
        }
        let result = if on { game::acquire() } else { game::release() };
        if let Err(e) = result {
            log!("fly: {e}");
            return;
        }
        game::set_fly(on);
        self.enabled = on;
    }

    fn tick(&mut self, tick: &Tick) {
        game::set_fly_input(self.enabled && tick.active);
    }

    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "blocks/s",
            value: self.speed,
            min: MIN_SPEED,
            max: 50.0,
        })
    }

    fn set_param(&mut self, value: f64) {
        self.speed = clamp_speed(value);
        game::set_fly_speed(self.speed as f32);
    }

    fn status(&self) -> String {
        match &self.unavailable {
            Some(e) => format!("({e})"),
            None => format!("{:.0}ブロック/秒", self.speed),
        }
    }
}
