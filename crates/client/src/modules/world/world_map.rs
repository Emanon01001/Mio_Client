//! WorldMap: a full-screen map of the terrain remembered so far (see
//! `game::blocks::surface`), opened with a key. While open it takes the mouse
//! and keyboard like the menu: the wheel zooms about the cursor, dragging
//! moves the map, a right click centres it on the player again, and Esc or
//! the key closes it. Saved places and slime chunks are marked. Owned terrain
//! archives are processed on the existing client worker, not in draw.
//!
//! The remembered terrain is sampled into a grid whose cells are never
//! smaller than `MIN_CELL` pixels (coarser when zoomed out), covering the
//! view and a margin around it, so dragging redraws from the same grid and
//! samples again only past the margin, on zooming, or every `REFRESH`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
mod auto_file;
mod autosave;
#[cfg(test)]
mod fixtures;
mod jobs;
mod storage;

use hudhook::imgui::{ImColor32, Key, MouseButton, Ui};

use crate::game::{self, blocks, blocks::surface};
use crate::modules::map_marks::{self, SlimeCache};
use crate::modules::minimap::{self, Fill};
use crate::modules::settings::{Detail, Value};
use crate::modules::{key_or_warn, Module};
use crate::{api, config, input, log};

const MIN_SCALE: f32 = 0.25;
const MAX_SCALE: f32 = 16.;
/// Smallest grid cell on screen (pixels).
const MIN_CELL: f32 = 6.;
/// The grid covers this fraction of the view beyond each edge.
const MARGIN: f32 = 0.25;
/// Remembered terrain changes while the map is open: sample it again, at
/// most this often, and only when it changed.
const REFRESH: Duration = Duration::from_secs(2);
/// Slime chunks are tinted once a chunk is at least this wide (pixels).
const SLIME_MIN_CHUNK: f32 = 10.;
/// Space around the map, and the heights of its header and footer.
const OUTER: f32 = 24.;
const BAR: f32 = 30.;

const BACKDROP: [f32; 4] = [0.02, 0.03, 0.05, 0.92];
const EMPTY: [f32; 4] = [0.06, 0.08, 0.11, 1.];

fn close_shortcut(previous: &mut bool, down: bool, key: u16, typing: bool) -> bool {
    let was = std::mem::replace(previous, down);
    let text_key = matches!(key,0x30..=0x39|0x41..=0x5a);
    down && !was && !(text_key && typing)
}

/// Whether an Esc press closes the map. Not while the menu is open or it
/// just closed the menu (the press is the menu's), nor while it cancels
/// typing or closes a popup in the map's own window.
fn escape_closes(menu_open: bool, menu_closing: bool, typing: bool, popup: bool) -> bool {
    !menu_open && !menu_closing && !typing && !popup
}

pub(crate) fn poll_storage() {
    autosave::poll();
    jobs::poll();
}
pub(crate) fn start_storage(root: &Path, cfg: &config::WorldMap) {
    autosave::start(root, cfg);
}
pub(crate) fn storage_diagnostic() -> String {
    autosave::status()
}
pub(crate) fn shutdown_storage() {
    jobs::poll();
    autosave::shutdown();
    jobs::shutdown();
}

/// The cell size (blocks, a power of two) for `scale` pixels per block.
fn cell_step(scale: f32) -> i32 {
    let mut step = 1;
    while (step as f32) * scale < MIN_CELL && step < 4096 {
        step *= 2;
    }
    step
}

/// The grid for a view centred on `center` (blocks) showing `view` blocks:
/// its first block (aligned to `step`) and its size in cells.
fn grid_bounds(center: [f64; 2], view: [f32; 2], step: i32) -> ([i32; 2], [usize; 2]) {
    let mut origin = [0; 2];
    let mut cells = [0; 2];
    for k in 0..2 {
        let cover = view[k] as f64 * (1. + 2. * MARGIN as f64);
        let first = ((center[k] - cover / 2.) / step as f64).floor() as i64 * step as i64;
        origin[k] = first.clamp(-30_000_000, 30_000_000) as i32;
        cells[k] = (cover / step as f64).ceil() as usize + 2;
    }
    (origin, cells)
}

/// The remembered terrain as rectangles, in blocks from `origin`.
struct Grid {
    step: i32,
    origin: [i32; 2],
    cells: [usize; 2],
    rects: Vec<Fill>,
    built: Instant,
    /// `surface::memory_revision()` it was built from.
    revision: u64,
}

