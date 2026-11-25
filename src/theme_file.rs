//! Themes made by users: a small TOML file in the `themes` folder next to the settings.
//! See `API.md` for the format.
//!
//! A theme is only colours (no code), so it cannot do anything but change how oxytide looks.
//! The panels are always dark glass; a theme picks the accent, the tint of the glass and the colours
//! of the wallpaper. Colours that would be hard to read are corrected.

use std::path::{Path, PathBuf};

use eframe::egui::Color32;
use serde::Deserialize;

use crate::ui::{self, Theme};

#[derive(Debug, Deserialize)]
struct RawColors {
    accent: String,
    panel: Option<String>,
    wallpaper: Option<Vec<String>>,
    light: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct Raw {
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    follow_cover: bool,
    colors: RawColors,
}

/// A checked theme file.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomTheme {
    /// The file name without `.toml`.
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    /// Follow the cover of the playing song, with these colours when there is nothing to follow.
    pub follow_cover: bool,
    pub accent: Color32,
    pub tint: Color32,
    pub light: f32,
    pub wallpaper: [Color32; 3],
}

impl CustomTheme {
    pub fn theme(&self) -> Theme {
        Theme { accent: self.accent, tint: self.tint, light: self.light }
    }
}

pub fn themes_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("oxytide").join("themes")
}

/// `#rgb` or `#rrggbb`.
pub fn parse_hex(text: &str) -> Result<Color32, String> {
    let t = text.trim().strip_prefix('#').ok_or_else(|| format!("“{text}” is not a colour (write it like #a6e3a1)"))?;
    let digit = |c: char| c.to_digit(16).map(|d| d as u8);
    let digits: Option<Vec<u8>> = t.chars().map(digit).collect();
    let d = digits.ok_or_else(|| format!("“{text}” is not a colour (write it like #a6e3a1)"))?;
    match d.len() {
        3 => Ok(Color32::from_rgb(d[0] * 17, d[1] * 17, d[2] * 17)),
        6 => Ok(Color32::from_rgb(d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5])),
        _ => Err(format!("“{text}” is not a colour (write it like #a6e3a1)")),
    }
}

