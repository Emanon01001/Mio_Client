//! Freecam changes CameraComponent position, never Actor position/velocity.
//! CameraComponent is validated by the existing local camera synchronization
//! callback, quaternion/optics checks and writable-page checks before writing.
use super::{read, readable};
use crate::input;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Mutex, OnceLock,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

pub(super) const POSITION: usize = 0x40;
const LIMIT: f32 = 30_000_000.;
static UNTIL: AtomicU64 = AtomicU64::new(0);
static SPEED: AtomicU32 = AtomicU32::new(10f32.to_bits());
static BOOST: AtomicU32 = AtomicU32::new(3f32.to_bits());
static PENDING: AtomicBool = AtomicBool::new(false);
static WRITES: AtomicU64 = AtomicU64::new(0);
static RESTORES: AtomicU64 = AtomicU64::new(0);
static REJECTS: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<Option<Session>> = Mutex::new(None);
static PROBE: Mutex<Option<CameraSample>> = Mutex::new(None);
static ABSENT: AtomicU64 = AtomicU64::new(0);

// The renderer does not draw from the CameraComponent written above:
// LevelRendererPlayer's camera setup reads another CameraComponent (the
// active render camera, an element of the same 0x120-byte pool) and derives
// its origin (+0x64C), an eye-offset copy (+0x658) and further state from it.
// Overwriting only the origin afterwards left that state inconsistent (the
// terrain was drawn sheared). Instead the render camera is found once, as the
// pool element whose position the setup just copied into the origin, and
// from then on receives the Freecam position before the setup runs, so all
// it derives agrees. Only while the latest camera update is recent.
/// Trampoline of the render camera setup (0 until hooked).
pub(super) static RENDER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static RENDER_TARGET: OnceLock<Result<RenderTarget, String>> = OnceLock::new();
/// The position to render from and when the camera update produced it.
static RENDER_POSITION: Mutex<Option<([f32; 3], u64)>> = Mutex::new(None);
static RENDER_WRITES: AtomicU64 = AtomicU64::new(0);
/// Calls of the render camera setup seen while Freecam wanted to render.
static RENDER_CALLS: AtomicU64 = AtomicU64::new(0);
/// The CameraComponent the Freecam update writes, and the render camera
/// found next to it (0 until found).
static LAST_CAMERA: AtomicUsize = AtomicUsize::new(0);
static RENDER_CAMERA: AtomicUsize = AtomicUsize::new(0);
/// Frames in which the setup drew from the Freecam position.
static RENDER_MATCHED: AtomicU64 = AtomicU64::new(0);
static RENDER_SEARCHES: AtomicU64 = AtomicU64::new(0);
/// CameraComponent pool element size, and how far around the updated camera
/// the render camera is looked for (a pool page holds 128 elements).
const CAMERA_STRIDE: usize = 0x120;
const CAMERA_SEARCH: usize = 128;
/// After a search that found nothing, the next one waits this long (ms).
const SEARCH_RETRY_MS: u64 = 100;
static NEXT_SEARCH: AtomicU64 = AtomicU64::new(0);
/// A render position older than this (ms) is not used: the camera updates
/// stopped (world left, game paused), so the normal view must come back.
const RENDER_FRESH_MS: u64 = 200;
/// The prologue of the render camera setup, before the origin store.
const RENDER_PROLOGUE: [u8; 12] = [
    0x41, 0x57, 0x41, 0x56, 0x41, 0x54, 0x56, 0x57, 0x53, 0x48, 0x81, 0xEC,
];

#[derive(Clone, Copy, Debug)]
struct RenderTarget {
    function: usize,
    origin: usize,
    copy: usize,
}

