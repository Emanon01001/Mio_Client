//! Portable, bounded terrain archives. Worker-only I/O.
use super::surface::{Column, SavedChunk};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"MIOMAP01";
pub(super) const MAX_CHUNKS: usize = 8192;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const RECORD: usize = 8 + 256 * 6;

pub(super) struct Terrain {
    pub dimension: String,
    pub world_id: Option<String>,
    pub chunks: Vec<SavedChunk>,
}
pub(super) struct Entry {
    pub name: String,
    pub bytes: u64,
}
fn text_valid(text: &str) -> bool {
    text.len() <= 256 && !text.chars().any(char::is_control)
}
pub(super) fn validate(t: &Terrain) -> Result<(), String> {
    if t.dimension.is_empty()
        || !text_valid(&t.dimension)
        || !text_valid(t.world_id.as_deref().unwrap_or(""))
        || t.chunks.is_empty()
        || t.chunks.len() > MAX_CHUNKS
    {
        return Err("地形ファイルのヘッダーが不正です".into());
    }
    let mut positions = HashSet::new();
    for chunk in &t.chunks {
        if chunk.position.iter().any(|v| v.unsigned_abs() > 1_875_000)
            || !positions.insert(chunk.position)
            || !chunk.columns.iter().any(|c| c.known)
        {
            return Err("地形ファイルのチャンク座標・内容が不正です".into());
        }
    }
    Ok(())
}
fn write_text(out: &mut impl Write, text: &str) -> Result<(), String> {
    out.write_all(&(text.len() as u16).to_le_bytes())
        .map_err(|e| e.to_string())?;
    out.write_all(text.as_bytes()).map_err(|e| e.to_string())
}
pub(super) fn encode(out: &mut impl Write, t: &Terrain) -> Result<(), String> {
    validate(t)?;
    out.write_all(MAGIC).map_err(|e| e.to_string())?;
    write_text(out, &t.dimension)?;
    write_text(out, t.world_id.as_deref().unwrap_or(""))?;
    out.write_all(&(t.chunks.len() as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    let mut ordered: Vec<_> = t.chunks.iter().collect();
    ordered.sort_unstable_by_key(|c| c.position);
    // One write per chunk record rather than per field (8000 chunks would be
    // about four million small writes).
    let mut record = [0u8; RECORD];
    for chunk in ordered {
        record[0..4].copy_from_slice(&chunk.position[0].to_le_bytes());
        record[4..8].copy_from_slice(&chunk.position[1].to_le_bytes());
        for (c, field) in chunk.columns.iter().zip(record[8..].as_chunks_mut::<6>().0) {
            let c = if c.known { *c } else { Column::default() };
            field[0..2].copy_from_slice(&c.height.to_le_bytes());
            field[2..6].copy_from_slice(&[c.color[0], c.color[1], c.color[2], u8::from(c.known)]);
        }
        out.write_all(&record).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn take<const N: usize>(input: &mut Cursor<&[u8]>) -> Result<[u8; N], String> {
    let mut bytes = [0; N];
    input
        .read_exact(&mut bytes)
        .map_err(|_| "地形ファイルが途中で切れています")?;
    Ok(bytes)
}
fn read_text(input: &mut Cursor<&[u8]>) -> Result<String, String> {
    let length = u16::from_le_bytes(take(input)?) as usize;
    if length > 256 {
        return Err("地形ファイルの文字列が長すぎます".into());
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|_| "地形ファイルが途中で切れています")?;
    let text = String::from_utf8(bytes).map_err(|_| "地形ファイルの文字コードが不正です")?;
    if !text_valid(&text) {
        return Err("地形ファイルの文字列が不正です".into());
    }
    Ok(text)
}
pub(super) fn decode(bytes: &[u8]) -> Result<Terrain, String> {
    if bytes.len() > MAX_BYTES {
        return Err("地形ファイルが16MiBの上限を超えています".into());
    }
    let mut input = Cursor::new(bytes);
    if &take::<8>(&mut input)? != MAGIC {
        return Err("未対応の地形ファイル形式です".into());
    }
    let dimension = read_text(&mut input)?;
    let id = read_text(&mut input)?;
    let count = u32::from_le_bytes(take(&mut input)?) as usize;
    if count == 0
        || count > MAX_CHUNKS
        || bytes.len().checked_sub(input.position() as usize) != count.checked_mul(RECORD)
    {
        return Err("地形ファイルのサイズ・チャンク数が不正です".into());
    }
    // The size was checked above: whole records follow the header.
    let body = &bytes[input.position() as usize..];
    let mut chunks = Vec::with_capacity(count);
    for record in body.as_chunks::<RECORD>().0 {
        let position = [
            i32::from_le_bytes(record[0..4].try_into().unwrap()),
            i32::from_le_bytes(record[4..8].try_into().unwrap()),
        ];
        let mut columns = [Column::default(); 256];
        for (column, field) in columns.iter_mut().zip(record[8..].as_chunks::<6>().0) {
            let known = field[5];
            if known > 1 {
                return Err("地形ファイルのフラグが不正です".into());
            }
            if known == 1 {
                *column = Column {
                    height: i16::from_le_bytes([field[0], field[1]]),
                    color: [field[2], field[3], field[4]],
                    known: true,
                    remembered: false,
                };
            }
        }
        chunks.push(SavedChunk {
            position,
            columns: Arc::new(columns),
        });
    }
    let terrain = Terrain {
        dimension,
        world_id: (!id.is_empty()).then_some(id),
        chunks,
    };
    validate(&terrain)?;
    Ok(terrain)
}
pub(super) fn compatible(
    saved: &Terrain,
    dimension: &str,
    world_id: Option<&str>,
) -> Result<(), String> {
    if saved.dimension != dimension {
        return Err(format!(
            "ディメンションが異なります（保存: {} / 現在: {dimension}）",
            saved.dimension
        ));
    }
    if let (Some(saved), Some(current)) = (saved.world_id.as_deref(), world_id) {
        if saved != current {
            return Err("保存した地形と現在のワールドIDが異なります".into());
        }
    }
    Ok(())
}
fn safe_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .take(64)
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "world".into()
    } else {
        clean
    }
}
pub(super) fn output(
    directory: &Path,
    label: &str,
    extension: &str,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<(), String>,
) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let path = directory.join(format!(
        "map_{}_{now}_{}.{extension}",
        safe_name(label),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut out = BufWriter::new(file);
    let result = write(&mut out)
        .and_then(|_| out.flush().map_err(|e| e.to_string()))
        .and_then(|_| out.get_ref().sync_all().map_err(|e| e.to_string()));
    drop(out);
    if let Err(e) = result {
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}
pub(super) fn save(directory: &Path, label: &str, t: &Terrain) -> Result<PathBuf, String> {
    validate(t)?;
    output(directory, label, "miomap", |out| encode(out, t))
}
pub(super) fn load(directory: &Path, filename: &str) -> Result<Terrain, String> {
    if Path::new(filename).components().count() != 1
        || !filename.ends_with(".miomap")
        || filename.contains(['/', '\\', ':'])
    {
        return Err("地形ファイル名が不正です".into());
    }
    decode(&read_bytes(&directory.join(filename))?)
}

pub(super) fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}
pub(super) fn list(directory: &Path) -> Result<Vec<Entry>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut files = Vec::new();
    for entry in entries.take(4096) {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || entry.path().extension().is_none_or(|e| e != "miomap")
        {
            continue;
        }
        let meta = entry.metadata().map_err(|e| e.to_string())?;
        files.push((
            meta.modified().ok(),
            Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes: meta.len(),
            },
        ));
    }
    files.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.1.name.cmp(&a.1.name)));
    Ok(files.into_iter().take(128).map(|(_, e)| e).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn terrain() -> Terrain {
        let mut first = [Column::default(); 256];
        first[0] = Column {
            height: -64,
            color: [100, 140, 160],
            known: true,
            remembered: false,
        };
        first[255] = Column {
            height: 320,
            color: [220, 60, 10],
            known: true,
            remembered: true,
        };
        let mut second = [Column::default(); 256];
        second[0] = Column {
            height: 70,
            color: [40, 80, 120],
            known: true,
            remembered: false,
        };
        Terrain {
            dimension: "minecraft:overworld".into(),
            world_id: Some("world_id+-=".into()),
            chunks: vec![
                SavedChunk {
                    position: [-1, -1],
                    columns: Arc::new(first),
                },
                SavedChunk {
                    position: [1, 0],
                    columns: Arc::new(second),
                },
            ],
        }
    }
    #[test]
    fn archive_round_trip_preserves_coordinates_colors_heights_and_unknown_columns() {
        let mut t = terrain();
        let mut bytes = Vec::new();
        encode(&mut bytes, &t).unwrap();
        let loaded = decode(&bytes).unwrap();
        assert_eq!(loaded.dimension, t.dimension);
        assert_eq!(loaded.world_id, t.world_id);
        assert_eq!(loaded.chunks.len(), 2);
        assert_eq!(loaded.chunks[0].position, [-1, -1]);
        assert_eq!(loaded.chunks[0].columns[0], t.chunks[0].columns[0]);
        assert_eq!(loaded.chunks[0].columns[255].height, 320);
        assert_eq!(loaded.chunks[0].columns[255].color, [220, 60, 10]);
        assert!(!loaded.chunks[0].columns[255].remembered);
        assert_eq!(loaded.chunks[0].columns[1], Column::default());
        assert!(compatible(&loaded, "minecraft:overworld", Some("world_id+-=")).is_ok());
        assert!(compatible(&loaded, "minecraft:nether", Some("world_id+-=")).is_err());
        assert!(compatible(&loaded, "minecraft:overworld", Some("other")).is_err());
        t.world_id = None;
        bytes.clear();
        encode(&mut bytes, &t).unwrap();
        assert!(decode(&bytes).unwrap().world_id.is_none());
    }
    #[test]
    fn malformed_archives_are_rejected_before_large_allocations_or_mutation() {
        let mut bytes = Vec::new();
        encode(&mut bytes, &terrain()).unwrap();
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err(), "truncation at {n}");
        }
        let mut bad = bytes.clone();
        bad[0] = b'?';
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad.push(0);
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad[10] = 0;
        assert!(decode(&bad).is_err());
        let header = bytes.len() - 2 * RECORD;
        let mut bad = bytes.clone();
        bad[header - 4..header].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad[header + 13] = 2;
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad[header..header + 4].copy_from_slice(&i32::MIN.to_le_bytes());
        assert!(decode(&bad).is_err());
        let first = bytes[header..header + RECORD].to_vec();
        let mut bad = bytes.clone();
        bad[header + RECORD..].copy_from_slice(&first);
        assert!(decode(&bad).is_err());
        assert!(decode(&vec![0; MAX_BYTES + 1]).is_err());
    }
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "mio-map-test-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(&self.0) {
                for e in entries.flatten() {
                    let _ = fs::remove_file(e.path());
                }
            }
            let _ = fs::remove_dir(&self.0);
        }
    }
    #[test]
    fn disk_round_trip_is_bounded_to_the_output_directory_and_keeps_existing_files() {
        let dir = Directory::new();
        fs::write(dir.0.join("keep.miomap"), b"keep").unwrap();
        for label in ["../../CON", "日本語の地図", "NUL", "a\\b:c", "", "../"] {
            let path = save(&dir.0, label, &terrain()).unwrap();
            assert_eq!(path.parent(), Some(dir.0.as_path()));
            let name = path.file_name().unwrap().to_str().unwrap();
            let loaded = load(&dir.0, name).unwrap();
            assert_eq!(loaded.chunks.len(), 2);
        }
        assert_eq!(fs::read(dir.0.join("keep.miomap")).unwrap(), b"keep");
        assert_eq!(list(&dir.0).unwrap().len(), 7);
        for name in [
            "../outside.miomap",
            "C:\\outside.miomap",
            "a/b.miomap",
            "keep.png",
        ] {
            assert!(load(&dir.0, name).is_err());
        }
        let before = list(&dir.0).unwrap().len();
        assert!(output(&dir.0, "failed", "miomap", |out| {
            out.write_all(b"partial").unwrap();
            Err("test failure".into())
        })
        .is_err());
        assert_eq!(list(&dir.0).unwrap().len(), before);
    }

    #[test]
    #[ignore = "manual: set MCBE_MAP_FIXTURE to a .miomap file to validate it read-only"]
    fn map_file_check() {
        let path = PathBuf::from(std::env::var_os("MCBE_MAP_FIXTURE").expect("MCBE_MAP_FIXTURE"));
        let terrain = load(
            path.parent().unwrap(),
            path.file_name().unwrap().to_str().unwrap(),
        )
        .unwrap();
        eprintln!(
            "Map archive: dimension={} chunks={} known_columns={}",
            terrain.dimension,
            terrain.chunks.len(),
            terrain
                .chunks
                .iter()
                .map(|c| c.columns.iter().filter(|c| c.known).count())
                .sum::<usize>()
        );
    }
}
