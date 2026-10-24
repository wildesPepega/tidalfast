//! The visualizer: a MilkDrop-style, music-reactive picture in a window of
//! its own, written in Rust on OpenGL.
//!
//! The audio engine hands the last fraction of a second of sound to a tap
//! (see `eq.rs`) while the window is open. [`analysis`] turns it into band
//! loudness, a waveform, a spectrum and beats; [`preset`] describes how the
//! picture moves and reacts; [`draw`] makes the waveform and shapes; [`gl`]
//! warps the previous frame, draws them on top and shows the result.

pub mod analysis;
pub mod draw;
pub mod gl;
pub mod preset;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Key, ViewportBuilder, ViewportCommand, ViewportId};
use eframe::egui_glow;

use crate::eq::EqShared;
use analysis::{Analyzer, Features, FRAMES};
use draw::Vertex;
use preset::{Look, Preset};

/// How the picture reacts, as set in the settings.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Seconds a preset plays before the next fades in.
    pub seconds: u32,
    /// 0 = 540p, 1 = 720p, 2 = 1080p (the render height; the window can be bigger).
    pub quality: u8,
    /// Switch presets with a hard cut on a beat instead of a fade.
    pub beat_cuts: bool,
    /// How readily beats fire (1 = normal).
    pub sensitivity: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self { seconds: 15, quality: 1, beat_cuts: false, sensitivity: 1.0 }
    }
}

impl Options {
    fn max_height(&self) -> u32 {
        match self.quality {
            0 => 540,
            1 => 720,
            _ => 1080,
        }
    }
}

/// What the visualizer asks the player to do (its window has the keys too).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    TogglePlay,
    NextTrack,
    PreviousTrack,
}

/// How long a preset takes to melt into the next.
const MORPH_SECONDS: f32 = 2.5;
/// How far behind the newest sample the picture looks: the sound the engine
/// has handed on is a little ahead of what the speakers play.
const LAG_SECONDS: f32 = 0.09;

const VIEWPORT: &str = "tidalfast-visualizer";

struct State {
    analyzer: Analyzer,
    buf: Vec<f32>,
    presets: Vec<Preset>,
    current: Preset,
    from: Option<Preset>,
    morph: f32,
    past: Vec<Preset>,
    seed: u64,
    since_switch: f32,
    locked: bool,
    time: f32,
    last: Option<Instant>,
    clear: bool,
    vertices: Vec<Vertex>,
    show_name: bool,
    name_since: f32,
    show_fps: bool,
    fps: f32,
    fps_frames: u32,
    fps_since: Instant,
    /// 0 = a notice when the song changes, 1 = always, 2 = never.
    song_mode: u8,
    song_since: f32,
    help: bool,
    last_pointer: Instant,
    /// Debug builds: where to save screenshots, and when the next is due.
    shot_dir: Option<String>,
    shots_taken: u32,
}

impl State {
    fn new() -> Self {
        let presets = preset::builtin();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        let mut rng = preset::Rng(seed | 1);
        let current = presets[rng.below(presets.len())].clone();
        Self {
            analyzer: Analyzer::default(),
            buf: Vec::new(),
            presets,
            current,
            from: None,
            morph: 1.0,
            past: Vec::new(),
            seed,
            since_switch: 0.0,
            locked: false,
            time: 0.0,
            last: None,
            clear: false,
            vertices: Vec::new(),
            show_name: true,
            name_since: 0.0,
            show_fps: false,
            fps: 0.0,
            fps_frames: 0,
            fps_since: Instant::now(),
            song_mode: 0,
            song_since: 0.0,
            help: false,
            last_pointer: Instant::now(),
            shot_dir: if cfg!(debug_assertions) { std::env::var("TIDALFAST_VIZ_SHOT").ok() } else { None },
            shots_taken: 0,
        }
        .pinned()
    }

    /// Debug builds: `TIDALFAST_VIZ_PRESET=<n>` shows built-in preset n and keeps it.
    fn pinned(mut self) -> Self {
        if cfg!(debug_assertions) {
            if let Some(n) = std::env::var("TIDALFAST_VIZ_PRESET").ok().and_then(|v| v.parse::<usize>().ok()) {
                self.current = self.presets[n % self.presets.len()].clone();
                self.locked = true;
            }
        }
        self
    }

    /// A different preset than the current one.
    fn choose(&mut self) -> Preset {
        for _ in 0..8 {
            self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut rng = preset::Rng(self.seed | 1);
            rng.next();
            let candidate = if rng.unit() < 0.45 {
                self.presets[rng.below(self.presets.len())].clone()
            } else {
                preset::random(self.seed >> 8)
            };
            if candidate.name != self.current.name {
                return candidate;
            }
        }
        preset::random(self.seed)
    }