impl Grid {
    fn build(origin: [i32; 2], cells: [usize; 2], step: i32) -> Self {
        // Read before sampling: a change during it makes the next check rebuild.
        let revision = surface::memory_revision();
        let columns = surface::remembered_area(origin[0], origin[1], step, cells[0], cells[1]);
        let at = |x: usize, z: usize| columns[z * cells[0] + x];
        let rects = minimap::merge(cells[0], cells[1], step as f32, |x, z| {
            let c = at(x, z);
            c.known
                .then(|| minimap::shade(c, (z > 0).then(|| at(x, z - 1))))
        })
        .into_iter()
        .map(|(a, b, color)| (a, b, ImColor32::from(color).to_bits()))
        .collect();
        Self {
            step,
            origin,
            cells,
            rects,
            built: Instant::now(),
            revision,
        }
    }

    /// Whether the grid still serves a view of `view` blocks around `center`
    /// with cells of `step`, the memory being at `revision`.
    fn covers(&self, center: [f64; 2], view: [f32; 2], step: i32, revision: u64) -> bool {
        step == self.step
            && (revision == self.revision || self.built.elapsed() < REFRESH)
            && (0..2).all(|k| {
                let start = self.origin[k] as f64;
                let end = start + (self.cells[k] as i64 * self.step as i64) as f64;
                center[k] - view[k] as f64 / 2. >= start && center[k] + view[k] as f64 / 2. <= end
            })
    }
}

pub struct WorldMap {
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    waypoints: bool,
    slime_chunks: bool,
    /// The block at the middle of the map.
    center: [f64; 2],
    /// Pixels per block.
    scale: f32,
    /// Keep the player in the middle (until the map is dragged or zoomed).
    follow: bool,
    key_was_down: bool,
    /// A popup of the map's window was open last frame (Esc closes it,
    /// before this frame's draw sees it).
    popup_was_open: bool,
    grid: Option<Grid>,
    placed: Vec<Fill>,
    slime: SlimeCache,
    directory: PathBuf,
    save_name: String,
    selected_file: usize,
    storage_error: String,
    auto_save: bool,
    save_interval: i32,
}

impl WorldMap {
    pub fn new(cfg: &config::WorldMap, root: &Path) -> Self {
        Self {
            key: key_or_warn("worldmap", &cfg.key),
            enabled: false,
            unavailable: blocks::available().err(),
            waypoints: cfg.waypoints,
            slime_chunks: cfg.slime_chunks,
            center: [0.; 2],
            scale: 2.,
            follow: true,
            key_was_down: false,
            popup_was_open: false,
            grid: None,
            placed: Vec::new(),
            slime: SlimeCache::default(),
            directory: root.join("maps"),
            save_name: "world".into(),
            selected_file: 0,
            storage_error: String::new(),
            auto_save: cfg.auto_save,
            save_interval: cfg.save_interval.clamp(5, 600),
        }
    }
}

