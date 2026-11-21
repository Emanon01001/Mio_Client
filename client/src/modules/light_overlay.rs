//! Read-only lighting candidates with a clipped floor mesh and compact badges.
mod render;
use super::{
    game::{self, blocks::lighting},
    key_or_warn,
    settings::{Detail, Value},
    Module, Param,
};
use crate::{config, log};
use hudhook::imgui::Ui;
pub struct LightOverlay {
    key: u16,
    on: bool,
    cfg: config::LightOverlay,
}
fn finite(value: f64, default: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}
impl LightOverlay {
    pub fn new(c: &config::LightOverlay) -> Self {
        let mut cfg = c.clone();
        cfg.radius = cfg.radius.clamp(4, 12);
        cfg.threshold = cfg.threshold.clamp(0, 15);
        cfg.opacity = finite(cfg.opacity, 0.12, 0., 0.4);
        cfg.line_width = finite(cfg.line_width, 1.5, 0.5, 3.);
        cfg.label_range = finite(cfg.label_range, 6., 2., 12.);
        cfg.max_labels = cfg.max_labels.clamp(1, 64);
        Self {
            key: key_or_warn("lightoverlay", &c.key),
            on: false,
            cfg,
        }
    }
}
impl Module for LightOverlay {
    fn name(&self) -> &str {
        "lightoverlay"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.on
    }
    fn set_enabled(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        let result = if on {
            lighting::available().and_then(|_| game::use_camera(true))
        } else {
            game::use_camera(false)
        };
        if let Err(e) = result {
            log!("LightOverlay: {e}");
            return;
        }
        lighting::configure(self.cfg.radius);
        lighting::set_active(on);
        self.on = on;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "radius",
            value: self.cfg.radius as f64,
            min: 4.,
            max: 12.,
        })
    }
    fn set_param(&mut self, v: f64) {
        if v.is_finite() {
            self.cfg.radius = v.clamp(4., 12.) as i32;
            lighting::configure(self.cfg.radius);
        }
    }
    fn status(&self) -> String {
        format!(
            "半径{}m / 明るさ{}以下",
            self.cfg.radius, self.cfg.threshold
        )
    }
    fn options(&self) -> Vec<Detail> {
        vec![
  Detail::integer("threshold","明るさの上限","赤は現在暗い床、黄は空の光だけで明るい床の候補です。Mob出現の全条件は判定しません。",self.cfg.threshold,0,15),
  Detail::boolean("filled","床を薄く塗る","地形が見える半透明のタイルで暗い床を示します。",self.cfg.filled),
  Detail::number("opacity","塗りの濃さ","床の色に重ねる濃さです。",self.cfg.opacity,0.,0.4),
  Detail::number("line_width","枠の太さ","角の目印と枠線の太さを変えます。",self.cfg.line_width,0.5,3.),
  Detail::boolean("labels","明るさを表示","近くの床にブロック光 | 現在の明るさを表示します。文字が重なる場所は省きます。",self.cfg.labels),
  Detail::number("label_range","数値の表示距離","カメラからこの距離以内だけに数値を表示します。",self.cfg.label_range,2.,12.),
  Detail::integer("max_labels","数値の表示上限","画面が文字で埋まらないように表示数を制限します。",self.cfg.max_labels,1,64),
  Detail::boolean("legend","色の説明を表示","色ごとの床の件数と、数値の意味を表示します。HUD編集で移動できます。",self.cfg.legend),
  Detail::x_offset(self.cfg.x, "Negative: from right."),
  Detail::y_offset(self.cfg.y, "Negative: from bottom."),
 ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "threshold" => self.cfg.threshold = v.number()?.clamp(0., 15.) as i32,
            "filled" => self.cfg.filled = v.boolean()?,
            "opacity" => self.cfg.opacity = v.number()?,
            "line_width" => self.cfg.line_width = v.number()?,
            "labels" => self.cfg.labels = v.boolean()?,
            "label_range" => self.cfg.label_range = v.number()?,
            "max_labels" => self.cfg.max_labels = v.number()?.clamp(1., 64.) as i32,
            "legend" => self.cfg.legend = v.boolean()?,
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn draw(&mut self, ui: &Ui) {
        if !super::hud_view() {
            return;
        }
        let (Some(camera), Some(cells), Some((x, y, z))) =
            (game::camera(), lighting::snapshot(), game::position())
        else {
            return;
        };
        render::draw(
            ui,
            &camera,
            &cells,
            [x as f32 + 0.5, y as f32, z as f32 + 0.5],
            &self.cfg,
        );
    }
}
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    render::preview(ui);
}
