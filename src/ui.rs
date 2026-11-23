//! Theme and small custom widgets shared by all views.

use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui::{
    self, Rect, Align2, Color32, CornerRadius, FontFamily, FontId, Response, Sense, Stroke, TextStyle, Ui,
};
pub use egui_phosphor::regular as icons;

/// Translucent colour, usable in `const` (premultiplies by hand).
pub const fn glass(alpha: u8, r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        (r as u32 * alpha as u32 / 255) as u8,
        (g as u32 * alpha as u32 / 255) as u8,
        (b as u32 * alpha as u32 / 255) as u8,
        alpha,
    )
}

// Acrylic dark theme: panels are translucent, so the wallpaper built from the
// current track's colours glows through them like frosted glass.
pub const BG: Color32 = Color32::from_rgb(11, 12, 18); // window clear colour
pub const CARD: Color32 = glass(14, 255, 255, 255);
pub const CARD_HOVER: Color32 = glass(30, 255, 255, 255);
pub const LINE: Color32 = glass(34, 255, 255, 255);
pub const EDGE: Color32 = glass(40, 255, 255, 255); // hairline on glass
pub const TEXT: Color32 = Color32::from_rgb(222, 228, 246);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 156, 180);
pub const DANGER: Color32 = Color32::from_rgb(243, 139, 168); // red
pub const WARN: Color32 = Color32::from_rgb(249, 226, 175); // yellow
pub const OK: Color32 = Color32::from_rgb(166, 227, 161); // green


// ------------------------------------------------------------------ the theme

/// The colours that follow the song: the accent and a dark tint for the panels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Theme {
    pub accent: Color32,
    /// Very dark colour of the glass panels.
    pub tint: Color32,
    /// How light the backdrop is (0 dark … 1 bright): bright ones get more opaque panels.
    pub light: f32,
}

pub const DEFAULT_ACCENT: Color32 = Color32::from_rgb(166, 227, 161); // green
const DEFAULT_TINT: Color32 = Color32::from_rgb(17, 18, 28);

impl Default for Theme {
    fn default() -> Self {
        Self { accent: DEFAULT_ACCENT, tint: DEFAULT_TINT, light: 0.0 }
    }
}

static ACCENT_BITS: AtomicU32 = AtomicU32::new(0);
static TINT_BITS: AtomicU32 = AtomicU32::new(0);
static LIGHT_BITS: AtomicU32 = AtomicU32::new(0);

fn pack(c: Color32) -> u32 {
    u32::from_be_bytes([1, c.r(), c.g(), c.b()])
}

fn unpack(bits: u32, fallback: Color32) -> Color32 {
    if bits == 0 {
        return fallback;
    }
    let [_, r, g, b] = bits.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

/// Make this the theme all widgets draw with.
pub fn set_theme(theme: Theme) {
    ACCENT_BITS.store(pack(theme.accent), Ordering::Relaxed);
    TINT_BITS.store(pack(theme.tint), Ordering::Relaxed);
    LIGHT_BITS.store(theme.light.to_bits(), Ordering::Relaxed);
}

pub fn accent() -> Color32 {
    unpack(ACCENT_BITS.load(Ordering::Relaxed), DEFAULT_ACCENT)
}

pub fn accent_dim() -> Color32 {
    let a = accent();
    glass(70, a.r(), a.g(), a.b())
}

/// The accent's hue (0‥1) when the theme follows a cover, `None` with the default look.
pub fn accent_hue() -> Option<f32> {
    let a = accent();
    (a != DEFAULT_ACCENT).then(|| to_hsl(a).0)
}

/// The dark field behind plots and meters: the glass, a little darker.
pub fn plot_background() -> Color32 {
    lerp_color(BG, panel_solid(), 0.5)
}

fn tint() -> Color32 {
    unpack(TINT_BITS.load(Ordering::Relaxed), DEFAULT_TINT)
}

fn light() -> f32 {
    f32::from_bits(LIGHT_BITS.load(Ordering::Relaxed))
}

/// Acrylic glass: dark, tinted by the song, more opaque in front of a bright backdrop.
pub fn panel() -> Color32 {
    let t = tint();
    glass((178.0 + 48.0 * light()) as u8, t.r(), t.g(), t.b())
}

/// Extra transparent glass (the full-screen player).
pub fn panel_clear() -> Color32 {
    let t = tint();
    glass((118.0 + 70.0 * light()) as u8, t.r(), t.g(), t.b())
}

/// Menus and dialogs: fully opaque.
pub fn panel_solid() -> Color32 {
    let t = tint();
    let (r, g, b) = (t.r() as u16 + 3, t.g() as u16 + 3, t.b() as u16 + 4);
    Color32::from_rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8)
}

pub fn to_hsl(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) / 2.0;
    if (mx - mn).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = mx - mn;
    let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

pub fn from_hsl(h: f32, s: f32, l: f32) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h.rem_euclid(1.0) * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(1.0) * 6.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color32::from_rgb(((r + m) * 255.0).round() as u8, ((g + m) * 255.0).round() as u8, ((b + m) * 255.0).round() as u8)
}

