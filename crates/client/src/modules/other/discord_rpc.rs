//! DiscordRPC: shows what the player is doing on their Discord profile
//! (Rich Presence) through the local Discord client. The game title Discord
//! shows is the name of the Discord application: Mio Client's own
//! (`BUILT_IN_APPLICATION_ID`) unless the config names another.
//!
//! Once a second the module works out the place (menu, single player or a
//! server) and dimension, and hands the activity to the connection thread
//! (`ipc`), which sends it only when it changed.

mod ipc;

use hudhook::imgui::Ui;

use crate::modules::{
    game::{self, session_info, world::Realm},
    key_or_warn,
    settings::{Detail, Value},
    Module, Tick,
};
use crate::{config, log, net};

const UPDATE_MS: u64 = 1000;

/// Where the player is, as far as the profile is concerned.
#[derive(Clone, Debug, PartialEq)]
enum Place {
    /// The game hook is unavailable: in the game, nothing more known.
    Unknown,
    Menu,
    Single,
    /// The address only tells servers apart (the elapsed time restarts on
    /// another server); it is never shown.
    Server(String),
}

/// Mio Client's Discord application, used when the config leaves
/// `application_id` empty. An Application ID is public (every Rich Presence
/// client sends it); no secret is needed for the local RPC.
pub(crate) const BUILT_IN_APPLICATION_ID: &str = "1574304177717383291";

/// An Application ID is a Discord snowflake: 17 to 20 digits.
fn valid_id(id: &str) -> bool {
    (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit())
}

/// The application to connect as: the configured one, Mio Client's when
/// none is configured, or None when the configured one is malformed.
fn application_id(configured: &str) -> Option<&str> {
    match configured.trim() {
        "" => Some(BUILT_IN_APPLICATION_ID),
        id if valid_id(id) => Some(id),
        _ => None,
    }
}

fn realm_name(name: &str, realm: Realm) -> String {
    match realm {
        Realm::Overworld => "オーバーワールド".into(),
        Realm::Nether => "ネザー".into(),
        Realm::End => "ジ・エンド".into(),
        Realm::Other => name.to_string(),
    }
}

