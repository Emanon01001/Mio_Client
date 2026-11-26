//! Stamps the client DLL with version info, so it is a named, versioned module
//! in Explorer and in a process's module list rather than an anonymous one.
//! A DLL carries no icon or manifest (it runs inside its host process). See
//! crates/shared/winres.rs.
#[path = "../shared/winres.rs"]
mod winres;

fn main() {
    println!("cargo:rerun-if-changed=../shared/winres.rs");
    winres::embed(&winres::Resource {
        description: "Mio Client",
        original_filename: "Mio_Client.dll",
        icon: None,
        manifest: false,
        dll: true,
    });
}
