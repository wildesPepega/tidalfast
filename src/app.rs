//! The egui application: login, library, search, player, equaliser and settings.

mod eq_view;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, TextureHandle};

use crate::audio::{Audio, AudioCmd, AudioEv, OutputDevice};
use crate::config::{RepeatMode, Settings};
use crate::system::{NowPlaying, SysEvent, System};
use crate::transition::TransitionMode;
use crate::discord::{Discord, DiscordCmd};
use crate::eq::EqShared;
use crate::link;
use crate::model::{format_duration, Album, Artist, ArtistPage, CreditGroup, ImportTrack, Lyrics, Playlist, SearchResults, Track};
use crate::net::{Cmd, Ev, Net, Opened};
use crate::ui::{
    self, action_button, icon_button, icons, nav_item, pill, play_button, pointer_on_hover,
    thin_slider, Icon, ACCENT, BG, CARD, CARD_HOVER, LINE, TEXT_DIM,
};

const COVER_ROW: u32 = 160;
const COVER_NOW: u32 = 320;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Search,
    Playlists,
    Artist,
    Import,
}

/// One track of a Spotify playlist being imported, and its TIDAL match.
struct ImportItem {
    src: ImportTrack,
    found: Option<Track>,
    /// The match agrees on title, artist and length.
    sure: bool,
    selected: bool,
    /// Matching for this track has finished.
    checked: bool,
}

struct ImportState {
    job: u64,
    cover: Option<String>,
    items: Vec<ImportItem>,
    /// The playlist itself has been read (matching may still be running).
    loaded: bool,
    /// Name of the TIDAL playlist to create.
    title: String,
}

