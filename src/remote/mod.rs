//! Remote access: open the Mac's Roxaruu windows from a browser on the same
//! Wi-Fi, after a password, and use them as if at the Mac.
//!
//! Each window draws a second copy of itself, off-screen, at the browser's
//! size, from the same live state (one session: typing in either place
//! shows in both), and streams what changed, losslessly, over a WebSocket
//! (`frames.rs`). Being off-screen, it keeps working with the Mac window
//! covered, minimized or the screen locked, at full rate (`clock.rs`).
//!
//! The picture's path: browser input wakes the window at once; it runs the
//! browser's copy and draws it on the GPU (`render.rs`); a thread finds and
//! encodes the changed regions; the link threads pass the frame straight
//! through (`endpoint.rs`, `hub.rs`). The page acks each frame once drawn,
//! and the window draws no further ahead than the slowest browser can take
//! (two frames unacked), so a slow link shows the newest picture, never a
//! queue of old ones.
//!
//! Lifecycle: off at every launch, never a saved setting. Turning it on asks
//! for a password each time; the window where that happens runs the HTTPS
//! server for all of them (`hub.rs`), holding the password's hash only in
//! memory, and marks itself in the running dir (`remote-server.json`: pid
//! and port, no secrets). Every window that sees a live marker opens its
//! private link; when the marker goes (turned off anywhere) or its window
//! dies, they close theirs. So closing the serving window, or turning it
//! off, stops the server and disconnects every browser.

pub mod auth;
mod clock;
mod endpoint;
mod meshes;
pub(crate) mod frames;
mod http;
mod hub;
pub mod input;
mod render;
#[cfg(test)]
pub(crate) mod testing;
mod tls;

use std::net::{IpAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::app::App;
use crate::instances::{self, RemoteWindow};
use crate::keymap::Command;
#[cfg(test)]
pub(crate) use endpoint::ACK;
use endpoint::{Endpoint, Out};
use input::ClientMsg;

/// The port browsers use: `https://<this Mac>:7878`.
pub const PORT: u16 = 7878;
/// Frames are at most this many pixels across (a big retina screen would
/// otherwise send huge ones for little gain).
const MAX_SIDE: f32 = 2560.0;
/// Drawing on the CPU: passes this close together are things moving (typing,
/// scrolling, a terminal printing), drawn at `MOVING_SCALE`; this long after
/// the last, it's drawn again sharp.
pub(crate) const SETTLED: Duration = Duration::from_millis(600);
/// ¾ of each side: about half the pixels to draw.
const MOVING_SCALE: f32 = 0.75;
/// The browser's copy runs at most this often unless input arrives (~60 fps).
const FRAME: Duration = Duration::from_millis(16);
/// …and at least this often while a browser looks, in case something
/// changed that asked no one to repaint.
const IDLE: Duration = Duration::from_secs(1);

/// What the window shows about remote access (status bar, dialogs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// A server is running (in this window or another).
    pub on: bool,
    /// The address to open, once serving.
    pub url: Option<String>,
    /// Browsers looking at this window.
    pub clients: usize,
    /// Why it isn't working, if it isn't.
    pub problem: Option<String>,
    /// This window runs the server: closing it ends remote access.
    pub here: bool,
}

/// What a window asks of remote access (`App::remote_want`, handled on the
/// next `Remote::update`).
#[derive(Clone, Debug, PartialEq)]
pub enum Want {
    /// Serve with this password (unless a server already runs elsewhere);
    /// whether it's to be used only from browsers (windows here minimize).
    On(String, bool),
    /// Stop the server, wherever it runs.
    Off,
}

/// Earlier builds saved the password's hash in `remote.json`; nothing is
/// saved now, so remove it.
pub fn forget_saved_password() {
    let _ = std::fs::remove_file(old_secrets_path());
}

pub(crate) fn old_secrets_path() -> PathBuf {
    crate::config::dir().join("remote.json")
}

/// This Mac's address on the local network (the route the OS would use to
/// reach the internet; nothing is sent).
/// Where a browser opens remote access on `port`: this computer's address
/// on the network (else this computer only).
pub fn address(port: u16) -> String {
    format!("https://{}:{port}", lan_ip().map_or_else(|| "localhost".to_string(), |ip| ip.to_string()))
}

