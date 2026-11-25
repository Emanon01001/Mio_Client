//! Native game backend. This facade owns sampling and hook lifetime; the
//! child modules resolve layouts, validate memory, copy entities/camera state,
//! and adapt native state to the public SDK. See analysis/notes/esp.md.
//!
//! The children live in folders by what they read (`native`: memory, layouts
//! and SDK publication; `level`: world, blocks and terrain; `actor`: entities,
//! players and skins; `player`: the local player's inventory, status and
//! controls; `attack`: attacks and hits; `render`: camera and view hooks) and
//! are re-exported here, so every one is addressed as `crate::game::<name>`.
mod actor;
mod attack;
mod level;
mod native;
mod player;
mod render;

use actor::entities;
pub use actor::{actors, friends};
pub(crate) use actor::{player_roster, skins};
pub use attack::{combat, hits, weapon};
pub use level::{blocks, world, world_info};
pub(crate) use level::{session_info, terrain_session};
pub(crate) use native::signatures;
use native::{layout, memory, metadata, sdk, snapshot};
use player::{control, inventory_transfer};
pub use player::{
    durability_guard, fishing, inventory, item_assist, items, mining, safe_walk, status, telemetry,
};
use render::camera;
pub use render::{freecam, freelook, visual};

pub mod actions;

use crate::modules::performance_profiler::metrics::{Scope, Stage};
use camera::learn_renderer;
pub use camera::{camera, Camera};
use control::apply_movement;
pub use control::{set_fly, set_fly_input, set_fly_speed};
use entities::collect_boxes;
pub use entities::Aabb;
pub(crate) use layout::build_name;
#[cfg(feature = "entrypoint")]
pub(crate) use layout::diagnostic as compatibility_diagnostic;
pub(crate) use layout::ACCESSOR_PATCH_OFFSET;
pub(crate) use layout::SUPPORTED;
pub use layout::{actions_build, available, init, known_build};
use layout::{layout, Layout, LAYOUT};
use memory::span;
pub(crate) use memory::{read, readable, string};
use sdk::{sdk_frame, sdk_velocity};

use crate::modules::patch;
use hudhook::mh::{MH_CreateHook, MH_DisableHook, MH_EnableHook, MH_Initialize, MH_STATUS};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

/// Minimum time between entity snapshots.
const SNAPSHOT_INTERVAL_MS: u64 = 8;
/// A sample older than this means the HUD is not being drawn (no world).
const SAMPLE_MAX_AGE_MS: u64 = 1000;

/// `bool accessor(client)`
type AccessorFn = unsafe extern "system" fn(*const u8) -> u8;

/// Trampoline to the original accessor; non-zero once the hook was created.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// Threads currently inside `detour`; disabling waits for it to drain.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
/// Hook users (modules that are enabled).
static USERS: Mutex<usize> = Mutex::new(0);
/// Whether the hook is enabled; changed only while `USERS` is locked. Not
/// implied by `USERS > 0`: a `release` whose detour did not drain in time
/// keeps its user but has already disabled the hook.
static ENABLED: AtomicBool = AtomicBool::new(false);

static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);
static Z: AtomicI32 = AtomicI32::new(0);
/// GetTickCount64 of the last sample (0 = never).
static SAMPLED_AT: AtomicU64 = AtomicU64::new(0);
/// The ClientInstance seen by the last sample. It lives as long as the game.
static CLIENT: AtomicUsize = AtomicUsize::new(0);
/// The local player seen by the last sample; only valid while `SAMPLED_AT` is fresh.
static PLAYER: AtomicUsize = AtomicUsize::new(0);

