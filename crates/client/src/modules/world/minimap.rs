//! Minimap: the terrain around the player seen from above, north up, with the
//! player's heading and (optionally) nearby entities. The terrain is read on
//! the game thread (`game::blocks::surface`); this only draws it.

use std::sync::Arc;

use hudhook::imgui::{sys, ImColor32, Ui};

use crate::game::{self, blocks, blocks::surface};
use crate::modules::entity_kind;
use crate::modules::hud::origin;
use crate::modules::map_marks::{self, SlimeCache};
use crate::modules::settings::{Detail, Value};
use crate::modules::waypoints::MapPoint;
use crate::modules::{key_or_warn, Module, Param};
use crate::{api, config, log};

const MIN_SIZE: f32 = 120.;
const MAX_SIZE: f32 = 400.;
/// Space between the panel's edge and the map.
const MARGIN: f32 = 6.;

/// A filled rectangle: top-left and bottom-right corners and packed colour.
pub(crate) type Fill = ([f32; 2], [f32; 2], u32);

/// The map's rectangles (runs of one colour along a row), relative to the
/// map's top-left corner, for one terrain snapshot and map size.
struct Cached {
    map: Arc<surface::Map>,
    side: f32,
    rects: Vec<Fill>,
}

/// `rects` moved by `shift` from `corner` and clipped to `corner..end`, into
/// `out` (cleared first). Rectangles left empty by the clip are dropped.
fn place(rects: &[Fill], corner: [f32; 2], end: [f32; 2], shift: [f32; 2], out: &mut Vec<Fill>) {
    out.clear();
    for &(a, b, color) in rects {
        let a = std::array::from_fn::<_, 2, _>(|k| (corner[k] + a[k] + shift[k]).max(corner[k]));
        let b = std::array::from_fn::<_, 2, _>(|k| (corner[k] + b[k] + shift[k]).min(end[k]));
        if a[0] < b[0] && a[1] < b[1] {
            out.push((a, b, color));
        }
    }
}

/// Draws filled rectangles into the background draw list, giving the same
/// vertices and indices as `add_rect(a, b, color).filled(true)` for each
/// (ImGui's AddRectFilled without rounding is `PrimReserve(6, 4)` followed by
/// `PrimRect`), with one reservation per chunk instead of one call per
/// rectangle. Indices are 16-bit, so a chunk stays well below 65536 vertices;
/// ImGui starts a new vertex offset when a chunk would pass the limit.
pub(crate) fn fill(rects: &[Fill]) {
    const CHUNK: usize = 2048;
    // AddRectFilled draws nothing for a fully transparent colour.
    const ALPHA: u32 = ImColor32::from_rgba(0, 0, 0, 0xFF).to_bits();
    // SAFETY: called while a frame is being built (inside `Ui::draw`), so
    // the background draw list exists; every reserved vertex and index is
    // written by exactly one PrimRect per rectangle of the chunk.
    unsafe {
        let list = sys::igGetBackgroundDrawList();
        for chunk in rects.chunks(CHUNK) {
            let shown = chunk.iter().filter(|r| r.2 & ALPHA != 0).count() as i32;
            sys::ImDrawList_PrimReserve(list, 6 * shown, 4 * shown);
            for &(a, b, color) in chunk.iter().filter(|r| r.2 & ALPHA != 0) {
                sys::ImDrawList_PrimRect(list, a.into(), b.into(), color);
            }
        }
    }
}

/// How a column is drawn: its colour lit by the slope towards the north, as
/// on vanilla maps (higher than the column north of it: lighter). Columns
/// remembered from an earlier visit are drawn darker.
pub(crate) fn shade(column: surface::Column, north: Option<surface::Column>) -> [f32; 4] {
    let light = match north.filter(|n| n.known) {
        Some(n) if column.height > n.height => 1.15,
        Some(n) if column.height < n.height => 0.78,
        _ => 1.,
    };
    let light = if column.remembered {
        light * 0.7
    } else {
        light
    };
    let [r, g, b] = column.color.map(|c| (c as f32 / 255. * light).min(1.));
    [r, g, b, 0.95]
}

/// The map's rectangles, relative to its top-left corner, `side` pixels wide.
fn rectangles(map: &surface::Map, side: f32) -> Vec<([f32; 2], [f32; 2], [f32; 4])> {
    let cells = map.side();
    let r = map.radius;
    merge(cells, cells, side / cells as f32, |x, z| {
        let (dx, dz) = (x as i32 - r, z as i32 - r);
        map.at(dx, dz)
            .filter(|c| c.known)
            .map(|c| shade(c, map.at(dx, dz - 1)))
    })
}

