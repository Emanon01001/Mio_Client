//! User-defined byte patches, located by signature in the game's main image.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::Instant;

use sigscan::{Pattern, PatternSet};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::config;
use crate::log;
use crate::modules::{key_or_warn, Module};

/// The executable sections of the main module, as (address, bytes) ranges.
fn executable_ranges() -> Vec<(usize, &'static [u8])> {
    unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as usize;
        if base == 0 {
            return Vec::new();
        }
        // PE headers always occupy at least the first page of a mapped image.
        let headers = std::slice::from_raw_parts(base as *const u8, 0x1000);
        sigscan::sections(headers)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.executable && s.virtual_size > 0)
            .map(|s| {
                let addr = base + s.rva as usize;
                let bytes = std::slice::from_raw_parts(addr as *const u8, s.virtual_size as usize);
                (addr, bytes)
            })
            .collect()
    }
}

/// Matches kept per built-in signature by the shared pass; enough for every
/// `find` (2) and `find_all` caller (at most 256).
const SHARED_LIMIT: usize = 256;
/// Code at the start of the first range used to choose each pattern's index
/// key (see `sigscan::PatternSet::with_sample`).
const KEY_SAMPLE: usize = 4 << 20;
/// Most threads the shared pass is divided among.
const SCAN_THREADS: usize = 8;

/// Matches of every `signatures::BUILT_IN` pattern, found in one pass over
/// the game's code the first time any of them is needed: one pass costs about
/// as much as five separate scans, and there are over forty. Addresses are in
/// range order with up to `SHARED_LIMIT` per range, so their first `n` are
/// exactly what a separate scan limited to `n <= SHARED_LIMIT` returns.
///
/// Taken before hooks or patches change the code, a signature also keeps
/// resolving after something it covers was hooked or patched.
fn shared() -> &'static HashMap<&'static str, Vec<usize>> {
    static SHARED: OnceLock<HashMap<&'static str, Vec<usize>>> = OnceLock::new();
    SHARED.get_or_init(|| {
        let started = Instant::now();
        let ranges = executable_ranges();
        // `BUILT_IN` is checked to parse by a test; one that did not would
        // simply keep being scanned separately (and report its error there).
        let parsed: Vec<(&'static str, Pattern)> = crate::signatures::BUILT_IN
            .iter()
            .filter_map(|&text| text.parse().ok().map(|p| (text, p)))
            .collect();
        let sample = ranges
            .first()
            .map_or(&[][..], |(_, bytes)| &bytes[..bytes.len().min(KEY_SAMPLE)]);
        let set = PatternSet::with_sample(parsed.iter().map(|(_, p)| p.clone()).collect(), sample);
        let mut found: HashMap<&'static str, Vec<usize>> =
            parsed.iter().map(|&(text, _)| (text, Vec::new())).collect();
        // Divided among threads the pass shortens almost in proportion
        // (sigscan's scan_bench: 546 ms alone, 107 ms on 8). Leave a core to
        // the game, which keeps rendering meanwhile.
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(1))
            .clamp(1, SCAN_THREADS);
        for (addr, bytes) in &ranges {
            let results = set.find_all_parallel(bytes, SHARED_LIMIT, threads);
            for ((text, _), offsets) in parsed.iter().zip(results) {
                found
                    .get_mut(text)
                    .expect("every parsed signature has an entry")
                    .extend(offsets.into_iter().map(|off| addr + off));
            }
        }
        log!(
            "{} built-in signatures resolved in one pass ({} ms, {threads} threads)",
            found.len(),
            started.elapsed().as_millis()
        );
        found
    })
}

