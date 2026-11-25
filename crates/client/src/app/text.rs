//! Display-text helpers shared by HUDs and notifications.
use std::borrow::Cow;
use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};

pub(crate) fn local_time() -> SYSTEMTIME {
    let mut time = SYSTEMTIME::default();
    unsafe { GetLocalTime(&mut time) };
    time
}

pub(crate) fn clock_hms() -> String {
    let time = local_time();
    format!("{:02}:{:02}:{:02}", time.wHour, time.wMinute, time.wSecond)
}

pub(crate) fn fit_width<'a>(
    text: &'a str,
    available: f32,
    width: impl Fn(&str) -> f32,
) -> Cow<'a, str> {
    if width(text) <= available {
        return Cow::Borrowed(text);
    }
    let suffix = "…";
    let limit = available - width(suffix);
    if limit < 0. {
        return Cow::Borrowed("");
    }
    // Text advances are non-decreasing. Search UTF-8 boundaries without
    // allocating a boundary table or measuring every successively shorter line.
    let (mut low, mut high) = (0, text.len());
    while low < high {
        let mut end = low + (high - low).div_ceil(2);
        while !text.is_char_boundary(end) {
            end += 1;
        }
        if width(&text[..end]) <= limit {
            low = end;
        } else {
            high = end - 1;
            while !text.is_char_boundary(high) {
                high -= 1;
            }
        }
    }
    if low == 0 {
        Cow::Borrowed(suffix)
    } else {
        Cow::Owned(format!("{}{suffix}", &text[..low]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_japanese_labels_clip_on_character_boundaries_with_bounded_measurements() {
        let text = "機能名".repeat(10_000);
        let calls = std::cell::Cell::new(0);
        let result = fit_width(&text, 160., |s| {
            calls.set(calls.get() + 1);
            s.chars().count() as f32 * 16.
        });
        assert_eq!(result, "機能名機能名機能名…");
        assert!(calls.get() < 24, "measured {} prefixes", calls.get());
        for text in ["aあ😀いz", "あいう", ""] {
            for limit in 0..100 {
                let width = |s: &str| {
                    s.chars()
                        .map(|c| if c.is_ascii() { 8. } else { 16. })
                        .sum::<f32>()
                };
                let fit = fit_width(text, limit as f32, width);
                assert!(width(&fit) <= limit as f32);
                if width(text) <= limit as f32 {
                    assert_eq!(fit, text);
                }
            }
        }
    }
}
