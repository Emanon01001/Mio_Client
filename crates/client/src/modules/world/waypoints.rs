//! Waypoints: named positions drawn as markers in the world, through walls,
//! with their distance. Managed with the `wp` console command and saved next
//! to the DLL. Drawing only; nothing in the game is modified.
//!
//! A waypoint remembers the dimension it was added in and is shown only
//! there. The world itself is not known, so one list is shared by every world.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use hudhook::imgui::Ui;
use serde::{Deserialize, Serialize};

use crate::game::telemetry;
use crate::game::world::{self, Realm};
use crate::modules::settings::{Detail, Value};
use crate::modules::view::project;
use crate::modules::{game, key_or_warn, Module, Tick};
use crate::{config, log};

pub const FILE_NAME: &str = "Mio_Client_waypoints.toml";
const MAX_WAYPOINTS: usize = 64;
const MAX_NAME: usize = 24;

const MARKER: [f32; 4] = [0.25, 0.9, 1.0, 0.95];
const TRACER: [f32; 4] = [0.25, 0.9, 1.0, 0.55];
const LABEL: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BACKING: [f32; 4] = [0.0, 0.0, 0.0, 0.5];

/// A named block position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Waypoint {
    name: String,
    x: i32,
    y: i32,
    z: i32,
    /// Where it was added ("overworld", "nether", "end", or another
    /// dimension's name). Absent in older files: shown everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dimension: Option<String>,
    #[serde(default)]
    automatic_death: bool,
}

impl Waypoint {
    /// Whether to show it while the player is in `current` (None = unknown).
    fn shown_in(&self, current: Option<&str>) -> bool {
        match (&self.dimension, current) {
            (Some(own), Some(current)) => own == current,
            _ => true,
        }
    }
}

/// The name a dimension is saved under.
fn dimension_key(name: &str, realm: Realm) -> String {
    match realm {
        Realm::Overworld => "overworld".into(),
        Realm::Nether => "nether".into(),
        Realm::End => "end".into(),
        Realm::Other => name.trim().to_lowercase(),
    }
}

/// The dimension the player is in now, if it can be read.
fn current_dimension() -> Option<String> {
    world::dimension().map(|(name, realm)| dimension_key(&name, realm))
}

/// The saved file: `[[waypoint]]` tables.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Store {
    #[serde(default, rename = "waypoint")]
    list: Vec<Waypoint>,
}

impl Store {
    fn add_death(
        &mut self,
        position: [i32; 3],
        dimension: Option<String>,
        keep: usize,
    ) -> Result<String, String> {
        if self.list.iter().filter(|w| !w.automatic_death).count() >= MAX_WAYPOINTS {
            return Err("waypoint list is full of manual markers".into());
        }
        while self.list.iter().filter(|w| w.automatic_death).count() >= keep.clamp(1, 16)
            || self.list.len() >= MAX_WAYPOINTS
        {
            if let Some(i) = self.list.iter().position(|w| w.automatic_death) {
                self.list.remove(i);
            } else {
                break;
            }
        }
        let name = (1..=128)
            .map(|i| format!("death-{i}"))
            .find(|n| self.find(n).is_none())
            .ok_or("no free death marker name")?;
        self.set(&name, position, dimension)?;
        let i = self.find(&name).ok_or("death marker was not added")?;
        self.list[i].automatic_death = true;
        Ok(name)
    }
    fn find(&self, name: &str) -> Option<usize> {
        self.list
            .iter()
            .position(|w| w.name.eq_ignore_ascii_case(name))
    }

    /// Adds a waypoint, or moves the one with the same name.
    fn set(
        &mut self,
        name: &str,
        [x, y, z]: [i32; 3],
        dimension: Option<String>,
    ) -> Result<(), String> {
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(format!("the name must be 1-{MAX_NAME} characters"));
        }
        match self.find(name) {
            Some(i) => {
                let w = &mut self.list[i];
                (w.x, w.y, w.z, w.dimension) = (x, y, z, dimension);
                w.automatic_death = false;
            }
            None if self.list.len() >= MAX_WAYPOINTS => {
                return Err(format!("at most {MAX_WAYPOINTS} waypoints"));
            }
            None => self.list.push(Waypoint {
                name: name.to_string(),
                x,
                y,
                z,
                dimension,
                automatic_death: false,
            }),
        }
        Ok(())
    }

    fn remove(&mut self, name: &str) -> bool {
        match self.find(name) {
            Some(i) => {
                self.list.remove(i);
                true
            }
            None => false,
        }
    }
}

