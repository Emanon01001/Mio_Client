//! Key binds: run any console command with a key (`bind G toggle esp`).
//! Saved next to the DLL in `Mio_Client_binds.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::input;
use crate::log;

pub const FILE_NAME: &str = "Mio_Client_binds.toml";
const MAX_BINDS: usize = 48;
const MAX_COMMAND: usize = 160;

/// One key and the command it runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Bind {
    key: String,
    command: String,
}

/// The saved file: `[[bind]]` tables.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Store {
    #[serde(default, rename = "bind")]
    list: Vec<Bind>,
}

impl Store {
    fn find(&self, vk: u16) -> Option<usize> {
        self.list
            .iter()
            .position(|b| input::parse_key(&b.key) == Some(vk))
    }

    /// Binds `key` to `command`, replacing what the key did before.
    fn set(&mut self, key: &str, command: &str) -> Result<String, String> {
        let vk = input::parse_key(key)
            .filter(|&vk| vk != 0)
            .ok_or_else(|| format!("unknown key `{key}`"))?;
        let command = command.trim();
        if command.is_empty() {
            return Err("no command given".into());
        }
        if command.chars().count() > MAX_COMMAND {
            return Err(format!(
                "the command is longer than {MAX_COMMAND} characters"
            ));
        }
        // A bind that redefines binds, or ejects on a stray key press, is a trap.
        let word = command.split_whitespace().next().unwrap_or("");
        if matches!(word, "bind" | "unbind") {
            return Err("a bind cannot run `bind` or `unbind`".into());
        }
        let name = input::key_name(vk);
        let bind = Bind {
            key: name.clone(),
            command: command.to_string(),
        };
        match self.find(vk) {
            Some(i) => self.list[i] = bind,
            None if self.list.len() >= MAX_BINDS => {
                return Err(format!("at most {MAX_BINDS} binds"));
            }
            None => self.list.push(bind),
        }
        Ok(name)
    }

    fn remove(&mut self, key: &str) -> bool {
        match input::parse_key(key).and_then(|vk| self.find(vk)) {
            Some(i) => {
                self.list.remove(i);
                true
            }
            None => false,
        }
    }
}

/// The binds, and which of their keys were down at the last poll.
pub struct Binds {
    path: Option<PathBuf>,
    store: Store,
    /// Keys held at the last poll, so a held key fires once.
    down: [u64; 4],
    /// Parsed once on load/edit. The on-disk strings/command order are unchanged.
    keys: Vec<u16>,
}

