//! Injection primitives shared by the CLI (`mcbe_injector`) and the GUI
//! (`mcbe_injector_gui`).

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenEventW, OpenProcess, SetEvent, WaitForSingleObject,
    EVENT_MODIFY_STATE, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;

pub const DEFAULT_PROCESS: &str = "Minecraft.Windows.exe";
pub const DLL_NAME: &str = "Mio_Client.dll";

pub fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Closes the wrapped handle on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn last_error(what: &str) -> String {
    format!("{what} failed (error {})", unsafe { GetLastError() })
}

/// The path belongs to the loader thread until that thread is known to have
/// exited. A timeout must never free memory LoadLibraryW may still be reading.
struct RemotePath<'a> {
    process: &'a Handle,
    address: *mut c_void,
    release: bool,
}
impl RemotePath<'_> {
    unsafe fn finish(&mut self, thread: HANDLE, timeout: u32) -> Result<(), String> {
        match WaitForSingleObject(thread, timeout) {
            WAIT_OBJECT_0 => self.release = true,
            WAIT_TIMEOUT => {
                return Err(concat!(
                    "LoadLibraryW timed out; loader may still be running ",
                    "(its path is retained until the target exits)"
                )
                .into());
            }
            _ => return Err(last_error("WaitForSingleObject(loader)")),
        }
        let mut code = 0u32;
        if GetExitCodeThread(thread, &mut code) == 0 {
            return Err(last_error("GetExitCodeThread"));
        }
        if code == 0 {
            return Err("LoadLibraryW failed inside the target process".into());
        }
        Ok(())
    }
}
impl Drop for RemotePath<'_> {
    fn drop(&mut self) {
        if self.release {
            unsafe {
                VirtualFreeEx(self.process.0, self.address, 0, MEM_RELEASE);
            }
        }
    }
}

pub fn find_process(name: &str) -> Result<u32, String> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateToolhelp32Snapshot"));
        }
        let snap = Handle(snap);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap.0, &mut entry);
        while ok != 0 {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if exe.eq_ignore_ascii_case(name) {
                return Ok(entry.th32ProcessID);
            }
            ok = Process32NextW(snap.0, &mut entry);
        }
    }
    Err(format!("process `{name}` is not running"))
}

