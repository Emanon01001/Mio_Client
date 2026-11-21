//! In-game overlay (Dear ImGui via hudhook): a toggleable menu listing every
//! module, plus a small always-on HUD showing which modules are enabled.

mod feature_list;
mod file_dialog;
mod fonts;
pub(crate) use fonts::debug_font;
mod hud_card;
mod menu;
#[cfg(test)]
pub(crate) mod preview;
pub(crate) mod window_layout;
use feature_list::FeatureList;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hudhook::hooks::dx11::ImguiDx11Hooks;
use hudhook::hooks::dx12::ImguiDx12Hooks;
use hudhook::imgui::{Condition, Context, Io, Ui};
use hudhook::windows::Win32::Foundation::HINSTANCE;
use hudhook::{Hudhook, ImguiRenderLoop, MessageFilter, RenderContext};
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ClipCursor, GetClipCursor, GetCursorPos, GetSystemMetrics, SetCursorPos, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::config;
use crate::console;
use crate::input;
use crate::log;
use crate::modules::{self, key_or_warn, Modules};

/// True while the overlay menu and console are visible.
pub static MENU_OPEN: AtomicBool = AtomicBool::new(false);
/// True while the world map is open: it takes the mouse and keyboard like
/// the menu, without the menu's windows.
pub static MAP_OPEN: AtomicBool = AtomicBool::new(false);
/// Set by the menu's Eject button; polled by the main loop.
pub static EJECT_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Set at shutdown: the overlay draws nothing and blocks no input.
pub static DORMANT: AtomicBool = AtomicBool::new(false);
// Keep the closing Esc out of native input until it has been released.
static ESCAPE_CLOSING: AtomicBool = AtomicBool::new(false);
static RENDER_API: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub(crate) fn renderer_name() -> &'static str {
    match RENDER_API.load(Ordering::Relaxed) {
        1 => "DirectX 11",
        2 => "DirectX 12",
        _ => "未取得",
    }
}
fn debug_widget_visible(debug: bool, name: &str) -> bool {
    // F3 replaces the on-screen panels: the HUD category, plus panels the
    // menu files under other categories (it groups by purpose, not by kind).
    let panel = feature_list::category(name) == feature_list::Category::Hud
        || matches!(
            name,
            "waila" | "targethud" | "playeralert" | "minimap" | "worldinfo" | "mobcount"
        );
    !debug || matches!(name, "debughud" | "worldmap" | "playerlist") || !panel
}
#[test]
fn f3_hides_duplicate_hud_but_preserves_world_visuals_and_map_controls() {
    for name in [
        "radar",
        "compass",
        "targethud",
        "minimap",
        "inventoryhud",
        "waila",
        "playeralert",
        "worldinfo",
        "mobcount",
    ] {
        assert!(!debug_widget_visible(true, name));
        assert!(debug_widget_visible(false, name));
    }
    for name in [
        "debughud",
        "worldmap",
        "lightoverlay",
        "trajectories",
        "esp",
    ] {
        assert!(debug_widget_visible(true, name));
    }
}
#[cfg(test)]
pub(crate) static TEST_CONTEXT: Mutex<()> = Mutex::new(());

/// The item images being read from the game's files by `prefetch`.
static ATLAS: Mutex<Option<std::thread::JoinHandle<Option<modules::item_hud::icons::Atlas>>>> =
    Mutex::new(None);

