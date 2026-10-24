//! The geometry drawn over the warped picture: a waveform or spectrum and a
//! polygon, as thin triangles (thick lines).

use std::f32::consts::TAU;

use super::analysis::{Features, BARS, WAVE};
use super::preset::{hsv, Look, WaveMode};

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
}

/// Draws in "height units": y from -1 to 1, x from -aspect to aspect.
struct Pen<'a> {
    out: &'a mut Vec<Vertex>,
    aspect: f32,
}

impl Pen<'_> {
    fn vertex(&mut self, p: [f32; 2], color: [f32; 4]) {
        self.out.push(Vertex { pos: [p[0] / self.aspect, p[1]], color });
    }

    fn quad(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2], color: [f32; 4]) {
        for p in [a, b, c, a, c, d] {
            self.vertex(p, color);
        }
    }

    /// A thick line through `points`.
    fn line(&mut self, points: &[[f32; 2]], closed: bool, thick: f32, color: [f32; 4]) {
        let n = points.len();
        if n < 2 {
            return;
        }
        let segments = if closed { n } else { n - 1 };
        let half = thick * 0.5;
        for i in 0..segments {
            let (p, q) = (points[i], points[(i + 1) % n]);
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                continue;
            }
            let (nx, ny) = (-dy / len * half, dx / len * half);
            // a little extra length at both ends hides the gaps at corners
            let (ex, ey) = (dx / len * half, dy / len * half);
            let (p, q) = ([p[0] - ex, p[1] - ey], [q[0] + ex, q[1] + ey]);
            self.quad([p[0] + nx, p[1] + ny], [q[0] + nx, q[1] + ny], [q[0] - nx, q[1] - ny], [p[0] - nx, p[1] - ny], color);
        }
    }
}

fn rgba(rgb: [f32; 3], alpha: f32) -> [f32; 4] {
    [rgb[0], rgb[1], rgb[2], alpha]
}

