//! Desktop integration: registers oxytide as the handler for `spotify:` and
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

/// Make oxytide the handler for `spotify:` and `tidal:` links (what a
/// browser opens after asking "open in oxytide?"), or give that up again.
/// Does nothing when everything is already as wanted.
#[cfg(target_os = "linux")]
pub fn set_link_handler(enabled: bool) {
    let (Some(dir), Some(exe)) = (dirs::data_dir(), launcher_path()) else {
        return;
    };
    let dir = dir.join("applications");
    let path = dir.join("oxytide.desktop");
    let mime = if enabled { "MimeType=x-scheme-handler/spotify;x-scheme-handler/tidal;\n" } else { "" };
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Oxytide\nComment=A simple, native TIDAL client\n\
         Exec=\"{}\" %u\nIcon=oxytide\nTerminal=false\nCategories=AudioVideo;Audio;Player;\n\
         {mime}StartupWMClass=oxytide\n",
        desktop_quote(&exe.to_string_lossy())
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
            .is_some_and(|text| text.lines().next().is_some_and(|l| l.contains("oxytide.desktop")));
        if ours && !changed {
            continue;
        }
        let gio = std::process::Command::new("gio")
            .args(["mime", &mime_type, "oxytide.desktop"])
            .output()
            .is_ok_and(|o| o.status.success());
        if !gio {
            let _ = std::process::Command::new("xdg-mime").args(["default", "oxytide.desktop", &mime_type]).status();
        }
    }
}

/// Registers (or removes) the URL protocols in the current user's registry,
/// which is what makes a browser offer to open the link in oxytide.
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

