//! Audio playback engine.
//!
//! Runs rodio on a dedicated thread (a cpal output stream is not `Send` on
//! every platform) and drives it from the UI through a command channel. Every
//! track plays on its own "deck" (a rodio player on the shared mixer), so the
//! next track can start while the current one fades out. The next track is
//! downloaded in the background ahead of time.

use std::io::Cursor;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, MixerDeviceSink, Player, Source};

use crate::eq::{EqShared, EqSource};
use crate::model::Track;
use crate::transition::{self, Fader, GainCtl, Plan, Transition};

pub enum AudioCmd {
    /// Play a track now (replaces whatever is playing).
    Load {
        url: String,
        track: Box<Track>,
        play: bool,
        quality: String,
        /// Begin this many seconds into the song (0: at the start).
        start_at: f64,
    },
    /// Get the next track ready; the engine switches to it on its own.
    Preload {
        url: String,
        track: Box<Track>,
        quality: String,
    },
    /// Like `Load` / `Preload`, but from the disk cache (no stream URL needed).
    LoadCached {
        track: Box<Track>,
        play: bool,
        start_at: f64,
    },
    PreloadCached {
        track: Box<Track>,
    },
    ClearPreload,
    /// Silence whatever plays right now (a skip is under way) without closing the device.
    Halt,
    SetTransition(Transition),
    Pause,
    Toggle,
    Stop,
    Seek(f64),
    SetVolume(f32),
    SetDevice(String),
}

pub enum AudioEv {
    Devices(Vec<OutputDevice>),
    Started {
        track: Box<Track>,
        duration: f64,
        quality: String,
        /// The engine moved on to the preloaded track by itself.
        auto: bool,
        /// Length of the crossfade into this track in seconds (0: a cut).
        fade: f32,
        /// Whether it is playing (not paused) right now: what the engine really did.
        playing: bool,
        /// Where in the song it began (0 unless a song is picked up again).
        start: f64,
    },
    Progress {
        position: f64,
        duration: f64,
    },
    Paused(bool),
    /// A song that was meant to come from the cache is not (any more) there: look it up normally.
    CacheMiss {
        track_id: u64,
        preload: bool,
    },
    Ended,
    Error(String),
}

/// Handle used by the UI thread.
pub struct Audio {
    tx: Sender<AudioCmd>,
    rx: Receiver<AudioEv>,
}

impl Audio {
    pub fn spawn(volume: f32, device: String, eq: Arc<EqShared>, transition: Transition) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("oxytide-audio".into())
            .spawn(move || audio_thread(cmd_rx, ev_tx, volume, device, eq, transition))
            .expect("spawn audio thread");
        Self {
            tx: cmd_tx,
            rx: ev_rx,
        }
    }

    pub fn send(&self, cmd: AudioCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn try_recv(&self) -> Option<AudioEv> {
        self.rx.try_recv().ok()
    }
}

/// A track that is playing (or fading out).
struct Deck {
    id: u64,
    player: Player,
    gain: Arc<GainCtl>,
    track: Box<Track>,
    quality: String,
    duration: f64,
    /// Track time minus the player's clock (pre-roll silence).
    offset: f64,
}

impl Deck {
    fn position(&self) -> f64 {
        self.player.get_pos().as_secs_f64() + self.offset
    }
}

/// How early a transition is prepared; the gap is filled with exact silence
/// so the next track starts on the planned sample, not on the next tick.
const LOOKAHEAD: f64 = 0.12;

/// The next track, while it downloads.
struct Pending {
    id: u64,
    track: Box<Track>,
    quality: String,
    bytes: Option<Vec<u8>>,
}

/// Results of background work.
enum Job {
    Downloaded { id: u64, result: Result<Vec<u8>, String> },
    /// The track the user asked to play right now.
    Loaded {
        id: u64,
        result: Result<Vec<u8>, String>,
        track: Box<Track>,
        quality: String,
        play: bool,
        from_cache: bool,
        start_at: f64,
    },
    /// A cached song could not be read.
    Miss { id: u64, track_id: u64, preload: bool },
}

struct Engine {
    sink: Option<MixerDeviceSink>,
    device_name: String,
    volume: f32,
    eq: Arc<EqShared>,
    transition: Transition,
    cur: Option<Deck>,
    /// Decks fading out after a transition.
    fading: Vec<Deck>,
    next: Option<Pending>,
    /// The latest `Load` still downloading; older ones are dropped.
    loading: Option<u64>,
    jobs_tx: Sender<Job>,
    jobs_rx: Receiver<Job>,
    next_id: u64,
    last_progress: Instant,
}