#[derive(Clone, Copy)]
struct CameraSample {
    camera: usize,
    position: [f32; 3],
    optics: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    player: usize,
    registry: usize,
    entity: u32,
    dimension: usize,
    camera: usize,
}
#[derive(Clone, Copy, Debug)]
struct Session {
    key: Identity,
    origin: [f32; 3],
    position: [f32; 3],
    last_written: [f32; 3],
    vanilla: [f32; 3],
    at: u64,
}
struct Frame {
    key: Identity,
    current: [f32; 3],
    orientation: [f32; 4],
    axes: [f32; 3],
    speed: f32,
    now: u64,
    wanted: bool,
}
#[derive(Debug, PartialEq)]
enum Update {
    Idle,
    Override([f32; 3]),
    Restore([f32; 3]),
}
fn update(state: &mut Option<Session>, f: Frame) -> Update {
    if state.is_some_and(|s| s.key != f.key) {
        *state = None;
    }
    if !f.wanted {
        return state
            .take()
            .map(|s| Update::Restore(s.restore(f.current)))
            .unwrap_or(Update::Idle);
    }
    let s = state.get_or_insert(Session {
        key: f.key,
        origin: f.current,
        position: f.current,
        last_written: f.current,
        vanilla: f.current,
        at: f.now,
    });
    s.update(f.current, f.orientation, f.axes, f.speed, f.now);
    Update::Override(s.position)
}
fn sane(position: [f32; 3]) -> bool {
    position.iter().all(|v| v.is_finite() && v.abs() <= LIMIT)
}
fn optics(values: [f32; 4]) -> bool {
    let [aspect, fov, near, far] = values;
    (0.1..10.).contains(&aspect)
        && (0.01..std::f32::consts::PI).contains(&fov)
        && (0.0001..10.).contains(&near)
        && far.is_finite()
        && far > near
        && far < 1e8
}
/// Quaternion rotates right (+X) and back (+Z) into world space.
fn step(position: [f32; 3], q: [f32; 4], axes: [f32; 3], distance: f32) -> [f32; 3] {
    if !distance.is_finite()
        || distance <= 0.
        || !sane(position)
        || !q.iter().chain(axes.iter()).all(|v| v.is_finite())
    {
        return position;
    }
    let [x, y, z, w] = q;
    let right = [
        1. - 2. * (y * y + z * z),
        2. * (x * y + w * z),
        2. * (x * z - w * y),
    ];
    let back = [
        2. * (x * z + w * y),
        2. * (y * z - w * x),
        1. - 2. * (x * x + y * y),
    ];
    let [forward, strafe, lift] = axes;
    let mut delta = std::array::from_fn::<_, 3, _>(|i| strafe * right[i] - forward * back[i]);
    delta[1] += lift;
    let length = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !length.is_finite() || length < 1e-6 {
        return position;
    }
    let next = std::array::from_fn(|i| position[i] + delta[i] / length * distance);
    if sane(next) {
        next
    } else {
        position
    }
}
impl Session {
    fn update(&mut self, vanilla: [f32; 3], q: [f32; 4], axes: [f32; 3], speed: f32, now: u64) {
        // The controller normally replaces CameraComponent position each frame.
        // Remember its latest value for restoration; never adopt our own write.
        if vanilla != self.last_written {
            self.vanilla = vanilla;
        }
        let dt = if now >= self.at {
            (now - self.at).min(50) as f32 / 1000.
        } else {
            0.
        };
        self.at = now;
        self.position = step(self.position, q, axes, speed * dt);
        self.last_written = self.position;
    }
    fn restore(self, current: [f32; 3]) -> [f32; 3] {
        if current == self.last_written {
            self.vanilla
        } else {
            current
        }
    }
}
/// Finds the render camera setup and the origin offsets it stores to.
pub(super) fn render_target() -> Result<usize, String> {
    RENDER_TARGET
        .get_or_init(|| unsafe {
            let site = super::patch::find(crate::signatures::RENDER_CAMERA_ORIGIN_SIGNATURE)
                .map_err(|e| format!("Freecam 描画カメラ: {e}"))?;
            let origin = read::<u32>(site + 14) as usize;
            let copy = read::<u32>(site + 48) as usize;
            let expected = super::layout().map(|l| l.camera_origin).unwrap_or(origin);
            if origin != expected || !(origin..origin + 0x40).contains(&copy) {
                return Err(format!(
                    "Freecam 描画カメラの配置が一致しません（{origin:#x} / {copy:#x}）"
                ));
            }
            let function = (site.saturating_sub(0x1000)..site)
                .rev()
                .find(|&at| {
                    std::slice::from_raw_parts(at as *const u8, RENDER_PROLOGUE.len())
                        == RENDER_PROLOGUE
                        && read::<u8>(at - 1) == 0xCC
                })
                .ok_or("Freecam 描画カメラの関数先頭が見つかりません")?;
            Ok(RenderTarget {
                function,
                origin,
                copy,
            })
        })
        .as_ref()
        .map(|t| t.function)
        .map_err(|e| e.clone())
}

