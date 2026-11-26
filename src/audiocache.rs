//! A cache of the songs that were played, on disk: playing one again starts at once (no stream
//! lookup, no download) and works without a connection. The newest songs stay; the oldest go
//! when the cache grows past its limit.
//!
//! The files are scrambled with a key that belongs to this installation, so they cannot simply be
//! opened in a player or copied around as music files. (That keeps casual copying out; it is no
//! protection against someone who has the key file too.)

use std::fs;
use std::io::Write;
use std::sync::OnceLock;

use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::ChaCha20;
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
    let dir = dirs::cache_dir()?.join("oxytide").join("audio");
    Some(Cache { dir, key: install_key()? })
}

/// Where the songs are kept.
pub fn dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("oxytide").join("audio"))
}

pub struct Cache {
    dir: PathBuf,
    key: [u8; 32],
}

/// "Scrambled song, version 1".
const MAGIC: &[u8; 4] = b"TFA1";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 8;
const HEADER_LEN: usize = 4 + NONCE_LEN + TAG_LEN;

/// This installation's key, made on first use and kept next to the settings.
fn install_key() -> Option<[u8; 32]> {
    static KEY: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    *KEY.get_or_init(|| {
        let path = dirs::config_dir()?.join("oxytide").join("cache.key");
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return Some(key);
            }
        }
        let key = random_bytes::<32>();
        // private from the first byte on (no moment where it is readable for others)
        crate::fsx::write_private(&path, &key).ok()?;
        Some(key)
    })
}

/// Unpredictable bytes without another dependency: the standard library seeds its hash with
/// the operating system's randomness for every new `RandomState`.
fn random_bytes<const N: usize>() -> [u8; N] {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; N];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i as u64);
        h.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        h.write_u32(std::process::id());
        let bytes = h.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out
}

/// What identifies the right key in the file (so a wrong key is noticed, not played as noise).
fn tag(key: &[u8; 32], nonce: &[u8; NONCE_LEN]) -> [u8; TAG_LEN] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"oxytide cache tag");
    hasher.update(key);
    hasher.update(nonce);
    let digest = hasher.finalize();
    let mut out = [0u8; TAG_LEN];
    out.copy_from_slice(&digest[..TAG_LEN]);
    out
}

impl Cache {
    #[cfg(test)]
    fn at(dir: PathBuf) -> Self {
        Self { dir, key: [9u8; 32] }
    }

    fn scramble(&self, plain: &[u8]) -> Vec<u8> {
        let nonce = random_bytes::<NONCE_LEN>();
        let mut out = Vec::with_capacity(HEADER_LEN + plain.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&tag(&self.key, &nonce));
        out.extend_from_slice(plain);
        ChaCha20::new((&self.key).into(), (&nonce).into()).apply_keystream(&mut out[HEADER_LEN..]);
        out
    }

    /// The song inside a scrambled file; `None` for anything else (an old plain copy, a file of
    /// another installation, a damaged one).
    fn unscramble(&self, file: Vec<u8>) -> Option<Vec<u8>> {
        if file.len() <= HEADER_LEN || &file[..4] != MAGIC {
            return None;
        }
        let nonce: [u8; NONCE_LEN] = file[4..4 + NONCE_LEN].try_into().ok()?;
        if file[4 + NONCE_LEN..HEADER_LEN] != tag(&self.key, &nonce) {
            return None;
        }
        let mut plain = file;
        plain.drain(..HEADER_LEN);
        ChaCha20::new((&self.key).into(), (&nonce).into()).apply_keystream(&mut plain);
        Some(plain)
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
        let Some(bytes) = fs::read(&path).ok().and_then(|f| self.unscramble(f)) else {
            // not ours to read (an unscrambled copy from an older version, or damaged): drop it
            let _ = fs::remove_file(&path);
            return None;
        };
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
        let scrambled = self.scramble(bytes);
        let written = fs::File::create(&part).and_then(|mut f| f.write_all(&scrambled).and_then(|_| f.flush()));
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
        let _ = fs::remove_file(self.loudness_file(track));
    }

