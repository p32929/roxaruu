//! The picture on the wire: only what changed since the last frame sent,
//! each changed region a lossless WebP — as sharp as the Mac's own pixels,
//! a few KB for a keystroke, and about JPEG's size for a whole new screen
//! (screen content compresses well losslessly). Browsers decode WebP
//! natively, off their main thread.
//!
//! Big changes on a dense (retina) screen — scrolling, a terminal pouring
//! out text — go at half resolution while they last (a quarter of the
//! pixels; drawn scaled up, about as sharp as a non-retina screen), then
//! the same regions are sent again at full resolution once things settle
//! (`SETTLE`). (Sending scrolled regions as copies doesn't work here: egui
//! scrolls by fractions of a pixel, so no row comes back exactly.)
//!
//! One message is one frame:
//!
//! ```text
//! u8 1 | u16 width | u16 height | u16 count | count × (u16 x, y, w, h | u32 len | len bytes of WebP)
//! ```
//!
//! (little-endian; width and height are the whole frame's; a region's
//! WebP may be smaller than its w×h, to be scaled up to fill it.) The page
//! draws every region, then acks, which is the flow control: see
//! `endpoint.rs`.
//!
//! Encoding runs on its own thread, so the window never waits for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::render::Image;

/// Rows compared together: a change marks its band's changed columns.
const BAND: u32 = 16;
/// Regions are cut into strips this tall, encoded in parallel (and decoded
/// in parallel by the browser).
const STRIP: u32 = 256;
/// Bands whose changed columns come within this of each other merge.
const NEAR: u32 = 64;
/// A change bigger than 1/BIG of the frame is "big" (sent at half
/// resolution on a dense screen).
const BIG: u64 = 4;
/// How long after the last big change the full-resolution pixels follow.
const SETTLE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// `rects` cut into strips at most `STRIP` tall.
fn strips(rects: &[Rect]) -> Vec<Rect> {
    rects
        .iter()
        .flat_map(|r| (r.y..r.y + r.h).step_by(STRIP as usize).map(|y| Rect { y, h: STRIP.min(r.y + r.h - y), ..*r }))
        .collect()
}

/// Bands (`y0`, height, first and last column) as regions: bands next to
/// each other whose spans are near merge.
fn merge(bands: impl Iterator<Item = (u32, u32, u32, u32)>) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for (y0, h, first, last) in bands {
        let band = Rect { x: first, y: y0, w: last - first + 1, h };
        match out.last_mut() {
            Some(r) if r.y + r.h == y0 && band.x <= r.x + r.w + NEAR && r.x <= band.x + band.w + NEAR => {
                let (x0, x1) = (r.x.min(band.x), (r.x + r.w).max(band.x + band.w));
                *r = Rect { x: x0, y: r.y, w: x1 - x0, h: r.h + h };
            }
            _ => out.push(band),
        }
    }
    out
}

/// Where `new` differs from `old` (both `width`×`height` RGBA): per band of
/// rows, the span of changed columns, merged (a typed character: its line,
/// and the status bar's line and column).
pub fn changed(old: &[u8], new: &[u8], width: u32, height: u32) -> Vec<Rect> {
    let stride = width as usize * 4;
    let bands = (0..height).step_by(BAND as usize).filter_map(|y0| {
        let h = BAND.min(height - y0);
        let mut span: Option<(u32, u32)> = None;
        for y in y0..y0 + h {
            let at = y as usize * stride;
            let (a, b) = (old.get(at..at + stride)?, new.get(at..at + stride)?);
            if a == b {
                continue;
            }
            let px = |i: usize| a[i * 4..i * 4 + 4] != b[i * 4..i * 4 + 4];
            let n = width as usize;
            let first = (0..n).find(|&i| px(i)).unwrap_or(0) as u32;
            let last = (0..n).rev().find(|&i| px(i)).unwrap_or(n - 1) as u32;
            span = Some(span.map_or((first, last), |(f, l)| (f.min(first), l.max(last))));
        }
        span.map(|(first, last)| (y0, h, first, last))
    });
    merge(bands)
}

/// The rows of `rects` together, as regions (each band's span covers every
/// rect crossing it).
fn union(rects: &[Rect], height: u32) -> Vec<Rect> {
    let bands = (0..height).step_by(BAND as usize).filter_map(|y0| {
        let h = BAND.min(height - y0);
        rects
            .iter()
            .filter(|r| r.w > 0 && r.y < y0 + h && y0 < r.y + r.h)
            .fold(None, |span: Option<(u32, u32)>, r| {
                let (f, l) = (r.x, r.x + r.w - 1);
                Some(span.map_or((f, l), |(a, b)| (a.min(f), b.max(l))))
            })
            .map(|(first, last)| (y0, h, first, last))
    });
    merge(bands)
}

