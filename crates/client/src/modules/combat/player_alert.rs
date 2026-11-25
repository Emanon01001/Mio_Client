//! Nearby player counts and entry notifications from the owned actor snapshot.
use crate::modules::{
    game::actors,
    hud::origin,
    key_or_warn,
    settings::{Detail, Value},
    Module, Param, Tick,
};
use crate::{api, config, log, notify};
use hudhook::imgui::Ui;
use std::collections::HashSet;

#[derive(Default)]
struct Proximity {
    inside: HashSet<u32>,
    notified: HashSet<u32>,
    last_notice: Option<u64>,
    nearest: Option<f32>,
}
impl Proximity {
    fn clear(&mut self) {
        self.inside.clear();
        self.notified.clear();
        self.nearest = None;
    }
    fn update(&mut self, candidates: &[(u32, f32)], range: f32, now: u64, cooldown: u64) -> bool {
        let mut next = HashSet::new();
        self.nearest = None;
        for &(id, distance) in candidates {
            // A 2-block exit margin avoids repeated notices at the boundary.
            let limit = range + if self.inside.contains(&id) { 2. } else { 0. };
            if distance.is_finite() && distance >= 0. && distance <= limit {
                next.insert(id);
                self.nearest = Some(self.nearest.map_or(distance, |old| old.min(distance)));
            }
        }
        self.inside = next;
        self.notified.retain(|id| self.inside.contains(id));
        let pending = self.inside.iter().any(|id| !self.notified.contains(id));
        if pending
            && self
                .last_notice
                .is_none_or(|last| now.saturating_sub(last) >= cooldown)
        {
            self.last_notice = Some(now);
            self.notified.extend(self.inside.iter().copied());
            true
        } else {
            false
        }
    }
    fn text(&self) -> String {
        self.nearest.map_or_else(
            || "周辺プレイヤー: 0人".into(),
            |d| format!("周辺プレイヤー: {}人 / 最短 {d:.1} m", self.inside.len()),
        )
    }
}

pub struct PlayerAlert {
    cfg: config::PlayerAlert,
    key: u16,
    enabled: bool,
    visible: bool,
    ready: bool,
    sampled: u64,
    nearby: Proximity,
}
impl PlayerAlert {
    pub fn new(c: &config::PlayerAlert) -> Self {
        let mut cfg = c.clone();
        cfg.range = if cfg.range.is_finite() {
            cfg.range.clamp(4., 128.)
        } else {
            32.
        };
        cfg.cooldown = if cfg.cooldown.is_finite() {
            cfg.cooldown.clamp(1., 120.)
        } else {
            5.
        };
        Self {
            key: key_or_warn("playeralert", &cfg.key),
            cfg,
            enabled: false,
            visible: false,
            ready: false,
            sampled: 0,
            nearby: Proximity::default(),
        }
    }
}

