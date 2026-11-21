//! Read-only HUD readouts: each is a module whose status line shows one value
//! in the overlay's module list (fps, cps, speed, direction, clock, memory,
//! entities, server). Nothing in the game is modified.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use super::settings::{Detail, Value};
use super::{game, key_or_warn, Module};
use crate::text::clock_hms;
use crate::{config, input, log, net};

static LIST_ACTIVE: AtomicBool = AtomicBool::new(false);
static LIST_X: AtomicI32 = AtomicI32::new(10);
static LIST_Y: AtomicI32 = AtomicI32::new(10);
static LIST_COMPACT: AtomicBool = AtomicBool::new(true);
static LIST_HIDE_IDLE: AtomicBool = AtomicBool::new(true);
/// Columns of the list; 0 = automatic.
static LIST_COLUMNS: AtomicU32 = AtomicU32::new(0);
static LIST_VERTICAL: AtomicBool = AtomicBool::new(true);
static LIST_WIDTH: AtomicU32 = AtomicU32::new(360);

pub fn list_width() -> f32 {
    LIST_WIDTH.load(Ordering::Relaxed) as f32
}

/// How the list is drawn: (compact, hide features that are only waiting,
/// columns with 0 for automatic).
pub fn list_style() -> (bool, bool, usize) {
    (
        LIST_COMPACT.load(Ordering::Relaxed),
        LIST_HIDE_IDLE.load(Ordering::Relaxed),
        if LIST_VERTICAL.load(Ordering::Relaxed) {
            1
        } else {
            LIST_COLUMNS.load(Ordering::Relaxed) as usize
        },
    )
}

