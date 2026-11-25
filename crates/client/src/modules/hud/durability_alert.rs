//! Read-only item/equipment warnings; one notice per worn-item episode.
use crate::modules::{
    game::items,
    key_or_warn,
    settings::{Detail, Value},
    Module, Param, Tick,
};
use crate::{config, log, notify};

#[derive(Default)]
struct Slot {
    identity: Option<(String, u16)>,
    warned: bool,
}
impl Slot {
    fn low(&mut self, item: Option<&items::Item>, threshold: f64) -> bool {
        let Some(item) = item.filter(|i| i.max_damage != 0) else {
            *self = Self::default();
            return false;
        };
        if self
            .identity
            .as_ref()
            .is_none_or(|(name, max)| name != &item.name || *max != item.max_damage)
        {
            self.identity = Some((item.name.clone(), item.max_damage));
            self.warned = false;
        }
        let percent = f64::from(item.remaining()) * 100. / f64::from(item.max_damage);
        if percent > threshold + 5. {
            self.warned = false;
        }
        percent <= threshold && !self.warned
    }
}
pub struct DurabilityAlert {
    cfg: config::DurabilityAlert,
    key: u16,
    enabled: bool,
    slots: [Slot; 13],
    world: Option<(usize, u32)>,
    next_sample: u64,
    next_notice: u64,
    status: String,
}
impl DurabilityAlert {
    pub fn new(c: &config::DurabilityAlert) -> Self {
        let mut cfg = c.clone();
        cfg.threshold = if cfg.threshold.is_finite() {
            cfg.threshold.clamp(1., 50.)
        } else {
            10.
        };
        cfg.cooldown = if cfg.cooldown.is_finite() {
            cfg.cooldown.clamp(1., 120.)
        } else {
            5.
        };
        Self {
            key: key_or_warn("durabilityalert", &cfg.key),
            cfg,
            enabled: false,
            slots: std::array::from_fn(|_| Slot::default()),
            world: None,
            next_sample: 0,
            next_notice: 0,
            status: "所持品データを待機中".into(),
        }
    }
    fn reset(&mut self) {
        self.slots = std::array::from_fn(|_| Slot::default());
        self.next_sample = 0;
        self.next_notice = 0;
    }
}
impl Module for DurabilityAlert {
    fn name(&self) -> &str {
        "durabilityalert"
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
        if let Err(e) = items::use_items(on) {
            log!("durabilityalert: {e}");
            return;
        }
        self.enabled = on;
        self.world = None;
        self.reset();
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "remaining %",
            value: self.cfg.threshold,
            min: 1.,
            max: 50.,
        })
    }
    fn set_param(&mut self, v: f64) {
        if v.is_finite() {
            self.cfg.threshold = v.clamp(1., 50.);
            self.reset();
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "hotbar",
                "Watch hotbar",
                "Warn for tools in the hotbar.",
                self.cfg.hotbar,
            ),
            Detail::boolean(
                "armor",
                "Watch armor",
                "Warn for equipped armor.",
                self.cfg.armor,
            ),
            Detail::number(
                "cooldown",
                "Notice cooldown (seconds)",
                "Batch warnings and avoid repeated notices.",
                self.cfg.cooldown,
                1.,
                120.,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "hotbar" => self.cfg.hotbar = value.boolean()?,
            "armor" => self.cfg.armor = value.boolean()?,
            "cooldown" => self.cfg.cooldown = value.number()?.clamp(1., 120.),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.reset();
        Ok(())
    }
    fn tick(&mut self, t: &Tick) {
        if !self.enabled || !t.active || t.now_ms < self.next_sample {
            return;
        }
        self.next_sample = t.now_ms.saturating_add(100);
        let Some(inv) = items::snapshot() else {
            self.world = None;
            self.status = "所持品データを待機中".into();
            return;
        };
        if self.world != Some(inv.world) {
            self.reset();
            self.world = Some(inv.world);
        }
        let mut pending = Vec::new();
        let mut low = 0;
        for i in 0..13 {
            let item = if i < 9 {
                self.cfg
                    .hotbar
                    .then(|| inv.slots.get(i).and_then(Option::as_ref))
                    .flatten()
            } else {
                self.cfg
                    .armor
                    .then(|| inv.armor.get(i - 9).and_then(Option::as_ref))
                    .flatten()
            };
            if self.slots[i].low(item, self.cfg.threshold) && t.now_ms >= self.next_notice {
                if let Some(item) = item {
                    pending.push((
                        i,
                        format!("{} {}/{}", item.name, item.remaining(), item.max_damage),
                    ));
                }
            }
            if let Some(item) = item.filter(|i| i.max_damage > 0) {
                low += usize::from(
                    f64::from(item.remaining()) * 100.
                        <= self.cfg.threshold * f64::from(item.max_damage),
                );
            }
        }
        self.status = format!("耐久が低い装備: {low}個");
        if !pending.is_empty() {
            let text = pending
                .iter()
                .take(3)
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let extra = if pending.len() > 3 {
                format!(" (+{})", pending.len() - 3)
            } else {
                String::new()
            };
            log!("耐久低下: {text}{extra}");
            notify::push(&format!("耐久低下: {text}{extra}"));
            for (i, _) in pending {
                self.slots[i].warned = true;
            }
            self.next_notice = t.now_ms.saturating_add((self.cfg.cooldown * 1000.) as u64);
        }
    }
    fn status(&self) -> String {
        self.status.clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worn_items_warn_once_and_rearm_after_repair_replacement_or_removal() {
        let mut slot = Slot::default();
        let mut item = items::Item {
            name: "netherite_pickaxe".into(),
            count: 1,
            damage: 95,
            max_damage: 100,
        };
        assert!(slot.low(Some(&item), 10.));
        slot.warned = true;
        assert!(!slot.low(Some(&item), 10.));
        item.damage = 86;
        assert!(!slot.low(Some(&item), 10.));
        item.damage = 80;
        assert!(!slot.low(Some(&item), 10.));
        item.damage = 95;
        assert!(slot.low(Some(&item), 10.));
        slot.warned = true;
        item.name = "diamond_pickaxe".into();
        assert!(slot.low(Some(&item), 10.));
        slot.low(None, 10.);
        assert!(slot.low(Some(&item), 10.));
        item.max_damage = 0;
        assert!(!slot.low(Some(&item), 10.));
    }
}