impl Engine {
    fn new(volume: f32, device_name: String, eq: Arc<EqShared>, transition: Transition) -> Self {
        let (jobs_tx, jobs_rx) = mpsc::channel();
        Self {
            sink: None,
            device_name,
            volume,
            eq,
            transition,
            cur: None,
            fading: Vec::new(),
            next: None,
            loading: None,
            jobs_tx,
            jobs_rx,
            next_id: 0,
            last_progress: Instant::now(),
        }
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Open the output device on first use.
    fn ensure_device(&mut self) -> Result<()> {
        if self.sink.is_none() {
            self.sink = Some(open_device(&self.device_name)?);
        }
        Ok(())
    }

    /// Decode a track onto a new, paused deck, starting after `pre_roll`
    /// seconds of silence.
    fn new_deck(
        &mut self,
        bytes: Vec<u8>,
        track: Box<Track>,
        quality: String,
        start_gain: f32,
        pre_roll: f64,
    ) -> Result<Deck> {
        self.ensure_device()?;
        let decoder = Decoder::new(Cursor::new(bytes)).map_err(|e| anyhow!("decode failed: {e}"))?;
        let duration = decoder.total_duration().map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let gain = GainCtl::new(start_gain);
        let source = EqSource::new(Fader::new(decoder, gain.clone(), pre_roll), self.eq.clone());
        let player = Player::connect_new(self.sink.as_ref().unwrap().mixer());
        player.pause();
        player.set_volume(self.volume);
        player.append(source);
        Ok(Deck {
            id: self.new_id(),
            player,
            gain,
            track,
            quality,
            duration,
            offset: -pre_roll,
        })
    }

    /// Start playing `track` as soon as its audio has downloaded. The
    /// download runs in the background, so the engine stays responsive and
    /// a newer `Load` simply replaces this one.
    fn load(&mut self, url: String, track: Box<Track>, quality: String, play: bool, start_at: f64) {
        self.stop_all();
        self.next = None;
        let id = self.new_id();
        self.loading = Some(id);
        let tx = self.jobs_tx.clone();
        std::thread::spawn(move || {
            let result = fetch(&url, track.id, &quality).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Job::Loaded { id, result, track, quality, play, from_cache: false, start_at });
        });
    }

    /// Start `track` from the disk cache.
    fn load_cached(&mut self, track: Box<Track>, play: bool, start_at: f64) {
        self.stop_all();
        self.next = None;
        let id = self.new_id();
        self.loading = Some(id);
        let tx = self.jobs_tx.clone();
        std::thread::spawn(move || {
            let job = match crate::audiocache::global().and_then(|c| c.read(track.id)) {
                Some((bytes, quality)) => Job::Loaded { id, result: Ok(bytes), track, quality, play, from_cache: true, start_at },
                None => Job::Miss { id, track_id: track.id, preload: false },
            };
            let _ = tx.send(job);
        });
    }

    fn preload_cached(&mut self, track: Box<Track>) {
        let id = self.new_id();
        let quality = crate::audiocache::global().and_then(|c| c.find(track.id)).map(|(_, q)| q).unwrap_or_default();
        self.next = Some(Pending { id, track: track.clone(), quality, bytes: None });
        let tx = self.jobs_tx.clone();
        std::thread::spawn(move || {
            let job = match crate::audiocache::global().and_then(|c| c.read(track.id)) {
                Some((bytes, _)) => Job::Downloaded { id, result: Ok(bytes) },
                None => Job::Miss { id, track_id: track.id, preload: true },
            };
            let _ = tx.send(job);
        });
    }