/// Escape a path for a quoted argument in a desktop entry's `Exec=` line (the desktop entry
/// spec: `"`, `` ` ``, `$` and backslashes get a backslash, and the value escapes backslashes once more).
#[cfg(target_os = "linux")]
fn desktop_quote(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            '\n' | '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Puts the icon where launchers and Wayland taskbars look it up by name.
#[cfg(target_os = "linux")]
fn install_icon() {
    let Some(dir) = dirs::data_dir().map(|d| d.join("icons/hicolor/256x256/apps")) else {
        return;
    };
    let path = dir.join("oxytide.png");
    if std::fs::read(&path).is_ok_and(|old| old == ICON_PNG) {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(path, ICON_PNG);
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn set_link_handler(_enabled: bool) {}

/// Windows: the icon in the taskbar, the program's name on disk and its Start menu entry.
#[cfg(windows)]
pub mod windows {
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// The name the program gives itself.
    const EXE_NAME: &str = "Oxytide.exe";

    #[link(name = "user32")]
    extern "system" {
        fn LoadImageW(instance: *mut c_void, name: *const u16, kind: u32, cx: i32, cy: i32, flags: u32) -> *mut c_void;
        fn SendMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetSystemMetrics(index: i32) -> i32;
        fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }

    /// Give the window both icons from the exe's resources. winit only sets the small one (title
    /// bar), and without the big one the taskbar and Alt+Tab show a blank program icon.
    pub fn set_window_icons(hwnd: *mut c_void) {
        const WM_SETICON: u32 = 0x0080;
        const IMAGE_ICON: u32 = 1;
        const SM_CXICON: i32 = 11;
        const SM_CXSMICON: i32 = 49;
        // SAFETY: plain Win32 calls with a valid window of this process; the resource id 1 is the
        // icon that build.rs puts into the exe (`MAKEINTRESOURCE(1)`), a missing one gives null.
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            let scaled = |metric: i32| GetSystemMetrics(metric).max(16) * dpi / 96;
            for (which, size) in [(1usize, scaled(SM_CXICON)), (0usize, scaled(SM_CXSMICON))] {
                let icon = LoadImageW(module, 1 as *const u16, IMAGE_ICON, size, size, 0);
                if !icon.is_null() {
                    SendMessageW(hwnd, WM_SETICON, which, icon as isize);
                }
            }
        }
    }

    /// Before the window opens: a copy that still runs under its old name (`tidalfast….exe`)
    /// renames itself to `Oxytide.exe` and starts again from there. True when this process
    /// should quit because the renamed copy takes over.
    pub fn rename_old_exe() -> bool {
        let Some(exe) = super::current_exe() else { return false };
        let Some(target) = new_name_for(&exe) else { return false };
        // an Oxytide.exe of its own may already be there (from a download): this copy replaces it
        if target.exists() && std::fs::remove_file(&target).is_err() {
            log::warn!("{} is in use: keeping the old name", target.display());
            return false;
        }
        // Windows lets a running program be renamed (not deleted) on the same drive
        if let Err(e) = std::fs::rename(&exe, &target) {
            log::warn!("could not rename {} to {}: {e}", exe.display(), target.display());
            return false;
        }
        // the same arguments (a link, say), without the wait this start may have been given
        let mut args = Vec::new();
        let mut given = std::env::args().skip(1);
        while let Some(a) = given.next() {
            if a == "--delay-start" {
                given.next();
            } else {
                args.push(a);
            }
        }
        let started = std::process::Command::new(&target).args(["--delay-start", "800"]).args(&args).spawn().is_ok();
        if !started {
            // carry on under the new name in this process
            log::warn!("could not restart as {}", target.display());
        }
        started
    }

    /// Old copies (`tidalfast….exe`) next to this program are out of date: they go.
    pub fn remove_old_copies() {
        let Some(exe) = super::current_exe() else { return };
        let Some(dir) = exe.parent() else { return };
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path != exe && new_name_for(&path).is_some() && path.is_file() {
                // (one that is running cannot be deleted: it stays, which is fine)
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// `C:\…\tidalfast-windows-x86_64.exe` → `C:\…\Oxytide.exe`; `None` for any other name.
    fn new_name_for(exe: &Path) -> Option<PathBuf> {
        let name = exe.file_name()?.to_str()?.to_ascii_lowercase();
        (name.starts_with("tidalfast") && name.ends_with(".exe")).then(|| exe.with_file_name(EXE_NAME))
    }

    /// The Start menu entry, pointing at this program. Made (or corrected) in the background when
    /// it is missing or points elsewhere; the entry of the old name goes.
    pub fn ensure_start_menu_entry() {
        let (Some(exe), Some(programs)) = (super::current_exe(), dirs::data_dir().map(|d| d.join("Microsoft\\Windows\\Start Menu\\Programs"))) else {
            return;
        };
        let link = programs.join("Oxytide.lnk");
        // what the entry was last made for (reading a .lnk needs COM; a note next to the settings does not)
        let note = dirs::config_dir().map(|d| d.join("oxytide").join("start-menu-entry"));
        let wanted = exe.to_string_lossy().into_owned();
        let done = link.exists() && note.as_ref().is_some_and(|n| std::fs::read_to_string(n).is_ok_and(|t| t == wanted));
        for old in ["tidalfast.lnk", "Tidalfast.lnk"] {
            let _ = std::fs::remove_file(programs.join(old));
        }
        if done {
            return;
        }
        let _ = std::fs::create_dir_all(&programs);
        // the paths reach PowerShell as environment variables, never as part of the script text
        let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:OXYTIDE_LINK); \
                      $s.TargetPath = $env:OXYTIDE_EXE; $s.WorkingDirectory = $env:OXYTIDE_DIR; \
                      $s.IconLocation = $env:OXYTIDE_EXE + ',0'; $s.Description = 'A simple, native TIDAL client'; $s.Save()";
        let dir = exe.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let made = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
            .env("OXYTIDE_LINK", &link)
            .env("OXYTIDE_EXE", &wanted)
            .env("OXYTIDE_DIR", &dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .is_ok_and(|s| s.success());
        if made {
            if let Some(note) = note {
                let _ = crate::fsx::write_private(&note, wanted.as_bytes());
            }
        } else {
            log::warn!("could not create the Start menu entry");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_the_old_names_are_renamed() {
            assert_eq!(new_name_for(Path::new("C:\\x\\tidalfast-windows-x86_64.exe")), Some(PathBuf::from("C:\\x\\Oxytide.exe")));
            assert_eq!(new_name_for(Path::new("C:\\x\\TidalFast.exe")), Some(PathBuf::from("C:\\x\\Oxytide.exe")));
            assert_eq!(new_name_for(Path::new("C:\\x\\Oxytide.exe")), None);
            assert_eq!(new_name_for(Path::new("C:\\x\\oxytide-windows-x86_64.exe")), None);
        }
    }
}

/// The now-playing plugin for GPU Screen Recorder (built from `gsr-plugin/`).
#[cfg(target_os = "linux")]
const GSR_PLUGIN: &[u8] = include_bytes!("../gsr-plugin/oxytide_overlay.so");

#[cfg(target_os = "linux")]
fn gsr_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let plugin = dirs::data_dir()?.join("oxytide/gsr/oxytide_overlay.so");
    let wrapper = dirs::home_dir()?.join(".local/bin/gpu-screen-recorder");
    Some((plugin, wrapper))
}

/// Is the recorder plugin installed (and does the plugin file match this version)?
#[cfg(target_os = "linux")]
pub fn gsr_plugin_installed() -> bool {
    let Some((plugin, wrapper)) = gsr_paths() else { return false };
    std::fs::read(&plugin).map_or(false, |old| old == GSR_PLUGIN)
        && std::fs::read_to_string(&wrapper).map_or(false, |w| w.contains("oxytide_overlay"))
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
        "#!/bin/sh\n# written by oxytide: loads the now-playing overlay plugin\n\
         case \"$*\" in\n  *--list*|*--version*|*-h*|*--help*) exec \"{real}\" \"$@\" ;;\nesac\n\
         exec \"{real}\" \"$@\" -p \"{plugin}\"\n",
        real = real.display(),
        plugin = plugin.display()
    );
    std::fs::write(&wrapper, script).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    // the plugin of the version that was still called "tidalfast"
    if let Some(old) = dirs::data_dir().map(|d| d.join("tidalfast/gsr/tidalfast_overlay.so")) {
        let _ = std::fs::remove_file(old);
    }
    Ok("Installed. Restart GPU Screen Recorder (replay or recording) to load it.".into())
}

/// Remove the plugin and the wrapper again.
#[cfg(target_os = "linux")]
pub fn remove_gsr_plugin() {
    let Some((plugin, wrapper)) = gsr_paths() else { return };
    // (also a wrapper written by the version that was still called "tidalfast")
    if std::fs::read_to_string(&wrapper).map_or(false, |w| w.contains("oxytide_overlay") || w.contains("tidalfast_overlay")) {
        let _ = std::fs::remove_file(&wrapper);
    }
    let _ = std::fs::remove_file(&plugin);
    if let Some(old) = dirs::data_dir().map(|d| d.join("tidalfast/gsr/tidalfast_overlay.so")) {
        let _ = std::fs::remove_file(old);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn paths_are_quoted_for_desktop_entries() {
        assert_eq!(desktop_quote("/opt/Oxytide/oxytide"), "/opt/Oxytide/oxytide");
        assert_eq!(desktop_quote("/a b/$x\"y`z%"), "/a b/\\\\$x\\\\\"y\\\\`z%%");
        assert_eq!(desktop_quote("/a\\b"), "/a\\\\\\\\b");
    }

    #[test]
    fn installs_and_removes_the_recorder_plugin() {
        let root = std::env::temp_dir().join(format!("oxytide-gsr-test-{}", std::process::id()));
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
