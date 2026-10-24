//! Parametric 10-band equaliser with preamp, automatic headroom, a look-ahead
//! limiter and a real-time spectrum analyser.
//!
//! * Every band is an RBJ biquad (peak, low/high shelf, low/high cut) running
//!   in `f64` (transposed direct form II) for clean low-frequency behaviour.
//! * Parameter changes are picked up from the UI thread and the coefficients
//!   are ramped over a few milliseconds, so moving a control never clicks.
//! * The look-ahead limiter keeps boosted material below full scale without
//!   hard clipping; the analyser feeds the response graph in the UI.
//!
//! [`EqSource`] wraps any rodio [`Source`] and does all of this on the audio
//! thread. [`EqShared`] is the lock-light bridge to the UI.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, Sample, SampleRate};
use serde::{Deserialize, Serialize};

pub const NUM_BANDS: usize = 10;
pub const MAX_GAIN_DB: f32 = 15.0;
pub const MAX_PREAMP_DB: f32 = 12.0;
pub const MIN_FREQ: f32 = 20.0;
pub const MAX_FREQ: f32 = 20_000.0;
pub const MIN_Q: f32 = 0.2;
pub const MAX_Q: f32 = 10.0;
/// Number of log-spaced analyser bins between 20 Hz and 20 kHz.
pub const SPEC_BINS: usize = 120;

/// Centre frequencies of the default ISO-style octave layout.
pub const DEFAULT_FREQS: [f32; NUM_BANDS] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

// ------------------------------------------------------------------ settings

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterKind {
    LowShelf,
    Peak,
    HighShelf,
    LowCut,
    HighCut,
}

