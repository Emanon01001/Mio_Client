//! Build identity and resolved native offsets; signatures live in crate::signatures.
pub(crate) use crate::compatibility::{derive, Addresses, Code};
use crate::compatibility::{Build, BUILDS};
use crate::game::{patch, read};
use crate::signatures::{ACCESSOR_SIGNATURE, ACTOR_SIGNATURE};
use crate::{config, log};
use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

// --- Captured offsets ----------------------------------------------------------

/// Where in `ACCESSOR_SIGNATURE` the captured offsets sit.
const ACCESSOR_CLIENT_INSTANCE: usize = 32;
const ACCESSOR_VT_LOCAL_PLAYER: usize = 39;
/// The `jne` the native HUD patch rewrites.
pub(crate) const ACCESSOR_PATCH_OFFSET: isize = 126;

const ACTOR_STATE: usize = 3;
const ACTOR_AABB: usize = 10;

// --- Layout ----------------------------------------------------------------------

/// Every offset this module uses, resolved once by `init`.
#[derive(Clone, Copy)]
pub(in crate::game) struct Layout {
    /// The accessor's entry point.
    pub(in crate::game) accessor: usize,
    // Read out of the game's code:
    /// accessor argument -> ClientInstance
    pub(in crate::game) client_instance: usize,
    /// ClientInstance vtable slot (in bytes) returning the local player
    pub(in crate::game) vt_local_player: usize,
    /// Actor -> StateVectorComponent { pos, prev pos, velocity }; pos is the eye
    pub(in crate::game) actor_state: usize,
    /// Actor -> AABBShapeComponent { min, max, width, height }
    pub(in crate::game) actor_aabb: usize,
    // Data layout: analyzed-build defaults, overridable in `[offsets]`:
    /// Actor -> entt registry
    pub(in crate::game) actor_registry: usize,
    /// registry -> component pools: a dense map's packed node array (begin, end)
    pub(in crate::game) registry_pools: usize,
    /// pool -> entity ids (begin, end)
    pub(in crate::game) pool_entities: usize,
    /// pool -> component pages (begin, end)
    pub(in crate::game) pool_pages: usize,
    /// components per page
    pub(in crate::game) pool_page_len: usize,
    /// ClientInstance -> 3 basis vectors (right, up, back), 4 floats apart
    pub(in crate::game) view_matrix: usize,
    /// ClientInstance -> LevelRenderer
    pub(in crate::game) level_renderer: usize,
    /// LevelRenderer -> LevelRendererPlayer
    pub(in crate::game) renderer_player: usize,
    /// LevelRendererPlayer -> camera position
    pub(in crate::game) camera_origin: usize,
    /// LevelRendererPlayer -> world projection m00 / m11
    pub(in crate::game) projection_x: usize,
    pub(in crate::game) projection_y: usize,
}

pub(in crate::game) static LAYOUT: OnceLock<Result<Layout, String>> = OnceLock::new();

pub(in crate::game) fn layout() -> Result<Layout, String> {
    match LAYOUT.get() {
        Some(layout) => layout.clone(),
        None => Err("game layout not initialized".into()),
    }
}

/// Reads a little-endian value out of matched code.
unsafe fn code<T: Copy + Into<u64>>(addr: usize) -> usize {
    let value: u64 = (addr as *const T).read_unaligned().into();
    value as usize
}

fn resolve(overrides: &config::Offsets) -> Result<Layout, String> {
    let accessor = patch::find(ACCESSOR_SIGNATURE).map_err(|e| format!("accessor: {e}"))?;
    let actor = patch::find(ACTOR_SIGNATURE).map_err(|e| format!("actor offsets: {e}"))?;
    let layout = unsafe {
        Layout {
            accessor,
            client_instance: code::<u8>(accessor + ACCESSOR_CLIENT_INSTANCE),
            vt_local_player: code::<u32>(accessor + ACCESSOR_VT_LOCAL_PLAYER),
            actor_state: code::<u32>(actor + ACTOR_STATE),
            actor_aabb: code::<u32>(actor + ACTOR_AABB),
            actor_registry: overrides.actor_registry.unwrap_or(0x10),
            registry_pools: overrides.registry_pools.unwrap_or(0x68),
            pool_entities: overrides.pool_entities.unwrap_or(0x20),
            pool_pages: overrides.pool_pages.unwrap_or(0x50),
            pool_page_len: overrides.pool_page_len.unwrap_or(128),
            view_matrix: overrides.view_matrix.unwrap_or(0x438),
            level_renderer: overrides.level_renderer.unwrap_or(0x1E0),
            renderer_player: overrides.renderer_player.unwrap_or(0x480),
            camera_origin: overrides.camera_origin.unwrap_or(0x64C),
            projection_x: overrides.projection_x.unwrap_or(0xF50),
            projection_y: overrides.projection_y.unwrap_or(0xF64),
        }
    };
    if layout.vt_local_player % 8 != 0 || layout.pool_page_len == 0 {
        return Err("resolved offsets are implausible".into());
    }
    Ok(layout)
}

