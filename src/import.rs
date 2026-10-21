//! Reading Spotify links without the Spotify API: playlists for import, and
//! tracks, albums and artists as a fallback when linkhop can't convert them.
//! Everything comes from Spotify's public embed pages, matched against TIDAL.

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::model::{Album, Artist, ImportTrack, Track};

pub struct Embed {
    pub name: String,
    /// Artist (albums), owner (playlists) or empty.
    pub subtitle: String,
    pub cover: Option<String>,
    pub tracks: Vec<ImportTrack>,
}

pub fn embed_url(kind: &str, id: &str) -> String {
    format!("https://open.spotify.com/embed/{kind}/{id}")
}

const USER_AGENTS: [&str; 3] = [
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
];

/// Download and read an embed page. Transient failures (network, 429, 5xx)
/// are retried with another browser identity and a growing pause.
pub async fn fetch_embed(http: &reqwest::Client, kind: &str, id: &str) -> Result<Embed> {
    let url = embed_url(kind, id);
    let mut last = anyhow!("Could not reach Spotify");
    for (attempt, agent) in USER_AGENTS.iter().enumerate() {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
        }
        let response = http
            .get(&url)
            .header("User-Agent", *agent)
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(Duration::from_secs(20))
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            Err(e) => {
                last = anyhow!("Could not reach Spotify ({e})");
                continue;
            }
        };
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("Spotify can't find this {kind} (is it public?)"));
        }
        if !status.is_success() {
            last = anyhow!("Spotify answered with {status}");
            continue;
        }
        let html = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                last = anyhow!("Could not read Spotify's answer ({e})");
                continue;
            }
        };
        match parse_embed(&html) {
            Ok(embed) => return Ok(embed),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Pull the entity out of the embed page's `__NEXT_DATA__` JSON.
pub fn parse_embed(html: &str) -> Result<Embed> {
    let unreadable = || anyhow!("Could not read the Spotify page (is it public?)");
    let marker = html.find("__NEXT_DATA__").ok_or_else(unreadable)?;
    let rest = &html[marker..];
    let from = rest.find('>').ok_or_else(unreadable)? + 1;
    let len = rest[from..].find("</script>").ok_or_else(unreadable)?;
    let data: Value = serde_json::from_str(&rest[from..from + len])?;
    let entity = [
        "/props/pageProps/state/data/entity",
        "/props/pageProps/data/entity",
        "/props/pageProps/entity",
    ]
    .iter()
    .find_map(|p| data.pointer(p).filter(|e| e.is_object()))
    .or_else(|| find_entity(&data, 0))
    .ok_or_else(unreadable)?;

    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(clean).filter(|s| !s.is_empty());
    let name = text(entity.get("name")).or_else(|| text(entity.get("title"))).unwrap_or_else(|| "Spotify import".into());
    let cover = entity
        .pointer("/coverArt/sources")
        .or_else(|| entity.pointer("/visualIdentity/image"))
        .and_then(Value::as_array)
        .and_then(|s| s.last())
        .and_then(|s| s.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut tracks: Vec<ImportTrack> = entity
        .get("trackList")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(import_track).collect())
        .unwrap_or_default();
    // a single track has no list: the entity is the track
    if tracks.is_empty() && entity.get("type").and_then(Value::as_str) == Some("track") {
        let artists = entity
            .get("artists")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| text(x.get("name"))).collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        tracks.push(ImportTrack {
            title: name.clone(),
            artists,
            duration: (entity.get("duration").and_then(Value::as_u64).unwrap_or(0) / 1000) as u32,
        });
    }
    let subtitle = if entity.get("type").and_then(Value::as_str) == Some("track") {
        tracks.first().map(|t| t.artists.clone()).unwrap_or_default()
    } else {
        text(entity.get("subtitle")).unwrap_or_default()
    };
    Ok(Embed { name, subtitle, cover, tracks })
}

fn import_track(t: &Value) -> Option<ImportTrack> {
    Some(ImportTrack {
        title: clean(t.get("title")?.as_str()?),
        artists: clean(t.get("subtitle").and_then(Value::as_str).unwrap_or("")),
        duration: (t.get("duration").and_then(Value::as_u64).unwrap_or(0) / 1000) as u32,
    })
}

/// If Spotify moves the entity, look for the first object that looks like one.
fn find_entity(v: &Value, depth: usize) -> Option<&Value> {
    if depth > 8 {
        return None;
    }
    match v {
        Value::Object(map) => {
            if map.contains_key("trackList") || (map.get("type").is_some_and(|t| t == "track") && map.contains_key("uri")) {
                return Some(v);
            }
            map.values().find_map(|c| find_entity(c, depth + 1))
        }
        Value::Array(list) => list.iter().find_map(|c| find_entity(c, depth + 1)),
        _ => None,
    }
}