/// A lossless WebP of `rect` within `image`, at half resolution if `half`
/// (each pixel the average of four).
fn webp(image: &Image, rect: Rect, half: bool) -> Option<Vec<u8>> {
    let stride = image.width as usize * 4;
    let (w, h, pixels) = if half {
        let px = |x: u32, y: u32, c: usize| u32::from(image.rgba[y as usize * stride + x as usize * 4 + c]);
        let (w, h) = (rect.w.div_ceil(2), rect.h.div_ceil(2));
        let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
        let (x_end, y_end) = (rect.x + rect.w - 1, rect.y + rect.h - 1);
        for y in 0..h {
            let (y0, y1) = (rect.y + y * 2, (rect.y + y * 2 + 1).min(y_end));
            for x in 0..w {
                let (x0, x1) = (rect.x + x * 2, (rect.x + x * 2 + 1).min(x_end));
                for c in 0..4 {
                    pixels.push(((px(x0, y0, c) + px(x1, y0, c) + px(x0, y1, c) + px(x1, y1, c) + 2) / 4) as u8);
                }
            }
        }
        (w, h, pixels)
    } else {
        let mut pixels = Vec::with_capacity(rect.w as usize * rect.h as usize * 4);
        for y in rect.y..rect.y + rect.h {
            let at = y as usize * stride + rect.x as usize * 4;
            pixels.extend_from_slice(image.rgba.get(at..at + rect.w as usize * 4)?);
        }
        (rect.w, rect.h, pixels)
    };
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(&pixels, w, h, image_webp::ColorType::Rgba8)
        .ok()?;
    Some(out)
}

/// One frame's message: `rects` of `image`, encoded in parallel.
pub fn encode(image: &Image, rects: &[Rect], half: bool) -> Option<Vec<u8>> {
    let fits = |r: &Rect| r.w > 0 && r.h > 0 && r.x + r.w <= image.width && r.y + r.h <= image.height;
    if !rects.iter().all(fits) {
        return None;
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunk = rects.len().div_ceil(threads).max(1);
    let parts: Vec<Option<Vec<Vec<u8>>>> = std::thread::scope(|s| {
        let jobs: Vec<_> = rects
            .chunks(chunk)
            .map(|group| s.spawn(move || group.iter().map(|&r| webp(image, r, half)).collect::<Option<Vec<_>>>()))
            .collect();
        jobs.into_iter().map(|j| j.join().ok().flatten()).collect()
    });
    let mut out = vec![1u8];
    for v in [image.width, image.height, rects.len() as u32] {
        out.extend_from_slice(&u16::try_from(v).ok()?.to_le_bytes());
    }
    let encoded = parts.into_iter().collect::<Option<Vec<_>>>()?.into_iter().flatten();
    for (r, bytes) in rects.iter().zip(encoded) {
        for v in [r.x, r.y, r.w, r.h] {
            out.extend_from_slice(&(v as u16).to_le_bytes());
        }
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    Some(out)
}

/// What encoding has cost so far (for the measurements in tests).
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub frames: u64,
    pub bytes: u64,
    pub encode_secs: f64,
    /// Frames sent at half resolution.
    pub half: u64,
}

struct Job {
    image: Image,
    /// Send the whole frame (a browser joined).
    key: bool,
    /// Drawn at a retina density: big changes may go at half resolution.
    dense: bool,
}

/// The encoding thread: takes the newest drawn frame, sends what changed
/// since the last one it sent.
pub struct Encoder {
    slot: SyncSender<Job>,
    busy: Arc<AtomicBool>,
    #[cfg_attr(not(test), allow(dead_code))] // read by the measurements in tests
    pub stats: Arc<Mutex<Stats>>,
}

impl Encoder {
    /// `send` gets each frame's message; `done` is called when the thread
    /// is free again.
    pub fn start(send: impl Fn(Vec<u8>) + Send + 'static, done: impl Fn() + Send + 'static) -> Encoder {
        let (slot, jobs) = sync_channel::<Job>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(Stats::default()));
        let (b, st) = (busy.clone(), stats.clone());
        let _ = std::thread::Builder::new()
            .name("remote-encoder".into())
            .spawn(move || run(&jobs, &b, &st, &send, &done));
        Encoder { slot, busy, stats }
    }

    /// Whether a new frame would wait (one is queued, or being encoded).
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Hand over a drawn frame; false if the thread is gone or full.
    pub fn submit(&self, image: Image, key: bool, dense: bool) -> bool {
        self.busy.store(true, Ordering::Release);
        match self.slot.try_send(Job { image, key, dense }) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }
}

