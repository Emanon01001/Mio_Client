//! Hardware discovery runs once, off the render thread, when DebugHUD is used.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};
static LINES: OnceLock<Vec<String>> = OnceLock::new();
static REQUESTED: AtomicBool = AtomicBool::new(false);
pub(super) fn request() {
    if LINES.get().is_none() {
        REQUESTED.store(true, Ordering::Release);
    }
}
pub(super) fn poll() {
    if REQUESTED.swap(false, Ordering::AcqRel) {
        let _ = LINES.get_or_init(discover);
    }
}

pub(super) fn lines() -> Option<&'static [String]> {
    LINES.get().map(Vec::as_slice)
}
fn discover() -> Vec<String> {
    let threads =
        unsafe { windows_sys::Win32::System::Threading::GetActiveProcessorCount(u16::MAX) };
    let threads = if threads == 0 {
        std::thread::available_parallelism().map_or(1, usize::from)
    } else {
        threads as usize
    };
    let mut out = vec![format!("CPU: {threads}x {}", cpu_name())];
    let adapters = adapters();
    if adapters.is_empty() {
        out.push("GPU（検出）: 未取得".into());
    } else {
        out.extend(
            adapters
                .into_iter()
                .map(|name| format!("GPU（検出）: {name}")),
        );
    }
    out
}
#[cfg(target_arch = "x86_64")]
fn cpu_name() -> String {
    use std::arch::x86_64::__cpuid;
    if __cpuid(0x8000_0000).eax < 0x8000_0004 {
        return "名前未取得".into();
    }
    let mut bytes = Vec::with_capacity(48);
    for leaf in 0x8000_0002..=0x8000_0004 {
        let r = __cpuid(leaf);
        for value in [r.eax, r.ebx, r.ecx, r.edx] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    String::from_utf8_lossy(&bytes)
        .trim_matches('\0')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
#[cfg(not(target_arch = "x86_64"))]
fn cpu_name() -> String {
    "名前未取得".into()
}

fn adapters() -> Vec<String> {
    use hudhook::windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for index in 0..16 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.Flags & 2 != 0 {
                continue;
            } // DXGI_ADAPTER_FLAG_SOFTWARE
            let length = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..length]);
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }
}