impl FilterKind {
    pub const ALL: [FilterKind; 5] = [
        FilterKind::LowShelf,
        FilterKind::Peak,
        FilterKind::HighShelf,
        FilterKind::LowCut,
        FilterKind::HighCut,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterKind::LowShelf => "Low Shelf",
            FilterKind::Peak => "Glocke",
            FilterKind::HighShelf => "High Shelf",
            FilterKind::LowCut => "Low Cut",
            FilterKind::HighCut => "High Cut",
        }
    }

    /// Whether the gain parameter has any effect.
    pub fn has_gain(self) -> bool {
        !matches!(self, FilterKind::LowCut | FilterKind::HighCut)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Band {
    pub enabled: bool,
    pub kind: FilterKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

impl Default for Band {
    fn default() -> Self {
        Self {
            enabled: true,
            kind: FilterKind::Peak,
            freq: 1000.0,
            gain_db: 0.0,
            q: 1.41,
        }
    }
}

impl Band {
    fn default_for(index: usize) -> Self {
        let kind = match index {
            0 => FilterKind::LowShelf,
            i if i == NUM_BANDS - 1 => FilterKind::HighShelf,
            _ => FilterKind::Peak,
        };
        Self {
            enabled: true,
            kind,
            freq: DEFAULT_FREQS[index],
            gain_db: 0.0,
            q: if kind == FilterKind::Peak { 1.41 } else { 0.71 },
        }
    }

    fn clamped(mut self) -> Self {
        self.freq = self.freq.clamp(MIN_FREQ, MAX_FREQ);
        self.gain_db = self.gain_db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        self.q = self.q.clamp(MIN_Q, MAX_Q);
        if !self.freq.is_finite() || !self.gain_db.is_finite() || !self.q.is_finite() {
            return Band::default();
        }
        self
    }

    /// A band that leaves the signal untouched.
    fn is_neutral(&self) -> bool {
        !self.enabled || (self.kind.has_gain() && self.gain_db.abs() < 0.005)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreset {
    pub name: String,
    pub preamp_db: f32,
    pub bands: [Band; NUM_BANDS],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    pub enabled: bool,
    pub preamp_db: f32,
    /// Compensate the largest boost automatically so the output cannot clip.
    pub auto_headroom: bool,
    pub bands: [Band; NUM_BANDS],
    pub user_presets: Vec<UserPreset>,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preamp_db: 0.0,
            auto_headroom: true,
            bands: std::array::from_fn(Band::default_for),
            user_presets: Vec::new(),
        }
    }
}

impl EqSettings {
    pub fn clamped(mut self) -> Self {
        self.preamp_db = if self.preamp_db.is_finite() {
            self.preamp_db.clamp(-MAX_PREAMP_DB, MAX_PREAMP_DB)
        } else {
            0.0
        };
        for band in &mut self.bands {
            *band = band.clamped();
        }
        self
    }

    /// Back to the default band layout and flat response.
    pub fn reset_layout(&mut self) {
        self.bands = std::array::from_fn(Band::default_for);
        self.preamp_db = 0.0;
    }

    pub fn apply_gains(&mut self, gains: &[f32; NUM_BANDS]) {
        self.bands = std::array::from_fn(Band::default_for);
        for (band, gain) in self.bands.iter_mut().zip(gains) {
            band.gain_db = *gain;
        }
        self.preamp_db = 0.0;
    }

    /// Combined response sampled at `points` log-spaced frequencies (20 Hz–20 kHz).
    pub fn curve(&self, sample_rate: f32, points: usize) -> Vec<f32> {
        let coeffs: Vec<Coeffs> = self
            .bands
            .iter()
            .filter(|b| !b.is_neutral())
            .map(|b| Coeffs::design(b, sample_rate as f64))
            .collect();
        (0..points)
            .map(|i| {
                let f = curve_freq(i, points) as f64;
                coeffs.iter().map(|c| c.magnitude_db(f, sample_rate as f64)).sum::<f64>() as f32
            })
            .collect()
    }

    /// Largest boost of the combined curve, never negative.
    pub fn peak_boost_db(&self, sample_rate: f32) -> f32 {
        let coeffs: Vec<Coeffs> = self
            .bands
            .iter()
            .filter(|b| !b.is_neutral())
            .map(|b| Coeffs::design(b, sample_rate as f64))
            .collect();
        let mut peak = 0.0_f64;
        for i in 0..160 {
            let t = i as f64 / 159.0;
            let f = MIN_FREQ as f64 * (MAX_FREQ as f64 / MIN_FREQ as f64).powf(t);
            let db: f64 = coeffs.iter().map(|c| c.magnitude_db(f, sample_rate as f64)).sum();
            peak = peak.max(db);
        }
        peak as f32
    }

    /// Automatic headroom reduction (dB, ≤ 0) currently applied on top of the preamp.
    pub fn auto_gain_db(&self, sample_rate: f32) -> f32 {
        if self.enabled && self.auto_headroom {
            -self.peak_boost_db(sample_rate)
        } else {
            0.0
        }
    }

    fn total_preamp_db(&self, sample_rate: f32) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        (self.preamp_db + self.auto_gain_db(sample_rate)).clamp(-36.0, MAX_PREAMP_DB)
    }
}

/// Frequency of sample `i` out of `points` on the log axis used by [`EqSettings::curve`].
pub fn curve_freq(i: usize, points: usize) -> f32 {
    let t = i as f32 / (points.max(2) - 1) as f32;
    MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(t)
}

impl Band {
    /// This band's own response (for highlighting the selected band).
    pub fn curve(&self, sample_rate: f32, points: usize) -> Vec<f32> {
        let c = Coeffs::design(self, sample_rate as f64);
        (0..points)
            .map(|i| c.magnitude_db(curve_freq(i, points) as f64, sample_rate as f64) as f32)
            .collect()
    }
}

// ------------------------------------------------------------------- presets

#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    /// Gains for the ten default bands.
    pub gains: [f32; NUM_BANDS],
}