fn clip(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

pub fn parse(id: &str, text: &str) -> Result<CustomTheme, String> {
    if text.len() > 16 * 1024 {
        return Err("the file is too big".into());
    }
    let raw: Raw = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if raw.name.trim().is_empty() {
        return Err("name is empty".into());
    }
    let accent_in = parse_hex(&raw.colors.accent)?;
    let (h, s, _) = ui::to_hsl(accent_in);
    // the glass is always dark; its hue follows the accent unless the theme says otherwise
    let tint_in = match &raw.colors.panel {
        Some(p) => parse_hex(p)?,
        None => ui::from_hsl(h, (s * 0.6).clamp(0.25, 0.5), 0.115),
    };
    let (th, ts, tl) = ui::to_hsl(tint_in);
    let tint = ui::from_hsl(th, ts, tl.min(0.2));
    // an accent is text and button fill on the glass: lighten it until it can be read
    let glass = ui::panel_solid_for(tint);
    let (ah, as_, mut al) = ui::to_hsl(accent_in);
    let mut accent = accent_in;
    while ui::contrast(accent, glass) < 4.5 && al < 0.95 {
        al += 0.03;
        accent = ui::from_hsl(ah, as_, al);
    }
    let wallpaper = match &raw.colors.wallpaper {
        Some(list) => {
            if list.is_empty() || list.len() > 3 {
                return Err("wallpaper takes one to three colours".into());
            }
            let parsed: Result<Vec<Color32>, String> = list.iter().map(|c| parse_hex(c)).collect();
            let parsed = parsed?;
            [parsed[0], parsed[parsed.len().min(2) - 1], parsed[parsed.len() - 1]]
        }
        None => [
            ui::from_hsl(h, (s * 0.7).clamp(0.3, 0.7), 0.36),
            ui::from_hsl((h + 0.08).rem_euclid(1.0), (s * 0.7).clamp(0.3, 0.7), 0.32),
            ui::from_hsl((h + 0.62).rem_euclid(1.0), (s * 0.6).clamp(0.25, 0.6), 0.36),
        ],
    };
    let mean = wallpaper.iter().map(|c| ui::luminance(*c)).sum::<f32>() / 3.0;
    let light = raw.colors.light.unwrap_or(((mean - 0.18) / 0.45).clamp(0.0, 1.0)).clamp(0.0, 1.0);
    Ok(CustomTheme {
        id: id.to_string(),
        name: clip(&raw.name, 40),
        author: clip(&raw.author, 60),
        description: clip(&raw.description, 200),
        follow_cover: raw.follow_cover,
        accent,
        tint,
        light,
        wallpaper,
    })
}

/// All themes in a folder, and what is wrong with the ones that cannot be used.
pub fn discover(dir: &Path) -> (Vec<CustomTheme>, Vec<(String, String)>) {
    let mut themes = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return (themes, problems) };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml") && p.is_file()).collect();
    files.sort();
    for path in files.into_iter().take(100) {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let result = if !crate::plugins::manifest::valid_id(&stem) {
            Err("the file name may only have small letters, digits, - and _".to_string())
        } else {
            std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse(&stem, &t))
        };
        match result {
            Ok(t) => themes.push(t),
            Err(e) => problems.push((stem, e)),
        }
    }
    (themes, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "name = \"Mini\"\n[colors]\naccent = \"#8ab4f8\"\n";

    #[test]
    fn a_theme_needs_only_a_name_and_an_accent() {
        let t = parse("mini", MINIMAL).unwrap();
        assert_eq!(t.name, "Mini");
        assert_eq!(t.accent, Color32::from_rgb(0x8a, 0xb4, 0xf8));
        assert!(!t.follow_cover);
        assert!(ui::luminance(t.tint) < 0.05, "the glass is dark");
        assert_eq!(t.wallpaper.len(), 3);
    }

    #[test]
    fn colours_are_read_in_both_hex_forms_and_bad_ones_are_refused() {
        assert_eq!(parse_hex("#fff").unwrap(), Color32::WHITE);
        assert_eq!(parse_hex("#A6e3A1").unwrap(), Color32::from_rgb(166, 227, 161));
        for bad in ["fff", "#ff", "#gggggg", "#1234567", "red", "", "rgb(1,2,3)"] {
            assert!(parse_hex(bad).is_err(), "{bad}");
        }
        assert!(parse("x", &MINIMAL.replace("#8ab4f8", "blue")).is_err());
        assert!(parse("x", "name = \"x\"").is_err());
        assert!(parse("x", &MINIMAL.replace("Mini", "")).is_err());
    }

    #[test]
    fn a_full_theme_keeps_what_it_says() {
        let t = parse(
            "full",
            "name = \"Full\"\nauthor = \"me\"\nfollow_cover = true\n[colors]\naccent = \"#ffb86c\"\npanel = \"#14121e\"\nwallpaper = [\"#1a2a6c\", \"#b21f1f\"]\nlight = 0.3\n",
        )
        .unwrap();
        assert!(t.follow_cover);
        assert_eq!(t.author, "me");
        assert_eq!(t.wallpaper[0], Color32::from_rgb(0x1a, 0x2a, 0x6c));
        assert_eq!(t.wallpaper[2], Color32::from_rgb(0xb2, 0x1f, 0x1f));
        assert!((t.light - 0.3).abs() < 1e-6);
        assert!(parse("x", &MINIMAL.replace("]\n", "]\nwallpaper = []\n").replace("[colors]\naccent", "[colors]\nwallpaper = []\naccent")).is_err());
    }

    #[test]
    fn unreadable_colours_are_corrected() {
        // a near-black accent on dark glass would be invisible
        let t = parse("dark", "name = \"d\"\n[colors]\naccent = \"#202020\"\n").unwrap();
        assert!(ui::contrast(t.accent, ui::panel_solid_for(t.tint)) >= 4.5, "{:?}", t.accent);
        // light glass is not possible: the panel is cut down to dark
        let t = parse("bright", "name = \"b\"\n[colors]\naccent = \"#8ab4f8\"\npanel = \"#ffffff\"\n").unwrap();
        assert!(ui::to_hsl(t.tint).2 <= 0.2 + 1e-3);
    }

    #[test]
    fn finds_themes_and_names_the_broken_ones() {
        let dir = std::env::temp_dir().join(format!("oxytide-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("good.toml"), MINIMAL).unwrap();
        std::fs::write(dir.join("bad.toml"), "name = \"x\"\n[colors]\naccent = \"nope\"\n").unwrap();
        std::fs::write(dir.join("Bad Name.toml"), MINIMAL).unwrap();
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        let (themes, problems) = discover(&dir);
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].id, "good");
        assert_eq!(problems.len(), 2);
    }
}