/// Resolves the layout. Call once, before any module uses this one.
pub fn init(overrides: &config::Offsets) {
    let result = LAYOUT.get_or_init(|| resolve(overrides));
    match result {
        Ok(l) => {
            log!(
                "game: accessor 0x{:X}, client +0x{:X}, player vt +0x{:X}, state +0x{:X}, box +0x{:X}",
                l.accessor,
                l.client_instance,
                l.vt_local_player,
                l.actor_state,
                l.actor_aabb
            );
            match build() {
                Some(b) => log!("game: analyzed build {}", b.name),
                None => {
                    log!("game: not an analyzed build ({SUPPORTED}); code offsets were re-read");
                    log!("      from this build, the camera/entity layout uses defaults or [offsets]");
                }
            }
            // Decide (and log) once here, not first on the game thread.
            trust();
        }
        Err(e) => log!("game: unavailable: {e}"),
    }
}

/// Whether the hook can be installed in this game build.
pub fn available() -> Result<(), String> {
    layout().map(|_| ())
}

/// The main image's address range.
pub(in crate::game) fn image() -> (usize, usize) {
    static IMAGE: OnceLock<(usize, usize)> = OnceLock::new();
    *IMAGE.get_or_init(|| unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as usize;
        if base == 0 {
            return (0, 0);
        }
        // The first header page is mapped for the module's whole lifetime.
        let pe = read::<u32>(base + 0x3C) as usize;
        let size = if pe < 0xF00 {
            read::<u32>(base + pe + 24 + 56) as usize
        } else {
            0
        };
        (base, base + size)
    })
}

/// The analyzed build that is running, if it is one of `BUILDS`.
fn build() -> Option<&'static Build> {
    static BUILD: OnceLock<Option<usize>> = OnceLock::new();
    let index = *BUILD.get_or_init(|| {
        let (base, _) = image();
        if base == 0 {
            return None;
        }
        unsafe {
            let pe = read::<u32>(base + 0x3C) as usize;
            if pe >= 0xF00 {
                return None;
            }
            let id = (
                read::<u32>(base + pe + 8),
                read::<u32>(base + pe + 24 + 56),
                read::<u32>(base + pe + 24 + 64),
            );
            BUILDS
                .iter()
                .position(|b| (b.timestamp, b.image_size, b.checksum) == id)
        }
    });
    index.map(|i| &BUILDS[i])
}

/// The running game as `Code`.
pub(in crate::game) struct Running {
    base: usize,
    sections: Vec<sigscan::Section>,
}

impl Running {
    pub(in crate::game) fn new() -> Option<Self> {
        let (base, _) = image();
        if base == 0 || !crate::game::memory::readable(base, 0x1000) {
            return None;
        }
        // The header page is mapped for the module's lifetime.
        let header = unsafe { std::slice::from_raw_parts(base as *const u8, 0x1000) };
        Some(Self {
            base,
            sections: sigscan::sections(header)?,
        })
    }
}

impl Code for Running {
    fn find(&self, signature: &str) -> Vec<usize> {
        patch::find_all(signature, 256)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| a.checked_sub(self.base))
            .collect()
    }
    fn bytes(&self, rva: usize, len: usize) -> Option<&[u8]> {
        let (_, end) = image();
        let at = self.base.checked_add(rva)?;
        if at.checked_add(len)? > end || !crate::game::memory::readable_span(at, len) {
            return None;
        }
        // Mapped image memory, checked readable just now; the image stays
        // loaded while the client runs.
        Some(unsafe { std::slice::from_raw_parts(at as *const u8, len) })
    }
    fn section(&self, name: &str) -> Option<(usize, usize)> {
        let s = self.sections.iter().find(|s| s.name == name)?;
        Some((s.rva as usize, s.virtual_size as usize))
    }
    fn base(&self) -> usize {
        self.base
    }
}

