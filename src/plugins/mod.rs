//! Plugins: small scripts that add to oxytide (buttons, menu entries, settings, behaviour) and run
//! in a sandbox. See `API.md` for what a plugin can do and the security rules it lives by.
//!
//! * A plugin is a folder in `plugins/` next to the settings: `plugin.toml` and `main.rhai`.
//! * Nothing runs until the user has seen what the plugin asks for and allowed it. A plugin that
//!   changes afterwards (script or permissions) has to be allowed again.
//! * Scripts cannot touch files, programs or the network on their own; everything goes through
//!   functions that check the plugin's permissions.

pub mod manifest;
pub mod registry;
pub mod runtime;

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use manifest::Manifest;
pub use registry::{Button, Category, Entry, MenuItem, Place, Registry};
pub use runtime::{FromPlugin, Shared, ToPlugin};

/// What the user has decided about one plugin (kept in the settings).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PluginCfg {
    pub enabled: bool,
    /// The permissions the user agreed to.
    pub granted: Vec<String>,
    /// The servers the user agreed to.
    pub hosts: Vec<String>,
    /// The fingerprint of the plugin that was agreed to.
    pub hash: String,
}

/// A plugin found on disk.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: Manifest,
    #[allow(dead_code)]
    pub dir: PathBuf,
    pub script: String,
    /// SHA-256 of `plugin.toml` and the script.
    pub hash: String,
    /// Comes with oxytide (not from the plugins folder).
    pub builtin: bool,
}

/// The plugins that come with oxytide (`examples/plugins`), so there is something to switch on
/// without copying folders around. A folder of the same name in the plugins folder wins.
const BUNDLED: &[(&str, &str)] = &[
    (include_str!("../../examples/plugins/focus/plugin.toml"), include_str!("../../examples/plugins/focus/main.rhai")),
    (include_str!("../../examples/plugins/hello/plugin.toml"), include_str!("../../examples/plugins/hello/main.rhai")),
    (include_str!("../../examples/plugins/now-playing-card/plugin.toml"), include_str!("../../examples/plugins/now-playing-card/main.rhai")),
];

/// The plugins that come with oxytide.
pub fn bundled(root: &Path) -> Vec<Plugin> {
    BUNDLED
        .iter()
        .filter_map(|(manifest_text, script)| {
            let manifest = manifest::parse(manifest_text).ok()?;
            let dir = root.join(&manifest.id);
            Some(Plugin { hash: hash_of(manifest_text, script), manifest, dir, script: script.to_string(), builtin: true })
        })
        .collect()
}

pub fn plugins_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("oxytide").join("plugins")
}

pub fn data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("oxytide").join("plugin-data")
}

