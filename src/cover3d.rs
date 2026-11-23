//! The cover of the full-screen player: a plate in space that tilts towards
//! the mouse, with a glare that follows it, and whose picture is flooded by
//! the next song's cover like by a liquid when the song changes.
//!
//! It is drawn by a small OpenGL program (egui's paint callback): the quad is
//! rotated and projected in the vertex shader, so the picture stays
//! perspective-correct; the liquid and the light are done in the fragment
//! shader. Without a usable OpenGL the plain flat cover is shown instead.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, TextureId};
use eframe::egui_glow;
use eframe::glow::{self, HasContext};

/// The shader's number of the explosion, and the least time it takes (seconds).
const EXPLODE: i32 = 9;
const EXPLODE_SECS: f32 = 3.6;

/// How far the plate tilts at the edge of the cover (radians).
const MAX_TILT: f32 = 0.30;
/// How far beyond the cover the mouse still pulls (in cover widths): just a hair, so the plate
/// only moves when the mouse is really on it.
const REACH: f32 = 0.05;
/// Space around the cover that the tilted plate may use (in cover widths).
const MARGIN: f32 = 0.22;

// ------------------------------------------------------------------ the effects

/// How the next cover takes the place of the old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CoverEffect {
    /// A liquid spreads over the old cover from one spot.
    #[default]
    Liquid,
    /// A ring wave runs outwards; inside it the new cover, with water ripples.
    Ripple,
    /// The old cover burns away along a noisy edge with glowing embers.
    Dissolve,
    /// Vertical blinds that open one after the other.
    Blinds,
    /// The plate turns around and shows the new cover on its back.
    Flip,
    /// The new cover pushes the old one out sideways.
    Push,
    /// Both turn into big blocks that shrink again.
    Pixelate,
    /// Slices of the picture jump and split into colours.
    Glitch,
    /// A clock hand wipes the new cover in.
    Iris,
    /// The cover is blown up, falls apart into shards and puts itself together as the new one.
    Explode,
    /// A glowing wave with ripples sweeps diagonally across (after Radiant Lyrics).
    Wave,
    /// Ink drops into the old cover and spreads in dark tendrils, the new cover coming up behind.
    Ink,
    /// A different one every song.
    Random,
}

impl CoverEffect {
    pub const ALL: [CoverEffect; 13] = [
        CoverEffect::Liquid,
        CoverEffect::Ripple,
        CoverEffect::Dissolve,
        CoverEffect::Blinds,
        CoverEffect::Flip,
        CoverEffect::Push,
        CoverEffect::Pixelate,
        CoverEffect::Glitch,
        CoverEffect::Iris,
        CoverEffect::Explode,
        CoverEffect::Wave,
        CoverEffect::Ink,
        CoverEffect::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CoverEffect::Liquid => "Liquid",
            CoverEffect::Ripple => "Ripple",
            CoverEffect::Dissolve => "Dissolve",
            CoverEffect::Blinds => "Blinds",
            CoverEffect::Flip => "Flip",
            CoverEffect::Push => "Push",
            CoverEffect::Pixelate => "Pixelate",
            CoverEffect::Glitch => "Glitch",
            CoverEffect::Iris => "Clock wipe",
            CoverEffect::Explode => "Explode",
            CoverEffect::Wave => "Wave",
            CoverEffect::Ink => "Ink",
            CoverEffect::Random => "Random",
        }
    }

    /// The shader's number for it; `Random` picks one that depends on the song.
    fn shader_id(self, track: u64) -> i32 {
        let real = [
            CoverEffect::Liquid,
            CoverEffect::Ripple,
            CoverEffect::Dissolve,
            CoverEffect::Blinds,
            CoverEffect::Flip,
            CoverEffect::Push,
            CoverEffect::Pixelate,
            CoverEffect::Glitch,
            CoverEffect::Iris,
            CoverEffect::Explode,
            CoverEffect::Wave,
            CoverEffect::Ink,
        ];
        match self {
            CoverEffect::Random => {
                let h = track.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33;
                (h % real.len() as u64) as i32
            }
            other => real.iter().position(|e| *e == other).unwrap_or(0) as i32,
        }
    }
}

// ------------------------------------------------------------------ the animation

/// Which cover is shown and how far the transition has got. Pure state, no drawing.
#[derive(Default)]
pub struct Anim {
    /// The song whose cover is the destination.
    track: Option<u64>,
    from: Option<TextureId>,
    to: Option<TextureId>,
    t0: f64,
    secs: f32,
    /// A new song whose cover has not arrived yet.
    pending: bool,
    /// Where the transition starts, in cover coordinates.
    pub origin: [f32; 2],
    /// The effect of the running (or last) transition, as the shader numbers it.
    pub effect: i32,
}

impl Anim {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed the current song and its cover (if loaded); returns (from, to, progress 0‥1).
    /// `secs` is how long a transition takes when this call finds a new song, `age` how
    /// long ago the song actually changed (so a late cover still starts in step with the sound).
    pub fn update(
        &mut self,
        now: f64,
        track: u64,
        tex: Option<TextureId>,
        secs: f32,
        age: f32,
        effect: CoverEffect,
    ) -> (Option<TextureId>, Option<TextureId>, f32) {
        if self.track != Some(track) {
            let first = self.track.is_none() || self.to.is_none();
            self.track = Some(track);
            if first {
                // nothing to cover yet: just show it
                self.from = tex;
                self.to = tex;
                self.pending = tex.is_none();
                self.secs = 0.0;
            } else {
                self.pending = true;
                self.secs = secs.max(0.2);
            }
            self.origin = origin_for(track);
            self.effect = effect.shader_id(track);
            if self.effect == EXPLODE && self.secs > 0.0 {
                // there is a lot going on: it needs time
                self.secs = self.secs.max(EXPLODE_SECS);
            }
        }
        if self.pending {
            if let Some(tex) = tex {
                // a transition in progress hands over to the newest cover where it stands
                if self.secs > 0.0 {
                    self.from = self.to;
                }
                self.to = Some(tex);
                self.t0 = now - (age as f64).min(self.secs as f64 * 0.9);
                self.pending = false;
            }
        } else if tex.is_some() && self.to != tex {
            // the same song's cover arrived in a better size
            let upgrade = self.to;
            self.to = tex;
            if self.from == upgrade {
                self.from = tex;
            }
        }
        let progress = if self.secs <= 0.0 || self.pending && self.to.is_none() {
            1.0
        } else {
            (((now - self.t0) as f32) / self.secs).clamp(0.0, 1.0)
        };
        (self.from, self.to, if self.pending { 0.0 } else { progress })
    }

    /// The transition is still moving.
    pub fn busy(&self, now: f64) -> bool {
        self.pending || (self.secs > 0.0 && ((now - self.t0) as f32) < self.secs)
    }
}

/// A starting point on the cover that differs from song to song (bottom half, mostly).
fn origin_for(track: u64) -> [f32; 2] {
    let mut h = track.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    let a = (h & 0xffff) as f32 / 65535.0;
    let b = ((h >> 16) & 0xffff) as f32 / 65535.0;
    [0.12 + a * 0.76, 0.55 + b * 0.4]
}

// ----------------------------------------------------------------------- the widget

enum GpuState {
    Idle,
    Ready(Gpu),
    Failed,
}

