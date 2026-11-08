//! Debug builds only: `TIDALFAST_DEMO=1` replaces the network with made-up
//! music (invented artists, albums, playlists, lyrics and generated cover art)
//! so the app can be photographed for the README without anyone's account.

use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use image::RgbaImage;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::config::Settings;
use crate::model::{Album, Artist, ArtistPage, CreditGroup, Lyrics, Playlist, PlaylistCreator, SearchResults, Track};
use crate::net::{Cmd, Ev};

pub fn enabled() -> bool {
    std::env::var_os("TIDALFAST_DEMO").is_some()
}

/// The settings of the demo person: signed in, nothing that talks to the outside.
pub fn settings(mut cfg: Settings) -> Settings {
    cfg.access_token = "demo".into();
    cfg.refresh_token = "demo".into();
    cfg.user_id = Some(1);
    cfg.user_name = "Alex".into();
    cfg.country_code = "US".into();
    cfg.discord = false;
    cfg.media_keys = false;
    cfg.tray = false;
    cfg.link_handler = false;
    cfg.check_updates = false;
    cfg.auto_update = false;
    cfg.overlay = false;
    cfg.overlay_api = false;
    cfg.overlay_recording = false;
    cfg.shuffle = false;
    cfg.volume = 0.8;
    // a gentle smile curve, so the equalizer has something to show
    cfg.eq.enabled = true;
    for (band, gain) in cfg.eq.bands.iter_mut().zip([5.0, 4.0, 2.5, 0.5, -1.0, -0.5, 1.5, 3.0, 4.0, 4.5]) {
        band.gain_db = gain;
    }
    cfg.last_track = None;
    cfg.last_source = None;
    cfg
}

// ------------------------------------------------------------------ catalog

/// (artist, [(album title, kind)]) — all invented.
const ARTISTS: &[(&str, &[(&str, &str)])] = &[
    ("Neon Orchard", &[("Static Bloom", "ALBUM"), ("Afterglow Avenue", "ALBUM"), ("Soft Circuitry", "EP"), ("Paper Lanterns", "SINGLE")]),
    ("Velvet Static", &[("Night Shift", "ALBUM"), ("Low Light Love", "ALBUM"), ("Dial Tone", "SINGLE")]),
    ("Kite Harbor", &[("Salt & Signal", "ALBUM"), ("Harbor Lights", "EP"), ("Open Water", "SINGLE")]),
    ("Marlow & the Tides", &[("Slow Current", "ALBUM"), ("Undertow", "ALBUM"), ("Driftwood", "SINGLE")]),
    ("Glass Meridian", &[("Cartography of Rain", "ALBUM"), ("Quiet Machines", "EP"), ("Northbound", "SINGLE")]),
    ("Juniper Static", &[("Greenhouse Radio", "ALBUM"), ("Late Bloomers", "ALBUM"), ("Sunroom", "SINGLE")]),
    ("Night Bloom", &[("Midnight Garden", "ALBUM"), ("Moon Phases", "EP"), ("Petal Fall", "SINGLE")]),
    ("Cobalt Parade", &[("Brass & Neon", "ALBUM"), ("City Fireworks", "ALBUM"), ("Last Train Home", "SINGLE")]),
    ("Echo Valley", &[("Canyon Sessions", "ALBUM"), ("Long Way Round", "EP"), ("Dust & Gold", "SINGLE")]),
    ("Saffron Drive", &[("Highway Sunday", "ALBUM"), ("Windows Down", "ALBUM"), ("Golden Hour", "SINGLE")]),
];

const ADJECTIVES: &[&str] = &[
    "Silent", "Electric", "Golden", "Hollow", "Crimson", "Lazy", "Distant", "Burning", "Gentle", "Midnight", "Broken", "Endless", "Neon", "Faded",
    "Restless", "Quiet",
];
const NOUNS: &[&str] = &[
    "Harbor", "Skyline", "Garden", "Signal", "Compass", "River", "Mirage", "Carousel", "Lullaby", "Highway", "Horizon", "Whisper", "Orbit",
    "Meadow", "Engine", "Letters",
];

