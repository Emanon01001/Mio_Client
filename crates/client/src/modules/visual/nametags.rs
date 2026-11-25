//! Display name, current health and distance above each live entity's hitbox.
//! Falls back to the entity kind if there is no current display name.
use crate::modules::{
    game::{self, actors},
    key_or_warn,
    settings::{Detail, Value},
    view::{dot, project},
    Module, Param,
};
use crate::{config, log};
use hudhook::imgui::Ui;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Distance {
    bytes: [u8; 24],
    len: usize,
}
impl std::fmt::Write for Distance {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let end = self.len.checked_add(text.len()).ok_or(std::fmt::Error)?;
        let out = self.bytes.get_mut(self.len..end).ok_or(std::fmt::Error)?;
        out.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}
impl Distance {
    fn new(value: f32) -> Self {
        let mut out = Self {
            bytes: [0; 24],
            len: 0,
        };
        // Visible labels are bounded by 256 blocks. Keep exactly Rust's .1
        // rounding, including ties; arithmetic quantization is not equivalent.
        write!(&mut out, "{value:.1}").expect("bounded label distance");
        out
    }
    fn text(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap()
    }
}

#[derive(Default)]
struct Label {
    name: Arc<str>,
    health: Option<(u32, u32)>,
    distance: Option<Distance>,
    font: (usize, u32),
    text: String,
    size: [f32; 2],
}
impl Label {
    fn update(
        &mut self,
        actor: &actors::Actor,
        distance: f32,
        flags: [bool; 3],
        font: (usize, u32),
        measure: impl FnOnce(&str) -> [f32; 2],
    ) -> bool {
        let [hp, show_distance, names] = flags;
        let name = if names {
            actor.display_name.as_ref().unwrap_or(&actor.name)
        } else {
            &actor.name
        };
        let health = actor
            .health
            .filter(|_| hp)
            .map(|(hp, max)| (hp.to_bits(), max.to_bits()));
        let distance = show_distance.then(|| Distance::new(distance));
        if self.font == font
            && self.health == health
            && self.distance == distance
            && (Arc::ptr_eq(&self.name, name) || self.name.as_ref() == name.as_ref())
        {
            return false;
        }
        self.name = Arc::clone(name);
        self.health = health;
        self.distance = distance;
        self.font = font;
        self.text.clear();
        self.text.push_str(name);
        if let Some((hp, max)) = health {
            write!(
                &mut self.text,
                "  HP {:.1}/{:.1}",
                f32::from_bits(hp),
                f32::from_bits(max)
            )
            .unwrap();
        }
        if let Some(distance) = distance {
            self.text.push_str("  ");
            self.text.push_str(distance.text());
            self.text.push_str(" m");
        }
        self.size = measure(&self.text);
        true
    }
}

