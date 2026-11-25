//! F3-style passive diagnostics. Native reads stay in the existing samplers;
//! this view only consumes owned snapshots and refreshes text at 10Hz.
use crate::modules::{
    game::{self, actors, blocks},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{api, config, log};
use hudhook::imgui::Ui;
use std::time::{Duration, Instant};
mod hardware;

const REFRESH_MS: u64 = 100;
/// Called by the worker before taking the render-shared module lock.
pub fn poll_hardware() {
    hardware::poll();
}

fn block_position(p: api::Vec3) -> [i32; 3] {
    [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32]
}
fn facing(yaw: f32) -> &'static str {
    const NAMES: [&str; 4] = ["南 (+Z)", "西 (-X)", "北 (-Z)", "東 (+X)"];
    NAMES[(yaw.rem_euclid(360.) / 90.).round() as usize % 4]
}
fn player_lines(p: api::Player) -> Vec<String> {
    let b = block_position(p.position);
    let chunk = b.map(|v| v.div_euclid(16));
    let local = b.map(|v| v.rem_euclid(16));
    vec![
        format!(
            "XYZ: {:.3} / {:.3} / {:.3}",
            p.position.x, p.position.y, p.position.z
        ),
        format!("ブロック: {} / {} / {}", b[0], b[1], b[2]),
        format!("チャンク: {} / {} / {}", chunk[0], chunk[1], chunk[2]),
        format!("チャンク内: {} / {} / {}", local[0], local[1], local[2]),
        format!(
            "Yaw: {:.2} / Pitch: {:.2}",
            (p.rotation.yaw + 180.).rem_euclid(360.) - 180.,
            p.rotation.pitch
        ),
        format!("方角: {}", facing(p.rotation.yaw)),
        format!(
            "速度: 水平 {:.2} / 全体 {:.2} blocks/s",
            p.horizontal_speed(),
            p.speed()
        ),
        format!(
            "速度成分: {:.2} / {:.2} / {:.2}",
            p.velocity.x, p.velocity.y, p.velocity.z
        ),
    ]
}

