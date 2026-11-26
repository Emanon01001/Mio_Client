//! Native reads and committed-memory/vector validation.
use std::ffi::c_void;
use std::mem::{size_of, MaybeUninit};
use std::sync::OnceLock;
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Memory::{
    VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32QueryWorkingSetEx, PSAPI_WORKING_SET_EX_INFORMATION,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub(crate) unsafe fn read<T: Copy>(addr: usize) -> T {
    (addr as *const T).read_unaligned()
}

/// Copy a small native field without directly dereferencing the source.
/// Windows reports an unreadable range or partial copy as a failed sample.
///
/// Safety: every bit pattern in the source must be valid for `T`.
pub(in crate::game) unsafe fn copy_field<T: Copy>(base: usize, offset: usize) -> Option<T> {
    let addr = base.checked_add(offset)?;
    let len = size_of::<T>();
    if base == 0 || addr.checked_add(len).is_none() {
        return None;
    }
    let mut value = MaybeUninit::<T>::uninit();
    let mut copied = 0;
    let ok = ReadProcessMemory(
        GetCurrentProcess(),
        addr as *const c_void,
        value.as_mut_ptr().cast(),
        len,
        &mut copied,
    );
    if ok == 0 || copied != len {
        return None;
    }
    Some(value.assume_init())
}

/// Whether `len` bytes at `addr` are committed, readable memory.
pub(crate) fn readable(addr: usize, len: usize) -> bool {
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
    if addr == 0 || unsafe { VirtualQuery(addr as *const c_void, &mut info, size) } == 0 {
        return false;
    }
    let end = info.BaseAddress as usize + info.RegionSize;
    info.State == MEM_COMMIT
        && info.Protect & (PAGE_READONLY | PAGE_READWRITE) != 0
        && info.Protect & PAGE_GUARD == 0
        && addr.checked_add(len).is_some_and(|last| last <= end)
}

/// Like `readable`, for a range that may span several regions (a section),
/// also accepting executable code.
pub(crate) fn readable_span(addr: usize, len: usize) -> bool {
    let Some(end) = addr.checked_add(len) else {
        return false;
    };
    let mut at = addr;
    while at < end {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
        if unsafe { VirtualQuery(at as *const c_void, &mut info, size) } == 0
            || info.State != MEM_COMMIT
            || info.Protect
                & (PAGE_READONLY | PAGE_READWRITE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE)
                == 0
            || info.Protect & PAGE_GUARD != 0
        {
            return false;
        }
        at = info.BaseAddress as usize + info.RegionSize;
    }
    addr != 0
}

/// Protection checks shared only within one read-only enumeration. Drop this
/// before calling native actions; no memory region survives into another tick.
pub(in crate::game) struct ReadBatch {
    regions: [(usize, usize); 16],
    next: usize,
    queries: usize,
    /// Resident-page checks live only for this enumeration, like `regions`.
    pages: [usize; 128],
}

const MAX_PAGE_QUERY: usize = 16;
fn page_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        let mut info = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut info) };
        info.dwPageSize as usize
    })
}
fn page_slot(page: usize, size: usize) -> usize {
    let index = page / size;
    (index ^ (index >> 7)) & 127
}
/// A valid working-set entry describes this page's current Win32 protection.
/// Invalid/nonresident entries have no usable protection bits; use VirtualQuery.
fn resident_readable(flags: usize) -> bool {
    let protection = (flags >> 4) & 0x7ff;
    flags & 1 != 0
        && protection & (PAGE_READONLY | PAGE_READWRITE) as usize != 0
        && protection & PAGE_GUARD as usize == 0
}
impl ReadBatch {
    pub fn new() -> Self {
        Self {
            regions: [(0, 0); 16],
            next: 0,
            queries: 0,
            pages: [0; 128],
        }
    }
    pub fn queries(&self) -> usize {
        self.queries
    }

    /// Bounded MSVC string copy sharing this enumeration's region checks.
    pub unsafe fn string(&mut self, address: usize, max_length: usize) -> Option<String> {
        self.with_string(address, max_length, str::to_owned)
    }