pub fn build(look: &Look, f: &Features, aspect: f32, out: &mut Vec<Vertex>) {
    out.clear();
    let mut pen = Pen { out, aspect };
    let mono = |i: usize| (f.wave_l[i] + f.wave_r[i]) * 0.5;
    let scale = look.wave_scale;
    let colour = rgba(look.wave_rgb, look.wave_alpha);
    let thick = look.wave_thick;
    let x_at = |i: usize| (i as f32 / (WAVE - 1) as f32 * 2.0 - 1.0) * aspect * 0.95;

    if look.wave_alpha > 0.01 {
        match look.wave_mode {
            WaveMode::None => {}
            WaveMode::Line => {
                let points: Vec<[f32; 2]> = (0..WAVE).map(|i| [x_at(i), mono(i) * scale]).collect();
                pen.line(&points, false, thick, colour);
            }
            WaveMode::DualLine => {
                let left: Vec<[f32; 2]> = (0..WAVE).map(|i| [x_at(i), 0.38 + f.wave_l[i] * scale * 0.6]).collect();
                let right: Vec<[f32; 2]> = (0..WAVE).map(|i| [x_at(i), -0.38 + f.wave_r[i] * scale * 0.6]).collect();
                pen.line(&left, false, thick, colour);
                pen.line(&right, false, thick, colour);
            }
            WaveMode::Circle => {
                let points: Vec<[f32; 2]> = (0..WAVE)
                    .map(|i| {
                        let a = i as f32 / WAVE as f32 * TAU;
                        let r = 0.3 + mono(i) * scale * 0.7;
                        [a.cos() * r, a.sin() * r]
                    })
                    .collect();
                pen.line(&points, true, thick, colour);
            }
            WaveMode::Spiral => {
                let points: Vec<[f32; 2]> = (0..WAVE)
                    .map(|i| {
                        let u = i as f32 / WAVE as f32;
                        let a = u * TAU * 3.0;
                        let r = 0.04 + u * 0.7 + mono(i) * scale * 0.25;
                        [a.cos() * r, a.sin() * r]
                    })
                    .collect();
                pen.line(&points, false, thick, colour);
            }
            WaveMode::Scope => {
                let points: Vec<[f32; 2]> = (0..WAVE).map(|i| [f.wave_l[i] * scale * 1.6, f.wave_r[i] * scale * 1.6]).collect();
                pen.line(&points, false, thick, colour);
            }
            WaveMode::Bars => {
                let w = aspect * 1.9 / BARS as f32;
                for (i, bar) in f.bars.iter().enumerate() {
                    let x = (i as f32 + 0.5) * w - aspect * 0.95;
                    let h = bar * scale * 1.2;
                    let c = rgba(hsv_shift(look.wave_rgb, i as f32 / BARS as f32 * 0.25), look.wave_alpha);
                    pen.quad([x - w * 0.4, -0.97], [x + w * 0.4, -0.97], [x + w * 0.4, -0.97 + h], [x - w * 0.4, -0.97 + h], c);
                }
            }
            WaveMode::Radial => {
                let spokes = BARS * 2;
                for i in 0..spokes {
                    let bar = f.bars[if i < BARS { i } else { spokes - 1 - i }];
                    let a = i as f32 / spokes as f32 * TAU;
                    let (r0, r1) = (0.1, 0.1 + bar * scale);
                    let c = rgba(hsv_shift(look.wave_rgb, i as f32 / spokes as f32 * 0.3), look.wave_alpha);
                    pen.line(&[[a.cos() * r0, a.sin() * r0], [a.cos() * r1, a.sin() * r1]], false, thick, c);
                }
            }
        }
    }

    if look.shape_sides >= 3 && look.shape_alpha > 0.01 {
        let colour = rgba(look.shape_rgb, look.shape_alpha);
        for copy in 0..look.shape_copies {
            let turn = copy as f32 / look.shape_copies as f32 * TAU;
            let centre = if look.shape_copies > 1 { [turn.cos() * 0.3, turn.sin() * 0.3] } else { [0.0, 0.0] };
            let points: Vec<[f32; 2]> = (0..look.shape_sides)
                .map(|i| {
                    let a = look.shape_angle + turn + i as f32 / look.shape_sides as f32 * TAU;
                    [centre[0] + a.cos() * look.shape_radius, centre[1] + a.sin() * look.shape_radius]
                })
                .collect();
            pen.line(&points, true, look.shape_thick, colour);
        }
    }
}

/// The colour with its hue moved on a little.
fn hsv_shift(rgb: [f32; 3], amount: f32) -> [f32; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    hsv(h + amount, s, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viz::preset::{builtin, random};

    #[test]
    fn every_preset_draws_finite_triangles() {
        let mut f = Features::default();
        for i in 0..WAVE {
            f.wave_l[i] = (i as f32 * 0.3).sin() * 0.6;
            f.wave_r[i] = (i as f32 * 0.2).cos() * 0.6;
        }
        f.bars = [0.5; BARS];
        let mut out = Vec::new();
        for preset in builtin().into_iter().chain((0..60).map(random)) {
            let look = preset.eval(&f, 3.0, 1.0 / 60.0);
            build(&look, &f, 16.0 / 9.0, &mut out);
            assert_eq!(out.len() % 3, 0, "{}", preset.name);
            assert!(out.iter().all(|v| v.pos.iter().chain(&v.color).all(|c| c.is_finite())), "{}", preset.name);
        }
    }

    #[test]
    fn a_line_has_the_thickness_asked_for() {
        let mut out = Vec::new();
        let mut pen = Pen { out: &mut out, aspect: 1.0 };
        pen.line(&[[0.0, 0.0], [1.0, 0.0]], false, 0.1, [1.0; 4]);
        let ys: Vec<f32> = out.iter().map(|v| v.pos[1]).collect();
        let (lo, hi) = (ys.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MIN, f32::max));
        assert!((hi - lo - 0.1).abs() < 1e-6);
    }
}