impl Module for WorldMap {
    fn name(&self) -> &str {
        "worldmap"
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
        if on {
            if let Some(e) = &self.unavailable {
                log!("worldmapを利用できません: {e}");
                return;
            }
            // Also keeps a key typed into chat or a menu from opening it.
            if !crate::modules::hud_view() {
                log!("worldmap: ワールドの画面で開いてください");
                return;
            }
        }
        if let Err(e) = if on { game::acquire() } else { game::release() } {
            log!("worldmap: {e}");
            return;
        }
        // While open the surface keeps being read around the player.
        surface::set_active(on);
        crate::gui::MAP_OPEN.store(on, std::sync::atomic::Ordering::Release);
        self.enabled = on;
        self.follow = true;
        self.popup_was_open = false;
        self.grid = None;
        // The key that opened the map is still down: close on its next press.
        self.key_was_down = true;
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean("auto_save", "地形を自動保存・復元", "地図を閉じていても地形を記録し、ワールドID・ディメンション別に保存します。退出時にも最終保存します。", self.auto_save),
            Detail::integer("save_interval", "自動保存の間隔（秒）", "変更した地形を定期的に保存します。", self.save_interval, 5, 600),
            Detail::boolean(
                "waypoints",
                "地点を表示",
                "ウェイポイント（水色）と死亡地点（赤紫）を名前付きで表示します。",
                self.waypoints,
            ),
            Detail::boolean(
                "slime_chunks",
                "スライムチャンクを表示",
                "オーバーワールドで、ある程度拡大したときにスライムチャンクを緑で塗ります。",
                self.slime_chunks,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "auto_save" => self.auto_save = value.boolean()?,
            "save_interval" => self.save_interval = value.number()?.round().clamp(5., 600.) as i32,
            "waypoints" => self.waypoints = value.boolean()?,
            "slime_chunks" => self.slime_chunks = value.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        autosave::request(self.auto_save, self.save_interval);
        Ok(())
    }
    fn status(&self) -> String {
        match (&self.unavailable, self.enabled) {
            (Some(e), _) => format!("({e})"),
            (None, true) => format!("表示中 / 1ブロック {:.2}px", self.scale),
            (None, false) => match surface::remembered_chunks() {
                0 => "記憶した地形なし".into(),
                n => format!("記憶{n}チャンク"),
            },
        }
    }
    fn draw_settings(&mut self, ui: &Ui) {
        self.storage_controls(ui);
    }
    fn draw(&mut self, ui: &Ui) {
        // The module's key (the key that opened it) or Esc closes the map.
        let down = input::key_down(self.key);
        let pressed = close_shortcut(
            &mut self.key_was_down,
            down,
            self.key,
            ui.io().want_text_input,
        );
        let escape = ui.is_key_pressed_no_repeat(Key::Escape)
            && escape_closes(
                crate::gui::menu_open(),
                crate::gui::escape_closing(),
                ui.io().want_text_input,
                self.popup_was_open,
            );
        if pressed || escape {
            if escape {
                // Keep the Esc away from the game (its pause menu) until released.
                crate::gui::hold_escape();
            }
            self.set_enabled(false);
            return;
        }
        let player = api::game().ok().and_then(|g| g.player().ok());
        let position: Option<[f32; 3]> = player.as_ref().map(|p| p.position.into());
        let yaw = player.as_ref().map_or(0., |p| p.rotation.yaw);
        self.paint(ui, position.map(|p| [p[0], p[2]]), yaw);
        let screen = ui.io().display_size;
        ui.window("WorldMap · 保存")
            .position(
                [(screen[0] - 424.).max(0.), OUTER],
                hudhook::imgui::Condition::FirstUseEver,
            )
            .size([400., 310.], hudhook::imgui::Condition::FirstUseEver)
            .save_settings(false)
            .build(|| self.storage_controls(ui));
        self.popup_was_open = crate::gui::popup_open();
    }
}

impl WorldMap {
    fn request_save(&mut self) {
        let result = (|| {
            let context = surface::storage_context()?;
            let snapshot = surface::save_snapshot()?;
            if context.owner != snapshot.owner {
                return Err("ワールドが変わったため保存を中止しました".into());
            }
            jobs::submit(
                self.directory.clone(),
                jobs::Action::Save {
                    label: self.save_name.clone(),
                    terrain: storage::Terrain {
                        dimension: context.dimension,
                        world_id: context.world_id,
                        chunks: snapshot.chunks,
                    },
                },
            )
        })();
        self.storage_error = result.err().unwrap_or_default();
    }
    /// Writes all remembered terrain of this dimension as a PNG (on the worker).
    fn request_png(&mut self) {
        let result = surface::save_snapshot().and_then(|snapshot| {
            jobs::submit(
                self.directory.clone(),
                jobs::Action::ExportPng {
                    label: self.save_name.clone(),
                    chunks: snapshot.chunks,
                },
            )
        });
        self.storage_error = result.err().unwrap_or_default();
    }
    fn storage_controls(&mut self, ui: &Ui) {
        let mut automatic = self.auto_save;
        if ui.checkbox("地形を自動保存・復元", &mut automatic) {
            self.auto_save = automatic;
            autosave::request(self.auto_save, self.save_interval);
        }
        ui.text_wrapped(autosave::status());
        if ui.button("今すぐ自動保存") {
            autosave::force();
        }
        ui.separator();
        ui.text_wrapped("記憶した地形を保存し、次回起動後も読み戻せます。");
        ui.text_wrapped("以下は別名で残す手動保存です。読み込みは現在のディメンションへ追加し、取得済みの地形を優先します。");
        ui.text_wrapped(format!("保存先: {}", self.directory.display()));
        ui.input_text("保存名", &mut self.save_name).build();
        if let Some((end, _)) = self.save_name.char_indices().nth(64) {
            self.save_name.truncate(end);
        }
        let (busy, status, files) = jobs::state();
        let disabled = ui.begin_disabled(busy);
        if ui.button("地形を保存") {
            self.request_save();
        }
        ui.same_line();
        if ui.button("PNGで書き出す") {
            self.request_png();
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "記憶した地形全体を北が上の画像にします（1ブロック=1px、未取得の場所は透明）。",
            );
        }
        if ui.button("保存一覧を更新") {
            self.storage_error = jobs::submit(self.directory.clone(), jobs::Action::Refresh)
                .err()
                .unwrap_or_default();
        }
        if files.is_empty() {
            ui.text_disabled("保存した地形はありません。一覧を更新すると読み込めます。");
        } else {
            self.selected_file = self.selected_file.min(files.len() - 1);
            let names: Vec<_> = files.iter().map(|e| e.name.as_str()).collect();
            ui.combo_simple_string("保存した地形", &mut self.selected_file, &names);
            ui.text_disabled(format!(
                "{:.1} KiB",
                files[self.selected_file].bytes as f64 / 1024.
            ));
            if ui.button("選択した地形を読み込む") {
                self.storage_error = (|| {
                    let context = surface::storage_context()?;
                    jobs::submit(
                        self.directory.clone(),
                        jobs::Action::Load {
                            filename: files[self.selected_file].name.clone(),
                            owner: context.owner,
                            dimension: context.dimension,
                            world_id: context.world_id,
                        },
                    )
                })()
                .err()
                .unwrap_or_default();
            }
        }
        drop(disabled);
        if !status.is_empty() {
            ui.text_wrapped(status);
        }
        if !self.storage_error.is_empty() {
            ui.text_wrapped(&self.storage_error);
        }
    }
    /// Draws the map; `player` is the player's X/Z (None outside a world).
    fn paint(&mut self, ui: &Ui, player: Option<[f32; 2]>, yaw: f32) {
        let screen = ui.io().display_size;
        if screen[0] < 200. || screen[1] < 160. {
            return;
        }
        let min = [OUTER, OUTER + BAR];
        let max = [screen[0] - OUTER, screen[1] - OUTER - BAR];
        let size = [max[0] - min[0], max[1] - min[1]];
        let middle = [min[0] + size[0] * 0.5, min[1] + size[1] * 0.5];
        if let Some(p) = player.filter(|_| self.follow) {
            self.center = [p[0] as f64, p[1] as f64];
        }
        self.handle_input(ui, min, max, middle);

        let scale = self.scale;
        let view = [size[0] / scale, size[1] / scale];
        let step = cell_step(scale);
        if !self
            .grid
            .as_ref()
            .is_some_and(|g| g.covers(self.center, view, step, surface::memory_revision()))
        {
            let (origin, cells) = grid_bounds(self.center, view, step);
            self.grid = Some(Grid::build(origin, cells, step));
        }
        let center = self.center;
        let to_screen = |x: f32, z: f32| {
            [
                middle[0] + ((x as f64 - center[0]) * scale as f64) as f32,
                middle[1] + ((z as f64 - center[1]) * scale as f64) as f32,
            ]
        };

        let draw = ui.get_background_draw_list();
        draw.add_rect([0., 0.], screen, BACKDROP)
            .filled(true)
            .build();
        draw.add_rect(min, max, EMPTY).filled(true).build();
        draw.with_clip_rect_intersect(min, max, || {
            if let Some(grid) = &self.grid {
                let corner = to_screen(grid.origin[0] as f32, grid.origin[1] as f32);
                self.placed.clear();
                for &(a, b, color) in &grid.rects {
                    let a =
                        std::array::from_fn::<_, 2, _>(|k| (corner[k] + a[k] * scale).max(min[k]));
                    let b =
                        std::array::from_fn::<_, 2, _>(|k| (corner[k] + b[k] * scale).min(max[k]));
                    if a[0] < b[0] && a[1] < b[1] {
                        self.placed.push((a, b, color));
                    }
                }
                minimap::fill(&self.placed);
            }
            let overworld = matches!(game::world::realm(), Some(game::world::Realm::Overworld));
            if self.slime_chunks && overworld && scale * 16. >= SLIME_MIN_CHUNK {
                let half = [view[0] as f64 / 2., view[1] as f64 / 2.];
                map_marks::slime_chunks(
                    &draw,
                    &mut self.slime,
                    [(center[0] - half[0]) as f32, (center[1] - half[1]) as f32],
                    [(center[0] + half[0]) as f32, (center[1] + half[1]) as f32],
                    to_screen,
                );
            }
            if self.waypoints {
                let me = player.map_or(middle, |p| to_screen(p[0], p[1]));
                for point in crate::modules::waypoints::on_map() {
                    let p = to_screen(point.x as f32 + 0.5, point.z as f32 + 0.5);
                    match map_marks::edge_point(me, p, min, max, 10.) {
                        Some((at, unit)) => map_marks::arrow(&draw, at, unit, point.death),
                        None => {
                            map_marks::place(&draw, p, 5., point.death);
                            if !point.name.is_empty() {
                                let text = ui.calc_text_size(&point.name);
                                let at = [p[0] - text[0] * 0.5, p[1] - 9. - text[1]];
                                draw.add_rect(
                                    [at[0] - 3., at[1] - 1.],
                                    [at[0] + text[0] + 3., at[1] + text[1] + 1.],
                                    [0., 0., 0., 0.6],
                                )
                                .filled(true)
                                .rounding(3.)
                                .build();
                                draw.add_text(at, crate::theme::TEXT, &point.name);
                            }
                        }
                    }
                }
            }
            if let Some(p) = player {
                // yaw 0 faces +Z (south, down on this north-up map).
                let p = to_screen(p[0], p[1]);
                let (sin, cos) = yaw.to_radians().sin_cos();
                let f = [-sin, cos];
                let s = [-f[1], f[0]];
                let at = |a: f32, b: f32| [p[0] + f[0] * a + s[0] * b, p[1] + f[1] * a + s[1] * b];
                draw.add_triangle(at(10., 0.), at(-6., 6.5), at(-6., -6.5), [1.; 4])
                    .filled(true)
                    .build();
                draw.add_triangle(at(10., 0.), at(-6., 6.5), at(-6., -6.5), [0., 0., 0., 0.9])
                    .thickness(1.5)
                    .build();
            }
        });
        draw.add_rect(min, max, crate::theme::BORDER).build();

        // Header: title and controls. Footer: the block under the cursor
        // and a scale bar.
        let text_h = ui.text_line_height();
        let header_y = OUTER + (BAR - text_h) * 0.5;
        draw.add_text([min[0], header_y], crate::theme::TEXT, "地図");
        let help =
            "ホイール: 拡大縮小  ・  ドラッグ: 移動  ・  右クリック: 現在地へ  ・  Esc: 閉じる";
        let help_w = ui.calc_text_size(help)[0];
        draw.add_text([max[0] - help_w, header_y], crate::theme::MUTED, help);
        let footer_y = max[1] + (BAR - text_h) * 0.5;
        let mouse = ui.io().mouse_pos;
        let mut footer = match surface::remembered_chunks() {
            0 => "記憶した地形がありません（歩いた場所が記録されます）".to_string(),
            n => format!("記憶{n}チャンク"),
        };
        if (0..2).all(|k| mouse[k] >= min[k] && mouse[k] <= max[k]) {
            let x = center[0] + ((mouse[0] - middle[0]) / scale) as f64;
            let z = center[1] + ((mouse[1] - middle[1]) / scale) as f64;
            footer = format!("X {}  Z {}    {footer}", x.floor() as i64, z.floor() as i64);
        }
        draw.add_text([min[0], footer_y], crate::theme::TEXT, &footer);
        let (blocks, length) = scale_bar(scale);
        let bar_y = footer_y + text_h * 0.5;
        let label = format!("{blocks} m");
        let label_w = ui.calc_text_size(&label)[0];
        let bar_x = max[0] - label_w - 8. - length;
        draw.add_line([bar_x, bar_y], [bar_x + length, bar_y], crate::theme::TEXT)
            .thickness(2.)
            .build();
        for x in [bar_x, bar_x + length] {
            draw.add_line([x, bar_y - 4.], [x, bar_y + 4.], crate::theme::TEXT)
                .thickness(2.)
                .build();
        }
        draw.add_text([max[0] - label_w, footer_y], crate::theme::TEXT, &label);
    }

