use super::brand;
use eframe::egui::{self, Color32, FontFamily, FontId, Stroke};

pub(super) const BG: Color32 = Color32::from_rgb(brand::BG[0], brand::BG[1], brand::BG[2]);
pub(super) const SURFACE: Color32 =
    Color32::from_rgb(brand::SURFACE[0], brand::SURFACE[1], brand::SURFACE[2]);
pub(super) const FIELD: Color32 =
    Color32::from_rgb(brand::FIELD[0], brand::FIELD[1], brand::FIELD[2]);
pub(super) const BORDER: Color32 =
    Color32::from_rgb(brand::BORDER[0], brand::BORDER[1], brand::BORDER[2]);
pub(super) const TEXT: Color32 = Color32::from_rgb(brand::TEXT[0], brand::TEXT[1], brand::TEXT[2]);
pub(super) const MUTED: Color32 =
    Color32::from_rgb(brand::MUTED[0], brand::MUTED[1], brand::MUTED[2]);
pub(super) const ACCENT: Color32 =
    Color32::from_rgb(brand::ACCENT[0], brand::ACCENT[1], brand::ACCENT[2]);
pub(super) const GOOD: Color32 = Color32::from_rgb(
    brand::SECONDARY[0],
    brand::SECONDARY[1],
    brand::SECONDARY[2],
);
pub(super) const WARN: Color32 = Color32::from_rgb(241, 193, 114);
pub(super) const ERROR: Color32 = Color32::from_rgb(247, 137, 151);

pub(super) fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut fonts = egui::FontDefinitions::default();
    let font_dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:/Windows".into())
        .join("Fonts");
    for name in ["meiryo.ttc", "YuGothM.ttc"] {
        if let Ok(data) = std::fs::read(font_dir.join(name)) {
            fonts.font_data.insert(
                "japanese-ui".into(),
                egui::FontData::from_owned(data).into(),
            );
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "japanese-ui".into());
            break;
        }
    }
    // Use Windows' Latin UI face first, retaining Japanese fallback for paths.
    if let Ok(data) = std::fs::read(font_dir.join("segoeui.ttf")) {
        fonts
            .font_data
            .insert("english-ui".into(), egui::FontData::from_owned(data).into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "english-ui".into());
    }
    ctx.set_fonts(fonts);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.text_styles = [
        (egui::TextStyle::Heading, FontId::proportional(24.)),
        (egui::TextStyle::Body, FontId::proportional(14.)),
        (egui::TextStyle::Button, FontId::proportional(14.)),
        (egui::TextStyle::Small, FontId::proportional(12.)),
        (egui::TextStyle::Monospace, FontId::monospace(13.)),
    ]
    .into();
    style.animation_time = 0.15;
    style.spacing.item_spacing = egui::vec2(10., 10.);
    style.spacing.button_padding = egui::vec2(14., 8.);
    style.spacing.interact_size.y = 36.;
    let v = &mut style.visuals;
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = FIELD;
    v.text_edit_bg_color = Some(FIELD);
    v.faint_bg_color = SURFACE;
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(MUTED);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgb(72, 79, 128);
    v.selection.stroke = Stroke::new(1., TEXT);
    v.warn_fg_color = WARN;
    v.error_fg_color = ERROR;
    for widget in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.corner_radius = 9.into();
        widget.bg_fill = Color32::from_rgb(32, 37, 57);
        widget.weak_bg_fill = Color32::from_rgb(32, 37, 57);
        widget.bg_stroke = Stroke::new(1., BORDER);
        widget.fg_stroke = Stroke::new(1., TEXT);
        widget.expansion = 0.;
    }
    v.widgets.hovered.bg_fill = Color32::from_rgb(48, 55, 85);
    v.widgets.hovered.weak_bg_fill = v.widgets.hovered.bg_fill;
    v.widgets.hovered.bg_stroke = Stroke::new(1., ACCENT.gamma_multiply(0.7));
    v.widgets.active.bg_stroke = Stroke::new(1., ACCENT);
    ctx.set_style_of(egui::Theme::Dark, style);
}
pub(super) fn card() -> egui::Frame {
    egui::Frame::NONE
        .fill(SURFACE)
        .stroke(Stroke::new(1., BORDER))
        .corner_radius(14)
        .inner_margin(20)
}

pub(super) fn icon() -> egui::IconData {
    egui::IconData {
        rgba: brand::icon_rgba(64),
        width: 64,
        height: 64,
    }
}
