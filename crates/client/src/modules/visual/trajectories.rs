//! Trajectories: while a bow, crossbow, trident or throwable is held, draws
//! the path its projectile would fly and outlines the block or entity it
//! would hit. A bow being drawn shows its current power; otherwise full power.
//! Drawing only: nothing is shot, aimed or changed.

use hudhook::imgui::Ui;

use crate::game::{self, blocks, blocks::flight, items};
use crate::modules::settings::{Detail, Value};
use crate::modules::view::{box_lines, segment, BoxPainter};
use crate::modules::{key_or_warn, Module, Tick};
use crate::{config, input, log};
use std::sync::Arc;

const VK_RBUTTON: u16 = 0x02;
/// How often the held item is checked (ms).
const ITEM_MS: u64 = 50;

const PATH: [f32; 4] = [0.40, 0.78, 0.98, 0.9];
const BLOCK: [f32; 4] = [0.98, 0.75, 0.41, 1.];
const ENTITY: [f32; 4] = [0.97, 0.45, 0.49, 1.];
type HeldToken = ((usize, u32), usize);
#[derive(Default)]
struct Charge {
    started: Option<(u64, HeldToken)>,
}
impl Charge {
    fn update(&mut self, drawing: bool, token: Option<HeldToken>, now: u64) -> f32 {
        let Some(token) = token.filter(|_| drawing) else {
            self.started = None;
            return 0.;
        };
        let start = match self.started {
            Some((at, old)) if old == token && now >= at => at,
            _ => {
                self.started = Some((now, token));
                now
            }
        };
        (now.saturating_sub(start) as f32 / 1000.).max(0.001)
    }
}

pub struct Trajectories {
    key: u16,
    enabled: bool,
    unavailable: Option<String>,
    checked_at: u64,
    /// Draw time belongs to one slot in one world; switching resets it.
    charge: Charge,
    paused: bool,
    /// The held item that launches a projectile, for the status line.
    held: Option<String>,
    line_width: f32,
    hit_box: bool,
    hit_info: bool,
    entities: bool,
    ticks: usize,
    update_hz: u32,
    info: Option<(Arc<flight::Flight>, String)>,
}

impl Trajectories {
    pub fn new(cfg: &config::Trajectories) -> Self {
        Self {
            key: key_or_warn("trajectories", &cfg.key),
            enabled: false,
            unavailable: blocks::available().and_then(|_| items::available()).err(),
            checked_at: 0,
            charge: Charge::default(),
            paused: false,
            held: None,
            line_width: if cfg.line_width.is_finite() {
                cfg.line_width.clamp(1., 5.)
            } else {
                2.
            },
            hit_box: cfg.hit_box,
            hit_info: cfg.hit_info,
            entities: cfg.entities,
            ticks: cfg.ticks.clamp(20, 200) as usize,
            update_hz: cfg.update_hz.clamp(10, 60),
            info: None,
        }
    }

    fn reset(&mut self) {
        self.charge = Charge::default();
        self.held = None;
        self.info = None;
        flight::set_wanted(None);
    }
}