/// The tilting cover with its liquid transition.
pub struct Cover3d {
    gpu: Arc<Mutex<GpuState>>,
    anim: Anim,
    /// Smoothed mouse position over the cover, -1‥1 on both axes.
    tilt: egui::Vec2,
    /// 0‥1: how much the mouse is over the cover (drives glare and lift).
    hover: f32,
    light: egui::Vec2,
    /// Debug builds: pretend the mouse is here, as a fraction of the cover (screenshots).
    #[cfg(debug_assertions)]
    pub debug_pointer: Option<egui::Vec2>,
    /// A mirror image below the cover.
    pub reflection: bool,
    /// A rainbow foil sheen that follows the mouse.
    pub holo: bool,
    /// 0‥1: the cover swells a little with the bass.
    pub pulse: f32,
}

impl Default for Cover3d {
    fn default() -> Self {
        Self { gpu: Arc::new(Mutex::new(GpuState::Idle)), anim: Anim::default(), tilt: egui::Vec2::ZERO, hover: 0.0, light: egui::vec2(0.3, 0.25), #[cfg(debug_assertions)] debug_pointer: None, reflection: false, holo: false, pulse: 0.0 }
    }
}

impl Cover3d {
    /// The player is not on screen: the next time it opens, show the song without a flood.
    pub fn reset(&mut self) {
        self.anim.reset();
        self.tilt = egui::Vec2::ZERO;
        self.hover = 0.0;
    }

    /// Draw the cover of `track` into `rect`. `best` is its picture in the big size,
    /// `fallback` a smaller one (used if the big one takes too long); `age` is how
    /// long ago the song changed; `flat` paints the plain cover without OpenGL.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        track: u64,
        best: Option<TextureId>,
        fallback: Option<TextureId>,
        age: f32,
        flood_secs: f32,
        effect: CoverEffect,
        flat: impl FnOnce(&mut egui::Ui, egui::Rect),
    ) {
        // the sharper picture is worth a moment's wait; a changing size mid-transition would show
        let tex = best.or(if age > 0.8 { fallback } else { None });
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.05);
        let (from, to, progress) = self.anim.update(now, track, tex, flood_secs, age, effect);

        let failed = matches!(*self.gpu.lock().unwrap_or_else(|e| e.into_inner()), GpuState::Failed);
        let (Some(to), false) = (to, failed) else {
            flat(ui, rect);
            return;
        };
        let from = from.unwrap_or(to);

        // the mouse: tilt towards it, light follows it
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        #[cfg(debug_assertions)]
        let pointer = self.debug_pointer.map(|f| rect.min + rect.size() * f).or(pointer);
        let (target, over) = match pointer {
            Some(p) if rect.expand(rect.width() * REACH).contains(p) => {
                let n = (p - rect.center()) / (rect.size() * 0.5);
                let inside = rect.contains(p);
                (egui::vec2(n.x.clamp(-1.0, 1.0), n.y.clamp(-1.0, 1.0)), if inside { 1.0 } else { 0.0 })
            }
            _ => (egui::Vec2::ZERO, 0.0),
        };
        let k = 1.0 - (-dt * 9.0).exp();
        self.tilt += (target - self.tilt) * k;
        self.hover += (over - self.hover) * k;
        if over > 0.0 {
            self.light = egui::vec2(0.5 + target.x * 0.5, 0.5 + target.y * 0.5);
        }
        let moving = (target - self.tilt).length() > 0.002 || (over - self.hover).abs() > 0.002;
        if moving || self.anim.busy(now) {
            ctx.request_repaint();
        }

        // a soft shadow while the cover rests; it fades out as soon as the mouse moves the plate
        let flip = if self.anim.effect == 4 && progress < 1.0 {
            let e = progress * progress * (3.0 - 2.0 * progress);
            e * std::f32::consts::PI
        } else {
            0.0
        };
        // (no shadow either while the cover is in pieces)
        let blown = if self.anim.effect == EXPLODE && progress < 1.0 {
            let s = |a: f32, b: f32, x: f32| (((x - a) / (b - a)).clamp(0.0, 1.0)).powi(2);
            1.0 - s(0.04, 0.14, progress) * (1.0 - s(0.94, 1.0, progress))
        } else {
            1.0
        };
        // (at the very edge the mouse is just outside the cover, but the plate already tilts fully:
        // the tilt counts too, not only the hover)
        let engaged = self.hover.max(self.tilt.length().min(1.0));
        let rest = (1.0 - engaged * 1.6).clamp(0.0, 1.0) * flip.cos().abs() * blown;
        if rest > 0.01 {
            ui.painter().add(
                egui::epaint::Shadow {
                    offset: [0, 16],
                    blur: 46,
                    spread: 0,
                    color: egui::Color32::from_black_alpha((150.0 * rest) as u8),
                }
                .as_shape(rect.shrink(rect.width() * 0.02), 20.0),
            );
        }

        // light spilling onto the floor under the cover, in the colour of the theme
        if self.reflection {
            let lit = (0.55 + 0.45 * self.hover) * blown * flip.cos().abs();
            if lit > 0.02 {
                let a = crate::ui::accent();
                let glow = egui::Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), (70.0 * lit + 50.0 * self.pulse) as u8);
                crate::ui::soft_ellipse(
                    &ui.painter().with_clip_rect(ui.clip_rect()),
                    egui::pos2(rect.center().x, rect.bottom() + rect.height() * 0.07),
                    egui::vec2(rect.width() * (0.62 + 0.08 * self.pulse), rect.height() * 0.22),
                    glow,
                );
            }
        }

        // the explosion needs room around the cover (but not beyond the panel it sits in)
        let exploding = self.anim.effect == EXPLODE && progress < 1.0;
        let margin = if exploding { 1.0 } else { MARGIN };
        let below = if self.reflection && !exploding { rect.width() * 0.30 } else { 0.0 };
        let outer = egui::Rect::from_min_max(
            rect.min - egui::Vec2::splat(rect.width() * margin),
            rect.max + egui::vec2(rect.width() * margin, rect.width() * margin + below),
        )
        .intersect(ui.clip_rect());
        let ppp = ctx.pixels_per_point();
        let offset = rect.center() - outer.center();
        // the whole picture jolts: a tremble while the charge builds, a kick at the blast
        let shake = if exploding {
            let t = now as f32;
            let amp = 2.5 * (progress / 0.14).clamp(0.0, 1.0).powi(2) * (1.0 - (progress / 0.14).floor().min(1.0))
                + 16.0 * (-(progress - 0.14).max(0.0) * 16.0).exp() * (progress >= 0.14) as i32 as f32;
            [amp * (t * 93.0).sin() * ppp, amp * (t * 77.0).cos() * ppp]
        } else {
            [0.0, 0.0]
        };
        let params = Params {
            size_px: rect.width() * ppp,
            rot: [self.tilt.y * MAX_TILT, self.tilt.x * MAX_TILT],
            zoom: 1.0 + 0.025 * self.hover + 0.04 * self.pulse,
            light: [self.light.x, self.light.y],
            glare: 0.18 * self.hover,
            progress,
            origin: self.anim.origin,
            time: now as f32,
            effect: self.anim.effect,
            flip,
            center: [offset.x * ppp, -offset.y * ppp],
            shake,
            reflect: self.reflection && !exploding,
            reflecting: false,
            holo: if self.holo { 1.0 } else { 0.0 },
        };
        let state = self.gpu.clone();
        ui.painter().add(egui::PaintCallback {
            rect: outer,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                if matches!(*guard, GpuState::Idle) {
                    *guard = match Gpu::new(painter.gl().clone()) {
                        Ok(gpu) => GpuState::Ready(gpu),
                        Err(e) => {
                            log::warn!("cover 3D unavailable: {e}");
                            GpuState::Failed
                        }
                    };
                }
                if let GpuState::Ready(gpu) = &*guard {
                    let (Some(a), Some(b)) = (painter.texture(from), painter.texture(to)) else { return };
                    let v = info.viewport_in_pixels();
                    gpu.render(&params, a, b, [v.left_px, v.from_bottom_px, v.width_px, v.height_px]);
                }
            })),
        });
    }
}

