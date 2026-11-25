//! The program used to be called "tidalfast". On the first start under its new name it takes over what
//! belongs to the user, so nobody has to sign in again or lose settings, plugins, themes or the song cache.
//!
//! * The settings folder is **copied**, the old one stays as a backup (it is small).
//! * The cache folder is **moved** (it can be big, and it is only a cache).
//! * Nothing is touched if the new folder already exists.
//! * The copy is made in a side folder and renamed when it is complete, so a crash cannot leave
//!   half a folder behind that would block the next try.

use std::path::{Path, PathBuf};

const OLD: &str = "tidalfast";
const NEW: &str = "oxytide";

pub fn run() {
    if let Some(base) = dirs::config_dir() {
        copy_if_new(&base.join(OLD), &base.join(NEW));
    }
    if let Some(base) = dirs::cache_dir() {
        move_if_new(&base.join(OLD), &base.join(NEW));
    }
    #[cfg(target_os = "linux")]
    remove_old_launcher_files();
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            // keeps the permissions too (the cache key is private to the user)
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Copy `old` to `new`, unless `new` exists or there is no `old`. True if it copied.
pub fn copy_if_new(old: &Path, new: &Path) -> bool {
    if new.exists() || !old.is_dir() {
        return false;
    }
    let side: PathBuf = new.with_extension("migrating");
    let _ = std::fs::remove_dir_all(&side);
    match copy_dir(old, &side).and_then(|_| std::fs::rename(&side, new)) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("could not take over the old settings: {e}");
            let _ = std::fs::remove_dir_all(&side);
            false
        }
    }
}

/// Move `old` to `new`, unless `new` exists or there is no `old`. True if it moved.
pub fn move_if_new(old: &Path, new: &Path) -> bool {
    if new.exists() || !old.is_dir() {
        return false;
    }
    std::fs::rename(old, new).is_ok()
}

/// The launcher entry and icon the old version put in the user's folders (they would show up twice).
#[cfg(target_os = "linux")]
fn remove_old_launcher_files() {
    let Some(data) = dirs::data_dir() else { return };
    let desktop = data.join("applications").join(format!("{OLD}.desktop"));
    if std::fs::read_to_string(&desktop).is_ok_and(|t| t.contains(&format!("Name={OLD}"))) {
        let _ = std::fs::remove_file(desktop);
    }
    let _ = std::fs::remove_file(data.join("icons/hicolor/256x256/apps").join(format!("{OLD}.png")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("oxytide-migrate-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn settings_login_key_plugins_and_themes_are_copied_and_the_old_ones_stay() {
        let base = temp();
        let old = base.join("tidalfast");
        std::fs::create_dir_all(old.join("plugins/hello")).unwrap();
        std::fs::create_dir_all(old.join("themes")).unwrap();
        std::fs::write(old.join("settings.json"), r#"{"access_token":"abc"}"#).unwrap();
        std::fs::write(old.join("cache.key"), [7u8; 32]).unwrap();
        std::fs::write(old.join("plugins/hello/main.rhai"), "let a = 1;").unwrap();
        std::fs::write(old.join("themes/x.toml"), "name = \"x\"").unwrap();
        let new = base.join("oxytide");
        assert!(copy_if_new(&old, &new));
        assert_eq!(std::fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"access_token":"abc"}"#);
        assert_eq!(std::fs::read(new.join("cache.key")).unwrap(), [7u8; 32]);
        assert!(new.join("plugins/hello/main.rhai").is_file());
        assert!(new.join("themes/x.toml").is_file());
        assert!(old.join("settings.json").is_file(), "the old folder is the backup");
        assert!(!base.join("oxytide.migrating").exists(), "no side folder is left");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(old.join("cache.key"), std::fs::Permissions::from_mode(0o600)).unwrap();
            let again = base.join("oxytide2");
            assert!(copy_if_new(&old, &again));
            assert_eq!(std::fs::metadata(again.join("cache.key")).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn nothing_is_overwritten_and_nothing_happens_without_an_old_folder() {
        let base = temp();
        let old = base.join("tidalfast");
        let new = base.join("oxytide");
        assert!(!copy_if_new(&old, &new), "no old folder");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("settings.json"), "old").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("settings.json"), "new").unwrap();
        assert!(!copy_if_new(&old, &new));
        assert_eq!(std::fs::read_to_string(new.join("settings.json")).unwrap(), "new");
        assert!(!move_if_new(&old, &new));
    }

    #[test]
    fn the_cache_is_moved() {
        let base = temp();
        let old = base.join("tidalfast");
        std::fs::create_dir_all(old.join("audio")).unwrap();
        std::fs::write(old.join("audio/1-LOSSLESS.audio"), "x").unwrap();
        let new = base.join("oxytide");
        assert!(move_if_new(&old, &new));
        assert!(new.join("audio/1-LOSSLESS.audio").is_file());
        assert!(!old.exists());
    }
}