    fn switch_to(&mut self, next: Preset, hard: bool) {
        // from what is on screen right now
        let shown = match &self.from {
            Some(from) if self.morph < 1.0 => Preset::mix(from, &self.current, self.morph),
            _ => self.current.clone(),
        };
        if self.past.len() >= 20 {
            self.past.remove(0);
        }
        self.past.push(self.current.clone());
        if hard {
            self.from = None;
            self.morph = 1.0;
            self.clear = true;
        } else {
            self.from = Some(shown);
            self.morph = 0.0;
        }
        self.current = next;
        self.since_switch = 0.0;
        self.name_since = self.time;
    }

    fn next(&mut self, hard: bool) {
        let next = self.choose();
        self.switch_to(next, hard);
    }

    fn previous(&mut self) {
        if let Some(previous) = self.past.pop() {
            let keep = self.past.clone();
            self.switch_to(previous, false);
            self.past = keep;
        }
    }

    /// One frame: hear, maybe switch presets, and build what to draw.
    fn tick(&mut self, eq: &EqShared, options: &Options, aspect: f32, demo: bool) -> (Look, Option<Features>) {
        let now = Instant::now();
        let dt = self.last.map_or(1.0 / 60.0, |l| now.duration_since(l).as_secs_f32()).clamp(0.001, 0.1);
        self.last = Some(now);
        self.time += dt;
        self.since_switch += dt;
        self.fps_frames += 1;
        if self.fps_since.elapsed() >= Duration::from_millis(500) {
            self.fps = self.fps_frames as f32 / self.fps_since.elapsed().as_secs_f32();
            self.fps_frames = 0;
            self.fps_since = now;
        }

        let rate = eq.sample_rate();
        let lag = (LAG_SECONDS * rate) as usize;
        let heard = !demo && eq.read_tap(FRAMES, lag, &mut self.buf);
        if demo {
            demo_audio(self.time, rate, &mut self.buf);
        } else if !heard {
            self.buf.clear();
            self.buf.resize(FRAMES * 2, 0.0);
        }
        let mut f = self.analyzer.update(&self.buf, rate, dt, options.sensitivity).clone();
        if f.level < 0.01 {
            // nothing playing: a slow idle motion instead of a dead screen
            for i in 0..analysis::WAVE {
                let x = i as f32 / analysis::WAVE as f32;
                f.wave_l[i] = ((x * 9.0 + self.time * 1.3).sin() * 0.5 + (x * 23.0 - self.time).sin() * 0.2) * 0.35;
                f.wave_r[i] = ((x * 7.0 - self.time * 1.1).cos() * 0.5 + (x * 17.0 + self.time).sin() * 0.2) * 0.35;
            }
            for (i, bar) in f.bars.iter_mut().enumerate() {
                *bar = 0.15 + 0.1 * ((i as f32 * 0.4 + self.time * 1.5).sin() * 0.5 + 0.5);
            }
        }

        // time for the next preset?
        if !self.locked && self.morph >= 1.0 {
            let due = self.since_switch >= options.seconds as f32;
            if due && options.beat_cuts {
                if f.hit || self.since_switch >= options.seconds as f32 + 4.0 {
                    self.next(f.hit);
                }
            } else if due {
                self.next(false);
            }
        }
        if self.morph < 1.0 {
            self.morph = (self.morph + dt / MORPH_SECONDS).min(1.0);
        }
        let preset = match &self.from {
            Some(from) if self.morph < 1.0 => {
                // smoothstep, so the change starts and ends gently
                let x = self.morph * self.morph * (3.0 - 2.0 * self.morph);
                Preset::mix(from, &self.current, x)
            }
            _ => self.current.clone(),
        };
        let look = preset.eval(&f, self.time, dt);
        draw::build(&look, &f, aspect, &mut self.vertices);
        (look, Some(f))
    }
}

