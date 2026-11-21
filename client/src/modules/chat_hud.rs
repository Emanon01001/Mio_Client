//! ChatHUD: the latest chat messages in a panel that stays on screen, with
//! the time each arrived, senders in their own colours, and repeats of the
//! same message collapsed into one line ("×3"). Movable in the HUD editor.
//! Reads the chat the SDK captures; sends nothing.

use std::collections::VecDeque;

use crate::api::{ChatDirection, ChatMessage};
use hudhook::imgui::Ui;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

use super::chat_log::{clean, truncate_chars, ChatCursor};
use super::radar::origin;
use super::settings::{Detail, Value};
use super::{key_or_warn, Module, Tick};
use crate::text::local_time;
use crate::{config, log};

/// Messages kept (more than are ever shown).
const KEPT: usize = 50;
const PADDING: f32 = 8.;
/// Characters of a message kept for display.
const MAX_CHARS: usize = 400;

const SYSTEM: [f32; 4] = [0.98, 0.85, 0.45, 1.];
const SELF: [f32; 4] = [0.35, 0.95, 0.85, 1.];
/// Sender colours, picked by a hash of the name so each keeps its colour.
const SENDERS: [[f32; 4]; 8] = [
    [0.55, 0.9, 0.55, 1.],
    [1., 0.62, 0.45, 1.],
    [0.75, 0.6, 1., 1.],
    [1., 0.55, 0.75, 1.],
    [1., 0.45, 0.45, 1.],
    [0.75, 0.95, 0.4, 1.],
    [0.6, 0.75, 1., 1.],
    [0.85, 0.75, 0.6, 1.],
];

fn sender_color(name: &str) -> [f32; 4] {
    // FNV-1a: stable across runs.
    let hash = name.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    });
    SENDERS[hash as usize % SENDERS.len()]
}

fn clock() -> String {
    let t = local_time();
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

/// One shown message.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    time: String,
    /// GetTickCount64 when it (or its latest repeat) arrived.
    at: u64,
    /// None: a system message. Sent messages are "自分".
    sender: Option<String>,
    sent: bool,
    text: String,
    count: u32,
    /// Wrapped lines for (text width, font size), cached.
    layout: Option<([f32; 2], Vec<String>)>,
}

/// Adds `message` to `entries`, merging it into the last entry if it repeats
/// it (same direction, sender and text). Keeps at most `KEPT` entries.
fn add(entries: &mut VecDeque<Entry>, message: &ChatMessage, time: &str, now: u64) {
    let sent = message.direction == ChatDirection::Sent;
    let sender = if sent {
        Some("自分".to_string())
    } else {
        message
            .sender
            .as_deref()
            .map(clean)
            .filter(|s| !s.is_empty())
    };
    let mut text = clean(&message.text);
    if truncate_chars(&mut text, MAX_CHARS) {
        text.push('…');
    }
    if text.is_empty() {
        return;
    }
    if let Some(last) = entries
        .back_mut()
        .filter(|e| e.sent == sent && e.sender == sender && e.text == text)
    {
        last.count += 1;
        last.at = now;
        last.time = time.to_string();
        return;
    }
    if entries.len() == KEPT {
        entries.pop_front();
    }
    entries.push_back(Entry {
        time: time.to_string(),
        at: now,
        sender,
        sent,
        text,
        count: 1,
        layout: None,
    });
}

