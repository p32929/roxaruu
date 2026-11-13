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
// Helpers for the UI walkthroughs, which need Unix shells (`headless`).
#![cfg_attr(all(test, not(unix)), allow(dead_code, unused_imports))]

mod app;
mod buffer;
mod config;
mod editor;
mod find;
mod fonts;
mod fstree;
mod fuzzy;
mod huge;
// The UI walkthroughs drive real Unix shells (zsh), symlinks and file modes
// (Unix only: inside). `cfg(test)` alone, so clippy knows it's test code.
#[cfg(test)]
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
/// software DirectX (WARP, in every Windows 10/11), OpenGL elsewhere (Mesa
/// draws it in software if it must). Both through wgpu, the one renderer.
static SAFE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Already saying the window can't open (two threads mustn't say it twice).
static GIVING_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() -> eframe::Result {
    use std::io::IsTerminal;
    init_log();
    tracing::info!(version = VERSION, os = std::env::consts::OS, pid = std::process::id(), "starting");
    tracing::info!(
        args = ?redacted(std::env::args_os().skip(1)),
        exe = ?std::env::current_exe().ok(),
        cwd = ?std::env::current_dir().ok(),
        config = %config::dir().display(),
        home = ?config::home(),
        "where"
    );
    on_crash();
    let from_shell = std::io::stdin().is_terminal();
    let mut launch = Launch::parse(std::env::args_os().skip(1));
    // Typed in a terminal with nothing after it (`rox`, `roxaruu`): the
    // folder you're in. Opened from the app list, the Dock or the Start Menu
    // (no terminal), or `rox --remote on …` without a folder: nothing.
    if launch.target.is_none() && opens_here(std::env::args_os().len() == 1, from_shell) {
        launch.target = std::env::current_dir().ok();
    }
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
    // `--remote off`, or `on` while it's already on: nothing to open.
    match &launch.remote {
        Some(RemoteArg::Off) => {
            let was = instances::remote_server().is_some();
            instances::clear_remote_server(false);
            println!("{}", if was { "Remote access is off." } else { "Remote access was already off." });
            return Ok(());
        }
        Some(RemoteArg::On(_)) if let Some(s) = instances::remote_server() => {
            println!("Remote access is already on: {}", remote::address(s.port));
            return Ok(());
        }
        _ => {}
    }
    // `--remote on`: the password from `--password`, or (the detached copy)
    // from its stdin, so it's never on disk, in an environment or in a
    // command line of ours.
    let password = match &launch.remote {
        Some(RemoteArg::On(Some(p))) => Some(p.clone()),
        Some(RemoteArg::On(None)) if !from_shell => {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            Some(line.trim_end_matches(['\r', '\n']).to_string())
        }
        _ => None,
    };
    if launch.remote.is_some() && password.as_deref().is_none_or(str::is_empty) {
        eprintln!("roxaruu: --remote on needs --password <password>");
        std::process::exit(2);
    }
    // Started from a shell: hand the window to a copy that isn't tied to it
    // and give the prompt back, so closing that terminal leaves the window.
    // With no screen at all (a server, from a script or `ssh host rox …`
    // too): to a copy on a virtual one.
    if (from_shell || no_screen()) && !launch.foreground {
        if no_screen() && which::which("xvfb-run").is_err() {
            eprintln!("roxaruu: there's no screen here to open a window on; on a server, install Xvfb (sudo apt install xvfb)");
            std::process::exit(1);
        }
        if let Some(window) = detach(&launch.target, password.as_deref()) {
            if password.is_some() {
                say_where_remote_is(window);
            }
            return Ok(());
        }
    }
    let (target, virtual_screen) = (launch.target.clone(), launch.virtual_screen);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Roxaruu")
        .with_min_inner_size([520.0, 420.0]);
    // 80% of the screen, centred; a fixed size if the screen can't be asked.
    viewport = match sys::new_window_rect() {
        // On a virtual screen nobody sees the window: the smallest it can be,
        // since a machine without a GPU draws every pixel on the CPU.
        _ if virtual_screen => viewport.with_inner_size([520.0, 420.0]),
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
    // Linux: who this window is (Wayland's app id, X11's WM_CLASS), so the
    // desktop puts roxaruu.desktop's icon on it and groups it in the dock.
    viewport = viewport.with_app_id("roxaruu");
    // Terminals keep streaming while the window is in the background.
    sys::prevent_app_nap();
    let safe = match std::env::var(RENDERER_ENV) {
        Ok(r) => r == "safe",
        Err(_) => {
            let read = || renderer_path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
            // Another start still trying (it hasn't drawn, and runs): wait
            // until it draws or dies, however long a slow machine takes —
            // then that's the answer for this one too.
            let mut last = read();
            while let Some(pid) = last.trim().split_once(':').and_then(|(_, p)| p.parse::<i32>().ok())
                && pid != std::process::id() as i32
                && sys::process_alive(pid)
            {
                std::thread::sleep(std::time::Duration::from_millis(100));
                last = read();
            }
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
    // Until the window draws, the file says this start is still trying: a
    // start that crashes leaves that behind, and the next one goes the safe
    // way. (No time limit: a slow machine is given as long as it takes.)
    remember_renderer(true);
    let renderer = eframe::Renderer::Wgpu;
    tracing::info!(safe, "opening the window");
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
            let mut root = Root::new(app::App::new(&cc.egui_ctx, target));
            root.2 = virtual_screen.then_some(false);
            root.1.virtual_screen = virtual_screen;
            if let Some(password) = password {
                root.0.remote_want = Some(remote::Want::On(password, false));
            }
            Ok(Box::new(root))
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
    sys::alert(
        "Roxaruu couldn't start",
        &format!(
            "Roxaruu couldn't open its window on this computer's graphics (tried the GPU, then without it).\n\n{why}\n\nDetails are in {log}"
        ),
    );
    std::process::exit(1);
}

/// `roxaruu --help`.
fn help() -> String {
    format!(
        "Roxaruu {} — a keyboard-first editor with terminals.

Usage:
  rox                  Open the folder you're in (same as `rox .`). From the
                       app list or the Dock: no folder, you pick one.
  rox <folder>         Open a folder; if it's already open, go to its window.
  rox <file>           Open the file, in its project (the git repo it's in).
  rox --foreground ... Stay attached to this terminal (logs show here).
                       Otherwise the window detaches: you get the prompt
                       back, and closing the terminal leaves it open.
  rox --remote on --password <password> [folder]
                       Turn remote access on: a window serves it (on a
                       server with no screen, on a virtual one, via Xvfb)
                       and this prints the address to open in a browser.
                       The password is kept in memory only.
  rox --remote off     Turn remote access off, wherever it runs.
  rox -h | --help      Show this help.
  rox -V | --version   Show the version.

`rox` is short for `roxaruu`; both work.

Inside: F1 shows every keyboard shortcut for where you are, ⇧⌘P runs any
command by name. Settings, keys and themes are JSON in ~/.config/roxaruu/.",
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

/// `--remote on|off`. `On(None)`: the password comes on stdin (the
/// detached copy).
#[derive(Debug, PartialEq)]
enum RemoteArg {
    On(Option<String>),
    Off,
}

/// What the command line asked for.
#[derive(Debug, PartialEq)]
struct Launch {
    /// `--help`, `--version`, or an option we don't know: print, no window.
    info: Option<Info>,
    /// `--foreground`: stay attached to the terminal (the detached copy is
    /// started with it; handy for seeing logs too).
    foreground: bool,
    /// `--remote on|off` (with `--password`).
    remote: Option<RemoteArg>,
    /// `--virtual-screen`: started on Xvfb by `detach` (no real screen).
    virtual_screen: bool,
    /// `roxaruu [folder | file]`. Bare, it's decided in `main` (`opens_here`):
    /// typed in a terminal, the folder you're in; else nothing.
    target: Option<std::path::PathBuf>,
}

impl Launch {
    fn parse(args: impl Iterator<Item = std::ffi::OsString>) -> Launch {
        let (mut info, mut foreground, mut remote, mut password, mut target) = (None, false, None, None, None);
        let mut virtual_screen = false;
        let mut args = args.peekable();
        while let Some(a) = args.next() {
            match a.to_str() {
                Some("-h" | "--help") => info = info.or(Some(Info::Help)),
                Some("-V" | "--version") => info = info.or(Some(Info::Version)),
                Some("--foreground") => foreground = true,
                Some("--virtual-screen") => virtual_screen = true,
                Some("--remote") => match args.next().as_ref().and_then(|v| v.to_str()) {
                    Some("on") => remote = Some(true),
                    Some("off") => remote = Some(false),
                    _ => info = info.or(Some(Info::Unknown("--remote (say on or off)".into()))),
                },
                Some("--password") => password = args.next().and_then(|v| v.into_string().ok()),
                Some(f) if f.starts_with('-') && f != "-" => info = info.or(Some(Info::Unknown(f.to_string()))),
                _ if target.is_none() => target = Some(std::path::PathBuf::from(a)),
                _ => {}
            }
        }
        // Absolute, so the copy opens the same thing wherever it starts.
        let target = target.map(|t| std::path::absolute(&t).unwrap_or(t));
        let remote = remote.map(|on| if on { RemoteArg::On(password) } else { RemoteArg::Off });
        Launch { info, foreground, remote, virtual_screen, target }
    }
}

/// Whether a start with no folder opens the one it's started in: only typed
/// bare in a terminal.
fn opens_here(bare: bool, from_terminal: bool) -> bool {
    bare && from_terminal
}

/// The arguments, for the log: never a password.
fn redacted(args: impl Iterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut hide = false;
    args.map(|a| {
        let out = if hide { "***".into() } else { a.clone() };
        hide = a == "--password";
        out
    })
    .collect()
}

/// Linux with no screen to draw on (a server): the window goes on a
/// virtual one, through `xvfb-run`.
fn no_screen() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

/// Start this program again, detached: its own process group, no terminal
/// for input or output (on a virtual screen if there's no real one), the
/// password for remote access, if any, on its stdin. Returns it, if it
/// started.
fn detach(target: &Option<std::path::PathBuf>, password: Option<&str>) -> Option<std::process::Child> {
    let exe = std::env::current_exe().ok()?;
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    if password.is_some() {
        args.extend(["--remote".into(), "on".into()]);
    }
    args.extend(target.iter().map(|t| t.clone().into_os_string()));
    let xvfb = no_screen().then(|| which::which("xvfb-run").ok()).flatten();
    if xvfb.is_some() {
        args.push("--virtual-screen".into());
    }
    let cmd = match xvfb {
        Some(xvfb) => {
            let mut c = std::process::Command::new(xvfb);
            c.args(["-a", "-s", "-screen 0 1920x1080x24"]).arg(&exe);
            c
        }
        None => std::process::Command::new(&exe),
    };
    spawn_detached(cmd, args.iter().map(|a| a.as_os_str()), password).ok()
}

fn spawn_detached<'a>(
    mut cmd: std::process::Command,
    args: impl Iterator<Item = &'a std::ffi::OsStr>,
    stdin: Option<&str>,
) -> std::io::Result<std::process::Child> {
    use std::io::Write;
    use std::process::Stdio;
    cmd.arg("--foreground")
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn()?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = writeln!(pipe, "{text}"); // closed when dropped
    }
    Ok(child)
}

/// After `--remote on`: wait for the new window to serve (however long it
/// takes to start), then say where — or, if it ended first, that it did.
fn say_where_remote_is(mut window: std::process::Child) {
    loop {
        if let Some(s) = instances::remote_server() {
            println!("Remote access is on: {}", remote::address(s.port));
            return;
        }
        if let Ok(Some(status)) = window.try_wait() {
            let log = log_path().map(|p| p.display().to_string()).unwrap_or_default();
            eprintln!("roxaruu: the window ended ({status}) before serving; see {log}");
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The window: the app, and its side of remote access (which stops when
/// this is dropped: the window closed or the app quit).
/// `.2`: on a virtual screen (`--virtual-screen`), whether remote access was
/// on last pass — nobody can see such a window but through it.
pub(crate) struct Root(pub(crate) app::App, remote::Remote, Option<bool>);

impl Root {
    pub(crate) fn new(app: app::App) -> Root {
        // Remote access starts off, and no password from an earlier build
        // stays on disk.
        remote::forget_saved_password();
        Root(app, remote::Remote::default(), None)
    }
}

/// wgpu as eframe sets it up, but on Windows through DirectX 12 only: the
/// system's own graphics, which every Windows 10/11 has. Vulkan, wgpu's other
/// choice there, comes from the GPU vendor and is where overlays and old
/// drivers crash or hang. (`WGPU_BACKEND` still overrides.) `safe`: Windows'
/// software DirectX (WARP), which needs no GPU driver at all.
fn wgpu_options(safe: bool) -> egui_wgpu::WgpuConfiguration {
    let mut config = egui_wgpu::WgpuConfiguration::default();
    // The safe way off Windows and macOS (Metal always works there): OpenGL,
    // which Mesa draws in software if the GPU's own driver can't.
    if safe
        && cfg!(not(any(windows, target_os = "macos")))
        && let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup
    {
        setup.instance_descriptor.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::GL);
    }
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
        // On a virtual screen the window is for remote access only: once
        // that's off, it goes (so `--remote on` again doesn't add another).
        if let Some(was_on) = &mut self.2 {
            let on = self.0.remote.on;
            if std::mem::replace(was_on, on) && !on {
                self.0.close_unseen(ctx);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // On a virtual screen, while browsers look, their copy runs the app:
        // drawing this one too (that nobody sees) would only take the CPU
        // from theirs. Otherwise it runs as usual (terminals keep being read).
        if self.2.is_none() || self.0.remote.clients == 0 {
            self.0.frame(ui);
        }
        if !DRAWN.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::info!("the window is up");
            remember_renderer(false);
            if self.2.is_none() {
                fit_to_screen(ui.ctx());
            }
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

    fn parse(args: &[&str]) -> Launch {
        Launch::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn bare_rox_opens_the_folder_youre_in_and_a_path_opens_it() {
        let here = std::env::current_dir().unwrap();
        assert!(opens_here(true, true), "rox, typed in a terminal: the folder you're in");
        assert!(!opens_here(true, false), "from the app list, Dock, Start Menu: nothing");
        assert!(!opens_here(false, true), "rox --remote on (no folder): nothing");
        assert_eq!(parse(&[]).target, None, "parsing alone never guesses");
        assert_eq!(parse(&["."]).target.map(|t| t.canonicalize().unwrap()), Some(here.canonicalize().unwrap()));
        // An absolute path here (a drive and backslashes on Windows) stays as it is.
        let file = std::env::temp_dir().join("x").join("y.rs");
        let f = file.to_string_lossy().into_owned();
        assert_eq!(parse(&[&f]).target, Some(file.clone()));
        assert!(parse(&["src"]).target.unwrap().is_absolute());
    }

    #[test]
    fn help_version_and_unknown_options_open_no_window() {
        assert_eq!(parse(&["-h"]).info, Some(Info::Help));
        assert_eq!(parse(&["--help"]).info, Some(Info::Help));
        assert_eq!(parse(&["--version"]).info, Some(Info::Version));
        assert_eq!(parse(&["-V"]).info, Some(Info::Version));
        assert_eq!(parse(&["--nope"]).info, Some(Info::Unknown("--nope".into())));
        assert_eq!(parse(&["src"]).info, None);
        assert_eq!(parse(&["--foreground"]).info, None);
        let h = help();
        for flag in ["--help", "--version", "--foreground", "rox <folder>"] {
            assert!(h.contains(flag), "{flag}");
        }
    }

    #[test]
    fn remote_on_and_off_from_the_command_line() {
        let l = parse(&["--remote", "on", "--password", "0000"]);
        assert_eq!((l.info, l.remote), (None, Some(RemoteArg::On(Some("0000".into())))));
        // Any order, with a folder.
        let dir = std::env::temp_dir().join("x");
        let d = dir.to_string_lossy().into_owned();
        let l = parse(&["--password", "pw", &d, "--remote", "on"]);
        assert_eq!((l.remote, l.target), (Some(RemoteArg::On(Some("pw".into()))), Some(dir)));
        // The detached copy: the password comes on stdin.
        assert_eq!(parse(&["--foreground", "--remote", "on"]).remote, Some(RemoteArg::On(None)));
        assert_eq!(parse(&["--remote", "off"]).remote, Some(RemoteArg::Off));
        assert!(matches!(parse(&["--remote", "maybe"]).info, Some(Info::Unknown(_))));
        assert!(matches!(parse(&["--remote"]).info, Some(Info::Unknown(_))));
        assert!(help().contains("--remote on --password"));
        assert!(parse(&["--foreground", "--virtual-screen", "--remote", "on"]).virtual_screen);
        assert!(!parse(&["--remote", "on"]).virtual_screen);
    }

    #[test]
    fn the_log_never_gets_the_password() {
        let args = ["--remote", "on", "--password", "secret", "src"].map(OsString::from);
        let logged = redacted(args.into_iter());
        assert_eq!(logged, ["--remote", "on", "--password", "***", "src"].map(OsString::from));
    }

    #[test]
    fn foreground_flag_comes_first_and_keeps_the_target() {
        let dir = std::env::temp_dir().join("x");
        let d = dir.to_string_lossy().into_owned();
        let l = parse(&["--foreground", &d]);
        assert!(l.foreground);
        assert_eq!(l.target, Some(dir));
        assert!(!parse(&[&d]).foreground);
    }

    /// The detached copy gets `--foreground`, its own process group, and no
    /// terminal: `sleep` stands in for the app.
    #[cfg(unix)]
    #[test]
    fn the_detached_copy_has_its_own_group_and_no_terminal() {
        let script = std::env::temp_dir().join(format!("detach-probe-{}", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\n[ \"$1\" = --foreground ] && [ ! -t 0 ] && sleep 5\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut child = spawn_detached(std::process::Command::new(&script), std::iter::empty(), None).unwrap();
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