/// WCAG relative luminance.
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio of two colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The theme for a cover with these (vivid) colours. Covers that are grey, or
/// whose colours would not read on the dark glass, fall back to the default
/// accent; bright covers get more opaque panels so the text stays readable.
pub fn theme_for(palette: [Color32; 3]) -> Theme {
    let default = Theme::default();
    // the colour with the most character: saturated, and not at the extremes of lightness
    let score = |c: Color32| {
        let (_, s, l) = to_hsl(c);
        s * (1.0 - (l - 0.55).abs() * 1.2).max(0.0)
    };
    let best = palette.iter().copied().max_by(|a, b| score(*a).total_cmp(&score(*b))).unwrap_or(DEFAULT_ACCENT);
    let (h, s, _) = to_hsl(best);
    // brightness of the backdrop the palette would give
    let mean = palette.iter().map(|c| luminance(*c)).sum::<f32>() / 3.0;
    let light = ((mean - 0.18) / 0.45).clamp(0.0, 1.0);
    if s < 0.22 {
        // a grey or washed-out cover: nothing to follow
        return Theme { light, ..default };
    }
    // a hint of the cover's hue in the glass, kept very dark
    let tint = from_hsl(h, (s * 0.6).clamp(0.25, 0.5), 0.115);
    // lively but readable: a fixed saturation and lightness band, brightened until the
    // contrast against the glass is comfortable (accents are also text and button fills)
    let mut accent = from_hsl(h, s.clamp(0.5, 0.9), 0.68);
    let mut l = 0.68;
    while contrast(accent, panel_solid_for(tint)) < 6.0 && l < 0.92 {
        l += 0.03;
        accent = from_hsl(h, s.clamp(0.5, 0.9), l);
    }
    Theme { accent, tint, light }
}

fn panel_solid_for(tint: Color32) -> Color32 {
    Color32::from_rgb(tint.r() + 3, tint.g() + 3, tint.b() + 4)
}

/// Eases a value from wherever it is to a new goal over a set time (smoothstep), and starts over
/// from the current value whenever the goal changes: colours that glide instead of jump.
pub struct Glide<T> {
    from: T,
    goal: T,
    t0: f64,
    secs: f32,
}

impl<T: Copy + PartialEq> Glide<T> {
    pub fn new(value: T) -> Self {
        Self { from: value, goal: value, t0: 0.0, secs: 1.0 }
    }

    fn progress(&self, now: f64) -> f32 {
        let t = (((now - self.t0) as f32) / self.secs.max(0.01)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The value at `now`. A new `goal` starts a glide of `secs`, `head_start` seconds
    /// of which have already passed (the song changed a moment ago).
    pub fn at(&mut self, now: f64, goal: T, secs: f32, head_start: f32, lerp: impl Fn(T, T, f32) -> T) -> T {
        if goal != self.goal {
            self.from = lerp(self.from, self.goal, self.progress(now));
            self.goal = goal;
            self.secs = secs.max(0.2);
            self.t0 = now - (head_start as f64).clamp(0.0, self.secs as f64 * 0.6);
        }
        lerp(self.from, self.goal, self.progress(now))
    }

    pub fn done(&self, now: f64) -> bool {
        (now - self.t0) as f32 >= self.secs
    }

    pub fn secs(&self) -> f32 {
        self.secs
    }
}

/// Blend towards `to` (used to ease the theme when the song changes).
pub fn lerp_theme(from: Theme, to: Theme, t: f32) -> Theme {
    Theme {
        accent: lerp_color(from.accent, to.accent, t),
        tint: lerp_color(from.tint, to.tint, t),
        light: from.light + (to.light - from.light) * t,
    }
}

/// Wallpaper colours used while nothing is playing.
pub const DEFAULT_PALETTE: [Color32; 3] = [
    Color32::from_rgb(86, 150, 104),
    Color32::from_rgb(72, 130, 156),
    Color32::from_rgb(110, 96, 168),
];

/// A translucent "acrylic" frame: dark glass with a hairline edge.
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

// ------------------------------------------------------------- song colours

/// Pick up to three vivid colours from cover art (RGBA8).
pub fn palette_from_rgba(rgba: &[u8], width: usize, height: usize) -> [Color32; 3] {
    let total = width * height;
    if total == 0 || rgba.len() < total * 4 {
        return DEFAULT_PALETTE;
    }
    let step = (total / 2048).max(1);
    let mut buckets: std::collections::HashMap<(u8, u8, u8), (f32, u32)> =
        std::collections::HashMap::new();
    for i in (0..total).step_by(step) {
        let p = i * 4;
        if rgba[p + 3] < 160 {
            continue;
        }
        let (r, g, b) = (rgba[p], rgba[p + 1], rgba[p + 2]);
        let (rf, gf, bf) = (r as f32, g as f32, b as f32);
        let mx = rf.max(gf).max(bf);
        let mn = rf.min(gf).min(bf);
        let sat = if mx <= 0.0 { 0.0 } else { (mx - mn) / mx };
        if sat < 0.16 || mx < 36.0 || mn > 238.0 {
            continue; // grey, near-black or blown out
        }
        let key = (r >> 5, g >> 5, b >> 5);
        let entry = buckets.entry(key).or_insert((0.0, 0));
        entry.0 += sat;
        entry.1 += 1;
    }
    let mut ranked: Vec<((u8, u8, u8), f32)> = buckets
        .into_iter()
        .map(|(k, (w, n))| (k, w * (n as f32).sqrt()))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut picked: Vec<Color32> = Vec::new();
    for ((r, g, b), _) in ranked {
        if picked.len() >= 3 {
            break;
        }
        let c = vivid(Color32::from_rgb((r << 5) | 16, (g << 5) | 16, (b << 5) | 16));
        if picked.iter().any(|p| color_dist(*p, c) < 64.0) {
            continue;
        }
        picked.push(c);
    }
    let mut fill = DEFAULT_PALETTE.iter();
    while picked.len() < 3 {
        picked.push(*fill.next().unwrap());
    }
    [picked[0], picked[1], picked[2]]
}

/// Lift a colour to a background-friendly vividness.
fn vivid(c: Color32) -> Color32 {
    let (r, g, b) = (c.r() as f32, c.g() as f32, c.b() as f32);
    let mean = (r + g + b) / 3.0;
    // more saturation …
    let mut out = [
        mean + (r - mean) * 1.35,
        mean + (g - mean) * 1.35,
        mean + (b - mean) * 1.35,
    ];
    // … normalised so the brightest channel sits around 185
    let mx = out.iter().cloned().fold(0.0_f32, f32::max);
    if mx > 1.0 {
        let k = 185.0 / mx;
        let mn = out.iter().cloned().fold(255.0_f32, f32::min);
        // do not push anything to black: keep a floor
        let floor = 18.0 - mn * k;
        for v in &mut out {
            *v = (*v * k).max(floor).clamp(0.0, 255.0);
        }
    }
    Color32::from_rgb(out[0] as u8, out[1] as u8, out[2] as u8)
}

fn color_dist(a: Color32, b: Color32) -> f32 {
    let d = |x: u8, y: u8| (x as f32 - y as f32).powi(2);
    (d(a.r(), b.r()) + d(a.g(), b.g()) + d(a.b(), b.b())).sqrt()
}

/// Box-filter RGBA8 down to a tiny square; stretched back up it reads as a blur.
pub fn blur_thumb(rgba: &[u8], width: usize, height: usize) -> egui::ColorImage {
    // average down to a small image, then blur that: stretched with linear
    // filtering this gives a soft, artefact-free backdrop
    let n = 32;
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return egui::ColorImage::new([n, n], BG);
    }
    let mut px = vec![[0f32; 3]; n * n];
    for cy in 0..n {
        for cx in 0..n {
            let x0 = cx * width / n;
            let x1 = (((cx + 1) * width / n).max(x0 + 1)).min(width);
            let y0 = cy * height / n;
            let y1 = (((cy + 1) * height / n).max(y0 + 1)).min(height);
            let mut sum = [0f32; 3];
            let mut cnt = 0f32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * width + x) * 4;
                    for c in 0..3 {
                        sum[c] += rgba[p + c] as f32;
                    }
                    cnt += 1.0;
                }
            }
            px[cy * n + cx] = sum.map(|v| v / cnt.max(1.0));
        }
    }
    // three box-blur passes ≈ a gaussian, edges clamped
    let radius = 3isize;
    for _ in 0..3 {
        for horizontal in [true, false] {
            let src = px.clone();
            for y in 0..n {
                for x in 0..n {
                    let mut sum = [0f32; 3];
                    for d in -radius..=radius {
                        let (sx, sy) = if horizontal {
                            ((x as isize + d).clamp(0, n as isize - 1) as usize, y)
                        } else {
                            (x, (y as isize + d).clamp(0, n as isize - 1) as usize)
                        };
                        let v = src[sy * n + sx];
                        for c in 0..3 {
                            sum[c] += v[c];
                        }
                    }
                    px[y * n + x] = sum.map(|v| v / (2 * radius + 1) as f32);
                }
            }
        }
    }
    // a little extra saturation: blurring washes colours out
    let mut img = egui::ColorImage::new([n, n], BG);
    for (out, v) in img.pixels.iter_mut().zip(px) {
        let grey = (v[0] + v[1] + v[2]) / 3.0;
        let c = v.map(|x| (grey + (x - grey) * 1.35).clamp(0.0, 255.0) as u8);
        *out = Color32::from_rgb(c[0], c[1], c[2]);
    }
    img
}