/// Modules that need the entity snapshot and the camera (ESP, waypoints, ...).
static WANT_BOXES: AtomicUsize = AtomicUsize::new(0);
/// Read-only entity overlays that need at most 20 snapshots per second.
static WANT_ENTITIES: AtomicUsize = AtomicUsize::new(0);
static WANT_CAMERA: AtomicUsize = AtomicUsize::new(0);
static CAMERA_AT: AtomicU64 = AtomicU64::new(0);
/// Entities in the last snapshot.
static BOX_COUNT: AtomicUsize = AtomicUsize::new(0);
static BOXES_AT: AtomicU64 = AtomicU64::new(0);
/// Bounding boxes of every entity except the local player.
static BOXES: LazyLock<snapshot::Snapshot<(u32, Aabb)>> = LazyLock::new(snapshot::Snapshot::new);

unsafe fn sample(l: &Layout, client: *const u8) {
    let _profile = Scope::new(Stage::NativeSample);
    // These offsets come from the accessor's own code, and the accessor has
    // just dereferenced the same pointers successfully.
    let instance: usize = read(client as usize + l.client_instance);
    if instance == 0 {
        terrain_session::observe_presence(false, GetTickCount64());
        player_roster::clear();
        return;
    }
    let vtable: usize = read(instance);
    let local_player: unsafe extern "system" fn(*const c_void) -> *const u8 =
        std::mem::transmute(read::<usize>(vtable + l.vt_local_player));
    let player = local_player(instance as *const c_void) as usize;
    terrain_session::observe_presence(player != 0, GetTickCount64());
    if player == 0 {
        player_roster::clear();
        return;
    }
    let state: usize = read(player + l.actor_state);
    let bounds: usize = read(player + l.actor_aabb);
    if state == 0 || bounds == 0 {
        return;
    }
    let eye: [f32; 3] = read(state);
    let feet: f32 = read(bounds + 4);
    let now = GetTickCount64();
    X.store(eye[0].floor() as i32, Ordering::Relaxed);
    Y.store(feet.floor() as i32, Ordering::Relaxed);
    Z.store(eye[2].floor() as i32, Ordering::Relaxed);
    CLIENT.store(instance, Ordering::Relaxed);
    PLAYER.store(player, Ordering::Relaxed);
    SAMPLED_AT.store(now, Ordering::Relaxed);
    player_roster::sample(player, [eye[0], feet, eye[2]], now);
    let column = [
        eye[0].floor() as i32,
        feet.floor() as i32,
        eye[2].floor() as i32,
    ];
    blocks::sample(player, column);
    blocks::surface::sample(player, column);
    blocks::lighting::sample(player, column);
    world::sample(player, now);
    session_info::sample(player, now);
    {
        let _scope = Scope::new(Stage::WorldSample);
        world_info::sample(player, instance, column, now);
    }
    actors::sample(player, now);
    {
        let _scope = Scope::new(Stage::SkinSample);
        skins::sample(player, now);
    }
    {
        let _scope = Scope::new(Stage::StatusSample);
        status::sample(player, now);
    }
    {
        let _scope = Scope::new(Stage::ItemSample);
        items::sample(player, now);
    }
    mining::sample(player, now);
    telemetry::sample(player, now);
    let look = blocks::look_wanted();
    let flight = blocks::flight::wanted();
    let feed = hits::wanted();
    if (look || flight || feed) && readable(instance + l.view_matrix, 48) {
        // The view basis is (right, up, back); the camera looks along -back.
        let view: [f32; 12] = read(instance + l.view_matrix);
        let forward = [-view[8], -view[9], -view[10]];
        if look {
            let _scope = Scope::new(Stage::LookSample);
            blocks::sample_look(player, eye, forward);
        }
        if flight {
            let _scope = Scope::new(Stage::FlightSample);
            blocks::flight::sample(player, eye, forward);
        }
        if feed {
            hits::sample(eye, forward, now);
        }
    }
    blocks::lighting::sample_debug(player);
    blocks::environment::sample_target(player, now);
    apply_movement(l, instance, state);
    if !crate::api::input_ready() {
        // State remains readable even if this version lacks native action hooks.
        let _ = sdk_frame(instance, player, false);
    }

    let scene = WANT_BOXES.load(Ordering::Relaxed) != 0;
    if (scene || WANT_CAMERA.load(Ordering::Relaxed) != 0)
        && now.saturating_sub(CAMERA_AT.load(Ordering::Relaxed)) >= SNAPSHOT_INTERVAL_MS
    {
        learn_renderer(l, instance);
        CAMERA_AT.store(now, Ordering::Relaxed);
    }
    if (scene || WANT_ENTITIES.load(Ordering::Relaxed) != 0)
        && now.saturating_sub(BOXES_AT.load(Ordering::Relaxed))
            >= if scene { SNAPSHOT_INTERVAL_MS } else { 50 }
    {
        // Never make the game thread wait for the render thread.
        let mut scope = Scope::new(Stage::EntitySnapshot);
        if let Some(count) = BOXES.publish(|boxes| collect_boxes(l, player, bounds, boxes)) {
            scope.units(count);
            BOX_COUNT.store(count, Ordering::Relaxed);
            BOXES_AT.store(now, Ordering::Relaxed);
        }
    }
    // `client` is the live ClientInstanceScreenModel used by both this HUD
    // property and normal chat submission. Never retain this native pointer.
    crate::native_chat::repeat::sample(client as usize, instance, player);
}

