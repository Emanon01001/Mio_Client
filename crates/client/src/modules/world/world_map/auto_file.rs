//! Fixed per-world archives, with bounded merging and replace-after-sync writes.
use super::storage::{self, Terrain};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Stable across process launches and Rust versions; embedded IDs also guard collisions.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
fn path(root: &Path, id: &str, dimension: &str) -> PathBuf {
    root.join("autosave")
        .join(format!("world_{:016x}", hash(id)))
        .join(format!("dimension_{:016x}.miomap", hash(dimension)))
}
fn scope(t: &Terrain, id: &str, dimension: &str) -> Result<(), String> {
    if t.world_id.as_deref() != Some(id) || t.dimension != dimension {
        Err("自動保存ファイルのワールドID・ディメンションが一致しません".into())
    } else {
        Ok(())
    }
}
/// A terrain file's bytes and their decoding (the bytes are valid then).
fn read_raw(path: &Path) -> Result<Option<(Vec<u8>, Terrain)>, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
        Ok(m) if !m.file_type().is_file() => return Err("通常の地形ファイルではありません".into()),
        _ => {}
    }
    let bytes = storage::read_bytes(path)?;
    let terrain = storage::decode(&bytes)?;
    Ok(Some((bytes, terrain)))
}
fn read(path: &Path) -> Result<Option<Terrain>, String> {
    read_raw(path).map(|found| found.map(|(_, t)| t))
}
/// The archive to merge into: the primary, or the backup if the primary is
/// missing or damaged; and whether the primary itself is valid (then it may
/// become the backup). A periodic save reads and decodes it only once.
fn read_pair_raw(
    path: &Path,
    id: &str,
    dimension: &str,
) -> Result<(Option<Terrain>, bool), String> {
    let first = read_raw(path);
    if let Ok(Some((_, t))) = first {
        scope(&t, id, dimension)?;
        return Ok((Some(t), true));
    }
    match read(&path.with_extension("bak")) {
        Ok(Some(t)) => {
            scope(&t, id, dimension)?;
            Ok((Some(t), false))
        }
        Ok(None) => first.map(|_| (None, false)),
        Err(e) => Err(format!("地形とバックアップを読み込めません: {e}")),
    }
}
fn directory(root: &Path, target: &Path, create: bool) -> Result<bool, String> {
    if create {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
    }
    let root = match root.canonicalize() {
        Ok(r) => r,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    let mut current = root.clone();
    for part in target
        .parent()
        .unwrap()
        .components()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        current.push(part.as_os_str());
        if create && !current.exists() {
            fs::create_dir(&current).map_err(|e| e.to_string())?;
        }
        match current.canonicalize() {
            Ok(c) if c.starts_with(&root) => {}
            Ok(_) => return Err("保存先がmapsフォルダーの外を参照しています".into()),
            Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(true)
}
pub(super) fn load(root: &Path, id: &str, dimension: &str) -> Result<Option<Terrain>, String> {
    let path = path(root, id, dimension);
    if !directory(root, &path, false)? {
        return Ok(None);
    }
    read_pair_raw(&path, id, dimension).map(|(terrain, _)| terrain)
}
fn merge(old: Option<Terrain>, new: &Terrain, center: [i32; 2]) -> Terrain {
    let mut chunks: HashMap<_, _> = old
        .into_iter()
        .flat_map(|t| t.chunks)
        .map(|c| (c.position, c.columns))
        .collect();
    for c in &new.chunks {
        match chunks.entry(c.position) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Arc::clone(&c.columns));
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let out = Arc::make_mut(e.get_mut());
                for (out, fresh) in out.iter_mut().zip(c.columns.iter()) {
                    if fresh.known {
                        *out = *fresh;
                    }
                }
            }
        }
    }
    let mut chunks: Vec<_> = chunks
        .into_iter()
        .map(|(position, columns)| super::surface::SavedChunk { position, columns })
        .collect();
    let center = center.map(|n| i64::from(n.div_euclid(16)));
    chunks.sort_unstable_by_key(|c| {
        let dx = i64::from(c.position[0]) - center[0];
        let dz = i64::from(c.position[1]) - center[1];
        (dx * dx + dz * dz, c.position)
    });
    chunks.truncate(storage::MAX_CHUNKS);
    Terrain {
        dimension: new.dimension.clone(),
        world_id: new.world_id.clone(),
        chunks,
    }
}
fn replace(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let wide = |p: &Path| -> Result<Vec<u16>, String> {
        let mut s: Vec<_> = p.as_os_str().encode_wide().collect();
        if s.contains(&0) {
            return Err("保存先にNUL文字があります".into());
        }
        s.push(0);
        Ok(s)
    };
    let from = wide(from)?;
    let to = wide(to)?;
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}
#[cfg(test)]
fn write_with(
    path: &Path,
    body: impl FnOnce(&mut std::io::BufWriter<fs::File>) -> Result<(), String>,
) -> Result<(), String> {
    let tmp = storage::output(path.parent().unwrap(), "autosave_pending", "tmp", body)?;
    let result = replace(&tmp, path);
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
#[cfg(test)]
fn write(path: &Path, terrain: &Terrain) -> Result<(), String> {
    write_with(path, |out| storage::encode(out, terrain))
}
/// Writes `terrain` as the new primary and keeps the old primary as the
/// backup when it is valid: the old file is renamed, not copied (the backup
/// was a full rewrite of up to 16 MiB on every periodic save). Order: the new
/// data is complete in a temporary file first; if the final rename fails, the
/// old primary is moved back. A crash between the renames leaves a valid
/// backup and no primary, which `read_pair` falls back to.
fn write_rotating(path: &Path, terrain: &Terrain, keep_backup: bool) -> Result<(), String> {
    let tmp = storage::output(path.parent().unwrap(), "autosave_pending", "tmp", |out| {
        storage::encode(out, terrain)
    })?;
    let backup = path.with_extension("bak");
    let result = if keep_backup {
        replace(path, &backup).and_then(|()| {
            replace(&tmp, path).inspect_err(|_| {
                let _ = replace(&backup, path);
            })
        })
    } else {
        replace(&tmp, path)
    };
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
pub(super) fn save(root: &Path, terrain: &Terrain, center: [i32; 2]) -> Result<PathBuf, String> {
    storage::validate(terrain)?;
    let id = terrain
        .world_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("ワールドIDが未取得です")?;
    let path = path(root, id, &terrain.dimension);
    directory(root, &path, true)?;
    // The primary is read and decoded once. Never replace a valid backup with
    // a corrupt primary: only a valid primary becomes the backup.
    let (old, primary_valid) = read_pair_raw(&path, id, &terrain.dimension)?;
    // A directory or link in place of the primary must not be overwritten.
    if let Ok(m) = fs::symlink_metadata(&path) {
        if !m.file_type().is_file() {
            return Err("自動保存先が通常のファイルではありません".into());
        }
    }
    let merged = merge(old, terrain, center);
    write_rotating(&path, &merged, primary_valid)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{chunk, terrain, Directory};
    use super::*;

    /// One periodic autosave of a large remembered map (on the client worker
    /// thread): the archive already exists, the session added a few chunks.
    #[test]
    #[ignore = "manual: cargo test -p client --release --lib autosave_cost -- --ignored --nocapture"]
    fn autosave_cost() {
        for count in [1000, 4000, 8000] {
            let dir = Directory::new();
            let mut t = terrain("A", "minecraft:overworld", 64);
            t.chunks = (0..count as i32)
                .map(|i| chunk([i % 90, i / 90], 64))
                .collect();
            save(&dir.0, &t, [0, 0]).unwrap();
            let mut times = Vec::new();
            for round in 0..7 {
                t.chunks.truncate(count - 1);
                t.chunks.push(chunk([round, 999], 70));
                let start = std::time::Instant::now();
                save(&dir.0, &t, [0, 0]).unwrap();
                times.push(start.elapsed());
            }
            times.sort();
            let size = std::fs::metadata(path(&dir.0, "A", "minecraft:overworld"))
                .unwrap()
                .len();
            println!(
                "autosave {count} chunks ({:.1} MiB): median {:?}, max {:?}",
                size as f64 / 1048576.,
                times[3],
                times[6]
            );
        }
    }
    const OVERWORLD: &str = "minecraft:overworld";
    #[test]
    fn separate_worlds_and_dimensions_restore_from_disk_after_restart() {
        let d = Directory::new();
        let a = terrain("../世界/A", OVERWORLD, 64);
        let b = terrain("別のID", OVERWORLD, 90);
        let nether = terrain("../世界/A", "minecraft:nether", 30);
        let pa = save(&d.0, &a, [0, 0]).unwrap();
        let pb = save(&d.0, &b, [0, 0]).unwrap();
        let pn = save(&d.0, &nether, [0, 0]).unwrap();
        assert_ne!(pa, pb);
        assert_ne!(pa, pn);
        assert!(pa.canonicalize().unwrap().starts_with(&d.0));
        for (id, dim, height) in [
            ("../世界/A", OVERWORLD, 64),
            ("別のID", OVERWORLD, 90),
            ("../世界/A", "minecraft:nether", 30),
        ] {
            let t = load(&d.0, id, dim).unwrap().unwrap();
            assert_eq!(t.chunks[0].columns[0].height, height);
        }
        assert!(load(&d.0, "unseen", OVERWORLD).unwrap().is_none());
    }
    #[test]
    fn replacement_merges_old_unknown_columns_but_new_observations_win() {
        let d = Directory::new();
        let mut a = terrain("A", OVERWORLD, 64);
        Arc::make_mut(&mut a.chunks[0].columns)[255] = a.chunks[0].columns[0];
        a.chunks.push(chunk([5, 5], 70));
        let path = save(&d.0, &a, [0, 0]).unwrap();
        let b = terrain("A", OVERWORLD, 80);
        assert_eq!(save(&d.0, &b, [0, 0]).unwrap(), path);
        let restored = load(&d.0, "A", OVERWORLD).unwrap().unwrap();
        assert_eq!(restored.chunks.len(), 2);
        let c = restored
            .chunks
            .iter()
            .find(|c| c.position == [0, 0])
            .unwrap();
        assert_eq!(c.columns[0].height, 80);
        assert_eq!(c.columns[255].height, 64);
        assert_eq!(
            read(&path.with_extension("bak")).unwrap().unwrap().chunks[0].columns[0].height,
            64
        );
        assert_eq!(
            fs::read_dir(path.parent().unwrap()).unwrap().count(),
            2,
            "no abandoned temporary files"
        );
    }
    #[test]
    fn rotation_keeps_the_previous_primary_and_recovers_from_a_crash_between_renames() {
        let d = Directory::new();
        let p = save(&d.0, &terrain("A", OVERWORLD, 64), [0, 0]).unwrap();
        save(&d.0, &terrain("A", OVERWORLD, 70), [0, 0]).unwrap();
        save(&d.0, &terrain("A", OVERWORLD, 80), [0, 0]).unwrap();
        let height = |path: &Path| read(path).unwrap().unwrap().chunks[0].columns[0].height;
        // The backup is always the previous valid primary.
        assert_eq!((height(&p), height(&p.with_extension("bak"))), (80, 70));
        // A crash after the old primary became the backup: no primary.
        fs::rename(&p, p.with_extension("bak")).unwrap();
        assert_eq!(
            load(&d.0, "A", OVERWORLD).unwrap().unwrap().chunks[0].columns[0].height,
            80
        );
        // The next save writes a primary and keeps that backup.
        save(&d.0, &terrain("A", OVERWORLD, 90), [0, 0]).unwrap();
        assert_eq!((height(&p), height(&p.with_extension("bak"))), (90, 80));
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 2);
    }

    #[test]
    fn corrupt_primary_recovers_backup_and_can_be_repaired_without_losing_backup() {
        let d = Directory::new();
        let first = terrain("A", OVERWORLD, 64);
        let p = save(&d.0, &first, [0, 0]).unwrap();
        save(&d.0, &terrain("A", OVERWORLD, 80), [0, 0]).unwrap();
        fs::write(&p, b"truncated").unwrap();
        assert_eq!(
            load(&d.0, "A", OVERWORLD).unwrap().unwrap().chunks[0].columns[0].height,
            64
        );
        save(&d.0, &terrain("A", OVERWORLD, 90), [0, 0]).unwrap();
        assert_eq!(
            load(&d.0, "A", OVERWORLD).unwrap().unwrap().chunks[0].columns[0].height,
            90
        );
        assert_eq!(
            read(&p.with_extension("bak")).unwrap().unwrap().chunks[0].columns[0].height,
            64
        );
        fs::write(&p, b"bad").unwrap();
        fs::write(p.with_extension("bak"), b"bad too").unwrap();
        assert!(save(&d.0, &first, [0, 0]).is_err());
        assert_eq!(fs::read(p).unwrap(), b"bad");
    }
    #[test]
    fn foreign_embedded_identity_is_never_loaded_or_overwritten() {
        let d = Directory::new();
        let a = terrain("A", OVERWORLD, 64);
        let p = save(&d.0, &a, [0, 0]).unwrap();
        write(&p, &terrain("B", OVERWORLD, 90)).unwrap();
        let bytes = fs::read(&p).unwrap();
        assert!(load(&d.0, "A", OVERWORLD).is_err());
        assert!(save(&d.0, &a, [0, 0]).is_err());
        assert_eq!(fs::read(p).unwrap(), bytes);
        assert!(save(
            &d.0,
            &Terrain {
                world_id: None,
                ..a
            },
            [0, 0]
        )
        .is_err());
    }
    #[test]
    fn failed_replacement_keeps_the_previous_file_and_cleans_its_temporary_file() {
        let d = Directory::new();
        let a = terrain("A", OVERWORLD, 64);
        let p = save(&d.0, &a, [0, 0]).unwrap();
        let bytes = fs::read(&p).unwrap();
        let backup = p.with_extension("bak");
        fs::create_dir(&backup).unwrap();
        assert!(save(&d.0, &terrain("A", OVERWORLD, 90), [0, 0]).is_err());
        assert_eq!(fs::read(&p).unwrap(), bytes);
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 2);
        fs::remove_dir(backup).unwrap();
        save(&d.0, &terrain("A", OVERWORLD, 90), [0, 0]).unwrap();
        assert_eq!(
            load(&d.0, "A", OVERWORLD).unwrap().unwrap().chunks[0].columns[0].height,
            90
        );
    }
    #[test]
    fn merged_archive_remains_bounded_and_keeps_the_nearest_chunks() {
        let mut old = terrain("A", OVERWORLD, 64);
        old.chunks = (0..storage::MAX_CHUNKS as i32)
            .map(|x| chunk([x, 0], 64))
            .collect();
        let new = Terrain {
            chunks: vec![chunk([-1, 0], 90)],
            ..terrain("A", OVERWORLD, 0)
        };
        let merged = merge(Some(old), &new, [0, 0]);
        assert_eq!(merged.chunks.len(), storage::MAX_CHUNKS);
        assert!(merged.chunks.iter().any(|c| c.position == [-1, 0]));
        assert!(!merged.chunks.iter().any(|c| c.position == [8191, 0]));
        storage::validate(&merged).unwrap();
    }
    #[test]
    fn stable_hash_has_a_fixed_known_value() {
        assert_eq!(hash(""), 0xcbf29ce484222325);
        assert_eq!(hash("hello"), 0xa430d84680aabd0b);
    }
}