pub fn presets() -> &'static [Preset] {
    const P: &[Preset] = &[
        Preset { name: "Flat", gains: [0.0; NUM_BANDS] },
        Preset { name: "Bass Boost", gains: [6.0, 5.5, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0] },
        Preset { name: "Bass Reducer", gains: [-6.0, -5.0, -3.5, -1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
        Preset { name: "Treble Boost", gains: [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 2.0, 4.0, 5.5, 6.5] },
        Preset { name: "Treble Reducer", gains: [0.0, 0.0, 0.0, 0.0, 0.0, -0.5, -2.0, -4.0, -5.5, -6.5] },
        Preset { name: "Vocal", gains: [-3.0, -2.5, -1.5, 0.5, 2.5, 4.0, 3.5, 2.0, 0.0, -1.0] },
        Preset { name: "Rock", gains: [4.5, 3.5, 2.0, -1.0, -2.5, -0.5, 2.0, 3.5, 4.5, 4.5] },
        Preset { name: "Pop", gains: [-1.5, 1.5, 3.5, 4.0, 2.5, 0.0, -1.0, -1.5, 0.5, 1.0] },
        Preset { name: "Jazz", gains: [3.0, 2.0, 1.0, 2.0, -1.5, -1.5, 0.0, 1.5, 3.0, 3.5] },
        Preset { name: "Classical", gains: [3.0, 2.5, 1.5, 1.0, -0.5, -0.5, 0.0, 1.5, 2.5, 3.0] },
        Preset { name: "Electronic", gains: [5.0, 4.0, 1.0, 0.0, -2.0, 1.5, 0.5, 1.5, 4.0, 5.0] },
        Preset { name: "Hip-Hop", gains: [5.0, 4.5, 2.0, 3.0, -1.0, -1.0, 1.5, -0.5, 2.0, 3.0] },
        Preset { name: "Acoustic", gains: [3.5, 3.0, 2.0, 0.5, 1.5, 1.5, 2.5, 3.0, 2.5, 1.5] },
        Preset { name: "Loudness", gains: [6.0, 4.0, 0.0, -1.0, -2.0, -1.0, 0.0, 2.0, 4.5, 5.5] },
        Preset { name: "Small speakers", gains: [-8.0, -6.0, -3.0, 0.0, 2.0, 3.5, 4.0, 3.0, 1.5, 0.0] },
        Preset { name: "Late night", gains: [2.0, 1.5, 0.5, 0.0, 0.0, 0.5, 1.0, 0.0, -2.5, -4.0] },
    ];
    P
}

// -------------------------------------------------------------- biquad maths

/// Normalised biquad coefficients (`a0 == 1`).
#[derive(Clone, Copy, Debug)]
struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    const IDENTITY: Coeffs = Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// RBJ audio-EQ-cookbook design.
    fn design(band: &Band, sample_rate: f64) -> Self {
        if band.is_neutral() {
            return Self::IDENTITY;
        }
        let f = (band.freq as f64).clamp(10.0, sample_rate * 0.45);
        let q = (band.q as f64).clamp(MIN_Q as f64, MAX_Q as f64);
        let a = 10f64.powf(band.gain_db as f64 / 40.0);
        let w0 = 2.0 * PI * f / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);

        let (b0, b1, b2, a0, a1, a2) = match band.kind {
            FilterKind::Peak => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => {
                let t = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + t),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - t),
                    (a + 1.0) + (a - 1.0) * cos + t,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - t,
                )
            }
            FilterKind::HighShelf => {
                let t = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + t),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - t),
                    (a + 1.0) - (a - 1.0) * cos + t,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - t,
                )
            }
            FilterKind::LowCut => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::HighCut => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Magnitude in dB at `freq`.
    fn magnitude_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let (c1, c2) = (w.cos(), (2.0 * w).cos());
        let num = self.b0 * self.b0
            + self.b1 * self.b1
            + self.b2 * self.b2
            + 2.0 * (self.b0 * self.b1 + self.b1 * self.b2) * c1
            + 2.0 * self.b0 * self.b2 * c2;
        let den = 1.0
            + self.a1 * self.a1
            + self.a2 * self.a2
            + 2.0 * (self.a1 + self.a1 * self.a2) * c1
            + 2.0 * self.a2 * c2;
        10.0 * (num.max(1e-30) / den.max(1e-30)).log10()
    }
}

