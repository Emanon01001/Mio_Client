//! Key state and key names for hotkeys and fly controls.

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetForegroundWindow, GetWindowThreadProcessId, WindowFromPoint, CURSORINFO,
    CURSOR_SHOWING,
};

/// Parses a key name from the config (`F6`, `END`, `W`, `LCONTROL`, `0x23`, `NONE`).
/// Returns the virtual-key code; 0 means "unbound".
pub fn parse_key(name: &str) -> Option<u16> {
    let name = name.trim().to_ascii_uppercase();
    if let Some(hex) = name.strip_prefix("0X") {
        return u16::from_str_radix(hex, 16).ok().filter(|&v| v <= 0xFF);
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&n).then_some(0x70 + n - 1);
    }
    if let Some(n) = name
        .strip_prefix("NUMPAD")
        .and_then(|n| n.parse::<u16>().ok())
    {
        return (0..=9).contains(&n).then_some(0x60 + n);
    }
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    Some(match name.as_str() {
        "" | "NONE" => 0,
        "MOUSE4" => 0x05,
        "MOUSE5" => 0x06,
        "BACKSPACE" => 0x08,
        "TAB" => 0x09,
        "ENTER" | "RETURN" => 0x0D,
        "PAUSE" => 0x13,
        "CAPSLOCK" => 0x14,
        "ESC" | "ESCAPE" => 0x1B,
        "SPACE" => 0x20,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "END" => 0x23,
        "HOME" => 0x24,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "PRINTSCREEN" => 0x2C,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "NUMLOCK" => 0x90,
        "SCROLLLOCK" => 0x91,
        "LSHIFT" => 0xA0,
        "RSHIFT" => 0xA1,
        "LCONTROL" | "LCTRL" => 0xA2,
        "RCONTROL" | "RCTRL" => 0xA3,
        "LALT" => 0xA4,
        "RALT" => 0xA5,
        _ => return None,
    })
}

/// Display name for a virtual-key code (inverse of [`parse_key`]).
pub fn key_name(vk: u16) -> String {
    match vk {
        0 => "NONE".into(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x05 => "MOUSE4".into(),
        0x06 => "MOUSE5".into(),
        0x09 => "TAB".into(),
        0x20 => "SPACE".into(),
        0x21 => "PAGEUP".into(),
        0x22 => "PAGEDOWN".into(),
        0x23 => "END".into(),
        0x24 => "HOME".into(),
        0x2D => "INSERT".into(),
        0x2E => "DELETE".into(),
        0xA0 => "LSHIFT".into(),
        0xA1 => "RSHIFT".into(),
        0xA2 => "LCONTROL".into(),
        0xA3 => "RCONTROL".into(),
        0xA4 => "LALT".into(),
        0xA5 => "RALT".into(),
        _ => format!("0x{vk:02X}"),
    }
}

/// Friendly labels are separate from the stable names saved in profiles.
pub fn display_key_name(vk: u16) -> String {
    match vk {
        0x08 => "BACKSPACE".into(),
        0x0d => "ENTER".into(),
        0x13 => "PAUSE".into(),
        0x14 => "CAPSLOCK".into(),
        0x1b => "ESC".into(),
        0x25 => "LEFT".into(),
        0x26 => "UP".into(),
        0x27 => "RIGHT".into(),
        0x28 => "DOWN".into(),
        0x2c => "PRINTSCREEN".into(),
        0x60..=0x69 => format!("NUMPAD{}", vk - 0x60),
        0x90 => "NUMLOCK".into(),
        0x91 => "SCROLLLOCK".into(),
        _ => key_name(vk),
    }
}

pub fn key_down(vk: u16) -> bool {
    vk != 0 && unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0
}

/// True if the foreground window belongs to this process (i.e. the game).
pub fn is_foreground() -> bool {
    foreground_window().is_some()
}

/// The foreground window, if it belongs to the game (this process).
fn foreground_window() -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        (pid == GetCurrentProcessId()).then_some(hwnd)
    }
}

/// Whether the window under the screen point `at` belongs to the game (this
/// process). Uses the same coordinates as `GetCursorPos`/`SetCursorPos`.
pub fn game_at(at: POINT) -> bool {
    unsafe {
        let hwnd = WindowFromPoint(at);
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid == GetCurrentProcessId()
    }
}

/// The game hides the cursor while actually playing and shows it in menus,
/// chat and inventories - used as a fallback for detecting the world view.
pub fn cursor_hidden() -> bool {
    unsafe {
        let mut info: CURSORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
        GetCursorInfo(&mut info) != 0 && info.flags & CURSOR_SHOWING == 0
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    #[test]
    fn physical_key_names_round_trip_and_legacy_hex_still_works() {
        for key in 0..=255 {
            assert_eq!(parse_key(&key_name(key)), Some(key));
        }
        for (name, key) in [
            ("ENTER", 0x0d),
            ("ESCAPE", 0x1b),
            ("UP", 0x26),
            ("NUMPAD9", 0x69),
            ("RSHIFT", 0xa1),
            ("0x26", 0x26),
        ] {
            assert_eq!(parse_key(name), Some(key));
        }
        assert_eq!(key_name(0x26), "0x26");
        assert_eq!(display_key_name(0x26), "UP");
    }
}
