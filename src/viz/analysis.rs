//! What the visualizer hears: loudness in three bands (relative to the
//! recent average, like MilkDrop's `bass`/`mid`/`treb`, where 1.0 is "as
//! loud as lately"), a spectrum, a waveform and beat detection.

use crate::eq::fft;

/// Frames per analysis (a power of two for the FFT).
pub const FRAMES: usize = 2048;
/// Spectrum bars handed to the renderer.
pub const BARS: usize = 64;
/// Waveform points handed to the renderer.
pub const WAVE: usize = 256;

#[derive(Clone)]
pub struct Features {
    /// Band loudness relative to the long-term average (about 0‥3, 1 = normal).
    pub bass: f32,
    pub mid: f32,
    pub treb: f32,
    /// The same, smoothed over about half a second.
    pub bass_att: f32,
    pub mid_att: f32,
    pub treb_att: f32,
    /// 1 on a beat, falling to 0 within about a fifth of a second.
    pub beat: f32,
    /// True for the one frame in which a beat was detected.
    pub hit: bool,
    /// Overall loudness, 0‥1.
    pub level: f32,
    /// Left and right waveform, -1‥1.
    pub wave_l: [f32; WAVE],
    pub wave_r: [f32; WAVE],
    /// Log-spaced spectrum, 0‥1.
    pub bars: [f32; BARS],
}

impl Default for Features {
    fn default() -> Self {
        Self {
            bass: 1.0,
            mid: 1.0,
            treb: 1.0,
            bass_att: 1.0,
            mid_att: 1.0,
            treb_att: 1.0,
            beat: 0.0,
            hit: false,
            level: 0.0,
            wave_l: [0.0; WAVE],
            wave_r: [0.0; WAVE],
            bars: [0.0; BARS],
        }
    }
}

pub struct Analyzer {
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Long-term average energy per band (the reference for "1.0").
    avg: [f32; 3],
    /// Short-term smoothed energy per band.
    att: [f32; 3],
    /// Recent bass energies, for the beat threshold.
    history: Vec<f32>,
    since_beat: f32,
    primed: bool,
    bar_smooth: [f32; BARS],
    pub features: Features,
}

impl Default for Analyzer {
    fn default() -> Self {
        let window = (0..FRAMES)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FRAMES as f32).cos())
            .collect();
        Self {
            window,
            re: vec![0.0; FRAMES],
            im: vec![0.0; FRAMES],
            avg: [0.02; 3],
            att: [1.0; 3],
            history: Vec::new(),
            since_beat: 1.0,
            primed: false,
            bar_smooth: [0.0; BARS],
            features: Features::default(),
        }
    }
}

/// Band edges in Hz.
const BASS: (f32, f32) = (30.0, 160.0);
const MID: (f32, f32) = (160.0, 2_500.0);
const TREB: (f32, f32) = (2_500.0, 12_000.0);
/// Spectrum range in Hz.
const LOW: f32 = 30.0;
const HIGH: f32 = 16_000.0;
/// Beats closer than this are one beat (360 BPM at the limit).
const MIN_GAP: f32 = 0.17;

