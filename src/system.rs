//! Desktop integration: media keys / system media controls and the tray icon.
//!
//! - Media keys and the "now playing" widgets of the desktop go through
//!   `souvlaki`: MPRIS on Linux (KDE, GNOME, Waybar's `mpris` module,
//!   `playerctl`, …) and the System Media Transport Controls on Windows.
//! - The tray icon is a StatusNotifierItem on Linux (KDE, Waybar, GNOME with
//!   the AppIndicator extension, …) and a notification-area icon on Windows.
//!
//! Everything reports back as [`SysEvent`]s, which the app polls once per
//! frame; the senders wake the UI so it also reacts while hidden.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use crate::desktop::ICON_PNG;

#[derive(Debug, Clone, PartialEq)]
pub enum SysEvent {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    /// Jump by this many seconds (negative = back).
    SeekBy(f64),
    SeekTo(f64),
    /// Bring the window back (from the tray or the desktop's "raise").
    Show,
    /// Tray icon clicked: show the window, or hide it when it is in front.
    ToggleWindow,
    Quit,
}

/// What the desktop should display for the current track.
#[derive(Clone, PartialEq, Default)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_url: String,
    pub duration: f64,
}

#[derive(Clone)]
struct Notifier {
    tx: Sender<SysEvent>,
    ctx: egui::Context,
}

impl Notifier {
    fn send(&self, event: SysEvent) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

pub struct System {
    notifier: Notifier,
    rx: Receiver<SysEvent>,
    hwnd: Option<*mut std::ffi::c_void>,
    media: Option<MediaControls>,
    tray: Option<tray::Tray>,
    /// Last state pushed to the desktop, to resend it after (re)enabling.
    now: Option<NowPlaying>,
    playback: Option<(bool, f64)>,
}

impl System {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            notifier: Notifier { tx, ctx: cc.egui_ctx.clone() },
            rx,
            hwnd: window_handle(cc),
            media: None,
            tray: None,
            now: None,
            playback: None,
        }
    }

    pub fn poll(&self) -> Vec<SysEvent> {
        self.rx.try_iter().collect()
    }

    pub fn tray_active(&self) -> bool {
        self.tray.is_some()
    }

    /// Turn the media keys / system media controls on or off.
    pub fn set_media(&mut self, on: bool) {
        if !on {
            self.media = None;
            return;
        }
        if self.media.is_some() {
            return;
        }
        // on Windows the controls need a window; without one they can't work
        #[cfg(windows)]
        if self.hwnd.is_none() {
            log::warn!("no window handle for the media controls");
            return;
        }
        let config = PlatformConfig {
            dbus_name: "tidalfast",
            display_name: "tidalfast",
            hwnd: self.hwnd,
        };
        let mut controls = match MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("media controls unavailable: {e:?}");
                return;
            }
        };
        let notifier = self.notifier.clone();
        let attached = controls.attach(move |event| {
            let sys = match event {
                MediaControlEvent::Play => SysEvent::Play,
                MediaControlEvent::Pause => SysEvent::Pause,
                MediaControlEvent::Toggle => SysEvent::Toggle,
                MediaControlEvent::Next => SysEvent::Next,
                MediaControlEvent::Previous => SysEvent::Previous,
                MediaControlEvent::Stop => SysEvent::Stop,
                MediaControlEvent::Seek(SeekDirection::Forward) => SysEvent::SeekBy(10.0),
                MediaControlEvent::Seek(SeekDirection::Backward) => SysEvent::SeekBy(-10.0),
                MediaControlEvent::SeekBy(SeekDirection::Forward, d) => SysEvent::SeekBy(d.as_secs_f64()),
                MediaControlEvent::SeekBy(SeekDirection::Backward, d) => SysEvent::SeekBy(-d.as_secs_f64()),
                MediaControlEvent::SetPosition(MediaPosition(d)) => SysEvent::SeekTo(d.as_secs_f64()),
                MediaControlEvent::Raise => SysEvent::Show,
                MediaControlEvent::Quit => SysEvent::Quit,
                MediaControlEvent::SetVolume(_) | MediaControlEvent::OpenUri(_) => return,
            };
            notifier.send(sys);
        });
        if let Err(e) = attached {
            log::warn!("media controls unavailable: {e:?}");
            return;
        }
        self.media = Some(controls);
        self.push_now();
        self.push_playback();
    }

    /// Show or remove the tray icon.
    pub fn set_tray(&mut self, on: bool) {
        if !on {
            self.tray = None;
        } else if self.tray.is_none() {
            self.tray = tray::Tray::start(self.notifier.clone());
        }
    }

    /// The track that is loaded (or `None` when nothing is).
    pub fn set_track(&mut self, now: Option<NowPlaying>) {
        if self.now == now {
            return;
        }
        self.now = now;
        self.push_now();
    }

    /// Playing/paused and the current position in seconds.
    pub fn set_playback(&mut self, playing: bool, position: f64) {
        self.playback = Some((playing, position));
        self.push_playback();
        if let Some(tray) = &self.tray {
            tray.set_playing(playing);
        }
    }

    fn push_now(&mut self) {
        let (Some(media), Some(now)) = (self.media.as_mut(), self.now.as_ref()) else { return };
        let result = media.set_metadata(MediaMetadata {
            title: non_empty(&now.title),
            artist: non_empty(&now.artist),
            album: non_empty(&now.album),
            cover_url: non_empty(&now.cover_url),
            duration: (now.duration > 0.0).then(|| Duration::from_secs_f64(now.duration)),
        });
        if let Err(e) = result {
            log::debug!("media metadata failed: {e:?}");
        }
    }

    fn push_playback(&mut self) {
        let Some(media) = self.media.as_mut() else { return };
        let playback = match (&self.now, self.playback) {
            (None, _) | (_, None) => MediaPlayback::Stopped,
            (Some(_), Some((playing, position))) => {
                let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
                if playing {
                    MediaPlayback::Playing { progress }
                } else {
                    MediaPlayback::Paused { progress }
                }
            }
        };
        if let Err(e) = media.set_playback(playback) {
            log::debug!("media playback state failed: {e:?}");
        }
    }
}

fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

#[cfg(windows)]
fn window_handle(cc: &eframe::CreationContext<'_>) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(windows))]
fn window_handle(_cc: &eframe::CreationContext<'_>) -> Option<*mut std::ffi::c_void> {
    None
}

/// The app icon as RGBA pixels.
fn icon_rgba(size: u32) -> Option<(Vec<u8>, u32, u32)> {
    let image = image::load_from_memory(ICON_PNG).ok()?;
    let image = image.resize(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
    let (w, h) = image.dimensions();
    Some((image.into_raw(), w, h))
}

// ------------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
mod tray {
    //! StatusNotifierItem over D-Bus (no GTK needed).

    use ksni::menu::{MenuItem, StandardItem};
    use ksni::TrayMethods;

    use super::{icon_rgba, Notifier, SysEvent};

    struct Item {
        notifier: Notifier,
        playing: bool,
    }

    impl ksni::Tray for Item {
        fn id(&self) -> String {
            "tidalfast".into()
        }

        fn title(&self) -> String {
            "tidalfast".into()
        }

        fn category(&self) -> ksni::Category {
            ksni::Category::ApplicationStatus
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let Some((mut data, width, height)) = icon_rgba(64) else { return Vec::new() };
            // RGBA → ARGB32 (network byte order)
            for px in data.chunks_exact_mut(4) {
                px.rotate_right(1);
            }
            vec![ksni::Icon { width: width as i32, height: height as i32, data }]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip { title: "tidalfast".into(), ..Default::default() }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.notifier.send(SysEvent::ToggleWindow);
        }

        fn secondary_activate(&mut self, _x: i32, _y: i32) {
            self.notifier.send(SysEvent::Toggle);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let entry = |label: &str, event: SysEvent| {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |this: &mut Self| this.notifier.send(event.clone())),
                    ..Default::default()
                }
                .into()
            };
            vec![
                entry("Show / hide window", SysEvent::ToggleWindow),
                MenuItem::Separator,
                entry(if self.playing { "Pause" } else { "Play" }, SysEvent::Toggle),
                entry("Next track", SysEvent::Next),
                entry("Previous track", SysEvent::Previous),
                MenuItem::Separator,
                entry("Quit", SysEvent::Quit),
            ]
        }
    }

    /// The tray icon lives on its own thread with its own async runtime.
    pub struct Tray {
        playing: tokio::sync::mpsc::UnboundedSender<bool>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl Tray {
        pub fn start(notifier: Notifier) -> Option<Self> {
            let (playing_tx, mut playing_rx) = tokio::sync::mpsc::unbounded_channel::<bool>();
            let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
            let spawned = std::thread::Builder::new().name("tidalfast-tray".into()).spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
                runtime.block_on(async move {
                    let tray = Item { notifier, playing: false };
                    let handle = match tray.spawn().await {
                        Ok(handle) => handle,
                        Err(e) => {
                            log::warn!("tray icon unavailable: {e}");
                            return;
                        }
                    };
                    loop {
                        tokio::select! {
                            _ = &mut stop_rx => break,
                            state = playing_rx.recv() => match state {
                                Some(playing) => { handle.update(|t| t.playing = playing).await; }
                                None => break,
                            },
                        }
                    }
                    handle.shutdown().await;
                });
            });
            spawned.ok()?;
            Some(Self { playing: playing_tx, stop: Some(stop_tx) })
        }

        pub fn set_playing(&self, playing: bool) {
            let _ = self.playing.send(playing);
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
        }
    }
}

