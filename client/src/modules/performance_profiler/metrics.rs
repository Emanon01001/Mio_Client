//! Bounded collectors. Recording never waits for the UI or allocates memory.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

const CAPACITY: usize = 512;
const MODULE_BASE: usize = 24;
static SESSION: AtomicU64 = AtomicU64::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Stage {
    Overlay,
    ModuleLock,
    NativeSample,
    EntitySnapshot,
    BlockScan,
    SurfaceScan,
    LightScan,
    ActorSample,
    ItemSample,
    StatusSample,
    SkinSample,
    LookSample,
    FlightSample,
    WorldSample,
    EspDraw,
    BlockDraw,
    CompassDraw,
    KeysDraw,
    ChunksDraw,
    WaypointsDraw,
    MenuDraw,
    ConsoleDraw,
    HudDraw,
    NotificationsDraw,
}
const STAGES: [(&str, &str, bool); MODULE_BASE] = [
    ("Overlay", "全体", true),
    ("Modules lock", "待機", false),
    ("Game sampling", "全体", true),
    ("Entity snapshot", "出力件数", false),
    ("BlockESP", "ブロック読取", false),
    ("Minimap terrain", "ブロック読取", false),
    ("LightOverlay", "走査列数", false),
    ("Actor facts", "取得", false),
    ("Inventory", "取得", false),
    ("Effects / Air", "取得", false),
    ("Skins", "取得", false),
    ("Target block", "取得", false),
    ("Trajectories", "取得", false),
    ("World info", "取得", false),
    ("ESP", "描画", false),
    ("BlockESP", "描画", false),
    ("Compass", "描画", false),
    ("Keystrokes", "描画", false),
    ("ChunkBorder", "描画", false),
    ("Waypoints", "描画", false),
    ("Menu / HUD editor", "描画", false),
    ("Console", "描画", false),
    ("HUD list", "描画", false),
    ("Notifications", "描画", false),
];