/// A place the user can navigate back to.
#[derive(Clone)]
enum Loc {
    Home,
    Search,
    Playlist(OpenPlaylist),
    Artist(u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchTab {
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NowTab {
    Lyrics,
    Credits,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum SettingsTab {
    Playback,
    Equalizer,
    Discord,
    System,
    Account,
}

const LIKED_UUID: &str = "liked";

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlAction {
    Open,
    Play,
    Share(ShareTarget),
    /// Title, description and cover.
    Edit,
    Delete,
}

/// Which service's link "Share" copies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareTarget {
    Tidal,
    Spotify,
    Youtube,
    /// linkhop page listing every service.
    All,
}

/// What a card or library row needs to know to draw its indicators and menu.
struct CardState {
    playing_here: bool,
    playing: bool,
    own: bool,
    /// Albums and artists can be shared as a linkhop link.
    shareable: bool,
    bars: [f32; VIZ_BARS],
}

enum Dialog {
    Edit {
        uuid: String,
        title: String,
        description: String,
        /// Image chosen for the new cover.
        cover: Option<std::path::PathBuf>,
    },
    Delete { uuid: String, title: String },
    Create { text: String },
}

struct User {
    name: String,
    country: String,
}

struct DeviceCode {
    user_code: String,
    uri: String,
    uri_complete: Option<String>,
}

#[derive(Clone)]
struct OpenPlaylist {
    uuid: String,
    title: String,
    creator: String,
    description: String,
    cover: Option<String>,
    tracks: Vec<Track>,
    loading: bool,
}

pub struct App {
    cfg: Settings,
    net: Net,
    audio: Audio,
    discord: Discord,

    user: Option<User>,
    device_code: Option<DeviceCode>,
    login_busy: bool,

    view: View,
    playlists: Vec<Playlist>,
    playlists_loading: bool,
    open_playlist: Option<OpenPlaylist>,
    search_query: String,
    results: Option<SearchResults>,
    search_loading: bool,
    search_pending: Option<Instant>,
    search_tab: SearchTab,
    history: Vec<Loc>,
    current_artist: Option<u64>,
    artist_pages: HashMap<u64, ArtistPage>,
    artist_all_tracks: bool,
    link_rx: std::sync::mpsc::Receiver<String>,
    home: Vec<(String, Vec<Playlist>)>,
    radio_for: Option<u64>,
    liked_ids: HashSet<u64>,
    playing_from: Option<String>,
    pending_play: Option<String>,
    dialog: Option<Dialog>,
    egui_ctx: egui::Context,
    sync_budget: u32,
    last_query: String,

    queue: Vec<Track>,
    queue_index: usize,
    shuffle_order: Vec<usize>,
    shuffle_pos: usize,
    current: Option<Track>,
    playing: bool,
    finished: bool,
    position: f64,
    duration: f64,
    now_quality: String,
    /// Track whose stream is being resolved and loaded (rapid skips drop older ones).
    loading: Option<u64>,
    scrub: f64,
    scrubbing: bool,

    devices: Vec<OutputDevice>,
    settings_open: bool,
    import: Option<ImportState>,
    import_job: u64,
    queue_open: bool,
    /// Sidebar animation: 0 = covers only, 1 = with names.
    sidebar_t: f32,
    /// Recent-searches dropdown under the search field.
    search_popup: bool,
    search_popup_rect: egui::Rect,
    /// Volume to return to when unmuting.
    unmuted_volume: f32,
    /// The upcoming track handed to the audio engine: (queue index, shuffle position, track id).
    preload: Option<(usize, Option<usize>, u64)>,
    /// Cover image picked in the edit dialog: file dialog result and preview.
    cover_pick: Option<std::sync::mpsc::Receiver<std::path::PathBuf>>,
    cover_preview: Option<TextureHandle>,
    /// Last click on a playlist in the collapsed sidebar.
    lib_click: Option<(String, Instant)>,
    /// Media keys / tray integration.
    system: System,
    /// The window is hidden (in the tray).
    window_hidden: bool,
    /// Quit for real, even with "close to tray".
    quitting: bool,
    /// What the desktop's media controls were last told: (track id, playing).
    sys_sent: Option<(Option<u64>, bool)>,
    /// The playback position jumped; tell the desktop again.
    sys_dirty: bool,
    now_open: bool,
    now_tab: NowTab,
    info_for: Option<u64>,
    lyrics: Option<Lyrics>,
    credits: Option<Vec<CreditGroup>>,
    lyric_idx: Option<usize>,
    viz: [f32; VIZ_BARS],
    viz_ref: f32,
    avatar_rect: egui::Rect,
    settings_tab: SettingsTab,
    profile_menu_open: bool,
    liked_count: Option<usize>,

    eq_shared: Arc<EqShared>,
    eq_preset: Option<String>,
    eq_selected: usize,
    eq_new_name: String,

    textures: HashMap<String, TextureHandle>,
    requested: HashSet<String>,
    /// Colours sampled from each loaded cover, used for the wallpaper.
    palettes: HashMap<String, [Color32; 3]>,
    /// Tiny blurred copies of covers, stretched behind the UI.
    blur_tex: HashMap<String, TextureHandle>,
    /// The wallpaper colours currently shown (eased towards the song's).
    bg_colors: [Color32; 3],
    /// Backdrop currently shown, the one fading out, and the fade progress.
    wall_url: Option<String>,
    wall_prev: Option<String>,
    wall_t: f32,
    wall_tried: HashSet<String>,

    toast: Option<(String, Instant)>,
    #[cfg(debug_assertions)]
    shot: debug_shot::Shot,
    dirty: bool,
    last_save: Instant,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start_link: Option<String>) -> Self {
        ui::apply_theme(&cc.egui_ctx);

        let cfg = Settings::load();
        let tokens = crate::tidal::Tokens {
            access_token: cfg.access_token.clone(),
            refresh_token: cfg.refresh_token.clone(),
            expires_in: 0,
            token_type: "Bearer".to_string(),
            user_id: cfg.user_id,
        };
        let net = Net::spawn(cfg.country_code.clone(), tokens).expect("start network worker");
        let eq_shared = EqShared::new(cfg.eq.clone());
        let audio = Audio::spawn(cfg.volume, cfg.output_device.clone(), eq_shared.clone(), cfg.transition());
        let discord = Discord::spawn(cfg.discord);

        let logged_in = cfg.is_logged_in();
        let user = logged_in.then(|| User {
            name: cfg.user_name.clone(),
            country: cfg.country_code.clone(),
        });

        let mut app = Self {
            playlists_loading: logged_in,
            cfg,
            net,
            audio,
            discord,
            user,
            device_code: None,
            login_busy: false,
            view: View::Playlists,
            playlists: Vec::new(),
            open_playlist: None,
            search_query: String::new(),
            results: None,
            search_loading: false,
            search_pending: None,
            search_tab: SearchTab::All,
            history: Vec::new(),
            current_artist: None,
            artist_pages: HashMap::new(),
            artist_all_tracks: false,
            link_rx: if cfg!(debug_assertions) && std::env::var_os("TIDALFAST_SHOT").is_some() {
                std::sync::mpsc::channel().1
            } else {
                crate::ipc::listen(cc.egui_ctx.clone())
            },
            home: Vec::new(),
            radio_for: None,
            liked_ids: HashSet::new(),
            playing_from: None,
            pending_play: None,
            dialog: None,
            egui_ctx: cc.egui_ctx.clone(),
            sync_budget: 0,
            last_query: String::new(),
            queue: Vec::new(),
            queue_index: 0,
            shuffle_order: Vec::new(),
            shuffle_pos: 0,
            current: None,
            playing: false,
            finished: false,
            position: 0.0,
            duration: 0.0,
            now_quality: String::new(),
            loading: None,
            scrub: 0.0,
            scrubbing: false,
            devices: Vec::new(),
            settings_open: false,
            import: None,
            import_job: 0,
            queue_open: false,
            sidebar_t: 0.0,
            search_popup: false,
            search_popup_rect: egui::Rect::NOTHING,
            unmuted_volume: 0.8,
            preload: None,
            now_open: false,
            now_tab: NowTab::Lyrics,
            info_for: None,
            lyrics: None,
            credits: None,
            lyric_idx: None,
            viz: [0.0; VIZ_BARS],
            viz_ref: -40.0,
            avatar_rect: egui::Rect::NOTHING,
            settings_tab: SettingsTab::Playback,
            lib_click: None,
            cover_pick: None,
            cover_preview: None,
            system: System::new(cc),
            window_hidden: false,
            quitting: false,
            sys_sent: None,
            sys_dirty: false,
            profile_menu_open: false,
            liked_count: None,
            eq_shared,
            eq_preset: None,
            eq_selected: 0,
            eq_new_name: String::new(),
            textures: HashMap::new(),
            requested: HashSet::new(),
            palettes: HashMap::new(),
            blur_tex: HashMap::new(),
            bg_colors: ui::DEFAULT_PALETTE,
            wall_url: None,
            wall_prev: None,
            wall_t: 1.0,
            wall_tried: HashSet::new(),
            toast: None,
            #[cfg(debug_assertions)]
            shot: debug_shot::Shot::from_env(),
            dirty: false,
            last_save: Instant::now(),
        };

        if app.cfg.volume > 0.0 {
            app.unmuted_volume = app.cfg.volume;
        }
        app.system.set_media(app.cfg.media_keys);
        app.system.set_tray(app.cfg.tray);
        if logged_in {
            // Show the last known account data immediately; fresh data replaces it.
            if let Some(list) = crate::cache::read::<Vec<Playlist>>("playlists") {
                app.playlists = list;
            }
            if let Some(home) = crate::cache::read::<Vec<(String, Vec<Playlist>)>>("home") {
                app.home = home;
            }
            if let Some(liked) = crate::cache::read::<Vec<Track>>("tracks-liked") {
                app.liked_count = Some(liked.len());
                app.liked_ids = liked.iter().map(|t| t.id).collect();
            }
            app.playlists_loading = app.playlists.is_empty();
        }
        if let Some(track) = app.cfg.last_track.clone() {
            // Show the last played track; pressing play starts it from the beginning.
            app.duration = track.duration as f64;
            app.queue = vec![track.clone()];
            app.queue_index = 0;
            // Restore the playlist/album the track came from, positioned at that track.
            let restored = app.cfg.last_source.clone().and_then(|src| {
                let tracks = crate::cache::read::<Vec<Track>>(&format!("tracks-{src}"))?;
                let index = tracks.iter().position(|t| t.id == track.id)?;
                Some((src, tracks, index))
            });
            match restored {
                Some((src, tracks, index)) => {
                    app.queue = tracks;
                    app.queue_index = index;
                    app.playing_from = Some(src);
                    if app.cfg.shuffle {
                        app.rebuild_shuffle(index);
                    }
                }
                None => {
                    app.radio_for = Some(track.id);
                    if logged_in {
                        app.net.send(Cmd::Radio { track_id: track.id });
                    }
                }
            }
            app.current = Some(track);
            app.finished = true;
        }
        if logged_in {
            app.net.send(Cmd::LoadUser);
            app.net.send(Cmd::LoadFavorites);
            app.net.send(Cmd::LoadHome);
        }
        if let Some(text) = start_link {
            app.open_external(&text);
        }
        app
    }

    // ---------------------------------------------------------------- events

    fn handle_net_events(&mut self, ctx: &egui::Context) {
        while let Some(ev) = self.net.try_recv() {
            match ev {
                Ev::DeviceCode {
                    user_code,
                    uri,
                    uri_complete,
                    ..
                } => {
                    self.device_code = Some(DeviceCode {
                        user_code,
                        uri,
                        uri_complete,
                    });
                }
                Ev::Tokens(tokens) => {
                    self.cfg.access_token = tokens.access_token;
                    self.cfg.refresh_token = tokens.refresh_token;
                    self.cfg.user_id = tokens.user_id;
                    self.dirty = true;
                }
                Ev::LoggedIn {
                    user_id,
                    user_name,
                    picture,
                    country_code,
                } => {
                    self.cfg.user_picture = picture.clone().unwrap_or_default();
                    self.cfg.user_id = Some(user_id);
                    self.cfg.user_name = user_name.clone();
                    self.cfg.country_code = country_code.clone();
                    self.user = Some(User {
                        name: user_name,
                        country: country_code,
                    });
                    self.device_code = None;
                    self.login_busy = false;
                    self.playlists_loading = true;
                    self.view = View::Playlists;
                    self.dirty = true;
                    self.net.send(Cmd::LoadFavorites);
                }
                Ev::Playlists(playlists) => {
                    // an edit changes what the open playlist shows
                    if let Some(open) = &mut self.open_playlist {
                        if let Some(pl) = playlists.iter().find(|p| p.uuid == open.uuid) {
                            open.title = pl.title.clone();
                            open.description = pl.description.clone().unwrap_or_default();
                            open.cover = pl.cover_url();
                        }
                    }
                    self.playlists = playlists;
                    self.playlists_loading = false;
                }
                Ev::PlaylistTracks { uuid, tracks } => {
                    if uuid == LIKED_UUID {
                        self.liked_count = Some(tracks.len());
                        self.liked_ids = tracks.iter().map(|t| t.id).collect();
                    }
                    if self.pending_play.as_deref() == Some(uuid.as_str()) {
                        self.pending_play = None;
                        self.play_tracks(tracks.clone(), 0, Some(uuid.clone()));
                    }
                    if let Some(open) = &mut self.open_playlist {
                        if open.uuid == uuid {
                            open.tracks = tracks;
                            open.loading = false;
                        }
                    }
                }
                Ev::Info(message) => self.toast(message),
                Ev::Home(sections) => {
                    self.home = sections;
                }
                Ev::Radio { track_id, tracks } => {
                    if self.radio_for == Some(track_id) {
                        let known: HashSet<u64> = self.queue.iter().map(|t| t.id).collect();
                        self.queue
                            .extend(tracks.into_iter().filter(|t| !known.contains(&t.id)));
                        if self.cfg.shuffle {
                            self.rebuild_shuffle(self.queue_index);
                        }
                    }
                }
                Ev::Search(results) => {
                    self.search_tab = SearchTab::All;
                    self.results = Some(results);
                    self.search_loading = false;
                }
                Ev::Stream { track_id, info, preload: true } => {
                    if let Some((idx, _, id)) = self.preload {
                        if id == track_id {
                            if let Some(track) = self.queue.get(idx).cloned() {
                                self.audio.send(AudioCmd::Preload {
                                    url: info.url,
                                    track: Box::new(track),
                                    quality: info.audio_quality.unwrap_or_default(),
                                });
                            }
                        }
                    }
                }
                Ev::Stream { track_id, info, preload: false } => {
                    // only the latest request counts: quick skips leave older streams behind
                    if self.loading == Some(track_id) && self.current.as_ref().map(|t| t.id) == Some(track_id) {
                        let track = self.current.clone().unwrap();
                        let quality = info.audio_quality.clone().unwrap_or_default();
                        self.audio.send(AudioCmd::Load {
                            url: info.url,
                            track: Box::new(track.clone()),
                            play: true,
                            quality: quality.clone(),
                        });
                        self.discord_track(&track, quality);
                    }
                }
                Ev::Image {
                    url,
                    width,
                    height,
                    rgba,
                } => {
                    if url.is_empty() {
                        continue;
                    }
                    self.make_wallpaper(&url, &rgba, width, height);
                    let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                    let texture = ctx.load_texture(&url, image, egui::TextureOptions::LINEAR);
                    self.textures.insert(url, texture);
                }
                Ev::Artist(page) => {
                    let id = page.artist.id;
                    self.artist_pages.insert(id, *page);
                }
                Ev::ImportStart { job, name, cover, tracks } => {
                    if job == self.import_job {
                        self.import = Some(ImportState {
                            job,
                            title: name,
                            cover,
                            items: tracks
                                .into_iter()
                                .map(|src| ImportItem { src, found: None, sure: false, selected: false, checked: false })
                                .collect(),
                            loaded: true,
                        });
                    }
                }
                Ev::ImportMatch { job, index, track, sure } => {
                    if let Some(state) = self.import.as_mut().filter(|s| s.job == job) {
                        if let Some(item) = state.items.get_mut(index) {
                            item.checked = true;
                            item.sure = sure;
                            item.selected = track.is_some();
                            item.found = track.map(|t| *t);
                        }
                    }
                }
                Ev::Lyrics { track_id, lyrics } => {
                    if self.info_for == Some(track_id) {
                        self.lyrics = Some(lyrics);
                    }
                }
                Ev::Credits { track_id, groups } => {
                    if self.info_for == Some(track_id) {
                        self.credits = Some(groups);
                    }
                }
                Ev::Copy(text) => {
                    ctx.copy_text(text);
                    self.toast("Link copied".into());
                }
                Ev::Open(opened, true) => {
                    // a link typed into the search: show what it points to
                    let mut results = SearchResults::default();
                    match opened {
                        Opened::Track(track) => results.tracks.push(*track),
                        Opened::Album(album) => results.albums.push(*album),
                        Opened::Artist(artist) => results.artists.push(*artist),
                        Opened::Playlist(pl) => results.playlists.push(*pl),
                    }
                    self.search_tab = SearchTab::All;
                    self.results = Some(results);
                    self.search_loading = false;
                    self.view = View::Search;
                }
                Ev::SpotifyPlaylist(id) => self.start_import(id),
                Ev::Open(opened, false) => match opened {
                    Opened::Track(track) => self.play_single(*track),
                    Opened::Album(album) => self.open_playlist_from(album.as_playlist()),
                    Opened::Artist(artist) => self.open_artist(artist.id, Some(*artist)),
                    Opened::Playlist(pl) => self.open_playlist_from(*pl),
                },
                Ev::Error(message) => {
                    self.loading = None;
                    if self.import.as_ref().is_some_and(|s| !s.loaded) {
                        // reading the playlist failed: leave the import view
                        self.import = None;
                        if self.view == View::Import {
                            self.go_back();
                        }
                    }
                    self.login_busy = false;
                    self.playlists_loading = false;
                    self.search_loading = false;
                    if let Some(open) = &mut self.open_playlist {
                        open.loading = false;
                    }
                    self.toast(message);
                }
            }
        }
    }

    fn handle_audio_events(&mut self) {
        while let Some(ev) = self.audio.try_recv() {
            match ev {
                AudioEv::Devices(devices) => {
                    if self.devices.is_empty()
                        && !self.cfg.output_device.is_empty()
                        && !devices.iter().any(|d| d.id == self.cfg.output_device)
                    {
                        // The stored device is gone; fall back to default.
                        self.cfg.output_device.clear();
                        self.audio.send(AudioCmd::SetDevice(String::new()));
                        self.dirty = true;
                    }
                    self.devices = devices;
                }
                AudioEv::Started {
                    track,
                    duration,
                    quality,
                    auto,
                } => {
                    // a start the user has already skipped past is stale
                    if auto && self.loading.is_some() || !auto && self.loading != Some(track.id) {
                        continue;
                    }
                    self.loading = None;
                    if auto {
                        // the engine moved on to the preloaded track by itself
                        if let Some((idx, shuffle_pos, id)) = self.preload.take() {
                            if self.queue.get(idx).map(|t| t.id) == Some(id) {
                                self.queue_index = idx;
                                if let Some(pos) = shuffle_pos {
                                    self.shuffle_pos = pos;
                                }
                            }
                        }
                        self.cfg.last_track = Some((*track).clone());
                        self.cfg.last_source = self.playing_from.clone();
                        self.dirty = true;
                        self.finished = false;
                        self.discord_track(&track, quality.clone());
                    }
                    self.current = Some(*track);
                    self.duration = duration;
                    self.position = 0.0;
                    self.now_quality = quality;
                    self.playing = true;
                    self.discord.send(DiscordCmd::SetPlaying {
                        playing: true,
                        position: 0.0,
                    });
                }
                AudioEv::Progress { position, duration } => {
                    if !self.scrubbing && self.loading.is_none() {
                        self.position = position;
                    }
                    if duration > 0.0 {
                        self.duration = duration;
                    }
                }
                AudioEv::Paused(paused) => {
                    self.playing = !paused;
                    self.discord.send(DiscordCmd::SetPlaying {
                        playing: !paused,
                        position: self.position,
                    });
                }
                AudioEv::Ended => {
                    if self.loading.is_none() {
                        self.advance(true);
                    }
                }
                AudioEv::Error(message) => {
                    self.loading = None;
                    self.toast(message);
                }
            }
        }
    }

    // --------------------------------------------------------------- playback

    fn play_tracks(&mut self, tracks: Vec<Track>, index: usize, source: Option<String>) {
        if tracks.is_empty() {
            return;
        }
        self.playing_from = source;
        if tracks.len() > 1 {
            self.radio_for = None;
        }
        self.queue = tracks;
        self.queue_index = index.min(self.queue.len() - 1);
        if self.cfg.shuffle {
            self.rebuild_shuffle(self.queue_index);
        }
        self.start_current();
    }

    fn set_liked(&mut self, track_id: u64, liked: bool) {
        if liked {
            self.liked_ids.insert(track_id);
        } else {
            self.liked_ids.remove(&track_id);
        }
        self.liked_count = Some(self.liked_ids.len());
        self.net.send(Cmd::SetLiked { track_id, liked });
        if self
            .open_playlist
            .as_ref()
            .is_some_and(|o| o.uuid == LIKED_UUID)
        {
            // refresh the open liked list after the server applied the change
            self.net.send(Cmd::LoadFavorites);
        }
    }

    fn apply_track_action(&mut self, track: Track, action: TrackAction) {
        match action {
            TrackAction::Play => self.play_single(track),
            TrackAction::SetLiked(liked) => self.set_liked(track.id, liked),
            TrackAction::AddToPlaylist { uuid, title } => self.net.send(Cmd::AddToPlaylist {
                uuid,
                title,
                track_id: track.id,
            }),
            TrackAction::Share(target) => {
                self.share_url(format!("https://tidal.com/track/{}", track.id), target);
            }
            TrackAction::OpenArtist(artist) => self.open_artist(artist.id, Some(artist)),
            TrackAction::OpenAlbum(album) => self.open_playlist_from(album.as_playlist()),
            TrackAction::PlayNext => {
                self.queue_insert(track, true);
                self.toast("Playing next".into());
            }
            TrackAction::AddToQueue => {
                self.queue_insert(track, false);
                self.toast("Added to queue".into());
            }
        }
    }

    /// Play one track on its own; TIDAL's radio then keeps the music going.
    fn play_single(&mut self, track: Track) {
        self.radio_for = Some(track.id);
        self.net.send(Cmd::Radio { track_id: track.id });
        self.play_tracks(vec![track], 0, None);
    }

    /// True when the play order follows `shuffle_order`.
    fn shuffled(&self) -> bool {
        self.cfg.shuffle && self.shuffle_order.len() == self.queue.len()
    }

    /// Indices into `queue` of the tracks that play after the current one.
    fn upcoming(&self) -> Vec<usize> {
        if self.shuffled() {
            self.shuffle_order.iter().skip(self.shuffle_pos + 1).copied().collect()
        } else {
            (self.queue_index + 1..self.queue.len()).collect()
        }
    }

    /// Put a track right after the current one (`next`) or at the end of the queue.
    fn queue_insert(&mut self, track: Track, next: bool) {
        if self.queue.is_empty() {
            self.play_single(track);
            return;
        }
        let shuffled = self.shuffled();
        let at = if next { (self.queue_index + 1).min(self.queue.len()) } else { self.queue.len() };
        self.queue.insert(at, track);
        if shuffled {
            for i in &mut self.shuffle_order {
                if *i >= at {
                    *i += 1;
                }
            }
            let pos = if next { self.shuffle_pos + 1 } else { self.shuffle_order.len() };
            self.shuffle_order.insert(pos, at);
        } else if self.cfg.shuffle {
            self.rebuild_shuffle(self.queue_index);
        }
    }

    /// Drop an upcoming track (never the current one).
    fn queue_remove(&mut self, idx: usize) {
        if idx == self.queue_index || idx >= self.queue.len() {
            return;
        }
        let shuffled = self.shuffled();
        self.queue.remove(idx);
        if idx < self.queue_index {
            self.queue_index -= 1;
        }
        if shuffled {
            if let Some(pos) = self.shuffle_order.iter().position(|&i| i == idx) {
                self.shuffle_order.remove(pos);
                if pos < self.shuffle_pos {
                    self.shuffle_pos -= 1;
                }
            }
            for i in &mut self.shuffle_order {
                if *i > idx {
                    *i -= 1;
                }
            }
        } else if self.cfg.shuffle {
            self.rebuild_shuffle(self.queue_index);
        }
    }

    /// Skip ahead to a track in the queue.
    fn queue_jump(&mut self, idx: usize) {
        if idx >= self.queue.len() {
            return;
        }
        if self.shuffled() {
            if let Some(pos) = self.shuffle_order.iter().position(|&i| i == idx) {
                self.shuffle_pos = pos;
            }
        }
        self.queue_index = idx;
        self.start_current();
    }

    fn rebuild_shuffle(&mut self, start: usize) {
        let n = self.queue.len();
        let mut order: Vec<usize> = (0..n).collect();
        let mut seed = Instant::now().elapsed().as_nanos() as u64 ^ (n as u64) << 32;
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        for i in (1..n).rev() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (seed >> 33) as usize % (i + 1);
            order.swap(i, j);
        }
        if let Some(p) = order.iter().position(|&x| x == start) {
            order.swap(0, p);
        }
        self.shuffle_order = order;
        self.shuffle_pos = 0;
    }

    fn start_current(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        let track = self.queue[self.queue_index].clone();
        self.cfg.last_track = Some(track.clone());
        self.cfg.last_source = self.playing_from.clone();
        self.dirty = true;
        self.current = Some(track.clone());
        self.position = 0.0;
        self.duration = track.duration as f64;
        self.playing = true;
        self.finished = false;
        self.preload = None;
        self.loading = Some(track.id);
        self.net.send(Cmd::ResolveTrack {
            track_id: track.id,
            preload: false,
        });
    }

    fn discord_track(&self, track: &Track, quality: String) {
        self.discord.send(DiscordCmd::SetTrack {
            title: track.title.clone(),
            artist: track.artist_name(),
            album: track.album_title(),
            art_url: track.cover_url(COVER_NOW).unwrap_or_default(),
            duration: track.duration as f64,
            url: track
                .url
                .clone()
                .unwrap_or_else(|| format!("https://tidal.com/browse/track/{}", track.id)),
            quality,
        });
    }

    /// The track `advance` would pick next, without moving there:
    /// (queue index, shuffle position).
    fn peek_next(&self) -> Option<(usize, Option<usize>)> {
        if self.queue.is_empty() || self.cfg.repeat == RepeatMode::One {
            return None;
        }
        if self.shuffled() {
            if self.shuffle_pos + 1 < self.shuffle_order.len() {
                let pos = self.shuffle_pos + 1;
                Some((self.shuffle_order[pos], Some(pos)))
            } else if self.cfg.repeat == RepeatMode::All && !self.shuffle_order.is_empty() {
                Some((self.shuffle_order[0], Some(0)))
            } else {
                None
            }
        } else if self.queue_index + 1 < self.queue.len() {
            Some((self.queue_index + 1, None))
        } else if self.cfg.repeat == RepeatMode::All {
            Some((0, None))
        } else {
            None
        }
    }

    /// Keep the audio engine's next track in line with the queue, so it can
    /// crossfade or play gaplessly into it.
    fn refresh_preload(&mut self) {
        if self.current.is_none() || self.finished {
            return;
        }
        let want = self
            .peek_next()
            .and_then(|(idx, pos)| self.queue.get(idx).map(|t| (idx, pos, t.id)));
        if want == self.preload {
            return;
        }
        if self.preload.take().is_some() {
            self.audio.send(AudioCmd::ClearPreload);
        }
        // don't fetch while the user is still skipping through tracks
        let remaining = self.duration - self.position;
        let Some((_, _, id)) = want else { return };
        if self.position < 5.0 && remaining > 45.0 {
            return;
        }
        self.preload = want;
        self.net.send(Cmd::ResolveTrack {
            track_id: id,
            preload: true,
        });
    }

    fn advance(&mut self, auto: bool) {
        if self.queue.is_empty() {
            return;
        }
        if auto && self.cfg.repeat == RepeatMode::One {
            self.start_current();
            return;
        }

        let next = if self.cfg.shuffle {
            if self.shuffle_order.is_empty() {
                Some(self.queue_index)
            } else if self.shuffle_pos + 1 < self.shuffle_order.len() {
                self.shuffle_pos += 1;
                Some(self.shuffle_order[self.shuffle_pos])
            } else if self.cfg.repeat == RepeatMode::All {
                self.shuffle_pos = 0;
                Some(self.shuffle_order[0])
            } else {
                None
            }
        } else if self.queue_index + 1 < self.queue.len() {
            Some(self.queue_index + 1)
        } else if self.cfg.repeat == RepeatMode::All {
            Some(0)
        } else {
            None
        };

        match next {
            Some(i) => {
                self.queue_index = i;
                self.start_current();
            }
            None => {
                self.playing = false;
                self.finished = true;
                self.audio.send(AudioCmd::Pause);
                self.discord.send(DiscordCmd::SetPlaying {
                    playing: false,
                    position: self.position,
                });
            }
        }
    }

    fn previous(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        if self.position > 3.0 {
            self.seek(0.0);
            return;
        }
        if self.cfg.shuffle {
            if self.shuffle_pos > 0 {
                self.shuffle_pos -= 1;
                self.queue_index = self.shuffle_order[self.shuffle_pos];
            }
        } else if self.queue_index > 0 {
            self.queue_index -= 1;
        } else if self.cfg.repeat == RepeatMode::All {
            self.queue_index = self.queue.len() - 1;
        }
        self.start_current();
    }

    fn toggle_play(&mut self) {
        if self.current.is_none() {
            return;
        }
        if self.finished {
            self.start_current();
        } else {
            self.audio.send(AudioCmd::Toggle);
        }
    }

    fn seek(&mut self, pos: f64) {
        self.position = pos;
        self.sys_dirty = true;
        self.audio.send(AudioCmd::Seek(pos));
        self.discord.send(DiscordCmd::Seek { position: pos });
    }

    fn set_volume(&mut self, vol: f32) {
        let vol = vol.clamp(0.0, 1.0);
        if vol > 0.0 {
            self.unmuted_volume = vol;
        }
        self.cfg.volume = vol;
        self.audio.send(AudioCmd::SetVolume(vol));
        self.touch();
    }

    fn toggle_mute(&mut self) {
        let vol = if self.cfg.volume > 0.0 { 0.0 } else { self.unmuted_volume.max(0.05) };
        self.set_volume(vol);
    }

    /// Jump forwards/backwards in the current track.
    fn seek_by(&mut self, secs: f64) {
        if self.current.is_none() || self.finished || self.duration <= 0.0 {
            return;
        }
        self.seek((self.position + secs).clamp(0.0, (self.duration - 0.5).max(0.0)));
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        if ctx.wants_keyboard_input() {
            return;
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if pressed(Modifiers::NONE, Key::Space) {
            self.toggle_play();
        }
        // with Ctrl first, so the plain arrows below don't fire as well
        if pressed(Modifiers::COMMAND, Key::ArrowRight) {
            self.advance(false);
        }
        if pressed(Modifiers::COMMAND, Key::ArrowLeft) {
            self.previous();
        }
        if pressed(Modifiers::COMMAND, Key::F) || pressed(Modifiers::NONE, Key::Slash) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("search_field")));
        }
        if pressed(Modifiers::NONE, Key::ArrowRight) {
            self.seek_by(10.0);
        }
        if pressed(Modifiers::NONE, Key::ArrowLeft) {
            self.seek_by(-10.0);
        }
        if pressed(Modifiers::NONE, Key::ArrowUp) {
            self.set_volume(self.cfg.volume + 0.05);
        }
        if pressed(Modifiers::NONE, Key::ArrowDown) {
            self.set_volume(self.cfg.volume - 0.05);
        }
        if pressed(Modifiers::NONE, Key::M) {
            self.toggle_mute();
        }
    }

    fn shuffle_changed(&mut self) {
        if self.cfg.shuffle {
            self.rebuild_shuffle(self.queue_index);
        } else {
            self.shuffle_order.clear();
            self.shuffle_pos = 0;
        }
        self.dirty = true;
    }

    fn cycle_repeat(&mut self) {
        self.cfg.repeat = match self.cfg.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.dirty = true;
    }

    // ------------------------------------------------------------ navigation

    fn current_loc(&self) -> Loc {
        match self.view {
            View::Search => Loc::Search,
            View::Artist => self.current_artist.map(Loc::Artist).unwrap_or(Loc::Home),
            View::Import => Loc::Home,
            View::Playlists => self
                .open_playlist
                .clone()
                .map(Loc::Playlist)
                .unwrap_or(Loc::Home),
        }
    }

    fn push_history(&mut self) {
        self.now_open = false;
        let here = self.current_loc();
        self.history.push(here);
        if self.history.len() > 40 {
            self.history.remove(0);
        }
    }

    fn go_back(&mut self) {
        match self.history.pop() {
            Some(Loc::Search) => {
                self.view = View::Search;
                self.open_playlist = None;
            }
            Some(Loc::Artist(id)) => {
                self.view = View::Artist;
                self.current_artist = Some(id);
                self.open_playlist = None;
            }
            Some(Loc::Playlist(open)) => {
                self.request_collection(&open.uuid);
                self.open_playlist = Some(open);
                self.view = View::Playlists;
            }
            Some(Loc::Home) | None => self.go_home(),
        }
    }

    fn go_home(&mut self) {
        self.now_open = false;
        self.history.clear();
        self.view = View::Playlists;
        self.open_playlist = None;
    }

    fn open_artist(&mut self, id: u64, hint: Option<Artist>) {
        if self.view == View::Artist && self.current_artist == Some(id) {
            return;
        }
        self.push_history();
        self.open_playlist = None;
        self.view = View::Artist;
        self.current_artist = Some(id);
        self.artist_all_tracks = false;
        self.artist_pages.entry(id).or_insert_with(|| ArtistPage {
            artist: hint.unwrap_or(Artist { id, ..Default::default() }),
            ..Default::default()
        });
        self.net.send(Cmd::LoadArtist { id });
    }

    /// Open a Spotify or TIDAL link (pasted, passed on the command line or
    /// handed over by a second instance).
    fn open_external(&mut self, text: &str) {
        let ctx = self.egui_ctx.clone();
        self.show_window(&ctx);
        let Some(link) = link::parse(text) else { return };
        if link.service == link::Service::Spotify && link.kind == link::Kind::Playlist {
            self.start_import(link.id);
            return;
        }
        if self.user.is_none() {
            self.toast("Please sign in to TIDAL first".into());
            return;
        }
        self.toast("Opening link…".into());
        self.net.send(Cmd::OpenLink { link, show: false });
    }

    /// Copy a link for `tidal_url` on the chosen service (via linkhop for non-TIDAL).
    /// Read a public Spotify playlist and show the import preview.
    fn start_import(&mut self, id: String) {
        if self.user.is_none() {
            self.toast("Please sign in to TIDAL first".into());
            return;
        }
        self.push_history();
        self.import_job += 1;
        self.import = Some(ImportState {
            job: self.import_job,
            cover: None,
            items: Vec::new(),
            loaded: false,
            title: String::new(),
        });
        self.view = View::Import;
        self.search_loading = false;
        self.net.send(Cmd::ImportSpotify { id, job: self.import_job });
    }

    fn share_url(&mut self, tidal_url: String, target: ShareTarget) {
        let target = match target {
            ShareTarget::Tidal => {
                self.egui_ctx.copy_text(tidal_url);
                self.toast("TIDAL link copied".into());
                return;
            }
            ShareTarget::Spotify => "spotify",
            ShareTarget::Youtube => "youtube_music",
            ShareTarget::All => "",
        };
        self.toast("Creating link…".into());
        self.net.send(Cmd::Share { url: tidal_url, target: target.to_string() });
    }

    // ----------------------------------------------------------- misc helpers

    /// Load covers that every view is about to need, before anything is drawn.
    fn prefetch_covers(&mut self) {
        let mut urls: Vec<String> = Vec::new();
        urls.extend(self.current.as_ref().and_then(|t| t.cover_url(COVER_NOW)));
        urls.extend(self.playlists.iter().filter_map(|p| p.cover_url()));
        for (_, list) in &self.home {
            urls.extend(list.iter().filter_map(|p| p.cover_url()));
        }
        if let Some(open) = &self.open_playlist {
            urls.extend(open.cover.clone());
            urls.extend(open.tracks.iter().take(60).filter_map(|t| t.cover_url(COVER_ROW)));
        }
        for url in urls {
            if self.sync_budget == 0 {
                break;
            }
            self.request_cover(&url);
        }
    }

    /// The cover the window background is made from.
    fn wallpaper_url(&self) -> Option<String> {
        self.current.as_ref().and_then(|t| t.cover_url(COVER_NOW))
    }

    /// Blurred backdrop and palette for the current cover (only that one:
    /// every other cover would just waste memory).
    fn make_wallpaper(&mut self, url: &str, rgba: &[u8], width: usize, height: usize) {
        if self.blur_tex.contains_key(url) || self.wallpaper_url().as_deref() != Some(url) {
            return;
        }
        self.palettes.insert(url.to_string(), ui::palette_from_rgba(rgba, width, height));
        let thumb = ui::blur_thumb(rgba, width, height);
        let tex = self.egui_ctx.load_texture(format!("bg:{url}"), thumb, egui::TextureOptions::LINEAR);
        self.blur_tex.insert(url.to_string(), tex);
    }

    /// The cover may have been loaded before it became the current one: build
    /// its backdrop from the disk cache then.
    fn ensure_wallpaper(&mut self) {
        let Some(url) = self.wallpaper_url() else { return };
        if self.blur_tex.contains_key(&url) || !self.textures.contains_key(&url) || !self.wall_tried.insert(url.clone()) {
            return;
        }
        if let Some(img) = crate::net::cached_cover(&url).and_then(|b| image::load_from_memory(&b).ok()) {
            let rgba = img.to_rgba8();
            self.make_wallpaper(&url, rgba.as_raw(), rgba.width() as usize, rgba.height() as usize);
        }
    }

    fn request_cover(&mut self, url: &str) {
        if url.is_empty() || self.textures.contains_key(url) {
            return;
        }
        // Covers already on disk are decoded right away (a few per frame) so
        // they are there on the first frame instead of popping in later.
        if self.sync_budget > 0 && !self.requested.contains(url) {
            if let Some(bytes) = crate::net::cached_cover(url) {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    self.sync_budget -= 1;
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let pixels = rgba.as_raw();
                    self.make_wallpaper(url, pixels, size[0], size[1]);
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, pixels);
                    let texture =
                        self.egui_ctx.load_texture(url, image, egui::TextureOptions::LINEAR);
                    self.textures.insert(url.to_string(), texture);
                    self.requested.insert(url.to_string());
                    self.egui_ctx.request_repaint();
                    return;
                }
            }
        }
        if self.requested.insert(url.to_string()) {
            self.net.send(Cmd::Image(url.to_string()));
        }
    }

    fn toast(&mut self, message: String) {
        log::info!("{message}");
        self.toast = Some((message, Instant::now()));
    }

    /// Media keys, tray and close-to-tray.
    fn handle_system(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && self.cfg.close_to_tray
            && self.system.tray_active()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_window(ctx);
        }

        for event in self.system.poll() {
            match event {
                SysEvent::Play if !self.playing => self.toggle_play(),
                SysEvent::Pause | SysEvent::Stop if self.playing => self.toggle_play(),
                SysEvent::Play | SysEvent::Pause | SysEvent::Stop => {}
                SysEvent::Toggle => self.toggle_play(),
                SysEvent::Next => self.advance(false),
                SysEvent::Previous => self.previous(),
                SysEvent::SeekBy(secs) => self.seek_by(secs),
                SysEvent::SeekTo(pos) => {
                    if self.current.is_some() && !self.finished && self.duration > 0.0 {
                        self.seek(pos.clamp(0.0, (self.duration - 0.5).max(0.0)));
                    }
                }
                SysEvent::Show => self.show_window(ctx),
                SysEvent::ToggleWindow => {
                    if self.window_hidden || !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
                        self.show_window(ctx);
                    } else {
                        self.hide_window(ctx);
                    }
                }
                SysEvent::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // keep the desktop's media widgets in step with what plays
        let id = self.current.as_ref().map(|t| t.id);
        let playing = self.playing && !self.finished;
        if self.sys_sent != Some((id, playing)) || self.sys_dirty {
            if self.sys_sent.map(|(sent, _)| sent) != Some(id) {
                let now = self.current.as_ref().map(|t| NowPlaying {
                    title: t.title.clone(),
                    artist: t.artist_name(),
                    album: t.album_title(),
                    cover_url: t.cover_url(COVER_NOW).unwrap_or_default(),
                    duration: t.duration as f64,
                });
                self.system.set_track(now);
            }
            self.system.set_playback(playing, self.position);
            self.sys_sent = Some((id, playing));
            self.sys_dirty = false;
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.window_hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        self.window_hidden = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        // Wayland can't hide a window, only minimise it
        if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    fn touch(&mut self) {
        self.dirty = true;
    }

    fn maybe_save(&mut self) {
        if self.dirty && self.last_save.elapsed() > Duration::from_millis(900) {
            self.cfg.save();
            self.dirty = false;
            self.last_save = Instant::now();
        }
    }

    fn logout(&mut self) {
        self.cfg.access_token.clear();
        self.cfg.refresh_token.clear();
        self.cfg.user_id = None;
        self.cfg.user_name.clear();
        self.cfg.user_picture.clear();
        self.cfg.last_track = None;
        self.cfg.last_source = None;
        crate::cache::clear();
        self.home.clear();
        self.liked_ids.clear();
        self.liked_count = None;
        self.cfg.save();
        self.dirty = false;
        self.user = None;
        self.playlists.clear();
        self.open_playlist = None;
        self.results = None;
        self.queue.clear();
        self.current = None;
        self.playing = false;
        self.audio.send(AudioCmd::Stop);
        self.discord.send(DiscordCmd::Clear);
        self.toast("Signed out. Restart to sign in again.".into());
    }

    // ------------------------------------------------------------------ draw

    fn header(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();

        // left: home button on a frosted tile
        let mut left = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // as wide as the collapsed library below it
        let (tile, home) = left.allocate_exact_size(egui::vec2(LIBRARY_COLLAPSED, 44.0), egui::Sense::click());
        let at_home = self.view == View::Playlists && self.open_playlist.is_none() && !self.now_open;
        if self.user.is_some() {
            pointer_on_hover(&home);
            let color = if at_home { ACCENT } else { ui::TEXT };
            let family = if at_home { ui::fill_family() } else { egui::FontFamily::Proportional };
            top_tile(&left, tile, home.hovered(), icons::HOUSE, 21.0, color, family);
            if home.on_hover_text("Home").clicked() {
                self.go_home();
            }
        }

        if self.user.is_none() {
            return;
        }

        // centre: search
        let width = (rect.width() * 0.42).clamp(300.0, 520.0);
        let center = egui::Rect::from_center_size(rect.center(), egui::vec2(width, rect.height()));
        let mut mid = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(center)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        // One capsule drawn by hand: egui's own frame is switched off, so the
        // field never shows a second outline when it gets focus.
        let search_id = egui::Id::new("search_field");
        let field = egui::Rect::from_center_size(rect.center(), egui::vec2(width, 42.0));
        let focused = mid.ctx().memory(|m| m.has_focus(search_id));
        let hovered = mid.rect_contains_pointer(field);
        let border = if focused {
            egui::Stroke::new(1.5_f32, ACCENT)
        } else if hovered {
            egui::Stroke::new(1.0_f32, ui::with_alpha(ACCENT, 110))
        } else {
            egui::Stroke::new(1.0_f32, ui::EDGE)
        };
        let capsule = egui::CornerRadius::same(21);
        let back = mid.interact(field, search_id.with("capsule"), egui::Sense::click());
        mid.painter().rect_filled(field, capsule, if hovered || focused { CARD_HOVER } else { CARD });
        mid.painter().rect_stroke(field, capsule, border, egui::StrokeKind::Inside);

        mid.painter().text(
            egui::pos2(field.left() + 20.0, field.center().y),
            egui::Align2::LEFT_CENTER,
            icons::MAGNIFYING_GLASS,
            egui::FontId::proportional(17.0),
            if focused { ACCENT } else { TEXT_DIM },
        );

        let inner = field.shrink2(egui::vec2(44.0, 0.0));
        let resp = mid.put(
            inner,
            egui::TextEdit::singleline(&mut self.search_query)
                .id(search_id)
                .frame(false)
                .hint_text(RichText::new("What do you want to listen to?").color(TEXT_DIM))
                .horizontal_align(egui::Align::Center)
                .vertical_align(egui::Align::Center)
                .margin(egui::vec2(0.0, 0.0))
                .desired_width(inner.width()),
        );
        if back.clicked() {
            resp.request_focus();
        }
        // clear button
        if !self.search_query.is_empty() {
            let x_rect = egui::Rect::from_center_size(
                egui::pos2(field.right() - 22.0, field.center().y),
                egui::vec2(26.0, 26.0),
            );
            let x = mid.interact(x_rect, search_id.with("clear"), egui::Sense::click());
            pointer_on_hover(&x);
            if x.hovered() {
                mid.painter().circle_filled(x_rect.center(), 12.0, LINE);
            }
            mid.painter().text(
                x_rect.center(),
                egui::Align2::CENTER_CENTER,
                icons::X,
                egui::FontId::proportional(15.0),
                if x.hovered() { ui::TEXT } else { TEXT_DIM },
            );
            if x.on_hover_text("Clear search").clicked() {
                self.search_query.clear();
                self.results = None;
                self.last_query.clear();
                self.search_pending = None;
                resp.request_focus();
            }
        }
        self.recent_dropdown(ui.ctx(), &resp, field);
        if resp.changed() {
            if link::parse(&self.search_query).is_some() {
                // a pasted link is looked up right away instead of waiting for a pause
                self.search_pending = None;
                self.perform_search(false);
            } else {
                self.view = View::Search;
                self.now_open = false;
                self.search_pending = Some(Instant::now());
            }
        }
        if resp.lost_focus() && mid.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.search_pending = None;
            self.perform_search(true);
            resp.request_focus();
        }

        // right: settings / account menu
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        let (tile, resp) = right.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
        pointer_on_hover(&resp);
        let open = resp.hovered() || self.profile_menu_open;
        let color = if self.profile_menu_open { ACCENT } else { ui::TEXT };
        top_tile(&right, tile, open, icons::GEAR, 21.0, color, egui::FontFamily::Proportional);
        if resp.on_hover_text("Settings").clicked() {
            self.profile_menu_open = !self.profile_menu_open;
        }
        self.avatar_rect = tile;
    }

    /// Spotify-style profile dropdown anchored below the avatar.
    fn profile_menu(&mut self, ctx: &egui::Context) {
        if !self.profile_menu_open || self.user.is_none() {
            return;
        }
        let user = self.user.as_ref().unwrap();
        let (name, country) = (user.name.clone(), user.country.clone());
        let mut open_settings = false;
        let mut logout = false;
        let area = egui::Area::new(egui::Id::new("profile_menu"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-ui::GAP, self.avatar_rect.bottom() + 8.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(ui::PANEL_SOLID)
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(210.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&name).strong().size(15.0));
                                ui.label(RichText::new(format!("Region {country}")).size(12.0).color(TEXT_DIM));
                            });
                        });
                        ui.add_space(4.0);
                        ui.separator();
                        if menu_item(ui, icons::GEAR, "Settings").clicked() {
                            open_settings = true;
                        }
                        if menu_item(ui, icons::SIGN_OUT, "Sign out").clicked() {
                            logout = true;
                        }
                    });
            });
        let clicked_away = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|p| {
                    !area.response.rect.contains(p) && !self.avatar_rect.contains(p)
                })
        });
        if open_settings {
            self.settings_open = true;
            self.profile_menu_open = false;
        } else if logout {
            self.profile_menu_open = false;
            self.logout();
        } else if clicked_away {
            self.profile_menu_open = false;
        }
    }

    /// Left rail: nothing but playlist covers, details on hover.
    /// Left card: liked songs first, then the user's own playlists. Collapsed
    /// it shows only the covers; the button on top slides the names in.
    fn library(&mut self, ui: &mut egui::Ui) {
        let t = self.sidebar_t;
        let mut want_create = false;
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let (icon, tip) = if self.cfg.sidebar_expanded {
                (icons::CARET_DOUBLE_LEFT, "Collapse library")
            } else {
                (icons::CARET_DOUBLE_RIGHT, "Expand library")
            };
            toggle = ui::square_icon_button(ui, icon, 56.0, 36.0).on_hover_text(tip).clicked();
            if t > 0.6 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let add = ui::square_icon_button(ui, icons::PLUS, 36.0, 36.0).on_hover_text("New playlist");
                    want_create = add.clicked();
                });
            }
        });
        if toggle {
            self.cfg.sidebar_expanded = !self.cfg.sidebar_expanded;
            self.touch();
        }
        ui.add_space(6.0);

        let all = self.all_playlists();
        let open_uuid = self.open_playlist.as_ref().map(|o| o.uuid.clone());
        let on_playlists = self.view == View::Playlists;
        let mut want = Vec::new();
        let mut act: Option<(Playlist, PlAction)> = None;
        egui::ScrollArea::vertical()
            .id_salt("library_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for pl in &all {
                    let url = pl.cover_url();
                    let selected = on_playlists && open_uuid.as_deref() == Some(pl.uuid.as_str());
                    let sub = match (pl.uuid == LIKED_UUID, pl.number_of_tracks) {
                        (true, Some(n)) => format!("Playlist · {n} tracks"),
                        (true, None) => "Playlist".to_string(),
                        (false, _) => format!("Playlist · {} tracks", pl.track_count()),
                    };
                    let st = self.card_state(&pl.uuid);
                    if let Some(a) = library_row(
                        ui,
                        &self.textures,
                        pl.uuid == LIKED_UUID,
                        url.as_deref(),
                        &pl.title,
                        &sub,
                        selected,
                        &st,
                        t,
                    ) {
                        act = Some((pl.clone(), a));
                    }
                    if let Some(u) = url {
                        want.push(u);
                    }
                }
                if self.playlists_loading && self.playlists.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.spinner();
                        ui.label(RichText::new("Loading playlists…").color(TEXT_DIM));
                    });
                }
            });
        self.request_all(want);
        if want_create {
            self.dialog = Some(Dialog::Create { text: String::new() });
        }
        if let Some((pl, mut action)) = act {
            // Collapsed sidebar: a click opens, a second click on the *same*
            // playlist plays. (egui's own double click ignores which widget
            // was hit, so quick clicks on two playlists would count.)
            if matches!(action, PlAction::Open) && self.sidebar_t < 0.5 {
                let again = self
                    .lib_click
                    .as_ref()
                    .is_some_and(|(uuid, at)| *uuid == pl.uuid && at.elapsed() < Duration::from_millis(400));
                if again {
                    action = PlAction::Play;
                    self.lib_click = None;
                } else {
                    self.lib_click = Some((pl.uuid.clone(), Instant::now()));
                }
            } else {
                self.lib_click = None;
            }
            self.apply_pl_action(pl, action);
        }
    }

    fn player_bar(&mut self, ui: &mut egui::Ui) {
        let current = self.current.clone();
        if let Some(url) = current.as_ref().and_then(|t| t.cover_url(COVER_NOW)) {
            self.request_cover(&url);
        }

        let mut want_toggle = false;
        let mut want_prev = false;
        let mut want_next = false;
        let mut want_shuffle = false;
        let mut want_repeat = false;
        let mut want_like: Option<(u64, bool)> = None;
        let mut want_artist: Option<Artist> = None;
        let want_queue: bool;
        let want_mute: bool;
        let mut want_now = false;
        let cover_rect: egui::Rect;
        let mut player_action: Option<(Track, TrackAction)> = None;
        let mut new_volume: Option<f32> = None;

        // three columns: now playing | transport above the seek bar | volume
        let area = ui.max_rect();
        let h = area.height();
        let mid_w = (area.width() * 0.42).clamp(280.0, 560.0);
        let mid_rect = egui::Rect::from_center_size(area.center(), egui::vec2(mid_w, h));
        let left_rect = egui::Rect::from_min_max(area.min, egui::pos2(mid_rect.left() - 16.0, area.bottom()));
        let right_rect = egui::Rect::from_min_max(egui::pos2(mid_rect.right() + 16.0, area.top()), area.max);
        let centered = egui::Layout::left_to_right(egui::Align::Center);
        let column = |ui: &mut egui::Ui, id: &str, rect: egui::Rect, layout: egui::Layout| {
            let mut child = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(rect).layout(layout));
            child.set_clip_rect(rect.expand(2.0).intersect(ui.clip_rect()));
            child
        };

        // left: cover, title, artists, quality
        let mut left = column(ui, "player_left", left_rect, centered);
        let now_resp = current
            .as_ref()
            .map(|_| left.interact(left_rect, egui::Id::new("now_playing"), egui::Sense::click()));
        {
            let ui = &mut left;
            let cover_url = current.as_ref().and_then(|t| t.cover_url(COVER_NOW));
            let cover_resp = cover(ui, &self.textures, cover_url.as_deref(), 56.0);
            cover_rect = cover_resp.rect;
            if current.is_some() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| cover_rect.contains(p)) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                ui.painter().rect_filled(cover_rect, 8.0, Color32::from_black_alpha(130));
                ui::paint_icon(
                    ui.painter(),
                    cover_rect.center(),
                    icons::ARROWS_OUT_SIMPLE,
                    22.0,
                    Color32::WHITE,
                    egui::FontFamily::Proportional,
                );
            }
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let title = current
                    .as_ref()
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "Nothing playing".to_string());
                ui.add(egui::Label::new(RichText::new(title).strong().size(15.0)).truncate());
                if let Some(track) = &current {
                    want_artist = artist_links(ui, &track.artist_list(), 13.0, TEXT_DIM);
                }
                let mut meta = Vec::new();
                match self.cfg.transition {
                    TransitionMode::Crossfade => meta.push(format!("CROSSFADE {:.1} s", self.cfg.crossfade_secs)),
                    TransitionMode::Off => {}
                }
                if !meta.is_empty() {
                    ui.add(egui::Label::new(RichText::new(meta.join("  ·  ")).size(10.5).color(ACCENT)).truncate());
                }
            });
        }
        if let Some(resp) = &now_resp {
            want_now = resp.clicked() && resp.interact_pointer_pos().is_some_and(|p| cover_rect.contains(p));
        }
        if let (Some(resp), Some(track)) = (now_resp, &current) {
            let ctx = TrackCtx { liked: &self.liked_ids, playlists: &self.playlists, playing: self.playing, bars: self.viz };
            let liked = self.liked_ids.contains(&track.id);
            resp.context_menu(|ui| track_menu(ui, track, liked, &ctx, &mut player_action));
        }

        // centre: transport buttons, seek bar underneath
        let mut mid = column(ui, "player_mid", mid_rect, egui::Layout::top_down(egui::Align::Center));
        {
            let ui = &mut mid;
            ui.spacing_mut().item_spacing.y = 2.0;
            let block = 42.0 + 2.0 + 20.0;
            ui.add_space(((h - block) * 0.5).max(0.0));
            ui.allocate_ui_with_layout(egui::vec2(214.0, 42.0), centered, |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let shuffle_tip = if self.cfg.shuffle { "Shuffle off" } else { "Shuffle on" };
                want_shuffle = icon_button(ui, Icon::Shuffle, 30.0, self.cfg.shuffle).on_hover_text(shuffle_tip).clicked();
                want_prev = icon_button(ui, Icon::Prev, 30.0, false)
                    .on_hover_text(format!("Previous (Ctrl + {})", icons::ARROW_LEFT))
                    .clicked();
                let play_tip = if self.playing { "Pause (Space)" } else { "Play (Space)" };
                want_toggle = play_button(ui, self.playing).on_hover_text(play_tip).clicked();
                want_next = icon_button(ui, Icon::Next, 30.0, false)
                    .on_hover_text(format!("Next (Ctrl + {})", icons::ARROW_RIGHT))
                    .clicked();
                let repeat_tip = match self.cfg.repeat {
                    RepeatMode::Off => "Repeat: off",
                    RepeatMode::All => "Repeat: all",
                    RepeatMode::One => "Repeat: this track",
                };
                want_repeat = icon_button(
                    ui,
                    Icon::Repeat(self.cfg.repeat == RepeatMode::One),
                    30.0,
                    self.cfg.repeat != RepeatMode::Off,
                )
                .on_hover_text(repeat_tip)
                .clicked();
            });
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 20.0), centered, |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let time_w = 40.0;
                let shown = if self.scrubbing { self.scrub } else { self.position };
                let time = |ui: &mut egui::Ui, secs: f64, align: egui::Align| {
                    ui.allocate_ui_with_layout(egui::vec2(time_w, 20.0), egui::Layout::left_to_right(egui::Align::Center).with_main_align(align), |ui| {
                        ui.label(RichText::new(format_duration(secs)).size(11.5).color(TEXT_DIM));
                    });
                };
                time(ui, shown, egui::Align::Max);
                let width = (ui.available_width() - time_w - 8.0).max(60.0);
                let mut t = if self.duration > 0.0 { (shown / self.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
                let resp = thin_slider(ui, &mut t, width);
                if resp.drag_started() {
                    self.scrubbing = true;
                }
                if resp.changed() {
                    self.scrub = t as f64 * self.duration;
                }
                if resp.drag_stopped() {
                    self.scrubbing = false;
                    self.seek(self.scrub.clamp(0.0, self.duration.max(0.0)));
                } else if resp.clicked() {
                    self.seek(self.scrub.clamp(0.0, self.duration.max(0.0)));
                }
                time(ui, self.duration, egui::Align::Min);
            });
        }

        // right: like, volume, queue
        let mut right = column(ui, "player_right", right_rect, egui::Layout::right_to_left(egui::Align::Center));
        {
            let ui = &mut right;
            ui.spacing_mut().item_spacing.x = 6.0;
            want_queue = icon_button(ui, Icon::Queue, 30.0, self.queue_open).on_hover_text("Queue").clicked();
            ui.add_space(4.0);
            let mut vol = self.cfg.volume;
            let slider_w = (ui.available_width() - 80.0).clamp(50.0, 120.0);
            let slider = thin_slider(ui, &mut vol, slider_w)
                .on_hover_text(format!("{} %", (self.cfg.volume * 100.0).round() as i32));
            if slider.changed() {
                new_volume = Some(vol);
            }
            // mouse wheel over the slider changes the volume too
            if slider.hovered() {
                let dy = ui.input(|i| i.raw_scroll_delta.y);
                if dy != 0.0 {
                    new_volume = Some((vol + dy.signum() * 0.03).clamp(0.0, 1.0));
                }
            }
            let speaker = match self.cfg.volume {
                v if v <= 0.0 => icons::SPEAKER_X,
                v if v < 0.5 => icons::SPEAKER_LOW,
                _ => icons::SPEAKER_HIGH,
            };
            let mute_tip = if self.cfg.volume <= 0.0 { "Unmute (M)" } else { "Mute (M)" };
            let speaker = ui
                .add(egui::Label::new(RichText::new(speaker).size(18.0).color(TEXT_DIM)).sense(egui::Sense::click()))
                .on_hover_text(mute_tip);
            pointer_on_hover(&speaker);
            want_mute = speaker.clicked();
            ui.add_space(6.0);
            let cur_id = current.as_ref().map(|t| t.id);
            let liked = cur_id.is_some_and(|id| self.liked_ids.contains(&id));
            if ui::heart_button(ui, liked, 18.0, cur_id.is_some()).clicked() {
                want_like = cur_id.map(|id| (id, !liked));
            }
        }

        if want_toggle {
            self.toggle_play();
        }
        if want_prev {
            self.previous();
        }
        if want_next {
            self.advance(false);
        }
        if want_shuffle {
            self.cfg.shuffle = !self.cfg.shuffle;
            self.shuffle_changed();
        }
        if want_repeat {
            self.cycle_repeat();
        }
        if let Some(vol) = new_volume {
            self.set_volume(vol);
        }
        if let Some((id, liked)) = want_like {
            self.set_liked(id, liked);
        }
        if want_queue {
            self.queue_open = !self.queue_open;
        }
        if want_mute {
            self.toggle_mute();
        }
        if want_now {
            self.now_open = !self.now_open;
        }
        if let Some((track, action)) = player_action {
            self.apply_track_action(track, action);
        }
        if let Some(artist) = want_artist {
            self.open_artist(artist.id, Some(artist));
        }
    }

    // ------------------------------------------------------------- view: search

    /// Recent searches drop down under the search field while it is focused
    /// and empty. It stays open while the pointer moves into it.
    fn recent_dropdown(&mut self, ctx: &egui::Context, field_resp: &egui::Response, field: egui::Rect) {
        if field_resp.has_focus() && self.search_query.is_empty() {
            self.search_popup = true;
        }
        let pressed_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !field.contains(p) && !self.search_popup_rect.contains(p))
        });
        if !self.search_query.is_empty() || pressed_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search_popup = false;
        }
        if !self.search_popup || self.cfg.recent_searches.is_empty() {
            self.search_popup_rect = egui::Rect::NOTHING;
            return;
        }

        let recent = self.cfg.recent_searches.clone();
        let mut pick: Option<String> = None;
        let mut remove: Option<String> = None;
        let mut clear = false;
        let area = egui::Area::new(egui::Id::new("recent_searches"))
            .order(egui::Order::Foreground)
            .fixed_pos(field.left_bottom() + egui::vec2(0.0, 8.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(ui::PANEL_SOLID)
                    .stroke(egui::Stroke::new(1.0_f32, ui::EDGE))
                    .corner_radius(egui::CornerRadius::same(16))
                    .inner_margin(egui::Margin::same(8))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(field.width() - 16.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(RichText::new("Recent searches").size(12.0).color(TEXT_DIM));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.add_space(8.0);
                                let all = ui.add(
                                    egui::Label::new(RichText::new("Clear all").size(12.0).color(TEXT_DIM))
                                        .sense(egui::Sense::click()),
                                );
                                pointer_on_hover(&all);
                                clear = all.clicked();
                            });
                        });
                        ui.add_space(4.0);
                        for q in recent.iter().take(8) {
                            let (row, resp) =
                                ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
                            pointer_on_hover(&resp);
                            let hovered = ui.rect_contains_pointer(row);
                            if hovered {
                                ui.painter().rect_filled(row, 10.0, CARD_HOVER);
                            }
                            let mid = row.center().y;
                            ui::paint_icon(
                                ui.painter(),
                                egui::pos2(row.left() + 20.0, mid),
                                icons::CLOCK_COUNTER_CLOCKWISE,
                                16.0,
                                TEXT_DIM,
                                egui::FontFamily::Proportional,
                            );
                            let text = ui.painter().layout_no_wrap(
                                ellipsize(q, 48),
                                egui::FontId::proportional(14.0),
                                ui::TEXT,
                            );
                            ui.painter().galley(egui::pos2(row.left() + 40.0, mid - text.size().y * 0.5), text, ui::TEXT);
                            let x_rect = egui::Rect::from_center_size(egui::pos2(row.right() - 18.0, mid), egui::vec2(26.0, 26.0));
                            let x = ui.interact(x_rect, resp.id.with("remove"), egui::Sense::click());
                            if hovered || x.hovered() {
                                pointer_on_hover(&x);
                                let color = if x.hovered() { ui::TEXT } else { TEXT_DIM };
                                ui::paint_icon(ui.painter(), x_rect.center(), icons::X, 14.0, color, egui::FontFamily::Proportional);
                            }
                            if x.clicked() {
                                remove = Some(q.clone());
                            } else if resp.clicked() {
                                pick = Some(q.clone());
                            }
                        }
                    });
            });
        self.search_popup_rect = area.response.rect;

        if clear {
            self.cfg.recent_searches.clear();
            self.search_popup = false;
            self.touch();
        }
        if let Some(q) = remove {
            self.cfg.recent_searches.retain(|r| r != &q);
            self.touch();
        }
        if let Some(q) = pick {
            self.search_popup = false;
            self.search_query = q;
            self.perform_search(true);
        }
    }

    fn perform_search(&mut self, remember: bool) {
        let query = self.search_query.trim().to_string();
        if query.is_empty() {
            self.results = None;
            self.search_loading = false;
            self.last_query.clear();
            return;
        }
        self.search_loading = true;
        self.view = View::Search;
        self.now_open = false;
        if let Some(link) = link::parse(&query) {
            if link.service == link::Service::Spotify && link.kind == link::Kind::Playlist {
                self.last_query = query;
                self.start_import(link.id);
                return;
            }
            // a pasted Spotify/TIDAL/YouTube/linkhop link: look up the song it points to
            self.last_query = query;
            self.net.send(Cmd::OpenLink { link, show: true });
            return;
        }
        if remember {
            self.cfg.recent_searches.retain(|q| q != &query);
            self.cfg.recent_searches.insert(0, query.clone());
            self.cfg.recent_searches.truncate(10);
            self.dirty = true;
        }
        self.last_query = query.clone();
        self.net.send(Cmd::Search(query));
    }

    /// Search-as-you-type: fire once the user paused typing.
    fn auto_search(&mut self, ctx: &egui::Context) {
        let Some(at) = self.search_pending else { return };
        let wait = Duration::from_millis(350);
        if at.elapsed() < wait {
            ctx.request_repaint_after(wait - at.elapsed());
            return;
        }
        self.search_pending = None;
        if self.search_query.trim() != self.last_query {
            self.perform_search(false);
        }
    }

    fn search_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Search");
        ui.add_space(4.0);

        if self.search_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Searching…");
            });
        }

        if self.results.is_none() && !self.search_loading {
            ui.add_space(24.0);
            ui.label(
                RichText::new("Search for tracks, albums, artists or playlists – or paste a TIDAL, Spotify, YouTube or linkhop link.")
                    .color(TEXT_DIM),
            );
        }

        let Some(results) = self.results.clone() else {
            return;
        };
        if results.is_empty() {
            ui.add_space(16.0);
            ui.label(RichText::new("No results.").color(TEXT_DIM));
            return;
        }

        // tabs
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (tab, label, count) in [
                (SearchTab::All, "All", usize::MAX),
                (SearchTab::Tracks, "Tracks", results.tracks.len()),
                (SearchTab::Albums, "Albums", results.albums.len()),
                (SearchTab::Artists, "Artists", results.artists.len()),
                (SearchTab::Playlists, "Playlists", results.playlists.len()),
            ] {
                if count == 0 {
                    continue;
                }
                if pill(ui, label, self.search_tab == tab, 22.0 + label.chars().count() as f32 * 8.0)
                    .clicked()
                {
                    self.search_tab = tab;
                }
            }
        });
        ui.add_space(10.0);

        let tab = self.search_tab;
        let all = tab == SearchTab::All;
        let mut play_single: Option<Track> = None;
        let mut menu_action: Option<(Track, TrackAction)> = None;
        let mut act: Option<(Playlist, PlAction)> = None;
        let mut want = Vec::new();
        let current_id = self.current.as_ref().map(|t| t.id);

        if (all || tab == SearchTab::Tracks) && !results.tracks.is_empty() {
            ui.label(RichText::new("Tracks").strong().size(18.0));
            ui.add_space(4.0);
            let limit = if all { 5 } else { results.tracks.len() };
            for (i, track) in results.tracks.iter().take(limit).enumerate() {
                let is_current = current_id == Some(track.id);
                let ctx = TrackCtx { liked: &self.liked_ids, playlists: &self.playlists, playing: self.playing, bars: self.viz };
                let resp = track_row(ui, &self.textures, i + 1, track, is_current, &mut want, &ctx, &mut menu_action);
                if resp.clicked() {
                    play_single = Some(track.clone());
                }
            }
            ui.add_space(14.0);
        }

        if (all || tab == SearchTab::Albums) && !results.albums.is_empty() {
            ui.label(RichText::new("Albums").strong().size(18.0));
            ui.add_space(4.0);
            let limit = if all { 6 } else { results.albums.len() };
            ui.horizontal_wrapped(|ui| {
                for album in results.albums.iter().take(limit) {
                    let url = album.cover_url(320);
                    let sub = album.artist_names();
                    let pl = album.as_playlist();
                    let st = self.card_state(&pl.uuid);
                    if let Some(a) = media_card(ui, &self.textures, url.as_deref(), false, false, &album.title, &sub, &st) {
                        act = Some((pl, a));
                    }
                    if let Some(u) = url {
                        want.push(u);
                    }
                }
            });
            ui.add_space(14.0);
        }

        if (all || tab == SearchTab::Artists) && !results.artists.is_empty() {
            ui.label(RichText::new("Artists").strong().size(18.0));
            ui.add_space(4.0);
            let limit = if all { 6 } else { results.artists.len() };
            ui.horizontal_wrapped(|ui| {
                for artist in results.artists.iter().take(limit) {
                    let url = artist.picture_url(320);
                    let pl = Playlist {
                        uuid: format!("artist:{}", artist.id),
                        title: format!("{} · Top tracks", artist.name),
                        square_image: url.clone(),
                        creator: Some(crate::model::PlaylistCreator {
                            id: None,
                            name: Some(artist.name.clone()),
                        }),
                        ..Default::default()
                    };
                    let st = self.card_state(&pl.uuid);
                    if let Some(a) = media_card(ui, &self.textures, url.as_deref(), false, true, &artist.name, "Artists", &st) {
                        act = Some((pl, a));
                    }
                    if let Some(u) = url {
                        want.push(u);
                    }
                }
            });
            ui.add_space(14.0);
        }

        if (all || tab == SearchTab::Playlists) && !results.playlists.is_empty() {
            ui.label(RichText::new("Playlists").strong().size(18.0));
            ui.add_space(4.0);
            let limit = if all { 6 } else { results.playlists.len() };
            ui.horizontal_wrapped(|ui| {
                for pl in results.playlists.iter().take(limit) {
                    let url = pl.cover_url();
                    let sub = format!("{} tracks", pl.track_count());
                    let st = self.card_state(&pl.uuid);
                    if let Some(a) = media_card(ui, &self.textures, url.as_deref(), false, false, &pl.title, &sub, &st) {
                        act = Some((pl.clone(), a));
                    }
                    if let Some(u) = url {
                        want.push(u);
                    }
                }
            });
        }

        self.request_all(want);
        if let Some((track, action)) = menu_action {
            self.apply_track_action(track, action);
        } else if let Some(track) = play_single {
            self.play_single(track);
        }
        if let Some((pl, action)) = act {
            self.apply_pl_action(pl, action);
        }
    }

    // ---------------------------------------------------------- view: library

    fn playlists_view(&mut self, ui: &mut egui::Ui) {
        if self.open_playlist.is_some() {
            self.open_playlist_view(ui);
            return;
        }

        ui.horizontal(|ui| {
            ui.heading("Home");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action_button(ui, icons::ARROW_CLOCKWISE, "Refresh", false).clicked() {
                    self.playlists_loading = true;
                    self.net.send(Cmd::LoadUser);
                }
            });
        });
        ui.add_space(6.0);

        let mine = ("Your playlists".to_string(), self.all_playlists());
        let mut sections: Vec<(String, Vec<Playlist>)> = Vec::new();
        let (mixes, rest): (Vec<_>, Vec<_>) = self
            .home
            .iter()
            .cloned()
            .partition(|(_, list)| list.first().is_some_and(|p| p.uuid.starts_with("mix:")));
        sections.extend(mixes);
        sections.push(mine);
        sections.extend(rest);

        let mut want = Vec::new();
        let mut act: Option<(Playlist, PlAction)> = None;
        for (title, list) in &sections {
            ui.add_space(8.0);
            ui.label(RichText::new(title).strong().size(20.0));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for pl in list {
                    let url = pl.cover_url();
                    let sub = match (&pl.description, &pl.creator) {
                        _ if pl.uuid == LIKED_UUID => "Your favorites".to_string(),
                        (Some(d), _) if pl.uuid.starts_with("mix:") => d.clone(),
                        (_, Some(c)) => c.name.clone().unwrap_or_default(),
                        _ => String::new(),
                    };
                    let st = self.card_state(&pl.uuid);
                    if let Some(a) = media_card(
                        ui,
                        &self.textures,
                        url.as_deref(),
                        pl.uuid == LIKED_UUID,
                        false,
                        &pl.title,
                        &sub,
                        &st,
                    ) {
                        act = Some((pl.clone(), a));
                    }
                    if let Some(u) = url {
                        want.push(u);
                    }
                }
            });
        }
        if self.playlists_loading && self.playlists.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading playlists…");
            });
        }
        self.request_all(want);
        if let Some((pl, action)) = act {
            self.apply_pl_action(pl, action);
        }
    }

    fn all_playlists(&self) -> Vec<Playlist> {
        let mut all = vec![Playlist {
            uuid: LIKED_UUID.to_string(),
            title: "Liked Songs".to_string(),
            number_of_tracks: self.liked_count.map(|n| n as u32),
            creator: Some(crate::model::PlaylistCreator {
                id: None,
                name: self.user.as_ref().map(|u| u.name.clone()),
            }),
            ..Default::default()
        }];
        all.extend(self.playlists.iter().cloned());
        all
    }

    fn request_collection(&self, uuid: &str) {
        if uuid == LIKED_UUID {
            self.net.send(Cmd::LoadFavorites);
        } else {
            self.net.send(Cmd::LoadPlaylist { uuid: uuid.to_string() });
        }
    }

    /// Start (or pause/resume) a whole playlist, album or mix.
    fn play_collection(&mut self, pl: &Playlist) {
        if self.playing_from.as_deref() == Some(pl.uuid.as_str()) {
            self.toggle_play();
            return;
        }
        if let Some(tracks) = crate::cache::read::<Vec<Track>>(&format!("tracks-{}", pl.uuid)) {
            if !tracks.is_empty() {
                self.play_tracks(tracks, 0, Some(pl.uuid.clone()));
                return;
            }
        }
        self.pending_play = Some(pl.uuid.clone());
        self.request_collection(&pl.uuid);
    }

    fn apply_pl_action(&mut self, pl: Playlist, action: PlAction) {
        match action {
            PlAction::Open => match pl.uuid.strip_prefix("artist:").and_then(|id| id.parse().ok()) {
                Some(id) => {
                    let name = pl.creator.and_then(|c| c.name).unwrap_or_default();
                    self.open_artist(id, Some(Artist { id, name, picture: None }));
                }
                None => self.open_playlist_from(pl),
            },
            PlAction::Share(target) => {
                if let Some((kind, id)) = pl.uuid.split_once(':') {
                    self.share_url(format!("https://tidal.com/{kind}/{id}"), target);
                }
            }
            PlAction::Play => self.play_collection(&pl),
            PlAction::Edit => {
                self.cover_preview = None;
                self.dialog = Some(Dialog::Edit {
                    uuid: pl.uuid,
                    title: pl.title,
                    description: pl.description.unwrap_or_default(),
                    cover: None,
                });
            }
            PlAction::Delete => {
                self.dialog = Some(Dialog::Delete { uuid: pl.uuid, title: pl.title });
            }
        }
    }

    fn dialog_ui(&mut self, ctx: &egui::Context) {
        // an image picked in the file dialog
        if let Some(path) = self.cover_pick.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.cover_pick = None;
            if let Some(Dialog::Edit { cover, .. }) = &mut self.dialog {
                self.cover_preview = std::fs::read(&path)
                    .ok()
                    .and_then(|bytes| image::load_from_memory(&bytes).ok())
                    .map(|img| {
                        let img = img.thumbnail(256, 256).to_rgba8();
                        let size = [img.width() as usize, img.height() as usize];
                        ctx.load_texture("cover_preview", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), Default::default())
                    });
                *cover = Some(path);
            }
        }
        let Some(dialog) = self.dialog.take() else { return };
        let frame = ui::card().fill(ui::PANEL_SOLID).inner_margin(egui::Margin::same(20));
        let mut keep: Option<Dialog> = None;
        let mut confirm: Option<Dialog> = None;
        let modal = egui::Modal::new(egui::Id::new("dialog_modal"))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(360.0);
                match dialog {
                    Dialog::Edit { uuid, mut title, mut description, cover } => {
                        ui.heading("Edit playlist");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            // cover: the chosen image, or a hint
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::click());
                            pointer_on_hover(&resp);
                            match &self.cover_preview {
                                Some(tex) if cover.is_some() => {
                                    egui::Image::new(egui::load::SizedTexture::new(tex.id(), rect.size()))
                                        .corner_radius(10.0)
                                        .paint_at(ui, rect);
                                }
                                _ => {
                                    ui.painter().rect_filled(rect, 10.0, CARD_HOVER);
                                    ui::paint_icon(ui.painter(), rect.center(), icons::IMAGE, 30.0, TEXT_DIM, egui::FontFamily::Proportional);
                                }
                            }
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, 10.0, Color32::from_black_alpha(90));
                                ui::paint_icon(ui.painter(), rect.center(), icons::PENCIL_SIMPLE, 24.0, Color32::WHITE, egui::FontFamily::Proportional);
                            }
                            let pick = resp.on_hover_text("Choose a cover image").clicked();
                            ui.vertical(|ui| {
                                ui.add_space(4.0);
                                ui.label(RichText::new("Cover").size(12.0).color(TEXT_DIM));
                                let name = cover
                                    .as_ref()
                                    .and_then(|p| p.file_name())
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "unchanged".to_string());
                                ui.label(RichText::new(ellipsize(&name, 28)).size(13.0));
                                if ui.button("Choose image…").clicked() || pick {
                                    if self.cover_pick.is_none() {
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        self.cover_pick = Some(rx);
                                        let ctx = ui.ctx().clone();
                                        std::thread::spawn(move || {
                                            let picked = rfd::FileDialog::new()
                                                .add_filter("Images (PNG, JPEG)", &["png", "jpg", "jpeg"])
                                                .set_title("Choose a playlist cover")
                                                .pick_file();
                                            if let Some(path) = picked {
                                                let _ = tx.send(path);
                                            }
                                            ctx.request_repaint();
                                        });
                                    }
                                }
                            });
                        });
                        ui.add_space(8.0);
                        ui.label(RichText::new("Title").size(12.0).color(TEXT_DIM));
                        ui.add(
                            egui::TextEdit::singleline(&mut title)
                                .margin(egui::vec2(10.0, 8.0))
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(6.0);
                        ui.label(RichText::new("Description").size(12.0).color(TEXT_DIM));
                        ui.add(
                            egui::TextEdit::multiline(&mut description)
                                .hint_text("What is this playlist about?")
                                .margin(egui::vec2(10.0, 8.0))
                                .desired_rows(4)
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(10.0);
                        let ok = !title.trim().is_empty();
                        let (mut done, mut cancel) = (false, false);
                        ui.horizontal(|ui| {
                            done = ui.add_enabled(ok, egui::Button::new("Save")).clicked();
                            cancel = ui.button("Cancel").clicked();
                        });
                        if done && ok {
                            confirm = Some(Dialog::Edit { uuid, title, description, cover });
                        } else if !cancel {
                            keep = Some(Dialog::Edit { uuid, title, description, cover });
                        }
                    }
                    Dialog::Create { mut text } => {
                        ui.heading("New playlist");
                        ui.add_space(8.0);
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut text)
                                .hint_text("Playlist name")
                                .margin(egui::vec2(10.0, 8.0))
                                .desired_width(f32::INFINITY),
                        );
                        if !resp.has_focus() && !resp.lost_focus() {
                            resp.request_focus();
                        }
                        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.add_space(10.0);
                        let ok = !text.trim().is_empty();
                        let (mut done, mut cancel) = (false, false);
                        ui.horizontal(|ui| {
                            done = ui.add_enabled(ok, egui::Button::new("Create")).clicked();
                            cancel = ui.button("Cancel").clicked();
                        });
                        if (done || enter) && ok {
                            confirm = Some(Dialog::Create { text });
                        } else if !cancel {
                            keep = Some(Dialog::Create { text });
                        }
                    }
                    Dialog::Delete { uuid, title } => {
                        ui.heading("Delete playlist?");
                        ui.add_space(8.0);
                        ui.label(format!("“{title}” will be deleted permanently."));
                        ui.add_space(10.0);
                        let (mut done, mut cancel) = (false, false);
                        ui.horizontal(|ui| {
                            done = ui
                                .add(egui::Button::new(RichText::new("Delete").color(ui::DANGER)))
                                .clicked();
                            cancel = ui.button("Cancel").clicked();
                        });
                        if done {
                            confirm = Some(Dialog::Delete { uuid, title });
                        } else if !cancel {
                            keep = Some(Dialog::Delete { uuid, title });
                        }
                    }
                }
            });
        if modal.should_close() {
            keep = None;
        }
        self.dialog = keep;
        match confirm {
            Some(Dialog::Edit { uuid, title, description, cover }) => {
                let title = title.trim().to_string();
                let description = description.trim().to_string();
                for pl in &mut self.playlists {
                    if pl.uuid == uuid {
                        pl.title = title.clone();
                        pl.description = Some(description.clone());
                    }
                }
                if let Some(open) = &mut self.open_playlist {
                    if open.uuid == uuid {
                        open.title = title.clone();
                        open.description = description.clone();
                    }
                }
                self.cover_preview = None;
                self.net.send(Cmd::EditPlaylist { uuid, title, description, cover });
            }
            Some(Dialog::Delete { uuid, .. }) => {
                self.playlists.retain(|p| p.uuid != uuid);
                if self.open_playlist.as_ref().is_some_and(|o| o.uuid == uuid) {
                    self.open_playlist = None;
                }
                if self.playing_from.as_deref() == Some(uuid.as_str()) {
                    self.playing_from = None;
                }
                self.net.send(Cmd::DeletePlaylist { uuid });
            }
            Some(Dialog::Create { text }) => {
                self.net.send(Cmd::CreatePlaylist { title: text.trim().to_string() });
            }
            None => {}
        }
    }

    /// Per-card state for the playing indicator and the right-click menu.
    fn card_state(&self, uuid: &str) -> CardState {
        CardState {
            playing_here: self.playing_from.as_deref() == Some(uuid),
            playing: self.playing,
            own: self.playlists.iter().any(|p| p.uuid == uuid),
            shareable: uuid.starts_with("album:") || uuid.starts_with("artist:"),
            bars: self.viz,
        }
    }

    fn open_playlist_from(&mut self, pl: Playlist) {
        let cached = crate::cache::read::<Vec<Track>>(&format!("tracks-{}", pl.uuid));
        let same = self.view == View::Playlists
            && self.open_playlist.as_ref().is_some_and(|o| o.uuid == pl.uuid);
        if !same {
            self.push_history();
        }
        self.view = View::Playlists;
        let creator = pl
            .creator
            .as_ref()
            .and_then(|c| c.name.clone())
            .unwrap_or_default();
        self.open_playlist = Some(OpenPlaylist {
            uuid: pl.uuid.clone(),
            title: pl.title.clone(),
            creator,
            description: pl.description.clone().unwrap_or_default(),
            cover: pl.cover_url(),
            tracks: cached.clone().unwrap_or_default(),
            loading: cached.is_none(),
        });
        self.request_collection(&pl.uuid);
    }

    fn open_playlist_view(&mut self, ui: &mut egui::Ui) {
        let Some(open) = self.open_playlist.clone() else {
            return;
        };

        ui.horizontal(|ui| {
            if action_button(ui, icons::ARROW_LEFT, "Back", false).clicked() {
                self.go_back();
            }
        });
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if open.uuid == LIKED_UUID {
                liked_art(ui, 140.0);
            } else {
                cover(ui, &self.textures, open.cover.as_deref(), 140.0);
            }
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(&open.title).size(24.0).strong());
                if !open.creator.is_empty() {
                    ui.label(RichText::new(&open.creator).weak());
                }
                if !open.description.is_empty() {
                    ui.add_space(4.0);
                    ui.set_max_width(ui.available_width().min(620.0));
                    ui.add(egui::Label::new(RichText::new(&open.description).color(TEXT_DIM).size(13.0)).wrap());
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} tracks", open.tracks.len()))
                        .weak()
                        .size(12.0),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if !open.tracks.is_empty() && action_button(ui, icons::PLAY, "Play all", true).clicked() {
                        self.play_tracks(open.tracks.clone(), 0, Some(open.uuid.clone()));
                    }
                    if self.card_state(&open.uuid).own && action_button(ui, icons::PENCIL_SIMPLE, "Edit details", false).clicked() {
                        self.cover_preview = None;
                        self.dialog = Some(Dialog::Edit {
                            uuid: open.uuid.clone(),
                            title: open.title.clone(),
                            description: open.description.clone(),
                            cover: None,
                        });
                    }
                });
            });
        });
        ui.add_space(8.0);
        ui.separator();

        if open.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading tracks…");
            });
            return;
        }
        if open.tracks.is_empty() {
            ui.label(RichText::new("This playlist is empty.").weak());
            return;
        }

        if let Some(url) = &open.cover {
            self.request_cover(url);
        }
        let current_id = self.current.as_ref().map(|t| t.id);
        let mut play: Option<usize> = None;
        let mut menu_action: Option<(Track, TrackAction)> = None;
        let mut want = Vec::new();
        for (i, track) in open.tracks.iter().enumerate() {
            let is_current = current_id == Some(track.id);
            let ctx = TrackCtx { liked: &self.liked_ids, playlists: &self.playlists, playing: self.playing, bars: self.viz };
            let resp = track_row(ui, &self.textures, i + 1, track, is_current, &mut want, &ctx, &mut menu_action);
            if resp.clicked() {
                play = Some(i);
            }
        }
        self.request_all(want);
        if let Some((track, action)) = menu_action {
            self.apply_track_action(track, action);
        } else if let Some(index) = play {
            let source = Some(open.uuid.clone());
            self.play_tracks(open.tracks, index, source);
        }
    }

    // ----------------------------------------------------------- view: import

    fn import_view(&mut self, ui: &mut egui::Ui) {
        let Some(mut st) = self.import.take() else { return };
        if action_button(ui, icons::ARROW_LEFT, "Back", false).clicked() {
            self.go_back();
            return;
        }
        ui.add_space(10.0);
        if !st.loaded {
            self.import = Some(st);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Reading Spotify playlist…").color(TEXT_DIM));
            });
            return;
        }

        let total = st.items.len();
        let done = st.items.iter().filter(|i| i.checked).count();
        let found = st.items.iter().filter(|i| i.found.is_some()).count();
        let unsure = st.items.iter().filter(|i| i.found.is_some() && !i.sure).count();
        let missing = done - st.items.iter().filter(|i| i.checked && i.found.is_some()).count();
        let selected = st.items.iter().filter(|i| i.selected).count();

        let mut want = Vec::new();
        if let Some(url) = &st.cover {
            self.request_cover(url);
        }
        let mut create = false;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(130.0, 130.0), egui::Sense::hover());
            paint_cover_r(ui, &self.textures, st.cover.as_deref(), rect, 10.0);
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("IMPORT SPOTIFY PLAYLIST").size(11.5).strong().color(ACCENT));
                ui.add_space(2.0);
                ui.add(
                    egui::TextEdit::singleline(&mut st.title)
                        .font(egui::FontId::proportional(22.0))
                        .margin(egui::vec2(10.0, 6.0))
                        .desired_width(380.0),
                );
                ui.add_space(4.0);
                if done < total {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_width(380.0)
                            .fill(ACCENT),
                    );
                    ui.label(
                        RichText::new(format!("{done} / {total} tracks matched"))
                            .size(12.5)
                            .color(TEXT_DIM),
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "{found} of {total} matched · {unsure} unsure · {missing} not found"
                        ))
                        .color(TEXT_DIM),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ready = done == total && selected > 0 && !st.title.trim().is_empty();
                    ui.add_enabled_ui(ready, |ui| {
                        if action_button(ui, icons::PLUS, &format!("Create as TIDAL playlist ({selected})"), true)
                            .clicked()
                        {
                            create = true;
                        }
                    });
                    if unsure > 0
                        && action_button(ui, icons::X, "Deselect uncertain", false).clicked()
                    {
                        for item in st.items.iter_mut().filter(|i| !i.sure) {
                            item.selected = false;
                        }
                    }
                });
            });
        });
        if total >= 100 {
            ui.add_space(6.0);
            ui.label(
                RichText::new("Spotify returns at most 100 tracks – the playlist may be longer.")
                    .size(12.5)
                    .color(ui::WARN),
            );
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        for item in &mut st.items {
            if import_row(ui, &self.textures, item, &mut want) {
                item.selected = !item.selected;
            }
        }
        self.request_all(want);

        if create {
            let track_ids = st
                .items
                .iter()
                .filter(|i| i.selected)
                .filter_map(|i| i.found.as_ref().map(|t| t.id))
                .collect();
            self.net.send(Cmd::ImportCreate { title: st.title.trim().to_string(), track_ids });
            self.toast("Creating playlist…".into());
            self.go_home();
            return;
        }
        self.import = Some(st);
    }

    // ------------------------------------------------------------ view: artist

    fn artist_view(&mut self, ui: &mut egui::Ui) {
        let Some(page) = self.current_artist.and_then(|id| self.artist_pages.get(&id)).cloned() else {
            return;
        };
        let artist = &page.artist;

        if action_button(ui, icons::ARROW_LEFT, "Back", false).clicked() {
            self.go_back();
        }
        ui.add_space(10.0);

        // header
        let pic = artist.picture_url(320);
        if let Some(url) = &pic {
            self.request_cover(url);
        }
        let mut play_top = false;
        let mut share = false;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(150.0, 150.0), egui::Sense::hover());
            paint_cover_r(ui, &self.textures, pic.as_deref(), rect, 75.0);
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.add_space(26.0);
                ui.label(RichText::new("ARTIST").size(11.5).strong().color(ACCENT));
                ui.label(RichText::new(&artist.name).size(34.0).strong());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if !page.top_tracks.is_empty()
                        && action_button(ui, icons::PLAY, "Play", true).clicked()
                    {
                        play_top = true;
                    }
                    if action_button(ui, icons::SHARE_NETWORK, "Share", false).clicked() {
                        share = true;
                    }
                });
            });
        });
        ui.add_space(10.0);

        if page.top_tracks.is_empty() && !page.done {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading…").color(TEXT_DIM));
            });
        }

        let mut want = Vec::new();
        let mut play: Option<usize> = None;
        let mut menu_action: Option<(Track, TrackAction)> = None;
        let mut act: Option<(Playlist, PlAction)> = None;
        let current_id = self.current.as_ref().map(|t| t.id);

        if !page.top_tracks.is_empty() {
            section_title(ui, "Popular tracks");
            let limit = if self.artist_all_tracks { page.top_tracks.len() } else { 5 };
            for (i, track) in page.top_tracks.iter().take(limit).enumerate() {
                let ctx = TrackCtx { liked: &self.liked_ids, playlists: &self.playlists, playing: self.playing, bars: self.viz };
                let resp = track_row(
                    ui,
                    &self.textures,
                    i + 1,
                    track,
                    current_id == Some(track.id),
                    &mut want,
                    &ctx,
                    &mut menu_action,
                );
                if resp.clicked() {
                    play = Some(i);
                }
            }
            if page.top_tracks.len() > 5 {
                ui.add_space(4.0);
                let (icon, label) = if self.artist_all_tracks {
                    (icons::CARET_RIGHT, "Show less")
                } else {
                    (icons::CARET_RIGHT, "Show all")
                };
                if action_button(ui, icon, label, false).clicked() {
                    self.artist_all_tracks = !self.artist_all_tracks;
                }
            }
        }

        for (title, list) in [
            ("Albums", &page.albums),
            ("EPs", &page.eps),
            ("Singles", &page.singles),
            ("Compilations", &page.compilations),
        ] {
            if list.is_empty() {
                continue;
            }
            section_title(ui, title);
            ui.horizontal_wrapped(|ui| {
                for album in list {
                    let url = album.cover_url(320);
                    let sub = match album.year() {
                        Some(year) => format!("{year} · {}", album_kind(album)),
                        None => album_kind(album).to_string(),
                    };
                    let pl = album.as_playlist();
                    let st = self.card_state(&pl.uuid);
                    if let Some(a) = media_card(ui, &self.textures, url.as_deref(), false, false, &album.title, &sub, &st) {
                        act = Some((pl, a));
                    }
                    if let Some(u) = url {
                        want.push(u);
                    }
                }
            });
        }

        if page.done
            && page.top_tracks.is_empty()
            && page.albums.is_empty()
            && page.eps.is_empty()
            && page.singles.is_empty()
            && page.compilations.is_empty()
        {
            ui.label(RichText::new("No releases found.").color(TEXT_DIM));
        }

        self.request_all(want);
        if play_top {
            self.play_tracks(page.top_tracks.clone(), 0, Some(format!("artist:{}", artist.id)));
        }
        if share {
            self.share_url(format!("https://tidal.com/artist/{}", artist.id), ShareTarget::All);
        }
        if let Some((track, action)) = menu_action {
            self.apply_track_action(track, action);
        } else if let Some(index) = play {
            self.play_tracks(page.top_tracks.clone(), index, Some(format!("artist:{}", artist.id)));
        }
        if let Some((pl, action)) = act {
            self.apply_pl_action(pl, action);
        }
    }

    // -------------------------------------------------- full-screen player

    fn now_playing_view(&mut self, ui: &mut egui::Ui) {
        let Some(track) = self.current.clone() else { return };
        let mut close = false;

        // top bar: close on the left, tabs on the right
        ui.horizontal(|ui| {
            if action_button(ui, icons::CARET_DOWN, "Close", false).clicked() {
                close = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (tab, label) in [(NowTab::Credits, "Credits"), (NowTab::Lyrics, "Lyrics")] {
                    if pill(ui, label, self.now_tab == tab, 30.0 + label.chars().count() as f32 * 8.0).clicked() {
                        self.now_tab = tab;
                    }
                }
            });
        });
        ui.add_space(6.0);

        let area = ui.available_rect_before_wrap();
        let left_w = area.width() * 0.5;

        // cover, title and artists
        let size = (left_w - 40.0).min(area.height() - 110.0).clamp(180.0, 620.0);
        let block_h = size + 96.0;
        let top = area.top() + ((area.height() - block_h) * 0.5).max(0.0);
        let cover_rect = egui::Rect::from_min_size(
            egui::pos2(area.left() + (left_w - size) * 0.5, top),
            egui::vec2(size, size),
        );
        let big = track.cover_url(750);
        if let Some(url) = &big {
            self.request_cover(url);
        }
        let url = match &big {
            Some(u) if self.textures.contains_key(u) => Some(u.clone()),
            _ => track.cover_url(COVER_NOW),
        };
        ui.painter().rect_filled(
            cover_rect.translate(egui::vec2(0.0, 10.0)).expand(2.0),
            16.0,
            Color32::from_black_alpha(90),
        );
        paint_cover_r(ui, &self.textures, url.as_deref(), cover_rect, 14.0);

        let text_rect = egui::Rect::from_min_size(
            egui::pos2(cover_rect.left(), cover_rect.bottom() + 16.0),
            egui::vec2(size, 76.0),
        );
        let mut text = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(text_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        text.spacing_mut().item_spacing.y = 4.0;
        text.add(egui::Label::new(RichText::new(&track.title).size(28.0).strong()).truncate());
        let mut open_artist = artist_links(&mut text, &track.artist_list(), 17.0, TEXT_DIM);

        // right column: lyrics or credits
        let right = egui::Rect::from_min_max(
            egui::pos2(area.left() + left_w + 20.0, area.top()),
            area.max,
        );
        let mut side = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let mut seek_to = None;
        match self.now_tab {
            NowTab::Lyrics => seek_to = self.lyrics_panel(&mut side),
            NowTab::Credits => {
                if open_artist.is_none() {
                    open_artist = self.credits_panel(&mut side);
                } else {
                    self.credits_panel(&mut side);
                }
            }
        }
        ui.allocate_rect(area, egui::Sense::hover());

        if let Some(t) = seek_to {
            self.seek(t);
        }
        if close {
            self.now_open = false;
        }
        if let Some(artist) = open_artist {
            self.open_artist(artist.id, Some(artist));
        }
    }

    /// Lyrics with the currently sung line highlighted; returns a clicked line's time.
    fn lyrics_panel(&mut self, ui: &mut egui::Ui) -> Option<f64> {
        let Some(lyrics) = self.lyrics.clone() else {
            ui.add_space(40.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading lyrics…").color(TEXT_DIM));
            });
            return None;
        };
        if lyrics.is_empty() {
            ui.add_space(40.0);
            ui.label(RichText::new("No lyrics available for this track.").size(16.0).color(TEXT_DIM));
            return None;
        }

        let mut seek_to = None;
        let pad = ui.available_height() * 0.35;
        let active = if lyrics.synced.is_empty() {
            None
        } else {
            lyrics.synced.partition_point(|(t, _)| *t <= self.position + 0.3).checked_sub(1)
        };
        let moved = active != self.lyric_idx;
        egui::ScrollArea::vertical()
            .id_salt("lyrics_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(pad);
                if lyrics.synced.is_empty() {
                    for line in lyrics.plain.lines() {
                        ui.add(egui::Label::new(RichText::new(line).size(22.0).strong().color(ui::TEXT)));
                    }
                } else {
                    ui.spacing_mut().item_spacing.y = 16.0;
                    for (i, (at, line)) in lyrics.synced.iter().enumerate() {
                        let shown = if line.is_empty() { icons::MUSIC_NOTES } else { line.as_str() };
                        let color = if Some(i) == active {
                            ui::TEXT
                        } else if active.is_some_and(|a| i < a) {
                            ui::with_alpha(TEXT_DIM, 120)
                        } else {
                            ui::with_alpha(TEXT_DIM, 170)
                        };
                        let size = if Some(i) == active { 30.0 } else { 26.0 };
                        let resp = ui.add(
                            egui::Label::new(RichText::new(shown).size(size).strong().color(color))
                                .sense(egui::Sense::click()),
                        );
                        pointer_on_hover(&resp);
                        if resp.clicked() {
                            seek_to = Some(*at);
                        }
                        if moved && Some(i) == active {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                }
                ui.add_space(pad);
            });
        self.lyric_idx = active;
        seek_to
    }

    /// Songwriters, performers and crew; returns a clicked person who has an artist page.
    fn credits_panel(&mut self, ui: &mut egui::Ui) -> Option<Artist> {
        let Some(groups) = self.credits.clone() else {
            ui.add_space(40.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading credits…").color(TEXT_DIM));
            });
            return None;
        };
        if groups.is_empty() {
            ui.add_space(40.0);
            ui.label(RichText::new("No credits available for this track.").size(16.0).color(TEXT_DIM));
            return None;
        }
        let mut groups = groups;
        groups.sort_by_key(|g| credit_priority(&g.role));

        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("credits_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(12.0);
                for group in &groups {
                    ui.label(RichText::new(credit_role(&group.role)).size(12.5).strong().color(ACCENT));
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (i, (name, id)) in group.people.iter().enumerate() {
                            if i > 0 {
                                ui.label(RichText::new(", ").size(18.0).color(TEXT_DIM));
                            }
                            let label = egui::Label::new(RichText::new(name).size(18.0));
                            match id {
                                Some(id) => {
                                    let resp = ui.add(label.sense(egui::Sense::click()));
                                    if resp.hovered() {
                                        resp.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                        let y = resp.rect.bottom() - 1.0;
                                        ui.painter().line_segment(
                                            [egui::pos2(resp.rect.left(), y), egui::pos2(resp.rect.right(), y)],
                                            egui::Stroke::new(1.0_f32, ui::TEXT),
                                        );
                                    }
                                    if resp.clicked() {
                                        clicked = Some(Artist { id: *id, name: name.clone(), picture: None });
                                    }
                                }
                                None => {
                                    ui.add(label);
                                }
                            }
                        }
                    });
                    ui.add_space(14.0);
                }
            });
        clicked
    }

    // ------------------------------------------------------------ queue panel

    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        enum Act {
            Jump(usize),
            Remove(usize),
            Clear,
        }
        let mut act: Option<Act> = None;
        let mut close = false;

        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Queue").strong().size(18.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
                pointer_on_hover(&resp);
                let color = if resp.hovered() { ui::TEXT } else { TEXT_DIM };
                ui::paint_icon(ui.painter(), rect.center(), icons::X, 17.0, color, egui::FontFamily::Proportional);
                close = resp.clicked();
            });
        });
        ui.add_space(8.0);

        let upcoming = self.upcoming();
        let mut want = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("queue_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if let Some(track) = &self.current {
                    queue_heading(ui, "Now playing", None);
                    queue_row(ui, &self.textures, track, true, &mut want);
                }
                if upcoming.is_empty() {
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new("No more tracks in the queue.").color(TEXT_DIM));
                    });
                    return;
                }
                if queue_heading(ui, "Next up", Some("Clear")) {
                    act = Some(Act::Clear);
                }
                for &idx in upcoming.iter().take(150) {
                    let Some(track) = self.queue.get(idx) else { continue };
                    match queue_row(ui, &self.textures, track, false, &mut want) {
                        QueueClick::Play => act = Some(Act::Jump(idx)),
                        QueueClick::Remove => act = Some(Act::Remove(idx)),
                        QueueClick::None => {}
                    }
                }
            });
        self.request_all(want);

        if close {
            self.queue_open = false;
        }
        match act {
            Some(Act::Jump(idx)) => self.queue_jump(idx),
            Some(Act::Remove(idx)) => self.queue_remove(idx),
            Some(Act::Clear) => {
                let mut idxs = self.upcoming();
                idxs.sort_unstable_by(|a, b| b.cmp(a));
                for idx in idxs {
                    self.queue_remove(idx);
                }
            }
            None => {}
        }
    }

    fn request_all(&mut self, urls: Vec<String>) {
        for url in urls {
            self.request_cover(&url);
        }
    }

    // ------------------------------------------------------- settings popup

    fn settings_modal(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let frame = egui::Frame::new()
            .fill(ui::PANEL_SOLID)
            .stroke(egui::Stroke::new(1.0_f32, ui::EDGE))
            .corner_radius(egui::CornerRadius::same(18))
            .inner_margin(egui::Margin::same(0));
        let modal = egui::Modal::new(egui::Id::new("settings_modal"))
            .frame(frame)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect().size();
                let modal_w = (screen.x - 120.0).clamp(760.0, 1120.0);
                let modal_h = (screen.y - 140.0).clamp(440.0, 780.0);
                ui.set_width(modal_w);
                ui.set_height(modal_h);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
                ui.horizontal_top(|ui| {
                    // tabs
                    ui.allocate_ui_with_layout(
                        egui::vec2(200.0, modal_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                ui.label(RichText::new("Settings").strong().size(18.0));
                            });
                            ui.add_space(14.0);
                            egui::Frame::new()
                                .inner_margin(egui::Margin::symmetric(10, 0))
                                .show(ui, |ui| {
                                    for (tab, icon, label) in [
                                        (SettingsTab::Playback, icons::PLAY_CIRCLE, "Playback"),
                                        (SettingsTab::Equalizer, icons::SLIDERS, "Equalizer"),
                                        (SettingsTab::Discord, icons::DISCORD_LOGO, "Discord"),
                                        (SettingsTab::System, icons::DESKTOP, "System"),
                                        (SettingsTab::Account, icons::USER_CIRCLE, "Account"),
                                    ] {
                                        if nav_item(ui, icon, label, self.settings_tab == tab, None).clicked() {
                                            self.settings_tab = tab;
                                        }
                                    }
                                });
                        },
                    );
                    let sep = ui.max_rect();
                    ui.painter().line_segment(
                        [
                            egui::pos2(sep.left() + 200.0, sep.top()),
                            egui::pos2(sep.left() + 200.0, sep.top() + modal_h),
                        ],
                        egui::Stroke::new(1.0_f32, LINE),
                    );
                    // content
                    ui.allocate_ui_with_layout(
                        egui::vec2(modal_w - 201.0, modal_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Frame::new()
                                .inner_margin(egui::Margin::symmetric(28, 22))
                                .show(ui, |ui| {
                                    ui.set_width(modal_w - 201.0 - 56.0);
                                    ui.set_height(modal_h - 44.0);
                                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt(("settings_scroll", self.settings_tab as u8))
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| self.settings_content(ui));
                                });
                        },
                    );
                });
            });
        if modal.should_close() {
            self.settings_open = false;
        }
    }

    fn settings_content(&mut self, ui: &mut egui::Ui) {
        match self.settings_tab {
            SettingsTab::Playback => {
                ui.heading("Playback");
                ui.add_space(8.0);

                ui.add_space(10.0);
                self.transition_settings(ui);
                ui.add_space(10.0);
                settings_label(ui, "Keyboard shortcuts");
                // each entry: alternatives separated by " / ", keys within one by "+"
                egui::Grid::new("shortcuts").num_columns(2).spacing([28.0, 8.0]).show(ui, |ui| {
                    // arrows come from the icon font; the text font has none
                    let (l, r, u, d) = (icons::ARROW_LEFT, icons::ARROW_RIGHT, icons::ARROW_UP, icons::ARROW_DOWN);
                    for (keys, what) in [
                        ("Space".to_string(), "Play / Pause"),
                        (format!("Ctrl+{r} / Ctrl+{l}"), "Next / previous track"),
                        (format!("{r} / {l}"), "Seek 10 seconds forward / back"),
                        (format!("{u} / {d}"), "Volume up / down"),
                        ("M".to_string(), "Mute"),
                        ("Ctrl+F / /".to_string(), "Search"),
                        ("Esc".to_string(), "Close full-screen player"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (i, combo) in keys.split(" / ").enumerate() {
                                if i > 0 {
                                    ui.label(RichText::new("or").size(11.5).color(TEXT_DIM));
                                }
                                for (j, key) in combo.split('+').enumerate() {
                                    if j > 0 {
                                        ui.label(RichText::new("+").size(11.5).color(TEXT_DIM));
                                    }
                                    ui::keycap(ui, key);
                                }
                            }
                        });
                        ui.label(RichText::new(what).size(12.5).color(TEXT_DIM));
                        ui.end_row();
                    }
                });

                ui.add_space(10.0);
                settings_label(ui, "Output device");
                let devices = self.devices.clone();
                let selected = if self.cfg.output_device.is_empty() {
                    "Default device".to_string()
                } else {
                    devices
                        .iter()
                        .find(|d| d.id == self.cfg.output_device)
                        .map(|d| d.label.clone())
                        .unwrap_or_else(|| self.cfg.output_device.clone())
                };
                let mut picked: Option<String> = None;
                egui::ComboBox::from_id_salt("settings_device")
                    .selected_text(ellipsize(&selected, 44))
                    .width(360.0)
                    .show_ui(ui, |ui| {
                        ui::green_selection(ui);
                        if ui
                            .selectable_label(self.cfg.output_device.is_empty(), "Default device")
                            .clicked()
                        {
                            picked = Some(String::new());
                        }
                        for device in &devices {
                            let is_sel = self.cfg.output_device == device.id;
                            if ui.selectable_label(is_sel, &device.label).clicked() {
                                picked = Some(device.id.clone());
                            }
                        }
                    });
                if let Some(device) = picked {
                    self.cfg.output_device = device.clone();
                    self.audio.send(AudioCmd::SetDevice(device));
                    self.touch();
                }
            }
            SettingsTab::Equalizer => self.equalizer_view(ui),
            SettingsTab::Discord => {
                ui.heading("Discord");
                ui.add_space(8.0);
                let mut discord = self.cfg.discord;
                if ui.checkbox(&mut discord, "Enable Rich Presence").changed() {
                    self.cfg.discord = discord;
                    if discord {
                        self.discord.send(DiscordCmd::Connect);
                        if let Some(track) = self.current.clone() {
                            self.discord_track(&track, self.now_quality.clone());
                            self.discord.send(DiscordCmd::SetPlaying {
                                playing: self.playing,
                                position: self.position,
                            });
                        }
                    } else {
                        self.discord.send(DiscordCmd::Disconnect);
                    }
                    self.touch();
                }
                ui.label(
                    RichText::new("Shows what you are listening to in Discord: title “by” artist, with cover art and progress.")
                        .color(TEXT_DIM),
                );
                if let Some(track) = &self.current {
                    ui.add_space(12.0);
                    ui::card().show(ui, |ui| {
                        ui.label(RichText::new("Preview").size(12.0).color(TEXT_DIM));
                        ui.label(RichText::new(&track.title).strong());
                        ui.label(RichText::new(format!("by {}", track.artist_name())).color(TEXT_DIM));
                    });
                }
            }
            SettingsTab::System => {
                ui.heading("System");
                ui.add_space(8.0);

                let mut media = self.cfg.media_keys;
                if ui.checkbox(&mut media, "Media keys and media controls").changed() {
                    self.cfg.media_keys = media;
                    self.system.set_media(media);
                    self.touch();
                }
                ui.label(
                    RichText::new(
                        "Control tidalfast with your media keys and show the current track in your desktop's \
                         media widgets (KDE, GNOME, Waybar, playerctl …) or the Windows media overlay.",
                    )
                    .color(TEXT_DIM),
                );

                ui.add_space(14.0);
                let mut tray = self.cfg.tray;
                if ui.checkbox(&mut tray, "Show tray icon").changed() {
                    self.cfg.tray = tray;
                    self.system.set_tray(tray);
                    if !tray && self.window_hidden {
                        let ctx = ui.ctx().clone();
                        self.show_window(&ctx);
                    }
                    self.touch();
                }
                ui.label(
                    RichText::new(
                        "Left click shows or hides the window, right click opens the menu. On Linux this works \
                         with KDE, Waybar, XFCE and other bars with StatusNotifier support; GNOME needs the \
                         “AppIndicator and KStatusNotifierItem Support” extension.",
                    )
                    .color(TEXT_DIM),
                );

                ui.add_space(14.0);
                let mut close_to_tray = self.cfg.close_to_tray;
                if ui
                    .add_enabled(self.cfg.tray, egui::Checkbox::new(&mut close_to_tray, "Keep running in the tray when closed"))
                    .changed()
                {
                    self.cfg.close_to_tray = close_to_tray;
                    self.touch();
                }
                ui.label(
                    RichText::new("Closing the window only hides tidalfast; the music keeps playing. “Quit” in the tray menu exits the app.")
                        .color(TEXT_DIM),
                );
            }
            SettingsTab::Account => {
                ui.heading("Account");
                ui.add_space(8.0);
                if let Some(user) = &self.user {
                    ui.label(RichText::new(&user.name).strong().size(17.0));
                    ui.label(RichText::new(format!("Region: {}", user.country)).color(TEXT_DIM));
                }
                ui.add_space(12.0);
                if ui.button("Sign out").clicked() {
                    self.settings_open = false;
                    self.logout();
                }
                ui.add_space(24.0);
                ui.label(
                    RichText::new(format!("tidalfast {}", env!("CARGO_PKG_VERSION")))
                        .size(12.0)
                        .color(TEXT_DIM),
                );
            }
        }
    }

    fn transition_settings(&mut self, ui: &mut egui::Ui) {
        settings_label(ui, "Transitions between tracks");
        let before = (self.cfg.transition, self.cfg.crossfade_secs);
        ui.horizontal(|ui| {
            for (mode, label) in [
                (TransitionMode::Off, "Gapless"),
                (TransitionMode::Crossfade, "Crossfade"),
            ] {
                let width = 30.0 + label.chars().count() as f32 * 7.5;
                if pill(ui, label, self.cfg.transition == mode, width).clicked() {
                    self.cfg.transition = mode;
                }
            }
        });
        let help = match self.cfg.transition {
            TransitionMode::Off => "The next track starts without a pause as soon as the current one ends.",
            TransitionMode::Crossfade => "The last seconds of a track fade into the next one.",
        };
        ui.label(RichText::new(help).size(12.5).color(TEXT_DIM));
        if self.cfg.transition != TransitionMode::Off {
            ui.add_space(4.0);
            settings_label(ui, "Crossfade length");
            ui.spacing_mut().slider_width = 300.0;
            ui.add(
                egui::Slider::new(&mut self.cfg.crossfade_secs, 0.5..=15.0)
                    .suffix(" s")
                    .fixed_decimals(1)
                    .clamping(egui::SliderClamping::Never),
            );
            self.cfg.crossfade_secs = self.cfg.crossfade_secs.clamp(0.1, 30.0);
        }
        if before != (self.cfg.transition, self.cfg.crossfade_secs) {
            self.audio.send(AudioCmd::SetTransition(self.cfg.transition()));
            self.touch();
        }
    }

    // ------------------------------------------------------------ view: login

    fn login_view(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.label(RichText::new("tidalfast").size(36.0).strong().color(ACCENT));
            ui.add_space(6.0);
            ui.label(RichText::new("A native TIDAL client").weak());
            ui.add_space(30.0);

            if let Some(code) = &self.device_code {
                ui.label("Sign in in your browser and enter this code:");
                ui.add_space(10.0);
                ui.label(
                    RichText::new(&code.user_code)
                        .size(40.0)
                        .monospace()
                        .strong()
                        .color(ACCENT),
                );
                ui.add_space(10.0);
                let url = code
                    .uri_complete
                    .clone()
                    .unwrap_or_else(|| code.uri.clone());
                if ui.button("Open in browser").clicked() {
                    open_url(&url);
                }
                ui.add_space(4.0);
                ui.label(RichText::new(&url).weak().size(12.0));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Waiting for confirmation…");
                });
            } else {
                let button =
                    egui::Button::new(
                        RichText::new("  Sign in with TIDAL  ").size(16.0).strong().color(BG),
                    )
                    .fill(ACCENT);
                if ui.add_enabled(!self.login_busy, button).clicked() {
                    self.login_busy = true;
                    self.net.send(Cmd::Login);
                }
                if self.login_busy {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Preparing connection…");
                    });
                }
            }
        });
    }

    // ------------------------------------------------------------------ shell

    fn status_toast(&mut self, ctx: &egui::Context) {
        let Some((message, at)) = &self.toast else {
            return;
        };
        let age = at.elapsed();
        if age > Duration::from_secs(4) {
            self.toast = None;
            return;
        }
        let alpha = (1.0 - (age.as_secs_f32() - 3.0).max(0.0)).clamp(0.0, 1.0);
        egui::Area::new(egui::Id::new("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -(PLAYER_HEIGHT + ui::GAP * 3.0)))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_black_alpha((220.0 * alpha) as u8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(message).color(Color32::from_gray(230)));
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(120));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(debug_assertions)]
        self.debug_shot(ctx);
        self.sync_budget = 24;
        self.handle_net_events(ctx);
        self.handle_audio_events();
        self.handle_system(ctx);
        self.refresh_preload();
        while let Ok(text) = self.link_rx.try_recv() {
            self.open_external(&text);
        }
        self.prefetch_covers();
        let full = self.now_open && self.current.is_some() && self.user.is_some();
        if full && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !ctx.wants_keyboard_input() {
            self.now_open = false;
        }
        if full {
            let id = self.current.as_ref().map(|t| t.id);
            if self.info_for != id {
                self.info_for = id;
                self.lyrics = None;
                self.credits = None;
                self.lyric_idx = None;
                if let Some(track_id) = id {
                    self.net.send(Cmd::LoadTrackInfo { track_id });
                }
            }
        }
        self.viz = if self.playing {
            viz_bars(&self.eq_shared.spectrum(), &mut self.viz_ref, ctx.input(|i| i.stable_dt))
        } else {
            [0.0; VIZ_BARS]
        };
        #[cfg(debug_assertions)]
        if self.shot.play {
            self.viz = [0.9, 0.55, 0.75, 0.4, 0.6];
        }

        self.shortcuts(ctx);

        // wallpaper: blurred cover + the palette of the current song
        let cover_url = self.current.as_ref().and_then(|t| t.cover_url(COVER_NOW));
        let target = cover_url
            .as_ref()
            .and_then(|u| self.palettes.get(u))
            .copied()
            .unwrap_or(ui::DEFAULT_PALETTE);
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let ease = 1.0 - (-dt * 3.2).exp();
        let mut moving = false;
        for (cur, tgt) in self.bg_colors.iter_mut().zip(target) {
            let next = ui::lerp_color(*cur, tgt, ease);
            if next == *cur && *cur != tgt {
                // close enough: snapping avoids stalling on integer rounding
                *cur = tgt;
                moving = true;
            } else if next != *cur {
                *cur = next;
                moving = true;
            }
        }
        if moving {
            ctx.request_repaint();
        }
        self.ensure_wallpaper();
        // cross-fade the backdrop when the song (cover) changes
        let ready = cover_url.clone().filter(|u| self.blur_tex.contains_key(u));
        if ready.is_some() && ready != self.wall_url {
            self.wall_prev = self.wall_url.take();
            self.wall_url = ready;
            self.wall_t = 0.0;
        }
        if self.wall_t < 1.0 {
            self.wall_t = (self.wall_t + dt / 0.8).min(1.0);
            ctx.request_repaint();
        }
        let tex = |u: &Option<String>| u.as_ref().and_then(|u| self.blur_tex.get(u)).map(|t| t.id());
        ui::paint_song_wallpaper(ctx, &self.bg_colors, tex(&self.wall_url), tex(&self.wall_prev), self.wall_t);

        // Layout: floating glass cards on the wallpaper, all with the same gap
        // to each other and to the window edge. Every card clips its content.
        let full = self.now_open && self.current.is_some() && self.user.is_some();
        let logged_in = self.user.is_some();
        let show_player = logged_in || self.current.is_some();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let g = ui::GAP;
                let screen = ui.max_rect();
                let header = egui::Rect::from_min_max(
                    screen.min + egui::vec2(g, g),
                    egui::pos2(screen.right() - g, screen.top() + g + 44.0),
                );
                let player = egui::Rect::from_min_max(
                    egui::pos2(screen.left() + g, screen.bottom() - g - PLAYER_HEIGHT),
                    screen.max - egui::vec2(g, g),
                );
                let bottom = if show_player { player.top() - g } else { screen.bottom() - g };
                let mut body = egui::Rect::from_min_max(
                    egui::pos2(screen.left() + g, header.bottom() + g),
                    egui::pos2(screen.right() - g, bottom),
                );

                section(ui, "header", header, None, egui::Vec2::ZERO, |ui| self.header(ui));
                if show_player {
                    section(ui, "player", player, Some(ui::PANEL), egui::vec2(18.0, 10.0), |ui| {
                        self.player_bar(ui)
                    });
                }
                if logged_in && !full {
                    let t = ui.ctx().animate_bool_with_time_and_easing(
                        egui::Id::new("library_open"),
                        self.cfg.sidebar_expanded,
                        0.24,
                        egui::emath::easing::cubic_in_out,
                    );
                    self.sidebar_t = t;
                    let width = LIBRARY_COLLAPSED + (LIBRARY_EXPANDED - LIBRARY_COLLAPSED) * t;
                    let side = egui::Rect::from_min_size(body.min, egui::vec2(width, body.height()));
                    section(ui, "library", side, Some(ui::PANEL), egui::vec2(8.0, 12.0), |ui| self.library(ui));
                    body.min.x = side.right() + g;
                }
                if logged_in && self.queue_open && !full {
                    let queue = egui::Rect::from_min_max(egui::pos2(body.right() - QUEUE_WIDTH, body.top()), body.max);
                    section(ui, "queue", queue, Some(ui::PANEL), egui::vec2(10.0, 12.0), |ui| self.queue_panel(ui));
                    body.max.x = queue.left() - g;
                }
                let fill = if full { ui::PANEL_CLEAR } else { ui::PANEL };
                // little side padding here: the scrollbar sits near the card edge,
                // the content gets its own margin inside the scroll area
                section(ui, "main", body, Some(fill), egui::vec2(8.0, 18.0), |ui| {
                    let pad = egui::Margin::symmetric(18, 0);
                    if self.user.is_none() {
                        egui::Frame::new().inner_margin(pad).show(ui, |ui| self.login_view(ui));
                        return;
                    }
                    if full {
                        egui::Frame::new().inner_margin(pad).show(ui, |ui| self.now_playing_view(ui));
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("main_scroll", self.view as u8))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Frame::new().inner_margin(pad).show(ui, |ui| match self.view {
                                View::Search => self.search_view(ui),
                                View::Playlists => self.playlists_view(ui),
                                View::Artist => self.artist_view(ui),
                                View::Import => self.import_view(ui),
                            });
                            ui.add_space(8.0);
                        });
                });
            });

        self.auto_search(ctx);
        self.dialog_ui(ctx);
        self.profile_menu(ctx);
        self.settings_modal(ctx);
        self.status_toast(ctx);
        self.maybe_save();

        let interval = if self.playing && self.settings_open && self.settings_tab == SettingsTab::Equalizer {
            33
        } else if self.playing && self.now_open {
            60
        } else if self.playing || self.login_busy || self.playlists_loading {
            150
        } else {
            500
        };
        ctx.request_repaint_after(Duration::from_millis(interval));
    }
}

