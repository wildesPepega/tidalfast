//! What the Discord status says, and how to write it yourself: the texts are templates with
//! `{variables}` that are filled in from the song that plays.

use serde::{Deserialize, Serialize};

/// The texts of the status and what is shown around them. All of them are templates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordFormat {
    /// "Listening to …" in the member list.
    pub name: String,
    /// First line.
    pub details: String,
    /// Second line.
    pub state: String,
    /// Shown when the mouse is on the cover.
    pub hover: String,
    pub button_label: String,
    pub show_button: bool,
    /// The running clock / progress bar.
    pub show_time: bool,
}

impl Default for DiscordFormat {
    fn default() -> Self {
        Self {
            name: "{title} by {artist}".into(),
            details: "{title}".into(),
            state: "{release}".into(),
            hover: "{album}".into(),
            button_label: "Listen on TIDAL".into(),
            show_button: true,
            show_time: true,
        }
    }
}

/// What is known about the album of a song besides what the song itself carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlbumMeta {
    /// `SINGLE`, `EP`, `ALBUM` …
    pub kind: Option<String>,
    pub year: String,
    pub tracks: u32,
}

/// Everything the variables can say about a song.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub title: String,
    /// All artists, joined with ", ".
    pub artist: String,
    pub main_artist: String,
    pub album: String,
    pub duration: i64,
    pub track_no: u32,
    pub quality: String,
    pub explicit: bool,
    pub meta: AlbumMeta,
}

/// The variables with a short description, for the settings page.
pub const VARIABLES: &[(&str, &str)] = &[
    ("title", "Song title"),
    ("artist", "All artists, separated by commas"),
    ("main_artist", "The first artist"),
    ("album", "Album, EP or single name"),
    ("release", "Single, EP: name or Album: name"),
    ("type", "Single, EP or Album"),
    ("year", "Release year"),
    ("tracks", "Number of tracks on the release"),
    ("track_no", "Track number on the release"),
    ("duration", "Length, like 3:46"),
    ("quality", "Hi-Res, Lossless, High …"),
    ("explicit", "\"Explicit\" for explicit songs"),
];

/// "Single", "EP: Name" or "Album: Name". Without the release's type a release named like the
/// song counts as a single.
pub fn release_label(kind: Option<&str>, album: &str, track_title: &str) -> String {
    let album = album.trim();
    match kind_label(kind, album, track_title) {
        "Single" => "Single".to_string(),
        "EP" if !album.is_empty() => format!("EP: {album}"),
        "Album" if !album.is_empty() => format!("Album: {album}"),
        _ => String::new(),
    }
}

/// "Single", "EP" or "Album" ("" when there is no album at all).
pub fn kind_label(kind: Option<&str>, album: &str, track_title: &str) -> &'static str {
    let album = album.trim();
    let plain = |t: &str| {
        let t = t.to_lowercase();
        t.trim().trim_end_matches("- single").trim_end_matches("(single)").trim().to_string()
    };
    match kind.map(str::to_uppercase).as_deref() {
        Some("SINGLE") => "Single",
        Some("EP") => "EP",
        Some(_) if !album.is_empty() => "Album",
        Some(_) => "",
        None if album.is_empty() => "",
        None if plain(album) == plain(track_title) => "Single",
        None => "Album",
    }
}

/// A readable name for TIDAL's quality codes.
pub fn quality_label(code: &str) -> String {
    match code.to_uppercase().as_str() {
        "HI_RES_LOSSLESS" => "Hi-Res Lossless".into(),
        "HI_RES" => "Hi-Res".into(),
        "LOSSLESS" => "Lossless".into(),
        "HIGH" => "High".into(),
        "LOW" => "Low".into(),
        "" => String::new(),
        other => other.to_string(),
    }
}

fn value(name: &str, i: &Info) -> Option<String> {
    let duration = |secs: i64| format!("{}:{:02}", secs / 60, secs % 60);
    Some(match name {
        "title" => i.title.clone(),
        "artist" => i.artist.clone(),
        "main_artist" => i.main_artist.clone(),
        "album" => i.album.clone(),
        "release" => release_label(i.meta.kind.as_deref(), &i.album, &i.title),
        "type" => kind_label(i.meta.kind.as_deref(), &i.album, &i.title).to_string(),
        "year" => i.meta.year.clone(),
        "tracks" => (i.meta.tracks > 0).then(|| i.meta.tracks.to_string()).unwrap_or_default(),
        "track_no" => (i.track_no > 0).then(|| i.track_no.to_string()).unwrap_or_default(),
        "duration" => duration(i.duration.max(0)),
        "quality" => quality_label(&i.quality),
        "explicit" => if i.explicit { "Explicit".into() } else { String::new() },
        _ => return None,
    })
}