pub fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// The CSS cursor for egui's.
fn css_cursor(icon: egui::CursorIcon) -> &'static str {
    use egui::CursorIcon as C;
    match icon {
        C::PointingHand => "pointer",
        C::Text => "text",
        C::VerticalText => "vertical-text",
        C::Crosshair => "crosshair",
        C::Move | C::AllScroll => "move",
        C::Grab => "grab",
        C::Grabbing => "grabbing",
        C::NotAllowed | C::NoDrop => "not-allowed",
        C::Wait => "wait",
        C::Progress => "progress",
        C::Help => "help",
        C::ContextMenu => "context-menu",
        C::Copy => "copy",
        C::Alias => "alias",
        C::Cell => "cell",
        C::ResizeHorizontal | C::ResizeColumn | C::ResizeEast | C::ResizeWest => "ew-resize",
        C::ResizeVertical | C::ResizeRow | C::ResizeNorth | C::ResizeSouth => "ns-resize",
        C::ResizeNeSw | C::ResizeNorthEast | C::ResizeSouthWest => "nesw-resize",
        C::ResizeNwSe | C::ResizeNorthWest | C::ResizeSouthEast => "nwse-resize",
        C::ZoomIn => "zoom-in",
        C::ZoomOut => "zoom-out",
        C::None => "none",
        _ => "default",
    }
}

/// One window's side of remote access.
pub struct Remote {
    endpoint: Option<Endpoint>,
    hub: Option<hub::Hub>,
    /// Keeps the computer awake while serving.
    awake: Option<crate::sys::Awake>,
    /// The browser's copy of the window.
    ctx: egui::Context,
    started: Instant,
    offscreen: Option<render::Offscreen>,
    fonts: Option<(String, String)>,
    /// The browser's room for the window: CSS width, height, pixel ratio.
    size: Option<(f32, f32, f32)>,
    visible: bool,
    /// The browser's keyboard and pointer, as the Mac would report them;
    /// messages held for the next pass (see `Input::splits_before`).
    input: input::Input,
    backlog: Vec<ClientMsg>,
    /// Wakes the window for the browser's frames (see `clock.rs`).
    clock: Option<clock::Clock>,
    encoder: Option<frames::Encoder>,
    /// Set while a frame waits for the encoder; it wakes the window when free.
    held: Arc<AtomicBool>,
    /// The last pass's output, not yet drawn (waiting for the browser or the
    /// encoder to catch up), with every texture change since the last draw.
    unrendered: Option<egui::FullOutput>,
    /// The next frame goes whole (a browser joined).
    key: bool,
    last_pass: Option<Instant>,
    /// Pixels per point of the browser's copy.
    ppp: f32,
    /// Time spent drawing frames on the GPU and reading them back (tests
    /// report it).
    pub(crate) render_secs: f64,
    pub(crate) renders: u64,
    /// What it did for its browsers since the last "remote timing" log line.
    timing: Timing,
    timing_from: Instant,
    /// The encoder's totals at that line.
    encoded: (u64, f64),
    /// On a virtual screen (`--virtual-screen`): nobody sees the window
    /// itself, so it's treated as hidden (the clock wakes it for its browsers).
    pub(crate) virtual_screen: bool,
    /// Drawing on the CPU (no GPU, e.g. a server: llvmpipe, WARP): every
    /// pixel costs, so frames go with fewer while things move (see `serve`).
    pub(crate) software: bool,
    /// Minimized for "browser only" (see `update`).
    minimized: bool,
    /// This pass's share of the browser's pixels to draw (1: all).
    scale: f32,
    /// Every texture the browser's copy uses (for browsers that draw
    /// themselves, joining later; and the picture renderer starting afresh).
    textures: meshes::Textures,
    /// Browsers draw the meshes themselves (`meshes.rs`), and the last
    /// meshes sent (a hash: the same again isn't sent).
    browser_draws: bool,
    sent_meshes: u64,
    cursor: Option<egui::CursorIcon>,
    /// What this window is listed as.
    listed: Option<(String, String)>,
    problem: Option<String>,
    /// The port browsers use (tests pick a free one).
    pub(crate) port: u16,
    /// Recent answers to slow questions: which window serves, what's the
    /// Wi-Fi address (asked at most every so often, not every frame).
    server_checked: Option<(Instant, Option<instances::RemoteServer>)>,
    ip_checked: Option<(Instant, Option<IpAddr>)>,
}

