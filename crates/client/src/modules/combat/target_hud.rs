use crate::modules::settings::{Detail, Value};
use crate::modules::{
    game::{actors, combat},
    hud::origin,
    key_or_warn, Module,
};
use crate::{api, config, log};
use hudhook::imgui::Ui;
pub struct TargetHud {
    key: u16,
    enabled: bool,
    x: i32,
    y: i32,
    last: Option<combat::TargetSnapshot>,
    title: String,
    detail: String,
    counts: String,
    health: bool,
}
impl TargetHud {
    pub fn new(cfg: &config::TargetHud) -> Self {
        Self {
            key: key_or_warn("targethud", &cfg.key),
            enabled: false,
            x: cfg.x,
            y: cfg.y,
            last: None,
            title: String::new(),
            detail: String::new(),
            counts: String::new(),
            health: cfg.health,
        }
    }
}
impl Module for TargetHud {
    fn name(&self) -> &str {
        "targethud"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "health",
                "Health",
                "Show current and maximum HP when available.",
                self.health,
            ),
            Detail::x_offset(self.x, "Positive: from left. Negative: from right."),
            Detail::y_offset(self.y, "Positive: from top. Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "health" => self.health = value.boolean()?,
            "x" => self.x = value.number()? as i32,
            "y" => self.y = value.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        if let Err(e) = actors::use_actors(on) {
            log!("targethud: {e}");
            return;
        }
        self.enabled = on;
        self.last = None;
    }
    fn draw(&mut self, ui: &Ui) {
        let Some(target) = combat::target_snapshot() else {
            self.last = None;
            return;
        };
        if api::game().ok().and_then(|g| g.player().ok()).is_none() {
            self.last = None;
            return;
        }
        if self.last != Some(target) {
            self.title = if target.actor_type & 0xFF == 63 {
                "対象: プレイヤー"
            } else {
                "対象: モブ"
            }
            .into();
            self.detail = format!("{:.2}ブロック   ID {:X}", target.distance, target.entity);
            self.counts = format!(
                "攻撃{}回 / 成功{}回 / 腕振り{}回",
                target.attacks, target.accepted, target.swings
            );
            self.last = Some(target);
        }
        let snapshot = actors::actors();
        let actor = snapshot.as_ref().and_then(|map| map.get(&target.entity));
        let hp = if self.health {
            actor.and_then(|a| a.health)
        } else {
            None
        };
        let title = actor.map_or_else(
            || self.title.clone(),
            |a| format!("対象: {}", a.display_name.as_deref().unwrap_or(&a.name)),
        );
        let hp_text = hp.map(|(value, max)| format!("HP {value:.1}/{max:.1}"));
        let width = ui.calc_text_size(&self.counts)[0]
            .max(ui.calc_text_size(&self.detail)[0])
            .max(ui.calc_text_size(&title)[0])
            .max(hp_text.as_ref().map_or(0., |s| ui.calc_text_size(s)[0]))
            .max(210.)
            + 24.;
        let h = ui.text_line_height_with_spacing();
        let height = h * if hp.is_some() { 4. } else { 3. } + if hp.is_some() { 30. } else { 22. };
        crate::hud_editor::measure("targethud", [width, height]);
        let screen = ui.io().display_size;
        let x = origin(self.x, screen[0], width);
        let y = origin(self.y, screen[1], height);
        let d = ui.get_background_draw_list();
        crate::theme::panel(&d, [x, y], [x + width, y + height]);
        for (i, text) in [&title, &self.detail, &self.counts].into_iter().enumerate() {
            d.add_text(
                [x + 12., y + 10. + i as f32 * h],
                if i == 0 {
                    crate::theme::TEXT
                } else {
                    crate::theme::MUTED
                },
                text,
            );
        }
        if let (Some((value, max)), Some(text)) = (hp, hp_text) {
            d.add_text([x + 12., y + 10. + 3. * h], crate::theme::TEXT, text);
            let by = y + height - 11.;
            crate::theme::progress(
                &d,
                [x + 12., by],
                [width - 24., 4.],
                value / max,
                crate::theme::SUCCESS,
            );
        }
    }
}