struct State {
    path: Option<PathBuf>,
    store: Store,
}

static STATE: Mutex<State> = Mutex::new(State {
    path: None,
    store: Store { list: Vec::new() },
});
static ACTIVE: AtomicBool = AtomicBool::new(false);
static DEATH_MARKERS: AtomicBool = AtomicBool::new(false);
pub(crate) fn show_deaths(on: bool) {
    DEATH_MARKERS.store(on, Ordering::Relaxed);
}
/// Called by the worker after a game-thread death event, never from a hook.
pub(crate) fn record_death(
    position: [i32; 3],
    dimension: Option<String>,
    keep: usize,
) -> Result<String, String> {
    let mut s = state();
    let name = s.store.add_death(position, dimension, keep)?;
    save(&s);
    Ok(name)
}
/// Draw a line from the bottom of the screen to each marker.
static TRACERS: AtomicBool = AtomicBool::new(false);
/// Also mark the waypoints' directions on the compass ribbon.
static ON_COMPASS: AtomicBool = AtomicBool::new(true);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Loads the saved waypoints from `dir` (the DLL's folder).
fn load(dir: &Path) {
    let path = dir.join(FILE_NAME);
    let store = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            log!(
                "waypoints: {} is invalid ({e}) - starting empty",
                path.display()
            );
            Store::default()
        }),
        Err(_) => Store::default(),
    };
    let mut state = state();
    state.path = Some(path);
    state.store = store;
}

fn save(state: &State) {
    let Some(path) = &state.path else {
        return;
    };
    let result = toml::to_string(&state.store)
        .map_err(|e| e.to_string())
        .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
    if let Err(e) = result {
        log!("ウェイポイントを保存できません: {}: {e}", path.display());
    }
}

fn parse_position(args: &[&str]) -> Option<[i32; 3]> {
    match args {
        [x, y, z] => Some([x.parse().ok()?, y.parse().ok()?, z.parse().ok()?]),
        _ => None,
    }
}

/// The `wp` console command; `args` is everything after the command word.
pub fn command(args: &[&str]) {
    let mut state = state();
    match args {
        [] | ["list"] => {
            if state.store.list.is_empty() {
                log!("地点はありません。wp add <名前>で追加できます");
            }
            let here = game::position();
            let current = current_dimension();
            for w in &state.store.list {
                // A distance only means something within one dimension.
                let distance = here
                    .filter(|_| w.dimension.is_none() || w.dimension == current)
                    .map(|(x, y, z)| {
                        let d = [w.x - x, w.y - y, w.z - z].map(|v| v as f64);
                        format!(
                            "  {:.0} m",
                            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                        )
                    });
                log!(
                    "  {:<24} {} {} {}  [{}]{}",
                    w.name,
                    w.x,
                    w.y,
                    w.z,
                    w.dimension.as_deref().unwrap_or("すべてのディメンション"),
                    distance.unwrap_or_default()
                );
            }
            if !ACTIVE.load(Ordering::Relaxed) && !state.store.list.is_empty() {
                log!("（地点は非表示です。toggle waypointsで表示できます）");
            }
        }
        ["add" | "set", name, rest @ ..] => {
            let position = if rest.is_empty() {
                game::position().map(|(x, y, z)| [x, y, z])
            } else {
                parse_position(rest)
            };
            let Some(position) = position else {
                if rest.is_empty() {
                    log!(
                        "wp: ワールド未接続です。座標を指定してください: wp add <名前> <x> <y> <z>"
                    );
                } else {
                    log!("使い方: wp add <名前> [<x> <y> <z>]（座標は整数）");
                }
                return;
            };
            let dimension = current_dimension();
            match state.store.set(name, position, dimension.clone()) {
                Ok(()) => {
                    save(&state);
                    let [x, y, z] = position;
                    log!(
                        "地点「{name}」= {x} {y} {z}  [{}]",
                        dimension.as_deref().unwrap_or("すべてのディメンション")
                    );
                    if !ACTIVE.load(Ordering::Relaxed) {
                        log!("（地点は非表示です。toggle waypointsで表示できます）");
                    }
                }
                Err(e) => log!("wp: {e}"),
            }
        }
        ["del" | "remove", name] => {
            if state.store.remove(name) {
                save(&state);
                log!("地点「{name}」を削除しました");
            } else {
                log!("wp: 「{name}」という地点はありません（wp listで一覧を表示）");
            }
        }
        ["clear"] => {
            let count = state.store.list.len();
            state.store.list.clear();
            save(&state);
            log!("{count}件の地点を削除しました");
        }
        _ => {
            log!("使い方: wp list | wp add <名前> [<x> <y> <z>] | wp del <名前> | wp clear");
        }
    }
}

