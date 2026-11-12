//! Roxaruu — a keyboard-first editor and terminals, on egui.
//!
//! **No `unwrap` / `expect` / `panic!` in shipped code** (Oxru's house rule): a
//! crash in an editor loses unsaved work, so fallible paths degrade instead.

#![deny(unsafe_code)]
// Windows: a desktop app, not a console program (no black window behind it).
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

mod app;
mod buffer;
mod config;
mod editor;
mod find;
mod fonts;
mod fstree;
mod fuzzy;
mod huge;
// The UI walkthroughs drive real Unix shells (zsh), symlinks and file modes.
#[cfg(all(test, unix))]
mod headless;
mod instances;
mod keymap;
mod palette;
mod recent;
mod remote;
mod search;
mod session;
mod syntax;
mod sys;
mod termbridge;
mod terminal;
mod terminalpane;
mod theme;
mod todo;
mod ui;
mod wrap;

use eframe::egui;

/// This build's version (Cargo.toml), shown in the status bar, on the
/// welcome screen and by `--version`, so an update is easy to confirm.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The app icon (`assets/icon.svg`, rendered): the window and Dock icon, and
/// the mark on the welcome screen.
pub(crate) const ICON: &[u8] = include_bytes!("../assets/icon.png");

/// The window has drawn: a crash from here on isn't a renderer that can't
/// start (see `main`).
static DRAWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set (to `safe`) on the copy started after the GPU couldn't make a window.
const RENDERER_ENV: &str = "ROXARUU_RENDERER";

/// This run draws the safe way, not through the GPU's driver: Windows' own
/// software DirectX (WARP, in every Windows 10/11; OpenGL there may be 1.1),
/// OpenGL elsewhere (Mesa draws it in software if it must).
static SAFE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Already saying the window can't open (the watchdog mustn't say it twice).
static GIVING_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long a window may take to first draw before its renderer counts as
/// stuck (a driver that hangs rather than fails).
const FIRST_DRAW_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

