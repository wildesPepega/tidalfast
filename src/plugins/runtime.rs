//! One plugin is one worker thread that runs its Rhai script in a sandbox.
//!
//! The script can only do what this file hands it: there is no file, process, environment or
//! network access of its own, `import` and `eval` are off, and every run has a limit on work and on
//! time. Each function checks the plugin's permission when it is called. The interface thread never
//! waits for a plugin (except for the short, bounded questions of the hooks).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhai::{CallFnOptions, Dynamic, Engine, EvalAltResult, ImmutableString, Map, Module, Position, Scope, AST};
use serde_json::Value;

use super::manifest::Permission;
use super::registry::{self, Button, Category, Entry, Kind, MenuItem, Place, Registry};

// ---------------------------------------------------------------------------- messages

/// From the interface to a plugin.
pub enum ToPlugin {
    Track(Value),
    Playing(bool),
    Action { id: String, data: Value },
    Setting { id: String, value: Value },
    /// A question that can be answered with “no” (the plugin has `hook_<name>`).
    Hook { name: String, data: Value, reply: Sender<bool> },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCmd {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Seek(f64),
    Volume(f32),
}

/// From a plugin to the interface.
#[derive(Debug)]
pub enum FromPlugin {
    /// The script ran; these are the hooks it answers.
    Ready { hooks: Vec<String> },
    Registry(Registry),
    Toast(String),
    Player(PlayerCmd),
    Clipboard(String),
    OpenUrl(String),
    Log(String),
    Error(String),
    /// The worker has ended (`reason` when it was not asked to).
    Stopped(Option<String>),
}

/// What plugins can read about the player (the interface keeps it up to date).
#[derive(Default)]
pub struct Shared {
    pub track: Option<Value>,
    pub position: f64,
    pub playing: bool,
    pub volume: f32,
}

/// What a worker needs to know about its plugin.
#[derive(Clone)]
pub struct Host {
    pub id: String,
    pub caps: HashSet<Permission>,
    pub hosts: Vec<String>,
    pub data_dir: PathBuf,
    pub shared: Arc<Mutex<Shared>>,
    pub out: Sender<(String, FromPlugin)>,
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

// ---------------------------------------------------------------------------- limits

const MAX_OPERATIONS: u64 = 1_000_000;
const INIT_BUDGET: Duration = Duration::from_millis(1500);
const CALL_BUDGET: Duration = Duration::from_millis(400);
const HOOK_BUDGET: Duration = Duration::from_millis(100);
const MAX_DATA_BYTES: usize = 512 * 1024;
const MAX_VOLATILE: usize = 256;
const HTTP_PER_MINUTE: usize = 30;
const HTTP_MAX_BODY: u64 = 1024 * 1024;
const TEXT_MAX: usize = 500;
const ERRORS_PER_MINUTE: usize = 20;
const HOOKS: [&str; 3] = ["hook_next", "hook_previous", "hook_play"];

// ---------------------------------------------------------------------------- state of a worker

#[derive(Default)]
struct State {
    registry: Registry,
    dirty: bool,
    data: serde_json::Map<String, Value>,
    data_dirty: bool,
    last_save: Option<Instant>,
    volatile: HashMap<String, Dynamic>,
    http_times: VecDeque<Instant>,
    /// Per handler call, so a script cannot flood the interface.
    toasts: u32,
    player_cmds: u32,
    one_shot: u32,
    errors: VecDeque<Instant>,
}

#[derive(Clone)]
struct Ctx {
    host: Rc<Host>,
    st: Rc<RefCell<State>>,
}

type R<T> = Result<T, Box<EvalAltResult>>;

fn fail(msg: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(msg.into()), Position::NONE).into()
}

impl Ctx {
    fn send(&self, msg: FromPlugin) {
        let _ = self.host.out.send((self.host.id.clone(), msg));
        (self.host.wake)();
    }

    fn need(&self, p: Permission) -> R<()> {
        if self.host.caps.contains(&p) {
            Ok(())
        } else {
            Err(fail(format!("permission denied: this plugin was not given “{}”", p.id())))
        }
    }

    fn data_path(&self) -> PathBuf {
        self.host.data_dir.join(format!("{}.json", self.host.id))
    }

    fn save(&self, force: bool) {
        let mut st = self.st.borrow_mut();
        if !st.data_dirty || (!force && st.last_save.is_some_and(|t| t.elapsed() < Duration::from_secs(1))) {
            return;
        }
        st.data_dirty = false;
        st.last_save = Some(Instant::now());
        let text = Value::Object(st.data.clone()).to_string();
        let path = self.data_path();
        let _ = std::fs::create_dir_all(&self.host.data_dir);
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn flush(&self) {
        let snapshot = {
            let mut st = self.st.borrow_mut();
            let d = st.dirty;
            st.dirty = false;
            d.then(|| st.registry.clone())
        };
        if let Some(r) = snapshot {
            self.send(FromPlugin::Registry(r));
        }
        self.save(false);
    }
}

fn clean(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect::<String>().trim().to_string()
}

fn valid_item_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

fn item_id(id: &ImmutableString) -> R<String> {
    if valid_item_id(id) {
        Ok(id.to_string())
    } else {
        Err(fail(format!("“{id}” is not a valid id: use 1–40 small letters, digits, - _ .")))
    }
}

fn to_json(d: &Dynamic) -> R<Value> {
    rhai::serde::from_dynamic::<Value>(d).map_err(|e| fail(format!("value cannot be stored: {e}")))
}

fn from_json(v: &Value) -> Dynamic {
    rhai::serde::to_dynamic(v).unwrap_or(Dynamic::UNIT)
}

fn map_str(m: &Map, key: &str) -> Option<String> {
    m.get(key).and_then(|d| d.clone().into_string().ok())
}

// ---------------------------------------------------------------------------- network

/// Is this address on the public internet (and not this computer or the home network)?
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (64..128).contains(&o[1])))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s0 = v6.segments()[0];
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (s0 & 0xfe00) == 0xfc00 || (s0 & 0xffc0) == 0xfe80)
        }
    }
}

