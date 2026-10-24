//! Optional "now playing" overlay for streaming and recording.
//!
//! A tiny HTTP server on localhost. `/` is a transparent page (cover, title,
//! artist, progress bar in the bottom left corner) to use as a browser source
//! or in a kiosk window; `/api/now` is the same information as JSON.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("overlay.html");

/// What is playing, as the overlay shows it.
#[derive(Clone, Default, PartialEq)]
pub struct Song {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: String,
    pub duration: f64,
}

#[derive(Default)]
struct Shared {
    song: Option<Song>,
    playing: bool,
    position: f64,
    at: Option<Instant>,
    /// The cover being fetched or fetched (url, picture once it arrived).
    cover: Option<(String, Option<image::RgbaImage>)>,
}

/// The running server (or nothing, while the overlay is off).
pub struct Overlay {
    shared: Arc<Mutex<Shared>>,
    server: Option<(u16, Arc<AtomicBool>)>,
    /// Why the last start failed.
    pub error: Option<String>,
}

impl Overlay {
    pub fn new() -> Self {
        Self { shared: Arc::default(), server: None, error: None }
    }

    /// Start, stop or move the server so it matches the setting.
    pub fn apply(&mut self, enabled: bool, port: u16) {
        if let Some((running, stop)) = &self.server {
            if enabled && *running == port {
                return;
            }
            stop.store(true, Ordering::Relaxed);
            self.server = None;
        }
        if !enabled {
            self.error = None;
            return;
        }
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        match TcpListener::bind(addr).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
            Ok(listener) => {
                let stop = Arc::new(AtomicBool::new(false));
                let (shared, flag) = (self.shared.clone(), stop.clone());
                std::thread::spawn(move || serve(listener, shared, flag));
                self.server = Some((port, stop));
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Port {port}: {e}")),
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
            let url = s.song.as_ref().map(|x| x.cover.clone()).unwrap_or_default();
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
        if let Some((_, stop)) = &self.server {
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
    match path {
        "/" | "/index.html" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE),
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
        "/api/now" => {
            let body = snapshot(shared).to_string();
            respond(&mut stream, "200 OK", "application/json", &body)
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", "not found"),
    }
}

fn fetch_cover(url: &str) -> Option<image::RgbaImage> {
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
    Some(image::imageops::resize(&img, crate::card::COVER_PX, crate::card::COVER_PX, image::imageops::FilterType::Triangle))
}

/// The card as raw pixels: the size in the headers, premultiplied RGBA in the body.
fn card_bytes(shared: &Mutex<Shared>, scale: f32) -> Option<(u32, u32, Vec<u8>)> {
    let s = shared.lock().ok()?;
    let song = s.song.as_ref()?;
    let mut position = s.position;
    if s.playing {
        position += s.at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    }
    let progress = (song.duration > 0.0).then(|| (position / song.duration) as f32);
    let cover = s.cover.as_ref().and_then(|(_, img)| img.as_ref());
    let card = crate::card::render(&song.title, &song.artist, cover, progress, scale);
    Some((card.width, card.height, card.pixels))
}

fn snapshot(shared: &Mutex<Shared>) -> serde_json::Value {
    let Ok(s) = shared.lock() else {
        return serde_json::json!({ "active": false });
    };
    let Some(song) = &s.song else {
        return serde_json::json!({ "active": false });
    };
    let mut position = s.position;
    if s.playing {
        position += s.at.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    }
    serde_json::json!({
        "active": true,
        "id": song.id,
        "title": song.title,
        "artist": song.artist,
        "album": song.album,
        "cover": song.cover,
        "duration": song.duration,
        "position": position.min(song.duration.max(0.0)),
        "playing": s.playing,
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

    #[test]
    fn serves_page_and_state() {
        let mut o = Overlay::new();
        o.apply(true, 47231);
        assert!(o.running(), "{:?}", o.error);
        assert!(get(47231, "/").contains("<title>"));
        assert!(get(47231, "/api/now").contains("\"active\":false"));
        o.set(Some(Song { id: 1, title: "T".into(), duration: 100.0, ..Default::default() }), false, 12.0);
        let json = get(47231, "/api/now");
        assert!(json.contains("\"title\":\"T\"") && json.contains("\"position\":12.0"), "{json}");
        let card = get(47231, "/card.rgba");
        assert!(card.contains("X-Width: 380") && card.contains("X-Height: 96"), "{}", &card[..card.len().min(80)]);
        assert!(get(47231, "/nope").starts_with("HTTP/1.1 404"));
        o.apply(false, 47231);
        std::thread::sleep(Duration::from_millis(300));
        assert!(TcpStream::connect(("127.0.0.1", 47231)).is_err());
    }
}