/// The render position, if Freecam is on and the camera update is recent.
fn render_position(now: u64) -> Option<[f32; 3]> {
    if !blocking() {
        return None;
    }
    let (position, at) = (*RENDER_POSITION.try_lock().ok()?)?;
    (now.saturating_sub(at) <= RENDER_FRESH_MS && sane(position)).then_some(position)
}

fn set_render_position(value: Option<[f32; 3]>, now: u64) {
    *RENDER_POSITION.lock().unwrap_or_else(|e| e.into_inner()) = value.map(|p| (p, now));
    if value.is_none() {
        // Found again on the next session: pools move between worlds.
        RENDER_CAMERA.store(0, Ordering::Relaxed);
    }
}

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// The render camera among `candidates` (address, position): the one whose
/// position the setup copied verbatim into its origin. When the updated camera
/// `own` and another both match, the other one (the render camera holds a copy
/// of the updated camera). None if it is not clear.
fn pick_render_camera(
    candidates: impl Iterator<Item = (usize, [f32; 3])>,
    origin: [f32; 3],
    own: usize,
) -> Option<usize> {
    let matches: Vec<usize> = candidates
        .filter(|&(_, p)| bits(p) == bits(origin))
        .map(|(at, _)| at)
        .collect();
    match matches.as_slice() {
        [one] => Some(*one),
        many => {
            let others: Vec<usize> = many.iter().copied().filter(|&at| at != own).collect();
            (others.len() == 1).then(|| others[0])
        }
    }
}

/// A live CameraComponent: readable, writable position, sane values.
unsafe fn camera_ok(camera: usize) -> bool {
    // A committed read-write page is readable too: one VirtualQuery.
    camera != 0 && super::freelook::writable(camera, 0x5c) && camera_values(camera)
}

/// The values of a CameraComponent known to be readable look sane.
unsafe fn camera_values(camera: usize) -> bool {
    sane(read(camera + POSITION)) && optics(read(camera + 0x4c))
}

/// The committed read-write region holding `at`, as [start, end).
unsafe fn writable_region(at: usize) -> Option<(usize, usize)> {
    use windows_sys::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_READWRITE,
    };
    let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
    (VirtualQuery(at as *const _, &mut m, std::mem::size_of_val(&m)) != 0
        && m.State == MEM_COMMIT
        && m.Protect & PAGE_READWRITE != 0
        && m.Protect & PAGE_GUARD == 0)
        .then(|| {
            let start = m.BaseAddress as usize;
            (start, start + m.RegionSize)
        })
}

/// Looks for the render camera around the updated camera, matching the
/// origin the setup has just written.
unsafe fn find_render_camera(own: usize, origin: [f32; 3], now: u64) -> Option<usize> {
    if own == 0 || now < NEXT_SEARCH.load(Ordering::Relaxed) {
        return None;
    }
    // One VirtualQuery for the whole pool page; candidates inside it are only
    // read (unused slots are passed over by their values).
    let (start, end) = writable_region(own)?;
    RENDER_SEARCHES.fetch_add(1, Ordering::Relaxed);
    let inside = |at: usize| at >= start && at.checked_add(0x5c).is_some_and(|e| e <= end);
    let candidates = (0..CAMERA_SEARCH)
        .flat_map(|k| {
            let step = k * CAMERA_STRIDE;
            [
                own.checked_add(step),
                own.checked_sub(step).filter(|_| k > 0),
            ]
        })
        .flatten()
        .filter(|&at| inside(at) && camera_values(at))
        .map(|at| (at, read::<[f32; 3]>(at + POSITION)));
    let found = pick_render_camera(candidates, origin, own);
    if found.is_none() {
        NEXT_SEARCH.store(now.saturating_add(SEARCH_RETRY_MS), Ordering::Relaxed);
    }
    found
}