// ------------------------------------------------------------- free helpers

const PLAYER_HEIGHT: f32 = 88.0;
/// Library width with covers only, and with names.
const LIBRARY_COLLAPSED: f32 = 72.0;
const LIBRARY_EXPANDED: f32 = 236.0;
const QUEUE_WIDTH: f32 = 330.0;

/// Lay out one area of the window: an optional glass card, padding, and a
/// child UI clipped to the card so nothing can spill over its edges.
fn section(
    ui: &mut egui::Ui,
    id: &str,
    rect: egui::Rect,
    fill: Option<Color32>,
    pad: egui::Vec2,
    add: impl FnOnce(&mut egui::Ui),
) {
    if let Some(fill) = fill {
        ui::paint_card(ui.painter(), rect, fill);
    }
    let inner = rect.shrink2(pad);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    // clip at the vertical padding, so scrolled content never runs into the
    // rounded corners; sideways only the border is kept free
    // (without a card there is no edge to keep free: only keep it on screen)
    let clip = if fill.is_some() {
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + 2.0, inner.top()),
            egui::pos2(rect.right() - 2.0, inner.bottom()),
        )
    } else {
        rect.expand(4.0)
    };
    child.set_clip_rect(clip.intersect(ui.clip_rect()));
    add(&mut child);
}