/// One biquad with ramped coefficients (transposed direct form II).
#[derive(Clone, Copy)]
struct Filter {
    c: Coeffs,
    step: [f64; 5],
    z1: f64,
    z2: f64,
}

impl Filter {
    fn new(c: Coeffs) -> Self {
        Self { c, step: [0.0; 5], z1: 0.0, z2: 0.0 }
    }

    fn retarget(&mut self, target: Coeffs, frames: f64) {
        self.step = [
            (target.b0 - self.c.b0) / frames,
            (target.b1 - self.c.b1) / frames,
            (target.b2 - self.c.b2) / frames,
            (target.a1 - self.c.a1) / frames,
            (target.a2 - self.c.a2) / frames,
        ];
    }

    #[inline]
    fn advance(&mut self) {
        self.c.b0 += self.step[0];
        self.c.b1 += self.step[1];
        self.c.b2 += self.step[2];
        self.c.a1 += self.step[3];
        self.c.a2 += self.step[4];
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.c.b0 * x + self.z1;
        self.z1 = self.c.b1 * x - self.c.a1 * y + self.z2;
        self.z2 = self.c.b2 * x - self.c.a2 * y;
        y
    }
}

// ------------------------------------------------------------- shared state

/// Runtime-shared EQ state: settings from the UI, meters and spectrum back.
pub struct EqShared {
    settings: Mutex<EqSettings>,
    version: AtomicU32,
    sample_rate: AtomicU32,
    peak: AtomicU32,
    reduction: AtomicU32,
    spectrum: Mutex<[f32; SPEC_BINS]>,
    /// The visualizer's view of the sound: switched on only while it is open.
    tap: Tap,
}

impl EqShared {
    pub fn new(eq: EqSettings) -> Arc<Self> {
        Arc::new(Self {
            settings: Mutex::new(eq.clamped()),
            version: AtomicU32::new(1),
            sample_rate: AtomicU32::new(44_100),
            peak: AtomicU32::new(0),
            reduction: AtomicU32::new(0),
            spectrum: Mutex::new([0.0; SPEC_BINS]),
            tap: Tap::default(),
        })
    }

    pub fn update(&self, eq: &EqSettings) {
        if let Ok(mut guard) = self.settings.lock() {
            *guard = eq.clone().clamped();
        }
        self.version.fetch_add(1, Ordering::Release);
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate.load(Ordering::Relaxed) as f32
    }

    /// Output peak level, linear (0..1+), smoothed with a short hold.
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    /// Current limiter gain reduction in dB (≥ 0).
    pub fn reduction_db(&self) -> f32 {
        f32::from_bits(self.reduction.load(Ordering::Relaxed))
    }

    /// Spectrum in dBFS per log-spaced bin (about -100 … 0).
    pub fn spectrum(&self) -> [f32; SPEC_BINS] {
        self.spectrum.lock().map(|s| *s).unwrap_or([-100.0; SPEC_BINS])
    }

    /// Start or stop copying the sound for the visualizer.
    pub fn set_tap(&self, on: bool) {
        self.tap.on.store(on, Ordering::Relaxed);
        if !on {
            if let Ok(mut ring) = self.tap.ring.lock() {
                ring.written = 0;
            }
        }
    }

    /// The newest `frames` stereo frames (interleaved L, R), ending `lag`
    /// frames before the latest one. False if the tap has less than that.
    pub fn read_tap(&self, frames: usize, lag: usize, out: &mut Vec<f32>) -> bool {
        let Ok(ring) = self.tap.ring.lock() else { return false };
        let capacity = TAP_FRAMES;
        if ring.written < frames + lag || frames + lag > capacity {
            return false;
        }
        out.clear();
        let end = ring.written - lag;
        for frame in end - frames..end {
            let i = (frame % capacity) * 2;
            out.push(ring.data[i]);
            out.push(ring.data[i + 1]);
        }
        true
    }

    pub fn clear_meters(&self) {
        self.peak.store(0, Ordering::Relaxed);
        self.reduction.store(0, Ordering::Relaxed);
        if let Ok(mut s) = self.spectrum.lock() {
            *s = [-100.0; SPEC_BINS];
        }
    }
}