unsafe extern "system" fn detour(client: *const u8) -> u8 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: AccessorFn = std::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    // The original validates `client` (and faults exactly as the game would).
    let result = original(client);
    if let Some(Ok(layout)) = LAYOUT.get() {
        sample(layout, client);
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}

fn fresh(at: u64) -> bool {
    at != 0 && unsafe { GetTickCount64() }.saturating_sub(at) <= SAMPLE_MAX_AGE_MS
}

/// The local player's block position, if in a world.
pub fn position() -> Option<(i32, i32, i32)> {
    fresh(SAMPLED_AT.load(Ordering::Relaxed)).then(|| {
        (
            X.load(Ordering::Relaxed),
            Y.load(Ordering::Relaxed),
            Z.load(Ordering::Relaxed),
        )
    })
}

/// Whether the HUD was drawn within the last few frames: the world view with
/// no screen (chat, inventory, pause) open. Stricter than `position()`, which
/// stays valid for a second; use it where a key press must not be mistaken for
/// typing.
pub fn in_world_view_now() -> bool {
    const RECENT_MS: u64 = 150;
    let at = SAMPLED_AT.load(Ordering::Relaxed);
    at != 0 && unsafe { GetTickCount64() }.saturating_sub(at) <= RECENT_MS
}

/// For analysis: (image base, ClientInstance, local player) as of the last
/// sample, if in a world. The pointers are only meaningful to an external
/// read-only tool; nothing in the client uses them off the game thread.
pub fn debug_pointers() -> Option<(usize, usize, usize)> {
    fresh(SAMPLED_AT.load(Ordering::Relaxed)).then(|| {
        (
            layout::image().0,
            CLIENT.load(Ordering::Relaxed),
            PLAYER.load(Ordering::Relaxed),
        )
    })
}