// ---------------------------------------------------------------------------- OpenGL

#[derive(Clone, Copy)]
struct Params {
    size_px: f32,
    /// Pitch and yaw (radians).
    rot: [f32; 2],
    zoom: f32,
    light: [f32; 2],
    glare: f32,
    progress: f32,
    origin: [f32; 2],
    time: f32,
    effect: i32,
    /// Extra turn of the plate around its vertical axis (the flip effect).
    flip: f32,
    /// Where the cover's middle is relative to the middle of the area we draw into (pixels, y up).
    center: [f32; 2],
    /// A jolt of the whole picture, pixels.
    shake: [f32; 2],
    /// Draw a reflection below the cover.
    reflect: bool,
    /// This pass draws the reflection.
    reflecting: bool,
    /// Foil sheen strength.
    holo: f32,
}

struct Gpu {
    gl: Arc<glow::Context>,
    program: glow::Program,
    shard: glow::Program,
    fx: glow::Program,
    vao: glow::VertexArray,
    /// egui keeps its textures in sRGB: sampling gives linear values.
    srgb_textures: bool,
}

const HEADER_DESKTOP: &str = "#version 330 core\nprecision highp float;\n";
const HEADER_ES: &str = "#version 300 es\nprecision highp float;\n";

const VERTEX: &str = r#"
uniform vec2 u_view;   // size of the area we draw into, pixels
uniform float u_size;  // edge of the cover, pixels
uniform vec2 u_rot;    // pitch, yaw
uniform float u_zoom;
uniform float u_flip;
uniform vec2 u_center;
uniform vec2 u_shake;
uniform int u_reflect;
out vec2 uv;
void main() {
    vec2 c = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    uv = vec2(c.x, 1.0 - c.y);
    vec3 q = vec3((c - 0.5) * u_size * u_zoom, 0.0);
    // the reflection: the plate mirrored at its bottom edge, with a small gap
    if (u_reflect == 1) q.y = -q.y - u_size * u_zoom * 1.04;
    // the flip turns the plate over; the mouse's lean carries over to the back side
    float yaw = u_flip + u_rot.y * cos(u_flip);
    float cy = cos(yaw), sy = sin(yaw), cx = cos(u_rot.x), sx = sin(u_rot.x);
    q = vec3(q.x * cy, q.y, -q.x * sy);               // yaw: the side under the mouse leans away
    q = vec3(q.x, q.y * cx - q.z * sx, q.y * sx + q.z * cx); // pitch
    float d = u_size * 3.0;                            // distance of the camera
    gl_Position = vec4((q.xy + u_center + u_shake) / (u_view * 0.5), 0.0, (d - q.z) / d);
}
"#;

const FRAGMENT: &str = r#"
in vec2 uv;
out vec4 o;
uniform sampler2D u_from;
uniform sampler2D u_to;
uniform float u_progress;
uniform vec2 u_origin;
uniform float u_time;
uniform vec2 u_light;
uniform float u_glare;
uniform float u_size;
uniform int u_srgb;
uniform int u_effect;
uniform int u_reflect;
uniform float u_holo;

const float PI = 3.14159265;

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}
float noise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
float fbm(vec2 p) {
    float v = 0.0, a = 0.5;
    for (int i = 0; i < 3; i++) { v += a * noise(p); p = p * 2.03 + 17.0; a *= 0.5; }
    return v / 0.875;
}
vec3 gamma(vec3 c) {
    bvec3 low = lessThan(c, vec3(0.0031308));
    return mix(1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, c * 12.92, vec3(low));
}
vec3 sample_tex(sampler2D s, vec2 p) {
    vec3 c = texture(s, clamp(p, vec2(0.002), vec2(0.998))).rgb;
    return u_srgb == 1 ? gamma(c) : c;
}
vec3 from_at(vec2 p) { return sample_tex(u_from, p); }
vec3 to_at(vec2 p) { return sample_tex(u_to, p); }
float ease(float t) { return t * t * (3.0 - 2.0 * t); }

// ---- 0: liquid. > 0 where the liquid already is
float liquid_field(vec2 p, float radius) {
    float n = fbm(p * 2.1 + vec2(u_time * 0.24, -u_time * 0.17));
    float m = fbm(p * 4.6 - vec2(u_time * 0.35, u_time * 0.16));
    float dist = length(p - u_origin) + (n - 0.5) * 0.66 + (m - 0.5) * 0.07;
    return radius - dist;
}
vec3 liquid(vec2 p, float t) {
    float radius = mix(-0.5, 2.2, ease(t));
    float f = liquid_field(p, radius);
    float eps = 0.004;
    vec2 g = vec2(liquid_field(p + vec2(eps, 0.0), radius) - f, liquid_field(p + vec2(0.0, eps), radius) - f) / eps;
    vec2 n2 = g / (length(g) + 1e-4);
    float inside = smoothstep(-0.004, 0.004, f);
    vec3 cNew = to_at(p - n2 * 0.038 * exp(-max(f, 0.0) * 22.0));
    vec3 cOld = from_at(p + n2 * 0.010 * exp(min(f, 0.0) * 40.0));
    vec3 col = mix(cOld, cNew, inside);
    float rim = exp(-pow(f / 0.016, 2.0));
    float sheen = exp(-max(f, 0.0) * 9.0) * step(0.0, f);
    float lit = 0.5 + 0.5 * dot(n2, normalize(vec2(-0.55, -0.8)));
    col += vec3(1.0) * rim * (0.06 + 0.26 * pow(lit, 2.0));
    col += vec3(1.0) * sheen * 0.05 * (0.4 + lit);
    col *= 1.0 - 0.22 * exp(-pow((f + 0.022) / 0.016, 2.0));
    return col;
}

// ---- 1: ripple
vec3 ripple(vec2 p, float t) {
    float e = ease(t);
    vec2 d = p - u_origin;
    float r = length(d);
    float x = r - mix(0.0, 1.6, e);
    float wave = sin(x * 70.0 - u_time * 6.0) * exp(-pow(x / 0.16, 2.0)) * 0.014 * (1.0 - 0.6 * e);
    vec2 off = d / (r + 1e-4) * wave;
    vec3 col = mix(from_at(p + off), to_at(p + off), smoothstep(0.004, -0.004, x));
    col += vec3(1.0) * exp(-pow(x / 0.02, 2.0)) * 0.22 * (1.0 - 0.5 * e);
    col += vec3(1.0) * exp(-pow((x + 0.05) / 0.03, 2.0)) * 0.05;
    return col;
}

// ---- 2: dissolve with embers
vec3 dissolve(vec2 p, float t) {
    float n = fbm(p * 5.5 + 3.0);
    float f = mix(-0.06, 1.1, t) - n;
    vec3 col = mix(from_at(p), to_at(p), smoothstep(-0.015, 0.015, f));
    col += vec3(1.0, 0.62, 0.25) * exp(-pow(f / 0.035, 2.0)) * 1.1;
    col += vec3(1.0, 0.8, 0.5) * exp(-pow(f / 0.012, 2.0)) * 0.5;
    return col;
}

