//! Stamps every injector executable with the app icon, an application manifest
//! and version info, so Explorer, the taskbar and the SmartScreen dialog show a
//! named program instead of an anonymous binary. See crates/shared/winres.rs.
#[path = "../shared/winres.rs"]
mod winres;

use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../shared/winres.rs");
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../assets/icon/mio.ico")
        .canonicalize()
        .expect("assets/icon/mio.ico");
    winres::embed(&winres::Resource {
        description: "Mio Launcher",
        original_filename: "mio_launcher.exe",
        icon: Some(icon),
        manifest: true,
        dll: false,
    });
}
