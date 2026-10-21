//! Data model shared between the TIDAL API client, the network worker and the UI.

use serde::{Deserialize, Serialize};

/// A single playable track.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub duration: u32,
    #[serde(default)]
    pub artist: Option<Artist>,
    #[serde(default)]
    pub artists: Option<Vec<Artist>>,
    #[serde(default)]
    pub album: Option<Album>,
    #[serde(default)]
    pub audio_quality: Option<String>,
    #[serde(default)]
    pub track_number: Option<u32>,
    /// Video thumbnail UUID (videos carry `imageId` instead of an album cover).
    #[serde(default)]
    pub image_id: Option<String>,
    #[serde(default)]
    pub explicit: Option<bool>,
    #[serde(default)]
    pub url: Option<String>,
}

impl Track {
    /// All artists of the track (main and featured), without duplicates.
    pub fn artist_list(&self) -> Vec<Artist> {
        let mut list: Vec<Artist> = Vec::new();
        for a in self.artists.iter().flatten().chain(self.artist.iter()) {
            if !a.name.is_empty() && !list.iter().any(|x| x.id == a.id && x.name == a.name) {
                list.push(a.clone());
            }
        }
        list
    }

    /// Every artist name, comma separated.
    pub fn artist_name(&self) -> String {
        self.artist_list()
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn album_title(&self) -> String {
        self.album
            .as_ref()
            .map(|a| a.title.clone())
            .unwrap_or_default()
    }

    /// The cover UUID, taken from the album or a video thumbnail.
    fn cover_id(&self) -> Option<&str> {
        self.album
            .as_ref()
            .and_then(|a| a.cover.as_deref())
            .or(self.image_id.as_deref())
    }

    /// A `https://resources.tidal.com/images/...` URL for the given square size.
    pub fn cover_url(&self, size: u32) -> Option<String> {
        self.cover_id().map(|id| tidal_image_url(id, size))
    }

    pub fn duration_label(&self) -> String {
        format_duration(self.duration as f64)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub picture: Option<String>,
}

impl Artist {
    pub fn picture_url(&self, size: u32) -> Option<String> {
        self.picture.as_deref().map(|id| tidal_image_url(id, size))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub artist: Option<Artist>,
    #[serde(default)]
    pub number_of_tracks: Option<u32>,
    #[serde(default)]
    pub release_date: Option<String>,
    /// "ALBUM", "EP" or "SINGLE".
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub explicit: Option<bool>,
}

/// TIDAL lists one release several times (clean and explicit versions,
/// re-registrations under new ids). Keep one per title and type: preferably
/// the explicit one with the most tracks, otherwise the original (lowest id).
pub fn dedupe_albums(albums: Vec<Album>) -> Vec<Album> {
    let key = |a: &Album| {
        let title = a.title.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
        (title, a.kind.clone().unwrap_or_default())
    };
    let rank = |a: &Album| (a.explicit.unwrap_or(false), a.number_of_tracks.unwrap_or(0), std::cmp::Reverse(a.id));
    let mut best: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for (i, album) in albums.iter().enumerate() {
        best.entry(key(album))
            .and_modify(|j| {
                if rank(album) > rank(&albums[*j]) {
                    *j = i;
                }
            })
            .or_insert(i);
    }
    // keep the original order
    let keep: std::collections::HashSet<usize> = best.into_values().collect();
    albums.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, a)| a).collect()
}

impl Album {
    /// The release year, if known.
    pub fn year(&self) -> Option<&str> {
        self.release_date.as_deref().and_then(|d| d.get(..4))
    }

    /// A playlist-shaped handle so albums can share the collection views.
    pub fn as_playlist(&self) -> Playlist {
        Playlist {
            uuid: format!("album:{}", self.id),
            title: self.title.clone(),
            square_image: self.cover_url(320),
            creator: Some(PlaylistCreator {
                id: None,
                name: Some(self.artist_names()),
            }),
            ..Default::default()
        }
    }