    fn preload(&mut self, url: String, track: Box<Track>, quality: String) {
        let id = self.new_id();
        self.next = Some(Pending { id, track, quality, bytes: None });
        let tx = self.jobs_tx.clone();
        let track_id = self.next.as_ref().map(|n| n.track.id).unwrap_or(0);
        let quality = self.next.as_ref().map(|n| n.quality.clone()).unwrap_or_default();
        std::thread::spawn(move || {
            let result = fetch(&url, track_id, &quality).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Job::Downloaded { id, result });
        });
    }

    fn handle_job(&mut self, job: Job, ev: &Sender<AudioEv>) {
        match job {
            Job::Miss { id, track_id, preload } => {
                if preload {
                    if self.next.as_ref().map(|n| n.id) == Some(id) {
                        self.next = None;
                    }
                } else if self.loading == Some(id) {
                    self.loading = None;
                } else {
                    return;
                }
                let _ = ev.send(AudioEv::CacheMiss { track_id, preload });
            }
            Job::Loaded { id, result, track, quality, play, from_cache, start_at } => {
                let track_id = track.id;
                if self.loading != Some(id) {
                    return;
                }
                self.loading = None;
                let deck = result
                    .map_err(|e| anyhow!(e))
                    .and_then(|bytes| self.new_deck(bytes, track, quality, 1.0, 0.0));
                match deck {
                    Ok(mut deck) => {
                        // a song that is picked up again: be at its place before the first sound
                        let mut began = 0.0;
                        if start_at > 0.5 && deck.player.try_seek(Duration::from_secs_f64(start_at)).is_ok() {
                            deck.offset = 0.0;
                            began = start_at;
                        }
                        if play {
                            deck.player.play();
                        }
                        let _ = ev.send(AudioEv::Started {
                            track: deck.track.clone(),
                            duration: deck.duration,
                                            quality: deck.quality.clone(),
                            auto: false,
                            fade: 0.0,
                            playing: play,
                            start: began,
                        });
                        self.cur = Some(deck);
                    }
                    Err(e) if from_cache => {
                        // a damaged file in the cache: forget it and get the song again
                        log::warn!("cached song unusable: {e:#}");
                        if let Some(cache) = crate::audiocache::global() {
                            cache.remove(track_id);
                        }
                        let _ = ev.send(AudioEv::CacheMiss { track_id, preload: false });
                    }
                    Err(e) => {
                        log::warn!("playback failed: {e:#}");
                        let _ = ev.send(AudioEv::Error(format!("Playback failed: {e}")));
                    }
                }
            }
            Job::Downloaded { id, result } => {
                if self.next.as_ref().map(|n| n.id) != Some(id) {
                    return;
                }
                match result {
                    Ok(bytes) => {
                        if let Some(next) = &mut self.next {
                            next.bytes = Some(bytes);
                        }
                    }
                    Err(e) => {
                        log::warn!("preload failed: {e}");
                        self.next = None;
                    }
                }
            }
        }
    }

    /// Switch to the preloaded track according to `plan`; it enters after
    /// `pre_roll` (real) seconds, exactly at the planned point.
    fn start_next(&mut self, plan: Plan, pre_roll: f64, ev: &Sender<AudioEv>) {
        let Some(pending) = self.next.take() else { return };
        let Some(bytes) = pending.bytes else { return };
        let fade = plan.fade;
        let start = if fade > 0.0 { 0.0 } else { 1.0 };
        let deck = self.new_deck(bytes, pending.track, pending.quality, start, pre_roll);
        let mut deck = match deck {
            Ok(deck) => deck,
            Err(e) => {
                log::warn!("transition failed: {e:#}");
                return;
            }
        };
        deck.id = pending.id;
        deck.gain.fade_to(1.0, fade, 0.0);
        deck.player.play();
        if let Some(old) = self.cur.take() {
            if fade > 0.0 {
                // the outgoing fade starts when the new track actually enters
                old.gain.fade_to(0.0, fade, pre_roll as f32);
                self.fading.push(old);
            } else {
                old.player.stop();
            }
        }
        let _ = ev.send(AudioEv::Started {
            track: deck.track.clone(),
            duration: deck.duration,
            quality: deck.quality.clone(),
            auto: true,
            fade,
            playing: true,
            start: 0.0,
        });
        self.cur = Some(deck);
    }

    /// Called every few milliseconds: background results, fades, transitions, progress.
    fn tick(&mut self, ev: &Sender<AudioEv>) {
        while let Ok(job) = self.jobs_rx.try_recv() {
            self.handle_job(job, ev);
        }
        self.fading.retain(|d| {
            let done = d.gain.faded_out() || d.player.empty();
            if done {
                d.player.stop();
            }
            !done
        });

        let Some(cur) = &self.cur else { return };
        if cur.player.is_paused() {
            return;
        }
        let pos = cur.position();
        let ended = cur.player.empty();
        let plan = self
            .next
            .as_ref()
            .filter(|n| n.bytes.is_some())
            .map(|_| transition::plan(self.transition, cur.duration));

        if let Some(plan) = plan {
            let timed = plan.fade > 0.0 && cur.duration > 0.0;
            if ended || (timed && pos >= plan.start_at - LOOKAHEAD) {
                let pre_roll = if ended { 0.0 } else { (plan.start_at - pos).clamp(0.0, LOOKAHEAD) };
                self.start_next(plan, pre_roll, ev);
                return;
            }
        }

        if ended {
            // Report the end once; the UI decides what comes next.
            self.cur = None;
            let _ = ev.send(AudioEv::Ended);
            return;
        }
        if self.last_progress.elapsed() >= Duration::from_millis(200) {
            self.last_progress = Instant::now();
            let _ = ev.send(AudioEv::Progress { position: pos, duration: cur.duration });
        }
    }

    fn players(&self) -> impl Iterator<Item = &Player> {
        self.cur.iter().chain(self.fading.iter()).map(|d| &d.player)
    }

    fn stop_all(&mut self) {
        for player in self.players() {
            player.stop();
        }
        self.cur = None;
        self.fading.clear();
    }

    fn release_device(&mut self) {
        self.stop_all();
        self.sink = None;
        self.eq.clear_meters();
    }
}