/// The full-window wallpaper: the current cover, heavily blurred, filling
/// the window and dimmed so text stays readable. `previous` fades out while
/// `cover` fades in (`t` 0‥1). Without a cover the palette glows instead.
/// How the wallpaper moves.
#[derive(Clone, Copy, Default)]
pub struct WallFx {
    /// Drifting colour fields instead of the blurred cover.
    pub fluid: bool,
    /// 0‥1 bass level (0 when the backdrop should not react).
    pub bass: f32,
    pub time: f32,
}

/// A soft round glow: opaque in the middle, nothing at the edge.
pub fn soft_orb(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: Color32) {
    const SEGMENTS: usize = 36;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    for i in 0..=SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + egui::vec2(a.cos(), a.sin()) * radius, Color32::TRANSPARENT);
    }
    for i in 0..SEGMENTS as u32 {
        mesh.add_triangle(0, i + 1, i + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Big colour fields in the song's colours that drift slowly and swell with the bass.
fn paint_fluid(painter: &egui::Painter, rect: Rect, colors: &[Color32; 3], fx: WallFx) {
    let fields = [
        colors[0],
        colors[1],
        colors[2],
        lerp_color(colors[0], colors[2], 0.5),
        lerp_color(colors[1], Color32::WHITE, 0.25),
    ];
    let size = rect.width().max(rect.height());
    for (i, color) in fields.iter().enumerate() {
        let k = i as f32;
        let x = 0.5 + 0.42 * (fx.time * (0.07 + 0.021 * k) + k * 2.1).sin();
        let y = 0.5 + 0.40 * (fx.time * (0.06 + 0.017 * k) + k * 1.3).cos();
        let center = egui::pos2(rect.left() + rect.width() * x, rect.top() + rect.height() * y);
        let radius = size * (0.46 + 0.04 * (fx.time * 0.15 + k).sin()) * (1.0 + 0.16 * fx.bass * (1.0 + 0.3 * (k % 2.0)));
        soft_orb(painter, center, radius, with_alpha(*color, (120.0 + 70.0 * fx.bass).min(255.0) as u8));
    }
}

pub fn paint_song_wallpaper(
    ctx: &egui::Context,
    colors: &[Color32; 3],
    cover: Option<egui::TextureId>,
    previous: Option<egui::TextureId>,
    t: f32,
    fx: WallFx,
) {
    let rect = ctx.screen_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(rect, 0.0, BG);

    // a square cover filling a wide window: crop top and bottom
    let aspect = rect.height() / rect.width().max(1.0);
    let uv = if aspect < 1.0 {
        Rect::from_min_max(egui::pos2(0.0, 0.5 - aspect * 0.5), egui::pos2(1.0, 0.5 + aspect * 0.5))
    } else {
        let w = 1.0 / aspect;
        Rect::from_min_max(egui::pos2(0.5 - w * 0.5, 0.0), egui::pos2(0.5 + w * 0.5, 1.0))
    };
    let alpha = |a: f32| {
        let v = (a.clamp(0.0, 1.0) * 255.0) as u8;
        Color32::from_rgba_premultiplied(v, v, v, v)
    };
    match (cover, previous) {
        _ if fx.fluid => paint_fluid(&painter, rect, colors, fx),
        (Some(new), old) => {
            if let Some(old) = old.filter(|_| t < 1.0) {
                painter.image(old, rect, uv, Color32::WHITE);
            }
            painter.image(new, rect, uv, alpha(t));
        }
        (None, _) => paint_palette_glow(&painter, rect, colors),
    }

    // dim evenly, a little more towards the edges and the bottom
    painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(9, 10, 16, (105.0 - 46.0 * fx.bass) as u8));
    let mut shade = egui::Mesh::default();
    let mid = rect.top() + rect.height() * 0.45;
    shade.colored_vertex(egui::pos2(rect.left(), mid), Color32::TRANSPARENT);
    shade.colored_vertex(egui::pos2(rect.right(), mid), Color32::TRANSPARENT);
    shade.colored_vertex(egui::pos2(rect.right(), rect.bottom()), Color32::from_black_alpha(70));
    shade.colored_vertex(egui::pos2(rect.left(), rect.bottom()), Color32::from_black_alpha(70));
    shade.add_triangle(0, 1, 2);
    shade.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(shade));
}

