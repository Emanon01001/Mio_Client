use crate::modules::settings::{Detail, Value};
use crate::modules::{
    game::{self, combat},
    key_or_warn, Module, Param, Tick,
};
use crate::{config, log};

pub struct KillAura {
    key: u16,
    enabled: bool,
    range: f64,
    cps: f64,
    max_targets: usize,
    players: bool,
    mobs: bool,
    fov: f64,
    line_of_sight: bool,
    hold_only: bool,
    unavailable: Option<String>,
    paused: Option<&'static str>,
}
impl KillAura {
    pub fn new(cfg: &config::KillAura) -> Self {
        let range = if cfg.range.is_finite() {
            cfg.range.clamp(1., 6.)
        } else {
            3.
        };
        let cps = if cfg.cps.is_finite() {
            cfg.cps.clamp(1., 20.)
        } else {
            8.
        };
        combat::configure(range as f32, cps, cfg.players, cfg.mobs);
        let fov = if cfg.fov.is_finite() {
            cfg.fov.clamp(1., 360.)
        } else {
            360.
        };
        combat::configure_limits(fov, cfg.line_of_sight, cfg.hold_only);
        let max_targets = cfg.max_targets.clamp(1, combat::MAX_TARGETS);
        combat::configure_targets(max_targets);
        Self {
            key: key_or_warn("killaura", &cfg.key),
            enabled: false,
            range,
            cps,
            max_targets,
            players: cfg.players,
            mobs: cfg.mobs,
            fov,
            line_of_sight: cfg.line_of_sight,
            hold_only: cfg.hold_only,
            unavailable: combat::available().err(),
            paused: None,
        }
    }
}
impl Module for KillAura {
    fn name(&self) -> &str {
        "killaura"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::number(
                "cps",
                "攻撃周期（回/秒）",
                "各周期で対象ごとに1回攻撃します。実際のダメージはゲーム側の判定に従います。",
                self.cps,
                1.,
                20.,
            ),
            Detail::boolean(
                "players",
                "Target players",
                "Allow player entities as targets.",
                self.players,
            ),
            Detail::boolean("mobs", "Target mobs", "Allow mobs as targets.", self.mobs),
            Detail::number("fov","攻撃対象の視野角","視線を中心にした角度です。360度で従来どおり全方向を対象にします。",self.fov,1.,360.),
            Detail::boolean("line_of_sight","壁越しに攻撃しない","選択した対象の手前にブロックがある場合は攻撃を見送ります。草・柵もブロック単位で判定します。",self.line_of_sight),
            Detail::boolean("hold_only","左ボタン長押し中だけ動作","ゲームの攻撃ボタンを押している間だけ自動攻撃します。",self.hold_only),
            Detail::integer("max_targets", "1周期の最大対象数", "1周期で近い対象から順番に攻撃します。1で従来の単体攻撃に戻ります。", self.max_targets as i32, 1, combat::MAX_TARGETS as i32),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "cps" => self.cps = value.number()?.clamp(1., 20.),
            "max_targets" => {
                self.max_targets = value
                    .number()?
                    .round()
                    .clamp(1., combat::MAX_TARGETS as f64)
                    as usize
            }
            "players" => self.players = value.boolean()?,
            "mobs" => self.mobs = value.boolean()?,
            "fov" => self.fov = value.number()?.clamp(1., 360.),
            "line_of_sight" => {
                let on = value.boolean()?;
                if on && self.enabled {
                    game::blocks::available()?;
                }
                self.line_of_sight = on;
            }
            "hold_only" => self.hold_only = value.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        combat::configure(self.range as f32, self.cps, self.players, self.mobs);
        combat::configure_limits(self.fov, self.line_of_sight, self.hold_only);
        combat::configure_targets(self.max_targets);
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        if let Some(e) = &self.unavailable {
            log!("killauraを利用できません: {e}");
            return;
        }
        if on && self.line_of_sight {
            if let Err(e) = game::blocks::available() {
                log!("KillAura: {e}");
                return;
            }
        }
        if !on {
            combat::request(false, 0);
        }
        if let Err(e) = if on { game::acquire() } else { game::release() } {
            log!("killaura: {e}");
            return;
        }
        self.enabled = on;
    }
    fn tick(&mut self, tick: &Tick) {
        self.paused = tick.paused;
        combat::request(self.enabled && tick.active, tick.now_ms);
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "range",
            value: self.range,
            min: 1.,
            max: 6.,
        })
    }
    fn set_param(&mut self, value: f64) {
        if value.is_finite() {
            self.range = value.clamp(1., 6.);
            combat::configure(self.range as f32, self.cps, self.players, self.mobs);
        }
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        if let Some(reason) = self.paused.filter(|_| self.enabled) {
            return format!("一時停止: {reason}");
        }
        format!(
            "射程{:.1}ブロック / {:.1}周期/秒 / 最大{}体",
            self.range, self.cps, self.max_targets
        )
    }
}