/// Asks the hook to keep `shared_boxes()` and `camera()` up to date. Counted: each
/// `want_boxes(true)` needs one matching `want_boxes(false)`.
pub fn want_boxes(on: bool) {
    if on {
        WANT_BOXES.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let left = WANT_BOXES
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .map_or(0, |n| n - 1);
    if left == 0 && WANT_ENTITIES.load(Ordering::Relaxed) == 0 {
        BOXES_AT.store(0, Ordering::Relaxed);
    }
    if left == 0 && WANT_CAMERA.load(Ordering::Relaxed) == 0 {
        CAMERA_AT.store(0, Ordering::Relaxed);
    }
}
/// Camera-only overlays do not need an enumeration of every entity's hitbox.
pub fn use_camera(on: bool) -> Result<(), String> {
    if on {
        acquire()?;
        WANT_CAMERA.fetch_add(1, Ordering::Relaxed);
    } else {
        release()?;
        let _ =
            WANT_CAMERA.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
        if WANT_CAMERA.load(Ordering::Relaxed) == 0 && WANT_BOXES.load(Ordering::Relaxed) == 0 {
            CAMERA_AT.store(0, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// Starts or stops one module's use of the scene (entity boxes and camera):
/// keeps the game hook installed and the snapshot running while in use.
pub fn use_scene(on: bool) -> Result<(), String> {
    if on {
        acquire()?;
    } else {
        release()?;
    }
    want_boxes(on);
    Ok(())
}

/// Share the entity snapshot without requesting a high-rate camera snapshot.
pub fn use_entities(on: bool) -> Result<(), String> {
    if on {
        acquire()?;
        WANT_ENTITIES.fetch_add(1, Ordering::Relaxed);
    } else {
        release()?;
        let left = WANT_ENTITIES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .map_or(0, |n| n - 1);
        if left == 0 && WANT_BOXES.load(Ordering::Relaxed) == 0 {
            BOXES_AT.store(0, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// How many entities the latest snapshot holds, if in a world.
pub fn box_count() -> Option<usize> {
    fresh(BOXES_AT.load(Ordering::Relaxed)).then(|| BOX_COUNT.load(Ordering::Relaxed))
}

/// Generation-bearing IDs join live boxes to independently sampled actor facts.
/// Readers share the immutable frame instead of cloning its vector.
pub(crate) fn shared_boxes() -> Option<Arc<Vec<(u32, Aabb)>>> {
    fresh(BOXES_AT.load(Ordering::Relaxed)).then(|| BOXES.load())
}

// --- Hook management -----------------------------------------------------------------

/// Starts sampling for one more user; the first one installs the hook.
pub fn acquire() -> Result<(), String> {
    let target = layout()?.accessor as *mut c_void;
    let mut users = USERS.lock().unwrap_or_else(|e| e.into_inner());
    if !ENABLED.load(Ordering::Relaxed) {
        unsafe {
            if ORIGINAL.load(Ordering::Acquire) == 0 {
                match MH_Initialize() {
                    MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
                    e => return Err(format!("MH_Initialize: {e:?}")),
                }
                let mut trampoline = std::ptr::null_mut();
                MH_CreateHook(target, detour as *mut c_void, &mut trampoline)
                    .ok()
                    .map_err(|e| format!("MH_CreateHook: {e:?}"))?;
                ORIGINAL.store(trampoline as usize, Ordering::Release);
            }
            MH_EnableHook(target)
                .ok()
                .map_err(|e| format!("MH_EnableHook: {e:?}"))?;
        }
        ENABLED.store(true, Ordering::Relaxed);
    }
    *users += 1;
    Ok(())
}

/// Ends one user's sampling; the last one removes the hook.
pub fn release() -> Result<(), String> {
    let target = layout()?.accessor as *mut c_void;
    let mut users = USERS.lock().unwrap_or_else(|e| e.into_inner());
    if *users == 0 {
        return Ok(());
    }
    if *users == 1 {
        unsafe {
            match MH_DisableHook(target) {
                MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_DISABLED => {}
                e => return Err(format!("MH_DisableHook: {e:?}")),
            }
            ENABLED.store(false, Ordering::Relaxed);
            // Let a call that was already inside `detour` finish (it is short)
            // before the caller may go on to unload this DLL.
            if !crate::hooks::drain(&IN_FLIGHT, 1) {
                return Err("game hook is still executing; keep DLL loaded".into());
            }
        }
        SAMPLED_AT.store(0, Ordering::Relaxed);
        BOXES_AT.store(0, Ordering::Relaxed);
        CAMERA_AT.store(0, Ordering::Relaxed);
        BOXES.clear();
    }
    *users -= 1;
    Ok(())
}
