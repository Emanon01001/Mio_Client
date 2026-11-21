//! CombatHUD: a panel summarising the fight — the target the player last hit
//! (name, health bar, distance), hits and kills landed this session, the
//! current combo, time since the last hit, and nearby threats. The hits come
//! from `game::hits`, which watches entity health; nothing is written to the
//! game.

use hudhook::imgui::Ui;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

use super::entity_kind;
use super::game::{self, hits};
use super::radar::origin;
use super::settings::{Detail, Value};
use super::{key_or_warn, Module};
use crate::{config, log, theme};

const WIDTH: f32 = 230.;
const PADDING: f32 = 10.;

pub struct CombatHud {
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    threats: bool,
    x: i32,
    y: i32,
}

impl CombatHud {
    pub fn new(cfg: &config::CombatHud) -> Self {
        Self {
            key: key_or_warn("combathud", &cfg.key),
            enabled: false,
            unavailable: game::available().err(),
            threats: cfg.threats,
            x: cfg.x,
            y: cfg.y,
        }
    }
}

impl Module for CombatHud {
    fn name(&self) -> &str {
        "combathud"
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
        if let Err(e) = hits::set_active(on) {
            log!("combathud: {e}");
            return;
        }
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "threats",
                "周囲の敵数を表示",
                "16ブロック以内の敵対モブとプレイヤーの数を表示します。",
                self.threats,
            ),
            Detail::x_offset(self.x, "Positive: from left. Negative: from right."),
            Detail::y_offset(self.y, "Positive: from top. Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "threats" => self.threats = value.boolean()?,
            "x" => self.x = value.number()? as i32,
            "y" => self.y = value.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        match hits::feed(unsafe { GetTickCount64() }) {
            Some(feed) => format!("撃破{} / ヒット{}", feed.kills, feed.hits),
            None => "（ワールド未接続）".into(),
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if !super::hud_view() {
            return;
        }
        let now = unsafe { GetTickCount64() };
        let Some(feed) = hits::feed(now) else {
            return;
        };
        self.paint(ui, &feed, now);
    }
}

impl CombatHud {
    fn paint(&self, ui: &Ui, feed: &hits::Feed, now: u64) {
        let screen = ui.io().display_size;
        let line = ui.text_line_height_with_spacing();
        // Rows: counts, combo, (threats), separator gap, focus name, bar.
        let has_focus = feed.focus.is_some();
        let mut rows = 2.0;
        if self.threats {
            rows += 1.;
        }
        let focus_height = if has_focus { line * 2. + 6. } else { line };
        let height = PADDING * 2. + rows * line + 6. + focus_height;
        let size = [WIDTH.min(screen[0] - 8.), height];
        crate::hud_editor::measure("combathud", size);
        let left = origin(self.x, screen[0], size[0]);
        let top = origin(self.y, screen[1], size[1]);
        let draw = ui.get_background_draw_list();
        theme::panel(&draw, [left, top], [left + size[0], top + size[1]]);
        let mut y = top + PADDING;
        let x = left + PADDING;
        let right = left + size[0] - PADDING;

        draw.add_text([x, y], theme::ACCENT, "戦闘");
        let counts = format!("撃破 {}   ヒット {}", feed.kills, feed.hits);
        let cw = ui.calc_text_size(&counts)[0];
        draw.add_text([right - cw, y], theme::TEXT, &counts);
        y += line;

        // Combo and time since the last hit.
        let since = if feed.last_hit_ms == 0 {
            "まだ攻撃していません".to_string()
        } else {
            format!(
                "{:.1}秒前",
                now.saturating_sub(feed.last_hit_ms) as f32 / 1000.
            )
        };
        let combo_color = if feed.combo >= 5 {
            theme::WARNING
        } else {
            theme::MUTED
        };
        draw.add_text([x, y], combo_color, format!("コンボ {}", feed.combo));
        let sw = ui.calc_text_size(&since)[0];
        draw.add_text([right - sw, y], theme::MUTED, &since);
        y += line;

        if self.threats {
            let color = if feed.threats > 0 {
                theme::DANGER
            } else {
                theme::MUTED
            };
            draw.add_text([x, y], color, format!("周囲の敵 {}", feed.threats));
            y += line;
        }

        y += 6.;
        match &feed.focus {
            Some(focus) => {
                let title = format!("{}   {:.1}m", focus.name, focus.distance);
                let title = crate::text::fit_width(&title, size[0] - 2. * PADDING, |s| {
                    ui.calc_text_size(s)[0]
                });
                draw.add_text([x, y], entity_kind::mark(focus.kind), &title);
                y += line;
                // Health bar.
                let frac = if focus.max_health > 0. {
                    (focus.health / focus.max_health).clamp(0., 1.)
                } else {
                    0.
                };
                let bar_top = y + 2.;
                let bar_bot = bar_top + line * 0.55;
                draw.add_rect([x, bar_top], [right, bar_bot], theme::SLOT)
                    .filled(true)
                    .rounding(3.)
                    .build();
                let fill = x + (right - x) * frac;
                let bar_color = if frac > 0.5 {
                    [0.4, 0.85, 0.45, 1.]
                } else if frac > 0.25 {
                    theme::WARNING
                } else {
                    theme::DANGER
                };
                if fill > x + 1. {
                    draw.add_rect([x, bar_top], [fill, bar_bot], bar_color)
                        .filled(true)
                        .rounding(3.)
                        .build();
                }
                let hp = format!("{:.0} / {:.0}", focus.health, focus.max_health);
                let hw = ui.calc_text_size(&hp)[0];
                draw.add_text([(x + right - hw) * 0.5, bar_top - 1.], theme::TEXT, &hp);
            }
            None => {
                draw.add_text([x, y], theme::MUTED, "対象なし");
            }
        }
    }
}

/// The panel with a sample fight, for the headless UI preview.
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let hud = CombatHud::new(&config::CombatHud {
        x: -20,
        y: 120,
        ..Default::default()
    });
    let feed = hits::Feed {
        hits: 12,
        kills: 3,
        last_hit_ms: 800,
        combo: 6,
        best_combo: 8,
        threats: 2,
        focus: Some(hits::Focus {
            name: "ゾンビ".into(),
            kind: entity_kind::Kind::Hostile,
            health: 7.0,
            max_health: 20.0,
            distance: 3.4,
        }),
        markers: Vec::new(),
    };
    hud.paint(ui, &feed, 1000);
}
