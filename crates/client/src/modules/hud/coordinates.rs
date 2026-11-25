//! XYZ display for the executable analyzed in analysis/notes/coordinates.md.
//!
//! The overlay line shows the position sampled by the shared game hook
//! (`game.rs`). Optionally (`native_hud`) three HUD checks are patched as well
//! so Minecraft draws its own display; the world's gamerule is never written.

use crate::signatures::{TEXT_SIGNATURE, VISIBILITY_SIGNATURE};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::game::world::{self, Realm};
use crate::modules::settings::{Detail, Value};
use crate::modules::{game, key_or_warn, patch, Module};
use crate::{config, log};

/// (name, signature, offset of the byte to change, replacement byte)
///
/// The accessor is the one the HUD screen actually asks (for both visibility
/// and text). Its JNE becomes JMP (75 -> EB) so it keeps the preset `true`;
/// the level and rule-table checks before it still return false when there
/// is no world.
const PATCHES: [(&str, &str, isize, &str); 3] = [
    (
        "coordinates accessor",
        crate::signatures::ACCESSOR_SIGNATURE,
        game::ACCESSOR_PATCH_OFFSET,
        "EB",
    ),
    ("coordinates visibility", VISIBILITY_SIGNATURE, 8, "87"),
    ("coordinates text", TEXT_SIGNATURE, 8, "87"),
];
static FIRST_SAMPLE_LOGGED: AtomicBool = AtomicBool::new(false);

/// Where a portal at (x, z) comes out in the linked dimension: the Nether is
/// 8 times smaller than the Overworld. None in dimensions without that link.
fn portal_equivalent(x: i32, z: i32, realm: Realm) -> Option<(&'static str, i32, i32)> {
    match realm {
        // Floor division, so negative coordinates land in the right block.
        Realm::Overworld => Some(("nether", x.div_euclid(8), z.div_euclid(8))),
        Realm::Nether => Some(("overworld", x.saturating_mul(8), z.saturating_mul(8))),
        Realm::End | Realm::Other => None,
    }
}

pub struct Coordinates {
    key: u16,
    /// Why the position source is unavailable, if it is.
    unavailable: Option<String>,
    hooked: bool,
    native_hud: bool,
    /// Also show where a portal here would lead (Overworld <-> Nether).
    nether: bool,
    /// Native HUD patches; empty unless `native_hud` is set.
    patches: Vec<patch::Patch>,
    error: Option<String>,
}

impl Coordinates {
    pub fn new(cfg: &config::Coordinates) -> Self {
        let unavailable = game::available().err();
        if let Some(e) = &unavailable {
            log!("coordinatesを利用できません: {e}");
        }
        let patches = if unavailable.is_none() && cfg.native_hud {
            Self::make_patches()
        } else {
            Vec::new()
        };
        Self {
            key: key_or_warn("coordinates", &cfg.key),
            unavailable,
            hooked: false,
            native_hud: cfg.native_hud,
            nether: cfg.nether,
            patches,
            error: None,
        }
    }

    fn make_patches() -> Vec<patch::Patch> {
        let configs: Vec<_> = PATCHES
            .into_iter()
            .map(|(name, signature, offset, bytes)| config::Patch {
                name: name.into(),
                key: "NONE".into(),
                enabled: false,
                signature: signature.into(),
                offset,
                bytes: bytes.into(),
            })
            .collect();
        patch::load(&configs)
    }

    fn set_native_hud(&mut self, on: bool) -> Result<(), String> {
        if self.native_hud == on {
            return Ok(());
        }
        if on && self.patches.is_empty() && self.unavailable.is_none() {
            self.patches = Self::make_patches();
        }
        if on && self.hooked {
            self.enable_patches()?;
        }
        if !on {
            self.disable_patches();
            if self.patches.iter().any(Module::enabled) {
                return Err("ゲーム標準HUDのパッチを復元できません".into());
            }
        }
        self.native_hud = on;
        self.error = None;
        Ok(())
    }

