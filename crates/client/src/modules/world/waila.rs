//! Read-only crosshair inspector. Native block queries remain on the game thread;
//! entity picking uses copied, owner-matched actor bounds on the worker thread.
pub(crate) mod picking;
use crate::game::{self, actors, blocks};
use crate::modules::settings::{Detail, Value};
use crate::modules::{key_or_warn, Module, Param, Tick};
use crate::{config, log};
use hudhook::imgui::Ui;

const MIN_REACH: f64 = 1.;
const MAX_REACH: f64 = 64.;
const INTERVAL_MS: u64 = 50;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Target {
    Block(blocks::Look),
    Entity {
        id: u32,
        actor: actors::Actor,
        distance: f32,
    },
}

pub struct Waila {
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    reach: f64,
    details: bool,
    fluids: bool,
    entities: bool,
    health: bool,
    identifiers: bool,
    x: i32,
    y: i32,
    sampled: u64,
    target: Option<Target>,
    lines: Vec<String>,
    summary: String,
}
fn clamp_reach(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(MIN_REACH, MAX_REACH)
    } else {
        6.
    }
}

/// Keep native/custom names on one line and strip Minecraft formatting codes.
fn clean_name(name: &str) -> String {
    let mut chars = name.chars();
    let mut out = String::new();
    for _ in 0..128 {
        let Some(c) = chars.next() else {
            break;
        };
        if c == '§' {
            chars.next();
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out.trim().to_string()
}
fn describe(target: &Target, details: bool, health: bool, identifiers: bool) -> Vec<String> {
    let mut lines = Vec::with_capacity(5);
    match target {
        Target::Block(hit) => {
            let name = hit.name.strip_prefix("minecraft:").unwrap_or(&hit.name);
            lines.push(format!("ブロック: {name}"));
            if details {
                lines.push(format!("距離 {:.1} ブロック", hit.distance));
                lines.push(format!(
                    "座標 {} / {} / {}",
                    hit.position[0], hit.position[1], hit.position[2]
                ));
            }
            if identifiers {
                lines.push(format!("ID: {}", hit.name));
            }
        }
        Target::Entity {
            actor, distance, ..
        } => {
            let name = actor
                .display_name
                .as_deref()
                .map(clean_name)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| actor.name.to_string());
            lines.push(format!("エンティティ: {name}"));
            if details {
                lines.push(format!("距離 {distance:.1} ブロック"));
                if let Some(b) = actor.bounds {
                    lines.push(format!(
                        "座標 {:.1} / {:.1} / {:.1}",
                        (b[0] + b[3]) * 0.5,
                        b[1],
                        (b[2] + b[5]) * 0.5
                    ));
                }
            }
            if health {
                if let Some((hp, max)) = actor.health {
                    lines.push(format!("体力 {hp:.1} / {max:.1}"));
                }
            }
            if identifiers {
                lines.push(format!("種類: {}", actor.name));
            }
        }
    }
    lines
}
/// Reuse the same occlusion and owner checks for passive diagnostics.
pub(in crate::modules) fn inspection_lines() -> Vec<String> {
    let Some(ray) = blocks::look_ray() else {
        return Vec::new();
    };
    let actors = actors::actors_for_local(ray.owner);
    let mut lines = Vec::new();
    if let Some(block) = &ray.block {
        lines.push(format!(
            "対象ブロック: {}, {}, {}",
            block.position[0], block.position[1], block.position[2]
        ));
        lines.push(block.name.clone());
        if let Some(light) = blocks::lighting::debug_snapshot(ray.owner, block.position) {
            lines.push(format!("光量: 空 {} / ブロック {}", light.sky, light.block));
        } else {
            lines.push("光量: 未取得".into());
        }
    } else {
        lines.push("対象ブロック: なし".into());
    }
    lines.push(String::new());
    match blocks::environment::fluid(ray.owner) {
        Some(Some(fluid)) => {
            lines.push(format!(
                "対象液体: {}, {}, {}",
                fluid.position[0], fluid.position[1], fluid.position[2]
            ));
            lines.push(format!("minecraft:{}", fluid.name));
        }
        Some(None) => lines.push("対象液体: minecraft:empty（なし）".into()),
        None => lines.push("対象液体: 未取得".into()),
    }
    if let Some(t @ Target::Entity { .. }) = picking::pick(&ray, actors.as_deref(), ray.reach) {
        lines.push(String::new());
        lines.extend(describe(&t, true, true, true));
    }
    lines
}
impl Waila {
    pub fn new(cfg: &config::Waila) -> Self {
        let reach = clamp_reach(cfg.reach);
        blocks::set_look_reach(reach as f32);
        blocks::set_look_fluids(cfg.fluids);
        Self {
            key: key_or_warn("waila", &cfg.key),
            enabled: false,
            unavailable: blocks::available()
                .err()
                .or_else(|| game::available().err()),
            reach,
            details: cfg.details,
            fluids: cfg.fluids,
            entities: cfg.entities,
            health: cfg.health,
            identifiers: cfg.identifiers,
            x: cfg.x,
            y: cfg.y,
            sampled: 0,
            target: None,
            lines: Vec::new(),
            summary: String::new(),
        }
    }
    fn rebuild(&mut self) {
        self.lines = self
            .target
            .as_ref()
            .map(|t| describe(t, self.details, self.health, self.identifiers))
            .unwrap_or_default();
        self.summary = self.lines.join(" · ");
    }
    fn reset(&mut self) {
        self.sampled = 0;
        self.target = None;
        self.lines.clear();
        self.summary.clear();
    }
}
impl Module for Waila {
    fn name(&self) -> &str {
        "waila"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "entities",
                "エンティティも表示",
                "照準が当たるプレイヤー・モブ・その他のエンティティも対象にします。",
                self.entities,
            ),
            Detail::boolean(
                "details",
                "Position and distance",
                "距離と座標を表示します。",
                self.details,
            ),
            Detail::boolean(
                "health",
                "Health",
                "取得できる場合に現在と最大の体力を表示します。",
                self.health,
            ),
            Detail::boolean(
                "identifiers",
                "種類IDも表示",
                "ブロックID・エンティティの種類IDを追加表示します。",
                self.identifiers,
            ),
            Detail::boolean(
                "fluids",
                "Stop at water and lava",
                "Off: look through fluids to the block behind them.",
                self.fluids,
            ),
            Detail::x_offset(self.x, "Pixels from the centered position."),
            Detail::y_offset(self.y, "Pixels below the crosshair (negative: above)."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "entities" => {
                let on = value.boolean()?;
                if on != self.entities && self.enabled {
                    actors::use_actors(on)?;
                }
                self.entities = on;
                self.target = None;
                self.sampled = 0;
            }
            "details" => self.details = value.boolean()?,
            "health" => self.health = value.boolean()?,
            "identifiers" => self.identifiers = value.boolean()?,
            "fluids" => {
                self.fluids = value.boolean()?;
                blocks::set_look_fluids(self.fluids);
                self.reset();
            }
            "x" => self.x = value.number()? as i32,
            "y" => self.y = value.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.rebuild();
        Ok(())
    }
    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        if let Some(e) = &self.unavailable {
            log!("wailaを利用できません: {e}");
            return;
        }
        if self.entities {
            if let Err(e) = actors::use_actors(on) {
                log!("waila: {e}");
                return;
            }
        }
        let result = if on { game::acquire() } else { game::release() };
        if let Err(e) = result {
            if self.entities {
                let _ = actors::use_actors(!on);
            }
            log!("waila: {e}");
            return;
        }
        blocks::set_look(on);
        self.enabled = on;
        self.reset();
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "reach",
            value: self.reach,
            min: MIN_REACH,
            max: MAX_REACH,
        })
    }
    fn set_param(&mut self, value: f64) {
        self.reach = clamp_reach(value);
        blocks::set_look_reach(self.reach as f32);
        self.reset();
    }
    fn tick(&mut self, tick: &Tick) {
        if !self.enabled || tick.now_ms.saturating_sub(self.sampled) < INTERVAL_MS {
            return;
        }
        self.sampled = tick.now_ms;
        let target = blocks::look_ray().and_then(|ray| {
            let actors = if self.entities {
                actors::actors_for_local(ray.owner)
            } else {
                None
            };
            picking::pick(&ray, actors.as_deref(), self.reach as f32)
        });
        if target != self.target {
            self.target = target;
            self.rebuild();
        }
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        if self.summary.is_empty() {
            if game::position().is_some() {
                "照準先に対象なし".into()
            } else {
                "（ワールド未接続）".into()
            }
        } else {
            self.summary.clone()
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if self.lines.is_empty() {
            return;
        }
        draw_card(ui, [self.x, self.y], &self.lines);
    }
}