impl Default for Remote {
    fn default() -> Self {
        Remote {
            endpoint: None,
            hub: None,
            awake: None,
            ctx: egui::Context::default(),
            started: Instant::now(),
            offscreen: None,
            fonts: None,
            size: None,
            visible: true,
            input: input::Input::default(),
            backlog: Vec::new(),
            clock: None,
            encoder: None,
            held: Arc::default(),
            unrendered: None,
            key: true,
            last_pass: None,
            ppp: 1.0,
            render_secs: 0.0,
            renders: 0,
            timing: Timing::default(),
            timing_from: Instant::now(),
            encoded: (0, 0.0),
            virtual_screen: false,
            software: false,
            minimized: false,
            scale: 1.0,
            textures: meshes::Textures::default(),
            browser_draws: false,
            sent_meshes: 0,
            cursor: None,
            listed: None,
            problem: None,
            port: PORT,
            server_checked: None,
            ip_checked: None,
        }
    }
}

/// What a window did for its browsers over a few seconds, for the log: where
/// a slow remote view spends its time (a machine with no GPU draws in software).
#[derive(Default)]
struct Timing {
    /// Times the window ran (`update`), and the longest wait between two.
    wakes: u32,
    longest_gap: Duration,
    last_wake: Option<Instant>,
    /// Passes of the browser's copy, and the time they took (laying out).
    passes: u32,
    pass_secs: f64,
    /// Frames drawn off-screen and read back, and the time that took.
    drawn: u32,
    draw_secs: f64,
    /// A frame held back: a browser hadn't acked, or the encoder was busy.
    held_browser: u32,
    held_busy: u32,
    /// Drawing a frame failed (the GPU didn't answer in time).
    failed: u32,
}

/// While browsers may look: wakes this window's copy for them directly,
/// whatever pace the window itself keeps (eframe's, which on a virtual
/// screen is slow).
static WAKE: Mutex<Option<clock::Waker>> = Mutex::new(None);

/// Something the browsers' copy shows changed (a terminal's output): draw it
/// for them within `delay`.
pub fn soon(delay: Duration) {
    if let Some(w) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        w.at(Instant::now() + delay);
    }
}

/// Closing the window (or quitting) drops it: the server, if it runs here,
/// stops, and its marker goes, so every browser disconnects.
impl Drop for Remote {
    fn drop(&mut self) {
        self.stop_serving();
        self.close();
    }
}

impl Remote {
    /// The browser's copy of the window (tests look at its menus).
    #[cfg(test)]
    pub(crate) fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Whether this window runs the server.
    pub(crate) fn serving(&self) -> bool {
        self.hub.is_some()
    }

    /// Close this window's private link (it's no longer listed).
    fn close(&mut self) {
        if self.endpoint.take().is_some() {
            instances::unregister_remote();
        }
        self.listed = None;
        self.size = None;
        self.encoder = None;
        self.clock = None;
        *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.unrendered = None;
    }