// ---- 3: blinds
vec3 blinds(vec2 p, float t) {
    float n = 9.0;
    float cell = floor(p.x * n), loc = fract(p.x * n);
    float q = ease(clamp((t - cell / n * 0.35) / 0.65, 0.0, 1.0));
    float inside = smoothstep(q + 0.02, q - 0.02, loc);
    vec3 col = mix(from_at(p), to_at(p), inside);
    col *= 1.0 - 0.30 * exp(-pow((loc - q) / 0.05, 2.0)) * step(0.001, q) * step(q, 0.999);
    return col;
}

// ---- 4: flip (the plate itself turns; this picks the side that faces us)
vec3 flip(vec2 p, float t) {
    bool back = t > 0.5;
    vec3 c = back ? to_at(vec2(1.0 - p.x, p.y)) : from_at(p);
    return c * (0.55 + 0.45 * abs(cos(ease(t) * PI)));
}

// ---- 5: push
vec3 push(vec2 p, float t) {
    float e = ease(t);
    float dir = u_origin.x < 0.5 ? 1.0 : -1.0;
    vec2 q = dir > 0.0 ? p : vec2(1.0 - p.x, p.y);
    bool fresh = q.x > 1.0 - e;
    vec3 col = fresh ? to_at(dir > 0.0 ? vec2(p.x - (1.0 - e), p.y) : vec2(p.x + (1.0 - e), p.y))
                     : from_at(dir > 0.0 ? vec2(p.x + e, p.y) : vec2(p.x - e, p.y));
    col *= 1.0 - 0.35 * exp(-pow((q.x - (1.0 - e)) / 0.05, 2.0)) * step(0.001, e) * step(e, 0.999);
    return col;
}

// ---- 6: pixelate
vec3 pixelate(vec2 p, float t) {
    float k = sin(t * PI);
    float cells = mix(240.0, 7.0, pow(k, 0.7));
    vec2 id = floor(p * cells);
    vec2 q = (id + 0.5) / cells;
    float m = smoothstep(0.42, 0.58, t + (hash(id) - 0.5) * 0.25);
    return mix(from_at(q), to_at(q), m);
}

// ---- 7: glitch
vec3 glitch(vec2 p, float t) {
    float k = sin(t * PI);
    float band = floor(p.y * 28.0);
    float r = hash(vec2(band, floor(u_time * 14.0)));
    float shift = (r - 0.5) * 0.22 * k * step(0.35, r * k * 1.6);
    vec2 u = vec2(p.x + shift, p.y);
    float m = step(0.5, t + (hash(vec2(band, 7.0)) - 0.5) * 0.5);
    float sp = 0.012 * k;
    vec3 c;
    c.r = mix(from_at(u + vec2(sp, 0.0)).r, to_at(u + vec2(sp, 0.0)).r, m);
    c.g = mix(from_at(u).g, to_at(u).g, m);
    c.b = mix(from_at(u - vec2(sp, 0.0)).b, to_at(u - vec2(sp, 0.0)).b, m);
    return c;
}

// ---- 8: clock wipe
vec3 iris(vec2 p, float t) {
    vec2 d = p - vec2(0.5);
    float a = (atan(d.x, -d.y) + PI) / (2.0 * PI);
    float e = ease(t);
    vec3 col = mix(from_at(p), to_at(p), smoothstep(e + 0.004, e - 0.004, a));
    col += vec3(1.0) * exp(-pow((a - e) / 0.012, 2.0)) * 0.35 * step(0.001, e) * step(e, 0.999);
    return col;
}

// ---- 10: wave. Ported from the cover transition of Radiant Lyrics by meowarex
// (https://github.com/meowarex/TidaLuna-Plugins, GPL-3.0): a glowing wave with ripples sweeps
// diagonally across the cover; a ring swells from the middle halfway through.
float wv_noise(vec2 q) { return noise(q); }
float wv_octaves(vec2 q) {
    float acc = 0.0, amp = 0.5;
    for (int k = 0; k < 4; k++) { acc += amp * wv_noise(q); q = q * 2.03 + 17.1; amp *= 0.5; }
    return acc;
}
vec2 mirror_uv(vec2 uv) { return 1.0 - abs(1.0 - mod(uv, 2.0)); }
vec3 wv_fringe(sampler2D s, vec2 uv, vec2 off) {
    return vec3(sample_tex(s, mirror_uv(uv + off)).r, sample_tex(s, mirror_uv(uv)).g, sample_tex(s, mirror_uv(uv - off)).b);
}
vec3 wave(vec2 uv, float t) {
    // nearly linear, so the wave does not crawl at the end and is gone exactly when the transition is
    float mixv = mix(t, ease(t), 0.08);
    float clock = u_time;
    float jitter = u_origin.x * 37.0 + u_origin.y * 91.0;
    // the colour of the new cover, punched up
    vec3 avg = vec3(0.0);
    for (int i = 0; i < 3; i++) for (int j = 0; j < 3; j++) avg += to_at(vec2(0.17 + 0.33 * float(i), 0.17 + 0.33 * float(j)));
    avg /= 9.0;
    float mean = (avg.r + avg.g + avg.b) / 3.0;
    vec3 hue = clamp(vec3(mean) + (avg - mean) * 1.6 + 0.12, 0.0, 1.0);

    vec2 fromMid = uv - 0.5;
    float radial = length(fromMid);
    float pulseAge = (t - 0.5) * 3.0;
    float pulse = pulseAge >= 0.0 ? exp(-pulseAge * 1.5) : 0.0;
    float swell = pulseAge > 0.0 ? exp(-abs(radial - pulseAge * 0.62) * 13.0) * pulse : 0.0;
    vec2 bentUv = 0.5 + fromMid + normalize(fromMid + 1e-5) * swell * 0.04;
    vec2 aberr = fromMid * (0.05 * swell);
    float busy = step(0.002, mixv) * step(mixv, 0.998);

    float w = uv.x * 0.74 + uv.y * 0.26 + sin(uv.y * 7.0 + clock * 1.7) * 0.045
        + (wv_octaves(uv * 3.0 + jitter + vec2(0.0, clock * 0.08)) - 0.47) * 0.16;
    // the wave's middle travels from just before the first corner (w about 0) to just past the
    // last one (w about 1.13, plus the glow's reach) so that it leaves exactly when the transition ends
    float gap = w - mix(-0.28, 1.34, mixv);
    float side = smoothstep(-0.01, 0.01, gap);
    float bulge = exp(-abs(gap) * 9.0) * busy;
    float wiggle = sin(gap * 70.0 - clock * 5.0) * exp(-abs(gap) * 7.0) * busy;
    vec2 shove = vec2(0.74, 0.26) * (bulge * 0.07 + wiggle * 0.018);
    vec3 shade = mix(wv_fringe(u_to, bentUv - shove, aberr + vec2(0.01 * bulge, 0.0)),
                     wv_fringe(u_from, bentUv + shove, aberr + vec2(0.02 * bulge, 0.0)), side);
    shade *= 1.0 + wiggle * 0.18;
    float shine = (exp(-abs(gap) * 45.0) * 1.4 + exp(-abs(gap) * 8.0) * 0.25) * busy;
    vec3 hueLight = mix(hue, vec3(1.0), 0.55);
    float headroom = 1.0 - 0.65 * dot(shade, vec3(0.3, 0.6, 0.1));
    shade += shine * 0.3 * hueLight * headroom;
    shade += swell * 0.1 * headroom;
    return shade;
}

