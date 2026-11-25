//! Field of view: a fixed custom FOV (Fov) and a held-key zoom (Zoom, below).
//!
//! Custom field of view: draws the world with a chosen FOV, beyond the range
//! of the game's own slider. Shares the field-of-view hook with zoom, which
//! still applies on top. The held item keeps its normal size.
//!
//! The game's dynamic FOV changes (sprinting, speed effects) are replaced by
//! the fixed value while this is on. The saved setting is not changed.

use crate::game::visual;
use crate::modules::settings::{Detail, Value};
use crate::modules::{key_or_warn, Module, Param, Tick};
use crate::{config, input, log};

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

// ---- Zoom ----------------------------------------------------------------
// Zoom: narrows the field of view while a key is held, by hooking the game's
// own field-of-view calculation. The held item keeps its normal size.

const MIN_FACTOR: f64 = 1.5;
const MAX_FACTOR: f64 = 30.0;

pub struct Zoom {
    hold_key: u16,
    enabled: bool,
    factor: f64,
    sensitivity: bool,
    sensitivity_scale: f64,
    /// How long a zoom in or out takes (ms, 0 = instant).
    smooth_ms: f64,
    /// Why the game function could not be found, if so.
    unavailable: Option<String>,
}

fn clamp_factor(factor: f64) -> f64 {
    if factor.is_finite() {
        factor.clamp(MIN_FACTOR, MAX_FACTOR)
    } else {
        4.0
    }
}

impl Zoom {
    pub fn new(cfg: &config::Zoom) -> Self {
        let hold_key = key_or_warn("zoom.hold_key", &cfg.hold_key);
        visual::set_zoom_hold_key(hold_key);
        let factor = clamp_factor(cfg.factor);
        visual::set_zoom_factor(factor as f32);
        let sensitivity_scale = if cfg.sensitivity_scale.is_finite() {
            cfg.sensitivity_scale.clamp(0.01, 1.)
        } else {
            0.25
        };
        visual::set_sensitivity_scale(sensitivity_scale as f32);
        let smooth_ms = if cfg.smooth_ms.is_finite() {
            cfg.smooth_ms.clamp(0., 1000.)
        } else {
            200.
        };
        visual::set_zoom_smoothing(smooth_ms as f32);
        Self {
            hold_key,
            enabled: false,
            factor,
            sensitivity: cfg.sensitivity,
            sensitivity_scale,
            smooth_ms,
            unavailable: visual::zoom_available().err(),
        }
    }
}

impl Module for Zoom {
    fn name(&self) -> &str {
        "zoom"
    }

    fn key(&self) -> u16 {
        0
    }

    fn supports_toggle_key(&self) -> bool {
        false
    }

    fn options(&self) -> Vec<Detail> {
        vec![Detail::key(
            "hold_key",
            "Hold key",
            "Zoom while holding this key during gameplay.",
            self.hold_key,
        ),
        Detail::boolean("sensitivity", "Reduce mouse sensitivity", "Experimental: scale the mouse option while holding zoom. Saved game settings are unchanged.", self.sensitivity),
        Detail::number("sensitivity_scale", "Sensitivity multiplier", "0.25 = one quarter of the sensitivity option value while zooming.", self.sensitivity_scale, 0.01,1.),
        Detail::number("smooth_ms", "切り替えの時間（ms）", "ズームの寄り・戻りにかける時間です。0で瞬時に切り替えます。感度の変更もこれに合わせて変わります。", self.smooth_ms, 0., 1000.),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "hold_key" => {
                self.hold_key = value.key()?;
                visual::set_zoom_hold_key(self.hold_key);
            }
            "sensitivity" => {
                let on = value.boolean()?;
                if self.enabled {
                    visual::set_zoom_sensitivity(on)?;
                }
                self.sensitivity = on;
            }
            "smooth_ms" => {
                self.smooth_ms = value.number()?.clamp(0., 1000.);
                visual::set_zoom_smoothing(self.smooth_ms as f32);
            }
            "sensitivity_scale" => {
                self.sensitivity_scale = value.number()?.clamp(0.01, 1.);
                visual::set_sensitivity_scale(self.sensitivity_scale as f32);
            }
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        if let Some(e) = &self.unavailable {
            log!("zoomを利用できません: {e}");
            return;
        }
        // Roll back the first hook if the second one cannot be switched.
        if !on {
            if let Err(e) = visual::set_zoom_sensitivity(false) {
                log!("zoom: {e}");
                return;
            }
        }
        if let Err(e) = visual::set_zoom_hook(on) {
            if !on && self.sensitivity {
                let _ = visual::set_zoom_sensitivity(true);
            }
            log!("zoom: {e}");
            return;
        }
        if on && self.sensitivity {
            if let Err(e) = visual::set_zoom_sensitivity(true) {
                let _ = visual::set_zoom_hook(false);
                log!("zoom: {e}");
                return;
            }
        }
        self.enabled = on;
    }

    fn tick(&mut self, tick: &Tick) {
        // Only while playing: the key types normally in chat and menus.
        visual::set_zooming(self.enabled && tick.active && input::key_down(self.hold_key));
    }

    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "magnification",
            value: self.factor,
            min: MIN_FACTOR,
            max: MAX_FACTOR,
        })
    }

    fn set_param(&mut self, value: f64) {
        self.factor = clamp_factor(value);
        visual::set_zoom_factor(self.factor as f32);
    }

    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        // Live state, so a zoom that does not come back can be told apart:
        // the key still seen as held, or the game not asking for the FOV.
        let (calls, _, current) = visual::zoom_stats();
        format!(
            "{:.1}倍（{}を押している間） / 現在{current:.2}倍・{} / getFov {calls}回",
            self.factor,
            input::key_name(self.hold_key),
            if visual::zooming() {
                "押下中"
            } else {
                "離している"
            },
        )
    }
}
