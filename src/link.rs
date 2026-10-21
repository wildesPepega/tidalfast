//! Recognises Spotify and TIDAL links (web URLs and `spotify:` URIs).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Artist,
    Playlist,
}

impl Kind {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "track" => Some(Kind::Track),
            "album" => Some(Kind::Album),
            "artist" => Some(Kind::Artist),
            "playlist" => Some(Kind::Playlist),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Artist => "artist",
            Kind::Playlist => "playlist",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Service {
    Spotify,
    Tidal,
    /// A YouTube / YouTube Music video; `id` is the video id.
    YouTube,
    /// A linkhop share page (`linkhop.paul.wtf/c/<id>`); `id` is the short id.
    /// For YouTube and linkhop links `kind` is a placeholder until resolved.
    Linkhop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub service: Service,
    pub kind: Kind,
    pub id: String,
}

impl Link {
    /// The canonical web URL, used for the linkhop lookup.
    pub fn url(&self) -> String {
        match self.service {
            Service::Spotify => format!("https://open.spotify.com/{}/{}", self.kind.word(), self.id),
            Service::Tidal => format!("https://tidal.com/{}/{}", self.kind.word(), self.id),
            Service::YouTube => format!("https://music.youtube.com/watch?v={}", self.id),
            Service::Linkhop => format!("https://linkhop.paul.wtf/c/{}", self.id),
        }
    }
}

/// Parse a pasted URL, a `spotify:track:<id>` URI or a `spotify://track/<id>` URL.
pub fn parse(input: &str) -> Option<Link> {
    let input = input.trim();
    if let Some(rest) = input.strip_prefix("spotify:") {
        let rest = rest.trim_start_matches("//");
        let mut parts = rest.split(|c| c == ':' || c == '/');
        let mut kind = parts.next()?;
        // `spotify:user:<name>:playlist:<id>`
        if kind == "user" {
            parts.next()?;
            kind = parts.next()?;
        }
        return spotify(kind, parts.next()?);
    }

    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let query = path.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or("")).unwrap_or("");
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match host.trim_start_matches("www.") {
        "open.spotify.com" | "play.spotify.com" => {
            // optional locale prefix and embed / user segments
            while segs.first().is_some_and(|s| s.starts_with("intl-") || *s == "embed") {
                segs.remove(0);
            }
            if segs.first() == Some(&"user") && segs.len() >= 4 {
                segs.drain(..2);
            }
            spotify(segs.first()?, segs.get(1)?)
        }
        "youtu.be" => video(segs.first()?),
        "youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            if segs.first() != Some(&"watch") {
                return None;
            }
            let id = query.split('&').find_map(|kv| kv.strip_prefix("v="))?;
            video(id)
        }
        "linkhop.paul.wtf" => {
            if segs.first() != Some(&"c") {
                return None;
            }
            let id = *segs.get(1)?;
            id.chars()
                .all(|c| c.is_ascii_alphanumeric())
                .then(|| Link { service: Service::Linkhop, kind: Kind::Track, id: id.to_string() })
        }
        "tidal.com" | "listen.tidal.com" | "desktop.tidal.com" => {
            while segs.first().is_some_and(|s| *s == "browse" || *s == "u") {
                segs.remove(0);
            }
            let kind = Kind::parse(segs.first()?)?;
            let id = *segs.get(1)?;
            let valid = if kind == Kind::Playlist {
                id.len() >= 32 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            } else {
                id.chars().all(|c| c.is_ascii_digit())
            };
            valid.then(|| Link { service: Service::Tidal, kind, id: id.to_string() })
        }
        _ => None,
    }
}

fn video(id: &str) -> Option<Link> {
    let valid = id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then(|| Link { service: Service::YouTube, kind: Kind::Track, id: id.to_string() })
}

fn spotify(kind: &str, id: &str) -> Option<Link> {
    let kind = Kind::parse(kind)?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| Link { service: Service::Spotify, kind, id: id.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_web() {
        let l = parse("https://open.spotify.com/intl-de/track/4cOdK2wGLETKBW3PvgPWqT?si=abc").unwrap();
        assert_eq!(l.service, Service::Spotify);
        assert_eq!(l.kind, Kind::Track);
        assert_eq!(l.id, "4cOdK2wGLETKBW3PvgPWqT");
    }

    #[test]
    fn spotify_uri() {
        assert_eq!(parse("spotify:album:1DFixLWuPkv3KT3TnV35m3").unwrap().kind, Kind::Album);
        assert_eq!(parse("spotify://artist/0OdUWJ0sBjDrqHygGUXeCF").unwrap().kind, Kind::Artist);
        assert_eq!(
            parse("spotify:user:someone:playlist:37i9dQZF1DXcBWIGoYBM5M").unwrap().kind,
            Kind::Playlist
        );
    }

    #[test]
    fn tidal() {
        let l = parse("https://tidal.com/browse/track/3202471").unwrap();
        assert_eq!((l.service, l.kind, l.id.as_str()), (Service::Tidal, Kind::Track, "3202471"));
        assert!(parse("https://listen.tidal.com/album/573283").is_some());
        assert!(parse("https://tidal.com/browse/playlist/7c2e1d5a-0c0a-4c1c-8f7c-0d3a5b1c2e4f").is_some());
    }

    #[test]
    fn youtube_and_linkhop() {
        for url in [
            "https://www.youtube.com/watch?v=J7p4bzqLvCw&t=5s",
            "https://youtu.be/J7p4bzqLvCw?si=x",
            "https://music.youtube.com/watch?v=J7p4bzqLvCw",
        ] {
            let l = parse(url).unwrap();
            assert_eq!(l.service, Service::YouTube);
            assert_eq!(l.url(), "https://music.youtube.com/watch?v=J7p4bzqLvCw");
        }
        let l = parse("https://linkhop.paul.wtf/c/7uCdPq").unwrap();
        assert_eq!((l.service, l.id.as_str()), (Service::Linkhop, "7uCdPq"));
        assert!(parse("https://www.youtube.com/").is_none());
        assert!(parse("https://linkhop.paul.wtf/").is_none());
    }

    #[test]
    fn rejects_other() {
        assert!(parse("https://example.com/track/1").is_none());
        assert!(parse("hello").is_none());
        assert!(parse("https://open.spotify.com/show/abc").is_none());
    }
}
