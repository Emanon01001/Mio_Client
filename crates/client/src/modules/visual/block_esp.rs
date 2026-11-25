//! Colored outlines around selected loaded ores and storage blocks, plus any
//! block names the user adds (`[blockesp] extra`, or the `blocks` command).
use crate::modules::settings::{Detail, Value};
use crate::modules::{
    game::{self, blocks},
    key_or_warn,
    view::{box_lines, project, BoxPainter},
    Module, Param, Tick,
};
use crate::{config, log};
use hudhook::imgui::Ui;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::Mutex;
mod nearest;
use nearest::Nearest;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static RADIUS: AtomicI32 = AtomicI32::new(24);
static DRAWN: AtomicUsize = AtomicUsize::new(0);
/// Draw a line from the bottom of the screen to the nearest matches.
static TRACERS: AtomicBool = AtomicBool::new(false);
/// At most this many tracers, nearest first, so a rich vein stays readable.
const MAX_TRACERS: usize = 12;
/// The closest match in the last frame: its kind and distance in blocks.
static NEAREST: Mutex<Option<(blocks::Kind, f32)>> = Mutex::new(None);

fn set_nearest(value: Option<(blocks::Kind, f32)>) {
    if let Ok(mut slot) = NEAREST.try_lock() {
        *slot = value;
    }
}

pub fn draw(ui: &Ui) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let (Some(camera), Some(found)) = (game::camera(), blocks::snapshot()) else {
        DRAWN.store(0, Ordering::Relaxed);
        set_nearest(None);
        return;
    };
    let radius = RADIUS.load(Ordering::Relaxed) as f32;
    let radius_squared = (radius + 1.).powi(2);
    let tracers = TRACERS.load(Ordering::Relaxed);
    let size = ui.io().display_size;
    let draw = ui.get_background_draw_list();
    // Six draw calls per box instead of twelve (see BoxPainter).
    let painter = BoxPainter::new(ui);
    let mut count = 0;
    let mut closest = Nearest::<_, MAX_TRACERS>::new(if tracers { MAX_TRACERS } else { 1 });
    for block in found.iter() {
        let p = block.position.map(|v| v as f32);
        let distance_squared = (0..3)
            .map(|k| (p[k] + 0.5 - camera.origin[k]).powi(2))
            .sum::<f32>();
        if distance_squared > radius_squared {
            continue;
        }
        closest.push((block.kind, distance_squared, p.map(|v| v + 0.5)));
        let visible = box_lines(&camera, size, p, p.map(|v| v + 1.))
            .is_some_and(|lines| painter.draw(&lines, block.kind.color(), 1.5));
        count += visible as usize;
    }
    DRAWN.store(count, Ordering::Relaxed);

    set_nearest(
        closest
            .iter()
            .next()
            .map(|&(kind, d2, _)| (kind, d2.sqrt())),
    );
    if tracers {
        let from = [size[0] * 0.5, size[1]];
        for &(kind, _, centre) in closest.iter() {
            if let Some(to) = project(&camera, size, centre) {
                let [r, g, b, _] = kind.color();
                draw.add_line(from, to, [r, g, b, 0.6]).build();
            }
        }
    }
}