/// Fill the `{variables}` of a template. `{{` and `}}` are literal braces; a variable that does
/// not exist stays as it is, so a typo is visible in the preview.
pub fn render(template: &str, info: &Info) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for n in chars.by_ref() {
                    if n == '}' {
                        closed = true;
                        break;
                    }
                    name.push(n);
                }
                match closed.then(|| value(name.trim().to_lowercase().as_str(), info)).flatten() {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(&name);
                        if closed {
                            out.push('}');
                        }
                    }
                }
            }
            other => out.push(other),
        }
    }
    // tidy: no doubled or dangling spaces where a variable came out empty
    let mut tidy = String::with_capacity(out.len());
    let mut space = false;
    for c in out.trim().chars() {
        if c == ' ' {
            if !space {
                tidy.push(c);
            }
            space = true;
        } else {
            space = false;
            tidy.push(c);
        }
    }
    tidy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Info {
        Info {
            title: "Slow Voltage".into(),
            artist: "Neon Orchard, Kite Harbor".into(),
            main_artist: "Neon Orchard".into(),
            album: "Static Bloom".into(),
            duration: 226,
            track_no: 3,
            quality: "HI_RES_LOSSLESS".into(),
            explicit: false,
            meta: AlbumMeta { kind: Some("ALBUM".into()), year: "2024".into(), tracks: 11 },
        }
    }

    #[test]
    fn the_default_format_is_what_the_status_always_said() {
        let f = DiscordFormat::default();
        let i = song();
        assert_eq!(render(&f.name, &i), "Slow Voltage by Neon Orchard, Kite Harbor");
        assert_eq!(render(&f.details, &i), "Slow Voltage");
        assert_eq!(render(&f.state, &i), "Album: Static Bloom");
        assert_eq!(render(&f.hover, &i), "Static Bloom");
    }

    #[test]
    fn fills_every_variable() {
        let i = song();
        let all: String = VARIABLES.iter().map(|(n, _)| format!("{{{n}}}|")).collect();
        assert_eq!(
            render(&all, &i),
            "Slow Voltage|Neon Orchard, Kite Harbor|Neon Orchard|Static Bloom|Album: Static Bloom|Album|2024|11|3|3:46|Hi-Res Lossless||"
        );
    }

    #[test]
    fn variables_are_case_insensitive_and_may_have_spaces() {
        assert_eq!(render("{ TITLE } ({Year})", &song()), "Slow Voltage (2024)");
    }

    #[test]
    fn unknown_variables_and_loose_braces_stay_visible() {
        assert_eq!(render("{titel} and {title", &song()), "{titel} and {title");
        assert_eq!(render("a } b", &song()), "a } b");
    }

    #[test]
    fn doubled_braces_are_literal() {
        assert_eq!(render("{{title}} = {title}", &song()), "{title} = Slow Voltage");
    }

    #[test]
    fn an_empty_variable_leaves_no_gaps() {
        let mut i = song();
        i.meta.year.clear();
        assert_eq!(render("{title}  ·  {year}  ·  {album}", &i), "Slow Voltage · · Static Bloom");
        assert_eq!(render("  {year}  ", &i), "");
        i.explicit = true;
        assert_eq!(render("{title} {explicit}", &i), "Slow Voltage Explicit");
    }

    #[test]
    fn the_release_line_says_single_ep_or_album() {
        assert_eq!(release_label(Some("SINGLE"), "Midnight Signals - Single", "Midnight Signals"), "Single");
        assert_eq!(release_label(Some("EP"), "Soft Circuitry", "Slow Voltage"), "EP: Soft Circuitry");
        assert_eq!(release_label(Some("ALBUM"), "Static Bloom", "Slow Voltage"), "Album: Static Bloom");
        assert_eq!(release_label(Some("album"), "Static Bloom", "x"), "Album: Static Bloom");
        assert_eq!(release_label(Some("COMPILATION"), "Best Of", "x"), "Album: Best Of");
        assert_eq!(release_label(None, "Paper Lanterns", "Paper Lanterns"), "Single");
        assert_eq!(release_label(None, "Paper Lanterns (Single)", "paper lanterns"), "Single");
        assert_eq!(release_label(None, "Static Bloom", "Golden Static"), "Album: Static Bloom");
        assert_eq!(release_label(None, "", "Golden Static"), "");
        assert_eq!(release_label(Some("EP"), "", "x"), "");
    }

    #[test]
    fn a_format_survives_the_settings_file_and_older_files_without_it() {
        let mut f = DiscordFormat::default();
        f.details = "{title} · {main_artist}".into();
        f.show_time = false;
        let text = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<DiscordFormat>(&text).unwrap(), f);
        // missing fields fall back to the defaults
        let partial: DiscordFormat = serde_json::from_str(r#"{"details":"x"}"#).unwrap();
        assert_eq!(partial.details, "x");
        assert_eq!(partial.state, "{release}");
        assert!(partial.show_button);
    }
}
