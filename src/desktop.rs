//! Desktop integration: registers tidalfast as the handler for `spotify:` and
//! `tidal:` links so clicking them (in a browser, in Discord …) opens the app.

/// The app icon (window, taskbar, launcher).
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// The file that starts this program: the AppImage itself (its mount point
/// disappears when it quits), or the executable.
fn launcher_path() -> Option<std::path::PathBuf> {
    match std::env::var_os("APPIMAGE") {
        Some(p) if cfg!(target_os = "linux") => Some(p.into()),
        _ => current_exe(),
    }
}

/// Path of the running executable. On Linux, `current_exe` appends
/// " (deleted)" when the file was replaced while running (an install over a
/// running instance); that suffix is not part of the path.
pub fn current_exe() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let text = exe.to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(p) => Some(p.into()),
        None => Some(exe),
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

/// The now-playing plugin for GPU Screen Recorder (built from `gsr-plugin/`).
#[cfg(target_os = "linux")]
const GSR_PLUGIN: &[u8] = include_bytes!("../gsr-plugin/tidalfast_overlay.so");

#[cfg(target_os = "linux")]
fn gsr_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let plugin = dirs::data_dir()?.join("tidalfast/gsr/tidalfast_overlay.so");
    let wrapper = dirs::home_dir()?.join(".local/bin/gpu-screen-recorder");
    Some((plugin, wrapper))
}

/// Is the recorder plugin installed (and does the plugin file match this version)?
#[cfg(target_os = "linux")]
pub fn gsr_plugin_installed() -> bool {
    let Some((plugin, wrapper)) = gsr_paths() else { return false };
    std::fs::read(&plugin).map_or(false, |old| old == GSR_PLUGIN)
        && std::fs::read_to_string(&wrapper).map_or(false, |w| w.contains("tidalfast_overlay"))
}

/// Install the plugin and a `gpu-screen-recorder` wrapper in `~/.local/bin`
/// that loads it. Returns what to tell the user.
#[cfg(target_os = "linux")]
pub fn install_gsr_plugin() -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let (plugin, wrapper) = gsr_paths().ok_or("No home directory")?;
    let bin_dir = wrapper.parent().ok_or("No bin directory")?.to_path_buf();
    // the real recorder: first one in PATH that is not our own wrapper
    let real = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .filter(|d| *d != bin_dir)
        .map(|d| d.join("gpu-screen-recorder"))
        .find(|p| p.is_file())
        .ok_or("GPU Screen Recorder was not found")?;
    std::fs::create_dir_all(plugin.parent().ok_or("No data directory")?).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    // replace via rename: a running recorder keeps its copy of the old file
    let tmp = plugin.with_extension("so.new");
    std::fs::write(&tmp, GSR_PLUGIN).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &plugin).map_err(|e| e.to_string())?;
    let script = format!(
        "#!/bin/sh\n# written by tidalfast: loads the now-playing overlay plugin\n\
         case \"$*\" in\n  *--list*|*--version*|*-h*|*--help*) exec \"{real}\" \"$@\" ;;\nesac\n\
         exec \"{real}\" \"$@\" -p \"{plugin}\"\n",
        real = real.display(),
        plugin = plugin.display()
    );
    std::fs::write(&wrapper, script).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    Ok("Installed. Restart GPU Screen Recorder (replay or recording) to load it.".into())
}

/// Remove the plugin and the wrapper again.
#[cfg(target_os = "linux")]
pub fn remove_gsr_plugin() {
    let Some((plugin, wrapper)) = gsr_paths() else { return };
    if std::fs::read_to_string(&wrapper).map_or(false, |w| w.contains("tidalfast_overlay")) {
        let _ = std::fs::remove_file(&wrapper);
    }
    let _ = std::fs::remove_file(&plugin);
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn installs_and_removes_the_recorder_plugin() {
        let root = std::env::temp_dir().join(format!("tidalfast-gsr-test-{}", std::process::id()));
        let (home, bin) = (root.join("home"), root.join("bin"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(bin.join("gpu-screen-recorder"), "#!/bin/sh\n").unwrap();
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_DATA_HOME", home.join("data"));
        std::env::set_var("PATH", &bin);

        assert!(!gsr_plugin_installed());
        install_gsr_plugin().unwrap();
        assert!(gsr_plugin_installed());
        let wrapper = std::fs::read_to_string(home.join(".local/bin/gpu-screen-recorder")).unwrap();
        assert!(wrapper.contains(bin.join("gpu-screen-recorder").to_str().unwrap()) && wrapper.contains("-p "));
        remove_gsr_plugin();
        assert!(!gsr_plugin_installed());
        let _ = std::fs::remove_dir_all(&root);
    }
}