fn cover(
    ui: &mut egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    url: Option<&str>,
    size: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_cover(ui, textures, url, rect);
    resp
}

fn paint_cover(
    ui: &egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    url: Option<&str>,
    rect: egui::Rect,
) {
    paint_cover_r(ui, textures, url, rect, 8.0);
}

fn paint_cover_r(
    ui: &egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    url: Option<&str>,
    rect: egui::Rect,
    radius: f32,
) {
    match url.and_then(|u| textures.get(u)) {
        Some(tex) => {
            egui::Image::new(egui::load::SizedTexture::new(tex.id(), rect.size()))
                .corner_radius(radius)
                .paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, radius, CARD_HOVER);
        }
    }
}


enum TrackAction {
    Play,
    SetLiked(bool),
    AddToPlaylist { uuid: String, title: String },
    PlayNext,
    AddToQueue,
    Share(ShareTarget),
    OpenArtist(Artist),
    OpenAlbum(Album),
}

struct TrackCtx<'a> {
    liked: &'a HashSet<u64>,
    playlists: &'a [Playlist],
    playing: bool,
    bars: [f32; VIZ_BARS],
}

/// The right-click menu of a track (list rows and the player).
fn track_menu(
    ui: &mut egui::Ui,
    track: &Track,
    liked: bool,
    ctx: &TrackCtx,
    action: &mut Option<(Track, TrackAction)>,
) {
    ui.set_min_width(230.0);
    let item = |icon: &str, label: &str| format!("{icon}  {label}");
    if ui.button(item(icons::PLAY, "Play")).clicked() {
        *action = Some((track.clone(), TrackAction::Play));
        ui.close_menu();
    }
    if ui.button(item(icons::QUEUE, "Play next")).clicked() {
        *action = Some((track.clone(), TrackAction::PlayNext));
        ui.close_menu();
    }
    if ui.button(item(icons::PLUS, "Add to queue")).clicked() {
        *action = Some((track.clone(), TrackAction::AddToQueue));
        ui.close_menu();
    }
    ui.separator();
    let like_label = if liked {
        "Remove from Liked Songs"
    } else {
        "Add to Liked Songs"
    };
    if ui.button(item(icons::HEART, like_label)).clicked() {
        *action = Some((track.clone(), TrackAction::SetLiked(!liked)));
        ui.close_menu();
    }
    ui.menu_button(item(icons::LIST_PLUS, "Add to playlist"), |ui| {
        ui.set_min_width(200.0);
        if ctx.playlists.is_empty() {
            ui.label(RichText::new("No playlists of your own").color(TEXT_DIM));
        }
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for pl in ctx.playlists {
                if ui.button(ellipsize(&pl.title, 32)).clicked() {
                    *action = Some((
                        track.clone(),
                        TrackAction::AddToPlaylist {
                            uuid: pl.uuid.clone(),
                            title: pl.title.clone(),
                        },
                    ));
                    ui.close_menu();
                }
            }
        });
    });
    ui.separator();
    if let Some(album) = track.album.as_ref().filter(|a| a.id != 0) {
        if ui.button(item(icons::VINYL_RECORD, "Go to album")).clicked() {
            let mut album = album.clone();
            if album.artists.is_empty() {
                album.artists = track.artist_list();
            }
            *action = Some((track.clone(), TrackAction::OpenAlbum(album)));
            ui.close_menu();
        }
    }
    let artists = track.artist_list();
    match artists.as_slice() {
        [] => {}
        [only] => {
            if ui.button(item(icons::MICROPHONE_STAGE, "Go to artist")).clicked() {
                *action = Some((track.clone(), TrackAction::OpenArtist(only.clone())));
                ui.close_menu();
            }
        }
        many => {
            ui.menu_button(item(icons::MICROPHONE_STAGE, "Go to artist"), |ui| {
                for artist in many {
                    if ui.button(ellipsize(&artist.name, 32)).clicked() {
                        *action = Some((track.clone(), TrackAction::OpenArtist(artist.clone())));
                        ui.close_menu();
                    }
                }
            });
        }
    }
    ui.separator();
    let mut target = None;
    ui.menu_button(item(icons::SHARE_NETWORK, "Share"), |ui| share_menu(ui, &mut target));
    if let Some(t) = target {
        *action = Some((track.clone(), TrackAction::Share(t)));
    }
}

