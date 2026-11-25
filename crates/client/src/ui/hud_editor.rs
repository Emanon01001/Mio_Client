//! Screen-space HUD placement. Dragging writes the existing profile settings.
use crate::{
    console,
    modules::{
        settings::{Detail, Kind, Value},
        Modules,
    },
};
use hudhook::imgui::{Condition, Key, StyleVar, Ui};
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

static SIZES: LazyLock<Mutex<BTreeMap<&'static str, [f32; 2]>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
pub fn measure(name: &'static str, size: [f32; 2]) {
    if size.iter().all(|v| v.is_finite() && *v > 0. && *v < 32768.) {
        SIZES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name, size);
    }
}
pub fn measured(name: &str, fallback: [f32; 2]) -> [f32; 2] {
    SIZES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .copied()
        .unwrap_or(fallback)
}
#[derive(Clone, Copy)]
enum Axis {
    Edge,
    Center(f32),
}
impl Axis {
    fn position(self, value: f32, screen: f32, length: f32) -> f32 {
        let p = match self {
            Self::Edge if value < 0. => screen + value - length,
            Self::Edge => value,
            Self::Center(align) => screen * 0.5 - length * align + value,
        };
        p.clamp(0., (screen - length).max(0.))
    }
    fn value(self, position: f32, screen: f32, length: f32, old: f32) -> f64 {
        match self {
            Self::Edge if old < 0. => (position + length - screen).round().min(-1.) as f64,
            Self::Edge => position.round() as f64,
            Self::Center(align) => (position - screen * 0.5 + length * align).round() as f64,
        }
    }
}
#[derive(Clone, Copy)]
struct Panel {
    name: &'static str,
    size: [f32; 2],
    axes: [Axis; 2],
    bias: [f32; 2],
    reset: [f64; 2],
}
const EDGE: [Axis; 2] = [Axis::Edge, Axis::Edge];
impl Panel {
    const fn edge(name: &'static str, size: [f32; 2], reset: [f64; 2]) -> Self {
        Self {
            name,
            size,
            axes: EDGE,
            bias: [0., 0.],
            reset,
        }
    }
}