/// The n-th made-up song title (never the same twice).
fn title(n: usize) -> String {
    if n < TITLES.len() {
        TITLES[(n * 7) % TITLES.len()].to_string()
    } else {
        let k = n - TITLES.len();
        format!("{} {}", ADJECTIVES[k % ADJECTIVES.len()], NOUNS[(k / ADJECTIVES.len() + k) % NOUNS.len()])
    }
}

const TITLES: &[&str] = &[
    "Midnight Signals", "Glass Hours", "Paper Moons", "Slow Voltage", "Afterglow", "Satellite Heart", "Golden Static",
    "Tidal Pull", "Blue Hour", "Lanterns", "Wildflower Radio", "Coastline", "Neon Rain", "Quiet Machines", "Open Roads",
    "Lighthouse", "Parallel Lines", "Honey & Smoke", "Daydream Engine", "Skyline Drift", "Velvet Hours", "Echoes in Amber",
    "Moonlit Highway", "Soft Collapse", "Paper Planes", "Heatwave", "Northern Lights", "Fading Polaroids", "Orbit",
    "Slow Dancing in the Dark", "Warm Signal", "Static Bloom", "Evergreen", "City of Glass", "Low Tide", "Spark",
    "Summer Static", "Rooftops", "Afterhours", "Silver Linings", "Carousel", "Voltage", "Sundial", "Weightless",
    "Mirrors", "Wanderlight", "Late Bus Home", "Cherry Moon", "Polaroid Summer", "Meridian", "Ghost Frequencies",
    "Thunder Tea", "Canvas Sky", "Salt Air", "Backseat Stars", "Hologram", "Firefly Season", "Underwater Light",
    "Pale Gold", "Starting Over Again",
];

struct Catalog {
    artists: Vec<Artist>,
    albums: Vec<Album>,
    /// Tracks of each album, same order as `albums`.
    album_tracks: Vec<Vec<Track>>,
}

fn cover_id(kind: char, n: u64) -> String {
    format!("{kind}{n:07x}-0000-4000-8000-00000000{n:04x}")
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut artists = Vec::new();
        let mut albums = Vec::new();
        let mut album_tracks = Vec::new();
        let mut track_id = 5_000_000u64;
        let mut title_ix = 0usize;
        for (a, (name, list)) in ARTISTS.iter().enumerate() {
            let artist = Artist { id: 100 + a as u64, name: (*name).into(), picture: Some(cover_id('p', 100 + a as u64)) };
            for (b, (title, kind)) in list.iter().enumerate() {
                let id = 9_000 + (a * 10 + b) as u64;
                let count = match *kind {
                    "ALBUM" => 9 + (a + b) % 4,
                    "EP" => 5,
                    _ => 1,
                };
                let album = Album {
                    id,
                    title: (*title).into(),
                    cover: Some(cover_id('c', id)),
                    artists: vec![artist.clone()],
                    artist: Some(artist.clone()),
                    number_of_tracks: Some(count as u32),
                    release_date: Some(format!("{}-0{}-1{}", 2019 + (a + b * 2) % 7, 1 + (a + b) % 9, b)),
                    kind: Some((*kind).into()),
                    explicit: Some(false),
                };
                let tracks = (0..count)
                    .map(|n| {
                        track_id += 1;
                        let title = if *kind == "SINGLE" { (*title).to_string() } else { self::title(title_ix) };
                        title_ix += 1;
                        Track {
                            id: track_id,
                            title,
                            duration: 150 + ((track_id * 37) % 120) as u32,
                            artist: Some(artist.clone()),
                            artists: Some(vec![artist.clone()]),
                            album: Some(album.clone()),
                            audio_quality: Some(if track_id % 3 == 0 { "HI_RES_LOSSLESS" } else { "LOSSLESS" }.into()),
                            track_number: Some(n as u32 + 1),
                            image_id: None,
                            explicit: Some(track_id % 7 == 0),
                            url: None,
                        }
                    })
                    .collect();
                albums.push(album);
                album_tracks.push(tracks);
            }
            artists.push(artist);
        }
        Catalog { artists, albums, album_tracks }
    })
}