// ----------------------------------------------------------------------- tap

/// Frames the tap keeps: about 0.37 s at 44.1 kHz.
const TAP_FRAMES: usize = 16_384;

#[derive(Default)]
struct Tap {
    on: AtomicBool,
    ring: Mutex<TapRing>,
}

struct TapRing {
    /// Interleaved L, R.
    data: Vec<f32>,
    /// Frames written in total.
    written: usize,
}

impl Default for TapRing {
    fn default() -> Self {
        Self { data: vec![0.0; TAP_FRAMES * 2], written: 0 }
    }
}

// ------------------------------------------------------------------ analyser

const FFT_SIZE: usize = 4096;

struct Analyzer {
    ring: Vec<f32>,
    pos: usize,
    since_fft: usize,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    smooth: [f32; SPEC_BINS],
    sample_rate: f32,
}

impl Analyzer {
    fn new(sample_rate: f32) -> Self {
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        Self {
            ring: vec![0.0; FFT_SIZE],
            pos: 0,
            since_fft: 0,
            window,
            re: vec![0.0; FFT_SIZE],
            im: vec![0.0; FFT_SIZE],
            smooth: [-100.0; SPEC_BINS],
            sample_rate,
        }
    }

    fn push(&mut self, mono: f32, shared: &EqShared) {
        self.ring[self.pos] = mono;
        self.pos = (self.pos + 1) % FFT_SIZE;
        self.since_fft += 1;
        if self.since_fft >= FFT_SIZE / 2 {
            self.since_fft = 0;
            self.analyse(shared);
        }
    }

    fn analyse(&mut self, shared: &EqShared) {
        for i in 0..FFT_SIZE {
            self.re[i] = self.ring[(self.pos + i) % FFT_SIZE] * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);

        // Hann window coherent gain is 0.5 → a full-scale sine reads 0 dBFS.
        let norm = 2.0 / (FFT_SIZE as f32 * 0.5);
        let df = self.sample_rate / FFT_SIZE as f32;
        let ratio = (MAX_FREQ / MIN_FREQ).powf(1.0 / SPEC_BINS as f32);
        let mut f_lo = MIN_FREQ;
        for bin in 0..SPEC_BINS {
            let f_hi = f_lo * ratio;
            let k_lo = (f_lo / df).floor() as usize;
            let k_hi = ((f_hi / df).ceil() as usize).min(FFT_SIZE / 2 - 1);
            let mut mag = 0.0_f32;
            for k in k_lo.max(1)..=k_hi.max(k_lo.max(1)) {
                let m = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * norm;
                mag = mag.max(m);
            }
            let db = (20.0 * mag.max(1e-6).log10()).clamp(-100.0, 6.0);
            let s = &mut self.smooth[bin];
            *s = if db > *s { *s + (db - *s) * 0.6 } else { *s + (db - *s) * 0.18 };
            f_lo = f_hi;
        }
        if let Ok(mut out) = shared.spectrum.try_lock() {
            *out = self.smooth;
        }
    }
}

/// In-place iterative radix-2 FFT.
pub(crate) fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0_f32, 0.0_f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

// ------------------------------------------------------------------- limiter

const LIMIT_CEILING: f32 = 0.985;

/// Look-ahead peak limiter: min-filter + box-filter of the required gain gives
/// a smooth attack that is guaranteed to reach the target before the peak.
struct Limiter {
    n: usize,
    channels: usize,
    delay: Vec<f32>,
    delay_pos: usize,
    req: Vec<f32>,
    mins: Vec<f32>,
    ring_pos: usize,
    gain: f32,
    release: f32,
}

impl Limiter {
    fn new(sample_rate: f32, channels: usize) -> Self {
        let n = ((sample_rate * 0.002) as usize).clamp(8, 512);
        Self {
            n,
            channels,
            delay: vec![0.0; (n - 1).max(1) * channels],
            delay_pos: 0,
            req: vec![1.0; n],
            mins: vec![1.0; n],
            ring_pos: 0,
            gain: 1.0,
            release: 1.0 - (-1.0 / (0.08 * sample_rate)).exp(),
        }
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.req.fill(1.0);
        self.mins.fill(1.0);
        self.gain = 1.0;
    }