/// The `blocks` console command: the user's own block names to outline, in
/// addition to the built-in ores and chests. `args` is everything after the
/// command word.
pub fn command(args: &[&str]) {
    let mut names = blocks::custom();
    match args {
        [] | ["list"] => {
            if names.is_empty() {
                log!(
                    "指定ブロックはありません。blocks add <ID> で追加できます（例: mob_spawner）。"
                );
            } else {
                log!("指定ブロック: {}", names.join(", "));
            }
            if !ACTIVE.load(Ordering::Acquire) {
                log!("（ブロックESPがOFFです。toggle blockespで表示できます）");
            }
            return;
        }
        ["add", name] => {
            let Some(pattern) = blocks::normalize_pattern(name) else {
                log!("blocks: `{name}`は有効なIDではありません（英数字・_・前後の*を使えます）");
                return;
            };
            if names.contains(&pattern) {
                log!("blocks: `{pattern}`は追加済みです");
                return;
            }
            if names.len() >= blocks::MAX_CUSTOM {
                log!("blocks: 最大{}件まで指定できます", blocks::MAX_CUSTOM);
                return;
            }
            names.push(pattern);
        }
        ["del" | "remove", name] => {
            let pattern = blocks::normalize_pattern(name).unwrap_or_default();
            let before = names.len();
            names.retain(|n| *n != pattern);
            if names.len() == before {
                log!("blocks: `{name}`は未登録です（blocks listで確認）");
                return;
            }
        }
        ["clear"] => names.clear(),
        _ => {
            log!("使い方: blocks list | blocks add <ID> | blocks del <ID> | blocks clear");
            log!("       /setblockのIDからminecraft:を除いて指定します。前後に*を使えます。");
            return;
        }
    }
    let used = blocks::set_custom(&names);
    if used.is_empty() {
        log!("指定ブロック: なし");
    } else {
        log!("指定ブロック: {}", used.join(", "));
    }
}