// ---- 11: ink. Drops of ink spread in tendrils; the new cover comes up behind them.
vec3 ink(vec2 p, float t) {
    float e = ease(t);
    vec2 w = p + 0.11 * vec2(fbm(p * 3.0 + u_time * 0.08), fbm(p * 3.0 + 7.3 - u_time * 0.08)) - 0.055;
    vec2 second = vec2(1.0 - u_origin.x, 1.0 - u_origin.y) * 0.8 + 0.1;
    float d = min(length(w - u_origin), length(w - second) * 1.2 + 0.18);
    float tendrils = fbm(w * 7.0 + vec2(0.0, u_time * 0.15));
    d += (tendrils - 0.5) * 0.26;
    float r = mix(-0.3, 2.1, e);
    float f = r - d;
    float inside = smoothstep(-0.006, 0.006, f);
    vec3 cNew = to_at(p);
    vec3 cOld = from_at(p);
    // the colour of the ink: very dark, with a hint of the new cover
    vec3 avg = (to_at(vec2(0.25, 0.25)) + to_at(vec2(0.75, 0.25)) + to_at(vec2(0.25, 0.75)) + to_at(vec2(0.75, 0.75))) * 0.25;
    vec3 pigment = avg * 0.10;
    float cloud = (0.5 + 0.5 * tendrils) * (1.0 - smoothstep(0.82, 1.0, t));
    // ahead of the front the old cover darkens a little, behind it the ink thins out
    float ahead = exp(min(f, 0.0) * 14.0) * 0.30 * cloud;
    float behind = exp(-max(f, 0.0) * 8.5) * 0.92 * cloud;
    vec3 col = mix(cOld * (1.0 - ahead) + pigment * ahead, cNew, inside);
    return mix(col, pigment, behind * inside);
}

void main() {
    // rounded corners
    vec2 q = abs(uv - 0.5) - (0.5 - 0.06);
    float sdf = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - 0.06;
    float aa = 1.2 / u_size;
    float alpha = clamp(0.5 - sdf / aa, 0.0, 1.0);
    if (alpha <= 0.0) { o = vec4(0.0); return; }

    float t = clamp(u_progress, 0.0, 1.0);
    if (u_reflect == 1) {
        // the floor: the picture a little out of focus (more so the further it is from the plate),
        // wobbling like water, and fading into a soft tint of the cover's own colour
        float far = 1.0 - uv.y;
        vec2 q = uv;
        q.x += sin(uv.y * 34.0 - u_time * 1.7) * 0.0045 * (0.3 + far * 3.0)
             + sin(uv.y * 71.0 + u_time * 2.3) * 0.0020 * far * 3.0;
        float blur = 0.003 + far * 0.050;
        float e = t >= 1.0 ? 1.0 : ease(t);
        vec3 acc = vec3(0.0);
        float wsum = 0.0;
        for (int i = -2; i <= 2; i++) {
            for (int j = -2; j <= 2; j++) {
                vec2 d = vec2(float(i), float(j)) * 0.5 * blur;
                float wt = exp(-float(i * i + j * j) * 0.25);
                vec2 sp = clamp(q + d, vec2(0.002), vec2(0.998));
                acc += mix(from_at(sp), to_at(sp), e) * wt;
                wsum += wt;
            }
        }
        vec3 rc = acc / wsum;
        // fades out from the plate downwards, quicker at the sides, with a faint shimmer
        float fade = pow(smoothstep(0.55, 1.0, uv.y), 1.25) * 0.62;
        fade *= 0.9 + 0.1 * sin(uv.y * 60.0 - u_time * 2.0);
        fade *= smoothstep(0.0, 0.10, uv.x) * smoothstep(1.0, 0.90, uv.x);
        // a slow light streak sliding across the floor
        float streak = smoothstep(0.16, 0.0, abs(fract(uv.x * 0.8 + far * 0.5 - u_time * 0.07) - 0.5) - 0.05);
        float lit = streak * 0.30;
        rc = mix(rc, vec3(dot(rc, vec3(0.3, 0.6, 0.1))), 0.25) * 0.95 + vec3(0.02, 0.025, 0.04) * (1.0 - far);
        rc += vec3(lit) * (1.0 - far);
        o = vec4(clamp(rc, 0.0, 1.0) * alpha * fade, alpha * fade);
        return;
    }
    vec3 col;
    if (t >= 1.0) {
        col = to_at(uv);
    } else if (u_effect == 0) { col = liquid(uv, t); }
    else if (u_effect == 1) { col = ripple(uv, t); }
    else if (u_effect == 2) { col = dissolve(uv, t); }
    else if (u_effect == 3) { col = blinds(uv, t); }
    else if (u_effect == 4) { col = flip(uv, t); }
    else if (u_effect == 5) { col = push(uv, t); }
    else if (u_effect == 6) { col = pixelate(uv, t); }
    else if (u_effect == 7) { col = glitch(uv, t); }
    else if (u_effect == 8) { col = iris(uv, t); }
    else if (u_effect == 10) { col = wave(uv, t); }
    else { col = ink(uv, t); }


    // light that follows the mouse, and a faint edge falloff for depth
    float dl = distance(uv, u_light);
    col += vec3(1.0) * u_glare * pow(smoothstep(0.95, 0.0, dl), 2.2);
    if (u_holo > 0.0) {
        // a rainbow foil: bands that slide with the light, a fine sparkle, strongest near the mouse
        float hover = clamp(u_glare / 0.18, 0.0, 1.0);
        float band = dot(uv - u_light, vec2(0.8, 0.6)) * 3.0 + noise(uv * 5.0) * 0.4;
        vec3 rainbow = 0.5 + 0.5 * cos(2.0 * PI * (band + vec3(0.0, 0.33, 0.67)));
        float near = smoothstep(1.05, 0.0, dl);
        float amount = (0.03 + 0.17 * hover) * near;
        col += rainbow * amount;
        // round twinkles in a grid: few of them, each flaring up and going out again
        vec2 cell = uv * 46.0;
        float h = hash(floor(cell));
        float dotv = smoothstep(0.30, 0.0, length(fract(cell) - 0.5));
        float twinkle = pow(max(0.0, sin(u_time * 2.4 + h * 40.0)), 6.0);
        col += vec3(1.0) * step(0.965, h) * dotv * twinkle * 0.9 * hover * near;
    }
    col *= 1.0 - 0.10 * smoothstep(0.35, 0.75, distance(uv, vec2(0.5)));
    col += vec3(1.0) * 0.045 * smoothstep(0.012, 0.0, -sdf); // light catching the rim of the plate

    col = clamp(col, 0.0, 1.0);
    o = vec4(col * alpha, alpha);
}
"#;


// The explosion. The plate is cut into a jittered lattice of triangles ("shards"); each one has its
// own random push, spin and timing, so the whole thing is a function of progress `u_p` alone:
//   0.00 - 0.14   the charge builds: cracks glow, the plate trembles
//   0.14          blast: flash, shock rings, sparks, the shards fly off, spinning and shrinking
//   0.50          the new cover takes the shards' faces, unseen in the chaos
//   0.58 - 0.95   the shards come back in a swirl, one after the other, and snap into place
//   0.95 - 1.00   a last glint
const SHARD_VS: &str = r#"
uniform vec2 u_view;
uniform vec2 u_center;
uniform vec2 u_shake;
uniform float u_size;
uniform vec2 u_rot;
uniform float u_zoom;
uniform float u_p;
uniform vec2 u_origin;
uniform float u_time;
out vec2 uv;
out vec3 bary;
flat out float which;
out float alpha_s;
out float shade;
out float hot;
out float landing;
out float heat;

const int N = 16;
const float PI = 3.14159265;