pub fn hash_of(manifest_text: &str, script: &str) -> String {
    let mut h = Sha256::new();
    h.update((manifest_text.len() as u64).to_le_bytes());
    h.update(manifest_text.as_bytes());
    h.update(script.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Read one plugin folder.
pub fn load(dir: &Path) -> Result<Plugin, String> {
    let manifest_text = std::fs::read_to_string(dir.join("plugin.toml")).map_err(|_| "no plugin.toml".to_string())?;
    let manifest = manifest::parse(&manifest_text)?;
    let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if folder != manifest.id {
        return Err(format!("the folder is called “{folder}” but the id is “{}” (they must match)", manifest.id));
    }
    let path = dir.join("main.rhai");
    let meta = std::fs::symlink_metadata(&path).map_err(|_| "no main.rhai".to_string())?;
    if !meta.is_file() {
        return Err("main.rhai is not a plain file".into());
    }
    if meta.len() as usize > manifest::MAX_SCRIPT_BYTES {
        return Err("main.rhai is too big (256 KB at most)".into());
    }
    let script = std::fs::read_to_string(&path).map_err(|e| format!("main.rhai: {e}"))?;
    let hash = hash_of(&manifest_text, &script);
    Ok(Plugin { manifest, dir: dir.to_path_buf(), script, hash, builtin: false })
}

/// All plugins in a folder, and what is wrong with the ones that cannot be loaded.
pub fn discover(root: &Path) -> (Vec<Plugin>, Vec<(String, String)>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else { return (found, problems) };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs.into_iter().take(64) {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        if name.starts_with('.') {
            continue;
        }
        match load(&dir) {
            Ok(p) => found.push(p),
            Err(e) => problems.push((name, e)),
        }
    }
    (found, problems)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The plugin asks for more than the user allowed.
    MorePermissions,
    /// The plugin's files are not the ones the user looked at.
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Off,
    NeedsConsent(Why),
    Allowed,
}

/// May this plugin run? Only when it is switched on, everything it asks for was allowed, and its
/// files are the ones that were looked at (with developer mode, edits to the script are accepted).
pub fn status(p: &Plugin, cfg: Option<&PluginCfg>, developer: bool) -> Status {
    let Some(cfg) = cfg.filter(|c| c.enabled) else { return Status::Off };
    let granted: HashSet<&str> = cfg.granted.iter().map(String::as_str).collect();
    let hosts: HashSet<&str> = cfg.hosts.iter().map(String::as_str).collect();
    let covered = p.manifest.permissions.iter().all(|x| granted.contains(x.id())) && p.manifest.hosts.iter().all(|h| hosts.contains(h.as_str()));
    if !covered {
        return Status::NeedsConsent(Why::MorePermissions);
    }
    if cfg.hash != p.hash && !developer {
        return Status::NeedsConsent(Why::Changed);
    }
    Status::Allowed
}

/// What switching a plugin on records: exactly what it asks for now.
pub fn consent(p: &Plugin) -> PluginCfg {
    PluginCfg {
        enabled: true,
        granted: p.manifest.permissions.iter().map(|x| x.id().to_string()).collect(),
        hosts: p.manifest.hosts.clone(),
        hash: p.hash.clone(),
    }
}

// ---------------------------------------------------------------------------- the running ones

/// A plugin that has been started.
pub struct Running {
    pub tx: Sender<ToPlugin>,
    pub hash: String,
    pub alive: bool,
    pub ready: bool,
    pub registry: Registry,
    pub hooks: Vec<String>,
    /// The last lines it printed or failed with (`true` = an error).
    pub log: VecDeque<(bool, String)>,
    pub stopped: Option<String>,
    /// What this run of the plugin says. Each run has its own channel, so the last words of a run
    /// that was switched off cannot reach the next one (and mark it as stopped).
    rx: Receiver<(String, FromPlugin)>,
}

pub struct Manager {
    pub plugins: Vec<Plugin>,
    pub problems: Vec<(String, String)>,
    pub running: BTreeMap<String, Running>,
    pub shared: Arc<Mutex<Shared>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    root: PathBuf,
    data: PathBuf,
}

const LOG_LINES: usize = 60;

impl Manager {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_dirs(wake, plugins_dir(), data_dir())
    }

    pub fn with_dirs(wake: Arc<dyn Fn() + Send + Sync>, root: PathBuf, data: PathBuf) -> Self {
        let mut m = Self {
            plugins: Vec::new(),
            problems: Vec::new(),
            running: BTreeMap::new(),
            shared: Arc::new(Mutex::new(Shared::default())),
            wake,
            root,
            data,
        };
        m.rescan();
        m
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Look at the folder again.
    pub fn rescan(&mut self) {
        let (mut plugins, problems) = discover(&self.root);
        for b in bundled(&self.root) {
            if !plugins.iter().any(|p| p.manifest.id == b.manifest.id) {
                plugins.push(b);
            }
        }
        plugins.sort_by(|a, b| a.manifest.name.to_lowercase().cmp(&b.manifest.name.to_lowercase()));
        self.plugins = plugins;
        self.problems = problems;
    }

    /// Start what may run and stop what may not (or has changed). `master` is the global switch.
    pub fn sync(&mut self, cfg: &BTreeMap<String, PluginCfg>, master: bool, developer: bool) {
        let wanted: BTreeMap<String, Plugin> = self
            .plugins
            .iter()
            .filter(|p| master && status(p, cfg.get(&p.manifest.id), developer) == Status::Allowed)
            .map(|p| (p.manifest.id.clone(), p.clone()))
            .collect();
        let stale: Vec<String> = self
            .running
            .iter()
            .filter(|(id, r)| wanted.get(*id).map_or(true, |p| p.hash != r.hash))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.stop(&id);
        }
        for (id, p) in wanted {
            if self.running.contains_key(&id) {
                continue;
            }
            let (out, rx) = channel();
            let host = runtime::Host {
                id: id.clone(),
                caps: p.manifest.permissions.iter().copied().collect(),
                hosts: p.manifest.hosts.clone(),
                data_dir: self.data.clone(),
                shared: self.shared.clone(),
                out,
                wake: self.wake.clone(),
            };
            let tx = runtime::spawn(host, p.script.clone());
            self.running.insert(
                id,
                Running { tx, hash: p.hash.clone(), alive: true, ready: false, registry: Registry::default(), hooks: Vec::new(), log: VecDeque::new(), stopped: None, rx },
            );
        }
    }

    pub fn stop(&mut self, id: &str) {
        if let Some(r) = self.running.remove(id) {
            let _ = r.tx.send(ToPlugin::Shutdown);
        }
    }

    /// Start a stopped plugin again (the next `sync` does it).
    pub fn restart(&mut self, id: &str) {
        self.stop(id);
    }

    pub fn stop_all(&mut self) {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }

    /// Take in what the plugins have said; what is returned is for the app to act on.
    pub fn poll(&mut self) -> Vec<(String, FromPlugin)> {
        let mut todo = Vec::new();
        let incoming: Vec<(String, FromPlugin)> = self.running.iter().flat_map(|(id, r)| r.rx.try_iter().map(move |(_, m)| (id.clone(), m))).collect();
        for (id, msg) in incoming {
            let Some(r) = self.running.get_mut(&id) else { continue };
            let line = |r: &mut Running, error: bool, text: String| {
                r.log.push_back((error, text));
                while r.log.len() > LOG_LINES {
                    r.log.pop_front();
                }
            };
            match msg {
                FromPlugin::Registry(reg) => r.registry = reg,
                FromPlugin::Log(t) => line(r, false, t),
                FromPlugin::Error(t) => line(r, true, t),
                FromPlugin::Ready { hooks } => {
                    r.ready = true;
                    r.hooks = hooks.clone();
                    todo.push((id, FromPlugin::Ready { hooks }));
                }
                FromPlugin::Stopped(reason) => {
                    r.alive = false;
                    if let Some(why) = &reason {
                        line(r, true, why.clone());
                    }
                    r.stopped = reason;
                }
                other => todo.push((id, other)),
            }
        }
        todo
    }

    pub fn send(&self, id: &str, msg: ToPlugin) {
        if let Some(r) = self.running.get(id).filter(|r| r.alive) {
            let _ = r.tx.send(msg);
        }
    }

    /// Tell every running plugin something.
    pub fn broadcast(&self, make: impl Fn() -> ToPlugin) {
        for r in self.running.values().filter(|r| r.alive && r.ready) {
            let _ = r.tx.send(make());
        }
    }

    pub fn buttons(&self, place: Place) -> Vec<(&str, &Button)> {
        self.running.iter().filter(|(_, r)| r.alive).flat_map(|(id, r)| r.registry.buttons.iter().filter(move |b| b.place == place).map(move |b| (id.as_str(), b))).collect()
    }

    pub fn menu_items(&self) -> Vec<(String, MenuItem)> {
        self.running.iter().filter(|(_, r)| r.alive).flat_map(|(id, r)| r.registry.menu_items.iter().map(move |m| (id.clone(), m.clone()))).collect()
    }

    pub fn categories(&self) -> Vec<(&str, &Category)> {
        self.running.iter().filter(|(_, r)| r.alive).flat_map(|(id, r)| r.registry.categories.iter().map(move |c| (id.as_str(), c))).collect()
    }

    /// Entries plugins have added to a built-in settings page.
    pub fn extra_entries(&self, page: &str) -> Vec<(&str, &Entry)> {
        self.running
            .iter()
            .filter(|(_, r)| r.alive)
            .flat_map(|(id, r)| r.registry.extra.iter().filter(move |(p, _)| p == page).map(move |(_, e)| (id.as_str(), e)))
            .collect()
    }

    /// The entries one plugin added to built-in settings pages (page, entry).
    pub fn extra_for(&self, id: &str) -> Vec<(String, Entry)> {
        self.running.get(id).filter(|r| r.alive).map(|r| r.registry.extra.clone()).unwrap_or_default()
    }

    /// Has this running plugin made any settings?
    pub fn has_settings(&self, id: &str) -> bool {
        self.running.get(id).is_some_and(|r| r.alive && (!r.registry.categories.is_empty() || !r.registry.extra.is_empty()))
    }

    pub fn hidden(&self, feature: &str) -> bool {
        self.running.values().any(|r| r.alive && r.registry.hidden.iter().any(|h| h == feature))
    }

    /// Ask the plugins that answer this hook; true unless one of them says no. The wait is short and
    /// bounded: a plugin that is slow is taken as saying yes.
    pub fn ask(&self, name: &str, data: &serde_json::Value) -> bool {
        let mut allowed = true;
        for r in self.running.values().filter(|r| r.alive && r.hooks.iter().any(|h| h == name)) {
            let (reply, answer) = channel();
            if r.tx.send(ToPlugin::Hook { name: name.to_string(), data: data.clone(), reply }).is_ok() {
                if let Ok(false) = answer.recv_timeout(Duration::from_millis(150)) {
                    allowed = false;
                }
            }
        }
        allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("oxytide-plugins-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(root: &Path, id: &str, perms: &str, script: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"1\"\napi = 1\npermissions = [{perms}]\n")).unwrap();
        std::fs::write(dir.join("main.rhai"), script).unwrap();
    }

    #[test]
    fn finds_plugins_and_explains_the_broken_ones() {
        let root = temp();
        write(&root, "good", "\"ui.toast\"", "");
        write(&root, "other", "", "");
        std::fs::write(root.join("other").join("plugin.toml"), "id = \"wrong\"\nname = \"x\"\nversion = \"1\"\napi = 1\n").unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        let (found, problems) = discover(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manifest.id, "good");
        assert_eq!(problems.len(), 2);
        assert!(problems.iter().any(|(n, e)| n == "other" && e.contains("must match")));
    }

    #[test]
    fn a_plugin_runs_only_after_consent_and_only_as_long_as_it_is_unchanged() {
        let root = temp();
        write(&root, "p", "\"ui.toast\"", "let a = 1;");
        let p = load(&root.join("p")).unwrap();
        assert_eq!(status(&p, None, false), Status::Off);
        let cfg = consent(&p);
        assert_eq!(status(&p, Some(&cfg), false), Status::Allowed);
        assert_eq!(status(&p, Some(&PluginCfg { enabled: false, ..cfg.clone() }), false), Status::Off);
        // the script is swapped
        write(&root, "p", "\"ui.toast\"", "let a = 2;");
        let changed = load(&root.join("p")).unwrap();
        assert_eq!(status(&changed, Some(&cfg), false), Status::NeedsConsent(Why::Changed));
        assert_eq!(status(&changed, Some(&cfg), true), Status::Allowed, "developer mode accepts edits to the script");
        // more permissions are never accepted silently, not even in developer mode
        write(&root, "p", "\"ui.toast\", \"player.control\"", "let a = 2;");
        let greedy = load(&root.join("p")).unwrap();
        assert_eq!(status(&greedy, Some(&cfg), true), Status::NeedsConsent(Why::MorePermissions));
        assert_eq!(status(&greedy, Some(&consent(&greedy)), false), Status::Allowed);
    }

    #[test]
    fn the_bundled_plugins_are_offered_and_a_folder_of_the_same_name_wins() {
        let root = temp();
        let m = Manager::with_dirs(Arc::new(|| {}), root.clone(), temp());
        for id in ["focus", "hello", "now-playing-card"] {
            assert!(m.plugins.iter().any(|p| p.manifest.id == id && p.builtin), "{id} is offered");
        }
        write(&root, "focus", "\"ui.modify\"", "let mine = 1;");
        let m = Manager::with_dirs(Arc::new(|| {}), root, temp());
        let focus: Vec<_> = m.plugins.iter().filter(|p| p.manifest.id == "focus").collect();
        assert_eq!(focus.len(), 1);
        assert!(!focus[0].builtin && focus[0].script.contains("mine"));
    }

    /// Switched off and on again, a plugin must run again (the last words of the old run, such
    /// as "stopped", must not reach the new one).
    #[test]
    fn a_plugin_switched_off_and_on_again_runs() {
        let root = temp();
        write(&root, "p", "\"ui.buttons\"", "ui::button(#{ id: \"b\", label: \"B\", place: \"header\" });");
        let mut m = Manager::with_dirs(Arc::new(|| {}), root, temp());
        let p = m.plugins.iter().find(|p| p.manifest.id == "p").unwrap().clone();
        let mut cfg = BTreeMap::new();
        cfg.insert("p".to_string(), consent(&p));
        let wait_ready = |m: &mut Manager| {
            let end = std::time::Instant::now() + Duration::from_secs(5);
            while !m.running.get("p").is_some_and(|r| r.ready) && std::time::Instant::now() < end {
                std::thread::sleep(Duration::from_millis(30));
                m.poll();
            }
        };
        m.sync(&cfg, true, false);
        wait_ready(&mut m);
        cfg.get_mut("p").unwrap().enabled = false;
        m.sync(&cfg, true, false);
        // the old run winds down meanwhile (the app does not poll while nothing runs)
        std::thread::sleep(Duration::from_millis(300));
        cfg.get_mut("p").unwrap().enabled = true;
        m.sync(&cfg, true, false);
        wait_ready(&mut m);
        std::thread::sleep(Duration::from_millis(200));
        m.poll();
        let r = m.running.get("p").expect("running again");
        assert!(r.alive && r.ready && r.stopped.is_none());
        assert_eq!(m.buttons(Place::Header).len(), 1);
    }

    #[test]
    fn the_hash_covers_both_files() {
        assert_ne!(hash_of("a", "b"), hash_of("ab", ""));
        assert_ne!(hash_of("a", "b"), hash_of("a", "c"));
        assert_eq!(hash_of("a", "b"), hash_of("a", "b"));
        assert_eq!(hash_of("", "").len(), 64);
    }

    #[test]
    fn the_manager_starts_what_is_allowed_and_stops_what_is_not() {
        let root = temp();
        write(&root, "on", "\"ui.buttons\"", "ui::button(#{ id: \"b\", label: \"B\", place: \"header\" });");
        write(&root, "off", "\"ui.buttons\"", "ui::button(#{ id: \"c\", label: \"C\", place: \"header\" });");
        let mut m = Manager::with_dirs(Arc::new(|| {}), root.clone(), temp());
        let on = m.plugins.iter().find(|p| p.manifest.id == "on").unwrap().clone();
        let mut cfg = BTreeMap::new();
        cfg.insert("on".to_string(), consent(&on));
        m.sync(&cfg, true, false);
        assert!(m.running.contains_key("on") && !m.running.contains_key("off"));
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while m.buttons(Place::Header).is_empty() && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
            m.poll();
        }
        assert_eq!(m.buttons(Place::Header).len(), 1);
        // the master switch stops everything
        m.sync(&cfg, false, false);
        assert!(m.running.is_empty());
        assert!(m.buttons(Place::Header).is_empty());
    }

    /// The examples in the repository are part of the documentation: they have to work.
    #[test]
    fn the_example_plugins_load_and_start_without_errors() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join("plugins");
        let mut m = Manager::with_dirs(Arc::new(|| {}), root, temp());
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        assert!(m.plugins.len() >= 3);
        let cfg: BTreeMap<String, PluginCfg> = m.plugins.iter().map(|p| (p.manifest.id.clone(), consent(p))).collect();
        m.sync(&cfg, true, false);
        let end = std::time::Instant::now() + Duration::from_secs(8);
        while m.running.values().any(|r| !r.ready && r.alive) && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
            m.poll();
        }
        for (id, r) in &m.running {
            assert!(r.ready && r.alive, "{id}: {:?}", r.stopped);
            assert!(!r.log.iter().any(|(error, _)| *error), "{id}: {:?}", r.log);
        }
        assert!(!m.buttons(Place::Header).is_empty());
        assert!(!m.menu_items().is_empty());
        assert!(m.hidden("lyrics") == false, "focus mode starts off");
    }

    #[test]
    fn the_example_themes_are_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join("themes");
        let (themes, problems) = crate::theme_file::discover(&dir);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(themes.len() >= 3);
    }

    /// API.md has to name everything the code offers.
    #[test]
    fn the_documentation_mentions_every_permission_part_and_icon() {
        let doc = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("API.md")).unwrap();
        for p in manifest::Permission::ALL {
            assert!(doc.contains(&format!("`{}`", p.id())), "permission {} is not in API.md", p.id());
        }
        for (part, _) in registry::FEATURES {
            assert!(doc.contains(&format!("`{part}`")), "part {part} is not in API.md");
        }
        for icon in registry::ICON_NAMES {
            assert!(doc.contains(&format!("`{icon}`")), "icon {icon} is not in API.md");
        }
        for page in registry::BUILTIN_PAGES {
            assert!(doc.contains(&format!("\"{page}\"")), "settings page {page} is not in API.md");
        }
        for handler in ["on_load", "on_track", "on_playing", "on_tick", "on_action", "on_setting", "on_unload", "hook_next", "hook_previous", "hook_play"] {
            assert!(doc.contains(handler), "{handler} is not in API.md");
        }
        assert!(doc.contains(&format!("api = {}", manifest::API_VERSION)));
    }
}
