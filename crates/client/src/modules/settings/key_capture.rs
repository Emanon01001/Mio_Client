//! Physical key edges for one ImGui key editor. No game input is generated.
use crate::input;
use hudhook::imgui::{Id, Ui};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Mutex;

static BLOCKING: AtomicBool = AtomicBool::new(false);
static LISTENING: AtomicBool = AtomicBool::new(false);
static CAPTURE: Mutex<Option<(Id, Edges)>> = Mutex::new(None);
static EVENT_KEY: AtomicU16 = AtomicU16::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Key(u16),
    Cancel,
}
fn choice(key: u16) -> Choice {
    match key {
        0x1b => Choice::Cancel,
        0x08 => Choice::Key(0),
        key => Choice::Key(key),
    }
}
fn event_key(message: u32, wparam: usize, lparam: isize) -> Option<u16> {
    match message {
        // WM_KEYDOWN / WM_SYSKEYDOWN: bit 30 is an autorepeat; bit 24
        // distinguishes right Ctrl/Alt. Shift uses the scan code instead.
        0x100 | 0x104 if (lparam as usize) & (1 << 30) == 0 => {
            let vk = u16::try_from(wparam)
                .ok()
                .filter(|k| (1..=255).contains(k))?;
            let extended = (lparam as usize) & (1 << 24) != 0;
            Some(match vk {
                0x10 => {
                    if (lparam as usize >> 16) & 255 == 0x36 {
                        0xa1
                    } else {
                        0xa0
                    }
                }
                0x11 => {
                    if extended {
                        0xa3
                    } else {
                        0xa2
                    }
                }
                0x12 => {
                    if extended {
                        0xa5
                    } else {
                        0xa4
                    }
                }
                k => k,
            })
        }
        0x20b => match (wparam >> 16) & 0xffff {
            // WM_XBUTTONDOWN
            1 => Some(5),
            2 => Some(6),
            _ => None,
        },
        _ => None,
    }
}
/// Called before message filtering, so a quick press+release between frames
/// is retained without forwarding it to Minecraft.
pub(crate) fn message(message: u32, wparam: usize, lparam: isize) {
    if LISTENING.load(Ordering::Acquire) {
        if let Some(key) = event_key(message, wparam, lparam) {
            let _ = EVENT_KEY.compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}
struct Edges {
    previous: [bool; 256],
}
impl Edges {
    fn new(previous: [bool; 256]) -> Self {
        Self { previous }
    }
    fn update(&mut self, down: [bool; 256]) -> Option<Choice> {
        let pressed = |vk: usize| down[vk] && !self.previous[vk];
        let choice = if pressed(0x1b) {
            Some(Choice::Cancel)
        } else if pressed(0x08) {
            Some(Choice::Key(0))
        } else {
            (1..=255)
                .filter(|vk| !matches!(vk, 1 | 2 | 4 | 0x10 | 0x11 | 0x12))
                .find(|&vk| pressed(vk))
                .map(|vk| Choice::Key(vk as u16))
        };
        self.previous = down;
        choice
    }
}
/// IME mode indicators of Japanese keyboards (英数, カタカナ, ひらがな, 半角,
/// 全角, ローマ字): GetAsyncKeyState reports the current mode as held all
/// the time, which would keep hotkeys blocked forever after a key capture.
/// They are not keys being pressed, so they are never waited for or chosen.
fn ime_state(vk: usize) -> bool {
    matches!(vk, 0xF0..=0xF6)
}
fn held(down: impl Fn(u16) -> bool) -> [bool; 256] {
    std::array::from_fn(|vk| !ime_state(vk) && down(vk as u16))
}
fn keys() -> [bool; 256] {
    held(input::key_down)
}
pub(crate) fn blocked() -> bool {
    BLOCKING.load(Ordering::Acquire)
}
/// Keep hotkeys blocked through release, including the client's eject key.
pub(crate) fn refresh() {
    if blocked() && !LISTENING.load(Ordering::Acquire) && !keys().iter().any(|d| *d) {
        BLOCKING.store(false, Ordering::Release);
    }
}
pub(crate) fn poll(focused: bool) {
    if !focused && LISTENING.load(Ordering::Acquire) {
        cancel();
    }
    refresh();
}
pub(crate) fn cancel() {
    if !LISTENING.swap(false, Ordering::AcqRel) {
        refresh();
        return;
    }
    *CAPTURE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    EVENT_KEY.store(0, Ordering::Release);
    refresh();
}
pub(super) fn control(ui: &Ui) -> Option<u16> {
    let owner = ui.new_id_str("##capture_owner");
    if ui.button("キーを押して設定") {
        *CAPTURE.lock().unwrap_or_else(|e| e.into_inner()) = Some((owner, Edges::new(keys())));
        EVENT_KEY.store(0, Ordering::Release);
        BLOCKING.store(true, Ordering::Release);
        LISTENING.store(true, Ordering::Release);
        ui.open_popup("キーの入力待ち");
    }
    let mut selected = None;
    ui.modal_popup_config("キーの入力待ち")
        .always_auto_resize(true)
        .resizable(false)
        .collapsible(false)
        .save_settings(false)
        .build(|| {
            ui.text("割り当てたいキーを押してください。");
            ui.text_disabled("Esc: キャンセル / Backspace: 割り当てを解除");
            ui.text_disabled("左右のShift・Ctrl・Alt、MOUSE4・MOUSE5にも対応します。");
            let choice = {
                let mut guard = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
                match guard.as_mut().filter(|(id, _)| *id == owner) {
                    Some((_, edges)) if input::is_foreground() => {
                        match EVENT_KEY.swap(0, Ordering::AcqRel) {
                            0 => edges.update(keys()),
                            key => Some(choice(key)),
                        }
                    }
                    Some(_) => Some(Choice::Cancel),
                    None => Some(Choice::Cancel),
                }
            };
            if choice.is_some() || ui.button("キャンセル") {
                if let Some(Choice::Key(key)) = choice {
                    selected = Some(key);
                }
                cancel();
                ui.close_current_popup();
            }
        });
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_keyboard_events_keep_sides_and_ignore_autorepeat_and_keyup() {
        assert_eq!(event_key(0x100, 0x75, 0), Some(0x75));
        assert_eq!(event_key(0x100, 0x75, 1 << 30), None);
        assert_eq!(event_key(0x101, 0x75, 0), None);
        assert_eq!(event_key(0x100, 0x10, 0x36 << 16), Some(0xa1));
        assert_eq!(event_key(0x100, 0x10, 0x2a << 16), Some(0xa0));
        assert_eq!(event_key(0x104, 0x12, 1 << 24), Some(0xa5));
        assert_eq!(event_key(0x100, 0x11, 0), Some(0xa2));
        assert_eq!(event_key(0x100, 0x11, 1 << 24), Some(0xa3));
        assert_eq!(event_key(0x20b, 1 << 16, 0), Some(5));
        assert_eq!(event_key(0x20b, 2 << 16, 0), Some(6));
    }
    fn down(keys: &[usize]) -> [bool; 256] {
        std::array::from_fn(|i| keys.contains(&i))
    }
    #[test]
    fn opening_click_and_preheld_keys_require_a_fresh_press() {
        let mut e = Edges::new(down(&[1, 0x4c]));
        assert_eq!(e.update(down(&[1, 0x4c])), None);
        assert_eq!(e.update(down(&[])), None);
        assert_eq!(e.update(down(&[0x4c])), Some(Choice::Key(0x4c)));
    }
    #[test]
    fn modifier_sides_and_mouse_side_buttons_keep_their_actual_codes() {
        for vk in [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 5, 6] {
            let generic = match vk {
                0xa0 | 0xa1 => 0x10,
                0xa2 | 0xa3 => 0x11,
                0xa4 | 0xa5 => 0x12,
                _ => 0,
            };
            let mut e = Edges::new(down(&[]));
            assert_eq!(e.update(down(&[vk, generic])), Some(Choice::Key(vk as u16)));
        }
    }
    #[test]
    fn ime_mode_indicators_never_count_as_held_or_chosen() {
        // The 半角 indicator (0xF3) reads as held with nothing pressed.
        let state = held(|vk| vk == 0xF3 || vk == 0xF4);
        assert!(!state.iter().any(|d| *d), "the release wait must finish");
        let state = held(|vk| vk == 0xF3 || vk == 0x4c);
        assert!(state[0x4c] && !state[0xF3]);
        // A mode switch during capture is not taken as the chosen key.
        let mut e = Edges::new(held(|vk| vk == 0xF3));
        assert_eq!(e.update(held(|vk| vk == 0xF4)), None);
        assert_eq!(
            e.update(held(|vk| vk == 0xF4 || vk == 0x75)),
            Some(Choice::Key(0x75))
        );
    }

    #[test]
    fn cancel_clear_function_and_letter_keys_are_distinct() {
        for (vk, expected) in [
            (0x1b, Choice::Cancel),
            (8, Choice::Key(0)),
            (0x75, Choice::Key(0x75)),
            (0x43, Choice::Key(0x43)),
        ] {
            assert_eq!(Edges::new(down(&[])).update(down(&[vk])), Some(expected));
        }
    }
}
