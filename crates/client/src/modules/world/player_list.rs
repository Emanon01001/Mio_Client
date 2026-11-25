//! Hold-to-view player roster.
use crate::game::{self, friends, player_roster};
use crate::modules::settings::{Detail, Value};
use crate::modules::{key_or_warn, Module};
use crate::{config, gui, input, log, theme};
use hudhook::imgui::{DrawListMut, Ui};

pub(in crate::modules) struct PlayerList {
    enabled: bool,
    hold_key: u16,
    rows: i32,
    columns: Columns,
    show_friends: bool,
    page: usize,
    previous_keys: [bool; 2],
}

impl PlayerList {
    pub(in crate::modules) fn new(config: &config::PlayerList) -> Self {
        Self {
            enabled: false,
            hold_key: key_or_warn("playerlist.hold_key", &config.hold_key),
            rows: config.rows.clamp(4, 32),
            columns: Columns {
                distance: config.distance,
                health: config.health,
            },
            show_friends: config.friends,
            page: 0,
            previous_keys: [false; 2],
        }
    }
}

impl Module for PlayerList {
    fn name(&self) -> &str {
        "playerlist"
    }

    fn key(&self) -> u16 {
        0
    }

    fn supports_toggle_key(&self) -> bool {
        false
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        let result = if on {
            player_roster::prepare().and_then(|_| game::acquire())
        } else {
            game::release()
        };
        if let Err(error) = result {
            log!("Player List: {error}");
            return;
        }
        player_roster::set_hold_key(if on { self.hold_key } else { 0 });
        self.enabled = on;
    }

    fn status(&self) -> String {
        if self.hold_key == 0 {
            "表示キーが未割り当てです".into()
        } else {
            format!("{} を長押し", input::display_key_name(self.hold_key))
        }
    }

    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::key(
                "hold_key",
                "表示キー",
                "ワールド内で押している間だけ参加者一覧を表示します。",
                self.hold_key,
            ),
            Detail::integer(
                "rows",
                "列あたりの人数",
                "画面の高さに収まるよう、必要に応じて列を増やします。",
                self.rows,
                4,
                32,
            ),
            Detail::boolean(
                "distance",
                "距離を表示",
                "取得できるプレイヤーまでの距離を表示します。",
                self.columns.distance,
            ),
            Detail::boolean(
                "health",
                "体力を表示",
                "取得できるプレイヤーの現在と最大の体力を表示します。",
                self.columns.health,
            ),
            Detail::boolean(
                "friends",
                "フレンドを表示",
                "Friendsに登録した名前に印を付けます。",
                self.show_friends,
            ),
        ]
    }

    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "hold_key" => {
                self.hold_key = value.key()?;
                if self.enabled {
                    player_roster::set_hold_key(self.hold_key);
                }
            }
            "rows" => self.rows = (value.number()? as i32).clamp(4, 32),
            "distance" => self.columns.distance = value.boolean()?,
            "health" => self.columns.health = value.boolean()?,
            "friends" => self.show_friends = value.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }

    fn draw(&mut self, ui: &Ui) {
        if !self.enabled
            || !input::key_down(self.hold_key)
            || gui::captures_input()
            || !game::in_world_view_now()
            || game::inventory::screen_open()
            || !input::is_foreground()
        {
            self.page = 0;
            self.previous_keys = [false; 2];
            return;
        }
        let keys = [0x25, 0x27].map(input::key_down);
        if keys[0] && !self.previous_keys[0] {
            self.page = self.page.saturating_sub(1);
        }
        if keys[1] && !self.previous_keys[1] {
            self.page += 1;
        }
        self.previous_keys = keys;
        let roster = player_roster::snapshot();
        let friends = self.show_friends.then(friends::list);
        draw_list(
            ui,
            roster.as_deref(),
            self.rows as usize,
            self.columns,
            friends.as_deref().map_or(&[], Vec::as_slice),
            &mut self.page,
        );
    }
}

#[derive(Clone, Copy)]
struct Columns {
    distance: bool,
    health: bool,
}

fn page_range(
    count: usize,
    page_size: usize,
    page: usize,
) -> (std::ops::Range<usize>, usize, usize) {
    let pages = count.div_ceil(page_size).max(1);
    let page = page.min(pages - 1);
    let start = page * page_size;
    (start..(start + page_size).min(count), page, pages)
}