    /// Process one frame in place (the output is delayed by `n - 1` frames).
    fn process(&mut self, frame: &mut [f32], active: bool) {
        let peak = frame.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let required = if active && peak > LIMIT_CEILING { LIMIT_CEILING / peak } else { 1.0 };

        self.req[self.ring_pos] = required;
        let min = self.req.iter().copied().fold(1.0_f32, f32::min);
        self.mins[self.ring_pos] = min;
        self.ring_pos = (self.ring_pos + 1) % self.n;
        let boxed = self.mins.iter().sum::<f32>() / self.n as f32;

        self.gain = if boxed < self.gain {
            boxed
        } else {
            self.gain + (boxed - self.gain) * self.release
        };

        let d = self.delay.len() / self.channels;
        let base = self.delay_pos * self.channels;
        for (c, sample) in frame.iter_mut().enumerate() {
            let delayed = self.delay[base + c];
            self.delay[base + c] = *sample;
            *sample = delayed * self.gain;
        }
        self.delay_pos = (self.delay_pos + 1) % d;
    }
}

// -------------------------------------------------------------------- source

const RAMP_FRAMES: f64 = 384.0;

/// A rodio source that applies the shared EQ chain to every frame.
pub struct EqSource<S> {
    inner: S,
    channels: usize,
    sample_rate: f32,
    shared: Arc<EqShared>,
    version: u32,
    check: u32,

    /// `[channel][band]`
    filters: Vec<Vec<Filter>>,
    preamp: f64,
    preamp_step: f64,
    ramp_left: u32,
    limiter_active: bool,

    limiter: Limiter,
    analyzer: Analyzer,
    frame: Vec<f32>,
    out: Vec<f32>,
    out_pos: usize,
    drain_left: usize,
    meter_peak: f32,
    meter_hold: f32,
    meter_count: u32,
    done: bool,
    /// Stereo frames waiting to be handed to the tap.
    tap_buf: Vec<f32>,
}

impl<S: Source> EqSource<S> {
    pub fn new(inner: S, shared: Arc<EqShared>) -> Self {
        let channels = (inner.channels().get() as usize).max(1);
        let sample_rate = inner.sample_rate().get() as f32;
        shared.sample_rate.store(sample_rate as u32, Ordering::Relaxed);

        let eq = shared.settings.lock().map(|g| g.clone()).unwrap_or_default();
        let limiter = Limiter::new(sample_rate, channels);
        let drain = limiter.delay.len() / channels;
        let mut source = Self {
            inner,
            channels,
            sample_rate,
            version: shared.version.load(Ordering::Acquire),
            shared,
            check: 0,
            filters: Vec::new(),
            preamp: 1.0,
            preamp_step: 0.0,
            ramp_left: 0,
            limiter_active: eq.enabled,
            limiter,
            analyzer: Analyzer::new(sample_rate),
            frame: vec![0.0; channels],
            out: vec![0.0; channels],
            out_pos: channels,
            drain_left: drain,
            meter_peak: 0.0,
            meter_hold: 0.0,
            meter_count: 0,
            done: false,
            tap_buf: Vec::new(),
        };
        // Start directly at the configured state — no ramp on the first frame.
        source.filters = (0..channels)
            .map(|_| {
                eq.bands
                    .iter()
                    .map(|b| Filter::new(Coeffs::design(b, sample_rate as f64)))
                    .collect()
            })
            .collect();
        source.preamp = db_to_lin(eq.total_preamp_db(sample_rate));
        source
    }

