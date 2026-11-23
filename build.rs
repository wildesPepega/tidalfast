//! The Windows (MinGW) build contains C++ code (the OpenH264 decoder). By default the linker
//! takes the C++ runtime from `libstdc++-6.dll`, which then has to sit next to the exe. Offer it
//! a directory that only holds the static `libstdc++.a`, so the runtime is linked into the exe.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows_gnu = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|v| v == "windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "gnu");
    if !windows_gnu {
        return;
    }
    let Some(out) = std::env::var_os("OUT_DIR").map(PathBuf::from) else { return };
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
    println!("cargo:warning=static libstdc++.a not found: the Windows exe will need libstdc++-6.dll");
}
