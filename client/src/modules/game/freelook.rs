//! Decouple only the local camera -> Actor rotation synchronization.
use super::{actions_build, patch, read, readable};
use super::{
    actors::type_hash,
    entities::{component_pools, PoolView},
    memory::ReadBatch,
};
use crate::signatures::CAMERA_PLAYER_SYNC_SIGNATURE;
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
};
use windows_sys::Win32::System::{
    Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_READWRITE},
    SystemInformation::GetTickCount64,
};
pub(super) static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static TARGET: OnceLock<Result<usize, String>> = OnceLock::new();
static UNTIL: AtomicU64 = AtomicU64::new(0);
static PENDING: AtomicUsize = AtomicUsize::new(0);
static SUPPRESSED: AtomicU64 = AtomicU64::new(0);
static RESTORES: AtomicU64 = AtomicU64::new(0);
static POSE_LOCKS: AtomicU64 = AtomicU64::new(0);
static POSE_REJECTS: AtomicU64 = AtomicU64::new(0);
static SCENE: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    rotation: [f32; 4],
    head: [f32; 2],
}
impl Pose {
    fn settled(self) -> Self {
        Self {
            rotation: [
                self.rotation[0],
                self.rotation[1],
                self.rotation[0],
                self.rotation[1],
            ],
            head: [self.head[0]; 2],
        }
    }
}
struct PoseView {
    rotation: usize,
    head: usize,
}
impl PoseView {
    unsafe fn resolve(player: usize) -> Option<Self> {
        let mut m = ReadBatch::new();
        if !m.readable(player, 0x258) {
            return None;
        }
        let registry = read::<usize>(player + 0x10);
        let entity = read::<u32>(player + 0x18);
        let [pool, rotation_pool] = component_pools(
            &mut m,
            registry,
            [
                type_hash("ActorHeadRotationComponent"),
                type_hash("ActorRotationComponent"),
            ],
        );
        let head = PoolView::new(&mut m, pool?, 8)?.component(&mut m, entity)?;
        let rotation = read::<usize>(player + 0x250);
        if PoolView::new(&mut m, rotation_pool?, 16)?.component(&mut m, entity)? != rotation {
            return None;
        }
        (m.readable(rotation, 16) && writable(rotation, 16) && writable(head, 8))
            .then_some(Self { rotation, head })
    }
    unsafe fn get(&self) -> Option<Pose> {
        let pose = Pose {
            rotation: read(self.rotation),
            head: read(self.head),
        };
        pose.rotation
            .iter()
            .chain(pose.head.iter())
            .all(|v| v.is_finite() && v.abs() < 1e6)
            .then_some(pose)
    }
    unsafe fn set(&self, pose: Pose) {
        (self.rotation as *mut [f32; 4]).write_unaligned(pose.rotation);
        (self.head as *mut [f32; 2]).write_unaligned(pose.head);
    }
}
// Cancellation waits for a capture already underway to publish PENDING.
// This closes the check/disable race during DLL shutdown.
static CAPTURE: Mutex<()> = Mutex::new(());
#[derive(Clone, Copy, Debug, PartialEq)]
struct Saved {
    key: (usize, u32, usize, usize),
    rotation: [f32; 4],
    pose: Pose,
}
fn transition(
    state: &mut Option<Saved>,
    key: (usize, u32, usize, usize),
    rotation: [f32; 4],
    pose: Pose,
    wanted: bool,
) -> Option<Saved> {
    if state.is_some_and(|s| s.key != key) {
        *state = None;
    }
    if wanted {
        if state.is_none() {
            *state = Some(Saved {
                key,
                rotation,
                pose: pose.settled(),
            });
        }
        *state
    } else {
        state.take()
    }
}
thread_local! {static SAVED:RefCell<(u64,Option<Saved>)>=const{RefCell::new((0,None))};}
/// The old camera/Actor no longer belongs to a world. Forget the obligation
/// without ever dereferencing it, and fence old thread-local copies by scene.
pub(super) fn forget_world() {
    if PENDING.load(Ordering::Acquire) == 0 {
        return;
    }
    let _capture = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
    SCENE.fetch_add(1, Ordering::Release);
    PENDING.store(0, Ordering::Release);
}
pub(super) fn resolve() -> Result<usize, String> {
    TARGET
        .get_or_init(|| {
            if !actions_build() {
                return Err("FreeLookには解析済みビルドが必要です".into());
            }
            patch::find(CAMERA_PLAYER_SYNC_SIGNATURE)
        })
        .clone()
}
pub fn available() -> Result<(), String> {
    super::actions::available()?;
    if ORIGINAL.load(Ordering::Acquire) == 0 {
        Err("カメラ同期フックを準備できません".into())
    } else {
        Ok(())
    }
}
pub fn request(on: bool, now: u64) {
    if on {
        UNTIL.store(now.saturating_add(100), Ordering::Release);
    } else if UNTIL.swap(0, Ordering::AcqRel) != 0 {
        drop(CAPTURE.lock().unwrap_or_else(|e| e.into_inner()));
    }
}
pub fn restored() -> bool {
    PENDING.load(Ordering::Acquire) == 0 && !super::freecam::pending()
}
pub fn active() -> bool {
    !restored()
}
pub fn diagnostic() -> String {
    format!(
        "FreeLook active={} pending={} suppressed={} restores={} pose_locks={} pose_rejects={}",
        active(),
        PENDING.load(Ordering::Acquire),
        SUPPRESSED.load(Ordering::Relaxed),
        RESTORES.load(Ordering::Relaxed),
        POSE_LOCKS.load(Ordering::Relaxed),
        POSE_REJECTS.load(Ordering::Relaxed)
    )
}
fn valid_quaternion(q: [f32; 4]) -> bool {
    q.iter().all(|v| v.is_finite()) && (0.8..=1.2).contains(&q.iter().map(|v| v * v).sum::<f32>())
}
pub(super) unsafe fn writable(at: usize, len: usize) -> bool {
    let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
    VirtualQuery(at as *const _, &mut m, std::mem::size_of_val(&m)) != 0
        && m.State == MEM_COMMIT
        && m.Protect & PAGE_READWRITE != 0
        && m.Protect & PAGE_GUARD == 0
        && at
            .checked_add(len)
            .is_some_and(|e| e <= m.BaseAddress as usize + m.RegionSize)
}
pub(super) unsafe extern "system" fn sync_hook(camera: usize, perspective: usize, player: usize) {
    super::actions::IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let original: unsafe extern "system" fn(usize, usize, usize) =
        std::mem::transmute(ORIGINAL.load(Ordering::Acquire));
    if UNTIL.load(Ordering::Acquire) == 0 && restored() && !super::freecam::blocking() {
        original(camera, perspective, player);
        super::actions::IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    let local = player != 0
        && player == super::PLAYER.load(Ordering::Acquire)
        && super::fresh(super::SAMPLED_AT.load(Ordering::Acquire));
    let mut skip = false;
    if local && readable(player, 0x1c) && readable(camera, 0x40) && writable(camera + 0x30, 16) {
        let key = (
            read::<usize>(player + 0x10),
            read::<u32>(player + 0x18),
            camera,
            read::<usize>(player + 0x1f0),
        );
        let q = read::<[f32; 4]>(camera + 0x30);
        if valid_quaternion(q) {
            let Some((pose_view, pose)) =
                PoseView::resolve(player).and_then(|view| view.get().map(|pose| (view, pose)))
            else {
                POSE_REJECTS.fetch_add(1, Ordering::Relaxed);
                original(camera, perspective, player);
                super::actions::IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
                return;
            };
            let _capture = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
            let now = GetTickCount64();
            let freecam = super::freecam::apply(camera, player, q, now);
            let wanted = freecam
                || (now < UNTIL.load(Ordering::Acquire)
                    && !crate::gui::captures_input()
                    && !super::inventory::screen_open()
                    && crate::input::is_foreground());
            SAVED.with(|cell| {
                let mut saved = cell.borrow_mut();
                let scene = SCENE.load(Ordering::Acquire);
                if saved.0 != scene {
                    *saved = (scene, None);
                }
                let state = &mut saved.1;
                let before = state.is_some();
                if let Some(saved) = transition(state, key, q, pose, wanted) {
                    pose_view.set(saved.pose);
                    POSE_LOCKS.fetch_add(1, Ordering::Relaxed);
                    if !wanted {
                        ((camera + 0x30) as *mut [f32; 4]).write_unaligned(saved.rotation);
                        RESTORES.fetch_add(1, Ordering::Relaxed);
                    }
                }
                match (before, state.is_some()) {
                    (false, true) => {
                        PENDING.fetch_add(1, Ordering::Release);
                    }
                    (true, false) => {
                        PENDING.fetch_sub(1, Ordering::Release);
                    }
                    _ => {}
                }
                skip = wanted;
                if skip {
                    SUPPRESSED.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    }
    if !skip {
        original(camera, perspective, player);
    }
    super::actions::IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_preserves_the_original_until_release_and_never_restores_into_another_camera() {
        let mut state = None;
        let key = (10, 42, 100, 1);
        let original = [0., 0., 0., 1.];
        let changed = [0., 0.707, 0., 0.707];
        let pose = Pose {
            rotation: [5., 10., 5., 10.],
            head: [10., 10.],
        };
        let turned = Pose {
            head: [45., 40.],
            ..pose
        };
        let saved = Saved {
            key,
            rotation: original,
            pose,
        };
        assert_eq!(
            transition(&mut state, key, original, pose, true),
            Some(saved)
        );
        assert_eq!(
            transition(&mut state, key, changed, turned, true),
            Some(saved)
        );
        assert_eq!(
            transition(&mut state, key, changed, turned, false),
            Some(saved)
        );
        assert!(state.is_none());
        transition(&mut state, key, original, pose, true);
        assert_eq!(
            transition(&mut state, (20, 42, 100, 1), changed, pose, false),
            None
        );
        assert!(state.is_none());
        transition(&mut state, key, original, pose, true);
        transition(&mut state, (10, 43, 200, 2), changed, turned, true);
        assert_eq!(
            transition(&mut state, (10, 43, 200, 2), original, pose, false),
            Some(Saved {
                key: (10, 43, 200, 2),
                rotation: changed,
                pose: turned.settled()
            })
        );
    }
    #[test]
    fn invalid_or_uninitialized_quaternions_are_rejected() {
        assert!(valid_quaternion([0., 0., 0., 1.]));
        assert!(!valid_quaternion([0.; 4]));
        assert!(!valid_quaternion([f32::NAN, 0., 0., 1.]));
        assert!(!valid_quaternion([0., 0., 0., 2.]));
    }
}
