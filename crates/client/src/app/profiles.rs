//! Explicit snapshots of module switches and their menu-adjustable values.
use crate::{log, modules::Modules};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
const MAX_BYTES: u64 = 64 * 1024;
const MAX_MODULES: usize = 256;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub name: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub options: std::collections::BTreeMap<String, crate::modules::settings::Value>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    /// The ClickGUI key (absent in profiles saved before it was included,
    /// or without the overlay running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    menu_key: Option<String>,
    #[serde(rename = "module")]
    modules: Vec<Setting>,
}
impl Profile {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("未対応のプロファイルバージョン: {}", self.version));
        }
        // Unassigned (NONE) is never a valid ClickGUI key.
        if self
            .menu_key
            .as_ref()
            .is_some_and(|k| k.len() > 32 || crate::input::parse_key(k).is_none_or(|key| key == 0))
        {
            return Err("ClickGUIのキーが不正です".into());
        }
        if self.modules.is_empty() || self.modules.len() > MAX_MODULES {
            return Err("プロファイルの機能数は1～256件にしてください".into());
        }
        let mut names = std::collections::HashSet::new();
        for m in &self.modules {
            if m.name.is_empty() || m.name.len() > 128 || !names.insert(m.name.to_ascii_lowercase())
            {
                return Err("機能名が空、または重複しています".into());
            }
            if m.key
                .as_ref()
                .is_some_and(|k| k.len() > 32 || crate::input::parse_key(k).is_none())
                || m.options.len() > 32
                || m.options
                    .iter()
                    .any(|(id, v)| id.is_empty() || id.len() > 64 || !v.valid())
            {
                return Err(format!("{}: 詳細設定が不正です", m.name));
            }
            if m.value.is_some_and(|v| !v.is_finite()) {
                return Err(format!("{}: 有限の数値を指定してください", m.name));
            }
        }
        Ok(())
    }
}
fn valid_name(name: &str) -> bool {
    if name.is_empty()
        || name.chars().count() > 32
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    !matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                let mut chars = suffix.chars();
                matches!(chars.next(), Some('0'..='9' | '¹' | '²' | '³')) && chars.next().is_none()
            })
}
pub struct Profiles {
    dir: PathBuf,
    pub names: Vec<String>,
    pub status: String,
}
impl Profiles {
    pub fn new(dll_dir: &Path) -> Self {
        let mut this = Self {
            dir: dll_dir.join("profiles"),
            names: Vec::new(),
            status: "機能の設定を保存・読み込みできます".into(),
        };
        if let Err(e) = this.refresh() {
            this.status = e;
        }
        this
    }
    fn path(&self, name: &str) -> Result<PathBuf, String> {
        if !valid_name(name) {
            return Err(
                "名前は1～32文字の文字・数字・_・-で指定してください（Windowsの予約名は使用不可）"
                    .into(),
            );
        }
        Ok(self.dir.join(format!("{name}.toml")))
    }
    fn refresh(&mut self) -> Result<(), String> {
        self.names.clear();
        if !self.dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.dir).map_err(|e| format!("profiles: {e}"))? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                continue;
            }
            let path = entry.path();
            if path
                .extension()
                .and_then(|p| p.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("toml"))
            {
                if let Some(name) = path
                    .file_stem()
                    .and_then(|p| p.to_str())
                    .filter(|s| valid_name(s))
                {
                    self.names.push(name.to_string());
                    if self.names.len() > 128 {
                        self.names.clear();
                        return Err("プロファイルが多すぎます（最大128件）".into());
                    }
                }
            }
        }
        self.names.sort_by_key(|s| s.to_lowercase());
        Ok(())
    }
    fn write(
        &mut self,
        name: &str,
        settings: Vec<Setting>,
        menu_key: Option<u16>,
    ) -> Result<(), String> {
        let path = self.path(name)?;
        let profile = Profile {
            version: 1,
            menu_key: menu_key.map(crate::input::key_name),
            modules: settings,
        };
        profile.validate()?;
        let text = toml::to_string_pretty(&profile).map_err(|e| e.to_string())?;
        if text.len() as u64 > MAX_BYTES {
            return Err("プロファイルのファイルが大きすぎます".into());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
        self.refresh()?;
        Ok(())
    }
    /// The folder profiles are saved in (next to the DLL).
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn read(&self, name: &str) -> Result<Profile, String> {
        read_file(&self.path(name)?)
    }
}