    /// Zoom with the wheel (about the cursor), drag to move, right click to
    /// follow the player again. Not while the cursor is over a menu window.
    fn handle_input(&mut self, ui: &Ui, min: [f32; 2], max: [f32; 2], middle: [f32; 2]) {
        let io = ui.io();
        let mouse = io.mouse_pos;
        let over_map =
            !io.want_capture_mouse && (0..2).all(|k| mouse[k] >= min[k] && mouse[k] <= max[k]);
        if !over_map {
            return;
        }
        if io.mouse_wheel != 0. {
            let before = self.scale;
            let after = (before * 1.25f32.powf(io.mouse_wheel)).clamp(MIN_SCALE, MAX_SCALE);
            // The block under the cursor stays under it.
            for k in 0..2 {
                let offset = (mouse[k] - middle[k]) as f64;
                self.center[k] += offset / before as f64 - offset / after as f64;
            }
            self.scale = after;
            self.follow = false;
        }
        if ui.is_mouse_down(MouseButton::Left) {
            let delta = io.mouse_delta;
            if delta[0] != 0. || delta[1] != 0. {
                self.center[0] -= (delta[0] / self.scale) as f64;
                self.center[1] -= (delta[1] / self.scale) as f64;
                self.follow = false;
            }
        }
        if ui.is_mouse_clicked(MouseButton::Right) {
            self.follow = true;
        }
    }
}

