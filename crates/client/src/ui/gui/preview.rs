//! Headless layout regression and optional render data for visual review.
use super::*;
use hudhook::imgui::DrawCmd;
use serde::Serialize;

#[derive(Serialize)]
struct Mesh {
    vertices: Vec<([f32; 2], [f32; 2], [u8; 4])>,
    indices: Vec<u32>,
    commands: Vec<(usize, usize, usize, [f32; 4], usize)>,
}
#[derive(Serialize)]
struct Frame {
    atlas_size: [u32; 2],
    display_size: [f32; 2],
    meshes: Vec<Mesh>,
    item_atlas_size: Option<[u32; 2]>,
}

impl Frame {
    /// The draw data of a rendered frame, for the offline rasterizer.
    fn capture(
        data: &hudhook::imgui::DrawData,
        atlas_size: [u32; 2],
        item_atlas_size: Option<[u32; 2]>,
    ) -> Self {
        let meshes = data
            .draw_lists()
            .map(|list| Mesh {
                vertices: list
                    .vtx_buffer()
                    .iter()
                    .map(|v| (v.pos, v.uv, v.col))
                    .collect(),
                indices: list.idx_buffer().iter().map(|&i| u32::from(i)).collect(),
                commands: list
                    .commands()
                    .filter_map(|c| match c {
                        DrawCmd::Elements { count, cmd_params } => Some((
                            count,
                            cmd_params.idx_offset,
                            cmd_params.vtx_offset,
                            cmd_params.clip_rect,
                            cmd_params.texture_id.id(),
                        )),
                        _ => None,
                    })
                    .collect(),
            })
            .collect();
        Self {
            atlas_size,
            display_size: data.display_size,
            meshes,
            item_atlas_size,
        }
    }
    fn write(&self, dir: &std::path::Path) {
        std::fs::write(dir.join("frame.toml"), toml::to_string(self).unwrap()).unwrap();
    }
}

/// Writes a rendered frame and its font atlas (`frame.toml`, `atlas-alpha.bin`)
/// for other modules' manual benchmarks.
pub(crate) fn export_frame(
    dir: &std::path::Path,
    data: &hudhook::imgui::DrawData,
    atlas: &([u32; 2], Vec<u8>),
) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("atlas-alpha.bin"), &atlas.1).unwrap();
    Frame::capture(data, atlas.0, None).write(dir);
}

struct TextureCapture;

pub(super) const OVERFLOW_HUD: [(&str, &str); 35] = [
    ("armorhud", "8スロットを使用中"),
    ("inventoryhud", "8スロットを使用中"),
    ("sessionstats", "0.0m / ジャンプ0回"),
    ("deathpoint", "死亡地点の記録を待機中"),
    ("vitals", "HP 20.0/20.0"),
    ("nametags", "2件の名前"),
    ("radar", ""),
    ("targethud", ""),
    ("combathud", "撃破0 / ヒット0"),
    ("compass", ""),
    ("coordinates", "488 96 97"),
    ("esp", "12個の枠"),
    ("blockesp", "0個のブロック"),
    (
        "autofish",
        "最初は手動で釣り竿を投げてください / 回収100回・再投100回",
    ),
    (
        "waila",
        "ブロック: brown_terracotta・距離 2.0 ブロック・座標 485 / 96 / 98",
    ),
    ("fps", "122"),
    ("cps", "0 | 0"),
    ("speed", "0.00ブロック/秒"),
    ("dimension", "Overworld"),
    ("clock", "14:28:06"),
    ("memory", "1849 MB"),
    ("entities", "26"),
    ("server", "(none)"),
    ("ping", "接続先サーバーを待機中"),
    ("keystrokes", ""),
    ("waypoints", "0 saved"),
    ("joinnotify", "入退室待ち"),
    ("mininghud", "採掘待機中"),
    ("itemcounter", "3種類 / 合計1個"),
    ("minimap", "半径32 / 記憶25チャンク"),
    ("mobcount", "23体 / 48m"),
    ("worldinfo", "07:35"),
    ("frametime", "平均 8.6 ms / 最大 15.1 ms"),
    ("inventorymove", "インベントリを開くと有効"),
    ("effects", "（データなし）"),
];

