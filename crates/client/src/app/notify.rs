//! Short on-screen notifications ("esp ON"), stacked in a screen corner and
//! faded out after a moment. Anything in the client may `push` one; the overlay
//! draws them.

use crate::text::fit_width as fit_text;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use hudhook::imgui::Ui;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

/// How long a notification stays (ms), the last part of which it fades.
const LIFETIME_MS: u64 = 2600;
const FADE_MS: u64 = 500;
/// At most this many are shown; older ones are dropped to make room.
const MAX_VISIBLE: usize = 6;
/// Longer texts are cut (a chat line can be arbitrarily long).
const MAX_CHARS: usize = 80;

struct Toast {
    text: String,
    at: u64,
}

/// The pending notifications, oldest first.
struct Queue {
    toasts: VecDeque<Toast>,
}

impl Queue {
    const fn new() -> Self {
        Self {
            toasts: VecDeque::new(),
        }
    }

    fn push(&mut self, text: &str, now: u64) {
        let text = shorten(text);
        if text.is_empty() {
            return;
        }
        // Repeating the newest message just restarts its timer.
        if let Some(last) = self.toasts.back_mut() {
            if last.text == text {
                last.at = now;
                return;
            }
        }
        self.toasts.push_back(Toast { text, at: now });
        while self.toasts.len() > MAX_VISIBLE {
            self.toasts.pop_front();
        }
    }

    /// Drops expired notifications and returns the rest with their opacity.
    fn visible(&mut self, now: u64) -> Vec<(&str, f32)> {
        self.toasts
            .retain(|t| now.saturating_sub(t.at) < LIFETIME_MS);
        self.toasts
            .iter()
            .map(|t| (t.text.as_str(), opacity(now.saturating_sub(t.at))))
            .collect()
    }
}

/// One line, at most `MAX_CHARS` characters, without control characters.
fn shorten(text: &str) -> String {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    if line.chars().count() <= MAX_CHARS {
        line.to_string()
    } else {
        let cut: String = line.chars().take(MAX_CHARS - 3).collect();
        format!("{}...", cut.trim_end())
    }
}

/// Fully opaque until the fade starts, then linearly down to nothing.
fn opacity(age_ms: u64) -> f32 {
    let left = LIFETIME_MS.saturating_sub(age_ms);
    (left as f32 / FADE_MS as f32).clamp(0.0, 1.0)
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static ENABLED: AtomicBool = AtomicBool::new(true);
/// False during startup, so restoring the configured modules is silent.
static READY: AtomicBool = AtomicBool::new(false);

/// Turns notifications on or off (`[gui] notifications`).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Called once the client has finished starting up.
pub fn ready() {
    READY.store(true, Ordering::Relaxed);
}

/// Shows `text` for a moment. Ignored while starting up or when switched off.
pub fn push(text: &str) {
    if ENABLED.load(Ordering::Relaxed) && READY.load(Ordering::Relaxed) {
        let now = unsafe { GetTickCount64() };
        QUEUE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text, now);
    }
}

/// Draws the notifications, newest at the bottom, above the bottom-right corner.
pub fn draw(ui: &Ui) {
    let now = unsafe { GetTickCount64() };
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let toasts = queue.visible(now);
    if toasts.is_empty() {
        return;
    }
    let size = ui.io().display_size;
    let line = ui.text_line_height();
    let (pad, gap, margin) = (12.0, 8.0, 20.0);
    let draw_list = ui.get_foreground_draw_list();
    // Leave the bottom strip free for the hotbar and the keystrokes widget.
    let mut bottom = size[1] * 0.78;
    for (text, alpha) in toasts.iter().rev() {
        let text = fit_text(text, (size[0] - margin * 2. - pad * 2.).max(0.), |s| {
            ui.calc_text_size(s)[0]
        });
        let width = ui.calc_text_size(&text)[0] + pad * 2.0;
        let height = line + pad * 2.0;
        let min = [size[0] - margin - width, bottom - height];
        crate::theme::panel_alpha(&draw_list, min, [min[0] + width, min[1] + height], *alpha);
        draw_list
            .add_circle(
                [min[0] + 5., min[1] + height * 0.5],
                2.,
                [0.40, 0.78, 0.98, *alpha],
            )
            .filled(true)
            .build();
        draw_list.add_text(
            [min[0] + pad, min[1] + pad],
            [
                crate::theme::TEXT[0],
                crate::theme::TEXT[1],
                crate::theme::TEXT[2],
                *alpha,
            ],
            &text,
        );
        bottom -= height + gap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_notices_fit_by_width_without_changing_stored_text() {
        let width = |s: &str| {
            s.chars()
                .map(|c| if c.is_ascii() { 8. } else { 16. })
                .sum::<f32>()
        };
        let text = "耐久保護を有効にしました";
        let short = fit_text(text, 80., width);
        assert_eq!(short, "耐久保護…");
        assert!(width(&short) <= 80.);
        assert_eq!(fit_text(text, 500., width), text);
        assert_eq!(fit_text(text, 5., width), "");
    }

    #[test]
    fn notifications_expire_fade_and_are_bounded() {
        let mut queue = Queue::new();
        queue.push("esp ON", 0);
        queue.push("fly ON", 100);
        assert_eq!(
            queue.visible(200),
            [("esp ON", 1.0), ("fly ON", 1.0)],
            "oldest first, fully opaque"
        );
        // Fading: half way through the fade of the first one.
        let shown = queue.visible(LIFETIME_MS - FADE_MS / 2);
        assert_eq!(shown[0].0, "esp ON");
        assert!((shown[0].1 - 0.5).abs() < 0.01, "{}", shown[0].1);
        // The first has expired, the second has not.
        assert_eq!(queue.visible(LIFETIME_MS + 50).len(), 1);
        assert!(queue.visible(LIFETIME_MS + 100).is_empty());

        for i in 0..20 {
            queue.push(&format!("message {i}"), 5000);
        }
        let shown = queue.visible(5000);
        assert_eq!(shown.len(), MAX_VISIBLE);
        assert_eq!(shown.last().unwrap().0, "message 19");
    }

    #[test]
    fn repeating_the_newest_message_restarts_it_instead_of_stacking() {
        let mut queue = Queue::new();
        queue.push("zoom ON", 0);
        queue.push("zoom ON", 2000);
        assert_eq!(queue.visible(2000 + LIFETIME_MS - 1).len(), 1);
        queue.push("zoom OFF", 5000);
        queue.push("zoom ON", 5001);
        assert_eq!(queue.visible(5002).len(), 2);
    }

    #[test]
    fn text_is_one_short_printable_line() {
        assert_eq!(shorten("  esp ON\n"), "esp ON");
        assert_eq!(shorten("a\tb\r\nc"), "a b  c");
        assert_eq!(shorten(""), "");
        let long = "あ".repeat(200);
        let cut = shorten(&long);
        assert_eq!(cut.chars().count(), MAX_CHARS);
        assert!(cut.ends_with("..."));
        // Empty texts are not queued at all.
        let mut queue = Queue::new();
        queue.push("   ", 0);
        assert!(queue.visible(0).is_empty());
    }

    #[test]
    fn nothing_is_shown_before_startup_finishes_or_when_switched_off() {
        // `ready()` is never called in the test process.
        push("hidden");
        assert!(QUEUE.lock().unwrap().toasts.is_empty());
    }
}
