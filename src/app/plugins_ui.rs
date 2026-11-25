//! Plugins and themes in the interface: running them, their buttons and menu entries, the settings
//! pages, the question “may this plugin do this?”, and the picker for themes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use serde_json::{json, Value};

use super::*;
use crate::plugins::manifest::{Permission, Risk};
use crate::plugins::registry::{self, Button, Entry, Kind, MenuItem, Place};
use crate::plugins::runtime::{FromPlugin, PlayerCmd, ToPlugin};
use crate::plugins::{self, Status, Why};
use crate::theme_file::{self, CustomTheme};

/// Everything the interface keeps for plugins and themes.
pub struct PluginUi {
    pub manager: plugins::Manager,
    pub themes: Vec<CustomTheme>,
    pub theme_problems: Vec<(String, String)>,
    /// The plugin whose “may it?” question is open.
    pub consent: Option<String>,
    /// The plugin settings page that is open (plugin id, page id).
    pub page: Option<(String, String)>,
    /// The menu entries of the song menu, refreshed every frame.
    pub menu: Vec<(String, MenuItem)>,
    /// Plugins were switched on or off, or changed: start and stop accordingly.
    pub dirty: bool,
    /// Plugins are off for this run (`--no-plugins`).
    pub off: bool,
    sent_track: Option<Option<u64>>,
    sent_playing: Option<bool>,
    last_link: HashMap<String, Instant>,
    texts: HashMap<String, String>,
}

impl PluginUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let wake = ctx.clone();
        let (themes, theme_problems) = theme_file::discover(&theme_file::themes_dir());
        Self {
            manager: plugins::Manager::new(Arc::new(move || wake.request_repaint())),
            themes,
            theme_problems,
            consent: None,
            page: None,
            menu: Vec::new(),
            dirty: true,
            off: std::env::args().any(|a| a == "--no-plugins"),
            sent_track: None,
            sent_playing: None,
            last_link: HashMap::new(),
            texts: HashMap::new(),
        }
    }
}

/// What plugins get to know about a song.
fn track_json(t: &Track) -> Value {
    json!({
        "id": t.id,
        "title": t.title,
        "artist": t.artist_name(),
        "artists": t.artist_list().iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
        "album": t.album.as_ref().map(|a| a.title.clone()).unwrap_or_default(),
        "duration": t.duration,
        "explicit": t.explicit.unwrap_or(false),
        "cover": t.cover_url(640).unwrap_or_default(),
    })
}

fn risk_color(r: Risk) -> Color32 {
    match r {
        Risk::Low => TEXT_DIM,
        Risk::Medium => ui::WARN,
        Risk::High => ui::DANGER,
    }
}

impl App {
    /// Plugins may run (switched on in the settings and not stopped by `--no-plugins`).
    pub(super) fn plugins_on(&self) -> bool {
        self.cfg.plugins_enabled && !self.pl.off
    }

    /// Has a plugin hidden this part of the interface?
    pub(super) fn plugin_hidden(&self, feature: &str) -> bool {
        self.plugins_on() && self.pl.manager.hidden(feature)
    }

    /// Ask the plugins that answer this hook whether it may go ahead.
    pub(super) fn plugin_allows(&self, hook: &str, data: &Value) -> bool {
        !self.plugins_on() || self.pl.manager.ask(hook, data)
    }

    pub(super) fn plugin_track_json(&self, t: &Track) -> Value {
        track_json(t)
    }

    pub(super) fn plugin_action(&self, plugin: &str, id: &str, data: Value) {
        self.pl.manager.send(plugin, ToPlugin::Action { id: id.to_string(), data });
    }

    fn plugin_name(&self, id: &str) -> String {
        self.pl.manager.plugins.iter().find(|p| p.manifest.id == id).map(|p| p.manifest.name.clone()).unwrap_or_else(|| id.to_string())
    }

