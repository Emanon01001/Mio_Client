//! Serviced only by the existing client worker. Game callbacks publish owned
//! handoffs; UI only changes atomics / reads a short status string.
use super::{auto_file, game, storage, surface};
use game::terrain_session::{self as sessions, Context, Entry, Snapshot};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

static REQUESTED: AtomicBool = AtomicBool::new(true);
static INTERVAL: AtomicU64 = AtomicU64::new(30_000);
static FORCE: AtomicBool = AtomicBool::new(false);
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static STATUS: Mutex<String> = Mutex::new(String::new());
const POLL_MS: u64 = 250;
const RETRY_MS: u64 = 5000;

struct Visit {
    entry: Entry,
    restored: bool,
    saved_revision: Option<u64>,
    saved_at: u64,
}
struct Engine {
    directory: PathBuf,
    active: bool,
    poll_at: u64,
    retry_at: u64,
    current: Option<Visit>,
    last_save: String,
    saves: u64,
    error: String,
}
impl Engine {
    fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            active: false,
            poll_at: 0,
            retry_at: 0,
            current: None,
            last_save: String::new(),
            saves: 0,
            error: String::new(),
        }
    }
    fn visit(
        &mut self,
        context: &Context,
        now: u64,
        restore: impl FnOnce(Vec<surface::SavedChunk>) -> Result<usize, String>,
    ) -> Result<(), String> {
        if self
            .current
            .as_ref()
            .is_none_or(|v| v.entry != context.entry)
        {
            self.current = Some(Visit {
                entry: context.entry.clone(),
                restored: false,
                saved_revision: None,
                saved_at: now,
            });
        }
        let current = self.current.as_mut().unwrap();
        if !current.restored {
            if let Some(terrain) =
                auto_file::load(&self.directory, &context.entry.id, &context.entry.dimension)?
            {
                let count = restore(terrain.chunks)?;
                crate::log!(
                    "WorldMap: {} / {} の地形を自動復元しました（{count}列追加）",
                    context.name,
                    context.entry.dimension
                );
            }
            current.restored = true;
        }
        Ok(())
    }
    fn due(&self, context: &Context, now: u64, interval: u64, force: bool, idle: bool) -> bool {
        self.current.as_ref().is_some_and(|v| {
            v.entry == context.entry
                && v.restored
                && (force || v.saved_revision != Some(context.revision))
                && (force || idle || now.saturating_sub(v.saved_at) >= interval)
        })
    }
    fn save(&mut self, snapshot: &Snapshot, now: u64) -> Result<(), String> {
        let c = &snapshot.context;
        let terrain = storage::Terrain {
            dimension: c.entry.dimension.to_string(),
            world_id: Some(c.entry.id.to_string()),
            chunks: snapshot.chunks.clone(),
        };
        let path = auto_file::save(&self.directory, &terrain, c.center)?;
        self.saves += 1;
        self.last_save = format!(
            "最終保存: {} / {}チャンク / 保存{}回",
            c.name,
            terrain.chunks.len(),
            self.saves
        );
        if let Some(current) = self.current.as_mut().filter(|v| v.entry == c.entry) {
            current.saved_revision = Some(c.revision);
            current.saved_at = now;
        }
        // Periodic saves do not flood the game console.
        if FORCE.load(Ordering::Relaxed) {
            crate::log!("WorldMap: {}", path.display());
        }
        Ok(())
    }
    fn drain(&mut self, now: u64) -> Result<(), String> {
        while let Some(snapshot) = sessions::take_final() {
            if let Err(e) = self.save(&snapshot, now) {
                sessions::return_final(snapshot);
                return Err(e);
            }
        }
        Ok(())
    }
    fn finish(&mut self, now: u64) -> Result<(), String> {
        self.drain(now)?;
        surface::finish_auto()?;
        self.current = None;
        self.drain(now)
    }
    fn start(&mut self) -> Result<(), String> {
        super::blocks::available()?;
        game::world_info::prepare_identity()?;
        game::acquire()?;
        sessions::set_running(true);
        surface::set_active(true);
        self.active = true;
        Ok(())
    }
    fn stop(&mut self) {
        if !self.active {
            return;
        }
        sessions::set_running(false);
        surface::set_active(false);
        if let Err(e) = game::release() {
            crate::log!("WorldMap: 地形取得の解除: {e}");
        }
        self.active = false;
        self.current = None;
    }
    fn tick(&mut self, now: u64) -> Result<(), String> {
        if !REQUESTED.load(Ordering::Acquire) {
            if self.active {
                self.finish(now)?;
                self.stop();
            }
            return Ok(());
        }
        if !self.active {
            self.start()?;
        }
        // A failed final write can leave collection frozen. Keep its snapshot
        // queued until it is safely written before restarting collection.
        if !sessions::running() {
            self.drain(now)?;
            sessions::set_running(true);
        }
        surface::finish_absent(now);
        self.drain(now)?;
        let Some(context) = surface::auto_context() else {
            return Ok(());
        };
        self.visit(&context, now, |chunks| {
            surface::restore_auto(&context, chunks)
        })?;
        // Restore can change the revision; capture the current binding again.
        if let Some(c) = surface::auto_context() {
            if self.due(
                &c,
                now,
                INTERVAL.load(Ordering::Relaxed),
                FORCE.load(Ordering::Acquire),
                sessions::idle(now),
            ) {
                if let Some(snapshot) = surface::auto_snapshot(c.owner) {
                    self.save(&snapshot, now)?;
                    FORCE.store(false, Ordering::Release);
                }
            }
        }
        Ok(())
    }
    fn status(&self) -> String {
        let current = surface::auto_context();
        let mode = if !REQUESTED.load(Ordering::Acquire) {
            "自動保存OFF".to_owned()
        } else if let Some(c) = current {
            format!("{} / {} / ID: {}", c.name, c.entry.dimension, c.entry.id)
        } else {
            "ワールドID・地形情報を待機中（未取得の間は自動保存・復元しません）".to_owned()
        };
        let mut text = format!(
            "{mode}\n保存間隔: {}秒",
            INTERVAL.load(Ordering::Relaxed) / 1000
        );
        if !self.last_save.is_empty() {
            text.push('\n');
            text.push_str(&self.last_save);
        }
        if !self.error.is_empty() {
            text.push_str("\n保存・復元エラー（再試行します）: ");
            text.push_str(&self.error);
        }
        text
    }
}
pub(super) fn request(on: bool, seconds: i32) {
    REQUESTED.store(on, Ordering::Release);
    INTERVAL.store(seconds.clamp(5, 600) as u64 * 1000, Ordering::Relaxed);
}
pub(super) fn force() {
    FORCE.store(true, Ordering::Release);
}
pub(super) fn status() -> String {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
pub(super) fn start(root: &Path, cfg: &crate::config::WorldMap) {
    request(cfg.auto_save, cfg.save_interval);
    *ENGINE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Engine::new(root.join("maps")));
}
pub(super) fn poll() {
    let now = unsafe { GetTickCount64() };
    let mut guard = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut engine) = guard.take() else {
        return;
    };
    drop(guard);
    if now >= engine.poll_at && now >= engine.retry_at {
        engine.poll_at = now.saturating_add(POLL_MS);
        match engine.tick(now) {
            Ok(()) => engine.error.clear(),
            Err(e) => {
                if engine.error != e {
                    crate::log!("WorldMap 自動保存: {e}");
                }
                engine.error = e;
                engine.retry_at = now.saturating_add(RETRY_MS);
            }
        }
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = engine.status();
    }
    *ENGINE.lock().unwrap_or_else(|e| e.into_inner()) = Some(engine);
}
pub(super) fn shutdown() {
    let Some(mut engine) = ENGINE.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    if engine.active {
        if let Err(e) = engine.finish(unsafe { GetTickCount64() }) {
            crate::log!("WorldMap: 終了時の最終保存に失敗しました: {e}");
        }
        engine.stop();
    }
    sessions::clear();
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{chunk, Directory};
    use super::*;
    fn context(id: &str, dimension: &str, session: u64, revision: u64) -> Context {
        Context {
            owner: (1, 2, session),
            entry: Entry {
                id: id.into(),
                dimension: dimension.into(),
                registry: 1,
                session,
            },
            name: "同じワールド名".into(),
            revision,
            center: [0, 0],
        }
    }
    fn snapshot(context: Context, height: i16) -> Snapshot {
        Snapshot {
            context,
            chunks: vec![chunk([0, 0], height)],
        }
    }
    const OVERWORLD: &str = "minecraft:overworld";
    #[test]
    fn periodic_saves_skip_unchanged_data_and_idle_checkpoint_does_not_end_visit() {
        let dir = Directory::new();
        let mut e = Engine::new(dir.0.clone());
        let mut c = context("A", OVERWORLD, 1, 10);
        e.visit(&c, 1000, |_| panic!("no archive yet")).unwrap();
        assert!(!e.due(&c, 30_999, 30_000, false, false));
        assert!(e.due(&c, 31_000, 30_000, false, false));
        e.save(&snapshot(c.clone(), 64), 31_000).unwrap();
        assert!(!e.due(&c, 90_000, 30_000, false, false));
        assert!(e.due(&c, 90_000, 30_000, true, false));
        c.revision += 1;
        assert!(!e.due(&c, 31_500, 30_000, false, false));
        assert!(e.due(&c, 31_500, 30_000, false, true));
        e.save(&snapshot(c.clone(), 70), 31_500).unwrap();
        e.visit(&c, 32_000, |_| panic!("pausing must not reimport the map"))
            .unwrap();
        assert!(!e.due(&c, 95_000, 30_000, false, true));
    }
    #[test]
    fn final_save_then_restart_rejoins_the_correct_world_and_dimension_once() {
        let dir = Directory::new();
        let mut first = Engine::new(dir.0.clone());
        let a = context("A", OVERWORLD, 1, 10);
        let b = context("B", OVERWORLD, 2, 20);
        let n = context("A", "minecraft:nether", 3, 30);
        for (c, height) in [(a, 64), (b, 90), (n, 30)] {
            first.visit(&c, 1000, |_| panic!("first visit")).unwrap();
            first.save(&snapshot(c, height), 1500).unwrap();
        }
        drop(first);
        let mut restarted = Engine::new(dir.0.clone());
        for (c, height) in [
            (context("A", OVERWORLD, 5, 40), 64),
            (context("B", OVERWORLD, 6, 50), 90),
            (context("A", "minecraft:nether", 7, 60), 30),
        ] {
            restarted
                .visit(&c, 2000, |chunks| {
                    assert_eq!(chunks[0].columns[0].height, height);
                    Ok(1)
                })
                .unwrap();
            restarted
                .visit(&c, 2500, |_| panic!("restore only on entry"))
                .unwrap();
        }
    }
    #[test]
    fn failed_restore_is_not_marked_complete_and_cannot_trigger_periodic_overwrite() {
        let dir = Directory::new();
        let mut e = Engine::new(dir.0.clone());
        let c = context("A", OVERWORLD, 1, 10);
        e.save(&snapshot(c.clone(), 64), 1000).unwrap();
        assert!(e
            .visit(&c, 2000, |_| Err("binding changed during I/O".into()))
            .is_err());
        assert!(!e.due(&c, 95_000, 30_000, true, true));
        e.visit(&c, 96_000, |chunks| {
            assert_eq!(chunks[0].columns[0].height, 64);
            Ok(1)
        })
        .unwrap();
        assert!(e.due(&c, 96_000, 30_000, false, false));
    }
    #[test]
    fn failed_save_keeps_the_revision_dirty_until_a_successful_retry() {
        let dir = Directory::new();
        let root = dir.0.join("maps");
        let mut e = Engine::new(root.clone());
        let c = context("A", OVERWORLD, 1, 10);
        e.visit(&c, 1000, |_| panic!("empty")).unwrap();
        std::fs::write(&root, b"blocked directory").unwrap();
        let s = snapshot(c.clone(), 64);
        assert!(e.save(&s, 31_000).is_err());
        assert_eq!(std::fs::read(&root).unwrap(), b"blocked directory");
        assert!(e.due(&c, 36_000, 30_000, false, false));
        assert_eq!(e.saves, 0);
        std::fs::remove_file(&root).unwrap();
        e.save(&s, 36_000).unwrap();
        assert_eq!(e.saves, 1);
        assert!(!e.due(&c, 90_000, 30_000, false, false));
    }
    #[test]
    fn clearing_memory_does_not_reload_archive_until_a_new_world_visit() {
        let dir = Directory::new();
        let mut e = Engine::new(dir.0.clone());
        let mut c = context("A", OVERWORLD, 1, 10);
        e.visit(&c, 1000, |_| panic!("empty")).unwrap();
        e.save(&snapshot(c.clone(), 64), 2000).unwrap();
        c.owner.2 += 1; // Cache generation changed, world session unchanged.
        e.visit(&c, 3000, |_| panic!("manual cache clear must stay cleared"))
            .unwrap();
        c.entry.session += 1;
        e.visit(&c, 4000, |chunks| {
            assert_eq!(chunks[0].columns[0].height, 64);
            Ok(1)
        })
        .unwrap();
    }

    #[test]
    fn exit_handoffs_survive_io_failure_and_bound_memory_until_a_successful_retry() {
        // Only this test uses the global handoff queue; no native callbacks run
        // in a unit-test process, and other coordinator tests use owned visits.
        sessions::clear();
        let dir = Directory::new();
        let root = dir.0.join("maps");
        let mut engine = Engine::new(root.clone());
        for i in 0..8 {
            assert!(sessions::retire(snapshot(
                context(&format!("world-{i}"), OVERWORLD, i + 1, i + 10),
                64 + i as i16
            )));
        }
        assert!(!sessions::retire(snapshot(
            context("full", OVERWORLD, 9, 19),
            90
        )));
        std::fs::write(&root, b"temporarily blocked").unwrap();
        assert!(engine.drain(1000).is_err());
        assert_eq!(engine.saves, 0);
        std::fs::remove_file(&root).unwrap();
        engine.drain(6000).unwrap();
        assert_eq!(engine.saves, 8);
        for i in 0..8 {
            let restored = auto_file::load(&root, &format!("world-{i}"), OVERWORLD)
                .unwrap()
                .unwrap();
            assert_eq!(restored.chunks[0].columns[0].height, 64 + i as i16);
        }
        assert!(sessions::take_final().is_none());
    }
}