float h11(float n) { return fract(sin(n * 127.1 + 31.7) * 43758.5453); }

vec2 lattice(int i, int j) {
    vec2 p = vec2(float(i), float(j)) / float(N);
    if (i > 0 && i < N && j > 0 && j < N) {
        vec2 r = vec2(h11(float(i * 131 + j * 17 + 3)), h11(float(i * 57 + j * 211 + 9))) - 0.5;
        p += r * 0.62 / float(N);
    }
    return p;
}

vec3 rot(vec3 v, vec3 a) {
    float cx = cos(a.x), sx = sin(a.x), cy = cos(a.y), sy = sin(a.y), cz = cos(a.z), sz = sin(a.z);
    v = vec3(v.x, v.y * cx - v.z * sx, v.y * sx + v.z * cx);
    v = vec3(v.x * cy + v.z * sy, v.y, -v.x * sy + v.z * cy);
    return vec3(v.x * cz - v.y * sz, v.x * sz + v.y * cz, v.z);
}

void main() {
    int tri = gl_VertexID / 3;
    int corner = gl_VertexID - tri * 3;
    int cell = tri / 2;
    int half_ = tri - cell * 2;
    int ci = cell - (cell / N) * N;
    int cj = cell / N;
    // the diagonal alternates from cell to cell
    bool flip = ((ci + cj) & 1) == 1;
    ivec2 a = ivec2(ci, cj), b = ivec2(ci + 1, cj), c = ivec2(ci + 1, cj + 1), d = ivec2(ci, cj + 1);
    ivec2 t0, t1, t2;
    if (!flip) {
        if (half_ == 0) { t0 = a; t1 = b; t2 = c; } else { t0 = a; t1 = c; t2 = d; }
    } else {
        if (half_ == 0) { t0 = a; t1 = b; t2 = d; } else { t0 = b; t1 = c; t2 = d; }
    }
    vec2 P0 = lattice(t0.x, t0.y), P1 = lattice(t1.x, t1.y), P2 = lattice(t2.x, t2.y);
    vec2 P = corner == 0 ? P0 : (corner == 1 ? P1 : P2);
    vec2 cen = (P0 + P1 + P2) / 3.0;

    float id = float(tri);
    float h1 = h11(id + 1.0), h2 = h11(id + 2.0), h3 = h11(id + 3.0), h4 = h11(id + 4.0), h5 = h11(id + 5.0), h6 = h11(id + 6.0);
    float p = u_p;

    // how far this shard is from home: out with the blast, back in a swirl
    float up = clamp((p - 0.14 - 0.03 * h5) / 0.30, 0.0, 1.0);
    float up_e = 1.0 - pow(1.0 - up, 2.4);
    float down = clamp((p - (0.58 + 0.14 * h3)) / 0.23, 0.0, 1.0);
    float down_e = down < 0.5 ? 4.0 * down * down * down : 1.0 - pow(-2.0 * down + 2.0, 3.0) / 2.0;
    float S = up_e * (1.0 - down_e);

    vec2 blast = vec2(0.5) + (u_origin - 0.5) * 0.35;
    vec2 to_c = cen - blast;
    float dist = length(to_c);
    vec2 dir = dist > 1e-4 ? to_c / dist : vec2(cos(id), sin(id));
    dir = normalize(dir + (vec2(h1, h2) - 0.5) * 0.55);
    float speed = (0.38 + 0.62 * h4) * (1.2 - 0.5 * clamp(dist, 0.0, 1.0));
    vec2 away = dir * speed + vec2(0.0, 0.2 + 0.7 * h6);   // uv y points down: they fall as they fly
    float zaway = (h2 - 0.3) * 1.2;                         // mostly towards the viewer
    vec2 perp = vec2(-dir.y, dir.x);
    float swirl = sin(PI * down_e) * (h5 - 0.5) * 1.1;
    vec2 disp = (away + perp * swirl) * S;
    vec3 spin = (vec3(h1, h2, h3) - 0.5) * vec3(14.0, 14.0, 18.0);
    vec3 ang = spin * S;
    float scale = 1.0 - 0.3 * S;

    vec3 r3 = rot(vec3((P - cen) * vec2(1.0, -1.0), 0.0) * scale, ang);
    vec3 world = vec3(cen.x - 0.5 + disp.x, -(cen.y - 0.5) - disp.y, zaway * S * 0.6) + r3;

    // the charge builds: everything trembles a little
    float wind = smoothstep(0.0, 0.14, p) * (1.0 - step(0.14, p));
    float tick = floor(u_time * 45.0);
    world.xy += (vec2(h11(id + tick), h11(id + 7.0 + tick)) - 0.5) * 0.010 * wind;

    // the same lean and perspective as the whole plate
    vec3 q = vec3(world.xy * u_size * u_zoom, world.z * u_size);
    float cy = cos(u_rot.y), sy = sin(u_rot.y), cx = cos(u_rot.x), sx = sin(u_rot.x);
    q = vec3(q.x * cy + q.z * sy, q.y, -q.x * sy + q.z * cy);
    q = vec3(q.x, q.y * cx - q.z * sx, q.y * sx + q.z * cx);
    float dcam = u_size * 3.0;
    gl_Position = vec4((q.xy + u_center + u_shake) / (u_view * 0.5), 0.0, max(dcam - q.z, dcam * 0.2) / dcam);

    uv = P;
    bary = vec3(corner == 0 ? 1.0 : 0.0, corner == 1 ? 1.0 : 0.0, corner == 2 ? 1.0 : 0.0);
    which = p > 0.52 + 0.05 * h4 ? 1.0 : 0.0;
    alpha_s = 1.0 - 0.3 * S;
    vec3 n = rot(vec3(0.0, 0.0, 1.0), ang);
    shade = 0.55 + 0.6 * abs(dot(n, normalize(vec3(-0.35, 0.55, 0.75)))) * (0.5 + 0.5 * (1.0 - S * 0.4));
    hot = exp(-pow((p - 0.18) / 0.11, 2.0)) * (0.3 + 0.7 * S);
    landing = pow(smoothstep(0.7, 1.0, down), 3.0) * smoothstep(1.0, 0.95, p);
    heat = wind;
}
"#;

const SHARD_FS: &str = r#"
in vec2 uv;
in vec3 bary;
flat in float which;
in float alpha_s;
in float shade;
in float hot;
in float landing;
in float heat;
out vec4 o;
uniform sampler2D u_from;
uniform sampler2D u_to;
uniform float u_size;
uniform int u_srgb;
uniform float u_glare;
uniform vec2 u_light;

vec3 gamma(vec3 c) {
    bvec3 low = lessThan(c, vec3(0.0031308));
    return mix(1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, c * 12.92, vec3(low));
}
vec3 sample_tex(sampler2D s, vec2 p) {
    vec3 c = texture(s, clamp(p, vec2(0.002), vec2(0.998))).rgb;
    return u_srgb == 1 ? gamma(c) : c;
}

void main() {
    // the rounded corners of the plate, as in the normal picture
    vec2 q = abs(uv - 0.5) - (0.5 - 0.06);
    float sdf = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - 0.06;
    float alpha = clamp(0.5 - sdf * u_size / 1.2, 0.0, 1.0) * alpha_s;
    if (alpha <= 0.0) { o = vec4(0.0); return; }
    vec3 col = (which > 0.5 ? sample_tex(u_to, uv) : sample_tex(u_from, uv));
    float edge = 1.0 - smoothstep(0.0, 0.07, min(bary.x, min(bary.y, bary.z)));
    col *= shade;
    // glowing cracks while the charge builds, white-hot edges at the blast, a flash as pieces land
    col += vec3(1.0, 0.45, 0.12) * heat * (0.15 + 2.2 * edge);
    col += vec3(1.0, 0.62, 0.25) * hot * (0.30 + 1.6 * edge);
    col += vec3(1.0, 0.95, 0.85) * landing * (0.35 + 1.2 * edge);
    col += vec3(1.0) * u_glare * pow(smoothstep(0.95, 0.0, distance(uv, u_light)), 2.2);
    col = clamp(col, 0.0, 1.0);
    o = vec4(col * alpha, alpha);
}
"#;

