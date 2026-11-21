use super::{
    game, key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, gui, log};
const KEY_IDS: [(&str, &str); 6] = [
    ("forward_key", "Forward key"),
    ("backward_key", "Backward key"),
    ("left_key", "Left key"),
    ("right_key", "Right key"),
    ("jump_key", "Jump key"),
    ("sneak_key", "Sneak key"),
];
pub struct InventoryMove {
    key: u16,
    enabled: bool,
    keys: [u16; 6],
    jump: bool,
    sneak: bool,
    automatic: bool,
    error: Option<String>,
}
impl InventoryMove {
    pub fn new(cfg: &config::InventoryMove) -> Self {
        let keys = [
            &cfg.forward_key,
            &cfg.backward_key,
            &cfg.left_key,
            &cfg.right_key,
            &cfg.jump_key,
            &cfg.sneak_key,
        ]
        .map(|key| key_or_warn("inventorymove", key));
        for (i, key) in keys.into_iter().enumerate() {
            game::inventory::set_key(i, key);
        }
        game::inventory::configure(cfg.jump, cfg.sneak, cfg.automatic);
        Self {
            key: key_or_warn("inventorymove", &cfg.key),
            enabled: false,
            keys,
            jump: cfg.jump,
            sneak: cfg.sneak,
            automatic: cfg.automatic,
            error: None,
        }
    }
}
impl Module for InventoryMove {
    fn name(&self) -> &str {
        "inventorymove"
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
        if on {
            if let Err(e) = game::actions::available()
                .and_then(|_| game::inventory::available())
                .and_then(|_| game::acquire())
            {
                log!("inventorymove: {e}");
                self.error = Some(e);
                return;
            }
        } else {
            game::inventory::request(false, 0);
            if let Err(e) = game::release() {
                self.error = Some(e);
                return;
            }
        }
        self.enabled = on;
        self.error = None;
    }
    fn tick(&mut self, tick: &Tick) {
        game::inventory::request(
            self.enabled && tick.focused && !gui::captures_input(),
            tick.now_ms,
        );
    }
    fn options(&self) -> Vec<Detail> {
        let mut details = vec![
            Detail::boolean("jump", "Allow jump key", "Use the jump key while the inventory is open.", self.jump),
            Detail::boolean("sneak", "Allow sneak key", "Enable if desired. Leave off to use Shift for inventory item transfers without crouching.", self.sneak),
            Detail::boolean("automatic", "Continue movement helpers", "Continue enabled movement helpers in the inventory. Chat and the client menu still pause them.", self.automatic),
        ];
        for (i, (id, label)) in KEY_IDS.into_iter().enumerate() {
            details.push(Detail::key(
                id,
                label,
                "Match this to your Minecraft movement binding.",
                self.keys[i],
            ));
        }
        details
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        if let Some(i) = KEY_IDS.iter().position(|(key, _)| *key == id) {
            self.keys[i] = value.key()?;
            game::inventory::set_key(i, self.keys[i]);
        } else {
            match id {
                "jump" => self.jump = value.boolean()?,
                "sneak" => self.sneak = value.boolean()?,
                "automatic" => self.automatic = value.boolean()?,
                _ => return Err(format!("不明な設定項目: {id}")),
            }
            game::inventory::configure(self.jump, self.sneak, self.automatic);
        }
        Ok(())
    }
    fn status(&self) -> String {
        self.error.clone().unwrap_or_else(game::inventory::status)
    }
}
