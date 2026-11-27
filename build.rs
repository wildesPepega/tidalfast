//! Windows (MinGW) builds only:
//!
//! * The exe gets its icon and file details (name, version) as resources, so Explorer, the
//!   taskbar and the Start menu show the Oxytide icon and name.
//! * The build contains C++ code (the OpenH264 decoder). By default the linker takes the C++
//!   runtime from `libstdc++-6.dll`, which then has to sit next to the exe. Offer it a directory
//!   that only holds the static `libstdc++.a`, so the runtime is linked into the exe.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let windows_gnu = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|v| v == "windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "gnu");
    if !windows_gnu {
        return;
    }
    let Some(out) = std::env::var_os("OUT_DIR").map(PathBuf::from) else { return };
    resources(&out);
    static_stdcxx(&out);
}

/// Compile the icon and version details with `windres` and link them into the exe.
fn resources(out: &Path) {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let icon = manifest_dir.join("assets").join("icon.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let mut parts: Vec<u32> = version.split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(4, 0);
    let numbers = parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    // windres reads C strings: forward slashes need no escaping
    let icon_path = icon.to_string_lossy().replace('\\', "/");
    let rc = format!(
        r#"1 ICON "{icon_path}"

1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Oxytide"
      VALUE "FileDescription", "Oxytide"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "oxytide"
      VALUE "OriginalFilename", "Oxytide.exe"
      VALUE "ProductName", "Oxytide"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let rc_path = out.join("oxytide.rc");
    let obj = out.join("oxytide-resources.o");
    if std::fs::write(&rc_path, rc).is_err() {
        println!("cargo:warning=could not write the resource script: the exe will have no icon");
        return;
    }
    for tool in ["x86_64-w64-mingw32-windres", "windres"] {
        let Ok(status) = Command::new(tool).arg("-i").arg(&rc_path).args(["-O", "coff", "-o"]).arg(&obj).status() else { continue };
        if status.success() {
            // an object file on the command line is always linked in (unlike a library member nobody refers to)
            println!("cargo:rustc-link-arg-bins={}", obj.display());
            return;
        }
    }
    println!("cargo:warning=windres not found or failed: the exe will have no icon");
}

fn static_stdcxx(out: &Path) {
    for compiler in ["x86_64-w64-mingw32-g++", "g++"] {
        let Ok(output) = Command::new(compiler).arg("-print-file-name=libstdc++.a").output() else { continue };
        let found = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        if found.is_absolute() && found.exists() {
            let dir = out.join("static-stdcxx");
            let _ = std::fs::create_dir_all(&dir);
            if std::fs::copy(&found, dir.join("libstdc++.a")).is_ok() {
                println!("cargo:rustc-link-search=native={}", dir.display());
            }
            return;
        }
    }
    println!("cargo:warning=static libstdc++.a not found: the exe will need libstdc++-6.dll");
}