pub fn list_position() -> Option<[i32; 2]> {
    LIST_ACTIVE.load(Ordering::Relaxed).then(|| {
        [
            LIST_X.load(Ordering::Relaxed),
            LIST_Y.load(Ordering::Relaxed),
        ]
    })
}
/// The grouped status list has the same profile-backed position controls as panels.
pub struct Layout;
impl Layout {
    pub fn new(c: &config::Gui) -> Self {
        LIST_ACTIVE.store(false, Ordering::Relaxed);
        LIST_X.store(c.hud_x.clamp(-7680, 7680), Ordering::Relaxed);
        LIST_Y.store(c.hud_y.clamp(-4320, 4320), Ordering::Relaxed);
        LIST_COMPACT.store(c.hud_compact, Ordering::Relaxed);
        LIST_HIDE_IDLE.store(c.hud_hide_idle, Ordering::Relaxed);
        LIST_COLUMNS.store(c.hud_columns.min(3), Ordering::Relaxed);
        LIST_VERTICAL.store(c.hud_vertical, Ordering::Relaxed);
        LIST_WIDTH.store(c.hud_width.clamp(200, 800), Ordering::Relaxed);
        Self
    }
}
impl Module for Layout {
    fn name(&self) -> &str {
        "hud"
    }
    fn key(&self) -> u16 {
        0
    }
    fn enabled(&self) -> bool {
        LIST_ACTIVE.load(Ordering::Relaxed)
    }
    fn set_enabled(&mut self, on: bool) {
        LIST_ACTIVE.store(on, Ordering::Relaxed);
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "compact",
                "コンパクト表示",
                "行の高さ・余白を詰めます。機能名の幅を確保し、長い状態文は折り返します。",
                LIST_COMPACT.load(Ordering::Relaxed),
            ),
            Detail::boolean(
                "hide_idle",
                "待機中の機能を隠す",
                "「未接続」「データ待ち」「待機中」の機能を一覧に出しません。ヘッダーの「待機」はこの非表示件数です。",
                LIST_HIDE_IDLE.load(Ordering::Relaxed),
            ),
            Detail::integer(
                "columns",
                "列の数",
                "0で自動（折り返し後の高さを基準に最大3列へ分けます）。1〜3で指定できます。画面に収まる列数が上限です。",
                LIST_COLUMNS.load(Ordering::Relaxed) as i32,
                0,
                3,
            ),
            Detail::boolean(
                "vertical",
                "縦1列表示",
                "左側に収めやすい縦1列の常時表示です。半分の幅に収まる短い行（FPS・時計など）は2つずつ横に並べて高さを節約し、それでも足りないときだけ文字サイズを調整して全項目を表示します。スクロールは使いません。列の数の指定より優先します。",
                LIST_VERTICAL.load(Ordering::Relaxed),
            ),
            Detail::integer("width", "縦表示の幅（px）", "長い状態文を折り返す幅です。横に余裕を持たせると、文字サイズを保って表示しやすくなります。", LIST_WIDTH.load(Ordering::Relaxed) as i32, 200, 800),
            Detail::integer(
                "x",
                "X offset",
                "Negative: from right.",
                LIST_X.load(Ordering::Relaxed),
                -7680,
                7680,
            ),
            Detail::integer(
                "y",
                "Y offset",
                "Negative: from bottom.",
                LIST_Y.load(Ordering::Relaxed),
                -4320,
                4320,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "x" => LIST_X.store(v.number()? as i32, Ordering::Relaxed),
            "y" => LIST_Y.store(v.number()? as i32, Ordering::Relaxed),
            "compact" => LIST_COMPACT.store(v.boolean()?, Ordering::Relaxed),
            "hide_idle" => LIST_HIDE_IDLE.store(v.boolean()?, Ordering::Relaxed),
            "columns" => LIST_COLUMNS.store(v.number()?.clamp(0., 3.) as u32, Ordering::Relaxed),
            "vertical" => LIST_VERTICAL.store(v.boolean()?, Ordering::Relaxed),
            "width" => LIST_WIDTH.store(v.number()?.clamp(200., 800.) as u32, Ordering::Relaxed),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
}

const VK_LBUTTON: u16 = 0x01;
const VK_RBUTTON: u16 = 0x02;
/// Clicks are counted over this window (ms).
const CPS_WINDOW_MS: u64 = 1000;
/// How often the process memory figure is refreshed (ms).
const MEMORY_REFRESH_MS: u64 = 500;

// --- Frame rate ------------------------------------------------------------------

/// f32 bits; the overlay's smoothed frames per second.
static FRAMERATE: AtomicU32 = AtomicU32::new(0);

/// Called by the overlay once per rendered frame.
pub fn frame(framerate: f32) {
    FRAMERATE.store(framerate.to_bits(), Ordering::Relaxed);
}

// --- Clicks ----------------------------------------------------------------------

/// One mouse button: its last state and when it was recently pressed.
struct Button {
    down: bool,
    presses: VecDeque<u64>,
}

impl Button {
    const fn new() -> Self {
        Self {
            down: false,
            presses: VecDeque::new(),
        }
    }

    /// Records a press on the up -> down edge and forgets old presses.
    fn update(&mut self, down: bool, now_ms: u64) {
        if down && !self.down {
            self.presses.push_back(now_ms);
        }
        self.down = down;
        while self
            .presses
            .front()
            .is_some_and(|&at| now_ms.saturating_sub(at) >= CPS_WINDOW_MS)
        {
            self.presses.pop_front();
        }
    }
}

static BUTTONS: Mutex<(Button, Button)> = Mutex::new((Button::new(), Button::new()));

/// Samples the physical mouse buttons. Called from the main loop every
/// iteration; presses made while the game is not focused are not counted.
pub fn poll(focused: bool, now_ms: u64) {
    let mut buttons = BUTTONS.lock().unwrap_or_else(|e| e.into_inner());
    buttons
        .0
        .update(focused && input::key_down(VK_LBUTTON), now_ms);
    buttons
        .1
        .update(focused && input::key_down(VK_RBUTTON), now_ms);
}

/// Physical (left, right) clicks in the last second.
pub fn cps() -> (usize, usize) {
    let buttons = BUTTONS.lock().unwrap_or_else(|e| e.into_inner());
    (buttons.0.presses.len(), buttons.1.presses.len())
}

// --- Readouts --------------------------------------------------------------------

/// The 8-point compass name for a Minecraft yaw (0 = south/+Z, 90 = west/-X).
fn compass(yaw: f32) -> &'static str {
    const NAMES: [&str; 8] = ["S", "SW", "W", "NW", "N", "NE", "E", "SE"];
    let sector = (yaw.rem_euclid(360.0) / 45.0).round() as usize % 8;
    NAMES[sector]
}

