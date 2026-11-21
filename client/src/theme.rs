//! Shared visual language for controls and passive overlays.
use crate::brand;
use hudhook::imgui::{ConfigFlags, Context, DrawListMut, StyleColor, Ui};

pub(crate) const TEXT: [f32; 4] = brand::color(brand::TEXT, 1.);
pub(crate) const MUTED: [f32; 4] = brand::color(brand::MUTED, 1.);
pub(crate) const ACCENT: [f32; 4] = brand::color(brand::ACCENT, 1.);
pub(crate) const SUCCESS: [f32; 4] = brand::color(brand::SECONDARY, 1.);
pub(crate) const WARNING: [f32; 4] = [0.98, 0.75, 0.41, 1.];
pub(crate) const DANGER: [f32; 4] = [0.97, 0.45, 0.49, 1.];
pub(crate) const SURFACE: [f32; 4] = brand::color(brand::SURFACE, 0.96);
pub(crate) const BORDER: [f32; 4] = brand::color(brand::BORDER, 0.85);
pub(crate) const SLOT: [f32; 4] = [0.105, 0.12, 0.19, 0.96];
pub(crate) const TRACK: [f32; 4] = [0.16, 0.18, 0.28, 1.];

pub(crate) fn logo(draw: &DrawListMut<'_>, min: [f32; 2], size: f32) {
    draw.add_rect(
        min,
        [min[0] + size, min[1] + size],
        brand::color(brand::LOGO_BG, 1.),
    )
    .filled(true)
    .rounding(size * 0.24)
    .build();
    let at = |p: [f32; 2]| [min[0] + p[0] * size, min[1] + p[1] * size];
    for segment in brand::MARK.windows(2) {
        draw.add_line(at(segment[0]), at(segment[1]), TEXT)
            .thickness(size * brand::MARK_WIDTH)
            .build();
    }
    draw.add_circle(at(brand::DOT), size * brand::DOT_RADIUS, SUCCESS)
        .filled(true)
        .build();
}

pub(crate) fn client_header(ui: &Ui) {
    let size = ui.text_line_height() * 3.1;
    let min = ui.cursor_screen_pos();
    logo(&ui.get_window_draw_list(), min, size);
    ui.dummy([size, size]);
    ui.same_line();
    ui.group(|| {
        let font = crate::gui::debug_font(ui).map(|font| ui.push_font(font));
        ui.text_colored(TEXT, brand::WORDMARK);
        drop(font);
        ui.text_colored(MUTED, format!("CLIENT  /  {}", env!("CARGO_PKG_VERSION")));
    });
    ui.text_wrapped(brand::TAGLINE);
    ui.separator();
}

pub(crate) fn apply(ctx: &mut Context) {
    ctx.io_mut()
        .config_flags
        .insert(ConfigFlags::NAV_ENABLE_KEYBOARD);
    let s = ctx.style_mut();
    s.window_padding = [16., 14.];
    s.frame_padding = [10., 7.];
    s.item_spacing = [10., 10.];
    s.item_inner_spacing = [8., 6.];
    s.indent_spacing = 14.;
    s.window_rounding = 12.;
    s.child_rounding = 9.;
    s.frame_rounding = 7.;
    s.popup_rounding = 9.;
    s.scrollbar_rounding = 8.;
    s.grab_rounding = 6.;
    s.tab_rounding = 7.;
    s.window_border_size = 1.;
    s.child_border_size = 1.;
    s.frame_border_size = 0.;
    s.popup_border_size = 1.;
    s.scrollbar_size = 10.;
    s.grab_min_size = 12.;
    s.window_title_align = [0.04, 0.5];
    for (key, color) in [
        (StyleColor::Text, TEXT),
        (StyleColor::TextDisabled, MUTED),
        (StyleColor::WindowBg, brand::color(brand::BG, 0.99)),
        (StyleColor::ChildBg, [0., 0., 0., 0.]),
        (StyleColor::PopupBg, brand::color(brand::SURFACE, 0.99)),
        (StyleColor::Border, BORDER),
        (StyleColor::BorderShadow, [0., 0., 0., 0.]),
        (StyleColor::FrameBg, [0.10, 0.115, 0.18, 1.]),
        (StyleColor::FrameBgHovered, [0.16, 0.18, 0.28, 1.]),
        (StyleColor::FrameBgActive, [0.21, 0.235, 0.36, 1.]),
        (StyleColor::TitleBg, brand::color(brand::BG, 1.)),
        (StyleColor::TitleBgActive, brand::color(brand::SURFACE, 1.)),
        (StyleColor::TitleBgCollapsed, brand::color(brand::BG, 0.97)),
        (StyleColor::MenuBarBg, [0.06, 0.08, 0.12, 1.]),
        (StyleColor::ScrollbarBg, [0., 0., 0., 0.]),
        (StyleColor::ScrollbarGrab, [0.23, 0.30, 0.39, 0.70]),
        (StyleColor::ScrollbarGrabHovered, [0.32, 0.41, 0.52, 0.90]),
        (StyleColor::ScrollbarGrabActive, [0.39, 0.52, 0.65, 1.]),
        (StyleColor::CheckMark, ACCENT),
        (StyleColor::SliderGrab, [0.49, 0.55, 0.88, 1.]),
        (StyleColor::SliderGrabActive, ACCENT),
        (StyleColor::Button, [0.13, 0.15, 0.23, 1.]),
        (StyleColor::ButtonHovered, [0.22, 0.25, 0.38, 1.]),
        (StyleColor::ButtonActive, [0.29, 0.33, 0.49, 1.]),
        (StyleColor::Header, [0.10, 0.12, 0.19, 1.]),
        (StyleColor::HeaderHovered, [0.18, 0.20, 0.31, 1.]),
        (StyleColor::HeaderActive, [0.24, 0.28, 0.42, 1.]),
        (StyleColor::Separator, [0.55, 0.68, 0.85, 0.10]),
        (
            StyleColor::SeparatorHovered,
            brand::color(brand::ACCENT, 0.5),
        ),
        (StyleColor::SeparatorActive, ACCENT),
        (StyleColor::ResizeGrip, brand::color(brand::ACCENT, 0.12)),
        (
            StyleColor::ResizeGripHovered,
            brand::color(brand::ACCENT, 0.40),
        ),
        (
            StyleColor::ResizeGripActive,
            brand::color(brand::ACCENT, 0.65),
        ),
        (StyleColor::Tab, [0.07, 0.10, 0.15, 1.]),
        (StyleColor::TabHovered, [0.24, 0.28, 0.42, 1.]),
        (StyleColor::TabActive, [0.20, 0.23, 0.35, 1.]),
        (
            StyleColor::TextSelectedBg,
            brand::color(brand::ACCENT, 0.35),
        ),
        (StyleColor::NavHighlight, brand::color(brand::ACCENT, 0.65)),
        (StyleColor::ModalWindowDimBg, [0., 0., 0., 0.50]),
    ] {
        s.colors[key as usize] = color;
    }
}