/// Hovering "Share" opens one button per service.
fn share_menu(ui: &mut egui::Ui, target: &mut Option<ShareTarget>) {
    ui.set_min_width(190.0);
    for (icon, label, t) in [
        (icons::WAVES, "TIDAL", ShareTarget::Tidal),
        (icons::SPOTIFY_LOGO, "Spotify", ShareTarget::Spotify),
        (icons::YOUTUBE_LOGO, "YouTube", ShareTarget::Youtube),
        (icons::LINK, "All services", ShareTarget::All),
    ] {
        if ui.button(format!("{icon}  {label}")).clicked() {
            *target = Some(t);
            ui.close_menu();
        }
    }
}

fn track_row(
    ui: &mut egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    number: usize,
    track: &Track,
    is_current: bool,
    want: &mut Vec<String>,
    ctx: &TrackCtx,
    action: &mut Option<(Track, TrackAction)>,
) -> egui::Response {
    let url = track.cover_url(COVER_ROW);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
    pointer_on_hover(&resp);
    let hovered = ui.rect_contains_pointer(rect);
    let painter = ui.painter();
    if is_current {
        ui::paint_current_row(painter, rect);
    } else if hovered {
        painter.rect_filled(rect, 10.0, CARD);
    }
    let mid = rect.center().y;
    let num_pos = egui::pos2(rect.left() + 24.0, mid);
    if is_current {
        playing_bars(ui, num_pos, 16.0, ACCENT, &ctx.bars, ctx.playing);
    } else if hovered {
        ui::paint_icon(painter, num_pos, icons::PLAY, 15.0, ui::TEXT, ui::fill_family());
    } else {
        painter.text(
            num_pos,
            egui::Align2::CENTER_CENTER,
            format!("{number}"),
            egui::FontId::proportional(12.5),
            TEXT_DIM,
        );
    }
    let cover_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 68.0, mid),
        egui::vec2(40.0, 40.0),
    );
    paint_cover(ui, textures, url.as_deref(), cover_rect);

    // wide rows get a separate album column
    let text_left = rect.left() + 100.0;
    let text_right = (rect.right() - 118.0).max(text_left + 1.0);
    let wide = rect.width() > 720.0;
    let title_right = if wide {
        text_left + (text_right - text_left) * 0.58
    } else {
        text_right
    };
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(text_left, rect.top() + 7.0),
        egui::pos2(title_right, rect.bottom() - 7.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.spacing_mut().item_spacing.y = 1.0;
    let title = RichText::new(&track.title).size(14.5);
    let title = if is_current { title.color(ACCENT).strong() } else { title };
    child.add(egui::Label::new(title).truncate());
    if let Some(artist) = artist_links(&mut child, &track.artist_list(), 12.5, TEXT_DIM) {
        *action = Some((track.clone(), TrackAction::OpenArtist(artist)));
    }

    if wide {
        let album_rect = egui::Rect::from_min_max(
            egui::pos2(title_right + 16.0, rect.top()),
            egui::pos2(text_right, rect.bottom()),
        );
        let mut album_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(album_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        album_ui.add(
            egui::Label::new(RichText::new(track.album_title()).size(12.5).color(TEXT_DIM)).truncate(),
        );
    }

    ui.painter().text(
        egui::pos2(rect.right() - 14.0, mid),
        egui::Align2::RIGHT_CENTER,
        track.duration_label(),
        egui::FontId::monospace(12.5),
        TEXT_DIM,
    );

    // like heart (visible on hover or when liked)
    let liked = ctx.liked.contains(&track.id);
    let heart_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 92.0, mid),
        egui::vec2(30.0, 30.0),
    );
    let heart = ui.interact(heart_rect, resp.id.with("heart"), egui::Sense::click());
    if liked || hovered || heart.hovered() {
        pointer_on_hover(&heart);
        let color = if liked {
            ACCENT
        } else if heart.hovered() {
            ui::TEXT
        } else {
            TEXT_DIM
        };
        ui::paint_heart(ui.painter(), heart_rect.center(), 17.0, color, liked);
    }
    if heart.clicked() {
        *action = Some((track.clone(), TrackAction::SetLiked(!liked)));
    }

    resp.context_menu(|ui| track_menu(ui, track, liked, ctx, action));

    if let Some(url) = url {
        want.push(url);
    }
    resp
}

