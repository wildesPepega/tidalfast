//! Desktop integration: registers tidalfast as the handler for `spotify:` and
//! `tidal:` links so clicking them (in a browser, in Discord …) opens the app.

/// The app icon (window, taskbar, launcher).
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// The file that starts this program: the AppImage itself (its mount point
/// disappears when it quits), or the executable.
fn launcher_path() -> Option<std::path::PathBuf> {
    match std::env::var_os("APPIMAGE") {
        Some(p) if cfg!(target_os = "linux") => Some(p.into()),
        _ => std::env::current_exe().ok(),
    }
}

/// Make tidalfast the handler for `spotify:` and `tidal:` links (what a
/// browser opens after asking "open in tidalfast?"), or give that up again.
/// Does nothing when everything is already as wanted.
#[cfg(target_os = "linux")]
pub fn set_link_handler(enabled: bool) {
    let (Some(dir), Some(exe)) = (dirs::data_dir(), launcher_path()) else {
        return;
    };
    let dir = dir.join("applications");
    let path = dir.join("tidalfast.desktop");
    let mime = if enabled { "MimeType=x-scheme-handler/spotify;x-scheme-handler/tidal;\n" } else { "" };
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=tidalfast\nComment=A simple, native TIDAL client\n\
         Exec=\"{}\" %u\nIcon=tidalfast\nTerminal=false\nCategories=AudioVideo;Audio;Player;\n\
         {mime}StartupWMClass=tidalfast\n",
        exe.display()
    );
    install_icon();
    let changed = std::fs::read_to_string(&path).map_or(true, |old| old != content);
    if changed {
        let _ = std::fs::create_dir_all(&dir);
        if std::fs::write(&path, content).is_err() {
            return;
        }
        let _ = std::process::Command::new("update-desktop-database").arg(&dir).status();
    }
    if !enabled {
        return;
    }
    // claim the schemes unless we already are the default (another app may
    // have taken over since). `gio` knows the truth even where `xdg-mime`
    // can't tell (it needs qtpaths on KDE).
    for scheme in ["spotify", "tidal"] {
        let mime_type = format!("x-scheme-handler/{scheme}");
        let ours = std::process::Command::new("gio")
            .args(["mime", &mime_type])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .is_some_and(|text| text.lines().next().is_some_and(|l| l.contains("tidalfast.desktop")));
        if ours && !changed {
            continue;
        }
        let gio = std::process::Command::new("gio")
            .args(["mime", &mime_type, "tidalfast.desktop"])
            .output()
            .is_ok_and(|o| o.status.success());
        if !gio {
            let _ = std::process::Command::new("xdg-mime").args(["default", "tidalfast.desktop", &mime_type]).status();
        }
    }
}

/// Registers (or removes) the URL protocols in the current user's registry,
/// which is what makes a browser offer to open the link in tidalfast.
#[cfg(windows)]
pub fn set_link_handler(enabled: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Some(exe) = launcher_path() else { return };
    let reg = |args: &[&str]| {
        let _ = std::process::Command::new("reg").args(args).creation_flags(CREATE_NO_WINDOW).status();
    };
    for scheme in ["spotify", "tidal"] {
        let key = format!("HKCU\\Software\\Classes\\{scheme}");
        if !enabled {
            reg(&["delete", &key, "/f"]);
            continue;
        }
        reg(&["add", &key, "/ve", "/d", &format!("URL:{scheme} Protocol"), "/f"]);
        reg(&["add", &key, "/v", "URL Protocol", "/d", "", "/f"]);
        reg(&[
            "add",
            &format!("{key}\\shell\\open\\command"),
            "/ve",
            "/d",
            &format!("\"{}\" \"%1\"", exe.display()),
            "/f",
        ]);
    }
}

/// Puts the icon where launchers and Wayland taskbars look it up by name.
#[cfg(target_os = "linux")]
fn install_icon() {
    let Some(dir) = dirs::data_dir().map(|d| d.join("icons/hicolor/256x256/apps")) else {
        return;
    };
    let path = dir.join("tidalfast.png");
    if std::fs::read(&path).is_ok_and(|old| old == ICON_PNG) {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(path, ICON_PNG);
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn set_link_handler(_enabled: bool) {}