    /// Once per frame: start and stop plugins, tell them what is playing, carry out what they ask.
    pub(super) fn plugins_frame(&mut self, ctx: &egui::Context) {
        if self.pl.dirty {
            self.pl.dirty = false;
            let on = self.plugins_on();
            self.pl.manager.sync(&self.cfg.plugin_cfg, on, self.cfg.plugin_dev_mode);
        }
        if self.pl.manager.running.is_empty() {
            self.pl.menu.clear();
            return;
        }
        // what plugins may read
        let id = self.current.as_ref().map(|t| t.id);
        let playing = self.playing && !self.finished;
        let position = if playing { self.position + self.position_at.elapsed().as_secs_f64().min(1.0) } else { self.position };
        let track_changed = self.pl.sent_track != Some(id);
        let track = self.current.as_ref().map(track_json);
        if let Ok(mut s) = self.pl.manager.shared.lock() {
            s.position = position;
            s.playing = playing;
            s.volume = self.cfg.volume;
            if track_changed {
                s.track = track.clone();
            }
        }
        if track_changed {
            self.pl.sent_track = Some(id);
            if let Some(t) = track {
                self.pl.manager.broadcast(|| ToPlugin::Track(t.clone()));
            }
        }
        if self.pl.sent_playing != Some(playing) {
            self.pl.sent_playing = Some(playing);
            self.pl.manager.broadcast(|| ToPlugin::Playing(playing));
        }
        self.pl.menu = self.pl.manager.menu_items();

        for (plugin, msg) in self.pl.manager.poll() {
            match msg {
                FromPlugin::Ready { .. } => {
                    // a plugin that has just started learns what is playing
                    if let Some(t) = self.current.as_ref().map(track_json) {
                        self.pl.manager.send(&plugin, ToPlugin::Track(t));
                    }
                    self.pl.manager.send(&plugin, ToPlugin::Playing(playing));
                }
                FromPlugin::Toast(text) => {
                    let name = self.plugin_name(&plugin);
                    self.toast(format!("{name}: {text}"));
                }
                FromPlugin::Player(cmd) => self.plugin_player_command(cmd),
                FromPlugin::Clipboard(text) => {
                    ctx.copy_text(text);
                    let name = self.plugin_name(&plugin);
                    self.toast(format!("{name} copied something to the clipboard"));
                }
                FromPlugin::OpenUrl(url) => {
                    let recently = self.pl.last_link.get(&plugin).is_some_and(|t| t.elapsed() < Duration::from_secs(3));
                    if !recently {
                        self.pl.last_link.insert(plugin.clone(), Instant::now());
                        let name = self.plugin_name(&plugin);
                        self.toast(format!("{name} opened a link"));
                        open_url(&url);
                    }
                }
                _ => {}
            }
        }
    }

    fn plugin_player_command(&mut self, cmd: PlayerCmd) {
        match cmd {
            PlayerCmd::Play => {
                if !self.playing {
                    self.toggle_play();
                }
            }
            PlayerCmd::Pause => {
                if self.playing {
                    self.toggle_play();
                }
            }
            PlayerCmd::Toggle => self.toggle_play(),
            PlayerCmd::Next => self.advance(false),
            PlayerCmd::Previous => self.previous(),
            PlayerCmd::Seek(s) => {
                if self.current.is_some() {
                    self.seek(s.min(self.duration.max(0.0)));
                }
            }
            PlayerCmd::Volume(v) => self.set_volume(v),
        }
    }

    // ------------------------------------------------------------ buttons

    /// The buttons plugins have put into the header, left of the settings button.
    pub(super) fn plugin_header_buttons(&mut self, ui: &mut egui::Ui, settings_tile: egui::Rect) {
        if !self.plugins_on() {
            return;
        }
        let items: Vec<(String, Button)> = self.pl.manager.buttons(Place::Header).into_iter().map(|(p, b)| (p.to_string(), b.clone())).collect();
        let mut x = settings_tile.left() - 8.0;
        let mut clicked = None;
        for (plugin, b) in items.iter().rev() {
            let tile = egui::Rect::from_min_size(egui::pos2(x - settings_tile.width(), settings_tile.top()), settings_tile.size());
            x -= settings_tile.width() + 8.0;
            let resp = ui.interact(tile, egui::Id::new(("plugin_header", plugin, &b.id)), egui::Sense::click());
            pointer_on_hover(&resp);
            top_tile(ui, tile, resp.hovered(), false, registry::icon(&b.icon), 20.0, ui::TEXT, egui::FontFamily::Proportional);
            if resp.on_hover_text(button_tip(b)).clicked() {
                clicked = Some((plugin.clone(), b.id.clone()));
            }
        }
        if let Some((plugin, id)) = clicked {
            self.plugin_action(&plugin, &id, Value::Null);
        }
    }