/// Spotify separates artists with ",\u{a0}" and pads with odd spaces.
fn clean(text: &str) -> String {
    text.replace(['\u{a0}', '\u{200b}'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The main artist of a comma-separated list.
pub fn first_artist(artists: &str) -> &str {
    artists.split(',').next().unwrap_or("").trim()
}

/// Search queries for a track, from most to least specific.
pub fn search_queries(src: &ImportTrack) -> Vec<String> {
    let artist = first_artist(&src.artists);
    let plain = src.title.split(" - ").next().unwrap_or(&src.title);
    let mut plain_title = String::new();
    let mut depth = 0;
    for c in plain.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => plain_title.push(c),
            _ => {}
        }
    }
    let plain_title = plain_title.trim().to_string();
    let mut queries = vec![format!("{} {}", src.title, artist)];
    if !plain_title.is_empty() && plain_title != src.title {
        queries.push(format!("{plain_title} {artist}"));
    }
    queries.push(src.title.clone());
    queries.dedup();
    queries
}

/// Lowercase letters and digits only, without "(feat. …)" or "- Remastered" suffixes.
fn norm(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    let text = text.split(" - ").next().unwrap_or(text);
    for c in text.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            c if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => {}
        }
    }
    out
}

fn score(src: &ImportTrack, cand: &Track) -> f32 {
    let (a, b) = (norm(&src.title), norm(&cand.title));
    let title = if a.is_empty() || b.is_empty() {
        0.0
    } else if a == b {
        2.0
    } else if a.contains(&b) || b.contains(&a) {
        1.0
    } else {
        0.0
    };
    let artists = norm(&src.artists);
    let artist = if cand
        .artist_list()
        .iter()
        .any(|x| !norm(&x.name).is_empty() && artists.contains(&norm(&x.name)))
    {
        2.0
    } else {
        0.0
    };
    if title == 0.0 || artist == 0.0 {
        return 0.0;
    }
    let diff = (src.duration as i64 - cand.duration as i64).unsigned_abs();
    let duration = match diff {
        0..=2 => 1.5,
        3..=5 => 0.75,
        _ => 0.0,
    };
    title + artist + duration
}

/// The best candidate and whether the match is trustworthy.
pub fn best_match(src: &ImportTrack, candidates: &[Track]) -> Option<(usize, bool)> {
    let mut best: Option<(usize, f32)> = None;
    for (i, cand) in candidates.iter().enumerate() {
        let s = score(src, cand);
        if s > 0.0 && best.is_none_or(|(_, b)| s > b) {
            best = Some((i, s));
        }
    }
    best.map(|(i, s)| (i, s >= 4.5))
}

/// The TIDAL album that matches a Spotify album: same title, preferably the same artist.
pub fn best_album(name: &str, artist: &str, candidates: &[Album]) -> Option<usize> {
    let raw = name.trim().to_lowercase();
    let (name, artist) = (norm(name), norm(artist));
    let mut best: Option<(usize, u32)> = None;
    for (i, album) in candidates.iter().enumerate() {
        let title = norm(&album.title);
        let title_score = if title.is_empty() || name.is_empty() {
            0
        } else if title == name {
            3
        } else if title.contains(&name) || name.contains(&title) {
            1
        } else {
            0
        };
        if title_score == 0 {
            continue;
        }
        let names = album.artists.iter().chain(album.artist.iter());
        let artist_score = if names.into_iter().any(|a| !norm(&a.name).is_empty() && artist.contains(&norm(&a.name))) { 2 } else { 0 };
        // a different artist is only acceptable for an exact title
        if artist_score == 0 && title_score < 3 {
            continue;
        }
        // "Emotion (Deluxe)" beats "Emotion" when that is what was asked for
        let exact = u32::from(album.title.trim().to_lowercase() == raw);
        let score = title_score + artist_score + exact;
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i)
}

