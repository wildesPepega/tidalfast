//! Background network worker. Owns the TIDAL client and runs it on a tokio
//! runtime, communicating with the UI thread over channels.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::runtime::Runtime;
use tokio::sync::mpsc::{self as tmpsc, UnboundedReceiver, UnboundedSender};

use crate::link::{self, Kind, Link, Service};
use crate::model::{Album, ImportTrack, ArtistPage, CreditGroup, Lyrics, Playlist, SearchResults, StreamInfo, Track};
use crate::tidal::{Poll, TidalClient, Tokens};

pub enum Cmd {
    /// Begin the device-code login flow.
    Login,
    /// Load the signed-in user, their name and playlists.
    LoadUser,
    LoadPlaylist {
        uuid: String,
    },
    SetLiked {
        track_id: u64,
        liked: bool,
    },
    AddToPlaylist {
        uuid: String,
        title: String,
        track_id: u64,
    },
    RenamePlaylist {
        uuid: String,
        title: String,
    },
    DeletePlaylist {
        uuid: String,
    },
    CreatePlaylist {
        title: String,
    },
    /// Load personal mixes and editorial playlists.
    LoadHome,
    /// Tracks similar to this one, for autoplay.
    Radio {
        track_id: u64,
    },
    /// Load the liked-songs list (delivered as `PlaylistTracks` with uuid `liked`).
    LoadFavorites,
    Search(String),
    ResolveTrack {
        track_id: u64,
        /// For the next track (preloading), not for immediate playback.
        preload: bool,
    },
    Image(String),
    /// Load an artist page (top tracks and releases).
    LoadArtist {
        id: u64,
    },
    /// Read a public Spotify playlist and start matching it on TIDAL.
    ImportSpotify {
        id: String,
        job: u64,
    },
    /// Internal: match track `index` of an import, then queue the next one.
    ImportStep {
        job: u64,
        tracks: Arc<Vec<ImportTrack>>,
        index: usize,
    },
    /// Create a TIDAL playlist from the chosen tracks.
    ImportCreate {
        title: String,
        track_ids: Vec<u64>,
    },
    /// Lyrics and credits for the full-screen player.
    LoadTrackInfo {
        track_id: u64,
    },
    /// Create a linkhop share link for a TIDAL URL and hand it back as `Ev::Copy`.
    Share {
        url: String,
        /// linkhop service id (`spotify`, `youtube_music`); empty for the page with all services.
        target: String,
    },
    /// Resolve a Spotify/TIDAL link to something we can open.
    OpenLink {
        link: Link,
        /// Show the result in the search view instead of opening/playing it.
        show: bool,
    },
}

/// What an external link points at, resolved on TIDAL.
pub enum Opened {
    Track(Box<Track>),
    Album(Box<Album>),
    Artist(Box<crate::model::Artist>),
    Playlist(Box<Playlist>),
}