#[test]
#[ignore = "manual: HUD layout/CPU baseline; set MIO_UI_PREVIEW to export a frame"]
fn hud_overflow_paint_cost() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = fonts::test_context(16.);
    let texture = ctx.fonts().build_alpha8_texture();
    let atlas_size = [texture.width, texture.height];
    if let Some(dir) = std::env::var_os("MIO_UI_PREVIEW") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("atlas-alpha.bin"),
            texture.data,
        )
        .unwrap();
    }
    ctx.io_mut().display_size = [1280., 720.];
    ctx.io_mut().delta_time = 1. / 60.;
    let mut card = hud_card::Card::default();
    for mode in ["uncached", "changing FPS", "cached", "vertical"] {
        let mut times = Vec::with_capacity(1000);
        for frame in 0..1100 {
            let start = std::time::Instant::now();
            if mode == "uncached" {
                card = hud_card::Card::default();
            }
            card.column_width = 360.;
            card.draw(
                ctx.frame(),
                [10, 10],
                OVERFLOW_HUD.map(|(id, text)| {
                    (
                        id,
                        if mode == "changing FPS" && id == "fps" {
                            (120 + frame % 10).to_string()
                        } else {
                            text.into()
                        },
                    )
                }),
                true,
                true,
                if mode == "vertical" { 1 } else { 0 },
            );
            std::hint::black_box(ctx.render().total_vtx_count);
            if frame >= 100 {
                times.push(start.elapsed());
            }
        }
        times.sort();
        println!(
            "HUD 35 entries / 1280x720 / 16px ({mode}): median {:?}, p90 {:?}; size {:?}",
            times[500],
            times[900],
            crate::hud_editor::measured("hud", [0., 0.])
        );
    }
    if let Some(dir) = std::env::var_os("MIO_UI_PREVIEW") {
        Frame::capture(ctx.render(), atlas_size, None).write(std::path::Path::new(&dir));
    }
}

#[test]
fn profiler_panel_and_details_render_at_font_and_screen_limits() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    for (size, font) in [
        ([1280., 720.], 12.),
        ([1280., 720.], 28.),
        ([2560., 1440.], 16.),
    ] {
        let mut ctx = fonts::test_context(font);
        ctx.io_mut().display_size = size;
        ctx.io_mut().delta_time = 1. / 60.;
        modules::performance_profiler::preview(ctx.frame());
        let data = ctx.render();
        assert!(data.total_vtx_count > 0);
        assert!(data
            .draw_lists()
            .flat_map(|l| l.vtx_buffer())
            .all(|v| v.pos.iter().all(|x| x.is_finite())));
    }
}
impl RenderContext for TextureCapture {
    fn load_texture(
        &mut self,
        _data: &[u8],
        _width: u32,
        _height: u32,
    ) -> Result<hudhook::imgui::TextureId, hudhook::windows::core::Error> {
        Ok(hudhook::imgui::TextureId::from(1))
    }
    fn replace_texture(
        &mut self,
        _id: hudhook::imgui::TextureId,
        _data: &[u8],
        _width: u32,
        _height: u32,
    ) -> Result<(), hudhook::windows::core::Error> {
        unreachable!()
    }
}

fn fixture() -> (Gui, Arc<Mutex<Modules>>) {
    let modules = Arc::new(Mutex::new(Modules::new(&config::Config::default())));
    let gui = Gui {
        modules: Arc::clone(&modules),
        profiles: Arc::new(Mutex::new(crate::profiles::Profiles::new(
            &std::env::temp_dir().join("mio_ui_preview_read_only"),
        ))),
        profile_name: "探索用".into(),
        module_filter: String::new(),
        feature_list: FeatureList::default(),
        hud_card: hud_card::Card::default(),
        expanded_details: Default::default(),
        setting_error: String::new(),
        menu_key: 0x4c,
        menu_key_name: "L".into(),
        eject_key: 0x23,
        key_was_down: AtomicBool::new(false),
        escape_requested: AtomicBool::new(false),
        open: true,
        hud_editor: crate::hud_editor::Editor::default(),
        command: String::new(),
        console_lines: 0,
        console_view: ConsoleView::default(),
        focus_command: false,
        font_size: 16.,
        item_atlas: None,
        cursor_clip: None,
        cursor_inside: None,
    };
    (gui, modules)
}