    /// What encoding frames has cost so far.
    pub(crate) fn encode_stats(&self) -> frames::Stats {
        self.encoder
            .as_ref()
            .map(|e| e.stats.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .unwrap_or_default()
    }

    /// Stop the server here, if it runs here (the password goes with it).
    fn stop_serving(&mut self) {
        if self.hub.take().is_some() {
            instances::clear_remote_server(true);
        }
        self.awake = None;
    }

    /// Open this window's private link, so the server can list it.
    fn open(&mut self, main: &egui::Context, frame: &eframe::Frame) {
        let redraw = frame
            .winit_window()
            .cloned()
            .map(|w| Box::new(move || w.request_redraw()) as Box<dyn Fn() + Send>);
        let clock = clock::Clock::start(redraw, main.clone());
        let waker = clock.waker();
        *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(clock.waker());
        match Endpoint::start(auth::random_token(), move || waker.wake()) {
            Ok(ep) => {
                let (sender, waker, held) = (ep.sender(), clock.waker(), self.held.clone());
                self.encoder = Some(frames::Encoder::start(
                    move |msg| sender.broadcast(Out::Frame(Arc::new(msg))),
                    move || {
                        if held.swap(false, Ordering::SeqCst) {
                            waker.wake();
                        }
                    },
                ));
                self.endpoint = Some(ep);
                self.clock = Some(clock);
            }
            Err(e) => self.trouble(format!("Couldn't start remote access: {e}")),
        }
    }

    /// Serve here with `password`, unless a server already runs (here or in
    /// another window): then there's nothing to do but show its address.
    fn turn_on(&mut self, password: &str, browser_only: bool) {
        self.problem = None;
        if self.serving() || instances::remote_server().is_some() {
            return;
        }
        let Some(secrets) = auth::Secrets::new(password) else {
            self.trouble("Couldn't start remote access: no random numbers");
            return;
        };
        let names: Vec<String> = lan_ip()
            .map(|ip| ip.to_string())
            .into_iter()
            .chain(["localhost".to_string(), "127.0.0.1".to_string()])
            .collect();
        let tls = match tls::server_config(&crate::config::dir().join("remote"), &names) {
            Ok(tls) => tls,
            Err(e) => {
                self.trouble(format!("Couldn't set up HTTPS: {e}"));
                return;
            }
        };
        let setup = hub::Setup { port: self.port, secrets, tls, windows: instances::remote_windows };
        match hub::Hub::start(setup) {
            Ok(hub) => self.hub = Some(hub),
            Err(e) => {
                self.trouble(format!("Couldn't use port {}: {e}", self.port));
                return;
            }
        }
        tracing::info!(port = self.port, "remote access on");
        instances::register_remote_server(self.port, browser_only);
        // A sleeping computer drops off the Wi-Fi: stay awake while serving
        // (the display may still sleep). Ends with this window.
        self.awake = Some(crate::sys::stay_awake());
    }

    /// Something went wrong: shown with the address, and logged once.
    fn trouble(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if self.problem.as_ref() != Some(&msg) {
            tracing::warn!(msg, "remote access");
        }
        self.problem = Some(msg);
    }

    /// Stop the server wherever it runs: here directly, elsewhere by
    /// removing its marker (that window notices within a second and stops).
    fn turn_off(&mut self) {
        tracing::info!(here = self.serving(), "remote access off");
        self.problem = None;
        if self.serving() {
            self.stop_serving();
        } else {
            instances::clear_remote_server(false);
        }
    }

    /// Called on every pass of the event loop, even with the window hidden:
    /// do what was asked, follow the server's marker, then serve any
    /// browser looking.
    pub fn update(&mut self, app: &mut App, main: &egui::Context, frame: &eframe::Frame) {
        let now = Instant::now();
        let fresh = |t: Instant, secs: u64| now.duration_since(t) < Duration::from_secs(secs);
        let asked = app.remote_want.take();
        match &asked {
            Some(Want::On(password, browser_only)) => self.turn_on(password, *browser_only),
            Some(Want::Off) => self.turn_off(),
            None => {}
        }
        let server = match self.server_checked {
            Some((t, server)) if asked.is_none() && fresh(t, 1) => server,
            _ => {
                let server = instances::remote_server();
                self.server_checked = Some((now, server));
                server
            }
        };
        // Used only from browsers: minimized meanwhile, so this computer draws
        // nothing (the window showing the address waits until it's read).
        let hide = !self.virtual_screen
            && server.is_some_and(|s| s.browser_only)
            && !matches!(app.ask, Some(crate::app::Ask::RemoteAddress));
        if hide != self.minimized {
            self.minimized = hide;
            main.send_viewport_cmd(egui::ViewportCommand::Minimized(hide));
        }
        // Serving here, but the marker is gone or someone else's: turned off
        // from another window.
        let me = std::process::id() as i32;
        if self.serving() && server.is_none_or(|s| s.pid != me) {
            self.stop_serving();
        }
        // Every window is reachable while a server runs, and only then.
        if server.is_none() {
            self.close();
        } else if self.endpoint.is_none() && self.problem.is_none() {
            self.open(main, frame);
        }
        self.list(app);
        let clients = self.endpoint.as_ref().map_or(0, Endpoint::clients);
        let ip = match self.ip_checked {
            _ if server.is_none() => None,
            Some((t, ip)) if fresh(t, 5) => ip,
            _ => {
                let ip = lan_ip();
                self.ip_checked = Some((now, ip));
                ip
            }
        };
        let url = ip.zip(server).map(|(ip, s)| format!("https://{ip}:{}", s.port));
        app.remote = Status {
            on: server.is_some(),
            url,
            clients,
            problem: self.problem.clone(),
            here: self.hub.is_some(),
        };
        // Hidden (minimized, covered, the screen locked): the window's own
        // repaint requests are delayed by eframe, so the clock takes them.
        let hidden = self.virtual_screen
            || main.input(|i| i.raw.viewports.get(&egui::ViewportId::ROOT).and_then(|v| v.visible()))
                == Some(false)
            || frame.winit_window().is_some_and(|w| w.is_visible() == Some(false));
        if let Some(clock) = &self.clock {
            clock.set_hidden(hidden && clients > 0);
        }
        if clients > 0 {
            self.timing.wakes += 1;
            if let Some(last) = self.timing.last_wake.replace(now) {
                self.timing.longest_gap = self.timing.longest_gap.max(now - last);
            }
            self.serve(app, main, frame);
            self.log_timing(app, hidden);
        } else {
            self.size = None;
            self.unrendered = None;
        }
    }

    /// Keep this window's listing (its folder's name) current.
    fn list(&mut self, app: &App) {
        let Some(ep) = &self.endpoint else { return };
        let (name, path) = match &app.project {
            Some(p) => (p.name.clone(), crate::ui::tilde(&p.root.to_string_lossy())),
            None => ("Roxaruu".to_string(), String::new()),
        };
        if self.listed.as_ref() != Some(&(name.clone(), path.clone())) {
            instances::register_remote(&RemoteWindow {
                pid: 0,
                port: ep.port,
                token: ep.token.clone(),
                name: name.clone(),
                path: path.clone(),
            });
            self.listed = Some((name, path));
        }
    }

    /// Every 5 s while browsers look: how fast frames go out and where the
    /// time went, so a slow remote view can be read from the log.
    fn log_timing(&mut self, app: &App, hidden: bool) {
        let secs = self.timing_from.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let t = std::mem::take(&mut self.timing);
        let enc = self.encode_stats();
        // (A new encoder, for a new browser, starts its totals again.)
        let (sent, enc_secs) = (enc.frames.saturating_sub(self.encoded.0), (enc.encode_secs - self.encoded.1).max(0.0));
        self.encoded = (enc.frames, enc.encode_secs);
        self.timing_from = Instant::now();
        let ms = |s: f64, n: u64| format!("{:.0}", if n == 0 { 0.0 } else { s * 1000.0 / n as f64 });
        tracing::debug!(
            sent_fps = format!("{:.1}", sent as f64 / secs),
            window_fps = format!("{:.1}", app.fps.unwrap_or(0.0)),
            wakes_per_s = format!("{:.1}", f64::from(t.wakes) / secs),
            longest_gap_ms = t.longest_gap.as_millis(),
            passes = t.passes,
            pass_ms = ms(t.pass_secs, t.passes.into()),
            drawn = t.drawn,
            draw_ms = ms(t.draw_secs, t.drawn.into()),
            encode_ms = ms(enc_secs, sent),
            held_browser = t.held_browser,
            held_busy = t.held_busy,
            failed = t.failed,
            hidden,
            "remote timing"
        );
    }

    /// Run the browser's copy of the window if it's due, and send the frame.
    fn serve(&mut self, app: &mut App, main: &egui::Context, frame: &eframe::Frame) {
        let Some(ep) = &self.endpoint else { return };
        let joined = ep.take_joined();
        let mut events = Vec::new();
        let mut resized = false;
        let mut drawer = false;
        let mut inbox = std::mem::take(&mut self.backlog);
        inbox.extend(ep.take_inbox());
        self.input.begin(self.started.elapsed().as_secs_f64());
        let mut inbox = inbox.into_iter();
        while let Some(msg) = inbox.next() {
            if self.input.splits_before(&msg) {
                self.backlog.push(msg);
                self.backlog.extend(inbox.by_ref());
                if let Some(c) = &self.clock {
                    c.waker().wake();
                }
                break;
            }
            match msg {
                ClientMsg::Size { w, h, dpr, mac } if w >= 1.0 && h >= 1.0 => {
                    resized |= self.size != Some((w, h, dpr.clamp(0.5, 4.0)));
                    self.size = Some((w, h, dpr.clamp(0.5, 4.0)));
                    self.input.mac = mac;
                }
                ClientMsg::Visible { on } => self.visible = on,
                ClientMsg::Drawer => drawer = true,
                m => events.extend(self.input.events(&m)),
            }
        }
        if joined {
            self.key = true; // the whole frame, even if unchanged
            // The page shows the keys for its full-screen button and window list.
            let _keys = crate::keymap::keys_like(self.input.mac);
            let caps = |c| app.config.keymap.combo_for(c).map(|k| k.label()).unwrap_or_default();
            let (full, windows) = (caps(Command::FullScreen), caps(Command::SwitchWindow));
            ep.broadcast(Out::Text(
                serde_json::json!({ "t": "keys", "full": full, "windows": windows }).to_string(),
            ));
        }
        let Some((w, h, dpr)) = self.size else { return };
        if !self.visible {
            return;
        }
        let now = Instant::now();
        // Input goes at once. Otherwise the window was woken for something
        // (a terminal's output, a timer, the browser's copy asking): run it,
        // at most once a frame.
        let since = self.last_pass.map_or(Duration::MAX, |t| now.duration_since(t));
        if events.is_empty() && !drawer && !joined && !resized && since < FRAME {
            if let Some(c) = &self.clock {
                c.at(now + (FRAME - since));
            }
            self.present(frame);
            return;
        }
        self.last_pass = Some(now);
        // Without a GPU, a pass right after another is something moving:
        // fewer pixels now, and a sharp frame once it settles.
        self.scale = if self.software && since < SETTLED && !joined && !resized { MOVING_SCALE } else { 1.0 };
        if self.scale < 1.0
            && let Some(c) = &self.clock
        {
            c.at(now + SETTLED);
        }
        // Lay out at the browser's size; keep big screens' frames sensible.
        let ppp = dpr.min(MAX_SIDE / w).min(MAX_SIDE / h).max(0.5);
        self.ppp = ppp;
        let fonts = (app.config.settings.ui_font.clone(), app.config.settings.code_font.clone());
        if self.fonts.as_ref() != Some(&fonts) {
            crate::fonts::install(&self.ctx, &fonts.0, &fonts.1);
            self.fonts = Some(fonts);
        }
        self.ctx.set_theme(main.theme());
        // Without a GPU each blink of the caret is a whole frame drawn on the CPU.
        let mut style = (*main.global_style()).clone();
        style.visuals.text_cursor.blink &= !self.software;
        self.ctx.set_global_style(style);
        let raw = |events: Vec<egui::Event>, time: f64| {
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
                time: Some(time),
                focused: true,
                events,
                ..Default::default()
            };
            raw.viewports.insert(
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(ppp),
                    focused: Some(true),
                    ..Default::default()
                },
            );
            raw
        };
        let time = self.input.time();
        app.remote_pass = true;
        // The browser's keys: a Mac's ⌘ from a window on Linux, and so on.
        let _keys = crate::keymap::keys_like(self.input.mac);
        // A swipe from the edge: out comes this view's drawer, as ⌘B would.
        if drawer && app.view().compact && !app.sidebar_shown() {
            app.focus_sidebar();
        }
        // A new size (or a new browser): lay out once first, so panels know
        // their sizes, then draw for real — keeping the first pass's
        // textures (the font atlas), which the renderer still needs.
        let started = Instant::now();
        let first = (resized || joined).then(|| self.ctx.run_ui(raw(Vec::new(), time), |ui| app.frame(ui)));
        let mut output = self.ctx.run_ui(raw(events, time), |ui| app.frame(ui));
        self.timing.passes += 1;
        self.timing.pass_secs += started.elapsed().as_secs_f64();
        app.remote_pass = false;
        // Texture changes of passes not drawn yet come first.
        let mut textures = self.unrendered.take().map(|o| o.textures_delta).unwrap_or_default();
        if let Some(first) = first {
            textures.append(first.textures_delta);
        }
        textures.append(std::mem::take(&mut output.textures_delta));
        output.textures_delta = textures;