fn candidates(
    map: &std::collections::HashMap<u32, actors::Actor>,
    position: [f32; 3],
) -> Vec<(u32, f32)> {
    map.iter()
        .filter_map(|(&id, a)| {
            if a.local || (a.name.as_ref() != "player" && a.actor_type & 0xFF != 63) {
                return None;
            }
            let b = a.bounds?;
            let feet = [(b[0] + b[3]) * 0.5, b[1], (b[2] + b[5]) * 0.5];
            let distance = (0..3)
                .map(|k| (feet[k] - position[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            Some((id, distance))
        })
        .collect()
}
impl Module for PlayerAlert {
    fn name(&self) -> &str {
        "playeralert"
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
        if let Err(e) = actors::use_actors(on) {
            log!("playeralert: {e}");
            return;
        }
        self.enabled = on;
        self.nearby = Proximity::default();
        self.visible = false;
        self.ready = false;
        self.sampled = 0;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "range",
            value: self.cfg.range,
            min: 4.,
            max: 128.,
        })
    }
    fn set_param(&mut self, v: f64) {
        if v.is_finite() {
            self.cfg.range = v.clamp(4., 128.);
            self.nearby.clear();
            self.ready = false;
            self.sampled = 0;
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "notification",
                "Entry notification",
                "Notify when a new player enters the radius. Also requires GUI notifications.",
                self.cfg.notification,
            ),
            Detail::boolean(
                "hud",
                "Show nearby count",
                "Show the number of nearby players and the closest distance.",
                self.cfg.hud,
            ),
            Detail::number(
                "cooldown",
                "Notice cooldown (seconds)",
                "Batch incoming players rather than repeatedly notifying.",
                self.cfg.cooldown,
                1.,
                120.,
            ),
            Detail::x_offset(self.cfg.x, "Negative: from right."),
            Detail::y_offset(self.cfg.y, "Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "notification" => self.cfg.notification = v.boolean()?,
            "hud" => self.cfg.hud = v.boolean()?,
            "cooldown" => self.cfg.cooldown = v.number()?.clamp(1., 120.),
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn tick(&mut self, tick: &Tick) {
        self.visible = self.enabled && tick.active && crate::modules::hud_view();
        if !self.visible || tick.now_ms.saturating_sub(self.sampled) < 200 {
            return;
        }
        self.sampled = tick.now_ms;
        let player = api::game().ok().and_then(|g| g.player().ok());
        let (Some(player), Some(map)) = (player, actors::actors()) else {
            self.nearby.clear();
            self.ready = false;
            return;
        };
        self.ready = true;
        if self.nearby.update(
            &candidates(&map, player.position.into()),
            self.cfg.range as f32,
            tick.now_ms,
            (self.cfg.cooldown * 1000.) as u64,
        ) && self.cfg.notification
        {
            notify::push(&self.nearby.text());
        }
    }
    fn status(&self) -> String {
        if self.ready {
            self.nearby.text()
        } else {
            "（データなし）".into()
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if !self.visible
            || !self.ready
            || !self.cfg.hud
            || !crate::modules::hud_view()
            || actors::actors().is_none()
        {
            return;
        }
        let text = self.nearby.text();
        let sz = ui.calc_text_size(&text);
        let screen = ui.io().display_size;
        let width = sz[0] + 20.;
        let height = sz[1] + 16.;
        crate::hud_editor::measure("playeralert", [width, height]);
        let x = origin(self.cfg.x, screen[0], width);
        let y = origin(self.cfg.y, screen[1], height);
        let color = if self.nearby.inside.is_empty() {
            crate::theme::SUCCESS
        } else {
            crate::theme::WARNING
        };
        let d = ui.get_background_draw_list();
        crate::theme::panel(&d, [x, y], [x + width, y + height]);
        d.add_text([x + 10., y + 8.], color, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_generations_and_deferred_notices() {
        let mut p = Proximity::default();
        assert!(p.update(&[(7, 31.)], 32., 0, 5000));
        assert!(!p.update(&[(7, 33.)], 32., 10000, 5000)); // exit margin
        assert_eq!(p.inside.len(), 1);
        assert!(!p.update(&[(7, 35.)], 32., 10001, 5000));
        assert!(p.inside.is_empty());
        assert!(p.update(&[(7, 30.)], 32., 10002, 5000)); // new entry
        assert!(!p.update(&[(7, 30.), (8, 10.)], 32., 10003, 5000));
        assert!(p.update(&[(7, 30.), (8, 10.)], 32., 15002, 5000));
        assert_eq!(p.nearest, Some(10.));
        // Same index, different generation: a new player, not a cached id.
        assert!(p.update(&[(7 | 0x40000, 12.)], 32., 20002, 5000));
        assert_eq!(p.inside.len(), 1);
        p.clear();
        assert_eq!(p.text(), "周辺プレイヤー: 0人");
        assert!(!p.update(&[(1, f32::NAN), (2, -1.), (3, 40.)], 32., 30000, 5000));
        assert!(p.inside.is_empty());
    }
    #[test]
    fn only_other_players_with_boxes_are_counted() {
        let player = actors::Actor {
            name: "player".into(),
            display_name: None,
            actor_type: 0x13F,
            health: Some((20., 20.)),
            bounds: Some([-0.3, 0., -0.3, 0.3, 1.8, 0.3]),
            local: true,
        };
        let mut map = std::collections::HashMap::new();
        map.insert(1, player.clone());
        map.insert(
            2,
            actors::Actor {
                local: false,
                ..player.clone()
            },
        );
        map.insert(
            3,
            actors::Actor {
                name: "cow".into(),
                actor_type: 11,
                local: false,
                ..player.clone()
            },
        );
        map.insert(
            4,
            actors::Actor {
                bounds: None,
                local: false,
                ..player
            },
        );
        assert_eq!(candidates(&map, [3., 0., 4.]), vec![(2, 5.)]);
    }
}