/// Soft colour glows, used while no cover is available.
fn paint_palette_glow(painter: &egui::Painter, rect: Rect, colors: &[Color32; 3]) {
    let blobs = [
        (egui::pos2(0.16, 0.10), 1.00_f32),
        (egui::pos2(0.90, 0.25), 0.90),
        (egui::pos2(0.50, 0.95), 1.05),
    ];
    let (nx, ny) = (28, 16);
    let aspect = rect.width() / rect.height().max(1.0);
    let mut mesh = egui::Mesh::default();
    for j in 0..=ny {
        for i in 0..=nx {
            let u = i as f32 / nx as f32;
            let v = j as f32 / ny as f32;
            let (mut r, mut g, mut b, mut a) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
            for (k, (centre, radius)) in blobs.iter().enumerate() {
                let dx = (u - centre.x) * aspect;
                let dy = v - centre.y;
                let d = (dx * dx + dy * dy).sqrt() / radius;
                let w = (1.0 - d).clamp(0.0, 1.0).powi(2);
                let c = colors[k];
                r += c.r() as f32 * w;
                g += c.g() as f32 * w;
                b += c.b() as f32 * w;
                a += w;
            }
            let color = Color32::from_rgba_unmultiplied(
                (r / a.max(0.001)) as u8,
                (g / a.max(0.001)) as u8,
                (b / a.max(0.001)) as u8,
                (a.clamp(0.0, 1.0) * 140.0) as u8,
            );
            mesh.colored_vertex(egui::pos2(rect.left() + u * rect.width(), rect.top() + v * rect.height()), color);
        }
    }
    let stride = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let idx = (j * stride + i) as u32;
            mesh.add_triangle(idx, idx + 1, idx + stride as u32 + 1);
            mesh.add_triangle(idx, idx + stride as u32 + 1, idx + stride as u32);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Spacing between the floating cards and the window edges.
pub const GAP: f32 = 12.0;
/// Corner radius of the floating cards.
pub const CARD_RADIUS: u8 = 16;

/// Paint a floating glass card.
pub fn paint_card(painter: &egui::Painter, rect: Rect, fill: Color32) {
    let radius = CornerRadius::same(CARD_RADIUS);
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0_f32, EDGE), egui::StrokeKind::Inside);
}

/// Family with the filled Phosphor glyphs (play, heart, …).
pub fn fill_family() -> FontFamily {
    FontFamily::Name("phosphor-fill".into())
}

/// Family with the bold Phosphor glyphs.
pub fn bold_family() -> FontFamily {
    FontFamily::Name("phosphor-bold".into())
}

/// Install the Phosphor icon fonts. Regular glyphs work inside any text;
/// filled and bold ones need `fill_family()` / `bold_family()`.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    for (name, variant) in [
        ("phosphor-fill", egui_phosphor::Variant::Fill),
        ("phosphor-bold", egui_phosphor::Variant::Bold),
    ] {
        fonts.font_data.insert(name.into(), variant.font_data().into());
        // keep the default fonts as fallback so egui finds its replacement glyphs
        let mut family = vec![name.to_string()];
        family.extend(fonts.families[&FontFamily::Proportional].clone());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }
    ctx.set_fonts(fonts);
}

