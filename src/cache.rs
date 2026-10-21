//! Small on-disk JSON cache so lists and covers show up instantly on start.

use std::path::PathBuf;

use serde::{de::DeserializeOwned, Serialize};

fn dir() -> Option<PathBuf> {
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