pub(crate) fn panel(draw: &DrawListMut<'_>, min: [f32; 2], max: [f32; 2]) {
    panel_alpha(draw, min, max, 1.);
}
pub(crate) fn panel_alpha(draw: &DrawListMut<'_>, min: [f32; 2], max: [f32; 2], alpha: f32) {
    let alpha = alpha.clamp(0., 1.);
    draw.add_rect(
        [min[0] + 1., min[1] + 4.],
        [max[0] + 1., max[1] + 4.],
        [0., 0., 0., 0.16 * alpha],
    )
    .filled(true)
    .rounding(12.)
    .build();
    let mut fill = SURFACE;
    fill[3] *= alpha;
    let mut border = BORDER;
    border[3] *= alpha;
    draw.add_rect(min, max, fill)
        .filled(true)
        .rounding(12.)
        .build();
    draw.add_rect(min, max, border).rounding(12.).build();
}
pub(crate) fn progress(
    draw: &DrawListMut<'_>,
    min: [f32; 2],
    size: [f32; 2],
    value: f32,
    color: [f32; 4],
) {
    let value = if value.is_finite() {
        value.clamp(0., 1.)
    } else {
        0.
    };
    draw.add_rect(min, [min[0] + size[0], min[1] + size[1]], TRACK)
        .filled(true)
        .rounding(size[1] * 0.5)
        .build();
    if value > 0. {
        draw.add_rect(min, [min[0] + size[0] * value, min[1] + size[1]], color)
            .filled(true)
            .rounding(size[1] * 0.5)
            .build();
    }
}
pub(crate) fn switch(ui: &Ui, id: &str, on: &mut bool) -> bool {
    let size = [42., 24.];
    let transparent = [0., 0., 0., 0.];
    let _colors = [
        ui.push_style_color(StyleColor::Button, transparent),
        ui.push_style_color(StyleColor::ButtonHovered, transparent),
        ui.push_style_color(StyleColor::ButtonActive, transparent),
    ];
    let changed = ui.button_with_size(id, size);
    if changed {
        *on = !*on;
    }
    let min = ui.item_rect_min();
    let max = ui.item_rect_max();
    let fill = if *on {
        [0.47, 0.53, 0.85, 1.]
    } else if ui.is_item_hovered() {
        [0.23, 0.30, 0.39, 1.]
    } else {
        [0.15, 0.21, 0.29, 1.]
    };
    let d = ui.get_window_draw_list();
    d.add_rect(min, max, fill)
        .filled(true)
        .rounding(12.)
        .build();
    let center = [if *on { max[0] - 12. } else { min[0] + 12. }, min[1] + 12.];
    d.add_circle(center, 8., if *on { TEXT } else { MUTED })
        .filled(true)
        .build();
    if ui.is_item_focused() {
        d.add_rect(
            [min[0] - 2., min[1] - 2.],
            [max[0] + 2., max[1] + 2.],
            ACCENT,
        )
        .rounding(14.)
        .build();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switch_toggles_once_on_release_and_holding_does_not_repeat() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut ctx = Context::create();
        ctx.set_ini_filename(None);
        ctx.io_mut().display_size = [400., 300.];
        ctx.io_mut().delta_time = 1. / 60.;
        ctx.fonts().build_rgba32_texture();
        apply(&mut ctx);
        let mut on = false;
        let mut frame = |down| {
            ctx.io_mut().add_mouse_pos_event([42., 42.]);
            ctx.io_mut()
                .add_mouse_button_event(hudhook::imgui::MouseButton::Left, down);
            let ui = ctx.frame();
            ui.window("test")
                .position([0., 0.], hudhook::imgui::Condition::Always)
                .size([200., 150.], hudhook::imgui::Condition::Always)
                .title_bar(false)
                .build(|| {
                    ui.set_cursor_pos([20., 30.]);
                    switch(ui, "##switch", &mut on);
                });
            ctx.render();
            on
        };
        assert!(!frame(false));
        assert!(!frame(false));
        assert!(!frame(true));
        assert!(!frame(true));
        assert!(frame(false));
        assert!(frame(false));
        assert!(frame(true));
        assert!(!frame(false));
    }
}
