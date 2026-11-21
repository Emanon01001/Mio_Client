//! JoinNotify: notices when players join or leave, from the game's own
//! system chat messages ("%sがゲームに参加しました", "%s joined the game",
//! and the Realm forms), with this session's history in its settings.
//! Reads the chat the SDK captures; sends nothing.

use std::collections::VecDeque;

use hudhook::imgui::Ui;

use super::chat_log::{clean, ChatCursor};
use super::settings::{Detail, Value};
use super::{key_or_warn, Module, Tick};
use crate::api::{ChatDirection, ChatMessage};
use crate::text::clock_hms;
use crate::{config, log, notify};

const KEPT: usize = 100;
/// Longer "names" are not player names (gamertags are at most 16 characters
/// plus a #1234 suffix).
const MAX_NAME: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Joined,
    Left,
}

/// The game's join/leave messages after the player name, from its language
/// files (ja_JP and en_US, `multiplayer.player.joined`/`.left` and the Realm
/// forms; the renamed forms end the same way).
const ENDINGS: [(&str, Event); 8] = [
    ("がゲームに参加しました", Event::Joined),
    ("がゲームを退出しました", Event::Left),
    (" が Realm にやってきました", Event::Joined),
    (" が Realm を去りました", Event::Left),
    (" joined the game", Event::Joined),
    (" left the game", Event::Left),
    (" joined the Realm", Event::Joined),
    (" left the Realm", Event::Left),
];
/// What the renamed forms put after the current name.
const RENAMED: [&str; 3] = [" (旧名", " (旧称 ", " (formerly known as "];

/// The player and event of a join/leave system message, if it is one.
/// Messages with a sender are chat from players, never these.
pub fn parse(message: &ChatMessage) -> Option<(String, Event)> {
    if message.direction != ChatDirection::Received
        || message.sender.as_deref().is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    let text = clean(&message.text);
    let (name, event) = ENDINGS
        .iter()
        .find_map(|&(ending, event)| text.strip_suffix(ending).map(|name| (name, event)))?;
    let name = RENAMED
        .iter()
        .find_map(|r| name.split_once(r).map(|(current, _)| current))
        .unwrap_or(name)
        .trim();
    (!name.is_empty() && name.chars().count() <= MAX_NAME).then(|| (name.to_string(), event))
}

pub struct JoinNotify {
    key: u16,
    enabled: bool,
    notifications: bool,
    chat: ChatCursor,
    /// (time, player, event), oldest first.
    history: VecDeque<(String, String, Event)>,
    joined: usize,
    left: usize,
}

impl JoinNotify {
    pub fn new(cfg: &config::JoinNotify) -> Self {
        Self {
            key: key_or_warn("joinnotify", &cfg.key),
            enabled: false,
            notifications: cfg.notifications,
            chat: ChatCursor::default(),
            history: VecDeque::new(),
            joined: 0,
            left: 0,
        }
    }

    fn record(&mut self, time: String, name: String, event: Event) {
        if self.notifications {
            notify::push(&match event {
                Event::Joined => format!("{name} が参加しました"),
                Event::Left => format!("{name} が退出しました"),
            });
        }
        match event {
            Event::Joined => self.joined += 1,
            Event::Left => self.left += 1,
        }
        if self.history.len() == KEPT {
            self.history.pop_front();
        }
        self.history.push_back((time, name, event));
    }
}

impl Module for JoinNotify {
    fn name(&self) -> &str {
        "joinnotify"
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
                log!("joinnotifyを利用できません: {e}");
                return;
            }
        }
        self.enabled = on;
    }
    fn tick(&mut self, tick: &Tick) {
        let Some(batch) = self.chat.poll(self.enabled, tick.now_ms) else {
            return;
        };
        for (name, event) in batch.messages.iter().filter_map(parse) {
            self.record(clock_hms(), name, event);
        }
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::boolean(
            "notifications",
            "通知を表示",
            "参加・退出のたびに画面に通知します（通知全体の設定も必要です）。",
            self.notifications,
        )]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "notifications" => self.notifications = value.boolean()?,
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }
    fn status(&self) -> String {
        if self.joined + self.left == 0 {
            "入退室待ち".into()
        } else {
            format!("参加{} / 退出{}", self.joined, self.left)
        }
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.separator();
        ui.text(format!(
            "このセッションの入退室（{}件）",
            self.history.len()
        ));
        if !self.history.is_empty() {
            ui.same_line();
            if ui.small_button("履歴を消去") {
                self.history.clear();
                self.joined = 0;
                self.left = 0;
            }
        }
        for (time, name, event) in self.history.iter().rev().take(30) {
            let (label, color) = match event {
                Event::Joined => ("参加", crate::theme::SUCCESS),
                Event::Left => ("退出", crate::theme::MUTED),
            };
            ui.text_colored(color, format!("{time}  {label}  {name}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(text: &str) -> ChatMessage {
        ChatMessage {
            id: 0,
            direction: ChatDirection::Received,
            sender: None,
            text: text.into(),
        }
    }

    #[test]
    fn join_and_leave_messages_in_both_languages_and_realm_forms() {
        let cases = [
            ("§eSteveがゲームに参加しました", "Steve", Event::Joined),
            ("§eSteveがゲームを退出しました", "Steve", Event::Left),
            (
                "Alex Smith (旧名Al) がゲームに参加しました",
                "Alex Smith",
                Event::Joined,
            ),
            ("Alex が Realm にやってきました", "Alex", Event::Joined),
            (
                "Alex (旧称 Al) が Realm にやってきました",
                "Alex",
                Event::Joined,
            ),
            ("Alex が Realm を去りました", "Alex", Event::Left),
            ("Steve joined the game", "Steve", Event::Joined),
            (
                "Steve (formerly known as Old) joined the game",
                "Steve",
                Event::Joined,
            ),
            ("Steve left the game", "Steve", Event::Left),
            ("Gamer#1234 joined the Realm", "Gamer#1234", Event::Joined),
            ("Gamer left the Realm", "Gamer", Event::Left),
        ];
        for (text, name, event) in cases {
            assert_eq!(
                parse(&system(text)),
                Some((name.to_string(), event)),
                "{text}"
            );
        }
    }

    #[test]
    fn player_chat_sent_messages_and_other_text_are_not_events() {
        let said = ChatMessage {
            sender: Some("Steve".into()),
            ..system("Alex joined the game")
        };
        assert_eq!(parse(&said), None);
        let sent = ChatMessage {
            direction: ChatDirection::Sent,
            ..system("Alex joined the game")
        };
        assert_eq!(parse(&sent), None);
        for text in [
            "joined the game",
            "がゲームに参加しました",
            "Welcome to the server",
            &format!("{} joined the game", "x".repeat(40)),
        ] {
            assert_eq!(parse(&system(text)), None, "{text}");
        }
    }

    #[test]
    fn events_are_counted_and_the_history_is_bounded() {
        let mut j = JoinNotify::new(&config::JoinNotify {
            notifications: false,
            ..Default::default()
        });
        for n in 0..150 {
            j.record(
                "t".into(),
                format!("p{n}"),
                if n % 3 == 0 {
                    Event::Left
                } else {
                    Event::Joined
                },
            );
        }
        assert_eq!(j.history.len(), KEPT);
        assert_eq!((j.joined, j.left), (100, 50));
        assert_eq!(j.history.back().unwrap().1, "p149");
        assert_eq!(j.status(), "参加100 / 退出50");
    }
}