fn draw_card(ui: &Ui, position: [i32; 2], lines: &[String]) {
    let screen = ui.io().display_size;
    if screen[0] < 40. || screen[1] < 40. {
        return;
    }
    let scale = ui.current_font_size() / 16.;
    let width = (lines
        .iter()
        .map(|s| ui.calc_text_size(s)[0])
        .fold(160. * scale, f32::max)
        + 24.)
        .min((480. * scale).min(screen[0] - 12.));
    let step = ui.text_line_height_with_spacing();
    let height = lines.len() as f32 * step + 20.;
    crate::hud_editor::measure("waila", [width, height]);
    let x = ((screen[0] - width) * 0.5 + position[0] as f32).clamp(0., (screen[0] - width).max(0.));
    let y = (screen[1] * 0.5 + position[1] as f32 - 3.).clamp(0., (screen[1] - height).max(0.));
    let d = ui.get_background_draw_list();
    crate::theme::panel(&d, [x, y], [x + width, y + height]);
    d.with_clip_rect([x, y], [x + width, y + height], || {
        for (i, line) in lines.iter().enumerate() {
            let line =
                crate::text::fit_width(line, (width - 24.).max(0.), |s| ui.calc_text_size(s)[0]);
            d.add_text(
                [x + 12., y + 10. + i as f32 * step],
                if i == 0 {
                    crate::theme::ACCENT
                } else {
                    crate::theme::TEXT
                },
                &line,
            );
        }
    });
}

