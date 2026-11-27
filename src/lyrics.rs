//! Synced lyrics, karaoke style: the line that is sung is large and bright, the
//! words fill in as they are sung, the rest fades with the distance and the
//! whole column glides instead of jumping.
//!
//! TIDAL only times whole lines. The times of the words inside a line are
//! estimated from the syllables (a word of three syllables takes about three
//! times as long as one of one), stretched over the time the line lasts.

use eframe::egui::{self, Color32, FontId, Pos2, Rect};

use crate::ui::{self, TEXT};

/// One word and when it is sung (seconds into the song).
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// One timed line of lyrics. An empty `words` list is an instrumental break.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub start: f64,
    /// When the next line begins (or a guess for the last one).
    pub end: f64,
    pub words: Vec<Word>,
}

/// Vowel groups: a good enough syllable count for most languages.
fn syllables(word: &str) -> usize {
    let mut count = 0;
    let mut in_vowel = false;
    for c in word.chars() {
        let vowel = matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y') || "äöüáéíóúàèìòùâêîôûåæœøãõ".contains(c.to_lowercase().next().unwrap_or(' '));
        if vowel && !in_vowel {
            count += 1;
        }
        in_vowel = vowel;
    }
    count.max(1)
}

/// Seconds per syllable when nothing better is known.
const DEFAULT_PACE: f64 = 0.23;

/// How fast each line is sung, in seconds per syllable. Lines that follow each other closely tell
/// the pace (a rapped verse is quick, a ballad slow); lines with a long gap after them cannot,
/// so they take the pace of their neighbours. This follows the singer's speed changes through the song.
fn paces(synced: &[(f64, String)], duration: f64) -> Vec<f64> {
    let n = synced.len();
    let measured: Vec<Option<f64>> = (0..n)
        .map(|i| {
            let syll: usize = synced[i].1.split_whitespace().map(syllables).sum();
            let next = synced.get(i + 1).map(|(t, _)| *t).unwrap_or(duration);
            let gap = next - synced[i].0;
            // a line that is clearly sung into the gap (no long pause): the gap, less a breath, is its length
            (syll >= 3 && gap > 0.4 && gap < 5.5).then(|| ((gap - 0.35) / syll as f64).clamp(0.10, 0.45))
        })
        .collect();
    let median = |mut v: Vec<f64>| -> Option<f64> {
        if v.is_empty() {
            return None;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    };
    let song = median(measured.iter().flatten().copied().collect()).unwrap_or(DEFAULT_PACE);
    (0..n)
        .map(|i| {
            let from = i.saturating_sub(4);
            let to = (i + 5).min(n);
            let near: Vec<f64> = measured[from..to].iter().flatten().copied().collect();
            // the neighbours' pace, drawn a little towards the whole song's
            match median(near) {
                Some(local) => 0.7 * local + 0.3 * song,
                None => song,
            }
        })
        .collect()
}

/// How long a line is actually sung, given the time until the next one.
fn sung_seconds(total_syllables: usize, gap: f64, pace: f64) -> f64 {
    let estimate = total_syllables as f64 * pace + 0.5;
    let gap = (gap - 0.08).max(0.2);
    if gap <= estimate * 1.5 {
        // no room for a pause: the line fills the time
        gap * 0.97
    } else {
        // a long gap after the line: it is sung at a natural pace, then there is a break
        estimate * 1.15
    }
}

/// Turn the timed lines of an LRC file into lines with timed words.
pub fn build(synced: &[(f64, String)], duration: f64) -> Vec<Line> {
    let mut lines = Vec::with_capacity(synced.len());
    let pace = paces(synced, duration);
    for (i, (start, text)) in synced.iter().enumerate() {
        let next = synced.get(i + 1).map(|(t, _)| *t).unwrap_or_else(|| (*start + 6.0).max(duration));
        let end = next.max(*start + 0.1);
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut timed = Vec::with_capacity(words.len());
        if !words.is_empty() {
            let last = words.len() - 1;
            let weights: Vec<f64> = words
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let pause = if w.ends_with([',', ';', ':']) { 0.45 } else if w.ends_with(['.', '!', '?', '…']) { 0.8 } else { 0.0 };
                    // a word takes longer the more syllables and letters (consonants take time too);
                    // the last word of a line is usually held
                    let letters = w.chars().filter(|c| c.is_alphanumeric()).count() as f64;
                    let body = 0.62 * syllables(w) as f64 + 0.05 * letters;
                    body * if i == last { 1.25 } else { 1.0 } + pause
                })
                .collect();
            let total: f64 = weights.iter().sum();
            let span = sung_seconds(words.iter().map(|w| syllables(w)).sum(), end - start, pace[i]);
            let mut at = *start;
            for (w, weight) in words.iter().zip(&weights) {
                let len = span * weight / total;
                timed.push(Word { text: (*w).to_string(), start: at, end: at + len });
                at += len;
            }
        }
        lines.push(Line { start: *start, end, words: timed });
    }
    lines
}

