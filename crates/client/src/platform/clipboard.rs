//! Unicode Win32 clipboard backend for ImGui's native text editing shortcuts.
use hudhook::imgui::ClipboardBackend;
use windows_sys::Win32::{
    Foundation::GlobalFree,
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
        },
        Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
    },
    UI::WindowsAndMessaging::GetForegroundWindow,
};

const UNICODE_TEXT: u32 = 13; // CF_UNICODETEXT
const MAX_BYTES: usize = 1024 * 1024;
pub struct Clipboard;
struct Open;
impl Open {
    unsafe fn new() -> Option<Self> {
        if OpenClipboard(GetForegroundWindow()) != 0 {
            Some(Self)
        } else {
            None
        }
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
fn decode(wide: &[u16]) -> Option<String> {
    let end = wide.iter().position(|&v| v == 0)?;
    Some(String::from_utf16_lossy(&wide[..end]))
}
impl ClipboardBackend for Clipboard {
    fn get(&mut self) -> Option<String> {
        unsafe {
            let _open = Open::new()?;
            let handle = GetClipboardData(UNICODE_TEXT);
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            if !(2..=MAX_BYTES).contains(&size) || size & 1 != 0 {
                return None;
            }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let text = decode(std::slice::from_raw_parts(ptr, size / 2));
            GlobalUnlock(handle);
            text
        }
    }
    fn set(&mut self, value: &str) {
        let wide: Vec<u16> = value
            .split('\0')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let bytes = wide.len() * 2;
        if bytes > MAX_BYTES {
            return;
        }
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if handle.is_null() {
                return;
            }
            let ptr = GlobalLock(handle) as *mut u16;
            if ptr.is_null() {
                GlobalFree(handle);
                return;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(handle);
            let Some(_open) = Open::new() else {
                GlobalFree(handle);
                return;
            };
            if EmptyClipboard() == 0 || SetClipboardData(UNICODE_TEXT, handle).is_null() {
                GlobalFree(handle);
            } // On success, Windows owns handle.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_unicode_and_stops_at_terminator() {
        let mut wide: Vec<u16> = "日本語 🧭".encode_utf16().chain(Some(0)).collect();
        wide.extend("old data".encode_utf16());
        assert_eq!(decode(&wide).as_deref(), Some("日本語 🧭"));
        assert_eq!(decode(&[0]), Some(String::new()));
        assert_eq!(decode(&[65]), None);
    }
}
