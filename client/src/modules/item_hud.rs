//! Owned item snapshots, shared by the armor and inventory panels.
mod archive;
mod grid;
pub(crate) mod icons;
use super::{
    game::items,
    key_or_warn,
    radar::origin,
    settings::{Detail, Value},
    Module,
};
use crate::{config, log};
use hudhook::imgui::Ui;

#[cfg(test)]
pub(crate) fn preview_grid(ui: &Ui) {
    let mut inv = items::Inventory {
        selected: 0,
        slots: vec![None; 36],
        armor: vec![None; 4],
        world: (0, 0),
    };
    for (slot, name, count, damage, max_damage) in [
        (0, "netherite_pickaxe", 1, 200, 2031),
        (1, "netherite_axe", 1, 1700, 2031),
        (2, "bread", 8, 0, 0),
        (3, "cooked_chicken", 16, 0, 0),
        (4, "fishing_rod", 1, 40, 384),
        (9, "chest", 3, 0, 0),
        (10, "stone", 64, 0, 0),
        (11, "dirt", 32, 0, 0),
        (12, "diamond_ore", 19, 0, 0),
        (18, "netherite_helmet", 1, 107, 407),
        (20, "water_bucket", 1, 0, 0),
        (35, "addon_unknown", 12, 0, 0),
    ] {
        inv.slots[slot] = Some(items::Item {
            name: name.into(),
            count,
            damage,
            max_damage,
        });
    }
    grid::draw(
        ui,
        &inv,
        &config::ItemHud {
            x: 680,
            y: 500,
            ..Default::default()
        },
    );
}
pub struct ItemHud {
    cfg: config::ItemHud,
    key: u16,
    enabled: bool,
    armor: bool,
    unavailable: Option<String>,
}
impl ItemHud {
    pub fn new(c: &config::ItemHud, armor: bool) -> Self {
        let mut cfg = c.clone();
        cfg.max_rows = cfg.max_rows.clamp(1, 36);
        cfg.slot_size = cfg.slot_size.clamp(28, 64);
        Self {
            cfg,
            key: key_or_warn("item HUD", &c.key),
            enabled: false,
            armor,
            unavailable: items::available().err(),
        }
    }
    fn lines(&self) -> Option<Vec<String>> {
        let inv = items::snapshot()?;
        if self.armor {
            Some(
                inv.armor
                    .iter()
                    .enumerate()
                    .map(|(slot, item)| {
                        let part = ["頭", "胴", "脚", "足"][slot];
                        item.as_ref().map_or_else(
                            || format!("{part}: なし"),
                            |i| {
                                let wear = if i.max_damage == 0 {
                                    String::new()
                                } else if self.cfg.percent {
                                    format!(
                                        " {:.0}% ({}/{})",
                                        i.remaining() as f32 / i.max_damage as f32 * 100.,
                                        i.remaining(),
                                        i.max_damage
                                    )
                                } else {
                                    format!(" {}/{}", i.remaining(), i.max_damage)
                                };
                                format!("{part}: {}{wear}", i.name)
                            },
                        )
                    })
                    .collect(),
            )
        } else {
            let mut counts = std::collections::BTreeMap::<&str, u32>::new();
            for i in inv
                .slots
                .iter()
                .take(if self.cfg.hotbar_only { 9 } else { 36 })
                .flatten()
            {
                *counts.entry(&i.name).or_default() += i.count as u32;
            }
            let total = counts.len();
            let mut lines: Vec<_> = counts
                .into_iter()
                .take(self.cfg.max_rows as usize)
                .map(|(n, c)| format!("{n} x{c}"))
                .collect();
            lines.insert(0, format!("選択中のスロット: {}", inv.selected + 1));
            if total > lines.len() - 1 {
                lines.push(format!("ほか{}種類", total - (lines.len() - 1)));
            }
            if lines.is_empty() {
                lines.push("Empty".into());
            }
            Some(lines)
        }
    }
}
pub(super) fn panel(ui: &Ui, name: &'static str, x: i32, y: i32, lines: &[String]) {
    let width = lines
        .iter()
        .map(|s| ui.calc_text_size(s)[0])
        .fold(150., f32::max)
        + 20.;
    let height = lines.len() as f32 * ui.text_line_height_with_spacing() + 16.;
    crate::hud_editor::measure(name, [width, height]);
    let screen = ui.io().display_size;
    let x = origin(x, screen[0], width);
    let y = origin(y, screen[1], height);
    let d = ui.get_background_draw_list();
    crate::theme::panel(&d, [x, y], [x + width, y + height]);
    for (i, line) in lines.iter().enumerate() {
        d.add_text(
            [
                x + 10.,
                y + 8. + i as f32 * ui.text_line_height_with_spacing(),
            ],
            if i == 0 {
                crate::theme::TEXT
            } else {
                crate::theme::MUTED
            },
            line,
        );
    }
}
impl Module for ItemHud {
    fn name(&self) -> &str {
        if self.armor {
            "armorhud"
        } else {
            "inventoryhud"
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
            log!("{}: {e}", self.name());
            return;
        }
        match items::use_items(on) {
            Ok(()) => self.enabled = on,
            Err(e) => log!("{}: {e}", self.name()),
        }
    }
    fn options(&self) -> Vec<Detail> {
        let mut d = vec![
            Detail::x_offset(self.cfg.x, "Negative: from right."),
            Detail::y_offset(self.cfg.y, "Negative: from bottom."),
        ];
        if self.armor {
            d.push(Detail::boolean(
                "percent",
                "Durability percent",
                "Also show remaining/max durability.",
                self.cfg.percent,
            ));
        } else {
            d.push(Detail::boolean(
                "grid",
                "Slot grid",
                "Show the inventory's real slots with icons. Turn off for the grouped text list.",
                self.cfg.grid,
            ));
            d.push(Detail::integer(
                "slot_size",
                "Slot size (pixels)",
                "Size of each inventory slot; the panel position is edited with Edit HUD.",
                self.cfg.slot_size,
                28,
                64,
            ));
            d.push(Detail::boolean(
                "show_counts",
                "Stack counts",
                "Show the number of items in each stack.",
                self.cfg.show_counts,
            ));
            d.push(Detail::boolean(
                "show_durability",
                "Durability bars",
                "Show remaining tool durability below its icon.",
                self.cfg.show_durability,
            ));
            d.push(Detail::boolean(
                "hotbar_only",
                "Hotbar only",
                "Show only the hotbar; otherwise include all 36 inventory slots.",
                self.cfg.hotbar_only,
            ));
            d.push(Detail::integer(
                "max_rows",
                "Maximum text rows",
                "Aggregate stacks by identifier.",
                self.cfg.max_rows,
                1,
                36,
            ));
        }
        d
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            "percent" => self.cfg.percent = v.boolean()?,
            "hotbar_only" => self.cfg.hotbar_only = v.boolean()?,
            "max_rows" => self.cfg.max_rows = (v.number()? as i32).clamp(1, 36),
            "grid" => self.cfg.grid = v.boolean()?,
            "slot_size" => self.cfg.slot_size = (v.number()? as i32).clamp(28, 64),
            "show_counts" => self.cfg.show_counts = v.boolean()?,
            "show_durability" => self.cfg.show_durability = v.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        self.unavailable.clone().unwrap_or_else(|| {
            items::snapshot().map_or_else(
                || "（所持品データなし）".into(),
                |i| format!("{}スロットを使用中", i.slots.iter().flatten().count()),
            )
        })
    }
    fn draw(&mut self, ui: &Ui) {
        if !super::hud_view() {
            return;
        }
        if !self.armor && self.cfg.grid {
            if let Some(inv) = items::snapshot() {
                grid::draw(ui, &inv, &self.cfg);
            }
            return;
        }
        if let Some(mut lines) = self.lines() {
            lines.insert(
                0,
                if self.armor {
                    crate::localization::module_name("armorhud").into()
                } else {
                    crate::localization::module_name("inventoryhud").into()
                },
            );
            panel(
                ui,
                if self.armor {
                    "armorhud"
                } else {
                    "inventoryhud"
                },
                self.cfg.x,
                self.cfg.y,
                &lines,
            );
        }
    }
}