/// The waypoints to show right now: those of the player's dimension.
fn shown() -> Vec<Waypoint> {
    let current = current_dimension();
    state()
        .store
        .list
        .iter()
        .filter(|w| w.shown_in(current.as_deref()))
        .filter(|w| {
            ACTIVE.load(Ordering::Relaxed)
                || w.automatic_death && DEATH_MARKERS.load(Ordering::Relaxed)
        })
        .cloned()
        .collect()
}

/// The Minecraft yaw (0 = south/+Z, 90 = west/-X) from one position towards
/// another, ignoring height.
pub fn bearing(from: [f32; 2], to: [f32; 2]) -> f32 {
    let (dx, dz) = (to[0] - from[0], to[1] - from[1]);
    (-dx).atan2(dz).to_degrees()
}

/// A place to mark on a map.
pub struct MapPoint {
    pub name: String,
    /// Block X and Z.
    pub x: i32,
    pub z: i32,
    /// Recorded automatically where the player died.
    pub death: bool,
}

/// Every saved place of the player's dimension, for the minimap and the
/// world map: shown there even while the in-world markers are off.
pub fn on_map() -> Vec<MapPoint> {
    let current = current_dimension();
    state()
        .store
        .list
        .iter()
        .filter(|w| w.shown_in(current.as_deref()))
        .map(|w| MapPoint {
            name: w.name.clone(),
            x: w.x,
            z: w.z,
            death: w.automatic_death,
        })
        .collect()
}

/// Names and block-centre X/Z of the waypoints the compass should mark; empty
/// unless waypoints are on and their compass option is set.
pub fn on_compass() -> Vec<(String, [f32; 2])> {
    if (!ACTIVE.load(Ordering::Relaxed) && !DEATH_MARKERS.load(Ordering::Relaxed))
        || !ON_COMPASS.load(Ordering::Relaxed)
    {
        return Vec::new();
    }
    shown()
        .into_iter()
        .map(|w| (w.name, [w.x as f32 + 0.5, w.z as f32 + 0.5]))
        .collect()
}

/// Draws the markers for this frame. Called from the overlay's render loop.
pub fn draw(ui: &Ui) {
    if !ACTIVE.load(Ordering::Relaxed) && !DEATH_MARKERS.load(Ordering::Relaxed) {
        return;
    }
    let Some(camera) = game::camera() else {
        return;
    };
    // Copied out, so the lock is not held while drawing.
    let list = shown();
    let tracers = TRACERS.load(Ordering::Relaxed);
    let size = ui.io().display_size;
    let draw_list = ui.get_background_draw_list();
    for w in &list {
        // The centre of the block, one block up (about eye height above it).
        let point = [w.x as f32 + 0.5, w.y as f32 + 1.0, w.z as f32 + 0.5];
        let Some([x, y]) = project(&camera, size, point) else {
            continue;
        };
        let offset = [
            point[0] - camera.origin[0],
            point[1] - camera.origin[1],
            point[2] - camera.origin[2],
        ];
        let distance = offset.iter().map(|v| v * v).sum::<f32>().sqrt();
        if tracers {
            draw_list
                .add_line([size[0] * 0.5, size[1]], [x, y], TRACER)
                .build();
        }
        draw_list
            .add_circle([x, y], 4.0, MARKER)
            .filled(true)
            .build();

        let label = format!("{} {distance:.0}m", w.name);
        let label_size = ui.calc_text_size(&label);
        let min = [x - label_size[0] * 0.5, y - 10.0 - label_size[1]];
        draw_list
            .add_rect(
                [min[0] - 3.0, min[1] - 1.0],
                [min[0] + label_size[0] + 3.0, min[1] + label_size[1] + 1.0],
                BACKING,
            )
            .filled(true)
            .rounding(3.0)
            .build();
        draw_list.add_text(min, LABEL, &label);
    }
}

pub struct Waypoints {
    key: u16,
    /// Why the game hook is unavailable, if it is.
    unavailable: Option<String>,
    enabled: bool,
}

impl Waypoints {
    pub fn new(cfg: &config::Waypoints, dir: &Path) -> Self {
        load(dir);
        TRACERS.store(cfg.tracers, Ordering::Relaxed);
        ON_COMPASS.store(cfg.compass, Ordering::Relaxed);
        Self {
            key: key_or_warn("waypoints", &cfg.key),
            unavailable: game::available().err(),
            enabled: false,
        }
    }
}

