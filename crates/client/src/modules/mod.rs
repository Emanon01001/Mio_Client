//! Toggleable features. Each module has a name, an optional hotkey and an
//! on/off state; some also react to the per-iteration [`Tick`].
//!
//! Features live in folders named after their ClickGUI category (`movement`,
//! `combat`, `world`, `visual`, `hud`, `other`) and are re-exported here, so
//! each is addressed as `crate::modules::<name>`. `hud` also holds the HUD
//! list and the simple readouts. Shared pieces stay at this level:
//! `settings` (option schema), `view` (projection and ray geometry) and
//! `entity_kind` (entity classification).

mod combat;
pub mod hud;
mod movement;
mod other;
mod visual;
mod world;

pub mod entity_kind;
pub mod settings;
pub(crate) mod view;

pub use combat::{
    auto_clicker, auto_weapon, combat_hud, friends, hit_marker, kill_aura, player_alert,
    target_hud, trigger_bot,
};
pub use hud::{
    air, chat_hud, compass, coordinates, debug_hud, durability_alert, effects, frame_time,
    item_counter, item_hud, keystrokes, low_health, mining_hud, ping, radar, session_stats, vitals,
};
pub use movement::{auto_move, fly, inventory_move, safe_walk, timer};
pub use other::{chat_log, chat_repeat, discord_rpc, join_notify, patch};
pub(crate) use other::{performance_profiler, skin_stealer};
use visual::block_outline;
pub use visual::{
    block_esp, breadcrumbs, camera, crosshair, esp, fov, fullbright, nametags, trajectories,
};
use world::player_list;
pub use world::{
    auto_fish, chunk_border, durability_guard, item_assist, light_overlay, map_marks, minimap,
    mob_count, slime, waila, waypoints, world_info, world_map,
};

use crate::game;

use crate::config::Config;
use crate::input;
use crate::log;

/// Per-iteration state passed to every module.
pub struct Tick {
    /// Automatic actions may run: the game is focused, no menu is open and
    /// (if configured) the player is in the world view.
    pub active: bool,
    /// Why `active` is false, for display.
    pub paused: Option<&'static str>,
    /// The game window has the keyboard focus.
    pub focused: bool,
    /// Milliseconds from an unhooked clock (GetTickCount64).
    pub now_ms: u64,
}

/// A fresh world view with no game screen over it (including InventoryMove).
/// Pure HUD features must not inherit the inventory movement exception.
pub(super) fn hud_view() -> bool {
    game::in_world_view_now() && !game::inventory::screen_open()
}

/// A module's single adjustable number, with the range the UI may offer.
pub struct Param {
    pub label: &'static str,
    pub value: f64,
    pub min: f64,
    pub max: f64,
}

// `Send`: modules are shared between the main loop and the render thread.
pub trait Module: Send {
    fn name(&self) -> &str;
    fn key(&self) -> u16;
    /// Hold-only features keep their own activation key, without an ON/OFF hotkey.
    fn supports_toggle_key(&self) -> bool {
        true
    }
    fn enabled(&self) -> bool;
    fn set_enabled(&mut self, on: bool);
    fn tick(&mut self, _tick: &Tick) {}
    fn param(&self) -> Option<Param> {
        None
    }
    /// Sets the module's parameter (clamped by the module); no-op without one.
    fn set_param(&mut self, _value: f64) {}
    fn options(&self) -> Vec<settings::Detail> {
        Vec::new()
    }
    fn set_option(&mut self, id: &str, _value: &settings::Value) -> Result<(), String> {
        Err(format!("不明な設定項目: {id}"))
    }
    fn status(&self) -> String {
        String::new()
    }
    fn draw(&mut self, _ui: &hudhook::imgui::Ui) {}
    /// Optional editors/buttons that do not fit the common scalar controls.
    fn draw_settings(&mut self, _ui: &hudhook::imgui::Ui) {}
}

/// Snapshot of one module for display.
pub struct ModuleView {
    pub name: String,
    pub key: u16,
    pub enabled: bool,
    pub param: Option<Param>,
    pub status: String,
}

/// Parses a configured key name, warning (and unbinding) if it is invalid.
pub fn key_or_warn(owner: &str, name: &str) -> u16 {
    input::parse_key(name).unwrap_or_else(|| {
        log!("{owner}: 不明なキー`{name}`の割り当てを解除しました");
        0
    })
}

fn canonical_name(name: &str) -> &str {
    if name.eq_ignore_ascii_case("tablist") {
        "playerlist"
    } else {
        name
    }
}

pub struct Modules {
    list: Vec<Box<dyn Module>>,
    key_state: Vec<bool>,
    hotkeys: std::collections::HashMap<usize, u16>,
}

