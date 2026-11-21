//! Update only gui.menu_key in the original TOML; I/O stays on the client worker.
use super::{DEFAULT_TEXT, FILE_NAME};
use crate::input;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use toml::{Spanned, Value};

static PENDING: Mutex<Option<u16>> = Mutex::new(None);
static STATUS: Mutex<String> = Mutex::new(String::new());
static RETRY_AT: AtomicU64 = AtomicU64::new(0);
const MAX_BYTES: u64 = 1024 * 1024;

pub(crate) fn validate(key: u16, eject: u16) -> Result<(), String> {
    if key == 0 || key > 255 {
        return Err("ClickGUIのキーは未割り当てにできません".into());
    }
    if key == eject {
        return Err("クライアントの終了キーはClickGUIに割り当てられません".into());
    }
    if matches!(key, 1 | 2 | 4) {
        return Err(
            "ClickGUIには通常のマウスボタンを使えません（MOUSE4・MOUSE5は使用できます）".into(),
        );
    }
    Ok(())
}
pub(crate) fn request(key: u16) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(key);
    RETRY_AT.store(0, Ordering::Relaxed);
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = "変更を保存しています…".into();
}
pub(crate) fn status() -> String {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(Deserialize)]
struct Document {
    gui: Option<Spanned<BTreeMap<String, Spanned<Value>>>>,
}
fn replace_key(source: &str, key: u16) -> Result<String, String> {
    let document: Document =
        toml::from_str(source).map_err(|e| format!("設定ファイルの構文を確認してください: {e}"))?;
    let value = format!("\"{}\"", input::key_name(key));
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut result = source.to_owned();
    match document.gui {
        Some(gui) => {
            if let Some(existing) = gui.get_ref().get("menu_key") {
                result.replace_range(existing.span(), &value);
            } else {
                let span = gui.span();
                let raw = source.get(span.clone()).ok_or("設定の取得範囲が不正です")?;
                if raw.starts_with('{') {
                    let comma = if gui.get_ref().is_empty() { "" } else { "," };
                    result.insert_str(span.start + 1, &format!(" menu_key = {value}{comma} "));
                } else {
                    // A table's span begins at its header. Insert immediately
                    // after that header, before any nested table or value.
                    let line_start = source[..span.start].rfind('\n').map_or(0, |i| i + 1);
                    let line_end = source[line_start..]
                        .find('\n')
                        .map(|i| line_start + i + 1)
                        .unwrap_or(source.len());
                    let header = source[line_start..line_end].trim();
                    let probe: Value = toml::from_str(&format!("{header}\n__mio_probe = true\n"))
                        .map_err(|_| "ClickGUI設定のテーブル位置を確認できません")?;
                    if probe
                        .get("gui")
                        .and_then(|g| g.get("__mio_probe"))
                        .and_then(Value::as_bool)
                        != Some(true)
                    {
                        if probe.get("gui").is_none() {
                            return Err("ClickGUI設定のテーブル位置を確認できません".into());
                        }
                        let entry = if header.starts_with('[') {
                            format!("[gui]{newline}menu_key = {value}{newline}")
                        } else {
                            format!("gui.menu_key = {value}{newline}")
                        };
                        result.insert_str(line_start, &entry);
                    } else {
                        let prefix = if source[..line_end].ends_with('\n') {
                            ""
                        } else {
                            newline
                        };
                        result
                            .insert_str(line_end, &format!("{prefix}menu_key = {value}{newline}"));
                    }
                }
            }
        }
        None => {
            if !result.is_empty() && !result.ends_with('\n') {
                result.push_str(newline);
            }
            result.push_str(&format!("[gui]{newline}menu_key = {value}{newline}"));
        }
    }
    let checked: Document =
        toml::from_str(&result).map_err(|e| format!("保存前の設定検証に失敗しました: {e}"))?;
    if checked
        .gui
        .as_ref()
        .and_then(|g| g.get_ref().get("menu_key"))
        .and_then(|v| v.get_ref().as_str())
        != Some(input::key_name(key).as_str())
    {
        return Err("変更後のClickGUIキーを確認できません".into());
    }
    Ok(result)
}
fn read(path: &Path) -> Result<Option<String>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("設定ファイルが1MiBの上限を超えています".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}
fn save(dir: &Path, key: u16) -> Result<(), String> {
    let path = dir.join(FILE_NAME);
    let before = read(&path)?;
    let text = replace_key(before.as_deref().unwrap_or(DEFAULT_TEXT), key)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = dir.join(format!(
        "{FILE_NAME}.menu-key.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = file
        .write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string());
    drop(file);
    let result = result.and_then(|_| {
        if read(&path)? != before {
            return Err("設定が編集中のため保存を再試行します".into());
        }
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
pub(crate) fn poll(dir: &Path) {
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    if now < RETRY_AT.load(Ordering::Relaxed) {
        return;
    }
    let Some(key) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let message = match save(dir, key) {
        Ok(()) => format!(
            "{}を保存しました（次回起動後も有効）",
            input::display_key_name(key)
        ),
        Err(e) => {
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            if pending.is_none() {
                *pending = Some(key);
            }
            RETRY_AT.store(now + 5000, Ordering::Relaxed);
            format!("現在のキーは変更済みです。設定保存を再試行します: {e}")
        }
    };
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = message;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_only_the_key_and_keeps_comments_unicode_crlf_and_other_values() {
        let source="# 日本語\r\n[gui] # UI\r\nmenu_key = 'L' # keep\r\nfont_size=18\r\n[zoom]\r\nfactor=10.0\r\n";
        assert_eq!(
            replace_key(source, 0x75).unwrap(),
            source.replace("'L'", "\"F6\"")
        );
    }
    #[test]
    fn inserts_missing_keys_in_tables_inline_tables_and_missing_gui_sections() {
        for source in [
            "[gui]\nenabled=true\n[zoom]\nfactor=10.0\n",
            "[gui]\n",
            "['gui'] # quoted\n[gui.extra]\nvalue=1\n",
            "gui = { enabled = true }\n",
            "gui = {}\n",
            "[zoom]\nfactor=10.0",
            "gui.enabled = true\n",
            "[gui.extra]\nvalue=1\n",
            "\u{feff}[gui]\nmenu_key='L'\n",
        ] {
            let result = replace_key(source, 0x75).unwrap_or_else(|e| panic!("{source:?}: {e}"));
            let parsed: super::super::Config = toml::from_str(&result).unwrap();
            assert_eq!(parsed.gui.menu_key, "F6");
            assert_eq!(
                parsed.zoom.factor,
                if source.contains("10.0") { 10. } else { 4. }
            );
        }
    }
    #[test]
    fn invalid_toml_and_unusable_menu_keys_are_rejected() {
        assert!(replace_key("[gui", 0x75).is_err());
        assert!(validate(0, 0x23).is_err());
        assert!(validate(0x23, 0x23).is_err());
        assert!(validate(0x75, 0x23).is_ok());
        assert!(validate(1, 0x23).is_err());
        assert!(validate(5, 0x23).is_ok());
    }

    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "mio-gui-key-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            for entry in std::fs::read_dir(&self.0).unwrap().flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    #[test]
    fn failed_atomic_replacement_preserves_the_file_then_saved_key_survives_reload() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = Directory::new();
        let path = dir.0.join(FILE_NAME);
        let source = "# keep comments\n[gui]\nmenu_key='L'\nfont_size=18\n[zoom]\nfactor=10.0\n";
        std::fs::write(&path, source).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(save(&dir.0, 0x75).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        assert_eq!(
            std::fs::read_dir(&dir.0).unwrap().count(),
            1,
            "clean up only the owned temporary file"
        );
        drop(lock);
        save(&dir.0, 0x75).unwrap();
        let loaded = super::super::load(&dir.0);
        assert_eq!(loaded.gui.menu_key, "F6");
        assert_eq!(loaded.gui.font_size, 18.);
        assert_eq!(loaded.zoom.factor, 10.);
        save(&dir.0, 0x76).unwrap();
        assert_eq!(super::super::load(&dir.0).gui.menu_key, "F7");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("# keep comments"));
        std::fs::remove_file(path).unwrap();
        save(&dir.0, 0x75).unwrap();
        assert_eq!(super::super::load(&dir.0).gui.menu_key, "F6");
    }
    #[test]
    fn worker_retries_without_clobbering_bad_toml_and_new_requests_supersede_old_ones() {
        let dir = Directory::new();
        let path = dir.0.join(FILE_NAME);
        std::fs::write(&path, "[gui").unwrap();
        request(0x75);
        poll(&dir.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[gui");
        assert_eq!(*PENDING.lock().unwrap(), Some(0x75));
        std::fs::write(&path, "[gui]\nmenu_key='L'\n").unwrap();
        request(0x76);
        poll(&dir.0);
        assert!(PENDING.lock().unwrap().is_none());
        assert_eq!(super::super::load(&dir.0).gui.menu_key, "F7");
    }
}