    /// Validated text is borrowed only inside this callback on the game thread.
    /// Valid UTF-8 needs no temporary allocation; invalid bytes retain the
    /// existing lossy-decoding behavior. Never retain native string storage.
    pub unsafe fn with_string<T>(
        &mut self,
        address: usize,
        max_length: usize,
        use_text: impl FnOnce(&str) -> T,
    ) -> Option<T> {
        if !self.readable(address, 32) {
            return None;
        }
        let length: usize = read(address + 16);
        let capacity: usize = read(address + 24);
        if length > capacity || length > max_length || (capacity < 16 && capacity != 15) {
            return None;
        }
        if length == 0 {
            return Some(use_text(""));
        }
        let data = if capacity >= 16 {
            read::<usize>(address)
        } else {
            address
        };
        if !self.readable(data, length) {
            return None;
        }
        Some(use_text(&String::from_utf8_lossy(
            std::slice::from_raw_parts(data as *const u8, length),
        )))
    }
    pub fn readable(&mut self, addr: usize, len: usize) -> bool {
        let Some(last) = addr.checked_add(len) else {
            return false;
        };
        if addr == 0 {
            return false;
        }
        if self
            .regions
            .iter()
            .any(|&(start, end)| start <= addr && addr < end && last <= end)
        {
            return true;
        }
        // VirtualQuery walks to the end of a homogeneous region. On a large
        // committed game heap even one call can take milliseconds. Query only
        // the pages being read when they are resident. Never infer protection
        // from invalid entries, and never carry checks into another sample.
        if self.resident_range(addr, len) {
            return true;
        }
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        self.queries += 1;
        if unsafe {
            VirtualQuery(
                addr as *const c_void,
                &mut info,
                std::mem::size_of_val(&info),
            )
        } == 0
            || info.State != MEM_COMMIT
            || info.Protect & (PAGE_READONLY | PAGE_READWRITE) == 0
            || info.Protect & PAGE_GUARD != 0
        {
            return false;
        }
        let start = info.BaseAddress as usize;
        let Some(end) = start.checked_add(info.RegionSize) else {
            return false;
        };
        self.regions[self.next] = (start, end);
        self.next = (self.next + 1) % self.regions.len();
        last <= end
    }
    fn resident_range(&mut self, addr: usize, len: usize) -> bool {
        let size = page_size();
        if size == 0 {
            return false;
        }
        let first = addr / size * size;
        // `readable` has checked address overflow, including zero-length reads.
        let last = (addr + len.saturating_sub(1)) / size * size;
        let count = (last - first) / size + 1;
        if count > MAX_PAGE_QUERY {
            return false;
        }
        let mut entries = [PSAPI_WORKING_SET_EX_INFORMATION::default(); MAX_PAGE_QUERY];
        let mut missing = 0;
        for i in 0..count {
            let page = first + i * size;
            if page != 0 && self.pages[page_slot(page, size)] == page {
                continue;
            }
            entries[missing].VirtualAddress = page as *mut c_void;
            missing += 1;
        }
        if missing == 0 {
            return true;
        }
        self.queries += 1;
        let ok = unsafe {
            K32QueryWorkingSetEx(
                GetCurrentProcess(),
                entries.as_mut_ptr().cast(),
                (missing * std::mem::size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32,
            )
        };
        if ok == 0
            || entries[..missing]
                .iter()
                .any(|entry| !resident_readable(unsafe { entry.VirtualAttributes.Flags }))
        {
            return false;
        }
        for entry in &entries[..missing] {
            let page = entry.VirtualAddress as usize;
            self.pages[page_slot(page, size)] = page;
        }
        true
    }
    pub unsafe fn span(&mut self, address: usize, element: usize) -> Option<(usize, usize)> {
        if !self.readable(address, 16) {
            return None;
        }
        let (begin, end) = (read::<usize>(address), read::<usize>(address + 8));
        (end >= begin && (end - begin) % element == 0 && self.readable(begin, end - begin))
            .then_some((begin, end))
    }
}

/// A readable (begin, end) pointer pair, as the first two words of a
/// std::vector whose elements are `element` bytes.
pub(in crate::game) unsafe fn span(addr: usize, element: usize) -> Option<(usize, usize)> {
    if !readable(addr, 16) {
        return None;
    }
    let (begin, end): (usize, usize) = (read(addr), read(addr + 8));
    let ok = end >= begin && (end - begin) % element == 0 && readable(begin, end - begin);
    ok.then_some((begin, end))
}

/// An owned copy of an MSVC x64 std::string, bounded before dereferencing it.
pub(crate) unsafe fn string(address: usize, max_length: usize) -> Option<String> {
    ReadBatch::new().string(address, max_length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, VirtualProtect, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS,
    };
    #[test]
    fn copy_field_handles_invalid_addresses_and_overflow() {
        let values = [1u32, 42];
        let base = values.as_ptr() as usize;
        unsafe {
            assert_eq!(copy_field::<u32>(base, 4), Some(42));
            assert_eq!(copy_field::<u32>(0, 4), None);
            assert_eq!(copy_field::<u32>(usize::MAX, 1), None);
            assert_eq!(copy_field::<u32>(usize::MAX - 1, 0), None);
            assert_eq!(copy_field::<u32>(1, 0), None);
        }
    }

    #[test]
    fn copy_field_rejects_a_partial_copy_across_an_inaccessible_page() {
        let size = page_size();
        let allocation = Pages::new(size * 2);
        allocation.protect(size, PAGE_NOACCESS);
        let base = allocation.base();
        unsafe {
            assert_eq!(copy_field::<u32>(base, size - 4), Some(0));
            assert_eq!(copy_field::<u64>(base, size - 4), None);
            assert_eq!(copy_field::<u32>(base, size), None);
        }
    }

    #[test]
    fn strings_copy_inline_and_heap_data_and_validate_capacity_and_bounds() {
        let mut inline = [0u64; 4];
        let mut heap = [0u64; 4];
        let text = "a player name that exceeds inline capacity";
        let mut m = ReadBatch::new();
        unsafe {
            let bytes = inline.as_mut_ptr().cast::<u8>();
            std::ptr::copy_nonoverlapping(b"Steve".as_ptr(), bytes, 5);
            inline[2] = 5;
            inline[3] = 15;
            assert_eq!(m.string(inline.as_ptr() as usize, 64), Some("Steve".into()));
            heap[0] = text.as_ptr() as u64;
            heap[2] = text.len() as u64;
            heap[3] = text.len() as u64;
            assert_eq!(m.string(heap.as_ptr() as usize, 64), Some(text.into()));
            assert_eq!(m.string(heap.as_ptr() as usize, 8), None);
            inline[3] = 4;
            assert_eq!(m.string(inline.as_ptr() as usize, 64), None);
            assert_eq!(m.string(0, 64), None);
            inline[2] = 1;
            inline[3] = 15;
            (inline.as_mut_ptr() as *mut u8).write(0xff);
            assert_eq!(
                m.with_string(inline.as_ptr() as usize, 64, str::to_owned),
                Some("�".into())
            );
        }
    }
    #[test]
    fn batch_respects_region_boundaries_and_is_not_reused_between_scans() {
        struct Allocation(*mut c_void);
        impl Drop for Allocation {
            fn drop(&mut self) {
                unsafe {
                    VirtualFree(self.0, 0, MEM_RELEASE);
                }
            }
        }
        let allocation = Allocation(unsafe {
            VirtualAlloc(
                std::ptr::null(),
                8192,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        });
        assert!(!allocation.0.is_null());
        let base = allocation.0 as usize;
        let mut previous = 0;
        assert_ne!(
            unsafe {
                VirtualProtect(
                    (base + 4096) as *const c_void,
                    4096,
                    PAGE_NOACCESS,
                    &mut previous,
                )
            },
            0
        );
        {
            let mut memory = ReadBatch::new();
            assert!(memory.readable(base, 8));
            let first_queries = memory.queries();
            assert!((1..=2).contains(&first_queries));
            assert!(memory.readable(base + 8, 8));
            assert_eq!(memory.queries(), first_queries);
            assert!(!memory.readable(base + 4090, 16));
            assert!(!memory.readable(base + 4096, 8));
            assert!(!memory.readable(0, 8));
            assert!(!memory.readable(usize::MAX, 8));
        }
        assert_ne!(
            unsafe { VirtualProtect(allocation.0, 4096, PAGE_NOACCESS, &mut previous) },
            0
        );
        let mut next_scan = ReadBatch::new();
        assert!(!next_scan.readable(base, 8));
        assert_eq!(next_scan.queries(), 2);
    }

    #[test]
    fn resident_flags_require_valid_read_protection_and_reject_guard_or_execute_only() {
        let flags = |protection: u32| 1 | ((protection as usize) << 4);
        assert!(resident_readable(flags(PAGE_READONLY)));
        assert!(resident_readable(flags(PAGE_READWRITE)));
        for protection in [
            PAGE_NOACCESS,
            PAGE_EXECUTE_READ,
            PAGE_READWRITE | PAGE_GUARD,
        ] {
            assert!(!resident_readable(flags(protection)));
        }
        assert!(!resident_readable(flags(PAGE_READWRITE) & !1));
    }

    struct Pages(*mut c_void);
    impl Pages {
        fn new(bytes: usize) -> Self {
            let p = Self(unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    bytes,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            });
            assert!(!p.0.is_null());
            p
        }
        fn base(&self) -> usize {
            self.0 as usize
        }
        fn protect(&self, offset: usize, protection: u32) {
            let mut previous = 0;
            assert_ne!(
                unsafe {
                    VirtualProtect(
                        (self.base() + offset) as *const c_void,
                        page_size(),
                        protection,
                        &mut previous,
                    )
                },
                0
            );
        }
    }
    impl Drop for Pages {
        fn drop(&mut self) {
            unsafe {
                VirtualFree(self.0, 0, MEM_RELEASE);
            }
        }
    }
    #[test]
    fn resident_pages_validate_every_page_and_never_consume_guard_or_keep_old_protection() {
        let size = page_size();
        let allocation = Pages::new(size * 5);
        let base = allocation.base();
        for i in 0..5 {
            unsafe {
                ((base + i * size) as *mut u8).write_volatile(7);
            }
        }
        allocation.protect(size, PAGE_NOACCESS);
        allocation.protect(size * 3, PAGE_READWRITE | PAGE_GUARD);
        allocation.protect(size * 4, PAGE_READONLY);
        let mut m = ReadBatch::new();
        assert!(m.readable(base + 16, 8));
        assert_eq!(m.queries(), 1, "resident read uses the page query only");
        assert!(m.readable(base + 32, 8));
        assert_eq!(m.queries(), 1);
        assert!(!m.readable(base + size - 4, 8));
        assert!(
            !m.readable(base, size * 3),
            "readable endpoints cannot hide an inaccessible middle page"
        );
        assert!(!m.readable(base + size * 3, 1));
        assert!(m.readable(base + size * 4, 1));
        let mut info = MEMORY_BASIC_INFORMATION::default();
        assert_ne!(
            unsafe {
                VirtualQuery(
                    (base + size * 3) as *const c_void,
                    &mut info,
                    std::mem::size_of_val(&info),
                )
            },
            0
        );
        assert_ne!(
            info.Protect & PAGE_GUARD,
            0,
            "checking must not touch a guard page"
        );
        allocation.protect(0, PAGE_NOACCESS);
        assert!(!ReadBatch::new().readable(base + 16, 8));
        assert!(!ReadBatch::new().readable(usize::MAX - 4, 16));
    }

    #[test]
    #[ignore = "manual benchmark: sparse resident reads in a large committed allocation"]
    fn large_heap_validation_cost() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};
        let allocation = Pages::new(256 * 1024 * 1024);
        let addresses: Vec<_> = (0..96)
            .rev()
            .map(|i| allocation.base() + (i * 1957 + 17) * 1024)
            .collect();
        for &a in &addresses {
            unsafe {
                (a as *mut u8).write_volatile(1);
            }
        }
        let run = |fast: bool| {
            let mut times = Vec::new();
            for _ in 0..25 {
                let mut batch = ReadBatch::new();
                let start = Instant::now();
                for &a in &addresses {
                    let ok = if fast {
                        batch.readable(a, 32)
                    } else {
                        // Same descending addresses as the old region cache:
                        // every next address precedes the previously cached region.
                        readable(a, 32)
                    };
                    assert!(black_box(ok));
                }
                times.push(start.elapsed());
            }
            times.sort_unstable();
            times[times.len() / 2]
        };
        let slow: Duration = run(false);
        let fast = run(true);
        eprintln!("96 sparse resident reads / 256MiB: region checks {slow:?}, resident page checks {fast:?}");
    }
}