/// Only `https://host/…` on the plugin's approved hosts, without credentials or odd ports.
pub fn check_url(url: &str, hosts: &[String]) -> Result<reqwest::Url, String> {
    let u = reqwest::Url::parse(url).map_err(|e| format!("not a web address: {e}"))?;
    if u.scheme() != "https" {
        return Err("only https:// addresses are allowed".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("addresses with a user name or password are not allowed".into());
    }
    if u.port().is_some_and(|p| p != 443) {
        return Err("only the standard port is allowed".into());
    }
    let host = u.host_str().ok_or("no server name")?.to_lowercase();
    if !hosts.iter().any(|h| *h == host) {
        return Err(format!("{host} is not one of the servers this plugin may talk to"));
    }
    Ok(u)
}

fn http_get(url: &str, hosts: &[String]) -> Result<(u16, String), String> {
    let mut url = check_url(url, hosts)?;
    for _ in 0..4 {
        let host = url.host_str().ok_or("no server name")?.to_lowercase();
        let addrs: Vec<SocketAddr> = (host.as_str(), 443).to_socket_addrs().map_err(|e| format!("cannot find {host}: {e}"))?.collect();
        if addrs.is_empty() || addrs.iter().any(|a| !is_public_ip(a.ip())) {
            return Err(format!("{host} does not point to a public server"));
        }
        // the addresses just checked are the ones used (no second lookup that could answer differently)
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&host, &addrs)
            .user_agent(concat!("tidalfast-plugin/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(url.clone()).send().map_err(|e| e.to_string())?;
        if resp.status().is_redirection() {
            let loc = resp.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).ok_or("redirect without a target")?;
            let next = url.join(loc).map_err(|e| e.to_string())?;
            url = check_url(next.as_str(), hosts)?;
            continue;
        }
        let status = resp.status().as_u16();
        let mut body = Vec::new();
        resp.take(HTTP_MAX_BODY).read_to_end(&mut body).map_err(|e| e.to_string())?;
        return Ok((status, String::from_utf8_lossy(&body).into_owned()));
    }
    Err("too many redirects".into())
}

// ---------------------------------------------------------------------------- the interface offered to scripts

fn add_entry(ctx: &Ctx, cat: &ImmutableString, entry: Entry) -> R<()> {
    ctx.need(Permission::UiSettings)?;
    let mut st = ctx.st.borrow_mut();
    let taken = |st: &State, id: &str| {
        st.registry.categories.iter().flat_map(|c| &c.entries).chain(st.registry.extra.iter().map(|(_, e)| e)).any(|e| e.id == id)
    };
    if !entry.id.is_empty() && taken(&st, &entry.id) {
        return Err(fail(format!("the id “{}” is already used", entry.id)));
    }
    let count = st.registry.categories.iter().map(|c| c.entries.len()).sum::<usize>() + st.registry.extra.len();
    if count >= registry::MAX_ENTRIES {
        return Err(fail("too many settings entries"));
    }
    if let Some(c) = st.registry.categories.iter_mut().find(|c| c.id == cat.as_str()) {
        c.entries.push(entry);
    } else if registry::BUILTIN_PAGES.contains(&cat.as_str()) {
        st.registry.extra.push((cat.to_string(), entry));
    } else {
        return Err(fail(format!("there is no settings page “{cat}” (create it first with settings::category)")));
    }
    st.dirty = true;
    Ok(())
}

fn stored(ctx: &Ctx, id: &str) -> Option<Value> {
    ctx.st.borrow().data.get(&format!("s.{id}")).cloned()
}

fn labelled(id: &ImmutableString, label: &ImmutableString, help: &ImmutableString, kind: Kind) -> R<Entry> {
    Ok(Entry { id: item_id(id)?, label: clean(label, 80), help: clean(help, 200), kind })
}

fn build_engine(ctx: &Ctx, deadline: Arc<AtomicU64>, started: Instant) -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(MAX_OPERATIONS);
    e.set_max_call_levels(24);
    e.set_max_expr_depths(64, 32);
    e.set_max_string_size(100_000);
    e.set_max_array_size(5_000);
    e.set_max_map_size(2_000);
    e.set_max_variables(300);
    // no files: `import` finds nothing; no running of text as code
    e.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());
    e.disable_symbol("eval");
    e.on_progress(move |_| if started.elapsed().as_millis() as u64 > deadline.load(Ordering::Relaxed) { Some(Dynamic::UNIT) } else { None });
    {
        let c = ctx.clone();
        e.on_print(move |s| c.send(FromPlugin::Log(clean(s, TEXT_MAX))));
        let c = ctx.clone();
        e.on_debug(move |s, _, _| c.send(FromPlugin::Log(clean(s, TEXT_MAX))));
    }

    // ---- app
    let mut m = Module::new();
    m.set_native_fn("version", || Ok(env!("CARGO_PKG_VERSION").to_string()));
    m.set_native_fn("api_version", || Ok(super::manifest::API_VERSION as i64));
    {
        let c = ctx.clone();
        m.set_native_fn("hide", move |feature: ImmutableString| -> R<()> {
            c.need(Permission::UiModify)?;
            if !registry::is_feature(&feature) {
                return Err(fail(format!("unknown part “{feature}” (see app::features())")));
            }
            let mut st = c.st.borrow_mut();
            if !st.registry.hidden.iter().any(|h| h == feature.as_str()) {
                st.registry.hidden.push(feature.to_string());
                st.dirty = true;
            }
            Ok(())
        });
        let c = ctx.clone();
        m.set_native_fn("show", move |feature: ImmutableString| -> R<()> {
            c.need(Permission::UiModify)?;
            let mut st = c.st.borrow_mut();
            st.registry.hidden.retain(|h| h != feature.as_str());
            st.dirty = true;
            Ok(())
        });
        m.set_native_fn("features", || Ok(registry::FEATURES.iter().map(|(id, _)| Dynamic::from(id.to_string())).collect::<Vec<_>>()));
    }
    e.register_static_module("app", m.into());

    // ---- player
    let mut m = Module::new();
    {
        let c = ctx.clone();
        m.set_native_fn("current", move || -> R<Dynamic> {
            c.need(Permission::PlayerRead)?;
            let track = c.host.shared.lock().map(|s| s.track.clone()).unwrap_or(None);
            Ok(track.map(|t| from_json(&t)).unwrap_or(Dynamic::UNIT))
        });
        let c = ctx.clone();
        m.set_native_fn("position", move || -> R<f64> {
            c.need(Permission::PlayerRead)?;
            Ok(c.host.shared.lock().map(|s| s.position).unwrap_or(0.0))
        });
        let c = ctx.clone();
        m.set_native_fn("is_playing", move || -> R<bool> {
            c.need(Permission::PlayerRead)?;
            Ok(c.host.shared.lock().map(|s| s.playing).unwrap_or(false))
        });
        let c = ctx.clone();
        m.set_native_fn("volume", move || -> R<f64> {
            c.need(Permission::PlayerRead)?;
            Ok(c.host.shared.lock().map(|s| s.volume as f64).unwrap_or(0.0))
        });
        let command = |c: &Ctx, cmd: PlayerCmd| -> R<()> {
            c.need(Permission::PlayerControl)?;
            {
                let mut st = c.st.borrow_mut();
                st.player_cmds += 1;
                if st.player_cmds > 5 {
                    return Err(fail("too many player commands at once"));
                }
            }
            c.send(FromPlugin::Player(cmd));
            Ok(())
        };
        for (name, cmd) in [("play", PlayerCmd::Play), ("pause", PlayerCmd::Pause), ("toggle", PlayerCmd::Toggle), ("next", PlayerCmd::Next), ("previous", PlayerCmd::Previous)] {
            let c = ctx.clone();
            m.set_native_fn(name, move || command(&c, cmd.clone()));
        }
        let c = ctx.clone();
        m.set_native_fn("seek", move |secs: f64| command(&c, PlayerCmd::Seek(secs.clamp(0.0, 86_400.0))));
        let c = ctx.clone();
        m.set_native_fn("set_volume", move |v: f64| command(&c, PlayerCmd::Volume(v.clamp(0.0, 1.0) as f32)));
    }
    e.register_static_module("player", m.into());

    // ---- ui
    let mut m = Module::new();
    {
        let c = ctx.clone();
        m.set_native_fn("toast", move |text: ImmutableString| -> R<()> {
            c.need(Permission::UiToast)?;
            {
                let mut st = c.st.borrow_mut();
                st.toasts += 1;
                if st.toasts > 3 {
                    return Err(fail("too many notifications at once"));
                }
            }
            let t = clean(&text, 200);
            if !t.is_empty() {
                c.send(FromPlugin::Toast(t));
            }
            Ok(())
        });
        let c = ctx.clone();
        m.set_native_fn("button", move |def: Map| -> R<()> {
            c.need(Permission::UiButtons)?;
            let id = item_id(&ImmutableString::from(map_str(&def, "id").unwrap_or_default()))?;
            let place = Place::parse(&map_str(&def, "place").unwrap_or_else(|| "header".into())).ok_or_else(|| fail("place must be “header”, “player” or “now”"))?;
            let button = Button {
                id: id.clone(),
                label: clean(&map_str(&def, "label").unwrap_or_default(), 40),
                icon: map_str(&def, "icon").unwrap_or_default(),
                tooltip: clean(&map_str(&def, "tooltip").unwrap_or_default(), 120),
                place,
            };
            let mut st = c.st.borrow_mut();
            st.registry.buttons.retain(|b| b.id != id);
            if st.registry.buttons.len() >= registry::MAX_BUTTONS {
                return Err(fail("too many buttons"));
            }
            st.registry.buttons.push(button);
            st.dirty = true;
            Ok(())
        });
        let c = ctx.clone();
        m.set_native_fn("menu_item", move |def: Map| -> R<()> {
            c.need(Permission::UiMenus)?;
            let id = item_id(&ImmutableString::from(map_str(&def, "id").unwrap_or_default()))?;
            let label = clean(&map_str(&def, "label").unwrap_or_default(), 60);
            if label.is_empty() {
                return Err(fail("a menu entry needs a label"));
            }
            let item = MenuItem { id: id.clone(), label, icon: map_str(&def, "icon").unwrap_or_default() };
            let mut st = c.st.borrow_mut();
            st.registry.menu_items.retain(|b| b.id != id);
            if st.registry.menu_items.len() >= registry::MAX_MENU_ITEMS {
                return Err(fail("too many menu entries"));
            }
            st.registry.menu_items.push(item);
            st.dirty = true;
            Ok(())
        });
    }
    e.register_static_module("ui", m.into());

    // ---- settings
    let mut m = Module::new();
    {
        let c = ctx.clone();
        m.set_native_fn("category", move |id: ImmutableString, title: ImmutableString, icon: ImmutableString| -> R<()> {
            c.need(Permission::UiSettings)?;
            let id = item_id(&id)?;
            let mut st = c.st.borrow_mut();
            if registry::BUILTIN_PAGES.contains(&id.as_str()) || st.registry.categories.iter().any(|x| x.id == id) {
                return Err(fail(format!("the settings page “{id}” exists already")));
            }
            if st.registry.categories.len() >= registry::MAX_CATEGORIES {
                return Err(fail("too many settings pages"));
            }
            st.registry.categories.push(Category { id, title: clean(&title, 30), icon: icon.to_string(), entries: Vec::new() });
            st.dirty = true;
            Ok(())
        });
        let c = ctx.clone();
        m.set_native_fn("heading", move |cat: ImmutableString, text: ImmutableString| -> R<()> {
            add_entry(&c, &cat, Entry { id: String::new(), label: clean(&text, 80), help: String::new(), kind: Kind::Heading })
        });
        let c = ctx.clone();
        m.set_native_fn("label", move |cat: ImmutableString, id: ImmutableString, text: ImmutableString| -> R<()> {
            add_entry(&c, &cat, Entry { id: item_id(&id)?, label: clean(&text, 200), help: String::new(), kind: Kind::Label })
        });
        let c = ctx.clone();
        m.set_native_fn("button", move |cat: ImmutableString, id: ImmutableString, label: ImmutableString, help: ImmutableString| -> R<()> {
            add_entry(&c, &cat, labelled(&id, &label, &help, Kind::Button)?)
        });
        let c = ctx.clone();
        m.set_native_fn("toggle", move |cat: ImmutableString, id: ImmutableString, label: ImmutableString, help: ImmutableString, default: bool| -> R<bool> {
            let value = stored(&c, &id).and_then(|v| v.as_bool()).unwrap_or(default);
            add_entry(&c, &cat, labelled(&id, &label, &help, Kind::Toggle(value))?)?;
            Ok(value)
        });
        let c = ctx.clone();
        m.set_native_fn("text", move |cat: ImmutableString, id: ImmutableString, label: ImmutableString, help: ImmutableString, default: ImmutableString| -> R<String> {
            let value = stored(&c, &id).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| clean(&default, TEXT_MAX));
            add_entry(&c, &cat, labelled(&id, &label, &help, Kind::Text(value.clone()))?)?;
            Ok(value)
        });
        let c = ctx.clone();
        m.set_native_fn(
            "number",
            move |cat: ImmutableString, id: ImmutableString, label: ImmutableString, help: ImmutableString, default: f64, min: f64, max: f64, step: f64| -> R<f64> {
                if !(min < max) || !(step > 0.0) {
                    return Err(fail("number: min must be less than max and step above 0"));
                }
                let value = stored(&c, &id).and_then(|v| v.as_f64()).unwrap_or(default).clamp(min, max);
                add_entry(&c, &cat, labelled(&id, &label, &help, Kind::Number { value, min, max, step })?)?;
                Ok(value)
            },
        );
        let c = ctx.clone();
        m.set_native_fn(
            "choice",
            move |cat: ImmutableString, id: ImmutableString, label: ImmutableString, help: ImmutableString, options: rhai::Array, default: ImmutableString| -> R<String> {
                let options: Vec<String> = options.into_iter().filter_map(|d| d.into_string().ok()).map(|s| clean(&s, 60)).filter(|s| !s.is_empty()).take(30).collect();
                if options.is_empty() {
                    return Err(fail("choice: the list of options is empty"));
                }
                let wanted = stored(&c, &id).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| default.to_string());
                let value = if options.contains(&wanted) { wanted } else { options[0].clone() };
                add_entry(&c, &cat, labelled(&id, &label, &help, Kind::Choice { options, value: value.clone() })?)?;
                Ok(value)
            },
        );
        let c = ctx.clone();
        m.set_native_fn("set_label", move |id: ImmutableString, text: ImmutableString| -> R<()> {
            c.need(Permission::UiSettings)?;
            let mut st = c.st.borrow_mut();
            let st = &mut *st;
            let text = clean(&text, 200);
            let found = st.registry.categories.iter_mut().flat_map(|c| c.entries.iter_mut()).chain(st.registry.extra.iter_mut().map(|(_, e)| e)).find(|e| e.id == id.as_str());
            match found {
                Some(entry) => {
                    entry.label = text;
                    st.dirty = true;
                    Ok(())
                }
                None => Err(fail(format!("no settings entry “{id}”"))),
            }
        });
        let c = ctx.clone();
        m.set_native_fn("get", move |id: ImmutableString| -> R<Dynamic> {
            c.need(Permission::UiSettings)?;
            let st = c.st.borrow();
            let found = st.registry.categories.iter().flat_map(|c| &c.entries).chain(st.registry.extra.iter().map(|(_, e)| e)).find(|e| e.id == id.as_str());
            Ok(match found.map(|e| &e.kind) {
                Some(Kind::Toggle(v)) => Dynamic::from(*v),
                Some(Kind::Text(v)) => Dynamic::from(v.clone()),
                Some(Kind::Number { value, .. }) => Dynamic::from(*value),
                Some(Kind::Choice { value, .. }) => Dynamic::from(value.clone()),
                _ => Dynamic::UNIT,
            })
        });
    }
    e.register_static_module("settings", m.into());

    // ---- storage (kept on disk) and state (kept while tidalfast runs)
    let mut m = Module::new();
    {
        let c = ctx.clone();
        m.set_native_fn("get", move |key: ImmutableString| -> R<Dynamic> {
            c.need(Permission::Storage)?;
            Ok(c.st.borrow().data.get(&format!("d.{key}")).map(from_json).unwrap_or(Dynamic::UNIT))
        });
        let c = ctx.clone();
        m.set_native_fn("set", move |key: ImmutableString, value: Dynamic| -> R<()> {
            c.need(Permission::Storage)?;
            if key.is_empty() || key.len() > 64 {
                return Err(fail("a key has 1–64 characters"));
            }
            let json = to_json(&value)?;
            let mut st = c.st.borrow_mut();
            let before = st.data.insert(format!("d.{key}"), json);
            let size = Value::Object(st.data.clone()).to_string().len();
            if size > MAX_DATA_BYTES {
                match before {
                    Some(old) => st.data.insert(format!("d.{key}"), old),
                    None => st.data.remove(&format!("d.{key}")),
                };
                return Err(fail("the plugin's storage is full (512 KB)"));
            }
            st.data_dirty = true;
            Ok(())
        });
        let c = ctx.clone();
        m.set_native_fn("remove", move |key: ImmutableString| -> R<()> {
            c.need(Permission::Storage)?;
            let mut st = c.st.borrow_mut();
            if st.data.remove(&format!("d.{key}")).is_some() {
                st.data_dirty = true;
            }
            Ok(())
        });
        let c = ctx.clone();
        m.set_native_fn("keys", move || -> R<Vec<Dynamic>> {
            c.need(Permission::Storage)?;
            Ok(c.st.borrow().data.keys().filter_map(|k| k.strip_prefix("d.")).map(|k| Dynamic::from(k.to_string())).collect())
        });
    }
    e.register_static_module("storage", m.into());

    let mut m = Module::new();
    {
        let c = ctx.clone();
        m.set_native_fn("get", move |key: ImmutableString| -> R<Dynamic> { Ok(c.st.borrow().volatile.get(key.as_str()).cloned().unwrap_or(Dynamic::UNIT)) });
        let c = ctx.clone();
        m.set_native_fn("set", move |key: ImmutableString, value: Dynamic| -> R<()> {
            let mut st = c.st.borrow_mut();
            if !st.volatile.contains_key(key.as_str()) && st.volatile.len() >= MAX_VOLATILE {
                return Err(fail("too many values in state"));
            }
            st.volatile.insert(key.to_string(), value);
            Ok(())
        });
    }
    e.register_static_module("state", m.into());

    // ---- network, clipboard, links
    let mut m = Module::new();
    {
        let c = ctx.clone();
        let get = move |url: &str| -> R<(u16, String)> {
            c.need(Permission::Network)?;
            {
                let mut st = c.st.borrow_mut();
                let now = Instant::now();
                while st.http_times.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
                    st.http_times.pop_front();
                }
                if st.http_times.len() >= HTTP_PER_MINUTE {
                    return Err(fail("too many requests (30 per minute)"));
                }
                st.http_times.push_back(now);
            }
            http_get(url, &c.host.hosts).map_err(fail)
        };
        let g = get.clone();
        m.set_native_fn("get", move |url: ImmutableString| -> R<Map> {
            let (status, body) = g(&url)?;
            let mut out = Map::new();
            out.insert("status".into(), Dynamic::from(status as i64));
            out.insert("ok".into(), Dynamic::from((200..300).contains(&status)));
            out.insert("body".into(), Dynamic::from(body));
            Ok(out)
        });
        m.set_native_fn("get_json", move |url: ImmutableString| -> R<Dynamic> {
            let (status, body) = get(&url)?;
            if !(200..300).contains(&status) {
                return Err(fail(format!("the server answered {status}")));
            }
            let v: Value = serde_json::from_str(&body).map_err(|e| fail(format!("not JSON: {e}")))?;
            Ok(from_json(&v))
        });
    }
    e.register_static_module("http", m.into());

    let mut m = Module::new();
    {
        let c = ctx.clone();
        m.set_native_fn("copy", move |text: ImmutableString| -> R<()> {
            c.need(Permission::Clipboard)?;
            if c.st.borrow().one_shot >= 1 {
                return Err(fail("only one clipboard or link action per call"));
            }
            c.st.borrow_mut().one_shot += 1;
            c.send(FromPlugin::Clipboard(clean(&text, 2000)));
            Ok(())
        });
        let c = ctx.clone();
        m.set_native_fn("open_url", move |url: ImmutableString| -> R<()> {
            c.need(Permission::OpenUrl)?;
            let u = reqwest::Url::parse(&url).map_err(|_| fail("not a web address"))?;
            if u.scheme() != "https" || u.host_str().is_none() || !u.username().is_empty() {
                return Err(fail("only https:// links can be opened"));
            }
            if c.st.borrow().one_shot >= 1 {
                return Err(fail("only one clipboard or link action per call"));
            }
            c.st.borrow_mut().one_shot += 1;
            c.send(FromPlugin::OpenUrl(u.to_string()));
            Ok(())
        });
    }
    e.register_static_module("system", m.into());

    // ---- small helpers
    let mut m = Module::new();
    m.set_native_fn("parse_json", |text: ImmutableString| -> R<Dynamic> {
        let v: Value = serde_json::from_str(&text).map_err(|e| fail(format!("not JSON: {e}")))?;
        Ok(from_json(&v))
    });
    m.set_native_fn("to_json", |value: Dynamic| -> R<String> { Ok(to_json(&value)?.to_string()) });
    m.set_native_fn("now", || -> R<f64> {
        Ok(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0))
    });
    e.register_static_module("util", m.into());
    e
}

