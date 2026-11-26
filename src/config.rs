//! Persisted user settings and session tokens.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::eq::EqSettings;
use crate::model::Track;
use crate::transition::{Transition, TransitionMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: Option<u64>,
    pub user_name: String,
    pub user_picture: String,
    pub country_code: String,

    pub volume: f32,
    pub output_device: String,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub discord: bool,
    /// Keep the song in Discord while it is paused (without the running clock).
    pub discord_when_paused: bool,
    /// The texts of the Discord status.
    pub discord_format: crate::presence::DiscordFormat,
    /// Media keys and the desktop's media controls (MPRIS / Windows SMTC).
    pub media_keys: bool,
    /// Tray icon.
    pub tray: bool,
    /// Handle `spotify:` and `tidal:` links (browsers offer to open them in oxytide).
    pub link_handler: bool,
    /// Colour the interface (accent, panels, menus) after the playing song's cover.
    pub theme_from_cover: bool,
    /// The blurred cover and its colours behind the app.
    pub cover_backdrop: bool,
    /// Play the looping videos that some albums have instead of their still cover.
    pub animated_covers: bool,
    /// Let plugins run at all (the master switch).
    pub plugins_enabled: bool,
    /// Accept edits to a plugin's script without asking again (for people who write plugins).
    pub plugin_dev_mode: bool,
    /// What the user decided for each plugin.
    pub plugin_cfg: std::collections::BTreeMap<String, crate::plugins::PluginCfg>,
    /// The theme file in use (its id); empty = the built-in look.
    pub theme: String,
    /// A mirror image below the cover in the full-screen player.
    pub cover_reflection: bool,
    /// A rainbow foil sheen on the cover that follows the mouse.
    pub cover_holo: bool,
    /// Drifting colour fields from the cover instead of the blurred cover as backdrop.
    pub fluid_backdrop: bool,
    /// Shifts the lyrics against the music (milliseconds, + = later), for latency of speakers or Bluetooth.
    pub lyric_offset_ms: i32,
    /// The sung word glows.
    pub lyric_glow: bool,
    /// Lines far from the sung one are softly blurred.
    pub lyric_depth: bool,
    /// How the next cover takes over in the full-screen player.
    pub cover_effect: crate::cover3d::CoverEffect,
    /// Serve the now-playing overlay page on localhost, and the port for it.
    pub overlay: bool,
    pub overlay_port: u16,
    /// Listen on 0.0.0.0 so other devices on the network can reach the overlay and API.
    pub overlay_lan: bool,
    /// Serve the JSON API (`/api/now`, `/api/cover`).
    pub overlay_api: bool,
    /// Offer the card to the GPU Screen Recorder plugin.
    pub overlay_recording: bool,
    /// Look for a new version at startup (and every few hours).
    pub check_updates: bool,
    /// Download and install new versions without asking.
    pub auto_update: bool,
    /// Closing the window keeps oxytide running in the tray.
    pub close_to_tray: bool,
    /// How one track leads into the next.
    pub transition: TransitionMode,
    /// Crossfade length in seconds.
    pub crossfade_secs: f32,

    pub eq: EqSettings,
    pub recent_searches: Vec<String>,
    /// Library sidebar shows names (true) or only covers (false).
    pub sidebar_expanded: bool,
    /// The track that was playing when the app was last used.
    pub last_track: Option<Track>,
    /// Keep the songs that were played on disk (start at once, work offline), and how much space they may take.
    pub cache_audio: bool,
    pub cache_limit_gb: f32,
    /// Where in that track playback was when the app was closed (seconds).
    pub last_position: f64,
    /// Playlist/album/mix the last track was played from (`None` for single songs).
    pub last_source: Option<String>,
    /// Which search hint the last start showed (the next start shows another one).
    pub last_search_hint: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            access_token: String::new(),
            refresh_token: String::new(),
            user_id: None,
            user_name: String::new(),
            user_picture: String::new(),
            country_code: "US".to_string(),
            volume: 0.8,
            output_device: String::new(),
            shuffle: false,
            repeat: RepeatMode::Off,
            discord: true,
            discord_when_paused: false,
            discord_format: Default::default(),
            media_keys: true,
            tray: false,
            link_handler: true,
            theme_from_cover: true,
            cover_backdrop: true,
            animated_covers: true,
            plugins_enabled: true,
            plugin_dev_mode: false,
            plugin_cfg: Default::default(),
            theme: String::new(),
            cover_reflection: true,
            cover_holo: true,
            fluid_backdrop: false,
            lyric_offset_ms: 0,
            lyric_glow: true,
            lyric_depth: true,
            cover_effect: Default::default(),
            overlay: false,
            overlay_port: 7788,
            overlay_lan: false,
            overlay_api: false,
            overlay_recording: false,
            check_updates: true,
            auto_update: false,
            close_to_tray: false,
            transition: TransitionMode::Off,
            crossfade_secs: 6.0,
            eq: EqSettings::default(),
            recent_searches: Vec::new(),
            sidebar_expanded: false,
            last_track: None,
            last_position: 0.0,
            cache_audio: true,
            cache_limit_gb: 2.0,
            last_source: None,
            last_search_hint: usize::MAX,
        }
    }
}

impl Settings {
    pub fn transition(&self) -> Transition {
        Transition { mode: self.transition, seconds: self.crossfade_secs }
    }

    pub fn is_logged_in(&self) -> bool {
        !self.refresh_token.is_empty()
    }

    fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oxytide")
            .join("settings.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    fn load_from(path: &std::path::Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) => return Settings::default(),
        };
        #[cfg(unix)]
        {
            // older versions wrote the file (it holds the sign-in) readable for everyone
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 != 0) {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
        }
        serde_json::from_str(&text).unwrap_or_else(|e| {
            // keep the damaged file: the next save would otherwise throw away the sign-in and every setting
            let backup = path.with_extension("json.broken");
            let kept = crate::fsx::write_private(&backup, text.as_bytes()).is_ok();
            log::warn!("settings parse error ({e}), using defaults{}", if kept { format!("; the old file is kept as {}", backup.display()) } else { String::new() });
            Settings::default()
        })
    }

    pub fn save(&self) {
        // demo runs (screenshots) work on made-up settings: they must never replace the real ones
        #[cfg(debug_assertions)]
        if crate::demo::enabled() {
            return;
        }
        self.save_to(&Self::path());
    }

    fn save_to(&self, path: &std::path::Path) {
        match serde_json::to_string_pretty(self) {
            // private (it holds the sign-in) and atomic (a crash while saving cannot leave half a file)
            Ok(text) => {
                if let Err(e) = crate::fsx::write_private(path, text.as_bytes()) {
                    log::warn!("failed to save settings: {e}");
                }
            }
            Err(e) => log::warn!("failed to serialize settings: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_file_is_kept_aside_and_saving_is_private() {
        let dir = std::env::temp_dir().join(format!("oxytide-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{\"refresh_token\": \"abc\", oops").unwrap();
        let s = Settings::load_from(&path);
        assert!(s.refresh_token.is_empty());
        assert!(std::fs::read_to_string(dir.join("settings.json.broken")).unwrap().contains("abc"));

        let s = Settings { refresh_token: "xyz".into(), ..Settings::default() };
        s.save_to(&path);
        assert_eq!(Settings::load_from(&path).refresh_token, "xyz");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