/// The line being sung at `time` (a little early, so the eye can follow), if any has begun.
pub fn active_line(lines: &[Line], time: f64) -> Option<usize> {
    lines.partition_point(|l| l.start <= time).checked_sub(1)
}

// ---------------------------------------------------------------------------- view

const FONT_SIZE: f32 = 32.0;
const ROW_HEIGHT: f32 = 42.0;
const LINE_GAP: f32 = 26.0;
/// Space left of the text so the glow round the first word is not cut off.
const GLOW_ROOM: f32 = 36.0;
/// Seconds after the last manual scroll before the column follows the song again.
const MANUAL_HOLD: f64 = 2.8;

struct WordBox {
    x: f32,
    y: f32,
    w: f32,
}

struct LineLayout {
    top: f32,
    height: f32,
    words: Vec<WordBox>,
}

/// The state of the lyrics column.
#[derive(Default)]
pub struct View {
    /// The song these lines belong to.
    pub track: Option<u64>,
    pub lines: Vec<Line>,
    layout: Vec<LineLayout>,
    layout_width: f32,
    /// Content coordinate shown at the top of the column.
    scroll: f32,
    follow_after: f64,
    started: bool,
    /// The sung word glows.
    pub glow: bool,
    /// Lines far from the sung one are softly blurred.
    pub depth: bool,
    /// The glow glides from word to word: where it is (in column coordinates), how big, how strong.
    glow_pos: Pos2,
    glow_size: egui::Vec2,
    glow_alpha: f32,
    glow_init: bool,
}

