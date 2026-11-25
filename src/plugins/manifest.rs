//! What a plugin says about itself (`plugin.toml`) and what it may be allowed to do.
//!
//! A plugin is a folder with `plugin.toml` and one script. It can do nothing at all that is not in
//! its list of permissions, and every permission has to be granted by the user.

use serde::Deserialize;

/// The version of the plugin interface this build speaks.
pub const API_VERSION: u32 = 1;
/// Largest script a plugin may have.
pub const MAX_SCRIPT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Low,
    Medium,
    High,
}

/// Everything a plugin can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    PlayerRead,
    PlayerControl,
    UiToast,
    UiButtons,
    UiMenus,
    UiSettings,
    UiModify,
    Hooks,
    Storage,
    Network,
    Clipboard,
    OpenUrl,
}

impl Permission {
    pub const ALL: [Permission; 12] = [
        Permission::PlayerRead,
        Permission::PlayerControl,
        Permission::UiToast,
        Permission::UiButtons,
        Permission::UiMenus,
        Permission::UiSettings,
        Permission::UiModify,
        Permission::Hooks,
        Permission::Storage,
        Permission::Network,
        Permission::Clipboard,
        Permission::OpenUrl,
    ];

    /// The name used in `plugin.toml`.
    pub fn id(self) -> &'static str {
        match self {
            Permission::PlayerRead => "player.read",
            Permission::PlayerControl => "player.control",
            Permission::UiToast => "ui.toast",
            Permission::UiButtons => "ui.buttons",
            Permission::UiMenus => "ui.menus",
            Permission::UiSettings => "ui.settings",
            Permission::UiModify => "ui.modify",
            Permission::Hooks => "hooks",
            Permission::Storage => "storage",
            Permission::Network => "network",
            Permission::Clipboard => "clipboard",
            Permission::OpenUrl => "open_url",
        }
    }

    pub fn parse(id: &str) -> Option<Permission> {
        Permission::ALL.into_iter().find(|p| p.id() == id)
    }

    /// What the user is told when asked to allow it.
    pub fn description(self) -> &'static str {
        match self {
            Permission::PlayerRead => "See what is playing (title, artist, album, position) and whether it is playing",
            Permission::PlayerControl => "Play, pause, skip, seek and change the volume",
            Permission::UiToast => "Show short notifications",
            Permission::UiButtons => "Add buttons to the window",
            Permission::UiMenus => "Add entries to the song menu",
            Permission::UiSettings => "Add its own settings page and entries to the settings",
            Permission::UiModify => "Hide built-in parts of the interface",
            Permission::Hooks => "Be asked before a song is skipped or started, and be able to cancel that",
            Permission::Storage => "Keep small amounts of its own data on this computer",
            Permission::Network => "Talk to the internet servers listed below (and only those)",
            Permission::Clipboard => "Put text into the clipboard",
            Permission::OpenUrl => "Open web links in your browser",
        }
    }

    pub fn risk(self) -> Risk {
        match self {
            Permission::Network | Permission::Hooks => Risk::High,
            Permission::PlayerControl | Permission::UiModify | Permission::Clipboard | Permission::OpenUrl => Risk::Medium,
            _ => Risk::Low,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawNetwork {
    #[serde(default)]
    hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Raw {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage: String,
    api: u32,
    #[serde(default)]
    permissions: Vec<String>,
    network: Option<RawNetwork>,
}

/// A checked `plugin.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub homepage: String,
    pub permissions: Vec<Permission>,
    /// Servers it wants to talk to (exact host names, HTTPS only).
    pub hosts: Vec<String>,
}

/// Ids are used as folder and file names: small letters, digits, `-` and `_`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A plain DNS name: no addresses, no ports, no wildcards.
pub fn valid_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    host.len() <= 253
        && labels.len() >= 2
        && !host.parse::<std::net::IpAddr>().is_ok()
        && labels.iter().all(|l| {
            !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        && labels.last().is_some_and(|tld| tld.chars().any(|c| c.is_ascii_lowercase()))
}

fn clip(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect()
}

/// Read and check a `plugin.toml`.
pub fn parse(text: &str) -> Result<Manifest, String> {
    if text.len() > 16 * 1024 {
        return Err("plugin.toml is too big".into());
    }
    let raw: Raw = toml::from_str(text).map_err(|e| format!("plugin.toml: {}", e.message()))?;
    if !valid_id(&raw.id) {
        return Err("id: use 1–40 small letters, digits, - and _".into());
    }
    if raw.api != API_VERSION {
        return Err(format!("this plugin is for interface version {}, Oxytide speaks version {API_VERSION}", raw.api));
    }
    if raw.name.trim().is_empty() {
        return Err("name is empty".into());
    }
    let mut permissions = Vec::new();
    for id in &raw.permissions {
        let p = Permission::parse(id).ok_or_else(|| format!("unknown permission “{id}”"))?;
        if !permissions.contains(&p) {
            permissions.push(p);
        }
    }
    permissions.sort();
    let hosts: Vec<String> = raw.network.map(|n| n.hosts).unwrap_or_default().into_iter().map(|h| h.trim().to_lowercase()).collect();
    for h in &hosts {
        if !valid_host(h) {
            return Err(format!("network host “{h}” is not a plain server name"));
        }
    }
    if hosts.len() > 8 {
        return Err("at most 8 network hosts".into());
    }
    if !hosts.is_empty() && !permissions.contains(&Permission::Network) {
        return Err("hosts are listed but the permission “network” is not".into());
    }
    if permissions.contains(&Permission::Network) && hosts.is_empty() {
        return Err("the permission “network” needs at least one host under [network]".into());
    }
    Ok(Manifest {
        id: raw.id,
        name: clip(raw.name.trim(), 60),
        version: clip(raw.version.trim(), 20),
        author: clip(raw.author.trim(), 60),
        description: clip(raw.description.trim(), 300),
        homepage: clip(raw.homepage.trim(), 200),
        permissions,
        hosts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
id = "hello"
name = "Hello"
version = "1.0"
api = 1
permissions = ["player.read", "ui.toast", "network"]
[network]
hosts = ["api.example.com"]
"#;

    #[test]
    fn reads_a_good_manifest() {
        let m = parse(GOOD).unwrap();
        assert_eq!(m.id, "hello");
        assert_eq!(m.hosts, ["api.example.com"]);
        assert!(m.permissions.contains(&Permission::Network));
        assert!(m.permissions.contains(&Permission::PlayerRead));
    }

    #[test]
    fn refuses_bad_ids_versions_and_unknown_permissions() {
        assert!(parse(&GOOD.replace("id = \"hello\"", "id = \"../x\"")).is_err());
        assert!(parse(&GOOD.replace("id = \"hello\"", "id = \"Hello\"")).is_err());
        assert!(parse(&GOOD.replace("api = 1", "api = 2")).is_err());
        assert!(parse(&GOOD.replace("\"ui.toast\"", "\"root\"")).is_err());
        assert!(parse("garbage").is_err());
    }

    #[test]
    fn network_hosts_must_be_plain_names_and_match_the_permission() {
        for bad in ["127.0.0.1", "localhost", "*.example.com", "example.com:8080", "http://example.com", "a b.com", "192.168.0.1"] {
            assert!(parse(&GOOD.replace("api.example.com", bad)).is_err(), "{bad}");
        }
        assert!(parse(&GOOD.replace(", \"network\"", "")).is_err(), "hosts without the permission");
        assert!(parse(&GOOD.replace("hosts = [\"api.example.com\"]", "hosts = []")).is_err(), "permission without hosts");
        assert!(valid_host("api.example.com"));
        assert!(!valid_host("example"));
    }

    #[test]
    fn every_permission_round_trips_its_name() {
        for p in Permission::ALL {
            assert_eq!(Permission::parse(p.id()), Some(p));
            assert!(!p.description().is_empty());
        }
    }
}
