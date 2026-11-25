//! mio_injector: loads `Mio_Client.dll` into Minecraft Bedrock (GDK build).
//! `mio_injector_gui` offers launch / inject / eject in a window instead.
//!
//!   mio_injector launch
//!   mio_injector inject [--process NAME] [--dll PATH]
//!   mio_injector eject  [--process NAME]
//!   mio_injector scan "<signature>" [--exe PATH]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use injector::{default_dll, eject, find_process, inject, launch, DEFAULT_PROCESS, DLL_NAME};

const DEFAULT_EXE: &str = r"C:\XboxGames\Minecraft for Windows\Content\Minecraft.Windows.exe";

/// Offline signature check against the exe on disk; prints match RVAs.
fn scan(signature: &str, exe: &Path) -> Result<(), String> {
    let pattern: sigscan::Pattern = signature.parse().map_err(|e| format!("{e}"))?;
    let data = std::fs::read(exe).map_err(|e| format!("cannot read {}: {e}", exe.display()))?;
    let sections = sigscan::sections(&data).ok_or("not a valid PE file")?;

    const LIMIT: usize = 16;
    let mut total = 0;
    for s in sections.iter().filter(|s| s.executable) {
        let start = s.raw_offset as usize;
        let end = start.saturating_add(s.raw_size as usize).min(data.len());
        let Some(raw) = data.get(start..end) else {
            continue;
        };
        for off in pattern.find_all(raw, LIMIT - total) {
            println!("  {:<8} RVA 0x{:X}", s.name, s.rva as usize + off);
            total += 1;
        }
        if total >= LIMIT {
            break;
        }
    }
    match total {
        0 => println!("no match"),
        1 => println!("1 match (unique - usable as a signature)"),
        n if n >= LIMIT => println!("{n}+ matches (not unique - make the signature longer)"),
        n => println!("{n} matches (not unique - make the signature longer)"),
    }
    Ok(())
}

fn usage() -> String {
    format!(
        "usage:\n  \
         mio_injector launch\n  \
         mio_injector inject [--process NAME] [--dll PATH]\n  \
         mio_injector eject  [--process NAME]\n  \
         mio_injector scan \"<signature>\" [--exe PATH]\n\n\
         defaults: --process {DEFAULT_PROCESS}, --dll <injector dir>\\{DLL_NAME}"
    )
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "inject".to_string());

    let mut process = DEFAULT_PROCESS.to_string();
    let mut dll = default_dll();
    let mut exe = PathBuf::from(DEFAULT_EXE);
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--process" => process = value("--process")?,
            "--dll" => dll = PathBuf::from(value("--dll")?),
            "--exe" => exe = PathBuf::from(value("--exe")?),
            _ => positional.push(arg),
        }
    }

    match command.as_str() {
        "launch" => {
            launch()?;
            println!("launch requested");
            Ok(())
        }
        "inject" => {
            let pid = find_process(&process)?;
            inject(pid, &dll)?;
            println!("injected {} into {process} (pid {pid})", dll.display());
            Ok(())
        }
        "eject" => {
            let pid = find_process(&process)?;
            eject(pid)?;
            println!("eject requested for {process} (pid {pid})");
            Ok(())
        }
        "scan" => {
            let signature = positional.first().ok_or_else(usage)?;
            scan(signature, &exe)
        }
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n{}", usage())),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
