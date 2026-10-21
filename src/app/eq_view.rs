//! The equaliser page: response graph with spectrum analyser, draggable band
//! nodes, per-band editor, faders with preamp and an output meter.

use eframe::egui::{
    self, ecolor::Hsva, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect,
    Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui,
};

use crate::eq::{
    self, curve_freq, Band, FilterKind, UserPreset, MAX_FREQ, MAX_GAIN_DB, MAX_PREAMP_DB, MAX_Q,
    MIN_FREQ, MIN_Q, NUM_BANDS, SPEC_BINS,
};
use crate::ui::{
    card, pill, pointer_on_hover, with_alpha, ACCENT, BG, CARD_HOVER, DANGER, LINE, OK, TEXT,
    TEXT_DIM, WARN,
};

use super::App;

/// Catppuccin sky — distinct from the green spectrum behind it.
const CURVE: Color32 = Color32::from_rgb(137, 220, 235);
const DB_RANGE: f32 = MAX_GAIN_DB;
const CURVE_POINTS: usize = 260;
const GRAPH_HEIGHT: f32 = 300.0;
const FADER_HEIGHT: f32 = 170.0;

#[derive(Default)]
struct Edit {
    changed: bool,
    /// The change came from a preset (so the preset name stays shown).
    keep_preset: bool,
}

fn band_color(index: usize) -> Color32 {
    Hsva::new(index as f32 / NUM_BANDS as f32 * 0.8 + 0.02, 0.55, 1.0, 1.0).into()
}

fn freq_label(freq: f32) -> String {
    if freq >= 1000.0 {
        let k = freq / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}k", k.round() as i32)
        } else {
            format!("{k:.1}k")
        }
    } else {
        format!("{}", freq.round() as i32)
    }
}

fn db_label(db: f32) -> String {
    if db.abs() < 0.05 {
        "0.0".to_string()
    } else {
        format!("{db:+.1}")
    }
}