fn run(jobs: &Receiver<Job>, busy: &AtomicBool, stats: &Mutex<Stats>, send: &dyn Fn(Vec<u8>), done: &dyn Fn()) {
    // What the browser has, at full resolution; regions it got at half
    // are in `soft` until sent again.
    let mut last: Option<Image> = None;
    let mut soft: Vec<Rect> = Vec::new();
    let out = |msg: Vec<u8>, t: Instant, half: bool| {
        let mut s = stats.lock().unwrap_or_else(|e| e.into_inner());
        s.frames += 1;
        s.bytes += msg.len() as u64;
        s.encode_secs += t.elapsed().as_secs_f64();
        s.half += u64::from(half);
        drop(s);
        send(msg);
    };
    loop {
        let wait = if soft.is_empty() { Duration::from_secs(3600) } else { SETTLE };
        let Job { image, key, dense } = match jobs.recv_timeout(wait) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => {
                // Settled: the full-resolution pixels of what went at half.
                if let Some(l) = &last
                    && let Some(msg) = encode(l, &strips(&union(&soft, l.height)), false)
                {
                    out(msg, Instant::now(), false);
                }
                soft.clear();
                continue;
            }
            // The `Encoder` (the sending side) was dropped.
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let t = Instant::now();
        let (w, h) = (image.width, image.height);
        // A browser joining, or a new size: the whole frame, sharp.
        let whole = key || last.as_ref().is_none_or(|l| (l.width, l.height) != (w, h));
        let mut rects = match &last {
            Some(l) if !whole => changed(&l.rgba, &image.rgba, w, h),
            _ => {
                soft.clear();
                vec![Rect { x: 0, y: 0, w, h }]
            }
        };
        let area: u64 = rects.iter().map(|r| u64::from(r.w) * u64::from(r.h)).sum();
        let half = dense && !whole && area * BIG > u64::from(w) * u64::from(h);
        if half {
            soft.extend(&rects);
        } else if !rects.is_empty() && !soft.is_empty() {
            // A small change after big ones: the soft regions go sharp with it.
            rects.append(&mut soft);
            rects = union(&rects, h);
        }
        if !rects.is_empty()
            && let Some(msg) = encode(&image, &strips(&rects), half)
        {
            out(msg, t, half);
            last = Some(image);
        }
        busy.store(false, Ordering::Release);
        done();
    }
}

