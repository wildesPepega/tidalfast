//! Discord Rich Presence over the local IPC socket.
//!
//! Runs on its own thread. Commands from the UI only change the thread's
//! picture of what is playing; a short debounce folds bursts (new track,
//! "playing" and a seek arrive within milliseconds) into one update, and
//! the activity is only sent when it actually differs from what Discord
//! already shows. A lost connection is re-established with a growing delay.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

/// Discord application id of tidalfast.
const APP_ID: &str = "1562542679034961980";

/// Commands arriving within this window are published together.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// Discord allows five updates per 20 s; stay below that.
const MIN_UPDATE_GAP: Duration = Duration::from_secs(4);
const RECONNECT_MIN: Duration = Duration::from_secs(2);
const RECONNECT_MAX: Duration = Duration::from_secs(30);
/// Position jumps smaller than this are not worth an update.
const DRIFT_SECS: i64 = 2;

pub enum DiscordCmd {
    Connect,
    Disconnect,
    SetTrack {
        title: String,
        artist: String,
        album: String,
        art_url: String,
        duration: f64,
        url: String,
        quality: String,
    },
    SetPlaying {
        playing: bool,
        position: f64,
    },
    /// The listener jumped to another position in the current track.
    Seek {
        position: f64,
    },
    Clear,
}

#[derive(Default, Clone, PartialEq)]
struct Track {
    title: String,
    artist: String,
    album: String,
    art_url: String,
    url: String,
    quality: String,
    duration: i64,
}

/// What the UI says is happening.
#[derive(Default)]
struct Now {
    track: Track,
    playing: bool,
    /// Epoch second at which position 0 of the track was (or would be) reached.
    start_epoch: i64,
}

impl Now {
    fn visible(&self) -> bool {
        self.playing && !self.track.title.is_empty()
    }
}

/// What Discord currently shows.
struct Shown {
    track: Track,
    start_epoch: i64,
    at: Instant,
}

pub struct Discord {
    tx: Sender<DiscordCmd>,
}

impl Discord {
    pub fn spawn(enabled: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tidalfast-discord".into())
            .spawn(move || Worker::new(enabled).run(rx))
            .expect("spawn discord thread");
        Self { tx }
    }

    pub fn send(&self, cmd: DiscordCmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Worker {
    client: Option<DiscordIpcClient>,
    want: bool,
    now: Now,
    shown: Option<Shown>,
    retry_at: Instant,
    backoff: Duration,
    /// An update held back by the rate limit.
    due: Option<Instant>,
}

impl Worker {
    fn new(want: bool) -> Self {
        Self {
            client: None,
            want,
            now: Now::default(),
            shown: None,
            retry_at: Instant::now(),
            backoff: RECONNECT_MIN,
            due: None,
        }
    }

    fn run(mut self, rx: Receiver<DiscordCmd>) {
        loop {
            // sleep until a command arrives or something is due
            let cmd = match self.next_wakeup() {
                Some(wait) => match rx.recv_timeout(wait) {
                    Ok(cmd) => Some(cmd),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(cmd) => Some(cmd),
                    Err(_) => break,
                },
            };
            if let Some(cmd) = cmd {
                self.apply(cmd);
                // fold the rest of the burst into the same update
                let deadline = Instant::now() + DEBOUNCE;
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(cmd) => self.apply(cmd),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => {
                            self.close();
                            return;
                        }
                    }
                }
            }
            self.sync();
        }
        self.close();
    }

    /// How long to sleep: `None` = until the next command.
    fn next_wakeup(&self) -> Option<Duration> {
        if !self.want {
            return None;
        }
        let now = Instant::now();
        let reconnect = (self.client.is_none() && (self.now.visible() || self.shown.is_some())).then_some(self.retry_at);
        [self.due, reconnect].into_iter().flatten().min().map(|t| t.saturating_duration_since(now))
    }

    fn apply(&mut self, cmd: DiscordCmd) {
        match cmd {
            DiscordCmd::Connect => {
                self.want = true;
                self.retry_at = Instant::now();
                self.backoff = RECONNECT_MIN;
            }
            DiscordCmd::Disconnect => {
                self.want = false;
                self.close();
            }
            DiscordCmd::SetTrack { title, artist, album, art_url, duration, url, quality } => {
                let track = Track { title, artist, album, art_url, url, quality, duration: duration.max(0.0) as i64 };
                if track != self.now.track {
                    self.now.track = track;
                    // a new track starts from the beginning
                    self.now.start_epoch = now_epoch();
                }
            }
            DiscordCmd::SetPlaying { playing, position } => {
                self.now.playing = playing;
                if playing {
                    self.now.start_epoch = now_epoch() - position.max(0.0) as i64;
                }
            }
            DiscordCmd::Seek { position } => {
                self.now.start_epoch = now_epoch() - position.max(0.0) as i64;
            }
            DiscordCmd::Clear => {
                self.now.track = Track::default();
            }
        }
    }