// ---------------------------------------------------------------------------- the worker

struct Script {
    engine: Engine,
    ast: AST,
    scope: Scope<'static>,
    deadline: Arc<AtomicU64>,
    started: Instant,
}

impl Script {
    fn has(&self, name: &str) -> bool {
        self.ast.iter_functions().any(|f| f.name == name)
    }

    fn call(&mut self, ctx: &Ctx, name: &str, args: Vec<Dynamic>, budget: Duration) -> Result<Dynamic, String> {
        {
            let mut st = ctx.st.borrow_mut();
            st.toasts = 0;
            st.player_cmds = 0;
            st.one_shot = 0;
        }
        self.deadline.store((self.started.elapsed() + budget).as_millis() as u64, Ordering::Relaxed);
        let options = CallFnOptions::new().eval_ast(false).rewind_scope(true);
        self.engine.call_fn_with_options::<Dynamic>(options, &mut self.scope, &self.ast, name, args).map_err(|e| describe(&e))
    }
}

fn describe(e: &EvalAltResult) -> String {
    match e {
        EvalAltResult::ErrorTerminated(..) => "stopped: it took too long".into(),
        EvalAltResult::ErrorTooManyOperations(..) => "stopped: it did too much work in one go".into(),
        EvalAltResult::ErrorRuntime(v, _) => v.to_string(),
        other => other.to_string(),
    }
}