fn main() -> eframe::Result {
    use std::io::IsTerminal;
    init_log();
    tracing::info!(version = VERSION, os = std::env::consts::OS, pid = std::process::id(), "starting");
    tracing::info!(
        args = ?std::env::args_os().skip(1).collect::<Vec<_>>(),
        exe = ?std::env::current_exe().ok(),
        cwd = ?std::env::current_dir().ok(),
        config = %config::dir().display(),
        home = ?config::home(),
        "where"
    );
    on_crash();
    let from_shell = std::io::stdin().is_terminal();
    let launch = Launch::parse(std::env::args_os().skip(1), from_shell);
    match &launch.info {
        Some(Info::Help) => {
            println!("{}", help());
            return Ok(());
        }
        Some(Info::Version) => {
            println!("roxaruu {VERSION}");
            return Ok(());
        }
        Some(Info::Unknown(flag)) => {
            eprintln!("roxaruu: unknown option {flag}\n\n{}", help());
            std::process::exit(2);
        }
        None => {}
    }
    // Started from a shell: hand the window to a copy that isn't tied to it
    // and give the prompt back, so closing that terminal leaves the window.
    if from_shell && !launch.foreground && detach(&launch.target) {
        return Ok(());
    }
    let target = launch.target.clone();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Roxaruu")
        .with_min_inner_size([520.0, 420.0]);
    // 80% of the screen, centred; a fixed size if the screen can't be asked.
    viewport = match sys::new_window_rect() {
        Some(r) => viewport.with_inner_size(r.size()).with_position(r.min),
        None => viewport.with_inner_size([1280.0, 820.0]),
    };
    // One title bar, ours. macOS: the traffic lights float over it.
    // Elsewhere: no system frame at all; ours has the window's buttons and
    // its edges resize it (`ui::window_buttons`, `window_edges`).
    if cfg!(target_os = "macos") {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
    } else {
        viewport = viewport.with_decorations(false);
    }
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON) {
        viewport = viewport.with_icon(icon);
    }
    // Terminals keep streaming while the window is in the background.
    sys::prevent_app_nap();
    let safe = match std::env::var(RENDERER_ENV) {
        Ok(r) => r == "safe",
        Err(_) => {
            let last = renderer_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
            match next_renderer(&last, sys::process_alive) {
                Start::Gpu => false,
                Start::Safe => {
                    if last.contains(':') {
                        tracing::warn!(last = last.trim(), "the last start never drew; drawing the safe way from now on");
                    }
                    true
                }
                Start::NeitherDraws => {
                    SAFE.store(true, std::sync::atomic::Ordering::Relaxed);
                    couldnt_start("Last time no window appeared, with the GPU or without.");
                }
            }
        }
    };
    SAFE.store(safe, std::sync::atomic::Ordering::Relaxed);
    remember_renderer(true);
    // A driver can hang instead of failing: no window after a while is a
    // renderer that can't start, like a crash.
    std::thread::spawn(|| {
        std::thread::sleep(FIRST_DRAW_WITHIN);
        if !DRAWN.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::error!(secs = FIRST_DRAW_WITHIN.as_secs(), "no window yet");
            couldnt_start("The window didn't appear.");
        }
    });
    let renderer = if safe && !cfg!(windows) { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu };
    tracing::info!(?renderer, safe, "opening the window");
    let result = eframe::run_native(
        "Roxaruu",
        eframe::NativeOptions {
            viewport,
            renderer,
            wgpu_options: wgpu_options(safe),
            centered: true,
            ..Default::default()
        },
        Box::new(move |cc| {
            if let Some(rs) = &cc.wgpu_render_state {
                let a = rs.adapter.get_info();
                tracing::info!(gpu = %a.name, backend = ?a.backend, driver = %a.driver_info, "graphics");
            }
            sys::quit_closes_the_window();
            // The detached copy's launcher has already exited: come to the
            // front ourselves rather than open behind the terminal.
            if launch.foreground {
                cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Ok(Box::new(Root::new(app::App::new(&cc.egui_ctx, target))))
        }),
    );
    // Remove this window's terminal-bridge shims and its open-folder marker.
    // (Dropping the app above already stopped remote access if it served
    // here; this makes sure its marker is gone, and the process exiting
    // ends the server's threads regardless.)
    termbridge::cleanup();
    instances::unregister();
    instances::unregister_remote();
    instances::clear_remote_server(true);
    if let Err(e) = &result {
        tracing::error!(error = %e, "the window couldn't open");
        if !DRAWN.load(std::sync::atomic::Ordering::Relaxed) {
            couldnt_start(&e.to_string());
        }
    }
    result
}

/// Where this run logs: `<local data>/Roxaruu/roxaruu.log` (Windows:
/// %LOCALAPPDATA%\Roxaruu; macOS: ~/Library/Application Support/Roxaruu;
/// Linux: ~/.local/share/Roxaruu). Every window adds to it; it starts over
/// once it passes 2 MB.
pub(crate) fn log_path() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("Roxaruu").join("roxaruu.log"))
}

/// How this computer draws, kept beside the log: `gpu:<pid>` / `safe:<pid>`
/// while that process tries it, `gpu` / `safe` once it drew. A GPU driver
/// can kill the process outright (no panic to catch): the next start sees a
/// try whose process is gone and draws the safe way (`SAFE`) from then on.
fn renderer_path() -> Option<std::path::PathBuf> {
    log_path().map(|p| p.with_file_name("renderer"))
}

/// Note this run's renderer: trying it, or (`trying` false) it drew.
fn remember_renderer(trying: bool) {
    let Some(path) = renderer_path() else { return };
    let name = if SAFE.load(std::sync::atomic::Ordering::Relaxed) { "safe" } else { "gpu" };
    let text = if trying { format!("{name}:{}", std::process::id()) } else { name.to_string() };
    let _ = std::fs::write(path, text);
}

#[derive(Debug, PartialEq)]
enum Start {
    Gpu,
    Safe,
    NeitherDraws,
}