// Light on top, added to the picture: the flash, two shock rings, and sparks.
const FX_VS: &str = r#"
uniform int u_pass;
uniform vec2 u_view;
uniform vec2 u_center;
uniform vec2 u_shake;
uniform float u_size;
uniform float u_p;
uniform vec2 u_origin;
out vec2 cover;
out vec4 spark;
out vec2 quad;

float h11(float n) { return fract(sin(n * 127.1 + 31.7) * 43758.5453); }

void main() {
    spark = vec4(0.0);
    cover = vec2(0.0);
    quad = vec2(0.0);
    if (u_pass == 0) {
        vec2 pq = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
        gl_Position = vec4(pq * 2.0 - 1.0, 0.0, 1.0);
        // where this corner lies in cover units (y up, the cover's middle is 0)
        cover = ((pq * 2.0 - 1.0) * u_view * 0.5 - u_center) / u_size;
        return;
    }
    int s = gl_VertexID / 6;
    int k = gl_VertexID - s * 6;
    vec2 corner = k == 0 ? vec2(-1.0, -1.0) : k == 1 ? vec2(1.0, -1.0) : k == 2 ? vec2(1.0, 1.0)
                : k == 3 ? vec2(-1.0, -1.0) : k == 4 ? vec2(1.0, 1.0) : vec2(-1.0, 1.0);
    float id = float(s);
    float h1 = h11(id + 1.0), h2 = h11(id + 2.0), h3 = h11(id + 3.0), h4 = h11(id + 4.0), h5 = h11(id + 5.0);
    float start = 0.14 + 0.05 * h2;
    float life = 0.30 + 0.30 * h3;
    float age = (u_p - start) / life;
    if (age < 0.0 || age > 1.0) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
    float a = h1 * 6.2831853;
    float speed = (0.5 + 1.6 * h4) * u_size;
    float ease = 1.0 - pow(1.0 - age, 2.2);
    vec2 origin_px = vec2(u_origin.x - 0.5, -(u_origin.y - 0.5)) * 0.35 * u_size + u_center;
    vec2 pos = origin_px + vec2(cos(a), sin(a)) * speed * ease - vec2(0.0, 0.55 * u_size * age * age);
    float size = (2.0 + 5.0 * h5) * (1.0 - age * 0.8);
    pos += corner * size;
    gl_Position = vec4((pos + u_shake) / (u_view * 0.5), 0.0, 1.0);
    quad = corner;
    float fade = (1.0 - age) * (1.0 - age);
    vec3 warm = mix(vec3(1.0, 0.95, 0.75), vec3(1.0, 0.45, 0.12), clamp(age * 1.4, 0.0, 1.0));
    spark = vec4(warm * fade * 1.4, 1.0);
}
"#;

const FX_FS: &str = r#"
in vec2 cover;
in vec4 spark;
in vec2 quad;
out vec4 o;
uniform int u_pass;
uniform float u_p;
uniform vec2 u_origin;

void main() {
    if (u_pass != 0) {
        float r = length(quad);
        float soft = smoothstep(1.0, 0.0, r);
        o = vec4(spark.rgb * soft * soft, 1.0);
        return;
    }
    vec2 org = vec2(u_origin.x - 0.5, -(u_origin.y - 0.5)) * 0.35;
    float r = length(cover - org);
    float p = u_p;
    float flash = exp(-pow((p - 0.145) / 0.022, 2.0)) * exp(-r * r * 7.0) * 2.2;
    float since = max(p - 0.14, 0.0);
    float ring = exp(-pow((r - since * 2.6) / 0.05, 2.0)) * (1.0 - smoothstep(0.14, 0.5, p)) * step(0.14, p) * 0.9;
    float ring2 = exp(-pow((r - since * 1.5) / 0.09, 2.0)) * (1.0 - smoothstep(0.14, 0.42, p)) * step(0.16, p) * 0.35;
    float charge = exp(-r * r * 5.0) * smoothstep(0.02, 0.14, p) * (1.0 - step(0.14, p)) * 0.5;
    float glint = exp(-pow((p - 0.955) / 0.03, 2.0)) * smoothstep(0.75, 0.0, length(cover)) * 0.7;
    vec3 col = vec3(1.0, 0.82, 0.55) * flash + vec3(1.0, 0.7, 0.35) * ring + vec3(1.0, 0.8, 0.6) * ring2
             + vec3(1.0, 0.45, 0.12) * charge + vec3(1.0, 0.97, 0.9) * glint;
    o = vec4(col, 1.0);
}
"#;

impl Gpu {
    fn new(gl: Arc<glow::Context>) -> Result<Self, String> {
        let version = gl.version().clone();
        let header = if version.is_embedded {
            if version.major < 3 {
                return Err(format!("OpenGL ES {}.{} is too old (3.0 needed)", version.major, version.minor));
            }
            HEADER_ES
        } else {
            if (version.major, version.minor) < (3, 3) {
                return Err(format!("OpenGL {}.{} is too old (3.3 needed)", version.major, version.minor));
            }
            HEADER_DESKTOP
        };
        // the same test egui uses to decide how it stores textures
        let srgb_textures = version.is_embedded || gl.supported_extensions().iter().any(|e| e.contains("sRGB"));
        unsafe {
            let program = compile(&gl, header, VERTEX, FRAGMENT)?;
            let shard = compile(&gl, header, SHARD_VS, SHARD_FS)?;
            let fx = compile(&gl, header, FX_VS, FX_FS)?;
            let vao = gl.create_vertex_array()?;
            Ok(Self { gl, program, shard, fx, vao, srgb_textures })
        }
    }

    /// Set every uniform the program has (the programs share the names).
    unsafe fn uniforms(&self, program: glow::Program, p: &Params, view: [f32; 2], pass: i32) {
        let gl = &self.gl;
        unsafe {
            gl.use_program(Some(program));
            let f1 = |name: &str, v: f32| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_1_f32(Some(&loc), v);
                }
            };
            let f2 = |name: &str, v: [f32; 2]| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_2_f32(Some(&loc), v[0], v[1]);
                }
            };
            let i1 = |name: &str, v: i32| {
                if let Some(loc) = gl.get_uniform_location(program, name) {
                    gl.uniform_1_i32(Some(&loc), v);
                }
            };
            f2("u_view", view);
            f2("u_center", p.center);
            f2("u_shake", p.shake);
            f1("u_size", p.size_px);
            f2("u_rot", p.rot);
            f1("u_zoom", p.zoom);
            f1("u_progress", p.progress);
            f1("u_p", p.progress);
            f2("u_origin", p.origin);
            f1("u_time", p.time);
            f2("u_light", p.light);
            f1("u_glare", p.glare);
            i1("u_effect", p.effect);
            i1("u_reflect", p.reflecting as i32);
            f1("u_holo", p.holo);
            f1("u_flip", p.flip);
            i1("u_srgb", self.srgb_textures as i32);
            i1("u_pass", pass);
            i1("u_from", 0);
            i1("u_to", 1);
        }
    }

    fn render(&self, p: &Params, from: glow::Texture, to: glow::Texture, viewport: [i32; 4]) {
        let [vx, vy, vw, vh] = viewport;
        if vw < 2 || vh < 2 {
            return;
        }
        let gl = &self.gl;
        let view = [vw as f32, vh as f32];
        unsafe {
            gl.viewport(vx, vy, vw, vh);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(from));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(to));
            gl.bind_vertex_array(Some(self.vao));
            if p.effect == EXPLODE && p.progress < 1.0 {
                // the shards, then light on top (added to what is there)
                self.uniforms(self.shard, p, view, 0);
                gl.draw_arrays(glow::TRIANGLES, 0, SHARD_VERTICES);
                gl.blend_func(glow::ONE, glow::ONE);
                self.uniforms(self.fx, p, view, 0);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                self.uniforms(self.fx, p, view, 1);
                gl.draw_arrays(glow::TRIANGLES, 0, SPARKS * 6);
            } else {
                if p.reflect {
                    let mirrored = Params { reflecting: true, ..*p };
                    self.uniforms(self.program, &mirrored, view, 0);
                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                }
                self.uniforms(self.program, p, view, 0);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            }
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }
}