fn all_tracks() -> Vec<Track> {
    catalog().album_tracks.iter().flatten().cloned().collect()
}

fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// A stable pick of `count` tracks for a playlist, starting somewhere depending on its name.
fn pick_tracks(key: &str, count: usize) -> Vec<Track> {
    let all = all_tracks();
    let start = hash(key) as usize % all.len();
    let step = 3 + (hash(key) >> 8) as usize % 4;
    (0..count).map(|i| all[(start + i * step) % all.len()].clone()).collect()
}

fn playlist(uuid: &str, title: &str, description: &str, tracks: u32, creator: &str, art: u64) -> Playlist {
    Playlist {
        uuid: uuid.into(),
        title: title.into(),
        description: Some(description.into()),
        image: None,
        square_image: Some(cover_id('l', art)),
        number_of_tracks: Some(tracks),
        number_of_videos: None,
        creator: Some(PlaylistCreator { id: Some(if creator == "Alex" { 1 } else { 0 }), name: Some(creator.into()) }),
    }
}

fn own_playlists() -> Vec<Playlist> {
    vec![
        playlist("demo-late-night", "Late Night Drive", "Neon streets and slow synths", 24, "Alex", 1),
        playlist("demo-focus", "Deep Focus", "Instrumental calm for getting things done", 31, "Alex", 2),
        playlist("demo-gym", "Gym Energy", "Heavy hitters for the last set", 28, "Alex", 3),
        playlist("demo-sunday", "Sunday Morning", "Coffee, sunlight and soft guitars", 19, "Alex", 4),
        playlist("demo-roadtrip", "Road Trip 2026", "Windows down, volume up", 42, "Alex", 5),
        playlist("demo-chill", "Chill Vibes", "Easy listening for lazy afternoons", 36, "Alex", 6),
        playlist("demo-throwbacks", "Throwbacks", "The songs we never got tired of", 33, "Alex", 7),
    ]
}

fn home_sections() -> Vec<(String, Vec<Playlist>)> {
    let mixes = (1..=5)
        .map(|n| {
            let mut p = playlist(&format!("mix:demo{n}"), &format!("Daily Mix {n}"), ["Neon Orchard, Velvet Static and more", "Kite Harbor, Glass Meridian and more", "Night Bloom, Echo Valley and more", "Cobalt Parade, Saffron Drive and more", "Fresh finds picked for you"][n - 1], 50, "Mix", 20 + n as u64);
            p.creator = Some(PlaylistCreator { id: None, name: Some("Made for you".into()) });
            p
        })
        .collect();
    let popular = vec![
        playlist("demo-ed-1", "Fresh Finds", "New music worth your time", 40, "Editorial", 31),
        playlist("demo-ed-2", "Indie Chill", "Soft guitars and dreamy synths", 60, "Editorial", 32),
        playlist("demo-ed-3", "Electro Sunrise", "Warm electronics for early hours", 45, "Editorial", 33),
        playlist("demo-ed-4", "Hi-Res Highlights", "Lossless favorites, in full quality", 35, "Editorial", 34),
        playlist("demo-ed-5", "Rainy Day Jazz", "Brushed drums and low lights", 50, "Editorial", 35),
    ];
    let albums = catalog().albums.iter().filter(|a| a.kind.as_deref() == Some("ALBUM")).take(6).map(Album::as_playlist).collect();
    vec![("Made for you".into(), mixes), ("Popular playlists".into(), popular), ("New albums".into(), albums)]
}

fn tracks_for(uuid: &str) -> Vec<Track> {
    let cat = catalog();
    if let Some(id) = uuid.strip_prefix("album:").and_then(|v| v.parse::<u64>().ok()) {
        return cat.albums.iter().position(|a| a.id == id).map(|i| cat.album_tracks[i].clone()).unwrap_or_default();
    }
    if let Some(id) = uuid.strip_prefix("artist:").and_then(|v| v.parse::<u64>().ok()) {
        return artist_tracks(id, 10);
    }
    let count = match uuid {
        "liked" => 36,
        _ => own_playlists().iter().chain(home_sections().iter().flat_map(|(_, l)| l.iter())).find(|p| p.uuid == uuid).map(|p| p.track_count().min(30) as usize).unwrap_or(20),
    };
    pick_tracks(uuid, count)
}