/// Paint one icon glyph centred on `center`.
pub fn paint_icon(painter: &egui::Painter, center: egui::Pos2, glyph: &str, size: f32, color: Color32, family: FontFamily) {
    painter.text(center, Align2::CENTER_CENTER, glyph, FontId::new(size, family), color);
}

pub fn apply_theme(ctx: &egui::Context) {
    setup_fonts(ctx);
    apply_visuals(ctx);
}

/// Set egui's colours and spacing from the current theme (cheap; again whenever the accent moves).
pub fn apply_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = panel_solid();
    v.extreme_bg_color = lerp_color(panel_solid(), Color32::WHITE, 0.055);
    v.faint_bg_color = CARD;
    v.window_stroke = Stroke::new(1.0_f32, EDGE);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(12);
    v.override_text_color = None;
    v.hyperlink_color = accent();
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = accent_dim();
    v.selection.stroke = Stroke::new(1.0_f32, accent());
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 34, spread: 0, color: Color32::from_black_alpha(130) };
    v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 26, spread: 0, color: Color32::from_black_alpha(120) };

    let radius = CornerRadius::same(10);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CARD;
    w.noninteractive.weak_bg_fill = CARD;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    w.noninteractive.corner_radius = radius;
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);

    w.inactive.bg_fill = CARD;
    w.inactive.weak_bg_fill = CARD;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    w.inactive.corner_radius = radius;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);

    w.hovered.bg_fill = CARD_HOVER;
    w.hovered.weak_bg_fill = CARD_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, EDGE);
    w.hovered.corner_radius = radius;
    w.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT);

    // pressed: a lighter frosted surface. egui also takes bold text colour from here.
    w.active.bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 42);
    w.active.weak_bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 42);
    w.active.bg_stroke = Stroke::new(1.0_f32, accent());
    w.active.corner_radius = radius;
    w.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);

    w.open.bg_fill = CARD_HOVER;
    w.open.weak_bg_fill = CARD_HOVER;
    w.open.corner_radius = radius;
    w.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
    ctx.set_visuals(v);

    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(9.0, 7.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        // thin floating scrollbars that only show while scrolling or hovered
        style.spacing.scroll = egui::style::ScrollStyle {
            bar_width: 6.0,
            floating_width: 2.0,
            bar_inner_margin: 4.0,
            bar_outer_margin: 2.0,
            dormant_background_opacity: 0.0,
            dormant_handle_opacity: 0.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.0,
            active_handle_opacity: 0.45,
            interact_handle_opacity: 0.7,
            ..egui::style::ScrollStyle::floating()
        };
        style.interaction.selectable_labels = false;
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(25.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
    });
}

/// Selected entries in lists and dropdowns: bright green with dark text.
/// (Text selection keeps the dim green, as egui does not recolour selected text.)
pub fn green_selection(ui: &mut Ui) {
    let v = ui.visuals_mut();
    v.selection.bg_fill = accent();
    v.selection.stroke = Stroke::new(1.0_f32, BG);
}

/// A round flat icon button, drawn inside `rect`.
pub fn round_icon_button(ui: &mut Ui, id: egui::Id, rect: Rect, icon: &str) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    pointer_on_hover(&resp);
    if resp.hovered() {
        ui.painter().circle_filled(rect.center(), rect.width() / 2.0, CARD_HOVER);
    }
    let color = if resp.hovered() { TEXT } else { TEXT_DIM };
    paint_icon(ui.painter(), rect.center(), icon, 18.0, color, FontFamily::Proportional);
    resp
}

/// A key cap for shortcut hints.
pub fn keycap(ui: &mut Ui, key: &str) {
    egui::Frame::new()
        .fill(CARD_HOVER)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(key).size(12.0).color(TEXT));
        });
}

/// A highlighted row (current track): frosted surface with an accent bar.
pub fn paint_current_row(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, CornerRadius::same(10), CARD_HOVER);
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + 10.0),
            egui::vec2(3.0, rect.height() - 20.0),
        ),
        2.0,
        accent(),
    );
}

pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn pointer_on_hover(resp: &Response) {
    if resp.hovered() {
        resp.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

// ------------------------------------------------------------------- icons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Prev,
    Next,
    Shuffle,
    /// Repeat; `true` when repeating a single track.
    Repeat(bool),
    Queue,
}

/// A flat transport icon button.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::click());
    pointer_on_hover(&resp);
    let color = if active || resp.is_pointer_button_down_on() {
        accent()
    } else if resp.hovered() {
        TEXT
    } else {
        TEXT_DIM
    };
    let (glyph, family) = match icon {
        Icon::Prev => (icons::SKIP_BACK, fill_family()),
        Icon::Next => (icons::SKIP_FORWARD, fill_family()),
        Icon::Shuffle => (icons::SHUFFLE, bold_family()),
        Icon::Repeat(true) => (icons::REPEAT_ONCE, bold_family()),
        Icon::Repeat(false) => (icons::REPEAT, bold_family()),
        Icon::Queue => (icons::QUEUE, bold_family()),
    };
    paint_icon(ui.painter(), rect.center(), glyph, size * 0.68, color, family);
    resp
}

/// The round primary play/pause button.
pub fn play_button(ui: &mut Ui, playing: bool) -> Response {
    let size = 42.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::click());
    pointer_on_hover(&resp);
    let fill = if resp.is_pointer_button_down_on() {
        accent()
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        TEXT
    };
    let p = ui.painter();
    p.circle_filled(rect.center(), size * 0.5, fill);
    let glyph = if playing { icons::PAUSE } else { icons::PLAY };
    paint_icon(p, rect.center(), glyph, size * 0.46, BG, fill_family());
    resp
}