    /// Pick up new settings from the UI thread and start a coefficient ramp.
    fn refresh(&mut self) {
        let version = self.shared.version.load(Ordering::Acquire);
        if version == self.version {
            return;
        }
        let Ok(eq) = self.shared.settings.try_lock().map(|g| g.clone()) else {
            return; // contended: try again on the next check
        };
        self.version = version;

        let sr = self.sample_rate as f64;
        for channel in &mut self.filters {
            for (filter, band) in channel.iter_mut().zip(eq.bands.iter()) {
                let target = if eq.enabled {
                    Coeffs::design(band, sr)
                } else {
                    Coeffs::IDENTITY
                };
                filter.retarget(target, RAMP_FRAMES);
            }
        }
        let target = db_to_lin(eq.total_preamp_db(self.sample_rate));
        self.preamp_step = (target - self.preamp) / RAMP_FRAMES;
        self.ramp_left = RAMP_FRAMES as u32;
        self.limiter_active = eq.enabled;
    }

    /// Produce the next processed frame into `self.out`. Returns false at the end.
    fn next_frame(&mut self) -> bool {
        if self.done {
            return false;
        }
        if self.check == 0 {
            self.refresh();
        }
        self.check = (self.check + 1) & 0xff;

        // Pull one frame from the inner source.
        let mut ended = false;
        for c in 0..self.channels {
            match self.inner.next() {
                Some(s) => self.frame[c] = s,
                None => {
                    if c == 0 {
                        ended = true;
                    }
                    self.frame[c] = 0.0;
                }
            }
        }
        if ended {
            if self.drain_left == 0 {
                self.done = true;
                return false;
            }
            self.drain_left -= 1;
        }

        // Equaliser.
        let ramping = self.ramp_left > 0;
        if ramping {
            self.preamp += self.preamp_step;
            self.ramp_left -= 1;
        }
        let preamp = self.preamp;
        for (c, bands) in self.filters.iter_mut().enumerate() {
            let mut x = self.frame[c] as f64 * preamp;
            for filter in bands.iter_mut() {
                if ramping {
                    filter.advance();
                }
                x = filter.process(x);
            }
            self.frame[c] = x as f32;
        }

        // Limiter (delays the frame by a couple of milliseconds).
        self.limiter.process(&mut self.frame, self.limiter_active);

        // Metering and analysis.
        let mut peak = 0.0_f32;
        let mut mono = 0.0_f32;
        for s in &mut self.frame {
            *s = s.clamp(-1.0, 1.0);
            peak = peak.max(s.abs());
            mono += *s;
        }
        self.analyzer.push(mono / self.channels as f32, &self.shared);
        if self.shared.tap.on.load(Ordering::Relaxed) {
            let (l, r) = (self.frame[0], self.frame[self.channels.min(2) - 1]);
            self.tap_buf.extend_from_slice(&[l, r]);
            if self.tap_buf.len() >= 1024 {
                self.flush_tap();
            }
        }
        self.meter_peak = self.meter_peak.max(peak);
        self.meter_count += 1;
        if self.meter_count >= 1024 {
            self.meter_hold = self.meter_peak.max(self.meter_hold * 0.85);
            self.shared.peak.store(self.meter_hold.to_bits(), Ordering::Relaxed);
            let reduction = -20.0 * self.limiter.gain.max(1e-4).log10();
            self.shared.reduction.store(reduction.to_bits(), Ordering::Relaxed);
            self.meter_peak = 0.0;
            self.meter_count = 0;
        }

        std::mem::swap(&mut self.frame, &mut self.out);
        self.out_pos = 0;
        true
    }
}

fn db_to_lin(db: f32) -> f64 {
    10f64.powf(db as f64 / 20.0)
}

impl<S: Source> EqSource<S> {
    /// Hand the collected frames to the tap. If the visualizer is reading
    /// right now they stay here for the next try: the audio thread never waits.
    fn flush_tap(&mut self) {
        if let Ok(mut ring) = self.shared.tap.ring.try_lock() {
            for pair in self.tap_buf.chunks_exact(2) {
                let i = (ring.written % TAP_FRAMES) * 2;
                ring.data[i] = pair[0];
                ring.data[i + 1] = pair[1];
                ring.written += 1;
            }
            self.tap_buf.clear();
        } else if self.tap_buf.len() > 16_384 {
            self.tap_buf.clear();
        }
    }
}