#[derive(Clone)]
pub(crate) struct Label {
    pub name: String,
    pub phase: &'static str,
    pub inclusive: bool,
}
#[derive(Clone)]
pub(crate) struct Row {
    pub label: Label,
    pub calls: u64,
    pub total_ns: u64,
    pub max_ns: u64,
    pub units: u64,
}
impl Row {
    pub fn average_ms(&self) -> f64 {
        self.total_ns as f64 / self.calls.max(1) as f64 / 1e6
    }
}
#[derive(Default, Clone)]
pub(crate) struct Snapshot {
    pub rows: Vec<Row>,
    pub seconds: f64,
    pub dropped: u64,
    pub omitted: usize,
}
#[derive(Default)]
struct Bucket {
    epoch: u64,
    calls: u64,
    total_ns: u64,
    max_ns: u64,
    units: u64,
}
impl Bucket {
    fn add(&mut self, epoch: u64, ns: u64, units: u64) {
        if self.epoch != epoch {
            *self = Self {
                epoch,
                ..Self::default()
            };
        }
        self.calls = self.calls.saturating_add(1);
        self.total_ns = self.total_ns.saturating_add(ns);
        self.max_ns = self.max_ns.max(ns);
        self.units = self.units.saturating_add(units);
    }
}
struct Recorder {
    slots: [Mutex<Bucket>; CAPACITY],
    labels: Mutex<Vec<Label>>,
    published: Mutex<Arc<Snapshot>>,
    dropped: AtomicU64,
    omitted: AtomicU64,
}
impl Recorder {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| Mutex::new(Bucket::default())),
            labels: Mutex::new(Vec::new()),
            published: Mutex::new(Arc::new(Snapshot::default())),
            dropped: AtomicU64::new(0),
            omitted: AtomicU64::new(0),
        }
    }
    fn record(&self, session: &AtomicU64, epoch: u64, slot: usize, ns: u64, units: u64) {
        let Some(slot) = self.slots.get(slot) else {
            return;
        };
        // The critical section contains arithmetic only. On contention drop
        // the measurement, never stall a game/render callback.
        if let Ok(mut b) = slot.try_lock() {
            if epoch != 0 && session.load(Ordering::Acquire) == epoch {
                b.add(epoch, ns, units);
            }
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn take(&self, epoch: u64, seconds: f64) -> Snapshot {
        let labels = self.labels.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows = Vec::with_capacity(labels.len());
        for (slot, label) in self.slots.iter().zip(labels.iter()) {
            // Only the worker takes a blocking lock; recording uses try_lock.
            let mut b = slot.lock().unwrap_or_else(|e| e.into_inner());
            if b.epoch == epoch && b.calls != 0 {
                rows.push(Row {
                    label: label.clone(),
                    calls: b.calls,
                    total_ns: b.total_ns,
                    max_ns: b.max_ns,
                    units: b.units,
                });
            }
            *b = Bucket::default();
        }
        Snapshot {
            rows,
            seconds: seconds.max(0.001),
            dropped: self.dropped.swap(0, Ordering::Relaxed),
            omitted: self.omitted.load(Ordering::Relaxed) as usize,
        }
    }
}
static RECORDER: LazyLock<Recorder> = LazyLock::new(Recorder::new);

pub(crate) fn configure<'a>(names: impl Iterator<Item = &'a str>) {
    stop();
    let mut labels = RECORDER.labels.lock().unwrap_or_else(|e| e.into_inner());
    labels.clear();
    labels.extend(STAGES.iter().map(|&(name, phase, inclusive)| Label {
        name: name.into(),
        phase,
        inclusive,
    }));
    let mut omitted = 0;
    for name in names {
        for phase in ["更新", "描画"] {
            if labels.len() < CAPACITY {
                labels.push(Label {
                    name: crate::localization::module_name(name).into(),
                    phase,
                    inclusive: false,
                });
            } else {
                omitted += 1;
            }
        }
    }
    RECORDER.omitted.store(omitted, Ordering::Relaxed);
    *RECORDER.published.lock().unwrap_or_else(|e| e.into_inner()) = Arc::default();
}
pub(crate) fn start() {
    stop();
    *RECORDER.published.lock().unwrap_or_else(|e| e.into_inner()) = Arc::default();
    let epoch = GENERATION
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
        .max(1);
    RECORDER.dropped.store(0, Ordering::Relaxed);
    SESSION.store(epoch, Ordering::Release);
}
pub(crate) fn stop() {
    SESSION.store(0, Ordering::Release);
}
pub(crate) fn active() -> bool {
    SESSION.load(Ordering::Relaxed) != 0
}
pub(crate) fn snapshot() -> Arc<Snapshot> {
    Arc::clone(&RECORDER.published.lock().unwrap_or_else(|e| e.into_inner()))
}
pub(crate) fn refresh(seconds: f64) {
    let epoch = SESSION.load(Ordering::Acquire);
    if epoch == 0 {
        return;
    }
    let next = RECORDER.take(epoch, seconds);
    if SESSION.load(Ordering::Acquire) == epoch {
        *RECORDER.published.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(next);
    }
}
fn frequency() -> u64 {
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut frequency = 0;
        unsafe {
            windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency);
        }
        frequency.max(0) as u64
    })
}
fn elapsed_ns(start: (i64, usize), end: (i64, usize), frequency: u64) -> Option<u64> {
    if start.1 != end.1 || frequency == 0 {
        return None;
    }
    let ticks = end.0.checked_sub(start.0)?;
    let ticks = u64::try_from(ticks).ok()?;
    Some(((ticks as u128 * 1_000_000_000) / frequency as u128).min(u64::MAX as u128) as u64)
}

