//! Isolated filesystem fixtures shared only by archive/coordinator tests.
use super::{
    storage::Terrain,
    surface::{Column, SavedChunk},
};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
pub(super) struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mio-autosave-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let root = std::env::temp_dir().canonicalize().unwrap();
        // Never remove a computed recursive path without checking its boundary.
        assert_eq!(self.0.parent(), Some(root.as_path()));
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("mio-autosave-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(super) fn chunk(position: [i32; 2], height: i16) -> SavedChunk {
    let mut columns = [Column::default(); 256];
    columns[0] = Column {
        height,
        color: [10, 80, 40],
        known: true,
        remembered: false,
    };
    SavedChunk {
        position,
        columns: Arc::new(columns),
    }
}
pub(super) fn terrain(id: &str, dimension: &str, height: i16) -> Terrain {
    Terrain {
        world_id: Some(id.into()),
        dimension: dimension.into(),
        chunks: vec![chunk([0, 0], height)],
    }
}