impl App {
    pub(super) fn equalizer_view(&mut self, ui: &mut Ui) {
        let mut edit = Edit::default();

        ui.horizontal(|ui| {
            ui.heading("Equalizer");
            ui.add_space(6.0);
            ui.label(RichText::new("Parametric · 10 bands · Limiter").color(TEXT_DIM));
        });
        ui.add_space(10.0);

        self.eq_toolbar(ui, &mut edit);
        ui.add_space(10.0);
        self.eq_graph(ui, &mut edit);
        ui.add_space(10.0);
        self.eq_band_editor(ui, &mut edit);
        ui.add_space(10.0);
        self.eq_faders(ui, &mut edit);
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Drag nodes: frequency & gain · Mouse wheel: Q · Double-click: reset gain · Right-click: band on/off",
            )
            .size(12.0)
            .color(TEXT_DIM),
        );

        if edit.changed {
            if !edit.keep_preset {
                self.eq_preset = None;
            }
            self.cfg.eq = self.cfg.eq.clone().clamped();
            self.eq_shared.update(&self.cfg.eq);
            self.touch();
        }
    }

    // ---------------------------------------------------------------- toolbar

    fn eq_toolbar(&mut self, ui: &mut Ui, edit: &mut Edit) {
        card().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let on = self.cfg.eq.enabled;
                if pill(ui, if on { "EQ AN" } else { "EQ OFF" }, on, 78.0).clicked() {
                    self.cfg.eq.enabled = !on;
                    edit.changed = true;
                    edit.keep_preset = true;
                }

                ui.add_space(6.0);
                ui.label(RichText::new("Preset").color(TEXT_DIM));
                let selected = self
                    .eq_preset
                    .clone()
                    .unwrap_or_else(|| "Custom".to_string());
                let mut pick_builtin: Option<usize> = None;
                let mut pick_user: Option<usize> = None;
                egui::ComboBox::from_id_salt("eq_preset")
                    .selected_text(selected.clone())
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        crate::ui::green_selection(ui);
                        for (i, preset) in eq::presets().iter().enumerate() {
                            let is_sel = self.eq_preset.as_deref() == Some(preset.name);
                            if ui.selectable_label(is_sel, preset.name).clicked() {
                                pick_builtin = Some(i);
                            }
                        }
                        if !self.cfg.eq.user_presets.is_empty() {
                            ui.separator();
                            ui.label(RichText::new("Custom").size(12.0).color(TEXT_DIM));
                            for (i, preset) in self.cfg.eq.user_presets.iter().enumerate() {
                                let is_sel = self.eq_preset.as_deref() == Some(preset.name.as_str());
                                if ui.selectable_label(is_sel, &preset.name).clicked() {
                                    pick_user = Some(i);
                                }
                            }
                        }
                    });
                if let Some(i) = pick_builtin {
                    let preset = eq::presets()[i];
                    self.cfg.eq.apply_gains(&preset.gains);
                    self.cfg.eq.enabled = true;
                    self.eq_preset = Some(preset.name.to_string());
                    edit.changed = true;
                    edit.keep_preset = true;
                }
                if let Some(i) = pick_user {
                    let preset = self.cfg.eq.user_presets[i].clone();
                    self.cfg.eq.bands = preset.bands;
                    self.cfg.eq.preamp_db = preset.preamp_db;
                    self.cfg.eq.enabled = true;
                    self.eq_preset = Some(preset.name);
                    edit.changed = true;
                    edit.keep_preset = true;
                }

                if ui.button("Reset").clicked() {
                    self.cfg.eq.reset_layout();
                    self.eq_preset = Some("Flat".to_string());
                    edit.changed = true;
                    edit.keep_preset = true;
                }

                ui.add_space(10.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.eq_new_name)
                        .hint_text("Name for new preset")
                        .desired_width(150.0),
                );
                let name = self.eq_new_name.trim().to_string();
                if ui
                    .add_enabled(!name.is_empty(), egui::Button::new("Save"))
                    .clicked()
                {
                    let preset = UserPreset {
                        name: name.clone(),
                        preamp_db: self.cfg.eq.preamp_db,
                        bands: self.cfg.eq.bands,
                    };
                    match self.cfg.eq.user_presets.iter_mut().find(|p| p.name == name) {
                        Some(existing) => *existing = preset,
                        None => self.cfg.eq.user_presets.push(preset),
                    }
                    self.eq_preset = Some(name);
                    self.eq_new_name.clear();
                    edit.changed = true;
                    edit.keep_preset = true;
                }
                let current_user = self
                    .eq_preset
                    .as_ref()
                    .and_then(|n| self.cfg.eq.user_presets.iter().position(|p| &p.name == n));
                if let Some(index) = current_user {
                    if ui.button("Delete").clicked() {
                        self.cfg.eq.user_presets.remove(index);
                        self.eq_preset = None;
                        edit.changed = true;
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------------ graph

    fn eq_graph(&mut self, ui: &mut Ui, edit: &mut Edit) {
        let (resp, painter) = ui.allocate_painter(
            vec2(ui.available_width(), GRAPH_HEIGHT),
            Sense::hover(),
        );
        let outer = resp.rect;
        let plot = Rect::from_min_max(
            outer.min + vec2(46.0, 14.0),
            outer.max - vec2(16.0, 28.0),
        );
        let ln_span = (MAX_FREQ / MIN_FREQ).ln();
        let x_of = |f: f32| plot.left() + (f / MIN_FREQ).ln() / ln_span * plot.width();
        let f_of = |x: f32| {
            MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(((x - plot.left()) / plot.width()).clamp(0.0, 1.0))
        };
        let y_of = |db: f32| plot.center().y - db / DB_RANGE * plot.height() * 0.5;
        let db_of = |y: f32| {
            ((plot.center().y - y) / (plot.height() * 0.5) * DB_RANGE).clamp(-MAX_GAIN_DB, MAX_GAIN_DB)
        };

        let enabled = self.cfg.eq.enabled;
        let sr = self.eq_shared.sample_rate().max(8000.0);

        painter.rect_filled(outer, CornerRadius::same(10), BG);
        let clipped = painter.with_clip_rect(plot.expand(2.0));
        let grid = Stroke::new(1.0_f32, with_alpha(LINE, 150));
        let grid_minor = Stroke::new(1.0_f32, with_alpha(LINE, 60));

        // dB grid
        for db in (-15..=15).step_by(3) {
            let y = y_of(db as f32);
            let stroke = if db == 0 { Stroke::new(1.0_f32, Color32::from_gray(90)) } else { grid_minor };
            painter.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], stroke);
            if db % 6 == 0 {
                let text = if db == 0 { "0".to_string() } else { format!("{db:+}") };
                painter.text(
                    pos2(plot.left() - 8.0, y),
                    Align2::RIGHT_CENTER,
                    text,
                    FontId::proportional(11.0),
                    TEXT_DIM,
                );
            }
        }
        // frequency grid
        let mut decade = 10.0_f32;
        while decade <= 10_000.0 {
            for m in 1..=9 {
                let f = decade * m as f32;
                if !(MIN_FREQ..=MAX_FREQ).contains(&f) {
                    continue;
                }
                let x = x_of(f);
                let major = matches!(m, 1 | 2 | 5);
                painter.line_segment(
                    [pos2(x, plot.top()), pos2(x, plot.bottom())],
                    if major { grid } else { grid_minor },
                );
                if major {
                    painter.text(
                        pos2(x, plot.bottom() + 6.0),
                        Align2::CENTER_TOP,
                        freq_label(f),
                        FontId::proportional(11.0),
                        TEXT_DIM,
                    );
                }
            }
            decade *= 10.0;
        }

        // spectrum analyser
        let spectrum = self.eq_shared.spectrum();
        let bar_w = plot.width() / SPEC_BINS as f32;
        for (i, db) in spectrum.iter().enumerate() {
            let frac = ((db + 85.0) / 70.0).clamp(0.0, 1.0);
            if frac <= 0.0 {
                continue;
            }
            let h = frac * plot.height() * 0.9;
            clipped.rect_filled(
                Rect::from_min_max(
                    pos2(plot.left() + i as f32 * bar_w, plot.bottom() - h),
                    pos2(plot.left() + (i + 1) as f32 * bar_w + 0.6, plot.bottom()),
                ),
                0.0,
                with_alpha(ACCENT, 34),
            );
        }

        // combined response
        let curve = self.cfg.eq.curve(sr, CURVE_POINTS);
        let points: Vec<_> = curve
            .iter()
            .enumerate()
            .map(|(i, db)| pos2(x_of(curve_freq(i, CURVE_POINTS)), y_of(db.clamp(-DB_RANGE - 4.0, DB_RANGE + 4.0))))
            .collect();
        let line_color = if enabled { CURVE } else { Color32::from_gray(110) };
        // selected band on its own
        let selected = self.eq_selected.min(NUM_BANDS - 1);
        let sel_band = self.cfg.eq.bands[selected];
        if sel_band.enabled && sel_band.kind.has_gain() {
            let own: Vec<_> = sel_band
                .curve(sr, CURVE_POINTS)
                .iter()
                .enumerate()
                .map(|(i, db)| pos2(x_of(curve_freq(i, CURVE_POINTS)), y_of(*db)))
                .collect();
            clipped.add(Shape::line(own, Stroke::new(1.2_f32, with_alpha(band_color(selected), 150))));
        }
        clipped.add(Shape::line(points, Stroke::new(2.4_f32, line_color)));
        painter.rect_stroke(outer, CornerRadius::same(10), Stroke::new(1.0_f32, LINE), StrokeKind::Inside);

        // band nodes (interact first, paint afterwards)
        let mut take_scroll = true;
        let mut hovered_info: Option<(usize, Response)> = None;
        for i in 0..NUM_BANDS {
            let band = self.cfg.eq.bands[i];
            let gain = if band.kind.has_gain() { band.gain_db } else { 0.0 };
            let pos = pos2(x_of(band.freq), y_of(gain));
            let hit = Rect::from_center_size(pos, vec2(26.0, 26.0));
            let node = ui.interact(hit, ui.id().with(("eq_node", i)), Sense::click_and_drag());
            if node.hovered() || node.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if node.clicked() || node.drag_started() {
                self.eq_selected = i;
            }
            if node.dragged() {
                if let Some(p) = node.interact_pointer_pos() {
                    let b = &mut self.cfg.eq.bands[i];
                    b.freq = f_of(p.x);
                    if b.kind.has_gain() {
                        b.gain_db = (db_of(p.y) * 10.0).round() / 10.0;
                    }
                    edit.changed = true;
                }
            }
            if node.double_clicked() {
                self.cfg.eq.bands[i].gain_db = 0.0;
                edit.changed = true;
            }
            if node.secondary_clicked() {
                let b = &mut self.cfg.eq.bands[i];
                b.enabled = !b.enabled;
                edit.changed = true;
            }
            if node.hovered() && take_scroll {
                let scroll = ui.input_mut(|inp| std::mem::take(&mut inp.smooth_scroll_delta.y));
                if scroll != 0.0 {
                    let b = &mut self.cfg.eq.bands[i];
                    b.q = (b.q * (1.0 + scroll * 0.004)).clamp(MIN_Q, MAX_Q);
                    self.eq_selected = i;
                    edit.changed = true;
                    take_scroll = false;
                }
                hovered_info = Some((i, node.clone()));
            }
        }
        for i in 0..NUM_BANDS {
            let band = self.cfg.eq.bands[i];
            let gain = if band.kind.has_gain() { band.gain_db } else { 0.0 };
            let pos = pos2(x_of(band.freq), y_of(gain));
            let color = band_color(i);
            let is_sel = i == selected;
            let radius = if is_sel { 10.0 } else { 8.5 };
            let fill = if band.enabled { color } else { Color32::from_gray(70) };
            painter.circle_filled(pos, radius + 3.0, with_alpha(color, if is_sel { 60 } else { 0 }));
            painter.circle_filled(pos, radius, fill);
            painter.circle_stroke(
                pos,
                radius,
                Stroke::new(if is_sel { 2.0_f32 } else { 1.0_f32 }, if is_sel { TEXT } else { BG }),
            );
            painter.text(
                pos,
                Align2::CENTER_CENTER,
                format!("{}", i + 1),
                FontId::proportional(10.5),
                BG,
            );
        }
        if let Some((i, node)) = hovered_info {
            let b = self.cfg.eq.bands[i];
            node.on_hover_ui(|ui| {
                ui.label(
                    RichText::new(format!("Band {} · {}", i + 1, b.kind.label())).strong(),
                );
                ui.label(format!(
                    "{} Hz   {} dB   Q {:.2}",
                    freq_label(b.freq),
                    db_label(b.gain_db),
                    b.q
                ));
            });
        }
    }

    // ----------------------------------------------------------- band editor

    fn eq_band_editor(&mut self, ui: &mut Ui, edit: &mut Edit) {
        let i = self.eq_selected.min(NUM_BANDS - 1);
        let color = band_color(i);
        card().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 6.0, color);
                ui.label(RichText::new(format!("Band {}", i + 1)).strong());
                ui.add_space(6.0);

                let band: &mut Band = &mut self.cfg.eq.bands[i];
                if ui.checkbox(&mut band.enabled, "Active").changed() {
                    edit.changed = true;
                }

                egui::ComboBox::from_id_salt("eq_kind")
                    .selected_text(band.kind.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        crate::ui::green_selection(ui);
                        for kind in FilterKind::ALL {
                            if ui.selectable_value(&mut band.kind, kind, kind.label()).changed() {
                                edit.changed = true;
                                if matches!(kind, FilterKind::LowCut | FilterKind::HighCut) {
                                    band.q = 0.71;
                                }
                            }
                        }
                    });

                ui.add_space(6.0);
                ui.label(RichText::new("Frequency").color(TEXT_DIM));
                let speed = (band.freq * 0.01).max(0.2);
                if ui
                    .add(
                        egui::DragValue::new(&mut band.freq)
                            .range(MIN_FREQ..=MAX_FREQ)
                            .speed(speed)
                            .max_decimals(0)
                            .suffix(" Hz"),
                    )
                    .changed()
                {
                    edit.changed = true;
                }

                ui.label(RichText::new("Gain").color(TEXT_DIM));
                let has_gain = band.kind.has_gain();
                if ui
                    .add_enabled(
                        has_gain,
                        egui::DragValue::new(&mut band.gain_db)
                            .range(-MAX_GAIN_DB..=MAX_GAIN_DB)
                            .speed(0.05)
                            .fixed_decimals(1)
                            .suffix(" dB"),
                    )
                    .changed()
                {
                    edit.changed = true;
                }

                ui.label(RichText::new("Q factor").color(TEXT_DIM));
                if ui
                    .add(
                        egui::DragValue::new(&mut band.q)
                            .range(MIN_Q..=MAX_Q)
                            .speed(0.01)
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    edit.changed = true;
                }
            });
        });
    }

    // ----------------------------------------------------------------- faders

    fn eq_faders(&mut self, ui: &mut Ui, edit: &mut Edit) {
        card().show(ui, |ui| {
            let enabled = self.cfg.eq.enabled;
            let columns = NUM_BANDS + 2;
            let col_w = ((ui.available_width() / columns as f32).floor()).max(44.0);
            let total_h = FADER_HEIGHT + 48.0;

            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;

                // preamp
                ui.allocate_ui_with_layout(vec2(col_w, total_h), Layout::top_down(Align::Center), |ui| {
                    ui.label(RichText::new(db_label(self.cfg.eq.preamp_db)).monospace().size(12.0));
                    let resp = fader(
                        ui,
                        &mut self.cfg.eq.preamp_db,
                        MAX_PREAMP_DB,
                        col_w,
                        TEXT,
                        false,
                        enabled,
                    );
                    if resp.changed() {
                        edit.changed = true;
                    }
                    ui.label(RichText::new("Preamp").size(11.5).color(TEXT_DIM));
                    let auto = self.cfg.eq.auto_gain_db(self.eq_shared.sample_rate().max(8000.0));
                    if self.cfg.eq.auto_headroom && enabled {
                        ui.label(RichText::new(format!("auto {:.1}", auto)).size(10.5).color(TEXT_DIM));
                    }
                });

                for i in 0..NUM_BANDS {
                    let band = self.cfg.eq.bands[i];
                    ui.allocate_ui_with_layout(vec2(col_w, total_h), Layout::top_down(Align::Center), |ui| {
                        let gain_text = if band.kind.has_gain() { db_label(band.gain_db) } else { "—".to_string() };
                        ui.label(RichText::new(gain_text).monospace().size(12.0));
                        let mut gain = band.gain_db;
                        let resp = fader(
                            ui,
                            &mut gain,
                            MAX_GAIN_DB,
                            col_w,
                            band_color(i),
                            i == self.eq_selected,
                            enabled && band.enabled && band.kind.has_gain(),
                        );
                        if resp.clicked() || resp.drag_started() {
                            self.eq_selected = i;
                        }
                        if resp.changed() {
                            self.cfg.eq.bands[i].gain_db = gain;
                            self.eq_selected = i;
                            edit.changed = true;
                        }
                        let label_color = if i == self.eq_selected { TEXT } else { TEXT_DIM };
                        ui.label(RichText::new(freq_label(band.freq)).size(11.5).color(label_color));
                        ui.label(RichText::new(format!("{}", i + 1)).size(10.0).color(band_color(i)));
                    });
                }

                // output meter
                ui.allocate_ui_with_layout(vec2(col_w, total_h), Layout::top_down(Align::Center), |ui| {
                    let peak = if self.playing { self.eq_shared.peak() } else { 0.0 };
                    let peak_db = 20.0 * peak.max(1e-5).log10();
                    let clip = peak_db > -0.2;
                    ui.label(
                        RichText::new(if peak > 1e-4 { format!("{peak_db:.1}") } else { "-∞".to_string() })
                            .monospace()
                            .size(12.0)
                            .color(if clip { DANGER } else { TEXT }),
                    );
                    meter(ui, peak_db, FADER_HEIGHT);
                    ui.label(RichText::new("Output").size(11.5).color(TEXT_DIM));
                    let gr = if self.playing && enabled { self.eq_shared.reduction_db() } else { 0.0 };
                    ui.label(
                        RichText::new(format!("Limiter {gr:.1} dB"))
                            .size(10.5)
                            .color(if gr > 0.1 { WARN } else { TEXT_DIM }),
                    );
                });
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut self.cfg.eq.auto_headroom, "Auto headroom (prevents clipping when boosting)")
                    .changed()
                {
                    edit.changed = true;
                    edit.keep_preset = true;
                }
            });
        });
    }
}