/// LevelRendererPlayer's camera setup: render from the Freecam camera.
pub(super) unsafe extern "system" fn render_hook(
    renderer: usize,
    params: usize,
    third: usize,
    fourth: usize,
) -> usize {
    super::actions::IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        std::mem::transmute(RENDER_ORIGINAL.load(Ordering::Acquire));
    let now = GetTickCount64();
    let position = render_position(now);
    // Before the setup: put the Freecam position into the render camera.
    let render_camera = RENDER_CAMERA.load(Ordering::Relaxed);
    let mut wrote = false;
    if let Some(position) = position {
        if camera_ok(render_camera) {
            ((render_camera + POSITION) as *mut [f32; 3]).write_unaligned(position);
            RENDER_WRITES.fetch_add(1, Ordering::Relaxed);
            wrote = true;
        }
    }
    let result = original(renderer, params, third, fourth);
    if let (Some(Ok(target)), Some(position)) = (RENDER_TARGET.get(), position) {
        RENDER_CALLS.fetch_add(1, Ordering::Relaxed);
        let vtable = super::camera::renderer_player_vtable();
        if vtable != 0
            && readable(renderer, target.origin + 12)
            && read::<usize>(renderer) == vtable
        {
            let origin = read::<[f32; 3]>(renderer + target.origin);
            if wrote && bits(origin) == bits(position) {
                RENDER_MATCHED.fetch_add(1, Ordering::Relaxed);
            } else {
                // Not found yet, or the setup read another camera: look again.
                let found = find_render_camera(LAST_CAMERA.load(Ordering::Relaxed), origin, now);
                RENDER_CAMERA.store(found.unwrap_or(0), Ordering::Relaxed);
            }
        }
    }
    super::actions::IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}