    /// Bring Discord in line with `self.now`.
    fn sync(&mut self) {
        if !self.want {
            return;
        }
        let visible = self.now.visible();
        if !visible {
            self.due = None;
        }
        if !visible && self.shown.is_none() {
            return;
        }
        if !self.ensure_connected() {
            return;
        }
        if !visible {
            let ok = self.client.as_mut().is_some_and(|c| c.clear_activity().is_ok());
            if ok {
                self.shown = None;
            } else {
                self.lost();
            }
            return;
        }
        if !self.needs_update() {
            self.due = None;
            return;
        }
        // don't flood Discord when the user skips through tracks
        if let Some(shown) = &self.shown {
            let since = shown.at.elapsed();
            if since < MIN_UPDATE_GAP {
                self.due = Some(Instant::now() + (MIN_UPDATE_GAP - since));
                return;
            }
        }
        self.due = None;
        if !self.publish() {
            // the socket may just have gone stale: one fresh attempt
            self.lost();
            if self.ensure_connected() && !self.publish() {
                self.lost();
            }
        }
    }

    fn needs_update(&self) -> bool {
        match &self.shown {
            None => true,
            Some(shown) => {
                shown.track != self.now.track || (shown.start_epoch - self.now.start_epoch).abs() > DRIFT_SECS
            }
        }
    }

    fn ensure_connected(&mut self) -> bool {
        if self.client.is_some() {
            return true;
        }
        if Instant::now() < self.retry_at {
            return false;
        }
        let mut client = DiscordIpcClient::new(APP_ID);
        match client.connect() {
            Ok(()) => {
                log::debug!("Discord verbunden");
                self.client = Some(client);
                self.backoff = RECONNECT_MIN;
                true
            }
            Err(e) => {
                log::debug!("Discord verbinden fehlgeschlagen: {e}");
                self.retry_at = Instant::now() + self.backoff;
                self.backoff = (self.backoff * 2).min(RECONNECT_MAX);
                false
            }
        }
    }

    /// Drop a broken connection; the next sync reconnects and republishes.
    fn lost(&mut self) {
        self.close();
        self.retry_at = Instant::now();
    }

    fn close(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.clear_activity().ok();
            client.close().ok();
        }
        self.shown = None;
    }

    fn publish(&mut self) -> bool {
        let Some(client) = self.client.as_mut() else { return false };
        let act = build_activity(&self.now);
        if client.set_activity(act).is_err() {
            return false;
        }
        self.shown = Some(Shown { track: self.now.track.clone(), start_epoch: self.now.start_epoch, at: Instant::now() });
        true
    }
}

/// Discord rejects texts shorter than 2 or longer than 128 characters.
fn fit(text: &str) -> String {
    let mut out: String = text.chars().take(128).collect();
    if out.chars().count() < 2 {
        out.push('\u{200b}');
    }
    out
}

fn build_activity(now: &Now) -> activity::Activity<'static> {
    let t = &now.track;
    let mut act = activity::Activity::new()
        .activity_type(activity::ActivityType::Listening)
        .name(if t.artist.is_empty() { fit(&t.title) } else { fit(&format!("{} by {}", t.title, t.artist)) })
        .status_display_type(activity::StatusDisplayType::Name)
        .details(fit(&t.title));
    if !t.artist.is_empty() {
        act = act.state(fit(&format!("by {}", t.artist)));
    }

    if now.start_epoch > 0 {
        let mut timestamps = activity::Timestamps::new().start(now.start_epoch);
        if t.duration > 0 {
            timestamps = timestamps.end(now.start_epoch + t.duration);
        }
        act = act.timestamps(timestamps);
    }

    if !t.art_url.is_empty() {
        let mut assets = activity::Assets::new().large_image(t.art_url.clone());
        let hover = match (t.album.is_empty(), t.quality.is_empty()) {
            (false, false) => format!("{} · {}", t.album, t.quality),
            (false, true) => t.album.clone(),
            (true, false) => t.quality.clone(),
            (true, true) => String::new(),
        };
        if !hover.is_empty() {
            assets = assets.large_text(fit(&hover));
        }
        act = act.assets(assets);
    }

    if !t.url.is_empty() {
        act = act.buttons(vec![activity::Button::new("Listen on TIDAL", t.url.clone())]);
    }
    act
}

fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}