/// What to start with after `last` (the renderer file); `alive` says
/// whether a process still runs (one still trying isn't a failure).
fn next_renderer(last: &str, alive: impl Fn(i32) -> bool) -> Start {
    let (name, pid) = match last.trim().split_once(':') {
        Some((n, p)) => (n, p.parse().ok()),
        None => (last.trim(), None),
    };
    let failed = pid.is_some_and(|p| !alive(p));
    match (name, failed) {
        ("safe", true) => Start::NeitherDraws,
        ("gpu", true) | ("safe", false) => Start::Safe,
        _ => Start::Gpu,
    }
}

fn init_log() {
    let Some(path) = log_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Full: the last one becomes roxaruu.log.1, so what led up to a problem
    // is still there after the next start.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 10 << 20) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    // Ours in detail (`ROXARUU_LOG=trace` for more, `info` for less); the
    // libraries' (wgpu, winit) only when they matter.
    let ours = std::env::var("ROXARUU_LOG").ok().and_then(|l| l.parse().ok()).unwrap_or(tracing::Level::DEBUG);
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(tracing::Level::INFO)
        .with_target("roxaruu", ours);
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .with_thread_names(true)
        .with_max_level(tracing::Level::TRACE)
        .finish()
        .with(filter)
        .try_init();
}

/// A crash is logged; one on the main thread before the window has drawn is
/// a window that can't start (see `couldnt_start`).
fn on_crash() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, thread = std::thread::current().name().unwrap_or("?"), "crashed");
        default(info);
        if std::thread::current().name() == Some("main") && !DRAWN.load(std::sync::atomic::Ordering::Relaxed) {
            couldnt_start(&info.to_string());
        }
    }));
}

/// The window couldn't open. Through the GPU: try once more the safe way
/// (`SAFE`) in a fresh copy (a process can open only one event loop); the
/// safe way too: say so in a dialog, with the log, rather than vanish.
fn couldnt_start(why: &str) -> ! {
    if GIVING_UP.swap(true, std::sync::atomic::Ordering::Relaxed) {
        loop {
            std::thread::park(); // the first to give up shows the dialog and exits
        }
    }
    if !SAFE.load(std::sync::atomic::Ordering::Relaxed) {
        let relaunched = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg("--foreground")
                .args(std::env::args_os().skip(1).filter(|a| a != "--foreground"))
                .env(RENDERER_ENV, "safe")
                .spawn()
        });
        match relaunched {
            Ok(_) => {
                tracing::warn!("trying again the safe way");
                std::process::exit(0);
            }
            Err(e) => tracing::error!(error = %e, "couldn't start a copy the safe way"),
        }
    }
    // Next start tries everything again (a driver update may have fixed it).
    if let Some(p) = renderer_path() {
        let _ = std::fs::remove_file(p);
    }
    let log = log_path().map(|p| p.display().to_string()).unwrap_or_default();
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Roxaruu couldn't start")
        .set_description(format!(
            "Roxaruu couldn't open its window on this computer's graphics (tried the GPU, then without it).\n\n{why}\n\nDetails are in {log}"
        ))
        .show();
    std::process::exit(1);
}