/// Synthetic music for screenshots and tests: a kick on every beat of 120
/// BPM, a bass line, hi-hats and a pad.
#[cfg(debug_assertions)]
fn demo_audio(time: f32, rate: f32, out: &mut Vec<f32>) {
    out.clear();
    let start = time - FRAMES as f32 / rate;
    for i in 0..FRAMES {
        let t = start + i as f32 / rate;
        let beat = t % 0.5;
        let kick = (std::f32::consts::TAU * (50.0 + 90.0 * (-beat * 30.0).exp()) * beat).sin() * (-beat * 9.0).exp() * 0.9;
        let hat_phase = t % 0.25;
        let hat = ((t * 9137.0).sin() * (t * 7919.0).cos()) * (-hat_phase * 60.0).exp() * 0.25;
        let bass = (std::f32::consts::TAU * 55.0 * t).sin() * 0.15;
        let pad = ((std::f32::consts::TAU * 440.0 * t).sin() + (std::f32::consts::TAU * 660.0 * t).sin()) * 0.06;
        let (l, r) = (kick + hat + bass + pad, kick + hat * 0.6 + bass + pad * 1.2);
        out.push(l);
        out.push(r);
    }
}

#[cfg(not(debug_assertions))]
fn demo_audio(_time: f32, _rate: f32, out: &mut Vec<f32>) {
    out.clear();
    out.resize(FRAMES * 2, 0.0);
}

enum GpuState {
    Idle,
    Ready(gl::Gpu),
    Failed(String),
}

struct Shared {
    open: AtomicBool,
    eq: Arc<EqShared>,
    state: Mutex<State>,
    gpu: Mutex<GpuState>,
    options: Mutex<Options>,
    /// "Title — Artist" of the playing song and a counter that changes with it.
    song: Mutex<(String, u64)>,
    actions: Mutex<Vec<Action>>,
    /// Release the GL objects with the next paint (the window is closing).
    free: AtomicBool,
    demo: bool,
}

/// The app's handle on the visualizer window.
pub struct Viz {
    shared: Arc<Shared>,
    last_song_id: u64,
}

impl Viz {
    pub fn new(eq: Arc<EqShared>) -> Self {
        let demo = cfg!(debug_assertions) && std::env::var_os("TIDALFAST_VIZ_DEMO").is_some();
        Self {
            shared: Arc::new(Shared {
                open: AtomicBool::new(false),
                eq,
                state: Mutex::new(State::new()),
                gpu: Mutex::new(GpuState::Idle),
                options: Mutex::new(Options::default()),
                song: Mutex::new((String::new(), 0)),
                actions: Mutex::new(Vec::new()),
                free: AtomicBool::new(false),
                demo,
            }),
            last_song_id: 0,
        }
    }

    /// Debug builds: true once, so a demo run opens the window by itself.
    pub fn demo_unopened(&self) -> bool {
        self.shared.state.lock().map_or(false, |s| s.shots_taken == 0 && s.time == 0.0)
    }

    pub fn is_open(&self) -> bool {
        self.shared.open.load(Ordering::Relaxed)
    }

    pub fn open(&self) {
        self.shared.free.store(false, Ordering::Relaxed);
        self.shared.eq.set_tap(true);
        self.shared.open.store(true, Ordering::Relaxed);
    }

    pub fn close(&self) {
        self.shared.open.store(false, Ordering::Relaxed);
        self.shared.eq.set_tap(false);
    }