fn artist_tracks(id: u64, count: usize) -> Vec<Track> {
    let cat = catalog();
    // a few from each release, so the list shows different covers
    let lists: Vec<&Vec<Track>> = cat
        .album_tracks
        .iter()
        .filter(|l| l.first().and_then(|t| t.artist.as_ref()).is_some_and(|a| a.id == id))
        .collect();
    (0..)
        .flat_map(|n| lists.iter().filter_map(move |l| l.get(n)))
        .take(count.min(lists.iter().map(|l| l.len()).sum()))
        .cloned()
        .collect()
}

fn search(query: &str) -> SearchResults {
    let cat = catalog();
    let q = query.to_lowercase();
    let tracks: Vec<Track> = all_tracks()
        .into_iter()
        .filter(|t| t.title.to_lowercase().contains(&q) || t.artist_name().to_lowercase().contains(&q) || t.album_title().to_lowercase().contains(&q))
        .take(12)
        .collect();
    let albums: Vec<Album> = cat.albums.iter().filter(|a| a.title.to_lowercase().contains(&q) || a.artist_names().to_lowercase().contains(&q)).take(8).cloned().collect();
    let artists: Vec<Artist> = cat.artists.iter().filter(|a| a.name.to_lowercase().contains(&q)).take(6).cloned().collect();
    let playlists: Vec<Playlist> = own_playlists().into_iter().chain(home_sections().into_iter().flat_map(|(_, l)| l)).filter(|p| p.title.to_lowercase().contains(&q)).take(6).collect();
    SearchResults { tracks, albums, artists, playlists }
}

fn artist_page(id: u64) -> ArtistPage {
    let cat = catalog();
    let artist = cat.artists.iter().find(|a| a.id == id).cloned().unwrap_or_default();
    let mine = |kind: &str| -> Vec<Album> {
        cat.albums.iter().filter(|a| a.artist.as_ref().is_some_and(|x| x.id == id) && a.kind.as_deref() == Some(kind)).cloned().collect()
    };
    ArtistPage {
        top_tracks: artist_tracks(id, 6),
        albums: mine("ALBUM"),
        eps: mine("EP"),
        singles: mine("SINGLE"),
        compilations: Vec::new(),
        done: true,
        artist,
    }
}

fn lyrics() -> Lyrics {
    const LINES: &[&str] = &[
        "The city hums its low electric tune",
        "while I count the lights along the avenue",
        "Every window holds a different room",
        "and every room is quietly glowing blue",
        "",
        "So send a signal, send it through the night",
        "I'll be the static that you tune into",
        "We don't need words, we don't need the light",
        "just the pulse of a song that carries us through",
        "",
        "Slow voltage in the afterglow",
        "carry me wherever the radio goes",
        "Paper moons above the overpass",
        "we are golden, we are made of glass",
        "",
        "The road goes on beyond the final sign",
        "and the dashboard paints your face in amber",
        "Hold the moment, let it overflow",
        "we'll remember it in slow motion",
        "",
        "So send a signal, send it through the night",
        "I'll be the static that you tune into",
        "Slow voltage in the afterglow",
        "carry me wherever the radio goes",
    ];
    let synced: Vec<(f64, String)> = LINES.iter().enumerate().map(|(i, l)| (8.0 + i as f64 * 6.5, (*l).to_string())).collect();
    let plain = LINES.join("\n");
    Lyrics { plain, synced }
}