/// A round length (blocks) for the scale bar and its length in pixels,
/// about 80 to 200 pixels long.
fn scale_bar(scale: f32) -> (u32, f32) {
    let mut blocks = 1u32;
    for candidate in [1u32, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000] {
        blocks = candidate;
        if candidate as f32 * scale >= 80. {
            break;
        }
    }
    (blocks, blocks as f32 * scale)
}

/// The map as it would look over a remembered island, for the headless UI
/// preview (the remembered terrain is not available without the game, so
/// the grid is built from a made-up one).
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let mut map = WorldMap::new(&config::WorldMap::default(), Path::new(""));
    map.unavailable = None;
    map.scale = 3.;
    map.follow = false;
    map.center = [0., 0.];
    let step = cell_step(map.scale);
    let screen = ui.io().display_size;
    let view = [
        (screen[0] - 2. * OUTER) / map.scale,
        (screen[1] - 2. * (OUTER + BAR)) / map.scale,
    ];
    let (origin, cells) = grid_bounds(map.center, view, step);
    let rects = minimap::merge(cells[0], cells[1], step as f32, |x, z| {
        let (bx, bz) = (origin[0] + x as i32 * step, origin[1] + z as i32 * step);
        let d = ((bx * bx + bz * bz) as f32).sqrt();
        (d < 180.).then(|| {
            let color = if d > 165. {
                [214, 200, 150]
            } else if (bx / 24 + bz / 24) % 3 == 0 {
                [50, 105, 40]
            } else {
                [92, 140, 60]
            };
            minimap::shade(
                surface::Column {
                    height: (70. - d * 0.05) as i16,
                    color,
                    known: true,
                    remembered: false,
                },
                None,
            )
        })
    })
    .into_iter()
    .map(|(a, b, color)| (a, b, ImColor32::from(color).to_bits()))
    .collect();
    map.grid = Some(Grid {
        step,
        origin,
        cells,
        rects,
        built: Instant::now(),
        revision: surface::memory_revision(),
    });
    map.paint(ui, Some([10., -20.]), 45.);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_key_release_rearms_and_typing_does_not_close_the_map() {
        // Esc closes the map, unless the press belongs to the menu, typing or a popup.
        assert!(escape_closes(false, false, false, false));
        assert!(!escape_closes(true, false, false, false));
        assert!(!escape_closes(false, true, false, false));
        assert!(!escape_closes(false, false, true, false));
        assert!(!escape_closes(false, false, false, true));
        let mut held = true;
        assert!(!close_shortcut(&mut held, true, 0x4d, false));
        assert!(!close_shortcut(&mut held, false, 0x4d, false));
        assert!(close_shortcut(&mut held, true, 0x4d, false));
        assert!(!close_shortcut(&mut held, true, 0x4d, false));
        assert!(!close_shortcut(&mut held, false, 0x4d, true));
        assert!(!close_shortcut(&mut held, true, 0x4d, true));
        assert!(!close_shortcut(&mut held, true, 0x4d, false));
        assert!(!close_shortcut(&mut held, false, 0x4d, false));
        assert!(close_shortcut(&mut held, true, 0x4d, false));
    }

    /// Manual: `cargo test -p client --release --lib world_map_build_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "manual benchmark"]
    fn world_map_build_cost() {
        surface::fill_memory_for_bench(90); // 8100 chunks, about the limit
        let area = [2512_f32, 1308.];
        for scale in [0.25_f32, 1., 3., 6., 16.] {
            let view = [area[0] / scale, area[1] / scale];
            let step = cell_step(scale);
            let (origin, cells) = grid_bounds([0., 0.], view, step);
            let mut times = Vec::new();
            let mut rects = 0;
            for _ in 0..7 {
                let start = Instant::now();
                let grid = Grid::build(origin, cells, step);
                times.push(start.elapsed());
                rects = grid.rects.len();
            }
            times.sort();
            println!(
                "scale {scale}: step {step}, {} cells, {rects} rects, build median {:?}",
                cells[0] * cells[1],
                times[3]
            );
        }
    }

    /// Manual: the cost of one frame of the open map (grid already built).
    #[test]
    #[ignore = "manual benchmark"]
    fn world_map_frame_cost() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        surface::fill_memory_for_bench(90);
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.fonts().build_alpha8_texture();
        ctx.io_mut().display_size = [2560., 1440.];
        ctx.io_mut().backend_flags |= hudhook::imgui::BackendFlags::RENDERER_HAS_VTX_OFFSET;
        for scale in [1_f32, 3., 6.] {
            let mut map = WorldMap::new(&config::WorldMap::default(), Path::new(""));
            map.unavailable = None;
            map.follow = false;
            map.scale = scale;
            let mut times = Vec::new();
            for _ in 0..60 {
                let start = Instant::now();
                let ui = ctx.frame();
                map.paint(ui, Some([0., 0.]), 0.);
                std::hint::black_box(ctx.render().total_vtx_count);
                times.push(start.elapsed());
            }
            // The first frame builds the grid.
            let first = times[0];
            times.sort();
            println!(
                "scale {scale}: first frame {first:?}, frame median {:?}, {} rects",
                times[30],
                map.grid.as_ref().map_or(0, |g| g.rects.len())
            );
        }
    }

    #[test]
    fn cells_coarsen_as_the_map_zooms_out_and_never_go_below_the_minimum() {
        assert_eq!(cell_step(16.), 1);
        assert_eq!(cell_step(6.), 1);
        assert_eq!(cell_step(5.9), 2);
        assert_eq!(cell_step(1.), 8);
        assert_eq!(cell_step(0.25), 32);
        for scale in [0.25, 0.4, 1., 2.5, 7., 16.] {
            let step = cell_step(scale);
            assert!(step as f32 * scale >= MIN_CELL);
            assert!(step == 1 || (step / 2) as f32 * scale < MIN_CELL);
        }
    }

    #[test]
    fn the_grid_covers_the_view_with_a_margin_and_aligns_to_its_cells() {
        let (origin, cells) = grid_bounds([100.5, -40.], [200., 100.], 4);
        assert!(origin.iter().all(|o| o % 4 == 0));
        let grid = Grid {
            step: 4,
            origin,
            cells,
            rects: Vec::new(),
            built: Instant::now(),
            revision: 7,
        };
        assert!(grid.covers([100.5, -40.], [200., 100.], 4, 7));
        // Dragged within the margin: still covered; past it, or another step: not.
        assert!(grid.covers([140., -60.], [200., 100.], 4, 7));
        assert!(!grid.covers([200., -40.], [200., 100.], 4, 7));
        assert!(!grid.covers([100.5, -40.], [200., 100.], 8, 7));
        // Old: kept while the memory is unchanged, rebuilt once it changed.
        let old = Grid {
            built: Instant::now() - REFRESH,
            ..grid
        };
        assert!(old.covers([100.5, -40.], [200., 100.], 4, 7));
        assert!(!old.covers([100.5, -40.], [200., 100.], 4, 8));
        // Recent: kept even though the memory changed (rate limit).
        let recent = Grid {
            built: Instant::now(),
            ..old
        };
        assert!(recent.covers([100.5, -40.], [200., 100.], 4, 8));
    }

    #[test]
    fn scale_bars_use_round_lengths() {
        assert_eq!(scale_bar(1.), (100, 100.));
        assert_eq!(scale_bar(16.), (5, 80.));
        assert_eq!(scale_bar(0.25), (500, 125.));
    }
}