#[test]
fn expanded_settings_at_font_limits_preserve_module_configuration() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let _settings = crate::game::blocks::CUSTOM_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for pixels in [12., 28.] {
        let mut ctx = fonts::test_context(pixels);
        ctx.io_mut().display_size = [1280., 720.];
        ctx.io_mut().delta_time = 1. / 60.;
        let (mut gui, shared) = fixture();
        let mut modules = shared.lock().unwrap_or_else(|e| e.into_inner());
        let before = serde_json::to_value(modules.settings()).unwrap();
        gui.expanded_details.extend(0..modules.len());
        for index in 0..modules.len() {
            gui.module_filter = modules.menu_entry(index).0.to_string();
            gui.draw_menu(ctx.frame(), &mut modules);
            let data = ctx.render();
            assert!(data
                .draw_lists()
                .flat_map(|l| l.vtx_buffer())
                .all(|v| v.pos.iter().all(|x| x.is_finite())));
        }
        assert_eq!(
            serde_json::to_value(modules.settings()).unwrap(),
            before,
            "drawing settings cannot mutate configuration"
        );
    }
}

#[test]
fn japanese_menu_and_console_build_without_game_or_input() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let _settings = crate::game::blocks::CUSTOM_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut ctx = fonts::test_context(16.);
    let output = std::env::var_os("MIO_UI_PREVIEW").map(std::path::PathBuf::from);
    let hud_only = std::env::var_os("MIO_HUD_CARD_ONLY").is_some();
    let light_only = std::env::var_os("MIO_LIGHT_ONLY").is_some();
    let regular_hud = std::env::var_os("MIO_HUD_REGULAR").is_some();
    let texture = ctx.fonts().build_alpha8_texture();
    let atlas_size = [texture.width, texture.height];
    if let Some(dir) = &output {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("atlas-alpha.bin"), texture.data).unwrap();
    }
    let resource_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("Microsoft.MinecraftUWP_1.26.6301.0_x64__8wekyb3d8bbwe/data/resource_packs");
    let mut item_atlas_size = None;
    modules::item_hud::icons::reset();
    if resource_root.is_dir() && !hud_only && !light_only {
        let atlas = modules::item_hud::icons::load(&resource_root).unwrap();
        item_atlas_size = Some([atlas.width, atlas.height]);
        if let Some(dir) = &output {
            std::fs::write(dir.join("item-atlas-rgba.bin"), &atlas.pixels).unwrap();
        }
        modules::item_hud::icons::upload(atlas, &mut TextureCapture);
    }
    let (mut gui, modules) = fixture();
    if std::env::var_os("MIO_UI_DETAILS").is_some() {
        gui.expanded_details.insert(0);
    }
    let editor = std::env::var_os("MIO_UI_EDITOR").is_some();
    if editor {
        gui.hud_editor.start();
    }
    // Cover a smaller display too: windows may be moved, but the UI must build
    // correctly with Japanese text at either resolution.
    for size in [[1280., 720.], [1600., 960.]] {
        ctx.io_mut().display_size = if light_only {
            [960., 640.]
        } else if hud_only {
            [360., 278.]
        } else {
            size
        };
        ctx.io_mut().delta_time = 1. / 60.;
        for _ in 0..2 {
            let ui = ctx.frame();
            if !hud_only && !light_only {
                if editor {
                    gui.hud_editor.draw(
                        ui,
                        &mut modules.lock().unwrap_or_else(|e| e.into_inner()),
                        &mut gui.profile_name,
                        "",
                    );
                } else {
                    gui.draw_menu(ui, &mut modules.lock().unwrap_or_else(|e| e.into_inner()));
                    gui.draw_console(ui);
                }
                modules::item_hud::preview_grid(ui);
                if std::env::var_os("MIO_MINING_COUNTER_PREVIEW").is_some() {
                    modules::mining_hud::preview(ui);
                    modules::item_counter::preview(ui);
                }
                if std::env::var_os("MIO_LOOK_PREVIEW").is_some() {
                    modules::waila::preview(ui);
                }
            }
            if std::env::var_os("MIO_CHAT_PREVIEW").is_some() {
                modules::chat_hud::preview(ui);
            }
            if std::env::var_os("MIO_COMBAT_PREVIEW").is_some() {
                modules::combat_hud::preview(ui);
                modules::hit_marker::preview(ui);
            }
            if std::env::var_os("MIO_WORLDMAP_PREVIEW").is_some() {
                modules::world_map::preview(ui);
            }
            if std::env::var_os("MIO_MAP_PREVIEW").is_some() {
                modules::minimap::preview(ui);
                modules::trajectories::preview(ui);
                modules::frame_time::preview(ui);
            }
            if std::env::var_os("MIO_LIGHT_PREVIEW").is_some() {
                modules::light_overlay::preview(ui);
            }
            if !light_only {
                // MIO_HUD_LONG: a list as long as one with most readouts on.
                let long = [
                    ("armorhud", "0スロットを使用中"),
                    ("sessionstats", "740.9m / ジャンプ38回"),
                    ("vitals", "HP 20.0/20.0"),
                    ("radar", ""),
                    ("targethud", ""),
                    ("combathud", "撃破1 / ヒット4"),
                    ("compass", ""),
                    ("coordinates", "-2 81 2"),
                    ("esp", "7個の枠"),
                    ("blockesp", "274個 / 最寄り: 石炭鉱石 8 m"),
                    ("waila", "grass・距離 1.0 ブロック"),
                    ("fps", "139"),
                    ("cps", "0 | 0"),
                    ("speed", "0.00ブロック/秒"),
                    ("direction", "NE 水平角 -142.2"),
                    ("dimension", "Overworld"),
                    ("clock", "21:21:54"),
                    ("memory", "2597 MB"),
                    ("server", "192.168.0.10:47086"),
                    ("keystrokes", ""),
                    ("joinnotify", "参加2 / 退出0"),
                    ("mininghud", "採掘待機中"),
                    ("itemcounter", "3種類 / 合計0個"),
                    ("minimap", "半径32 / 記憶166チャンク"),
                    ("mobcount", "17体 / 48m"),
                    ("worldinfo", "20:20"),
                    ("frametime", "平均 7.4 ms / 最大 12.8 ms"),
                    ("chathud", "3件"),
                ];
                let short = [
                    ("inventoryhud", "6スロットを使用中"),
                    ("radar", ""),
                    ("targethud", ""),
                    ("compass", ""),
                    ("inventorymove", "インベントリ中の移動が有効"),
                    ("coordinates", "（ワールド未接続）"),
                ];
                let entries: &[(&str, &str)] = if std::env::var_os("MIO_HUD_LONG").is_some() {
                    &long
                } else {
                    &short
                };
                hud_card::draw(
                    ui,
                    if hud_only { [20, 20] } else { [1100, 500] },
                    entries.iter().map(|&(id, status)| (id, status.into())),
                    !regular_hud,
                    !regular_hud,
                    0,
                );
            }
            let data = ctx.render();
            assert!(data.total_vtx_count > 0);
            if item_atlas_size.is_some() {
                let batches=data.draw_lists().flat_map(|l|l.commands()).filter(|c|matches!(c,DrawCmd::Elements{cmd_params,..} if cmd_params.texture_id.id()==1)).count();
                assert_eq!(batches, 1, "all inventory icons must share one atlas batch");
            }
        }
    }
    if let Some(dir) = output {
        Frame::capture(ctx.render(), atlas_size, item_atlas_size).write(&dir);
    }
    modules::item_hud::icons::reset();
}

