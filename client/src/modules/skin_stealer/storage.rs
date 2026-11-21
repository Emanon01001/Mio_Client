//! Bounded PNG encoding and collision-safe filenames. No native pointers.
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn encode(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if super::skins::rgba_len(width, height) != Some(rgba.len()) {
        return Err("画像サイズと画素数が一致しません".into());
    }
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(rgba))
            .map_err(|e| e.to_string())?;
    }
    Ok(encoded)
}
fn safe_name(name: &str) -> String {
    let name = crate::modules::chat_log::clean(name);
    let name: String = name
        .chars()
        .take(48)
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() {
        "player".into()
    } else {
        name.into()
    }
}
fn write_new(directory: &Path, stem: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    for suffix in 0..100 {
        let path = directory.join(format!("{stem}_{suffix}.png"));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        if let Err(e) = file.write_all(bytes).and_then(|_| file.flush()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(e.to_string());
        }
        return Ok(path);
    }
    Err("同名ファイルが多すぎます。時間をおいて再度保存してください".into())
}
pub(super) fn save(
    directory: &Path,
    name: &str,
    entity: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    let bytes = encode(width, height, rgba)?;
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    // Fixed prefix prevents Windows device names (CON, NUL, COM1, ...).
    write_new(
        directory,
        &format!("skin_{}_{entity:08X}_{time}", safe_name(name)),
        &bytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_round_trip_preserves_channels_and_alpha() {
        let pixels = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 12, 34, 56, 78];
        let png = encode(2, 2, &pixels).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut output = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut output).unwrap();
        assert_eq!(
            (info.width, info.height, info.color_type),
            (2, 2, png::ColorType::Rgba)
        );
        assert_eq!(&output[..info.buffer_size()], &pixels);
        assert!(encode(0, 1, &[]).is_err());
        assert!(encode(2, 2, &pixels[..12]).is_err());
        assert!(encode(u32::MAX, u32::MAX, &[]).is_err());
    }
    #[test]
    fn names_stay_in_export_directory_and_existing_files_survive() {
        for raw in [
            "../../CON",
            "a\\..\\b:*?",
            "§a日本 太郎",
            "..",
            "NUL",
            "\0\n",
        ] {
            let clean = safe_name(raw);
            assert!(!clean.is_empty());
            assert_eq!(
                Path::new(&format!("skin_{clean}.png")).components().count(),
                1
            );
            assert!(!clean
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)));
        }
        assert_eq!(safe_name("§a日本 太郎"), "日本 太郎");
        assert_eq!(safe_name(&"あ".repeat(100)).chars().count(), 48);
        let directory = std::env::temp_dir().join(format!(
            "mcbe_skin_export_test_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let a = write_new(&directory, "skin_test", b"first").unwrap();
        let b = write_new(&directory, "skin_test", b"second").unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read(a).unwrap(), b"first");
        assert_eq!(fs::read(b).unwrap(), b"second");
        fs::remove_dir_all(directory).unwrap();
    }
}