    fn loudness_file(&self, track: u64) -> PathBuf {
        self.dir.join(format!("{track}.gain"))
    }

    /// Keep the loudness of a song next to it (a cached song has no stream info to take it from).
    pub fn store_loudness(&self, track: u64, loudness: crate::audio::Loudness) {
        let text = format!("{} {}", loudness.gain_db, loudness.peak);
        let _ = fs::create_dir_all(&self.dir);
        let _ = fs::write(self.loudness_file(track), text);
    }

    pub fn loudness(&self, track: u64) -> Option<crate::audio::Loudness> {
        let text = fs::read_to_string(self.loudness_file(track)).ok()?;
        let mut parts = text.split_whitespace().map(|p| p.parse::<f32>().ok());
        let gain_db = parts.next()??;
        let peak = parts.next()??;
        (gain_db.is_finite() && peak.is_finite()).then_some(crate::audio::Loudness { gain_db, peak })
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
                // and the song's loudness (`<id>.gain` next to `<id>-<quality>.audio`)
                let id = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('-').next()).and_then(|n| n.parse::<u64>().ok());
                if let Some(id) = id {
                    let _ = fs::remove_file(self.loudness_file(id));
                }
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
    let dir = std::env::temp_dir().join(format!("oxytide-audio-test-{}-{name}", std::process::id()));
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
    fn files_on_disk_are_not_the_song() {
        let c = Cache::at(scratch("scrambled"));
        let song: Vec<u8> = b"fLaC-pretend-this-is-a-song-".iter().cycle().take(4000).copied().collect();
        c.store(5, "LOSSLESS", &song);
        let (path, _) = c.find(5).unwrap();
        let on_disk = fs::read(&path).unwrap();
        assert_eq!(on_disk.len(), song.len() + HEADER_LEN);
        assert!(!on_disk.windows(10).any(|w| w == b"pretend-th"), "no readable piece of the song");
        assert!(!on_disk.starts_with(b"fLaC"), "does not look like a FLAC file");
        assert_eq!(c.read(5).unwrap().0, song);
        // two copies of the same song look different (fresh nonce)
        c.store(6, "LOSSLESS", &song);
        let (p6, _) = c.find(6).unwrap();
        assert_ne!(fs::read(p6).unwrap()[HEADER_LEN..], on_disk[HEADER_LEN..]);
        c.clear();
    }

    #[test]
    fn another_installations_files_and_old_plain_copies_are_dropped() {
        let dir = scratch("foreign");
        let mine = Cache::at(dir.clone());
        mine.store(8, "LOSSLESS", b"a song that is long enough to matter");
        let stranger = Cache { dir: dir.clone(), key: [1u8; 32] };
        assert!(stranger.read(8).is_none(), "wrong key: not played as noise");
        assert!(!mine.has(8), "and removed");
        // an unscrambled file as the first version left them
        fs::write(dir.join("9-LOSSLESS.audio"), b"fLaC plain old copy that nobody should be able to just play").unwrap();
        assert!(mine.has(9));
        assert!(mine.read(9).is_none());
        assert!(!mine.has(9), "dropped for good");
        mine.clear();
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
    fn loudness_is_kept_with_the_song_and_goes_with_it() {
        let c = Cache::at(scratch("loudness"));
        c.store(3, "LOSSLESS", b"song");
        assert!(c.loudness(3).is_none());
        c.store_loudness(3, crate::audio::Loudness { gain_db: -7.5, peak: 0.98 });
        let l = c.loudness(3).unwrap();
        assert_eq!((l.gain_db, l.peak), (-7.5, 0.98));
        c.remove(3);
        assert!(c.loudness(3).is_none());
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
        assert_eq!(c.size(), 4 * (100 + HEADER_LEN as u64));
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
