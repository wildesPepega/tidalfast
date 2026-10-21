//! Spotify playlist import: read a public playlist from its embed page and
//! match its tracks against TIDAL.

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::model::{ImportTrack, Track};

pub struct Embed {
    pub name: String,
    pub cover: Option<String>,
    pub tracks: Vec<ImportTrack>,
}

pub fn embed_url(playlist_id: &str) -> String {
    format!("https://open.spotify.com/embed/playlist/{playlist_id}")
}

/// Pull the playlist out of the embed page's `__NEXT_DATA__` JSON.
pub fn parse_embed(html: &str) -> Result<Embed> {
    let marker = html
        .find("id=\"__NEXT_DATA__\"")
        .ok_or_else(|| anyhow!("Could not read the Spotify playlist (is it public?)"))?;
    let rest = &html[marker..];
    let from = rest.find('>').ok_or_else(|| anyhow!("Unexpected Spotify page"))? + 1;
    let len = rest[from..]
        .find("</script>")
        .ok_or_else(|| anyhow!("Unexpected Spotify page"))?;
    let data: Value = serde_json::from_str(&rest[from..from + len])?;
    let entity = data
        .pointer("/props/pageProps/state/data/entity")
        .ok_or_else(|| anyhow!("Could not read the Spotify playlist (is it public?)"))?;

    let name = ["name", "title"]
        .iter()
        .find_map(|k| entity.get(*k).and_then(Value::as_str))
        .unwrap_or("Spotify import")
        .to_string();
    let cover = entity
        .pointer("/coverArt/sources")
        .and_then(Value::as_array)
        .and_then(|s| s.last())
        .and_then(|s| s.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let tracks = entity
        .get("trackList")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|t| {
                    Some(ImportTrack {
                        title: t.get("title")?.as_str()?.to_string(),
                        artists: t.get("subtitle").and_then(Value::as_str).unwrap_or("").to_string(),
                        duration: (t.get("duration").and_then(Value::as_u64).unwrap_or(0) / 1000) as u32,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Embed { name, cover, tracks })
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
            let html = c
                .http()
                .get(embed_url("37i9dQZF1DX4UtSsGT1Sbe"))
                .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            let embed = parse_embed(&html).unwrap();
            println!("{} ({} tracks)", embed.name, embed.tracks.len());
            let (mut sure, mut unsure, mut missing) = (0, 0, 0);
            for src in embed.tracks.iter().take(30) {
                let q = format!("{} {}", src.title, src.artists.split(", ").next().unwrap_or(""));
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
