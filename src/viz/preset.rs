//! Presets: how the picture moves and reacts.
//!
//! A preset is data, in the spirit of MilkDrop's per-frame equations: every
//! number is `base` plus how strongly it follows the bass, mids, treble, the
//! beat and a slow oscillation. All motion numbers are "per frame at 60 fps"
//! and scaled by the real frame time, so the picture looks the same at any
//! frame rate. The previous frame is warped by these numbers (the feedback
//! that gives MilkDrop its flowing look), then a waveform or shape is drawn
//! on top.

use std::f32::consts::TAU;

use super::analysis::Features;

/// One animated number.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub base: f32,
    pub bass: f32,
    pub mid: f32,
    pub treb: f32,
    pub beat: f32,
    /// Oscillation amplitude and speed (Hz).
    pub osc: f32,
    pub hz: f32,
}

impl Param {
    pub const fn c(base: f32) -> Self {
        Self { base, bass: 0.0, mid: 0.0, treb: 0.0, beat: 0.0, osc: 0.0, hz: 0.0 }
    }
    pub const fn audio(base: f32, bass: f32, mid: f32, treb: f32, beat: f32) -> Self {
        Self { base, bass, mid, treb, beat, osc: 0.0, hz: 0.0 }
    }
    pub const fn swing(base: f32, osc: f32, hz: f32) -> Self {
        Self { base, bass: 0.0, mid: 0.0, treb: 0.0, beat: 0.0, osc, hz }
    }

    pub fn eval(&self, f: &Features, t: f32) -> f32 {
        self.base
            + self.bass * (f.bass_att - 1.0)
            + self.mid * (f.mid_att - 1.0)
            + self.treb * (f.treb_att - 1.0)
            + self.beat * f.beat
            + self.osc * (t * self.hz * TAU).sin()
    }

