//! Slot positions match the inventory: slots 9..35 above hotbar slots 0..8.
use super::{icons, items};
use hudhook::imgui::Ui;

const COLUMNS: usize = 9;
const PADDING: f32 = 10.;
const HOTBAR_GAP: f32 = 8.;
pub(super) fn slots(hotbar_only: bool) -> impl Iterator<Item = usize> {
    (if hotbar_only { 36 } else { 9 }..36).chain(0..9)
}
pub(super) fn size(cell: f32, hotbar_only: bool, title_height: f32) -> [f32; 2] {
    [
        PADDING * 2. + COLUMNS as f32 * cell,
        PADDING * 2.
            + title_height
            + cell * if hotbar_only { 1. } else { 4. }
            + if hotbar_only { 0. } else { HOTBAR_GAP },
    ]
}

pub(super) fn draw(ui: &Ui, inv: &items::Inventory, cfg: &crate::config::ItemHud) {
    let cell = cfg.slot_size.clamp(28, 64) as f32;
    let heading = ui.text_line_height_with_spacing();
    let [width, height] = size(cell, cfg.hotbar_only, heading);
    crate::hud_editor::measure("inventoryhud", [width, height]);
    let screen = ui.io().display_size;
    let origin = [
        crate::modules::hud::origin(cfg.x, screen[0], width),
        crate::modules::hud::origin(cfg.y, screen[1], height),
    ];
    let d = ui.get_background_draw_list();
    crate::theme::panel(&d, origin, [origin[0] + width, origin[1] + height]);
    d.add_text(
        [origin[0] + PADDING, origin[1] + 8.],
        crate::theme::TEXT,
        crate::localization::module_name("inventoryhud"),
    );
    let images = icons::current();
    // Keep one atlas batch even when every slot has a count and durability bar.
    d.channels_split(3, |channels| {
        for (display, slot) in slots(cfg.hotbar_only).enumerate() {
            channels.set_current(0);
            let row = display / COLUMNS;
            let p = [
                origin[0] + PADDING + (display % COLUMNS) as f32 * cell,
                origin[1]
                    + PADDING
                    + heading
                    + row as f32 * cell
                    + if !cfg.hotbar_only && row == 3 {
                        HOTBAR_GAP
                    } else {
                        0.
                    },
            ];
            let end = [p[0] + cell - 2., p[1] + cell - 2.];
            d.add_rect(p, end, crate::theme::SLOT)
                .filled(true)
                .rounding(4.)
                .build();
            let Some(item) = inv.slots.get(slot).and_then(Option::as_ref) else {
                continue;
            };
            let icon_size = cell - 8.;
            channels.set_current(1);
            let drawn = images.as_ref().is_some_and(|images| {
                images.draw(&d, &item.name, [p[0] + 3., p[1] + 3.], icon_size)
            });
            channels.set_current(2);
            if !drawn {
                let name = images
                    .as_ref()
                    .map_or(item.name.as_str(), |i| i.name(&item.name));
                let limit = cell - 7.;
                let mut stop = 0;
                for (at, c) in name.char_indices() {
                    if ui.calc_text_size(&name[..at + c.len_utf8()])[0] > limit {
                        break;
                    }
                    stop = at + c.len_utf8();
                }
                let label = if stop == 0 { "?" } else { &name[..stop] };
                d.add_text([p[0] + 3., p[1] + 3.], [0.9, 0.9, 0.95, 1.], label);
            }
            if cfg.show_durability && item.max_damage > 0 {
                let ratio = item.remaining() as f32 / item.max_damage as f32;
                let bar = [p[0] + 4., end[1] - 5.];
                let length = cell - 10.;
                crate::theme::progress(
                    &d,
                    bar,
                    [length, 3.],
                    ratio,
                    if ratio < 0.2 {
                        crate::theme::DANGER
                    } else if ratio < 0.5 {
                        crate::theme::WARNING
                    } else {
                        crate::theme::SUCCESS
                    },
                );
            }
            if cfg.show_counts && item.count > 1 {
                let text = item.count.to_string();
                let measured = ui.calc_text_size(&text);
                let pos = [
                    end[0] - measured[0] - 2.,
                    end[1]
                        - measured[1]
                        - if cfg.show_durability && item.max_damage > 0 {
                            6.
                        } else {
                            1.
                        },
                ];
                d.add_text([pos[0] + 1., pos[1] + 1.], [0., 0., 0., 1.], &text);
                d.add_text(pos, [1., 1., 1., 1.], text);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_preserves_all_slots_and_hotbar_order_including_empty_cells() {
        let full: Vec<_> = slots(false).collect();
        assert_eq!(full.len(), 36);
        assert_eq!(&full[..9], &(9..18).collect::<Vec<_>>());
        assert_eq!(&full[27..], &(0..9).collect::<Vec<_>>());
        assert_eq!(slots(true).collect::<Vec<_>>(), (0..9).collect::<Vec<_>>());
        let mut unique = full.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique, (0..36).collect::<Vec<_>>());
        assert_eq!(size(40., false, 24.), [380., 212.]);
        assert_eq!(size(40., true, 24.), [380., 84.]);
    }
}