// ----------------------------------------------------------------- Windows

#[cfg(windows)]
mod tray {
    //! Notification-area icon. Created on the UI thread, whose message loop
    //! also delivers its events.

    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::{icon_rgba, Notifier, SysEvent};

    pub struct Tray {
        _icon: TrayIcon,
        toggle: MenuItem,
    }

    impl Tray {
        pub fn start(notifier: Notifier) -> Option<Self> {
            let menu = Menu::new();
            let show = MenuItem::new("Show / hide window", true, None);
            let toggle = MenuItem::new("Play", true, None);
            let next = MenuItem::new("Next track", true, None);
            let previous = MenuItem::new("Previous track", true, None);
            let quit = MenuItem::new("Quit", true, None);
            menu.append_items(&[&show, &PredefinedMenuItem::separator(), &toggle, &next, &previous, &PredefinedMenuItem::separator(), &quit])
                .ok()?;

            let actions = [
                (show.id().clone(), SysEvent::ToggleWindow),
                (toggle.id().clone(), SysEvent::Toggle),
                (next.id().clone(), SysEvent::Next),
                (previous.id().clone(), SysEvent::Previous),
                (quit.id().clone(), SysEvent::Quit),
            ];
            let menu_notifier = notifier.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if let Some((_, action)) = actions.iter().find(|(id, _)| *id == event.id) {
                    menu_notifier.send(action.clone());
                }
            }));
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    notifier.send(SysEvent::ToggleWindow);
                }
            }));

            let (rgba, w, h) = icon_rgba(32)?;
            let icon = tray_icon::Icon::from_rgba(rgba, w, h).ok()?;
            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("tidalfast")
                .with_icon(icon)
                .build()
                .map_err(|e| log::warn!("tray icon unavailable: {e}"))
                .ok()?;
            Some(Self { _icon: tray, toggle })
        }

        pub fn set_playing(&self, playing: bool) {
            self.toggle.set_text(if playing { "Pause" } else { "Play" });
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod tray {
    use super::Notifier;

    pub struct Tray;

    impl Tray {
        pub fn start(_notifier: Notifier) -> Option<Self> {
            None
        }

        pub fn set_playing(&self, _playing: bool) {}
    }
}