pub fn available() -> Result<(), String> {
    super::freelook::available()?;
    if RENDER_ORIGINAL.load(Ordering::Acquire) == 0 {
        return Err(render_target()
            .err()
            .unwrap_or_else(|| "Freecam 描画カメラのフックを準備できません".into()));
    }
    let anchor = super::patch::find(crate::signatures::CAMERA_POSITION_COPY_SIGNATURE)
        .map_err(|e| format!("Freecam カメラ位置の検証: {e}"))?;
    if unsafe { read::<u8>(anchor + 4) } as usize != POSITION {
        return Err("Freecam カメラ位置の配置が一致しません".into());
    }
    Ok(())
}
pub fn request(on: bool, now: u64, speed: f32, boost: f32) {
    if on {
        SPEED.store(speed.clamp(0.1, 100.).to_bits(), Ordering::Relaxed);
        BOOST.store(boost.clamp(1., 10.).to_bits(), Ordering::Relaxed);
        UNTIL.store(now.saturating_add(150), Ordering::Release);
    } else if UNTIL.swap(0, Ordering::AcqRel) != 0 {
        // Wait until a callback that already observed the request has published
        // its restore obligation. No native pointers are written from here.
        drop(STATE.lock().unwrap_or_else(|e| e.into_inner()));
    }
}
pub fn blocking() -> bool {
    let until = UNTIL.load(Ordering::Acquire);
    until != 0 && ABSENT.load(Ordering::Relaxed) == 0 && unsafe { GetTickCount64() } < until
}
pub(super) fn pending() -> bool {
    PENDING.load(Ordering::Acquire)
}
pub(super) fn forget() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *state = None;
    PENDING.store(false, Ordering::Release);
    set_render_position(None, 0);
}
pub(super) fn observe_player(player: usize, now: u64) {
    if player != 0 {
        ABSENT.store(0, Ordering::Relaxed);
        return;
    }
    let since = ABSENT.load(Ordering::Relaxed);
    if since == 0 {
        ABSENT.store(now, Ordering::Relaxed);
    } else if now.saturating_sub(since) >= 250 {
        if pending() {
            forget();
        }
        super::freelook::forget_world();
    }
}
/// Only called while the live local Actor and its pose are validated, on the
/// native camera callback. Identity includes the actor's generation and world.
pub(super) unsafe fn apply(camera: usize, player: usize, q: [f32; 4], now: u64) -> bool {
    if !blocking() && !pending() {
        return false;
    }
    // Read-write covers readable: one query for the camera.
    if !super::freelook::writable(camera, 0x5c) || !readable(player, 0x208) {
        REJECTS.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let current = read::<[f32; 3]>(camera + POSITION);
    let optical = read(camera + 0x4c);
    *PROBE.lock().unwrap_or_else(|e| e.into_inner()) = Some(CameraSample {
        camera,
        position: current,
        optics: optical,
    });
    if !sane(current) || !optics(optical) {
        REJECTS.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let key = Identity {
        player,
        registry: read(player + 0x10),
        entity: read(player + 0x18),
        dimension: read(player + 0x1f0),
        camera,
    };
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let active =
        !crate::gui::captures_input() && !super::inventory::screen_open() && input::is_foreground();
    let held = |key| active && input::key_down(key);
    let axis = |plus, minus| held(plus) as i32 as f32 - held(minus) as i32 as f32;
    let axes = [axis(0x57, 0x53), axis(0x44, 0x41), axis(0x20, 0x10)];
    let speed = f32::from_bits(SPEED.load(Ordering::Relaxed))
        * if held(0x11) {
            f32::from_bits(BOOST.load(Ordering::Relaxed))
        } else {
            1.
        };
    let action = update(
        &mut guard,
        Frame {
            key,
            current,
            orientation: q,
            axes,
            speed,
            now,
            wanted: blocking(),
        },
    );
    PENDING.store(guard.is_some(), Ordering::Release);
    match action {
        Update::Idle => {
            set_render_position(None, now);
            false
        }
        Update::Override(position) => {
            ((camera + POSITION) as *mut [f32; 3]).write_unaligned(position);
            LAST_CAMERA.store(camera, Ordering::Relaxed);
            set_render_position(Some(position), now);
            WRITES.fetch_add(1, Ordering::Relaxed);
            true
        }
        Update::Restore(position) => {
            ((camera + POSITION) as *mut [f32; 3]).write_unaligned(position);
            set_render_position(None, now);
            RESTORES.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}
pub fn status() -> String {
    let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(s) => {
            let distance = s
                .position
                .iter()
                .zip(s.origin)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt();
            format!(
                "カメラ {:.1} / {:.1} / {:.1}（開始地点から{distance:.1}m）",
                s.position[0], s.position[1], s.position[2]
            )
        }
        None if REJECTS.load(Ordering::Relaxed) != 0 => {
            "カメラの配置を検証できません（freecamdebugで確認）".into()
        }
        None => "カメラを待機中 / W A S D・Space・Shift・Ctrl".into(),
    }
}
pub fn diagnostic() -> String {
    let status = status();
    let probe = PROBE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|p| {
            format!(
                "camera={:#x} position={:?} optics[aspect,fov,near,far]={:?}",
                p.camera, p.position, p.optics
            )
        })
        .unwrap_or_else(|| "camera sample: waiting".into());
    format!(
        "Freecam requested={} pending={} render_hook={} renderer_vtable={} render_camera={:#x} render_calls={} render_writes={} render_matched={} render_searches={} writes={} restores={} rejects={} / {}",
        blocking(),
        pending(),
        match RENDER_TARGET.get() {
            Some(Ok(t)) if RENDER_ORIGINAL.load(Ordering::Acquire) != 0 =>
                format!("{:#x}(origin {:#x}, copy {:#x})", t.function, t.origin, t.copy),
            Some(Err(e)) => e.clone(),
            _ => "未設置".into(),
        },
        if super::camera::renderer_player_vtable() != 0 {
            "learned"
        } else {
            "unknown"
        },
        RENDER_CAMERA.load(Ordering::Relaxed),
        RENDER_CALLS.load(Ordering::Relaxed),
        RENDER_WRITES.load(Ordering::Relaxed),
        RENDER_MATCHED.load(Ordering::Relaxed),
        RENDER_SEARCHES.load(Ordering::Relaxed),
        WRITES.load(Ordering::Relaxed),
        RESTORES.load(Ordering::Relaxed),
        REJECTS.load(Ordering::Relaxed),
        format_args!("{status}\n{probe}")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disable_restores_once_and_world_camera_or_entity_changes_never_restore_stale_values() {
        let key = Identity {
            player: 1,
            registry: 2,
            entity: 3,
            dimension: 4,
            camera: 5,
        };
        let frame = |key, current, wanted, now| Frame {
            key,
            current,
            wanted,
            now,
            orientation: [0., 0., 0., 1.],
            axes: [1., 0., 0.],
            speed: 10.,
        };
        let mut s = None;
        assert_eq!(
            update(&mut s, frame(key, [1., 2., 3.], true, 1000)),
            Update::Override([1., 2., 3.])
        );
        let Update::Override(p) = update(&mut s, frame(key, [1., 2., 3.], true, 1050)) else {
            panic!()
        };
        assert!((p[2] - 2.5).abs() < 1e-6);
        assert_eq!(
            update(&mut s, frame(key, p, false, 1060)),
            Update::Restore([1., 2., 3.])
        );
        assert_eq!(
            update(&mut s, frame(key, [1., 2., 3.], false, 1070)),
            Update::Idle
        );
        for new_key in [
            Identity { player: 9, ..key },
            Identity { registry: 9, ..key },
            Identity { entity: 9, ..key },
            Identity {
                dimension: 9,
                ..key
            },
            Identity { camera: 9, ..key },
        ] {
            update(&mut s, frame(key, [1., 2., 3.], true, 2000));
            assert_eq!(
                update(&mut s, frame(new_key, [90.; 3], false, 2010)),
                Update::Idle
            );
            assert!(s.is_none());
            update(&mut s, frame(key, [1., 2., 3.], true, 2020));
            assert_eq!(
                update(&mut s, frame(new_key, [90.; 3], true, 2030)),
                Update::Override([90.; 3])
            );
            s = None;
        }
    }
    #[test]
    fn the_render_camera_is_the_element_whose_position_was_copied() {
        let origin = [316.5, 140.25, 51.125];
        let other = [10., 20., 30.];
        // Only the render camera holds the copied origin.
        let list = [(0x100, other), (0x220, origin), (0x340, other)];
        assert_eq!(
            pick_render_camera(list.into_iter(), origin, 0x100),
            Some(0x220)
        );
        // The updated camera also holds it: the other match is the copy.
        let list = [(0x100, origin), (0x220, origin), (0x340, other)];
        assert_eq!(
            pick_render_camera(list.into_iter(), origin, 0x100),
            Some(0x220)
        );
        // Only the updated camera matches: the setup reads it.
        let list = [(0x100, origin), (0x220, other)];
        assert_eq!(
            pick_render_camera(list.into_iter(), origin, 0x100),
            Some(0x100)
        );
        // Ambiguous or nothing: none.
        let list = [(0x100, other), (0x220, origin), (0x340, origin)];
        assert_eq!(pick_render_camera(list.into_iter(), origin, 0x100), None);
        assert_eq!(
            pick_render_camera([(0x100, other)].into_iter(), origin, 0x100),
            None
        );
        // Bitwise, not approximately.
        let near = [316.5, 140.25, f32::from_bits(51.125f32.to_bits() + 1)];
        assert_eq!(
            pick_render_camera([(0x220, near)].into_iter(), origin, 0x100),
            None
        );
    }

    #[test]
    fn movement_is_view_relative_normalized_and_bounded() {
        let p = [0.; 3];
        let q = [0., 0., 0., 1.];
        assert_eq!(step(p, q, [1., 0., 0.], 10.), [0., 0., -10.]);
        assert_eq!(step(p, q, [0., 1., 0.], 10.), [10., 0., 0.]);
        assert_eq!(step(p, q, [0., 0., 1.], 10.), [0., 10., 0.]);
        let diagonal = step(p, q, [1., 1., 1.], 10.);
        assert!((diagonal.iter().map(|v| v * v).sum::<f32>().sqrt() - 10.).abs() < 1e-5);
        let turn = [
            0.,
            std::f32::consts::FRAC_1_SQRT_2,
            0.,
            std::f32::consts::FRAC_1_SQRT_2,
        ];
        let west = step(p, turn, [1., 0., 0.], 10.);
        assert!((west[0] + 10.).abs() < 1e-4 && west[2].abs() < 1e-4);
        assert_eq!(step(p, q, [f32::NAN, 0., 0.], 1.), p);
        assert_eq!(step(p, q, [1., 0., 0.], f32::INFINITY), p);
        assert_eq!(step([LIMIT, 0., 0.], q, [0., 1., 0.], 10.), [LIMIT, 0., 0.]);
        assert!(optics([16. / 9., 1.2, 0.1, 1000.]));
        for v in [[0.; 4], [1., 1., 0., 1000.], [1., f32::NAN, 0.1, 1000.]] {
            assert!(!optics(v));
        }
    }
    #[test]
    fn elapsed_time_pause_restore_and_native_position_changes_are_preserved() {
        let mut s = Session {
            key: Identity {
                player: 1,
                registry: 2,
                entity: 3,
                dimension: 4,
                camera: 5,
            },
            origin: [0.; 3],
            position: [0.; 3],
            last_written: [0.; 3],
            vanilla: [0.; 3],
            at: 1000,
        };
        let q = [0., 0., 0., 1.];
        s.update([0.; 3], q, [1., 0., 0.], 10., 1010);
        assert!((s.position[2] + 0.1).abs() < 1e-6);
        let detached = s.position;
        s.update(detached, q, [0.; 3], 10., 20_000);
        assert_eq!(s.position, detached, "pause cannot accumulate movement");
        s.update(detached, q, [1., 0., 0.], 10., 20_010);
        assert!((s.position[2] + 0.2).abs() < 1e-6);
        assert_eq!(s.restore(s.last_written), [0.; 3]);
        assert_eq!(
            s.restore([7., 8., 9.]),
            [7., 8., 9.],
            "keep a new native camera position"
        );
        s.update([1., 2., 3.], q, [0.; 3], 10., 20_020);
        assert_eq!(s.restore(s.last_written), [1., 2., 3.]);
        let p = s.position;
        s.update(p, q, [1., 0., 0.], 10., 5);
        assert_eq!(s.position, p, "clock rollback must not move the camera");
    }
    #[test]
    fn camera_speed_is_independent_of_frame_rate_and_boost_is_multiplicative() {
        let key = Identity {
            player: 1,
            registry: 2,
            entity: 3,
            dimension: 4,
            camera: 5,
        };
        let run = |interval, speed| {
            let mut state = None;
            let mut current = [0.; 3];
            for now in (1000..=2000).step_by(interval) {
                let result = update(
                    &mut state,
                    Frame {
                        key,
                        current,
                        orientation: [0., 0., 0., 1.],
                        axes: [1., 0., 0.],
                        speed,
                        now,
                        wanted: true,
                    },
                );
                if let Update::Override(p) = result {
                    current = p;
                } else {
                    panic!()
                }
            }
            current
        };
        for interval in [5, 10, 20, 50] {
            assert!((run(interval, 10.)[2] + 10.).abs() < 0.001);
            assert!((run(interval, 30.)[2] + 30.).abs() < 0.001);
        }
    }
}
