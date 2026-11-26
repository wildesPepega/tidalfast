//! Small file helpers for data that must not be half written or readable by other users.

use std::io::Write;
use std::path::Path;

/// Write `bytes` to `path` so that it is either the old or the new file, never half of one, and
/// (on Unix) readable only by this user. The folder is created (private, too) if needed.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    create_private_dir(dir)?;
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        // an old temp file of the same name may have had other permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Create a folder (and its parents); the folder itself is private to this user on Unix.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// A folder for sockets and scratch files that only this user can enter: `$XDG_RUNTIME_DIR`
/// where there is one (it is private by definition), otherwise a folder of our own in the
/// user's cache (never the shared temp folder, where another user could plant files).
#[cfg(unix)]
pub fn runtime_dir() -> std::path::PathBuf {
    if let Some(dir) = dirs::runtime_dir() {
        return dir;
    }
    let dir = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("oxytide").join("run");
    let _ = create_private_dir(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_and_private_files() {
        let dir = std::env::temp_dir().join(format!("oxytide-fsx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("secret.json");
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1, "no temp file stays behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
