//! Continuous movement through vanilla button tracking; no simulated keys:
//! AutoWalk/AutoSneak/AutoJump/AutoSwim, and AutoSprint below.
use crate::modules::settings::{Detail, Value};
use crate::modules::{game, key_or_warn, Module, Tick};
use crate::{config, log};

enum Mode {
    Walk,
    Sneak,
    Jump,
    Swim,
}
pub struct AutoMove {
    mode: Mode,
    key: u16,
    enabled: bool,
    option: bool,
    unavailable: Option<String>,
    paused: Option<&'static str>,
}
impl AutoMove {
    pub fn walk(cfg: &config::AutoWalk) -> Self {
        game::actions::set_walk_pause_back(cfg.pause_back);
        Self::new(Mode::Walk, &cfg.key, cfg.pause_back)
    }
    pub fn sneak(cfg: &config::AutoSneak) -> Self {
        game::actions::set_sneak_forward_only(cfg.forward_only);
        Self::new(Mode::Sneak, &cfg.key, cfg.forward_only)
    }
    pub fn jump(cfg: &config::AutoJump) -> Self {
        game::actions::set_jump_forward_only(cfg.forward_only);
        Self::new(Mode::Jump, &cfg.key, cfg.forward_only)
    }
    pub fn swim(cfg: &config::AutoSwim) -> Self {
        game::actions::set_swim_forward_only(cfg.forward_only);
        Self::new(Mode::Swim, &cfg.key, cfg.forward_only)
    }
    fn new(mode: Mode, key: &str, option: bool) -> Self {
        let name = match mode {
            Mode::Walk => "autowalk",
            Mode::Sneak => "autosneak",
            Mode::Jump => "autojump",
            Mode::Swim => "autoswim",
        };
        Self {
            mode,
            key: key_or_warn(name, key),
            enabled: false,
            option,
            unavailable: game::actions::available().err(),
            paused: None,
        }
    }
    fn request(&self, active: bool, now: u64) {
        match self.mode {
            Mode::Walk => game::actions::walk(active, now),
            Mode::Sneak => game::actions::sneak(active, now),
            Mode::Jump => game::actions::auto_jump(active, now),
            Mode::Swim => game::actions::auto_swim(active, now),
        }
    }
}
impl Module for AutoMove {
    fn name(&self) -> &str {
        match self.mode {
            Mode::Walk => "autowalk",
            Mode::Sneak => "autosneak",
            Mode::Jump => "autojump",
            Mode::Swim => "autoswim",
        }
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
            log!("{} unavailable: {e}", self.name());
            return;
        }
        if !on {
            self.request(false, 0);
        }
        if let Err(e) = if on { game::acquire() } else { game::release() } {
            log!("{}: {e}", self.name());
            return;
        }
        self.enabled = on;
    }
    fn tick(&mut self, tick: &Tick) {
        let active = game::inventory::movement_active(tick.active, tick.focused, tick.now_ms);
        self.paused = if active { None } else { tick.paused };
        self.request(self.enabled && active, tick.now_ms);
    }
    fn options(&self) -> Vec<Detail> {
        vec![match self.mode {
            Mode::Walk => Detail::boolean("pause_back", "Pause while moving backward", "Temporarily release automatic forward input while a backward movement button is held.", self.option),
            Mode::Sneak => Detail::boolean("forward_only", "Only while moving forward", "Sneak only while a forward movement button is active, including AutoWalk.", self.option),
            Mode::Jump => Detail::boolean("forward_only", "Only while moving forward", "Jump on landing without sprinting. Disable this to also jump while standing still. Sneak pauses it.", self.option),
            Mode::Swim => Detail::boolean("forward_only", "Only while moving forward", "Float upward while your head is underwater. Sneak pauses floating so you can dive.", self.option),
        }]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        let value = value.boolean()?;
        match (&self.mode, id) {
            (Mode::Walk, "pause_back") => game::actions::set_walk_pause_back(value),
            (Mode::Sneak, "forward_only") => game::actions::set_sneak_forward_only(value),
            (Mode::Jump, "forward_only") => game::actions::set_jump_forward_only(value),
            (Mode::Swim, "forward_only") => game::actions::set_swim_forward_only(value),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.option = value;
        Ok(())
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        if !self.enabled {
            return "通常の移動入力".into();
        }
        if let Some(reason) = self.paused {
            return format!("一時停止: {reason}");
        }
        match self.mode {
            Mode::Walk | Mode::Sneak => {
                game::actions::assist_status(matches!(self.mode, Mode::Walk))
            }
            Mode::Jump | Mode::Swim => {
                game::actions::jump_assist_status(matches!(self.mode, Mode::Swim))
            }
        }
    }
}

// ---- AutoSprint ------------------------------------------------------------

/// Requests sprint through the game's MoveInputComponent.
pub struct AutoSprint {
    key: u16,
    enabled: bool,
    jump: bool,
    unavailable: Option<String>,
    /// Why no input is being sent right now, if so.
    paused: Option<&'static str>,
}

impl AutoSprint {
    pub fn new(cfg: &config::AutoSprint) -> Self {
        game::actions::set_sprint_jump(cfg.jump);
        Self {
            key: key_or_warn("autosprint", &cfg.key),
            enabled: false,
            jump: cfg.jump,
            unavailable: game::actions::available().err(),
            paused: None,
        }
    }
}

impl Module for AutoSprint {
    fn name(&self) -> &str {
        "autosprint"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn options(&self) -> Vec<Detail> {
        vec![Detail::boolean(
            "jump",
            "Jump on ground",
            "Automatically jump only while touching the ground. Manual Space is unaffected.",
            self.jump,
        )]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "jump" => {
                self.jump = value.boolean()?;
                game::actions::set_sprint_jump(self.jump);
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
            log!("autosprintを利用できません: {e}");
            return;
        }
        if !on {
            game::actions::sprint(false, 0);
        }
        let result = if on { game::acquire() } else { game::release() };
        if let Err(e) = result {
            log!("autosprint: {e}");
            return;
        }
        self.enabled = on;
    }

    fn tick(&mut self, tick: &Tick) {
        let active = game::inventory::movement_active(tick.active, tick.focused, tick.now_ms);
        self.paused = if active { None } else { tick.paused };
        game::actions::sprint(self.enabled && active, tick.now_ms);
    }

    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        match self.paused {
            Some(reason) if self.enabled => format!("（一時停止: {reason}）"),
            _ => game::actions::status(true),
        }
    }
}
