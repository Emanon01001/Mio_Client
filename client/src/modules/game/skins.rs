//! Read-only ClientSkin snapshots. Native pointers never leave this callback;
//! a requested image is copied once, then PNG encoding runs on the client worker.
pub(crate) mod geometry;
pub(crate) mod layout;
use super::{
    actors,
    entities::{component_pools, PoolView},
    memory::ReadBatch,
    read,
};
use crate::modules::entity_kind::{kind, Kind};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, LazyLock, Mutex,
};
use windows_sys::Win32::System::{
    Diagnostics::Debug::ReadProcessMemory, SystemInformation::GetTickCount64,
    Threading::GetCurrentProcess,
};

const OWNER_HASH: u32 = actors::type_hash("ActorOwnerComponent");
const INTERVAL: u64 = 500;
const MAX_AGE: u64 = 2000;
const REQUEST_TIMEOUT: u64 = 5000;
const MAX_PLAYERS: usize = 64;

#[derive(Clone, Debug)]
pub(crate) struct PlayerSkin {
    pub entity: u32,
    pub name: String,
    pub skin_id: String,
    pub width: u32,
    pub height: u32,
    pub local: bool,
    owner: (usize, u32),
}

pub(crate) struct Export {
    pub player: PlayerSkin,
    pub rgba: Vec<u8>,
    pub directory: PathBuf,
    pub mode: ExportMode,
    pub model: Result<Option<geometry::Model>, String>,
    pub geometry_id: Option<String>,
    generation: u64,
}
struct Request {
    player: PlayerSkin,
    directory: PathBuf,
    at: u64,
    mode: ExportMode,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportMode {
    Skin,
    Raw,
}
struct State {
    active: bool,
    generation: u64,
    at: u64,
    last_attempt: u64,
    candidates: usize,
    rejected: usize,
    players: Arc<Vec<PlayerSkin>>,
    pending: Option<Request>,
    ready: Option<Export>,
    busy: bool,
    status: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            active: false,
            generation: 0,
            at: 0,
            last_attempt: 0,
            candidates: 0,
            rejected: 0,
            players: Arc::new(Vec::new()),
            pending: None,
            ready: None,
            busy: false,
            status: "保存するプレイヤーを一覧から選択してください".into(),
        }
    }
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));
static EMPTY: LazyLock<Arc<Vec<PlayerSkin>>> = LazyLock::new(|| Arc::new(Vec::new()));
static ACTIVE: AtomicBool = AtomicBool::new(false);
fn now() -> u64 {
    unsafe { GetTickCount64() }
}

