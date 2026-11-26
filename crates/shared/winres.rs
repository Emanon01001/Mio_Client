//! Build-script helper: embeds a Windows version-info resource (and, for an
//! executable, the app icon and an application manifest) so the binary carries
//! a visible identity. Its product name and version then show in Explorer's
//! Properties, in Task Manager, and as the program name in the SmartScreen
//! "unknown publisher" dialog.
//!
//! Included by each crate's `build.rs` with `#[path]`. The README covers code
//! signing for publisher verification.
//!
//! `embed_resource` must be a build-dependency of the including crate.
use std::{env, fs, path::PathBuf};

/// What to stamp onto one crate's binaries. One resource is shared by every
/// target of the crate, so the strings describe the crate's primary artifact.
pub struct Resource {
    /// `FileDescription`: the name Task Manager and the SmartScreen dialog show.
    pub description: &'static str,
    /// `OriginalFilename`/`InternalName`, e.g. `"mio_launcher.exe"`.
    pub original_filename: &'static str,
    /// Absolute path to the `.ico`, or `None` (a DLL needs no icon).
    pub icon: Option<PathBuf>,
    /// Embed an `asInvoker`, per-monitor-DPI, long-path manifest (executables
    /// only; a DLL takes its manifest from its host process).
    pub manifest: bool,
    /// `true` for a DLL (`VFT_DLL`), `false` for an executable (`VFT_APP`).
    pub dll: bool,
}

/// asInvoker (no spurious UAC elevation prompt at launch), per-monitor-v2 DPI,
/// long-path support, and the Windows 8–11 supported-OS GUIDs.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;

/// Escapes a path for an rc string literal (C-style), dropping any `\\?\`.
fn rc_path(path: &std::path::Path) -> String {
    let p = path.display().to_string();
    p.strip_prefix(r"\\?\").unwrap_or(&p).replace('\\', r"\\")
}

pub fn embed(res: &Resource) {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let num = |name: &str| {
        env::var(name)
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(0)
    };
    let (major, minor, patch) = (
        num("CARGO_PKG_VERSION_MAJOR"),
        num("CARGO_PKG_VERSION_MINOR"),
        num("CARGO_PKG_VERSION_PATCH"),
    );
    let version = format!("{major}.{minor}.{patch}.0");
    let comma = format!("{major},{minor},{patch},0");
    let file_type = if res.dll { "0x2L" } else { "0x1L" };
    let internal = res
        .original_filename
        .strip_suffix(".exe")
        .or_else(|| res.original_filename.strip_suffix(".dll"))
        .unwrap_or(res.original_filename);

    let mut rc = String::new();
    if let Some(icon) = &res.icon {
        println!("cargo:rerun-if-changed={}", icon.display());
        rc.push_str(&format!("1 ICON \"{}\"\n", rc_path(icon)));
    }
    if res.manifest {
        let path = out.join("mio.manifest");
        fs::write(&path, MANIFEST).unwrap();
        // 24 = RT_MANIFEST, id 1 = the process default (CREATEPROCESS_MANIFEST).
        rc.push_str(&format!("1 24 \"{}\"\n", rc_path(&path)));
    }
    rc.push_str(&format!(
        "1 VERSIONINFO\n\
         FILEVERSION {comma}\n\
         PRODUCTVERSION {comma}\n\
         FILEOS 0x40004L\n\
         FILETYPE {file_type}\n\
         BEGIN\n\
         BLOCK \"StringFileInfo\"\n\
         BEGIN\n\
         BLOCK \"040904b0\"\n\
         BEGIN\n\
         VALUE \"CompanyName\", \"Mio\"\n\
         VALUE \"FileDescription\", \"{description}\"\n\
         VALUE \"FileVersion\", \"{version}\"\n\
         VALUE \"InternalName\", \"{internal}\"\n\
         VALUE \"OriginalFilename\", \"{original}\"\n\
         VALUE \"ProductName\", \"Mio Client\"\n\
         VALUE \"ProductVersion\", \"{version}\"\n\
         END\n\
         END\n\
         BLOCK \"VarFileInfo\"\n\
         BEGIN\n\
         VALUE \"Translation\", 0x409, 1200\n\
         END\n\
         END\n",
        description = res.description,
        original = res.original_filename,
    ));

    let rc_file = out.join("mio_resource.rc");
    fs::write(&rc_file, rc).unwrap();
    embed_resource::compile(&rc_file, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