impl Modules {
    pub fn new(config: &Config) -> Self {
        game::init(&config.offsets);
        // Each module with whether the config wants it on at startup.
        let mut entries: Vec<(Box<dyn Module>, bool)> = vec![
            (Box::new(hud::Layout::new(&config.gui)), config.gui.show_hud),
            (
                Box::new(effects::Effects::new(&config.effects)),
                config.effects.enabled,
            ),
            (Box::new(air::Air::new(&config.air)), config.air.enabled),
            (
                Box::new(safe_walk::SafeWalk::new(&config.safewalk)),
                config.safewalk.enabled,
            ),
            (
                Box::new(durability_guard::DurabilityGuard::new(
                    &config.durabilityguard,
                )),
                config.durabilityguard.enabled,
            ),
            (
                Box::new(auto_fish::AutoFish::new(&config.autofish)),
                config.autofish.enabled,
            ),
            (
                Box::new(durability_alert::DurabilityAlert::new(
                    &config.durabilityalert,
                )),
                config.durabilityalert.enabled,
            ),
            (
                Box::new(item_hud::ItemHud::new(&config.armorhud, true)),
                config.armorhud.enabled,
            ),
            (
                Box::new(item_hud::ItemHud::new(&config.inventoryhud, false)),
                config.inventoryhud.enabled,
            ),
            (
                Box::new(session_stats::SessionStats::new(&config.sessionstats)),
                config.sessionstats.enabled,
            ),
            (
                Box::new(waypoints::Deathpoint::new(&config.deathpoint)),
                config.deathpoint.enabled,
            ),
            (
                Box::new(item_assist::ItemAssist::new(&config.autotool, false)),
                config.autotool.enabled,
            ),
            (
                Box::new(item_assist::ItemAssist::new(&config.autoeat, true)),
                config.autoeat.enabled,
            ),
            (
                Box::new(crosshair::Crosshair::new(&config.crosshair)),
                config.crosshair.enabled,
            ),
            (
                Box::new(low_health::LowHealth::new(&config.lowhealth)),
                config.lowhealth.enabled,
            ),
            (
                Box::new(player_alert::PlayerAlert::new(&config.playeralert)),
                config.playeralert.enabled,
            ),
            (
                Box::new(vitals::Vitals::new(&config.vitals)),
                config.vitals.enabled,
            ),
            (
                Box::new(nametags::NameTags::new(&config.nametags)),
                config.nametags.enabled,
            ),
            (
                Box::new(radar::Radar::new(&config.radar)),
                config.radar.enabled,
            ),
            (
                Box::new(target_hud::TargetHud::new(&config.targethud)),
                config.targethud.enabled,
            ),
            (
                Box::new(combat_hud::CombatHud::new(&config.combathud)),
                config.combathud.enabled,
            ),
            (
                Box::new(hit_marker::HitMarker::new(&config.hitmarker)),
                config.hitmarker.enabled,
            ),
            (
                Box::new(compass::Compass::new(&config.compass)),
                config.compass.enabled,
            ),
            (
                Box::new(inventory_move::InventoryMove::new(&config.inventorymove)),
                config.inventorymove.enabled,
            ),
            (
                Box::new(auto_move::AutoSprint::new(&config.auto_sprint)),
                config.auto_sprint.enabled,
            ),
            (
                Box::new(auto_move::AutoMove::walk(&config.auto_walk)),
                config.auto_walk.enabled,
            ),
            (
                Box::new(auto_move::AutoMove::sneak(&config.auto_sneak)),
                config.auto_sneak.enabled,
            ),
            (
                Box::new(auto_move::AutoMove::jump(&config.auto_jump)),
                config.auto_jump.enabled,
            ),
            (
                Box::new(auto_move::AutoMove::swim(&config.auto_swim)),
                config.auto_swim.enabled,
            ),
            (
                Box::new(auto_clicker::AutoClicker::new(&config.auto_clicker)),
                config.auto_clicker.enabled,
            ),
            (
                Box::new(kill_aura::KillAura::new(&config.killaura)),
                config.killaura.enabled,
            ),
            (
                Box::new(timer::Timer::new(&config.timer)),
                config.timer.enabled,
            ),
            (
                Box::new(coordinates::Coordinates::new(&config.coordinates)),
                config.coordinates.enabled,
            ),
            (Box::new(esp::Esp::new(&config.esp)), config.esp.enabled),
            (
                Box::new(block_outline::BlockOutline::new(&config.blockoutline)),
                config.blockoutline.enabled,
            ),
            (
                Box::new(player_list::PlayerList::new(&config.playerlist)),
                config.playerlist.enabled,
            ),
            (
                Box::new(block_esp::BlockEsp::new(&config.blockesp)),
                config.blockesp.enabled,
            ),
            (Box::new(fly::Fly::new(&config.fly)), config.fly.enabled),
            (Box::new(fov::Zoom::new(&config.zoom)), config.zoom.enabled),
            (Box::new(fov::Fov::new(&config.fov)), config.fov.enabled),
            (
                Box::new(waila::Waila::new(&config.waila)),
                config.waila.enabled,
            ),
            (
                Box::new(breadcrumbs::Breadcrumbs::new(&config.breadcrumbs)),
                config.breadcrumbs.enabled,
            ),
            (
                Box::new(fullbright::Fullbright::new(&config.fullbright)),
                config.fullbright.enabled,
            ),
        ];
        for (info, on) in hud::modules(config) {
            entries.push((Box::new(info), on));
        }
        entries.push((Box::new(ping::Ping::new(&config.ping)), config.ping.enabled));
        entries.push((
            Box::new(keystrokes::Keystrokes::new(&config.keystrokes)),
            config.keystrokes.enabled,
        ));
        entries.push((
            Box::new(waypoints::Waypoints::new(&config.waypoints, &config.dir)),
            config.waypoints.enabled,
        ));
        entries.push((
            Box::new(chunk_border::ChunkBorder::new(&config.chunkborder)),
            config.chunkborder.enabled,
        ));
        entries.push((
            Box::new(chat_log::ChatLog::new(&config.chatlog, &config.dir)),
            config.chatlog.enabled,
        ));
        entries.push((
            Box::new(chat_hud::ChatHud::new(&config.chathud)),
            config.chathud.enabled,
        ));
        entries.push((
            Box::new(join_notify::JoinNotify::new(&config.joinnotify)),
            config.joinnotify.enabled,
        ));
        entries.push((
            Box::new(chat_repeat::ChatRepeat::new(&config.chatrepeat)),
            config.chatrepeat.enabled,
        ));
        entries.push((
            Box::new(mining_hud::MiningHud::new(&config.mininghud)),
            config.mininghud.enabled,
        ));
        entries.push((
            Box::new(item_counter::ItemCounter::new(&config.itemcounter)),
            config.itemcounter.enabled,
        ));
        entries.push((
            Box::new(trigger_bot::TriggerBot::new(&config.triggerbot)),
            config.triggerbot.enabled,
        ));
        entries.push((
            Box::new(auto_weapon::AutoWeapon::new(&config.autoweapon)),
            config.autoweapon.enabled,
        ));
        entries.push((
            Box::new(friends::Friends::new(&config.friends)),
            config.friends.enabled,
        ));
        entries.push((
            Box::new(minimap::Minimap::new(&config.minimap)),
            config.minimap.enabled,
        ));
        entries.push((
            Box::new(trajectories::Trajectories::new(&config.trajectories)),
            config.trajectories.enabled,
        ));
        entries.push((
            Box::new(mob_count::MobCount::new(&config.mobcount)),
            config.mobcount.enabled,
        ));
        entries.push((
            Box::new(world_info::WorldInfo::new(&config.worldinfo)),
            config.worldinfo.enabled,
        ));
        entries.push((
            Box::new(light_overlay::LightOverlay::new(&config.lightoverlay)),
            config.lightoverlay.enabled,
        ));
        entries.push((
            Box::new(camera::FreeLook::new(&config.freelook)),
            config.freelook.enabled,
        ));
        entries.push((
            Box::new(camera::Freecam::new(&config.freecam)),
            config.freecam.enabled,
        ));
        entries.push((
            Box::new(frame_time::FrameTime::new(&config.frametime)),
            config.frametime.enabled,
        ));
        entries.push((
            Box::new(performance_profiler::PerformanceProfiler::new(
                &config.performanceprofiler,
            )),
            config.performanceprofiler.enabled,
        ));
        entries.push((
            Box::new(discord_rpc::DiscordRpc::new(&config.discordrpc)),
            config.discordrpc.enabled,
        ));
        entries.push((
            Box::new(world_map::WorldMap::new(&config.worldmap, &config.dir)),
            false,
        ));
        entries.push((
            Box::new(debug_hud::DebugHud::new(&config.debughud)),
            config.debughud.enabled,
        ));
        // `patch::load` returns one module per `[[patch]]`, in order.
        entries.push((
            Box::new(skin_stealer::SkinStealer::new(
                &config.skinstealer,
                &config.dir,
            )),
            config.skinstealer.enabled,
        ));
        for (p, cfg) in patch::load(&config.patch).into_iter().zip(&config.patch) {
            entries.push((Box::new(p), cfg.enabled));
        }

        let (list, wanted): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
        let mut modules = Self {
            key_state: vec![false; list.len()],
            hotkeys: Default::default(),
            list,
        };
        performance_profiler::metrics::configure(modules.list.iter().map(|m| m.name()));
        for (i, on) in wanted.into_iter().enumerate() {
            if on {
                modules.set(i, true);
            }
        }
        modules
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Lightweight metadata for presentation; callers retain the original index.
    pub(crate) fn menu_entry(&self, index: usize) -> (&str, bool) {
        let module = &self.list[index];
        (module.name(), module.enabled())
    }
    /// The hotkey shown on a menu row (no status or parameter is built).
    pub(crate) fn menu_key(&self, index: usize) -> u16 {
        self.key_at(index)
    }
    pub(crate) fn supports_toggle_key(&self, index: usize) -> bool {
        self.list[index].supports_toggle_key()
    }
    pub(crate) fn draw_settings(&mut self, index: usize, ui: &hudhook::imgui::Ui) {
        self.list[index].draw_settings(ui);
    }

    /// HUD needs only enabled names/statuses, without allocating menu views
    /// or calculating status/parameters for disabled modules each frame.
    pub fn hud_lines(&self) -> impl Iterator<Item = (&str, String)> {
        self.list
            .iter()
            .filter(|m| m.enabled() && m.name() != "hud")
            .map(|m| (m.name(), m.status()))
    }
    pub fn draw_widgets(&mut self, ui: &hudhook::imgui::Ui) {
        self.draw_widgets_filtered(ui, |_| true);
    }
    pub(crate) fn draw_widgets_filtered(
        &mut self,
        ui: &hudhook::imgui::Ui,
        visible: impl Fn(&str) -> bool,
    ) {
        for (index, module) in self.list.iter_mut().enumerate() {
            if module.enabled() && visible(module.name()) {
                let _profile = performance_profiler::metrics::Scope::module(index, true);
                module.draw(ui);
            }
        }
    }

    pub fn view(&self, index: usize) -> ModuleView {
        let m = &self.list[index];
        ModuleView {
            name: m.name().to_string(),
            key: self.key_at(index),
            enabled: m.enabled(),
            param: m.param(),
            status: m.status(),
        }
    }

    pub fn set(&mut self, index: usize, on: bool) {
        let module = &mut self.list[index];
        module.set_enabled(on);
        // A module may refuse to enable (e.g. signature not found).
        let state = if module.enabled() { "ON" } else { "OFF" };
        log!(
            "{} ({}) -> {state}",
            crate::localization::module_name(module.name()),
            module.name()
        );
        // Say when a switch did not take, rather than showing the old state.
        if module.enabled() == on {
            crate::notify::push(&format!(
                "{} {state}",
                crate::localization::module_name(module.name())
            ));
        } else {
            crate::notify::push(&format!(
                "{}は利用できません",
                crate::localization::module_name(module.name())
            ));
        }
    }

    pub fn set_param_at(&mut self, index: usize, value: f64) {
        self.list[index].set_param(value);
    }

    pub(crate) fn find(&self, name: &str) -> Option<usize> {
        let name = canonical_name(name);
        self.list
            .iter()
            .position(|m| m.name().eq_ignore_ascii_case(name))
    }

    pub fn toggle(&mut self, name: &str) {
        match self.find(name) {
            Some(i) => {
                let on = !self.list[i].enabled();
                self.set(i, on);
            }
            None => log!("`{name}`という機能はありません（listで一覧を表示）"),
        }
    }

    pub fn set_param(&mut self, name: &str, value: f64) {
        let Some(i) = self.find(name) else {
            log!("`{name}`という機能はありません（listで一覧を表示）");
            return;
        };
        let module = &mut self.list[i];
        if module.param().is_none() {
            log!("{}には数値設定がありません", module.name());
            return;
        }
        module.set_param(value);
        log!("{}: {}", module.name(), module.status());
    }

    fn key_at(&self, index: usize) -> u16 {
        if !self.list[index].supports_toggle_key() {
            return 0;
        }
        self.hotkeys
            .get(&index)
            .copied()
            .unwrap_or_else(|| self.list[index].key())
    }
    pub fn set_key_at(&mut self, index: usize, key: u16) {
        if !self.list[index].supports_toggle_key() {
            return;
        }
        self.hotkeys.insert(index, key);
        self.key_state[index] = input::key_down(key);
    }
    pub fn options(&self, index: usize) -> Vec<settings::Detail> {
        self.list[index].options()
    }
    pub fn set_option_at(
        &mut self,
        index: usize,
        id: &str,
        value: &settings::Value,
    ) -> Result<(), String> {
        let detail = self.list[index]
            .options()
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| format!("不明な設定項目: {id}"))?;
        detail.validate(value)?;
        self.list[index].set_option(id, value)
    }
    fn setting_at(&self, i: usize) -> crate::profiles::Setting {
        let m = &self.list[i];
        crate::profiles::Setting {
            name: m.name().into(),
            enabled: m.enabled(),
            value: m.param().map(|p| p.value),
            key: m
                .supports_toggle_key()
                .then(|| input::key_name(self.key_at(i))),
            options: m
                .options()
                .into_iter()
                .map(|d| (d.id.into(), d.value()))
                .collect(),
        }
    }
    pub fn settings(&self) -> Vec<crate::profiles::Setting> {
        (0..self.list.len()).map(|i| self.setting_at(i)).collect()
    }
    /// Validate all types/ranges/keys before mutation. Roll back switches and
    /// every detailed setting together if a native operation fails.
    pub fn apply_settings(
        &mut self,
        settings: &[crate::profiles::Setting],
    ) -> Result<Vec<String>, String> {
        let mut seen = std::collections::HashSet::new();
        let mut plan = Vec::new();
        let mut skipped = Vec::new();
        for s in settings {
            if !seen.insert(canonical_name(&s.name).to_ascii_lowercase()) {
                return Err(format!("duplicate module {}", s.name));
            }
            if s.value.is_some_and(|v| !v.is_finite()) {
                return Err(format!("{}: 有限の数値を指定してください", s.name));
            }
            let Some(i) = self.find(&s.name) else {
                skipped.push(s.name.clone());
                continue;
            };
            if let Some(key) = s
                .key
                .as_ref()
                .filter(|_| self.list[i].supports_toggle_key())
            {
                if input::parse_key(key).is_none() {
                    return Err(format!("{}: 不正な切り替えキーです", s.name));
                }
            }
            if let Some(value) = s.value {
                let p = self.list[i]
                    .param()
                    .ok_or_else(|| format!("{}には数値設定がありません", s.name))?;
                if value < p.min || value > p.max {
                    return Err(format!(
                        "{}の値は{}～{}の範囲で指定してください",
                        s.name, p.min, p.max
                    ));
                }
            }
            let details = self.list[i].options();
            for (id, value) in &s.options {
                let detail = details
                    .iter()
                    .find(|d| d.id == id)
                    .ok_or_else(|| format!("{}: 不明な設定項目 {id}", s.name))?;
                detail
                    .validate(value)
                    .map_err(|e| format!("{}: {e}", s.name))?;
            }
            plan.push((i, s.clone()));
        }
        if plan.is_empty() {
            return Err("プロファイルに適用できる機能がありません".into());
        }
        let before: Vec<_> = plan
            .iter()
            .map(|(i, _)| (*i, self.setting_at(*i)))
            .collect();
        if let Err(error) = self.apply_plan(&plan) {
            let restore = self.apply_plan(&before);
            return Err(match restore {
                Ok(()) => format!("{error}（以前の設定へ戻しました）"),
                Err(e) => format!("{error}（設定の復元にも失敗: {e}）"),
            });
        }
        Ok(skipped)
    }
    fn apply_plan(&mut self, plan: &[(usize, crate::profiles::Setting)]) -> Result<(), String> {
        for (i, s) in plan {
            if !s.enabled && self.list[*i].enabled() {
                self.list[*i].set_enabled(false);
            }
        }
        for (i, s) in plan {
            if let Some(v) = s.value {
                if self.list[*i].param().is_some_and(|p| p.value != v) {
                    self.list[*i].set_param(v);
                }
            }
            for (id, value) in &s.options {
                let current = self.list[*i]
                    .options()
                    .into_iter()
                    .find(|d| d.id == id)
                    .map(|d| d.value());
                if current.as_ref() != Some(value) {
                    self.set_option_at(*i, id, value)?;
                }
            }
            if let Some(key) = s
                .key
                .as_ref()
                .filter(|_| self.list[*i].supports_toggle_key())
            {
                self.set_key_at(*i, input::parse_key(key).ok_or("invalid key")?);
            }
        }
        for (i, s) in plan {
            if s.enabled && !self.list[*i].enabled() {
                self.list[*i].set_enabled(true);
            }
        }
        for (i, s) in plan {
            if self.list[*i].enabled() != s.enabled {
                return Err(format!(
                    "{}を{}にできませんでした",
                    self.list[*i].name(),
                    if s.enabled { "enabled" } else { "disabled" }
                ));
            }
        }
        Ok(())
    }