/// `roxaruu --help`.
fn help() -> String {
    format!(
        "Roxaruu {} — a keyboard-first editor with terminals.

Usage:
  rox                  Open the folder you're in (same as `rox .`).
  rox <folder>         Open a folder; if it's already open, go to its window.
  rox <file>           Open the file, in its project (the git repo it's in).
  rox --foreground ... Stay attached to this terminal (logs show here).
                       Otherwise the window detaches: you get the prompt
                       back, and closing the terminal leaves it open.
  rox -h | --help      Show this help.
  rox -V | --version   Show the version.

`rox` is short for `roxaruu`; both work.

Inside: F1 shows every keyboard shortcut for where you are, ⇧⌘P runs any
command by name. Settings, keys and themes are JSON in ~/.config/oxru/.",
        VERSION
    )
}

/// Asked for something other than a window.
#[derive(Debug, PartialEq)]
enum Info {
    Help,
    Version,
    Unknown(String),
}

/// What the command line asked for.
#[derive(Debug, PartialEq)]
struct Launch {
    /// `--help`, `--version`, or an option we don't know: print, no window.
    info: Option<Info>,
    /// `--foreground`: stay attached to the terminal (the detached copy is
    /// started with it; handy for seeing logs too).
    foreground: bool,
    /// `roxaruu [folder | file]`. Bare `roxaruu` (or `rox`) in a terminal
    /// opens the folder you're in, same as `roxaruu .`; from the Dock or
    /// Finder there's no terminal, and it opens empty.
    target: Option<std::path::PathBuf>,
}

impl Launch {
    fn parse(args: impl Iterator<Item = std::ffi::OsString>, from_shell: bool) -> Launch {
        let mut args = args.peekable();
        let info = args.peek().and_then(|a| match a.to_str() {
            Some("-h" | "--help") => Some(Info::Help),
            Some("-V" | "--version") => Some(Info::Version),
            Some(f) if f.starts_with('-') && f != "-" && f != "--foreground" => {
                Some(Info::Unknown(f.to_string()))
            }
            _ => None,
        });
        let foreground = args.next_if(|a| a == "--foreground").is_some();
        let target = args
            .next()
            .map(std::path::PathBuf::from)
            .or_else(|| from_shell.then(std::env::current_dir).and_then(Result::ok))
            // Absolute, so the copy opens the same thing wherever it starts.
            .map(|t| std::path::absolute(&t).unwrap_or(t));
        Launch { info, foreground, target }
    }
}

/// Start this program again, detached: its own process group, no terminal
/// for input or output. Returns whether it started.
fn detach(target: &Option<std::path::PathBuf>) -> bool {
    match std::env::current_exe() {
        Ok(exe) => spawn_detached(&exe, target.iter().map(|t| t.as_os_str())).is_ok(),
        Err(_) => false,
    }
}

fn spawn_detached<'a>(
    exe: &std::path::Path,
    args: impl Iterator<Item = &'a std::ffi::OsStr>,
) -> std::io::Result<std::process::Child> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(exe);
    cmd.arg("--foreground")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn()
}

/// The window: the app, and its side of remote access (which stops when
/// this is dropped: the window closed or the app quit).
pub(crate) struct Root(pub(crate) app::App, remote::Remote);

impl Root {
    pub(crate) fn new(app: app::App) -> Root {
        // Remote access starts off, and no password from an earlier build
        // stays on disk.
        remote::forget_saved_password();
        Root(app, remote::Remote::default())
    }
}

/// wgpu as eframe sets it up, but on Windows through DirectX 12 only: the
/// system's own graphics, which every Windows 10/11 has. Vulkan, wgpu's other
/// choice there, comes from the GPU vendor and is where overlays and old
/// drivers crash or hang. (`WGPU_BACKEND` still overrides.) `safe`: Windows'
/// software DirectX (WARP), which needs no GPU driver at all.
fn wgpu_options(safe: bool) -> egui_wgpu::WgpuConfiguration {
    let mut config = egui_wgpu::WgpuConfiguration::default();
    if cfg!(windows)
        && let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup
    {
        setup.instance_descriptor.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12);
        if safe {
            setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, _| {
                adapters
                    .iter()
                    .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
                    .cloned()
                    .ok_or_else(|| "no software DirectX (WARP)".to_string())
            }));
        }
    }
    config
}

/// Off macOS the screen's size is known only once the window is on it:
/// 80% of it, around where the window was centred (`centered`).
fn fit_to_screen(ctx: &egui::Context) {
    if cfg!(target_os = "macos") {
        return;
    }
    let (monitor, outer) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().outer_rect));
    let (Some(monitor), Some(outer)) = (monitor, outer) else {
        return;
    };
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, monitor);
    let size = sys::window_rect(screen, screen).size();
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(outer.center() - size / 2.0));
}

