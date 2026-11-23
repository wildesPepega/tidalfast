//! A cache of the songs that were played, on disk: playing one again starts at once (no stream
//! lookup, no download) and works without a connection. The newest songs stay; the oldest go
//! when the cache grows past its limit.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;

static ENABLED: AtomicBool = AtomicBool::new(true);
static LIMIT: AtomicU64 = AtomicU64::new(2 * 1024 * 1024 * 1024);

/// Switch the cache on or off and set how big it may get (bytes).
pub fn configure(enabled: bool, limit_bytes: u64) {
    ENABLED.store(enabled, Ordering::Relaxed);
    LIMIT.store(limit_bytes, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The cache in the user's cache folder.
pub fn global() -> Option<Cache> {
    dirs::cache_dir().map(|d| Cache { dir: d.join("tidalfast").join("audio") })
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    #[cfg(test)]
    fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn file(&self, track: u64, quality: &str) -> PathBuf {
        let safe: String = quality.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        self.dir.join(format!("{track}-{safe}.audio"))
    }

    /// The cached file of a track and the quality it was stored in.
    pub fn find(&self, track: u64) -> Option<(PathBuf, String)> {
        let prefix = format!("{track}-");
        fs::read_dir(&self.dir).ok()?.flatten().find_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let quality = name.strip_prefix(&prefix)?.strip_suffix(".audio")?.to_string();
            (entry.metadata().ok()?.len() > 0).then(|| (entry.path(), quality))
        })
    }

    pub fn has(&self, track: u64) -> bool {
        self.find(track).is_some()
    }

    /// The song's bytes and quality; counts as "used just now" for the cleaning up.
    pub fn read(&self, track: u64) -> Option<(Vec<u8>, String)> {
        let (path, quality) = self.find(track)?;
        let bytes = fs::read(&path).ok()?;
        if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some((bytes, quality))
    }

    /// Keep a downloaded song. Written aside and renamed, so a file is never half there.
    pub fn store(&self, track: u64, quality: &str, bytes: &[u8]) {
        if bytes.is_empty() || fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        // one version of a song at a time
        self.remove(track);
        let target = self.file(track, quality);
        let part = target.with_extension("part");
        let written = fs::File::create(&part).and_then(|mut f| f.write_all(bytes).and_then(|_| f.flush()));
        if written.is_err() || fs::rename(&part, &target).is_err() {
            let _ = fs::remove_file(&part);
        }
    }

    pub fn remove(&self, track: u64) {
        while let Some((path, _)) = self.find(track) {
            if fs::remove_file(path).is_err() {
                break;
            }
        }
    }

    /// (path, size, last used) of every cached song.
    fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "audio"))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
            })
            .collect()
    }

    /// Bytes the cache takes.
    pub fn size(&self) -> u64 {
        self.entries().iter().map(|(_, len, _)| len).sum()
    }

    /// Delete the songs used longest ago until the cache fits `limit` (the newest always stays).
    pub fn prune(&self, limit: u64) {
        let mut entries = self.entries();
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        entries.sort_by_key(|(_, _, used)| *used);
        let keep_newest = entries.len().saturating_sub(1);
        for (path, len, _) in entries.into_iter().take(keep_newest) {
            if total <= limit {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }

    /// Throw everything away.
    pub fn clear(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }

    /// Clean up after a store, with the configured limit.
    pub fn prune_to_limit(&self) {
        self.prune(LIMIT.load(Ordering::Relaxed));
    }
}

/// A scratch folder that disappears again (for the tests).
#[cfg(test)]
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tidalfast-audio-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_finds_and_reads_back_a_song_with_its_quality() {
        let c = Cache::at(scratch("roundtrip"));
        assert!(!c.has(42));
        c.store(42, "HI_RES_LOSSLESS", b"song bytes");
        let (path, quality) = c.find(42).unwrap();
        assert!(path.ends_with("42-HI_RES_LOSSLESS.audio"));
        assert_eq!(quality, "HI_RES_LOSSLESS");
        assert_eq!(c.read(42).unwrap(), (b"song bytes".to_vec(), "HI_RES_LOSSLESS".to_string()));
        assert!(!c.has(4), "an id that is only the start of another is not a match");
        assert!(c.find(2).is_none());
        c.clear();
    }

    #[test]
    fn a_new_version_replaces_the_old_one() {
        let c = Cache::at(scratch("replace"));
        c.store(7, "LOSSLESS", b"old");
        c.store(7, "HI_RES_LOSSLESS", b"newer and bigger");
        assert_eq!(c.entries().len(), 1);
        assert_eq!(c.read(7).unwrap().1, "HI_RES_LOSSLESS");
        c.clear();
    }

    #[test]
    fn empty_downloads_are_not_kept() {
        let c = Cache::at(scratch("empty"));
        c.store(1, "LOSSLESS", b"");
        assert!(!c.has(1));
        c.clear();
    }

    #[test]
    fn the_oldest_songs_go_first_when_it_grows_too_big() {
        let c = Cache::at(scratch("prune"));
        for id in 1..=4u64 {
            c.store(id, "LOSSLESS", &vec![0u8; 100]);
            // make the order of use unmistakable
            let (path, _) = c.find(id).unwrap();
            let file = fs::OpenOptions::new().write(true).open(path).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 * id)).unwrap();
        }
        assert_eq!(c.size(), 400);
        // song 1 is played again: it is now the newest
        c.read(1).unwrap();
        c.prune(250);
        assert!(c.has(1) && c.has(4), "the recently used stay");
        assert!(!c.has(2) && !c.has(3), "the longest unused go");
        assert!(c.size() <= 250);
        // even a limit smaller than one song keeps the newest
        c.prune(1);
        assert_eq!(c.entries().len(), 1);
        c.clear();
        assert_eq!(c.size(), 0);
    }
}
