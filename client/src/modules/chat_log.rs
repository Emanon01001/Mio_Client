//! Chat log: appends every chat message the SDK captures to `chat.log` next
//! to the DLL, with a timestamp, and raises a notification when a received
//! message contains one of the configured words. Reads only; sends nothing.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::settings::{Detail, Value};
use super::{key_or_warn, Module, Tick};
use crate::api::{self, ChatBatch, ChatDirection, ChatMessage};
use crate::text::local_time;
use crate::{config, log, notify};
use hudhook::imgui::{Condition, Ui};
use std::collections::VecDeque;

pub const FILE_NAME: &str = "chat.log";
const MAX_HIGHLIGHTS: usize = 16;
const MAX_HISTORY: usize = 512;
const MAX_DISPLAY_CHARS: usize = 2048;

/// Removes Minecraft's formatting codes (`§` plus one character) and control
/// characters, so the log is plain text with one message per line.
pub(crate) fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else if c.is_control() {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// One log line: `[2026-11-15 04:40:12] <- Steve: hello` (received) or
/// `[...] -> hello` (sent).
fn format_line(time: &str, message: &ChatMessage) -> String {
    let text = clean(&message.text);
    let sender = message.sender.as_deref().map(clean).unwrap_or_default();
    match (message.direction, sender.is_empty()) {
        (ChatDirection::Sent, _) => format!("[{time}] -> {text}"),
        (ChatDirection::Received, true) => format!("[{time}] <- {text}"),
        (ChatDirection::Received, false) => format!("[{time}] <- {sender}: {text}"),
    }
}

/// The first highlight word a received message contains, ignoring case.
fn mention<'a>(message: &ChatMessage, words: &'a [String]) -> Option<&'a str> {
    if message.direction != ChatDirection::Received {
        return None;
    }
    let text = clean(&message.text).to_lowercase();
    words
        .iter()
        .find(|word| text.contains(word.as_str()))
        .map(String::as_str)
}

/// Lower-cased, non-empty, de-duplicated highlight words.
fn normalize_words(words: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in words {
        let word = clean(word)
            .chars()
            .take(64)
            .collect::<String>()
            .to_lowercase();
        if !word.is_empty() && !out.contains(&word) && out.len() < MAX_HIGHLIGHTS {
            out.push(word);
        }
    }
    out
}
pub(super) fn truncate_chars(value: &mut String, max: usize) -> bool {
    if let Some((at, _)) = value.char_indices().nth(max) {
        value.truncate(at);
        true
    } else {
        false
    }
}

