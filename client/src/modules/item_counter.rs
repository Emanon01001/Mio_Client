use super::{
    game::items,
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log};
use hudhook::imgui::Ui;
use std::sync::Arc;
const MAX_TARGETS: usize = 16;
pub struct ItemCounter {
    cfg: config::ItemCounter,
    key: u16,
    enabled: bool,
    last: Option<Arc<items::Inventory>>,
    rows: Vec<(String, u32)>,
    lines: Vec<String>,
    input: String,
    error: String,
}
fn parse(text: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for token in text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
    {
        let token = token.to_ascii_lowercase();
        let name = token.strip_prefix("minecraft:").unwrap_or(&token);
        if name.is_empty()
            || name.len() > 64
            || name.starts_with(':')
            || name.ends_with(':')
            || name.bytes().filter(|&b| b == b':').count() > 1
            || !name.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b':' | b'.')
            })
        {
            return Err(format!("アイテムIDを指定してください: {token}"));
        }
        if !names.iter().any(|s| s == name) {
            names.push(name.to_string());
        }
        if names.len() > MAX_TARGETS {
            return Err(format!("対象は{MAX_TARGETS}種類まで指定できます"));
        }
    }
    Ok(names)
}
fn count(inv: &items::Inventory, cfg: &config::ItemCounter) -> Vec<(String, u32)> {
    let mut targets = Vec::new();
    if cfg.held {
        if let Some(item) = inv.slots.get(inv.selected).and_then(Option::as_ref) {
            targets.push(item.name.clone());
        }
    }
    for id in &cfg.targets {
        if !targets.contains(id) {
            targets.push(id.clone());
        }
    }
    targets
        .into_iter()
        .filter_map(|id| {
            let n = inv
                .slots
                .iter()
                .take(if cfg.hotbar_only { 9 } else { 36 })
                .flatten()
                .filter(|i| i.name == id)
                .map(|i| u32::from(i.count))
                .sum();
            (cfg.show_zero || n > 0).then_some((id, n))
        })
        .collect()
}
impl ItemCounter {
    pub fn new(cfg: &config::ItemCounter) -> Self {
        let mut cfg = cfg.clone();
        cfg.targets = parse(&cfg.targets.join(" ")).unwrap_or_else(|e| {
            log!("ItemCounter: {e}");
            config::ItemCounter::default().targets
        });
        Self {
            key: key_or_warn("itemcounter", &cfg.key),
            cfg,
            enabled: false,
            last: None,
            rows: Vec::new(),
            lines: Vec::new(),
            input: String::new(),
            error: String::new(),
        }
    }
    fn refresh(&mut self) {
        let inv = items::snapshot();
        if matches!((&inv,&self.last),(Some(a),Some(b)) if Arc::ptr_eq(a,b))
            || (inv.is_none() && self.last.is_none() && !self.lines.is_empty())
        {
            return;
        }
        self.rows = inv.as_ref().map_or_else(Vec::new, |s| count(s, &self.cfg));
        self.lines = vec!["ItemCounter".into()];
        self.lines
            .extend(self.rows.iter().map(|(id, n)| format!("{id} × {n}")));
        if self.rows.is_empty() {
            self.lines.push(
                if inv.is_some() {
                    "対象アイテムなし"
                } else {
                    "所持品データを待機中"
                }
                .into(),
            );
        }
        self.last = inv;
    }
    fn invalidate(&mut self) {
        self.last = None;
        self.rows.clear();
        self.lines.clear();
    }
}
impl Module for ItemCounter {
    fn name(&self) -> &str {
        "itemcounter"
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
            log!("ItemCounter: {e}");
            return;
        }
        self.enabled = on;
        self.invalidate();
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::text("targets", self.cfg.targets.join(" ")),
            Detail::boolean(
                "held",
                "手持ちアイテムも数える",
                "選択中のホットバーのアイテムを先頭に追加し、同じ種類は重複表示しません。",
                self.cfg.held,
            ),
            Detail::boolean(
                "hotbar_only",
                "ホットバーだけ数える",
                "ONにすると通常インベントリを除外し、ホットバーの9スロットだけを数えます。",
                self.cfg.hotbar_only,
            ),
            Detail::boolean(
                "show_zero",
                "所持数0も表示",
                "OFFにすると持っていない対象は一覧から隠します。",
                self.cfg.show_zero,
            ),
            Detail::x_offset(self.cfg.x, "Negative: from right."),
            Detail::y_offset(self.cfg.y, "Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "targets" => self.cfg.targets = parse(v.text()?)?,
            "held" => self.cfg.held = v.boolean()?,
            "hotbar_only" => self.cfg.hotbar_only = v.boolean()?,
            "show_zero" => self.cfg.show_zero = v.boolean()?,
            "x" => self.cfg.x = v.number()? as i32,
            "y" => self.cfg.y = v.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.invalidate();
        Ok(())
    }
    fn tick(&mut self, _: &Tick) {
        if self.enabled {
            self.refresh();
        }
    }
    fn status(&self) -> String {
        if self.last.is_none() {
            "所持品データを待機中".into()
        } else {
            format!(
                "{}種類 / 合計{}個",
                self.rows.len(),
                self.rows.iter().map(|(_, n)| n).sum::<u32>()
            )
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if self.lines.is_empty() {
            return;
        }
        super::item_hud::panel(ui, "itemcounter", self.cfg.x, self.cfg.y, &self.lines);
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.separator();
        ui.text("対象アイテム（ID）");
        ui.set_next_item_width(-1.);
        let enter = ui
            .input_text("##itemcounter_targets", &mut self.input)
            .hint("arrow, torch, bread")
            .enter_returns_true(true)
            .build();
        if ui.button("追加") || enter {
            match parse(&self.input).and_then(|added| {
                parse(
                    &self
                        .cfg
                        .targets
                        .iter()
                        .chain(added.iter())
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }) {
                Ok(names) => {
                    self.cfg.targets = names;
                    self.input.clear();
                    self.error.clear();
                    self.invalidate();
                }
                Err(e) => self.error = e,
            }
        }
        if let Some(inv) = items::snapshot() {
            if let Some(item) = inv.slots.get(inv.selected).and_then(Option::as_ref) {
                ui.same_line();
                if ui.button("手持ちを追加") {
                    match parse(&format!("{} {}", self.cfg.targets.join(" "), item.name)) {
                        Ok(names) => {
                            self.cfg.targets = names;
                            self.error.clear();
                            self.invalidate();
                        }
                        Err(e) => self.error = e,
                    }
                }
            }
        }
        let mut remove = None;
        for (i, name) in self.cfg.targets.iter().enumerate() {
            let _id = ui.push_id_usize(i);
            if ui.small_button("削除") {
                remove = Some(i);
            }
            ui.same_line();
            ui.text(name);
        }
        if let Some(i) = remove {
            self.cfg.targets.remove(i);
            self.invalidate();
        }
        if !self.cfg.targets.is_empty() && ui.button("対象をすべて削除") {
            self.cfg.targets.clear();
            self.invalidate();
        }
        ui.text_wrapped("アイテムIDを指定します。minecraft: は省略できます。対象リストはプロファイルに保存できます。");
        if !self.error.is_empty() {
            ui.text_colored(crate::theme::WARNING, &self.error);
        }
    }
}
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    super::item_hud::panel(
        ui,
        "itemcounter",
        -20,
        750,
        &[
            "ItemCounter".into(),
            "arrow × 64".into(),
            "torch × 128".into(),
            "bread × 16".into(),
        ],
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    fn item(name: &str, n: u8) -> Option<items::Item> {
        Some(items::Item {
            name: name.into(),
            count: n,
            damage: 0,
            max_damage: 0,
        })
    }
    #[test]
    fn counter_combines_stacks_without_armor_or_duplicate_held_targets() {
        let mut inv = items::Inventory {
            selected: 0,
            slots: vec![None; 36],
            armor: vec![item("arrow", 99)],
            world: (1, 2),
        };
        inv.slots[0] = item("arrow", 64);
        inv.slots[8] = item("arrow", 7);
        inv.slots[9] = item("arrow", 32);
        inv.slots[35] = item("bread", 8);
        let mut cfg = config::ItemCounter::default();
        assert_eq!(
            count(&inv, &cfg),
            [
                ("arrow".into(), 103),
                ("torch".into(), 0),
                ("bread".into(), 8)
            ]
        );
        cfg.hotbar_only = true;
        cfg.show_zero = false;
        assert_eq!(count(&inv, &cfg), [("arrow".into(), 71)]);
        cfg.targets.clear();
        cfg.held = false;
        assert!(count(&inv, &cfg).is_empty());
        cfg.held = true;
        inv.selected = 999;
        assert!(count(&inv, &cfg).is_empty());
    }
    #[test]
    fn item_targets_normalize_deduplicate_and_reject_invalid_or_unbounded_input() {
        assert_eq!(
            parse("minecraft:ARROW, bread arrow").unwrap(),
            ["arrow", "bread"]
        );
        assert!(parse("").unwrap().is_empty());
        for s in [
            "bad/name",
            "*arrow",
            "minecraft:",
            "a=1",
            ":arrow",
            "foo:",
            "foo::bar",
        ] {
            assert!(parse(s).is_err());
        }
        assert!(parse(&"x".repeat(65)).is_err());
        assert!(parse(
            &(0..17)
                .map(|i| format!("item_{i}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .is_err());
    }
}