pub struct DebugHud {
    key: u16,
    on: bool,
    target: bool,
    system: bool,
    background: bool,
    font_scale: f64,
    opacity: f64,
    clock: Option<game::world_info::Subscription>,
    x: i32,
    y: i32,
    right_x: i32,
    right_y: i32,
    sampled: Option<u64>,
    left: Vec<String>,
    right: Vec<String>,
    title: String,
    machine: Vec<String>,
    performance: String,
    display: String,
    renderer: String,
    memory: String,
    performance_at: Option<Instant>,
}
impl DebugHud {
    pub fn new(c: &config::DebugHud) -> Self {
        Self {
            key: key_or_warn("debughud", &c.key),
            on: false,
            target: c.target,
            system: c.system,
            background: c.background,
            font_scale: if c.font_scale.is_finite() {
                c.font_scale.clamp(0.75, 3.)
            } else {
                2.5
            },
            opacity: if c.opacity.is_finite() {
                c.opacity.clamp(0., 1.)
            } else {
                0.45
            },
            clock: None,
            x: c.x,
            y: c.y,
            right_x: c.right_x,
            right_y: c.right_y,
            sampled: None,
            left: Vec::new(),
            right: Vec::new(),
            title: game::build_name()
                .map(|v| format!("Minecraft Bedrock {v}"))
                .unwrap_or_else(|| "Minecraft Bedrock".into()),
            machine: Vec::new(),
            performance: String::new(),
            display: String::new(),
            renderer: String::new(),
            memory: String::new(),
            performance_at: None,
        }
    }
}
impl Module for DebugHud {
    fn name(&self) -> &str {
        "debughud"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.on
    }
    fn set_enabled(&mut self, on: bool) {
        if self.on == on {
            return;
        }
        if on && self.target {
            if let Err(e) = blocks::available() {
                log!("DebugHUD: {e}");
                return;
            }
        }
        // This shared subscription owns the HUD hook and metadata sampling.
        if let Err(e) = actors::use_actors(on) {
            log!("DebugHUD: {e}");
            return;
        }
        if self.target {
            blocks::set_look(on);
            let _ = blocks::lighting::set_debug_active(on);
            blocks::environment::set_target(on);
        }
        self.clock = if on {
            game::world_info::subscribe().ok()
        } else {
            None
        };
        if on && self.system && self.machine.is_empty() {
            hardware::request();
        }
        self.on = on;
        self.sampled = None;
        self.performance_at = None;
        self.left.clear();
        self.right.clear();
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "target",
                "照準先を表示",
                "ブロック・エンティティの名前、座標、距離、取得できる体力を表示します。",
                self.target,
            ),
            Detail::boolean(
                "system",
                "表示環境を表示",
                "CPU名・検出したGPU名・描画API・画面サイズ・プロセスメモリを表示します。",
                self.system,
            ),
            Detail::boolean(
                "background",
                "文字の背景を表示",
                "文字の後ろに濃い背景を描き、明るい場所でも読みやすくします。",
                self.background,
            ),
            Detail::number(
                "font_scale",
                "文字の大きさ",
                "参考画像のような大きな文字で表示します。狭い画面では収まるよう縮小します。",
                self.font_scale,
                0.75,
                3.,
            ),
            Detail::number(
                "opacity",
                "背景の濃さ",
                "文字幅に合わせた背景の不透明度です。",
                self.opacity,
                0.,
                1.,
            ),
            Detail::integer(
                "x",
                "左側の横位置",
                "負の値は画面右端を基準にします。",
                self.x,
                -7680,
                7680,
            ),
            Detail::integer(
                "y",
                "左側の縦位置",
                "負の値は画面下端を基準にします。",
                self.y,
                -4320,
                4320,
            ),
            Detail::integer(
                "right_x",
                "右側の横位置",
                "負の値は画面右端を基準にします。",
                self.right_x,
                -7680,
                7680,
            ),
            Detail::integer(
                "right_y",
                "右側の縦位置",
                "負の値は画面下端を基準にします。",
                self.right_y,
                -4320,
                4320,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "target" => {
                let target = v.boolean()?;
                if self.target != target && self.on {
                    if target {
                        blocks::available()?;
                    }
                    blocks::set_look(target);
                    let _ = blocks::lighting::set_debug_active(target);
                    blocks::environment::set_target(target);
                }
                self.target = target;
            }
            "system" => {
                self.system = v.boolean()?;
                if self.system && self.on && self.machine.is_empty() {
                    hardware::request();
                }
            }
            "background" => self.background = v.boolean()?,
            "font_scale" => self.font_scale = v.number()?.clamp(0.75, 3.),
            "opacity" => self.opacity = v.number()?.clamp(0., 1.),
            "x" => self.x = v.number()? as i32,
            "y" => self.y = v.number()? as i32,
            "right_x" => self.right_x = v.number()? as i32,
            "right_y" => self.right_y = v.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.sampled = None;
        Ok(())
    }
    fn status(&self) -> String {
        "座標・チャンク・向き・表示環境".into()
    }
    fn tick(&mut self, t: &Tick) {
        if !self.on
            || self
                .sampled
                .is_some_and(|at| t.now_ms.saturating_sub(at) < REFRESH_MS)
        {
            return;
        }
        self.sampled = Some(t.now_ms);
        if self.system && self.machine.is_empty() {
            if let Some(lines) = hardware::lines() {
                self.machine = lines.to_vec();
            }
        }
        self.left.clear();
        self.right.clear();
        // SDK positions include the feet height. Never reuse a previous world.
        if let Some(p) = api::game().ok().and_then(|g| g.player().ok()) {
            let details = game::world_info::details();
            self.left.push(format!(
                "ワールド名: {}",
                details
                    .as_ref()
                    .and_then(|d| d.name.as_deref())
                    .unwrap_or("未取得")
            ));
            self.left.push(format!(
                "ワールドID: {}",
                details
                    .as_ref()
                    .and_then(|d| d.world_id.as_deref())
                    .unwrap_or("未取得")
            ));
            let dimension = game::world::dimension();
            self.left.push(format!(
                "ディメンション: {}",
                dimension.as_ref().map_or("未取得", |(name, _)| name)
            ));
            self.left.push(format!(
                "難易度: {}",
                details
                    .as_ref()
                    .and_then(|d| d.difficulty)
                    .and_then(|v| ["ピースフル", "イージー", "ノーマル", "ハード"].get(v as usize))
                    .copied()
                    .unwrap_or("未取得")
            ));
            let clock = game::world_info::info();
            self.left.push(
                clock
                    .map(|v| {
                        let (_, h, m) = game::world_info::clock(v.ticks);
                        format!(
                            "ワールド時刻: {} ({h:02}:{m:02})",
                            v.ticks.rem_euclid(24000)
                        )
                    })
                    .unwrap_or_else(|| "ワールド時刻: 未取得".into()),
            );
            let mut position = player_lines(p);
            if let Some((_, realm)) = &dimension {
                let conversion = match realm {
                    game::world::Realm::Overworld => Some(("ネザー", 0.125)),
                    game::world::Realm::Nether => Some(("オーバーワールド", 8.)),
                    _ => None,
                };
                if let Some((name, scale)) = conversion {
                    position.insert(
                        1,
                        format!(
                            "{name}: X {:.2}, Y {:.2}, Z {:.2}",
                            p.position.x * scale,
                            p.position.y,
                            p.position.z * scale
                        ),
                    );
                }
            }
            self.left.extend(position);
            if let Some(a) = actors::actors() {
                self.left.push(format!(
                    "取得済みエンティティ: {}（自分を除く）",
                    a.values().filter(|a| !a.local).count()
                ));
            }
            self.left.push(
                clock
                    .map(|v| format!("経過日数: {}", v.ticks.div_euclid(24000)))
                    .unwrap_or_else(|| "経過日数: 未取得".into()),
            );
            self.left.push(format!(
                "バイオーム: {}",
                details
                    .as_ref()
                    .and_then(|d| d.biome.as_deref())
                    .unwrap_or("未取得")
            ));
            if dimension
                .as_ref()
                .is_some_and(|(_, realm)| *realm == game::world::Realm::Overworld)
            {
                let b = block_position(p.position);
                self.left.push(format!(
                    "スライムチャンク: {}",
                    crate::modules::slime::at_block(b[0], b[2])
                ));
            }
            self.left.push(
                details
                    .as_ref()
                    .and_then(|d| d.sounds)
                    .map(|(total, real)| format!("サウンド数: {total}（実チャンネル {real}）"))
                    .unwrap_or_else(|| "サウンド数: 未取得".into()),
            );
            if self.target {
                let target = crate::modules::waila::inspection_lines();
                if target.is_empty() {
                    self.right.push("対象なし・取得待ち".into());
                } else {
                    self.right.extend(target);
                }
            }
        } else {
            self.left.push("（ワールド未接続・データ取得待ち）".into());
        }
        if self.system {
            self.memory = format!("プロセスメモリ: {}", crate::modules::hud::memory_text());
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if crate::gui::MAP_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if self
            .performance_at
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(250))
        {
            self.performance = format!(
                "FPS: {:.0} / フレーム時間: {:.2} ms",
                ui.io().framerate,
                ui.io().delta_time * 1000.
            );
            self.display = format!(
                "画面: {:.0} x {:.0}",
                ui.io().display_size[0],
                ui.io().display_size[1]
            );
            self.renderer = format!("描画API: {}", crate::gui::renderer_name());
            self.performance_at = Some(Instant::now());
        }
        let left = std::iter::once(self.title.as_str())
            .chain(std::iter::once(self.performance.as_str()))
            .chain(self.left.iter().map(String::as_str));
        let system = self
            .machine
            .iter()
            .take(usize::from(self.system))
            .map(String::as_str)
            .chain(self.system.then_some(self.display.as_str()))
            .chain(
                self.machine
                    .iter()
                    .skip(1)
                    .take(usize::from(self.system) * self.machine.len())
                    .map(String::as_str),
            )
            .chain(self.system.then_some(self.renderer.as_str()))
            .chain(self.system.then_some(self.memory.as_str()))
            .chain(self.system.then_some(""));
        let right = system.chain(self.right.iter().map(String::as_str));
        let screen = ui.io().display_size;
        if !screen.iter().all(|v| v.is_finite() && *v > 0.) {
            return;
        }
        let max_width = (screen[0] * 0.48 - 8.).max(1.);
        let requested_size = ui.current_font_size() * self.font_scale as f32;
        // Changing a font outside Begin/End marks ImGui's implicit Debug window
        // as used. Give this draw-only overlay an invisible, non-interactive host.
        ui.window("##mio_debughud_surface")
            .position([0., 0.], hudhook::imgui::Condition::Always)
            .size(screen, hudhook::imgui::Condition::Always)
            .no_decoration()
            .no_inputs()
            .no_nav()
            .draw_background(false)
            .save_settings(false)
            .focus_on_appearing(false)
            .bring_to_front_on_focus(false)
            .build(|| {
                let _raster = crate::gui::debug_font(ui).map(|id| ui.push_font(id));
                let base_size = ui.current_font_size();
                let widest = left
                    .clone()
                    .chain(right.clone())
                    .map(|s| ui.calc_text_size(s)[0] + 8.)
                    .fold(1., f32::max);
                let rows = left.clone().count().max(right.clone().count()).max(1) as f32;
                let scale = (requested_size / base_size)
                    .min(max_width / widest)
                    .min((screen[1] - 20.).max(1.) / (rows * (base_size + 3.)))
                    .max(0.5);
                let _font = FontScale::new(ui, scale);
                let style = ColumnStyle {
                    background: self.background,
                    opacity: self.opacity as f32,
                    right: false,
                    shadow: (base_size * scale / 16.).max(1.),
                };
                let size = column(ui, left, [self.x, self.y], max_width, style);
                crate::hud_editor::measure("debughud", size);
                column(
                    ui,
                    right,
                    [self.right_x, self.right_y],
                    max_width,
                    ColumnStyle {
                        right: true,
                        ..style
                    },
                );
            });
    }
}