#[test]
fn menu_key_edges_update_capture_once_and_shutdown_always_releases_it() {
    assert!(menu_state(false, false, true, true, false));
    assert!(menu_state(true, true, true, true, false));
    assert!(menu_state(true, true, false, true, false));
    assert!(!menu_state(true, false, true, true, false));
    assert!(!menu_state(false, false, true, false, false));
    assert!(!menu_state(true, true, true, true, true));
}

#[test]
fn escape_only_closes_the_menu_after_popup_and_editor_actions() {
    assert!(escape_closes_menu(true, false, false, false));
    for (open, capture, editor, popup) in [
        (false, false, false, false),
        (true, true, false, false),
        (true, false, true, false),
        (true, false, false, true),
        (true, true, true, true),
    ] {
        assert!(!escape_closes_menu(open, capture, editor, popup));
    }
    assert!(escape_press(0x100, 0x1b, 0));
    assert!(escape_press(0x104, 0x1b, 0));
    assert!(
        !escape_press(0x100, 0x1b, 1 << 30),
        "ignore held Esc repeats"
    );
    assert!(!escape_press(0x101, 0x1b, 0), "ignore key release");
    assert!(!escape_press(0x100, 0x4c, 0), "retain configured menu key");
}

#[test]
fn queued_escape_dismisses_a_popup_before_requesting_menu_close() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = Context::create();
    ctx.set_ini_filename(None);
    ctx.io_mut().display_size = [1280., 720.];
    ctx.io_mut().delta_time = 1. / 60.;
    ctx.fonts().build_alpha8_texture();
    let (gui, _) = fixture();
    struct RestoreMenu(bool);
    impl Drop for RestoreMenu {
        fn drop(&mut self) {
            MENU_OPEN.store(self.0, Ordering::Release);
        }
    }
    let _restore = RestoreMenu(MENU_OPEN.swap(true, Ordering::AcqRel));
    let escape = || {
        gui.before_wnd_proc(
            hudhook::windows::Win32::Foundation::HWND(std::ptr::null_mut()),
            0x100,
            hudhook::windows::Win32::Foundation::WPARAM(0x1b),
            hudhook::windows::Win32::Foundation::LPARAM(0),
        );
    };
    {
        let ui = ctx.frame();
        ui.window("test menu").build(|| {
            ui.open_popup("key choices");
            ui.popup("key choices", || ui.text("L"));
        });
    }
    ctx.render();
    assert!(popup_open());
    escape();
    assert!(!gui.escape_requested.load(Ordering::Acquire));
    {
        let ui = ctx.frame();
        ui.window("test menu").build(|| {
            ui.popup("key choices", || ui.close_current_popup());
        });
    }
    ctx.render();
    assert!(!popup_open());
    escape();
    assert!(gui.escape_requested.swap(false, Ordering::AcqRel));
    gui.before_wnd_proc(
        hudhook::windows::Win32::Foundation::HWND(std::ptr::null_mut()),
        0x100,
        hudhook::windows::Win32::Foundation::WPARAM(0x1b),
        hudhook::windows::Win32::Foundation::LPARAM(1 << 30),
    );
    assert!(!gui.escape_requested.load(Ordering::Acquire));
}