    fn mix(a: &Self, b: &Self, x: f32) -> Self {
        let l = |p: f32, q: f32| p + (q - p) * x;
        Self {
            base: l(a.base, b.base),
            bass: l(a.bass, b.bass),
            mid: l(a.mid, b.mid),
            treb: l(a.treb, b.treb),
            beat: l(a.beat, b.beat),
            osc: l(a.osc, b.osc),
            hz: l(a.hz, b.hz),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaveMode {
    None,
    /// The waveform across the screen.
    Line,
    /// Left above, right below.
    DualLine,
    /// The waveform bent into a ring.
    Circle,
    /// A spiral that opens with the signal.
    Spiral,
    /// Left against right (a Lissajous figure).
    Scope,
    /// Spectrum bars along the bottom.
    Bars,
    /// Spectrum bars in a ring.
    Radial,
}

#[derive(Clone, Copy, Debug)]
pub struct Wave {
    pub mode: WaveMode,
    pub scale: Param,
    pub thick: Param,
    pub alpha: Param,
    /// Hue 0‥1 and how fast it drifts (per second).
    pub hue: Param,
    pub hue_rate: f32,
    pub sat: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Shape {
    /// 0 = none; 3 triangle, 4 square … a high number is a circle.
    pub sides: u32,
    pub radius: Param,
    /// Turn speed in radians per second.
    pub spin: Param,
    pub thick: Param,
    /// Copies around the centre.
    pub copies: u32,
    pub hue: Param,
    pub alpha: Param,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mirror {
    None,
    X,
    Y,
    Quad,
    /// A kaleidoscope with this many segments.
    Kaleido(u32),
}

#[derive(Clone, Copy, Debug)]
pub struct Comp {
    pub mirror: Mirror,
    pub bright: Param,
    pub vignette: f32,
    pub aberration: Param,
    /// How strongly a beat flashes or inverts the picture.
    pub flash: f32,
    pub invert: f32,
}

#[derive(Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub zoom: Param,
    pub rot: Param,
    pub swirl: Param,
    pub dx: Param,
    pub dy: Param,
    pub warp: Param,
    pub warp_freq: f32,
    pub warp_speed: f32,
    pub decay: Param,
    pub hue_shift: Param,
    pub wave: Wave,
    pub shape: Shape,
    pub comp: Comp,
}

/// A preset worked out for one frame: what the shaders and the geometry use.
#[derive(Clone, Debug)]
pub struct Look {
    pub zoom: f32,
    pub rot: f32,
    pub swirl: f32,
    pub dx: f32,
    pub dy: f32,
    pub warp: f32,
    pub warp_freq: f32,
    pub warp_speed: f32,
    pub decay: f32,
    pub hue_shift: f32,
    pub mirror: f32,
    pub kaleido: f32,
    pub bright: f32,
    pub vignette: f32,
    pub aberration: f32,
    pub flash: f32,
    pub invert: f32,
    pub wave_mode: WaveMode,
    pub wave_scale: f32,
    pub wave_thick: f32,
    pub wave_rgb: [f32; 3],
    pub wave_alpha: f32,
    pub shape_sides: u32,
    pub shape_radius: f32,
    pub shape_angle: f32,
    pub shape_thick: f32,
    pub shape_copies: u32,
    pub shape_rgb: [f32; 3],
    pub shape_alpha: f32,
}

pub fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

impl Preset {
    /// Work out this frame. `dt` is the frame time in seconds, `t` the time since
    /// the visualizer opened.
    pub fn eval(&self, f: &Features, t: f32, dt: f32) -> Look {
        let k = (dt * 60.0).clamp(0.2, 4.0);
        let e = |p: &Param| p.eval(f, t);
        let (mirror, kaleido) = match self.comp.mirror {
            Mirror::None => (0.0, 0.0),
            Mirror::X => (1.0, 0.0),
            Mirror::Y => (2.0, 0.0),
            Mirror::Quad => (3.0, 0.0),
            Mirror::Kaleido(n) => (4.0, n.max(2) as f32),
        };
        let wave_hue = e(&self.wave.hue) + t * self.wave.hue_rate;
        let shape_hue = e(&self.shape.hue) + t * self.wave.hue_rate;
        Look {
            zoom: e(&self.zoom).clamp(0.9, 1.2).powf(k),
            rot: e(&self.rot) * k,
            swirl: e(&self.swirl) * k,
            dx: e(&self.dx) * k,
            dy: e(&self.dy) * k,
            warp: (e(&self.warp) * k).clamp(0.0, 0.2),
            warp_freq: self.warp_freq,
            warp_speed: self.warp_speed,
            decay: e(&self.decay).clamp(0.5, 0.999).powf(k),
            hue_shift: e(&self.hue_shift) * k,
            mirror,
            kaleido,
            bright: e(&self.comp.bright).clamp(0.3, 2.5),
            vignette: self.comp.vignette,
            aberration: e(&self.comp.aberration).clamp(0.0, 0.05),
            flash: self.comp.flash * f.beat,
            invert: self.comp.invert * f.beat,
            wave_mode: self.wave.mode,
            wave_scale: e(&self.wave.scale),
            wave_thick: e(&self.wave.thick).clamp(0.001, 0.08),
            wave_rgb: hsv(wave_hue, self.wave.sat, 1.0),
            wave_alpha: e(&self.wave.alpha).clamp(0.0, 1.0),
            shape_sides: self.shape.sides,
            shape_radius: e(&self.shape.radius).clamp(0.0, 1.5),
            shape_angle: t * e(&self.shape.spin),
            shape_thick: e(&self.shape.thick).clamp(0.001, 0.08),
            shape_copies: self.shape.copies.max(1),
            shape_rgb: hsv(shape_hue, self.wave.sat, 1.0),
            shape_alpha: e(&self.shape.alpha).clamp(0.0, 1.0),
        }
    }

    /// Halfway between two presets (`x` = 0 is `a`, 1 is `b`): numbers blend,
    /// the kinds of things drawn switch over at the middle.
    pub fn mix(a: &Preset, b: &Preset, x: f32) -> Preset {
        let m = |p: &Param, q: &Param| Param::mix(p, q, x);
        let l = |p: f32, q: f32| p + (q - p) * x;
        let pick = x >= 0.5;
        let mut out = if pick { b.clone() } else { a.clone() };
        out.zoom = m(&a.zoom, &b.zoom);
        out.rot = m(&a.rot, &b.rot);
        out.swirl = m(&a.swirl, &b.swirl);
        out.dx = m(&a.dx, &b.dx);
        out.dy = m(&a.dy, &b.dy);
        out.warp = m(&a.warp, &b.warp);
        out.warp_freq = l(a.warp_freq, b.warp_freq);
        out.warp_speed = l(a.warp_speed, b.warp_speed);
        out.decay = m(&a.decay, &b.decay);
        out.hue_shift = m(&a.hue_shift, &b.hue_shift);
        out.wave.scale = m(&a.wave.scale, &b.wave.scale);
        out.wave.thick = m(&a.wave.thick, &b.wave.thick);
        out.wave.hue = m(&a.wave.hue, &b.wave.hue);
        out.wave.hue_rate = l(a.wave.hue_rate, b.wave.hue_rate);
        out.wave.sat = l(a.wave.sat, b.wave.sat);
        // a wave that appears or vanishes fades through its alpha
        let (wa, wb) = (if a.wave.mode == WaveMode::None { 0.0 } else { 1.0 }, if b.wave.mode == WaveMode::None { 0.0 } else { 1.0 });
        out.wave.alpha = if a.wave.mode == b.wave.mode || (wa > 0.0 && wb > 0.0) {
            m(&a.wave.alpha, &b.wave.alpha)
        } else {
            let fade = if pick { (x - 0.5) * 2.0 } else { 1.0 - x * 2.0 };
            let mut p = if pick { b.wave.alpha } else { a.wave.alpha };
            p.base *= fade;
            p.beat *= fade;
            p.bass *= fade;
            p
        };
        out.shape.radius = m(&a.shape.radius, &b.shape.radius);
        out.shape.spin = m(&a.shape.spin, &b.shape.spin);
        out.shape.thick = m(&a.shape.thick, &b.shape.thick);
        out.shape.hue = m(&a.shape.hue, &b.shape.hue);
        out.shape.alpha = m(&a.shape.alpha, &b.shape.alpha);
        out.comp.bright = m(&a.comp.bright, &b.comp.bright);
        out.comp.vignette = l(a.comp.vignette, b.comp.vignette);
        out.comp.aberration = m(&a.comp.aberration, &b.comp.aberration);
        out.comp.flash = l(a.comp.flash, b.comp.flash);
        out.comp.invert = l(a.comp.invert, b.comp.invert);
        out.name = if pick { b.name.clone() } else { a.name.clone() };
        out
    }
}

// ------------------------------------------------------------------ built-in

fn base(name: &str) -> Preset {
    Preset {
        name: name.to_string(),
        zoom: Param::c(1.01),
        rot: Param::c(0.0),
        swirl: Param::c(0.0),
        dx: Param::c(0.0),
        dy: Param::c(0.0),
        warp: Param::c(0.0),
        warp_freq: 6.0,
        warp_speed: 1.0,
        decay: Param::c(0.96),
        hue_shift: Param::c(0.0),
        wave: Wave {
            mode: WaveMode::Line,
            scale: Param::c(0.3),
            thick: Param::c(0.008),
            alpha: Param::c(0.9),
            hue: Param::c(0.55),
            hue_rate: 0.03,
            sat: 0.75,
        },
        shape: Shape {
            sides: 0,
            radius: Param::c(0.2),
            spin: Param::c(0.5),
            thick: Param::c(0.008),
            copies: 1,
            hue: Param::c(0.1),
            alpha: Param::c(0.8),
        },
        comp: Comp {
            mirror: Mirror::None,
            bright: Param::c(1.0),
            vignette: 0.35,
            aberration: Param::c(0.0),
            flash: 0.0,
            invert: 0.0,
        },
    }
}

pub fn builtin() -> Vec<Preset> {
    let mut list = Vec::new();

    let mut p = base("Tunnel Pulse");
    p.zoom = Param::audio(1.02, 0.012, 0.0, 0.0, 0.03);
    p.rot = Param::swing(0.0, 0.004, 0.1);
    p.decay = Param::c(0.962);
    p.hue_shift = Param::c(0.006);
    p.wave.mode = WaveMode::Circle;
    p.wave.scale = Param::audio(0.22, 0.08, 0.0, 0.0, 0.0);
    p.wave.thick = Param::audio(0.007, 0.0, 0.0, 0.0, 0.006);
    p.comp.aberration = Param::audio(0.001, 0.0, 0.0, 0.0, 0.004);
    list.push(p);

    let mut p = base("Kaleido Bloom");
    p.zoom = Param::audio(1.012, 0.008, 0.0, 0.0, 0.02);
    p.rot = Param::swing(0.005, 0.003, 0.07);
    p.swirl = Param::swing(0.04, 0.03, 0.05);
    p.decay = Param::c(0.972);
    p.wave.mode = WaveMode::Radial;
    p.wave.scale = Param::c(0.35);
    p.wave.thick = Param::c(0.012);
    p.wave.hue = Param::c(0.85);
    p.comp.mirror = Mirror::Kaleido(6);
    p.comp.vignette = 0.5;
    p.comp.aberration = Param::audio(0.002, 0.0, 0.0, 0.0, 0.003);
    list.push(p);

    let mut p = base("Liquid Neon");
    p.zoom = Param::c(1.004);
    p.warp = Param::audio(0.011, 0.012, 0.004, 0.0, 0.012);
    p.warp_freq = 7.0;
    p.warp_speed = 1.1;
    p.decay = Param::c(0.978);
    p.hue_shift = Param::c(0.012);
    p.wave.mode = WaveMode::Line;
    p.wave.scale = Param::audio(0.3, 0.1, 0.0, 0.0, 0.0);
    p.wave.thick = Param::c(0.01);
    p.wave.hue = Param::c(0.9);
    p.comp.bright = Param::c(1.08);
    p.comp.aberration = Param::audio(0.001, 0.0, 0.0, 0.0, 0.006);
    list.push(p);

    let mut p = base("Spiral Galaxy");
    p.zoom = Param::audio(1.014, 0.006, 0.0, 0.0, 0.01);
    p.rot = Param::c(0.003);
    p.swirl = Param::swing(0.55, 0.25, 0.04);
    p.decay = Param::c(0.955);
    p.hue_shift = Param::c(0.004);
    p.wave.mode = WaveMode::Spiral;
    p.wave.scale = Param::audio(0.4, 0.12, 0.0, 0.0, 0.0);
    p.shape.sides = 3;
    p.shape.copies = 3;
    p.shape.radius = Param::audio(0.12, 0.08, 0.0, 0.0, 0.05);
    p.shape.spin = Param::c(0.8);
    p.shape.alpha = Param::c(0.7);
    list.push(p);

    let mut p = base("Heartbeat");
    p.zoom = Param::audio(1.0, 0.0, 0.0, 0.0, 0.05);
    p.decay = Param::c(0.93);
    p.hue_shift = Param::c(0.02);
    p.wave.mode = WaveMode::None;
    p.shape.sides = 48;
    p.shape.radius = Param::audio(0.2, 0.05, 0.0, 0.0, 0.12);
    p.shape.thick = Param::audio(0.012, 0.0, 0.0, 0.0, 0.015);
    p.shape.hue = Param::c(0.0);
    p.shape.alpha = Param::c(0.9);
    p.comp.invert = 0.12;
    p.comp.flash = 0.08;
    p.comp.vignette = 0.5;
    list.push(p);

    let mut p = base("Mirror Room");
    p.zoom = Param::audio(1.01, 0.006, 0.0, 0.0, 0.015);
    p.rot = Param::swing(0.0, 0.003, 0.12);
    p.decay = Param::c(0.965);
    p.hue_shift = Param::c(0.005);
    p.wave.mode = WaveMode::DualLine;
    p.wave.scale = Param::audio(0.25, 0.08, 0.0, 0.0, 0.0);
    p.wave.hue = Param::c(0.3);
    p.comp.mirror = Mirror::Quad;
    list.push(p);

    let mut p = base("Rain Waves");
    p.zoom = Param::c(1.0);
    p.dy = Param::audio(-0.004, -0.003, 0.0, 0.0, 0.0);
    p.decay = Param::c(0.945);
    p.warp = Param::c(0.003);
    p.wave.mode = WaveMode::Line;
    p.wave.scale = Param::audio(0.45, 0.1, 0.0, 0.0, 0.0);
    p.wave.thick = Param::c(0.009);
    p.wave.hue = Param::c(0.58);
    p.comp.mirror = Mirror::Y;
    list.push(p);

    let mut p = base("Starburst");
    p.zoom = Param::audio(1.03, 0.01, 0.0, 0.0, 0.03);
    p.rot = Param::c(0.002);
    p.decay = Param::c(0.95);
    p.hue_shift = Param::c(0.01);
    p.wave.mode = WaveMode::Radial;
    p.wave.scale = Param::c(0.5);
    p.wave.thick = Param::c(0.01);
    p.wave.hue = Param::c(0.05);
    p.comp.vignette = 0.45;
    list.push(p);

    let mut p = base("Hexa Fractal");
    p.zoom = Param::audio(1.04, 0.01, 0.0, 0.0, 0.02);
    p.rot = Param::swing(0.0, 0.006, 0.05);
    p.swirl = Param::c(0.2);
    p.decay = Param::c(0.95);
    p.wave.mode = WaveMode::None;
    p.shape.sides = 6;
    p.shape.copies = 3;
    p.shape.radius = Param::audio(0.16, 0.06, 0.0, 0.0, 0.06);
    p.shape.spin = Param::c(0.6);
    p.shape.hue = Param::swing(0.5, 0.3, 0.03);
    p.comp.mirror = Mirror::Kaleido(3);
    list.push(p);

    let mut p = base("Aurora");
    p.zoom = Param::c(1.003);
    p.warp = Param::audio(0.02, 0.01, 0.0, 0.0, 0.01);
    p.warp_freq = 3.0;
    p.warp_speed = 0.4;
    p.decay = Param::c(0.986);
    p.hue_shift = Param::c(0.01);
    p.wave.mode = WaveMode::DualLine;
    p.wave.scale = Param::audio(0.3, 0.1, 0.0, 0.0, 0.0);
    p.wave.thick = Param::c(0.02);
    p.wave.alpha = Param::c(0.55);
    p.wave.hue = Param::c(0.4);
    p.wave.hue_rate = 0.015;
    p.comp.vignette = 0.6;
    list.push(p);

    let mut p = base("Lissajous");
    p.zoom = Param::c(1.006);
    p.rot = Param::swing(0.0, 0.004, 0.08);
    p.decay = Param::c(0.962);
    p.hue_shift = Param::c(0.008);
    p.wave.mode = WaveMode::Scope;
    p.wave.scale = Param::audio(0.65, 0.2, 0.0, 0.0, 0.0);
    p.wave.thick = Param::c(0.006);
    p.wave.hue = Param::c(0.15);
    p.comp.bright = Param::c(1.1);
    list.push(p);

    let mut p = base("Pulse Grid");
    p.zoom = Param::audio(1.02, 0.01, 0.0, 0.0, 0.03);
    p.rot = Param::c(0.001);
    p.decay = Param::c(0.94);
    p.hue_shift = Param::c(0.015);
    p.wave.mode = WaveMode::Bars;
    p.wave.scale = Param::c(0.8);
    p.wave.thick = Param::c(0.012);
    p.wave.hue = Param::c(0.7);
    p.shape.sides = 4;
    p.shape.radius = Param::audio(0.1, 0.05, 0.0, 0.0, 0.08);
    p.shape.spin = Param::c(0.4);
    p.shape.hue = Param::c(0.2);
    p.comp.mirror = Mirror::Quad;
    p.comp.flash = 0.05;
    list.push(p);

    list
}

// -------------------------------------------------------------------- random

/// A tiny deterministic generator, so a preset can be rebuilt from its seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    pub fn unit(&mut self) -> f32 {
        self.next() as f32 / u32::MAX as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

/// A new preset: one of the built-ins, shaken up (stronger or weaker
/// reactions, other colours, another wave, mirror and spin).
pub fn random(seed: u64) -> Preset {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    for _ in 0..4 {
        rng.next();
    }
    let all = builtin();
    let mut p = all[rng.below(all.len())].clone();
    p.name = format!("Random {:04x}", seed & 0xffff);
    let react = rng.range(0.6, 1.6);
    for param in [&mut p.zoom, &mut p.rot, &mut p.swirl, &mut p.warp, &mut p.wave.scale, &mut p.shape.radius] {
        param.bass *= react;
        param.beat *= react;
    }
    p.zoom.base = 1.0 + (p.zoom.base - 1.0) * rng.range(0.6, 1.5);
    p.rot.base *= if rng.unit() < 0.5 { -1.0 } else { 1.0 } * rng.range(0.5, 1.6);
    p.swirl.base *= rng.range(-1.0, 1.4);
    p.decay.base = (p.decay.base + rng.range(-0.012, 0.012)).clamp(0.9, 0.99);
    p.wave.hue.base = rng.unit();
    p.shape.hue.base = rng.unit();
    p.wave.hue_rate = rng.range(0.0, 0.08);
    p.hue_shift.base = rng.range(0.0, 0.025);
    p.wave.sat = rng.range(0.5, 0.95);
    if rng.unit() < 0.5 {
        const MODES: [WaveMode; 7] = [
            WaveMode::Line,
            WaveMode::DualLine,
            WaveMode::Circle,
            WaveMode::Spiral,
            WaveMode::Scope,
            WaveMode::Bars,
            WaveMode::Radial,
        ];
        p.wave.mode = MODES[rng.below(MODES.len())];
    }
    if rng.unit() < 0.4 {
        p.shape.sides = [0, 3, 4, 5, 6, 8, 32][rng.below(7)];
        p.shape.copies = 1 + rng.below(4) as u32;
    }
    if rng.unit() < 0.6 {
        p.comp.mirror = match rng.below(5) {
            0 => Mirror::None,
            1 => Mirror::X,
            2 => Mirror::Y,
            3 => Mirror::Quad,
            _ => Mirror::Kaleido(3 + rng.below(6) as u32),
        };
    }
    p.comp.flash = if rng.unit() < 0.3 { rng.range(0.03, 0.1) } else { p.comp.flash };
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_are_sane_at_any_loudness() {
        let mut loud = Features::default();
        loud.bass_att = 3.0;
        loud.mid_att = 3.0;
        loud.treb_att = 3.0;
        loud.beat = 1.0;
        let mut quiet = Features::default();
        quiet.bass_att = 0.0;
        quiet.mid_att = 0.0;
        quiet.treb_att = 0.0;
        for preset in builtin().into_iter().chain((0..40).map(random)) {
            for f in [&loud, &quiet, &Features::default()] {
                for dt in [1.0 / 144.0, 1.0 / 60.0, 1.0 / 20.0] {
                    let l = preset.eval(f, 12.3, dt);
                    // the picture must not blow up or vanish from one frame to the next
                    assert!((0.7..=2.2).contains(&l.zoom), "{} zoom {}", preset.name, l.zoom);
                    assert!((0.3..1.0).contains(&l.decay), "{} decay {}", preset.name, l.decay);
                    assert!(l.rot.abs() < 0.2 && l.swirl.abs() < 3.0, "{} rot {} swirl {}", preset.name, l.rot, l.swirl);
                    assert!(l.warp >= 0.0 && l.warp <= 0.2);
                    assert!(l.wave_rgb.iter().chain(&l.shape_rgb).all(|c| (0.0..=1.0).contains(c)));
                }
            }
        }
    }

    #[test]
    fn frame_rate_does_not_change_the_speed() {
        let p = &builtin()[0];
        let f = Features::default();
        let one = p.eval(&f, 1.0, 1.0 / 60.0);
        let two = p.eval(&f, 1.0, 1.0 / 30.0);
        // half the frame rate: twice the step per frame, so the same zoom per second
        assert!((one.zoom.powi(2) - two.zoom).abs() < 1e-4);
        assert!((one.rot * 2.0 - two.rot).abs() < 1e-5);
    }

    #[test]
    fn random_presets_are_reproducible_and_varied() {
        assert_eq!(random(7).name, random(7).name);
        assert_eq!(format!("{:?}", random(7).wave), format!("{:?}", random(7).wave));
        let names: std::collections::HashSet<String> = (0..30).map(|s| format!("{:?}", random(s).wave.hue)).collect();
        assert!(names.len() > 20);
    }

    #[test]
    fn mixing_endpoints_give_the_ends() {
        let all = builtin();
        let (a, b) = (&all[0], &all[2]);
        assert_eq!(Preset::mix(a, b, 0.0).name, a.name);
        assert_eq!(Preset::mix(a, b, 1.0).name, b.name);
        let mid = Preset::mix(a, b, 0.5);
        assert!((mid.zoom.base - (a.zoom.base + b.zoom.base) / 2.0).abs() < 1e-6);
    }
}
