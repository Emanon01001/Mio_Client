use super::settings::{Detail, Value};
use super::{game, key_or_warn, Module, Tick};
use crate::{config, log};

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