/// Note an error; true when there have been so many that the plugin should be stopped.
fn note_error(ctx: &Ctx, what: &str, err: &str) -> bool {
    ctx.send(FromPlugin::Error(format!("{what}: {}", clean(err, 300))));
    let mut st = ctx.st.borrow_mut();
    let now = Instant::now();
    st.errors.push_back(now);
    while st.errors.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
        st.errors.pop_front();
    }
    st.errors.len() > ERRORS_PER_MINUTE
}

fn load_data(ctx: &Ctx) {
    let path = ctx.data_path();
    let text = std::fs::metadata(&path).ok().filter(|m| m.len() as usize <= MAX_DATA_BYTES).and_then(|_| std::fs::read_to_string(&path).ok());
    if let Some(Value::Object(map)) = text.and_then(|t| serde_json::from_str(&t).ok()) {
        ctx.st.borrow_mut().data = map;
    }
}

/// Start a plugin's worker; the returned sender is how the interface talks to it.
pub fn spawn(host: Host, script: String) -> Sender<ToPlugin> {
    let (tx, rx) = channel();
    let name = format!("plugin-{}", host.id);
    let _ = std::thread::Builder::new().name(name).spawn(move || {
        let out = host.out.clone();
        let wake = host.wake.clone();
        let id = host.id.clone();
        // a plugin that panics must not take the app with it
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(host, script, rx)));
        let reason = match result {
            Ok(reason) => reason,
            Err(_) => Some("the plugin crashed".to_string()),
        };
        let _ = out.send((id, FromPlugin::Stopped(reason)));
        wake();
    });
    tx
}