#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    draw_card(
        ui,
        [0, 280],
        &describe(
            &Target::Entity {
                id: 42,
                actor: actors::Actor {
                    name: "cow".into(),
                    display_name: Some("テストのウシ".into()),
                    actor_type: 11,
                    health: Some((8., 10.)),
                    bounds: Some([0., 64., 2., 1., 65.4, 3.]),
                    local: false,
                },
                distance: 2.4,
            },
            true,
            true,
            true,
        ),
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_details_health_and_identifiers_are_independent() {
        let block = Target::Block(blocks::Look {
            name: "minecraft:deepslate_diamond_ore".into(),
            position: [-12, -58, 301],
            distance: 4.31,
        });
        assert_eq!(
            describe(&block, false, false, false),
            ["ブロック: deepslate_diamond_ore"]
        );
        let text = describe(&block, true, true, true).join("\n");
        assert!(
            text.contains("-12 / -58 / 301")
                && text.contains("4.3")
                && text.contains("ID: minecraft:")
        );
        assert!(!text.contains("体力"));
        assert_eq!(
            (clamp_reach(0.), clamp_reach(500.), clamp_reach(f64::NAN)),
            (1., 64., 6.)
        );
    }
    #[test]
    fn custom_names_remain_single_line_and_unknown_health_is_not_invented() {
        let mut actor = actors::Actor {
            name: "cow".into(),
            display_name: Some("§a名前\nテスト".into()),
            actor_type: 11,
            health: None,
            bounds: Some([0., 64., 2., 1., 65.4, 3.]),
            local: false,
        };
        let make = |actor| Target::Entity {
            id: 12,
            actor,
            distance: 2.,
        };
        let text = describe(&make(actor.clone()), true, true, true).join(" | ");
        assert!(text.contains("名前テスト"));
        assert!(!text.contains("§") && !text.contains("体力"));
        actor.health = Some((8., 10.));
        assert!(describe(&make(actor.clone()), false, true, false)
            .join(" | ")
            .contains("体力 8.0 / 10.0"));
        assert!(!describe(&make(actor), true, false, true)
            .join(" | ")
            .contains("体力"));
    }
}
