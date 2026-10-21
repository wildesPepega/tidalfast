//! Linux integration: registers tidalfast as the handler for `spotify:` and
//! `tidal:` links so clicking them (e.g. in Discord) opens the app.

/// The app icon (window, taskbar, launcher).
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

#[cfg(target_os = "linux")]
pub fn register_link_handler() {
    let (Some(dir), Ok(exe)) = (dirs::data_dir(), std::env::current_exe()) else {
        return;
    };
    let dir = dir.join("applications");
    let path = dir.join("tidalfast.desktop");
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=tidalfast\nComment=A simple, native TIDAL client\n\
         Exec=\"{}\" %u\nIcon=tidalfast\nTerminal=false\nCategories=AudioVideo;Audio;Player;\n\
         MimeType=x-scheme-handler/spotify;x-scheme-handler/tidal;\nStartupWMClass=tidalfast\n",
        exe.display()
    );
    install_icon();
    if std::fs::read_to_string(&path).is_ok_and(|old| old == content) {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::write(&path, content).is_err() {
        return;
    }
    for scheme in ["spotify", "tidal"] {
        let _ = std::process::Command::new("xdg-mime")
            .args(["default", "tidalfast.desktop", &format!("x-scheme-handler/{scheme}")])
            .status();
    }
    let _ = std::process::Command::new("update-desktop-database").arg(&dir).status();
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

#[cfg(not(target_os = "linux"))]
pub fn register_link_handler() {}