/// The activity for `place`, entered at `since` (Unix seconds), with the
/// world's name and player count (`session`) and dimension when known.
fn activity(
    cfg: &config::DiscordRpc,
    place: &Place,
    session: Option<&session_info::Info>,
    dimension: Option<&(String, Realm)>,
    since: u64,
) -> ipc::Activity {
    let details = match place {
        Place::Unknown => "プレイ中",
        Place::Menu => "メニュー画面",
        Place::Single => "シングルプレイ",
        Place::Server(_) => "マルチプレイ",
    };
    let mut parts = Vec::new();
    // Readings left over from the last world are not shown in the menu.
    if matches!(place, Place::Single | Place::Server(_)) {
        if let Some(session) = session {
            if let Some(world) = session.world.as_ref().filter(|_| cfg.show_world) {
                parts.push(world.clone());
            }
            // Alone in a single-player world the count says nothing.
            let multiplayer = matches!(place, Place::Server(_));
            if let Some(players) = session
                .players
                .filter(|&n| cfg.show_players && (multiplayer || n > 1))
            {
                parts.push(format!("{players}人"));
            }
        }
        if let Some((name, realm)) = dimension.filter(|_| cfg.show_dimension) {
            parts.push(realm_name(name, *realm));
        }
    }
    let image = cfg.large_image.trim();
    ipc::Activity {
        details: details.into(),
        state: (!parts.is_empty()).then(|| parts.join(" · ")),
        start: cfg.show_elapsed.then_some(since),
        large_image: (!image.is_empty()).then(|| image.to_string()),
        large_text: Some(cfg.large_text.clone()),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub struct DiscordRpc {
    cfg: config::DiscordRpc,
    key: u16,
    worker: Option<ipc::Worker>,
    /// The game hook is held (world detection works).
    hooked: bool,
    /// Keeps the world name and player count coming while enabled.
    session: Option<session_info::Subscription>,
    next_update: u64,
    /// The current place and when it began (Unix seconds).
    since: Option<(Place, u64)>,
}

impl DiscordRpc {
    pub fn new(c: &config::DiscordRpc) -> Self {
        Self {
            cfg: c.clone(),
            key: key_or_warn("discordrpc", &c.key),
            worker: None,
            hooked: false,
            session: None,
            next_update: 0,
            since: None,
        }
    }

    fn place(&self) -> Place {
        if !self.hooked {
            Place::Unknown
        } else if game::position().is_none() {
            Place::Menu
        } else {
            net::current().map_or(Place::Single, Place::Server)
        }
    }

    /// Sends the current activity to the connection thread now.
    fn update(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        let place = self.place();
        let now = unix_now();
        let since = match &self.since {
            Some((p, at)) if *p == place => *at,
            _ => {
                self.since = Some((place.clone(), now));
                now
            }
        };
        let dimension = game::world::dimension();
        let session = session_info::info();
        // A malformed ID connects to nothing (the status says why).
        let id = application_id(&self.cfg.application_id).unwrap_or_default();
        worker.set(
            id,
            Some(activity(
                &self.cfg,
                &place,
                session.as_ref(),
                dimension.as_ref(),
                since,
            )),
        );
    }
}

impl Module for DiscordRpc {
    fn name(&self) -> &str {
        "discordrpc"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.worker.is_some()
    }
    fn set_enabled(&mut self, on: bool) {
        if on == self.enabled() {
            return;
        }
        if on {
            match ipc::Worker::start(ipc::PIPE_PREFIX) {
                Ok(worker) => self.worker = Some(worker),
                Err(e) => {
                    log!("discordrpc: 接続スレッドを開始できません: {e}");
                    return;
                }
            }
            // Without the hook it still shows that the game is running.
            match game::acquire() {
                Ok(()) => {
                    self.hooked = true;
                    self.session = Some(session_info::subscribe());
                }
                Err(e) => log!("discordrpc: ワールドの判定を使わずに表示します（{e}）"),
            }
            self.since = None;
            self.next_update = 0;
        } else {
            // Clears the activity and joins the connection thread.
            self.worker = None;
            self.session = None;
            if std::mem::take(&mut self.hooked) {
                if let Err(e) = game::release() {
                    log!("discordrpc: {e}");
                }
            }
        }
    }
    fn tick(&mut self, t: &Tick) {
        if self.worker.is_none() || t.now_ms < self.next_update {
            return;
        }
        self.next_update = t.now_ms + UPDATE_MS;
        self.update();
    }
    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "show_world",
                "ワールド名を表示",
                "ワールド一覧に出る名前を表示します。サーバーでは、サーバーが設定したワールド名になります。",
                self.cfg.show_world,
            ),
            Detail::boolean(
                "show_players",
                "人数を表示",
                "ワールドにいるプレイヤーの人数（自分を含む）を表示します。シングルプレイで1人のときは表示しません。",
                self.cfg.show_players,
            ),
            Detail::boolean(
                "show_dimension",
                "ディメンションを表示",
                "ワールドにいる間、オーバーワールド・ネザー・ジ・エンドを表示します。",
                self.cfg.show_dimension,
            ),
            Detail::boolean(
                "show_elapsed",
                "経過時間を表示",
                "今のワールド（またはメニュー）に入ってからの時間を表示します。",
                self.cfg.show_elapsed,
            ),
            Detail::text("application_id", self.cfg.application_id.clone()),
            Detail::text("large_image", self.cfg.large_image.clone()),
            Detail::text("large_text", self.cfg.large_text.clone()),
        ]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        match id {
            "show_world" => self.cfg.show_world = v.boolean()?,
            "show_players" => self.cfg.show_players = v.boolean()?,
            "show_dimension" => self.cfg.show_dimension = v.boolean()?,
            "show_elapsed" => self.cfg.show_elapsed = v.boolean()?,
            "application_id" => self.cfg.application_id = v.text()?.trim().to_string(),
            "large_image" => self.cfg.large_image = v.text()?.trim().to_string(),
            "large_text" => self.cfg.large_text = v.text()?.to_string(),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        self.next_update = 0;
        Ok(())
    }
    fn status(&self) -> String {
        let malformed = application_id(&self.cfg.application_id).is_none();
        match &self.worker {
            None => String::new(),
            Some(_) if malformed => {
                "Application IDは17～20桁の数字です（空欄で組み込みのIDを使用）".into()
            }
            Some(worker) => worker.status(),
        }
    }
    fn draw_settings(&mut self, ui: &Ui) {
        let mut changed = false;
        ui.text_colored(crate::theme::MUTED, "Application ID（空欄: Mio Client）");
        ui.set_next_item_width(-1.);
        if ui
            .input_text("##application_id", &mut self.cfg.application_id)
            .chars_decimal(true)
            .build()
        {
            self.cfg.application_id.truncate(20);
            changed = true;
        }
        ui.text_colored(crate::theme::MUTED, "大きい画像のキー（任意）");
        ui.set_next_item_width(-1.);
        if ui
            .input_text("##large_image", &mut self.cfg.large_image)
            .build()
        {
            if let Some((at, _)) = self.cfg.large_image.char_indices().nth(64) {
                self.cfg.large_image.truncate(at);
            }
            changed = true;
        }
        ui.text_colored(crate::theme::MUTED, "画像にカーソルを置いたときの文字");
        ui.set_next_item_width(-1.);
        if ui
            .input_text("##large_text", &mut self.cfg.large_text)
            .build()
        {
            if let Some((at, _)) = self.cfg.large_text.char_indices().nth(128) {
                self.cfg.large_text.truncate(at);
            }
            changed = true;
        }
        if changed {
            self.next_update = 0;
        }
        ui.text_wrapped(
            "Application IDが空欄なら、Mio Clientのアプリケーションとして表示します。\
             別の名前で表示したい場合は、Discord Developer Portal（discord.com/developers/applications）で\
             作ったアプリケーションのIDを入力してください（アプリケーション名がゲーム名として表示されます）。\
             画像はそのアプリケーションのRich Presence > Art Assetsに登録したキーを指定します。",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> config::DiscordRpc {
        config::DiscordRpc::default()
    }

    fn session(world: Option<&str>, players: Option<usize>) -> session_info::Info {
        session_info::Info {
            world: world.map(Into::into),
            players,
        }
    }

    #[test]
    fn servers_show_the_world_name_and_player_count_but_never_the_address() {
        let server = Place::Server("play.example.net:19132".into());
        let nether = ("Nether".to_string(), Realm::Nether);
        let info = session(Some("Survival"), Some(12));
        let a = activity(&cfg(), &server, Some(&info), Some(&nether), 100);
        assert_eq!(a.details, "マルチプレイ");
        assert_eq!(a.state.as_deref(), Some("Survival · 12人 · ネザー"));
        assert_eq!(a.start, Some(100));
        assert!(!format!("{:?}", a.to_json()).contains("example.net"));
        // Parts not read on this build are left out.
        let a = activity(&cfg(), &server, Some(&session(None, Some(3))), None, 100);
        assert_eq!(a.state.as_deref(), Some("3人"));
        let hidden = config::DiscordRpc {
            show_world: false,
            show_players: false,
            ..cfg()
        };
        let a = activity(&hidden, &server, Some(&info), Some(&nether), 100);
        assert_eq!(a.state.as_deref(), Some("ネザー"));
    }

    #[test]
    fn single_player_counts_only_company_and_menus_show_no_world() {
        let overworld = ("Overworld".to_string(), Realm::Overworld);
        let alone = session(Some("My World"), Some(1));
        let a = activity(&cfg(), &Place::Single, Some(&alone), Some(&overworld), 5);
        assert_eq!(a.details, "シングルプレイ");
        assert_eq!(a.state.as_deref(), Some("My World · オーバーワールド"));
        let hosting = session(Some("My World"), Some(3));
        let a = activity(&cfg(), &Place::Single, Some(&hosting), None, 5);
        assert_eq!(a.state.as_deref(), Some("My World · 3人"));
        // Readings left over from the last world are not shown in the menu.
        let a = activity(&cfg(), &Place::Menu, Some(&hosting), Some(&overworld), 5);
        assert_eq!((a.details.as_str(), a.state), ("メニュー画面", None));
        let quiet = config::DiscordRpc {
            show_world: false,
            show_dimension: false,
            show_elapsed: false,
            ..cfg()
        };
        let a = activity(&quiet, &Place::Single, Some(&alone), Some(&overworld), 5);
        assert_eq!(
            (a.details.as_str(), a.state, a.start),
            ("シングルプレイ", None, None)
        );
        let a = activity(&cfg(), &Place::Unknown, None, None, 5);
        assert_eq!(a.details, "プレイ中");
        // Custom dimensions keep their own name; no image key: no image.
        let custom = ("Moon".to_string(), Realm::Other);
        let a = activity(&cfg(), &Place::Single, None, Some(&custom), 5);
        assert_eq!(a.state.as_deref(), Some("Moon"));
        assert_eq!(a.large_image, None);
    }

    #[test]
    fn an_empty_application_id_uses_mio_clients_own() {
        assert!(valid_id(BUILT_IN_APPLICATION_ID));
        assert_eq!(application_id(""), Some(BUILT_IN_APPLICATION_ID));
        assert_eq!(application_id("  "), Some(BUILT_IN_APPLICATION_ID));
        assert_eq!(cfg().application_id, "");
        // A configured one wins; a malformed one is not replaced silently.
        assert_eq!(
            application_id(" 123456789012345678 "),
            Some("123456789012345678")
        );
        assert_eq!(application_id("12345"), None);
    }

    #[test]
    fn application_ids_are_discord_snowflakes() {
        assert!(valid_id("123456789012345678"));
        assert!(valid_id("12345678901234567"));
        assert!(!valid_id("1234567890123456"));
        assert!(!valid_id("12345678901234567890 "));
        assert!(!valid_id("12345678901234567a"));
        assert!(!valid_id(""));
    }
}
