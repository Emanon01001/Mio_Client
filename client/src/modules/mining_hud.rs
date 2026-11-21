use super::{
    game::mining,
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log};
use hudhook::imgui::Ui;
pub struct MiningHud {
    cfg: config::MiningHud,
    key: u16,
    enabled: bool,
    sampled: u64,
    reading: Option<mining::Reading>,
    lines: Vec<String>,
}
impl MiningHud {
    pub fn new(cfg: &config::MiningHud) -> Self {
        Self {
            cfg: cfg.clone(),
            key: key_or_warn("mininghud", &cfg.key),
            enabled: false,
            sampled: 0,
            reading: None,
            lines: Vec::new(),
        }
    }
    fn rebuild(&mut self) {
        self.lines = vec!["MiningHUD".into()];
        if let Some(r) = &self.reading {
            self.lines
                .push(format!("{}  {:.0}%", r.name, r.progress * 100.));
            if self.cfg.coordinates {
                self.lines.push(format!(
                    "座標 {} / {} / {}",
                    r.position[0], r.position[1], r.position[2]
                ));
            }
        } else {
            self.lines.push("採掘待機中".into());
        }
    }
}
impl Module for MiningHud {
    fn name(&self) -> &str {
        "mininghud"
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
        if let Err(e) = mining::use_mining(on) {
            log!("MiningHUD: {e}");
            return;
        }
        self.enabled = on;
        self.sampled = 0;
        self.reading = None;
        self.rebuild();
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "coordinates",
                "座標も表示",
                "採掘中のブロックの座標を表示します。",
                self.cfg.coordinates,
            ),
            Detail::boolean(
                "hide_idle",
                "採掘していないときは隠す",
                "OFFにすると待機中もカードを表示します。",
                self.cfg.hide_idle,
            ),
            Detail::x_offset(self.cfg.x, "Negative: from right."),
            Detail::y_offset(self.cfg.y, "Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "coordinates" => self.cfg.coordinates = v.boolean()?,
            "hide_idle" => self.cfg.hide_idle = v.boolean()?,
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.rebuild();
        Ok(())
    }
    fn tick(&mut self, tick: &Tick) {
        if !self.enabled || tick.now_ms.saturating_sub(self.sampled) < 50 {
            return;
        }
        self.sampled = tick.now_ms;
        let r = mining::snapshot();
        if r != self.reading {
            self.reading = r;
            self.rebuild();
        }
    }
    fn status(&self) -> String {
        self.reading.as_ref().map_or_else(
            || "採掘待機中".into(),
            |r| format!("{} {:.0}%", r.name, r.progress * 100.),
        )
    }
    fn draw(&mut self, ui: &Ui) {
        if self.reading.is_none() && self.cfg.hide_idle {
            return;
        }
        draw(
            ui,
            [self.cfg.x, self.cfg.y],
            &self.lines,
            self.reading.as_ref().map(|r| r.progress),
        );
    }
}
fn draw(ui: &Ui, pos: [i32; 2], lines: &[String], value: Option<f32>) {
    let screen = ui.io().display_size;
    if screen[0] < 40. || screen[1] < 40. {
        return;
    }
    let width = (lines
        .iter()
        .map(|s| ui.calc_text_size(s)[0])
        .fold(210., f32::max)
        + 24.)
        .min((screen[0] - 12.).min(480.));
    let step = ui.text_line_height_with_spacing();
    let height = lines.len() as f32 * step + 34.;
    let x = super::radar::origin(pos[0], screen[0], width);
    let y = super::radar::origin(pos[1], screen[1], height);
    crate::hud_editor::measure("mininghud", [width, height]);
    let d = ui.get_background_draw_list();
    crate::theme::panel(&d, [x, y], [x + width, y + height]);
    for (i, s) in lines.iter().enumerate() {
        let s = crate::text::fit_width(s, width - 24., |s| ui.calc_text_size(s)[0]);
        d.add_text(
            [x + 12., y + 10. + i as f32 * step],
            if i == 0 {
                crate::theme::ACCENT
            } else {
                crate::theme::TEXT
            },
            &s,
        );
    }
    crate::theme::progress(
        &d,
        [x + 12., y + height - 14.],
        [width - 24., 5.],
        value.unwrap_or(0.),
        crate::theme::ACCENT,
    );
}
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    draw(
        ui,
        [-330, 750],
        &[
            "MiningHUD".into(),
            "diamond_ore  64%".into(),
            "座標 12 / -58 / 30".into(),
        ],
        Some(0.64),
    );
}
