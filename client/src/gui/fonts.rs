//! One Japanese font for both Latin and Japanese text; never merge mismatched baselines.
use hudhook::imgui::{Context, FontConfig, FontGlyphRanges, FontId, FontSource};
use std::path::PathBuf;

const RANGES: &[u32] = &[
    0x20, 0xff, 0x2000, 0x23ff, 0x25a0, 0x25ff, 0x3000, 0x30ff, 0x31f0, 0x31ff, 0x4e00, 0x9fff,
    0xff00, 0xffef, 0,
];

pub(super) fn apply_style(ctx: &mut Context) {
    crate::theme::apply(ctx);
}

pub(super) fn initialize(ctx: &mut Context, pixels: f32) -> Option<FontId> {
    let pixels = if pixels.is_finite() {
        pixels.clamp(12., 28.)
    } else {
        16.
    };
    let fonts = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
        .join("Fonts");
    // Meiryo is designed for small on-screen Japanese text. The complete face
    // supplies ASCII too, so Latin letters retain matching weight and height.
    for (file, family) in [
        ("meiryo.ttc", "Meiryo"),
        ("YuGothM.ttc", "Yu Gothic Medium"),
        ("msgothic.ttc", "MS Gothic"),
    ] {
        let Ok(data) = std::fs::read(fonts.join(file)) else {
            continue;
        };
        if !matches!(data.get(..4), Some(b"ttcf" | b"\0\x01\0\0" | b"OTTO")) {
            continue;
        }
        ctx.fonts().clear();
        ctx.fonts()
            .flags
            .insert(hudhook::imgui::FontAtlasFlags::NO_POWER_OF_TWO_HEIGHT);
        let id = ctx.fonts().add_font(&[FontSource::TtfData {
            data: &data,
            size_pixels: pixels,
            config: Some(FontConfig {
                name: Some(format!("{family} {pixels:.0}px")),
                glyph_ranges: FontGlyphRanges::from_slice(RANGES),
                oversample_h: 2,
                oversample_v: 1,
                pixel_snap_h: false,
                ..Default::default()
            }),
        }]);
        // Rasterize the F3 text separately instead of magnifying the 16px UI.
        // Keep the same face and complete Japanese coverage for world names.
        ctx.fonts().add_font(&[FontSource::TtfData {
            data: &data,
            size_pixels: 32.,
            config: Some(FontConfig {
                name: Some(format!("DebugHUD {family} 32px")),
                glyph_ranges: FontGlyphRanges::from_slice(RANGES),
                oversample_h: 1,
                oversample_v: 1,
                pixel_snap_h: false,
                ..Default::default()
            }),
        }]);
        crate::log!("日本語フォント: {family}（{pixels:.0}px）");
        return Some(id);
    }
    crate::log!("日本語フォントを読み込めません。Windowsにメイリオまたは游ゴシックをインストールしてください。");
    None
}

/// A headless ImGui context for tests: no ini file, the client's font at
/// `pixels` and its style, with the font atlas built.
#[cfg(test)]
pub(super) fn test_context(pixels: f32) -> Context {
    let mut ctx = Context::create();
    ctx.set_ini_filename(None);
    initialize(&mut ctx, pixels).expect("Japanese Windows font");
    apply_style(&mut ctx);
    ctx.fonts().build_alpha8_texture();
    ctx
}

pub(crate) fn debug_font(ui: &hudhook::imgui::Ui) -> Option<FontId> {
    // initialize owns this atlas: index 0 is the ordinary UI, index 1 is F3.
    ui.fonts().fonts().get(1).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hudhook::imgui::internal::RawCast;

    #[test]
    fn japanese_font_rasterizes_translations_with_one_consistent_face() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ctx = Context::create();
        let id = initialize(&mut ctx, 16.).expect("Windows Japanese font is required for the UI");
        let started = std::time::Instant::now();
        let texture = ctx.fonts().build_alpha8_texture();
        assert!(texture.width > 0 && texture.height > 0);
        eprintln!(
            "Japanese atlas {}x{}, {} bytes, {:?}",
            texture.width,
            texture.height,
            texture.data.len(),
            started.elapsed()
        );
        let atlas = ctx.fonts();
        let font = atlas.get_font(id).unwrap();
        let large_id = atlas.fonts()[1];
        let large = atlas.get_font(large_id).unwrap();
        assert_eq!(font.font_size, 16.);
        assert_eq!(large.font_size, 32.);
        let raw = unsafe { font.raw() };
        assert_eq!(
            raw.ConfigDataCount, 1,
            "Latin and Japanese must use one face"
        );
        let source = concat!(
            include_str!("../../../shared/branding.rs"),
            include_str!("../localization.rs"),
            include_str!("../modules/freecam.rs"),
            include_str!("../modules/game/freecam.rs"),
            include_str!("menu.rs"),
            include_str!("../hud_editor.rs"),
            include_str!("../modules/waila.rs"),
            include_str!("../modules/block_outline.rs"),
            include_str!("../modules/player_list.rs"),
            include_str!("../modules/mining_hud.rs"),
            include_str!("../modules/item_counter.rs"),
            include_str!("../modules/kill_aura.rs"),
            include_str!("../modules/trigger_bot.rs"),
            include_str!("../modules/auto_weapon.rs"),
            include_str!("../modules/zoom.rs"),
            include_str!("../modules/friends.rs"),
            include_str!("../modules/minimap.rs"),
            include_str!("../modules/trajectories.rs"),
            include_str!("../modules/frame_time.rs"),
            include_str!("../modules/performance_profiler/mod.rs"),
            include_str!("../modules/performance_profiler/metrics.rs"),
            include_str!("../modules/debug_hud.rs"),
            include_str!("../modules/skin_stealer.rs"),
            include_str!("../modules/skin_stealer/compatible.rs"),
            include_str!("../modules/game/skins/geometry.rs"),
            include_str!("../modules/game/skins/layout.rs"),
            include_str!("../modules/world_info.rs"),
            include_str!("../modules/light_overlay.rs"),
            include_str!("../modules/light_overlay/render.rs"),
            include_str!("../modules/freelook.rs"),
            include_str!("../modules/mob_count.rs"),
            include_str!("../modules/chat_log.rs"),
            include_str!("../modules/chat_repeat.rs"),
            include_str!("../modules/settings.rs"),
            include_str!("../modules/settings/key_capture.rs"),
            include_str!("../config/menu_key.rs")
        );
        let mut characters = std::collections::BTreeSet::new();
        characters.extend(
            source
                .chars()
                .filter(|&c| c >= '\u{3000}' && !c.is_control()),
        );
        characters.extend("ABCxyz0123 → ← × ％ ▸ ▾ ·".chars());
        for c in characters {
            for target in [font, large] {
                let glyph = unsafe {
                    hudhook::imgui::sys::ImFont_FindGlyphNoFallback(
                        target.raw() as *const _ as *mut _,
                        c as u32,
                    )
                };
                assert!(!glyph.is_null(), "Missing Japanese UI glyph {c}");
            }
        }
    }
}