fn audio_thread(
    rx: Receiver<AudioCmd>,
    ev: Sender<AudioEv>,
    volume: f32,
    device: String,
    eq: Arc<EqShared>,
    transition: Transition,
) {
    let mut engine = Engine::new(volume, device, eq, transition);
    let _ = ev.send(AudioEv::Devices(list_devices()));

    loop {
        // tick fast while playing so transitions start on time
        let wait = if engine.cur.is_some() { 25 } else { 250 };
        match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(cmd) => handle(cmd, &mut engine, &ev),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        engine.tick(&ev);
    }
}

fn handle(cmd: AudioCmd, engine: &mut Engine, ev: &Sender<AudioEv>) {
    match cmd {
        AudioCmd::Load {
            url,
            track,
            play,
            quality,
            start_at,
        } => engine.load(url, track, quality, play, start_at),
        AudioCmd::Preload { url, track, quality } => engine.preload(url, track, quality),
        AudioCmd::LoadCached { track, play, start_at } => engine.load_cached(track, play, start_at),
        AudioCmd::PreloadCached { track } => engine.preload_cached(track),
        AudioCmd::ClearPreload => engine.next = None,
        AudioCmd::SetTransition(t) => {
            engine.transition = t;
        }
        AudioCmd::Pause => {
            if engine.cur.is_some() {
                for player in engine.players() {
                    player.pause();
                }
                let _ = ev.send(AudioEv::Paused(true));
            }
        }
        AudioCmd::Toggle => {
            if let Some(cur) = &engine.cur {
                let resume = cur.player.is_paused();
                for player in engine.players() {
                    if resume {
                        player.play();
                    } else {
                        player.pause();
                    }
                }
                let _ = ev.send(AudioEv::Paused(!resume));
            }
        }
        AudioCmd::Halt => {
            engine.next = None;
            engine.loading = None;
            engine.stop_all();
        }
        AudioCmd::Stop => {
            engine.next = None;
            engine.loading = None;
            engine.release_device();
        }
        AudioCmd::Seek(pos) => {
            // a seek ends any running fade
            for old in engine.fading.drain(..) {
                old.player.stop();
            }
            if let Some(cur) = &mut engine.cur {
                if cur.player.try_seek(Duration::from_secs_f64(pos.max(0.0))).is_ok() {
                    // the player's clock is now track time
                    cur.offset = 0.0;
                }
            }
        }
        AudioCmd::SetVolume(v) => {
            engine.volume = v;
            for player in engine.players() {
                player.set_volume(v);
            }
        }
        AudioCmd::SetDevice(name) => {
            engine.device_name = name;
            engine.release_device();
        }
    }
}

/// An output the user can pick: `id` is stored in the settings, `label` is shown.
#[derive(Clone, Debug)]
pub struct OutputDevice {
    pub id: String,
    pub label: String,
}

const CPAL_PREFIX: &str = "cpal:";