pub struct BlockEsp {
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    ores: bool,
    chests: bool,
    pattern: String,
    error: String,
    last_aimed: Option<String>,
    sampled_look: u64,
}
impl BlockEsp {
    pub fn new(cfg: &config::BlockEsp) -> Self {
        let radius = cfg.radius.clamp(4, 64);
        RADIUS.store(radius, Ordering::Relaxed);
        blocks::configure(radius, cfg.ores, cfg.chests);
        blocks::set_custom(&cfg.extra);
        TRACERS.store(cfg.tracers, Ordering::Relaxed);
        Self {
            key: key_or_warn("blockesp", &cfg.key),
            enabled: false,
            unavailable: blocks::available().err(),
            ores: cfg.ores,
            chests: cfg.chests,
            pattern: String::new(),
            error: String::new(),
            last_aimed: None,
            sampled_look: 0,
        }
    }
}
impl Module for BlockEsp {
    fn name(&self) -> &str {
        "blockesp"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::text("extra", blocks::custom().join("\n")),
            Detail::boolean(
                "ores",
                "Ores",
                "Outline ore blocks (not compressed mineral blocks).",
                self.ores,
            ),
            Detail::boolean(
                "chests",
                "Chests",
                "Outline chests, barrels and shulker boxes.",
                self.chests,
            ),
            Detail::boolean(
                "tracers",
                "Tracers",
                "Draw a line from the bottom of the screen to the nearest matches.",
                TRACERS.load(Ordering::Relaxed),
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "extra" => {
                let names = parse_targets(value.text()?)?;
                blocks::set_custom(&names);
                return Ok(());
            }
            "ores" => self.ores = value.boolean()?,
            "chests" => self.chests = value.boolean()?,
            "tracers" => {
                TRACERS.store(value.boolean()?, Ordering::Relaxed);
                return Ok(());
            }
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        blocks::configure(RADIUS.load(Ordering::Relaxed), self.ores, self.chests);
        Ok(())
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        if let Some(error) = &self.unavailable {
            log!("blockespを利用できません: {error}");
            return;
        }
        if on == self.enabled {
            return;
        }
        if let Err(error) = game::use_camera(on) {
            log!("blockesp: {error}");
            return;
        }
        blocks::set_active(on);
        blocks::set_look(on);
        ACTIVE.store(on, Ordering::Release);
        self.enabled = on;
    }
    fn tick(&mut self, t: &Tick) {
        if self.enabled && t.active && t.now_ms.saturating_sub(self.sampled_look) >= 100 {
            self.sampled_look = t.now_ms;
            self.last_aimed = blocks::look().flatten().map(|hit| hit.name);
        }
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.separator();
        ui.text(crate::localization::text("Custom blocks"));
        ui.set_next_item_width(200.);
        let enter = ui
            .input_text("##block_pattern", &mut self.pattern)
            .hint(crate::localization::text("diamond_block, *shulker_box"))
            .enter_returns_true(true)
            .build();
        ui.same_line();
        if ui.button(crate::localization::text("Add")) || enter {
            match parse_targets(&self.pattern).and_then(|added| {
                let mut names = blocks::custom();
                for name in added {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                if names.len() > blocks::MAX_CUSTOM {
                    return Err(format!("最大{}件まで指定できます", blocks::MAX_CUSTOM));
                }
                Ok(names)
            }) {
                Ok(names) => {
                    blocks::set_custom(&names);
                    self.pattern.clear();
                    self.error.clear();
                }
                Err(e) => self.error = e,
            }
        }
        if let Some(name) = &self.last_aimed {
            if ui.button(format!("最後に照準を合わせたブロックを追加: {name}")) {
                let mut names = blocks::custom();
                if !names.contains(name) {
                    if names.len() < blocks::MAX_CUSTOM {
                        names.push(name.clone());
                        blocks::set_custom(&names);
                        self.error.clear();
                    } else {
                        self.error = format!("最大{}件まで指定できます", blocks::MAX_CUSTOM);
                    }
                }
            }
        }
        let mut names = blocks::custom();
        let mut remove = None;
        for (i, name) in names.iter().enumerate() {
            let _id = ui.push_id_usize(i);
            if ui.small_button("Remove") {
                remove = Some(i);
            }
            ui.same_line();
            ui.text(name);
        }
        if let Some(i) = remove {
            names.remove(i);
            blocks::set_custom(&names);
        }
        if !names.is_empty() && ui.small_button("Clear custom blocks") {
            blocks::set_custom(&[]);
        }
        if !self.error.is_empty() {
            ui.text_wrapped(&self.error);
        }
        ui.text_wrapped(crate::localization::text("IDs, not translated names. Separate multiple IDs with spaces or commas. * works at either end. Save in Profiles."));
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "radius",
            value: RADIUS.load(Ordering::Relaxed) as f64,
            min: 4.,
            max: 64.,
        })
    }
    fn set_param(&mut self, value: f64) {
        if value.is_finite() {
            let radius = value.clamp(4., 64.).round() as i32;
            RADIUS.store(radius, Ordering::Relaxed);
            blocks::configure(radius, self.ores, self.chests);
        }
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            format!("({e})")
        } else {
            let drawn = DRAWN.load(Ordering::Relaxed);
            let closest = *NEAREST.lock().unwrap_or_else(|e| e.into_inner());
            match closest.filter(|_| self.enabled) {
                Some((kind, distance)) => {
                    format!("{drawn}個 / 最寄り: {} {distance:.0} m", kind.label())
                }
                None => format!("{drawn}個のブロック"),
            }
        }
    }
}

fn parse_targets(text: &str) -> Result<Vec<String>, String> {
    if text.len() > 4096 {
        return Err("対象の指定が多すぎます".into());
    }
    let mut names = Vec::new();
    for token in text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
    {
        let name = blocks::normalize_pattern(token)
            .ok_or_else(|| format!("不正なブロックID・パターン: {token}"))?;
        if !names.contains(&name) {
            names.push(name);
        }
        if names.len() > blocks::MAX_CUSTOM {
            return Err(format!("最大{}件まで指定できます", blocks::MAX_CUSTOM));
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_text_normalizes_and_rejects_invalid_lists_without_truncation() {
        assert_eq!(
            parse_targets("minecraft:Diamond_Block, *shulker_box\n diamond_block").unwrap(),
            ["diamond_block", "*shulker_box"]
        );
        assert!(parse_targets("stone;dirt").is_err());
        assert!(parse_targets(&"stone ".repeat(1000)).is_err());
        assert!(parse_targets(
            &(0..33)
                .map(|n| format!("block_{n}"))
                .collect::<Vec<_>>()
                .join(",")
        )
        .is_err());
        assert!(parse_targets("").unwrap().is_empty());
    }
}
