//! Animated covers: for some albums TIDAL has a short looping video (H.264 in an
//! MP4) next to the cover picture. A background thread downloads it, decodes it
//! frame by frame with OpenH264 and hands the newest picture over; the UI turns
//! that into a texture.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use openh264::decoder::{DecodeOptions, DecodedYUV, Decoder, Flush};
use openh264::formats::YUVSource;

/// One picture of the video.
pub struct Frame {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

/// Where TIDAL keeps the video of an animated cover (`size` 640 is plenty).
pub fn video_url(id: &str, size: u32) -> String {
    format!("https://resources.tidal.com/videos/{}/{size}x{size}.mp4", id.replace('-', "/"))
}

struct Shared {
    frame: Mutex<Option<Frame>>,
    stop: AtomicBool,
    paused: AtomicBool,
}

/// A running animation. Dropping it stops the thread.
pub struct Player {
    shared: Arc<Shared>,
}

impl Player {
    pub fn start(url: String, ctx: eframe::egui::Context) -> Self {
        let shared = Arc::new(Shared { frame: Mutex::new(None), stop: AtomicBool::new(false), paused: AtomicBool::new(false) });
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name("oxytide-cover-video".into())
            .spawn(move || {
                if let Err(e) = run(&url, &thread_shared, &ctx) {
                    log::info!("animated cover not played: {e}");
                }
            })
            .ok();
        Self { shared }
    }

    /// The newest picture, if one arrived since the last call.
    pub fn take_frame(&self) -> Option<Frame> {
        self.shared.frame.lock().ok().and_then(|mut f| f.take())
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

// ------------------------------------------------------------ for the UI

/// The animation of one song with the texture its pictures are drawn into.
struct Active {
    track: u64,
    player: Player,
    tex: Option<eframe::egui::TextureHandle>,
}

/// The animated cover of the song that plays now (and the one it replaced, for the
/// duration of a transition).
#[derive(Default)]
pub struct Covers {
    active: Option<Active>,
    leaving: Option<(Active, Instant)>,
    backdrop_at: Option<Instant>,
}

impl Covers {
    /// Once per frame. `video` is where the current song's video is (if it has one and it should
    /// play). Returns a blurred picture for the backdrop when the animation has a new one
    /// and `backdrop` is wanted.
    pub fn update(
        &mut self,
        ctx: &eframe::egui::Context,
        track: Option<u64>,
        video: Option<String>,
        playing: bool,
        hold_secs: f32,
        backdrop: bool,
    ) -> Option<eframe::egui::ColorImage> {
        use eframe::egui;
        match (track, video) {
            (Some(t), Some(url)) => {
                if self.active.as_ref().map(|a| a.track) != Some(t) {
                    self.retire();
                    self.active = Some(Active { track: t, player: Player::start(url, ctx.clone()), tex: None });
                }
            }
            _ => self.retire(),
        }
        if self.leaving.as_ref().is_some_and(|(_, since)| since.elapsed().as_secs_f32() > hold_secs) {
            self.leaving = None;
        }
        let a = self.active.as_mut()?;
        a.player.set_paused(!playing);
        let frame = a.player.take_frame()?;
        let image = egui::ColorImage::from_rgba_unmultiplied(frame.size, &frame.rgba);
        match &mut a.tex {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => a.tex = Some(ctx.load_texture(format!("cover-video:{}", a.track), image, egui::TextureOptions::LINEAR)),
        }
        // the backdrop moves along, a little slower than the cover
        let due = self.backdrop_at.map_or(true, |t| t.elapsed() >= Duration::from_millis(90));
        if backdrop && due {
            self.backdrop_at = Some(Instant::now());
            return Some(crate::ui::blur_thumb(&frame.rgba, frame.size[0], frame.size[1]));
        }
        None
    }

    /// The video's current picture for this song, once the first one has arrived.
    pub fn texture(&self, track: u64) -> Option<eframe::egui::TextureId> {
        self.active.as_ref().filter(|a| a.track == track).and_then(|a| a.tex.as_ref()).map(|t| t.id())
    }

    /// Stop the running animation but keep its last picture around (a transition may still show it).
    fn retire(&mut self) {
        if let Some(old) = self.active.take() {
            old.player.stop();
            if old.tex.is_some() {
                self.leaving = Some((old, Instant::now()));
            }
        }
    }
}

/// One coded picture, ready for the decoder (Annex B, with the parameter sets in front of the first).
struct Packet {
    data: Vec<u8>,
}

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// MP4 stores each NAL unit behind a length; the decoder wants start codes.
fn avcc_to_annexb(sample: &[u8], length_size: usize, out: &mut Vec<u8>) -> bool {
    let mut at = 0;
    while at + length_size <= sample.len() {
        let len = sample[at..at + length_size].iter().fold(0usize, |n, b| (n << 8) | *b as usize);
        at += length_size;
        if len == 0 || at + len > sample.len() {
            return at >= sample.len();
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&sample[at..at + len]);
        at += len;
    }
    true
}

/// The coded pictures of the first video track, and the time each one is shown.
fn demux(bytes: Vec<u8>) -> Result<(Vec<Packet>, Duration), String> {
    let size = bytes.len() as u64;
    let mut reader = mp4::Mp4Reader::read_header(Cursor::new(bytes), size).map_err(|e| e.to_string())?;
    let (track_id, count, rate, sps, pps, length_size) = {
        let (id, track) = reader
            .tracks()
            .iter()
            .find(|(_, t)| matches!(t.track_type(), Ok(mp4::TrackType::Video)))
            .ok_or("no video track")?;
        let avcc = track
            .trak
            .mdia
            .minf
            .stbl
            .stsd
            .avc1
            .as_ref()
            .map(|a| a.avcc.length_size_minus_one as usize + 1)
            .unwrap_or(4);
        (
            *id,
            track.sample_count(),
            track.frame_rate(),
            track.sequence_parameter_set().map_err(|e| e.to_string())?.to_vec(),
            track.picture_parameter_set().map_err(|e| e.to_string())?.to_vec(),
            avcc,
        )
    };
    let mut packets = Vec::with_capacity(count as usize);
    for id in 1..=count {
        let Some(sample) = reader.read_sample(track_id, id).map_err(|e| e.to_string())? else { continue };
        let mut data = Vec::with_capacity(sample.bytes.len() + 64);
        if sample.is_sync {
            // the parameter sets in front of every key frame
            for set in [&sps, &pps] {
                data.extend_from_slice(&START_CODE);
                data.extend_from_slice(set);
            }
        }
        if !avcc_to_annexb(&sample.bytes, length_size, &mut data) {
            continue;
        }
        packets.push(Packet { data });
    }
    if packets.is_empty() {
        return Err("no pictures".into());
    }
    let fps = if rate.is_finite() && rate >= 1.0 { rate.min(60.0) } else { 30.0 };
    Ok((packets, Duration::from_secs_f64(1.0 / fps)))
}

fn run(url: &str, shared: &Shared, ctx: &eframe::egui::Context) -> Result<(), String> {
    let bytes = if let Some(path) = url.strip_prefix("file://") {
        // a local file (screenshots, tests)
        std::fs::read(path).map_err(|e| e.to_string())?
    } else {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(25))
            .build()
            .map_err(|e| e.to_string())?
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| e.to_string())?
            .to_vec()
    };
    if shared.stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    let (packets, interval) = demux(bytes)?;