/// A rounded text button with an optional leading icon.
pub fn action_button(ui: &mut Ui, icon: &str, label: &str, primary: bool) -> Response {
    let width = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(14.0), TEXT)
        .size()
        .x;
    let icon_w = if icon.is_empty() { 0.0 } else { 24.0 };
    let size = egui::vec2(width + icon_w + 32.0, 36.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    if enabled {
        pointer_on_hover(&resp);
    }
    let (fill, ink) = match (enabled, primary, resp.hovered()) {
        (false, _, _) => (CARD, TEXT_DIM),
        (true, true, false) => (accent(), BG),
        (true, true, true) => (lerp_color(accent(), Color32::WHITE, 0.25), BG),
        (true, false, false) => (CARD, TEXT),
        (true, false, true) => (CARD_HOVER, TEXT),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(14.0), ink);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(18), fill);
    let mut x = rect.left() + 16.0;
    if !icon.is_empty() {
        let family = if primary { fill_family() } else { FontFamily::Proportional };
        paint_icon(p, egui::pos2(x + 8.0, rect.center().y), icon, 17.0, ink, family);
        x += icon_w;
    }
    p.galley(egui::pos2(x, rect.center().y - galley.size().y * 0.5), galley, ink);
    resp
}

/// Fill and edge of the glass controls (header buttons, tabs): the panel glass with a breath of
/// the accent, more when hovered or selected.
pub fn glass_colors(hovered: bool, active: bool) -> (Color32, Color32) {
    let glow = if active { 0.26 } else if hovered { 0.17 } else { 0.08 };
    let fill = with_alpha(lerp_color(panel_solid(), accent(), glow), if hovered || active { 238 } else { 220 });
    let edge = if active {
        with_alpha(accent(), 160)
    } else if hovered {
        with_alpha(accent(), 100)
    } else {
        with_alpha(lerp_color(Color32::WHITE, accent(), 0.5), 56)
    };
    (fill, edge)
}

/// A text field in the look of the search capsule: a glass field with a rounded edge that lights up
/// in the accent when it has the focus. `rows` 1 is a single line; `width` may be infinite.
#[allow(clippy::too_many_arguments)]
pub fn glass_input(ui: &mut Ui, id: egui::Id, text: &mut String, hint: &str, width: f32, rows: usize, font_size: f32) -> Response {
    let single = rows <= 1;
    let height = if single { (font_size + 24.0).max(40.0) } else { rows as f32 * (font_size + 8.0) + 22.0 };
    let width = if width.is_finite() { width } else { ui.available_width() };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    let hovered = ui.rect_contains_pointer(rect);

    // a little lighter than the glass around it, with the accent seeping in while typing
    let base = lerp_color(panel_solid(), Color32::WHITE, 0.055);
    let fill = with_alpha(lerp_color(base, accent(), if focused { 0.09 } else { 0.0 }), 250);
    let (stroke, edge) = if focused {
        (1.5_f32, with_alpha(accent(), 210))
    } else if hovered {
        (1.0_f32, with_alpha(accent(), 100))
    } else {
        (1.0_f32, glass_colors(false, false).1)
    };
    let radius = CornerRadius::same(12);
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter().rect_stroke(rect, radius, Stroke::new(stroke, edge), egui::StrokeKind::Inside);

    let inner = rect.shrink2(egui::vec2(14.0, if single { 0.0 } else { 10.0 }));
    let edit = if single { egui::TextEdit::singleline(text) } else { egui::TextEdit::multiline(text).desired_rows(rows) };
    let back = ui.interact(rect, id.with("frame"), Sense::click());
    let resp = ui.put(
        inner,
        edit.id(id)
            .frame(false)
            .hint_text(egui::RichText::new(hint).color(TEXT_DIM))
            .font(FontId::proportional(font_size))
            .margin(egui::vec2(0.0, 0.0))
            .vertical_align(if single { egui::Align::Center } else { egui::Align::Min })
            .desired_width(inner.width()),
    );
    if back.clicked() {
        resp.request_focus();
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    resp
}

/// A check box in the glass look: a rounded square that fills with the accent when checked.
/// Returns a response that is `changed()` when it was clicked.
pub fn checkbox(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let text_w = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(14.5), TEXT).size().x;
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(30.0 + text_w + 6.0, 30.0), Sense::click());
    if ui.is_enabled() {
        pointer_on_hover(&resp);
    }
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    let hovered = resp.hovered();
    let boxr = Rect::from_center_size(egui::pos2(rect.left() + 11.0, rect.center().y), egui::vec2(20.0, 20.0));
    let p = ui.painter();
    let radius = CornerRadius::same(7);
    if *value {
        p.rect_filled(boxr, radius, lerp_color(accent(), Color32::WHITE, if hovered { 0.18 } else { 0.0 }));
        paint_icon(p, boxr.center(), icons::CHECK, 14.0, BG, bold_family());
    } else {
        let base = lerp_color(panel_solid(), Color32::WHITE, if hovered { 0.10 } else { 0.055 });
        p.rect_filled(boxr, radius, base);
        let edge = if hovered { with_alpha(accent(), 140) } else { glass_colors(false, false).1 };
        p.rect_stroke(boxr, radius, Stroke::new(1.0_f32, edge), egui::StrokeKind::Inside);
    }
    let ink = if ui.is_enabled() { TEXT } else { TEXT_DIM };
    p.text(egui::pos2(boxr.right() + 10.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.5), ink);
    resp
}

