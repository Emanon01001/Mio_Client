//! A configurable crosshair drawn on top of the normal game view.
use super::{
    game, key_or_warn,
    settings::{Detail, Value},
    Module, Param, Tick,
};
use crate::{api, config, log};
use hudhook::imgui::Ui;

pub struct Crosshair {
    cfg: config::Crosshair,
    key: u16,
    enabled: bool,
    visible: bool,
    spread: f32,
    sampled: u64,
}

fn number(v: f64, default: f64, min: f64, max: f64) -> f64 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        default
    }
}
impl Crosshair {
    pub fn new(cfg: &config::Crosshair) -> Self {
        let mut cfg = cfg.clone();
        cfg.size = number(cfg.size, 6., 2., 32.);
        cfg.gap = number(cfg.gap, 4., 0., 24.);
        cfg.thickness = number(cfg.thickness, 2., 1., 6.);
        cfg.opacity = number(cfg.opacity, 0.9, 0.1, 1.);
        Self {
            key: key_or_warn("crosshair", &cfg.key),
            cfg,
            enabled: false,
            visible: false,
            spread: 0.,
            sampled: 0,
        }
    }
}
impl Module for Crosshair {
    fn name(&self) -> &str {
        "crosshair"
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
        let result = if on { game::acquire() } else { game::release() };
        if let Err(e) = result {
            log!("crosshair: {e}");
            return;
        }
        self.enabled = on;
        self.visible = false;
        self.sampled = 0;
        self.spread = 0.;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "size",
            value: self.cfg.size,
            min: 2.,
            max: 32.,
        })
    }
    fn set_param(&mut self, v: f64) {
        self.cfg.size = number(v, 6., 2., 32.);
    }
    fn options(&self) -> Vec<Detail> {
        let c = &self.cfg;
        vec![
            Detail::number(
                "gap",
                "Center gap",
                "Pixels from the center to each arm.",
                c.gap,
                0.,
                24.,
            ),
            Detail::number(
                "thickness",
                "Thickness",
                "Line width in pixels.",
                c.thickness,
                1.,
                6.,
            ),
            Detail::boolean(
                "dot",
                "Center dot",
                "Draw a dot at the screen center.",
                c.dot,
            ),
            Detail::boolean(
                "outline",
                "Black outline",
                "Keep the crosshair visible on bright blocks.",
                c.outline,
            ),
            Detail::boolean("t_style", "T shape", "Leave out the top arm.", c.t_style),
            Detail::boolean(
                "dynamic",
                "Movement spread",
                "Widen the gap while moving; visual only, not weapon accuracy.",
                c.dynamic,
            ),
            Detail::integer("red", "Red", "Color channel.", c.color[0] as i32, 0, 255),
            Detail::integer(
                "green",
                "Green",
                "Color channel.",
                c.color[1] as i32,
                0,
                255,
            ),
            Detail::integer("blue", "Blue", "Color channel.", c.color[2] as i32, 0, 255),
            Detail::number("opacity", "Opacity", "Overlay opacity.", c.opacity, 0.1, 1.),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        let c = &mut self.cfg;
        match id {
            "gap" => c.gap = number(v.number()?, 4., 0., 24.),
            "thickness" => c.thickness = number(v.number()?, 2., 1., 6.),
            "dot" => c.dot = v.boolean()?,
            "outline" => c.outline = v.boolean()?,
            "t_style" => c.t_style = v.boolean()?,
            "dynamic" => {
                c.dynamic = v.boolean()?;
                self.spread = 0.;
            }
            "red" | "green" | "blue" => {
                c.color[match id {
                    "red" => 0,
                    "green" => 1,
                    _ => 2,
                }] = v.number()?.clamp(0., 255.) as u8
            }
            "opacity" => c.opacity = number(v.number()?, 0.9, 0.1, 1.),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, tick: &Tick) {
        self.visible = self.enabled && tick.active && super::hud_view();
        if !self.visible || tick.now_ms.saturating_sub(self.sampled) < 50 {
            return;
        }
        self.sampled = tick.now_ms;
        self.spread = if self.cfg.dynamic {
            api::game()
                .ok()
                .and_then(|g| g.player().ok())
                .map_or(0., |p| (p.horizontal_speed() * 0.8).clamp(0., 12.))
        } else {
            0.
        };
    }
    fn status(&self) -> String {
        format!(
            "{:.0} px{}",
            self.cfg.size,
            if self.cfg.dynamic {
                " 移動連動"
            } else {
                ""
            }
        )
    }
    fn draw(&mut self, ui: &Ui) {
        if !self.visible || !super::hud_view() {
            return;
        }
        let size = ui.io().display_size;
        let center = [size[0] * 0.5, size[1] * 0.5];
        let c = &self.cfg;
        let gap = c.gap as f32 + self.spread;
        let length = c.size as f32;
        let color = [
            c.color[0] as f32 / 255.,
            c.color[1] as f32 / 255.,
            c.color[2] as f32 / 255.,
            c.opacity as f32,
        ];
        let black = [0., 0., 0., c.opacity as f32];
        let d = ui.get_background_draw_list();
        for direction in [[-1., 0.], [1., 0.], [0., 1.], [0., -1.]] {
            if c.t_style && direction[1] < 0. {
                continue;
            }
            let a = std::array::from_fn(|k| center[k] + direction[k] * gap);
            let b = std::array::from_fn(|k| center[k] + direction[k] * (gap + length));
            if c.outline {
                d.add_line(a, b, black)
                    .thickness(c.thickness as f32 + 2.)
                    .build();
            }
            d.add_line(a, b, color)
                .thickness(c.thickness as f32)
                .build();
        }
        if c.dot {
            let radius = (c.thickness as f32 * 0.5).max(1.);
            if c.outline {
                d.add_circle(center, radius + 1., black)
                    .filled(true)
                    .build();
            }
            d.add_circle(center, radius, color).filled(true).build();
        }
    }
}