fn fps_text() -> String {
    format!("{:.0}", f32::from_bits(FRAMERATE.load(Ordering::Relaxed)))
}

fn cps_text() -> String {
    let (left, right) = cps();
    format!("{left} | {right}")
}

fn player() -> Option<mcbe_sdk::Player> {
    crate::api::game().ok()?.player().ok()
}

fn speed_text() -> String {
    match player() {
        Some(p) => format!("{:.2}ブロック/秒", p.horizontal_speed()),
        None => "（ワールド未接続）".into(),
    }
}

fn direction_text() -> String {
    match player() {
        Some(p) => format!(
            "{}  水平角 {:.1}  垂直角 {:.1}",
            compass(p.rotation.yaw),
            p.rotation.yaw,
            p.rotation.pitch
        ),
        None => "（ワールド未接続）".into(),
    }
}

pub(super) fn memory_text() -> String {
    static AT: AtomicU64 = AtomicU64::new(0);
    static MEGABYTES: AtomicU64 = AtomicU64::new(0);
    let now = unsafe { GetTickCount64() };
    if now.saturating_sub(AT.load(Ordering::Relaxed)) >= MEMORY_REFRESH_MS {
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } != 0 {
            MEGABYTES.store(counters.WorkingSetSize as u64 >> 20, Ordering::Relaxed);
        }
        AT.store(now, Ordering::Relaxed);
    }
    format!("{} MB", MEGABYTES.load(Ordering::Relaxed))
}

fn entities_text() -> String {
    match game::box_count() {
        Some(count) => count.to_string(),
        None => "（ワールド未接続）".into(),
    }
}

fn server_text() -> String {
    net::current().unwrap_or_else(|| "(none)".into())
}

fn dimension_text() -> String {
    match game::world::dimension() {
        Some((name, _)) => name,
        None => "（ワールド未接続）".into(),
    }
}

/// GetTickCount64 when the stopwatch was last switched on.
static STOPWATCH_START: AtomicU64 = AtomicU64::new(0);

/// `m:ss.t` below an hour, `h:mm:ss` from then on.
fn format_elapsed(ms: u64) -> String {
    let (hours, minutes, seconds) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}.{}", ms / 100 % 10)
    }
}

fn stopwatch_text() -> String {
    let start = STOPWATCH_START.load(Ordering::Relaxed);
    format_elapsed(unsafe { GetTickCount64() }.saturating_sub(start))
}

/// Switching the stopwatch on starts it from zero.
fn stopwatch_toggle(on: bool) -> Result<(), String> {
    if on {
        STOPWATCH_START.store(unsafe { GetTickCount64() }, Ordering::Relaxed);
    }
    Ok(())
}

/// Something a readout needs done when it is switched on or off.
type Toggle = fn(bool) -> Result<(), String>;

/// One readout: a named module whose status is a value computed on demand.
pub struct Info {
    name: &'static str,
    key: u16,
    enabled: bool,
    text: fn() -> String,
    /// Run on enable/disable; a failure leaves the readout as it was.
    toggle: Option<Toggle>,
}

impl Info {
    fn new(
        name: &'static str,
        cfg: &config::Toggle,
        text: fn() -> String,
        toggle: Option<Toggle>,
    ) -> Self {
        Self {
            name,
            key: key_or_warn(name, &cfg.key),
            enabled: false,
            text,
            toggle,
        }
    }
}

impl Module for Info {
    fn name(&self) -> &str {
        self.name
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
        if let Some(toggle) = self.toggle {
            if let Err(e) = toggle(on) {
                log!("{}: {e}", self.name);
                return;
            }
        }
        self.enabled = on;
    }

    fn status(&self) -> String {
        if self.enabled {
            (self.text)()
        } else {
            String::new()
        }
    }
}