/// Rectangles covering a `cols` x `rows` grid of `cell`-pixel cells, row by
/// row from the top-left, where `color(x, z)` gives a cell's colour (None:
/// nothing drawn). Equal horizontal runs are merged, then runs with the same
/// span and colour are extended across adjacent rows.
pub(crate) fn merge(
    cols: usize,
    rows: usize,
    cell: f32,
    color: impl Fn(usize, usize) -> Option<[f32; 4]>,
) -> Vec<([f32; 2], [f32; 2], [f32; 4])> {
    let mut out: Vec<([f32; 2], [f32; 2], [f32; 4])> = Vec::new();
    // The previous row's runs, left to right: (start, end, colour, rectangle).
    // Runs of a row are disjoint and found in order, so a run continues the
    // one above it found by walking the previous row once (no hashing).
    let mut previous: Vec<(usize, usize, [u32; 4], usize)> = Vec::new();
    let mut current = Vec::new();
    for z in 0..rows {
        current.clear();
        let y = z as f32 * cell;
        let mut above = 0;
        let mut run: Option<(usize, [f32; 4])> = None;
        for x in 0..=cols {
            let here = (x < cols).then(|| color(x, z)).flatten();
            match (run, here) {
                (Some((_, a)), Some(b)) if a == b => continue,
                _ => {}
            }
            if let Some((start, a)) = run.take() {
                let bits = a.map(f32::to_bits);
                while above < previous.len() && previous[above].0 < start {
                    above += 1;
                }
                let index = match previous.get(above) {
                    Some(&(s, e, c, index)) if s == start && e == x && c == bits => {
                        out[index].1[1] = (z + 1) as f32 * cell;
                        index
                    }
                    _ => {
                        out.push((
                            [start as f32 * cell, y],
                            [x as f32 * cell, (z + 1) as f32 * cell],
                            a,
                        ));
                        out.len() - 1
                    }
                };
                current.push((start, x, bits, index));
            }
            run = here.map(|c| (x, c));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    out
}

pub struct Minimap {
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    radius: i32,
    size: f32,
    x: i32,
    y: i32,
    entities: bool,
    chunk_grid: bool,
    coordinates: bool,
    waypoints: bool,
    slime_chunks: bool,
    slime: SlimeCache,
    centered: bool,
    height_limit: f32,
    cached: Option<Cached>,
    /// Actor facts are subscribed (to colour entity marks by kind).
    actors: bool,
    /// The cached rectangles placed on screen this frame (reused buffer).
    placed: Vec<Fill>,
}

impl Minimap {
    pub fn new(cfg: &config::Minimap) -> Self {
        let radius = cfg.radius.clamp(surface::MIN_RADIUS, surface::MAX_RADIUS);
        surface::set_radius(radius);
        Self {
            key: key_or_warn("minimap", &cfg.key),
            enabled: false,
            unavailable: blocks::available().err(),
            radius,
            size: if cfg.size.is_finite() {
                cfg.size.clamp(MIN_SIZE, MAX_SIZE)
            } else {
                200.
            },
            x: cfg.x,
            y: cfg.y,
            entities: cfg.entities,
            chunk_grid: cfg.chunk_grid,
            coordinates: cfg.coordinates,
            waypoints: cfg.waypoints,
            slime_chunks: cfg.slime_chunks,
            slime: SlimeCache::default(),
            centered: cfg.centered,
            height_limit: if cfg.height_limit.is_finite() {
                cfg.height_limit.clamp(0., 128.)
            } else {
                0.
            },
            cached: None,
            actors: false,
            placed: Vec::new(),
        }
    }
}

impl Module for Minimap {
    fn name(&self) -> &str {
        "minimap"
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
            log!("minimapを利用できません: {e}");
            return;
        }
        if let Err(e) = if on { game::acquire() } else { game::release() } {
            log!("minimap: {e}");
            return;
        }
        if self.entities {
            if let Err(e) = game::use_entities(on) {
                let _ = if on { game::release() } else { game::acquire() };
                log!("minimap: {e}");
                return;
            }
        }
        self.use_actors(on && self.entities);
        surface::set_active(on);
        self.cached = None;
        self.enabled = on;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "radius",
            value: self.radius as f64,
            min: surface::MIN_RADIUS as f64,
            max: surface::MAX_RADIUS as f64,
        })
    }
    fn set_param(&mut self, value: f64) {
        if value.is_finite() {
            self.radius = (value.round() as i32).clamp(surface::MIN_RADIUS, surface::MAX_RADIUS);
            surface::set_radius(self.radius);
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::number(
                "size",
                "Size (pixels)",
                "Width and height of the map.",
                self.size as f64,
                MIN_SIZE as f64,
                MAX_SIZE as f64,
            ),
            Detail::boolean(
                "entities",
                "Show entities",
                "Mark loaded entities on the map.",
                self.entities,
            ),
            Detail::boolean(
                "centered",
                "自分を地図の中央に固定",
                "移動に合わせて地形を滑らかにずらします。北が上です。",
                self.centered,
            ),
            Detail::boolean(
                "chunk_grid",
                "チャンク境界を表示",
                "16ブロックごとの境界線を重ねます。",
                self.chunk_grid,
            ),
            Detail::boolean(
                "coordinates",
                "座標と縮尺を表示",
                "地図の下に現在のX・Zと横幅を表示します。",
                self.coordinates,
            ),
            Detail::boolean(
                "waypoints",
                "地点を表示",
                "ウェイポイント（水色）と死亡地点（赤紫）を表示します。範囲外の地点は縁に方向の矢印を出します。",
                self.waypoints,
            ),
            Detail::boolean(
                "slime_chunks",
                "スライムチャンクを表示",
                "オーバーワールドのスライムチャンクを緑で塗ります（統合版はどのワールドでも同じ位置）。",
                self.slime_chunks,
            ),
            Detail::number(
                "height_limit",
                "エンティティの高度差",
                "自分との高さの差がこの値以内の対象を表示。0で制限しません。",
                self.height_limit as f64,
                0.,
                128.,
            ),
            Detail::x_offset(self.x, "Positive: from left. Negative: from right."),
            Detail::y_offset(self.y, "Positive: from top. Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "size" => self.size = value.number()?.clamp(MIN_SIZE as f64, MAX_SIZE as f64) as f32,
            "x" => self.x = value.number()? as i32,
            "y" => self.y = value.number()? as i32,
            "centered" => self.centered = value.boolean()?,
            "chunk_grid" => self.chunk_grid = value.boolean()?,
            "coordinates" => self.coordinates = value.boolean()?,
            "waypoints" => self.waypoints = value.boolean()?,
            "slime_chunks" => self.slime_chunks = value.boolean()?,
            "height_limit" => self.height_limit = value.number()?.clamp(0., 128.) as f32,
            "entities" => {
                let on = value.boolean()?;
                if self.enabled && on != self.entities {
                    game::use_entities(on)?;
                    self.use_actors(on);
                }
                self.entities = on;
            }
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        let remembered = match surface::remembered_chunks() {
            0 => String::new(),
            n => format!(" / 記憶{n}チャンク"),
        };
        match surface::snapshot() {
            Some(_) => format!("半径{}{remembered}", self.radius),
            None => "（ワールド未接続）".into(),
        }
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.text_disabled(
            "一度読み込んだ地上の地形は、クライアントを解放するまで記憶し、暗く表示します。",
        );
        if ui.button("記憶した地形を消去") {
            surface::forget();
            log!("minimap: 記憶した地形を消去しました");
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if !crate::modules::hud_view() {
            return;
        }
        let (Some(map), Some(player)) = (
            surface::snapshot(),
            api::game().ok().and_then(|g| g.player().ok()),
        ) else {
            self.cached = None;
            return;
        };
        let boxes = self.entities.then(game::shared_boxes).flatten();
        let facts = self.entities.then(game::actors::actors).flatten();
        let points = if self.waypoints {
            crate::modules::waypoints::on_map()
        } else {
            Vec::new()
        };
        let slime = self.slime_chunks
            && matches!(game::world::realm(), Some(game::world::Realm::Overworld));
        self.paint(
            ui,
            &map,
            player.position.into(),
            player.rotation.yaw,
            &Marks {
                boxes: boxes.as_deref().map(Vec::as_slice),
                facts: facts.as_deref(),
                points: &points,
                slime,
            },
        );
    }
}

/// What is drawn over the terrain: entities (coloured by kind from their
/// facts), saved places and slime chunks.
struct Marks<'a> {
    boxes: Option<&'a [(u32, game::Aabb)]>,
    facts: Option<&'a std::collections::HashMap<u32, game::actors::Actor>>,
    points: &'a [MapPoint],
    slime: bool,
}

impl Minimap {
    /// Subscribes to actor facts (or ends the subscription) so entity marks
    /// are coloured by kind; without them marks are drawn in one colour.
    fn use_actors(&mut self, on: bool) {
        if on && !self.actors {
            match game::actors::use_actors(true) {
                Ok(()) => self.actors = true,
                Err(e) => log!("minimap: 種類での色分けはできません: {e}"),
            }
        } else if !on && std::mem::take(&mut self.actors) {
            let _ = game::actors::use_actors(false);
        }
    }

    /// Draws `map` with the player at `position` facing `yaw`, and `marks`.
    fn paint(
        &mut self,
        ui: &Ui,
        map: &Arc<surface::Map>,
        position: [f32; 3],
        yaw: f32,
        marks: &Marks,
    ) {
        let (boxes, facts) = (marks.boxes, marks.facts);
        if !position.iter().all(|v| v.is_finite() && v.abs() < 3e7) || !yaw.is_finite() {
            return;
        }
        if (position[0] - map.center[0] as f32).abs() > map.radius as f32 * 2.
            || (position[2] - map.center[1] as f32).abs() > map.radius as f32 * 2.
        {
            self.cached = None;
            return;
        }
        let screen = ui.io().display_size;
        let size = self
            .size
            .min((screen[0] - 10.).max(1.))
            .min((screen[1] - 10.).max(1.));
        crate::hud_editor::measure("minimap", [size, size]);
        let left = origin(self.x, screen[0], size);
        let top = origin(self.y, screen[1], size);
        let side = (size - 2. * MARGIN).max(1.);
        let corner = [left + MARGIN, top + MARGIN];
        if self
            .cached
            .as_ref()
            .is_none_or(|c| !Arc::ptr_eq(&c.map, map) || c.side != side)
        {
            self.cached = Some(Cached {
                rects: rectangles(map, side)
                    .into_iter()
                    .map(|(a, b, color)| (a, b, ImColor32::from(color).to_bits()))
                    .collect(),
                map: Arc::clone(map),
                side,
            });
        }
        let cached = self.cached.as_ref().unwrap();

        let draw = ui.get_background_draw_list();
        crate::theme::panel(&draw, [left, top], [left + size, top + size]);
        let cell = side / map.side() as f32;
        // World (x, z) to screen, relative to the centre column's middle.
        let middle = [
            corner[0] + (map.radius as f32 + 0.5) * cell,
            corner[1] + (map.radius as f32 + 0.5) * cell,
        ];
        let center = if self.centered {
            [position[0], position[2]]
        } else {
            [map.center[0] as f32 + 0.5, map.center[1] as f32 + 0.5]
        };
        let shift = [
            (map.center[0] as f32 + 0.5 - center[0]) * cell,
            (map.center[1] as f32 + 0.5 - center[1]) * cell,
        ];
        let to_screen = |x: f32, z: f32| map_point(middle, center, cell, [x, z]);
        let end = [corner[0] + side, corner[1] + side];
        draw.with_clip_rect_intersect(corner, end, || {
            place(&cached.rects, corner, end, shift, &mut self.placed);
            fill(&self.placed);
            if marks.slime {
                let reach = map.radius as f32 + 1.;
                map_marks::slime_chunks(
                    &draw,
                    &mut self.slime,
                    [center[0] - reach, center[1] - reach],
                    [center[0] + reach, center[1] + reach],
                    to_screen,
                );
            }
            if self.chunk_grid {
                for axis in 0..2 {
                    let start = (center[axis].floor() as i32 - map.radius - 1).div_euclid(16) * 16;
                    let finish = center[axis].ceil() as i32 + map.radius + 1;
                    for block in (start..=finish).step_by(16) {
                        let p = if axis == 0 {
                            to_screen(block as f32, center[1])
                        } else {
                            to_screen(center[0], block as f32)
                        };
                        if p[axis] < corner[axis] || p[axis] > end[axis] {
                            continue;
                        }
                        let (a, b) = if axis == 0 {
                            ([p[0], corner[1]], [p[0], end[1]])
                        } else {
                            ([corner[0], p[1]], [end[0], p[1]])
                        };
                        draw.add_line(a, b, [0., 0., 0., 0.7]).thickness(2.).build();
                        draw.add_line(a, b, [0.7, 0.85, 1., 0.5]).build();
                    }
                }
            }
            for (id, a) in boxes.into_iter().flatten() {
                if !a.iter().all(|v| v.is_finite()) || a[4] - a[1] < 0.3 {
                    continue;
                }
                let height = (a[1] + a[4]) * 0.5 - position[1];
                if self.height_limit > 0. && height.abs() > self.height_limit {
                    continue;
                }
                let color = entity_kind::mark(entity_kind::of(facts, *id));
                let p = to_screen((a[0] + a[3]) * 0.5, (a[2] + a[5]) * 0.5);
                if !(corner[0]..=end[0]).contains(&p[0]) || !(corner[1]..=end[1]).contains(&p[1]) {
                    continue;
                }
                draw.add_circle(p, 3.5, [0., 0., 0., 0.9])
                    .filled(true)
                    .build();
                draw.add_circle(p, 2.5, color).filled(true).build();
            }
            // Saved places: on the map, or an arrow at the edge towards them.
            let me = to_screen(position[0], position[2]);
            for point in marks.points {
                let p = to_screen(point.x as f32 + 0.5, point.z as f32 + 0.5);
                match map_marks::edge_point(me, p, corner, end, 6.) {
                    None => map_marks::place(&draw, p, 4., point.death),
                    Some((at, unit)) => map_marks::arrow(&draw, at, unit, point.death),
                }
            }
            // The player: an arrow along the heading (yaw 0 faces +Z, south,
            // which is down on a north-up map).
            let p = to_screen(position[0], position[2]);
            let (sin, cos) = yaw.to_radians().sin_cos();
            let f = [-sin, cos];
            let s = [-f[1], f[0]];
            let at = |a: f32, b: f32| [p[0] + f[0] * a + s[0] * b, p[1] + f[1] * a + s[1] * b];
            draw.add_triangle(at(7., 0.), at(-4., 4.5), at(-4., -4.5), [1., 1., 1., 1.])
                .filled(true)
                .build();
            draw.add_triangle(at(7., 0.), at(-4., 4.5), at(-4., -4.5), [0., 0., 0., 0.8])
                .thickness(1.)
                .build();
        });
        let text = ui.calc_text_size("N");
        for (name, p) in [
            ("N", [left + size * 0.5 - text[0] * 0.5, top + 2.]),
            (
                "S",
                [
                    left + size * 0.5 - text[0] * 0.5,
                    top + size - text[1] - 2. - if self.coordinates { text[1] + 5. } else { 0. },
                ],
            ),
            ("W", [left + 3., top + size * 0.5 - text[1] * 0.5]),
            (
                "E",
                [left + size - text[0] - 3., top + size * 0.5 - text[1] * 0.5],
            ),
        ] {
            draw.add_text([p[0] + 1., p[1] + 1.], [0., 0., 0., 1.], name);
            draw.add_text(p, crate::theme::TEXT, name);
        }
        if self.coordinates {
            let footer = format!(
                "{} {}  ·  {}m",
                position[0].floor() as i32,
                position[2].floor() as i32,
                map.side()
            );
            let footer = crate::text::fit_width(&footer, side - 8., |s| ui.calc_text_size(s)[0]);
            let at = [corner[0] + 4., end[1] - text[1] - 3.];
            draw.add_rect(
                [corner[0], at[1] - 2.],
                [end[0], end[1]],
                [0.025, 0.04, 0.065, 0.94],
            )
            .filled(true)
            .build();
            draw.add_text(at, crate::theme::TEXT, footer);
        }
    }
}

fn map_point(middle: [f32; 2], center: [f32; 2], scale: f32, point: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|k| middle[k] + (point[k] - center[k]) * scale)
}

