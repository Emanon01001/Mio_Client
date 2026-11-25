//! One owned job, serviced by the existing client worker outside Modules' lock.
use super::{storage, surface};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

pub(super) enum Action {
    Save {
        label: String,
        terrain: storage::Terrain,
    },
    Load {
        filename: String,
        owner: (usize, usize, u64),
        dimension: String,
        world_id: Option<String>,
    },
    /// The remembered terrain as a PNG image.
    ExportPng {
        label: String,
        chunks: Vec<surface::SavedChunk>,
    },
    Refresh,
}
struct Job {
    directory: PathBuf,
    action: Action,
}
static QUEUE: Mutex<Option<Job>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);
static STATUS: Mutex<String> = Mutex::new(String::new());
static FILES: LazyLock<Mutex<Arc<Vec<storage::Entry>>>> =
    LazyLock::new(|| Mutex::new(Arc::new(Vec::new())));

pub(super) fn submit(directory: PathBuf, action: Action) -> Result<(), String> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if STOPPED.load(Ordering::Acquire) {
        return Err("クライアントを終了しています".into());
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        return Err("保存・読み込み処理中です".into());
    }
    *queue = Some(Job { directory, action });
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = "処理中…".into();
    Ok(())
}
pub(super) fn state() -> (bool, String, Arc<Vec<storage::Entry>>) {
    (
        BUSY.load(Ordering::Acquire),
        STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        Arc::clone(&FILES.lock().unwrap_or_else(|e| e.into_inner())),
    )
}
fn execute(directory: &std::path::Path, action: Action) -> Result<String, String> {
    match action {
        Action::Save { label, terrain } => {
            let path = storage::save(directory, &label, &terrain)?;
            Ok(format!("保存しました: {}", path.display()))
        }
        Action::Load {
            filename,
            owner,
            dimension,
            world_id,
        } => {
            let terrain = storage::load(directory, &filename)?;
            storage::compatible(&terrain, &dimension, world_id.as_deref())?;
            let added = surface::restore(owner, terrain.chunks)?;
            Ok(format!(
                "地形を読み込みました: {added}列を追加（現在の地形を優先）"
            ))
        }
        Action::ExportPng { label, chunks } => {
            let (path, picture) = storage::export_png(directory, &label, &chunks)?;
            let scale = match picture.step {
                1 => "1ブロック=1px".to_string(),
                n => format!("{n}ブロック=1px（広すぎるため縮小）"),
            };
            Ok(format!(
                "PNGを書き出しました（{}x{}, {scale}, 左上 X {} Z {}）: {}",
                picture.width,
                picture.height,
                picture.origin[0],
                picture.origin[1],
                path.display()
            ))
        }
        Action::Refresh => Ok("保存一覧を更新しました".into()),
    }
}
pub(super) fn poll() {
    let Some(job) = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let result = execute(&job.directory, job.action);
    let mut text = result.unwrap_or_else(|e| format!("処理できませんでした: {e}"));
    match storage::list(&job.directory) {
        Ok(files) => *FILES.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(files),
        Err(e) => text.push_str(&format!(" / 一覧の取得に失敗: {e}")),
    }
    crate::log!("WorldMap: {text}");
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = text;
    BUSY.store(false, Ordering::Release);
}
pub(super) fn shutdown() {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    STOPPED.store(true, Ordering::Release);
    queue.take();
    BUSY.store(false, Ordering::Release);
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *FILES.lock().unwrap_or_else(|e| e.into_inner()) = Arc::default();
}
