//! Hiding and showing the window on KDE Plasma under Wayland.
//!
//! A Wayland application cannot unmap its own window (winit ignores
//! `set_visible`, and a minimized window stays in the task manager and the
//! window switcher). KWin can: its scripting interface sets the window's
//! "skip taskbar / switcher / pager" flags and minimizes it, which makes the
//! window disappear completely, and undoes all of that to bring it back.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const SCRIPT_NAME: &str = "oxytide-window";

/// Serializes the (slow) D-Bus round trips, so a hide followed by a show cannot overtake each other.
static BUSY: Mutex<()> = Mutex::new(());

/// The window is hidden by us right now.
static HIDDEN: AtomicBool = AtomicBool::new(false);

/// Is this a Plasma Wayland session, where `set_hidden` is the way to hide the window?
pub fn available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("KDE")))
}

/// Make this process's window disappear (or reappear), in the background.
pub fn set_hidden(hidden: bool) {
    HIDDEN.store(hidden, Ordering::SeqCst);
    std::thread::spawn(move || {
        let _guard = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = run(hidden) {
            log::warn!("KWin window control failed: {e}");
        }
    });
}

/// Bring the window back if it is hidden. A hidden (minimized) Wayland window is not drawn, so the
/// app's own loop does not run and cannot react to the tray icon: whoever notices the request
/// (the tray, the single-instance socket) has to call this from its own thread.
pub fn wake_if_hidden() {
    if available() && HIDDEN.load(Ordering::SeqCst) {
        set_hidden(false);
    }
}

fn script(hidden: bool) -> String {
    format!(
        "(function () {{\n\
         var pid = {pid}, hide = {hidden};\n\
         var list = workspace.windowList ? workspace.windowList() : workspace.clientList();\n\
         for (var i = 0; i < list.length; i++) {{\n\
           var w = list[i];\n\
           if (w.pid !== pid) continue;\n\
           w.skipTaskbar = hide; w.skipSwitcher = hide; w.skipPager = hide;\n\
           w.minimized = hide;\n\
           if (!hide) {{ if ('activeWindow' in workspace) workspace.activeWindow = w; else workspace.activeClient = w; }}\n\
         }}\n\
         }})();\n",
        pid = std::process::id(),
    )
}

fn script_path() -> PathBuf {
    // KWin runs this file: it must live where no other user can swap it
    crate::fsx::runtime_dir().join("oxytide-window.js")
}

/// Call a method over D-Bus with whichever command line tool exists; the reply as text.
fn call(path: &str, member: &str, args: &[&str]) -> Result<String, String> {
    let typed: Vec<String> = args.iter().map(|a| if a.parse::<i32>().is_ok() { format!("int32:{a}") } else { format!("string:{a}") }).collect();
    let attempts: [(&str, Vec<String>); 3] = [
        (
            "dbus-send",
            [vec!["--session".into(), "--print-reply".into(), "--dest=org.kde.KWin".into(), path.to_string(), member.to_string()], typed].concat(),
        ),
        ("gdbus", [vec!["call".into(), "--session".into(), "--dest".into(), "org.kde.KWin".into(), "--object-path".into(), path.to_string(), "--method".into(), member.to_string()], args.iter().map(|a| a.to_string()).collect()].concat()),
        ("qdbus6", [vec!["org.kde.KWin".into(), path.to_string(), member.to_string()], args.iter().map(|a| a.to_string()).collect()].concat()),
    ];
    let mut last = "no D-Bus tool (dbus-send, gdbus, qdbus6) found".to_string();
    for (tool, argv) in attempts {
        match Command::new(tool).args(&argv).stdin(Stdio::null()).output() {
            Ok(out) if out.status.success() => return Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
            Ok(out) => last = format!("{tool}: {}", String::from_utf8_lossy(&out.stderr).trim()),
            Err(_) => {}
        }
    }
    Err(last)
}

/// The script's number out of a reply like `   int32 3`, `(3,)` or `3`.
fn parse_id(reply: &str) -> Option<i32> {
    reply.split(|c: char| !(c.is_ascii_digit() || c == '-')).filter(|t| !t.is_empty()).last()?.parse().ok()
}

fn run(hidden: bool) -> Result<(), String> {
    let path = script_path();
    std::fs::write(&path, script(hidden)).map_err(|e| e.to_string())?;
    let file = path.to_string_lossy().into_owned();
    let _ = call("/Scripting", "org.kde.kwin.Scripting.unloadScript", &[SCRIPT_NAME]);
    let reply = call("/Scripting", "org.kde.kwin.Scripting.loadScript", &[&file, SCRIPT_NAME])?;
    let id = parse_id(&reply).ok_or_else(|| format!("unexpected reply: {reply}"))?;
    // Plasma 6 names the script object /Scripting/Script<id>, Plasma 5 /<id>
    let started = call(&format!("/Scripting/Script{id}"), "org.kde.kwin.Script.run", &[])
        .or_else(|_| call(&format!("/{id}"), "org.kde.kwin.Script.run", &[]));
    // the script runs inside KWin a moment later
    std::thread::sleep(Duration::from_millis(400));
    let _ = call("/Scripting", "org.kde.kwin.Scripting.unloadScript", &[SCRIPT_NAME]);
    let _ = std::fs::remove_file(&path);
    started.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_script_number_from_each_tool() {
        assert_eq!(parse_id("method return time=1.5 sender=:1.2 -> destination=:1.99 serial=4 reply_serial=2\n   int32 7\n"), Some(7));
        assert_eq!(parse_id("(7,)\n"), Some(7));
        assert_eq!(parse_id("7\n"), Some(7));
        assert_eq!(parse_id("garbage"), None);
    }

    #[test]
    fn the_script_targets_this_process_only() {
        let hide = script(true);
        assert!(hide.contains(&format!("var pid = {}, hide = true;", std::process::id())));
        assert!(script(false).contains("hide = false"));
    }
}
