//! Settings presentation. Original module indices remain the only mutation IDs.
use super::{
    feature_list::{category, Category, Sort},
    Gui, EJECT_REQUESTED,
};
use crate::{
    console, input, log,
    modules::{self, Modules},
    theme,
};
use hudhook::imgui::{Condition, StyleColor, StyleVar, Ui};
use std::sync::atomic::Ordering;

pub(super) fn initial_width(screen: [f32; 2]) -> f32 {
    620_f32.min((screen[0] - 120.) * 0.48).max(420.)
}

pub(super) fn inline_fits(ui: &Ui, text: &str) -> bool {
    ui.item_rect_max()[0] - ui.window_pos()[0] + ui.calc_text_size(text)[0] + 10.
        <= ui.window_content_region_max()[0]
}

/// The parts of a module a collapsed menu row needs.
struct Row {
    name: String,
    key: u16,
    enabled: bool,
}

impl Gui {
    pub(super) fn draw_menu(&mut self, ui: &Ui, modules: &mut Modules) {
        use super::window_layout::{self, Rect, Window};
        let screen = ui.io().display_size;
        // Where the menu was left last time, or the default.
        let initial = window_layout::initial(
            Window::Menu,
            screen,
            Rect {
                position: [40., 40.],
                size: [initial_width(screen), (screen[1] - 80.).clamp(380., 880.)],
            },
            [420., 380.],
        );
        ui.window("Mio Client###mio_client")
            .position(initial.position, Condition::FirstUseEver)
            .size(initial.size, Condition::FirstUseEver)
            .size_constraints(
                [420., 380.],
                [(screen[0] - 40.).max(420.), (screen[1] - 40.).max(380.)],
            )
            .save_settings(false)
            .build(|| {
                window_layout::observe(Window::Menu, ui.window_pos(), ui.window_size());
                theme::client_header(ui);
                if ui.button("HUDの配置を編集") {
                    self.hud_editor.start();
                }
                let hint = if self.menu_key == 0x1b {
                    "Esc で閉じる".to_owned()
                } else {
                    format!("{} / Esc で閉じる", self.menu_key_name)
                };
                if inline_fits(ui, &hint) {
                    ui.same_line();
                    ui.text_disabled(hint);
                }
                if let Some(_tree) = ui
                    .tree_node_config("ClickGUIの設定")
                    .push()
                {
                    if let Some(key) = modules::settings::key_control(ui,"メニューを開くキー",self.menu_key) {
                        match crate::config::menu_key::validate(key,self.eject_key) {
                            Ok(()) => {
                                self.menu_key = key;
                                self.menu_key_name = input::display_key_name(key);
                                self.key_was_down.store(input::key_down(key),Ordering::Release);
                                crate::config::menu_key::request(key);
                                self.setting_error.clear();
                            }
                            Err(e) => self.setting_error = e,
                        }
                    }
                    ui.text_wrapped("変更は設定ファイルへ自動保存されます。終了キー・未割り当て・通常のマウスボタンは使用できません。");
                    let status=crate::config::menu_key::status();
                    if !status.is_empty() {ui.text_wrapped(status);}
                }
                if let Some(_tree) = ui
                    .tree_node_config("プロファイル・保存")
                    .default_open(true)
                    .push()
                {
                    if let Ok(profiles) = self.profiles.try_lock() {
                        if !profiles.names.is_empty() {
                            ui.set_next_item_width(-1.);
                            if let Some(_combo) =
                                ui.begin_combo("##saved_profiles", &self.profile_name)
                            {
                                for name in &profiles.names {
                                    if ui
                                        .selectable_config(name)
                                        .selected(name == &self.profile_name)
                                        .build()
                                    {
                                        self.profile_name = name.clone();
                                    }
                                }
                            }
                        }
                        if !profiles.status.is_empty() {
                            ui.text_wrapped(&profiles.status);
                        }
                    }
                    ui.set_next_item_width(-1.);
                    ui.input_text("##profile_name", &mut self.profile_name)
                        .hint("プロファイル名")
                        .build();
                    if ui.button("保存") {
                        console::submit(&format!("profile save {}", self.profile_name));
                    }
                    ui.same_line();
                    if ui.button("読み込み") {
                        console::submit(&format!("profile load {}", self.profile_name));
                    }
                    ui.same_line();
                    if ui.button("一覧を更新") {
                        console::submit("profile list");
                    }
                    if ui.button("ファイルから読み込み…") {
                        let folder = self.profiles.try_lock().map(|p| p.dir().to_path_buf()).ok();
                        super::file_dialog::open_profile(folder);
                    }
                    if ui.is_item_hovered() {
                        ui.tooltip_text("任意の場所のプロファイル（.toml）を選んで読み込みます");
                    }
                }
                ui.separator();
                ui.set_next_item_width(-1.);
                ui.input_text("##module_filter", &mut self.module_filter)
                    .hint("機能を検索 / 日本語・コマンド名")
                    .build();
                // Wrap pills at narrow widths or large font sizes.
                for (index, filter) in std::iter::once(None)
                    .chain(Category::ALL.map(Some))
                    .enumerate()
                {
                    let label = filter.map_or("すべて", Category::short_label);
                    let width = ui.calc_text_size(label)[0] + 20.;
                    if index > 0
                        && ui.item_rect_max()[0] - ui.window_pos()[0] + width + 10.
                            < ui.window_content_region_max()[0]
                    {
                        ui.same_line();
                    }
                    let _color = (self.feature_list.category_filter == filter)
                        .then(|| ui.push_style_color(StyleColor::Button, [0.16, 0.33, 0.43, 1.]));
                    // An ID of its own, apart from other buttons with the same text.
                    if ui.button(format!("{label}##category")) {
                        self.feature_list.category_filter = filter;
                    }
                }
                ui.set_next_item_width((ui.content_region_avail()[0] * 0.60).min(250.));
                if let Some(_combo) = ui.begin_combo("##sort", self.feature_list.sort.label()) {
                    for sort in Sort::ALL {
                        if ui
                            .selectable_config(sort.label())
                            .selected(self.feature_list.sort == sort)
                            .build()
                        {
                            self.feature_list.sort = sort;
                        }
                    }
                }
                self.feature_list.update(
                    modules.len(),
                    |i| modules.menu_entry(i),
                    &self.module_filter,
                );
                ui.same_line();
                ui.text_disabled(format!(
                    "{} / {} 機能",
                    self.feature_list.order.len(),
                    modules.len()
                ));
                if !self.setting_error.is_empty() {
                    ui.text_colored(theme::WARNING, &self.setting_error);
                }
                ui.child_window("##features")
                    .size([0., -ui.frame_height_with_spacing() - 24.])
                    .build(|| {
                        let mut previous = None;
                        if self.feature_list.order.is_empty() {
                            ui.text_disabled("一致する機能がありません");
                        }
                        // Taken out while the rows (which may change settings) are drawn.
                        let order = std::mem::take(&mut self.feature_list.order);
                        for &i in &order {
                            let _id = ui.push_id_usize(i);
                            // Only what a row shows: building every module's status
                            // and parameter (`view`) is left to the expanded one.
                            let m = {
                                let (name, enabled) = modules.menu_entry(i);
                                Row {
                                    name: name.to_owned(),
                                    key: modules.menu_key(i),
                                    enabled,
                                }
                            };
                            if self.feature_list.sort == Sort::Category {
                                let group = category(&m.name);
                                if previous != Some(group) {
                                    ui.spacing();
                                    ui.text_colored(theme::MUTED, group.label());
                                    previous = Some(group);
                                }
                            }
                            let expanded = self.expanded_details.contains(&i);
                            let key = if m.key == 0 {
                                String::new()
                            } else {
                                format!("  ·  {}", input::key_name(m.key))
                            };
                            let label = format!(
                                "{}  {}{}",
                                if expanded { "▾" } else { "▸" },
                                crate::localization::module_name(&m.name),
                                key
                            );
                            let row_width = (ui.content_region_avail()[0] - 54.).max(100.);
                            let label =
                                crate::text::fit_width(&label, (row_width - 24.).max(0.), |s| {
                                    ui.calc_text_size(s)[0]
                                });
                            let height = ui.frame_height().max(32.);
                            {
                                let _align =
                                    ui.push_style_var(StyleVar::ButtonTextAlign([0.04, 0.5]));
                                if ui.button_with_size(
                                    format!("{label}###details"),
                                    [row_width, height],
                                ) {
                                    if expanded {
                                        self.expanded_details.remove(&i);
                                    } else {
                                        self.expanded_details.insert(i);
                                    }
                                }
                            }
                            if ui.is_item_hovered() {
                                ui.tooltip_text(format!(
                                    "{}\n設定を開く · コマンド名: {}",
                                    crate::localization::module_name(&m.name),
                                    m.name
                                ));
                            }
                            ui.same_line();
                            let mut on = m.enabled;
                            // Align the smaller switch vertically without changing the row height.
                            ui.set_cursor_pos([
                                ui.cursor_pos()[0],
                                ui.cursor_pos()[1] + (height - 24.) * 0.5,
                            ]);
                            if theme::switch(ui, "##enabled", &mut on) {
                                modules.set(i, on);
                            }
                            if expanded {
                                ui.indent();
                                self.draw_details(ui, modules, i);
                                ui.unindent();
                                ui.spacing();
                            }
                        }
                        self.feature_list.order = order;
                    });
                ui.separator();
                if ui.button("クライアントを終了") {
                    EJECT_REQUESTED.store(true, Ordering::Relaxed);
                }
                ui.same_line();
                let hint = crate::text::fit_width(
                    "設定は即時反映・保存はプロファイルへ",
                    ui.content_region_avail()[0],
                    |s| ui.calc_text_size(s)[0],
                );
                ui.text_disabled(hint);
            });
    }