/// Artist names as individual links; returns the artist that was clicked.
fn artist_links(ui: &mut egui::Ui, artists: &[Artist], size: f32, color: Color32) -> Option<Artist> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
        for (i, artist) in artists.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new(", ").size(size).color(color));
            }
            let resp = ui.add(
                egui::Label::new(RichText::new(&artist.name).size(size).color(color))
                    .sense(egui::Sense::click()),
            );
            if resp.hovered() {
                resp.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                let y = resp.rect.bottom() - 1.0;
                ui.painter().line_segment(
                    [egui::pos2(resp.rect.left(), y), egui::pos2(resp.rect.right(), y)],
                    egui::Stroke::new(1.0_f32, ui::TEXT),
                );
            }
            if resp.clicked() {
                clicked = Some(artist.clone());
            }
        }
    });
    clicked
}

enum QueueClick {
    None,
    Play,
    Remove,
}

/// Small heading in the queue; returns true if its action link was clicked.
fn queue_heading(ui: &mut egui::Ui, title: &str, action: Option<&str>) -> bool {
    let mut clicked = false;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(RichText::new(title).strong().size(13.5).color(TEXT_DIM));
        if let Some(label) = action {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                let resp = ui.add(
                    egui::Label::new(RichText::new(label).size(12.5).color(TEXT_DIM)).sense(egui::Sense::click()),
                );
                pointer_on_hover(&resp);
                clicked = resp.clicked();
            });
        }
    });
    ui.add_space(2.0);
    clicked
}

