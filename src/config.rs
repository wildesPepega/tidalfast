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
    /// Media keys and the desktop's media controls (MPRIS / Windows SMTC).
    pub media_keys: bool,
    /// Tray icon.
    pub tray: bool,
    /// Handle `spotify:` and `tidal:` links (browsers offer to open them in tidalfast).
    pub link_handler: bool,
    /// Visualizer: seconds per preset, render quality (0‥2), hard cuts on the beat, beat sensitivity.
    pub viz_seconds: u32,
    pub viz_quality: u8,
    pub viz_beat_cuts: bool,
    pub viz_sensitivity: f32,
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
    /// Closing the window keeps tidalfast running in the tray.
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
    /// Playlist/album/mix the last track was played from (`None` for single songs).
    pub last_source: Option<String>,
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
            media_keys: true,
            tray: false,
            link_handler: true,
            viz_seconds: 15,
            viz_quality: 1,
            viz_beat_cuts: false,
            viz_sensitivity: 1.0,
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
            last_source: None,
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
            .join("tidalfast")
            .join("settings.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("settings parse error ({e}), using defaults");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    log::warn!("failed to save settings: {e}");
                }
            }
            Err(e) => log::warn!("failed to serialize settings: {e}"),
        }
    }
}