pub enum Ev {
    DeviceCode {
        user_code: String,
        uri: String,
        uri_complete: Option<String>,
    },
    Tokens(Tokens),
    LoggedIn {
        user_id: u64,
        user_name: String,
        picture: Option<String>,
        country_code: String,
    },
    Playlists(Vec<Playlist>),
    PlaylistTracks {
        uuid: String,
        tracks: Vec<Track>,
    },
    Home(Vec<(String, Vec<Playlist>)>),
    /// A short success message for the UI toast.
    Info(String),
    Radio {
        track_id: u64,
        tracks: Vec<Track>,
    },
    Search(SearchResults),
    Stream {
        track_id: u64,
        info: StreamInfo,
        preload: bool,
    },
    Image {
        url: String,
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    Artist(Box<ArtistPage>),
    ImportStart {
        job: u64,
        name: String,
        cover: Option<String>,
        tracks: Vec<ImportTrack>,
    },
    ImportMatch {
        job: u64,
        index: usize,
        track: Option<Box<Track>>,
        sure: bool,
    },
    Lyrics {
        track_id: u64,
        lyrics: Lyrics,
    },
    Credits {
        track_id: u64,
        groups: Vec<CreditGroup>,
    },
    /// Text for the clipboard.
    Copy(String),
    Open(Opened, bool),
    /// A link (e.g. a short link) turned out to be a Spotify playlist: import it.
    SpotifyPlaylist(String),
    Error(String),
}

pub struct Net {
    tx: UnboundedSender<Cmd>,
    rx: Receiver<Ev>,
    _rt: Runtime,
}

impl Net {
    pub fn spawn(country_code: String, tokens: Tokens) -> anyhow::Result<Self> {
        let (cmd_tx, cmd_rx) = tmpsc::unbounded_channel::<Cmd>();
        let (ev_tx, ev_rx) = mpsc::channel::<Ev>();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;

        let mut client = TidalClient::new(country_code)?;
        client.set_tokens(&tokens);

        rt.spawn_blocking(prune_cache);
        rt.spawn(worker(client, cmd_rx, cmd_tx.clone(), ev_tx));

        Ok(Self {
            tx: cmd_tx,
            rx: ev_rx,
            _rt: rt,
        })
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn try_recv(&self) -> Option<Ev> {
        self.rx.try_recv().ok()
    }
}

async fn worker(
    mut client: TidalClient,
    mut rx: UnboundedReceiver<Cmd>,
    tx: UnboundedSender<Cmd>,
    ev: Sender<Ev>,
) {
    let mut import_job = 0u64;
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Login => login(&mut client, &ev).await,
            Cmd::LoadUser => load_user(&mut client, &ev).await,
            Cmd::LoadPlaylist { uuid } => match client.collection_tracks(&uuid).await {
                Ok(tracks) => {
                    send_tracks(&ev, uuid, tracks);
                }
                Err(e) => send_error(&ev, e),
            },
            Cmd::SetLiked { track_id, liked } => {
                if let Err(e) = client.set_liked(track_id, liked).await {
                    send_error(&ev, e);
                    // resync the liked list so the hearts show the real state
                    if let Some(id) = client.user_id {
                        if let Ok(tracks) = client.favorite_tracks(id).await {
                            send_tracks(&ev, "liked".into(), tracks);
                        }
                    }
                }
            }
            Cmd::AddToPlaylist { uuid, title, track_id } => {
                match client.add_to_playlist(&uuid, track_id).await {
                    Ok(true) => {
                        let _ = ev.send(Ev::Info(format!("Added to “{title}”")));
                        if let Ok(tracks) = client.collection_tracks(&uuid).await {
                            send_tracks(&ev, uuid, tracks);
                        }
                        if let Some(id) = client.user_id {
                            if let Ok(list) = client.user_playlists(id).await {
                                crate::cache::write("playlists", &list);
                                let _ = ev.send(Ev::Playlists(list));
                            }
                        }
                    }
                    Ok(false) => {
                        let _ = ev.send(Ev::Info(format!("Already in “{title}”")));
                    }
                    Err(e) => send_error(&ev, e),
                }
            }
            Cmd::RenamePlaylist { uuid, title } => {
                let result = client.rename_playlist(&uuid, &title).await;
                finish_playlist_edit(&mut client, &ev, result, "Playlist renamed").await;
            }
            Cmd::DeletePlaylist { uuid } => {
                let result = client.delete_playlist(&uuid).await;
                finish_playlist_edit(&mut client, &ev, result, "Playlist deleted").await;
            }
            Cmd::CreatePlaylist { title } => {
                let result = client.create_playlist(&title).await.map(|_| ());
                finish_playlist_edit(&mut client, &ev, result, "Playlist created").await;
            }
            Cmd::LoadHome => load_home(&mut client, &ev).await,
            Cmd::Radio { track_id } => match client.track_radio(track_id).await {
                Ok(tracks) => {
                    let _ = ev.send(Ev::Radio { track_id, tracks });
                }
                Err(e) => log::warn!("radio failed: {e:#}"),
            },
            Cmd::LoadFavorites => match client.user_id {
                Some(id) => match client.favorite_tracks(id).await {
                    Ok(tracks) => {
                        send_tracks(&ev, "liked".to_string(), tracks);
                    }
                    Err(e) => send_error(&ev, e),
                },
                None => send_error(&ev, anyhow::anyhow!("not signed in")),
            },
            Cmd::Search(query) => match client.search(&query).await {
                Ok(results) => {
                    let _ = ev.send(Ev::Search(results));
                }
                Err(e) => send_error(&ev, e),
            },
            Cmd::ResolveTrack { track_id, preload } => {
                match client.stream_info(track_id).await {
                    Ok(info) => {
                        let _ = ev.send(Ev::Stream { track_id, info, preload });
                    }
                    // a failed preload just means no smooth transition
                    Err(e) if preload => log::warn!("preload resolve failed: {e:#}"),
                    Err(e) => send_error(&ev, e),
                }
            }
            Cmd::LoadArtist { id } => load_artist(&mut client, &ev, id).await,
            Cmd::ImportSpotify { id, job } => {
                import_job = job;
                let fetched = crate::import::fetch_embed(&client.http(), "playlist", &id).await;
                match fetched {
                    Ok(embed) if embed.tracks.is_empty() => {
                        send_error(&ev, anyhow::anyhow!("The Spotify playlist is empty or not public"));
                    }
                    Ok(embed) => {
                        let tracks = Arc::new(embed.tracks.clone());
                        let _ = ev.send(Ev::ImportStart {
                            job,
                            name: embed.name,
                            cover: embed.cover,
                            tracks: embed.tracks,
                        });
                        let _ = tx.send(Cmd::ImportStep { job, tracks, index: 0 });
                    }
                    Err(e) => send_error(&ev, e),
                }
            }
            Cmd::ImportStep { job, tracks, index } => {
                // a newer import replaced this one
                if job != import_job || index >= tracks.len() {
                    continue;
                }
                let src = &tracks[index];
                let found = find_on_tidal(&mut client, src).await;
                let sure = found.as_ref().is_some_and(|f| f.1);
                let _ = ev.send(Ev::ImportMatch {
                    job,
                    index,
                    track: found.map(|f| Box::new(f.0)),
                    sure,
                });
                let _ = tx.send(Cmd::ImportStep { job, tracks, index: index + 1 });
            }
            Cmd::ImportCreate { title, track_ids } => {
                let count = track_ids.len();
                let result = async {
                    let uuid = client.create_playlist(&title).await?;
                    client.add_tracks(&uuid, &track_ids).await
                }
                .await;
                let message = format!("Playlist “{title}” created with {count} tracks");
                finish_playlist_edit(&mut client, &ev, result, &message).await;
            }
            Cmd::LoadTrackInfo { track_id } => {
                let lyrics = client.lyrics(track_id).await.unwrap_or_else(|e| {
                    log::warn!("lyrics failed: {e:#}");
                    Lyrics::default()
                });
                let _ = ev.send(Ev::Lyrics { track_id, lyrics });
                let groups = client.credits(track_id).await.unwrap_or_else(|e| {
                    log::warn!("credits failed: {e:#}");
                    Vec::new()
                });
                let _ = ev.send(Ev::Credits { track_id, groups });
            }
            Cmd::Share { url, target } => {
                let http = client.http();
                let ev = ev.clone();
                tokio::spawn(async move {
                    let shared = linkhop(&http, &url, target.is_empty()).await.and_then(|v| {
                        let found = if target.is_empty() {
                            v.pointer("/share/url")
                        } else if v.pointer(&format!("/targets/{target}/status")).and_then(|s| s.as_str()) == Some("ok") {
                            v.pointer(&format!("/targets/{target}/url"))
                        } else {
                            None
                        };
                        found
                            .and_then(|u| u.as_str())
                            .map(str::to_string)
                            .ok_or_else(|| anyhow::anyhow!("not found there"))
                    });
                    let _ = ev.send(match shared {
                        Ok(url) => Ev::Copy(url),
                        Err(e) => Ev::Error(format!("Sharing failed: {e}")),
                    });
                });
            }
            Cmd::OpenLink { link, show } => open_link(&mut client, &ev, link, show).await,
            Cmd::Image(url) => {
                let http = client.http();
                let ev = ev.clone();
                tokio::spawn(async move { fetch_image(http, url, ev).await });
            }
        }
    }
}

