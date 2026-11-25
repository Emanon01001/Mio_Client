//! Owned terrain handoffs and world-presence signals. No filesystem I/O here.
use crate::game::blocks::surface::SavedChunk;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) type Owner = (usize, usize, u64);
#[derive(Clone, Debug)]
pub(crate) struct Identity {
    pub id: Arc<str>,
    pub name: Arc<str>,
    pub level: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub id: Arc<str>,
    pub dimension: Arc<str>,
    pub registry: usize,
    pub session: u64,
}
#[derive(Clone)]
pub(crate) struct Context {
    pub owner: Owner,
    pub entry: Entry,
    pub name: Arc<str>,
    pub revision: u64,
    pub center: [i32; 2],
}
pub(crate) struct Snapshot {
    pub context: Context,
    pub chunks: Vec<SavedChunk>,
}
static RUNNING: AtomicBool = AtomicBool::new(false);
struct Presence {
    present: AtomicBool,
    session: AtomicU64,
    last: AtomicU64,
    since: AtomicU64,
}
impl Presence {
    const fn new() -> Self {
        Self {
            present: AtomicBool::new(false),
            session: AtomicU64::new(0),
            last: AtomicU64::new(0),
            since: AtomicU64::new(0),
        }
    }
    fn reset(&self) {
        self.present.store(false, Ordering::Release);
        self.since.store(0, Ordering::Release);
        self.last.store(0, Ordering::Release);
    }
    fn observe(&self, present: bool, now: u64) {
        if present {
            let since = self.since.swap(0, Ordering::AcqRel);
            if !self.present.swap(true, Ordering::AcqRel)
                && (since == 0 || now.saturating_sub(since) >= 250)
            {
                self.session.fetch_add(1, Ordering::AcqRel);
            }
            self.last.store(now, Ordering::Release);
        } else {
            self.present.store(false, Ordering::Release);
            let _ = self
                .since
                .compare_exchange(0, now.max(1), Ordering::AcqRel, Ordering::Acquire);
        }
    }
    fn absent(&self, now: u64) -> bool {
        let since = self.since.load(Ordering::Acquire);
        !self.present.load(Ordering::Acquire)
            && since != 0
            && now.saturating_sub(since) >= 250
            && self.last.load(Ordering::Acquire) <= since
    }
    fn idle(&self, now: u64) -> bool {
        let at = self.last.load(Ordering::Acquire);
        at != 0 && now.saturating_sub(at) >= 2000
    }
}
static PRESENCE: Presence = Presence::new();
static FINALS: Mutex<VecDeque<Snapshot>> = Mutex::new(VecDeque::new());
const MAX_FINALS: usize = 8;

pub(crate) fn running() -> bool {
    RUNNING.load(Ordering::Acquire)
}
pub(crate) fn set_running(on: bool) {
    RUNNING.store(on, Ordering::Release);
    if !on {
        PRESENCE.reset();
    }
}
pub(crate) fn observe_presence(present: bool, now: u64) {
    if !running() {
        return;
    }
    PRESENCE.observe(present, now);
}
pub(crate) fn session() -> u64 {
    PRESENCE.session.load(Ordering::Acquire)
}
pub(crate) fn absent(now: u64) -> bool {
    PRESENCE.absent(now)
}
pub(crate) fn idle(now: u64) -> bool {
    PRESENCE.idle(now)
}
/// On contention/full queue the producer keeps its old terrain and retries
/// the world switch next sample. Never discard an unsaved world's handoff.
pub(crate) fn retire(snapshot: Snapshot) -> bool {
    let Ok(mut queue) = FINALS.try_lock() else {
        return false;
    };
    if queue.len() >= MAX_FINALS {
        return false;
    }
    queue.push_back(snapshot);
    true
}
pub(crate) fn take_final() -> Option<Snapshot> {
    FINALS.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
}
pub(crate) fn return_final(snapshot: Snapshot) {
    FINALS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_front(snapshot);
}
pub(crate) fn clear() {
    FINALS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_empty_callbacks_do_not_restart_a_world_but_confirmed_exit_does() {
        let p = Presence::new();
        p.observe(true, 1000);
        let first = p.session.load(Ordering::Acquire);
        p.observe(false, 1010);
        assert!(!p.absent(1100));
        p.observe(true, 1100);
        assert_eq!(p.session.load(Ordering::Acquire), first);
        p.observe(false, 1200);
        assert!(p.absent(1500));
        p.observe(true, 1600);
        assert_eq!(p.session.load(Ordering::Acquire), first + 1);
        assert!(!p.absent(2000));
        p.reset();
        assert!(!p.idle(10_000));
        p.observe(true, 11_000);
        assert_eq!(p.session.load(Ordering::Acquire), first + 2);
    }
    #[test]
    fn a_pause_can_checkpoint_without_reporting_an_exit() {
        let p = Presence::new();
        p.observe(true, 1000);
        let session = p.session.load(Ordering::Acquire);
        assert!(!p.idle(2999));
        assert!(p.idle(3000));
        assert!(!p.absent(30_000));
        p.observe(true, 30_001);
        assert_eq!(p.session.load(Ordering::Acquire), session);
        assert!(!p.idle(30_001));
    }
}