struct FontScale<'a> {
    ui: &'a Ui,
    old: f32,
}
impl<'a> FontScale<'a> {
    fn new(ui: &'a Ui, scale: f32) -> Self {
        let font = ui.current_font();
        let old =
            ui.current_font_size() / (font.font_size * font.scale * ui.io().font_global_scale);
        ui.set_window_font_scale(old * scale);
        Self { ui, old }
    }
}
impl Drop for FontScale<'_> {
    fn drop(&mut self) {
        self.ui.set_window_font_scale(self.old);
    }
}
#[derive(Clone, Copy)]
struct ColumnStyle {
    background: bool,
    opacity: f32,
    right: bool,
    shadow: f32,
}

fn column<'a>(
    ui: &Ui,
    lines: impl Iterator<Item = &'a str> + Clone,
    offsets: [i32; 2],
    max_width: f32,
    style: ColumnStyle,
) -> [f32; 2] {
    let step = ui.text_line_height() + 3.;
    let width = (lines
        .clone()
        .map(|s| ui.calc_text_size(s)[0] + 8.)
        .fold(1., f32::max))
    .min(max_width);
    let height = (lines.clone().count() as f32 * step).min(ui.io().display_size[1]);
    let screen = ui.io().display_size;
    let x = crate::modules::hud::origin(offsets[0], screen[0], width);
    let y = crate::modules::hud::origin(offsets[1], screen[1], height);
    let draw = ui.get_background_draw_list();
    draw.with_clip_rect_intersect([x, y], [x + width, y + height], || {
        for (i, line) in lines.enumerate() {
            if line.is_empty() {
                continue;
            }
            let top = y + i as f32 * step;
            if top >= y + height {
                break;
            }
            let row_width = (ui.calc_text_size(line)[0] + 8.).min(width);
            let row_x = if style.right {
                x + width - row_width
            } else {
                x
            };
            if style.background {
                draw.add_rect(
                    [row_x, top],
                    [row_x + row_width, top + step],
                    [0., 0., 0., style.opacity],
                )
                .filled(true)
                .build();
            }
            draw.add_text(
                [row_x + 4. + style.shadow, top + style.shadow],
                [0., 0., 0., 0.9],
                line,
            );
            let color = if line.ends_with(": false") {
                crate::theme::DANGER
            } else if line.ends_with(": true") {
                crate::theme::SUCCESS
            } else {
                crate::theme::TEXT
            };
            draw.add_text([row_x + 4., top], color, line);
        }
    });
    [width.max(1.), height.max(1.)]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_debug_overlay_does_not_expose_the_implicit_debug_window_or_change_font_size() {
        use hudhook::imgui::internal::RawWrapper;
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for fonts in 1..=2 {
            let mut ctx = hudhook::imgui::Context::create();
            ctx.set_ini_filename(None);
            ctx.fonts()
                .add_font(&[hudhook::imgui::FontSource::DefaultFontData { config: None }]);
            if fonts == 2 {
                ctx.fonts()
                    .add_font(&[hudhook::imgui::FontSource::DefaultFontData {
                        config: Some(hudhook::imgui::FontConfig {
                            size_pixels: 32.,
                            ..Default::default()
                        }),
                    }]);
            }
            ctx.fonts().build_rgba32_texture();
            ctx.io_mut().display_size = [2560., 1440.];
            ctx.io_mut().delta_time = 1. / 60.;
            let mut hud = DebugHud::new(&config::DebugHud::default());
            hud.left = vec!["XYZ: -85.821 / 70.000 / 243.260".into()];
            hud.machine = vec!["CPU: test processor".into(), "GPU: test adapter".into()];
            for _ in 0..2 {
                let ui = ctx.frame();
                let size = ui.current_font_size();
                hud.draw(ui);
                assert_eq!(ui.current_font_size(), size);
                let data = ctx.render();
                assert!(data.total_vtx_count > 0);
                for list in data.draw_lists() {
                    let name = unsafe { std::ffi::CStr::from_ptr(list.raw()._OwnerName) }
                        .to_string_lossy();
                    assert!(
                        !name.starts_with("Debug##"),
                        "unexpected implicit window: {name}"
                    );
                }
            }
        }
    }
    #[test]
    fn negative_coordinates_have_euclidean_chunk_and_local_positions() {
        let p = api::Player {
            position: api::Vec3::new(-0.1, -64.1, -16.01),
            eye_position: api::Vec3::default(),
            velocity: api::Vec3::new(3., 0., 4.),
            rotation: api::Rotation {
                yaw: -90.,
                pitch: 20.,
            },
        };
        let lines = player_lines(p);
        assert!(lines.iter().any(|s| s == "ブロック: -1 / -65 / -17"));
        assert!(lines.iter().any(|s| s == "チャンク: -1 / -5 / -2"));
        assert!(lines.iter().any(|s| s == "チャンク内: 15 / 15 / 15"));
        assert!(lines.iter().any(|s| s.contains("水平 5.00")));
        for (yaw, expected) in [
            (0., "南 (+Z)"),
            (90., "西 (-X)"),
            (180., "北 (-Z)"),
            (-90., "東 (+X)"),
            (450., "西 (-X)"),
        ] {
            assert_eq!(facing(yaw), expected);
        }
    }
    #[test]
    fn debug_columns_render_with_finite_clipped_geometry_at_small_and_large_sizes() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.fonts()
            .add_font(&[hudhook::imgui::FontSource::DefaultFontData { config: None }]);
        ctx.fonts().build_rgba32_texture();
        let lines = [
            "DebugHUD",
            "XYZ: -0.100 / -64.100 / -16.010",
            "minecraft:deepslate_diamond_ore",
            "",
            "1920 x 1080",
        ];
        for screen in [[640., 360.], [1280., 720.], [2560., 1440.]] {
            ctx.io_mut().display_size = screen;
            ctx.io_mut().delta_time = 1. / 60.;
            let ui = ctx.frame();
            let max_width = screen[0] * 0.48 - 8.;
            let style = ColumnStyle {
                background: true,
                opacity: 0.45,
                right: false,
                shadow: 1.,
            };
            let size = column(ui, lines.iter().copied(), [10, 10], max_width, style);
            column(
                ui,
                lines.iter().copied(),
                [-10, 10],
                max_width,
                ColumnStyle {
                    right: true,
                    ..style
                },
            );
            assert!(size[0] <= max_width && size[1] <= screen[1]);
            let data = ctx.render();
            assert!(data.total_vtx_count > 0);
            for list in data.draw_lists() {
                assert!(list
                    .vtx_buffer()
                    .iter()
                    .all(|v| v.pos.iter().all(|v| v.is_finite())));
                for command in list.commands() {
                    if let hudhook::imgui::DrawCmd::Elements { cmd_params, .. } = command {
                        let [x, y, right, bottom] = cmd_params.clip_rect;
                        assert!(x >= 0. && y >= 0. && right <= screen[0] && bottom <= screen[1]);
                    }
                }
            }
        }
    }
    #[test]
    fn scaling_is_restored_and_right_rows_share_the_same_edge() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.fonts()
            .add_font(&[hudhook::imgui::FontSource::DefaultFontData { config: None }]);
        ctx.fonts().build_rgba32_texture();
        ctx.io_mut().display_size = [1280., 720.];
        ctx.io_mut().delta_time = 1. / 60.;
        let ui = ctx.frame();
        ui.set_window_font_scale(1.25);
        let original = ui.current_font_size();
        {
            let _scale = FontScale::new(ui, 2.);
            assert!((ui.current_font_size() - original * 2.).abs() < 0.001);
            column(
                ui,
                ["CPU: long model name", "GPU"].into_iter(),
                [-10, 10],
                500.,
                ColumnStyle {
                    background: true,
                    opacity: 0.45,
                    right: true,
                    shadow: 2.,
                },
            );
        }
        assert!((ui.current_font_size() - original).abs() < 0.001);
        let data = ctx.render();
        // Filled row rectangles use the configured alpha. Their right edges
        // must match even though their widths differ.
        let alpha = (0.45_f32 * 255.) as u8;
        let rects: Vec<_> = data
            .draw_lists()
            .flat_map(|l| l.vtx_buffer())
            .filter(|v| v.col[..3] == [0, 0, 0] && v.col[3].abs_diff(alpha) <= 1)
            .collect();
        assert_eq!(rects.len(), 8);
        let edge = |vertices: &[&hudhook::imgui::DrawVert]| {
            vertices
                .iter()
                .map(|v| v.pos[0])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        assert_eq!(edge(&rects[..4]), 1270.);
        assert_eq!(edge(&rects[4..]), 1270.);
        assert!(rects[4].pos[0] > rects[0].pos[0]);
    }
}