/// One compact queue entry; the current track is not clickable.
fn queue_row(
    ui: &mut egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    track: &Track,
    current: bool,
    want: &mut Vec<String>,
) -> QueueClick {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 50.0),
        if current { egui::Sense::hover() } else { egui::Sense::click() },
    );
    let hovered = ui.rect_contains_pointer(rect);
    if current {
        ui::paint_current_row(ui.painter(), rect);
    } else if hovered {
        pointer_on_hover(&resp);
        ui.painter().rect_filled(rect, 10.0, CARD);
    }
    let url = track.cover_url(COVER_ROW);
    let cover_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 30.0, rect.center().y),
        egui::vec2(38.0, 38.0),
    );
    paint_cover(ui, textures, url.as_deref(), cover_rect);
    if let Some(u) = url {
        want.push(u);
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 58.0, rect.top() + 8.0),
        egui::pos2(rect.right() - 40.0, rect.bottom() - 6.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.spacing_mut().item_spacing.y = 1.0;
    let title = RichText::new(&track.title).size(14.0);
    child.add(egui::Label::new(if current { title.color(ACCENT).strong() } else { title }).truncate());
    child.add(egui::Label::new(RichText::new(track.artist_name()).size(12.0).color(TEXT_DIM)).truncate());

    let mut result = QueueClick::None;
    if !current && hovered {
        let x_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 20.0, rect.center().y),
            egui::vec2(28.0, 28.0),
        );
        let x = ui.interact(x_rect, resp.id.with("remove"), egui::Sense::click());
        pointer_on_hover(&x);
        let color = if x.hovered() { ui::DANGER } else { TEXT_DIM };
        ui::paint_icon(ui.painter(), x_rect.center(), icons::X, 16.0, color, egui::FontFamily::Proportional);
        if x.clicked() {
            result = QueueClick::Remove;
        }
    }
    if matches!(result, QueueClick::None) && resp.clicked() && !current {
        result = QueueClick::Play;
    }
    result
}

/// Vocals first, then writers, then producers, then the rest.
fn credit_priority(role: &str) -> u8 {
    let r = role.to_lowercase();
    if r.contains("vocal") || r.contains("singer") || r.contains("main artist") {
        0
    } else if r.contains("composer") || r.contains("lyricist") || r.contains("writer") {
        1
    } else if r.contains("producer") {
        2
    } else {
        3
    }
}

