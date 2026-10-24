//! Theme and small custom widgets shared by all views.

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
pub const ACCENT: Color32 = Color32::from_rgb(166, 227, 161); // green
pub const ACCENT_DIM: Color32 = glass(70, 166, 227, 161);
pub const BG: Color32 = Color32::from_rgb(11, 12, 18); // window clear colour
pub const PANEL: Color32 = glass(178, 16, 17, 26); // acrylic glass
pub const PANEL_CLEAR: Color32 = glass(118, 12, 13, 20); // extra transparent (full-screen player)
pub const PANEL_SOLID: Color32 = Color32::from_rgb(19, 20, 30); // menus, modals: fully opaque
pub const CARD: Color32 = glass(14, 255, 255, 255);
pub const CARD_HOVER: Color32 = glass(30, 255, 255, 255);
pub const LINE: Color32 = glass(34, 255, 255, 255);
pub const EDGE: Color32 = glass(40, 255, 255, 255); // hairline on glass
pub const TEXT: Color32 = Color32::from_rgb(222, 228, 246);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 156, 180);
pub const DANGER: Color32 = Color32::from_rgb(243, 139, 168); // red
pub const WARN: Color32 = Color32::from_rgb(249, 226, 175); // yellow
pub const OK: Color32 = Color32::from_rgb(166, 227, 161); // green

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
pub fn paint_song_wallpaper(
    ctx: &egui::Context,
    colors: &[Color32; 3],
    cover: Option<egui::TextureId>,
    previous: Option<egui::TextureId>,
    t: f32,
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
        (Some(new), old) => {
            if let Some(old) = old.filter(|_| t < 1.0) {
                painter.image(old, rect, uv, Color32::WHITE);
            }
            painter.image(new, rect, uv, alpha(t));
        }
        (None, _) => paint_palette_glow(&painter, rect, colors),
    }

    // dim evenly, a little more towards the edges and the bottom
    painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(9, 10, 16, 105));
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
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL_SOLID;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.window_stroke = Stroke::new(1.0_f32, EDGE);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(12);
    v.override_text_color = None;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
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
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
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
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0_f32, BG);
}

/// A flat icon button with a rounded hover background.
pub fn square_icon_button(ui: &mut Ui, icon: &str, width: f32, height: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click());
    pointer_on_hover(&resp);
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(10), CARD_HOVER);
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
        ACCENT,
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
    Visualizer,
}

/// A flat transport icon button.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::click());
    pointer_on_hover(&resp);
    let color = if active || resp.is_pointer_button_down_on() {
        ACCENT
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
        Icon::Visualizer => (icons::WAVEFORM, bold_family()),
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
        ACCENT
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
        (true, true, false) => (ACCENT, BG),
        (true, true, true) => (Color32::from_rgb(190, 238, 185), BG),
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

/// A compact toggle pill with a text label.
pub fn pill(ui: &mut Ui, label: &str, active: bool, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 24.0), Sense::click());
    pointer_on_hover(&resp);
    let (fill, text) = if active {
        (ACCENT, BG)
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
            ACCENT,
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
        if selected { ACCENT } else { ink },
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
        p.rect_filled(r, CornerRadius::same(8), ACCENT);
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
        if active { ACCENT } else { TEXT_DIM },
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
        ACCENT
    } else if resp.hovered() {
        TEXT
    } else {
        TEXT_DIM
    };
    paint_heart(ui.painter(), rect.center(), size, color, liked);
    resp
}