impl Module for Trajectories {
    fn name(&self) -> &str {
        "trajectories"
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
            log!("trajectoriesを利用できません: {e}");
            return;
        }
        // The held item, the camera to draw with, and entities to hit.
        let steps: [fn(bool) -> Result<(), String>; 2] = [items::use_items, game::use_camera];
        for (done, step) in steps.iter().enumerate() {
            if let Err(e) = step(on) {
                log!("trajectories: {e}");
                // Undo the steps already taken, so nothing stays subscribed.
                for undo in &steps[..done] {
                    let _ = undo(!on);
                }
                return;
            }
        }
        if self.entities {
            if let Err(e) = game::use_entities(on) {
                for undo in steps {
                    let _ = undo(!on);
                }
                log!("trajectories: {e}");
                return;
            }
        }
        flight::set_active(on);
        self.reset();
        self.enabled = on;
        flight::configure(self.ticks, self.entities, self.update_hz);
    }
    fn tick(&mut self, tick: &Tick) {
        if !self.enabled || tick.now_ms.saturating_sub(self.checked_at) < ITEM_MS {
            return;
        }
        self.checked_at = tick.now_ms;
        self.paused = !tick.focused
            || crate::gui::captures_input()
            || !game::in_world_view_now()
            || game::inventory::screen_open();
        if self.paused {
            self.reset();
            return;
        }
        let inventory = items::snapshot();
        let held = inventory.as_ref().and_then(|inventory| {
            inventory
                .slots
                .get(inventory.selected)
                .and_then(Option::as_ref)
                .map(|item| item.name.as_str())
        });
        let drawing = held == Some("bow") && input::key_down(VK_RBUTTON);
        let seconds = self.charge.update(
            drawing,
            inventory.as_ref().map(|i| (i.world, i.selected)),
            tick.now_ms,
        );
        let physics = held.and_then(|name| flight::physics(name, seconds));
        if physics.is_none() {
            self.held = None;
        } else if self.held.as_deref() != held {
            self.held = held.map(str::to_owned);
        }
        flight::set_wanted(physics);
    }
    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        if self.enabled && self.paused {
            return "一時停止（画面操作中・未接続）".into();
        }
        match &self.held {
            Some(name) => name.clone(),
            None => "投擲物を持っていません".into(),
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::number(
                "line_width",
                "軌道の太さ",
                "軌道を描く線の太さ（ピクセル）です。",
                self.line_width as f64,
                1.,
                5.,
            ),
            Detail::boolean(
                "hit_box",
                "着弾対象を枠で表示",
                "最初に当たるブロックまたはエンティティを囲みます。",
                self.hit_box,
            ),
            Detail::boolean(
                "hit_info",
                "着弾距離を表示",
                "着弾位置に、発射点からの距離を表示します。推定値です。",
                self.hit_info,
            ),
            Detail::boolean(
                "entities",
                "エンティティへの衝突を調べる",
                "OFFにするとブロックだけで軌道を止めます。",
                self.entities,
            ),
            Detail::integer(
                "ticks",
                "予測するティック数",
                "1秒は20ティックです。短くすると遠方の計算量を減らせます。",
                self.ticks as i32,
                20,
                200,
            ),
            Detail::integer(
                "update_hz",
                "更新回数/秒",
                "下げるとゲーム側の読み取り回数を減らせます。",
                self.update_hz as i32,
                10,
                60,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "line_width" => self.line_width = v.number()?.clamp(1., 5.) as f32,
            "hit_box" => self.hit_box = v.boolean()?,
            "hit_info" => self.hit_info = v.boolean()?,
            "entities" => {
                let on = v.boolean()?;
                if self.enabled && on != self.entities {
                    game::use_entities(on)?;
                }
                self.entities = on;
            }
            "ticks" => self.ticks = v.number()?.clamp(20., 200.) as usize,
            "update_hz" => self.update_hz = v.number()?.clamp(10., 60.) as u32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        flight::configure(self.ticks, self.entities, self.update_hz);
        Ok(())
    }
    fn draw(&mut self, ui: &Ui) {
        if !crate::modules::hud_view() {
            return;
        }
        let (Some(flight), Some(camera)) = (flight::flight(), game::camera()) else {
            return;
        };
        self.paint(ui, &flight, &camera);
    }
}
impl Trajectories {
    fn paint(&mut self, ui: &Ui, flight: &Arc<flight::Flight>, camera: &game::Camera) {
        let size = ui.io().display_size;
        let draw = ui.get_background_draw_list();
        for pair in flight.points.windows(2) {
            if let Some((a, b)) = segment(camera, size, pair[0], pair[1]) {
                draw.add_line(a, b, [0., 0., 0., 0.65])
                    .thickness(self.line_width + 2.)
                    .build();
                draw.add_line(a, b, PATH).thickness(self.line_width).build();
            }
        }
        let Some(hit) = &flight.hit else {
            return;
        };
        let (min, max, color) = match hit.target {
            flight::Target::Block(p) => {
                let min = p.map(|v| v as f32);
                (min, min.map(|v| v + 1.), BLOCK)
            }
            flight::Target::Entity(b) => ([b[0], b[1], b[2]], [b[3], b[4], b[5]], ENTITY),
        };
        if self.hit_box {
            if let Some(lines) = box_lines(camera, size, min, max) {
                BoxPainter::new(ui).draw(&lines, color, 1.5);
            }
        }
        if let Some(p) = crate::modules::view::project(camera, size, hit.point).filter(|p| {
            (4. ..=size[0] - 4.).contains(&p[0]) && (4. ..=size[1] - 4.).contains(&p[1])
        }) {
            draw.add_circle(p, 4., color).filled(true).build();
            if self.hit_info {
                if self
                    .info
                    .as_ref()
                    .is_none_or(|(old, _)| !Arc::ptr_eq(old, flight))
                {
                    let distance = flight
                        .points
                        .first()
                        .map(|first| {
                            (0..3)
                                .map(|k| (hit.point[k] - first[k]).powi(2))
                                .sum::<f32>()
                                .sqrt()
                        })
                        .unwrap_or(0.);
                    let kind = if matches!(hit.target, flight::Target::Block(_)) {
                        "ブロック"
                    } else {
                        "エンティティ"
                    };
                    self.info = Some((Arc::clone(flight), format!("{kind}  {distance:.1}m")));
                }
                let label = &self.info.as_ref().unwrap().1;
                let text = ui.calc_text_size(label);
                let at = [
                    (p[0] + 10.).clamp(4., (size[0] - text[0] - 8.).max(4.)),
                    (p[1] + 8.).clamp(4., (size[1] - text[1] - 8.).max(4.)),
                ];
                draw.add_rect(
                    [at[0] - 4., at[1] - 3.],
                    [at[0] + text[0] + 4., at[1] + text[1] + 3.],
                    [0.025, 0.04, 0.065, 0.96],
                )
                .filled(true)
                .rounding(4.)
                .build();
                draw.add_text(at, crate::theme::TEXT, label);
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let mut trajectories = Trajectories::new(&config::Trajectories::default());
    let camera = game::Camera {
        origin: [0.; 3],
        right: [1., 0., 0.],
        up: [0., 1., 0.],
        back: [0., 0., 1.],
        scale: [1., 1.],
    };
    let points = vec![[0., -0.4, -1.], [0.4, -0.4, -3.], [1., -1., -6.]];
    let flight = Arc::new(flight::Flight {
        hit: Some(flight::Hit {
            point: *points.last().unwrap(),
            target: flight::Target::Block([1, -2, -6]),
        }),
        points,
    });
    trajectories.paint(ui, &flight, &camera);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draw_power_resets_on_release_slot_change_world_change_and_clock_reset() {
        let mut charge = Charge::default();
        let token = ((1, 2), 0);
        assert_eq!(charge.update(false, Some(token), 100), 0.);
        assert!(charge.update(true, Some(token), 101) < 0.01);
        assert_eq!(charge.update(true, Some(token), 601), 0.5);
        assert!(charge.update(true, Some(((1, 2), 1)), 602) < 0.01);
        assert!(charge.update(true, Some(((2, 2), 1)), 900) < 0.01);
        assert!(charge.update(true, Some(((2, 2), 1)), 100) < 0.01);
        assert_eq!(charge.update(true, None, 101), 0.);
    }
}