/// Every readout with its configured initial state, in display order.
pub fn modules(config: &config::Config) -> Vec<(Info, bool)> {
    // The entity count needs the entity snapshot kept running while shown.
    let scene: Toggle = game::use_scene;
    type Readout<'a> = (
        &'static str,
        &'a config::Toggle,
        fn() -> String,
        Option<Toggle>,
    );
    let table: [Readout<'_>; 10] = [
        ("fps", &config.fps, fps_text, None),
        ("cps", &config.cps, cps_text, None),
        ("speed", &config.speed, speed_text, None),
        ("direction", &config.direction, direction_text, None),
        ("dimension", &config.dimension, dimension_text, None),
        ("clock", &config.clock, clock_hms, None),
        (
            "stopwatch",
            &config.stopwatch,
            stopwatch_text,
            Some(stopwatch_toggle),
        ),
        ("memory", &config.memory, memory_text, None),
        ("entities", &config.entities, entities_text, Some(scene)),
        ("server", &config.server, server_text, None),
    ];
    table
        .into_iter()
        .map(|(name, cfg, text, toggle)| (Info::new(name, cfg, text, toggle), cfg.enabled))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vertical_mode_preserves_horizontal_preference_and_old_defaults() {
        let _serial = game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old: config::Config = toml::from_str("").unwrap();
        let shipped: config::Config =
            toml::from_str(include_str!("../../default_config.toml")).unwrap();
        assert!(old.gui.hud_vertical && shipped.gui.hud_vertical);
        assert_eq!(old.gui.hud_width, shipped.gui.hud_width);
        let mut layout = Layout::new(&old.gui);
        layout.set_option("columns", &Value::Number(3.)).unwrap();
        layout.set_option("vertical", &Value::Bool(true)).unwrap();
        assert_eq!(list_style().2, 1);
        layout.set_option("vertical", &Value::Bool(false)).unwrap();
        assert_eq!(list_style().2, 3);
        Layout::new(&old.gui);
    }

    #[test]
    fn compass_follows_minecraft_yaw() {
        assert_eq!(compass(0.0), "S");
        assert_eq!(compass(90.0), "W");
        assert_eq!(compass(180.0), "N");
        assert_eq!(compass(-180.0), "N");
        assert_eq!(compass(-90.0), "E");
        assert_eq!(compass(45.0), "SW");
        assert_eq!(compass(-45.0), "SE");
        assert_eq!(compass(135.0), "NW");
        assert_eq!(compass(-135.0), "NE");
        // Sector boundaries and wrap-around.
        assert_eq!(compass(22.0), "S");
        assert_eq!(compass(23.0), "SW");
        assert_eq!(compass(359.0), "S");
        assert_eq!(compass(720.0 + 90.0), "W");
    }

    #[test]
    fn stopwatch_shows_tenths_then_switches_to_hours() {
        assert_eq!(format_elapsed(0), "0:00.0");
        assert_eq!(format_elapsed(950), "0:00.9");
        assert_eq!(format_elapsed(61_234), "1:01.2");
        assert_eq!(format_elapsed(59 * 60_000 + 59_999), "59:59.9");
        assert_eq!(format_elapsed(3_600_000), "1:00:00");
        assert_eq!(
            format_elapsed(3_600_000 * 12 + 5 * 60_000 + 7_000),
            "12:05:07"
        );
        // Switching it on restarts from (about) zero.
        stopwatch_toggle(true).unwrap();
        assert!(
            stopwatch_text().starts_with("0:00."),
            "{}",
            stopwatch_text()
        );
        assert_eq!(dimension_text(), "（ワールド未接続）");
    }

    #[test]
    fn button_counts_presses_not_holds_and_expires_them() {
        let mut button = Button::new();
        button.update(true, 0); // press
        button.update(true, 10); // still held: not another click
        button.update(false, 20);
        button.update(true, 30); // second press
        assert_eq!(button.presses.len(), 2);
        button.update(false, 999);
        assert_eq!(button.presses.len(), 2);
        button.update(false, 1000); // the press at 0 is now a second old
        assert_eq!(button.presses.len(), 1);
        button.update(false, 1030);
        assert_eq!(button.presses.len(), 0);
    }
}