/// Starts reading the item images (about 0.1 s of file reads and PNG
/// decoding) on another thread, so it overlaps with resolving the modules'
/// signatures instead of following it. `start` takes the result.
pub fn prefetch(cfg: &config::Gui) {
    if !cfg.enabled {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("Mio Client item images".into())
        .spawn(modules::item_hud::icons::load_game);
    if let Ok(handle) = spawned {
        *ATLAS.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }
}

/// The prefetched item images, or read them now when nothing was prefetched.
fn item_atlas() -> Option<modules::item_hud::icons::Atlas> {
    let pending = ATLAS.lock().unwrap_or_else(|e| e.into_inner()).take();
    match pending {
        Some(handle) => handle.join().unwrap_or_else(|_| {
            log!("所持品HUDの画像を読み込めません（文字表示を使用します）");
            None
        }),
        None => modules::item_hud::icons::load_game(),
    }
}

/// Shared by Windows message filtering and native mouse/attack callbacks, since the
/// game can receive input through paths that bypass its window procedure.
/// The overlay takes the mouse and keyboard while a menu/map is open, and
/// through release of the Esc that closed it: the game sees no input.
pub(crate) fn captures_input() -> bool {
    (MENU_OPEN.load(Ordering::Acquire)
        || MAP_OPEN.load(Ordering::Acquire)
        || ESCAPE_CLOSING.load(Ordering::Acquire))
        && !DORMANT.load(Ordering::Acquire)
}

/// The menu (and console) is open.
pub(crate) fn menu_open() -> bool {
    MENU_OPEN.load(Ordering::Acquire) && !DORMANT.load(Ordering::Acquire)
}

fn menu_state(open: bool, was_down: bool, down: bool, focused: bool, dormant: bool) -> bool {
    if dormant {
        false
    } else if down && !was_down && focused {
        !open
    } else {
        open
    }
}

fn menu_key_allowed(key: u16, text_input: bool, game_chat: bool) -> bool {
    let editing_key = matches!(key, 0x08..=0x0d | 0x10..=0x14 | 0x20..=0x28 | 0x2e |
        0x30..=0x39 | 0x41..=0x5a | 0x60..=0x6f | 0xa0..=0xa5 | 0xba..=0xe2);
    !editing_key || !(text_input || game_chat)
}

fn escape_press(message: u32, key: usize, flags: isize) -> bool {
    matches!(message, 0x100 | 0x104) && key == 0x1b && flags & (1 << 30) == 0
}

fn escape_closes_menu(open: bool, capturing_key: bool, editing_hud: bool, popup: bool) -> bool {
    open && !capturing_key && !editing_hud && !popup
}

fn popup_open() -> bool {
    // hudhook drains window messages on the render thread with its ImGui
    // context current, before NewFrame dismisses popups in response to Esc.
    // AnyPopup ignores the string ID and checks the whole popup stack.
    unsafe {
        hudhook::imgui::sys::igIsPopupOpen(
            std::ptr::null(),
            hudhook::imgui::sys::ImGuiPopupFlags_AnyPopup as i32,
        )
    }
}

/// The wrapped height of each console log line, for the wrap width and font
/// size they were measured at. `heights[i]` is the line numbered `first + i`
/// (see `console::with_numbered_lines`); lines are only appended at the back
/// and dropped at the front, so only new lines need measuring.
#[derive(Default)]
struct ConsoleView {
    first: u64,
    heights: std::collections::VecDeque<f32>,
    wrap: f32,
    font: f32,
}

impl ConsoleView {
    fn sync(
        &mut self,
        first: u64,
        lines: &std::collections::VecDeque<String>,
        wrap: f32,
        font: f32,
        height: impl Fn(&str) -> f32,
    ) {
        let end = self.first + self.heights.len() as u64;
        if self.wrap != wrap || self.font != font || first < self.first || first > end {
            self.heights.clear();
            (self.first, self.wrap, self.font) = (first, wrap, font);
        }
        while self.first < first {
            self.heights.pop_front();
            self.first += 1;
        }
        self.heights.truncate(lines.len());
        for line in lines.iter().skip(self.heights.len()) {
            self.heights.push_back(height(line));
        }
    }
}

struct Gui {
    modules: Arc<Mutex<Modules>>,
    profiles: Arc<Mutex<crate::profiles::Profiles>>,
    profile_name: String,
    module_filter: String,
    feature_list: FeatureList,
    hud_card: hud_card::Card,
    expanded_details: std::collections::HashSet<usize>,
    setting_error: String,
    menu_key: u16,
    menu_key_name: String,
    eject_key: u16,
    key_was_down: AtomicBool,
    escape_requested: AtomicBool,
    open: bool,
    hud_editor: crate::hud_editor::Editor,
    /// The console's input line, and how many log lines were drawn last frame.
    command: String,
    console_lines: usize,
    /// Wrapped heights of the log lines, so only the visible ones are drawn.
    console_view: ConsoleView,
    /// Put the keyboard focus on the console input on the next frame.
    focus_command: bool,
    font_size: f32,
    item_atlas: Option<modules::item_hud::icons::Atlas>,
    /// While the menu is open: the cursor confinement the game had when it
    /// opened (None: none), put back when it closes.
    cursor_clip: Option<Option<RECT>>,
    /// While the menu is open: the last cursor position over the game.
    cursor_inside: Option<POINT>,
}

impl Gui {
    fn draw_hud(&mut self, ui: &Ui, modules: &Modules) {
        let Some([x, y]) = modules::hud::list_position() else {
            return;
        };
        let (compact, hide_idle, columns) = modules::hud::list_style();
        self.hud_card.column_width = modules::hud::list_width();
        self.hud_card
            .draw(ui, [x, y], modules.hud_lines(), compact, hide_idle, columns);
    }
}

/// The cursor confinement now in effect (`GetClipCursor`), or None when the
/// cursor may move over the whole virtual screen.
fn current_clip() -> Option<RECT> {
    unsafe {
        let mut clip: RECT = std::mem::zeroed();
        if GetClipCursor(&mut clip) == 0 {
            return None;
        }
        let (left, top) = (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        );
        let whole = RECT {
            left,
            top,
            right: left + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: top + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        };
        let covers = clip.left <= whole.left
            && clip.top <= whole.top
            && clip.right >= whole.right
            && clip.bottom >= whole.bottom;
        (!covers).then_some(clip)
    }
}

impl Gui {
    /// While the menu is open the cursor moves freely over the game (the
    /// game may confine it to a smaller area during play, so the confinement
    /// is lifted) but is brought back when it leaves the game's windows. When
    /// the menu closes, the confinement the game had when it opened is put
    /// back, so the cursor does not wander off the game during play. Nothing
    /// is done while another application is in front.
    ///
    /// Leaving is told by the window under the cursor, not by a rectangle, so
    /// it does not depend on the window's coordinates (DPI scaling, borders).
    fn confine_cursor(&mut self, open: bool) {
        if open {
            self.cursor_clip.get_or_insert_with(current_clip);
            if !input::is_foreground() {
                return;
            }
            unsafe {
                ClipCursor(std::ptr::null());
                let mut at = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut at) == 0 {
                    return;
                }
                if input::game_at(at) {
                    self.cursor_inside = Some(at);
                } else if let Some(inside) = self.cursor_inside {
                    SetCursorPos(inside.x, inside.y);
                }
            }
        } else if let Some(saved) = self.cursor_clip.take() {
            self.cursor_inside = None;
            if input::is_foreground() {
                unsafe { ClipCursor(saved.as_ref().map_or(std::ptr::null(), |r| r)) };
            }
        }
    }

    /// The in-game console: the log, and a line to type commands into.
    fn draw_console(&mut self, ui: &Ui) {
        self.draw_console_with(ui, |gui, ui| {
            gui.draw_log(ui, |f| {
                console::with_numbered_lines(|first, lines| f(first, lines))
            })
        });
    }

    /// The log lines inside the console's scrolling child window. Only the
    /// lines in view are laid out and drawn: the wrapped height of every line
    /// is kept (`ConsoleView`), and the lines above and below the view are
    /// stood in for by empty space of the same height, so positions, content
    /// size and scrolling are those of drawing every line. The log's lock is
    /// held only to bring the heights up to date and copy the visible lines.
    ///
    /// `lines` calls its argument once with the log (see
    /// `console::with_numbered_lines`); tests pass their own.
    fn draw_log(
        &mut self,
        ui: &Ui,
        lines: impl FnOnce(&mut dyn FnMut(u64, &std::collections::VecDeque<String>)),
    ) {
        let spacing = ui.clone_style().item_spacing[1];
        let wrap = ui.content_region_avail()[0].max(1.0);
        let font = ui.current_font_size();
        let start = ui.cursor_pos()[1];
        let (top, bottom) = (ui.scroll_y(), ui.scroll_y() + ui.window_size()[1]);
        let view = &mut self.console_view;
        let (mut count, mut above, mut visible, mut below) = (0, 0.0, Vec::new(), 0.0);
        lines(&mut |first, lines| {
            view.sync(first, lines, wrap, font, |line| {
                ui.calc_text_size_with_opts(line, false, wrap)[1]
            });
            let mut y = start;
            for (line, &height) in lines.iter().zip(&view.heights) {
                if y + height < top {
                    above += height + spacing;
                } else if y > bottom {
                    below += height + spacing;
                } else {
                    visible.push(line.clone());
                }
                y += height + spacing;
            }
            count = lines.len();
        });
        // A dummy of height h advances the cursor by h + spacing.
        if above > 0.0 {
            ui.dummy([0.0, (above - spacing).max(0.0)]);
        }
        for line in &visible {
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
        if below > 0.0 {
            ui.dummy([0.0, (below - spacing).max(0.0)]);
        }
        // Follow new output. The count stops changing once the
        // history is full, so also follow while at the bottom.
        if count != self.console_lines || ui.scroll_y() >= ui.scroll_max_y() {
            ui.set_scroll_here_y_with_ratio(1.0);
        }
        self.console_lines = count;
    }

    fn draw_console_with(&mut self, ui: &Ui, log: impl FnOnce(&mut Self, &Ui)) {
        use window_layout::{Rect, Window};
        let screen = ui.io().display_size;
        let x = menu::initial_width(screen) + 60.;
        // Beside the module menu, which grows with the module list; or where
        // it was left last time.
        let initial = window_layout::initial(
            Window::Console,
            screen,
            Rect {
                position: [x, 40.0],
                size: [(screen[0] - x - 20.).clamp(300., 720.), 400.],
            },
            [200., 150.],
        );
        ui.window("Mio · コンソール###console")
            .position(initial.position, Condition::FirstUseEver)
            .size(initial.size, Condition::FirstUseEver)
            .save_settings(false)
            .build(|| {
                window_layout::observe(Window::Console, ui.window_pos(), ui.window_size());
                if ui.button(crate::localization::text("Copy log")) {
                    ui.set_clipboard_text(console::text());
                }
                if menu::inline_fits(ui, "Ctrl+A / C / X / V") {
                    ui.same_line();
                }
                ui.text_disabled("Ctrl+A / C / X / V");
                ui.separator();
                let input_height = ui.frame_height_with_spacing();
                ui.child_window("##log")
                    .size([0.0, -input_height])
                    .build(|| log(self, ui));

                if self.focus_command {
                    ui.set_keyboard_focus_here();
                    self.focus_command = false;
                }
                ui.set_next_item_width(-1.0);
                let entered = ui
                    .input_text("##command", &mut self.command)
                    .hint(crate::localization::text("command (try `help`)"))
                    .enter_returns_true(true)
                    .build();
                if entered {
                    console::submit(&self.command);
                    self.command.clear();
                    // Enter takes the focus away; keep typing possible.
                    self.focus_command = true;
                }
            });
    }
}

impl ImguiRenderLoop for Gui {
    fn before_wnd_proc(
        &self,
        _hwnd: hudhook::windows::Win32::Foundation::HWND,
        message: u32,
        wparam: hudhook::windows::Win32::Foundation::WPARAM,
        lparam: hudhook::windows::Win32::Foundation::LPARAM,
    ) -> hudhook::BeforeWndProc {
        modules::settings::key_capture::message(message, wparam.0, lparam.0);
        if escape_press(message, wparam.0, lparam.0)
            && escape_closes_menu(
                menu_open(),
                modules::settings::key_capture::blocked(),
                self.hud_editor.active,
                popup_open(),
            )
        {
            self.escape_requested.store(true, Ordering::Release);
        }
        hudhook::BeforeWndProc::Continue
    }
    fn initialize<'a>(&'a mut self, ctx: &mut Context, rc: &'a mut dyn RenderContext) {
        // Do not drop an imgui.ini into the game's working directory.
        ctx.set_ini_filename(None);
        ctx.set_clipboard_backend(crate::clipboard::Clipboard);
        fonts::initialize(ctx, self.font_size);
        fonts::apply_style(ctx);
        modules::item_hud::icons::reset();
        if let Some(atlas) = self
            .item_atlas
            .take()
            .or_else(modules::item_hud::icons::load_game)
        {
            modules::item_hud::icons::upload(atlas, rc);
        }
    }

    fn before_render<'a>(&'a mut self, ctx: &mut Context, _rc: &'a mut dyn RenderContext) {
        // The game hides the system cursor during play, so draw our own.
        self.open = menu_open();
        ctx.io_mut().mouse_draw_cursor = captures_input();
    }

    fn render(&mut self, ui: &mut Ui) {
        if DORMANT.load(Ordering::Relaxed) {
            modules::settings::key_capture::cancel();
            self.open = false;
            self.confine_cursor(false);
            MENU_OPEN.store(false, Ordering::Relaxed);
            return;
        }

        use modules::performance_profiler::metrics::{Scope, Stage};
        let _overlay_profile = Scope::new(Stage::Overlay);

        self.open = menu_open();
        if !self.open {
            modules::settings::key_capture::cancel();
            self.hud_editor.close();
        }

        self.confine_cursor(captures_input());

        let shared = Arc::clone(&self.modules);
        let lock_profile = Scope::new(Stage::ModuleLock);
        let mut modules = shared.lock().unwrap_or_else(|e| e.into_inner());
        drop(lock_profile);
        let debug = modules
            .find("debughud")
            .is_some_and(|i| modules.menu_entry(i).1);

        modules::hud::frame(ui.io().framerate);
        if !debug {
            {
                let _scope = Scope::new(Stage::CompassDraw);
                modules::compass::draw(ui);
            }
            {
                let _scope = Scope::new(Stage::KeysDraw);
                modules::keystrokes::draw(ui);
            }
        }
        {
            let _scope = Scope::new(Stage::ChunksDraw);
            modules::chunk_border::draw(ui);
        }
        {
            let _scope = Scope::new(Stage::EspDraw);
            modules::esp::draw(ui);
        }
        {
            let _scope = Scope::new(Stage::BlockDraw);
            modules::block_esp::draw(ui);
        }
        {
            let _scope = Scope::new(Stage::WaypointsDraw);
            modules::waypoints::draw(ui);
        }
        if debug {
            modules.draw_widgets_filtered(ui, |name| debug_widget_visible(true, name));
        } else {
            modules.draw_widgets(ui);
        }
        if !debug {
            let _scope = Scope::new(Stage::HudDraw);
            self.draw_hud(ui, &modules);
        }
        if self.open && self.hud_editor.active {
            let _scope = Scope::new(Stage::MenuDraw);
            let profile_status = self
                .profiles
                .try_lock()
                .map(|p| p.status.clone())
                .unwrap_or_default();
            self.hud_editor
                .draw(ui, &mut modules, &mut self.profile_name, &profile_status);
        } else if self.open {
            let _scope = Scope::new(Stage::MenuDraw);
            self.draw_menu(ui, &mut modules);
        }
        drop(modules);
        if self.open && !self.hud_editor.active {
            let _scope = Scope::new(Stage::ConsoleDraw);
            self.draw_console(ui);
        }
        {
            let _scope = Scope::new(Stage::NotificationsDraw);
            crate::notify::draw(ui);
        }
    }

    fn message_filter(&self, io: &Io) -> MessageFilter {
        // hudhook calls this before before_render/render: update capture here,
        // so its window filter and native mouse gate see the same frame.
        modules::settings::key_capture::refresh();
        if !input::key_down(0x1b) {
            ESCAPE_CLOSING.store(false, Ordering::Release);
        }
        let close_escape =
            self.escape_requested.swap(false, Ordering::AcqRel) && input::is_foreground();
        if close_escape {
            ESCAPE_CLOSING.store(true, Ordering::Release);
        }
        let down = input::key_down(self.menu_key);
        let was = self.key_was_down.swap(down, Ordering::AcqRel);
        let key_allowed = menu_key_allowed(
            self.menu_key,
            io.want_text_input,
            modules::game::inventory::chat_open(unsafe {
                windows_sys::Win32::System::SystemInformation::GetTickCount64()
            }),
        );
        let open = MENU_OPEN.load(Ordering::Acquire);
        MENU_OPEN.store(
            !close_escape
                && menu_state(
                    open,
                    was,
                    down,
                    input::is_foreground()
                    && key_allowed
                    && !modules::settings::key_capture::blocked()
                    // An explicitly assigned Esc can open the menu. Closing
                    // it still respects popup, key capture and HUD priorities.
                    && !(self.menu_key == 0x1b && open)
                    && !ESCAPE_CLOSING.load(Ordering::Acquire),
                    DORMANT.load(Ordering::Acquire),
                ),
            Ordering::Release,
        );
        if captures_input() {
            // Keep clicks and keystrokes out of the game while the menu is up.
            MessageFilter::InputAll
        } else {
            MessageFilter::empty()
        }
    }
}