/// A vertical gain fader with centre detent, tick marks and a filled throw.
fn fader(
    ui: &mut Ui,
    value: &mut f32,
    range: f32,
    width: f32,
    color: Color32,
    selected: bool,
    active: bool,
) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, FADER_HEIGHT), Sense::click_and_drag());
    pointer_on_hover(&resp);
    let top = rect.top() + 10.0;
    let bottom = rect.bottom() - 10.0;
    let y_of = |v: f32| bottom - (v + range) / (2.0 * range) * (bottom - top);

    let mut new = *value;
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let v = (bottom - p.y) / (bottom - top) * 2.0 * range - range;
            new = (v * 10.0).round() / 10.0;
            if new.abs() < 0.4 {
                new = 0.0;
            }
        }
    }
    if resp.double_clicked() {
        new = 0.0;
    }
    if resp.hovered() {
        let scroll = ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta.y));
        if scroll != 0.0 {
            new += scroll * 0.02;
        }
    }
    new = ((new * 10.0).round() / 10.0).clamp(-range, range);
    if (new - *value).abs() > f32::EPSILON {
        *value = new;
        resp.mark_changed();
    }

    let cx = rect.center().x;
    let p = ui.painter();
    let tone = if active { color } else { Color32::from_gray(90) };
    if selected {
        p.rect_filled(rect.shrink2(vec2(2.0, 0.0)), CornerRadius::same(6), with_alpha(color, 16));
    }
    // ticks
    let step = if range > 12.5 { 3.0 } else { 3.0 };
    let mut db = -range;
    while db <= range + 0.01 {
        let y = y_of(db);
        let major = db.abs() < 0.01;
        let len = if major { 9.0 } else { 4.0 };
        p.line_segment(
            [pos2(cx - 8.0 - len, y), pos2(cx - 8.0, y)],
            Stroke::new(1.0_f32, if major { Color32::from_gray(120) } else { LINE }),
        );
        db += step;
    }
    // groove and throw
    p.rect_filled(
        Rect::from_min_max(pos2(cx - 2.0, top), pos2(cx + 2.0, bottom)),
        2.0,
        LINE,
    );
    let (y0, y1) = (y_of(0.0), y_of(*value));
    p.rect_filled(
        Rect::from_min_max(pos2(cx - 2.0, y0.min(y1)), pos2(cx + 2.0, y0.max(y1))),
        2.0,
        tone,
    );
    // knob
    let knob = Rect::from_center_size(pos2(cx, y1), vec2(26.0, 13.0));
    let hot = resp.hovered() || resp.dragged();
    p.rect_filled(
        knob,
        CornerRadius::same(4),
        if hot { Color32::from_gray(238) } else { Color32::from_gray(205) },
    );
    p.line_segment(
        [pos2(knob.left() + 4.0, y1), pos2(knob.right() - 4.0, y1)],
        Stroke::new(2.0_f32, tone),
    );
    resp
}

/// Vertical level meter, -60 … 0 dBFS with green/yellow/red zones.
fn meter(ui: &mut Ui, peak_db: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(16.0, height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), CARD_HOVER);
    let segments = 48;
    let gap = 1.5;
    let seg_h = (rect.height() - gap * (segments as f32 - 1.0)) / segments as f32;
    let level = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
    for s in 0..segments {
        let frac = (s as f32 + 1.0) / segments as f32;
        let y1 = rect.bottom() - s as f32 * (seg_h + gap);
        let seg = Rect::from_min_max(pos2(rect.left() + 2.0, y1 - seg_h), pos2(rect.right() - 2.0, y1));
        let db = frac * 60.0 - 60.0;
        let color = if db > -3.0 {
            DANGER
        } else if db > -12.0 {
            WARN
        } else {
            OK
        };
        let lit = frac <= level;
        p.rect_filled(seg, 1.0, if lit { color } else { with_alpha(color, 22) });
    }
}
