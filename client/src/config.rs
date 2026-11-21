//! `Mio_Client.toml`, loaded from the directory containing the DLL.

use std::path::Path;

use serde::Deserialize;
pub(crate) mod menu_key;

use crate::log;

pub const FILE_NAME: &str = "Mio_Client.toml";
const DEFAULT_TEXT: &str = include_str!("../default_config.toml");

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub gui: Gui,
    pub auto_sprint: AutoSprint,
    pub auto_walk: AutoWalk,
    pub auto_sneak: AutoSneak,
    pub auto_jump: AutoJump,
    pub auto_swim: AutoSwim,
    pub inventorymove: InventoryMove,
    pub auto_clicker: AutoClicker,
    pub killaura: KillAura,
    pub triggerbot: TriggerBot,
    pub autoweapon: AutoWeapon,
    pub friends: Friends,
    pub timer: Timer,
    pub coordinates: Coordinates,
    pub esp: Esp,
    pub blockesp: BlockEsp,
    pub blockoutline: BlockOutline,
    #[serde(alias = "tablist")]
    pub playerlist: PlayerList,
    pub fly: Fly,
    pub zoom: Zoom,
    pub fov: Fov,
    pub fullbright: Fullbright,
    pub waila: Waila,
    pub mininghud: MiningHud,
    pub itemcounter: ItemCounter,
    pub breadcrumbs: Breadcrumbs,
    pub fps: Toggle,
    pub cps: Toggle,
    pub speed: Toggle,
    pub direction: Toggle,
    pub dimension: Toggle,
    pub stopwatch: Toggle,
    pub compass: Compass,
    pub radar: Radar,
    pub targethud: TargetHud,
    pub combathud: CombatHud,
    pub hitmarker: HitMarker,
    pub vitals: Vitals,
    pub nametags: NameTags,
    pub crosshair: Crosshair,
    pub lowhealth: LowHealth,
    pub playeralert: PlayerAlert,
    pub effects: Effects,
    pub air: Air,
    pub armorhud: ItemHud,
    #[serde(default = "default_inventory_hud")]
    pub inventoryhud: ItemHud,
    pub sessionstats: SessionStats,
    pub deathpoint: Deathpoint,
    pub autotool: ItemAssist,
    pub autoeat: ItemAssist,
    pub autofish: AutoFish,
    pub durabilityalert: DurabilityAlert,
    pub durabilityguard: DurabilityGuard,
    pub safewalk: SafeWalk,
    pub clock: Toggle,
    pub memory: Toggle,
    pub entities: Toggle,
    pub server: Toggle,
    pub ping: Ping,
    pub keystrokes: Keystrokes,
    pub waypoints: Waypoints,
    pub chunkborder: Toggle,
    pub chatlog: ChatLog,
    pub chatrepeat: ChatRepeat,
    pub chathud: ChatHud,
    pub joinnotify: JoinNotify,
    pub minimap: Minimap,
    pub trajectories: Trajectories,
    pub frametime: FrameTime,
    pub performanceprofiler: PerformanceProfiler,
    pub debughud: DebugHud,
    pub skinstealer: Toggle,
    pub worldmap: WorldMap,
    pub mobcount: MobCount,
    pub worldinfo: HudPosition,
    pub lightoverlay: LightOverlay,
    pub freelook: FreeLook,
    pub freecam: Freecam,
    pub offsets: Offsets,
    pub patch: Vec<Patch>,
    /// The folder the config was loaded from (the DLL's), for other saved files.
    #[serde(skip)]
    pub dir: std::path::PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SafeWalk {
    pub key: String,
    pub enabled: bool,
    pub margin: f64,
}
impl Default for SafeWalk {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            margin: 0.12,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DurabilityGuard {
    pub key: String,
    pub enabled: bool,
    pub reserve: i32,
    pub empty_slot: bool,
}
impl Default for DurabilityGuard {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            reserve: 10,
            empty_slot: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AutoFish {
    pub key: String,
    pub enabled: bool,
    pub recast: bool,
    pub delay: f64,
    pub reserve: i32,
    pub clear_view: bool,
}
impl Default for AutoFish {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            recast: true,
            delay: 0.8,
            reserve: 5,
            clear_view: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DurabilityAlert {
    pub key: String,
    pub enabled: bool,
    pub threshold: f64,
    pub hotbar: bool,
    pub armor: bool,
    pub cooldown: f64,
}
impl Default for DurabilityAlert {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            threshold: 10.,
            hotbar: true,
            armor: true,
            cooldown: 5.,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ItemHud {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub percent: bool,
    pub hotbar_only: bool,
    pub max_rows: i32,
    /// Inventory slot grid; false preserves the grouped text list.
    pub grid: bool,
    pub slot_size: i32,
    pub show_counts: bool,
    pub show_durability: bool,
}
impl Default for ItemHud {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: 240,
            percent: true,
            hotbar_only: false,
            max_rows: 12,
            grid: true,
            slot_size: 40,
            show_counts: true,
            show_durability: true,
        }
    }
}
fn default_inventory_hud() -> ItemHud {
    ItemHud {
        x: -20,
        y: 520,
        ..ItemHud::default()
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SessionStats {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub reset_key: String,
}
impl Default for SessionStats {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: 150,
            reset_key: "NONE".into(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Deathpoint {
    pub key: String,
    pub enabled: bool,
    pub keep: i32,
}
impl Default for Deathpoint {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            keep: 5,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ItemAssist {
    pub key: String,
    pub enabled: bool,
    pub hunger: i32,
    pub min_durability: i32,
    pub restore: bool,
}
impl Default for ItemAssist {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            hunger: 16,
            min_durability: 5,
            restore: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Effects {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub duration: bool,
    pub amplifier: bool,
    pub max_rows: i32,
    pub warn_expiring: bool,
    pub warn_seconds: f64,
}
impl Default for Effects {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: -20,
            y: 350,
            duration: true,
            amplifier: true,
            max_rows: 12,
            warn_expiring: false,
            warn_seconds: 10.,
        }
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Air {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub only_underwater: bool,
    pub ticks: bool,
}
impl Default for Air {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: -260,
            only_underwater: true,
            ticks: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Crosshair {
    pub key: String,
    pub enabled: bool,
    pub size: f64,
    pub gap: f64,
    pub thickness: f64,
    pub dot: bool,
    pub outline: bool,
    pub t_style: bool,
    pub dynamic: bool,
    pub color: [u8; 3],
    pub opacity: f64,
}
impl Default for Crosshair {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            size: 6.,
            gap: 4.,
            thickness: 2.,
            dot: true,
            outline: true,
            t_style: false,
            dynamic: false,
            color: [255, 255, 255],
            opacity: 0.9,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct LowHealth {
    pub key: String,
    pub enabled: bool,
    pub threshold: f64,
    pub cooldown: f64,
    pub notification: bool,
    pub border: bool,
    pub absorption: bool,
}
impl Default for LowHealth {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            threshold: 30.,
            cooldown: 10.,
            notification: true,
            border: true,
            absorption: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct PlayerAlert {
    pub key: String,
    pub enabled: bool,
    pub range: f64,
    pub cooldown: f64,
    pub notification: bool,
    pub hud: bool,
    pub x: i32,
    pub y: i32,
}
impl Default for PlayerAlert {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            range: 32.,
            cooldown: 5.,
            notification: true,
            hud: true,
            x: -20,
            y: 290,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct BlockEsp {
    pub key: String,
    pub enabled: bool,
    pub radius: i32,
    pub ores: bool,
    pub chests: bool,
    /// More block names to outline, e.g. "mob_spawner" or "*shulker_box".
    pub extra: Vec<String>,
    /// Draw a line from the bottom of the screen to the nearest matches.
    pub tracers: bool,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Waypoints {
    pub key: String,
    pub enabled: bool,
    /// Draw a line from the bottom of the screen to each waypoint.
    pub tracers: bool,
    /// Mark the waypoints' directions on the compass ribbon.
    pub compass: bool,
}
impl Default for Waypoints {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            tracers: false,
            compass: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ChatLog {
    pub key: String,
    pub enabled: bool,
    /// Received messages containing one of these words raise a notification.
    pub highlight: Vec<String>,
    pub notifications: bool,
    pub file: bool,
}
impl Default for ChatLog {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            highlight: Vec::new(),
            notifications: true,
            file: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct BlockOutline {
    pub enabled: bool,
    pub key: String,
    pub color: [u8; 4],
    pub thickness: f64,
}

impl Default for BlockOutline {
    fn default() -> Self {
        Self {
            enabled: false,
            key: "NONE".into(),
            color: [109, 220, 255, 255],
            thickness: 2.,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PlayerList {
    pub enabled: bool,
    pub hold_key: String,
    pub rows: i32,
    pub distance: bool,
    pub health: bool,
    pub friends: bool,
}

impl Default for PlayerList {
    fn default() -> Self {
        Self {
            enabled: false,
            hold_key: "TAB".into(),
            rows: 18,
            distance: true,
            health: true,
            friends: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Waila {
    pub key: String,
    pub enabled: bool,
    /// How far the look-at ray goes, in blocks.
    pub reach: f64,
    /// Also show the target's coordinates and distance.
    pub details: bool,
    /// Stop at water and lava instead of looking through them.
    pub fluids: bool,
    pub entities: bool,
    pub health: bool,
    pub identifiers: bool,
    /// Pixels below the crosshair.
    pub y: i32,
    pub x: i32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct MiningHud {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub coordinates: bool,
    pub hide_idle: bool,
}
impl Default for MiningHud {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: 430,
            coordinates: true,
            hide_idle: true,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ItemCounter {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub targets: Vec<String>,
    pub held: bool,
    pub hotbar_only: bool,
    pub show_zero: bool,
}
impl Default for ItemCounter {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: 550,
            targets: vec!["arrow".into(), "torch".into(), "bread".into()],
            held: true,
            hotbar_only: false,
            show_zero: true,
        }
    }
}
impl Default for Waila {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            reach: 6.,
            details: true,
            fluids: false,
            entities: true,
            health: true,
            identifiers: false,
            y: 24,
            x: 0,
        }
    }
}

#[cfg(test)]
mod look_info_tests {
    use super::*;
    #[test]
    fn old_map_and_flight_sections_gain_settings_without_losing_existing_values() {
        let old:Config=toml::from_str("[minimap]\nkey='M'\nenabled=true\nradius=48\nx=-35\ny=90\n[trajectories]\nkey='T'\nenabled=true\n").unwrap();
        assert_eq!(
            (
                old.minimap.key.as_str(),
                old.minimap.radius,
                old.minimap.x,
                old.minimap.y
            ),
            ("M", 48, -35, 90)
        );
        assert!(
            old.minimap.enabled
                && old.minimap.entities
                && old.minimap.centered
                && old.minimap.coordinates
                && !old.minimap.chunk_grid
        );
        assert!(
            old.trajectories.enabled
                && old.trajectories.hit_box
                && old.trajectories.hit_info
                && old.trajectories.entities
        );
        let default: Config = toml::from_str(DEFAULT_TEXT).unwrap();
        assert_eq!(
            (
                old.trajectories.line_width,
                old.trajectories.ticks,
                old.trajectories.update_hz
            ),
            (
                default.trajectories.line_width,
                default.trajectories.ticks,
                default.trajectories.update_hz
            )
        );
        assert_eq!(default.minimap.height_limit, 0.);
    }
    #[test]
    fn combat_additions_preserve_legacy_aura_defaults_and_trigger_is_opt_in() {
        let old: KillAura = toml::from_str("range=4.0\ncps=10.0\nplayers=false").unwrap();
        assert_eq!(old.max_targets, 8);
        assert_eq!(
            toml::from_str::<KillAura>("max_targets=1")
                .unwrap()
                .max_targets,
            1
        );
        assert_eq!(
            (
                old.range,
                old.cps,
                old.players,
                old.fov,
                old.line_of_sight,
                old.hold_only
            ),
            (4., 10., false, 360., false, false)
        );
        let built = Config::default();
        let shipped: Config = toml::from_str(DEFAULT_TEXT).unwrap();
        assert!(!built.triggerbot.enabled && !shipped.triggerbot.enabled);
        assert_eq!(shipped.killaura.max_targets, built.killaura.max_targets);
        assert!(!built.autoweapon.enabled && !shipped.autoweapon.enabled);
        assert!(!built.friends.enabled && !shipped.friends.enabled);
        assert_eq!(
            (built.autoweapon.reserve, built.autoweapon.axes),
            (shipped.autoweapon.reserve, shipped.autoweapon.axes)
        );
        assert_eq!(built.friends.names, shipped.friends.names);
        assert_eq!(
            (
                built.triggerbot.range,
                built.triggerbot.cps,
                built.triggerbot.players,
                built.triggerbot.mobs
            ),
            (
                shipped.triggerbot.range,
                shipped.triggerbot.cps,
                shipped.triggerbot.players,
                shipped.triggerbot.mobs
            )
        );
        assert_eq!(
            (
                shipped.killaura.fov,
                shipped.killaura.line_of_sight,
                shipped.killaura.hold_only
            ),
            (360., false, false)
        );
    }
    #[test]
    fn legacy_block_hud_config_keeps_positions_and_adds_optional_entity_details() {
        let legacy: Waila =
            toml::from_str("reach=12.0\nx=-80\ny=32\ndetails=false\nfluids=true").unwrap();
        assert_eq!(
            (
                legacy.reach,
                legacy.x,
                legacy.y,
                legacy.details,
                legacy.fluids
            ),
            (12., -80, 32, false, true)
        );
        assert!(legacy.entities && legacy.health && !legacy.identifiers);
        let explicit: Waila =
            toml::from_str("entities=false\nhealth=false\nidentifiers=true").unwrap();
        assert!(!explicit.entities && !explicit.health && explicit.identifiers);
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Breadcrumbs {
    pub key: String,
    pub enabled: bool,
    /// How many points of the path are kept.
    pub points: f64,
}
impl Default for Breadcrumbs {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            points: 600.,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Fov {
    pub key: String,
    pub enabled: bool,
    /// The field of view to draw the world with (30-170).
    pub degrees: f64,
}
impl Default for Fov {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            degrees: 110.,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Radar {
    pub key: String,
    pub enabled: bool,
    pub range: f64,
    pub size: f32,
    pub x: i32,
    pub y: i32,
}
impl Default for Radar {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: true,
            range: 48.,
            size: 180.,
            x: -20,
            y: 90,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TargetHud {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub health: bool,
}
impl Default for TargetHud {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: true,
            x: -20,
            y: -160,
            health: true,
        }
    }
}

/// The combat summary panel.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct CombatHud {
    pub key: String,
    pub enabled: bool,
    pub threats: bool,
    pub x: i32,
    pub y: i32,
}
impl Default for CombatHud {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            threats: true,
            x: -20,
            y: 120,
        }
    }
}
/// The X over the crosshair when an attack lands.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct HitMarker {
    pub key: String,
    pub enabled: bool,
    pub size: f32,
    pub thickness: f32,
}
impl Default for HitMarker {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            size: 10.,
            thickness: 2.,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Vitals {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub food: bool,
    pub experience: bool,
}
impl Default for Vitals {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: -180,
            food: true,
            experience: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct NameTags {
    pub key: String,
    pub enabled: bool,
    pub range: f64,
    pub players: bool,
    pub mobs: bool,
    pub others: bool,
    pub health: bool,
    pub names: bool,
    pub distance: bool,
    pub max_labels: i32,
}
impl Default for NameTags {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            range: 48.,
            players: true,
            mobs: true,
            others: false,
            health: true,
            names: true,
            distance: true,
            max_labels: 32,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Compass {
    pub key: String,
    pub enabled: bool,
    pub width: f64,
    pub y: i32,
    /// Horizontal offset from screen center.
    pub x: i32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct KillAura {
    pub key: String,
    pub enabled: bool,
    pub range: f64,
    pub cps: f64,
    pub max_targets: usize,
    pub players: bool,
    pub mobs: bool,
    pub fov: f64,
    pub line_of_sight: bool,
    pub hold_only: bool,
}
impl Default for KillAura {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            range: 3.,
            cps: 8.,
            max_targets: 8,
            players: true,
            mobs: true,
            fov: 360.,
            line_of_sight: false,
            hold_only: false,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TriggerBot {
    pub key: String,
    pub enabled: bool,
    pub range: f64,
    pub cps: f64,
    pub players: bool,
    pub mobs: bool,
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AutoWeapon {
    pub key: String,
    pub enabled: bool,
    pub reserve: i32,
    pub axes: bool,
}
impl Default for AutoWeapon {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            reserve: 5,
            axes: false,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Friends {
    pub key: String,
    pub enabled: bool,
    pub names: Vec<String>,
}
impl Default for Friends {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            names: Vec::new(),
        }
    }
}
impl Default for TriggerBot {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            range: 3.,
            cps: 8.,
            players: false,
            mobs: true,
        }
    }
}
impl Default for Compass {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: true,
            width: 400.,
            y: 10,
            x: 0,
        }
    }
}
impl Default for BlockEsp {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            radius: 24,
            ores: true,
            chests: true,
            extra: Vec::new(),
            tracers: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Zoom {
    pub enabled: bool,
    /// Zooms in while this key is held.
    pub hold_key: String,
    /// Magnification while zooming.
    pub factor: f64,
    /// Opt-in scaling of the mouse sensitivity option while holding zoom.
    pub sensitivity: bool,
    pub sensitivity_scale: f64,
    /// How long a zoom in or out takes, in ms (0 = instant).
    pub smooth_ms: f64,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            enabled: false,
            hold_key: "C".into(),
            factor: 4.0,
            sensitivity: false,
            sensitivity_scale: 0.25,
            smooth_ms: 200.,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Fullbright {
    pub key: String,
    pub enabled: bool,
    /// The gamma reported to the renderer (the game's own range is 0-1).
    pub gamma: f64,
}

impl Default for Fullbright {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            gamma: 10.0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Minimap {
    pub key: String,
    pub enabled: bool,
    /// Blocks shown from the player to each edge (8-64).
    pub radius: i32,
    pub size: f32,
    pub entities: bool,
    pub chunk_grid: bool,
    pub coordinates: bool,
    pub centered: bool,
    pub height_limit: f32,
    /// Mark saved places (waypoints, death points) on the map.
    pub waypoints: bool,
    /// Tint slime chunks (Overworld).
    pub slime_chunks: bool,
    pub x: i32,
    pub y: i32,
}
impl Default for Minimap {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            radius: 32,
            size: 200.,
            entities: true,
            chunk_grid: false,
            coordinates: true,
            centered: true,
            height_limit: 0.,
            waypoints: true,
            slime_chunks: true,
            x: -20,
            y: 290,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Trajectories {
    pub key: String,
    pub enabled: bool,
    pub line_width: f32,
    pub hit_box: bool,
    pub hit_info: bool,
    pub entities: bool,
    pub ticks: u32,
    pub update_hz: u32,
}
impl Default for Trajectories {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            line_width: 2.,
            hit_box: true,
            hit_info: true,
            entities: true,
            ticks: 200,
            update_hz: 60,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct FrameTime {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub line: bool,
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PerformanceProfiler {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub rows: usize,
    pub update_ms: u64,
}
impl Default for PerformanceProfiler {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: 320,
            rows: 12,
            update_ms: 1000,
        }
    }
}
impl Default for FrameTime {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 20,
            y: -310,
            line: true,
        }
    }
}
/// The latest chat messages in a panel.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ChatHud {
    pub key: String,
    pub enabled: bool,
    /// Messages shown (1-20), newest last.
    pub lines: usize,
    /// Panel width in pixels; longer messages wrap.
    pub width: f32,
    /// Show the time each message arrived.
    pub time: bool,
    /// Seconds a message stays shown; 0 keeps the latest `lines` shown.
    pub fade: f32,
    pub x: i32,
    pub y: i32,
}
impl Default for ChatHud {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            lines: 8,
            width: 420.,
            time: true,
            fade: 0.,
            x: -20,
            y: -200,
        }
    }
}
/// Notices when players join or leave (from the game's system messages).
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct JoinNotify {
    pub key: String,
    pub enabled: bool,
    pub notifications: bool,
}
impl Default for JoinNotify {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            notifications: true,
        }
    }
}
/// The full-screen map of remembered terrain.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct WorldMap {
    pub key: String,
    /// Ignored at start: the map opens only from the world view.
    pub enabled: bool,
    pub waypoints: bool,
    pub slime_chunks: bool,
    pub auto_save: bool,
    pub save_interval: i32,
}
impl Default for WorldMap {
    fn default() -> Self {
        Self {
            key: "M".into(),
            enabled: false,
            waypoints: true,
            slime_chunks: true,
            auto_save: true,
            save_interval: 30,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct MobCount {
    pub key: String,
    pub enabled: bool,
    pub range: f64,
    pub rows: i32,
    pub x: i32,
    pub y: i32,
}
impl Default for MobCount {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            range: 48.,
            rows: 10,
            x: -240,
            y: 200,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DebugHud {
    pub key: String,
    pub enabled: bool,
    pub target: bool,
    pub system: bool,
    pub background: bool,
    pub font_scale: f64,
    pub opacity: f64,
    pub x: i32,
    pub y: i32,
    pub right_x: i32,
    pub right_y: i32,
}
impl Default for DebugHud {
    fn default() -> Self {
        Self {
            key: "F3".into(),
            enabled: false,
            target: true,
            system: true,
            background: true,
            font_scale: 2.5,
            opacity: 0.45,
            x: 10,
            y: 10,
            right_x: -10,
            right_y: 10,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct HudPosition {
    pub key: String,
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Ping {
    pub key: String,
    pub enabled: bool,
    pub interval_ms: u64,
    pub x: i32,
    pub y: i32,
}
impl Default for Ping {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            interval_ms: 2000,
            x: 10,
            y: -170,
        }
    }
}
impl Default for HudPosition {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: -240,
            y: 90,
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LightOverlay {
    pub key: String,
    pub enabled: bool,
    pub radius: i32,
    pub threshold: i32,
    pub labels: bool,
    pub filled: bool,
    pub opacity: f64,
    pub line_width: f64,
    pub label_range: f64,
    pub max_labels: i32,
    pub legend: bool,
    pub x: i32,
    pub y: i32,
}
impl Default for LightOverlay {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            radius: 8,
            threshold: 0,
            labels: true,
            filled: true,
            opacity: 0.12,
            line_width: 1.5,
            label_range: 6.,
            max_labels: 16,
            legend: true,
            x: 20,
            y: -440,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct FreeLook {
    pub key: String,
    pub enabled: bool,
    pub hold_key: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Freecam {
    pub key: String,
    pub enabled: bool,
    pub speed: f64,
    pub boost: f64,
}
impl Default for Freecam {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            speed: 10.,
            boost: 3.,
        }
    }
}
impl Default for FreeLook {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            hold_key: "LALT".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ChatRepeat {
    pub key: String,
    pub enabled: bool,
    pub text: String,
    /// Seconds, retaining the existing config/profile unit (0.001 = 1ms).
    pub interval: f64,
    pub count: i32,
}
impl Default for ChatRepeat {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            text: String::new(),
            interval: 5.,
            count: 10,
        }
    }
}

/// A module with nothing to configure but its hotkey and initial state.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Toggle {
    pub key: String,
    pub enabled: bool,
}

impl Default for Toggle {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Keystrokes {
    pub key: String,
    pub enabled: bool,
    /// Pixels from the left/top edge; negative = from the right/bottom edge.
    pub x: i32,
    pub y: i32,
}

impl Default for Keystrokes {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            x: 10,
            y: -10,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct General {
    pub eject_key: String,
    /// Only perform automatic actions while in the world view.
    pub require_hidden_cursor: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            eject_key: "END".into(),
            require_hidden_cursor: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Gui {
    pub enabled: bool,
    pub menu_key: String,
    /// Always-on list of enabled modules in the top-left corner.
    pub show_hud: bool,
    pub hud_x: i32,
    pub hud_y: i32,
    /// Smaller rows, margins and width for the list.
    pub hud_compact: bool,
    /// Leave out features that are only waiting (not connected, no data yet).
    pub hud_hide_idle: bool,
    /// Columns of the list (1-3); 0 splits it automatically when it is long.
    pub hud_columns: u32,
    /// Fit one always-visible vertical list; keeps the horizontal preference.
    pub hud_vertical: bool,
    pub hud_width: u32,
    /// `auto`, `dx11` or `dx12`.
    pub renderer: String,
    /// Also open a separate console window (always opened if the overlay
    /// cannot start, since the in-game console needs the overlay).
    pub console_window: bool,
    /// Brief on-screen notices when a feature is switched, a bind runs, etc.
    pub notifications: bool,
    /// Japanese UI font height in pixels, applied on the next injection.
    pub font_size: f32,
}

impl Default for Gui {
    fn default() -> Self {
        Self {
            enabled: true,
            menu_key: "L".into(),
            show_hud: true,
            hud_x: 10,
            hud_y: 10,
            hud_compact: true,
            hud_hide_idle: true,
            hud_columns: 0,
            hud_vertical: true,
            hud_width: 360,
            renderer: "auto".into(),
            console_window: false,
            notifications: true,
            font_size: 16.,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AutoSprint {
    pub key: String,
    pub enabled: bool,
    /// Request vanilla jump only while grounded and moving forward with autosprint.
    pub jump: bool,
}

impl Default for AutoSprint {
    fn default() -> Self {
        Self {
            key: "F6".into(),
            enabled: false,
            jump: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct InventoryMove {
    pub key: String,
    pub enabled: bool,
    pub jump: bool,
    pub sneak: bool,
    pub automatic: bool,
    pub forward_key: String,
    pub backward_key: String,
    pub left_key: String,
    pub right_key: String,
    pub jump_key: String,
    pub sneak_key: String,
}
impl Default for InventoryMove {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: true,
            jump: true,
            sneak: false,
            automatic: true,
            forward_key: "W".into(),
            backward_key: "S".into(),
            left_key: "A".into(),
            right_key: "D".into(),
            jump_key: "SPACE".into(),
            sneak_key: "LSHIFT".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AutoWalk {
    pub key: String,
    pub enabled: bool,
    pub pause_back: bool,
}
impl Default for AutoWalk {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            pause_back: true,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AutoSneak {
    pub key: String,
    pub enabled: bool,
    pub forward_only: bool,
}
impl Default for AutoSneak {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            forward_only: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AutoJump {
    pub key: String,
    pub enabled: bool,
    pub forward_only: bool,
}
impl Default for AutoJump {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            forward_only: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AutoSwim {
    pub key: String,
    pub enabled: bool,
    pub forward_only: bool,
}
impl Default for AutoSwim {
    fn default() -> Self {
        Self {
            key: "NONE".into(),
            enabled: false,
            forward_only: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AutoClicker {
    pub key: String,
    pub enabled: bool,
    pub cps: f64,
}

impl Default for AutoClicker {
    fn default() -> Self {
        Self {
            key: "F7".into(),
            enabled: false,
            cps: 12.0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Timer {
    pub key: String,
    pub enabled: bool,
    pub speed: f64,
}

impl Default for Timer {
    fn default() -> Self {
        Self {
            key: "F8".into(),
            enabled: false,
            speed: 1.5,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Fly {
    pub key: String,
    pub enabled: bool,
    /// Blocks per second.
    pub speed: f64,
}

impl Default for Fly {
    fn default() -> Self {
        Self {
            key: "F4".into(),
            enabled: false,
            speed: 10.0,
        }
    }
}

/// Overrides for the data-layout offsets in `modules/game/layout.rs`. Anything left
/// out keeps its built-in default; offsets the game's code reveals are read
/// from the running game and are not listed here.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Offsets {
    pub actor_registry: Option<usize>,
    pub registry_pools: Option<usize>,
    pub pool_entities: Option<usize>,
    pub pool_pages: Option<usize>,
    pub pool_page_len: Option<usize>,
    pub view_matrix: Option<usize>,
    pub level_renderer: Option<usize>,
    pub renderer_player: Option<usize>,
    pub camera_origin: Option<usize>,
    pub projection_x: Option<usize>,
    pub projection_y: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Esp {
    pub key: String,
    pub enabled: bool,
    /// Also outline very small entities (dropped items, arrows, orbs).
    pub show_small: bool,
    /// Draw a line from the bottom of the screen to each entity.
    pub tracers: bool,
    /// Which kinds get a tracer line (while `tracers` is on).
    pub tracer_players: bool,
    pub tracer_hostile: bool,
    pub tracer_neutral: bool,
    pub tracer_passive: bool,
    pub tracer_other: bool,
    /// Blocks; 0 = no limit.
    pub max_distance: f32,
    pub players: bool,
    pub passive: bool,
    pub hostile: bool,
    pub other: bool,
    pub player_color: [u8; 3],
    pub passive_color: [u8; 3],
    pub hostile_color: [u8; 3],
    pub other_color: [u8; 3],
}

impl Default for Esp {
    fn default() -> Self {
        Self {
            key: "F10".into(),
            enabled: false,
            show_small: false,
            tracers: false,
            tracer_players: true,
            tracer_hostile: true,
            tracer_neutral: true,
            tracer_passive: true,
            tracer_other: false,
            max_distance: 0.0,
            players: true,
            passive: true,
            hostile: true,
            other: true,
            player_color: [64, 200, 255],
            passive_color: [80, 230, 100],
            hostile_color: [255, 64, 64],
            other_color: [200, 200, 200],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Coordinates {
    pub key: String,
    pub enabled: bool,
    /// Also force Minecraft's own coordinate display on.
    pub native_hud: bool,
    /// Also show the matching Nether (or Overworld) X Z.
    pub nether: bool,
}

impl Default for Coordinates {
    fn default() -> Self {
        Self {
            key: "F9".into(),
            enabled: true,
            native_hud: false,
            nether: false,
        }
    }
}

/// A user-defined byte patch located by signature.
#[derive(Debug, Deserialize)]
pub struct Patch {
    pub name: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub enabled: bool,
    pub signature: String,
    /// Offset from the signature match to the bytes to overwrite.
    #[serde(default)]
    pub offset: isize,
    pub bytes: String,
}

/// Loads the config, creating the default file on first run. Any error falls
/// back to defaults so a bad config never takes the game down.
/// Files from before the rename to Mio_Client, and their new names.
const RENAMED: [(&str, &str); 4] = [
    ("mcbe_client.toml", "Mio_Client.toml"),
    ("mcbe_client_binds.toml", "Mio_Client_binds.toml"),
    ("mcbe_client_waypoints.toml", "Mio_Client_waypoints.toml"),
    ("mcbe_client_windows.toml", "Mio_Client_windows.toml"),
];

/// Carries settings over from the old file names: an old file is copied to
/// its new name once, when the new one does not exist yet. The old file is
/// kept, so an older DLL still finds its settings.
fn migrate_names(dir: &Path) {
    for (old, new) in RENAMED {
        let (from, to) = (dir.join(old), dir.join(new));
        if to.exists() || !from.is_file() {
            continue;
        }
        match std::fs::copy(&from, &to) {
            Ok(_) => log!("{old} の設定を {new} へ引き継ぎました"),
            Err(e) => log!("{old} を {new} へ引き継げません: {e}"),
        }
    }
}

pub fn load(dir: &Path) -> Config {
    migrate_names(dir);
    let mut config = read(dir);
    config.dir = dir.to_path_buf();
    config
}

fn read(dir: &Path) -> Config {
    let path = dir.join(FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::write(&path, DEFAULT_TEXT) {
                Ok(()) => log!("created default config: {}", path.display()),
                Err(e) => log!("could not write default config: {e}"),
            }
            DEFAULT_TEXT.to_string()
        }
        Err(e) => {
            log!("could not read {}: {e} - using defaults", path.display());
            return Config::default();
        }
    };
    match toml::from_str(&text) {
        Ok(config) => config,
        Err(e) => match recover(&text) {
            Some((config, dropped)) => {
                log!("config error: {e}");
                log!(
                    "  ignoring [{}] (defaults used there); the other sections are kept",
                    dropped.join("], [")
                );
                config
            }
            None => {
                log!("config error, using defaults: {e}");
                Config::default()
            }
        },
    }
}

/// For a file that is valid TOML but does not fit `Config` (an invalid value
/// such as a color component of 300): keeps every top-level section that fits
/// on its own and drops the ones that do not, instead of losing the whole file
/// to one bad value. None when nothing can be kept (e.g. a syntax error).
fn recover(text: &str) -> Option<(Config, Vec<String>)> {
    let table: toml::Table = text.parse().ok()?;
    let fits = |table: toml::Table| toml::Value::Table(table).try_into::<Config>();
    let mut kept = toml::Table::new();
    let mut dropped = Vec::new();
    for (key, value) in table {
        let single = toml::Table::from_iter([(key.clone(), value.clone())]);
        if fits(single).is_ok() {
            kept.insert(key, value);
        } else {
            dropped.push(key);
        }
    }
    if dropped.is_empty() {
        return None;
    }
    fits(kept).ok().map(|config| (config, dropped))
}

#[cfg(test)]
mod tests {
    #[test]
    fn old_file_names_are_copied_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("mio_rename_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mcbe_client.toml"),
            "[gui]
menu_key = 'F6'
",
        )
        .unwrap();
        std::fs::write(dir.join("mcbe_client_binds.toml"), "old binds").unwrap();
        // A new file that already exists is never overwritten.
        std::fs::write(dir.join("Mio_Client_binds.toml"), "new binds").unwrap();
        let config = super::load(&dir);
        assert_eq!(config.gui.menu_key, "F6", "settings carried over");
        assert_eq!(
            std::fs::read_to_string(dir.join("Mio_Client_binds.toml")).unwrap(),
            "new binds"
        );
        // Old files stay for an older DLL; missing ones are not created.
        assert!(dir.join("mcbe_client.toml").exists());
        assert!(!dir.join("Mio_Client_waypoints.toml").exists());
        // Later starts change nothing (the new file now exists).
        std::fs::write(
            dir.join("mcbe_client.toml"),
            "[gui]
menu_key = 'F7'
",
        )
        .unwrap();
        assert_eq!(super::load(&dir).gui.menu_key, "F6");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn player_list_loads_the_legacy_configuration() {
        let old: super::Config =
            toml::from_str("[tablist]\nenabled=true\nhold_key='F8'\nrows=12\n").unwrap();
        let shipped: super::Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        assert!(old.playerlist.enabled);
        assert_eq!(old.playerlist.hold_key, "F8");
        assert_eq!(old.playerlist.rows, 12);
        assert!(old.playerlist.distance && old.playerlist.health && old.playerlist.friends);
        assert!(
            shipped.playerlist.distance && shipped.playerlist.health && shipped.playerlist.friends
        );
    }

    #[test]
    fn debug_hud_is_optional_and_preserves_the_shipped_f3_defaults() {
        let c: super::Config = toml::from_str("").unwrap();
        let shipped: super::Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        assert_eq!(c.debughud.key, "F3");
        assert!(
            !c.debughud.enabled && c.debughud.target && c.debughud.system && c.debughud.background
        );
        assert_eq!(
            (
                c.debughud.x,
                c.debughud.y,
                c.debughud.right_x,
                c.debughud.right_y
            ),
            (10, 10, -10, 10)
        );
        assert_eq!(shipped.debughud.key, c.debughud.key);
        assert_eq!(shipped.debughud.right_x, c.debughud.right_x);
        assert_eq!(c.debughud.font_scale, 2.5);
        assert_eq!(c.debughud.opacity, 0.45);
        assert_eq!(shipped.debughud.font_scale, c.debughud.font_scale);
        assert_eq!(shipped.debughud.opacity, c.debughud.opacity);
        let custom: super::Config =
            toml::from_str("[debughud]\nkey='F2'\ntarget=false\nright_x=-40\n").unwrap();
        assert_eq!(custom.debughud.key, "F2");
        assert!(!custom.debughud.target);
        assert_eq!(custom.debughud.right_x, -40);
    }
    #[test]
    fn environment_features_are_opt_in_and_old_sections_keep_their_values() {
        let old: super::Config =
            toml::from_str("[chatlog]\nhighlight=['hello']\n[frametime]\nx=-50\ny=200\n").unwrap();
        assert!(
            !old.mobcount.enabled
                && !old.worldinfo.enabled
                && !old.lightoverlay.enabled
                && !old.freelook.enabled
        );
        assert_eq!(old.freelook.hold_key, "LALT");
        assert_eq!(old.chatlog.highlight, ["hello"]);
        assert!(old.chatlog.notifications && old.chatlog.file && old.frametime.line);
        assert_eq!((old.frametime.x, old.frametime.y), (-50, 200));
        let shipped: super::Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        assert_eq!(shipped.mobcount.range, old.mobcount.range);
        assert_eq!(shipped.worldinfo.y, old.worldinfo.y);
        assert_eq!(shipped.lightoverlay.threshold, old.lightoverlay.threshold);
        assert_eq!(shipped.freelook.hold_key, old.freelook.hold_key);
    }
    #[test]
    fn one_invalid_value_drops_only_its_section() {
        let text = "[gui]\nfont_size = 21.0\n[crosshair]\ncolor = [300, 0, 0]\n";
        assert!(toml::from_str::<super::Config>(text).is_err());
        let (config, dropped) = super::recover(text).unwrap();
        assert_eq!(dropped, ["crosshair"]);
        assert_eq!(config.gui.font_size, 21.0);
        let defaults = super::Config::default();
        assert_eq!(config.crosshair.color, defaults.crosshair.color);
        // Syntax errors and valid files are not "recovered".
        assert!(super::recover("[gui\nfont_size = 1").is_none());
        assert!(super::recover("[gui]\nfont_size = 21.0\n").is_none());
    }

    #[test]
    fn inventory_grid_defaults_and_legacy_text_mode_preserve_existing_positions() {
        let old: super::Config =
            toml::from_str("[inventoryhud]\nx=-40\ny=300\nhotbar_only=true\nmax_rows=6\n").unwrap();
        assert!(
            old.inventoryhud.grid
                && old.inventoryhud.show_counts
                && old.inventoryhud.show_durability
        );
        assert_eq!(
            (
                old.inventoryhud.x,
                old.inventoryhud.y,
                old.inventoryhud.slot_size
            ),
            (-40, 300, 40)
        );
        assert!(old.inventoryhud.hotbar_only);
        let text: super::Config = toml::from_str("[inventoryhud]\ngrid=false\n").unwrap();
        assert!(!text.inventoryhud.grid);
    }
    #[test]
    fn legacy_gui_settings_use_a_readable_japanese_font_size() {
        let old: super::Config =
            toml::from_str("[gui]\nenabled=true\nmenu_key='INSERT'\n").unwrap();
        assert_eq!(old.gui.font_size, 16.);
        let custom: super::Config = toml::from_str("[gui]\nfont_size=20.0\n").unwrap();
        assert_eq!(custom.gui.font_size, 20.);
    }
    #[test]
    fn edge_and_wear_helpers_default_off_and_preserve_legacy_configs() {
        let legacy: super::Config = toml::from_str("").unwrap();
        let shipped: super::Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        assert!(!legacy.safewalk.enabled && !legacy.durabilityguard.enabled);
        assert!(!shipped.safewalk.enabled && !shipped.durabilityguard.enabled);
        assert_eq!(shipped.safewalk.margin, legacy.safewalk.margin);
        assert_eq!(
            shipped.durabilityguard.reserve,
            legacy.durabilityguard.reserve
        );
        assert!(!shipped.durabilityguard.empty_slot);
        let cfg:super::Config=toml::from_str("[safewalk]\nenabled=true\nmargin=0.2\n[durabilityguard]\nreserve=20\nempty_slot=false\n").unwrap();
        assert!(cfg.safewalk.enabled);
        assert_eq!(cfg.safewalk.margin, 0.2);
        assert_eq!(cfg.durabilityguard.reserve, 20);
        assert!(!cfg.durabilityguard.empty_slot);
    }
    #[test]
    fn item_helpers_are_optional_and_shipped_defaults_match() {
        let legacy: super::Config = toml::from_str("").unwrap();
        let shipped: super::Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        assert!(!legacy.autofish.enabled && !legacy.durabilityalert.enabled);
        assert!(!shipped.autofish.enabled && !shipped.durabilityalert.enabled);
        assert_eq!(shipped.autofish.delay, legacy.autofish.delay);
        assert_eq!(shipped.autofish.reserve, legacy.autofish.reserve);
        assert_eq!(
            shipped.durabilityalert.threshold,
            legacy.durabilityalert.threshold
        );
        assert!(shipped.durabilityalert.armor && shipped.durabilityalert.hotbar);
    }
    #[test]
    fn mining_and_counter_defaults_load_in_old_configs_and_match_the_template() {
        let legacy: super::Config = toml::from_str("").unwrap();
        let shipped: super::Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        assert!(!legacy.mininghud.enabled && !legacy.itemcounter.enabled);
        assert!(!shipped.mininghud.enabled && !shipped.itemcounter.enabled);
        assert_eq!(
            (
                shipped.mininghud.x,
                shipped.mininghud.y,
                shipped.mininghud.coordinates,
                shipped.mininghud.hide_idle
            ),
            (
                legacy.mininghud.x,
                legacy.mininghud.y,
                legacy.mininghud.coordinates,
                legacy.mininghud.hide_idle
            )
        );
        assert_eq!(shipped.itemcounter.targets, legacy.itemcounter.targets);
        assert_eq!(
            (
                shipped.itemcounter.x,
                shipped.itemcounter.y,
                shipped.itemcounter.held,
                shipped.itemcounter.hotbar_only,
                shipped.itemcounter.show_zero
            ),
            (
                legacy.itemcounter.x,
                legacy.itemcounter.y,
                legacy.itemcounter.held,
                legacy.itemcounter.hotbar_only,
                legacy.itemcounter.show_zero
            )
        );
    }
    #[test]
    fn movement_helpers_default_off_and_old_configs_remain_compatible() {
        let legacy: super::Config = toml::from_str("[auto_sprint]\njump = false\n").unwrap();
        assert!(!legacy.auto_walk.enabled && !legacy.auto_sneak.enabled);
        assert!(!legacy.auto_jump.enabled && !legacy.auto_swim.enabled);
        assert!(legacy.auto_jump.forward_only && !legacy.auto_swim.forward_only);
        assert!(legacy.auto_walk.pause_back && !legacy.auto_sneak.forward_only);
        let shipped: super::Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        assert!(shipped.inventorymove.enabled && legacy.inventorymove.enabled);
        assert!(
            shipped.inventorymove.jump
                && !shipped.inventorymove.sneak
                && shipped.inventorymove.automatic
        );
        assert_eq!(shipped.inventorymove.forward_key, "W");
        assert!(!shipped.auto_walk.enabled && !shipped.auto_sneak.enabled);
        assert!(!shipped.auto_jump.enabled && !shipped.auto_swim.enabled);
        assert_eq!(shipped.auto_jump.key, "NONE");
        assert_eq!(shipped.auto_swim.key, "NONE");
        assert_eq!(
            shipped.auto_jump.forward_only,
            legacy.auto_jump.forward_only
        );
        assert_eq!(
            shipped.auto_swim.forward_only,
            legacy.auto_swim.forward_only
        );
        assert_eq!(shipped.auto_walk.key, "NONE");
        assert_eq!(shipped.auto_sneak.key, "NONE");
        assert_eq!(shipped.auto_walk.pause_back, legacy.auto_walk.pause_back);
        assert_eq!(
            shipped.auto_sneak.forward_only,
            legacy.auto_sneak.forward_only
        );
    }
    use super::Config;

    #[test]
    fn legacy_autosprint_enables_jump_and_explicit_false_disables_it() {
        let legacy: Config = toml::from_str("[auto_sprint]\nenabled = true\n").unwrap();
        assert!(legacy.auto_sprint.jump);
        let no_jump: Config = toml::from_str("[auto_sprint]\njump = false\n").unwrap();
        assert!(!no_jump.auto_sprint.jump);
    }

    #[test]
    fn shipped_default_file_parses_and_matches_the_built_in_defaults() {
        let shipped: Config = toml::from_str(super::DEFAULT_TEXT).unwrap();
        let built_in = Config::default();
        assert_eq!(
            (
                shipped.radar.enabled,
                shipped.radar.range,
                shipped.radar.size,
                shipped.radar.x,
                shipped.radar.y
            ),
            (
                built_in.radar.enabled,
                built_in.radar.range,
                built_in.radar.size,
                built_in.radar.x,
                built_in.radar.y
            )
        );
        assert_eq!(
            (
                shipped.targethud.enabled,
                shipped.targethud.x,
                shipped.targethud.y
            ),
            (
                built_in.targethud.enabled,
                built_in.targethud.x,
                built_in.targethud.y
            )
        );
        assert_eq!(shipped.coordinates.enabled, built_in.coordinates.enabled);
        assert_eq!(
            (shipped.gui.hud_x, shipped.gui.hud_y),
            (built_in.gui.hud_x, built_in.gui.hud_y)
        );
        assert_eq!(shipped.compass.x, built_in.compass.x);
        assert_eq!(shipped.waila.x, built_in.waila.x);
        assert_eq!(shipped.crosshair, built_in.crosshair);
        assert_eq!(shipped.lowhealth, built_in.lowhealth);
        assert_eq!(shipped.playeralert, built_in.playeralert);
        assert_eq!(shipped.effects, built_in.effects);
        assert_eq!(shipped.air, built_in.air);
        assert_eq!(shipped.nametags.names, built_in.nametags.names);
        assert_eq!(shipped.keystrokes.x, built_in.keystrokes.x);
        assert_eq!(shipped.keystrokes.y, built_in.keystrokes.y);
        assert_eq!(
            (
                shipped.zoom.enabled,
                shipped.zoom.hold_key.as_str(),
                shipped.zoom.factor,
                shipped.zoom.sensitivity,
                shipped.zoom.sensitivity_scale
            ),
            (
                built_in.zoom.enabled,
                built_in.zoom.hold_key.as_str(),
                built_in.zoom.factor,
                built_in.zoom.sensitivity,
                built_in.zoom.sensitivity_scale
            )
        );
        assert_eq!(
            (
                shipped.fullbright.key.as_str(),
                shipped.fullbright.enabled,
                shipped.fullbright.gamma
            ),
            (
                built_in.fullbright.key.as_str(),
                built_in.fullbright.enabled,
                built_in.fullbright.gamma
            )
        );
        assert_eq!(
            (
                shipped.waila.key.as_str(),
                shipped.waila.enabled,
                shipped.waila.reach,
                shipped.waila.details,
                shipped.waila.fluids,
                shipped.waila.entities,
                shipped.waila.health,
                shipped.waila.identifiers,
                shipped.waila.y
            ),
            (
                built_in.waila.key.as_str(),
                built_in.waila.enabled,
                built_in.waila.reach,
                built_in.waila.details,
                built_in.waila.fluids,
                built_in.waila.entities,
                built_in.waila.health,
                built_in.waila.identifiers,
                built_in.waila.y
            )
        );
        assert_eq!(
            (
                shipped.breadcrumbs.enabled,
                shipped.breadcrumbs.points,
                shipped.fov.enabled,
                shipped.fov.degrees,
                shipped.coordinates.nether,
                &shipped.blockesp.extra
            ),
            (
                built_in.breadcrumbs.enabled,
                built_in.breadcrumbs.points,
                built_in.fov.enabled,
                built_in.fov.degrees,
                built_in.coordinates.nether,
                &built_in.blockesp.extra
            )
        );
        for (shipped, built_in) in [
            (&shipped.dimension, &built_in.dimension),
            (&shipped.stopwatch, &built_in.stopwatch),
            (&shipped.fps, &built_in.fps),
            (&shipped.cps, &built_in.cps),
            (&shipped.speed, &built_in.speed),
            (&shipped.direction, &built_in.direction),
            (&shipped.clock, &built_in.clock),
            (&shipped.memory, &built_in.memory),
            (&shipped.entities, &built_in.entities),
            (&shipped.server, &built_in.server),
            (&shipped.chunkborder, &built_in.chunkborder),
        ] {
            assert_eq!(shipped.key, built_in.key);
            assert_eq!(shipped.enabled, built_in.enabled);
        }
        assert_eq!(
            (
                shipped.waypoints.key.as_str(),
                shipped.waypoints.enabled,
                shipped.waypoints.tracers,
                shipped.waypoints.compass
            ),
            (
                built_in.waypoints.key.as_str(),
                built_in.waypoints.enabled,
                built_in.waypoints.tracers,
                built_in.waypoints.compass
            )
        );
        assert_eq!(
            (
                shipped.chatlog.key.as_str(),
                shipped.chatlog.enabled,
                &shipped.chatlog.highlight,
                shipped.blockesp.tracers,
                shipped.gui.notifications
            ),
            (
                built_in.chatlog.key.as_str(),
                built_in.chatlog.enabled,
                &built_in.chatlog.highlight,
                built_in.blockesp.tracers,
                built_in.gui.notifications
            )
        );
    }

    #[test]
    fn readout_sections_are_optional_and_configurable() {
        let config: Config =
            toml::from_str("[fps]\nenabled = true\n[keystrokes]\nkey = \"K\"\nx = -20\n").unwrap();
        assert!(config.fps.enabled);
        let positions: Config =
            toml::from_str("[gui]\nhud_x=-20\nhud_y=30\n[compass]\nx=120\n[waila]\nx=-80\n")
                .unwrap();
        assert_eq!(
            (
                positions.gui.hud_x,
                positions.gui.hud_y,
                positions.compass.x,
                positions.waila.x
            ),
            (-20, 30, 120, -80)
        );
        assert_eq!(config.fps.key, "NONE");
        // Extra block names are a plain list; other new sections default to off.
        let extra: Config =
            toml::from_str("[blockesp]\nextra = [\"mob_spawner\", \"*shulker_box\"]\n").unwrap();
        assert_eq!(extra.blockesp.extra, ["mob_spawner", "*shulker_box"]);
        assert!(extra.blockesp.ores && extra.blockesp.chests);
        assert!(!config.waila.enabled && !config.breadcrumbs.enabled && !config.fov.enabled);
        assert!(!config.vitals.enabled && !config.nametags.enabled);
        assert!(!config.effects.enabled && !config.air.enabled && config.nametags.names);
        let status: Config = toml::from_str(
            "[effects]\nwarn_expiring=true\nmax_rows=8\n[air]\nonly_underwater=false\nticks=true\n",
        )
        .unwrap();
        assert!(status.effects.warn_expiring && status.air.ticks && !status.air.only_underwater);
        assert_eq!(status.effects.max_rows, 8);
        assert!(
            !config.crosshair.enabled && !config.lowhealth.enabled && !config.playeralert.enabled
        );
        let alerts: Config = toml::from_str("[crosshair]\ncolor=[0,255,100]\nt_style=true\n[lowhealth]\nthreshold=45.0\nabsorption=true\n[playeralert]\nrange=64.0\nhud=false\n").unwrap();
        assert_eq!(alerts.crosshair.color, [0, 255, 100]);
        assert!(alerts.crosshair.t_style && alerts.lowhealth.absorption);
        assert_eq!(
            (alerts.lowhealth.threshold, alerts.playeralert.range),
            (45., 64.)
        );
        assert!(!alerts.playeralert.hud);
        assert!(config.vitals.food && config.vitals.experience && config.targethud.health);
        assert!(!config.zoom.sensitivity);
        assert_eq!(
            (
                config.zoom.sensitivity_scale,
                config.nametags.range,
                config.nametags.max_labels
            ),
            (0.25, 48., 32)
        );
        let custom: Config = toml::from_str("[vitals]\nx=-10\nfood=false\n[nametags]\nmax_labels=12\n[zoom]\nsensitivity=true\nsensitivity_scale=0.1\n").unwrap();
        assert_eq!(custom.vitals.x, -10);
        assert!(!custom.vitals.food && custom.zoom.sensitivity);
        assert_eq!(
            (custom.nametags.max_labels, custom.zoom.sensitivity_scale),
            (12, 0.1)
        );
        assert!(!config.coordinates.nether && config.blockesp.extra.is_empty());
        assert!(!config.cps.enabled);
        assert!(config.radar.enabled && config.targethud.enabled);
        assert!(config.compass.enabled);
        assert_eq!((config.compass.width, config.compass.y), (400., 10));
        assert!(!config.killaura.enabled);
        assert_eq!((config.killaura.range, config.killaura.cps), (3., 8.));
        assert!(config.killaura.players && config.killaura.mobs);
        assert_eq!(
            (
                config.keystrokes.key.as_str(),
                config.keystrokes.x,
                config.keystrokes.y
            ),
            ("K", -20, -10)
        );
    }
}
