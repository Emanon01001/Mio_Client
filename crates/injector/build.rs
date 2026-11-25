//! Embeds the Mio icon in every injector executable, so Explorer, the taskbar
//! and shortcuts show it. The .rc script is generated into OUT_DIR so the
//! only tracked input is the icon itself.
use std::{env, fs, path::PathBuf};

fn main() {
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../assets/icon/mio.ico")
        .canonicalize()
        .expect("assets/icon/mio.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // rc.exe does not take verbatim (\\?\) paths, and its string literals are
    // C-style: escape backslashes.
    let path = icon.display().to_string();
    let path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .replace('\\', r"\\");
    let rc = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("mio.rc");
    fs::write(&rc, format!("1 ICON \"{path}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
