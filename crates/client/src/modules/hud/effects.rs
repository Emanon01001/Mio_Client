//! Status-effect names, levels and remaining game time, without wall-clock extrapolation.
use crate::modules::{
    game::status::{self, Effect},
    hud::origin,
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log, notify};
use hudhook::imgui::Ui;
use std::collections::HashSet;

fn name(id: u32) -> String {
    const NAMES: [&str; 37] = [
        "なし",
        "移動速度上昇",
        "移動速度低下",
        "採掘速度上昇",
        "採掘速度低下",
        "攻撃力上昇",
        "即時回復",
        "即時ダメージ",
        "跳躍力上昇",
        "吐き気",
        "再生能力",
        "耐性",
        "火炎耐性",
        "水中呼吸",
        "透明化",
        "盲目",
        "暗視",
        "空腹",
        "弱体化",
        "毒",
        "衰弱",
        "体力増強",
        "衝撃吸収",
        "満腹",
        "浮遊",
        "致死毒",
        "コンジットパワー",
        "低速落下",
        "不吉な予感",
        "村の英雄",
        "暗闇",
        "試練の予感",
        "蓄風",
        "巣張り",
        "滲出",
        "虫食い",
        "襲撃の予感",
    ];
    NAMES
        .get(id as usize)
        .map_or_else(|| format!("効果 {id}"), |s| s.to_string())
}
fn time(ticks: Option<u32>) -> String {
    ticks.map_or_else(
        || "無限".into(),
        |n| {
            let seconds = (n as u64).div_ceil(20);
            format!("{}:{:02}", seconds / 60, seconds % 60)
        },
    )
}
fn line(e: &Effect, duration: bool, amplifier: bool) -> String {
    let mut text = name(e.id);
    if amplifier {
        const ROMAN: [&str; 10] = ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"];
        text.push(' ');
        text.push_str(
            &ROMAN
                .get(e.amplifier as usize)
                .map_or_else(|| (e.amplifier + 1).to_string(), |s| s.to_string()),
        );
    }
    if duration {
        text.push_str(&format!("   {}", time(e.ticks)));
    }
    text
}
#[derive(Default)]
struct Expiration {
    warned: HashSet<u32>,
    last: Option<u64>,
}
impl Expiration {
    fn update(&mut self, effects: &[Effect], threshold: u32, now: u64) -> Vec<u32> {
        let near: HashSet<u32> = effects
            .iter()
            .filter(|e| e.ticks.is_some_and(|t| t > 0 && t <= threshold))
            .map(|e| e.id)
            .collect();
        self.warned.retain(|id| near.contains(id));
        if self.last.is_some_and(|at| now.saturating_sub(at) < 2000) {
            return Vec::new();
        }
        let mut fresh: Vec<u32> = near.difference(&self.warned).copied().collect();
        fresh.sort_unstable();
        if !fresh.is_empty() {
            self.warned.extend(fresh.iter().copied());
            self.last = Some(now);
        }
        fresh
    }
}
pub struct Effects {
    cfg: config::Effects,
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    expiry: Expiration,
    sampled: u64,
}
impl Effects {
    pub fn new(c: &config::Effects) -> Self {
        let mut cfg = c.clone();
        cfg.max_rows = cfg.max_rows.clamp(1, 38);
        cfg.warn_seconds = if cfg.warn_seconds.is_finite() {
            cfg.warn_seconds.clamp(1., 120.)
        } else {
            10.
        };
        Self {
            key: key_or_warn("effects", &cfg.key),
            cfg,
            enabled: false,
            unavailable: status::available().err(),
            expiry: Expiration::default(),
            sampled: 0,
        }
    }
}
impl Module for Effects {
    fn name(&self) -> &str {
        "effects"
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
            log!("effectsを利用できません: {e}");
            return;
        }
        if let Err(e) = status::use_status(on) {
            log!("effects: {e}");
            return;
        }
        self.enabled = on;
        self.expiry = Expiration::default();
        self.sampled = 0;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::x_offset(self.cfg.x, "Negative: from right."),
            Detail::y_offset(self.cfg.y, "Negative: from bottom."),
            Detail::boolean("duration","Remaining time","Game ticks at 20 ticks/second; no wall-clock countdown while paused.",self.cfg.duration),
            Detail::boolean("amplifier","Effect level","Show the effect level (I, II, etc.).",self.cfg.amplifier),
            Detail::integer("max_rows","Maximum effects","Shortest remaining times are displayed first.",self.cfg.max_rows,1,38),
            Detail::boolean("warn_expiring","Expiry notification","Notify once as an effect approaches expiry; refresh/reapplication rearms it. Uses GUI notifications.",self.cfg.warn_expiring),
            Detail::number("warn_seconds","Expiry threshold (seconds)","Time remaining at which to notify.",self.cfg.warn_seconds,1.,120.),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            "duration" => self.cfg.duration = v.boolean()?,
            "amplifier" => self.cfg.amplifier = v.boolean()?,
            "max_rows" => self.cfg.max_rows = (v.number()? as i32).clamp(1, 38),
            "warn_expiring" => {
                self.cfg.warn_expiring = v.boolean()?;
                self.expiry = Expiration::default();
            }
            "warn_seconds" => {
                self.cfg.warn_seconds = v.number()?.clamp(1., 120.);
                self.expiry = Expiration::default();
            }
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, tick: &Tick) {
        if !self.enabled
            || !self.cfg.warn_expiring
            || !tick.active
            || !crate::modules::hud_view()
            || tick.now_ms.saturating_sub(self.sampled) < 100
        {
            return;
        }
        self.sampled = tick.now_ms;
        let Some(effects) = status::reading().and_then(|r| r.effects) else {
            self.expiry = Expiration::default();
            return;
        };
        let ids = self
            .expiry
            .update(&effects, (self.cfg.warn_seconds * 20.) as u32, tick.now_ms);
        if !ids.is_empty() {
            notify::push(&format!(
                "まもなく効果が終了: {}",
                ids.into_iter().map(name).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        status::reading().and_then(|r| r.effects).map_or_else(
            || "（データなし）".into(),
            |e| format!("{}件の効果", e.len()),
        )
    }
    fn draw(&mut self, ui: &Ui) {
        if !crate::modules::hud_view() {
            return;
        }
        let Some(effects) = status::reading()
            .and_then(|r| r.effects)
            .filter(|e| !e.is_empty())
        else {
            return;
        };
        let mut rows: Vec<_> = effects.iter().collect();
        rows.sort_unstable_by_key(|e| (e.ticks.unwrap_or(u32::MAX), e.id));
        rows.truncate(self.cfg.max_rows as usize);
        let mut text = vec![format!(
            "{}（{}）",
            crate::localization::module_name("effects"),
            effects.len()
        )];
        text.extend(
            rows.into_iter()
                .map(|e| line(e, self.cfg.duration, self.cfg.amplifier)),
        );
        let width = text
            .iter()
            .map(|s| ui.calc_text_size(s)[0])
            .fold(100., f32::max)
            + 20.;
        let step = ui.text_line_height_with_spacing();
        let height = text.len() as f32 * step + 16.;
        crate::hud_editor::measure("effects", [width, height]);
        let screen = ui.io().display_size;
        let x = origin(self.cfg.x, screen[0], width);
        let y = origin(self.cfg.y, screen[1], height);
        let d = ui.get_background_draw_list();
        crate::theme::panel(&d, [x, y], [x + width, y + height]);
        for (i, s) in text.iter().enumerate() {
            d.add_text(
                [x + 10., y + 8. + i as f32 * step],
                if i == 0 {
                    crate::theme::ACCENT
                } else {
                    crate::theme::TEXT
                },
                s,
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn effect_labels_use_game_ticks_and_do_not_invent_unknown_names() {
        let e = Effect {
            id: 8,
            amplifier: 0,
            ticks: Some(7959),
        };
        assert_eq!(line(&e, true, true), "跳躍力上昇 I   6:38");
        assert_eq!(time(Some(1)), "0:01");
        assert_eq!(time(None), "無限");
        assert_eq!(name(38), "効果 38");
    }
    #[test]
    fn expiry_notices_rearm_after_refresh_and_batch_pending_effects() {
        let mut tracker = Expiration::default();
        let effect = |id, ticks| Effect {
            id,
            amplifier: 0,
            ticks,
        };
        assert!(tracker.update(&[effect(8, Some(500))], 200, 0).is_empty());
        assert_eq!(tracker.update(&[effect(8, Some(200))], 200, 100), vec![8]);
        assert!(tracker
            .update(&[effect(8, Some(100))], 200, 3000)
            .is_empty());
        tracker.update(&[effect(8, Some(500))], 200, 3001);
        assert_eq!(
            tracker.update(&[effect(8, Some(100)), effect(10, None)], 200, 4000),
            vec![8]
        );
        assert!(tracker
            .update(&[effect(8, Some(99)), effect(12, Some(100))], 200, 4001)
            .is_empty());
        assert_eq!(tracker.update(&[effect(12, Some(60))], 200, 6000), vec![12]);
    }
}