    /// The buttons plugins have put into the player bar or the full-screen player (in a row).
    pub(super) fn plugin_row_buttons(&mut self, ui: &mut egui::Ui, place: Place, size: f32, tiles: bool) {
        if !self.plugins_on() {
            return;
        }
        let items: Vec<(String, Button)> = self.pl.manager.buttons(place).into_iter().map(|(p, b)| (p.to_string(), b.clone())).collect();
        let mut clicked = None;
        for (plugin, b) in items {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            pointer_on_hover(&resp);
            if tiles {
                top_tile(ui, rect, resp.hovered(), false, registry::icon(&b.icon), 18.0, ui::TEXT, egui::FontFamily::Proportional);
            } else {
                if resp.hovered() {
                    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, CARD_HOVER);
                }
                let color = if resp.hovered() { ui::TEXT } else { TEXT_DIM };
                ui::paint_icon(ui.painter(), rect.center(), registry::icon(&b.icon), size * 0.58, color, egui::FontFamily::Proportional);
            }
            if resp.on_hover_text(button_tip(&b)).clicked() {
                clicked = Some((plugin, b.id.clone()));
            }
        }
        if let Some((plugin, id)) = clicked {
            self.plugin_action(&plugin, &id, Value::Null);
        }
    }

    // ------------------------------------------------------------ themes

    /// The theme file in use, if one is chosen and still exists.
    pub(super) fn custom_theme(&self) -> Option<&CustomTheme> {
        if self.cfg.theme.is_empty() {
            return None;
        }
        self.pl.themes.iter().find(|t| t.id == self.cfg.theme)
    }

    pub(super) fn theme_settings(&mut self, ui: &mut egui::Ui) {
        settings_group(ui, "Theme");
        ui.label(RichText::new("Colours for the whole app. The built-in look follows the cover; a theme file sets its own colours (see API.md to make one).").size(12.5).color(TEXT_DIM));
        ui.add_space(6.0);
        let mut choose: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            if pill(ui, "Built-in", self.cfg.theme.is_empty(), 90.0).clicked() {
                choose = Some(String::new());
            }
            for t in &self.pl.themes {
                let label = &t.name;
                let width = 28.0 + label.chars().count() as f32 * 8.0;
                if pill(ui, label, self.cfg.theme == t.id, width).on_hover_text(if t.author.is_empty() { t.description.clone() } else { format!("{} (by {})", t.description, t.author) }).clicked() {
                    choose = Some(t.id.clone());
                }
            }
        });
        if let Some(id) = choose {
            self.cfg.theme = id;
            self.touch();
        }
        if self.pl.themes.is_empty() {
            ui.label(RichText::new("No theme files yet.").size(12.5).color(TEXT_DIM));
        }
        for (name, why) in &self.pl.theme_problems {
            ui.label(RichText::new(format!("{name}.toml: {why}")).size(12.0).color(ui::DANGER));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui::glass_button(ui, icons::FOLDER_OPEN, "Open themes folder").clicked() {
                let dir = theme_file::themes_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_url(&dir.to_string_lossy());
            }
            if ui::glass_button(ui, icons::ARROW_CLOCKWISE, "Reload").clicked() {
                let (themes, problems) = theme_file::discover(&theme_file::themes_dir());
                self.pl.themes = themes;
                self.pl.theme_problems = problems;
            }
        });
    }

    // ------------------------------------------------------------ settings

    /// The “Plugins” page: the master switch, and every plugin with what it may do.
    pub(super) fn plugins_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Plugins");
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Plugins are small scripts that add buttons, menus and settings or change how Oxytide behaves. \
                 They run in a sandbox: they cannot read your files, start programs or see your TIDAL account. \
                 A plugin can only do what you allow here.",
            )
            .size(12.5)
            .color(TEXT_DIM),
        );
        if self.pl.off {
            ui.add_space(6.0);
            ui.label(RichText::new("Plugins are switched off for this run (started with --no-plugins).").color(ui::WARN));
        }
        ui.add_space(6.0);
        let mut on = self.cfg.plugins_enabled;
        if toggle_row(ui, &mut on, "Allow plugins", "Off: no plugin runs, whatever else is set.") {
            self.cfg.plugins_enabled = on;
            self.pl.dirty = true;
            self.touch();
        }
        let mut dev = self.cfg.plugin_dev_mode;
        if toggle_row(ui, &mut dev, "Developer mode", "For writing plugins: edits to a plugin's script are accepted without asking again. Asking for more permissions still needs your approval.") {
            self.cfg.plugin_dev_mode = dev;
            self.pl.dirty = true;
            self.touch();
        }
        ui.horizontal(|ui| {
            if ui::glass_button(ui, icons::FOLDER_OPEN, "Open plugins folder").clicked() {
                let dir = self.pl.manager.root().to_path_buf();
                let _ = std::fs::create_dir_all(&dir);
                open_url(&dir.to_string_lossy());
            }
            if ui::glass_button(ui, icons::ARROW_CLOCKWISE, "Reload").clicked() {
                self.pl.manager.rescan();
                self.pl.dirty = true;
            }
        });
        ui.add_space(8.0);

        let list: Vec<plugins::Plugin> = self.pl.manager.plugins.clone();
        if list.is_empty() {
            ui.label(RichText::new(format!("No plugins found. Put a plugin folder into {}", self.pl.manager.root().display())).size(12.5).color(TEXT_DIM));
        }
        for p in list {
            self.plugin_card(ui, &p);
            ui.add_space(8.0);
        }
        for (name, why) in self.pl.manager.problems.clone() {
            ui.label(RichText::new(format!("{name}: {why}")).size(12.0).color(ui::DANGER));
        }
    }

    fn plugin_card(&mut self, ui: &mut egui::Ui, p: &plugins::Plugin) {
        let id = p.manifest.id.clone();
        let status = plugins::status(p, self.cfg.plugin_cfg.get(&id), self.cfg.plugin_dev_mode);
        let running = self.pl.manager.running.get(&id);
        let (alive, stopped, log): (bool, Option<String>, Vec<(bool, String)>) = match running {
            Some(r) => (r.alive, r.stopped.clone(), r.log.iter().cloned().collect()),
            None => (false, None, Vec::new()),
        };
        let mut turn_on = false;
        let mut turn_off = false;
        let mut restart = false;
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&p.manifest.name).strong().size(15.0));
                ui.label(RichText::new(format!("v{}", p.manifest.version)).size(12.0).color(TEXT_DIM));
                if !p.manifest.author.is_empty() {
                    ui.label(RichText::new(format!("by {}", p.manifest.author)).size(12.0).color(TEXT_DIM));
                }
            });
            if !p.manifest.description.is_empty() {
                ui.label(RichText::new(&p.manifest.description).size(12.5).color(ui::lerp_color(ui::TEXT, TEXT_DIM, 0.35)));
            }
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if p.manifest.permissions.is_empty() {
                    ui.label(RichText::new("asks for no permissions").size(12.0).color(TEXT_DIM));
                }
                for perm in &p.manifest.permissions {
                    ui.label(RichText::new(perm.id()).size(12.0).color(risk_color(perm.risk())).monospace()).on_hover_text(perm.description());
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                match status {
                    Status::Off => {
                        ui.label(RichText::new("Off").color(TEXT_DIM));
                        turn_on = ui::glass_button(ui, "", "Turn on…").clicked();
                    }
                    Status::NeedsConsent(why) => {
                        let text = match why {
                            Why::Changed => "Changed since you allowed it: needs your approval again",
                            Why::MorePermissions => "Asks for more than you allowed: needs your approval",
                        };
                        ui.label(RichText::new(text).color(ui::WARN));
                        turn_on = ui::glass_button(ui, "", "Review…").clicked();
                        turn_off = ui::glass_button(ui, "", "Turn off").clicked();
                    }
                    Status::Allowed => {
                        if !self.plugins_on() {
                            ui.label(RichText::new("Allowed, but plugins are switched off").color(TEXT_DIM));
                        } else if let Some(why) = &stopped {
                            ui.label(RichText::new(format!("Stopped: {}", ellipsize(why, 60))).color(ui::DANGER));
                            restart = ui::glass_button(ui, "", "Restart").clicked();
                        } else if alive {
                            ui.label(RichText::new("Running").color(accent()));
                        } else {
                            ui.label(RichText::new("Starting…").color(TEXT_DIM));
                        }
                        turn_off = ui::glass_button(ui, "", "Turn off").clicked();
                    }
                }
            });
            if !log.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("Messages ({})", log.len())).size(12.0).color(TEXT_DIM)).id_salt(("plugin_log", &id)).show(ui, |ui| {
                    for (error, line) in log.iter().rev().take(12) {
                        ui.label(RichText::new(line).size(11.5).monospace().color(if *error { ui::DANGER } else { TEXT_DIM }));
                    }
                });
            }
        });
        if turn_on {
            self.pl.consent = Some(id.clone());
        }
        if turn_off {
            if let Some(c) = self.cfg.plugin_cfg.get_mut(&id) {
                c.enabled = false;
            }
            self.pl.dirty = true;
            self.touch();
        }
        if restart {
            self.pl.manager.restart(&id);
            self.pl.dirty = true;
        }
    }

    /// “May this plugin do this?” — what it asks for, in plain words, before anything runs.
    pub(super) fn plugin_consent_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.pl.consent.clone() else { return };
        let Some(p) = self.pl.manager.plugins.iter().find(|p| p.manifest.id == id).cloned() else {
            self.pl.consent = None;
            return;
        };
        let was = self.cfg.plugin_cfg.get(&id).filter(|c| c.enabled).is_some();
        let frame = ui::card().fill(ui::panel_solid()).inner_margin(egui::Margin::same(22));
        let mut allow = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("plugin_consent")).frame(frame).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(format!("Allow “{}”?", p.manifest.name)).strong().size(18.0));
            ui.label(RichText::new(format!("v{}{}", p.manifest.version, if p.manifest.author.is_empty() { String::new() } else { format!(" · by {}", p.manifest.author) })).size(12.5).color(TEXT_DIM));
            if was {
                ui.add_space(4.0);
                ui.label(RichText::new("This plugin was changed since you last allowed it.").color(ui::WARN));
            }
            ui.add_space(10.0);
            ui.label(RichText::new(if p.manifest.permissions.is_empty() { "It asks for no permissions." } else { "It asks to be able to:" }).size(13.0));
            ui.add_space(2.0);
            for perm in &p.manifest.permissions {
                ui.horizontal_top(|ui| {
                    let (mark, color) = match perm.risk() {
                        Risk::Low => (icons::CHECK, TEXT_DIM),
                        Risk::Medium => (icons::WARNING, ui::WARN),
                        Risk::High => (icons::WARNING_OCTAGON, ui::DANGER),
                    };
                    ui.label(RichText::new(mark).color(color));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(perm.description()).size(13.0));
                        if *perm == Permission::Network {
                            ui.label(RichText::new(p.manifest.hosts.join(", ")).size(12.0).monospace().color(ui::DANGER));
                        }
                    });
                });
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "A plugin is a program written by someone else. It cannot read your files, start programs or see your TIDAL account, \
                     but it can do everything listed above. Only allow plugins from people you trust.",
                )
                .size(12.0)
                .color(TEXT_DIM),
            );
            ui.add_space(2.0);
            ui.label(RichText::new(format!("Fingerprint {}", &p.hash[..16])).size(11.0).monospace().color(TEXT_DIM));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                allow = action_button(ui, icons::CHECK, "Allow and turn on", true).clicked();
                cancel = action_button(ui, icons::X, "Cancel", false).clicked();
            });
        });
        if allow {
            self.cfg.plugin_cfg.insert(id.clone(), plugins::consent(&p));
            self.pl.manager.restart(&id);
            self.pl.dirty = true;
            self.pl.consent = None;
            self.touch();
        } else if cancel || modal.should_close() {
            self.pl.consent = None;
        }
    }

    /// A settings page that a plugin made.
    pub(super) fn plugin_page(&mut self, ui: &mut egui::Ui) {
        let Some((plugin, cat)) = self.pl.page.clone() else { return };
        let found = self.pl.manager.categories().into_iter().find(|(p, c)| *p == plugin && c.id == cat).map(|(_, c)| c.clone());
        let Some(category) = found else {
            ui.label(RichText::new("This page is gone (the plugin has stopped).").color(TEXT_DIM));
            return;
        };
        ui.heading(&category.title);
        ui.label(RichText::new(format!("from the plugin “{}”", self.plugin_name(&plugin))).size(12.0).color(TEXT_DIM));
        ui.add_space(6.0);
        for entry in &category.entries {
            self.plugin_entry(ui, &plugin, entry);
        }
    }

    /// What plugins added at the end of a built-in settings page.
    pub(super) fn plugin_extra_entries(&mut self, ui: &mut egui::Ui, page: &str) {
        if !self.plugins_on() {
            return;
        }
        let entries: Vec<(String, Entry)> = self.pl.manager.extra_entries(page).into_iter().map(|(p, e)| (p.to_string(), e.clone())).collect();
        if entries.is_empty() {
            return;
        }
        settings_group(ui, "From plugins");
        for (plugin, entry) in entries {
            self.plugin_entry(ui, &plugin, &entry);
        }
    }

    /// One line of a plugin's settings; a change goes to the plugin (and is shown at once).
    fn plugin_entry(&mut self, ui: &mut egui::Ui, plugin: &str, e: &Entry) {
        let mut change: Option<Value> = None;
        let key = format!("{plugin}/{}", e.id);
        let help = |ui: &mut egui::Ui, text: &str| {
            if !text.is_empty() {
                ui.label(RichText::new(text).size(12.5).color(TEXT_DIM));
            }
        };
        match &e.kind {
            Kind::Heading => {
                ui.add_space(8.0);
                ui.label(RichText::new(&e.label).strong().size(14.5));
            }
            Kind::Label => {
                ui.label(RichText::new(&e.label).size(13.0).color(ui::lerp_color(ui::TEXT, TEXT_DIM, 0.3)));
            }
            Kind::Toggle(v) => {
                let mut value = *v;
                if toggle_row(ui, &mut value, &e.label, &e.help) {
                    change = Some(Value::Bool(value));
                }
            }
            Kind::Text(v) => {
                ui.label(RichText::new(&e.label).size(13.5));
                let buffer = self.pl.texts.entry(key.clone()).or_insert_with(|| v.clone());
                let resp = ui::glass_input(ui, egui::Id::new(("plugin_text", &key)), buffer, "", ui.available_width().min(420.0), 1, 14.5);
                if resp.lost_focus() {
                    change = Some(Value::String(buffer.clone()));
                }
                help(ui, &e.help);
            }
            Kind::Number { value, min, max, step } => {
                ui.label(RichText::new(&e.label).size(13.5));
                let mut v = *value;
                let resp = ui.add(egui::Slider::new(&mut v, *min..=*max).step_by(*step));
                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                    change = serde_json::Number::from_f64(v).map(Value::Number);
                }
                help(ui, &e.help);
            }
            Kind::Choice { options, value } => {
                ui.label(RichText::new(&e.label).size(13.5));
                ui.horizontal_wrapped(|ui| {
                    for o in options {
                        if pill(ui, o, o == value, 28.0 + o.chars().count() as f32 * 8.0).clicked() {
                            change = Some(Value::String(o.clone()));
                        }
                    }
                });
                help(ui, &e.help);
            }
            Kind::Button => {
                if ui::glass_button(ui, "", &e.label).clicked() {
                    change = Some(Value::Bool(true));
                }
                help(ui, &e.help);
            }
        }
        if let Some(value) = change {
            // show it at once; the plugin confirms (or corrects) it a moment later
            if let Some(r) = self.pl.manager.running.get_mut(plugin) {
                let all = r.registry.categories.iter_mut().flat_map(|c| c.entries.iter_mut()).chain(r.registry.extra.iter_mut().map(|(_, e)| e));
                if let Some(entry) = all.into_iter().find(|x| x.id == e.id) {
                    match (&mut entry.kind, &value) {
                        (Kind::Toggle(v), Value::Bool(b)) => *v = *b,
                        (Kind::Text(v), Value::String(s)) => *v = s.clone(),
                        (Kind::Number { value: v, .. }, Value::Number(n)) => *v = n.as_f64().unwrap_or(*v),
                        (Kind::Choice { value: v, .. }, Value::String(s)) => *v = s.clone(),
                        _ => {}
                    }
                }
            }
            self.pl.manager.send(plugin, ToPlugin::Setting { id: e.id.clone(), value });
        }
        ui.add_space(4.0);
    }
}

fn button_tip(b: &Button) -> String {
    if !b.tooltip.is_empty() {
        b.tooltip.clone()
    } else if !b.label.is_empty() {
        b.label.clone()
    } else {
        "Plugin".to_string()
    }
}

/// The song menu entries plugins added.
pub(super) fn menu_entries(ui: &mut egui::Ui, items: &[(String, MenuItem)], track: &Track, action: &mut Option<(Track, TrackAction)>) {
    if items.is_empty() {
        return;
    }
    ui.separator();
    for (plugin, item) in items {
        if ui.button(format!("{}  {}", registry::icon(&item.icon), item.label)).clicked() {
            *action = Some((track.clone(), TrackAction::Plugin { plugin: plugin.clone(), id: item.id.clone() }));
            ui.close_menu();
        }
    }
}
