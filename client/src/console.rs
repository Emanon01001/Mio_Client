//! The client's log and command line. Lines are always kept in memory for the
//! in-game console (drawn by `gui.rs`); a separate console window showing the
//! same log and accepting the same commands is optional.

use std::collections::VecDeque;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Console::{
    AllocConsole, FreeConsole, ReadConsoleW, SetConsoleTitleW, WriteConsoleW,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use windows_sys::Win32::System::IO::CancelSynchronousIo;

static OUT: Mutex<Option<File>> = Mutex::new(None);

/// How many log lines the in-game console keeps.
const HISTORY: usize = 500;
static LINES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
/// Lines dropped from the front of `LINES` so far; changed only under its lock.
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Where submitted command lines go (the main loop).
static COMMANDS: Mutex<Option<Sender<String>>> = Mutex::new(None);

fn open_console(name: &str) -> Option<File> {
    OpenOptions::new().read(true).write(true).open(name).ok()
}

fn write_line(out: &mut File, line: &str) {
    let text = format!("[mcbe] {line}\r\n");
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut at = 0;
    while at < wide.len() {
        let mut written = 0;
        let ok = unsafe {
            WriteConsoleW(
                out.as_raw_handle(),
                wide[at..].as_ptr(),
                (wide.len() - at).min(u32::MAX as usize) as u32,
                &mut written,
                std::ptr::null(),
            )
        };
        if ok == 0 || written == 0 {
            // CONOUT$ is normally a console, but retain a UTF-8 fallback.
            if at == 0 {
                let _ = out.write_all(text.as_bytes());
            }
            break;
        }
        at += written as usize;
    }
}

/// Opens a console for the process. Returns true if we created it (and so
/// should free it on detach) rather than reusing one the host already had.
pub fn attach() -> bool {
    let owned = unsafe { AllocConsole() } != 0;
    if owned {
        let title: Vec<u16> = format!("{}\0", crate::brand::CLIENT_NAME)
            .encode_utf16()
            .collect();
        unsafe { SetConsoleTitleW(title.as_ptr()) };
    }
    let mut out = open_console("CONOUT$");
    // Show what was logged before the window existed.
    if let Some(out) = out.as_mut() {
        for line in LINES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            write_line(out, line);
        }
    }
    *OUT.lock().unwrap_or_else(|e| e.into_inner()) = out;
    owned
}

pub fn detach(owned: bool) {
    *OUT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if owned {
        unsafe { FreeConsole() };
    }
}

pub fn print(args: fmt::Arguments<'_>) {
    let line = args.to_string();
    if let Some(out) = OUT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        write_line(out, &line);
    }
    let mut lines = LINES.lock().unwrap_or_else(|e| e.into_inner());
    if lines.len() == HISTORY {
        lines.pop_front();
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
    lines.push_back(line);
}

/// Runs `f` over kept lines, oldest first, with the oldest line's number.
/// Lines are numbered from 0, so a caller can tell which it has already seen.
pub fn with_numbered_lines<R>(f: impl FnOnce(u64, &VecDeque<String>) -> R) -> R {
    let lines = LINES.lock().unwrap_or_else(|e| e.into_inner());
    f(DROPPED.load(Ordering::Relaxed), &lines)
}

pub fn text() -> String {
    with_numbered_lines(|_, lines| {
        let bytes =
            lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1) * 2;
        let mut text = String::with_capacity(bytes);
        for (i, line) in lines.iter().enumerate() {
            if i != 0 {
                text.push_str("\r\n");
            }
            text.push_str(line);
        }
        text
    })
}

/// Sets where `submit` sends command lines.
pub fn set_command_sink(tx: Sender<String>) {
    *COMMANDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
}

/// Queues a command line typed into the in-game console.
pub fn submit(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    print(format_args!("> {line}"));
    if let Some(tx) = COMMANDS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(line.to_string());
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::console::print(format_args!($($arg)*)) };
}

/// Background thread forwarding console input lines to the main loop.
pub struct Reader {
    thread: JoinHandle<()>,
}

impl Reader {
    pub fn spawn(tx: Sender<String>) -> Option<Self> {
        let input = open_console("CONIN$")?;
        let thread = std::thread::Builder::new()
            .name("mcbe-console".into())
            .spawn(move || {
                let mut line = Vec::<u16>::new();
                let mut buffer = [0u16; 256];
                loop {
                    let mut read = 0;
                    let ok = unsafe {
                        ReadConsoleW(
                            input.as_raw_handle(),
                            buffer.as_mut_ptr().cast(),
                            buffer.len() as u32,
                            &mut read,
                            std::ptr::null(),
                        )
                    };
                    if ok == 0 || read == 0 {
                        break;
                    }
                    for &c in &buffer[..read as usize] {
                        if c == u16::from(b'\n') {
                            let text = String::from_utf16_lossy(&line).trim().to_string();
                            line.clear();
                            if !text.is_empty() && tx.send(text).is_err() {
                                return;
                            }
                        } else {
                            // Retain UTF-16 across reads, including split surrogate pairs.
                            line.push(c);
                        }
                    }
                }
            })
            .ok()?;
        Some(Self { thread })
    }

    /// Cancels the blocking console read and waits for the thread to end.
    /// Returns false if it is still running (the DLL must then stay loaded).
    pub fn stop(self) -> bool {
        let handle = self.thread.as_raw_handle();
        for _ in 0..40 {
            unsafe {
                CancelSynchronousIo(handle);
                if WaitForSingleObject(handle, 50) == WAIT_OBJECT_0 {
                    return true;
                }
            }
        }
        false
    }
}