fn credits() -> Vec<CreditGroup> {
    let group = |role: &str, people: &[&str]| CreditGroup { role: role.into(), people: people.iter().map(|p| ((*p).to_string(), None)).collect() };
    vec![
        group("Artist", &["Neon Orchard"]),
        group("Songwriter", &["Mara Linden", "Theo Okafor"]),
        group("Producer", &["Theo Okafor"]),
        group("Mixing Engineer", &["Iris Calder"]),
        group("Mastering Engineer", &["Jonah Pike"]),
        group("Label", &["Orchard Lane Records"]),
    ]
}

// ------------------------------------------------------------------- worker

pub async fn worker(mut rx: UnboundedReceiver<Cmd>, ev: Sender<Ev>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::LoadUser => {
                let _ = ev.send(Ev::LoggedIn { user_id: 1, user_name: "Alex".into(), picture: None, country_code: "US".into() });
                let _ = ev.send(Ev::Playlists(own_playlists()));
                let _ = ev.send(Ev::Home(home_sections()));
            }
            Cmd::LoadHome => {
                let _ = ev.send(Ev::Home(home_sections()));
            }
            Cmd::LoadFavorites => {
                let _ = ev.send(Ev::PlaylistTracks { uuid: "liked".into(), tracks: tracks_for("liked") });
            }
            Cmd::LoadPlaylist { uuid } => {
                let tracks = tracks_for(&uuid);
                let _ = ev.send(Ev::PlaylistTracks { uuid, tracks });
            }
            Cmd::Search(query) => {
                let _ = ev.send(Ev::Search(search(&query)));
            }
            Cmd::LoadArtist { id } => {
                let _ = ev.send(Ev::Artist(Box::new(artist_page(id))));
            }
            Cmd::Radio { track_id } => {
                let tracks = pick_tracks(&track_id.to_string(), 20);
                let _ = ev.send(Ev::Radio { track_id, tracks });
            }
            Cmd::LoadTrackInfo { track_id } => {
                let _ = ev.send(Ev::Lyrics { track_id, lyrics: lyrics() });
                let _ = ev.send(Ev::Credits { track_id, groups: credits() });
            }
            Cmd::Image(url) => {
                let size = url
                    .rsplit('/')
                    .next()
                    .and_then(|f| f.split('x').next())
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(320)
                    .clamp(64, 640);
                let img = cover_art(&url, size);
                let (width, height) = img.dimensions();
                let _ = ev.send(Ev::Image { url, width: width as usize, height: height as usize, rgba: img.into_raw() });
            }
            _ => {}
        }
    }
}

