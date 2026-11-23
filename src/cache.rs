//! Small on-disk JSON cache so lists and covers show up instantly on start.

use std::path::PathBuf;

use serde::{de::DeserializeOwned, Serialize};

pub fn dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("tidalfast").join("data"))
}

fn file(name: &str) -> Option<PathBuf> {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    dir().map(|d| d.join(format!("{safe}.json")))
}

pub fn read<T: DeserializeOwned>(name: &str) -> Option<T> {
    let text = std::fs::read_to_string(file(name)?).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write<T: Serialize>(name: &str, value: &T) {
    let Some(path) = file(name) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(value) {
        let _ = std::fs::write(path, text);
    }
}

/// Forget all cached account data (on logout).
pub fn clear() {
    if let Some(dir) = dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Bytes in a folder, including everything below it.
fn folder_size(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => folder_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Everything the app keeps to start fast: songs, covers, the lists of the account.
pub fn total_size() -> u64 {
    [crate::audiocache::dir(), crate::net::cache_dir(), dir()]
        .into_iter()
        .flatten()
        .map(|p| folder_size(&p))
        .sum()
}

/// Delete all of it. (Settings and the sign-in are not part of the cache.)
pub fn clear_all() {
    if let Some(songs) = crate::audiocache::global() {
        songs.clear();
    }
    for path in [crate::net::cache_dir(), dir()].into_iter().flatten() {
        let _ = std::fs::remove_dir_all(path);
    }
}