pub struct NameTags {
    key: u16,
    enabled: bool,
    range: f64,
    players: bool,
    mobs: bool,
    others: bool,
    health: bool,
    names: bool,
    distance: bool,
    max_labels: i32,
    drawn: usize,
    visible: Vec<(f32, [f32; 2], u32)>,
    labels: HashMap<u32, Label>,
    snapshot_at: Option<u64>,
}
fn range(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(4., 256.)
    } else {
        48.
    }
}
impl NameTags {
    pub fn new(c: &config::NameTags) -> Self {
        Self {
            key: key_or_warn("nametags", &c.key),
            enabled: false,
            range: range(c.range),
            players: c.players,
            mobs: c.mobs,
            others: c.others,
            health: c.health,
            names: c.names,
            distance: c.distance,
            max_labels: c.max_labels.clamp(1, 128),
            drawn: 0,
            visible: Vec::new(),
            labels: HashMap::new(),
            snapshot_at: None,
        }
    }
}
fn selected(a: &actors::Actor, players: bool, mobs: bool, others: bool) -> bool {
    !a.local
        && if a.name.as_ref() == "player" {
            players
        } else if a.health.is_some() {
            mobs
        } else {
            others
        }
}
#[cfg(test)]
fn label(a: &actors::Actor, distance: f32, hp: bool, show_distance: bool, names: bool) -> String {
    let mut s = if names {
        a.display_name.as_deref().unwrap_or(&a.name)
    } else {
        &a.name
    }
    .to_string();
    if hp {
        if let Some((value, max)) = a.health {
            s.push_str(&format!("  HP {value:.1}/{max:.1}"));
        }
    }
    if show_distance {
        s.push_str(&format!("  {distance:.1} m"));
    }
    s
}
impl Module for NameTags {
    fn name(&self) -> &str {
        "nametags"
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
        // Acquire the actor snapshot first, rolling it back if the camera fails.
        if let Err(e) = actors::use_actors(on) {
            log!("nametags: {e}");
            return;
        }
        if let Err(e) = game::use_camera(on) {
            let _ = actors::use_actors(!on);
            log!("nametags: {e}");
            return;
        }
        self.enabled = on;
        self.drawn = 0;
        self.visible.clear();
        self.labels.clear();
        self.snapshot_at = None;
    }
    fn param(&self) -> Option<Param> {
        Some(Param {
            label: "range",
            value: self.range,
            min: 4.,
            max: 256.,
        })
    }
    fn set_param(&mut self, v: f64) {
        self.range = range(v);
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "names",
                "Display names",
                "Show the current player/custom name; fall back to entity kind when empty.",
                self.names,
            ),
            Detail::boolean(
                "players",
                "Players",
                "Show other player entities.",
                self.players,
            ),
            Detail::boolean(
                "mobs",
                "Mobs",
                "Show living entities with a health attribute.",
                self.mobs,
            ),
            Detail::boolean(
                "others",
                "Other entities",
                "Include dropped items and other entities without health.",
                self.others,
            ),
            Detail::boolean(
                "health",
                "Health",
                "Show current/maximum health when available.",
                self.health,
            ),
            Detail::boolean(
                "distance",
                "Distance",
                "Show distance from the camera in blocks.",
                self.distance,
            ),
            Detail::integer(
                "max_labels",
                "Maximum labels",
                "Keep the closest visible labels when there are many entities.",
                self.max_labels,
                1,
                128,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "names" => self.names = v.boolean()?,
            "players" => self.players = v.boolean()?,
            "mobs" => self.mobs = v.boolean()?,
            "others" => self.others = v.boolean()?,
            "health" => self.health = v.boolean()?,
            "distance" => self.distance = v.boolean()?,
            "max_labels" => self.max_labels = (v.number()? as i32).clamp(1, 128),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        format!("{}件の名前", self.drawn)
    }
    fn draw(&mut self, ui: &Ui) {
        self.drawn = 0;
        let (Some(camera), Some((at, actors))) = (game::camera(), actors::reading()) else {
            self.labels.clear();
            self.visible.clear();
            self.snapshot_at = None;
            return;
        };
        self.paint(ui, &camera, &actors, at);
    }
}
impl NameTags {
    fn paint(
        &mut self,
        ui: &Ui,
        camera: &game::Camera,
        actors: &HashMap<u32, actors::Actor>,
        at: u64,
    ) {
        if self.snapshot_at != Some(at) {
            self.labels.retain(|id, _| actors.contains_key(id));
            self.snapshot_at = Some(at);
        }
        let size = ui.io().display_size;
        self.visible.clear();
        for (&id, a) in actors {
            if !selected(a, self.players, self.mobs, self.others) {
                continue;
            }
            let Some(b) = a.bounds else {
                continue;
            };
            let point = [(b[0] + b[3]) * 0.5, b[4] + 0.25, (b[2] + b[5]) * 0.5];
            let rel = std::array::from_fn(|k| point[k] - camera.origin[k]);
            let dist2 = dot(rel, rel);
            if dist2 > (self.range * self.range) as f32 {
                continue;
            }
            let Some(pos) = project(camera, size, point) else {
                continue;
            };
            if !(0. ..=size[0]).contains(&pos[0]) || !(0. ..=size[1]).contains(&pos[1]) {
                continue;
            }
            self.visible.push((dist2, pos, id));
        }
        let compare = |a: &(f32, [f32; 2], u32), b: &(f32, [f32; 2], u32)| {
            a.0.total_cmp(&b.0).then(a.2.cmp(&b.2))
        };
        let limit = self.max_labels as usize;
        if self.visible.len() > limit {
            self.visible.select_nth_unstable_by(limit, compare);
            self.visible.truncate(limit);
        }
        self.visible.sort_unstable_by(compare);
        let font = (
            ui.current_font() as *const _ as usize,
            ui.current_font_size().to_bits(),
        );
        // Draw the farthest first so nearby labels remain legible on overlap.
        let d = ui.get_background_draw_list();
        for (dist2, pos, id) in self.visible.iter().rev() {
            let a = &actors[id];
            let cached = self.labels.entry(*id).or_default();
            cached.update(
                a,
                dist2.sqrt(),
                [self.health, self.distance, self.names],
                font,
                |s| ui.calc_text_size(s),
            );
            let sz = cached.size;
            let x = (pos[0] - sz[0] * 0.5).clamp(0., (size[0] - sz[0]).max(0.));
            let y = (pos[1] - sz[1]).max(0.);
            d.add_rect(
                [x - 4., y - 2.],
                [x + sz[0] + 4., y + sz[1] + 2.],
                [0., 0., 0., 0.65],
            )
            .filled(true)
            .rounding(3.)
            .build();
            d.add_text(
                [x, y],
                if a.name.as_ref() == "player" {
                    [0.5, 0.85, 1., 1.]
                } else {
                    [1., 1., 1., 1.]
                },
                &cached.text,
            );
        }
        self.drawn = self.visible.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(count: usize) -> HashMap<u32, actors::Actor> {
        (0..count)
            .map(|i| {
                let x = (i % 23) as f32 / 23. * 4. - 2.;
                let z = -8. - i as f32 * 0.017;
                (
                    0x40000 | i as u32,
                    actors::Actor {
                        name: "player".into(),
                        display_name: Some(format!("[MVP+] Player_{i:04}").into()),
                        actor_type: 0x13f,
                        health: Some((20., 20.)),
                        bounds: Some([x - 0.3, 0., z - 0.3, x + 0.3, 1.8, z + 0.3]),
                        local: false,
                    },
                )
            })
            .collect()
    }
    fn camera(x: f32) -> game::Camera {
        game::Camera {
            origin: [x, 0., 0.],
            right: [1., 0., 0.],
            up: [0., 1., 0.],
            back: [0., 0., 1.],
            scale: [1., 1.],
        }
    }
    fn context() -> hudhook::imgui::Context {
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.fonts().build_alpha8_texture();
        ctx.io_mut().display_size = [1280., 720.];
        ctx.io_mut().delta_time = 1. / 60.;
        ctx
    }
    #[test]
    fn cached_labels_match_formatting_rounding_flags_and_font_changes() {
        let mut actor = fixture(1).remove(&0x40000).unwrap();
        actor.display_name = Some("[VIP] 日本語 Player".into());
        let mut cached = Label::default();
        let calls = std::cell::Cell::new(0);
        let measure = |s: &str| {
            calls.set(calls.get() + 1);
            [s.len() as f32, 16.]
        };
        let font = (1, 16f32.to_bits());
        for flags in [
            [true, true, true],
            [false, true, true],
            [true, false, true],
            [true, true, false],
        ] {
            for distance in [0.25, 0.251, 1.05, 1.049, 10.05, 255.95] {
                cached.update(&actor, distance, flags, font, measure);
                assert_eq!(
                    cached.text,
                    label(&actor, distance, flags[0], flags[1], flags[2])
                );
                calls.set(0);
                assert!(!cached.update(&actor, distance, flags, font, measure));
                assert_eq!(calls.get(), 0);
            }
        }
        actor.health = Some((3.25, 20.));
        assert!(cached.update(&actor, 3., [true, true, true], font, measure));
        assert_eq!(cached.text, label(&actor, 3., true, true, true));
        actor.display_name = Some("Renamed".into());
        assert!(cached.update(&actor, 3., [true, true, true], font, measure));
        assert!(cached.text.starts_with("Renamed"));
        assert!(cached.update(
            &actor,
            3.,
            [true, true, true],
            (2, 20f32.to_bits()),
            measure
        ));
    }

    #[test]
    fn retained_cache_matches_uncached_vertices_and_keeps_only_closest_labels() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ctx = context();
        let mut actors = fixture(512);
        let mut cached = NameTags::new(&config::NameTags::default());
        cached.max_labels = 22;
        let mut cold = NameTags::new(&config::NameTags::default());
        cold.max_labels = 22;
        for i in 0..6 {
            let camera = camera(i as f32 * 0.003);
            if i == 3 {
                actors.remove(&0x40000);
            }
            if i == 4 {
                actors.get_mut(&0x40001).unwrap().display_name = Some("Renamed".into());
            }
            cached.paint(ctx.frame(), &camera, &actors, i * 100);
            let a: Vec<_> = ctx
                .render()
                .draw_lists()
                .flat_map(|d| d.vtx_buffer().iter().map(|v| (v.pos, v.uv, v.col)))
                .collect();
            cold.labels.clear();
            cold.paint(ctx.frame(), &camera, &actors, i * 100);
            let b: Vec<_> = ctx
                .render()
                .draw_lists()
                .flat_map(|d| d.vtx_buffer().iter().map(|v| (v.pos, v.uv, v.col)))
                .collect();
            assert!(!a.is_empty());
            assert_eq!(a, b);
            assert_eq!(cached.drawn, 22);
            let mut expected: Vec<_> = actors
                .iter()
                .map(|(&id, a)| {
                    let b = a.bounds.unwrap();
                    let rel = [
                        (b[0] + b[3]) * 0.5 - camera.origin[0],
                        b[4] + 0.25,
                        (b[2] + b[5]) * 0.5,
                    ];
                    (dot(rel, rel), id)
                })
                .collect();
            expected.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            assert_eq!(
                cached.visible.iter().map(|v| v.2).collect::<Vec<_>>(),
                expected[..22].iter().map(|v| v.1).collect::<Vec<_>>()
            );
            assert!(cached.labels.keys().all(|id| actors.contains_key(id)));
        }
    }