    /// Applies every native HUD patch, or none of them.
    fn enable_patches(&mut self) -> Result<(), String> {
        for i in 0..self.patches.len() {
            self.patches[i].set_enabled(true);
            if !self.patches[i].enabled() {
                let reason = self.patches[i].status();
                self.disable_patches();
                return Err(format!("ゲーム標準HUDのパッチに失敗: {reason}"));
            }
        }
        Ok(())
    }

    fn disable_patches(&mut self) {
        for patch in &mut self.patches {
            if patch.enabled() {
                patch.set_enabled(false);
            }
        }
    }
}

impl Module for Coordinates {
    fn draw_settings(&mut self, ui: &hudhook::imgui::Ui) {
        if ui.button(crate::localization::text("Copy XYZ")) {
            crate::console::submit("coords copy");
        }
        ui.same_line();
        ui.text_disabled(crate::localization::text("Current block coordinates"));
    }
    fn name(&self) -> &str {
        "coordinates"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "native_hud",
                "Native coordinate HUD",
                "Also show Minecraft's coordinate HUD without changing the gamerule.",
                self.native_hud,
            ),
            Detail::boolean(
                "nether",
                "Portal coordinates",
                "Also show the matching X Z in the Nether (or in the Overworld while in the Nether).",
                self.nether,
            ),
        ]
    }
    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "native_hud" => self.set_native_hud(value.boolean()?),
            "nether" => {
                self.nether = value.boolean()?;
                Ok(())
            }
            _ => Err(format!("不明な設定項目: {id}")),
        }
    }
    fn enabled(&self) -> bool {
        // Include a partial restore failure so disable_all() can retry it.
        self.hooked || self.patches.iter().any(Module::enabled)
    }

    fn set_enabled(&mut self, on: bool) {
        if let Some(e) = &self.unavailable {
            log!("coordinatesを利用できません: {e}");
            return;
        }
        if on {
            if !self.hooked {
                match game::acquire() {
                    Ok(()) => self.hooked = true,
                    Err(e) => {
                        log!("coordinates: {e}");
                        self.error = Some(e);
                        return;
                    }
                }
            }
            self.error = if self.native_hud {
                self.enable_patches().err()
            } else {
                None
            };
            if let Some(e) = &self.error {
                log!("coordinates: {e}");
            }
        } else {
            self.disable_patches();
            if self.hooked {
                match game::release() {
                    Ok(()) => self.hooked = false,
                    Err(e) => log!("coordinates: {e}"),
                }
            }
            self.error = self
                .enabled()
                .then(|| "完全に解除できませんでした。もう一度お試しください".to_string());
        }
    }

    fn status(&self) -> String {
        if let Some(e) = self.unavailable.as_ref().or(self.error.as_ref()) {
            return format!("({e})");
        }
        match game::position() {
            Some((x, y, z)) => {
                if !FIRST_SAMPLE_LOGGED.swap(true, Ordering::Relaxed) {
                    log!("座標の取得を開始: {x} {y} {z}");
                }
                let portal = self
                    .nether
                    .then(world::realm)
                    .flatten()
                    .and_then(|realm| portal_equivalent(x, z, realm));
                match portal {
                    Some((name, px, pz)) => format!("{x} {y} {z}  ({name} {px} {pz})"),
                    None => format!("{x} {y} {z}"),
                }
            }
            None => "（ワールド未接続）".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_coordinates_scale_by_eight_and_floor_negative_values() {
        assert_eq!(
            portal_equivalent(800, -160, Realm::Overworld),
            Some(("nether", 100, -20))
        );
        // -1 is in the Nether block -1, not 0.
        assert_eq!(
            portal_equivalent(-1, 7, Realm::Overworld),
            Some(("nether", -1, 0))
        );
        assert_eq!(
            portal_equivalent(-9, -8, Realm::Overworld),
            Some(("nether", -2, -1))
        );
        assert_eq!(
            portal_equivalent(100, -20, Realm::Nether),
            Some(("overworld", 800, -160))
        );
        assert_eq!(
            portal_equivalent(i32::MAX, i32::MIN, Realm::Nether),
            Some(("overworld", i32::MAX, i32::MIN))
        );
        assert_eq!(portal_equivalent(1, 2, Realm::End), None);
        assert_eq!(portal_equivalent(1, 2, Realm::Other), None);
    }
}
