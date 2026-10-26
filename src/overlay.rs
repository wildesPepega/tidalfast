//! Optional "now playing" overlay for streaming and recording.
//!
//! A tiny HTTP server (localhost, or all interfaces if asked) with three independent parts:
//! - the overlay page (`/`): a transparent card for a browser source,
//! - the API (`/api/now`, `/api/cover`): what plays now and next, as JSON,
//! - the card as pixels (`/card.rgba`) for the recorder plugin.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("overlay.html");

/// One track as the API describes it.
#[derive(Clone, Default, PartialEq)]
pub struct Brief {
    pub id: u64,
    pub title: String,
    /// All artists, joined with ", ".
    pub artist: String,
    pub artists: Vec<String>,
    pub album: String,
    pub cover: String,
    pub duration: f64,
    pub url: String,
}

impl Brief {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "title": self.title,
            "artist": self.artist,
            "artists": self.artists,
            "album": self.album,
            "duration": self.duration,
            "cover": self.cover,
            "url": self.url,
        })
    }

    /// "Title — Artist"
    fn label(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.title, self.artist)
        }
    }
}

/// What is playing now and what follows.
#[derive(Clone, Default, PartialEq)]
pub struct Song {
    pub now: Brief,
    pub next: Option<Brief>,
}

/// Which parts of the server answer.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Features {
    pub page: bool,
    pub api: bool,
    pub card: bool,
}

impl Features {
    pub fn any(self) -> bool {
        self.page || self.api || self.card
    }
}

/// A downloaded cover: the file as TIDAL sent it and a small decoded copy for the card.
struct Cover {
    bytes: Vec<u8>,
    image: image::RgbaImage,
}

#[derive(Default)]
struct Shared {
    song: Option<Song>,
    playing: bool,
    position: f64,
    at: Option<Instant>,
    features: Features,
    /// The cover being fetched or fetched (url, picture once it arrived).
    cover: Option<(String, Option<Cover>)>,
}

/// The running server (or nothing, while the overlay is off).
pub struct Overlay {
    shared: Arc<Mutex<Shared>>,
    server: Option<(u16, bool, Arc<AtomicBool>)>,
    /// Why the last start failed.
    pub error: Option<String>,
}

impl Overlay {
    pub fn new() -> Self {
        Self { shared: Arc::default(), server: None, error: None }
    }

    /// Start, stop or move the server so it matches the setting.
    /// `all_interfaces`: listen on 0.0.0.0 (other devices can connect) instead of 127.0.0.1.
    pub fn apply(&mut self, enabled: bool, port: u16, all_interfaces: bool) {
        let mut restarting = false;
        if let Some((running, wide, stop)) = &self.server {
            if enabled && *running == port && *wide == all_interfaces {
                return;
            }
            stop.store(true, Ordering::Relaxed);
            self.server = None;
            restarting = true;
        }
        if !enabled {
            self.error = None;
            return;
        }
        let host = if all_interfaces { Ipv4Addr::UNSPECIFIED } else { Ipv4Addr::LOCALHOST };
        let addr = SocketAddr::from((host, port));
        let mut bound = TcpListener::bind(addr);
        // the old server lets go of its port within a moment
        for _ in 0..4 {
            if !restarting || bound.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(60));
            bound = TcpListener::bind(addr);
        }
        match bound.and_then(|l| l.set_nonblocking(true).map(|_| l)) {
            Ok(listener) => {
                let stop = Arc::new(AtomicBool::new(false));
                let (shared, flag) = (self.shared.clone(), stop.clone());
                std::thread::spawn(move || serve(listener, shared, flag));
                self.server = Some((port, all_interfaces, stop));
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Port {port}: {e}")),
        }
    }

    pub fn set_features(&self, features: Features) {
        if let Ok(mut s) = self.shared.lock() {
            s.features = features;
        }
    }

    pub fn running(&self) -> bool {
        self.server.is_some()
    }

