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

/// How far the plate tilts at the edge of the cover (radians).
const MAX_TILT: f32 = 0.30;
/// How far beyond the cover the mouse still pulls (in cover widths).
const REACH: f32 = 0.35;
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
    /// A different one every song.
    Random,
}

impl CoverEffect {
    pub const ALL: [CoverEffect; 10] = [
        CoverEffect::Liquid,
        CoverEffect::Ripple,
        CoverEffect::Dissolve,
        CoverEffect::Blinds,
        CoverEffect::Flip,
        CoverEffect::Push,
        CoverEffect::Pixelate,
        CoverEffect::Glitch,
        CoverEffect::Iris,
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
}

impl Default for Cover3d {
    fn default() -> Self {
        Self { gpu: Arc::new(Mutex::new(GpuState::Idle)), anim: Anim::default(), tilt: egui::Vec2::ZERO, hover: 0.0, light: egui::vec2(0.3, 0.25), #[cfg(debug_assertions)] debug_pointer: None }
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
                (egui::vec2(n.x.clamp(-1.0, 1.0), n.y.clamp(-1.0, 1.0)), if inside { 1.0 } else { 0.55 })
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

        // a soft shadow that slides opposite to the tilt
        let shadow_offset = egui::vec2(-self.tilt.x * 16.0, 16.0 - self.tilt.y * 10.0 - self.hover * 4.0);
        ui.painter().add(
            egui::epaint::Shadow {
                offset: [shadow_offset.x as i8, shadow_offset.y as i8],
                blur: 46,
                spread: 0,
                color: egui::Color32::from_black_alpha(150),
            }
            .as_shape(rect.shrink(rect.width() * 0.02), 20.0),
        );

        let outer = rect.expand(rect.width() * MARGIN);
        let ppp = ctx.pixels_per_point();
        let params = Params {
            size_px: rect.width() * ppp,
            rot: [self.tilt.y * MAX_TILT, self.tilt.x * MAX_TILT],
            zoom: 1.0 + 0.025 * self.hover,
            light: [self.light.x, self.light.y],
            glare: 0.18 * self.hover,
            progress,
            origin: self.anim.origin,
            time: now as f32,
            effect: self.anim.effect,
            flip: if self.anim.effect == 4 && progress < 1.0 {
                let e = progress * progress * (3.0 - 2.0 * progress);
                e * std::f32::consts::PI
            } else {
                0.0
            },
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
}

struct Gpu {
    gl: Arc<glow::Context>,
    program: glow::Program,
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
out vec2 uv;
void main() {
    vec2 c = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    uv = vec2(c.x, 1.0 - c.y);
    vec3 q = vec3((c - 0.5) * u_size * u_zoom, 0.0);
    // the flip turns the plate over; the mouse's lean carries over to the back side
    float yaw = u_flip + u_rot.y * cos(u_flip);
    float cy = cos(yaw), sy = sin(yaw), cx = cos(u_rot.x), sx = sin(u_rot.x);
    q = vec3(q.x * cy, q.y, -q.x * sy);               // yaw: the side under the mouse leans away
    q = vec3(q.x, q.y * cx - q.z * sx, q.y * sx + q.z * cx); // pitch
    float d = u_size * 3.0;                            // distance of the camera
    gl_Position = vec4(q.xy / (u_view * 0.5), 0.0, (d - q.z) / d);
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

void main() {
    // rounded corners
    vec2 q = abs(uv - 0.5) - (0.5 - 0.06);
    float sdf = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - 0.06;
    float aa = 1.2 / u_size;
    float alpha = clamp(0.5 - sdf / aa, 0.0, 1.0);
    if (alpha <= 0.0) { o = vec4(0.0); return; }

    float t = clamp(u_progress, 0.0, 1.0);
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
    else { col = iris(uv, t); }

    // light that follows the mouse, and a faint edge falloff for depth
    float dl = distance(uv, u_light);
    col += vec3(1.0) * u_glare * pow(smoothstep(0.95, 0.0, dl), 2.2);
    col *= 1.0 - 0.10 * smoothstep(0.35, 0.75, distance(uv, vec2(0.5)));
    col += vec3(1.0) * 0.045 * smoothstep(0.012, 0.0, -sdf); // light catching the rim of the plate

    col = clamp(col, 0.0, 1.0);
    o = vec4(col * alpha, alpha);
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
            let vao = gl.create_vertex_array()?;
            Ok(Self { gl, program, vao, srgb_textures })
        }
    }

    fn render(&self, p: &Params, from: glow::Texture, to: glow::Texture, viewport: [i32; 4]) {
        let [vx, vy, vw, vh] = viewport;
        if vw < 2 || vh < 2 {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.viewport(vx, vy, vw, vh);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.program));
            let f1 = |name: &str, v: f32| {
                if let Some(loc) = gl.get_uniform_location(self.program, name) {
                    gl.uniform_1_f32(Some(&loc), v);
                }
            };
            let f2 = |name: &str, v: [f32; 2]| {
                if let Some(loc) = gl.get_uniform_location(self.program, name) {
                    gl.uniform_2_f32(Some(&loc), v[0], v[1]);
                }
            };
            let i1 = |name: &str, v: i32| {
                if let Some(loc) = gl.get_uniform_location(self.program, name) {
                    gl.uniform_1_i32(Some(&loc), v);
                }
            };
            f2("u_view", [vw as f32, vh as f32]);
            f1("u_size", p.size_px);
            f2("u_rot", p.rot);
            f1("u_zoom", p.zoom);
            f1("u_progress", p.progress);
            f2("u_origin", p.origin);
            f1("u_time", p.time);
            f2("u_light", p.light);
            f1("u_glare", p.glare);
            i1("u_effect", p.effect);
            f1("u_flip", p.flip);
            i1("u_srgb", self.srgb_textures as i32);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(from));
            i1("u_from", 0);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(to));
            i1("u_to", 1);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }
}

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
            assert!((0..9).contains(&id));
            assert_eq!(id, CoverEffect::Random.shader_id(track));
            seen.insert(id);
        }
        assert!(seen.len() >= 7, "{seen:?}");
        assert_eq!(CoverEffect::Liquid.shader_id(5), 0);
        assert_eq!(CoverEffect::Iris.shader_id(5), 8);
    }
}