/// How far the build-specific features may trust the running executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Trust {
    /// Not analyzed and unlike the analyzed builds (signatures missing):
    /// build-specific features stay off.
    None,
    /// Not analyzed, but the required baseline signatures are present.
    /// Each optional feature still has to validate its own dependencies.
    Reads,
    /// An analyzed build, or a patch whose build-specific addresses were all
    /// derived and checked: core action backends may prepare themselves.
    /// Optional features and their ABI checks are still independent.
    Full,
}

struct Identity {
    trust: Trust,
    addresses: Addresses,
}

/// The trust in an executable: an analyzed `build` is trusted fully with its
/// table. Otherwise it must have every required baseline signature (`missing` counts
/// those found nowhere) to be trusted at all, and every derived address to be
/// trusted fully. `derive` runs only when needed.
fn decide(
    build: Option<&Build>,
    missing: impl FnOnce() -> usize,
    derive: impl FnOnce() -> Addresses,
) -> Identity {
    if let Some(build) = build {
        return Identity {
            trust: Trust::Full,
            addresses: build.into(),
        };
    }
    if missing() != 0 {
        return Identity {
            trust: Trust::None,
            addresses: Addresses::default(),
        };
    }
    let addresses = derive();
    let trust = if addresses.complete() {
        Trust::Full
    } else {
        Trust::Reads
    };
    Identity { trust, addresses }
}

fn identity() -> &'static Identity {
    static IDENTITY: OnceLock<Identity> = OnceLock::new();
    IDENTITY.get_or_init(|| {
        let identity = decide(
            build(),
            || patch::missing_baseline().len(),
            || Running::new().map(|r| derive(&r)).unwrap_or_default(),
        );
        if build().is_none() {
            match identity.trust {
                Trust::None => log!(
                    "game: 未解析のビルドで、基本機能の必須シグネチャが不足しています。ビルド固有の機能は無効です（compatdebugで診断）"
                ),
                Trust::Reads => log!(
                    "game: 未解析のビルドです。シグネチャは確認できましたが、アドレスの一部を求められないため、読み取り機能だけを有効にします ({:X?})",
                    identity.addresses
                ),
                Trust::Full => log!(
                    "game: 未解析のビルドです。基本シグネチャとアドレスを確認しました。各機能は依存関係とABIを個別に照合します"
                ),
            }
        }
        identity
    })
}

/// How far the running executable is trusted (see `Trust`).
pub(crate) fn trust() -> Trust {
    identity().trust
}

/// The build-specific addresses of the running executable.
pub(crate) fn addresses() -> &'static Addresses {
    &identity().addresses
}

/// Exact analyzed PE identity; patches do not inherit the displayed version.
pub(crate) fn build_name() -> Option<&'static str> {
    build().map(|b| b.name)
}
pub(crate) fn diagnostic() -> String {
    let (base, end) = image();
    if base == 0 {
        return "PE unavailable".into();
    }
    let pe = unsafe { read::<u32>(base + 0x3c) } as usize;
    let identity = if pe < 0xf00 {
        unsafe {
            format!(
                "timestamp={:#x} size={:#x} checksum={:#x}",
                read::<u32>(base + pe + 8),
                end - base,
                read::<u32>(base + pe + 24 + 64)
            )
        }
    } else {
        "unavailable".into()
    };
    format!(
        "PE {identity}\nexact_build={:?} policy={:?}\n\
         NOTE: MATCH is an anchor result, not proof of ABI compatibility.\n{}\n\
         Skin raw: {:?}\nSkin compatible: {:?}",
        build_name(),
        trust(),
        patch::signature_diagnostic(),
        crate::game::skins::layout::raw(),
        crate::game::skins::layout::model()
    )
}
/// Whether features that only read the game may run: an analyzed build, or
/// a patch of one (`Trust::Reads` or better).
pub fn known_build() -> bool {
    trust() >= Trust::Reads
}

/// Whether features that write to the game or act in it may run: an analyzed
/// build, or a patch whose addresses were all derived (`Trust::Full`).
pub fn actions_build() -> bool {
    trust() == Trust::Full
}