    pub fn toggle(&self) {
        if self.is_open() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn set_options(&self, options: Options) {
        if let Ok(mut o) = self.shared.options.lock() {
            *o = options;
        }
    }

    /// The playing song, for the notice in the picture. `id` changes with the track.
    pub fn set_song(&mut self, text: String, id: u64) {
        if id != self.last_song_id {
            self.last_song_id = id;
            if let Ok(mut song) = self.shared.song.lock() {
                *song = (text, id);
            }
            if let Ok(mut state) = self.shared.state.lock() {
                state.song_since = state.time;
            }
        }
    }

    pub fn take_actions(&self) -> Vec<Action> {
        self.shared.actions.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default()
    }

    /// Keep the window alive; call this every frame while it should be open.
    pub fn show(&self, ctx: &egui::Context) {
        if !self.is_open() {
            return;
        }
        let shared = self.shared.clone();
        let builder = ViewportBuilder::default()
            .with_title("tidalfast visualizer")
            .with_app_id("tidalfast")
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([320.0, 240.0]);
        ctx.show_viewport_deferred(ViewportId::from_hash_of(VIEWPORT), builder, move |ctx, _class| {
            window(ctx, &shared);
        });
    }
}

fn window(ctx: &egui::Context, sh: &Arc<Shared>) {
    // closed with the window's own button
    if ctx.input(|i| i.viewport().close_requested()) {
        sh.open.store(false, Ordering::Relaxed);
        sh.eq.set_tap(false);
        sh.free.store(true, Ordering::Relaxed);
    }
    let options = sh.options.lock().map(|o| *o).unwrap_or_default();
    let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    let mut keys = Keys::default();
    ctx.input(|i| {
        let pressed = |k: Key| i.key_pressed(k);
        keys.next = pressed(Key::N) || (pressed(Key::ArrowRight) && !i.modifiers.command);
        keys.previous = pressed(Key::P) || (pressed(Key::ArrowLeft) && !i.modifiers.command);
        keys.hard = pressed(Key::H);
        keys.lock = pressed(Key::L);
        keys.name = pressed(Key::T);
        keys.fps = pressed(Key::D);
        keys.song = pressed(Key::I);
        keys.help = pressed(Key::F1) || i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?"));
        keys.fullscreen = pressed(Key::F) || pressed(Key::F11);
        keys.escape = pressed(Key::Escape);
        keys.space = pressed(Key::Space);
        keys.track_next = pressed(Key::ArrowRight) && i.modifiers.command;
        keys.track_prev = pressed(Key::ArrowLeft) && i.modifiers.command;
        keys.moved = i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_pressed();
    });
    if keys.fullscreen {
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
    }
    if keys.escape {
        if fullscreen {
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
        } else {
            sh.open.store(false, Ordering::Relaxed);
            sh.eq.set_tap(false);
            sh.free.store(true, Ordering::Relaxed);
        }
    }
    if let Ok(mut actions) = sh.actions.lock() {
        if keys.space {
            actions.push(Action::TogglePlay);
        }
        if keys.track_next {
            actions.push(Action::NextTrack);
        }
        if keys.track_prev {
            actions.push(Action::PreviousTrack);
        }
    }

    let screen = ctx.screen_rect();
    let aspect = (screen.width() / screen.height().max(1.0)).clamp(0.2, 8.0);
    let (look, features) = {
        let Ok(mut state) = sh.state.lock() else { return };
        if keys.next {
            state.next(false);
        }
        if keys.hard {
            state.next(true);
        }
        if keys.previous {
            state.previous();
        }
        if keys.lock {
            state.locked = !state.locked;
            state.name_since = state.time;
        }
        if keys.name {
            state.show_name = !state.show_name;
        }
        if keys.fps {
            state.show_fps = !state.show_fps;
        }
        if keys.song {
            state.song_mode = (state.song_mode + 1) % 3;
            state.song_since = state.time;
        }
        if keys.help {
            state.help = !state.help;
        }
        if keys.moved {
            state.last_pointer = Instant::now();
        }
        state.tick(&sh.eq, &options, aspect, sh.demo)
    };
    let _ = features;

    let vertices = sh.state.lock().map(|s| s.vertices.clone()).unwrap_or_default();
    let clear = sh.state.lock().map(|mut s| std::mem::take(&mut s.clear)).unwrap_or(false);
    let time = sh.state.lock().map(|s| s.time).unwrap_or(0.0);
    let params = gl::FrameParams { look, time, aspect, max_height: options.max_height(), clear };

    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::BLACK)).show(ctx, |ui| {
        let rect = ui.max_rect();
        let response = ui.allocate_rect(rect, egui::Sense::click());
        if response.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
        }
        // the cursor goes away in fullscreen after a moment
        let idle = sh.state.lock().map(|s| s.last_pointer.elapsed() > Duration::from_secs(2)).unwrap_or(false);
        if fullscreen && idle {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }

        let paint = sh.clone();
        let release = sh.free.load(Ordering::Relaxed);
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                let mut gpu = paint.gpu.lock().unwrap_or_else(|e| e.into_inner());
                if release {
                    if let GpuState::Ready(g) = std::mem::replace(&mut *gpu, GpuState::Idle) {
                        g.destroy();
                    }
                    return;
                }
                if matches!(*gpu, GpuState::Idle) {
                    *gpu = match gl::Gpu::new(painter.gl().clone()) {
                        Ok(g) => GpuState::Ready(g),
                        Err(e) => GpuState::Failed(e),
                    };
                }
                if let GpuState::Ready(g) = &mut *gpu {
                    let v = info.viewport_in_pixels();
                    g.render(&params, &vertices, [v.left_px, v.from_bottom_px, v.width_px, v.height_px], painter.intermediate_fbo());
                }
            })),
        });

        overlay(ui, sh, rect);
    });
    #[cfg(debug_assertions)]
    debug_capture(ctx, sh);
    ctx.request_repaint();
}