impl<S: Source> Iterator for EqSource<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.out_pos >= self.channels && !self.next_frame() {
            return None;
        }
        let sample = self.out[self.out_pos];
        self.out_pos += 1;
        Some(sample)
    }
}

impl<S: Source> Source for EqSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)?;
        for channel in &mut self.filters {
            for filter in channel {
                filter.z1 = 0.0;
                filter.z2 = 0.0;
            }
        }
        self.limiter.reset();
        self.out_pos = self.channels;
        self.done = false;
        self.drain_left = self.limiter.delay.len() / self.channels;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_band_hits_its_gain_at_centre() {
        let band = Band { kind: FilterKind::Peak, freq: 1000.0, gain_db: 6.0, q: 1.0, enabled: true };
        let c = Coeffs::design(&band, 48_000.0);
        assert!((c.magnitude_db(1000.0, 48_000.0) - 6.0).abs() < 0.01);
        assert!(c.magnitude_db(100.0, 48_000.0).abs() < 0.5);
    }

    #[test]
    fn shelves_and_cuts_behave() {
        let low = Band { kind: FilterKind::LowShelf, freq: 200.0, gain_db: 8.0, q: 0.71, enabled: true };
        let c = Coeffs::design(&low, 44_100.0);
        assert!((c.magnitude_db(20.0, 44_100.0) - 8.0).abs() < 0.3);
        assert!(c.magnitude_db(10_000.0, 44_100.0).abs() < 0.3);
        let hi = Band { kind: FilterKind::HighShelf, freq: 4000.0, gain_db: -6.0, q: 0.71, enabled: true };
        let c = Coeffs::design(&hi, 44_100.0);
        assert!((c.magnitude_db(18_000.0, 44_100.0) + 6.0).abs() < 0.5);
        let cut = Band { kind: FilterKind::LowCut, freq: 100.0, gain_db: 0.0, q: 0.71, enabled: true };
        let c = Coeffs::design(&cut, 44_100.0);
        assert!(c.magnitude_db(20.0, 44_100.0) < -20.0);
        assert!(c.magnitude_db(5000.0, 44_100.0).abs() < 0.1);
    }

    #[test]
    fn auto_headroom_covers_the_peak() {
        let mut eq = EqSettings::default();
        eq.enabled = true;
        eq.apply_gains(&[6.0, 6.0, 6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(eq.peak_boost_db(48_000.0) >= 6.0);
        assert!(eq.auto_gain_db(48_000.0) <= -6.0);
        eq.enabled = false;
        assert_eq!(eq.auto_gain_db(48_000.0), 0.0);
    }

    #[test]
    fn limiter_holds_the_ceiling() {
        let mut lim = Limiter::new(48_000.0, 2);
        let mut worst = 0.0_f32;
        for i in 0..20_000 {
            let loud = if (5_000..6_000).contains(&i) { 3.0 } else { 0.3 };
            let s = (i as f32 * 0.05).sin() * loud;
            let mut frame = [s, -s];
            lim.process(&mut frame, true);
            worst = worst.max(frame[0].abs()).max(frame[1].abs());
        }
        assert!(worst <= LIMIT_CEILING + 1e-3, "worst {worst}");
    }

    #[test]
    fn fft_finds_a_sine() {
        let n = FFT_SIZE;
        let mut re: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * 64.0 * i as f32 / n as f32).sin()).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let peak = (0..n / 2)
            .max_by(|&a, &b| (re[a].hypot(im[a])).total_cmp(&re[b].hypot(im[b])))
            .unwrap();
        assert_eq!(peak, 64);
    }

    #[test]
    fn old_settings_files_still_load() {
        let json = r#"{"enabled":true,"preamp_db":1.0,"bands_db":[7.0,6.0,5.0,3.0,1.0,0.0,0.0,0.0,0.0,0.0]}"#;
        let eq: EqSettings = serde_json::from_str(json).unwrap();
        assert!(eq.enabled);
        assert_eq!(eq.bands.len(), NUM_BANDS);
    }
}