/// A glass button with an icon and a label, the height of the segmented control.
pub fn glass_button(ui: &mut Ui, icon: &str, label: &str) -> Response {
    let text_w = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(14.0), TEXT).size().x;
    let icon_w = if icon.is_empty() { 0.0 } else { 26.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(text_w + icon_w + 34.0, 38.0), Sense::click());
    pointer_on_hover(&resp);
    let (fill, edge) = glass_colors(resp.hovered(), false);
    let p = ui.painter();
    let radius = CornerRadius::same(13);
    p.rect_filled(rect, radius, fill);
    p.rect_stroke(rect, radius, Stroke::new(1.0_f32, edge), egui::StrokeKind::Inside);
    let ink = lerp_color(TEXT, accent(), if resp.hovered() { 0.5 } else { 0.3 });
    let mut x = rect.left() + 17.0;
    if !icon.is_empty() {
        paint_icon(p, egui::pos2(x + 8.0, rect.center().y), icon, 17.0, ink, FontFamily::Proportional);
        x += icon_w;
    }
    p.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(14.0), ink);
    resp
}

/// Tabs in one glass capsule; the highlight slides to the chosen one. Returns the index clicked.
pub fn segmented(ui: &mut Ui, id: egui::Id, labels: &[&str], selected: usize) -> Option<usize> {
    let font = FontId::proportional(14.0);
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| ui.painter().layout_no_wrap((*l).to_string(), font.clone(), TEXT).size().x + 34.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, 38.0), Sense::hover());
    let (fill, edge) = glass_colors(false, false);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(13), fill);
    p.rect_stroke(rect, CornerRadius::same(13), Stroke::new(1.0_f32, edge), egui::StrokeKind::Inside);

    // where each segment is, and the highlight gliding to the chosen one
    let mut x = rect.left() + 3.0;
    let slots: Vec<Rect> = widths
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(egui::pos2(x, rect.top() + 3.0), egui::vec2(*w, 32.0));
            x += w;
            r
        })
        .collect();
    let target = slots.get(selected).copied().unwrap_or(rect);
    let glide_x = ui.ctx().animate_value_with_time(id.with("x"), target.left(), 0.18);
    let glide_w = ui.ctx().animate_value_with_time(id.with("w"), target.width(), 0.18);
    let mark = Rect::from_min_size(egui::pos2(glide_x, target.top()), egui::vec2(glide_w, target.height()));
    let p = ui.painter();
    p.rect_filled(mark, CornerRadius::same(10), with_alpha(lerp_color(panel_solid(), accent(), 0.34), 255));
    p.rect_stroke(mark, CornerRadius::same(10), Stroke::new(1.0_f32, with_alpha(accent(), 150)), egui::StrokeKind::Inside);

    let mut clicked = None;
    for (i, (slot, label)) in slots.iter().zip(labels).enumerate() {
        let resp = ui.interact(*slot, id.with(i), Sense::click());
        pointer_on_hover(&resp);
        if resp.clicked() {
            clicked = Some(i);
        }
        let ink = if i == selected {
            accent()
        } else if resp.hovered() {
            TEXT
        } else {
            TEXT_DIM
        };
        ui.painter().text(slot.center(), Align2::CENTER_CENTER, *label, font.clone(), ink);
    }
    clicked
}

/// A compact toggle pill with a text label.
pub fn pill(ui: &mut Ui, label: &str, active: bool, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 24.0), Sense::click());
    pointer_on_hover(&resp);
    let (fill, text) = if active {
        (accent(), BG)
    } else if resp.hovered() {
        (CARD_HOVER, TEXT)
    } else {
        (CARD, TEXT_DIM)
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), fill);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.5),
        text,
    );
    resp
}

/// A full-width sidebar entry with an accent bar when selected.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool, badge: Option<&str>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 36.0),
        Sense::click(),
    );
    pointer_on_hover(&resp);
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(10), CARD_HOVER);
        p.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + 8.0),
                egui::vec2(3.0, rect.height() - 16.0),
            ),
            2.0,
            accent(),
        );
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(10), CARD);
    }
    let ink = if selected { TEXT } else { TEXT_DIM };
    paint_icon(
        p,
        egui::pos2(rect.left() + 26.0, rect.center().y),
        icon,
        18.0,
        if selected { accent() } else { ink },
        FontFamily::Proportional,
    );
    p.text(
        egui::pos2(rect.left() + 46.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.5),
        ink,
    );
    if let Some(badge) = badge {
        let galley = p.layout_no_wrap(badge.to_string(), FontId::proportional(10.5), BG);
        let size = galley.size() + egui::vec2(10.0, 4.0);
        let r = egui::Rect::from_center_size(
            egui::pos2(rect.right() - size.x * 0.5 - 8.0, rect.center().y),
            size,
        );
        p.rect_filled(r, CornerRadius::same(8), accent());
        p.galley(r.center() - galley.size() * 0.5, galley, BG);
    }
    resp
}

/// A thin horizontal slider on a normalised 0‥1 value (seek bar, volume).
pub fn thin_slider(ui: &mut Ui, value: &mut f32, width: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(width, 20.0), Sense::click_and_drag());
    pointer_on_hover(&resp);
    let pad = 7.0;
    let track_w = (rect.width() - pad * 2.0).max(1.0);
    if resp.dragged() || resp.drag_started() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - rect.left() - pad) / track_w).clamp(0.0, 1.0);
            if (t - *value).abs() > f32::EPSILON {
                *value = t;
                resp.mark_changed();
            }
        }
    }
    let t = value.clamp(0.0, 1.0);
    let y = rect.center().y;
    let x0 = rect.left() + pad;
    let x1 = x0 + track_w * t;
    let active = resp.hovered() || resp.dragged();
    let p = ui.painter();
    p.rect_filled(
        egui::Rect::from_min_max(egui::pos2(x0, y - 2.0), egui::pos2(x0 + track_w, y + 2.0)),
        2.0,
        LINE,
    );
    p.rect_filled(
        egui::Rect::from_min_max(egui::pos2(x0, y - 2.0), egui::pos2(x1.max(x0), y + 2.0)),
        2.0,
        if active { accent() } else { TEXT_DIM },
    );
    if active {
        p.circle_filled(egui::pos2(x1, y), 6.0, TEXT);
    }
    resp
}