fn credit_role(role: &str) -> String {
    role.to_uppercase()
}

/// One import entry: checkbox, TIDAL match and status. True when clicked.
fn import_row(
    ui: &mut egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    item: &ImportItem,
    want: &mut Vec<String>,
) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
    let selectable = item.found.is_some();
    if selectable {
        pointer_on_hover(&resp);
        if resp.hovered() {
            ui.painter().rect_filled(rect, 10.0, CARD);
        }
    }
    let mid = rect.center().y;
    let (glyph, family, color) = if item.selected {
        (icons::CHECK_SQUARE, ui::fill_family(), ACCENT)
    } else {
        (icons::SQUARE, egui::FontFamily::Proportional, if selectable { TEXT_DIM } else { ui::LINE })
    };
    ui::paint_icon(ui.painter(), egui::pos2(rect.left() + 22.0, mid), glyph, 21.0, color, family);

    let cover_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 66.0, mid), egui::vec2(40.0, 40.0));
    let url = item.found.as_ref().and_then(|t| t.cover_url(COVER_ROW));
    paint_cover(ui, textures, url.as_deref(), cover_rect);
    if let Some(u) = url {
        want.push(u);
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 98.0, rect.top() + 7.0),
        egui::pos2((rect.right() - 130.0).max(rect.left() + 99.0), rect.bottom() - 7.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.spacing_mut().item_spacing.y = 1.0;
    let dim = if selectable { ui::TEXT } else { TEXT_DIM };
    let (title, sub) = match &item.found {
        Some(t) if !item.sure => (
            t.title.clone(),
            format!("Spotify: {} – {}", item.src.title, item.src.artists),
        ),
        Some(t) => (t.title.clone(), t.artist_name()),
        None => (item.src.title.clone(), item.src.artists.clone()),
    };
    child.add(egui::Label::new(RichText::new(title).size(14.5).color(dim)).truncate());
    let sub_color = if item.found.is_some() && !item.sure { ui::WARN } else { TEXT_DIM };
    child.add(egui::Label::new(RichText::new(sub).size(12.5).color(sub_color)).truncate());

    let (status, status_color) = match (&item.found, item.checked, item.sure) {
        (Some(_), _, true) => ("sure", ui::OK),
        (Some(_), _, false) => ("unsure", ui::WARN),
        (None, true, _) => ("not found", ui::DANGER),
        (None, false, _) => ("searching…", TEXT_DIM),
    };
    ui.painter().text(
        egui::pos2(rect.right() - 14.0, mid),
        egui::Align2::RIGHT_CENTER,
        status,
        egui::FontId::proportional(12.5),
        status_color,
    );
    resp.clicked() && selectable
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.add_space(18.0);
    ui.label(RichText::new(title).strong().size(20.0));
    ui.add_space(6.0);
}

fn album_kind(album: &Album) -> &'static str {
    match album.kind.as_deref() {
        Some("EP") => "EP",
        Some("SINGLE") => "Single",
        _ => "Album",
    }
}

fn settings_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(TEXT_DIM));
}

fn menu_item(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
    pointer_on_hover(&resp);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, CARD_HOVER);
    }
    ui::paint_icon(
        ui.painter(),
        egui::pos2(rect.left() + 22.0, rect.center().y),
        icon,
        17.0,
        TEXT_DIM,
        egui::FontFamily::Proportional,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 42.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.5),
        ui::TEXT,
    );
    resp
}

/// The green-to-cyan "liked songs" tile with a heart.
fn liked_art(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_liked_art(ui.painter(), rect, 12.0, 0);
    resp
}

/// The green-to-cyan "liked songs" tile with a heart, with rounded corners
/// like every other cover (so hover overlays line up with it).
/// `dim`: black overlay strength (0‥255), baked into the colours so it covers
/// exactly the same area as the art, down to the last edge pixel.
fn paint_liked_art(painter: &egui::Painter, rect: egui::Rect, radius: f32, dim: u8) {
    let (a, b) = (Color32::from_rgb(166, 227, 161), Color32::from_rgb(137, 220, 235));
    let mid = Color32::from_rgb(148, 226, 213);
    // colour at a point: bilinear blend of the four corner colours
    let color_at = |p: egui::Pos2| {
        let u = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let v = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let lerp = |x: Color32, y: Color32, t: f32| {
            Color32::from_rgb(
                (x.r() as f32 + (y.r() as f32 - x.r() as f32) * t) as u8,
                (x.g() as f32 + (y.g() as f32 - x.g() as f32) * t) as u8,
                (x.b() as f32 + (y.b() as f32 - x.b() as f32) * t) as u8,
            )
        };
        let c = lerp(lerp(a, mid, u), lerp(mid, b, u), v);
        let keep = 1.0 - dim as f32 / 255.0;
        Color32::from_rgb((c.r() as f32 * keep) as u8, (c.g() as f32 * keep) as u8, (c.b() as f32 * keep) as u8)
    };
    // outline of the rounded rectangle, as a fan around the centre
    let r = radius.min(rect.width() * 0.5).max(0.0);
    let corners = [
        (egui::pos2(rect.right() - r, rect.top() + r), -90.0f32),
        (egui::pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (egui::pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (egui::pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    let mut outline = Vec::new();
    for (centre, start) in corners {
        for step in 0..=8 {
            let angle = (start + step as f32 * 90.0 / 8.0).to_radians();
            outline.push(centre + egui::vec2(angle.cos(), angle.sin()) * r);
        }
    }
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    for p in &outline {
        mesh.colored_vertex(*p, color_at(*p));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(egui::Shape::mesh(mesh));
    ui::paint_icon(painter, rect.center(), icons::HEART, rect.width() * 0.46, BG, ui::fill_family());
}

/// Round play/pause button drawn over covers.
fn overlay_play(ui: &mut egui::Ui, center: egui::Pos2, radius: f32, pause: bool, id: egui::Id) -> egui::Response {
    let rect = egui::Rect::from_center_size(center, egui::vec2(radius * 2.0, radius * 2.0));
    let resp = ui.interact(rect, id, egui::Sense::click());
    pointer_on_hover(&resp);
    let r = if resp.hovered() { radius + 1.5 } else { radius };
    let painter = ui.painter();
    painter.circle_filled(center + egui::vec2(0.0, 1.5), r, Color32::from_black_alpha(70));
    painter.circle_filled(center, r, ACCENT);
    let glyph = if pause { icons::PAUSE } else { icons::PLAY };
    ui::paint_icon(painter, center, glyph, radius * 0.95, BG, ui::fill_family());
    resp
}

/// Bars shown by the playing indicator.
const VIZ_BARS: usize = 5;

/// Condense the analyser's log-spaced spectrum (dB) into a few 0‥1 bar heights.
///
/// `reference` is an automatic gain: the loudest recent bar level, falling
/// slowly, so quiet and loud tracks both use the full height without pinning.
fn viz_bars(spectrum: &[f32; crate::eq::SPEC_BINS], reference: &mut f32, dt: f32) -> [f32; VIZ_BARS] {
    const RANGE_DB: f32 = 30.0;
    let per_bar = crate::eq::SPEC_BINS / VIZ_BARS;
    let mut levels = [0.0_f32; VIZ_BARS];
    let mut loudest = -100.0_f32;
    for (i, level) in levels.iter_mut().enumerate() {
        let group = &spectrum[i * per_bar..(i + 1) * per_bar];
        // mean level of the group (not its peak), with a mild treble lift
        *level = group.iter().sum::<f32>() / group.len() as f32 + i as f32 * 2.5;
        loudest = loudest.max(*level);
    }
    // jump up to new peaks, fall ~4 dB/s
    *reference = if loudest > *reference { loudest } else { (*reference - 4.0 * dt).max(loudest.max(-90.0)) };
    let mut bars = [0.0; VIZ_BARS];
    for (bar, level) in bars.iter_mut().zip(levels) {
        let t = ((level - (*reference - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
        *bar = t * t;
    }
    bars
}

/// A small live spectrum: "this is playing". Paused shows flat, low bars.
fn playing_bars(ui: &egui::Ui, center: egui::Pos2, height: f32, color: Color32, bars: &[f32; VIZ_BARS], live: bool) {
    let step = height * 0.3;
    for (i, level) in bars.iter().enumerate() {
        let h = height * if live { 0.12 + 0.88 * level } else { 0.14 };
        let x = center.x + (i as f32 - (VIZ_BARS as f32 - 1.0) / 2.0) * step;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x - height * 0.09, center.y + height * 0.5 - h),
                egui::pos2(x + height * 0.09, center.y + height * 0.5),
            ),
            1.0,
            color,
        );
    }
    if live {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
}

fn playlist_menu(ui: &mut egui::Ui, st: &CardState, action: &mut Option<PlAction>) {
    ui.set_min_width(190.0);
    let (play_icon, play_label) = if st.playing_here && st.playing {
        (icons::PAUSE, "Pause")
    } else {
        (icons::PLAY, "Play")
    };
    if ui.button(format!("{play_icon}  {play_label}")).clicked() {
        *action = Some(PlAction::Play);
        ui.close_menu();
    }
    if ui.button(format!("{}  Open", icons::FOLDER_OPEN)).clicked() {
        *action = Some(PlAction::Open);
        ui.close_menu();
    }
    if st.shareable {
        ui.separator();
        let mut target = None;
        ui.menu_button(format!("{}  Share", icons::SHARE_NETWORK), |ui| share_menu(ui, &mut target));
        if let Some(t) = target {
            *action = Some(PlAction::Share(t));
        }
    }
    if st.own {
        ui.separator();
        if ui.button(format!("{}  Edit details", icons::PENCIL_SIMPLE)).clicked() {
            *action = Some(PlAction::Edit);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Delete", icons::TRASH)).color(ui::DANGER))
            .clicked()
        {
            *action = Some(PlAction::Delete);
            ui.close_menu();
        }
    }
}

/// A square media card (album, playlist, mix) or a round one (artist).
fn media_card(
    ui: &mut egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    url: Option<&str>,
    liked: bool,
    round: bool,
    title: &str,
    subtitle: &str,
    st: &CardState,
) -> Option<PlAction> {
    let card_w = 176.0;
    let img = card_w - 32.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(card_w, img + 74.0), egui::Sense::click());
    pointer_on_hover(&resp);
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.painter().rect_filled(rect, 14.0, CARD);
    }
    let art = egui::Rect::from_min_size(rect.min + egui::vec2(16.0, 14.0), egui::vec2(img, img));
    if liked {
        paint_liked_art(ui.painter(), art, 12.0, 0);
    } else {
        paint_cover_r(ui, textures, url, art, if round { img / 2.0 } else { 12.0 });
    }

    let mut action = None;
    let btn_center = egui::pos2(art.right() - 26.0, art.bottom() - 26.0);
    if hovered {
        let pause = st.playing_here && st.playing;
        if overlay_play(ui, btn_center, 20.0, pause, resp.id.with("play")).clicked() {
            action = Some(PlAction::Play);
        }
    } else if st.playing_here {
        ui.painter().circle_filled(btn_center, 17.0, ACCENT);
        playing_bars(ui, btn_center, 17.0, BG, &st.bars, st.playing);
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 16.0, art.bottom() + 8.0),
        egui::pos2(rect.right() - 12.0, rect.bottom() - 4.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(if round { egui::Align::Center } else { egui::Align::Min })),
    );
    child.spacing_mut().item_spacing.y = 1.0;
    let t = RichText::new(title).strong().size(13.5);
    child.add(egui::Label::new(if st.playing_here { t.color(ACCENT) } else { t }).truncate());
    child.add(egui::Label::new(RichText::new(subtitle).size(12.0).color(TEXT_DIM)).truncate());

    resp.context_menu(|ui| playlist_menu(ui, st, &mut action));
    if action.is_none() && resp.clicked() {
        action = Some(PlAction::Open);
    }
    action
}

/// One entry of the library: cover (with play button on hover and the
/// visualizer while playing), title and subtitle.
#[allow(clippy::too_many_arguments)]
/// A square (or wide) button tile on the top bar. Dark and nearly opaque, so
/// the icon stays readable on every wallpaper colour.
fn top_tile(
    ui: &egui::Ui,
    tile: egui::Rect,
    hovered: bool,
    icon: &str,
    size: f32,
    color: Color32,
    family: egui::FontFamily,
) {
    let fill = if hovered {
        Color32::from_rgba_unmultiplied(44, 46, 64, 235)
    } else {
        Color32::from_rgba_unmultiplied(19, 20, 30, 215)
    };
    let painter = ui.painter();
    let radius = egui::CornerRadius::same(ui::CARD_RADIUS);
    painter.rect_filled(tile, radius, fill);
    painter.rect_stroke(tile, radius, egui::Stroke::new(1.0_f32, ui::EDGE), egui::StrokeKind::Inside);
    ui::paint_icon(painter, tile.center(), icon, size, color, family);
}

fn library_row(
    ui: &mut egui::Ui,
    textures: &HashMap<String, TextureHandle>,
    liked: bool,
    cover_url: Option<&str>,
    title: &str,
    subtitle: &str,
    selected: bool,
    st: &CardState,
    reveal: f32,
) -> Option<PlAction> {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), egui::Sense::click());
    pointer_on_hover(&resp);
    // collapsed: the name moves into a tooltip
    let resp = if reveal < 0.5 { resp.on_hover_text(title) } else { resp };
    let hovered = ui.rect_contains_pointer(rect);
    if selected {
        ui.painter().rect_filled(rect, 10.0, CARD_HOVER);
    } else if hovered {
        ui.painter().rect_filled(rect, 10.0, CARD);
    }
    // centred in the collapsed sidebar: (72 − 2 × 8 padding − 46) / 2
    let art_x = ((LIBRARY_COLLAPSED - 16.0 - 46.0) * 0.5).floor();
    let art = egui::Rect::from_min_size(rect.min + egui::vec2(art_x, 6.0), egui::vec2(46.0, 46.0));
    let collapsed = reveal < 0.5;
    // the dimming and the play button belong to the cover alone
    let over_art = ui.rect_contains_pointer(art);
    let dim = if over_art {
        120
    } else if st.playing_here {
        150
    } else {
        0
    };
    if liked {
        paint_liked_art(ui.painter(), art, 8.0, dim);
    } else {
        paint_cover_r(ui, textures, cover_url, art, 8.0);
        if dim > 0 {
            ui.painter().rect_filled(art, 8.0, Color32::from_black_alpha(dim));
        }
    }

    let mut action = None;
    if over_art {
        // collapsed: a click opens the playlist (a quick second click plays
        // it, see `library`); expanded: play button on the cover
        if !collapsed {
            let pause = st.playing_here && st.playing;
            if overlay_play(ui, art.center(), 15.0, pause, resp.id.with("play")).clicked() {
                action = Some(PlAction::Play);
            }
        } else if st.playing_here {
            playing_bars(ui, art.center(), 22.0, ACCENT, &st.bars, st.playing);
        }
    } else if st.playing_here {
        playing_bars(ui, art.center(), 22.0, ACCENT, &st.bars, st.playing);
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(art.right() + 10.0, rect.top() + 10.0),
        egui::pos2(rect.right() - 8.0, rect.bottom() - 8.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.spacing_mut().item_spacing.y = 2.0;
    // names fade in while the sidebar slides open
    let fade = |c: Color32| ui::with_alpha(c, (reveal.clamp(0.0, 1.0) * 255.0) as u8);
    let highlight = selected || st.playing_here;
    let t = RichText::new(title).size(14.0).color(fade(if highlight { ACCENT } else { ui::TEXT }));
    child.add(egui::Label::new(if highlight { t.strong() } else { t }).truncate());
    child.add(egui::Label::new(RichText::new(subtitle).size(12.0).color(fade(TEXT_DIM))).truncate());

    resp.context_menu(|ui| playlist_menu(ui, st, &mut action));
    if action.is_none() {
        if resp.clicked() {
            action = Some(PlAction::Open);
        }
    }
    action
}

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    {
        // The URL goes straight to the shell's default browser handler: no
        // console window, and `&` in the query is not taken for a command separator.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// Debug builds only: `TIDALFAST_SHOT=out.png` saves a screenshot of the app
/// window after a few seconds and quits. `TIDALFAST_SHOT_SEARCH=<text>` types
/// a search first; `TIDALFAST_SHOT_SETTINGS`, `_NOW` and `_QUEUE` open the
/// settings, the full-screen player and the queue; `_PLAY` pretends to play,
/// `_ARTIST=<id>` opens an artist page and `_PROFILE` the profile menu.
#[cfg(debug_assertions)]
mod debug_shot {
    use std::time::{Duration, Instant};

    use eframe::egui;

    #[derive(Default)]
    pub struct Shot {
        pub path: Option<String>,
        pub search: Option<String>,
        pub settings: bool,
        pub now: bool,
        pub queue: bool,
        pub play: bool,
        pub artist: Option<u64>,
        pub profile: bool,
        /// `TIDALFAST_SHOT_SIDEBAR=open|closed`
        pub sidebar: Option<bool>,
        pub started: Option<Instant>,
        pub requested: bool,
        pub prepared: bool,
    }

    impl Shot {
        pub fn from_env() -> Self {
            Self {
                path: std::env::var("TIDALFAST_SHOT").ok(),
                search: std::env::var("TIDALFAST_SHOT_SEARCH").ok(),
                settings: std::env::var("TIDALFAST_SHOT_SETTINGS").is_ok(),
                now: std::env::var("TIDALFAST_SHOT_NOW").is_ok(),
                queue: std::env::var("TIDALFAST_SHOT_QUEUE").is_ok(),
                play: std::env::var("TIDALFAST_SHOT_PLAY").is_ok(),
                artist: std::env::var("TIDALFAST_SHOT_ARTIST").ok().and_then(|v| v.parse().ok()),
                profile: std::env::var("TIDALFAST_SHOT_PROFILE").is_ok(),
                sidebar: std::env::var("TIDALFAST_SHOT_SIDEBAR").ok().map(|v| v == "open"),
                ..Default::default()
            }
        }
    }

    impl super::App {
        pub(super) fn debug_shot(&mut self, ctx: &egui::Context) {
            let Some(path) = self.shot.path.clone() else { return };
            let started = *self.shot.started.get_or_insert_with(Instant::now);
            if !self.shot.prepared && started.elapsed() > Duration::from_secs(2) {
                self.shot.prepared = true;
                if let Some(q) = self.shot.search.clone() {
                    self.search_query = q;
                    self.perform_search(false);
                    ctx.memory_mut(|m| m.request_focus(egui::Id::new("search_field")));
                }
                if std::env::var_os("TIDALFAST_SHOT_EDIT").is_some() {
                    if let Some(pl) = self.playlists.iter().find(|p| p.uuid != super::LIKED_UUID).cloned() {
                        self.apply_pl_action(pl, super::PlAction::Edit);
                    }
                }
                self.settings_open = self.shot.settings;
                self.now_open = self.shot.now;
                self.queue_open = self.shot.queue;
                self.profile_menu_open = self.shot.profile;
                if let Some(open) = self.shot.sidebar {
                    // only for this run; not saved
                    self.cfg.sidebar_expanded = open;
                }
                if let Some(id) = self.shot.artist {
                    self.open_artist(id, None);
                }
                if self.shot.play {
                    // pretend to play (no sound): "playing" from the last source
                    self.playing = true;
                    self.playing_from = self.cfg.last_source.clone();
                }
            }
            if !self.shot.requested && started.elapsed() > Duration::from_secs(6) {
                self.shot.requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            let image = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                    let _ = buf.save(&path);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
