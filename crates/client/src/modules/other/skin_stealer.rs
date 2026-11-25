//! Explicit local PNG export of already-loaded player textures.
mod compatible;
mod storage;

use crate::modules::{
    game::{actors, skins},
    key_or_warn, Module,
};
use crate::{config, log};
use hudhook::imgui::Ui;
use std::path::{Path, PathBuf};

pub struct SkinStealer {
    key: u16,
    enabled: bool,
    directory: PathBuf,
    error: String,
}
impl SkinStealer {
    pub fn new(cfg: &config::Toggle, root: &Path) -> Self {
        Self {
            key: key_or_warn("skinstealer", &cfg.key),
            enabled: false,
            directory: root.join("skins"),
            error: String::new(),
        }
    }
}
impl Module for SkinStealer {
    fn name(&self) -> &str {
        "skinstealer"
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
        let result = if on {
            skins::available().and_then(|_| actors::use_actors(true))
        } else {
            actors::use_actors(false)
        };
        match result {
            Ok(()) => {
                skins::enable(on);
                self.enabled = on;
                self.error.clear();
            }
            Err(e) => {
                log!("SkinStealer: {e}");
                self.error = e;
            }
        }
    }
    fn status(&self) -> String {
        format!("{}人 / {}", skins::snapshot().len(), skins::status())
    }
    fn draw_settings(&mut self, ui: &Ui) {
        ui.text_wrapped("読み込み済みプレイヤーのスキン画像をPNGに保存します。自分も対象です。ONにしただけでは保存しません。");
        ui.text_wrapped("通常のスキンは元の画像を保存し、モデルを判定できる場合は腕の太さを表示します。対応するPersonaは顔・服・手足を合成し、元画像の細かさに応じて64×64または128×128の幅広の腕（Steve型）へ変換します。顔のアニメは最初の画像を使います。立体形状には制限があります。");
        ui.text_wrapped("元の画像だけ欲しい場合は「元テクスチャを保存」を選んでください。これはスキンとして読み込める形式とは限りません。");
        ui.text_wrapped(format!("保存先: {}", self.directory.display()));
        if !self.enabled {
            ui.text_wrapped("SkinStealerをONにしてワールドへ戻ってください。");
        }
        if ui.button("一覧を更新") {
            skins::refresh();
        }
        ui.text_wrapped(skins::status());
        let players = skins::snapshot();
        if self.enabled && players.is_empty() {
            ui.text_wrapped(
                "取得できるプレイヤーがいません。メニューを閉じてから再度開いてください。",
            );
        }
        let busy = skins::busy();
        let compatible = skins::compatible_available();
        if let Err(e) = &compatible {
            ui.text_wrapped(format!("スキン用PNG: {e}"));
        }
        for p in players.iter() {
            let _id = ui.push_id_usize(p.entity as usize);
            ui.separator();
            ui.text_wrapped(format!(
                "{}{} — {} × {}",
                p.name,
                if p.local { "（自分）" } else { "" },
                p.width,
                p.height
            ));
            ui.text_wrapped(format!("ID: {}", p.skin_id));
            let _disabled = ui.begin_disabled(busy || !self.enabled);
            let compatible_button = ui.begin_disabled(compatible.is_err());
            if ui.button("スキン用PNGに保存") {
                self.error = skins::request(p, self.directory.clone(), skins::ExportMode::Skin)
                    .err()
                    .unwrap_or_default();
            }
            drop(compatible_button);
            if ui.button("元テクスチャを保存") {
                self.error = skins::request(p, self.directory.clone(), skins::ExportMode::Raw)
                    .err()
                    .unwrap_or_default();
            }
        }
        if !self.error.is_empty() {
            ui.text_wrapped(&self.error);
        }
    }
}

/// Disk I/O and compression run on the existing worker, never in Present or
/// under the Modules mutex. Only one bounded, owned image can be queued.
pub(crate) fn poll_exports() {
    let Some(mut job) = skins::take_export() else {
        return;
    };
    let result = (|| {
        let raw = job.mode == skins::ExportMode::Raw;
        let (width, height, rgba, arms) = if raw {
            (
                job.player.width,
                job.player.height,
                std::mem::take(&mut job.rgba),
                "",
            )
        } else {
            let model = job.model.as_ref().map_err(Clone::clone)?;
            let convert = model
                .as_ref()
                .map(compatible::needs_conversion)
                .transpose()?
                .unwrap_or(false);
            if let Some(model) = model.as_ref().filter(|_| convert) {
                let body = skins::geometry::Texture {
                    width: job.player.width,
                    height: job.player.height,
                    rgba: std::mem::take(&mut job.rgba),
                };
                let (side, rgba) = compatible::convert(model, &body)?;
                (side, side, rgba, "・幅広の腕（Steve型）")
            } else if matches!(
                (job.player.width, job.player.height),
                (64, 32) | (64, 64) | (128, 128)
            ) {
                (
                    job.player.width,
                    job.player.height,
                    std::mem::take(&mut job.rgba),
                    compatible::arm_label(job.geometry_id.as_deref()),
                )
            } else {
                return Err("この画像サイズは通常スキンに対応しません。元テクスチャは別ボタンで保存できます".into());
            }
        };
        let suffix = if raw { "raw" } else { "skin" };
        storage::save(
            &job.directory,
            &format!("{}_{suffix}", job.player.name),
            job.player.entity,
            width,
            height,
            &rgba,
        )
        .map(|path| {
            format!(
                "{}（{width}×{height}{arms}）を保存しました: {}",
                if raw {
                    "元テクスチャ"
                } else {
                    "スキン用PNG"
                },
                path.display()
            )
        })
    })();
    match &result {
        Ok(text) => log!("SkinStealer: {text}"),
        Err(e) => log!("SkinStealer: 保存失敗: {e}"),
    }
    skins::finish(&job, result);
}

pub(crate) fn command(args: &[&str], directory: &Path) {
    let players = skins::snapshot();
    match args {
        [] | ["list"] => {
            log!("SkinStealer: {}人 / {}", players.len(), skins::status());
            for p in players.iter() {
                log!(
                    "  {:08X} {} {}x{}{}",
                    p.entity,
                    p.name,
                    p.width,
                    p.height,
                    if p.local { " (self)" } else { "" }
                );
            }
        }
        [action @ ("save" | "raw"), target] => {
            let selected = if *target == "self" {
                players.iter().find(|p| p.local)
            } else {
                u32::from_str_radix(target.trim_start_matches("0x"), 16)
                    .ok()
                    .and_then(|id| players.iter().find(|p| p.entity == id))
            };
            match selected {
                Some(p) => match skins::request(p, directory.join("skins"),if *action=="raw" {skins::ExportMode::Raw}else{skins::ExportMode::Skin}) {
                    Ok(()) => log!("SkinStealer: {}の画像取得を要求しました", p.name),
                    Err(e) => log!("SkinStealer: {e}"),
                },
                None => log!("SkinStealer: 対象がありません。ONにしてワールドへ戻り、skins listで確認してください"),
            }
        }
        _ => log!("skins list | skins save self | skins save <一覧の16進ID> | skins raw self | skins raw <ID>"),
    }
}