    /// A module's settings: description, key, parameter, options, its own
    /// controls and status.
    pub(super) fn draw_details(&mut self, ui: &Ui, modules: &mut Modules, i: usize) {
        let m = modules.view(i);
        ui.text_wrapped(modules::settings::description(&m.name));
        let toggle_key = modules.supports_toggle_key(i);
        let mut details = modules.options(i);
        if toggle_key {
            if let Some(key) = modules::settings::key_control(ui, "Toggle key", m.key) {
                modules.set_key_at(i, key);
            }
        } else if details
            .first()
            .is_some_and(|d| matches!(d.kind, modules::settings::Kind::Key(_)))
        {
            // For hold-only features the activation key is the first control.
            let hold_key = details.remove(0);
            self.draw_detail(ui, modules, i, &m.name, &hold_key);
        }
        if let Some(p) = m.param {
            ui.text_colored(theme::MUTED, crate::localization::text(p.label));
            if let Some(value) = modules::settings::number_input(
                ui,
                "##primary",
                p.value,
                p.min,
                p.max,
                false,
                "%.2f",
            ) {
                modules.set_param_at(i, value);
            }
        }
        for detail in details {
            self.draw_detail(ui, modules, i, &m.name, &detail);
        }
        modules.draw_settings(i, ui);
        if !m.status.is_empty() {
            ui.text_wrapped(&m.status);
        }
    }

    fn draw_detail(
        &mut self,
        ui: &Ui,
        modules: &mut Modules,
        i: usize,
        name: &str,
        detail: &modules::settings::Detail,
    ) {
        if let Some(value) = modules::settings::control(ui, detail) {
            self.setting_error = match modules.set_option_at(i, detail.id, &value) {
                Ok(()) => String::new(),
                Err(e) => {
                    log!("{name}: {e}");
                    format!("{name}: {e}")
                }
            };
        }
    }
}