pub(crate) fn available() -> Result<(), String> {
    layout::raw().map(|_| ())
}
pub(crate) fn compatible_available() -> Result<(), String> {
    layout::model().map(|_| ())
}
pub(crate) fn enable(active: bool) {
    ACTIVE.store(false, Ordering::Release);
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let generation = s.generation.wrapping_add(1);
    *s = State {
        active,
        generation,
        ..State::default()
    };
    ACTIVE.store(active, Ordering::Release);
}
pub(crate) fn snapshot() -> Arc<Vec<PlayerSkin>> {
    let s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if s.active && s.at != 0 && now().saturating_sub(s.at) <= MAX_AGE {
        Arc::clone(&s.players)
    } else {
        Arc::clone(&EMPTY)
    }
}
pub(crate) fn status() -> String {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .status
        .clone()
}
pub(crate) fn busy() -> bool {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).busy
}
pub(crate) fn refresh() {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).last_attempt = 0;
}
pub(crate) fn request(
    player: &PlayerSkin,
    directory: PathBuf,
    mode: ExportMode,
) -> Result<(), String> {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if !s.active {
        return Err("SkinStealerをONにしてください".into());
    }
    if mode == ExportMode::Skin {
        compatible_available()?;
    }
    if s.busy {
        return Err("現在の保存処理が終わるまでお待ちください".into());
    }
    if s.at == 0
        || now().saturating_sub(s.at) > MAX_AGE
        || !s.players.iter().any(|p| same_skin(p, player))
    {
        return Err("一覧が古くなっています。ワールドへ戻って更新してください".into());
    }
    s.pending = Some(Request {
        player: player.clone(),
        directory,
        at: now(),
        mode,
    });
    s.busy = true;
    s.status = "画像の取得を待機中。進まない場合はメニューを閉じてください".into();
    Ok(())
}
fn same_skin(a: &PlayerSkin, b: &PlayerSkin) -> bool {
    a.owner == b.owner
        && a.entity == b.entity
        && a.skin_id == b.skin_id
        && a.width == b.width
        && a.height == b.height
}
/// Bound dimensions before multiplication/allocation, including malformed headers.
pub(crate) fn rgba_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return None;
    }
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}
struct Image {
    skin: usize,
    id: String,
    width: u32,
    height: u32,
    pixels: usize,
    bytes: usize,
}
unsafe fn image(
    m: &mut ReadBatch,
    pool: &PoolView,
    entity: u32,
    registry: usize,
    fields: layout::Raw,
) -> Option<Image> {
    let component = pool.component(m, entity)?;
    let actor: usize = read(component);
    if !m.readable(actor, fields.actor_ref.checked_add(8)?.max(0x1c))
        || read::<usize>(actor + 0x10) != registry
        || read::<u32>(actor + 0x18) != entity
    {
        return None;
    }
    // ClientSkin is owned by a distinct visual entity, not this Actor's ID.
    // Actor::setSkin stores SerializedSkinRef in its cached component +0xAD8.
    let reference: usize = read(actor + fields.actor_ref);
    if !m.readable(reference, 16) {
        return None;
    }
    let skin = read(reference);
    read_image(m, skin, fields)
}
unsafe fn read_image(m: &mut ReadBatch, skin: usize, fields: layout::Raw) -> Option<Image> {
    if !m.readable(skin, (fields.image + 0x30).max(fields.id + 32)) {
        return None;
    }
    let id = m.string(skin + fields.id, 256)?;
    if id.is_empty() || id.contains('\0') {
        return None;
    }
    let mut image = read_texture(m, skin + fields.image, fields)?;
    image.id = id;
    image.skin = skin;
    Some(image)
}
unsafe fn read_texture(m: &mut ReadBatch, address: usize, fields: layout::Raw) -> Option<Image> {
    if !m.readable(address, 0x30) || read::<u32>(address) != 4 {
        return None;
    }
    let width = read(address + 4);
    let height = read(address + 8);
    let bytes = rgba_len(width, height)?;
    // mce::Blob is {deleter, data, byte count}, NOT a std::vector.
    if read::<usize>(address + fields.length) != bytes {
        return None;
    }
    let pixels = read(address + fields.data);
    m.readable(pixels, bytes).then_some(Image {
        id: String::new(),
        skin: 0,
        width,
        height,
        pixels,
        bytes,
    })
}
unsafe fn copy_pixels(img: &Image) -> Result<Vec<u8>, String> {
    let mut rgba = vec![0u8; img.bytes];
    let mut copied = 0;
    if ReadProcessMemory(
        GetCurrentProcess(),
        img.pixels as *const _,
        rgba.as_mut_ptr().cast(),
        rgba.len(),
        &mut copied,
    ) == 0
        || copied != rgba.len()
    {
        return Err("スキン画像のコピーに失敗しました".into());
    }
    Ok(rgba)
}
struct Collection {
    fields: layout::Raw,
    rows: Vec<PlayerSkin>,
    pool: PoolView,
    memory: ReadBatch,
    candidates: usize,
    rejected: usize,
}
unsafe fn collect(player: usize) -> Option<Collection> {
    let fields = layout::raw().ok()?;
    let mut m = ReadBatch::new();
    if !m.readable(player, 0x1c) {
        return None;
    }
    let owner = (read(player + 0x10), read(player + 0x18));
    let facts = actors::actors_for_local(owner)?;
    let pool = component_pools(&mut m, owner.0, [OWNER_HASH])[0]?;
    let pool = PoolView::new(&mut m, pool, 8)?;
    let mut rows = Vec::new();
    let mut candidates = 0;
    let mut rejected = 0;
    for entry in pool.entities().step_by(4).take(4096) {
        let entity = read(entry);
        let local = entity == owner.1;
        let actor = facts.get(&entity);
        // Ignore preview actors, deleted pool slots and non-player components.
        if !local
            && !actor
                .is_some_and(|a| a.bounds.is_some() && kind(&a.name, a.actor_type) == Kind::Player)
        {
            continue;
        }
        if rows.len() == MAX_PLAYERS && !local {
            continue;
        }
        candidates += 1;
        let Some(img) = image(&mut m, &pool, entity, owner.0, fields) else {
            rejected += 1;
            continue;
        };
        let name = actor
            .and_then(|a| a.display_name.as_deref())
            .map(crate::modules::chat_log::clean)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if local {
                    "自分".into()
                } else {
                    format!("Player {entity:08X}")
                }
            });
        if rows.len() == MAX_PLAYERS {
            // Keep self available even if the native dense order puts it last.
            rows.pop();
        }
        rows.push(PlayerSkin {
            entity,
            name,
            skin_id: img.id,
            width: img.width,
            height: img.height,
            local,
            owner,
        });
    }
    rows.sort_by(|a, b| {
        b.local
            .cmp(&a.local)
            .then_with(|| a.name.cmp(&b.name))
            .then(a.entity.cmp(&b.entity))
    });
    Some(Collection {
        fields,
        rows,
        pool,
        memory: m,
        candidates,
        rejected,
    })
}
pub(super) unsafe fn sample(player: usize, at: u64) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let (generation, requested) = {
        let Ok(mut s) = STATE.try_lock() else {
            return;
        };
        let interval = if s.pending.is_some() { 50 } else { INTERVAL };
        if !s.active || at.saturating_sub(s.last_attempt) < interval {
            return;
        }
        s.last_attempt = at;
        (s.generation, s.pending.is_some())
    };
    let Some(Collection {
        fields,
        rows,
        pool,
        memory: mut m,
        candidates,
        rejected,
    }) = collect(player)
    else {
        return;
    };
    let request = {
        let Ok(mut s) = STATE.try_lock() else {
            return;
        };
        if !s.active || s.generation != generation {
            return;
        }
        s.at = at;
        s.candidates = candidates;
        s.rejected = rejected;
        s.players = Arc::new(rows);
        if !requested {
            return;
        }
        s.pending.take()
    };
    let Some(request) = request else {
        return;
    };
    let present = snapshot().iter().any(|p| same_skin(p, &request.player));
    let result = (|| {
        if !present {
            return Err("対象が変わりました。一覧から選び直してください");
        }
        if at.saturating_sub(request.at) > REQUEST_TIMEOUT {
            return Err("画像の取得が期限切れになりました");
        }
        let owner = (read::<usize>(player + 0x10), read::<u32>(player + 0x18));
        if owner != request.player.owner {
            return Err("ワールドが切り替わりました。一覧から選び直してください");
        }
        let Some(img) = image(&mut m, &pool, request.player.entity, owner.0, fields) else {
            return Err("対象のスキンが読み込まれていません");
        };
        if img.id != request.player.skin_id
            || img.width != request.player.width
            || img.height != request.player.height
        {
            return Err("スキンが変更されました。一覧から選び直してください");
        }
        // Use the OS copy for this large, externally-owned range: a concurrent
        // unload must fail the request rather than faulting in slice::to_vec.
        let rgba = copy_pixels(&img).map_err(|_| "スキン画像のコピーに失敗しました")?;
        let geometry_id = if request.mode == ExportMode::Skin {
            m.string(img.skin + 0x60, 256)
        } else {
            None
        };
        let custom_geometry = geometry_id.as_deref().is_some_and(|name| {
            !name.is_empty()
                && !matches!(
                    name,
                    "geometry.humanoid"
                        | "geometry.humanoid.custom"
                        | "geometry.humanoid.customSlim"
                )
        });
        let model = if request.mode == ExportMode::Skin
            && (img.id.starts_with("persona-")
                || custom_geometry
                || img.width > 128
                || img.height > 128)
        {
            geometry::capture(&mut ReadBatch::new(), img.skin, geometry_id.as_deref()).map(Some)
        } else {
            Ok(None)
        };
        let Some(after) = image(
            &mut ReadBatch::new(),
            &pool,
            request.player.entity,
            owner.0,
            fields,
        ) else {
            return Err("取得中にスキンが変更されました");
        };
        if after.pixels != img.pixels
            || after.skin != img.skin
            || after.id != img.id
            || after.width != img.width
            || after.height != img.height
        {
            return Err("取得中にスキンが変更されました");
        }
        Ok(Export {
            player: request.player,
            rgba,
            directory: request.directory,
            mode: request.mode,
            model,
            geometry_id,
            generation,
        })
    })();
    // The request has left the pending slot: always publish its result so a
    // contended UI status read cannot leave the exporter permanently busy.
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if s.active && s.generation == generation {
        match result {
            Ok(job) => {
                s.ready = Some(job);
                s.status = "PNGの保存を待機中".into();
            }
            Err(error) => {
                s.busy = false;
                s.status = error.into();
            }
        }
    }
}
/// Called only by the client worker, outside the shared Modules lock.
pub(crate) fn take_export() -> Option<Export> {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if s.pending
        .as_ref()
        .is_some_and(|r| now().saturating_sub(r.at) > REQUEST_TIMEOUT)
    {
        s.pending = None;
        s.busy = false;
        s.status =
            "画像の取得が期限切れになりました。メニューを閉じて更新後に再度保存してください".into();
    }
    s.ready.take()
}
pub(crate) fn finish(job: &Export, result: Result<String, String>) {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if s.generation == job.generation {
        s.busy = false;
        s.status = result.unwrap_or_else(|e| format!("保存失敗: {e}"));
    }
}
pub(crate) fn diagnostic() -> String {
    let s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut text = format!(
        "SkinStealer enabled={} age={}ms busy={} players={} candidates={} rejected_images={} status={}\n",
        s.active,
        now().saturating_sub(s.at),
        s.busy,
        s.players.len(),
        s.candidates,
        s.rejected,
        s.status
    );
    for p in s.players.iter() {
        text.push_str(&format!(
            "{:08X} {} local={} {}x{} id={}\n",
            p.entity, p.name, p.local, p.width, p.height, p.skin_id
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIELDS: layout::Raw = layout::Raw {
        actor_ref: 0xad8,
        id: 0x20,
        image: 0x80,
        data: 0x20,
        length: 0x28,
    };
    #[test]
    fn rejects_unbounded_image_headers_before_allocation() {
        assert_eq!(rgba_len(64, 32), Some(8192));
        assert_eq!(rgba_len(256, 256), Some(262144));
        assert_eq!(rgba_len(1024, 1024), Some(4 * 1024 * 1024));
        for (w, h) in [(0, 64), (64, 0), (1025, 64), (u32::MAX, u32::MAX)] {
            assert_eq!(rgba_len(w, h), None);
        }
    }
    #[test]
    fn selection_tokens_reject_world_entity_generation_and_skin_changes() {
        let a = PlayerSkin {
            entity: 1,
            name: "self".into(),
            skin_id: "skin".into(),
            width: 64,
            height: 64,
            local: true,
            owner: (0x1000, 1),
        };
        assert!(same_skin(&a, &a));
        for b in [
            PlayerSkin {
                owner: (0x2000, 1),
                ..a.clone()
            },
            PlayerSkin {
                owner: (0x1000, 2),
                ..a.clone()
            },
            PlayerSkin {
                entity: 0x40001,
                ..a.clone()
            },
            PlayerSkin {
                skin_id: "changed".into(),
                ..a.clone()
            },
            PlayerSkin {
                height: 32,
                ..a.clone()
            },
        ] {
            assert!(!same_skin(&a, &b));
        }
    }
    #[test]
    fn image_header_checks_format_blob_length_and_msvc_string() {
        let mut skin = [0u64; 24];
        let pixels = vec![42u8; 64 * 32 * 4];
        let base = skin.as_mut_ptr() as usize;
        // A real MSVC inline string followed by the native Image header/Blob.
        skin[4] = u64::from_le_bytes(*b"skin\0\0\0\0");
        skin[6] = 4;
        skin[7] = 15;
        skin[16] = 4 | (64u64 << 32);
        skin[17] = 32;
        skin[20] = pixels.as_ptr() as u64;
        skin[21] = pixels.len() as u64;
        unsafe {
            let img = read_image(&mut ReadBatch::new(), base, FIELDS).unwrap();
            assert_eq!(
                (img.id.as_str(), img.width, img.height, img.bytes),
                ("skin", 64, 32, 8192)
            );
            assert_eq!(img.pixels, pixels.as_ptr() as usize);
            for (slot, value) in [
                (16, 3 | (64u64 << 32)),
                (17, 0),
                (20, 0),
                (21, 8191),
                (7, 0),
                (6, 257),
            ] {
                let before = skin[slot];
                skin[slot] = value;
                assert!(
                    read_image(&mut ReadBatch::new(), base, FIELDS).is_none(),
                    "header slot {slot}"
                );
                skin[slot] = before;
            }
        }
    }
    #[test]
    fn real_actor_skin_handles_moved_fields_and_independent_visual_entity_id() {
        for fields in [
            FIELDS,
            layout::Raw {
                actor_ref: 0xb18,
                id: 0x40,
                image: 0x60,
                ..FIELDS
            },
        ] {
            let mut skin = [0u64; 24];
            let pixels = vec![42u8; 64 * 32 * 4];
            skin[fields.id / 8] = u64::from_le_bytes(*b"skin\0\0\0\0");
            skin[(fields.id + 16) / 8] = 4;
            skin[(fields.id + 24) / 8] = 15;
            skin[fields.image / 8] = 4 | (64u64 << 32);
            skin[(fields.image + 8) / 8] = 32;
            skin[(fields.image + fields.data) / 8] = pixels.as_ptr() as u64;
            skin[(fields.image + fields.length) / 8] = pixels.len() as u64;
            let reference = [skin.as_ptr() as usize, 0];
            let mut actor = vec![0usize; (fields.actor_ref + 8) / 8];
            let registry = 0xde00;
            let entity = 0x40010u32;
            actor[2] = registry;
            actor[3] = entity as usize;
            actor[fields.actor_ref / 8] = reference.as_ptr() as usize;
            let mut components = [0usize; 128];
            components[0] = actor.as_ptr() as usize;
            let pages = [components.as_ptr() as usize];
            let dense = [entity];
            let mut sparse_page = [u32::MAX; 2048];
            sparse_page[16] = 0x40000;
            let sparse = [sparse_page.as_ptr() as usize];
            let mut pool = [0usize; 12];
            pool[1] = sparse.as_ptr() as usize;
            pool[2] = pool[1] + 8;
            pool[4] = dense.as_ptr() as usize;
            pool[5] = pool[4] + 4;
            pool[10] = pages.as_ptr() as usize;
            pool[11] = pool[10] + 8;
            let mut m = ReadBatch::new();
            unsafe {
                let owners = PoolView::new(&mut m, pool.as_ptr() as usize, 8).unwrap();
                assert_eq!(
                    image(&mut m, &owners, entity, registry, fields).unwrap().id,
                    "skin"
                );
                assert!(image(&mut m, &owners, entity, registry + 1, fields).is_none());
                assert!(image(&mut m, &owners, entity + 0x40000, registry, fields).is_none());
                actor[3] = entity as usize + 1;
                assert!(image(&mut m, &owners, entity, registry, fields).is_none());
                actor[3] = entity as usize;
                actor[fields.actor_ref / 8] = 0;
                assert!(image(&mut m, &owners, entity, registry, fields).is_none());
            }
        }
    }
}