impl View {
    pub fn set(&mut self, track: u64, synced: &[(f64, String)], duration: f64) {
        self.track = Some(track);
        self.lines = build(synced, duration);
        self.layout.clear();
        self.layout_width = 0.0;
        self.started = false;
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn lay_out(&mut self, ui: &egui::Ui, width: f32) {
        let font = FontId::proportional(FONT_SIZE);
        let measure = |text: &str| ui.painter().layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x;
        let space = measure("a a") - measure("aa");
        let mut top = 0.0;
        self.layout.clear();
        for line in &self.lines {
            let mut words = Vec::with_capacity(line.words.len());
            let (mut x, mut row) = (0.0_f32, 0.0_f32);
            for w in &line.words {
                let ww = measure(&w.text);
                if x > 0.0 && x + ww > width {
                    x = 0.0;
                    row += 1.0;
                }
                words.push(WordBox { x, y: row * ROW_HEIGHT, w: ww });
                x += ww + space;
            }
            let height = if line.words.is_empty() { ROW_HEIGHT * 0.8 } else { (row + 1.0) * ROW_HEIGHT };
            self.layout.push(LineLayout { top, height, words });
            top += height + LINE_GAP;
        }
        self.layout_width = width;
    }

    /// Draw the column into the space that is left; returns the time of a clicked line.
    pub fn show(&mut self, ui: &mut egui::Ui, time: f64, playing: bool) -> Option<f64> {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, egui::Sense::hover());
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        // room on the left for the glow of the first word of a line
        let inner = Rect::from_min_max(rect.min + egui::vec2(GLOW_ROOM, 0.0), rect.max - egui::vec2(24.0, 0.0));
        if (inner.width() - self.layout_width).abs() > 0.5 || self.layout.len() != self.lines.len() {
            self.lay_out(ui, inner.width());
        }
        if self.layout.is_empty() {
            return None;
        }
        let painter = ui.painter_at(rect);

        // where the column wants to be: the sung line a bit above the middle
        let active = active_line(&self.lines, time + 0.25);
        let sung_now = active_line(&self.lines, time);
        let focus_y = rect.height() * 0.40;
        let target = match active {
            Some(i) => self.layout[i].top + self.layout[i].height * 0.5 - focus_y,
            None => -focus_y + 40.0,
        };
        // the wheel scrolls by hand for a moment
        let layer = ui.layer_id();
        let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p) && ctx.layer_id_at(p).is_none_or(|l| l == layer));
        let wheel = if over { ctx.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
        if wheel != 0.0 {
            self.scroll -= wheel;
            self.follow_after = now + MANUAL_HOLD;
            ctx.request_repaint();
        }
        let content_end = self.layout.last().map_or(0.0, |l| l.top + l.height);
        if !self.started {
            self.scroll = target;
            self.started = true;
        } else if now >= self.follow_after {
            let k = 1.0 - (-dt * 6.5).exp();
            self.scroll += (target - self.scroll) * k;
            if (target - self.scroll).abs() > 0.3 {
                ctx.request_repaint();
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.scroll = self.scroll.clamp(-rect.height() * 0.6, content_end + rect.height() * 0.2);

        let font = FontId::proportional(FONT_SIZE);
        let pointer = ctx.input(|i| i.pointer.hover_pos()).filter(|&p| ctx.layer_id_at(p).is_none_or(|l| l == layer));
        let mut seek = None;
        let view_mid = rect.height() * 0.5;

        // one glow that follows the singing: it sits at the sung edge of the current word and glides
        // (also across lines) instead of a new one lighting up for every word
        if self.glow {
            let mut target: Option<(Pos2, egui::Vec2)> = None;
            if let Some(i) = sung_now {
                let (line, lay) = (&self.lines[i], &self.layout[i]);
                if !line.words.is_empty() && time < line.end + 0.35 {
                    let idx = line.words.iter().rposition(|w| w.start <= time).unwrap_or(0);
                    let (w, b) = (&line.words[idx], &lay.words[idx]);
                    let fill = (((time - w.start) / (w.end - w.start).max(0.05)).clamp(0.0, 1.0)) as f32;
                    // it stays on the word (moving only a little along it) and is as wide as the word, so at the
                    // last word of a line it does not run off to the right
                    target = Some((Pos2::new(b.x + b.w * (0.40 + 0.20 * fill), lay.top + b.y + ROW_HEIGHT * 0.5 - 3.0), egui::vec2(20.0 + b.w * 0.45, ROW_HEIGHT * 0.8)));
                }
            }
            let ease = |rate: f32| 1.0 - (-dt * rate).exp();
            match target {
                Some((pos, size)) => {
                    // after a seek, or from nothing: appear in place instead of flying over
                    if !self.glow_init || self.glow_alpha < 0.03 || (pos - self.glow_pos).length() > 420.0 {
                        self.glow_pos = pos;
                        self.glow_size = size;
                        self.glow_alpha = self.glow_alpha.min(0.02);
                        self.glow_init = true;
                    }
                    self.glow_pos += (pos - self.glow_pos) * ease(11.0);
                    self.glow_size += (size - self.glow_size) * ease(9.0);
                    self.glow_alpha += (1.0 - self.glow_alpha) * ease(7.0);
                }
                None => self.glow_alpha += (0.0 - self.glow_alpha) * ease(6.0),
            }
            if self.glow_alpha > 0.02 {
                let at = Pos2::new(inner.left() + self.glow_pos.x, rect.top() + self.glow_pos.y - self.scroll);
                let edge = smoothstep(rect.top(), rect.top() + 70.0, at.y).min(1.0 - smoothstep(rect.bottom() - 90.0, rect.bottom(), at.y));
                let halo = ui::lerp_color(TEXT, ui::accent(), 0.6);
                ui::soft_ellipse(&painter, at, self.glow_size, with_alpha_f(halo, 0.30 * self.glow_alpha * edge));
                ctx.request_repaint();
            }
        }
        for (i, (line, lay)) in self.lines.iter().zip(&self.layout).enumerate() {
            let y0 = rect.top() + lay.top - self.scroll;
            if y0 > rect.bottom() + 40.0 || y0 + lay.height < rect.top() - 40.0 {
                continue;
            }
            // brightness by distance from the middle of the column, fading out at both edges
            let dist = ((y0 + lay.height * 0.5 - rect.top() - view_mid).abs() / rect.height()).clamp(0.0, 1.0);
            let falloff = 1.0 - smoothstep(0.0, 0.62, dist);
            let edge = smoothstep(rect.top(), rect.top() + 70.0, y0 + lay.height * 0.5).min(1.0 - smoothstep(rect.bottom() - 90.0, rect.bottom(), y0 + lay.height * 0.5));
            // the scroll leads a little, but the highlighting follows the singing exactly, so the
            // last words of a line are not greyed out before they are sung
            let is_active = sung_now == Some(i);
            let line_rect = Rect::from_min_size(Pos2::new(inner.left(), y0), egui::vec2(inner.width(), lay.height));
            // only the part inside the column takes clicks: a line that scrolls past the top edge
            // must not cover the tabs (Lyrics / Credits) above it
            let hit = line_rect.expand2(egui::vec2(8.0, LINE_GAP * 0.5)).intersect(rect);
            let hovered = hit.is_positive() && pointer.is_some_and(|p| hit.contains(p));
            if hit.is_positive() {
                let resp = ui.interact(hit, egui::Id::new(("lyric", i)), egui::Sense::click());
                if resp.hovered() {
                    ui::pointer_on_hover(&resp);
                }
                if resp.clicked() {
                    seek = Some(line.start);
                }
            }
            let base = (0.20 + 0.34 * falloff + if hovered { 0.16 } else { 0.0 }).clamp(0.0, 1.0) * edge;

            if line.words.is_empty() {
                // instrumental break: three dots that light up one after the other
                let p = ((time - line.start) / (line.end - line.start).max(0.1)).clamp(0.0, 1.0) as f32;
                for k in 0..3 {
                    let lit = if is_active { (p * 3.0 - k as f32).clamp(0.0, 1.0) } else { 0.0 };
                    let a = (base + (1.0 - base) * lit * 0.9) * edge;
                    let r = 5.0 + 1.5 * lit;
                    painter.circle_filled(Pos2::new(inner.left() + 10.0 + k as f32 * 22.0, y0 + lay.height * 0.5), r, with_alpha_f(TEXT, a));
                }
                continue;
            }

            for (w, b) in line.words.iter().zip(&lay.words) {
                let pos = Pos2::new(inner.left() + b.x, y0 + b.y);
                let dim = with_alpha_f(TEXT, base);
                if !is_active {
                    // far from the sung line the words go out of focus: a ring of faint copies
                    let blur = if self.depth { smoothstep(0.14, 0.60, dist) * 3.6 } else { 0.0 };
                    if blur > 0.5 {
                        painter.text(pos, egui::Align2::LEFT_TOP, &w.text, font.clone(), with_alpha_f(TEXT, base * 0.10));
                        for k in 0..8 {
                            let a = k as f32 * std::f32::consts::FRAC_PI_4;
                            painter.text(pos + egui::vec2(a.cos(), a.sin()) * blur, egui::Align2::LEFT_TOP, &w.text, font.clone(), with_alpha_f(TEXT, base * 0.022));
                        }
                    } else {
                        painter.text(pos, egui::Align2::LEFT_TOP, &w.text, font.clone(), dim);
                    }
                    continue;
                }
                // the sung part of the word is bright, the rest waits in the dim colour
                let fill = (((time - w.start) / (w.end - w.start).max(0.05)).clamp(0.0, 1.0)) as f32;
                let lift = -3.0 * (fill * (1.0 - fill) * 4.0);
                let pos = pos + egui::vec2(0.0, lift);
                painter.text(pos, egui::Align2::LEFT_TOP, &w.text, font.clone(), with_alpha_f(TEXT, 0.40 * edge));
                if fill > 0.0 {
                    let clip = Rect::from_min_max(Pos2::new(pos.x - 2.0, pos.y - 4.0), Pos2::new(pos.x + b.w * fill + 1.0, pos.y + ROW_HEIGHT));
                    painter.with_clip_rect(clip.intersect(rect)).text(pos, egui::Align2::LEFT_TOP, &w.text, font.clone(), with_alpha_f(TEXT, edge));
                }
            }
        }
        if playing || active.is_some() && now < self.follow_after {
            ctx.request_repaint();
        }
        seek
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn with_alpha_f(c: Color32, a: f32) -> Color32 {
    ui::with_alpha(c, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_syllables_in_several_languages() {
        assert_eq!(syllables("love"), 2); // good enough: "o" and "e"
        assert_eq!(syllables("night"), 1);
        assert_eq!(syllables("electric"), 3);
        assert_eq!(syllables("Straße"), 2);
        assert_eq!(syllables("..."), 1);
        assert_eq!(syllables("über"), 2);
    }

    #[test]
    fn words_share_a_line_in_order_and_do_not_overlap() {
        let lines = build(&[(10.0, "So send a signal through the night".into()), (14.0, "".into()), (20.0, "Next line".into())], 200.0);
        assert_eq!(lines.len(), 3);
        let l = &lines[0];
        assert_eq!(l.words.len(), 7);
        assert!((l.words[0].start - 10.0).abs() < 1e-9);
        for w in l.words.windows(2) {
            assert!((w[0].end - w[1].start).abs() < 1e-9, "words are back to back");
            assert!(w[0].end > w[0].start);
        }
        assert!(l.words.last().unwrap().end < 14.0, "stays inside the line");
        assert!(lines[1].words.is_empty(), "an empty line is a break");
    }

    #[test]
    fn longer_words_take_longer_and_the_last_one_is_held() {
        let lines = build(&[(10.0, "to strengths to".into()), (20.0, "x".into())], 100.0);
        let w = &lines[0].words;
        let dur = |i: usize| w[i].end - w[i].start;
        assert!(dur(1) > dur(0), "{} vs {}", dur(1), dur(0));
        assert!(dur(2) > dur(0), "the last word is held: {} vs {}", dur(2), dur(0));
    }

    #[test]
    fn a_line_before_a_long_pause_is_sung_at_the_pace_of_the_song() {
        let text = "one two three four five six seven eight";
        let mut fast: Vec<(f64, String)> = (0..8).map(|i| (10.0 + i as f64 * 1.6, text.to_string())).collect();
        let mut slow: Vec<(f64, String)> = (0..8).map(|i| (10.0 + i as f64 * 4.0, text.to_string())).collect();
        for song in [&mut fast, &mut slow] {
            let last = song.last().unwrap().0;
            song.push((last + 12.0, "end".into()));
        }
        let sung = |song: &[(f64, String)]| {
            let l = build(song, 300.0);
            let line = &l[song.len() - 2];
            line.words.last().unwrap().end - line.start
        };
        assert!(sung(&fast) < sung(&slow), "fast {} vs slow {}", sung(&fast), sung(&slow));
    }

    #[test]
    fn a_line_with_a_long_gap_is_sung_at_its_own_pace() {
        let lines = build(&[(0.0, "Hello there".into()), (30.0, "Next".into())], 100.0);
        let end = lines[0].words.last().unwrap().end;
        assert!(end < 5.0, "{end}: not stretched over the whole gap");
        let tight = build(&[(0.0, "Hello there my friend how are you".into()), (3.0, "Next".into())], 100.0);
        let end = tight[0].words.last().unwrap().end;
        assert!(end > 2.5 && end < 3.0, "{end}: fills a short gap");
    }

    #[test]
    fn finds_the_sung_line() {
        let lines = build(&[(5.0, "a".into()), (10.0, "b".into()), (15.0, "c".into())], 100.0);
        assert_eq!(active_line(&lines, 2.0), None);
        assert_eq!(active_line(&lines, 5.0), Some(0));
        assert_eq!(active_line(&lines, 12.0), Some(1));
        assert_eq!(active_line(&lines, 99.0), Some(2));
    }
}
