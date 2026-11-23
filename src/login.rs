//! The look of the sign-in screen: slowly drifting colour clouds behind a glass card,
//! the app icon, a big button, and the device code as a row of tiles.

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Response, RichText, Sense, Ui};

use crate::ui::{self, accent, with_alpha, TEXT, TEXT_DIM};

/// A soft round glow: opaque in the middle, nothing at the edge.
pub fn orb(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    ui::soft_orb(painter, center, radius, color);
}

/// Colour clouds that drift around slowly (in the colours of the theme).
pub fn backdrop(painter: &egui::Painter, rect: Rect, time: f32) {
    let a = accent();
    let (h, s, l) = ui::to_hsl(a);
    let shifted = |dh: f32| ui::from_hsl((h + dh).rem_euclid(1.0), s.max(0.45), l.clamp(0.4, 0.62));
    let colors = [a, shifted(0.12), shifted(-0.1), shifted(0.5)];
    let size = rect.width().max(rect.height());
    for (i, color) in colors.iter().enumerate() {
        let k = i as f32;
        let x = 0.5 + 0.38 * (time * (0.11 + 0.03 * k) + k * 1.9).sin();
        let y = 0.5 + 0.34 * (time * (0.09 + 0.025 * k) + k * 2.7).cos();
        let center = Pos2::new(rect.left() + rect.width() * x, rect.top() + rect.height() * y);
        orb(painter, center, size * (0.42 + 0.05 * (time * 0.2 + k).sin()), with_alpha(*color, 62));
    }
}

/// The app icon, loaded once.
pub fn logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("login-logo");
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(tex);
    }
    let image = image::load_from_memory(crate::desktop::ICON_PNG).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let tex = ctx.load_texture("login-logo", egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// The main button: filled with the accent colour, brighter under the mouse.
pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let font = FontId::proportional(16.0);
    let text_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2((text_w + 56.0).max(230.0), 46.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled {
        ui::pointer_on_hover(&resp);
    }
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.15);
    let base = if enabled { accent() } else { ui::lerp_color(ui::panel_solid(), accent(), 0.35) };
    let fill = ui::lerp_color(base, Color32::WHITE, 0.16 * hover);
    let p = ui.painter();
    if enabled {
        // a halo that grows when hovered
        p.rect_filled(rect.expand(1.5 + 3.0 * hover), 25.0, with_alpha(accent(), (12.0 + 26.0 * hover) as u8));
    }
    p.rect_filled(rect, 23.0, fill);
    let dark = if ui::luminance(fill) > 0.5 { Color32::from_rgb(14, 15, 22) } else { Color32::WHITE };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, FontId::proportional(16.0), dark);
    resp
}

/// A quieter button with an outline.
pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    let font = FontId::proportional(14.0);
    let text_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(text_w + 34.0, 36.0), Sense::click());
    ui::pointer_on_hover(&resp);
    let (fill, edge) = ui::glass_colors(resp.hovered(), false);
    ui.painter().rect_filled(rect, 18.0, fill);
    ui.painter().rect_stroke(rect, 18.0, egui::Stroke::new(1.0_f32, edge), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, TEXT);
    resp
}

/// The device code, one tile per character.
pub fn code_tiles(ui: &mut Ui, code: &str) {
    let chars: Vec<char> = code.chars().collect();
    let tile = egui::vec2(40.0, 54.0);
    let gap = 7.0;
    let total = chars.len() as f32 * tile.x + (chars.len().saturating_sub(1)) as f32 * gap;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, tile.y), Sense::hover());
    let p = ui.painter();
    for (i, c) in chars.iter().enumerate() {
        let min = rect.min + egui::vec2(i as f32 * (tile.x + gap), 0.0);
        let r = Rect::from_min_size(min, tile);
        p.rect_filled(r, 11.0, ui::lerp_color(ui::panel_solid(), accent(), 0.10));
        p.rect_stroke(r, 11.0, egui::Stroke::new(1.0_f32, with_alpha(accent(), 110)), egui::StrokeKind::Inside);
        p.text(r.center(), egui::Align2::CENTER_CENTER, c, FontId::monospace(30.0), accent());
    }
}

/// Small grey text.
pub fn hint(text: &str) -> RichText {
    RichText::new(text).size(12.5).color(TEXT_DIM)
}

/// Centred text that stays readable on any background: light, with a soft dark shadow.
pub fn title(ui: &mut Ui, text: &str, size: f32) {
    let font = FontId::proportional(size);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), TEXT);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(0.0, 2.0), Sense::hover());
    let at = rect.center_top();
    for (dx, dy, a) in [(0.0, 1.5, 120u8), (0.0, 0.0, 70)] {
        ui.painter().text(at + egui::vec2(dx, dy), egui::Align2::CENTER_TOP, text, font.clone(), Color32::from_black_alpha(a));
    }
    ui.painter().text(at, egui::Align2::CENTER_TOP, text, font, Color32::WHITE);
}