fn timestamp() -> String {
    let t = local_time();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// Each consumer keeps its own cursor and starts with the next captured message.
#[derive(Default)]
pub(super) struct ChatCursor {
    cursor: u64,
    polled_at: u64,
}

impl ChatCursor {
    pub(super) fn start(&mut self) -> api::Result<()> {
        self.cursor = api::game()?.chat_since(u64::MAX)?.cursor;
        Ok(())
    }

    pub(super) fn poll(&mut self, enabled: bool, now: u64) -> Option<ChatBatch> {
        if !enabled || now.saturating_sub(self.polled_at) < 250 {
            return None;
        }
        self.polled_at = now;
        let batch = api::game().ok()?.chat_since(self.cursor).ok()?;
        self.cursor = batch.cursor;
        (!batch.messages.is_empty()).then_some(batch)
    }
}

pub struct ChatLog {
    key: u16,
    enabled: bool,
    path: PathBuf,
    highlight: Vec<String>,
    chat: ChatCursor,
    lines: usize,
    /// Why the last write failed, shown instead of failing every 250 ms.
    error: Option<String>,
    history: VecDeque<(String, String, bool)>,
    search: String,
    words_input: String,
    notifications: bool,
    file: bool,
    only_mentions: bool,
    settings_error: String,
}

impl ChatLog {
    /// The search over the messages captured since it was switched on: by
    /// text, sender and time, optionally only highlighted ones, with copy.
    fn draw_search(&mut self, ui: &Ui) {
        ui.input_text("検索", &mut self.search).build();
        truncate_chars(&mut self.search, 256);
        ui.checkbox("強調したメッセージだけ", &mut self.only_mentions);
        let query = self.search.to_lowercase();
        let visible: Vec<_> = self
            .history
            .iter()
            .filter(|(lower, _, highlight)| {
                (!self.only_mentions || *highlight) && lower.contains(&query)
            })
            .collect();
        if ui.button("検索結果をコピー") {
            ui.set_clipboard_text(
                visible
                    .iter()
                    .map(|(_, line, _)| line.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        ui.same_line();
        ui.text(format!("{} / {}件", visible.len(), self.history.len()));
        ui.child_window("chat_lines").size([0., 0.]).build(|| {
            for (_, line, highlight) in visible {
                let color = if *highlight {
                    crate::theme::WARNING
                } else {
                    crate::theme::TEXT
                };
                let _wrap = ui.push_text_wrap_pos();
                ui.text_colored(color, line);
            }
        });
    }

    pub fn new(cfg: &config::ChatLog, dir: &Path) -> Self {
        Self {
            key: key_or_warn("chatlog", &cfg.key),
            enabled: false,
            path: dir.join(FILE_NAME),
            highlight: normalize_words(&cfg.highlight),
            chat: ChatCursor::default(),
            lines: 0,
            error: None,
            history: VecDeque::new(),
            search: String::new(),
            words_input: String::new(),
            notifications: cfg.notifications,
            file: cfg.file,
            only_mentions: false,
            settings_error: String::new(),
        }
    }

    fn append(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| {
                let mut text = lines.join("\r\n");
                text.push_str("\r\n");
                file.write_all(text.as_bytes())
            });
        match result {
            Ok(()) => {
                self.lines += lines.len();
                self.error = None;
            }
            Err(e) => {
                // Say so once, not on every poll.
                if self.error.is_none() {
                    log!("チャットログへ書き込めません: {}: {e}", self.path.display());
                }
                self.error = Some(e.to_string());
            }
        }
    }
    fn remember(&mut self, line: &str, highlighted: bool) {
        // Keep full disk logs; bound only the additional on-screen history.
        let mut display = line.to_owned();
        if truncate_chars(&mut display, MAX_DISPLAY_CHARS) {
            display.push('…');
        }
        if self.history.len() == MAX_HISTORY {
            self.history.pop_front();
        }
        self.history
            .push_back((display.to_lowercase(), display, highlighted));
    }
}

impl Module for ChatLog {
    fn name(&self) -> &str {
        "chatlog"
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
                log!("chatlogを利用できません: {e}");
                return;
            }
            self.error = None;
        }
        self.enabled = on;
    }

    fn tick(&mut self, tick: &Tick) {
        let Some(batch) = self.chat.poll(self.enabled, tick.now_ms) else {
            return;
        };
        let time = timestamp();
        let mut lines = Vec::with_capacity(batch.messages.len() + 1);
        if batch.missed {
            lines.push(format!("[{time}] (some messages were not captured)"));
        }
        for message in &batch.messages {
            let line = format_line(&time, message);
            let highlighted = mention(message, &self.highlight).map(str::to_owned);
            self.remember(&line, highlighted.is_some());
            lines.push(line);
            if let Some(word) = highlighted.filter(|_| self.notifications) {
                notify::push(&format!("チャット（{word}）: {}", clean(&message.text)));
            }
        }
        if self.file {
            self.append(&lines);
        }
    }

    fn status(&self) -> String {
        match &self.error {
            Some(e) => format!("({e})"),
            None => format!("{}行", self.lines),
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "notifications",
                "キーワードを通知",
                "受信本文に登録語が含まれたときに通知します。",
                self.notifications,
            ),
            Detail::boolean(
                "file",
                "ファイルにも保存",
                "DLLと同じ場所のchat.logへ時刻付きで追記します。",
                self.file,
            ),
            Detail::text("highlight", self.highlight.join("\n")),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "notifications" => self.notifications = v.boolean()?,
            "file" => {
                self.file = v.boolean()?;
                if !self.file {
                    self.error = None;
                }
            }
            "highlight" => {
                let words = v.text()?.lines().map(str::to_owned).collect::<Vec<_>>();
                if words.len() > MAX_HIGHLIGHTS || words.iter().any(|s| s.chars().count() > 64) {
                    return Err("キーワードは16件まで、1件64文字までです".into());
                }
                self.highlight = normalize_words(&words);
            }
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.input_text("キーワード", &mut self.words_input).build();
        truncate_chars(&mut self.words_input, 65);
        ui.same_line();
        if ui.button("追加") && !self.words_input.trim().is_empty() {
            let mut words = self.highlight.clone();
            words.push(self.words_input.clone());
            match self.set_option(
                "highlight",
                &Value::Text {
                    text: words.join("\n"),
                },
            ) {
                Ok(()) => {
                    self.words_input.clear();
                    self.settings_error.clear();
                }
                Err(e) => self.settings_error = e,
            }
        }
        let mut remove = None;
        for (i, word) in self.highlight.iter().enumerate() {
            let _id = ui.push_id_usize(i);
            if ui.small_button("削除") {
                remove = Some(i);
            }
            ui.same_line();
            ui.text(word);
        }
        if let Some(i) = remove {
            self.highlight.remove(i);
        }
        if !self.settings_error.is_empty() {
            ui.text_wrapped(&self.settings_error);
        }
    }
    fn draw(&mut self, ui: &Ui) {
        if !crate::gui::menu_open() {
            return;
        }
        ui.window("ChatLog — チャット検索###chat_search")
            .size([600., 360.], Condition::FirstUseEver)
            .save_settings(false)
            .build(|| self.draw_search(ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_history_is_bounded_unicode_safe_and_keeps_original_case() {
        let mut chat = ChatLog::new(&config::ChatLog::default(), Path::new("."));
        for n in 0..600 {
            chat.remember(&format!("[time] <- Alex: Diamond {n} 日本語"), n % 2 == 0);
        }
        assert_eq!(chat.history.len(), MAX_HISTORY);
        let (lower, line, highlight) = chat.history.front().unwrap();
        assert!(line.contains("Diamond 88"));
        assert!(lower.contains("alex: diamond 88 日本語"));
        assert!(*highlight);
        chat.remember(&"界".repeat(5000), false);
        let (_, last, _) = chat.history.back().unwrap();
        assert_eq!(last.chars().count(), MAX_DISPLAY_CHARS + 1);
        assert!(last.ends_with('…'));
        let mut text = "abc日本語".to_string();
        assert!(truncate_chars(&mut text, 4));
        assert_eq!(text, "abc日");
        let before = chat.highlight.clone();
        assert!(chat
            .set_option(
                "highlight",
                &Value::Text {
                    text: "x".repeat(65)
                }
            )
            .is_err());
        assert_eq!(chat.highlight, before);
        assert_eq!(
            normalize_words(&["§aDIAMOND".into(), "  diamond  ".into()]),
            ["diamond"]
        );
    }

    fn message(direction: ChatDirection, sender: Option<&str>, text: &str) -> ChatMessage {
        ChatMessage {
            id: 1,
            direction,
            sender: sender.map(String::from),
            text: text.into(),
        }
    }

    #[test]
    fn formatting_codes_and_control_characters_are_removed() {
        assert_eq!(clean("§aHello §l§oworld§r!"), "Hello world!");
        assert_eq!(clean("line one\nline two\t."), "line one line two .");
        assert_eq!(clean("  spaced  "), "spaced");
        // A trailing lone section sign does not eat past the end.
        assert_eq!(clean("end§"), "end");
        assert_eq!(clean("日本語 §cテスト"), "日本語 テスト");
    }

    #[test]
    fn lines_show_direction_sender_and_time() {
        let time = "2026-11-15 04:40:12";
        assert_eq!(
            format_line(
                time,
                &message(ChatDirection::Received, Some("Steve"), "§ehi")
            ),
            "[2026-11-15 04:40:12] <- Steve: hi"
        );
        assert_eq!(
            format_line(
                time,
                &message(ChatDirection::Received, None, "Server restarting")
            ),
            "[2026-11-15 04:40:12] <- Server restarting"
        );
        assert_eq!(
            format_line(time, &message(ChatDirection::Received, Some(" "), "x")),
            "[2026-11-15 04:40:12] <- x"
        );
        assert_eq!(
            format_line(time, &message(ChatDirection::Sent, Some("me"), "/tp 1 2 3")),
            "[2026-11-15 04:40:12] -> /tp 1 2 3"
        );
    }

    #[test]
    fn highlights_match_received_messages_only_ignoring_case_and_formatting() {
        let words = normalize_words(&[
            "  Emanon ".into(),
            "DIAMOND".into(),
            "".into(),
            "emanon".into(),
        ]);
        assert_eq!(words, ["emanon", "diamond"]);
        let received = |text| message(ChatDirection::Received, Some("Alex"), text);
        assert_eq!(
            mention(&received("hey EMANON, come here"), &words),
            Some("emanon")
        );
        assert_eq!(
            mention(&received("found §bDia§bmond"), &words),
            Some("diamond")
        );
        assert_eq!(mention(&received("nothing here"), &words), None);
        assert_eq!(
            mention(&message(ChatDirection::Sent, None, "emanon"), &words),
            None
        );
        assert_eq!(mention(&received("emanon"), &[]), None);
        let many: Vec<String> = (0..40).map(|i| format!("w{i}")).collect();
        assert_eq!(normalize_words(&many).len(), MAX_HIGHLIGHTS);
    }

    #[test]
    fn lines_are_appended_to_the_file_and_failures_are_reported_once() {
        let dir = std::env::temp_dir().join(format!("mcbe_client_chat_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut log = ChatLog::new(&config::ChatLog::default(), &dir);
        log.append(&["[t] <- a: one".into(), "[t] -> two".into()]);
        log.append(&[]);
        log.append(&["[t] <- b: three".into()]);
        assert_eq!(log.lines, 3);
        assert_eq!(log.status(), "3行");
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert_eq!(text, "[t] <- a: one\r\n[t] -> two\r\n[t] <- b: three\r\n");

        // A path that cannot be a file: the error is kept for the status line.
        log.path = dir.clone();
        log.append(&["[t] lost".into()]);
        assert!(log.error.is_some());
        assert_eq!(log.lines, 3);
        assert!(log.status().starts_with('('));
        let _ = std::fs::remove_dir_all(&dir);

        // Outside the game there is no chat to log, so it refuses to turn on.
        log.set_enabled(true);
        assert!(!log.enabled());
        let stamp = timestamp();
        assert_eq!(stamp.len(), 19);
        assert_eq!(&stamp[4..5], "-");
    }
}