/// Ask linkhop (https://linkhop.paul.wtf) to convert a music link.
async fn linkhop(http: &reqwest::Client, url: &str, share: bool) -> anyhow::Result<serde_json::Value> {
    let mut query = vec![("url", url)];
    if share {
        query.push(("share", "true"));
    }
    let resp = http
        .get("https://linkhop.paul.wtf/api/v1/convert")
        .query(&query)
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("linkhop answered with {}", resp.status());
    }
    Ok(resp.json().await?)
}

/// Find the TIDAL track for a Spotify track: the most specific search first,
/// then looser ones, and a network hiccup gets one more try.
async fn find_on_tidal(client: &mut TidalClient, src: &ImportTrack) -> Option<(Track, bool)> {
    let mut best: Option<(Track, bool)> = None;
    for query in crate::import::search_queries(src) {
        let mut results = client.search(&query).await;
        if results.is_err() {
            tokio::time::sleep(Duration::from_millis(600)).await;
            results = client.search(&query).await;
        }
        let results = match results {
            Ok(r) => r,
            Err(e) => {
                log::warn!("import search failed: {e:#}");
                continue;
            }
        };
        if let Some((i, sure)) = crate::import::best_match(src, &results.tracks) {
            let found = (results.tracks[i].clone(), sure);
            if sure {
                return Some(found);
            }
            best.get_or_insert(found);
        }
    }
    best
}

