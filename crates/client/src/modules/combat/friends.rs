use crate::modules::{
    game::friends as native,
    key_or_warn,
    settings::{Detail, Value},
    Module, Modules,
};
use crate::{config, log};
use hudhook::imgui::Ui;

fn clean(name: &str) -> Result<String, String> {
    let mut chars = name.chars();
    let mut result = String::new();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
            continue;
        }
        if c.is_control() {
            return Err("名前に制御文字は使えません".into());
        }
        result.push(c);
    }
    let result = result.trim();
    if result.is_empty() || result.chars().count() > 64 {
        return Err("名前は1〜64文字で入力してください".into());
    }
    Ok(result.into())
}
fn parse(text: &str) -> Result<Vec<String>, String> {
    if text.len() > 4096 {
        return Err("名前の合計が長すぎます".into());
    }
    let mut names: Vec<String> = Vec::new();
    for name in text.lines().filter(|s| !s.trim().is_empty()) {
        let name = clean(name)?;
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
        if names.len() > 32 {
            return Err("登録できる名前は32件までです".into());
        }
    }
    Ok(names)
}
pub struct Friends {
    key: u16,
    enabled: bool,
    input: String,
    error: String,
}
impl Friends {
    pub fn new(c: &config::Friends) -> Self {
        let names = match parse(&c.names.join("\n")) {
            Ok(names) => names,
            Err(e) => {
                log!("Friends: {e}");
                Vec::new()
            }
        };
        native::enable(false);
        native::set(names);
        Self {
            key: key_or_warn("friends", &c.key),
            enabled: false,
            input: String::new(),
            error: String::new(),
        }
    }
}
impl Module for Friends {
    fn name(&self) -> &str {
        "friends"
    }
    fn key(&self) -> u16 {
        self.key
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        if on && !self.enabled {
            if let Err(e) = native::available() {
                log!("Friends: {e}");
                return;
            }
        }
        self.enabled = on;
        native::enable(on);
    }
    fn status(&self) -> String {
        format!("{}人を登録 / 自動攻撃から除外", native::list().len())
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::text("names", native::list().join("\n"))]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        if id != "names" {
            return Err(format!("不明な設定項目: {id}"));
        }
        native::set(parse(v.text()?)?);
        Ok(())
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.text_wrapped("表示名を完全一致で登録します。英字の大小は区別しません。KillAura・TriggerBot・AutoClickerの連打から除外します。手動の単発攻撃は維持します。");
        ui.text_wrapped("名前が取得できないプレイヤーも保護します。同じ表示名のプレイヤーはすべて対象になります。");
        ui.input_text("表示名", &mut self.input)
            .hint("Steve / 日本 太郎")
            .build();
        ui.same_line();
        if ui.button("追加") {
            let mut names = (*native::list()).clone();
            match clean(&self.input).and_then(|name| {
                names.push(name);
                parse(&names.join("\n"))
            }) {
                Ok(names) => {
                    native::set(names);
                    self.input.clear();
                    self.error.clear();
                }
                Err(e) => self.error = e,
            }
        }
        let names = native::list();
        let mut remove = None;
        for (i, name) in names.iter().enumerate() {
            let _id = ui.push_id_usize(i);
            if ui.small_button("削除") {
                remove = Some(i);
            }
            ui.same_line();
            ui.text(name);
        }
        if let Some(i) = remove {
            let mut names = (*names).clone();
            names.remove(i);
            native::set(names);
        }
        if !names.is_empty() && ui.button("すべて削除") {
            native::set(Vec::new());
        }
        if !self.error.is_empty() {
            ui.text_wrapped(&self.error);
        }
    }
}
pub fn command(args: &[&str], modules: &mut Modules) {
    let Some(index) = modules.find("friends") else {
        return;
    };
    let mut names = (*native::list()).clone();
    let result = match args {
        [] | ["list"] => {
            log!(
                "Friends: {}人（機能{}）",
                names.len(),
                if native::active() {
                    "有効"
                } else {
                    "無効または未登録"
                }
            );
            for name in names {
                log!("  {name}");
            }
            return;
        }
        ["clear"] => {
            names.clear();
            Ok(())
        }
        [action, rest @ ..] if !rest.is_empty() => {
            clean(&rest.join(" ")).and_then(|name| match *action {
                "add" => {
                    names.push(name);
                    Ok(())
                }
                "del" => {
                    let before = names.len();
                    names.retain(|n| !n.eq_ignore_ascii_case(&name));
                    if names.len() == before {
                        Err("その名前は登録されていません".into())
                    } else {
                        Ok(())
                    }
                }
                _ => Err("friends list|add <表示名>|del <表示名>|clear".into()),
            })
        }
        _ => Err("friends list|add <表示名>|del <表示名>|clear".into()),
    }
    .and_then(|_| {
        modules.set_option_at(
            index,
            "names",
            &Value::Text {
                text: names.join("\n"),
            },
        )
    });
    match result {
        Ok(()) => log!("Friends: {}人を登録", native::list().len()),
        Err(e) => log!("Friends: {e}"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_normalized_without_losing_spaces_or_japanese() {
        assert_eq!(
            parse(" §aSteve\nsteve\n日本 太郎\n").unwrap(),
            ["Steve", "日本 太郎"]
        );
        assert!(clean("bad\tname").is_err());
        assert!(clean("§a").is_err());
        assert!(parse(&"x".repeat(65)).is_err());
        let names = (0..33)
            .map(|i| format!("player{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse(&names).is_err());
    }
}
