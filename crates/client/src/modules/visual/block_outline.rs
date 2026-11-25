//! Draw a colored outline around the targeted block cell using the shared ray.
use crate::game::{self, blocks};
use crate::modules::settings::{Detail, Value};
use crate::modules::view::{box_lines, BoxPainter};
use crate::modules::{key_or_warn, Module};
use crate::{config, gui, log};
use hudhook::imgui::Ui;

pub(in crate::modules) struct BlockOutline {
    key: u16,
    enabled: bool,
    color: [u8; 4],
    thickness: f64,
}

impl BlockOutline {
    pub(in crate::modules) fn new(config: &config::BlockOutline) -> Self {
        Self {
            key: key_or_warn("blockoutline", &config.key),
            enabled: false,
            color: config.color,
            thickness: if config.thickness.is_finite() {
                config.thickness.clamp(1., 6.)
            } else {
                2.
            },
        }
    }
}

impl Module for BlockOutline {
    fn name(&self) -> &str {
        "blockoutline"
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
        let result = if on {
            blocks::available().and_then(|_| game::use_camera(true))
        } else {
            game::use_camera(false)
        };
        if let Err(error) = result {
            log!("Block Outline: {error}");
            return;
        }
        blocks::set_outline(on);
        self.enabled = on;
    }

    fn options(&self) -> Vec<Detail> {
        let [r, g, b, a] = self.color;
        vec![
            Detail::number(
                "thickness",
                "枠の太さ",
                "画面上の線幅を調整します。",
                self.thickness,
                1.,
                6.,
            ),
            Detail::text("color", format!("{r:02X}{g:02X}{b:02X}{a:02X}")),
        ]
    }

    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "thickness" => self.thickness = value.number()?.clamp(1., 6.),
            "color" => {
                let color = value.text()?;
                if color.len() != 8 || !color.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("色は8桁の16進数で指定してください".into());
                }
                self.color = u32::from_str_radix(color, 16)
                    .map_err(|e| e.to_string())?
                    .to_be_bytes();
            }
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }

    fn draw_settings(&mut self, ui: &Ui) {
        let mut color = self.color.map(|v| f32::from(v) / 255.);
        if ui.color_edit4("枠の色", &mut color) {
            self.color = color.map(|v| (v * 255.).round() as u8);
        }
    }

    fn draw(&mut self, ui: &Ui) {
        if !self.enabled || gui::captures_input() || !game::in_world_view_now() {
            return;
        }
        let Some((position, camera)) = blocks::outline_position().zip(game::camera()) else {
            return;
        };
        let min = position.map(|v| v as f32);
        let max = min.map(|v| v + 1.);
        if let Some(lines) = box_lines(&camera, ui.io().display_size, min, max) {
            BoxPainter::new(ui).draw(
                &lines,
                self.color.map(|v| f32::from(v) / 255.),
                self.thickness as f32,
            );
        }
    }
}