    /// Edge-detects hotkeys. `focused` gates toggling so keys typed into
    /// other applications are ignored.
    pub fn poll_hotkeys(&mut self, focused: bool) {
        for i in 0..self.list.len() {
            let down = input::key_down(self.key_at(i));
            let pressed = down && !self.key_state[i];
            self.key_state[i] = down;
            if pressed && focused && !crate::gui::captures_input() {
                let on = !self.list[i].enabled();
                self.set(i, on);
            }
        }
    }

    pub fn tick(&mut self, tick: &Tick) {
        hud::poll(tick.focused, tick.now_ms);
        for (index, module) in self.list.iter_mut().enumerate() {
            let _profile = performance_profiler::metrics::Scope::module(index, false);
            module.tick(tick);
        }
    }

    pub fn print_list(&self) {
        for (i, m) in self.list.iter().enumerate() {
            let state = if m.enabled() { "ON " } else { "OFF" };
            let key = match self.key_at(i) {
                0 => "-".to_string(),
                vk => input::key_name(vk),
            };
            log!(
                "  [{state}] {} ({}) キー {key} {}",
                crate::localization::module_name(m.name()),
                m.name(),
                m.status()
            );
        }
    }

    pub fn disable_all(&mut self) {
        for module in &mut self.list {
            if module.enabled() {
                module.set_enabled(false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn zoom_ignores_legacy_toggle_bindings_and_preserves_its_hold_key_and_factor() {
        use super::{settings, Modules};
        let _serial = super::game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let config = crate::config::Config {
            zoom: toml::from_str("key = 'F6'\nhold_key = 'C'\nfactor = 10.0\n").unwrap(),
            ..Default::default()
        };
        let mut modules = Modules::new(&config);
        let index = modules.find("zoom").unwrap();
        assert!(!modules.supports_toggle_key(index));
        assert_eq!(modules.view(index).key, 0);
        modules.set_key_at(index, crate::input::parse_key("F7").unwrap());
        assert_eq!(modules.view(index).key, 0);
        let mut legacy = modules.setting_at(index);
        legacy.key = Some("F8".into());
        legacy.value = Some(12.);
        legacy
            .options
            .insert("hold_key".into(), settings::Value::Key("MOUSE4".into()));
        modules.apply_settings(&[legacy]).unwrap();
        let saved = modules.setting_at(index);
        assert_eq!(saved.key, None);
        assert_eq!(saved.value, Some(12.));
        assert_eq!(
            saved.options["hold_key"],
            settings::Value::Key("MOUSE4".into())
        );
        assert_eq!(modules.menu_key(index), 0);
    }

    #[test]
    fn profiler_collects_module_and_shared_work_and_resets_without_old_samples() {
        use super::performance_profiler::metrics::{self, Scope, Stage};
        let _serial = super::game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut modules = super::Modules::new(&crate::config::Config::default());
        let index = modules.find("performanceprofiler").unwrap();
        assert!(!modules.view(index).enabled);
        modules.set(index, true);
        let tick = |now_ms| super::Tick {
            active: false,
            paused: Some("test"),
            focused: false,
            now_ms,
        };
        modules.tick(&tick(100));
        {
            let mut work = Scope::new(Stage::BlockScan);
            work.units(123);
        }
        modules.tick(&tick(1100));
        let first = metrics::snapshot();
        assert!(first
            .rows
            .iter()
            .any(|r| r.label.name == "BlockESP" && r.units == 123));
        assert!(first
            .rows
            .iter()
            .any(|r| r.label.name == "AutoFish" && r.label.phase == "更新"));
        modules.set(index, false);
        {
            let mut work = Scope::new(Stage::BlockScan);
            work.units(999);
        }
        assert!(!metrics::active());
        modules.set(index, true);
        assert!(metrics::snapshot().rows.is_empty());
        modules.tick(&tick(2100));
        modules.tick(&tick(3100));
        assert!(metrics::snapshot().rows.iter().all(|r| r.units == 0));
        modules.set(index, false);
    }
    use super::*;
    #[test]
    fn chat_repeat_settings_round_trip_without_starting_or_sending() {
        let _serial = game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut modules = Modules::new(&Config::default());
        let i = modules.find("chatrepeat").unwrap();
        for (id, v) in [
            (
                "text",
                settings::Value::Text {
                    text: "こんにちは".into(),
                },
            ),
            ("interval", settings::Value::Number(0.251)),
            ("count", settings::Value::Number(3.)),
        ] {
            modules.set_option_at(i, id, &v).unwrap();
        }
        let saved = modules.settings();
        modules
            .set_option_at(
                i,
                "text",
                &settings::Value::Text {
                    text: "別の本文".into(),
                },
            )
            .unwrap();
        modules.apply_settings(&saved).unwrap();
        assert_eq!(modules.settings(), saved);
        assert!(!modules.view(i).enabled);
    }
    #[test]
    fn environment_settings_round_trip_and_out_of_game_native_features_refuse_activation() {
        let _serial = game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut modules = Modules::new(&Config::default());
        for name in [
            "mobcount",
            "worldinfo",
            "lightoverlay",
            "freelook",
            "freecam",
        ] {
            let i = modules.find(name).unwrap();
            assert!(!modules.view(i).enabled);
            modules.toggle(name);
            assert!(!modules.view(i).enabled);
        }
        for (name, id, v) in [
            ("mobcount", "rows", settings::Value::Number(5.)),
            ("worldinfo", "x", settings::Value::Number(-30.)),
            ("hud", "vertical", settings::Value::Bool(true)),
            ("hud", "width", settings::Value::Number(420.)),
            ("ping", "interval_ms", settings::Value::Number(3000.)),
            ("ping", "x", settings::Value::Number(-25.)),
            ("lightoverlay", "threshold", settings::Value::Number(7.)),
            ("freelook", "hold_key", settings::Value::Key("RALT".into())),
            ("freecam", "speed", settings::Value::Number(17.)),
            ("freecam", "boost", settings::Value::Number(2.)),
            ("frametime", "line", settings::Value::Bool(false)),
            ("performanceprofiler", "rows", settings::Value::Number(8.)),
            (
                "performanceprofiler",
                "update_ms",
                settings::Value::Number(500.),
            ),
            (
                "chatlog",
                "highlight",
                settings::Value::Text {
                    text: "diamond\n日本語".into(),
                },
            ),
        ] {
            let i = modules.find(name).unwrap();
            modules.set_option_at(i, id, &v).unwrap();
        }
        let saved = modules.settings();
        let i = modules.find("lightoverlay").unwrap();
        assert!(modules
            .set_option_at(i, "threshold", &settings::Value::Number(16.))
            .is_err());
        modules
            .set_option_at(i, "threshold", &settings::Value::Number(0.))
            .unwrap();
        modules.apply_settings(&saved).unwrap();
        assert_eq!(modules.settings(), saved);
    }

    #[test]
    fn english_titles_keep_japanese_descriptions_options_and_stable_profile_ids() {
        let _serial = game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let modules = Modules::new(&Config::default());
        let mut ids = std::collections::HashSet::new();
        for index in 0..modules.len() {
            let (id, _) = modules.menu_entry(index);
            assert!(id.is_ascii(), "command id changed: {id}");
            assert!(ids.insert(id));
            assert_ne!(
                crate::localization::module_name(id),
                id,
                "missing module title: {id}"
            );
            assert!(
                crate::localization::module_name(id).is_ascii(),
                "non-English title: {id}"
            );
            assert!(
                !settings::description(id).is_ascii(),
                "description lost Japanese: {id}"
            );
            for option in modules.options(index) {
                assert!(option.id.is_ascii(), "saved key changed: {}", option.id);
                if !option.label.is_empty() && !matches!(option.label, "X" | "Y") {
                    assert!(
                        !option.label.is_ascii(),
                        "untranslated label: {id}/{}: {}",
                        option.id,
                        option.label
                    );
                }
                if !option.hint.is_empty() {
                    assert!(
                        !option.hint.is_ascii(),
                        "untranslated hint: {id}/{}",
                        option.id
                    );
                }
            }
        }
    }

    #[test]
    fn profiles_prevalidate_and_restore_switches_and_values_when_enable_fails() {
        use crate::profiles::Setting;
        struct Fake {
            name: &'static str,
            on: bool,
            value: f64,
            refuse: bool,
            flag: bool,
        }
        impl Module for Fake {
            fn name(&self) -> &str {
                self.name
            }
            fn key(&self) -> u16 {
                0
            }
            fn enabled(&self) -> bool {
                self.on
            }
            fn set_enabled(&mut self, on: bool) {
                if !(on && self.refuse) {
                    self.on = on;
                }
            }
            fn param(&self) -> Option<Param> {
                Some(Param {
                    label: "value",
                    value: self.value,
                    min: 1.,
                    max: 10.,
                })
            }
            fn set_param(&mut self, v: f64) {
                self.value = v;
            }
            fn options(&self) -> Vec<settings::Detail> {
                vec![settings::Detail::boolean("flag", "Flag", "", self.flag)]
            }
            fn set_option(&mut self, _: &str, value: &settings::Value) -> Result<(), String> {
                self.flag = value.boolean()?;
                Ok(())
            }
        }
        let mut m = Modules {
            list: vec![
                Box::new(Fake {
                    name: "a",
                    on: true,
                    value: 2.,
                    refuse: false,
                    flag: true,
                }),
                Box::new(Fake {
                    name: "b",
                    on: false,
                    value: 3.,
                    refuse: true,
                    flag: true,
                }),
            ],
            key_state: vec![false; 2],
            hotkeys: Default::default(),
        };
        let before = m.settings();
        let invalid = [
            Setting {
                name: "a".into(),
                enabled: false,
                value: Some(5.),
                key: Some("F9".into()),
                options: [("flag".into(), settings::Value::Bool(false))].into(),
            },
            Setting {
                name: "b".into(),
                enabled: true,
                value: Some(100.),
                key: None,
                options: Default::default(),
            },
        ];
        assert!(m.apply_settings(&invalid).is_err());
        assert_eq!(m.settings(), before);
        let mut bad_type = invalid[0].clone();
        bad_type
            .options
            .insert("flag".into(), settings::Value::Number(1.));
        assert!(m.apply_settings(&[bad_type]).is_err());
        assert_eq!(m.settings(), before);
        let failing = [
            invalid[0].clone(),
            Setting {
                value: Some(6.),
                ..invalid[1].clone()
            },
        ];
        assert!(m
            .apply_settings(&failing)
            .unwrap_err()
            .contains("以前の設定へ戻しました"));
        assert_eq!(m.settings(), before);
        let good = [
            invalid[0].clone(),
            Setting {
                name: "future_module".into(),
                enabled: true,
                value: None,
                key: None,
                options: Default::default(),
            },
        ];
        assert_eq!(m.apply_settings(&good).unwrap(), ["future_module"]);
        assert!(!m.view(0).enabled);
        assert_eq!(m.view(0).param.unwrap().value, 5.);
        assert_eq!(m.view(0).key, input::parse_key("F9").unwrap());
        assert_eq!(
            m.settings()[0].options["flag"],
            settings::Value::Bool(false)
        );
        let same = m.settings();
        assert!(m
            .apply_settings(&[good[0].clone(), good[0].clone()])
            .is_err());
        assert_eq!(m.settings(), same);
    }

    /// Builds the real module list outside the game (so the game hook is
    /// unavailable) and drives it the way the main loop and overlay do.
    #[test]
    fn readouts_register_report_and_fail_closed_outside_the_game() {
        // Building the modules replaces the process-wide custom block list.
        let _serial = game::blocks::CUSTOM_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("mio_client_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = Config {
            dir: dir.clone(),
            ..Config::default()
        };
        config.fps.enabled = true;
        config.clock.enabled = true;
        config.patch.clear();
        let mut modules = Modules::new(&config);

        // Exercise the same typed setters used by the overlay on real modules.
        let baseline = modules.settings();
        for (name, id, value) in [
            ("autosprint", "jump", settings::Value::Bool(false)),
            ("debughud", "target", settings::Value::Bool(false)),
            ("debughud", "system", settings::Value::Bool(false)),
            ("debughud", "background", settings::Value::Bool(false)),
            ("debughud", "font_scale", settings::Value::Number(1.75)),
            ("debughud", "opacity", settings::Value::Number(0.65)),
            ("debughud", "x", settings::Value::Number(40.)),
            ("debughud", "right_x", settings::Value::Number(-40.)),
            ("safewalk", "margin", settings::Value::Number(0.25)),
            ("durabilityguard", "reserve", settings::Value::Number(2030.)),
            (
                "durabilityguard",
                "empty_slot",
                settings::Value::Bool(false),
            ),
            ("autowalk", "pause_back", settings::Value::Bool(false)),
            ("autosneak", "forward_only", settings::Value::Bool(true)),
            ("autojump", "forward_only", settings::Value::Bool(false)),
            ("autoswim", "forward_only", settings::Value::Bool(true)),
            ("inventorymove", "sneak", settings::Value::Bool(true)),
            ("inventorymove", "automatic", settings::Value::Bool(false)),
            (
                "inventorymove",
                "forward_key",
                settings::Value::Key("0x26".into()),
            ),
            ("killaura", "cps", settings::Value::Number(13.)),
            ("killaura", "players", settings::Value::Bool(false)),
            ("esp", "tracers", settings::Value::Bool(true)),
            ("esp", "max_distance", settings::Value::Number(64.)),
            ("esp", "passive", settings::Value::Bool(false)),
            ("esp", "player_r", settings::Value::Number(200.)),
            ("armorhud", "percent", settings::Value::Bool(false)),
            ("inventoryhud", "hotbar_only", settings::Value::Bool(true)),
            ("inventoryhud", "max_rows", settings::Value::Number(6.)),
            ("inventoryhud", "grid", settings::Value::Bool(false)),
            ("inventoryhud", "slot_size", settings::Value::Number(48.)),
            ("inventoryhud", "show_counts", settings::Value::Bool(false)),
            (
                "inventoryhud",
                "show_durability",
                settings::Value::Bool(false),
            ),
            (
                "sessionstats",
                "reset_key",
                settings::Value::Key("F12".into()),
            ),
            ("sessionstats", "x", settings::Value::Number(-300.)),
            ("mininghud", "coordinates", settings::Value::Bool(false)),
            ("mininghud", "hide_idle", settings::Value::Bool(false)),
            ("mininghud", "x", settings::Value::Number(-200.)),
            (
                "itemcounter",
                "targets",
                settings::Value::Text {
                    text: "arrow bread".into(),
                },
            ),
            ("itemcounter", "held", settings::Value::Bool(false)),
            ("itemcounter", "hotbar_only", settings::Value::Bool(true)),
            ("itemcounter", "show_zero", settings::Value::Bool(false)),
            ("itemcounter", "y", settings::Value::Number(500.)),
            ("killaura", "fov", settings::Value::Number(90.)),
            ("killaura", "max_targets", settings::Value::Number(4.)),
            ("killaura", "hold_only", settings::Value::Bool(true)),
            ("killaura", "line_of_sight", settings::Value::Bool(true)),
            ("triggerbot", "cps", settings::Value::Number(6.)),
            ("triggerbot", "players", settings::Value::Bool(true)),
            ("triggerbot", "mobs", settings::Value::Bool(false)),
            ("minimap", "chunk_grid", settings::Value::Bool(true)),
            ("minimap", "centered", settings::Value::Bool(false)),
            ("minimap", "coordinates", settings::Value::Bool(false)),
            ("minimap", "height_limit", settings::Value::Number(32.)),
            ("trajectories", "line_width", settings::Value::Number(3.)),
            ("trajectories", "hit_box", settings::Value::Bool(false)),
            ("trajectories", "hit_info", settings::Value::Bool(false)),
            ("trajectories", "entities", settings::Value::Bool(false)),
            ("trajectories", "ticks", settings::Value::Number(80.)),
            ("trajectories", "update_hz", settings::Value::Number(30.)),
            ("autoweapon", "reserve", settings::Value::Number(15.)),
            ("autoweapon", "axes", settings::Value::Bool(true)),
            (
                "friends",
                "names",
                settings::Value::Text {
                    text: "Steve\n日本 太郎".into(),
                },
            ),
            ("deathpoint", "keep", settings::Value::Number(3.)),
            ("autotool", "min_durability", settings::Value::Number(15.)),
            ("autoeat", "hunger", settings::Value::Number(14.)),
            ("autoeat", "restore", settings::Value::Bool(false)),
            ("blockesp", "ores", settings::Value::Bool(false)),
            ("radar", "size", settings::Value::Number(250.)),
            ("radar", "x", settings::Value::Number(-80.)),
            ("targethud", "y", settings::Value::Number(-240.)),
            ("targethud", "health", settings::Value::Bool(false)),
            ("vitals", "food", settings::Value::Bool(false)),
            ("vitals", "x", settings::Value::Number(-20.)),
            ("nametags", "mobs", settings::Value::Bool(false)),
            ("nametags", "max_labels", settings::Value::Number(16.)),
            ("nametags", "names", settings::Value::Bool(false)),
            ("effects", "duration", settings::Value::Bool(false)),
            ("effects", "warn_expiring", settings::Value::Bool(true)),
            ("effects", "warn_seconds", settings::Value::Number(15.)),
            ("effects", "max_rows", settings::Value::Number(6.)),
            ("air", "ticks", settings::Value::Bool(true)),
            ("air", "only_underwater", settings::Value::Bool(false)),
            ("crosshair", "gap", settings::Value::Number(8.)),
            ("crosshair", "green", settings::Value::Number(120.)),
            ("crosshair", "dynamic", settings::Value::Bool(true)),
            ("lowhealth", "absorption", settings::Value::Bool(true)),
            ("lowhealth", "cooldown", settings::Value::Number(30.)),
            ("playeralert", "notification", settings::Value::Bool(false)),
            ("playeralert", "x", settings::Value::Number(20.)),
            ("compass", "y", settings::Value::Number(30.)),
            ("compass", "x", settings::Value::Number(-100.)),
            ("hud", "x", settings::Value::Number(-20.)),
            ("hud", "y", settings::Value::Number(30.)),
            ("keystrokes", "x", settings::Value::Number(40.)),
            ("zoom", "hold_key", settings::Value::Key("MOUSE4".into())),
            ("zoom", "sensitivity", settings::Value::Bool(true)),
            ("zoom", "sensitivity_scale", settings::Value::Number(0.1)),
            ("coordinates", "native_hud", settings::Value::Bool(true)),
            ("coordinates", "nether", settings::Value::Bool(true)),
            ("waila", "details", settings::Value::Bool(false)),
            ("blockoutline", "thickness", settings::Value::Number(3.)),
            (
                "blockoutline",
                "color",
                settings::Value::Text {
                    text: "12FF34AB".into(),
                },
            ),
            ("playerlist", "hold_key", settings::Value::Key("F8".into())),
            ("playerlist", "rows", settings::Value::Number(12.)),
            ("playerlist", "distance", settings::Value::Bool(false)),
            ("playerlist", "health", settings::Value::Bool(false)),
            ("playerlist", "friends", settings::Value::Bool(false)),
            ("waila", "fluids", settings::Value::Bool(true)),
            ("waila", "y", settings::Value::Number(-40.)),
            ("waila", "x", settings::Value::Number(120.)),
            ("waypoints", "tracers", settings::Value::Bool(true)),
            ("waypoints", "compass", settings::Value::Bool(false)),
            ("blockesp", "tracers", settings::Value::Bool(true)),
        ] {
            let i = modules.find(name).unwrap();
            modules.set_option_at(i, id, &value).unwrap();
            assert_eq!(modules.setting_at(i).options[id], value, "{name}.{id}");
        }
        friends::command(&["add", "日本", "花子"], &mut modules);
        let friends_index = modules.find("friends").unwrap();
        let names = modules.setting_at(friends_index).options["names"].clone();
        assert_eq!(
            names,
            settings::Value::Text {
                text: "Steve\n日本 太郎\n日本 花子".into()
            }
        );
        assert!(modules
            .set_option_at(
                friends_index,
                "names",
                &settings::Value::Text {
                    text: "bad\tname".into()
                }
            )
            .is_err());
        assert_eq!(modules.setting_at(friends_index).options["names"], names);
        friends::command(&["del", "steve"], &mut modules);
        assert_eq!(
            modules.setting_at(friends_index).options["names"],
            settings::Value::Text {
                text: "日本 太郎\n日本 花子".into()
            }
        );
        friends::command(&["clear"], &mut modules);
        let index = modules.find("playerlist").unwrap();
        assert_eq!(modules.find("TabList"), Some(index));
        let mut old_player_list = modules.setting_at(index);
        old_player_list.name = "tablist".into();
        old_player_list
            .options
            .retain(|name, _| matches!(name.as_str(), "hold_key" | "rows"));
        assert!(modules
            .apply_settings(&[old_player_list.clone()])
            .unwrap()
            .is_empty());
        assert_eq!(modules.setting_at(index).name, "playerlist");
        assert!(modules
            .apply_settings(&[old_player_list, modules.setting_at(index)])
            .is_err());
        modules.apply_settings(&baseline).unwrap();
        assert_eq!(modules.settings(), baseline);

        let names: Vec<String> = (0..modules.len()).map(|i| modules.view(i).name).collect();
        for name in [
            "hud",
            "autowalk",
            "autosneak",
            "autojump",
            "autoswim",
            "inventorymove",
            "safewalk",
            "durabilityguard",
            "fps",
            "cps",
            "speed",
            "direction",
            "clock",
            "memory",
            "entities",
            "server",
            "keystrokes",
            "waypoints",
            "chunkborder",
            "zoom",
            "fullbright",
            "compass",
            "killaura",
            "radar",
            "targethud",
            "vitals",
            "nametags",
            "crosshair",
            "lowhealth",
            "playeralert",
            "effects",
            "air",
            "armorhud",
            "inventoryhud",
            "sessionstats",
            "deathpoint",
            "autotool",
            "autoeat",
            "fov",
            "waila",
            "breadcrumbs",
            "dimension",
            "stopwatch",
            "chatlog",
        ] {
            assert_eq!(
                names.iter().filter(|n| *n == name).count(),
                1,
                "{name}: {names:?}"
            );
        }
        let view = |modules: &Modules, name: &str| {
            modules.view(names.iter().position(|n| n == name).unwrap())
        };

        // The config's initial state reached the right modules.
        assert!(view(&modules, "fps").enabled && view(&modules, "clock").enabled);
        assert!(!view(&modules, "cps").enabled && !view(&modules, "esp").enabled);

        for name in [
            "cps",
            "speed",
            "direction",
            "memory",
            "server",
            "keystrokes",
        ] {
            modules.toggle(name);
            assert!(view(&modules, name).enabled, "{name}");
        }
        hud::frame(144.4);
        modules.tick(&Tick {
            active: false,
            paused: Some("ゲームが非アクティブ"),
            focused: false,
            now_ms: 1,
        });
        assert_eq!(view(&modules, "fps").status, "144");
        assert_eq!(view(&modules, "cps").status, "0 | 0");
        assert_eq!(view(&modules, "speed").status, "（ワールド未接続）");
        assert_eq!(view(&modules, "direction").status, "（ワールド未接続）");
        assert_eq!(view(&modules, "server").status, "(none)");
        let clock = view(&modules, "clock").status;
        assert!(clock.len() == 8 && clock.as_bytes()[2] == b':', "{clock}");
        let memory = view(&modules, "memory").status;
        let megabytes: u64 = memory.strip_suffix(" MB").unwrap().parse().unwrap();
        assert!(megabytes > 0, "{memory}");
        // A disabled readout computes nothing.
        modules.toggle("memory");
        assert_eq!(view(&modules, "memory").status, "");

        // Anything needing the game's data refuses to turn on here.
        for name in [
            "effects",
            "air",
            "crosshair",
            "lowhealth",
            "playeralert",
            "vitals",
            "nametags",
            "entities",
            "waypoints",
            "chunkborder",
            "esp",
            "zoom",
            "fullbright",
            "killaura",
            "fov",
            "waila",
            "breadcrumbs",
            "blockesp",
            "chatlog",
            "debughud",
            "skinstealer",
        ] {
            modules.toggle(name);
            assert!(!view(&modules, name).enabled, "{name}");
        }
        // Readouts that need no game data do turn on, and say so honestly.
        for name in ["dimension", "stopwatch"] {
            modules.toggle(name);
            assert!(view(&modules, name).enabled, "{name}");
        }
        assert_eq!(view(&modules, "dimension").status, "（ワールド未接続）");
        assert!(view(&modules, "stopwatch").status.starts_with("0:00."));
        modules.set_param("fov", 500.0);
        modules.set_param("waila", 0.0);
        modules.set_param("breadcrumbs", 10.0);
        modules.set_param("crosshair", 100.0);
        modules.set_param("lowhealth", 1.0);
        modules.set_param("playeralert", 1000.0);
        let value = |name| view(&modules, name).param.unwrap().value;
        assert_eq!(
            (value("crosshair"), value("lowhealth"), value("playeralert")),
            (32., 5., 128.)
        );
        assert_eq!(
            (value("fov"), value("waila"), value("breadcrumbs")),
            (170.0, 1.0, 50.0)
        );
        // The view hooks say why instead of showing a setting they cannot apply.
        assert!(view(&modules, "zoom").status.starts_with("(getFov:"));
        assert!(view(&modules, "fullbright")
            .status
            .starts_with("(gamma option name:"));
        // Their settings still clamp.
        modules.set_param("zoom", 1000.0);
        modules.set_param("fullbright", -5.0);
        let param = |name| view(&modules, name).param.unwrap().value;
        assert_eq!((param("zoom"), param("fullbright")), (30.0, 1.0));

        // The waypoint list itself works without the game, and is saved.
        waypoints::command(&["add", "home", "10", "64", "-20"]);
        waypoints::command(&["add", "mine", "0", "12", "0"]);
        waypoints::command(&["del", "mine"]);
        waypoints::command(&["add", "here"]); // not in a world: refused
        let saved = std::fs::read_to_string(dir.join(waypoints::FILE_NAME)).unwrap();
        assert!(
            saved.contains("name = \"home\"") && saved.contains("z = -20"),
            "{saved}"
        );
        assert!(
            !saved.contains("mine") && !saved.contains("here"),
            "{saved}"
        );
        // A fresh start reads the file back.
        drop(waypoints::Waypoints::new(&config.waypoints, &dir));
        waypoints::command(&["clear"]);
        let cleared = std::fs::read_to_string(dir.join(waypoints::FILE_NAME)).unwrap();
        assert!(!cleared.contains("home"), "{cleared}");

        modules.disable_all();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