/// A made-up island (sea, beach, grass, a forest, a river, a snowy hill)
/// drawn as the minimap would show it, for the headless UI preview.
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let radius = 32;
    let mut columns = Vec::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let (x, z) = (dx as f32, dz as f32);
            let d = (x * x + z * z * 1.3).sqrt();
            let hill = ((x - 10.).powi(2) + (z + 12.).powi(2)).sqrt();
            let height =
                (72. - d * 0.35 + (12. - hill).max(0.) * 1.2 + (x * 0.3).sin() * 1.5) as i16;
            let river = ((x * 0.15).sin() * 6. - z).abs() < 1.6 && d < 26.;
            let color = if d > 28. || river {
                [52, 92, 180]
            } else if d > 25. {
                [214, 200, 150]
            } else if hill < 5. {
                [240, 242, 245]
            } else if hill < 9. {
                [125, 125, 125]
            } else if x < -6. && z > 0. && (dx * 7 + dz * 3) % 5 != 0 {
                [50, 105, 40]
            } else {
                [92, 140, 60]
            };
            let unread = d > 30. && dx > 20;
            columns.push(surface::Column {
                height: if color == [52, 92, 180] { 62 } else { height },
                color,
                known: !unread,
                // The north-west corner as if remembered from an earlier visit.
                remembered: !unread && dx < -14 && dz < -14,
            });
        }
    }
    let map = Arc::new(surface::Map {
        center: [0, 0],
        radius,
        columns,
    });
    let mut minimap = Minimap::new(&config::Minimap {
        x: -20,
        y: 20,
        chunk_grid: true,
        ..Default::default()
    });
    let mobs = [
        (1, [4., 70., -3., 4.6, 71.8, -2.4]),
        (2, [-12., 75., 9., -11.4, 76.8, 9.6]),
        (3, [8., 60., 14., 8.6, 61.8, 14.6]),
        (4, [-6., 68., -10., -5.4, 69.8, -9.4]),
        (5, [14., 66., 4., 14.6, 67.8, 4.6]),
    ];
    // A zombie, a cow, a player, a wolf and an item: one mark of each colour.
    let facts = [
        (1, "zombie"),
        (2, "cow"),
        (3, "player"),
        (4, "wolf"),
        (5, "item"),
    ]
    .into_iter()
    .map(|(id, name)| {
        let actor = game::actors::Actor {
            name: name.into(),
            display_name: None,
            actor_type: 0,
            health: None,
            bounds: None,
            local: false,
        };
        (id, actor)
    })
    .collect();
    // A waypoint on the map, one beyond its north-east edge, a death point.
    let points = [
        ("home", -8, 6, false),
        ("mine", 90, -70, false),
        ("", 12, -14, true),
    ]
    .map(|(name, x, z, death)| MapPoint {
        name: name.into(),
        x,
        z,
        death,
    });
    minimap.paint(
        ui,
        &map,
        [0.4, 71.6, 0.3],
        135.,
        &Marks {
            boxes: Some(&mobs),
            facts: Some(&facts),
            points: &points,
            slime: true,
        },
    );
}

