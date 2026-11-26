//! Stamps the signature-checker executables with the app icon, a manifest and
//! version info, so they are named programs in Explorer and the SmartScreen
//! dialog. See crates/shared/winres.rs.
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
        description: "Mio Signature Checker",
        original_filename: "mio_signature_checker.exe",
        icon: Some(icon),
        manifest: true,
        dll: false,
    });
}