        if let Some(ep) = &self.endpoint {
            // Copies go to the browser's clipboard, links open there, Full
            // Screen is the page's; the cursor follows egui's.
            for cmd in &output.platform_output.commands {
                let msg = match cmd {
                    egui::OutputCommand::CopyText(text) => serde_json::json!({ "t": "clip", "s": text }),
                    egui::OutputCommand::OpenUrl(open) => serde_json::json!({ "t": "open", "url": open.url }),
                    _ => continue,
                };
                ep.broadcast(Out::Text(msg.to_string()));
            }
            let full = output.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| {
                v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Fullscreen(_)))
            });
            if full {
                ep.broadcast(Out::Text(serde_json::json!({ "t": "full" }).to_string()));
            }
            if std::mem::take(&mut app.remote_windows) {
                ep.broadcast(Out::Text(serde_json::json!({ "t": "windows" }).to_string()));
            }
            let cursor = output.platform_output.cursor_icon;
            if self.cursor != Some(cursor) {
                self.cursor = Some(cursor);
                ep.broadcast(Out::Text(
                    serde_json::json!({ "t": "cursor", "c": css_cursor(cursor) }).to_string(),
                ));
            }
        }

        // When the browser's copy wants its next pass (the caret blinking,
        // an animation), within reason.
        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(IDLE, |v| v.repaint_delay)
            .clamp(FRAME, IDLE);
        if let Some(c) = &self.clock {
            c.at(now + delay);
        }
        self.unrendered = Some(output);
        self.present(frame);
    }

    /// Draw the last pass and hand it to the encoder — unless a browser or
    /// the encoder is behind: then it waits, and they wake the window when
    /// they catch up (by then it may have been replaced by a newer pass).
    fn present(&mut self, frame: &eframe::Frame) {
        let (Some(ep), Some(encoder)) = (&self.endpoint, &self.encoder) else { return };
        if self.unrendered.is_none() {
            return;
        }
        if !ep.ready() {
            self.timing.held_browser += 1;
            // An ack wakes the window; a browser that never acks gets
            // frames anyway after a while.
            if let Some(c) = &self.clock {
                c.at(Instant::now() + endpoint::ACK_PATIENCE);
            }
            return;
        }
        self.held.store(true, Ordering::SeqCst);
        if encoder.busy() {
            self.timing.held_busy += 1;
            return;
        }
        self.held.store(false, Ordering::SeqCst);
        let Some(state) = frame.wgpu_render_state() else {
            self.trouble("Remote access needs the GPU renderer");
            return;
        };
        let Some(mut output) = self.unrendered.take() else { return };
        self.software = state.adapter.get_info().device_type == egui_wgpu::wgpu::DeviceType::Cpu;
        let started = Instant::now();
        self.textures.apply(&output.textures_delta);
        if ep.draws() {
            // The browsers draw: send what to draw, not a picture.
            let key = std::mem::take(&mut self.key) || !self.browser_draws;
            self.browser_draws = true;
            let ppp = self.ctx.pixels_per_point();
            let size = self.ctx.content_rect().size() * ppp;
            let primitives = self.ctx.tessellate(std::mem::take(&mut output.shapes), ppp);
            let meshes = meshes::meshes(&primitives);
            let hash = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                meshes.hash(&mut h);
                h.finish()
            };
            let textures_changed = !output.textures_delta.is_empty();
            if key || textures_changed || hash != self.sent_meshes {
                self.sent_meshes = hash;
                let textures = meshes::textures(key.then_some(&self.textures), &output.textures_delta);
                encoder.submit_meshes(meshes::frame(size, ppp, &textures, &meshes));
            }
            self.timing.drawn += 1;
            self.timing.draw_secs += started.elapsed().as_secs_f64();
            return;
        }
        if std::mem::take(&mut self.browser_draws) {
            // Back to pictures: the renderer missed the textures meanwhile.
            self.offscreen = None;
            output.textures_delta = self.textures.all();
            self.key = true;
        }
        let offscreen = self
            .offscreen
            .get_or_insert_with(|| render::Offscreen::new(&state.device, &state.queue));
        let Some(image) = offscreen.render(&self.ctx, &output, self.scale) else {
            self.timing.failed += 1;
            return;
        };
        let secs = started.elapsed().as_secs_f64();
        self.render_secs += secs;
        self.renders += 1;
        self.timing.drawn += 1;
        self.timing.draw_secs += secs;
        encoder.submit(image, std::mem::take(&mut self.key), self.ppp * self.scale >= 1.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_map_to_css() {
        assert_eq!(css_cursor(egui::CursorIcon::Text), "text");
        assert_eq!(css_cursor(egui::CursorIcon::PointingHand), "pointer");
        assert_eq!(css_cursor(egui::CursorIcon::Default), "default");
    }

    #[test]
    fn lan_ip_is_a_real_address_when_there_is_one() {
        if let Some(ip) = lan_ip() {
            assert!(!ip.is_loopback() && !ip.is_unspecified());
        }
    }
}
