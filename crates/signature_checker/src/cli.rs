use crate::{audit_file, default_exe, save_report, summary};
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Default)]
pub struct Options {
    pub exe: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub help: bool,
}
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options, String> {
    let mut out = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => out.help = true,
            Some("--exe" | "--baseline" | "--out") => {
                let value = PathBuf::from(
                    args.next()
                        .filter(|s| !s.is_empty())
                        .ok_or("Missing option value")?,
                );
                if value.to_string_lossy().starts_with("--") {
                    return Err("Missing option value".into());
                }
                let slot = match arg.to_str() {
                    Some("--exe") => &mut out.exe,
                    Some("--baseline") => &mut out.baseline,
                    _ => &mut out.out,
                };
                if slot.replace(value).is_some() {
                    return Err("Duplicate option".into());
                }
            }
            _ if !arg.to_string_lossy().starts_with('-') && out.exe.is_none() => {
                out.exe = Some(arg.into())
            }
            _ => return Err(format!("Unknown argument: {}", arg.to_string_lossy())),
        }
    }
    Ok(out)
}
pub const HELP: &str = "Mio Signature Checker (offline, read-only)\n\
Usage: mio_signature_checker_cli [--exe <Minecraft.Windows.exe>] [--baseline <before.json>] [--out <new.json>]\n\
Drag an EXE onto mio_signature_checker.exe for the GUI.\n\
Exit codes: 0=static checks passed, 1=issues, 2=manual review, 3=input/I/O error.\n\
Reports never overwrite existing files. A baseline must be JSON exported by this tool.";
pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    let options = match parse(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}\n{HELP}");
            return 3;
        }
    };
    if options.help {
        println!("{HELP}");
        return 0;
    }
    let exe = options.exe.unwrap_or_else(default_exe);
    let report = match audit_file(&exe, options.baseline.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 3;
        }
    };
    println!("{}\n{}", report.exe, summary(&report));
    for s in &report.signatures {
        println!(
            "{:10?} {:8?} {:3}{} {:45} {}",
            s.state,
            s.role,
            s.rvas.len(),
            if s.capped { "+" } else { " " },
            s.name,
            s.rvas
                .iter()
                .take(8)
                .map(|r| format!("0x{r:X}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for c in &report.checks {
        println!(
            "{:7?} {}: {} {:?}",
            c.state, c.name, c.detail, c.rvas_or_values
        );
    }
    for change in &report.changes {
        println!("CHANGED {}: {}", change.name, change.detail);
    }
    if let Some(path) = options.out {
        if let Err(e) = save_report(&path, &report) {
            eprintln!("{e}");
            return 3;
        }
        println!("Report: {}", path.display());
    }
    report.outcome.exit_code()
}
