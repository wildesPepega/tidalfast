//! What a plugin has put into the interface: buttons, menu entries, settings, and the built-in
//! parts it has hidden. The plugin's worker builds it; the interface only draws it.

/// Where a button goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The bar at the top, next to the settings button.
    Header,
    /// The player bar at the bottom.
    PlayerBar,
    /// The top bar of the full-screen player.
    Now,
}

impl Place {
    pub fn parse(s: &str) -> Option<Place> {
        match s {
            "header" => Some(Place::Header),
            "player" => Some(Place::PlayerBar),
            "now" => Some(Place::Now),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub tooltip: String,
    pub place: Place,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    pub icon: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Heading,
    Label,
    Toggle(bool),
    Text(String),
    Number { value: f64, min: f64, max: f64, step: f64 },
    Choice { options: Vec<String>, value: String },
    Button,
}

/// One line on a settings page.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub help: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub entries: Vec<Entry>,
}

/// Everything one plugin shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registry {
    pub buttons: Vec<Button>,
    pub menu_items: Vec<MenuItem>,
    pub categories: Vec<Category>,
    /// Entries added to a built-in settings page (`playback`, `appearance`, …).
    pub extra: Vec<(String, Entry)>,
    /// Built-in parts hidden by the plugin.
    pub hidden: Vec<String>,
}

pub const MAX_BUTTONS: usize = 8;
pub const MAX_MENU_ITEMS: usize = 16;
pub const MAX_CATEGORIES: usize = 6;
pub const MAX_ENTRIES: usize = 80;

/// The built-in settings pages plugins can add entries to.
pub const BUILTIN_PAGES: [&str; 7] = ["playback", "equalizer", "appearance", "discord", "overlay", "system", "account"];

/// The parts of the interface a plugin may hide (with `ui.modify`), and what each is.
pub const FEATURES: [(&str, &str); 12] = [
    ("lyrics", "The Lyrics tab of the full-screen player"),
    ("credits", "The Credits tab of the full-screen player"),
    ("queue", "The queue button in the player bar"),
    ("like", "The heart button in the player bar"),
    ("volume", "The volume control in the player bar"),
    ("shuffle", "The shuffle button"),
    ("repeat", "The repeat button"),
    ("search", "The search field in the header"),
    ("settings.equalizer", "The Equalizer settings page"),
    ("settings.discord", "The Discord settings page"),
    ("settings.overlay", "The Overlay settings page"),
    ("settings.account", "The Account settings page"),
];

pub fn is_feature(id: &str) -> bool {
    FEATURES.iter().any(|(f, _)| *f == id)
}

/// A glyph for an icon name used by plugins (a few dozen common ones); unknown names get a puzzle piece.
pub fn icon(name: &str) -> &'static str {
    use egui_phosphor::regular as i;
    match name {
        "star" => i::STAR,
        "heart" => i::HEART,
        "plus" => i::PLUS,
        "minus" => i::MINUS,
        "gear" => i::GEAR,
        "download" => i::DOWNLOAD_SIMPLE,
        "upload" => i::UPLOAD_SIMPLE,
        "music" => i::MUSIC_NOTE,
        "play" => i::PLAY,
        "pause" => i::PAUSE,
        "skip-forward" => i::SKIP_FORWARD,
        "skip-back" => i::SKIP_BACK,
        "link" => i::LINK,
        "copy" => i::COPY,
        "trash" => i::TRASH,
        "info" => i::INFO,
        "warning" => i::WARNING,
        "check" => i::CHECK,
        "x" => i::X,
        "search" => i::MAGNIFYING_GLASS,
        "list" => i::LIST,
        "clock" => i::CLOCK,
        "moon" => i::MOON,
        "sun" => i::SUN,
        "bell" => i::BELL,
        "globe" => i::GLOBE,
        "code" => i::CODE,
        "palette" => i::PALETTE,
        "share" => i::SHARE_NETWORK,
        "bookmark" => i::BOOKMARK_SIMPLE,
        "tag" => i::TAG,
        "timer" => i::TIMER,
        "shuffle" => i::SHUFFLE,
        "repeat" => i::REPEAT,
        "microphone" => i::MICROPHONE,
        "headphones" => i::HEADPHONES,
        "speaker" => i::SPEAKER_HIGH,
        "lightning" => i::LIGHTNING,
        "sparkle" => i::SPARKLE,
        "eye" => i::EYE,
        "eye-slash" => i::EYE_SLASH,
        "pencil" => i::PENCIL_SIMPLE,
        "folder" => i::FOLDER_SIMPLE,
        "chart" => i::CHART_BAR,
        _ => i::PUZZLE_PIECE,
    }
}

/// The icon names plugins can use (for the documentation).
pub const ICON_NAMES: [&str; 44] = [
    "star", "heart", "plus", "minus", "gear", "download", "upload", "music", "play", "pause", "skip-forward", "skip-back", "link", "copy",
    "trash", "info", "warning", "check", "x", "search", "list", "clock", "moon", "sun", "bell", "globe", "code", "palette", "share",
    "bookmark", "tag", "timer", "shuffle", "repeat", "microphone", "headphones", "speaker", "lightning", "sparkle", "eye", "eye-slash",
    "pencil", "folder", "chart",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_icon_name_has_its_own_glyph() {
        let fallback = icon("no-such-icon");
        for name in ICON_NAMES {
            assert_ne!(icon(name), fallback, "{name} falls back to the puzzle piece");
        }
    }

    #[test]
    fn features_are_known_by_name() {
        assert!(is_feature("lyrics"));
        assert!(!is_feature("everything"));
    }
}