// ----------------------------------------------------------------- cover art

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    crate::viz::preset::hsv(h, s, v)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Abstract artwork for a cover URL: a gradient with one bold shape, always the same for the same URL.
pub fn cover_art(url: &str, size: u32) -> RgbaImage {
    let mut h = hash(url.rsplit_once('/').map_or(url, |(base, _)| base));
    // spread the bits (splitmix64 finalizer)
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    let unit = |shift: u32| ((h >> shift) & 0xff) as f32 / 255.0;
    let hue = unit(0);
    let (c1, c2) = (hsv(hue, 0.65, 0.95), hsv(hue + 0.12 + unit(8) * 0.25, 0.8, 0.45));
    let accent = hsv(hue + 0.5 + unit(16) * 0.1, 0.45, 1.0);
    let style = (h >> 24) % 6;
    let (cx, cy) = (0.3 + unit(32) * 0.4, 0.3 + unit(40) * 0.4);
    let mut img = RgbaImage::new(size, size);
    for (px, py, pixel) in img.enumerate_pixels_mut() {
        let (x, y) = (px as f32 / size as f32, py as f32 / size as f32);
        let mut col = mix(c1, c2, (x * 0.4 + y * 0.9).clamp(0.0, 1.0));
        let (dx, dy) = (x - cx, y - cy);
        let d = (dx * dx + dy * dy).sqrt();
        let blend = |col: &mut [f32; 3], with: [f32; 3], a: f32| *col = mix(*col, with, a.clamp(0.0, 1.0));
        match style {
            0 => blend(&mut col, accent, (0.2 - d) * 60.0),
            1 => blend(&mut col, accent, (((d * 28.0).sin() * 0.5 + 0.5) - 0.55) * 6.0 * (1.0 - d * 1.6).max(0.0)),
            2 => blend(&mut col, accent, (((x + y) * 9.0).sin() * 0.5 + 0.5 - 0.6) * 5.0),
            3 => {
                let (gx, gy) = ((x * 7.0).fract() - 0.5, (y * 7.0).fract() - 0.5);
                blend(&mut col, accent, (0.28 - (gx * gx + gy * gy).sqrt()) * 25.0 * 0.7);
            }
            4 => blend(&mut col, accent, (0.025 - (y - 0.5 - 0.18 * (x * 11.0 + cx * 9.0).sin()).abs()) * 40.0),
            _ => {
                // a diamond
                let diamond = (x - cx).abs() + (y - cy).abs();
                blend(&mut col, accent, (0.28 - diamond) * 40.0);
            }
        }
        // soft vignette
        let v = 1.0 - ((x - 0.5).powi(2) + (y - 0.5).powi(2)) * 0.5;
        *pixel = image::Rgba([(col[0] * v * 255.0) as u8, (col[1] * v * 255.0) as u8, (col[2] * v * 255.0) as u8, 255]);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_consistent() {
        let cat = catalog();
        assert_eq!(cat.artists.len(), ARTISTS.len());
        assert!(all_tracks().len() > 150);
        let ids: std::collections::HashSet<u64> = all_tracks().iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), all_tracks().len(), "unique track ids");
        let titles: std::collections::HashSet<String> = (0..200).map(title).collect();
        assert_eq!(titles.len(), 200, "unique song titles");
        let page = artist_page(100);
        assert!(page.top_tracks.windows(2).any(|w| w[0].album_title() != w[1].album_title()), "top tracks from several releases");
        assert!(!search("night").tracks.is_empty());
        assert_eq!(artist_page(100).albums.len(), 2);
        assert_eq!(tracks_for("album:9000").len(), cat.album_tracks[0].len());
        assert_eq!(tracks_for("liked").len(), 36);
    }

    #[test]
    fn cover_art_is_stable_and_varied() {
        let a = cover_art("https://resources.tidal.com/images/c0000001/x/320x320.jpg", 64);
        let b = cover_art("https://resources.tidal.com/images/c0000001/x/640x640.jpg", 64);
        assert_eq!(a.as_raw(), b.as_raw(), "same art at every size");
        let c = cover_art("https://resources.tidal.com/images/c0000002/x/320x320.jpg", 64);
        assert_ne!(a.as_raw(), c.as_raw());
        if let Some(dir) = std::env::var_os("TIDALFAST_DEMO_DUMP") {
            for n in 0..12u64 {
                cover_art(&format!("https://resources.tidal.com/images/{}/320x320.jpg", cover_id('c', 9000 + n)), 200)
                    .save(std::path::Path::new(&dir).join(format!("cover{n}.png")))
                    .unwrap();
            }
        }
    }

    /// With `TIDALFAST_DEMO_DUMP=<dir>`: the overlay card for the demo's current song, as a PNG with transparency.
    #[test]
    fn dumps_the_overlay_card() {
        let Some(dir) = std::env::var_os("TIDALFAST_DEMO_DUMP") else { return };
        let tracks = tracks_for("demo-late-night");
        let (now, next) = (&tracks[1], &tracks[2]);
        let cover = cover_art(&now.cover_url(320).unwrap(), crate::card::COVER_PX);
        let label = format!("{} — {}", next.title, next.artist_name());
        let card = crate::card::render(&now.title, &now.artist_name(), Some(&label), Some(&cover), Some(0.42), 1.0);
        // premultiplied → straight alpha for the PNG
        let mut px = card.pixels.clone();
        for p in px.chunks_mut(4) {
            let alpha = p[3] as u32;
            if alpha > 0 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 / alpha).min(255)) as u8;
                }
            }
        }
        RgbaImage::from_raw(card.width, card.height, px).unwrap().save(std::path::Path::new(&dir).join("card.png")).unwrap();
    }
}