const PANELS: [Panel; 25] = [
    Panel::edge("ping", [340., 44.], [10., -170.]),
    Panel::edge("debughud", [450., 260.], [10., 10.]),
    Panel::edge("chathud", [420., 200.], [-20., -200.]),
    Panel::edge("lightoverlay", [300., 80.], [20., -440.]),
    Panel::edge("mobcount", [260., 220.], [-240., 200.]),
    Panel::edge("worldinfo", [280., 90.], [-240., 90.]),
    Panel::edge("mininghud", [260., 110.], [20., 430.]),
    Panel::edge("itemcounter", [260., 140.], [20., 550.]),
    Panel::edge("armorhud", [360., 110.], [20., 240.]),
    Panel::edge("inventoryhud", [380., 212.], [-20., 520.]),
    Panel::edge("sessionstats", [250., 100.], [20., 150.]),
    Panel::edge("hud", [320., 238.], [10., 10.]),
    Panel {
        name: "compass",
        size: [420., 55.],
        axes: [Axis::Center(0.5), Axis::Edge],
        bias: [0., 0.],
        reset: [0., 10.],
    },
    Panel::edge("radar", [180., 180.], [-20., 90.]),
    Panel::edge("minimap", [200., 200.], [-20., 290.]),
    Panel::edge("frametime", [280., 110.], [20., -310.]),
    Panel::edge("performanceprofiler", [760., 380.], [20., 320.]),
    Panel::edge("keystrokes", [128., 150.], [10., -10.]),
    Panel::edge("targethud", [330., 110.], [-20., -160.]),
    Panel::edge("combathud", [230., 150.], [-20., 120.]),
    Panel::edge("vitals", [280., 110.], [20., -180.]),
    Panel::edge("playeralert", [300., 32.], [-20., 290.]),
    Panel::edge("effects", [230., 160.], [-20., 350.]),
    Panel::edge("air", [220., 45.], [20., -260.]),
    Panel {
        name: "waila",
        size: [300., 100.],
        axes: [Axis::Center(0.5), Axis::Center(0.)],
        bias: [0., -3.],
        reset: [0., 24.],
    },
];
fn option(options: &[Detail], id: &str) -> Option<f64> {
    options
        .iter()
        .find(|d| d.id == id)
        .and_then(|d| d.value().number().ok())
}
fn offsets(options: &[Detail]) -> Option<[f32; 2]> {
    Some([option(options, "x")? as f32, option(options, "y")? as f32])
}
fn write_position(modules: &mut Modules, index: usize, values: [f64; 2]) -> Result<(), String> {
    let options = modules.options(index);
    let mut new = [Value::Number(0.), Value::Number(0.)];
    let mut old = [Value::Number(0.), Value::Number(0.)];
    for (axis, id) in ["x", "y"].into_iter().enumerate() {
        let detail = options
            .iter()
            .find(|d| d.id == id)
            .ok_or("このHUDには位置設定がありません")?;
        let Kind::Number { min, max, .. } = detail.kind else {
            return Err("HUDの位置設定が不正です".into());
        };
        new[axis] = Value::Number(values[axis].clamp(min, max));
        old[axis] = detail.value();
        detail.validate(&new[axis])?;
    }
    modules.set_option_at(index, "x", &new[0])?;
    if let Err(error) = modules.set_option_at(index, "y", &new[1]) {
        let _ = modules.set_option_at(index, "x", &old[0]);
        return Err(error);
    }
    Ok(())
}
#[derive(Clone, Copy)]
struct Drag {
    name: &'static str,
    mouse: [f32; 2],
    position: [f32; 2],
    size: [f32; 2],
    offsets: [f32; 2],
}
pub struct Editor {
    pub active: bool,
    selected: &'static str,
    drag: Option<Drag>,
    snap: bool,
    only_selected: bool,
    error: String,
}
impl Default for Editor {
    fn default() -> Self {
        Self {
            active: false,
            selected: "hud",
            drag: None,
            snap: false,
            only_selected: false,
            error: String::new(),
        }
    }
}
impl Editor {
    pub fn start(&mut self) {
        self.active = true;
        self.drag = None;
        self.error.clear();
    }
    pub fn close(&mut self) {
        self.active = false;
        self.drag = None;
    }
    pub fn draw(
        &mut self,
        ui: &Ui,
        modules: &mut Modules,
        profile: &mut String,
        profile_status: &str,
    ) {
        if ui.is_key_pressed(Key::Escape) {
            self.close();
            return;
        }
        if !ui.io().mouse_down[0] {
            self.drag = None;
        }
        let screen = ui.io().display_size;
        // Draw the selected panel last, so overlapping panels can be separated.
        let mut panels = PANELS.to_vec();
        panels.sort_by_key(|p| p.name == self.selected);
        {
            let _padding = ui.push_style_var(StyleVar::WindowPadding([0., 0.]));
            let _minimum = ui.push_style_var(StyleVar::WindowMinSize([1., 1.]));
            for panel in panels {
                if self.only_selected && panel.name != self.selected {
                    continue;
                }
                let Some(index) = modules.find(panel.name) else {
                    continue;
                };
                let options = modules.options(index);
                let Some(values) = offsets(&options) else {
                    continue;
                };
                let view = modules.view(index);
                let mut size = measured(panel.name, panel.size);
                if matches!(panel.name, "radar" | "minimap") {
                    let side = (option(&options, "size").unwrap_or(panel.size[0] as f64) as f32)
                        .min((screen[0] - 10.).max(1.))
                        .min((screen[1] - 10.).max(1.));
                    size = [side; 2];
                }
                if panel.name == "compass" {
                    size = [
                        view.param
                            .map_or(400., |p| p.value as f32)
                            .min((screen[0] - 20.).max(1.))
                            + 20.,
                        55.,
                    ];
                }
                for axis in 0..2 {
                    size[axis] = size[axis].min(screen[axis].max(1.));
                }
                let position = std::array::from_fn(|k| {
                    panel.axes[k].position(values[k] + panel.bias[k], screen[k], size[k])
                });
                ui.window(format!("##hud_editor_{}", panel.name))
                    .position(position, Condition::Always)
                    .size(size, Condition::Always)
                    .title_bar(false)
                    .resizable(false)
                    .movable(false)
                    .scroll_bar(false)
                    .save_settings(false)
                    .draw_background(false)
                    .focus_on_appearing(false)
                    .build(|| {
                        ui.invisible_button("##drag", size);
                        if ui.is_item_activated() {
                            self.selected = panel.name;
                            self.drag = Some(Drag {
                                name: panel.name,
                                mouse: ui.io().mouse_pos,
                                position,
                                size,
                                offsets: values,
                            });
                        }
                        if ui.is_item_active() {
                            if let Some(drag) = self.drag.filter(|d| d.name == panel.name) {
                                let target = std::array::from_fn::<_, 2, _>(|k| {
                                    let raw =
                                        drag.position[k] + ui.io().mouse_pos[k] - drag.mouse[k];
                                    let snapped = if self.snap {
                                        (raw / 8.).round() * 8.
                                    } else {
                                        raw
                                    };
                                    snapped.clamp(0., (screen[k] - drag.size[k]).max(0.))
                                });
                                let new = std::array::from_fn(|k| {
                                    panel.axes[k].value(
                                        target[k],
                                        screen[k],
                                        drag.size[k],
                                        drag.offsets[k],
                                    ) - panel.bias[k] as f64
                                });
                                self.error = write_position(modules, index, new)
                                    .err()
                                    .unwrap_or_default();
                            }
                        }
                        let color = if self.selected == panel.name {
                            crate::theme::ACCENT
                        } else {
                            [0.72, 0.82, 0.95, 0.95]
                        };
                        // Follow window stacking so previews cannot paint over
                        // the layout controls. Keep the invisible drag hitbox.
                        let d = ui.get_window_draw_list();
                        // Empty previews need their own dark backing against
                        // bright terrain. Active HUD contents remain visible.
                        if !view.enabled {
                            d.add_rect(
                                position,
                                [position[0] + size[0], position[1] + size[1]],
                                [0.035, 0.052, 0.078, 0.94],
                            )
                            .filled(true)
                            .rounding(12.)
                            .build();
                        }
                        d.add_rect(
                            position,
                            [position[0] + size[0], position[1] + size[1]],
                            color,
                        )
                        .thickness(if self.selected == panel.name {
                            2.5
                        } else {
                            1.5
                        })
                        .rounding(12.)
                        .build();
                        let label = format!(
                            "{}{}",
                            crate::localization::module_name(panel.name),
                            if view.enabled { "" } else { "（無効）" }
                        );
                        let label = crate::text::fit_width(&label, (size[0] - 20.).max(0.), |s| {
                            ui.calc_text_size(s)[0]
                        });
                        d.add_rect(
                            [position[0] + 4., position[1] + 4.],
                            [
                                position[0] + ui.calc_text_size(&label)[0] + 16.,
                                position[1] + ui.text_line_height() + 12.,
                            ],
                            [0.035, 0.052, 0.078, 0.98],
                        )
                        .filled(true)
                        .rounding(5.)
                        .build();
                        d.add_text(
                            [position[0] + 10., position[1] + 8.],
                            if self.selected == panel.name {
                                crate::theme::ACCENT
                            } else {
                                crate::theme::TEXT
                            },
                            &label,
                        );
                    });
            }
        }
        ui.window("Mio · HUDの配置編集###HUDの配置編集")
            .position([(screen[0]*0.5-230.).max(0.),40.],Condition::FirstUseEver)
            .size([460.,(screen[1]-80.).clamp(320.,680.)],Condition::FirstUseEver)
            .size_constraints([360.,280.],[(screen[0]-20.).max(360.),(screen[1]-20.).max(280.)])
            .save_settings(false).build(|| {
                ui.text_colored(crate::theme::ACCENT,"MIO / HUD LAYOUT");
                ui.text_wrapped(crate::localization::text("Drag a HUD frame. Preview frames also work without a world. Features stay as configured."));
                ui.separator();
                ui.text_colored(crate::theme::MUTED,crate::localization::text("Panel"));
                ui.set_next_item_width(-1.);
                if let Some(_combo)=ui.begin_combo("##panel",crate::localization::module_name(self.selected)) {
                    for panel in PANELS {if ui.selectable_config(crate::localization::module_name(panel.name)).selected(self.selected==panel.name).build(){self.selected=panel.name;self.drag=None;}}
                }
                ui.checkbox(crate::localization::text("Only selected panel"),&mut self.only_selected);
                ui.checkbox(crate::localization::text("Snap 8 px"),&mut self.snap);
                if let Some(index)=modules.find(self.selected) {
                    for detail in modules.options(index).into_iter().filter(|d|matches!(d.id,"x"|"y")) {
                        if let Some(value)=crate::modules::settings::control(ui,&detail) {
                            self.error=modules.set_option_at(index,detail.id,&value).err().unwrap_or_default();
                        }
                    }
                    if ui.button(crate::localization::text("Reset position")) {
                        if let Some(panel)=PANELS.iter().find(|p|p.name==self.selected){self.error=write_position(modules,index,panel.reset).err().unwrap_or_default();}
                    }
                }
                if !self.error.is_empty(){ui.text_colored(crate::theme::WARNING,&self.error);}
                ui.separator();
                ui.text_colored(crate::theme::MUTED,crate::localization::text("Profile"));
                ui.set_next_item_width(-1.);ui.input_text("##editor_profile",profile).build();
                if ui.button(crate::localization::text("Save profile")){console::submit(&format!("profile save {profile}"));}
                ui.same_line();if ui.button(crate::localization::text("Load profile")){console::submit(&format!("profile load {profile}"));}
                if !profile_status.is_empty(){ui.text_wrapped(profile_status);}
                ui.separator();
                if ui.button(crate::localization::text("Done")){self.close();}ui.same_line();ui.text_disabled(crate::localization::text("Esc: return to menu"));
            });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dragging_preserves_anchors_at_multiple_resolutions_and_the_far_edge() {
        for (screen, length) in [(1920., 300.), (1280., 220.)] {
            for (axis, value) in [
                (Axis::Edge, 10.),
                (Axis::Edge, -20.),
                (Axis::Center(0.5), -40.),
                (Axis::Center(0.), 24.),
            ] {
                let p = axis.position(value, screen, length);
                let new = axis.value(p, screen, length, value) as f32;
                assert!((axis.position(new, screen, length) - p).abs() <= 1.);
            }
            let value = Axis::Edge.value(screen - length, screen, length, -20.) as f32;
            assert_eq!(value, -1.); // zero would switch a right anchor to the left
            assert_eq!(
                Axis::Edge.position(value, screen, length),
                screen - length - 1.
            );
        }
        assert_eq!(Axis::Edge.position(4000., 1280., 300.), 980.);
    }
    #[test]
    fn mouse_drag_changes_profile_positions_without_toggling_features() {
        let _serial = crate::gui::TEST_CONTEXT
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _serial = crate::game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cfg = crate::config::Config::default();
        let mut modules = Modules::new(&cfg);
        let enabled: Vec<_> = modules
            .settings()
            .iter()
            .map(|s| (s.name.clone(), s.enabled))
            .collect();
        let index = modules.find("radar").unwrap();
        let mut ctx = hudhook::imgui::Context::create();
        ctx.set_ini_filename(None);
        ctx.io_mut().display_size = [1280., 720.];
        ctx.io_mut().delta_time = 1. / 60.;
        ctx.fonts().build_rgba32_texture();
        let mut editor = Editor::default();
        editor.start();
        editor.only_selected = true;
        editor.selected = "radar";
        let mut profile = "test".to_string();
        let mut frame = |mouse, down| {
            ctx.io_mut().add_mouse_pos_event(mouse);
            ctx.io_mut()
                .add_mouse_button_event(hudhook::imgui::MouseButton::Left, down);
            editor.draw(ctx.frame(), &mut modules, &mut profile, "");
            ctx.render();
        };
        frame([1100., 160.], false);
        frame([1100., 160.], false);
        frame([1100., 160.], true);
        frame([1000., 220.], true);
        frame([1000., 220.], false);
        assert_eq!(offsets(&modules.options(index)), Some([-120., 150.]));
        let saved = modules.settings();
        write_position(&mut modules, index, [-20., 90.]).unwrap();
        modules.apply_settings(&saved).unwrap();
        assert_eq!(offsets(&modules.options(index)), Some([-120., 150.]));
        assert_eq!(
            modules
                .settings()
                .iter()
                .map(|s| (s.name.clone(), s.enabled))
                .collect::<Vec<_>>(),
            enabled
        );
    }
}