pub fn inject(pid: u32, dll: &Path) -> Result<(), String> {
    let dll = dll
        .canonicalize()
        .map_err(|e| format!("DLL not found at {}: {e}", dll.display()))?;
    // canonicalize() yields a `\\?\` path, which LoadLibraryW accepts.
    let path = wide(dll.as_os_str());
    let bytes = path.len() * 2;

    unsafe {
        let process = OpenProcess(
            PROCESS_CREATE_THREAD
                | PROCESS_QUERY_INFORMATION
                | PROCESS_VM_OPERATION
                | PROCESS_VM_WRITE
                | PROCESS_VM_READ,
            0,
            pid,
        );
        if process.is_null() {
            return Err(last_error("OpenProcess"));
        }
        let process = Handle(process);

        let remote = VirtualAllocEx(
            process.0,
            null(),
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote.is_null() {
            return Err(last_error("VirtualAllocEx"));
        }
        let mut remote = RemotePath {
            process: &process,
            address: remote,
            release: true,
        };

        let result = (|| {
            if WriteProcessMemory(
                process.0,
                remote.address,
                path.as_ptr() as *const c_void,
                bytes,
                null_mut(),
            ) == 0
            {
                return Err(last_error("WriteProcessMemory"));
            }

            // kernel32 is mapped at the same address in every process of a session.
            let kernel32 = GetModuleHandleW(wide("kernel32.dll".as_ref()).as_ptr());
            let Some(load_library) = GetProcAddress(kernel32, c"LoadLibraryW".as_ptr().cast())
            else {
                return Err(last_error("GetProcAddress(LoadLibraryW)"));
            };
            let start: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(load_library);

            let thread = CreateRemoteThread(
                process.0,
                null(),
                0,
                Some(start),
                remote.address,
                0,
                null_mut(),
            );
            if thread.is_null() {
                return Err(last_error("CreateRemoteThread"));
            }
            let thread = Handle(thread);
            remote.release = false;
            remote.finish(thread.0, 15_000)
        })();
        result
    }
}

/// The client's eject event; it exists exactly while the client is running.
fn open_eject_event(pid: u32) -> Option<Handle> {
    let name = wide(format!("Local\\Mio_Client_eject_{pid}").as_ref());
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    (!event.is_null()).then_some(Handle(event))
}

/// Whether the client is currently running inside process `pid`.
pub fn is_loaded(pid: u32) -> bool {
    open_eject_event(pid).is_some()
}

pub fn eject(pid: u32) -> Result<(), String> {
    let event = open_eject_event(pid).ok_or("client is not loaded in that process")?;
    if unsafe { SetEvent(event.0) } == 0 {
        return Err(last_error("SetEvent"));
    }
    Ok(())
}

/// Starts Minecraft Bedrock the way the Start menu does. Returns once the
/// launch is requested; the process appears a moment later.
pub fn launch() -> Result<(), String> {
    // The packaged app cannot be started from its exe path, so go through the
    // shell: first the app's Start menu identity, then the `minecraft:` URI.
    const TARGETS: [&str; 2] = [
        r"shell:AppsFolder\Microsoft.MinecraftUWP_8wekyb3d8bbwe!Game",
        "minecraft:",
    ];
    const SW_SHOWNORMAL: i32 = 1;
    let mut code = 0;
    for target in TARGETS {
        let result = unsafe {
            ShellExecuteW(
                null_mut(),
                wide("open".as_ref()).as_ptr(),
                wide(target.as_ref()).as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecuteW reports success as a value greater than 32.
        code = result as usize;
        if code > 32 {
            return Ok(());
        }
    }
    Err(format!(
        "could not launch Minecraft (ShellExecute error {code}); is it installed?"
    ))
}

/// `Mio_Client.dll` next to the running executable.
pub fn default_dll() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(DLL_NAME)))
        .unwrap_or_else(|| PathBuf::from(DLL_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateThread, GetCurrentProcessId, INFINITE,
    };
    unsafe extern "system" fn loader_wait(event: *mut c_void) -> u32 {
        WaitForSingleObject(event, INFINITE);
        1
    }
    #[test]
    fn loader_timeout_retains_arguments_until_execution_has_finished() {
        unsafe {
            // A local blocked loader surrogate: no DLL injection or game interaction.
            let process = Handle(OpenProcess(
                PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_QUERY_INFORMATION,
                0,
                GetCurrentProcessId(),
            ));
            assert!(!process.0.is_null());
            let event = Handle(CreateEventW(null(), 1, 0, null()));
            assert!(!event.0.is_null());
            let thread = Handle(CreateThread(
                null(),
                0,
                Some(loader_wait),
                event.0,
                0,
                null_mut(),
            ));
            assert!(!thread.0.is_null());
            let address = VirtualAllocEx(
                process.0,
                null(),
                64,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            );
            assert!(!address.is_null());
            let mut path = RemotePath {
                process: &process,
                address,
                release: false,
            };
            let timed_out = path.finish(thread.0, 0).is_err();
            let retained = !path.release;
            drop(path);
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let queried =
                VirtualQueryEx(process.0, address, &mut info, std::mem::size_of_val(&info));
            let still_allocated = queried != 0 && info.State == MEM_COMMIT;
            SetEvent(event.0);
            let completed = WaitForSingleObject(thread.0, 5000) == WAIT_OBJECT_0;
            // The test knows completion now, so reclaim the deliberate timeout retention.
            if completed {
                VirtualFreeEx(process.0, address, 0, MEM_RELEASE);
            }
            assert!(timed_out && retained && still_allocated && completed);
            let address = VirtualAllocEx(
                process.0,
                null(),
                64,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            );
            let mut path = RemotePath {
                process: &process,
                address,
                release: false,
            };
            assert!(path.finish(thread.0, 0).is_ok());
            assert!(path.release);
        }
    }
}
