//! The overlay card (cover, title, artist, progress) as a bitmap.
//!
//! The recorder plugin cannot lay out text or decode pictures, so the card is
//! drawn here and handed over as premultiplied RGBA pixels.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::RgbaImage;

/// Card size at scale 1.
pub const WIDTH: f32 = 380.0;
pub const HEIGHT: f32 = 96.0;
/// The cover is kept at this size (enough for scale 2).
pub const COVER_PX: u32 = 144;

const BACKGROUND: [f32; 4] = [12.0, 14.0, 18.0, 0.82];
const TITLE: [f32; 3] = [255.0, 255.0, 255.0];
const ARTIST: [f32; 3] = [196.0, 201.0, 212.0];

pub struct Card {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, top row first.
    pub pixels: Vec<u8>,
}

fn font() -> &'static FontRef<'static> {
    use std::sync::OnceLock;
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        let defs = egui::FontDefinitions::default();
        let data = defs
            .font_data
            .get("Ubuntu-Light")
            .or_else(|| defs.font_data.values().next())
            .expect("egui ships fonts");
        // the font data lives as long as the program: leak one copy
        let bytes: &'static [u8] = Box::leak(data.font.to_vec().into_boxed_slice());
        FontRef::try_from_slice(bytes).expect("valid font")
    })
}

/// Coverage (0‥1) of a rounded rectangle at pixel centre (x, y).
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (dx, dy) = ((x - w / 2.0).abs() - (w / 2.0 - r), (y - h / 2.0).abs() - (h / 2.0 - r));
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r;
    (0.5 - outside).clamp(0.0, 1.0)
}

struct Canvas {
    w: u32,
    h: u32,
    px: Vec<f32>, // premultiplied RGBA, 0‥255
}

impl Canvas {
    /// Source-over of `color` with `alpha` (0‥1) onto pixel (x, y).
    fn blend(&mut self, x: i32, y: i32, color: [f32; 3], alpha: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || alpha <= 0.0 {
            return;
        }
        let i = (y as u32 * self.w + x as u32) as usize * 4;
        let keep = 1.0 - alpha;
        for c in 0..3 {
            self.px[i + c] = color[c] * alpha + self.px[i + c] * keep;
        }
        self.px[i + 3] = 255.0 * alpha + self.px[i + 3] * keep;
    }

    /// Text starting at `x` with its baseline at `y`, cut off with "…" at `max_w`.
    fn text(&mut self, text: &str, x: f32, y: f32, size: f32, max_w: f32, color: [f32; 3], bold: bool) {
        let scaled = font().as_scaled(PxScale::from(size));
        let ellipsis = scaled.h_advance(scaled.glyph_id('…'));
        let total: f32 = text.chars().map(|c| scaled.h_advance(scaled.glyph_id(c))).sum();
        let mut limit = if total <= max_w { f32::INFINITY } else { max_w - ellipsis };
        let mut pen = x;
        let mut cut = false;
        let mut glyphs = Vec::new();
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            let adv = scaled.h_advance(id);
            if pen + adv - x > limit {
                cut = true;
                break;
            }
            glyphs.push((id, pen));
            pen += adv;
        }
        if cut {
            glyphs.push((scaled.glyph_id('…'), pen));
            limit = 0.0;
        }
        let _ = limit;
        let offsets: &[f32] = if bold { &[0.0, size * 0.035] } else { &[0.0] };
        for (id, gx) in glyphs {
            for dx in offsets {
                let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(gx + dx, y));
                if let Some(outline) = font().outline_glyph(glyph) {
                    let b = outline.px_bounds();
                    outline.draw(|px, py, cov| {
                        self.blend(b.min.x as i32 + px as i32, b.min.y as i32 + py as i32, color, cov);
                    });
                }
            }
        }
    }
}

/// Draw the card for a song at `scale` (1.0 = 380×96 pixels).
pub fn render(title: &str, artist: &str, cover: Option<&RgbaImage>, progress: Option<f32>, scale: f32) -> Card {
    let s = scale.clamp(0.5, 3.0);
    let (w, h) = ((WIDTH * s).round() as u32, (HEIGHT * s).round() as u32);
    let mut cv = Canvas { w, h, px: vec![0.0; (w * h * 4) as usize] };

    // background
    let radius = 16.0 * s;
    for y in 0..h {
        for x in 0..w {
            let a = rounded(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius) * BACKGROUND[3];
            let i = (y * w + x) as usize * 4;
            cv.px[i] = BACKGROUND[0] * a;
            cv.px[i + 1] = BACKGROUND[1] * a;
            cv.px[i + 2] = BACKGROUND[2] * a;
            cv.px[i + 3] = 255.0 * a;
        }
    }

    // cover
    let (cx, cy, cs) = (12.0 * s, 12.0 * s, 72.0 * s);
    let side = cs.round() as u32;
    let scaled_cover = cover.map(|c| image::imageops::resize(c, side, side, image::imageops::FilterType::Triangle));
    for y in 0..side {
        for x in 0..side {
            let a = rounded(x as f32 + 0.5, y as f32 + 0.5, cs, cs, 10.0 * s);
            let color = match &scaled_cover {
                Some(img) => {
                    let p = img.get_pixel(x, y).0;
                    [p[0] as f32, p[1] as f32, p[2] as f32]
                }
                None => [48.0, 52.0, 62.0],
            };
            cv.blend(cx as i32 + x as i32, cy as i32 + y as i32, color, a);
        }
    }

    // text and progress bar
    let tx = cx + cs + 14.0 * s;
    let tw = w as f32 - tx - 14.0 * s;
    cv.text(title, tx, 36.0 * s, 17.0 * s, tw, TITLE, true);
    cv.text(artist, tx, 58.0 * s, 14.0 * s, tw, ARTIST, false);
    if let Some(p) = progress {
        let (by, bh) = ((70.0 * s).round() as i32, (4.0 * s).round().max(2.0) as i32);
        for y in 0..bh {
            for x in 0..tw.round() as i32 {
                let filled = (x as f32) < tw * p.clamp(0.0, 1.0);
                cv.blend(tx as i32 + x, by + y, [255.0; 3], if filled { 1.0 } else { 0.18 });
            }
        }
    }

    let pixels = cv.px.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
    Card { width: w, height: h, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_card_with_rounded_transparent_corners() {
        let card = render("A rather long title that must be cut off with an ellipsis", "Artist", None, Some(0.4), 1.0);
        assert_eq!((card.width, card.height), (380, 96));
        assert_eq!(card.pixels[3], 0, "corner is transparent");
        let mid = ((48 * 380 + 300) * 4) as usize;
        assert!(card.pixels[mid + 3] > 150, "background is mostly opaque");
        // some text pixels are brighter than the background
        assert!(card.pixels.chunks(4).any(|p| p[0] > 200 && p[3] > 200));
        if let Some(dir) = std::env::var_os("TIDALFAST_CARD_DUMP") {
            let img = RgbaImage::from_raw(card.width, card.height, card.pixels.clone()).unwrap();
            img.save(std::path::Path::new(&dir).join("card.png")).unwrap();
            std::fs::write(std::path::Path::new(&dir).join("card.rgba"), &card.pixels).unwrap();
        }
    }
}
