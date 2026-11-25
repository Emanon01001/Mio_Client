//! Camera features: Freecam (a detached camera) and FreeLook (looking around
//! without turning), below. Native camera changes stay on their update callbacks.
use crate::modules::{
    game::{self, freecam as native},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, input, log};

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

// ---- FreeLook ------------------------------------------------------------
pub struct FreeLook {
    key: u16,
    hold: u16,
    on: bool,
}
impl FreeLook {
    pub fn new(c: &config::FreeLook) -> Self {
        Self {
            key: key_or_warn("freelook", &c.key),
            hold: key_or_warn("freelook.hold_key", &c.hold_key),
            on: false,
        }
    }
}
impl Module for FreeLook {
    fn name(&self) -> &str {
        "freelook"
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
        if let Err(e) = if on {
            game::freelook::available().and_then(|_| game::acquire())
        } else {
            game::release()
        } {
            log!("FreeLook: {e}");
            return;
        }
        self.on = on;
        if !on {
            game::freelook::request(false, 0);
        }
    }
    fn tick(&mut self, t: &Tick) {
        game::freelook::request(
            self.on && t.active && !game::inventory::screen_open() && input::key_down(self.hold),
            t.now_ms,
        );
    }
    fn status(&self) -> String {
        if game::freelook::active() {
            "見回し中".into()
        } else {
            format!("{}を押して見回す", input::key_name(self.hold))
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::key(
            "hold_key",
            "見回しキー",
            "押している間はカメラだけを回し、離すと元の向きへ戻します。",
            self.hold,
        )]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        if id != "hold_key" {
            return Err(format!("不明な設定項目: {id}"));
        }
        self.hold = v.key()?;
        game::freelook::request(false, 0);
        Ok(())
    }
}
