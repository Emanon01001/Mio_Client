//! Read-only view of a whole file without copying it.
//!
//! The file is opened sharing read access only, so no process can write to
//! it while the view exists: the bytes cannot change under the audit, which
//! is what the size/time check of the copying reader approximates. When the
//! file cannot be opened that way (someone has it open for writing) or not
//! mapped, the caller falls back to reading a copy.
use std::{fs::File, path::Path};

pub(crate) struct Mapped {
    view: *const u8,
    len: usize,
    _file: File,
}

impl Mapped {
    /// `Ok(None)` when the file cannot be mapped read-only and unshared for
    /// writing; then reading a copy may still work.
    #[cfg(windows)]
    pub(crate) fn open(path: &Path, limit: u64) -> Result<Option<Self>, String> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            Storage::FileSystem::FILE_SHARE_READ,
            System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY},
        };
        let Ok(file) = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
        else {
            return Ok(None);
        };
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > limit {
            return Err(format!(
                "File is not regular or exceeds the {} MiB limit",
                limit / 1024 / 1024
            ));
        }
        // An empty file cannot be mapped; the copying reader handles it.
        let Ok(len) = usize::try_from(meta.len()) else {
            return Ok(None);
        };
        if len == 0 {
            return Ok(None);
        }
        // SAFETY: plain Win32 calls on a handle owned by `file`; the mapping
        // handle is closed once the view (which keeps it alive) exists.
        let view = unsafe {
            let mapping = CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            );
            if mapping.is_null() {
                return Ok(None);
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            CloseHandle(mapping);
            view.Value as *const u8
        };
        if view.is_null() {
            return Ok(None);
        }
        Ok(Some(Self {
            view,
            len,
            _file: file,
        }))
    }

    #[cfg(not(windows))]
    pub(crate) fn open(_path: &Path, _limit: u64) -> Result<Option<Self>, String> {
        Ok(None)
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        // SAFETY: the view covers `len` readable bytes until drop, and the
        // file cannot be written while it is open (see the module comment).
        unsafe { std::slice::from_raw_parts(self.view, self.len) }
    }
}

// SAFETY: the view is process-wide read-only memory; any thread may read or
// unmap it, and it is unmapped once (on drop).
unsafe impl Send for Mapped {}

impl Drop for Mapped {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: `view` came from MapViewOfFile and is unmapped once.
        unsafe {
            windows_sys::Win32::System::Memory::UnmapViewOfFile(
                windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.view as *mut _,
                },
            );
        }
    }
}