/// Splits `text` into lines no wider than `first` (the first line) and
/// `rest` (the others), measuring each character with `width`. Breaks
/// between characters (Japanese has no spaces), preferring the last space
/// on a line when there is one.
fn wrap(text: &str, first: f32, rest: f32, width: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0.;
    let mut limit = first.max(1.);
    let mut buffer = [0u8; 4];
    for c in text.chars() {
        let w = width(c.encode_utf8(&mut buffer));
        if used + w > limit && !line.is_empty() {
            // Carry the word after the last space to the next line.
            let carry = match line.rfind(' ') {
                Some(space) if space > 0 && c != ' ' => line.split_off(space + 1),
                _ => String::new(),
            };
            lines.push(std::mem::take(&mut line).trim_end().to_string());
            used = carry
                .chars()
                .map(|c| width(c.encode_utf8(&mut buffer)))
                .sum();
            line = carry;
            limit = rest.max(1.);
        }
        if line.is_empty() && c == ' ' {
            continue;
        }
        line.push(c);
        used += w;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

pub struct ChatHud {
    key: u16,
    enabled: bool,
    lines: usize,
    width: f32,
    show_time: bool,
    /// Seconds a message stays shown; 0: always the latest `lines`.
    fade: f32,
    x: i32,
    y: i32,
    entries: VecDeque<Entry>,
    chat: ChatCursor,
}

impl ChatHud {
    pub fn new(cfg: &config::ChatHud) -> Self {
        Self {
            key: key_or_warn("chathud", &cfg.key),
            enabled: false,
            lines: cfg.lines.clamp(1, 20),
            width: if cfg.width.is_finite() {
                cfg.width.clamp(200., 900.)
            } else {
                420.
            },
            show_time: cfg.time,
            fade: if cfg.fade.is_finite() {
                cfg.fade.clamp(0., 600.)
            } else {
                0.
            },
            x: cfg.x,
            y: cfg.y,
            entries: VecDeque::new(),
            chat: ChatCursor::default(),
        }
    }
}

impl Module for ChatHud {
    fn name(&self) -> &str {
        "chathud"
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
        if on {
            if let Err(e) = self.chat.start() {
                log!("chathudを利用できません: {e}");
                return;
            }
        }
        self.entries.clear();
        self.enabled = on;
    }
    fn tick(&mut self, tick: &Tick) {
        let Some(batch) = self.chat.poll(self.enabled, tick.now_ms) else {
            return;
        };
        let time = clock();
        for message in &batch.messages {
            add(&mut self.entries, message, &time, tick.now_ms);
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::integer(
                "lines",
                "表示するメッセージ数",
                "新しいものから表示する件数です。",
                self.lines as i32,
                1,
                20,
            ),
            Detail::number(
                "width",
                "幅（ピクセル）",
                "長いメッセージはこの幅で折り返します。",
                self.width as f64,
                200.,
                900.,
            ),
            Detail::boolean(
                "time",
                "時刻を表示",
                "受信した時刻（時:分）を先頭に表示します。",
                self.show_time,
            ),
            Detail::number(
                "fade",
                "表示する秒数",
                "受信からこの秒数が過ぎたメッセージを隠します。0で常に表示します。",
                self.fade as f64,
                0.,
                600.,
            ),
            Detail::x_offset(self.x, "Positive: from left. Negative: from right."),
            Detail::y_offset(self.y, "Positive: from top. Negative: from bottom."),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "lines" => self.lines = (value.number()? as usize).clamp(1, 20),
            "width" => self.width = value.number()?.clamp(200., 900.) as f32,
            "time" => self.show_time = value.boolean()?,
            "fade" => self.fade = value.number()?.clamp(0., 600.) as f32,
            "x" => self.x = value.number()? as i32,
            "y" => self.y = value.number()? as i32,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        match self.entries.len() {
            0 => "受信待ち".into(),
            n => format!("{n}件"),
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if !super::hud_view() && !crate::gui::captures_input() {
            return;
        }
        let now = unsafe { GetTickCount64() };
        self.paint(ui, now);
    }
}

impl ChatHud {
    fn paint(&mut self, ui: &Ui, now: u64) {
        let screen = ui.io().display_size;
        let width = self.width.min(screen[0] - 8.).max(80.);
        let inner = width - 2. * PADDING;
        let line_height = ui.text_line_height() + 3.;
        let font = ui.current_font_size();
        let fade_ms = (self.fade * 1000.) as u64;
        let first = self.entries.len().saturating_sub(self.lines);
        // Lay out the shown entries (cached per width and font).
        let mut rows = 0;
        let mut shown = Vec::new();
        for (index, entry) in self.entries.iter_mut().enumerate().skip(first) {
            if fade_ms > 0 && now.saturating_sub(entry.at) > fade_ms {
                continue;
            }
            let prefix = prefix(entry, self.show_time);
            let prefix_width = ui.calc_text_size(&prefix)[0];
            let key = [inner, font + if self.show_time { 0.5 } else { 0. }];
            if entry.layout.as_ref().is_none_or(|(k, _)| *k != key) {
                let mut text = entry.text.clone();
                if entry.count > 1 {
                    text.push_str(&format!(" ×{}", entry.count));
                }
                let lines = wrap(&text, inner - prefix_width, inner, |s| {
                    ui.calc_text_size(s)[0]
                });
                entry.layout = Some((key, lines));
            }
            rows += entry.layout.as_ref().map_or(1, |(_, l)| l.len());
            shown.push((index, prefix_width));
        }
        let height = 2. * PADDING + rows.max(1) as f32 * line_height;
        let size = [width, height.min(screen[1] - 8.)];
        crate::hud_editor::measure("chathud", size);
        if shown.is_empty() && !crate::gui::captures_input() {
            return;
        }
        let left = origin(self.x, screen[0], size[0]);
        let top = origin(self.y, screen[1], size[1]);
        let draw = ui.get_background_draw_list();
        crate::theme::panel_alpha(&draw, [left, top], [left + size[0], top + size[1]], 0.75);
        if shown.is_empty() {
            draw.add_text(
                [left + PADDING, top + PADDING],
                crate::theme::MUTED,
                "チャットを受信すると表示します",
            );
            return;
        }
        let mut y = top + PADDING;
        draw.with_clip_rect_intersect([left, top], [left + size[0], top + size[1]], || {
            for &(index, prefix_width) in &shown {
                let entry = &self.entries[index];
                let x = left + PADDING;
                // Time (muted), then the sender in its colour.
                let time_width = if self.show_time {
                    let time = format!("{} ", entry.time);
                    draw.add_text([x, y], crate::theme::MUTED, &time);
                    ui.calc_text_size(&time)[0]
                } else {
                    0.
                };
                let color = match (&entry.sender, entry.sent) {
                    (_, true) => SELF,
                    (Some(name), false) => sender_color(name),
                    (None, false) => SYSTEM,
                };
                if let Some(name) = &entry.sender {
                    draw.add_text([x + time_width, y], color, format!("{name}: "));
                }
                let text_color = if entry.sender.is_none() {
                    SYSTEM
                } else {
                    crate::theme::TEXT
                };
                let lines = entry.layout.as_ref().map_or(&[][..], |(_, l)| l.as_slice());
                for (n, line) in lines.iter().enumerate() {
                    let at = if n == 0 { x + prefix_width } else { x };
                    draw.add_text([at, y], text_color, line);
                    y += line_height;
                }
            }
        });
    }
}

/// What precedes a message's text on its first line: the time and sender.
fn prefix(entry: &Entry, show_time: bool) -> String {
    let mut out = String::new();
    if show_time {
        out.push_str(&entry.time);
        out.push(' ');
    }
    if let Some(name) = &entry.sender {
        out.push_str(name);
        out.push_str(": ");
    }
    out
}

/// The panel with a few messages, for the headless UI preview.
#[cfg(test)]
pub(crate) fn preview(ui: &Ui) {
    let mut hud = ChatHud::new(&config::ChatHud {
        x: -20,
        y: -20,
        ..Default::default()
    });
    let message = |direction, sender: Option<&str>, text: &str| ChatMessage {
        id: 0,
        direction,
        sender: sender.map(String::from),
        text: text.into(),
    };
    for (time, m) in [
        (
            "12:01",
            message(
                ChatDirection::Received,
                None,
                "§eSteveがゲームに参加しました",
            ),
        ),
        (
            "12:02",
            message(ChatDirection::Received, Some("Steve"), "こんにちは！"),
        ),
        (
            "12:02",
            message(ChatDirection::Sent, None, "やあ、ダイヤ見つけたよ"),
        ),
        (
            "12:03",
            message(ChatDirection::Received, Some("Alex"), "gg"),
        ),
        (
            "12:03",
            message(ChatDirection::Received, Some("Alex"), "gg"),
        ),
        (
            "12:03",
            message(ChatDirection::Received, Some("Alex"), "gg"),
        ),
        (
            "12:04",
            message(
                ChatDirection::Received,
                Some("Steve"),
                "長いメッセージは設定した幅で折り返して、続きを次の行に表示します。",
            ),
        ),
    ] {
        add(&mut hud.entries, &m, time, 0);
    }
    hud.paint(ui, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(direction: ChatDirection, sender: Option<&str>, text: &str) -> ChatMessage {
        ChatMessage {
            id: 0,
            direction,
            sender: sender.map(String::from),
            text: text.into(),
        }
    }

    #[test]
    fn repeats_collapse_and_only_the_latest_are_kept() {
        let mut entries = VecDeque::new();
        let gg = message(ChatDirection::Received, Some("Alex"), "§agg");
        add(&mut entries, &gg, "12:00", 1);
        add(&mut entries, &gg, "12:01", 2);
        add(&mut entries, &gg, "12:02", 3);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(
            (e.count, e.text.as_str(), e.time.as_str(), e.at),
            (3, "gg", "12:02", 3)
        );
        // The same words from someone else, or sent by me, are new lines.
        add(
            &mut entries,
            &message(ChatDirection::Received, Some("Steve"), "gg"),
            "t",
            4,
        );
        add(
            &mut entries,
            &message(ChatDirection::Sent, None, "gg"),
            "t",
            5,
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].sender.as_deref(), Some("自分"));
        // Empty (after formatting codes) messages are skipped.
        add(
            &mut entries,
            &message(ChatDirection::Received, None, "§r"),
            "t",
            6,
        );
        assert_eq!(entries.len(), 3);
        for n in 0..100 {
            add(
                &mut entries,
                &message(ChatDirection::Received, None, &format!("m{n}")),
                "t",
                7,
            );
        }
        assert_eq!(entries.len(), KEPT);
        assert_eq!(entries.back().unwrap().text, "m99");
    }

    #[test]
    fn lines_wrap_to_the_width_preferring_spaces() {
        // Every character 1 wide.
        let one = |s: &str| s.chars().count() as f32;
        assert_eq!(wrap("abcdefgh", 3., 4., one), ["abc", "defg", "h"]);
        assert_eq!(
            wrap("hello big world", 6., 6., one),
            ["hello", "big", "world"]
        );
        assert_eq!(wrap("日本語の文章", 2., 4., one), ["日本", "語の文章"]);
        assert_eq!(wrap("", 5., 5., one), [""]);
        // A first line with no room still makes progress.
        assert_eq!(wrap("ab", 0., 5., one), ["a", "b"]);
    }

    #[test]
    fn sender_colours_are_stable() {
        assert_eq!(sender_color("Steve"), sender_color("Steve"));
        let distinct: std::collections::HashSet<_> = ["Steve", "Alex", "Notch", "Herobrine"]
            .iter()
            .map(|n| sender_color(n).map(f32::to_bits))
            .collect();
        assert!(distinct.len() >= 2);
    }
}