fn draw_list(
    ui: &Ui,
    roster: Option<&player_roster::Roster>,
    rows: usize,
    show: Columns,
    friends: &[String],
    page: &mut usize,
) {
    let [width, height] = ui.io().display_size;
    if !width.is_finite() || !height.is_finite() || width < 120. || height < 120. {
        return;
    }
    let line = ui.text_line_height() + 7.;
    let font = ui.current_font_size();
    let rows = rows.min(((height * 0.82 - 20. - 4. * line) / line).max(1.) as usize);
    let count = roster.map_or(0, |r| r.players.len());
    let distance_width = if show.distance { 4.8 * font } else { 0. };
    let health_width = if show.health { 6.5 * font } else { 0. };
    let desired_width = 14. * font + distance_width + health_width;
    let max_columns = (((width - 48.) / desired_width) as usize).clamp(1, 4);
    let columns = count.div_ceil(rows).clamp(1, max_columns);
    let (range, displayed_page, pages) = page_range(count, rows * columns, *page);
    let visible = range.len();
    let column_width = ((width - 48.) / columns as f32).min(desired_width);
    let panel_width = 24. + column_width * columns as f32;
    let rows = visible.div_ceil(columns).max(1);
    let panel_height = 20. + (rows as f32 + 3.) * line + if pages > 1 { line } else { 0. };
    let x = (width - panel_width) / 2.;
    let y = 16.;
    let draw = ui.get_foreground_draw_list();
    theme::panel(&draw, [x, y], [x + panel_width, y + panel_height]);
    draw.add_text([x + 12., y + 11.], theme::TEXT, "Player List");
    if let Some(roster) = roster {
        *page = displayed_page;
        let count_text = match roster.loaded {
            Some(loaded) => format!("参加者 {count} 人  /  読み込み済み {loaded} 人"),
            None => format!("参加者 {count} 人"),
        };
        cell(
            &draw,
            [x + 12., y + 11. + line],
            panel_width - 24.,
            line,
            theme::MUTED,
            &count_text,
        );
        let detail_scale =
            ((column_width - 8. * font) / (distance_width + health_width).max(1.)).clamp(0., 1.);
        let distance_width = distance_width * detail_scale;
        let health_width = health_width * detail_scale;
        let name_width = column_width - distance_width - health_width;
        let header = y + 12. + line * 2.;
        for column in 0..visible.div_ceil(rows) {
            let left = x + 12. + column as f32 * column_width;
            let distance_x = left + name_width;
            let health_x = distance_x + distance_width;
            cell(
                &draw,
                [left + 12., header],
                name_width - 12.,
                line,
                theme::MUTED,
                "名前",
            );
            cell(
                &draw,
                [distance_x, header],
                distance_width,
                line,
                theme::MUTED,
                "距離",
            );
            cell(
                &draw,
                [health_x, header],
                health_width,
                line,
                theme::MUTED,
                "HP",
            );
        }
        for (i, player) in roster.players[range].iter().enumerate() {
            let left = x + 12. + (i / rows) as f32 * column_width;
            let top = header + (1 + i % rows) as f32 * line;
            let label = if player.local {
                Some(("自分", theme::ACCENT))
            } else if friends.iter().any(|f| f.eq_ignore_ascii_case(&player.name)) {
                Some(("フレンド", theme::SUCCESS))
            } else {
                None
            };
            let marker = if player.loaded {
                theme::SUCCESS
            } else {
                theme::MUTED
            };
            draw.add_circle([left + 4., top + line * 0.5], 2., marker)
                .filled(true)
                .build();
            let label_width = label.map_or(0., |(s, _)| ui.calc_text_size(s)[0] + 8.);
            cell(
                &draw,
                [left + 12., top],
                name_width - 14. - label_width,
                line,
                theme::TEXT,
                &player.name,
            );
            if let Some((tag, color)) = label {
                cell(
                    &draw,
                    [left + (name_width - label_width).max(12.), top],
                    label_width - 5.,
                    line,
                    color,
                    tag,
                );
            }
            cell(
                &draw,
                [left + name_width, top],
                distance_width,
                line,
                theme::TEXT,
                player.distance.as_deref().unwrap_or("—"),
            );
            cell(
                &draw,
                [left + name_width + distance_width, top],
                health_width,
                line,
                theme::TEXT,
                player.health.as_deref().unwrap_or("—"),
            );
        }
        if pages > 1 {
            draw.add_text(
                [x + 12., y + panel_height - line],
                theme::MUTED,
                format!("← {} / {pages} →", displayed_page + 1),
            );
        } else if count == 0 {
            draw.add_text([x + 12., header], theme::MUTED, "参加者なし");
        }
    } else {
        draw.add_text([x + 12., y + 38.], theme::MUTED, "参加者一覧を取得中");
    }
}

fn cell(
    draw: &DrawListMut<'_>,
    at: [f32; 2],
    width: f32,
    height: f32,
    color: [f32; 4],
    text: &str,
) {
    if width > 3. {
        draw.with_clip_rect_intersect(at, [at[0] + width - 3., at[1] + height], || {
            draw.add_text([at[0], at[1] + 2.], color, text);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::page_range;

    #[test]
    fn pages_cover_all_players_and_clamp_when_the_roster_shrinks() {
        assert_eq!(page_range(150, 72, 0), (0..72, 0, 3));
        assert_eq!(page_range(150, 72, 1), (72..144, 1, 3));
        assert_eq!(page_range(150, 72, 2), (144..150, 2, 3));
        assert_eq!(page_range(7, 72, 2), (0..7, 0, 1));
        assert_eq!(page_range(0, 72, 2), (0..0, 0, 1));
    }
}