/// The TIDAL artist with exactly this name.
pub fn best_artist(name: &str, candidates: &[Artist]) -> Option<usize> {
    let name = norm(name);
    candidates.iter().position(|a| !name.is_empty() && norm(&a.name) == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Artist;

    fn track(title: &str, artist: &str, duration: u32) -> Track {
        Track {
            id: 1,
            title: title.into(),
            duration,
            artists: Some(vec![Artist { id: 1, name: artist.into(), picture: None }]),
            ..Default::default()
        }
    }

    fn src(title: &str, artists: &str, duration: u32) -> ImportTrack {
        ImportTrack { title: title.into(), artists: artists.into(), duration }
    }

    #[test]
    fn exact_match_is_sure() {
        let c = [track("Blinding Lights", "The Weeknd", 200)];
        assert_eq!(best_match(&src("Blinding Lights", "The Weeknd", 201), &c), Some((0, true)));
    }

    #[test]
    fn remaster_suffix_and_features_ignored() {
        let c = [track("Africa", "TOTO", 295)];
        let m = best_match(&src("Africa - Remastered 2014", "TOTO", 295), &c);
        assert_eq!(m, Some((0, true)));
        let c = [track("Song (feat. X)", "A", 100)];
        assert!(best_match(&src("Song", "A, X", 100), &c).is_some());
    }

    #[test]
    fn wrong_artist_or_title_rejected() {
        let c = [track("Blinding Lights", "Someone Else", 200)];
        assert!(best_match(&src("Blinding Lights", "The Weeknd", 200), &c).is_none());
        let c = [track("Other Song", "The Weeknd", 200)];
        assert!(best_match(&src("Blinding Lights", "The Weeknd", 200), &c).is_none());
    }

    #[test]
    fn different_length_is_unsure() {
        let c = [track("Song", "Band", 400)];
        assert_eq!(best_match(&src("Song", "Band", 200), &c), Some((0, false)));
    }

    fn page(json: &str) -> String {
        format!("<html><script id=\"__NEXT_DATA__\" type=\"application/json\">{json}</script></html>")
    }

    #[test]
    fn reads_playlist_with_odd_spaces() {
        let html = page(r#"{"props":{"pageProps":{"state":{"data":{"entity":{"type":"playlist","name":"Hits","subtitle":"Spotify",
            "trackList":[{"title":"expectations (feat. Chappell Roan)","subtitle":"Olivia Rodrigo,\u00a0Chappell Roan","duration":223935}]}}}}}}"#);
        let e = parse_embed(&html).unwrap();
        assert_eq!(e.name, "Hits");
        assert_eq!(e.tracks[0].artists, "Olivia Rodrigo, Chappell Roan");
        assert_eq!(first_artist(&e.tracks[0].artists), "Olivia Rodrigo");
        assert_eq!(e.tracks[0].duration, 223);
    }

    #[test]
    fn reads_a_single_track_and_a_moved_entity() {
        let html = page(r#"{"props":{"pageProps":{"state":{"data":{"entity":{"type":"track","name":"Never Gonna Give You Up",
            "artists":[{"name":"Rick Astley"}],"duration":213573}}}}}}"#);
        let e = parse_embed(&html).unwrap();
        assert_eq!((e.tracks[0].title.as_str(), e.subtitle.as_str(), e.tracks[0].duration), ("Never Gonna Give You Up", "Rick Astley", 213));
        // Spotify rearranges its page: the entity is found anyway
        let html = page(r#"{"props":{"somewhere":{"deeper":[{"type":"album","name":"X","uri":"spotify:album:1","trackList":[{"title":"A","subtitle":"B","duration":1000}]}]}}}"#);
        assert_eq!(parse_embed(&html).unwrap().tracks.len(), 1);
        assert!(parse_embed("<html>nothing</html>").is_err());
    }

    #[test]
    fn searches_get_looser() {
        let q = search_queries(&src("Song (feat. X) - Remastered", "A, B", 100));
        assert_eq!(q[0], "Song (feat. X) - Remastered A");
        assert!(q.contains(&"Song A".to_string()) && q.contains(&"Song (feat. X) - Remastered".to_string()));
    }

    #[test]
    fn albums_and_artists_match_by_name() {
        let album = |title: &str, artist: &str| Album {
            id: 1,
            title: title.into(),
            artists: vec![Artist { id: 1, name: artist.into(), picture: None }],
            ..Default::default()
        };
        let list = [album("Emotion", "Someone Else"), album("Emotion (Deluxe)", "Carly Rae Jepsen"), album("Emotion", "Carly Rae Jepsen")];
        assert_eq!(best_album("Emotion", "Carly Rae Jepsen", &list), Some(2));
        assert_eq!(best_album("Nothing Alike", "Carly Rae Jepsen", &list), None);
        let artists = [Artist { id: 1, name: "Band of Horses Tribute".into(), picture: None }, Artist { id: 2, name: "Band of Horses".into(), picture: None }];
        assert_eq!(best_artist("Band of Horses", &artists), Some(1));
    }

    /// Manual check against the real services: `cargo test live_import -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_import() {
        let cfg = crate::config::Settings::load();
        let tokens = crate::tidal::Tokens {
            access_token: cfg.access_token.clone(),
            refresh_token: cfg.refresh_token.clone(),
            user_id: cfg.user_id,
            ..Default::default()
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mut c = crate::tidal::TidalClient::new(cfg.country_code.clone()).unwrap();
            c.set_tokens(&tokens);
            let embed = fetch_embed(&c.http(), "playlist", "37i9dQZF1DX4UtSsGT1Sbe").await.unwrap();
            println!("{} ({} tracks)", embed.name, embed.tracks.len());
            let (mut sure, mut unsure, mut missing) = (0, 0, 0);
            for src in embed.tracks.iter().take(30) {
                let q = format!("{} {}", src.title, first_artist(&src.artists));
                let res = c.search(&q).await.unwrap();
                match best_match(src, &res.tracks) {
                    Some((i, true)) => { sure += 1; let _ = i; }
                    Some((i, false)) => { unsure += 1; println!("UNSURE {} - {} => {} - {}", src.title, src.artists, res.tracks[i].title, res.tracks[i].artist_name()); }
                    None => { missing += 1; println!("MISSING {} - {}", src.title, src.artists); }
                }
            }
            println!("sure {sure} unsure {unsure} missing {missing}");
        });
    }
}