/// Follow a short link (`spotify.link/…`) to the page it points at.
async fn resolve_short(http: &reqwest::Client, url: &str) -> anyhow::Result<Link> {
    let resp = http
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if let Some(link) = link::parse(resp.url().as_str()) {
        if link.service != Service::SpotifyShort {
            return Ok(link);
        }
    }
    // some short links answer with a page that names the target
    let body = resp.text().await.unwrap_or_default();
    body.split(|c: char| c == '"' || c == '\'' || c == '<' || c == '>' || c.is_whitespace())
        .filter(|w| w.contains("open.spotify.com/") || w.starts_with("spotify:"))
        .map(|w| w.replace("\\u002F", "/").replace("&amp;", "&"))
        .find_map(|w| link::parse(&w))
        .ok_or_else(|| anyhow::anyhow!("The short link leads nowhere"))
}

async fn open_link(client: &mut TidalClient, ev: &Sender<Ev>, link: Link, show: bool) {
    let link = if link.service == Service::SpotifyShort {
        match resolve_short(&client.http(), &link.id).await {
            Ok(l) => l,
            Err(e) => return send_error(ev, e),
        }
    } else {
        link
    };
    if link.service == Service::Spotify && link.kind == Kind::Playlist {
        let _ = ev.send(Ev::SpotifyPlaylist(link.id));
        return;
    }
    let tidal = if link.service == Service::Tidal {
        link
    } else {
        let url = match link.service {
            Service::Linkhop => format!("https://linkhop.paul.wtf/api/v1/c/{}", link.id),
            _ => format!(
                "https://linkhop.paul.wtf/api/v1/convert?url={}",
                urlencode(&link.url())
            ),
        };
        let found = linkhop_get(&client.http(), &url).await.ok().and_then(|v| {
            // a link that already points at TIDAL needs no matching
            let tidal_url = if v.pointer("/source/service").and_then(|s| s.as_str()) == Some("tidal") {
                v.pointer("/source/url")?.as_str()?
            } else {
                let target = v.pointer("/targets/tidal")?;
                if target.get("status")?.as_str()? != "ok" {
                    return None;
                }
                target.get("url")?.as_str()?
            };
            link::parse(tidal_url)
        });
        match found {
            Some(l) => l,
            None => {
                // linkhop is down or doesn't know it: match it on TIDAL ourselves
                if link.service == Service::Spotify {
                    match spotify_fallback(client, &link).await {
                        Ok(opened) => {
                            let _ = ev.send(Ev::Open(opened, show));
                            return;
                        }
                        Err(e) => log::warn!("spotify fallback failed: {e:#}"),
                    }
                }
                let _ = ev.send(Ev::Error("This link was not found on TIDAL".into()));
                return;
            }
        }
    };
    let opened = match (tidal.kind, tidal.id.parse::<u64>()) {
        (Kind::Playlist, _) => client.playlist(&tidal.id).await.map(|p| Opened::Playlist(Box::new(p))),
        (_, Err(_)) => Err(anyhow::anyhow!("invalid link")),
        (Kind::Track, Ok(id)) => client.track(id).await.map(|t| Opened::Track(Box::new(t))),
        (Kind::Album, Ok(id)) => client.album(id).await.map(|a| Opened::Album(Box::new(a))),
        (Kind::Artist, Ok(id)) => client.artist_info(id).await.map(|a| Opened::Artist(Box::new(a))),
    };
    match opened {
        Ok(o) => {
            let _ = ev.send(Ev::Open(o, show));
        }
        Err(e) => send_error(ev, e),
    }
}