impl eframe::App for Root {
    /// Runs even while the window is hidden, covered or minimized: remote
    /// access draws the browser's copy of the window from here.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.1.update(&mut self.0, ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.frame(ui);
        if !DRAWN.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::info!("the window is up");
            remember_renderer(false);
            fit_to_screen(ui.ctx());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn a_renderer_that_never_drew_is_not_tried_again() {
        let dead = |_| false;
        let alive = |_| true;
        assert_eq!(next_renderer("", dead), Start::Gpu);
        assert_eq!(next_renderer("gpu", dead), Start::Gpu);
        assert_eq!(next_renderer("gpu:42\n", alive), Start::Gpu, "another window is still starting");
        assert_eq!(next_renderer("gpu:42", dead), Start::Safe);
        assert_eq!(next_renderer("safe", dead), Start::Safe);
        assert_eq!(next_renderer("safe:42", alive), Start::Safe);
        assert_eq!(next_renderer("safe:42", dead), Start::NeitherDraws);
    }

    fn parse(args: &[&str], from_shell: bool) -> Launch {
        Launch::parse(args.iter().map(OsString::from), from_shell)
    }

    #[test]
    fn bare_rox_in_a_terminal_opens_the_folder_youre_in() {
        let here = std::env::current_dir().unwrap();
        assert_eq!(parse(&[], true).target, Some(here.clone()));
        assert_eq!(parse(&["."], true).target.map(|t| t.canonicalize().unwrap()), Some(here.canonicalize().unwrap()));
        assert_eq!(parse(&[], false).target, None, "from the Dock: empty");
        // An absolute path here (a drive and backslashes on Windows) stays as it is.
        let file = std::env::temp_dir().join("x").join("y.rs");
        let f = file.to_string_lossy().into_owned();
        assert_eq!(parse(&[&f], false).target, Some(file.clone()));
        assert!(parse(&["src"], true).target.unwrap().is_absolute());
    }

    #[test]
    fn help_version_and_unknown_options_open_no_window() {
        assert_eq!(parse(&["-h"], true).info, Some(Info::Help));
        assert_eq!(parse(&["--help"], true).info, Some(Info::Help));
        assert_eq!(parse(&["--version"], true).info, Some(Info::Version));
        assert_eq!(parse(&["-V"], true).info, Some(Info::Version));
        assert_eq!(parse(&["--nope"], true).info, Some(Info::Unknown("--nope".into())));
        assert_eq!(parse(&["src"], true).info, None);
        assert_eq!(parse(&["--foreground"], true).info, None);
        let h = help();
        for flag in ["--help", "--version", "--foreground", "rox <folder>"] {
            assert!(h.contains(flag), "{flag}");
        }
    }

    #[test]
    fn foreground_flag_comes_first_and_keeps_the_target() {
        let dir = std::env::temp_dir().join("x");
        let d = dir.to_string_lossy().into_owned();
        let l = parse(&["--foreground", &d], true);
        assert!(l.foreground);
        assert_eq!(l.target, Some(dir));
        assert!(!parse(&[&d], true).foreground);
    }

    /// The detached copy gets `--foreground`, its own process group, and no
    /// terminal: `sleep` stands in for the app.
    #[cfg(unix)]
    #[test]
    fn the_detached_copy_has_its_own_group_and_no_terminal() {
        let script = std::env::temp_dir().join(format!("detach-probe-{}", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\n[ \"$1\" = --foreground ] && [ ! -t 0 ] && sleep 5\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut child = spawn_detached(&script, std::iter::empty()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(child.try_wait().unwrap().is_none(), "got --foreground and no terminal");
        let pgid = std::process::Command::new("ps")
            .args(["-o", "pgid=", "-p", &child.id().to_string()])
            .output()
            .unwrap();
        let pgid: u32 = String::from_utf8_lossy(&pgid.stdout).trim().parse().unwrap();
        assert_eq!(pgid, child.id(), "leads its own process group");
        let _ = child.kill();
        let _ = std::fs::remove_file(&script);
    }
}