#[test]
fn configurable_editing_keys_do_not_toggle_the_menu_while_typing() {
    for key in [0x08, 0x0d, 0x20, 0x25, 0x26, 0x28, 0x60, 0xa0, 0xa3, 0xba] {
        assert!(!menu_key_allowed(key, true, false));
        assert!(!menu_key_allowed(key, false, true));
        assert!(menu_key_allowed(key, false, false));
    }
    assert!(menu_key_allowed(0x75, true, true));
    assert!(
        menu_key_allowed(0x2d, true, true),
        "legacy INSERT shortcut remains available"
    );
}

#[test]
fn letter_menu_key_does_not_toggle_while_typing_and_needs_a_new_press_after_typing() {
    for (text_input, chat) in [(true, false), (false, true), (true, true)] {
        assert!(!menu_key_allowed(0x4c, text_input, chat));
        assert!(menu_key_allowed(0x2d, text_input, chat));
    }
    assert!(menu_key_allowed(0x4c, false, false));
    // The edge is consumed even while text input suppresses the shortcut.
    assert!(!menu_state(
        false,
        false,
        true,
        menu_key_allowed(0x4c, true, false),
        false
    ));
    assert!(!menu_state(
        false,
        true,
        true,
        menu_key_allowed(0x4c, false, false),
        false
    ));
    assert!(menu_state(
        false,
        false,
        true,
        menu_key_allowed(0x4c, false, false),
        false
    ));
}