/// Read a Spotify track, album or artist from its embed page and look it up
/// on TIDAL by name.
async fn spotify_fallback(client: &mut TidalClient, link: &Link) -> anyhow::Result<Opened> {
    let embed = crate::import::fetch_embed(&client.http(), link.kind.word(), &link.id).await?;
    match link.kind {
        Kind::Track => {
            let src = embed.tracks.first().ok_or_else(|| anyhow::anyhow!("empty track page"))?;
            let (track, _) = find_on_tidal(client, src).await.ok_or_else(|| anyhow::anyhow!("no match"))?;
            Ok(Opened::Track(Box::new(track)))
        }
        Kind::Album => {
            let results = client.search(&format!("{} {}", embed.name, embed.subtitle)).await?;
            let i = crate::import::best_album(&embed.name, &embed.subtitle, &results.albums)
                .ok_or_else(|| anyhow::anyhow!("no match"))?;
            let album = client.album(results.albums[i].id).await?;
            Ok(Opened::Album(Box::new(album)))
        }
        Kind::Artist => {
            let results = client.search(&embed.name).await?;
            let i = crate::import::best_artist(&embed.name, &results.artists).ok_or_else(|| anyhow::anyhow!("no match"))?;
            let artist = client.artist_info(results.artists[i].id).await?;
            Ok(Opened::Artist(Box::new(artist)))
        }
        Kind::Playlist => Err(anyhow::anyhow!("playlists are imported, not opened")),
    }
}

/// GET a linkhop answer; network errors, 429 and 5xx are retried.
async fn linkhop_get(http: &reqwest::Client, url: &str) -> anyhow::Result<serde_json::Value> {
    let mut last = anyhow::anyhow!("linkhop unreachable");
    for attempt in 0..3u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(600 * attempt)).await;
        }
        let resp = match http.get(url).timeout(Duration::from_secs(20)).send().await {
            Ok(r) => r,
            Err(e) => {
                last = e.into();
                continue;
            }
        };
        let status = resp.status();
        if status.is_success() {
            match resp.json().await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    last = e.into();
                    continue;
                }
            }
        }
        last = anyhow::anyhow!("linkhop answered with {status}");
        if !(status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS) {
            break;
        }
    }
    Err(last)
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn load_artist(client: &mut TidalClient, ev: &Sender<Ev>, id: u64) {
    let artist = client.artist_info(id).await.unwrap_or_else(|e| {
        log::warn!("artist info failed: {e:#}");
        crate::model::Artist { id, ..Default::default() }
    });
    let mut page = ArtistPage { artist, ..Default::default() };
    let _ = ev.send(Ev::Artist(Box::new(page.clone())));

    match client.collection_tracks(&format!("artist:{id}")).await {
        Ok(tracks) => page.top_tracks = tracks,
        Err(e) => log::warn!("artist top tracks failed: {e:#}"),
    }
    let _ = ev.send(Ev::Artist(Box::new(page.clone())));

    match client.artist_albums(id, "ALBUMS").await {
        Ok(list) => page.albums = list,
        Err(e) => log::warn!("artist albums failed: {e:#}"),
    }
    let _ = ev.send(Ev::Artist(Box::new(page.clone())));

    match client.artist_albums(id, "EPSANDSINGLES").await {
        Ok(list) => {
            let (singles, eps) = list.into_iter().partition(|a| a.kind.as_deref() == Some("SINGLE"));
            page.singles = singles;
            page.eps = eps;
        }
        Err(e) => log::warn!("artist EPs failed: {e:#}"),
    }
    let _ = ev.send(Ev::Artist(Box::new(page.clone())));

    match client.artist_albums(id, "COMPILATIONS").await {
        Ok(list) => page.compilations = list,
        Err(e) => log::warn!("artist compilations failed: {e:#}"),
    }
    page.done = true;
    let _ = ev.send(Ev::Artist(Box::new(page)));
}

