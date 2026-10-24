//! Update check and self-update from the GitHub releases of tidalfast.
//!
//! The newest release is compared with the running version; installing
//! downloads the matching file, verifies its SHA-256 (when GitHub gives one)
//! and replaces the running program. Windows can't overwrite a running exe,
//! but it can rename it, so the old file is moved aside first.

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

const REPO: &str = "wildesPepega/tidalfast";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// The file for this platform, if the release has one.
    pub asset: Option<Asset>,
}

pub enum UpdateEv {
    /// `manual`: the user asked, so "up to date" is worth saying.
    Checked { manual: bool, result: std::result::Result<Option<Release>, String> },
    Progress(f32),
    Installed,
    InstallFailed(String),
}

pub struct Updater {
    tx: Sender<UpdateEv>,
    rx: Receiver<UpdateEv>,
    ctx: egui::Context,
}

impl Updater {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, ctx }
    }

    pub fn poll(&self) -> Vec<UpdateEv> {
        self.rx.try_iter().collect()
    }

    pub fn check(&self, manual: bool) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = latest().map_err(|e| format!("{e:#}"));
            let _ = tx.send(UpdateEv::Checked { manual, result });
            ctx.request_repaint();
        });
    }

    pub fn install(&self, asset: Asset) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = download(&asset, |p| {
                let _ = tx.send(UpdateEv::Progress(p));
                ctx.request_repaint();
            })
            .and_then(|bytes| replace_running(&bytes));
            let _ = tx.send(match result {
                Ok(()) => UpdateEv::Installed,
                Err(e) => UpdateEv::InstallFailed(format!("{e:#}")),
            });
            ctx.request_repaint();
        });
    }
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(format!("tidalfast/{VERSION}"))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()?)
}

/// `Some(release)` when GitHub has a newer one than this version.
fn latest() -> Result<Option<Release>> {
    let resp = client()?
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .context("Could not reach GitHub")?;
    if !resp.status().is_success() {
        bail!("GitHub answered with {}", resp.status());
    }
    parse_release(&resp.json::<Value>()?)
}

fn parse_release(v: &Value) -> Result<Option<Release>> {
    let tag = v.get("tag_name").and_then(Value::as_str).ok_or_else(|| anyhow!("no release found"))?;
    let version = tag.trim_start_matches('v').to_string();
    if !is_newer(&version, VERSION) {
        return Ok(None);
    }
    let wanted = asset_name();
    let asset = v
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find(|a| a.get("name").and_then(Value::as_str) == wanted))
        .and_then(|a| {
            Some(Asset {
                url: a.get("browser_download_url")?.as_str()?.to_string(),
                size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
                sha256: a
                    .get("digest")
                    .and_then(Value::as_str)
                    .and_then(|d| d.strip_prefix("sha256:"))
                    .map(str::to_lowercase),
            })
        });
    Ok(Some(Release {
        version,
        notes: v.get("body").and_then(Value::as_str).unwrap_or("").trim().to_string(),
        page: v.get("html_url").and_then(Value::as_str).unwrap_or("").to_string(),
        asset,
    }))
}

/// "0.3.1" > "0.3.0"; anything after a `-` (pre-release) is ignored.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    (0..a.len().max(b.len())).map(|i| (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0))).find(|(x, y)| x != y).is_some_and(|(x, y)| x > y)
}

/// The release file that fits this installation.
fn asset_name() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("tidalfast-windows-x86_64.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(if std::env::var_os("APPIMAGE").is_some() { "tidalfast-x86_64.AppImage" } else { "tidalfast-linux-x86_64" })
    } else {
        None
    }
}

/// The file that gets replaced: the AppImage itself, or the program.
fn target_path() -> Result<PathBuf> {
    match std::env::var_os("APPIMAGE") {
        Some(p) if cfg!(target_os = "linux") => Ok(PathBuf::from(p)),
        _ => crate::desktop::current_exe().context("Could not locate the running program"),
    }
}