impl Binds {
    /// Loads the saved binds from `dir` (the DLL's folder).
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        let store = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log!(
                    "binds: {} is invalid ({e}) - starting empty",
                    path.display()
                );
                Store::default()
            }),
            Err(_) => Store::default(),
        };
        let keys = store
            .list
            .iter()
            .map(|b| input::parse_key(&b.key).unwrap_or(0))
            .collect();
        Self {
            path: Some(path),
            store,
            down: [0; 4],
            keys,
        }
    }
    fn rebuild_keys(&mut self) {
        self.keys = self
            .store
            .list
            .iter()
            .map(|b| input::parse_key(&b.key).unwrap_or(0))
            .collect();
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let result = toml::to_string(&self.store)
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
        if let Err(e) = result {
            log!("キー割り当てを保存できません: {}: {e}", path.display());
        }
    }

    /// The commands to run now: one per bind whose key has just gone down.
    /// `allowed` is false while keys are being typed somewhere (chat, menus),
    /// which also swallows the press so it does not fire later.
    pub fn poll(&mut self, key_down: impl Fn(u16) -> bool, allowed: bool) -> Vec<String> {
        let mut fired = Vec::new();
        let mut now_down = [0u64; 4];
        for (bind, &vk) in self.store.list.iter().zip(&self.keys) {
            if vk == 0 {
                continue;
            }
            let word = vk as usize / 64;
            let bit = 1u64 << (vk % 64);
            if key_down(vk) {
                if allowed && self.down[word] & bit == 0 {
                    fired.push(bind.command.clone());
                }
                now_down[word] |= bit;
            }
        }
        self.down = now_down;
        fired
    }

    /// The `bind` / `unbind` / `binds` console commands. `line` is the whole
    /// command line, so the bound command keeps its own spacing and arguments.
    pub fn command(&mut self, line: &str) {
        let line = line.trim();
        let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match (word, rest) {
            ("binds", _) | ("bind", "" | "list") => {
                if self.store.list.is_empty() {
                    log!("キー割り当てはありません。bind <キー> <コマンド>で追加できます（例: bind G toggle esp）");
                }
                for bind in &self.store.list {
                    log!("  {:<10} {}", bind.key, bind.command);
                }
            }
            ("bind", _) => {
                let (key, command) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                match self.store.set(key, command) {
                    Ok(name) => {
                        self.rebuild_keys();
                        self.save();
                        log!("{name}に割り当てました: {}", command.trim());
                    }
                    Err(e) => {
                        log!("bind: {e}");
                        log!("使い方: bind <キー> <コマンド> | bind list | unbind <キー>");
                    }
                }
            }
            ("unbind", key) if !key.is_empty() => {
                if self.store.remove(key) {
                    self.rebuild_keys();
                    self.save();
                    log!("{}の割り当てを解除しました", key.to_ascii_uppercase());
                } else {
                    log!("unbind: `{key}`に割り当てはありません（bind listで確認）");
                }
            }
            _ => log!("使い方: bind <キー> <コマンド> | bind list | unbind <キー>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binds() -> Binds {
        Binds {
            path: None,
            store: Store::default(),
            down: [0; 4],
            keys: Vec::new(),
        }
    }

    #[test]
    fn binding_validates_keys_and_commands_and_replaces_per_key() {
        let mut store = Store::default();
        assert_eq!(store.set("g", "toggle esp"), Ok("G".into()));
        assert_eq!(store.set("F6", "  set fov 140 "), Ok("F6".into()));
        assert_eq!(store.list[1].command, "set fov 140");
        // The same key (by any spelling) is rebound, not duplicated.
        assert_eq!(store.set("0x47", "toggle fly"), Ok("G".into()));
        assert_eq!(store.list.len(), 2);
        assert_eq!(store.list[0].command, "toggle fly");

        for (key, command) in [
            ("bogus", "list"),
            ("NONE", "list"),
            ("G", "   "),
            ("G", "bind H toggle esp"),
            ("G", "unbind G"),
        ] {
            assert!(store.set(key, command).is_err(), "{key} {command}");
        }
        assert!(store.set("H", &"x".repeat(MAX_COMMAND + 1)).is_err());
        assert_eq!(store.list.len(), 2);

        assert!(store.remove("g"));
        assert!(!store.remove("g"));
        assert!(!store.remove("bogus"));
        assert_eq!(store.list.len(), 1);
    }

    #[test]
    fn a_key_fires_once_per_press_and_never_while_typing() {
        let mut binds = binds();
        binds.store.set("G", "toggle esp").unwrap();
        binds.store.set("H", "wp add here").unwrap();
        binds.rebuild_keys();
        let g = input::parse_key("G").unwrap();
        let h = input::parse_key("H").unwrap();

        assert!(binds.poll(|_| false, true).is_empty());
        assert_eq!(binds.poll(|vk| vk == g, true), ["toggle esp"]);
        // Still held: nothing more.
        assert!(binds.poll(|vk| vk == g, true).is_empty());
        // A second key while the first is held fires only the new one.
        assert_eq!(binds.poll(|vk| vk == g || vk == h, true), ["wp add here"]);
        assert!(binds.poll(|_| false, true).is_empty());
        assert_eq!(binds.poll(|vk| vk == g, true), ["toggle esp"]);

        // Pressed while typing in chat: ignored, even after typing ends with
        // the key still down.
        assert!(binds.poll(|_| false, true).is_empty());
        assert!(binds.poll(|vk| vk == h, false).is_empty());
        assert!(binds.poll(|vk| vk == h, true).is_empty());
        assert!(binds.poll(|_| false, true).is_empty());
        assert_eq!(binds.poll(|vk| vk == h, true), ["wp add here"]);
    }

    #[test]
    fn commands_edit_the_list_and_the_file_round_trips() {
        let mut binds = binds();
        binds.command("bind g toggle   esp");
        binds.command("bind F7 set zoom 8");
        binds.command("bind");
        binds.command("bind nonsense-key list");
        binds.command("unbind f7");
        binds.command("unbind f7");
        assert_eq!(
            binds.store.list,
            [Bind {
                key: "G".into(),
                // The command keeps its own spacing.
                command: "toggle   esp".into()
            }]
        );
        let text = toml::to_string(&binds.store).unwrap();
        assert!(text.contains("[[bind]]"), "{text}");
        assert_eq!(toml::from_str::<Store>(&text).unwrap(), binds.store);
        assert_eq!(toml::from_str::<Store>("").unwrap(), Store::default());
    }
    #[test]
    fn edits_preserve_held_edges_and_invalid_or_duplicate_file_keys() {
        let mut b = binds();
        b.command("bind G first");
        assert_eq!(b.poll(|k| k == 0x47, true), ["first"]);
        b.command("bind G second");
        assert!(b.poll(|k| k == 0x47, true).is_empty());
        b.poll(|_| false, true);
        assert_eq!(b.poll(|k| k == 0x47, true), ["second"]);
        b.command("unbind G");
        assert!(b.poll(|_| true, true).is_empty());
        b.store.list = vec![
            Bind {
                key: "bad key".into(),
                command: "invalid".into(),
            },
            Bind {
                key: "NONE".into(),
                command: "none".into(),
            },
            Bind {
                key: "F1".into(),
                command: "a".into(),
            },
            Bind {
                key: "f1".into(),
                command: "b".into(),
            },
        ];
        b.rebuild_keys();
        assert_eq!(b.poll(|_| true, true), ["a", "b"]);
        assert!(b.poll(|_| true, true).is_empty());
    }

    #[test]
    fn file_is_saved_and_loaded_from_the_dll_folder() {
        let dir = std::env::temp_dir().join(format!("mio_client_binds_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut binds = Binds::load(&dir);
        assert!(binds.store.list.is_empty());
        binds.command("bind K coords copy");
        let again = Binds::load(&dir);
        assert_eq!(again.store, binds.store);
        assert_eq!(again.store.list[0].command, "coords copy");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