/// Triangles of the shattered plate (a 16 × 16 lattice, two triangles per cell), and sparks.
const SHARD_VERTICES: i32 = 16 * 16 * 2 * 3;
const SPARKS: i32 = 260;

unsafe fn compile(gl: &glow::Context, header: &str, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &format!("{header}{source}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                gl.delete_program(program);
                return Err(format!("shader error: {log}"));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("link error: {log}"));
        }
        Ok(program)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(n: u64) -> Option<TextureId> {
        Some(TextureId::User(n))
    }

    #[test]
    fn the_first_song_is_shown_without_a_flood() {
        let mut a = Anim::default();
        let (from, to, p) = a.update(10.0, 1, tex(1), 1.4, 0.0, CoverEffect::Liquid);
        assert_eq!((from, to, p), (tex(1), tex(1), 1.0));
        assert!(!a.busy(10.0));
    }

    #[test]
    fn a_new_song_floods_over_the_given_time() {
        let mut a = Anim::default();
        a.update(0.0, 1, tex(1), 1.4, 0.0, CoverEffect::Liquid);
        let (from, to, p) = a.update(5.0, 2, tex(2), 4.0, 0.0, CoverEffect::Liquid);
        assert_eq!((from, to), (tex(1), tex(2)));
        assert_eq!(p, 0.0);
        assert!(a.busy(5.0));
        let (_, _, p) = a.update(7.0, 2, tex(2), 4.0, 0.0, CoverEffect::Liquid);
        assert!((p - 0.5).abs() < 1e-6, "{p}");
        let (_, _, p) = a.update(9.5, 2, tex(2), 4.0, 0.0, CoverEffect::Liquid);
        assert_eq!(p, 1.0);
        assert!(!a.busy(9.5));
    }

    #[test]
    fn the_flood_waits_for_the_new_cover() {
        let mut a = Anim::default();
        a.update(0.0, 1, tex(1), 1.4, 0.0, CoverEffect::Liquid);
        // the new song's picture is not loaded yet: the old one stays, no flood
        let (from, to, p) = a.update(3.0, 2, None, 2.0, 0.0, CoverEffect::Liquid);
        assert_eq!((from, to, p), (tex(1), tex(1), 0.0));
        assert!(a.busy(3.0));
        // it arrives later; the flood starts then
        let (from, to, p) = a.update(4.0, 2, tex(2), 2.0, 0.0, CoverEffect::Liquid);
        assert_eq!((from, to, p), (tex(1), tex(2), 0.0));
        let (_, _, p) = a.update(5.0, 2, tex(2), 2.0, 0.0, CoverEffect::Liquid);
        assert!((p - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_better_size_of_the_same_cover_does_not_flood() {
        let mut a = Anim::default();
        a.update(0.0, 1, tex(1), 1.4, 0.0, CoverEffect::Liquid);
        let (from, to, p) = a.update(1.0, 1, tex(9), 1.4, 0.0, CoverEffect::Liquid);
        assert_eq!((from, to, p), (tex(9), tex(9), 1.0));
    }

    #[test]
    fn skipping_during_a_flood_continues_from_where_it_is() {
        let mut a = Anim::default();
        a.update(0.0, 1, tex(1), 1.4, 0.0, CoverEffect::Liquid);
        a.update(1.0, 2, tex(2), 2.0, 0.0, CoverEffect::Liquid);
        let (from, to, p) = a.update(1.5, 3, tex(3), 2.0, 0.0, CoverEffect::Liquid);
        assert_eq!((from, to, p), (tex(2), tex(3), 0.0));
    }

    #[test]
    fn the_liquid_starts_somewhere_different_for_each_song() {
        assert_ne!(origin_for(1), origin_for(2));
        for id in 0..50u64 {
            let [x, y] = origin_for(id);
            assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
        }
    }

    #[test]
    fn a_late_cover_starts_in_step_with_the_sound() {
        let mut a = Anim::default();
        a.update(0.0, 1, tex(1), 1.4, 0.0, CoverEffect::Liquid);
        a.update(10.0, 2, None, 6.0, 0.0, CoverEffect::Liquid);
        // the cover arrives 1.5 s after the song changed: the transition is already a quarter through
        let (_, _, p) = a.update(11.5, 2, tex(2), 6.0, 1.5, CoverEffect::Liquid);
        assert!((p - 0.25).abs() < 1e-3, "{p}");
    }

    #[test]
    fn random_picks_a_real_effect_that_stays_the_same_for_a_song() {
        let mut seen = std::collections::HashSet::new();
        for track in 0..200u64 {
            let id = CoverEffect::Random.shader_id(track);
            assert!((0..12).contains(&id));
            assert_eq!(id, CoverEffect::Random.shader_id(track));
            seen.insert(id);
        }
        assert!(seen.len() >= 10, "{seen:?}");
        assert_eq!(CoverEffect::Explode.shader_id(1), EXPLODE);
        assert_eq!(CoverEffect::Liquid.shader_id(5), 0);
        assert_eq!(CoverEffect::Iris.shader_id(5), 8);
    }

    #[test]
    fn the_explosion_takes_at_least_its_time_even_when_asked_for_less() {
        let mut a = Anim::default();
        a.update(0.0, 1, tex(1), 1.4, 0.0, CoverEffect::Explode);
        let (_, _, p) = a.update(10.0, 2, tex(2), 1.4, 0.0, CoverEffect::Explode);
        assert_eq!(p, 0.0);
        let (_, _, p) = a.update(11.4, 2, tex(2), 1.4, 0.0, CoverEffect::Explode);
        assert!(p < 0.45, "{p}: a 1.4 s call must not rush the explosion");
        let (_, _, p) = a.update(10.0 + EXPLODE_SECS as f64 + 0.1, 2, tex(2), 1.4, 0.0, CoverEffect::Explode);
        assert_eq!(p, 1.0);
        // a longer crossfade makes it longer still
        let mut b = Anim::default();
        b.update(0.0, 1, tex(1), 1.4, 0.0, CoverEffect::Explode);
        b.update(10.0, 2, tex(2), 8.0, 0.0, CoverEffect::Explode);
        let (_, _, p) = b.update(14.0, 2, tex(2), 8.0, 0.0, CoverEffect::Explode);
        assert!((p - 0.5).abs() < 1e-3, "{p}");
    }
}