/// The browser's side, for tests: applies frames to a picture.
#[cfg(test)]
#[derive(Default)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(test)]
impl Screen {
    /// Draw one frame's message; false if it isn't one.
    pub fn apply(&mut self, msg: &[u8]) -> bool {
        let u16_at = |i: usize| msg.get(i..i + 2).map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])));
        let (Some(1), Some(w), Some(h), Some(n)) = (msg.first().copied(), u16_at(1), u16_at(3), u16_at(5)) else {
            return false;
        };
        if (w, h) != (self.width, self.height) {
            *self = Screen { width: w, height: h, rgba: vec![0; (w * h * 4) as usize] };
        }
        let mut at = 7;
        for _ in 0..n {
            let (Some(x), Some(y), Some(rw), Some(rh)) = (u16_at(at), u16_at(at + 2), u16_at(at + 4), u16_at(at + 6))
            else {
                return false;
            };
            let len = u32::from_le_bytes(msg[at + 8..at + 12].try_into().unwrap()) as usize;
            let bytes = &msg[at + 12..at + 12 + len];
            at += 12 + len;
            let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).unwrap();
            let (iw, ih) = d.dimensions();
            let mut px = vec![0; d.output_buffer_size().unwrap()];
            d.read_image(&mut px).unwrap();
            let bpp = px.len() / (iw * ih) as usize;
            // Scaled to fill its region, as the page draws it (nearest here).
            for row in 0..rh {
                for col in 0..rw {
                    let s = (((row * ih / rh) * iw + col * iw / rw) as usize) * bpp;
                    let t = (((y + row) * w + x + col) * 4) as usize;
                    self.rgba[t..t + 3].copy_from_slice(&px[s..s + 3]);
                    self.rgba[t + 3] = if bpp == 4 { px[s + 3] } else { 255 };
                }
            }
        }
        true
    }

    pub fn image(&self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone()).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Image {
        let f = &f;
        let rgba = (0..h).flat_map(|y| (0..w).flat_map(move |x| f(x, y))).collect::<Vec<_>>();
        Image { width: w, height: h, rgba }
    }

    #[test]
    fn only_what_changed_is_found() {
        let a = picture(300, 100, |x, y| [(x % 7) as u8 * 30, (y % 5) as u8 * 40, 9, 255]);
        assert!(changed(&a.rgba, &a.rgba, 300, 100).is_empty(), "nothing changed");
        // A "character" typed at (40..48, 20..30) and the status bar's
        // number at (250..260, 90..100): two regions, each tight in x.
        let b = picture(300, 100, |x, y| {
            let typed = (40..48).contains(&x) && (20..30).contains(&y);
            let status = (250..260).contains(&x) && (90..100).contains(&y);
            if typed || status { [255, 0, 0, 255] } else { [(x % 7) as u8 * 30, (y % 5) as u8 * 40, 9, 255] }
        });
        let rects = changed(&a.rgba, &b.rgba, 300, 100);
        assert_eq!(rects, vec![Rect { x: 40, y: 16, w: 8, h: 16 }, Rect { x: 250, y: 80, w: 10, h: 20 }]);
        // Changes in neighbouring bands, near each other, make one region.
        let c = picture(300, 100, |x, y| if (10..20).contains(&x) && (5..40).contains(&y) { [1; 4] } else { [0; 4] });
        let z = picture(300, 100, |_, _| [0; 4]);
        assert_eq!(changed(&z.rgba, &c.rgba, 300, 100), vec![Rect { x: 10, y: 0, w: 10, h: 48 }]);
        // Regions together: each band spans every region crossing it.
        let u = union(&[Rect { x: 10, y: 0, w: 10, h: 20 }, Rect { x: 15, y: 10, w: 30, h: 10 }], 100);
        assert_eq!(u, vec![Rect { x: 10, y: 0, w: 35, h: 32 }]);
        assert_eq!(strips(&[Rect { x: 0, y: 0, w: 10, h: 600 }]).len(), 3, "big regions go in strips");
    }

    #[test]
    fn frames_round_trip_losslessly() {
        let a = picture(520, 300, |x, y| [(x * 3) as u8, (y * 5) as u8, (x ^ y) as u8, 255]);
        let mut screen = Screen::default();
        assert!(screen.apply(&encode(&a, &strips(&[Rect { x: 0, y: 0, w: 520, h: 300 }]), false).unwrap()));
        assert_eq!(screen.rgba, a.rgba, "a whole frame, exactly");
        let b = picture(520, 300, |x, y| {
            if (100..140).contains(&x) && (60..70).contains(&y) { [7, 7, 7, 255] } else { [(x * 3) as u8, (y * 5) as u8, (x ^ y) as u8, 255] }
        });
        let msg = encode(&b, &changed(&a.rgba, &b.rgba, 520, 300), false).unwrap();
        assert!(msg.len() < 1000, "a small change is a small message: {}", msg.len());
        assert!(screen.apply(&msg));
        assert_eq!(screen.rgba, b.rgba, "and the change lands exactly");
        // At half resolution: a quarter of the pixels, scaled back up.
        let half = encode(&a, &[Rect { x: 0, y: 0, w: 520, h: 300 }], true).unwrap();
        assert!(screen.apply(&half));
        assert_eq!((screen.width, screen.height), (520, 300));
        assert!(encode(&a, &[Rect { x: 500, y: 0, w: 40, h: 10 }], false).is_none(), "outside the frame");
    }

    #[test]
    fn big_changes_go_soft_then_sharp() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let s = sent.clone();
        let enc = Encoder::start(move |m| s.lock().unwrap().push(m), || {});
        let shade = |k: u32| picture(64, 64, move |x, y| [(x * k) as u8, (y * k) as u8, k as u8, 255]);
        let wait = |n: usize| {
            let end = Instant::now() + Duration::from_secs(5);
            while enc.busy() || sent.lock().unwrap().len() < n {
                assert!(Instant::now() < end, "never sent");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        assert!(enc.submit(shade(1), false, true));
        wait(1);
        assert!(enc.submit(shade(1), false, true));
        wait(1);
        assert_eq!(sent.lock().unwrap().len(), 1, "an unchanged frame isn't sent");
        assert!(enc.submit(shade(1), true, true));
        wait(2);
        assert_eq!(enc.stats.lock().unwrap().frames, 2, "a key frame is sent even unchanged");
        // Everything changes: half resolution, then (once settled) full.
        assert!(enc.submit(shade(3), false, true));
        wait(4);
        assert_eq!(enc.stats.lock().unwrap().half, 1);
        let mut screen = Screen::default();
        for m in sent.lock().unwrap().iter() {
            screen.apply(m);
        }
        assert_eq!(screen.rgba, shade(3).rgba, "sharp once settled");
        // Not dense: big changes go sharp at once.
        assert!(enc.submit(shade(5), false, false));
        wait(5);
        assert_eq!(enc.stats.lock().unwrap().half, 1);
    }
}
