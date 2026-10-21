//! Transitions between tracks: gapless playback or a fixed-length crossfade.

use std::f32::consts::FRAC_PI_2;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, Sample, SampleRate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TransitionMode {
    /// Gapless: the next track starts the moment the current one ends.
    #[default]
    Off,
    /// A fixed-length crossfade at the end of every track.
    // (settings written by older versions may still say "Automix")
    #[serde(alias = "Automix")]
    Crossfade,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    pub mode: TransitionMode,
    /// Crossfade length in seconds.
    pub seconds: f32,
}

// ------------------------------------------------------------------- gain

fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

fn store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

/// Gain of one playing track, shared between the audio thread and its source.
pub struct GainCtl {
    /// Fade position 0‥1 the source moves towards.
    target: AtomicU32,
    /// Seconds a full 0→1 fade takes.
    fade_secs: AtomicU32,
    /// Seconds to wait before the fade starts.
    delay_secs: AtomicU32,
    /// Bumped on every new fade command.
    seq: AtomicU32,
    /// Where the fade currently is, as reported back by the source.
    current: AtomicU32,
}

impl GainCtl {
    pub fn new(start: f32) -> Arc<Self> {
        Arc::new(Self {
            target: AtomicU32::new(start.to_bits()),
            fade_secs: AtomicU32::new(0f32.to_bits()),
            delay_secs: AtomicU32::new(0f32.to_bits()),
            seq: AtomicU32::new(0),
            current: AtomicU32::new(start.to_bits()),
        })
    }

    /// Fade to `target` over `secs`, starting after `delay` seconds.
    pub fn fade_to(&self, target: f32, secs: f32, delay: f32) {
        store(&self.fade_secs, secs.max(0.0));
        store(&self.delay_secs, delay.max(0.0));
        store(&self.target, target.clamp(0.0, 1.0));
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// True once a fade-out has reached silence.
    pub fn faded_out(&self) -> bool {
        load(&self.target) <= 0.0 && load(&self.current) <= 0.001
    }
}

/// Applies a [`GainCtl`] to a source: an equal-power fade and a little
/// leading silence for exact timing.
pub struct Fader<S> {
    inner: S,
    ctl: Arc<GainCtl>,
    channels: usize,
    sample_rate: f32,
    /// Samples of silence still to play before the source starts.
    pre_roll: usize,
    slot: usize,
    countdown: u32,
    seq: u32,
    delay_frames: u64,
    fade: f32,
    target: f32,
    step: f32,
    gain: f32,
}

impl<S: Source> Fader<S> {
    pub fn new(inner: S, ctl: Arc<GainCtl>, pre_roll_secs: f64) -> Self {
        let channels = (inner.channels().get() as usize).max(1);
        let sample_rate = inner.sample_rate().get() as f32;
        let fade = load(&ctl.current);
        let pre_roll = (pre_roll_secs.max(0.0) * sample_rate as f64) as usize * channels;
        Self {
            inner,
            channels,
            sample_rate,
            pre_roll,
            slot: 0,
            countdown: 0,
            // (sentinel: the first frame picks up commands given before)
            seq: u32::MAX,
            delay_frames: 0,
            fade,
            target: fade,
            step: 1.0,
            gain: (fade * FRAC_PI_2).sin(),
            ctl,
        }
    }

    /// Once per frame: follow fade commands.
    fn frame(&mut self) {
        if self.countdown == 0 {
            self.countdown = 64;
            let seq = self.ctl.seq.load(Ordering::Acquire);
            if seq != self.seq {
                self.seq = seq;
                self.delay_frames = (load(&self.ctl.delay_secs) * self.sample_rate) as u64;
            }
            self.target = load(&self.ctl.target);
            let secs = load(&self.ctl.fade_secs);
            self.step = if secs <= 0.0 { 1.0 } else { 1.0 / (secs * self.sample_rate) };
            store(&self.ctl.current, self.fade);
        }
        self.countdown -= 1;
        if self.delay_frames > 0 {
            self.delay_frames -= 1;
        } else if self.fade < self.target {
            self.fade = (self.fade + self.step).min(self.target);
        } else if self.fade > self.target {
            self.fade = (self.fade - self.step).max(self.target);
        }
        self.gain = (self.fade * FRAC_PI_2).sin();
    }
}

impl<S: Source> Iterator for Fader<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.pre_roll > 0 {
            self.pre_roll -= 1;
            return Some(0.0);
        }
        let sample = self.inner.next()?;
        if self.slot == 0 {
            self.frame();
        }
        self.slot = (self.slot + 1) % self.channels;
        Some(sample * self.gain)
    }
}

impl<S: Source> Source for Fader<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len().map(|n| n + self.pre_roll)
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
        self.pre_roll = 0;
        self.slot = 0;
        self.inner.try_seek(pos)
    }
}

// ------------------------------------------------------------------- plan

/// When and how to move from the current track to the next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// Position in the current track at which the next one starts.
    pub start_at: f64,
    /// Fade length in seconds; 0 = gapless cut when the track ends.
    pub fade: f32,
}

pub fn plan(transition: Transition, cur_duration: f64) -> Plan {
    let gapless = Plan { start_at: cur_duration, fade: 0.0 };
    if cur_duration <= 0.0 {
        return gapless;
    }
    match transition.mode {
        TransitionMode::Off => gapless,
        TransitionMode::Crossfade => {
            let fade = transition.seconds.min((cur_duration / 3.0) as f32).max(0.0);
            Plan { start_at: cur_duration - fade as f64, fade }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1000;

    fn square(secs: u64) -> impl Source + Clone {
        rodio::source::SignalGenerator::new(
            std::num::NonZero::new(RATE).unwrap(),
            100.0,
            rodio::source::Function::Square,
        )
        .take_duration(Duration::from_secs(secs))
    }

    #[test]
    fn crossfade_starts_before_the_end() {
        let t = Transition { mode: TransitionMode::Crossfade, seconds: 6.0 };
        let p = plan(t, 200.0);
        assert_eq!((p.start_at, p.fade), (194.0, 6.0));
        // short tracks never fade longer than a third of their length
        assert_eq!(plan(t, 9.0).fade, 3.0);
        let off = Transition { mode: TransitionMode::Off, seconds: 6.0 };
        assert_eq!(plan(off, 200.0).fade, 0.0);
    }

    #[test]
    fn old_automix_setting_still_loads() {
        let mode: TransitionMode = serde_json::from_str("\"Automix\"").unwrap();
        assert_eq!(mode, TransitionMode::Crossfade);
    }

    #[test]
    fn fader_ramps_with_equal_power() {
        let ctl = GainCtl::new(0.0);
        ctl.fade_to(1.0, 1.0, 0.0);
        let out: Vec<f32> = Fader::new(square(2), ctl.clone(), 0.0).collect();
        let peak = |range: std::ops::Range<usize>| out[range].iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak(0..20) < 0.1);
        assert!(peak(400..600) > 0.6 && peak(400..600) < 0.95);
        assert!(peak(1500..out.len()) > 0.95);
    }

    #[test]
    fn fader_pre_roll_delays_the_start() {
        let ctl = GainCtl::new(1.0);
        let src = square(1);
        let plain = src.clone().count();
        let out: Vec<f32> = Fader::new(src, ctl, 0.25).collect();
        assert_eq!(out.len(), plain + 250);
        assert!(out[..250].iter().all(|s| *s == 0.0));
        assert!(out[300..].iter().any(|s| s.abs() > 0.5));
    }
}
