//! Detached camera controls; native camera changes stay on its update callback.
use super::{
    game::{self, freecam as native},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log};

pub struct Freecam {
    key: u16,
    on: bool,
    speed: f64,
    boost: f64,
}
impl Freecam {
    pub fn new(c: &config::Freecam) -> Self {
        Self {
            key: key_or_warn("freecam", &c.key),
            on: false,
            speed: finite(c.speed, 10., 0.1, 100.),
            boost: finite(c.boost, 3., 1., 10.),
        }
    }
}
fn finite(value: f64, default: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}
impl Module for Freecam {
    fn name(&self) -> &str {
        "freecam"
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
        // As a camera user: the render camera override checks the renderer
        // objects learned while the camera is in use.
        let result = if on {
            native::available().and_then(|_| game::use_camera(true))
        } else {
            native::request(false, 0, self.speed as f32, self.boost as f32);
            game::use_camera(false)
        };
        if let Err(e) = result {
            log!("Freecam: {e}");
            return;
        }
        self.on = on;
    }
    fn tick(&mut self, t: &Tick) {
        native::request(self.on, t.now_ms, self.speed as f32, self.boost as f32);
    }
    fn status(&self) -> String {
        native::status()
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::number(
                "speed",
                "移動速度",
                "カメラだけの移動速度（ブロック/秒）。",
                self.speed,
                0.1,
                100.,
            ),
            Detail::number(
                "boost",
                "加速倍率",
                "Ctrlを押している間の速度倍率。",
                self.boost,
                1.,
                10.,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "speed" => self.speed = finite(value.number()?, 10., 0.1, 100.),
            "boost" => self.boost = finite(value.number()?, 3., 1., 10.),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn draw_settings(&mut self, ui: &hudhook::imgui::Ui) {
        ui.text_wrapped("W/A/S/Dで視点を移動、Spaceで上昇、Shiftで下降、Ctrlで加速します。プレイヤーへの移動・攻撃・使用入力は停止します。メニュー・チャット表示中はカメラ移動を止め、OFFにすると通常視点へ戻ります。FreeLookより優先します。");
    }
}