/// Why this installation can't update itself (`None` if it can).
pub fn cannot_self_update() -> Option<String> {
    asset_name()?;
    let target = match target_path() {
        Ok(t) => t,
        Err(e) => return Some(format!("{e:#}")),
    };
    let dir = target.parent()?;
    let probe = dir.join(".tidalfast-write-test");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(_) => Some(format!("{} is not writable (installed by a package manager?)", dir.display())),
    }
}

pub fn supported() -> bool {
    asset_name().is_some()
}

fn download(asset: &Asset, progress: impl Fn(f32)) -> Result<Vec<u8>> {
    // only ever fetch from GitHub
    if !(asset.url.starts_with("https://github.com/") || asset.url.starts_with("https://objects.githubusercontent.com/")) {
        bail!("Unexpected download address");
    }
    let mut resp = client()?.get(&asset.url).send().context("Download failed")?;
    if !resp.status().is_success() {
        bail!("Download failed ({})", resp.status());
    }
    let total = resp.content_length().unwrap_or(asset.size).max(1);
    let mut bytes = Vec::with_capacity(total as usize);
    let mut buf = [0u8; 64 * 1024];
    let mut last = 0.0f32;
    loop {
        let n = resp.read(&mut buf).context("Download interrupted")?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        let p = (bytes.len() as f32 / total as f32).min(1.0);
        if p - last >= 0.01 {
            last = p;
            progress(p);
        }
    }
    if asset.size > 0 && bytes.len() as u64 != asset.size {
        bail!("The download is incomplete");
    }
    if let Some(expected) = &asset.sha256 {
        let actual = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
        if &actual != expected {
            bail!("The download is corrupt (checksum mismatch)");
        }
    }
    Ok(bytes)
}

fn replace_running(bytes: &[u8]) -> Result<()> {
    // a program, not an error page
    let magic_ok = bytes.starts_with(b"\x7fELF") || bytes.starts_with(b"MZ");
    if bytes.len() < 1_000_000 || !magic_ok {
        bail!("The downloaded file is not a program");
    }
    let target = target_path()?;
    let dir = target.parent().ok_or_else(|| anyhow!("no folder"))?;
    let tmp = dir.join(".tidalfast-update.tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("Could not write to {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        // replacing the file by name works while it runs
        std::fs::rename(&tmp, &target).context("Could not replace the program")?;
    }
    #[cfg(windows)]
    {
        let old = old_path(&target);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&target, &old).context("Could not move the old program aside")?;
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::rename(&old, &target);
            return Err(anyhow!(e).context("Could not put the new program in place"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn old_path(target: &std::path::Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    target.with_file_name(name)
}

/// Delete what an update on Windows left behind (called at startup).
pub fn clean_up() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_path(&exe));
    }
}

/// Start the (updated) program; it waits a moment so this process can quit first.
pub fn start_new_instance() {
    if let Ok(path) = target_path() {
        let _ = std::process::Command::new(path)
            .args(["--delay-start", "1500"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.3.1", "0.3.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(!is_newer("0.3.0-beta", "0.3.0"));
    }

    #[test]
    fn reads_a_release() {
        let release = serde_json::json!({
            "tag_name": "v99.0.0",
            "html_url": "https://github.com/x/y/releases/tag/v99.0.0",
            "body": "notes",
            "assets": [{ "name": asset_name().unwrap_or("none"), "browser_download_url": "https://github.com/x/y/a",
                         "size": 5, "digest": "sha256:ABCD" }]
        });
        let r = parse_release(&release).unwrap().unwrap();
        assert_eq!(r.version, "99.0.0");
        if asset_name().is_some() {
            assert_eq!(r.asset.unwrap().sha256.as_deref(), Some("abcd"));
        }
        let old = serde_json::json!({ "tag_name": "v0.0.1", "assets": [] });
        assert!(parse_release(&old).unwrap().is_none());
    }
}