/// Applies a profile: the ClickGUI key is checked before any module changes
/// (an invalid one changes nothing), and handed to the overlay only once the
/// modules were applied. Returns what was left out.
fn apply(profile: Profile, modules: &mut Modules) -> Result<Vec<String>, String> {
    let menu_key = match &profile.menu_key {
        Some(name) => crate::config::menu_key::check_profile_key(name)?,
        None => None,
    };
    let mut skipped = modules.apply_settings(&profile.modules)?;
    match menu_key {
        Some(key) => crate::config::menu_key::apply_from_profile(key),
        // Saved with a key, but the overlay is not running.
        None if profile.menu_key.is_some() => skipped.push("ClickGUI".into()),
        None => {}
    }
    Ok(skipped)
}

/// A profile file anywhere on disk, validated and bounded like the saved ones.
fn read_file(path: &Path) -> Result<Profile, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_BYTES {
        return Err("プロファイルのファイルが大きすぎます".into());
    }
    let profile: Profile = toml::from_str(&text).map_err(|e| e.to_string())?;
    profile.validate()?;
    Ok(profile)
}

/// A path typed or picked for `profile import`: the rest of the command line,
/// with surrounding quotes removed.
fn import_path(args: &[&str]) -> Option<PathBuf> {
    let joined = args.join(" ");
    let path = joined.trim().trim_matches('"').trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Describes what applying a profile left out.
fn applied(label: &str, skipped: Vec<String>) -> String {
    if skipped.is_empty() {
        format!("{label}を読み込みました")
    } else {
        format!("{label}を読み込みました（未適用: {}）", skipped.join(", "))
    }
}

impl Profiles {
    pub fn command(&mut self, args: &[&str], modules: &mut Modules) {
        let result: Result<String, String> = match args {
            [] | ["list"] => self.refresh().map(|_| {
                if self.names.is_empty() {
                    "保存済みプロファイルはありません".into()
                } else {
                    format!("プロファイル: {}", self.names.join(", "))
                }
            }),
            ["save", name] => self
                .write(name, modules.settings(), crate::config::menu_key::current())
                .map(|_| format!("プロファイル「{name}」を保存しました")),
            ["load", name] => self
                .read(name)
                .and_then(|profile| apply(profile, modules))
                .map(|skipped| applied(&format!("プロファイル「{name}」"), skipped)),
            ["import", rest @ ..] => match import_path(rest) {
                Some(path) => read_file(&path)
                    .and_then(|profile| apply(profile, modules))
                    .map(|skipped| applied(&format!("ファイル「{}」", path.display()), skipped)),
                None => Err("profile import <ファイルのパス>".into()),
            },
            _ => Err(
                "profile list | profile save <name> | profile load <name> | profile import <path>"
                    .into(),
            ),
        };
        self.status = match result {
            Ok(text) => text,
            Err(e) => format!("プロファイルのエラー: {e}"),
        };
        log!("{}", self.status);
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn text_options_do_not_change_legacy_key_strings_and_round_trip_targets() {
        use crate::modules::settings::Value;
        let old: super::Setting =
            toml::from_str("name='hud'\nenabled=true\n[options]\nkey='F6'").unwrap();
        assert_eq!(old.options["key"], Value::Key("F6".into()));
        let mut s = old;
        s.options.insert(
            "extra".into(),
            Value::Text {
                text: "diamond_block\n*shulker_box".into(),
            },
        );
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<super::Setting>(&text).unwrap(), s);
        assert!(!Value::Text {
            text: "a\0b".into()
        }
        .valid());
    }
    use super::*;
    #[test]
    fn legacy_profiles_keep_missing_details_and_new_details_round_trip() {
        let old: Profile =
            toml::from_str("version = 1\n[[module]]\nname = 'autosprint'\nenabled = true\n")
                .unwrap();
        old.validate().unwrap();
        assert!(old.modules[0].key.is_none() && old.modules[0].options.is_empty());
        let mut current = old;
        current.modules[0].key = Some("F6".into());
        current.modules[0]
            .options
            .insert("jump".into(), crate::modules::settings::Value::Bool(false));
        current.modules.push(Setting {
            name: "zoom".into(),
            enabled: false,
            value: Some(4.),
            key: Some("NONE".into()),
            options: [(
                "hold_key".into(),
                crate::modules::settings::Value::Key("C".into()),
            )]
            .into(),
        });
        current.validate().unwrap();
        let encoded = toml::to_string_pretty(&current).unwrap();
        let decoded: Profile = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.modules, current.modules);
        current.modules[0].options.insert(
            "jump".into(),
            crate::modules::settings::Value::Number(f64::NAN),
        );
        assert!(current.validate().is_err());
    }
    #[test]
    fn profiles_import_from_any_path_with_quotes_and_spaces() {
        assert_eq!(
            import_path(&[r#""C:\My Files\pvp  set.toml""#]),
            Some(PathBuf::from(r"C:\My Files\pvp  set.toml"))
        );
        assert_eq!(
            import_path(&["a", "b.toml"]),
            Some(PathBuf::from("a b.toml"))
        );
        assert_eq!(import_path(&[" \"\" "]), None);
        assert_eq!(import_path(&[]), None);
        let dir = std::env::temp_dir().join(format!("mio_profile_import_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("shared set.toml");
        std::fs::write(
            &good,
            "version = 1
[[module]]
name = 'zoom'
enabled = true
",
        )
        .unwrap();
        let settings = read_file(&good).unwrap().modules;
        assert_eq!(
            (settings[0].name.as_str(), settings[0].enabled),
            ("zoom", true)
        );
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "not a profile").unwrap();
        assert!(read_file(&bad).is_err());
        assert!(read_file(&dir.join("missing.toml")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn profiles_with_and_without_a_clickgui_key_are_read_and_bad_keys_rejected() {
        let old: Profile =
            toml::from_str("version = 1\n[[module]]\nname = 'zoom'\nenabled = true\n").unwrap();
        old.validate().unwrap();
        assert_eq!(old.menu_key, None);
        let new: Profile = toml::from_str(
            "version = 1\nmenu_key = \"RSHIFT\"\n[[module]]\nname = 'zoom'\nenabled = true\n",
        )
        .unwrap();
        new.validate().unwrap();
        assert_eq!(new.menu_key.as_deref(), Some("RSHIFT"));
        for bad in ["NOT_A_KEY", "", "NONE"] {
            let profile = Profile {
                menu_key: Some(bad.into()),
                ..new_profile()
            };
            assert!(profile.validate().is_err(), "{bad}");
        }
    }
    fn new_profile() -> Profile {
        toml::from_str("version = 1\n[[module]]\nname = 'zoom'\nenabled = true\n").unwrap()
    }
    #[test]
    fn names_cannot_escape_the_directory_or_use_reserved_windows_paths() {
        for name in [
            "", "..", "../test", "a/b", r"a\b", "x.toml", "C:foo", "a b", "NUL", "con", "COM1",
            "LPT9",
        ] {
            assert!(!valid_name(name), "{name}");
        }
        for name in ["default", "explore-1", "戦闘", "COM10"] {
            assert!(valid_name(name), "{name}");
        }
    }
    #[test]
    fn saved_profiles_round_trip_replace_existing_and_reject_invalid_input() {
        let root = std::env::temp_dir().join(format!("mio_profiles_{}", std::process::id()));
        let mut p = Profiles::new(&root);
        let settings = vec![Setting {
            name: "killaura".into(),
            enabled: false,
            value: Some(3.),
            key: None,
            options: Default::default(),
        }];
        p.write("default", settings.clone(), None).unwrap();
        let read = p.read("default").unwrap();
        assert_eq!((read.modules, read.menu_key), (settings.clone(), None));
        let replacement = vec![Setting {
            enabled: true,
            ..settings[0].clone()
        }];
        // The ClickGUI key is saved by name next to the modules.
        p.write("default", replacement.clone(), Some(0x2d)).unwrap();
        let read = p.read("default").unwrap();
        assert_eq!(read.modules, replacement);
        assert_eq!(read.menu_key.as_deref(), Some("INSERT"));
        assert!(std::fs::read_to_string(p.path("default").unwrap())
            .unwrap()
            .contains("menu_key = \"INSERT\""));
        assert_eq!(p.names, ["default"]);
        assert!(p
            .write("bad", vec![settings[0].clone(), settings[0].clone()], None)
            .is_err());
        assert!(p
            .write(
                "bad",
                vec![Setting {
                    value: Some(f64::NAN),
                    ..settings[0].clone()
                }],
                None
            )
            .is_err());
        let path = p.path("default").unwrap();
        std::fs::write(&path, "version = 9\nmodule = []\n").unwrap();
        assert!(p.read("default").is_err());
        std::fs::write(&path, "x".repeat(MAX_BYTES as usize + 1)).unwrap();
        assert!(p.read("default").is_err());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(root.join("profiles"));
        let _ = std::fs::remove_dir(root);
    }
}