/// CPU time of one overlay frame with the menu and a full console open, the
/// render thread's heaviest state. Manual: prints the median per frame.
#[test]
#[ignore = "manual: cargo test -p client --release --lib overlay_frame_cost -- --ignored --nocapture"]
fn overlay_frame_cost() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = fonts::test_context(16.);
    ctx.io_mut().display_size = [1920., 1080.];
    ctx.io_mut().delta_time = 1. / 60.;
    // A full log: the console keeps the last 500 lines.
    for i in 0..600 {
        crate::log!(
            "  [ON ] 座標 (coordinates) キー F{} @0x14{i:07X} シグネチャの検索結果とモジュールの状態を表示する長めの行です",
            i % 12
        );
    }
    let (mut gui, shared) = fixture();
    let mut measure = |label: &str, frame: &mut dyn FnMut(&mut Gui, &Ui)| {
        let mut times = Vec::new();
        for _ in 0..200 {
            let start = std::time::Instant::now();
            let ui = ctx.frame();
            frame(&mut gui, ui);
            std::hint::black_box(ctx.render().total_vtx_count);
            times.push(start.elapsed());
        }
        times.sort();
        println!(
            "{label}: median {:?}, p90 {:?}",
            times[times.len() / 2],
            times[times.len() * 9 / 10]
        );
    };
    measure("menu", &mut |gui, ui| {
        gui.draw_menu(ui, &mut shared.lock().unwrap_or_else(|e| e.into_inner()))
    });
    measure("console", &mut |gui, ui| gui.draw_console(ui));
    measure("menu + console", &mut |gui, ui| {
        gui.draw_menu(ui, &mut shared.lock().unwrap_or_else(|e| e.into_inner()));
        gui.draw_console(ui);
    });
    // How long a frame keeps the log locked (every `log!` waits meanwhile).
    let mut held = Vec::new();
    measure("console (log lock)", &mut |gui, ui| {
        gui.draw_console_with(ui, |gui, ui| {
            gui.draw_log(ui, |f| {
                let start = std::time::Instant::now();
                console::with_numbered_lines(|first, lines| f(first, lines));
                held.push(start.elapsed());
            })
        })
    });
    held.sort();
    println!(
        "log lock held per frame: median {:?}, max {:?}",
        held[held.len() / 2],
        held[held.len() - 1]
    );
}

/// Drawing only the console lines in view gives exactly the vertices of
/// drawing every line (the implementation before), frame by frame, while
/// lines are logged, dropped at the front, and the view is scrolled.
#[test]
fn console_drawing_only_visible_lines_matches_drawing_every_line() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let context = || {
        let mut ctx = fonts::test_context(16.);
        ctx.io_mut().display_size = [1280., 720.];
        ctx.io_mut().delta_time = 1. / 60.;
        ctx
    };
    // The drawing this replaced, verbatim.
    fn every_line(gui: &mut Gui, ui: &Ui, lines: &std::collections::VecDeque<String>) {
        for line in lines {
            let _color = ui.push_style_color(
                hudhook::imgui::StyleColor::Text,
                if line.starts_with("> ") {
                    crate::theme::ACCENT
                } else {
                    crate::theme::TEXT
                },
            );
            ui.text_wrapped(line);
        }
        let count = lines.len();
        if count != gui.console_lines || ui.scroll_y() >= ui.scroll_max_y() {
            ui.set_scroll_here_y_with_ratio(1.0);
        }
        gui.console_lines = count;
    }
    fn vertices(ctx: &mut Context) -> Vec<([f32; 2], [f32; 2], [u8; 4])> {
        ctx.render()
            .draw_lists()
            .flat_map(|l| l.vtx_buffer().iter().map(|v| (v.pos, v.uv, v.col)))
            .collect()
    }
    // Both sequences get the same log and input, one after the other (only
    // one ImGui context may exist at a time).
    let run = |visible_only: bool| {
        let (mut gui, _modules) = fixture();
        let mut ctx = context();
        let mut lines = std::collections::VecDeque::new();
        let mut first = 0u64;
        let mut logged = 0;
        let mut frames = Vec::new();
        for frame in 0..90 {
            for _ in 0..frame % 4 {
                lines.push_back(match logged % 5 {
                    0 => format!("> command {logged}"),
                    1 => String::new(),
                    2 => format!("{logged}: 短い行"),
                    3 => format!(
                        "{logged}: 折り返しが必要になる長い行です。シグネチャの検索結果とモジュールの状態を表示します {}",
                        "x".repeat(logged % 90)
                    ),
                    _ => format!("{logged}: a fairly ordinary log line of medium length"),
                });
                logged += 1;
                if lines.len() > 60 {
                    lines.pop_front();
                    first += 1;
                }
            }
            // Scroll up for a while, then back down, over the log.
            ctx.io_mut().mouse_pos = [menu::initial_width([1280., 720.]) + 200., 200.];
            ctx.io_mut().mouse_wheel = match frame {
                45..=48 => 2.0,
                70..=72 => -3.0,
                _ => 0.0,
            };
            let ui = ctx.frame();
            if visible_only {
                gui.draw_console_with(ui, |gui, ui| gui.draw_log(ui, |f| f(first, &lines)));
            } else {
                gui.draw_console_with(ui, |gui, ui| every_line(gui, ui, &lines));
            }
            frames.push((vertices(&mut ctx), gui.console_lines, first));
        }
        frames
    };
    let (fast, full) = (run(true), run(false));
    let scrolled = full.windows(2).filter(|w| w[0].0 != w[1].0).count();
    assert!(scrolled > 30, "the view changed only {scrolled} times");
    for (frame, (a, b)) in fast.iter().zip(&full).enumerate() {
        assert!(!a.0.is_empty());
        assert_eq!(a, b, "frame {frame}");
    }
}

