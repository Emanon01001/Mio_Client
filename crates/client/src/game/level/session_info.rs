//! The world's name and player count, read on the game callback every few
//! seconds while a feature (DiscordRPC) subscribes. Reuses the world
//! identity reader (WorldMap) and the level roster (PlayerList); either may
//! be unverified for a build, and that part is then left out.
use crate::game::{player_roster, world_info};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

const INTERVAL_MS: u64 = 5000;
/// Older readings are not reported (left the world, or the hook stopped).
const MAX_AGE_MS: u64 = 3 * INTERVAL_MS;

static USERS: AtomicUsize = AtomicUsize::new(0);
static LAST: AtomicU64 = AtomicU64::new(0);
static INFO: Mutex<Option<(u64, Info)>> = Mutex::new(None);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub world: Option<String>,
    /// The local player included.
    pub players: Option<usize>,
}

/// Keeps the readings coming while held.
pub(crate) struct Subscription(());

pub(crate) fn subscribe() -> Subscription {
    // Each reader may be unverified for this build: that part stays None.
    let _ = world_info::prepare_identity();
    let _ = player_roster::prepare();
    USERS.fetch_add(1, Ordering::AcqRel);
    LAST.store(0, Ordering::Relaxed);
    Subscription(())
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if USERS.fetch_sub(1, Ordering::AcqRel) == 1 {
            *INFO.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
}

/// On the game callback, with the local player.
pub(in crate::game) unsafe fn sample(player: usize, now: u64) {
    if USERS.load(Ordering::Acquire) == 0
        || now.saturating_sub(LAST.load(Ordering::Relaxed)) < INTERVAL_MS
    {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    let info = Info {
        world: world_info::world_name(player),
        players: player_roster::count(player),
    };
    if let Ok(mut slot) = INFO.try_lock() {
        *slot = Some((now, info));
    }
}

/// The latest reading, if recent.
pub(crate) fn info() -> Option<Info> {
    let slot = INFO.lock().unwrap_or_else(|e| e.into_inner());
    let (at, info) = slot.as_ref()?;
    (unsafe { GetTickCount64() }.saturating_sub(*at) <= MAX_AGE_MS).then(|| info.clone())
}