    pub fn cover_url(&self, size: u32) -> Option<String> {
        self.cover.as_deref().map(|id| tidal_image_url(id, size))
    }

    pub fn artist_names(&self) -> String {
        if self.artists.is_empty() {
            return self.artist.as_ref().map(|a| a.name.clone()).unwrap_or_default();
        }
        self.artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub uuid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub square_image: Option<String>,
    #[serde(default)]
    pub number_of_tracks: Option<u32>,
    #[serde(default)]
    pub number_of_videos: Option<u32>,
    #[serde(default)]
    pub creator: Option<PlaylistCreator>,
}

impl Playlist {
    /// A ready-to-use square cover URL (playlist covers are already full URLs).
    pub fn cover_url(&self) -> Option<String> {
        normalize_image(self.square_image.as_deref().or(self.image.as_deref()))
    }

    pub fn track_count(&self) -> u32 {
        self.number_of_tracks.unwrap_or(0) + self.number_of_videos.unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistCreator {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

/// A track of a playlist being imported from another service.
#[derive(Debug, Clone, Default)]
pub struct ImportTrack {
    pub title: String,
    /// Artist names, comma separated.
    pub artists: String,
    /// Length in seconds.
    pub duration: u32,
}

/// Song lyrics; `synced` holds (seconds, line) pairs when timestamps exist.
#[derive(Debug, Clone, Default)]
pub struct Lyrics {
    pub plain: String,
    pub synced: Vec<(f64, String)>,
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.plain.trim().is_empty() && self.synced.is_empty()
    }
}

/// One credit role (Composer, Producer, …) and the people in it.
#[derive(Debug, Clone, Default)]
pub struct CreditGroup {
    pub role: String,
    pub people: Vec<(String, Option<u64>)>,
}

/// Everything shown on an artist page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtistPage {
    pub artist: Artist,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub eps: Vec<Album>,
    pub singles: Vec<Album>,
    pub compilations: Vec<Album>,
    /// False while further parts are still being fetched.
    pub done: bool,
}

/// Resolved stream information for a track.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub url: String,
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub audio_quality: Option<String>,
    #[serde(default)]
    pub bit_depth: Option<u32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
}

/// Turn `https://resources.tidal.com/images/<uuid>/<size>.jpg` from a raw
/// UUID, or pass through an already-complete URL.
pub fn tidal_image_url(id: &str, size: u32) -> String {
    let path = id.replace('-', "/");
    format!("https://resources.tidal.com/images/{path}/{size}x{size}.jpg")
}

fn normalize_image(value: Option<&str>) -> Option<String> {
    let v = value?.trim();
    if v.is_empty() {
        None
    } else if v.starts_with("http") {
        Some(v.to_string())
    } else {
        Some(tidal_image_url(v, 320))
    }
}

pub fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--:--".to_string();
    }
    let total = secs as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: u64, title: &str, kind: &str, tracks: u32, explicit: bool) -> Album {
        Album {
            id,
            title: title.into(),
            kind: Some(kind.into()),
            number_of_tracks: Some(tracks),
            explicit: Some(explicit),
            ..Default::default()
        }
    }

    #[test]
    fn duplicate_releases_collapse_to_one() {
        let list = vec![
            album(3, "Meteora", "ALBUM", 13, false),
            album(1, "Meteora", "ALBUM", 13, false),
            album(7, "Kamikaze", "ALBUM", 13, false),
            album(5, "Kamikaze", "ALBUM", 13, true),
            album(9, "Kamikaze ", "ALBUM", 13, true),
            album(8, "Houdini", "SINGLE", 1, true),
            album(2, "Houdini", "ALBUM", 12, true),
            album(4, "Waiting for the End", "SINGLE", 2, false),
            album(6, "waiting for the end", "SINGLE", 3, false),
        ];
        let ids: Vec<u64> = dedupe_albums(list).iter().map(|a| a.id).collect();
        // lowest id wins ties, explicit and longer versions win otherwise;
        // a single and an album of the same name stay apart
        assert_eq!(ids, vec![1, 5, 8, 2, 6]);
    }
}
