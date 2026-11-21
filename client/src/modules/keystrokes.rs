//! Keystrokes: shows the movement keys and mouse buttons in the overlay,
//! lit while held, with clicks per second on the mouse buttons.
//!
//! Reads the physical keys only (default bindings: W/A/S/D, Space).

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use hudhook::imgui::Ui;

use super::settings::{Detail, Value};
use super::{game, hud, key_or_warn, Module};
use crate::{config, input};

const CELL: f32 = 40.0;
const GAP: f32 = 4.0;
const SPACE_HEIGHT: f32 = 18.0;
const WIDTH: f32 = CELL * 3.0 + GAP * 2.0;
const HEIGHT: f32 = CELL * 3.0 + SPACE_HEIGHT + GAP * 3.0;

const IDLE_FILL: [f32; 4] = crate::theme::SURFACE;
const HELD_FILL: [f32; 4] = crate::theme::ACCENT;
const IDLE_TEXT: [f32; 4] = crate::theme::TEXT;
const HELD_TEXT: [f32; 4] = [0.035, 0.052, 0.078, 1.];

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Configured position; see `origin`.
static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);

/// Top-left corner of a `length`-long widget on an axis `screen` long. A
/// non-negative offset is measured from the left/top edge; a negative one from
/// the right/bottom edge to the widget's far side.
fn origin(offset: i32, screen: f32, length: f32) -> f32 {
    super::radar::origin(offset, screen, length)
}

/// Draws the widget for this frame. Called from the overlay's render loop.
pub fn draw(ui: &Ui) {
    // Only in the world view, like the other in-world overlays.
    if !ACTIVE.load(Ordering::Relaxed) || game::position().is_none() {
        return;
    }
    let size = ui.io().display_size;
    crate::hud_editor::measure("keystrokes", [WIDTH, HEIGHT]);
    let left = origin(X.load(Ordering::Relaxed), size[0], WIDTH);
    let top = origin(Y.load(Ordering::Relaxed), size[1], HEIGHT);
    let focused = input::is_foreground();
    let held = |vk: u16| focused && input::key_down(vk);
    let (left_cps, right_cps) = hud::cps();
    let draw_list = ui.get_background_draw_list();

    let key = |x: f32, y: f32, w: f32, h: f32, label: &str, down: bool| {
        let (fill, text) = if down {
            (HELD_FILL, HELD_TEXT)
        } else {
            (IDLE_FILL, IDLE_TEXT)
        };
        let min = [left + x, top + y];
        draw_list
            .add_rect(min, [min[0] + w, min[1] + h], fill)
            .filled(true)
            .rounding(7.0)
            .build();
        let label_size = ui.calc_text_size(label);
        draw_list.add_text(
            [
                min[0] + (w - label_size[0]) * 0.5,
                min[1] + (h - label_size[1]) * 0.5,
            ],
            text,
            label,
        );
    };

    let step = CELL + GAP;
    key(step, 0.0, CELL, CELL, "W", held(0x57));
    key(0.0, step, CELL, CELL, "A", held(0x41));
    key(step, step, CELL, CELL, "S", held(0x53));
    key(step * 2.0, step, CELL, CELL, "D", held(0x44));
    let half = (WIDTH - GAP) * 0.5;
    key(
        0.0,
        step * 2.0,
        half,
        CELL,
        &format!("L {left_cps}"),
        held(0x01),
    );
    key(
        half + GAP,
        step * 2.0,
        half,
        CELL,
        &format!("R {right_cps}"),
        held(0x02),
    );
    key(0.0, step * 3.0, WIDTH, SPACE_HEIGHT, "", held(0x20));
}

pub struct Keystrokes {
    key: u16,
    enabled: bool,
}

impl Keystrokes {
    pub fn new(cfg: &config::Keystrokes) -> Self {
        X.store(cfg.x, Ordering::Relaxed);
        Y.store(cfg.y, Ordering::Relaxed);
        Self {
            key: key_or_warn("keystrokes", &cfg.key),
            enabled: false,
        }
    }
}

impl Module for Keystrokes {
    fn name(&self) -> &str {
        "keystrokes"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::integer(
                "x",
                "X offset",
                "Positive: from left. Negative: from right.",
                X.load(Ordering::Relaxed),
                -7680,
                7680,
            ),
            Detail::integer(
                "y",
                "Y offset",
                "Positive: from top. Negative: from bottom.",
                Y.load(Ordering::Relaxed),
                -4320,
                4320,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "x" => X.store(value.number()? as i32, Ordering::Relaxed),
            "y" => Y.store(value.number()? as i32, Ordering::Relaxed),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        ACTIVE.store(on, Ordering::Relaxed);
        self.enabled = on;
    }
}

#[cfg(test)]
mod tests {
    use super::origin;

    #[test]
    fn negative_offsets_anchor_to_the_far_edge() {
        assert_eq!(origin(10, 1920.0, 128.0), 10.0);
        assert_eq!(origin(0, 1920.0, 128.0), 0.0);
        // 10 px between the widget's right side and the screen's right edge.
        assert_eq!(origin(-10, 1920.0, 128.0), 1920.0 - 10.0 - 128.0);
    }
}