/// A lightweight card frame.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, EDGE))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(14))
}

/// A heart, either filled or as an outline.
pub fn paint_heart(painter: &egui::Painter, center: egui::Pos2, size: f32, color: Color32, filled: bool) {
    let family = if filled { fill_family() } else { FontFamily::Proportional };
    paint_icon(painter, center, icons::HEART, size * 1.15, color, family);
}

/// A round heart toggle button.
pub fn heart_button(ui: &mut Ui, liked: bool, size: f32, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size + 8.0, size + 8.0), Sense::click());
    if enabled {
        pointer_on_hover(&resp);
    }
    let color = if !enabled {
        LINE
    } else if liked {
        accent()
    } else if resp.hovered() {
        TEXT
    } else {
        TEXT_DIM
    };
    paint_heart(ui.painter(), rect.center(), size, color, liked);
    resp
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    fn flat(c: Color32) -> [Color32; 3] {
        [c, c, c]
    }

    #[test]
    fn a_colourful_cover_gives_a_readable_accent_of_the_same_hue() {
        let blue = Color32::from_rgb(70, 110, 220);
        let t = theme_for([blue, Color32::from_rgb(90, 60, 160), Color32::from_rgb(30, 80, 120)]);
        assert!(contrast(t.accent, panel_solid_for(t.tint)) >= 6.0);
        assert!((to_hsl(t.accent).0 - to_hsl(blue).0).abs() < 0.05, "same hue");
        assert!(luminance(t.tint) < 0.02, "the glass stays dark");
    }

    #[test]
    fn every_hue_stays_readable_on_the_glass_and_under_dark_button_text() {
        for step in 0..72 {
            let c = from_hsl(step as f32 / 72.0, 0.85, 0.5);
            let t = theme_for(flat(c));
            assert!(contrast(t.accent, panel_solid_for(t.tint)) >= 4.5, "hue {step}: {:?}", t.accent);
            assert!(contrast(t.accent, BG) >= 5.0, "hue {step}: dark text on the accent");
        }
    }

    #[test]
    fn grey_black_and_white_covers_fall_back_to_the_default_accent() {
        for c in [Color32::from_gray(128), Color32::from_gray(8), Color32::from_gray(250), Color32::from_rgb(120, 124, 122)] {
            assert_eq!(theme_for(flat(c)).accent, DEFAULT_ACCENT, "{c:?}");
        }
    }

    #[test]
    fn bright_covers_get_more_opaque_panels_than_dark_ones() {
        let dark = theme_for(flat(Color32::from_rgb(30, 20, 90)));
        let bright = theme_for(flat(Color32::from_rgb(255, 235, 120)));
        assert!(bright.light > 0.8 && dark.light < 0.2, "{} {}", bright.light, dark.light);
        set_theme(dark);
        let a_dark = panel().a();
        set_theme(bright);
        let a_bright = panel().a();
        set_theme(Theme::default());
        assert!(a_bright > a_dark + 30, "{a_bright} vs {a_dark}");
    }

    #[test]
    fn the_theme_glides_between_two_songs() {
        let a = Theme::default();
        let b = theme_for(flat(Color32::from_rgb(220, 60, 90)));
        assert_eq!(lerp_theme(a, b, 0.0), a);
        assert_eq!(lerp_theme(a, b, 1.0), b);
        let mid = lerp_theme(a, b, 0.5);
        assert!(mid != a && mid != b);
    }

    #[test]
    fn a_glide_starts_from_where_it_is_and_takes_its_time() {
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let mut g = Glide::new(0.0_f32);
        assert_eq!(g.at(10.0, 0.0, 2.0, 0.0, lerp), 0.0);
        // a new goal: nothing happens at once, then it moves smoothly, and it ends on the goal
        assert_eq!(g.at(10.0, 1.0, 2.0, 0.0, lerp), 0.0);
        let mid = g.at(11.0, 1.0, 2.0, 0.0, lerp);
        assert!((mid - 0.5).abs() < 1e-5, "{mid}");
        assert!(!g.done(11.0));
        assert_eq!(g.at(12.5, 1.0, 2.0, 0.0, lerp), 1.0);
        assert!(g.done(12.5));
        // a goal that changes midway starts from the value reached so far, not from the old start
        let mut g = Glide::new(0.0_f32);
        g.at(0.0, 1.0, 2.0, 0.0, lerp);
        let reached = g.at(1.0, 1.0, 2.0, 0.0, lerp);
        let after = g.at(1.0, 0.0, 2.0, 0.0, lerp);
        assert!((after - reached).abs() < 1e-5, "no jump: {reached} -> {after}");
        // a head start shortens the wait (the song changed before the colours were known)
        let mut g = Glide::new(0.0_f32);
        g.at(5.0, 1.0, 4.0, 2.0, lerp);
        assert!((g.at(5.0, 1.0, 4.0, 2.0, lerp) - 0.5).abs() < 1e-5);
    }
}