async fn login(client: &mut TidalClient, ev: &Sender<Ev>) {
    let device = match client.device_authorization().await {
        Ok(d) => d,
        Err(e) => return send_error(ev, e),
    };
    let _ = ev.send(Ev::DeviceCode {
        user_code: device.user_code.clone(),
        uri: device.verification_uri.clone(),
        uri_complete: device.verification_uri_complete.clone(),
    });

    let mut interval = device.interval.max(1);
    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    loop {
        if Instant::now() > deadline {
            let _ = ev.send(Ev::Error(
                "Sign-in expired — please try again".into(),
            ));
            return;
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        match client.poll_device_token(&device.device_code).await {
            Ok(Poll::Done(tokens)) => {
                let _ = ev.send(Ev::Tokens(*tokens));
                load_user(client, ev).await;
                return;
            }
            Ok(Poll::SlowDown) => interval += 2,
            Ok(Poll::Pending) => {}
            Err(e) => return send_error(ev, e),
        }
    }
}

async fn load_user(client: &mut TidalClient, ev: &Sender<Ev>) {
    match client.session_info().await {
        Ok((user_id, country_code)) => {
            // The session call may have refreshed the token; persist the result.
            let _ = ev.send(Ev::Tokens(client.current_tokens()));
            let (user_name, picture) = client
                .user_profile(user_id)
                .await
                .unwrap_or_default();
            let _ = ev.send(Ev::LoggedIn {
                user_id,
                user_name,
                picture,
                country_code,
            });
            match client.user_playlists(user_id).await {
                Ok(playlists) => {
                    crate::cache::write("playlists", &playlists);
                    let _ = ev.send(Ev::Playlists(playlists));
                }
                Err(e) => send_error(ev, e),
            }
            load_home(client, ev).await;
        }
        Err(e) => send_error(ev, e),
    }
}

/// Report an edit's result and refresh the playlist list either way.
async fn finish_playlist_edit(
    client: &mut TidalClient,
    ev: &Sender<Ev>,
    result: anyhow::Result<()>,
    message: &str,
) {
    match result {
        Ok(()) => {
            let _ = ev.send(Ev::Info(message.to_string()));
        }
        Err(e) => send_error(ev, e),
    }
    if let Some(id) = client.user_id {
        if let Ok(list) = client.user_playlists(id).await {
            crate::cache::write("playlists", &list);
            let _ = ev.send(Ev::Playlists(list));
        }
    }
}

fn send_tracks(ev: &Sender<Ev>, uuid: String, tracks: Vec<Track>) {
    crate::cache::write(&format!("tracks-{uuid}"), &tracks);
    let _ = ev.send(Ev::PlaylistTracks { uuid, tracks });
}

async fn load_home(client: &mut TidalClient, ev: &Sender<Ev>) {
    match client.home_sections().await {
        Ok(sections) => {
            crate::cache::write("home", &sections);
            let _ = ev.send(Ev::Home(sections));
        }
        Err(e) => log::warn!("home failed: {e:#}"),
    }
}

/// Raw bytes of a cover from the disk cache, if present.
pub fn cached_cover(url: &str) -> Option<Vec<u8>> {
    std::fs::read(cache_file(url)?).ok()
}

/// Upper bound for the on-disk cover cache.
const CACHE_LIMIT_BYTES: u64 = 400 * 1024 * 1024;

fn cache_dir() -> Option<std::path::PathBuf> {
    dirs::cache_dir().map(|d| d.join("tidalfast").join("covers"))
}

/// Stable file name for a URL (FNV-1a, hex).
fn cache_file(url: &str) -> Option<std::path::PathBuf> {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in url.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    cache_dir().map(|d| d.join(format!("{hash:016x}.img")))
}

/// Delete the least recently written covers once the cache grows too large.
fn prune_cache() {
    let Some(dir) = cache_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.len(), meta.modified().ok()?))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= CACHE_LIMIT_BYTES {
        return;
    }
    files.sort_by_key(|f| f.2);
    for (path, size, _) in files {
        if total <= CACHE_LIMIT_BYTES * 3 / 4 {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

async fn fetch_image(http: reqwest::Client, url: String, ev: Sender<Ev>) {
    let path = cache_file(&url);

    // 1) disk cache
    if let Some(path) = &path {
        if let Ok(bytes) = tokio::fs::read(path).await {
            if send_image(&url, &bytes, &ev) {
                return;
            }
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    // 2) network, then store
    let bytes = match http.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.bytes().await {
            Ok(b) => b,
            Err(_) => return,
        },
        _ => return,
    };
    if send_image(&url, &bytes, &ev) {
        if let Some(path) = path {
            if let Some(dir) = path.parent() {
                let _ = tokio::fs::create_dir_all(dir).await;
            }
            let _ = tokio::fs::write(&path, &bytes).await;
        }
    }
}

/// Decode and forward an image; false if the bytes are not a usable image.
fn send_image(url: &str, bytes: &[u8], ev: &Sender<Ev>) -> bool {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            let _ = ev.send(Ev::Image {
                url: url.to_string(),
                width: width as usize,
                height: height as usize,
                rgba: rgba.into_raw(),
            });
            true
        }
        Err(e) => {
            log::warn!("cover decode failed: {e}");
            false
        }
    }
}

fn send_error(ev: &Sender<Ev>, error: anyhow::Error) {
    log::warn!("network error: {error:#}");
    let _ = ev.send(Ev::Error(error.to_string()));
}