/// The minimap and frame-time panels build without the game and produce a
/// finite mesh inside the screen, at a small and a large display.
#[test]
fn minimap_and_frame_time_panels_build_without_the_game() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = fonts::test_context(16.);
    for size in [[800., 600.], [2560., 1440.]] {
        ctx.io_mut().display_size = size;
        ctx.io_mut().delta_time = 1. / 60.;
        let ui = ctx.frame();
        modules::minimap::preview(ui);
        modules::trajectories::preview(ui);
        modules::frame_time::preview(ui);
        let data = ctx.render();
        let vertices: Vec<_> = data.draw_lists().flat_map(|l| l.vtx_buffer()).collect();
        // The island's runs of colour, the panels, the arrow and the text.
        assert!(vertices.len() > 1000, "{}", vertices.len());
        assert!(vertices.iter().all(|v| v.pos.iter().all(|x| x.is_finite())
            && (-1. ..=size[0] + 1.).contains(&v.pos[0])
            && (-1. ..=size[1] + 1.).contains(&v.pos[1])));
    }
}
/// The compact card is narrower and shorter than the regular one, and
/// hiding idle features drops their rows (and only theirs).
#[test]
fn compact_card_and_hidden_idle_features_take_less_room() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = Context::create();
    ctx.set_ini_filename(None);
    fonts::initialize(&mut ctx, 16.).expect("Japanese Windows font");
    ctx.fonts().build_alpha8_texture();
    ctx.io_mut().display_size = [2560., 1440.];
    let entries = || {
        [
            ("radar", ""),
            ("fps", "132"),
            ("coordinates", "（ワールド未接続）"),
            ("vitals", "（データなし）"),
            ("inventorymove", "インベントリを開くと有効"),
            ("autofish", "一時停止: メニュー"),
        ]
        .map(|(id, status)| (id, String::from(status)))
    };
    let mut size = |compact: bool, hide_idle: bool| {
        let ui = ctx.frame();
        hud_card::draw(ui, [10, 10], entries(), compact, hide_idle, 1);
        ctx.render();
        crate::hud_editor::measured("hud", [0., 0.])
    };
    let regular = size(false, false);
    let compact = size(true, false);
    let hidden = size(true, true);
    assert!(
        compact[0] < regular[0] && compact[1] < regular[1],
        "{compact:?} {regular:?}"
    );
    assert_eq!(hidden[0], compact[0]);
    // Three rows are only waiting: 19 px each in the vertical compact layout.
    assert!(
        (compact[1] - hidden[1] - 3. * 19.).abs() < 0.5,
        "{compact:?} {hidden:?}"
    );
}
#[test]
#[ignore = "manual: cargo test -p client --release --lib minimap_paint_cost -- --ignored --nocapture"]
fn minimap_paint_cost() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = fonts::test_context(16.);
    ctx.io_mut().display_size = [1920., 1080.];
    ctx.io_mut().delta_time = 1. / 60.;
    modules::minimap::paint_cost(&mut |draw| {
        let start = std::time::Instant::now();
        draw(ctx.frame());
        std::hint::black_box(ctx.render().total_vtx_count);
        start.elapsed()
    });
}
#[test]
fn lighting_tiles_badges_and_legend_build_at_small_and_large_resolutions() {
    let _serial = TEST_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = fonts::test_context(16.);
    for size in [[800., 600.], [2560., 1440.]] {
        ctx.io_mut().display_size = size;
        ctx.io_mut().delta_time = 1. / 60.;
        modules::light_overlay::preview(ctx.frame());
        let data = ctx.render();
        assert!(data.total_vtx_count > 100);
        assert!(data.draw_lists().flat_map(|l| l.vtx_buffer()).all(|v| v
            .pos
            .iter()
            .all(|p| p.is_finite())
            && v.pos[0] >= -size[0]
            && v.pos[0] <= size[0] * 2.
            && v.pos[1] >= -size[1]
            && v.pos[1] <= size[1] * 2.));
    }
}