    /// Tell the overlay what plays now (cheap; call every frame).
    pub fn set(&self, song: Option<Song>, playing: bool, position: f64) {
        if self.server.is_none() {
            return;
        }
        if let Ok(mut s) = self.shared.lock() {
            s.song = song;
            s.playing = playing;
            s.position = position;
            s.at = Some(Instant::now());
            let url = s.song.as_ref().map(|x| x.now.cover.clone()).unwrap_or_default();
            if url.is_empty() {
                s.cover = None;
            } else if s.cover.as_ref().map(|(u, _)| u) != Some(&url) {
                s.cover = Some((url.clone(), None));
                let shared = self.shared.clone();
                std::thread::spawn(move || {
                    let image = fetch_cover(&url);
                    if let Ok(mut s) = shared.lock() {
                        if s.cover.as_ref().map(|(u, _)| u) == Some(&url) {
                            s.cover = Some((url, image));
                        }
                    }
                });
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        if let Some((_, _, stop)) = &self.server {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

fn serve(listener: TcpListener, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = shared.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &shared);
                });
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn handle(mut stream: TcpStream, shared: &Mutex<Shared>) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let mut parts = request.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = target.split('?').next().unwrap_or("");
    if method != "GET" && method != "HEAD" {
        return respond(&mut stream, "405 Method Not Allowed", "text/plain", "");
    }
    let features = shared.lock().map(|s| s.features).unwrap_or_default();
    let allowed = match path {
        "/" | "/index.html" | "/overlay.json" => features.page,
        "/card.rgba" => features.card,
        p if p.starts_with("/api/") => features.api,
        _ => true,
    };
    if !allowed {
        return respond(&mut stream, "404 Not Found", "application/json", r#"{"error":"switched off in tidalfast's settings"}"#);
    }
    match path {
        "/" | "/index.html" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE),
        "/overlay.json" | "/api/now" => {
            let body = snapshot(shared).to_string();
            respond(&mut stream, "200 OK", "application/json", &body)
        }
        "/api/cover" => {
            let cover = shared.lock().ok().and_then(|s| s.cover.as_ref().and_then(|(_, c)| c.as_ref().map(|c| c.bytes.clone())));
            match cover {
                Some(bytes) => {
                    let kind = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "image/png" } else { "image/jpeg" };
                    respond_bytes(&mut stream, "200 OK", kind, "", &bytes)
                }
                None => respond(&mut stream, "404 Not Found", "application/json", r#"{"error":"no cover"}"#),
            }
        }
        "/card.rgba" => {
            let scale = target
                .split_once('?')
                .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("scale=")))
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(1.0);
            match card_bytes(shared, scale) {
                Some((w, h, px)) => respond_bytes(&mut stream, "200 OK", "application/octet-stream", &format!("X-Width: {w}\r\nX-Height: {h}\r\n"), &px),
                None => respond(&mut stream, "204 No Content", "text/plain", ""),
            }
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
    }
}

fn fetch_cover(url: &str) -> Option<Cover> {
    let bytes = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?
        .get(url)
        .send()
        .ok()?
        .bytes()
        .ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let image = image::imageops::resize(&img, crate::card::COVER_PX, crate::card::COVER_PX, image::imageops::FilterType::Triangle);
    Some(Cover { bytes: bytes.to_vec(), image })
}

/// The card as raw pixels: the size in the headers, premultiplied RGBA in the body.
fn card_bytes(shared: &Mutex<Shared>, scale: f32) -> Option<(u32, u32, Vec<u8>)> {
    let s = shared.lock().ok()?;
    if !s.features.card || !s.playing {
        return None;
    }
    let song = s.song.as_ref()?;
    let progress = (song.now.duration > 0.0).then(|| (position_now(&s) / song.now.duration) as f32);
    let cover = s.cover.as_ref().and_then(|(_, c)| c.as_ref()).map(|c| &c.image);
    let next = song.next.as_ref().map(Brief::label);
    let card = crate::card::render(&song.now.title, &song.now.artist, next.as_deref(), cover, progress, scale);
    Some((card.width, card.height, card.pixels))
}

/// Where the song is now: the last reported position plus the time since.
fn position_now(s: &Shared) -> f64 {
    let mut position = s.position;
    if s.playing {
        position += s.at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    }
    position
}

fn snapshot(shared: &Mutex<Shared>) -> serde_json::Value {
    let Ok(s) = shared.lock() else {
        return serde_json::json!({ "active": false, "playing": false, "track": null, "next": null });
    };
    let Some(song) = &s.song else {
        return serde_json::json!({ "active": false, "playing": false, "track": null, "next": null });
    };
    let duration = song.now.duration.max(0.0);
    let position = position_now(&s).min(duration);
    serde_json::json!({
        "active": true,
        "playing": s.playing,
        "position": position,
        "duration": duration,
        "progress": if duration > 0.0 { position / duration } else { 0.0 },
        "track": song.now.json(),
        "next": song.next.as_ref().map(Brief::json),
    })
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &str) -> std::io::Result<()> {
    respond_bytes(stream, status, kind, "", body.as_bytes())
}

fn respond_bytes(stream: &mut TcpStream, status: &str, kind: &str, extra: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Access-Control-Allow-Origin: *\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    fn brief(id: u64, title: &str) -> Brief {
        Brief { id, title: title.into(), artist: "A".into(), artists: vec!["A".into()], duration: 100.0, ..Default::default() }
    }

    #[test]
    fn serves_page_api_and_card_independently() {
        let mut o = Overlay::new();
        o.set_features(Features { page: true, api: true, card: true });
        o.apply(true, 47231, false);
        assert!(o.running(), "{:?}", o.error);
        assert!(get(47231, "/").contains("<title>"));
        assert!(get(47231, "/api/now").contains("\"active\":false"));
        o.set(Some(Song { now: brief(1, "T"), next: Some(brief(2, "N")) }), false, 12.0);
        let json = get(47231, "/api/now");
        assert!(json.contains("\"title\":\"T\"") && json.contains("\"position\":12.0"), "{json}");
        assert!(json.contains("\"next\":{") && json.contains("\"title\":\"N\"") && json.contains("\"progress\":0.12"), "{json}");
        assert!(get(47231, "/api/cover").starts_with("HTTP/1.1 404"), "no cover without url");
        assert!(get(47231, "/card.rgba").starts_with("HTTP/1.1 204"), "paused: no card");
        o.set(Some(Song { now: brief(1, "T"), next: Some(brief(2, "N")) }), true, 12.0);
        let card = get(47231, "/card.rgba");
        assert!(card.contains("X-Width: 320") && card.contains("X-Height: 94"), "{}", &card[..card.len().min(80)]);
        assert!(get(47231, "/nope").starts_with("HTTP/1.1 404"));

        o.set_features(Features { page: false, api: true, card: false });
        assert!(get(47231, "/").contains("switched off"));
        assert!(get(47231, "/overlay.json").contains("switched off"));
        assert!(get(47231, "/card.rgba").contains("switched off"));
        assert!(get(47231, "/api/now").contains("\"title\":\"T\""));
        o.set_features(Features { page: true, api: false, card: false });
        assert!(get(47231, "/api/now").contains("switched off"));
        assert!(get(47231, "/overlay.json").contains("\"title\":\"T\""));

        o.apply(false, 47231, false);
        std::thread::sleep(Duration::from_millis(300));
        assert!(TcpStream::connect(("127.0.0.1", 47231)).is_err());
    }

    #[test]
    #[ignore = "needs the network"]
    fn serves_the_cover() {
        let mut o = Overlay::new();
        o.set_features(Features { page: false, api: true, card: false });
        o.apply(true, 47232, false);
        let mut b = brief(1, "T");
        b.cover = "https://resources.tidal.com/images/bdc3106f/c00d/4ff6/8df2/7139ec2ca491/320x320.jpg".into();
        o.set(Some(Song { now: b, next: None }), true, 0.0);
        for _ in 0..40 {
            if get(47232, "/api/cover").starts_with("HTTP/1.1 200") {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        panic!("no cover");
    }

    #[test]
    fn listens_on_all_interfaces_only_when_asked() {
        // this computer's address on the network (skip where there is none)
        let lan = std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|u| u.connect("192.0.2.1:9").map(|_| u))
            .and_then(|u| u.local_addr())
            .map(|a| a.ip());
        let Ok(lan) = lan.map(|ip| ip.to_string()) else { return };
        if lan.starts_with("127.") {
            return;
        }
        let mut o = Overlay::new();
        o.set_features(Features { page: true, api: false, card: false });
        o.apply(true, 47233, false);
        assert!(TcpStream::connect((lan.as_str(), 47233)).is_err(), "closed to the network by default");
        o.apply(true, 47233, true);
        assert!(TcpStream::connect((lan.as_str(), 47233)).is_ok(), "open on {lan}");
        assert!(TcpStream::connect(("127.0.0.1", 47233)).is_ok());
    }
}