/// Debug builds: save a screenshot of the window every few seconds into the
/// folder in `TIDALFAST_VIZ_SHOT`, then quit the app after the third.
#[cfg(debug_assertions)]
fn debug_capture(ctx: &egui::Context, sh: &Arc<Shared>) {
    let Ok(mut state) = sh.state.lock() else { return };
    let Some(dir) = state.shot_dir.clone() else { return };
    let due = 4.0 + state.shots_taken as f32 * 2.0;
    if state.time > due && state.time < due + 1.0 && ctx.input(|i| i.time) > 0.0 {
        let want = state.shots_taken;
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        match image {
            Some(image) => {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = buf.save(format!("{dir}/viz_{want}.png"));
                }
                state.shots_taken += 1;
                if state.shots_taken >= 3 {
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
                }
            }
            None => ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default())),
        }
    }
}

#[derive(Default)]
struct Keys {
    next: bool,
    previous: bool,
    hard: bool,
    lock: bool,
    name: bool,
    fps: bool,
    song: bool,
    help: bool,
    fullscreen: bool,
    escape: bool,
    space: bool,
    track_next: bool,
    track_prev: bool,
    moved: bool,
}

/// Fading text over the picture: the preset, the song, the frame rate, the keys.
fn overlay(ui: &mut egui::Ui, sh: &Arc<Shared>, rect: egui::Rect) {
    let (name, locked, name_age, show_name, fps, show_fps, song_mode, song_age, help, time) = {
        let Ok(s) = sh.state.lock() else { return };
        (
            s.current.name.clone(),
            s.locked,
            s.time - s.name_since,
            s.show_name,
            s.fps,
            s.show_fps,
            s.song_mode,
            s.time - s.song_since,
            s.help,
            s.time,
        )
    };
    let painter = ui.painter();
    let text = |pos: egui::Pos2, align: egui::Align2, text: String, size: f32, alpha: f32| {
        if alpha <= 0.01 {
            return;
        }
        let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
        // a soft shadow keeps it readable on any picture
        painter.text(pos + egui::vec2(1.5, 1.5), align, &text, egui::FontId::proportional(size), Color32::from_black_alpha(a));
        painter.text(pos, align, text, egui::FontId::proportional(size), Color32::from_white_alpha(a));
    };
    let fade = |age: f32, hold: f32| (1.0 - (age - hold) / 1.5).clamp(0.0, 1.0);

    if let GpuState::Failed(message) = &*sh.gpu.lock().unwrap_or_else(|e| e.into_inner()) {
        text(rect.center(), egui::Align2::CENTER_CENTER, format!("The visualizer can't run here: {message}"), 16.0, 1.0);
        return;
    }
    let preset_alpha = if show_name || locked { if locked { 1.0 } else { fade(name_age, 4.0) } } else { 0.0 };
    let label = if locked { format!("{name}  (locked)") } else { name };
    text(rect.left_top() + egui::vec2(16.0, 14.0), egui::Align2::LEFT_TOP, label, 15.0, preset_alpha);
    if show_fps {
        text(rect.right_top() + egui::vec2(-16.0, 14.0), egui::Align2::RIGHT_TOP, format!("{fps:.0} fps"), 14.0, 1.0);
    }
    let song = sh.song.lock().map(|s| s.0.clone()).unwrap_or_default();
    if !song.is_empty() {
        let alpha = match song_mode {
            0 => fade(song_age, 5.0),
            1 => 1.0,
            _ => 0.0,
        };
        text(rect.left_bottom() + egui::vec2(16.0, -16.0), egui::Align2::LEFT_BOTTOM, song, 20.0, alpha);
    }
    if help {
        let lines = [
            "N / →  next preset",
            "P / ←  previous preset",
            "H  cut to the next preset on the beat",
            "L  keep this preset",
            "T  show or hide the preset name",
            "I  song display: notice / always / off",
            "D  frame rate",
            "F or double-click  fullscreen",
            "Esc  leave fullscreen, then close",
            "Space  play / pause    Ctrl+← →  previous / next track",
        ];
        let top = rect.center() - egui::vec2(0.0, lines.len() as f32 * 12.0);
        painter.rect_filled(
            egui::Rect::from_center_size(rect.center(), egui::vec2(430.0, lines.len() as f32 * 26.0 + 30.0)),
            12.0,
            Color32::from_black_alpha(170),
        );
        for (i, line) in lines.iter().enumerate() {
            text(egui::pos2(rect.center().x - 195.0, top.y + i as f32 * 24.0), egui::Align2::LEFT_CENTER, line.to_string(), 15.0, 1.0);
        }
    }
    let _ = time;
}