    loop {
        let mut decoder = Decoder::new().map_err(|e| e.to_string())?;
        let options = DecodeOptions::new().flush_after_decode(Flush::NoFlush);
        let mut start = Instant::now();
        let mut shown = 0u32;
        // shows one decoded picture at the video's pace; false when told to stop
        let mut show = |yuv: &DecodedYUV<'_>| -> bool {
            let (w, h) = yuv.dimensions();
            let mut rgba = vec![0u8; w * h * 4];
            yuv.write_rgba8(&mut rgba);
            loop {
                if shared.stop.load(Ordering::Relaxed) {
                    return false;
                }
                if shared.paused.load(Ordering::Relaxed) {
                    // a pause shifts the whole schedule
                    let paused_at = Instant::now();
                    std::thread::sleep(Duration::from_millis(60));
                    start += paused_at.elapsed();
                    continue;
                }
                let now = Instant::now();
                let due = start + interval * shown;
                if now >= due {
                    break;
                }
                std::thread::sleep((due - now).min(Duration::from_millis(30)));
            }
            if let Ok(mut slot) = shared.frame.lock() {
                *slot = Some(Frame { size: [w, h], rgba });
            }
            ctx.request_repaint();
            shown += 1;
            true
        };
        for packet in &packets {
            // with B-frames a picture comes out a few packets after the one that carries it
            if let Ok(Some(yuv)) = decoder.decode_with_options(&packet.data, options.clone()) {
                if !show(&yuv) {
                    return Ok(());
                }
            }
        }
        if let Ok(rest) = decoder.flush_remaining() {
            for yuv in rest {
                if !show(&yuv) {
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_url_from_the_video_id() {
        assert_eq!(
            video_url("e9e126a9-97b6-4f99-b7b6-c242cb6eb31d", 640),
            "https://resources.tidal.com/videos/e9e126a9/97b6/4f99/b7b6/c242cb6eb31d/640x640.mp4"
        );
    }

    #[test]
    fn turns_length_prefixed_units_into_start_code_units() {
        // two NAL units: 3 bytes and 2 bytes, 4-byte lengths
        let sample = [0, 0, 0, 3, 0x65, 1, 2, 0, 0, 0, 2, 0x41, 9];
        let mut out = Vec::new();
        assert!(avcc_to_annexb(&sample, 4, &mut out));
        assert_eq!(out, [0, 0, 0, 1, 0x65, 1, 2, 0, 0, 0, 1, 0x41, 9]);
        // a length that runs past the end is rejected
        let mut bad = Vec::new();
        assert!(!avcc_to_annexb(&[0, 0, 0, 9, 1, 2], 4, &mut bad));
    }

    /// With `OXYTIDE_TEST_MP4=<file>`: the real decoder turns a real cover video into pictures.
    #[test]
    fn decodes_a_real_cover_video() {
        let Some(path) = std::env::var_os("OXYTIDE_TEST_MP4") else { return };
        let (packets, interval) = demux(std::fs::read(path).unwrap()).unwrap();
        assert!(packets.len() > 100, "{}", packets.len());
        assert!((interval.as_secs_f64() - 1.0 / 30.0).abs() < 0.002);
        let mut decoder = Decoder::new().unwrap();
        let options = DecodeOptions::new().flush_after_decode(Flush::NoFlush);
        let mut pictures = 0;
        for p in &packets {
            if let Ok(Some(yuv)) = decoder.decode_with_options(&p.data, options.clone()) {
                assert_eq!(yuv.dimensions(), (640, 640));
                pictures += 1;
            }
        }
        pictures += decoder.flush_remaining().map(|v| v.len()).unwrap_or(0);
        assert!(pictures >= packets.len() - 3, "{pictures} of {}", packets.len());
    }
}