/// Up to `limit` matches, in address-range order.
fn matches(signature: &str, limit: usize) -> Result<Vec<usize>, String> {
    if limit <= SHARED_LIMIT && crate::signatures::BUILT_IN.contains(&signature) {
        if let Some(found) = shared().get(signature) {
            return Ok(found.iter().take(limit).copied().collect());
        }
    }
    let pattern = signature
        .parse::<Pattern>()
        .map_err(|e| format!("signature: {e}"))?;
    let mut found = Vec::new();
    for (addr, bytes) in executable_ranges() {
        let left = limit.saturating_sub(found.len());
        if left == 0 {
            break;
        }
        found.extend(
            pattern
                .find_all(bytes, left)
                .into_iter()
                .map(|off| addr + off),
        );
    }
    Ok(found)
}

/// The only match, or why there is not exactly one.
fn unique(found: &[usize]) -> Result<usize, String> {
    match found {
        [only] => Ok(*only),
        [] => Err("シグネチャが見つかりません".into()),
        _ => Err("シグネチャが一意に一致しません".into()),
    }
}

/// Required baseline signatures missing from the one shared scan. Optional
/// read backends check their own anchors; derivation anchors have fallbacks.
pub fn missing_baseline() -> Vec<&'static str> {
    let found = shared();
    crate::signatures::BUILT_IN
        .iter()
        .copied()
        .filter(|s| {
            crate::signatures::baseline_required(s) && found.get(s).is_none_or(Vec::is_empty)
        })
        .collect()
}
/// Uses the pre-hook shared scan; never rescans patched instructions.
pub(crate) fn signature_diagnostic() -> String {
    use std::fmt::Write;
    let found = shared();
    let mut out = String::new();
    for e in crate::signatures::CATALOG {
        let n = found.get(e.pattern).map_or(0, Vec::len);
        let scope = if crate::signatures::baseline_required(e.pattern) {
            "baseline"
        } else {
            "optional/derived"
        };
        let status = match n {
            0 => "MISSING",
            1 => "MATCH",
            _ => "CANDIDATES",
        };
        let _ = writeln!(out, "{status} {} count={n} scope={scope}", e.name);
    }
    out
}

pub fn find(signature: &str) -> Result<usize, String> {
    unique(&matches(signature, 2)?)
}

/// Addresses of up to `limit` matches of the signature in the game's
/// executable code, for anchors that are told apart by what they refer to.
pub fn find_all(signature: &str, limit: usize) -> Result<Vec<usize>, String> {
    matches(signature, limit)
}

/// The same as `find(exact)`, where `exact` is the built-in `shape` with
/// `fixed` written over its wildcards at `at`: the shape's matches come from
/// the shared pass and are told apart by those bytes. Scans for `exact` when
/// the shape matched too often for its list to be complete.
pub fn find_filled(shape: &str, at: usize, fixed: &[u8], exact: &str) -> Result<usize, String> {
    let all = matches(shape, SHARED_LIMIT)?;
    if all.len() >= SHARED_LIMIT {
        return find(exact);
    }
    // Each address is a match of `shape`, which covers `at..at + fixed.len()`
    // (the callers' tests check that), so the bytes are mapped code.
    let found: Vec<usize> = all
        .into_iter()
        .filter(|&addr| unsafe {
            std::slice::from_raw_parts((addr + at) as *const u8, fixed.len()) == fixed
        })
        .take(2)
        .collect();
    unique(&found)
}

fn write(addr: usize, data: &[u8]) -> bool {
    unsafe {
        let ptr = addr as *mut c_void;
        let mut old = 0u32;
        if VirtualProtect(ptr, data.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len());
        VirtualProtect(ptr, data.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), ptr, data.len());
        true
    }
}
/// A configured signed offset need not stay inside the matched code range.
/// Validate its entire readable span before preserving the original bytes.
fn original_bytes(addr: usize, len: usize) -> Option<Vec<u8>> {
    if addr == 0 || len == 0 {
        return None;
    }
    let end = addr.checked_add(len)?;
    let mut cursor = addr;
    while cursor < end {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            VirtualQuery(
                cursor as *const c_void,
                &mut info,
                std::mem::size_of_val(&info),
            )
        } == 0
            || info.State != MEM_COMMIT
            || info.Protect & PAGE_GUARD != 0
            || info.Protect
                & (PAGE_READONLY
                    | PAGE_READWRITE
                    | PAGE_WRITECOPY
                    | PAGE_EXECUTE_READ
                    | PAGE_EXECUTE_READWRITE
                    | PAGE_EXECUTE_WRITECOPY)
                == 0
        {
            return None;
        }
        let next = (info.BaseAddress as usize).checked_add(info.RegionSize)?;
        if next <= cursor {
            return None;
        }
        cursor = next.min(end);
    }
    Some(unsafe { std::slice::from_raw_parts(addr as *const u8, len) }.to_vec())
}