impl Analyzer {
    /// Feed the newest [`FRAMES`] stereo frames (interleaved L, R) and the
    /// seconds since the last call. `sensitivity` scales how readily beats fire.
    pub fn update(&mut self, stereo: &[f32], rate: f32, dt: f32, sensitivity: f32) -> &Features {
        let dt = dt.clamp(0.001, 0.25);
        let frames = (stereo.len() / 2).min(FRAMES);
        let offset = stereo.len() / 2 - frames;

        // waveform (decimated) and the FFT input
        let mut peak = 0.0f32;
        for i in 0..FRAMES {
            let (l, r) = if i < frames {
                (stereo[(offset + i) * 2], stereo[(offset + i) * 2 + 1])
            } else {
                (0.0, 0.0)
            };
            peak = peak.max(l.abs()).max(r.abs());
            self.re[i] = (l + r) * 0.5 * self.window[i];
            self.im[i] = 0.0;
            if i % (FRAMES / WAVE) == 0 {
                let k = i / (FRAMES / WAVE);
                self.features.wave_l[k] = l;
                self.features.wave_r[k] = r;
            }
        }
        fft(&mut self.re, &mut self.im);

        let df = rate / FRAMES as f32;
        let mag = |k: usize| (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() / (FRAMES as f32 * 0.25);
        let band = |(lo, hi): (f32, f32)| -> f32 {
            let (a, b) = (((lo / df) as usize).max(1), ((hi / df) as usize).min(FRAMES / 2 - 1));
            // the band's energy (not the mean level of its bins, which a
            // single tone in a wide band would drown)
            let mut sum = 0.0f32;
            for k in a..=b.max(a) {
                let m = mag(k);
                sum += m * m;
            }
            sum.sqrt()
        };
        let energy = [band(BASS), band(MID), band(TREB)];

        // relative loudness: the average adapts over a few seconds
        if !self.primed {
            self.primed = true;
            self.avg = [energy[0].max(1e-3), energy[1].max(1e-3), energy[2].max(1e-3)];
        }
        let mut rel = [0.0f32; 3];
        for i in 0..3 {
            let slow = 1.0 - (-dt / 4.0).exp();
            self.avg[i] += (energy[i].max(self.avg[i] * 0.2) - self.avg[i]) * slow;
            self.avg[i] = self.avg[i].max(1e-4);
            rel[i] = (energy[i] / self.avg[i]).min(4.0);
            let follow = 1.0 - (-dt / 0.45).exp();
            self.att[i] += (rel[i] - self.att[i]) * follow;
        }
        let silent = peak < 0.003;
        if silent {
            rel = [1.0; 3];
            self.att = [1.0; 3];
        }

        // beats: the bass jumps above its own recent level
        self.since_beat += dt;
        let level = energy[0];
        self.history.push(level);
        let keep = (1.2 / dt).clamp(10.0, 240.0) as usize;
        if self.history.len() > keep {
            let excess = self.history.len() - keep;
            self.history.drain(..excess);
        }
        let mean = self.history.iter().sum::<f32>() / self.history.len() as f32;
        let var = self.history.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / self.history.len() as f32;
        let threshold = mean + (1.5 * var.sqrt()).max(mean * 0.25) / sensitivity.max(0.2);
        let hit = !silent && level > threshold && level > 0.012 && self.since_beat > MIN_GAP && self.history.len() > 8;
        if hit {
            self.since_beat = 0.0;
        }

        let f = &mut self.features;
        f.hit = hit;
        f.beat = if hit { 1.0 } else { f.beat * (-dt * 6.0).exp() };
        f.bass = rel[0];
        f.mid = rel[1];
        f.treb = rel[2];
        f.bass_att = self.att[0];
        f.mid_att = self.att[1];
        f.treb_att = self.att[2];
        f.level = (peak * 1.4).min(1.0);

        // spectrum bars: log-spaced, in dB mapped to 0‥1, with a slow fall
        let ratio = (HIGH / LOW).powf(1.0 / BARS as f32);
        let mut lo = LOW;
        for bar in 0..BARS {
            let hi = lo * ratio;
            let (a, b) = (((lo / df) as usize).max(1), ((hi / df).ceil() as usize).min(FRAMES / 2 - 1));
            let mut m = 0.0f32;
            for k in a..=b.max(a) {
                m = m.max(mag(k));
            }
            let db = 20.0 * m.max(1e-6).log10();
            let v = ((db + 70.0) / 62.0).clamp(0.0, 1.0);
            let s = &mut self.bar_smooth[bar];
            *s = if v > *s { v } else { *s + (v - *s) * (1.0 - (-dt * 9.0).exp()) };
            f.bars[bar] = *s;
            lo = hi;
        }
        &self.features
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 44_100.0;

    /// Interleaved stereo for `secs` of a signal starting at `start` seconds.
    fn render(start: f32, secs: f32, f: impl Fn(f32) -> f32) -> Vec<f32> {
        let n = (secs * RATE) as usize;
        (0..n).flat_map(|i| {
            let v = f(start + i as f32 / RATE);
            [v, v]
        }).collect()
    }

    /// A kick drum every `period` seconds: a short low sine with a decay.
    fn kicks(t: f32, period: f32) -> f32 {
        let p = t % period;
        (std::f32::consts::TAU * 60.0 * p).sin() * (-p * 25.0).exp() * 0.8
    }

    /// Run the analyser over a signal in 60 fps steps; the beats it found.
    fn beats(f: impl Fn(f32) -> f32, secs: f32) -> usize {
        let mut a = Analyzer::default();
        let mut hits = 0;
        let step = 1.0 / 60.0;
        let mut t = 0.0;
        while t < secs {
            let block = render(t + step - FRAMES as f32 / RATE, FRAMES as f32 / RATE, &f);
            if a.update(&block, RATE, step, 1.0).hit {
                hits += 1;
            }
            t += step;
        }
        hits
    }

    #[test]
    fn finds_the_kicks() {
        // 120 BPM for 10 s = 20 kicks; the first seconds teach the threshold
        let n = beats(|t| kicks(t, 0.5), 10.0);
        assert!((14..=26).contains(&n), "{n} beats");
    }

    #[test]
    fn silence_and_steady_tones_make_no_beats() {
        assert_eq!(beats(|_| 0.0, 5.0), 0);
        let tone = |t: f32| (std::f32::consts::TAU * 80.0 * t).sin() * 0.5;
        assert!(beats(tone, 8.0) <= 2);
    }

    #[test]
    fn bands_follow_the_pitch() {
        let mut a = Analyzer::default();
        let low = render(0.0, FRAMES as f32 / RATE, |t| (std::f32::consts::TAU * 80.0 * t).sin() * 0.5);
        for _ in 0..30 {
            a.update(&low, RATE, 1.0 / 60.0, 1.0);
        }
        // now a high tone: treble jumps well above its average, bass drops
        let high = render(0.0, FRAMES as f32 / RATE, |t| (std::f32::consts::TAU * 6000.0 * t).sin() * 0.5);
        let f = a.update(&high, RATE, 1.0 / 60.0, 1.0).clone();
        assert!(f.treb > 1.5, "treb {}", f.treb);
        assert!(f.bass < 0.5, "bass {}", f.bass);
        // and the spectrum peaks near 6 kHz
        let top = f.bars.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        let hz = LOW * (HIGH / LOW).powf((top as f32 + 0.5) / BARS as f32);
        assert!((4_500.0..8_000.0).contains(&hz), "{hz} Hz");
    }
}