impl Module for Waypoints {
    fn name(&self) -> &str {
        "waypoints"
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn options(&self) -> Vec<Detail> {
        vec![
            Detail::boolean(
                "tracers",
                "Tracers",
                "Draw a line from the bottom of the screen to each waypoint.",
                TRACERS.load(Ordering::Relaxed),
            ),
            Detail::boolean(
                "compass",
                "Show on compass",
                "Mark each waypoint's direction on the compass ribbon.",
                ON_COMPASS.load(Ordering::Relaxed),
            ),
        ]
    }

    fn set_option(&mut self, id: &str, value: &Value) -> Result<(), String> {
        match id {
            "tracers" => TRACERS.store(value.boolean()?, Ordering::Relaxed),
            "compass" => ON_COMPASS.store(value.boolean()?, Ordering::Relaxed),
            _ => return Err(format!("不明な設定項目: {id}")),
        }
        Ok(())
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        if let Some(e) = &self.unavailable {
            log!("waypointsを利用できません: {e}");
            return;
        }
        if on == self.enabled {
            return;
        }
        if let Err(e) = game::use_camera(on) {
            log!("waypoints: {e}");
            return;
        }
        ACTIVE.store(on, Ordering::Relaxed);
        self.enabled = on;
    }

    fn status(&self) -> String {
        if let Some(e) = &self.unavailable {
            return format!("({e})");
        }
        if !self.enabled {
            return String::new();
        }
        let total = state().store.list.len();
        let here = shown().len();
        if here == total {
            format!("{total} saved")
        } else {
            format!("{here} here, {total} saved")
        }
    }
}

// ---- Deathpoint: records each death as an automatic waypoint ---------------
pub struct Deathpoint {
    cfg: config::Deathpoint,
    key: u16,
    enabled: bool,
    last: Option<[i32; 3]>,
}
impl Deathpoint {
    pub fn new(c: &config::Deathpoint) -> Self {
        let mut cfg = c.clone();
        cfg.keep = cfg.keep.clamp(1, 16);
        Self {
            cfg,
            key: key_or_warn("deathpoint", &c.key),
            enabled: false,
            last: None,
        }
    }
}
impl Module for Deathpoint {
    fn name(&self) -> &str {
        "deathpoint"
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
        if on && !game::known_build() {
            log!("deathpoint: 対応するゲームビルドが必要です");
            return;
        }
        if let Err(e) = game::use_camera(on) {
            log!("deathpoint: {e}");
            return;
        }
        telemetry::set_death(on);
        show_deaths(on);
        self.enabled = on;
    }
    fn options(&self) -> Vec<Detail> {
        vec![Detail::integer(
            "keep",
            "Keep death markers",
            "Automatic markers only; manual waypoints are preserved.",
            self.cfg.keep,
            1,
            16,
        )]
    }
    fn set_option(&mut self, id: &str, v: &Value) -> Result<(), String> {
        if id != "keep" {
            return Err(format!("不明な設定項目: {id}"));
        }
        self.cfg.keep = (v.number()? as i32).clamp(1, 16);
        Ok(())
    }
    fn tick(&mut self, _: &Tick) {
        if self.enabled {
            if let Some(d) = telemetry::take_death() {
                match record_death(d.position, d.dimension, self.cfg.keep as usize) {
                    Ok(name) => {
                        self.last = Some(d.position);
                        log!("死亡地点を記録: {name} {:?}", d.position);
                    }
                    Err(e) => log!("deathpoint: {e}"),
                }
            }
        }
    }
    fn status(&self) -> String {
        self.last.map_or_else(
            || "死亡地点の記録を待機中".into(),
            |[x, y, z]| format!("{x} {y} {z}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn death_retention_preserves_manual_names_and_capacity() {
        let mut s = Store::default();
        s.set("death-1", [1, 2, 3], None).unwrap();
        for i in 0..5 {
            s.add_death([i, 0, 0], Some("nether".into()), 2).unwrap();
        }
        assert_eq!(s.list.len(), 3);
        assert_eq!(s.list[0].name, "death-1");
        assert!(!s.list[0].automatic_death);
        assert_eq!(
            s.list
                .iter()
                .filter(|w| w.automatic_death)
                .map(|w| w.x)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<Store>(&text).unwrap(), s);
        let mut full = Store::default();
        for i in 0..MAX_WAYPOINTS {
            full.set(&format!("w{i}"), [0; 3], None).unwrap();
        }
        assert!(full.add_death([9; 3], None, 5).is_err());
        assert_eq!(full.list.len(), MAX_WAYPOINTS);
    }

    #[test]
    fn set_adds_moves_by_name_and_remove_deletes() {
        let mut store = Store::default();
        store.set("home", [1, 64, -3], None).unwrap();
        store.set("mine", [100, 12, 200], None).unwrap();
        // Same name in another case moves it instead of adding a duplicate.
        store
            .set("HOME", [5, 70, 5], Some("nether".into()))
            .unwrap();
        assert_eq!(store.list.len(), 2);
        assert_eq!(
            (store.list[0].x, store.list[0].y, store.list[0].z),
            (5, 70, 5)
        );
        assert_eq!(store.list[0].name, "home");
        // Moving it also moves it to the dimension it was set in.
        assert_eq!(store.list[0].dimension.as_deref(), Some("nether"));

        assert!(store.remove("Mine"));
        assert!(!store.remove("mine"));
        assert_eq!(store.list.len(), 1);
    }

    #[test]
    fn rejects_bad_names_and_too_many() {
        let mut store = Store::default();
        assert!(store.set("", [0, 0, 0], None).is_err());
        assert!(store
            .set(&"x".repeat(MAX_NAME + 1), [0, 0, 0], None)
            .is_err());
        for i in 0..MAX_WAYPOINTS {
            store.set(&format!("w{i}"), [0, 0, 0], None).unwrap();
        }
        assert!(store.set("one-more", [0, 0, 0], None).is_err());
        // Moving an existing one is still allowed at the limit.
        assert!(store.set("w0", [1, 2, 3], None).is_ok());
    }

    #[test]
    fn file_round_trips_and_older_files_without_dimensions_still_load() {
        let mut store = Store::default();
        store.set("home", [1, 64, -3], None).unwrap();
        store
            .set("村", [-250, -60, 1024], Some("overworld".into()))
            .unwrap();
        let text = toml::to_string(&store).unwrap();
        assert!(text.contains("[[waypoint]]"), "{text}");
        // No dimension: the key is left out rather than written empty.
        assert_eq!(text.matches("dimension").count(), 1, "{text}");
        assert_eq!(toml::from_str::<Store>(&text).unwrap(), store);
        assert_eq!(toml::from_str::<Store>("").unwrap(), Store::default());

        let old = "[[waypoint]]\nname = \"base\"\nx = 1\ny = 2\nz = 3\n";
        let loaded: Store = toml::from_str(old).unwrap();
        assert_eq!(loaded.list[0].dimension, None);
        assert_eq!(loaded.list[0].name, "base");
    }

    #[test]
    fn waypoints_show_only_in_their_own_dimension() {
        let waypoint = |dimension: Option<&str>| Waypoint {
            name: "w".into(),
            x: 0,
            y: 0,
            z: 0,
            dimension: dimension.map(String::from),
            automatic_death: false,
        };
        assert!(waypoint(Some("nether")).shown_in(Some("nether")));
        assert!(!waypoint(Some("nether")).shown_in(Some("overworld")));
        // Unknown on either side: show rather than hide.
        assert!(waypoint(None).shown_in(Some("overworld")));
        assert!(waypoint(Some("nether")).shown_in(None));
        assert!(waypoint(None).shown_in(None));

        assert_eq!(dimension_key("Overworld", Realm::Overworld), "overworld");
        assert_eq!(dimension_key("Nether", Realm::Nether), "nether");
        assert_eq!(dimension_key("TheEnd", Realm::End), "end");
        assert_eq!(dimension_key(" Moon Base ", Realm::Other), "moon base");
    }

    #[test]
    fn bearing_follows_minecraft_yaw() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(close(bearing([0., 0.], [0., 10.]), 0.)); // +Z = south
        assert!(close(bearing([0., 0.], [-10., 0.]), 90.)); // -X = west
        assert!(close(bearing([0., 0.], [0., -10.]).abs(), 180.)); // -Z = north
        assert!(close(bearing([0., 0.], [10., 0.]), -90.)); // +X = east
        assert!(close(bearing([5., 5.], [-5., 15.]), 45.)); // south-west
    }

    #[test]
    fn parses_positions() {
        assert_eq!(parse_position(&["1", "-64", "300"]), Some([1, -64, 300]));
        assert_eq!(parse_position(&["1", "2"]), None);
        assert_eq!(parse_position(&["1", "2.5", "3"]), None);
        assert_eq!(parse_position(&["a", "b", "c"]), None);
    }
}