pub struct Patch {
    name: String,
    key: u16,
    enabled: bool,
    /// Resolved patch address, or why it could not be resolved.
    addr: Result<usize, String>,
    bytes: Vec<u8>,
    original: Vec<u8>,
}

impl Patch {
    fn new(cfg: &config::Patch) -> Self {
        let bytes = sigscan::parse_hex_bytes(&cfg.bytes).map_err(|e| format!("bytes: {e}"));
        let addr = find(&cfg.signature)
            .and_then(|at| {
                at.checked_add_signed(cfg.offset)
                    .ok_or_else(|| "オフセットが範囲外です".to_string())
            })
            .and_then(|at| bytes.as_ref().map(|_| at).map_err(Clone::clone));

        match &addr {
            Ok(at) => log!("patch `{}` resolved at 0x{at:X}", cfg.name),
            Err(e) => log!("パッチ`{}`を利用できません: {e}", cfg.name),
        }
        Self {
            name: cfg.name.clone(),
            key: key_or_warn(&cfg.name, &cfg.key),
            enabled: false,
            addr,
            bytes: bytes.unwrap_or_default(),
            original: Vec::new(),
        }
    }
}

impl Module for Patch {
    fn name(&self) -> &str {
        &self.name
    }

    fn key(&self) -> u16 {
        self.key
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        let at = match &self.addr {
            Ok(at) => *at,
            Err(e) => {
                log!("パッチ`{}`を利用できません: {e}", self.name);
                return;
            }
        };
        if on == self.enabled {
            return;
        }
        let ok = if on {
            let Some(original) = original_bytes(at, self.bytes.len()) else {
                log!("パッチ`{}`: 対象メモリを読み取れません", self.name);
                return;
            };
            self.original = original;
            write(at, &self.bytes)
        } else {
            write(at, &self.original)
        };
        if ok {
            self.enabled = on;
        } else {
            log!("パッチ`{}`: メモリへ書き込めません", self.name);
        }
    }

    fn status(&self) -> String {
        match &self.addr {
            Ok(at) => format!("@0x{at:X}"),
            Err(e) => format!("({e})"),
        }
    }
}

pub fn load(configs: &[config::Patch]) -> Vec<Patch> {
    configs.iter().map(Patch::new).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS,
    };
    #[test]
    fn rejects_unmapped_offsets_and_spans_crossing_an_unreadable_page() {
        unsafe {
            let p = VirtualAlloc(
                std::ptr::null(),
                8192,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            );
            assert!(!p.is_null());
            let address = p as usize;
            std::ptr::copy_nonoverlapping([0x90u8, 0x91, 0x92].as_ptr(), p.cast::<u8>(), 3);
            let valid = original_bytes(address, 3);
            let mut old = 0;
            let protected = VirtualProtect(
                (address + 4096) as *const c_void,
                4096,
                PAGE_NOACCESS,
                &mut old,
            ) != 0;
            let crossed = original_bytes(address + 4094, 4);
            let hidden = original_bytes(address + 4096, 1);
            VirtualFree(p, 0, MEM_RELEASE);
            assert_eq!(valid, Some(vec![0x90, 0x91, 0x92]));
            assert!(protected && crossed.is_none() && hidden.is_none());
            assert!(original_bytes(0, 8).is_none());
            assert!(original_bytes(usize::MAX, 8).is_none());
        }
    }
}