fn module_loaded(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    !unsafe { GetModuleHandleW(wide.as_ptr()) }.is_null()
}

/// Installs the render hooks. Returns true if the overlay is active (in which
/// case unloading the DLL must go through `hudhook::eject`).
pub fn start(
    hmodule: HMODULE,
    cfg: &config::Gui,
    eject_key: u16,
    modules: Arc<Mutex<Modules>>,
    profiles: Arc<Mutex<crate::profiles::Profiles>>,
) -> bool {
    if !cfg.enabled {
        return false;
    }
    crate::notify::set_enabled(cfg.notifications);
    let dx12 = match cfg.renderer.to_ascii_lowercase().as_str() {
        "dx12" => true,
        "dx11" => false,
        "auto" => module_loaded("d3d12.dll"),
        other => {
            log!("gui.rendererの`{other}`は無効です。autoを使います");
            module_loaded("d3d12.dll")
        }
    };
    let configured_key = key_or_warn("gui.menu_key", &cfg.menu_key);
    let menu_key = if config::menu_key::validate(configured_key, eject_key).is_ok() {
        configured_key
    } else if eject_key != 0x4c {
        0x4c
    } else {
        0x2d
    };
    let gui = Gui {
        modules,
        profiles,
        profile_name: "default".into(),
        module_filter: String::new(),
        feature_list: FeatureList::default(),
        hud_card: hud_card::Card::default(),
        expanded_details: Default::default(),
        setting_error: String::new(),
        menu_key,
        menu_key_name: input::display_key_name(menu_key),
        eject_key,
        key_was_down: AtomicBool::new(false),
        escape_requested: AtomicBool::new(false),
        open: false,
        hud_editor: crate::hud_editor::Editor::default(),
        command: String::new(),
        console_lines: 0,
        console_view: ConsoleView::default(),
        focus_command: false,
        font_size: cfg.font_size,
        item_atlas: item_atlas(),
        cursor_clip: None,
        cursor_inside: None,
    };

    let hmodule = hmodule as usize;
    // hudhook panics on some setup failures; never let that reach the game.
    let result = std::panic::catch_unwind(move || {
        let builder = Hudhook::builder();
        let builder = if dx12 {
            builder.with::<ImguiDx12Hooks>(gui)
        } else {
            builder.with::<ImguiDx11Hooks>(gui)
        };
        builder
            .with_hmodule(HINSTANCE(hmodule as _))
            .build()
            .apply()
    });

    let api = if dx12 { "DX12" } else { "DX11" };
    match result {
        Ok(Ok(())) => {
            RENDER_API.store(if dx12 { 2 } else { 1 }, Ordering::Relaxed);
            log!(
                "オーバーレイを開始しました（{api}）。{}でメニューを開きます",
                input::display_key_name(menu_key)
            );
            true
        }
        Ok(Err(e)) => {
            log!("オーバーレイの開始に失敗（{api}）: {e:?}。別ウィンドウのコンソールを使います");
            false
        }
        Err(_) => {
            log!("オーバーレイの初期化中にエラー（{api}）。別ウィンドウのコンソールを使います");
            false
        }
    }
}