/// For messages: the builds the build-specific features support.
pub(crate) const SUPPORTED: &str =
    "Minecraft 1.26.6301.0 / 1.26.65.1 x64, or a patch of them that passes the run-time checks";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signatures::{
        FISH_EVENT_SIGNATURE, GET_BLOCK_SIGNATURE, INVENTORY_REQUEST_BEGIN_SIGNATURE,
    };

    /// Where each analyzed build's executable is kept on this machine.
    fn executable(build: &Build) -> Option<Vec<u8>> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)?;
        let path = match build.name {
            "1.26.6301.0" => root.join(
                "Microsoft.MinecraftUWP_1.26.6301.0_x64__8wekyb3d8bbwe/Minecraft.Windows.exe",
            ),
            _ => std::path::PathBuf::from(
                r"C:\XboxGames\Minecraft for Windows\Content\Minecraft.Windows.exe",
            ),
        };
        std::fs::read(path).ok()
    }

    /// An executable file as `Code`.
    struct File {
        image: Vec<u8>,
        sections: Vec<sigscan::Section>,
    }
    impl File {
        fn new(image: Vec<u8>) -> Self {
            let sections = sigscan::sections(&image[..0x1000]).unwrap();
            Self { image, sections }
        }
    }
    impl Code for File {
        fn find(&self, signature: &str) -> Vec<usize> {
            let text = self.sections.iter().find(|s| s.name == ".text").unwrap();
            let code = &self.image[text.raw_offset as usize..][..text.raw_size as usize];
            let pattern = signature.parse::<sigscan::Pattern>().unwrap();
            let found = pattern.find_all(code, 256);
            found.into_iter().map(|at| at + text.rva as usize).collect()
        }
        fn bytes(&self, rva: usize, len: usize) -> Option<&[u8]> {
            let s = self
                .sections
                .iter()
                .find(|s| (s.rva as usize..(s.rva + s.raw_size) as usize).contains(&rva))?;
            let at = s.raw_offset as usize + rva - s.rva as usize;
            (rva + len <= (s.rva + s.raw_size) as usize).then(|| &self.image[at..at + len])
        }
        fn section(&self, name: &str) -> Option<(usize, usize)> {
            let s = self.sections.iter().find(|s| s.name == name)?;
            Some((s.rva as usize, s.virtual_size as usize))
        }
        fn base(&self) -> usize {
            0x1_4000_0000
        }
    }

    /// The running-image view reads this test executable's own sections,
    /// across their regions, and refuses ranges past the image.
    #[test]
    fn running_image_reads_its_sections_and_nothing_beyond() {
        let running = Running::new().expect("own image");
        assert_eq!(running.bytes(0, 2), Some(&b"MZ"[..]));
        let (start, size) = running.section(".rdata").expect(".rdata");
        let all = running.bytes(start, size).expect("whole .rdata");
        assert_eq!(all.len(), size);
        let (text, _) = running.section(".text").expect(".text");
        assert!(
            running.bytes(text, 16).is_some(),
            "executable pages are readable"
        );
        let (_, end) = image();
        assert_eq!(running.bytes(end - running.base() - 4, 8), None);
        assert_eq!(running.section(".nonexistent"), None);
    }

    #[test]
    fn unknown_builds_are_trusted_by_what_they_pass() {
        let table = Addresses::from(&BUILDS[1]);
        let never = || -> Addresses { panic!("derived for an analyzed build") };
        // Analyzed: fully, with its own table, without deriving.
        let known = decide(Some(&BUILDS[1]), || panic!("counted"), never);
        assert_eq!((known.trust, known.addresses), (Trust::Full, table));
        // A signature missing: nothing, without deriving.
        let unlike = decide(None, || 1, never);
        assert_eq!(
            (unlike.trust, unlike.addresses),
            (Trust::None, Addresses::default())
        );
        // All signatures and all addresses: fully.
        let patched = decide(None, || 0, || table);
        assert_eq!(patched.trust, Trust::Full);
        // One address missing: read-only features, keeping what was derived.
        let partial = Addresses {
            hook_vtable: None,
            ..table
        };
        let reads = decide(None, || 0, || partial);
        assert_eq!((reads.trust, reads.addresses), (Trust::Reads, partial));
        assert!(Trust::None < Trust::Reads && Trust::Reads < Trust::Full);
        let missing = |pattern: &str| {
            crate::signatures::BUILT_IN
                .iter()
                .filter(|&&s| s == pattern && crate::signatures::baseline_required(s))
                .count()
        };
        let optional = decide(
            None,
            || missing(crate::signatures::SKIN_GEOMETRY_FIELD_SIGNATURE),
            || table,
        );
        assert_eq!(optional.trust, Trust::Full);
        let core = decide(
            None,
            || missing(crate::signatures::ACCESSOR_SIGNATURE),
            never,
        );
        assert_eq!(core.trust, Trust::None);
    }

    /// On each analyzed build, deriving the addresses from code (what a build
    /// that was not analyzed relies on) gives exactly the analyzed table.
    #[test]
    fn derived_addresses_match_the_analyzed_tables() {
        let mut checked = 0;
        for build in &BUILDS {
            let Some(image) = executable(build) else {
                continue;
            };
            let pe = u32::from_le_bytes(image[0x3C..0x40].try_into().unwrap()) as usize;
            if u32::from_le_bytes(image[pe + 8..pe + 12].try_into().unwrap()) != build.timestamp {
                continue;
            }
            let derived = derive(&File::new(image));
            assert!(derived.complete(), "{} {derived:X?}", build.name);
            assert_eq!(derived, Addresses::from(build), "{}", build.name);
            checked += 1;
        }
        eprintln!("derived the addresses of {checked} build(s)");
    }
    #[test]
    fn captured_skin_layouts_match_both_analyzed_executables() {
        let mut checked = 0;
        for build in &BUILDS {
            let Some(image) = executable(build) else {
                continue;
            };
            let file = File::new(image);
            let pe = u32::from_le_bytes(file.image[0x3c..0x40].try_into().unwrap()) as usize;
            if u32::from_le_bytes(file.image[pe + 8..pe + 12].try_into().unwrap())
                != build.timestamp
            {
                continue;
            }
            let skin = crate::game::skins::layout::derive_raw(&file).expect(build.name);
            assert_eq!(
                (skin.actor_ref, skin.id, skin.image, skin.data, skin.length),
                (0xad8, 0x20, 0x80, 0x20, 0x28)
            );
            checked += 1;
        }
        eprintln!("captured skin fields from {checked} executable(s)");
    }

    /// Each build's table agrees with its executable: its identity, and every
    /// vtable whose slot must hold a function found by signature. Skipped for
    /// executables not present (the installed game may have been updated).
    #[test]
    fn build_tables_match_their_executables() {
        let mut checked = 0;
        for build in &BUILDS {
            let Some(image) = executable(build) else {
                continue;
            };
            let pe = u32::from_le_bytes(image[0x3C..0x40].try_into().unwrap()) as usize;
            let word = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
            if (word(pe + 8), word(pe + 24 + 56), word(pe + 24 + 64))
                != (build.timestamp, build.image_size, build.checksum)
            {
                // The installed game is another version now.
                continue;
            }
            let sections = sigscan::sections(&image[..0x1000]).unwrap();
            let raw = |rva: usize| {
                sections.iter().find_map(|s| {
                    let start = s.rva as usize;
                    (start..start + s.raw_size as usize)
                        .contains(&rva)
                        .then(|| s.raw_offset as usize + rva - start)
                })
            };
            let text = sections.iter().find(|s| s.name == ".text").unwrap();
            let code = &image[text.raw_offset as usize..][..text.raw_size as usize];
            let function = |signature: &str| {
                let found = signature
                    .parse::<sigscan::Pattern>()
                    .unwrap()
                    .find_all(code, 2);
                assert_eq!(found.len(), 1, "{} {signature}", build.name);
                found[0] + text.rva as usize
            };
            let slot = |table: usize, offset: usize| {
                let at = raw(table + offset).unwrap();
                // Preferred image base 0x140000000.
                u64::from_le_bytes(image[at..at + 8].try_into().unwrap()) as usize - 0x1_4000_0000
            };
            assert_eq!(
                slot(build.source_vtable, 0x10),
                function(GET_BLOCK_SIGNATURE)
            );
            assert_eq!(
                slot(build.hook_vtable, 0x220),
                function(FISH_EVENT_SIGNATURE)
            );
            assert_eq!(
                slot(build.inventory_manager_vtable, 0x28),
                function(INVENTORY_REQUEST_BEGIN_SIGNATURE)
            );
            // The air registry pointer lives in writable data.
            let data = sections.iter().find(|s| s.name == ".data").unwrap();
            assert!((data.rva as usize..(data.rva + data.virtual_size) as usize)
                .contains(&build.air_registry));
            checked += 1;
        }
        let ids: Vec<_> = BUILDS
            .iter()
            .map(|b| (b.timestamp, b.image_size, b.checksum))
            .collect();
        assert!(ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id)));
        eprintln!("checked {checked} build table(s) against executables");
    }
}