fn run(host: Host, source: String, rx: Receiver<ToPlugin>) -> Option<String> {
    let ctx = Ctx { host: Rc::new(host), st: Rc::new(RefCell::new(State::default())) };
    load_data(&ctx);
    let started = Instant::now();
    let deadline = Arc::new(AtomicU64::new(INIT_BUDGET.as_millis() as u64));
    let engine = build_engine(&ctx, deadline.clone(), started);
    let ast = match engine.compile(&source) {
        Ok(ast) => ast,
        Err(e) => return Some(format!("the script has an error: {e}")),
    };
    let mut script = Script { engine, ast, scope: Scope::new(), deadline, started };
    if let Err(e) = script.engine.run_ast_with_scope(&mut script.scope, &script.ast) {
        return Some(format!("the script failed to start: {}", describe(&e)));
    }
    let hooks: Vec<String> = HOOKS.iter().filter(|h| script.has(h)).map(|h| h.trim_start_matches("hook_").to_string()).collect();
    ctx.flush();
    ctx.send(FromPlugin::Ready { hooks });
    if script.has("on_load") {
        if let Err(e) = script.call(&ctx, "on_load", vec![], CALL_BUDGET) {
            note_error(&ctx, "on_load", &e);
        }
        ctx.flush();
    }

    let mut last_tick = Instant::now();
    loop {
        let message = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut too_many = false;
        match message {
            Some(ToPlugin::Shutdown) => break,
            Some(ToPlugin::Track(t)) => {
                if script.has("on_track") {
                    if let Err(e) = script.call(&ctx, "on_track", vec![from_json(&t)], CALL_BUDGET) {
                        too_many = note_error(&ctx, "on_track", &e);
                    }
                }
            }
            Some(ToPlugin::Playing(p)) => {
                if script.has("on_playing") {
                    if let Err(e) = script.call(&ctx, "on_playing", vec![Dynamic::from(p)], CALL_BUDGET) {
                        too_many = note_error(&ctx, "on_playing", &e);
                    }
                }
            }
            Some(ToPlugin::Action { id, data }) => {
                if script.has("on_action") {
                    if let Err(e) = script.call(&ctx, "on_action", vec![Dynamic::from(id), from_json(&data)], CALL_BUDGET) {
                        too_many = note_error(&ctx, "on_action", &e);
                    }
                }
            }
            Some(ToPlugin::Setting { id, value }) => {
                // the plugin hears the value that was accepted (clamped, cleaned), not the raw one
                if let Some(accepted) = apply_setting(&ctx, &id, &value) {
                    if script.has("on_setting") {
                        if let Err(e) = script.call(&ctx, "on_setting", vec![Dynamic::from(id), from_json(&accepted)], CALL_BUDGET) {
                            too_many = note_error(&ctx, "on_setting", &e);
                        }
                    }
                }
            }
            Some(ToPlugin::Hook { name, data, reply }) => {
                let function = format!("hook_{name}");
                let allow = if ctx.host.caps.contains(&Permission::Hooks) && script.has(&function) {
                    let args = if name == "play" { vec![from_json(&data)] } else { vec![] };
                    match script.call(&ctx, &function, args, HOOK_BUDGET) {
                        Ok(v) => v.as_bool().unwrap_or(true),
                        Err(e) => {
                            too_many = note_error(&ctx, &function, &e);
                            true
                        }
                    }
                } else {
                    true
                };
                let _ = reply.send(allow);
            }
            None => {
                if last_tick.elapsed() >= Duration::from_secs(1) {
                    last_tick = Instant::now();
                    if script.has("on_tick") {
                        if let Err(e) = script.call(&ctx, "on_tick", vec![], CALL_BUDGET) {
                            too_many = note_error(&ctx, "on_tick", &e);
                        }
                    }
                }
            }
        }
        ctx.flush();
        if too_many {
            ctx.save(true);
            return Some("stopped after too many errors".into());
        }
    }
    if script.has("on_unload") {
        let _ = script.call(&ctx, "on_unload", vec![], CALL_BUDGET);
    }
    ctx.save(true);
    None
}

