//! The Windows file-open dialog for importing a profile. Shown from a
//! thread of its own so the game keeps rendering while it is open; the
//! chosen file is handed to the main loop as a `profile import` command.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

use crate::{console, log};

/// One dialog at a time.
static OPEN: AtomicBool = AtomicBool::new(false);

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens the dialog in `folder` (the profile folder) unless one is open.
pub(super) fn open_profile(folder: Option<PathBuf>) {
    if OPEN.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("profile-file-dialog".into())
        .spawn(move || {
            if let Some(path) = pick(folder) {
                console::submit(&format!("profile import \"{}\"", path.display()));
            }
            OPEN.store(false, Ordering::Release);
        });
    if let Err(e) = spawned {
        OPEN.store(false, Ordering::Release);
        log!("ファイル選択を開けません: {e}");
    }
}

/// Shows the dialog and returns the chosen file, if any.
fn pick(folder: Option<PathBuf>) -> Option<PathBuf> {
    // Pairs of display name and pattern, ending with an empty pair.
    let filter: Vec<u16> = "プロファイル (*.toml)\0*.toml\0すべてのファイル (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title = wide("プロファイルを読み込む");
    // The profile folder may not exist yet (nothing saved).
    let initial = folder
        .filter(|f| f.is_dir())
        .map(|f| wide(&f.to_string_lossy()));
    let mut file = vec![0u16; 1024];
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrInitialDir = initial.as_ref().map_or(std::ptr::null(), |w| w.as_ptr());
    dialog.lpstrTitle = title.as_ptr();
    // OFN_NOCHANGEDIR: the game's working directory must not move.
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    // SAFETY: every pointer refers to a buffer that outlives the call.
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        return None;
    }
    let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    (end > 0).then(|| PathBuf::from(String::from_utf16_lossy(&file[..end])))
}