pub(crate) struct Scope {
    start: Option<(i64, usize)>,
    epoch: u64,
    slot: usize,
    units: u64,
}
impl Scope {
    pub(crate) fn new(stage: Stage) -> Self {
        Self::slot(stage as usize)
    }
    pub(crate) fn module(index: usize, draw: bool) -> Self {
        Self::slot(
            MODULE_BASE
                .saturating_add(index.saturating_mul(2))
                .saturating_add(draw as usize),
        )
    }
    fn slot(slot: usize) -> Self {
        let epoch = SESSION.load(Ordering::Acquire);
        Self {
            start: if epoch != 0 && slot < CAPACITY && frequency() != 0 {
                super::super::timer::profile_counter()
            } else {
                None
            },
            epoch,
            slot,
            units: 0,
        }
    }
    pub(crate) fn units(&mut self, count: usize) {
        self.units = count as u64;
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        let Some(start) = self.start else { return };
        if SESSION.load(Ordering::Acquire) != self.epoch {
            return;
        }
        let Some(end) = super::super::timer::profile_counter() else {
            return;
        };
        let Some(ns) = elapsed_ns(start, end, frequency()) else {
            return;
        };
        RECORDER.record(&SESSION, self.epoch, self.slot, ns, self.units);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recorder() -> Recorder {
        let r = Recorder::new();
        r.labels.lock().unwrap().push(Label {
            name: "test".into(),
            phase: "更新",
            inclusive: false,
        });
        r
    }
    #[test]
    fn aggregates_exact_totals_and_window_reset() {
        let r = recorder();
        let session = AtomicU64::new(1);
        r.record(&session, 1, 0, 1_000_000, 40);
        r.record(&session, 1, 0, 3_000_000, 60);
        let s = r.take(1, 2.);
        let row = &s.rows[0];
        assert_eq!(
            (row.calls, row.total_ns, row.max_ns, row.units),
            (2, 4_000_000, 3_000_000, 100)
        );
        assert_eq!(row.average_ms(), 2.);
        assert_eq!(s.seconds, 2.);
        assert!(r.take(1, 1.).rows.is_empty());
    }
    #[test]
    fn old_inflight_spans_and_old_buckets_do_not_leak_after_reset() {
        let r = recorder();
        let session = AtomicU64::new(1);
        r.record(&session, 1, 0, 50, 10);
        session.store(2, Ordering::Release);
        assert!(r.take(2, 1.).rows.is_empty());
        r.record(&session, 2, 0, 100, 20);
        r.record(&session, 1, 0, 9999, 99);
        let s = r.take(2, 1.);
        assert_eq!((s.rows[0].calls, s.rows[0].total_ns), (1, 100));
    }
    #[test]
    fn contended_collectors_drop_instead_of_waiting_and_are_bounded() {
        let r = recorder();
        let session = AtomicU64::new(1);
        let guard = r.slots[0].lock().unwrap();
        r.record(&session, 1, 0, 100, 1);
        drop(guard);
        r.record(&session, 1, CAPACITY, 100, 1);
        let s = r.take(1, 1.);
        assert_eq!(s.dropped, 1);
        assert!(s.rows.is_empty());
    }
    #[test]
    fn counter_conversion_rejects_source_changes_backwards_time_and_overflow() {
        assert_eq!(elapsed_ns((100, 1), (120, 1), 10_000_000), Some(2000));
        assert_eq!(elapsed_ns((100, 0), (120, 1), 10), None);
        assert_eq!(elapsed_ns((100, 1), (90, 1), 10), None);
        assert_eq!(elapsed_ns((i64::MIN, 1), (i64::MAX, 1), 10), None);
        assert_eq!(elapsed_ns((0, 1), (1, 1), 0), None);
    }
    #[test]
    fn concurrent_writers_preserve_accounting_even_with_dropped_samples() {
        let r = recorder();
        let session = AtomicU64::new(1);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let r = &r;
                let session = &session;
                scope.spawn(move || {
                    for _ in 0..10_000 {
                        r.record(session, 1, 0, 10, 2);
                    }
                });
            }
        });
        let s = r.take(1, 1.);
        let row = &s.rows[0];
        assert_eq!(row.calls + s.dropped, 40_000);
        assert_eq!((row.total_ns, row.units), (row.calls * 10, row.calls * 2));
    }

    #[test]
    #[ignore = "manual optimized overhead measurement; changes global profiler session"]
    fn profiler_scope_overhead() {
        configure(std::iter::empty());
        const N: usize = 200_000;
        let begin = std::time::Instant::now();
        for _ in 0..N {
            std::hint::black_box(Scope::new(Stage::ActorSample));
        }
        let off = begin.elapsed();
        start();
        let begin = std::time::Instant::now();
        for _ in 0..N {
            std::hint::black_box(Scope::new(Stage::ActorSample));
        }
        let on = begin.elapsed();
        refresh(on.as_secs_f64());
        stop();
        let result = snapshot();
        let row = result
            .rows
            .iter()
            .find(|r| r.label.name == "Actor facts")
            .unwrap();
        assert_eq!(row.calls, N as u64);
        assert_eq!(result.dropped, 0);
        eprintln!("profiler scope: off={:.2} ns on={:.2} ns, {} samples; includes two raw counter reads and recording",
            off.as_nanos() as f64/N as f64, on.as_nanos() as f64/N as f64, row.calls);
    }
}