/// A setting was changed by the user: check the value, keep it, show it. The accepted value, or
/// nothing if it is not valid for that entry.
fn apply_setting(ctx: &Ctx, id: &str, value: &Value) -> Option<Value> {
    let mut st = ctx.st.borrow_mut();
    let st = &mut *st;
    let found = st.registry.categories.iter_mut().flat_map(|c| c.entries.iter_mut()).chain(st.registry.extra.iter_mut().map(|(_, e)| e)).find(|e| e.id == id);
    let entry = found?;
    let ok = match (&mut entry.kind, value) {
        (Kind::Toggle(v), Value::Bool(b)) => {
            *v = *b;
            true
        }
        (Kind::Text(v), Value::String(s)) => {
            *v = clean(s, TEXT_MAX);
            true
        }
        (Kind::Number { value: v, min, max, .. }, Value::Number(n)) => match n.as_f64() {
            Some(x) => {
                *v = x.clamp(*min, *max);
                true
            }
            None => false,
        },
        (Kind::Choice { options, value: v }, Value::String(s)) if options.contains(s) => {
            *v = s.clone();
            true
        }
        (Kind::Button, _) => true,
        _ => false,
    };
    if !ok {
        return None;
    }
    let accepted = match &entry.kind {
        Kind::Toggle(v) => Some(Value::Bool(*v)),
        Kind::Text(v) => Some(Value::String(v.clone())),
        Kind::Number { value, .. } => serde_json::Number::from_f64(*value).map(Value::Number),
        Kind::Choice { value, .. } => Some(Value::String(value.clone())),
        _ => None,
    };
    if let Some(a) = &accepted {
        st.data.insert(format!("s.{id}"), a.clone());
        st.data_dirty = true;
    }
    st.dirty = true;
    // a button has no value: the plugin hears that it was pressed
    Some(accepted.unwrap_or(Value::Bool(true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct Run {
        tx: Sender<ToPlugin>,
        rx: Receiver<(String, FromPlugin)>,
        shared: Arc<Mutex<Shared>>,
        dir: PathBuf,
    }

    fn start(script: &str, caps: &[Permission], hosts: &[&str]) -> Run {
        let dir = std::env::temp_dir().join(format!("tidalfast-plugin-test-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
        let (out, rx) = channel();
        let shared = Arc::new(Mutex::new(Shared { track: Some(serde_json::json!({"title": "Song", "artist": "Band"})), position: 12.5, playing: true, volume: 0.5 }));
        let host = Host {
            id: "test".into(),
            caps: caps.iter().copied().collect(),
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            data_dir: dir.clone(),
            shared: shared.clone(),
            out,
            wake: Arc::new(|| {}),
        };
        let tx = spawn(host, script.to_string());
        Run { tx, rx, shared, dir }
    }

    /// The next message that matches, within a few seconds.
    fn wait(run: &Run, mut pick: impl FnMut(&FromPlugin) -> bool) -> Option<FromPlugin> {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Ok((_, m)) = run.rx.recv_timeout(Duration::from_millis(100)) {
                if pick(&m) {
                    return Some(m);
                }
            }
        }
        None
    }

    fn errors(run: &Run) -> String {
        match wait(run, |m| matches!(m, FromPlugin::Error(_))) {
            Some(FromPlugin::Error(e)) => e,
            _ => String::new(),
        }
    }

    #[test]
    fn a_plugin_registers_buttons_and_settings_and_answers_actions() {
        let run = start(
            r#"
            ui::button(#{ id: "hi", label: "Hi", icon: "star", place: "header" });
            settings::category("mine", "Mine", "gear");
            let on = settings::toggle("mine", "loud", "Loud", "Be loud", true);
            fn on_action(id, data) { ui::toast("clicked " + id); }
            "#,
            &[Permission::UiButtons, Permission::UiSettings, Permission::UiToast],
            &[],
        );
        let Some(FromPlugin::Registry(r)) = wait(&run, |m| matches!(m, FromPlugin::Registry(_))) else { panic!("no registry") };
        assert_eq!(r.buttons.len(), 1);
        assert_eq!(r.buttons[0].id, "hi");
        assert_eq!(r.categories[0].entries[0].kind, Kind::Toggle(true));
        run.tx.send(ToPlugin::Action { id: "hi".into(), data: Value::Null }).unwrap();
        let Some(FromPlugin::Toast(t)) = wait(&run, |m| matches!(m, FromPlugin::Toast(_))) else { panic!("no toast") };
        assert_eq!(t, "clicked hi");
        let _ = run.tx.send(ToPlugin::Shutdown);
    }

    #[test]
    fn nothing_works_without_the_permission() {
        let run = start(r#"fn on_action(id, data) { player::play(); }"#, &[], &[]);
        wait(&run, |m| matches!(m, FromPlugin::Ready { .. })).expect("ready");
        run.tx.send(ToPlugin::Action { id: "x".into(), data: Value::Null }).unwrap();
        let e = errors(&run);
        assert!(e.contains("permission denied") && e.contains("player.control"), "{e}");
        // and no command got through
        assert!(wait(&run, |m| matches!(m, FromPlugin::Player(_))).is_none());
    }

    #[test]
    fn reading_the_player_needs_player_read_and_gives_the_data() {
        let run = start(r#"fn on_action(id, data) { ui::toast(player::current().title + " " + player::position() + " " + player::is_playing()); }"#, &[Permission::PlayerRead, Permission::UiToast], &[]);
        wait(&run, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        run.shared.lock().unwrap().position = 12.5;
        run.tx.send(ToPlugin::Action { id: "x".into(), data: Value::Null }).unwrap();
        let Some(FromPlugin::Toast(t)) = wait(&run, |m| matches!(m, FromPlugin::Toast(_))) else { panic!() };
        assert_eq!(t, "Song 12.5 true");
    }

    #[test]
    fn an_endless_loop_is_stopped() {
        let run = start("loop { }", &[], &[]);
        let Some(FromPlugin::Stopped(Some(why))) = wait(&run, |m| matches!(m, FromPlugin::Stopped(_))) else { panic!("did not stop") };
        assert!(why.contains("too long") || why.contains("too much work"), "{why}");
    }

    #[test]
    fn a_handler_that_never_ends_is_cut_off_and_the_plugin_keeps_living() {
        let run = start("fn on_action(id, data) { loop { } }\nfn on_playing(p) { ui::toast(\"alive\"); }", &[Permission::UiToast], &[]);
        wait(&run, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        run.tx.send(ToPlugin::Action { id: "x".into(), data: Value::Null }).unwrap();
        assert!(!errors(&run).is_empty());
        run.tx.send(ToPlugin::Playing(true)).unwrap();
        assert!(wait(&run, |m| matches!(m, FromPlugin::Toast(t) if t == "alive")).is_some());
    }

    #[test]
    fn eval_import_and_files_are_not_available() {
        for script in [r#"eval("1 + 1");"#, r#"import "other" as o;"#, r#"let f = open("/etc/passwd");"#, r#"os::exec("ls");"#] {
            let run = start(script, &[], &[]);
            match wait(&run, |m| matches!(m, FromPlugin::Stopped(_) | FromPlugin::Ready { .. })) {
                Some(FromPlugin::Stopped(Some(_))) => {}
                other => panic!("{script}: {other:?}"),
            }
        }
    }

    #[test]
    fn settings_changed_by_the_user_are_checked_stored_and_reported() {
        let run = start(
            r#"
            settings::category("mine", "Mine", "gear");
            settings::toggle("mine", "loud", "Loud", "", false);
            settings::number("mine", "vol", "Vol", "", 5.0, 0.0, 10.0, 1.0);
            settings::choice("mine", "mode", "Mode", "", ["a", "b"], "a");
            fn on_setting(id, value) { ui::toast(id + "=" + value); }
            "#,
            &[Permission::UiSettings, Permission::UiToast],
            &[],
        );
        wait(&run, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        run.tx.send(ToPlugin::Setting { id: "loud".into(), value: Value::Bool(true) }).unwrap();
        assert!(wait(&run, |m| matches!(m, FromPlugin::Toast(t) if t == "loud=true")).is_some());
        // out of range is clamped, a wrong type or an unknown option is refused
        run.tx.send(ToPlugin::Setting { id: "vol".into(), value: serde_json::json!(99) }).unwrap();
        assert!(wait(&run, |m| matches!(m, FromPlugin::Toast(t) if t == "vol=10.0")).is_some());
        run.tx.send(ToPlugin::Setting { id: "mode".into(), value: serde_json::json!("zzz") }).unwrap();
        run.tx.send(ToPlugin::Setting { id: "loud".into(), value: serde_json::json!("yes") }).unwrap();
        run.tx.send(ToPlugin::Setting { id: "mode".into(), value: serde_json::json!("b") }).unwrap();
        assert!(wait(&run, |m| matches!(m, FromPlugin::Toast(t) if t == "mode=b")).is_some());
        let _ = run.tx.send(ToPlugin::Shutdown);
        wait(&run, |m| matches!(m, FromPlugin::Stopped(None))).unwrap();
        // and they survive a restart of the plugin
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(run.dir.join("test.json")).unwrap()).unwrap();
        assert_eq!(saved["s.loud"], Value::Bool(true));
        assert_eq!(saved["s.mode"], "b");
    }

    #[test]
    fn storage_is_kept_limited_and_needs_its_permission() {
        let run = start(
            r#"
            fn on_action(id, data) {
                if id == "set" { storage::set("k", #{ a: 1 }); ui::toast("set"); }
                if id == "get" { ui::toast("got " + storage::get("k").a); }
                if id == "big" {
                    let chunk = ""; chunk.pad(60000, 'x');
                    for i in 0..12 { storage::set("big" + i, chunk); }
                }
            }
            "#,
            &[Permission::Storage, Permission::UiToast],
            &[],
        );
        wait(&run, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        run.tx.send(ToPlugin::Action { id: "set".into(), data: Value::Null }).unwrap();
        wait(&run, |m| matches!(m, FromPlugin::Toast(_))).unwrap();
        run.tx.send(ToPlugin::Action { id: "get".into(), data: Value::Null }).unwrap();
        assert!(wait(&run, |m| matches!(m, FromPlugin::Toast(t) if t == "got 1")).is_some());
        run.tx.send(ToPlugin::Action { id: "big".into(), data: Value::Null }).unwrap();
        let e = errors(&run);
        assert!(e.contains("full"), "{e}");
        let none = start(r#"fn on_action(id, data) { storage::set("k", 1); }"#, &[], &[]);
        wait(&none, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        none.tx.send(ToPlugin::Action { id: "x".into(), data: Value::Null }).unwrap();
        assert!(errors(&none).contains("storage"));
    }

    #[test]
    fn network_only_reaches_the_approved_servers() {
        let run = start(
            r#"fn on_action(id, data) { let r = http::get(id); }"#,
            &[Permission::Network],
            &["api.example.com"],
        );
        wait(&run, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        for (url, expect) in [
            ("https://evil.example.org/x", "not one of the servers"),
            ("http://api.example.com/x", "https"),
            ("https://api.example.com:8443/x", "port"),
            ("https://user:pw@api.example.com/x", "user name"),
            ("file:///etc/passwd", "https"),
        ] {
            run.tx.send(ToPlugin::Action { id: url.into(), data: Value::Null }).unwrap();
            let e = errors(&run);
            assert!(e.contains(expect), "{url}: {e}");
        }
    }

    #[test]
    fn urls_and_addresses_are_checked() {
        let hosts = vec!["api.example.com".to_string()];
        assert!(check_url("https://api.example.com/path?q=1", &hosts).is_ok());
        assert!(check_url("https://API.example.com/", &hosts).is_ok());
        assert!(check_url("https://example.com/", &hosts).is_err());
        assert!(check_url("https://api.example.com.evil.com/", &hosts).is_err());
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "172.16.0.1", "169.254.1.1", "0.0.0.0", "100.64.0.1", "::1", "fe80::1", "fc00::1", "::ffff:127.0.0.1", "224.0.0.1"] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn hooks_can_say_no_and_only_with_the_permission() {
        let script = "fn hook_next() { false }\nfn hook_play(t) { t.title != \"blocked\" }";
        let run = start(script, &[Permission::Hooks], &[]);
        let Some(FromPlugin::Ready { hooks }) = wait(&run, |m| matches!(m, FromPlugin::Ready { .. })) else { panic!() };
        assert_eq!(hooks, ["next", "play"]);
        let ask = |name: &str, data: Value| {
            let (reply, answer) = channel();
            run.tx.send(ToPlugin::Hook { name: name.into(), data, reply }).unwrap();
            answer.recv_timeout(Duration::from_secs(2)).unwrap()
        };
        assert!(!ask("next", Value::Null));
        assert!(ask("previous", Value::Null));
        assert!(!ask("play", serde_json::json!({"title": "blocked"})));
        assert!(ask("play", serde_json::json!({"title": "fine"})));
        // without the permission the answer is always yes
        let without = start(script, &[], &[]);
        wait(&without, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        let (reply, answer) = channel();
        without.tx.send(ToPlugin::Hook { name: "next".into(), data: Value::Null, reply }).unwrap();
        assert!(answer.recv_timeout(Duration::from_secs(2)).unwrap());
    }

    #[test]
    fn hiding_parts_needs_the_permission_and_a_known_name() {
        let run = start(r#"app::hide("lyrics");"#, &[Permission::UiModify], &[]);
        let Some(FromPlugin::Registry(r)) = wait(&run, |m| matches!(m, FromPlugin::Registry(_))) else { panic!() };
        assert_eq!(r.hidden, ["lyrics"]);
        let bad = start(r#"app::hide("everything");"#, &[Permission::UiModify], &[]);
        assert!(matches!(wait(&bad, |m| matches!(m, FromPlugin::Stopped(_))), Some(FromPlugin::Stopped(Some(_)))));
        let denied = start(r#"app::hide("lyrics");"#, &[], &[]);
        assert!(matches!(wait(&denied, |m| matches!(m, FromPlugin::Stopped(_))), Some(FromPlugin::Stopped(Some(_)))));
    }

    #[test]
    fn a_script_cannot_flood_the_interface() {
        let run = start(r#"fn on_action(id, data) { for i in 0..20 { ui::toast("spam"); } }"#, &[Permission::UiToast], &[]);
        wait(&run, |m| matches!(m, FromPlugin::Ready { .. })).unwrap();
        run.tx.send(ToPlugin::Action { id: "x".into(), data: Value::Null }).unwrap();
        assert!(errors(&run).contains("too many"));
        let mut toasts = 0;
        while let Ok((_, m)) = run.rx.recv_timeout(Duration::from_millis(300)) {
            if matches!(m, FromPlugin::Toast(_)) {
                toasts += 1;
            }
        }
        assert!(toasts <= 3, "{toasts}");
    }

    #[test]
    fn bad_ids_and_texts_are_cleaned_or_refused() {
        let run = start(r#"ui::button(#{ id: "../../x", label: "a" });"#, &[Permission::UiButtons], &[]);
        assert!(matches!(wait(&run, |m| matches!(m, FromPlugin::Stopped(_))), Some(FromPlugin::Stopped(Some(_)))));
        assert_eq!(clean("a\u{0}b\u{7}c", 10), "abc");
        assert_eq!(clean("  hello  ", 3), "h");
    }
}
