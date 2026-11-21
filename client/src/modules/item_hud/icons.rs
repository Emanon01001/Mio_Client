//! Read vanilla assets once, then draw a single shared atlas. No game objects
//! or pointers reach the renderer and no copyrighted texture is embedded.
use super::archive;
use hudhook::{
    imgui::{DrawListMut, TextureId},
    RenderContext,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    io::{Cursor, Read},
    path::Path,
    sync::{Arc, Mutex},
};

const TILE: usize = 32;
const PITCH: usize = TILE + 2;
const MAX_IMAGES: usize = 4096;
const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = 32 * 1024 * 1024;
type Uv = [[f32; 2]; 2];
#[derive(Clone, Copy)]
pub(super) enum Icon {
    Flat(Uv),
    Cube { top: Uv, side: Uv, front: Uv },
}
pub(crate) struct Atlas {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    icons: HashMap<String, Icon>,
    names: HashMap<String, String>,
}
pub(super) struct Icons {
    texture: TextureId,
    icons: HashMap<String, Icon>,
    names: HashMap<String, String>,
}
static LOADED: Mutex<Option<Arc<Icons>>> = Mutex::new(None);

pub(crate) fn reset() {
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(super) fn current() -> Option<Arc<Icons>> {
    LOADED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
pub(crate) fn upload(atlas: Atlas, context: &mut dyn RenderContext) {
    reset();
    match context.load_texture(&atlas.pixels, atlas.width, atlas.height) {
        Ok(texture) => {
            crate::log!(
                "所持品HUD: アイテム画像を準備しました（対応ID {}件、{}×{}）",
                atlas.icons.len(),
                atlas.width,
                atlas.height
            );
            *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(Icons {
                texture,
                icons: atlas.icons,
                names: atlas.names,
            }));
        }
        Err(e) => crate::log!("所持品HUDの画像を転送できません: {e:?}"),
    }
}
pub(crate) fn load_game() -> Option<Atlas> {
    let root = std::env::current_exe()
        .ok()?
        .parent()?
        .join("data/resource_packs");
    match load(&root) {
        Ok(atlas) => Some(atlas),
        Err(e) => {
            crate::log!("所持品HUDの画像を読み込めません: {e}（文字表示を使用します）");
            None
        }
    }
}

#[derive(Default)]
struct Assets {
    images: BTreeMap<String, Vec<u8>>,
    image_bytes: usize,
    textures: HashMap<String, Value>,
    blocks: HashMap<String, Value>,
    items: HashMap<String, String>,
    names: HashMap<String, String>,
}
fn pack_version(name: &str) -> Option<Vec<u32>> {
    if name == "vanilla" {
        return Some(Vec::new());
    }
    name.strip_prefix("vanilla_")?
        .split('.')
        .map(|n| n.parse().ok())
        .collect()
}
fn read(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > 32 * 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= 32 * 1024 * 1024).then_some(bytes)
}
fn json(bytes: &[u8]) -> Option<Value> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if let Ok(value) = serde_json::from_slice(bytes) {
        return Some(value);
    }
    // Vanilla resource JSON permits comments and trailing commas. Strip them
    // only outside quoted strings; paths/URLs and escaped quotes stay intact.
    let mut clean = Vec::with_capacity(bytes.len());
    let (mut i, mut quoted, mut escaped) = (0, false, false);
    while i < bytes.len() {
        let c = bytes[i];
        if quoted {
            clean.push(c);
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                quoted = false;
            }
            i += 1;
        } else if bytes.get(i..i + 2) == Some(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            i += 2;
            while i + 1 < bytes.len() && bytes.get(i..i + 2) != Some(b"*/") {
                i += 1;
            }
            if i + 1 >= bytes.len() {
                return None;
            }
            i += 2;
            clean.push(b' ');
        } else {
            quoted = c == b'"';
            clean.push(c);
            i += 1;
        }
    }
    let mut output = Vec::with_capacity(clean.len());
    (quoted, escaped) = (false, false);
    for (i, &c) in clean.iter().enumerate() {
        if !quoted
            && c == b','
            && clean[i + 1..]
                .iter()
                .find(|c| !c.is_ascii_whitespace())
                .is_some_and(|c| matches!(c, b'}' | b']'))
        {
            continue;
        }
        output.push(c);
        if quoted {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                quoted = false;
            }
        } else {
            quoted = c == b'"';
        }
    }
    serde_json::from_slice(&output).ok()
}
fn merge(values: &mut HashMap<String, Value>, json: &Value) {
    if let Some(map) = json.as_object() {
        values.extend(map.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
}
fn icon_key(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| value.get("texture")?.as_str())
}
fn texture_path(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("path")?.as_str())
        .or_else(|| texture_path(value.as_array()?.first()?))
}
impl Assets {
    fn image(&mut self, key: String, bytes: &[u8]) {
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            return;
        }
        let previous = self.images.get(&key).map_or(0, Vec::len);
        let next = self.image_bytes - previous + bytes.len();
        if next > MAX_ENCODED_BYTES
            || (self.images.len() >= MAX_IMAGES && !self.images.contains_key(&key))
        {
            return;
        }
        self.images.insert(key, bytes.to_vec());
        self.image_bytes = next;
    }
    fn language(&mut self, data: &[u8]) {
        if let Ok(text) = std::str::from_utf8(data) {
            for line in text.lines() {
                if let Some((key, value)) = line.trim_start_matches('\u{feff}').split_once('=') {
                    if key.starts_with("item.") || key.starts_with("tile.") {
                        self.names.insert(
                            key.to_string(),
                            value.split('\t').next().unwrap_or(value).trim().to_string(),
                        );
                    }
                }
            }
        }
    }
    fn metadata(&mut self, pack: &Path) {
        if let Some(v) = read(&pack.join("blocks.json")).and_then(|b| json(&b)) {
            merge(&mut self.blocks, &v);
        }
        for (file, kind) in [
            ("textures.brarchive", 0),
            ("items.brarchive", 1),
            ("texts.brarchive", 2),
        ] {
            let Some(data) = read(&pack.join("__brarchive").join(file)) else {
                continue;
            };
            let Ok(entries) = archive::entries(&data) else {
                continue;
            };
            for entry in entries {
                match kind {
                    0 if matches!(entry.name, "item_texture.json" | "terrain_texture.json") => {
                        if let Some(v) =
                            json(entry.data).and_then(|v| v.get("texture_data").cloned())
                        {
                            merge(&mut self.textures, &v);
                        }
                    }
                    1 => {
                        if let Some(v) = json(entry.data) {
                            let item = &v["minecraft:item"];
                            if let (Some(id), Some(icon)) = (
                                item["description"]["identifier"].as_str(),
                                icon_key(&item["components"]["minecraft:icon"]),
                            ) {
                                self.items.insert(
                                    id.trim_start_matches("minecraft:").to_string(),
                                    icon.to_string(),
                                );
                            }
                        }
                    }
                    2 if entry.name == "ja_JP.lang" => {
                        if let Ok(text) = std::str::from_utf8(entry.data) {
                            for line in text.lines() {
                                if let Some((key, value)) =
                                    line.trim_start_matches('\u{feff}').split_once('=')
                                {
                                    if key.starts_with("item.") || key.starts_with("tile.") {
                                        self.names
                                            .insert(key.to_string(), value.trim().to_string());
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    fn images(&mut self, dir: &Path, prefix: &str, depth: usize) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten().take(512) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.is_dir() {
                if depth != 0 || matches!(name, "items" | "blocks") {
                    self.images(&path, &format!("{prefix}/{name}"), depth + 1);
                }
            } else if let Some(stem) = name.strip_suffix(".brarchive") {
                if depth == 0 && !matches!(stem, "items" | "blocks") {
                    continue;
                }
                let Some(data) = read(&path) else { continue };
                let Ok(entries) = archive::entries(&data) else {
                    continue;
                };
                for entry in entries {
                    if let Some(stem_name) = entry
                        .name
                        .strip_suffix(".png")
                        .filter(|_| !entry.data.is_empty())
                    {
                        let key = format!("{prefix}/{stem}/{stem_name}");
                        self.image(key, entry.data);
                    }
                }
            } else if let Some(stem) = name.strip_suffix(".png") {
                if depth == 0 {
                    continue;
                }
                if let Some(data) = read(&path) {
                    let key = format!("{prefix}/{stem}");
                    self.image(key, &data);
                }
            }
        }
    }
}

fn decode(data: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(data),
        png::Limits {
            bytes: 8 * 1024 * 1024,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    if info.width == 0 || info.width > 512 || info.height == 0 || info.height > 4096 {
        return None;
    }
    let size = reader.output_buffer_size()?;
    if size > 8 * 1024 * 1024 {
        return None;
    }
    let mut bytes = vec![0; size];
    let info = reader.next_frame(&mut bytes).ok()?;
    let channels = info.color_type.samples();
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let side = info.width.min(info.height) as usize;
    let mut rgba = vec![0; TILE * TILE * 4];
    for y in 0..TILE {
        for x in 0..TILE {
            let source = ((y * side / TILE) * info.width as usize + x * side / TILE) * channels;
            let s = bytes.get(source..source + channels)?;
            let color = match info.color_type {
                png::ColorType::Rgba => [s[0], s[1], s[2], s[3]],
                png::ColorType::Rgb => [s[0], s[1], s[2], 255],
                png::ColorType::GrayscaleAlpha => [s[0], s[0], s[0], s[1]],
                png::ColorType::Grayscale => [s[0], s[0], s[0], 255],
                _ => return None,
            };
            rgba[(y * TILE + x) * 4..(y * TILE + x + 1) * 4].copy_from_slice(&color);
        }
    }
    Some(rgba)
}

pub(crate) fn load(root: &Path) -> Result<Atlas, String> {
    let mut packs: Vec<_> = std::fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| {
            let version = pack_version(e.file_name().to_str()?)?;
            e.path().is_dir().then_some((version, e.path()))
        })
        .take(128)
        .collect();
    packs.sort_by(|a, b| a.0.cmp(&b.0));
    if packs.is_empty() {
        return Err("バニラのリソースパックがありません".into());
    }
    let mut assets = Assets::default();
    for (_, pack) in packs {
        assets.metadata(&pack);
        if let Some(data) = read(&pack.join("texts/ja_JP.lang")) {
            assets.language(&data);
        }
        assets.images(&pack.join("__brarchive/textures"), "textures", 0);
        assets.images(&pack.join("textures"), "textures", 0);
    }
    let mut decoded = Vec::new();
    for (name, bytes) in &assets.images {
        if let Some(image) = decode(bytes) {
            decoded.push((name.clone(), image));
        }
    }
    if decoded.is_empty() {
        return Err("アイテム画像がありません".into());
    }
    let columns = (decoded.len() as f64).sqrt().ceil() as usize;
    let width = columns * PITCH;
    let height = decoded.len().div_ceil(columns) * PITCH;
    let mut pixels = vec![0; width * height * 4];
    let mut tiles = HashMap::new();
    for (index, (name, image)) in decoded.into_iter().enumerate() {
        let (x, y) = ((index % columns) * PITCH + 1, (index / columns) * PITCH + 1);
        for row in 0..TILE {
            pixels[((y + row) * width + x) * 4..((y + row) * width + x + TILE) * 4]
                .copy_from_slice(&image[row * TILE * 4..(row + 1) * TILE * 4]);
        }
        // Extrude one texel into the gutter so bilinear filtering cannot pull
        // transparent pixels into solid block edges or adjacent sprite tiles.
        for row in 0..TILE {
            for (dx, sx) in [(x - 1, 0), (x + TILE, TILE - 1)] {
                pixels[((y + row) * width + dx) * 4..((y + row) * width + dx + 1) * 4]
                    .copy_from_slice(&image[(row * TILE + sx) * 4..(row * TILE + sx + 1) * 4]);
            }
        }
        for dx in 0..PITCH {
            let sx = dx.saturating_sub(1).min(TILE - 1);
            for (dy, sy) in [(y - 1, 0), (y + TILE, TILE - 1)] {
                pixels[(dy * width + x - 1 + dx) * 4..(dy * width + x + dx) * 4]
                    .copy_from_slice(&image[(sy * TILE + sx) * 4..(sy * TILE + sx + 1) * 4]);
            }
        }
        tiles.insert(
            name,
            [
                [x as f32 / width as f32, y as f32 / height as f32],
                [
                    (x + TILE) as f32 / width as f32,
                    (y + TILE) as f32 / height as f32,
                ],
            ],
        );
    }
    let mut icons = HashMap::new();
    for (name, &uv) in &tiles {
        if let Some(id) = name.strip_prefix("textures/items/") {
            icons.insert(id.to_string(), Icon::Flat(uv));
        }
    }
    for (id, value) in &assets.textures {
        if let Some(path) = value.get("textures").and_then(texture_path) {
            if let Some(&uv) = tiles.get(path) {
                let icon = if path.starts_with("textures/blocks/") {
                    Icon::Cube {
                        top: uv,
                        side: uv,
                        front: uv,
                    }
                } else {
                    Icon::Flat(uv)
                };
                icons.entry(id.clone()).or_insert(icon);
            }
        }
    }
    for (id, key) in &assets.items {
        if let Some(&icon) = icons.get(key) {
            icons.insert(id.clone(), icon);
        }
    }
    for material in ["wooden", "stone", "iron", "golden", "diamond", "netherite"] {
        for tool in ["sword", "pickaxe", "axe", "shovel", "hoe"] {
            let texture = format!(
                "textures/items/{}_{}",
                match material {
                    "wooden" => "wood",
                    "golden" => "gold",
                    s => s,
                },
                tool
            );
            if let Some(&uv) = tiles.get(&texture) {
                icons.insert(format!("{material}_{tool}"), Icon::Flat(uv));
            }
        }
    }
    for (id, value) in &assets.blocks {
        let id = id.trim_start_matches("minecraft:");
        if matches!(icons.get(id), Some(Icon::Flat(_))) {
            continue;
        }
        let shape = value.get("blockshape").and_then(Value::as_str);
        let Some(value) = value.get("textures") else {
            continue;
        };
        let face = |key| -> Option<Uv> {
            let value = value
                .as_str()
                .or_else(|| value.get(key)?.as_str())
                .or_else(|| value.get("side")?.as_str())?;
            let path = assets
                .textures
                .get(value)?
                .get("textures")
                .and_then(texture_path)?;
            tiles.get(path).copied()
        };
        if let (Some(top), Some(side)) = (face("up"), face("north")) {
            let icon = if shape.is_some_and(|s| !matches!(s, "normal" | "block")) {
                Icon::Flat(side)
            } else {
                Icon::Cube {
                    top,
                    side: face("east").unwrap_or(side),
                    front: side,
                }
            };
            icons.insert(id.to_string(), icon);
        }
    }
    for (path, &uv) in &tiles {
        if let Some(id) = path.strip_prefix("textures/blocks/") {
            icons.entry(id.to_string()).or_insert(Icon::Cube {
                top: uv,
                side: uv,
                front: uv,
            });
            for (prefix, suffix) in [
                ("planks_", "_planks"),
                ("log_", "_log"),
                ("leaves_", "_leaves"),
                ("sapling_", "_sapling"),
            ] {
                if let Some(wood) = id.strip_prefix(prefix) {
                    icons
                        .entry(format!("{wood}{suffix}"))
                        .or_insert(if prefix == "sapling_" {
                            Icon::Flat(uv)
                        } else {
                            Icon::Cube {
                                top: uv,
                                side: uv,
                                front: uv,
                            }
                        });
                }
            }
        }
    }
    // Converted identifiers are aliases only; resource and saved item IDs stay intact.
    for (id, top, side) in [
        ("chest", "chest_top", "chest_front"),
        ("trapped_chest", "chest_top", "trapped_chest_front"),
        ("ender_chest", "ender_chest_top", "ender_chest_front"),
    ] {
        if let (Some(&top), Some(&side)) = (
            tiles.get(&format!("textures/blocks/{top}")),
            tiles.get(&format!("textures/blocks/{side}")),
        ) {
            let wall = if id == "ender_chest" {
                "ender_chest_side"
            } else {
                "chest_side"
            };
            let wall = tiles
                .get(&format!("textures/blocks/{wall}"))
                .copied()
                .unwrap_or(side);
            icons.entry(id.into()).or_insert(Icon::Cube {
                top,
                side: wall,
                front: side,
            });
        }
    }
    for (id, key) in [
        ("golden_apple", "apple_golden"),
        ("enchanted_golden_apple", "apple_golden"),
        ("water_bucket", "bucket_water"),
        ("lava_bucket", "bucket_lava"),
        ("milk_bucket", "bucket_milk"),
        ("bucket", "bucket_empty"),
        ("totem_of_undying", "totem"),
    ] {
        if let Some(&icon) = icons.get(key) {
            icons.insert(id.to_string(), icon);
        }
    }
    let mut names = HashMap::new();
    for (key, value) in &assets.names {
        if let Some(id) = key
            .strip_prefix("item.")
            .or_else(|| key.strip_prefix("tile."))
            .and_then(|k| k.strip_suffix(".name"))
        {
            if !id.contains('.') {
                names.insert(id.to_string(), value.clone());
            }
        }
    }
    for id in icons.keys() {
        for key in [
            format!("item.{id}.name"),
            format!("tile.{id}.name"),
            format!("tile.{id}.{id}.name"),
        ] {
            if let Some(name) = assets.names.get(&key) {
                names.insert(id.clone(), name.clone());
                break;
            }
        }
    }
    Ok(Atlas {
        pixels,
        width: width as u32,
        height: height as u32,
        icons,
        names,
    })
}

impl Icons {
    pub(super) fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.names.get(id).map_or(id, String::as_str)
    }
    pub(super) fn draw(&self, d: &DrawListMut<'_>, id: &str, min: [f32; 2], size: f32) -> bool {
        let Some(icon) = self.icons.get(id) else {
            return false;
        };
        match *icon {
            Icon::Flat([uv0, uv1]) => {
                d.add_image(self.texture, min, [min[0] + size, min[1] + size])
                    .uv_min(uv0)
                    .uv_max(uv1)
                    .build();
            }
            Icon::Cube { top, side, front } => {
                let point = |x, y| [min[0] + size * x, min[1] + size * y];
                let uv = |u: Uv| [u[0], [u[1][0], u[0][1]], u[1], [u[0][0], u[1][1]]];
                let t = uv(top);
                let s = uv(side);
                let f = uv(front);
                d.add_image_quad(
                    self.texture,
                    point(0.5, 0.),
                    point(1., 0.25),
                    point(0.5, 0.5),
                    point(0., 0.25),
                )
                .uv(t[0], t[1], t[2], t[3])
                .build();
                d.add_image_quad(
                    self.texture,
                    point(0., 0.25),
                    point(0.5, 0.5),
                    point(0.5, 1.),
                    point(0., 0.75),
                )
                .uv(s[0], s[1], s[2], s[3])
                .col([0.73, 0.73, 0.73, 1.])
                .build();
                d.add_image_quad(
                    self.texture,
                    point(0.5, 0.5),
                    point(1., 0.25),
                    point(1., 0.75),
                    point(0.5, 1.),
                )
                .uv(f[0], f[1], f[2], f[3])
                .col([0.55, 0.55, 0.55, 1.])
                .build();
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Manual timing against the installed game's resource packs.
    #[test]
    #[ignore = "manual: cargo test -p client --release --lib atlas_load_benchmark -- --ignored --nocapture"]
    fn atlas_load_benchmark() {
        let root =
            std::path::Path::new(r"C:\XboxGames\Minecraft for Windows\Content\data\resource_packs");
        let mut times = Vec::new();
        let mut summary = String::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let atlas = load(root).expect("atlas");
            times.push(start.elapsed());
            summary = format!(
                "{} icons, {}x{} atlas, {} names",
                atlas.icons.len(),
                atlas.width,
                atlas.height,
                atlas.names.len()
            );
        }
        times.sort();
        println!(
            "atlas load: median {:.1} ms (min {:.1}, max {:.1}); {summary}",
            times[1].as_secs_f64() * 1e3,
            times[0].as_secs_f64() * 1e3,
            times[2].as_secs_f64() * 1e3
        );
    }
    #[test]
    fn encoded_images_are_bounded_and_replacements_keep_the_budget_accurate() {
        let mut assets = Assets::default();
        for i in 0..MAX_IMAGES + 2 {
            assets.image(format!("item_{i}"), b"x");
        }
        assert_eq!(
            (assets.images.len(), assets.image_bytes),
            (MAX_IMAGES, MAX_IMAGES)
        );
        assets.image("item_0".into(), b"xyz");
        assert_eq!(
            (assets.images.len(), assets.image_bytes),
            (MAX_IMAGES, MAX_IMAGES + 2)
        );
        assets.image("large".into(), &vec![0; MAX_IMAGE_BYTES + 1]);
        assert!(!assets.images.contains_key("large"));
    }
    #[test]
    fn resource_json_accepts_comments_without_changing_string_contents() {
        let v = json(
            br#"{/* resource */ "url":"https://x/a,//b", "values":[1,2,], // comment
          "quote":"a\"b", }"#,
        )
        .unwrap();
        assert_eq!(v["url"], "https://x/a,//b");
        assert_eq!(v["quote"], "a\"b");
        assert_eq!(v["values"].as_array().unwrap().len(), 2);
        assert!(json(b"{/* unfinished").is_none());
    }
    #[test]
    fn versions_are_numeric_and_only_vanilla_packs_participate() {
        assert!(pack_version("vanilla").unwrap() < pack_version("vanilla_1.9").unwrap());
        assert!(pack_version("vanilla_1.9").unwrap() < pack_version("vanilla_1.26.50").unwrap());
        assert!(pack_version("chemistry").is_none());
        assert!(pack_version("vanilla_trial").is_none());
    }
    #[test]
    fn png_dimensions_transparency_and_animation_first_frame_are_bounded() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 4);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let mut pixels = vec![
                255, 0, 0, 0, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ];
            pixels.extend([0; 16]);
            writer.write_image_data(&pixels).unwrap();
        }
        let rgba = decode(&png).unwrap();
        assert_eq!(&rgba[..4], &[255, 0, 0, 0]);
        assert_eq!(&rgba[(TILE - 1) * 4..TILE * 4], &[0, 255, 0, 255]);
        assert_eq!(&rgba[(TILE * TILE - 1) * 4..], &[255, 255, 255, 255]);
        assert!(decode(b"not a PNG").is_none());
    }
    #[test]
    fn supplied_assets_cover_tools_food_and_blocks_without_embedding_them() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("Microsoft.MinecraftUWP_1.26.6301.0_x64__8wekyb3d8bbwe/data/resource_packs");
        if !root.is_dir() {
            eprintln!("Supplied game assets are absent; archive/PNG fixture tests still run.");
            return;
        }
        let start = std::time::Instant::now();
        let atlas = load(&root).unwrap();
        eprintln!(
            "Item atlas {}x{} / {} icons / {} bytes / {:?}",
            atlas.width,
            atlas.height,
            atlas.icons.len(),
            atlas.pixels.len(),
            start.elapsed()
        );
        for id in [
            "wooden_hoe",
            "stone_hoe",
            "netherite_pickaxe",
            "netherite_axe",
            "netherite_shovel",
            "netherite_helmet",
            "bread",
            "cooked_chicken",
            "fishing_rod",
            "chest",
            "diamond_ore",
            "dirt",
            "stone",
            "cobblestone",
        ] {
            assert!(atlas.icons.contains_key(id), "missing supplied icon: {id}");
        }
        assert_eq!(atlas.names.get("bread").map(String::as_str), Some("パン"));
        assert!(matches!(atlas.icons.get("stone"), Some(Icon::Cube { .. })));
        assert!(matches!(atlas.icons.get("bread"), Some(Icon::Flat(_))));
        assert_eq!(
            atlas.pixels.len(),
            atlas.width as usize * atlas.height as usize * 4
        );
        assert!(atlas.width <= 2200 && atlas.height <= 2200);
        if let Some(output) = std::env::var_os("MCBE_ITEM_ATLAS_OUTPUT") {
            let file = std::fs::File::create(output).unwrap();
            let mut encoder = png::Encoder::new(file, atlas.width, atlas.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&atlas.pixels)
                .unwrap();
        }
    }
}
