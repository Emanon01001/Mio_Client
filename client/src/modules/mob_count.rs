//! Counts owned actor snapshots; no native calls in rendering.
use super::{
    game::actors,
    key_or_warn,
    settings::{Detail, Value},
    Module, Param, Tick,
};
use crate::{api, config, log};
use hudhook::imgui::Ui;
use std::collections::{BTreeMap, HashMap};
fn count(
    all: &HashMap<u32, actors::Actor>,
    position: [f32; 3],
    range: f64,
) -> Vec<(String, usize)> {
    let mut counts = BTreeMap::<String, usize>::new();
    for actor in all.values() {
        if actor.local
            || actor.actor_type & 0xff == 63
            || actor.actor_type & 0x100 == 0
            || actor.health.is_some_and(|(hp, _)| hp <= 0.)
        {
            continue;
        }
        let Some(b) = actor.bounds else {
            continue;
        };
        let center = std::array::from_fn::<_, 3, _>(|k| (b[k] + b[k + 3]) * 0.5);
        let distance = (0..3)
            .map(|k| (center[k] - position[k]).powi(2) as f64)
            .sum::<f64>();
        if distance.is_finite() && distance <= range * range {
            *counts.entry(actor.name.to_string()).or_default() += 1;
        }
    }
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rows
}
fn label(name: &str) -> &str {
    match name {
        "zombie" => "ゾンビ",
        "creeper" => "クリーパー",
        "skeleton" => "スケルトン",
        "spider" => "クモ",
        "cow" => "ウシ",
        "pig" => "ブタ",
        "sheep" => "ヒツジ",
        "chicken" => "ニワトリ",
        "enderman" => "エンダーマン",
        "slime" => "スライム",
        "drowned" => "ドラウンド",
        "villager" | "villager_v2" => "村人",
        _ => name,
    }
}
pub struct MobCount {
    key: u16,
    on: bool,
    range: f64,
    rows: usize,
    x: i32,
    y: i32,
    next: u64,
    lines: Vec<String>,
    total: usize,
}
impl MobCount {
    pub fn new(c: &config::MobCount) -> Self {
        Self {
            key: key_or_warn("mobcount", &c.key),
            on: false,
            range: if c.range.is_finite() {
                c.range.clamp(4., 128.)
            } else {
                48.
            },
            rows: c.rows.clamp(1, 32) as usize,
            x: c.x,
            y: c.y,
            next: 0,
            lines: Vec::new(),
            total: 0,
        }
    }
}
impl Module for MobCount {
    fn name(&self) -> &str {
        "mobcount"
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
        if let Err(e) = actors::use_actors(on) {
            log!("MobCount: {e}");
            return;
        }
        self.on = on;
        self.lines.clear();
        self.next = 0;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "range",
            value: self.range,
            min: 4.,
            max: 128.,
        })
    }
    fn set_param(&mut self, v: f64) {
        if v.is_finite() {
            self.range = v.clamp(4., 128.);
            self.next = 0;
        }
    }
    fn tick(&mut self, t: &Tick) {
        if !self.on || t.now_ms < self.next {
            return;
        }
        self.next = t.now_ms + 250;
        let (Some(all), Some(p)) = (
            actors::actors(),
            api::game().ok().and_then(|g| g.player().ok()),
        ) else {
            self.lines.clear();
            self.total = 0;
            return;
        };
        let counts = count(&all, p.position.into(), self.range);
        self.total = counts.iter().map(|(_, n)| n).sum();
        self.lines = vec![format!("MobCount  {}体 / {:.0}m", self.total, self.range)];
        for (name, n) in counts.iter().take(self.rows) {
            self.lines.push(format!("{} × {n}", label(name)));
        }
        if counts.len() > self.rows {
            self.lines
                .push(format!("ほか{}種類", counts.len() - self.rows));
        }
    }
    fn status(&self) -> String {
        format!("{}体 / {:.0}m", self.total, self.range)
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::integer(
                "rows",
                "表示する種類数",
                "多い順に表示します。未読込のMobは含みません。",
                self.rows as i32,
                1,
                32,
            ),
            Detail::x_offset(self.x, "Negative: from right."),
            Detail::y_offset(self.y, "Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "rows" => self.rows = v.number()?.clamp(1., 32.) as usize,
            "x" => self.x = v.number()? as i32,
            "y" => self.y = v.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.next = 0;
        Ok(())
    }
    fn draw(&mut self, ui: &Ui) {
        if super::hud_view() && !self.lines.is_empty() {
            super::item_hud::panel(ui, "mobcount", self.x, self.y, &self.lines);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_only_nearby_living_mobs_and_sorts_deterministically() {
        let actor = |name: &str, t: u32, x: f32, health, local| actors::Actor {
            name: name.into(),
            display_name: None,
            actor_type: t,
            health,
            bounds: Some([x - 0.5, 0., -0.5, x + 0.5, 2., 0.5]),
            local,
        };
        let mut all = HashMap::new();
        for (i, a) in [
            actor("zombie", 0x120, 2., Some((20., 20.)), false),
            actor("zombie", 0x120, -2., None, false),
            actor("cow", 0x10b, 3., Some((10., 10.)), false),
            actor("cow", 0x10b, 3., Some((0., 10.)), false),
            actor("pig", 0x10c, 30., None, false),
            actor("item", 64, 1., None, false),
            actor("player", 319, 1., None, false),
            actor("zombie", 0x120, 1., None, true),
            actor("cow", 0x10b, f32::NAN, None, false),
        ]
        .into_iter()
        .enumerate()
        {
            all.insert(i as u32, a);
        }
        assert_eq!(
            count(&all, [0., 1., 0.], 10.),
            vec![("zombie".into(), 2), ("cow".into(), 1)]
        );
        assert_eq!(count(&all, [0., 1., 0.], 2.), vec![("zombie".into(), 2)]);
        assert!(count(&all, [f32::NAN, 0., 0.], 10.).is_empty());
    }
}