/// Manual benchmark (see `gui::preview::minimap_paint_cost`): the time to
/// paint one frame of a map whose rectangles are already cached, for an
/// uneven map of each radius (few equal neighbours, as on rough terrain).
#[cfg(test)]
pub(crate) type TimedFrame<'a> = dyn FnMut(&mut dyn FnMut(&Ui)) -> std::time::Duration + 'a;
#[cfg(test)]
pub(crate) fn paint_cost(frame: &mut TimedFrame) {
    for radius in [32, 64] {
        let side = (2 * radius + 1) as usize;
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        const PALETTE: [[u8; 3]; 6] = [
            [92, 140, 60],
            [50, 105, 40],
            [125, 125, 125],
            [214, 200, 150],
            [52, 92, 180],
            [125, 90, 60],
        ];
        let columns: Vec<_> = (0..side * side)
            .map(|_| surface::Column {
                height: 64 + (next() % 3) as i16,
                color: PALETTE[(next() % PALETTE.len() as u32) as usize],
                known: true,
                remembered: false,
            })
            .collect();
        let map = Arc::new(surface::Map {
            center: [0, 0],
            radius,
            columns,
        });
        let mut minimap = Minimap::new(&config::Minimap {
            radius,
            size: 400.,
            ..Default::default()
        });
        let median = |frame: &mut TimedFrame, draw: &mut dyn FnMut(&Ui)| {
            let mut times: Vec<_> = (0..200).map(|_| frame(draw)).collect();
            times.sort();
            (times[times.len() / 2], times[times.len() * 9 / 10])
        };
        let (paint, p90) = median(frame, &mut |ui| {
            minimap.paint(
                ui,
                &map,
                [0.4, 65.6, 0.3],
                135.,
                &Marks {
                    boxes: None,
                    facts: None,
                    points: &[],
                    slime: false,
                },
            )
        });
        let (empty, _) = median(frame, &mut |_| {});
        let rects = minimap.cached.as_ref().map_or(0, |c| c.rects.len());
        println!(
            "minimap radius {radius} ({rects} rectangles): frame median {paint:?}, p90 {p90:?}; empty frame {empty:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surface::{Column, Map};

    fn column(height: i16, color: [u8; 3]) -> Column {
        Column {
            height,
            color,
            known: true,
            remembered: false,
        }
    }

    /// The implementation before the row walk (hashing the previous row's
    /// runs), kept to check the walk gives exactly the same rectangles.
    fn merge_reference(
        cols: usize,
        rows: usize,
        cell: f32,
        color: impl Fn(usize, usize) -> Option<[f32; 4]>,
    ) -> Vec<([f32; 2], [f32; 2], [f32; 4])> {
        let mut out: Vec<([f32; 2], [f32; 2], [f32; 4])> = Vec::new();
        let mut previous: std::collections::HashMap<(usize, usize, [u32; 4]), usize> =
            std::collections::HashMap::new();
        let mut current = std::collections::HashMap::new();
        for z in 0..rows {
            current.clear();
            let y = z as f32 * cell;
            let mut run: Option<(usize, [f32; 4])> = None;
            for x in 0..=cols {
                let here = (x < cols).then(|| color(x, z)).flatten();
                match (run, here) {
                    (Some((_, a)), Some(b)) if a == b => continue,
                    _ => {}
                }
                if let Some((start, a)) = run.take() {
                    let key = (start, x, a.map(f32::to_bits));
                    let index = if let Some(&index) = previous.get(&key) {
                        out[index].1[1] = (z + 1) as f32 * cell;
                        index
                    } else {
                        let index = out.len();
                        out.push((
                            [start as f32 * cell, y],
                            [x as f32 * cell, (z + 1) as f32 * cell],
                            a,
                        ));
                        index
                    };
                    current.insert(key, index);
                }
                run = here.map(|c| (x, c));
            }
            std::mem::swap(&mut previous, &mut current);
        }
        out
    }

    #[test]
    fn walking_the_previous_row_gives_the_rectangles_of_hashing_it() {
        let mut seed = 0x1234_5678_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for (cols, rows, colours) in [
            (1, 1, 1),
            (7, 5, 2),
            (40, 30, 3),
            (129, 129, 6),
            (300, 2, 1),
        ] {
            for _ in 0..20 {
                let grid: Vec<Option<[f32; 4]>> = (0..cols * rows)
                    .map(|_| {
                        let c = next() % (colours + 1);
                        (c > 0).then(|| [c as f32 / 8., 0.5, 0.25, 1.])
                    })
                    .collect();
                let color = |x: usize, z: usize| grid[z * cols + x];
                assert_eq!(
                    merge(cols, rows, 3., color),
                    merge_reference(cols, rows, 3., color),
                    "{cols}x{rows}"
                );
            }
        }
    }

    #[test]
    fn rows_merge_equal_colours_and_skip_unknown_columns() {
        let grass = [100, 150, 50];
        let water = [50, 90, 180];
        // 3 x 3, all at one height: rows are lit evenly.
        let map = Map {
            center: [0, 0],
            radius: 1,
            columns: vec![
                column(64, grass),
                column(64, grass),
                column(64, water),
                Column::default(),
                column(64, grass),
                column(64, grass),
                column(64, water),
                column(64, water),
                column(64, water),
            ],
        };
        let rects = rectangles(&map, 30.);
        let lit = |c: [u8; 3]| shade(column(64, c), Some(column(64, c)));
        assert_eq!(
            rects,
            vec![
                ([0., 0.], [20., 10.], lit(grass)),
                ([20., 0.], [30., 10.], lit(water)),
                // The unknown column is left out.
                ([10., 10.], [30., 20.], lit(grass)),
                ([0., 20.], [30., 30.], lit(water)),
            ]
        );
    }

    #[test]
    fn slopes_facing_north_are_lit_like_vanilla_maps() {
        let c = [100, 100, 100];
        let flat = shade(column(64, c), Some(column(64, c)));
        let rising = shade(column(65, c), Some(column(64, c)));
        let falling = shade(column(63, c), Some(column(64, c)));
        assert!(rising[0] > flat[0] && flat[0] > falling[0]);
        // Without a known northern neighbour the colour is unchanged.
        assert_eq!(shade(column(65, c), None), flat);
        assert_eq!(shade(column(65, c), Some(Column::default())), flat);
    }

    #[test]
    fn flat_terrain_merges_vertically_and_keeps_every_pixel_color() {
        let mut map = Map {
            center: [0, 0],
            radius: 32,
            columns: vec![column(64, [90, 140, 50]); 65 * 65],
        };
        assert_eq!(rectangles(&map, 195.).len(), 1);
        map.radius = 2;
        map.columns = (0..25)
            .map(|i| {
                if i % 7 == 0 {
                    Column::default()
                } else {
                    column(64 + (i % 3) as i16, [40 + (i % 2) as u8 * 100, 100, 90])
                }
            })
            .collect();
        let rects = rectangles(&map, 50.);
        for dz in -2..=2 {
            for dx in -2..=2 {
                let x = (dx + 2) as f32 * 10. + 5.;
                let y = (dz + 2) as f32 * 10. + 5.;
                let actual: Vec<_> = rects
                    .iter()
                    .filter(|(a, b, _)| x >= a[0] && x < b[0] && y >= a[1] && y < b[1])
                    .map(|(_, _, c)| *c)
                    .collect();
                let expected = map
                    .at(dx, dz)
                    .filter(|c| c.known)
                    .map(|c| shade(c, map.at(dx, dz - 1)));
                assert_eq!(actual, expected.into_iter().collect::<Vec<_>>());
            }
        }
    }
    /// Every triangle drawn, as (position, uv, colour, clip rectangle) per
    /// corner, with indices resolved through each command's vertex offset.
    /// Position, uv, colour, clip rectangle.
    type Corner = ([f32; 2], [f32; 2], [u8; 4], [f32; 4]);
    fn triangles(data: &hudhook::imgui::DrawData) -> Vec<Corner> {
        use hudhook::imgui::DrawCmd;
        let mut out = Vec::new();
        for list in data.draw_lists() {
            let (vtx, idx) = (list.vtx_buffer(), list.idx_buffer());
            for command in list.commands() {
                if let DrawCmd::Elements { count, cmd_params } = command {
                    for &i in &idx[cmd_params.idx_offset..cmd_params.idx_offset + count] {
                        let v = vtx[cmd_params.vtx_offset + i as usize];
                        out.push((v.pos, v.uv, v.col, cmd_params.clip_rect));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn batched_fill_draws_the_triangles_of_one_filled_rect_per_call() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.fonts().build_alpha8_texture();
        ctx.io_mut().display_size = [1920., 1080.];
        // As every hudhook renderer sets it: lists may pass 65536 vertices.
        ctx.io_mut().backend_flags |= hudhook::imgui::BackendFlags::RENDERER_HAS_VTX_OFFSET;
        let mut seed = 0x9e37_79b9_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        // 30000 rectangles (about 80000 visible vertices) pass the 16-bit index limit;
        // some colours are fully transparent; a clip rectangle is active.
        for count in [0, 1, 3, 2048, 2049, 30_000] {
            let rects: Vec<Fill> = (0..count)
                .map(|_| {
                    let a = [(next() % 1800) as f32 * 0.5, (next() % 1000) as f32 * 0.5];
                    let size = [1. + (next() % 40) as f32 * 0.25, 1. + (next() % 9) as f32];
                    let alpha = [0, 0x40, 0xF2][(next() % 3) as usize];
                    (
                        a,
                        [a[0] + size[0], a[1] + size[1]],
                        next() & 0x00FF_FFFF | alpha << 24,
                    )
                })
                .collect();
            let mut frame = |batched: bool| {
                let ui = ctx.frame();
                let draw = ui.get_background_draw_list();
                draw.add_rect([5., 5.], [9., 9.], [1., 1., 1., 1.]).build();
                draw.with_clip_rect_intersect([10., 10.], [700., 400.], || {
                    if batched {
                        fill(&rects);
                    } else {
                        for &(a, b, color) in &rects {
                            draw.add_rect(a, b, ImColor32::from_bits(color))
                                .filled(true)
                                .build();
                        }
                    }
                });
                draw.add_text([20., 20.], [1., 1., 1., 1.], "N");
                drop(draw);
                triangles(ctx.render())
            };
            let expected = frame(false);
            let got = frame(true);
            assert_eq!(got.len(), expected.len(), "{count} rectangles");
            assert!(got == expected, "{count} rectangles: triangles differ");
        }
    }

    #[test]
    fn map_projection_is_continuous_across_negative_block_and_chunk_boundaries() {
        for center in [[-16.01, -0.01], [-16., 0.], [-15.99, 0.01]] {
            assert_eq!(map_point([100., 100.], center, 3., center), [100., 100.]);
            let north = map_point([100., 100.], center, 3., [center[0], center[1] - 1.]);
            assert!((north[1] - 97.).abs() < 1e-4);
        }
        let a = map_point([100., 100.], [-16.01, 0.], 3., [-16., 0.]);
        let b = map_point([100., 100.], [-15.99, 0.], 3., [-16., 0.]);
        assert!((a[0] - b[0] - 0.06).abs() < 1e-4);
    }
}