/// Output devices with their real names. On PipeWire/PulseAudio systems these are
/// the sinks ("GoXLRMini Music", …); otherwise the raw cpal devices are listed.
pub fn list_devices() -> Vec<OutputDevice> {
    let sinks = list_sinks();
    if !sinks.is_empty() {
        return sinks;
    }
    let host = cpal::default_host();
    let mut seen = std::collections::HashSet::new();
    host.output_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok())
                .filter(|desc| seen.insert(desc.to_string()))
                .map(|desc| OutputDevice {
                    id: format!("{CPAL_PREFIX}{desc}"),
                    label: device_label(&desc),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The name Windows shows for the device ("Music (GoXLR Mini)"), without the
/// type and interface suffixes cpal appends ("[Virtual] via Line").
fn device_label(desc: &cpal::DeviceDescription) -> String {
    let name = desc.name().to_string();
    desc.extended()
        .iter()
        .find(|line| line.starts_with(&name) || line.contains(&name))
        .cloned()
        .unwrap_or(name)
}

fn list_sinks() -> Vec<OutputDevice> {
    let Ok(out) = std::process::Command::new("pactl")
        .args(["-f", "json", "list", "sinks"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    value
        .as_array()
        .map(|sinks| {
            sinks
                .iter()
                .filter_map(|sink| {
                    let id = sink.get("name")?.as_str()?.to_string();
                    let label = sink
                        .get("description")
                        .and_then(|d| d.as_str())
                        .filter(|d| !d.is_empty())
                        .unwrap_or(&id)
                        .to_string();
                    Some(OutputDevice { id, label })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn open_device(name: &str) -> Result<MixerDeviceSink> {
    let host = cpal::default_host();
    let device = if let Some(cpal_name) = name.strip_prefix(CPAL_PREFIX) {
        std::env::remove_var("PIPEWIRE_NODE");
        std::env::remove_var("PULSE_SINK");
        host.output_devices()
            .ok()
            .and_then(|mut devices| {
                devices.find(|d| {
                    d.description()
                        .map(|s| s.to_string() == cpal_name)
                        .unwrap_or(false)
                })
            })
            .or_else(|| host.default_output_device())
    } else {
        // Route the default (PipeWire/Pulse) device to the chosen sink.
        if name.is_empty() {
            std::env::remove_var("PIPEWIRE_NODE");
            std::env::remove_var("PULSE_SINK");
        } else {
            std::env::set_var("PIPEWIRE_NODE", name);
            std::env::set_var("PULSE_SINK", name);
        }
        host.default_output_device()
    }
    .ok_or_else(|| anyhow!("no output device found"))?;

    rodio::DeviceSinkBuilder::from_device(device)
        .context("opening output stream")?
        .open_stream()
        .context("starting output stream")
}

/// Download a song and keep a copy in the cache.
fn fetch(url: &str, track_id: u64, quality: &str) -> Result<Vec<u8>> {
    let bytes = download(url)?;
    if crate::audiocache::enabled() && track_id != 0 {
        if let Some(cache) = crate::audiocache::global() {
            cache.store(track_id, quality, &bytes);
            cache.prune_to_limit();
        }
    }
    Ok(bytes)
}

fn download(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("oxytide/0.1")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .build()?;
    let resp = client.get(url).send().context("fetching audio stream")?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "Stream download failed ({})",
            resp.status()
        ));
    }
    Ok(resp.bytes().context("reading audio data")?.to_vec())
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::io::{Read, Write};

    /// A one-shot HTTP server that answers every request with `body`.
    fn serve(body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}/song.flac")
    }

    #[test]
    fn a_downloaded_song_is_kept_and_found_again() {
        let dir = std::env::temp_dir().join(format!("oxytide-fetch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CACHE_HOME", &dir);
        // (the key of this installation is made next to the settings: keep the real ones out of it)
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        crate::audiocache::configure(true, 1 << 30);
        let url = serve(vec![7u8; 5000]);

        let bytes = fetch(&url, 31337, "LOSSLESS").unwrap();
        assert_eq!(bytes.len(), 5000);
        let cache = crate::audiocache::global().unwrap();
        let (cached, quality) = cache.read(31337).expect("kept");
        assert_eq!((cached.len(), quality.as_str()), (5000, "LOSSLESS"));

        // switched off: nothing is kept
        crate::audiocache::configure(false, 1 << 30);
        fetch(&url, 99, "LOSSLESS").unwrap();
        assert!(!cache.has(99));
        crate::audiocache::configure(true, 1 << 30);
        cache.clear();
    }
}