    #[test]
    #[ignore = "manual: cargo test -p client --release --lib name_tags_paint_cost -- --ignored --nocapture"]
    fn name_tags_paint_cost() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ctx = context();
        for count in [75, 512] {
            let actors = fixture(count);
            for mode in ["uncached", "cached", "moving"] {
                let mut tags = NameTags::new(&config::NameTags::default());
                tags.max_labels = 22;
                let mut samples = Vec::new();
                for i in 0..900 {
                    let camera = camera(if mode == "moving" {
                        (i % 30) as f32 * 0.01
                    } else {
                        0.
                    });
                    let start = std::time::Instant::now();
                    if mode == "uncached" {
                        tags.labels.clear();
                    }
                    tags.paint(ctx.frame(), &camera, &actors, i / 10);
                    std::hint::black_box(ctx.render().total_vtx_count);
                    if i >= 100 {
                        samples.push(start.elapsed());
                    }
                }
                samples.sort();
                println!(
                    "NameTags {count} entities / 22 labels / {mode}: median {:?}, p90 {:?}",
                    samples[400], samples[720]
                );
            }
        }
    }
    #[test]
    fn labels_filter_the_local_player_and_do_not_invent_health() {
        let mut a = actors::Actor {
            name: "cow".into(),
            display_name: None,
            actor_type: 0,
            health: Some((7.5, 10.)),
            bounds: None,
            local: false,
        };
        assert!(selected(&a, false, true, false));
        assert_eq!(label(&a, 3., true, true, true), "cow  HP 7.5/10.0  3.0 m");
        a.health = None;
        assert!(!selected(&a, true, true, false));
        assert!(selected(&a, false, false, true));
        assert_eq!(label(&a, 3., true, false, true), "cow");
        a.display_name = Some("Bessie".into());
        assert_eq!(label(&a, 3., false, false, true), "Bessie");
        assert_eq!(label(&a, 3., false, false, false), "cow");
        a.name = "player".into();
        assert!(!selected(&a, false, true, true));
        a.local = true;
        assert!(!selected(&a, true, true, true));
    }
}
