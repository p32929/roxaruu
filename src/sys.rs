//! **The** unsafe code. Every `unsafe` block in Oxru is in this file, behind a
//! safe function; the rest of the crate is `deny(unsafe_code)`, so a new one
//! can't appear anywhere else without failing the build.
//!
//! They are calls into libc for things the standard library doesn't expose:
//! asking whether a process is alive, what it is called, and what it costs.
//! There's no safe-Rust
//! way to do any of them — the crates that offer these (`nix`, `sysinfo`,
//! `libproc`) wrap the same C calls in the same `unsafe`, so depending on one
//! would move this code out of sight rather than remove it. Keeping it here,
//! a few calls in one small file, means the whole unsafe surface of the app can
//! be read in a minute.
//!
//! What each wrapper has to guarantee is written above it. They take plain
//! integers and return owned values, so no pointer or lifetime obligation
//! escapes this module.

// The one exemption from the crate-wide `deny(unsafe_code)` in `main.rs`.
#![allow(unsafe_code)]

/// Whether process `pid` is still running.
///
/// Signal 0 asks the kernel to do the permission and existence checks without
/// delivering anything, which is the standard liveness probe. Used to tell a
/// live Oxru instance from a stale marker file left by one that was killed.
///
/// Safety: `kill` takes two integers and returns one. Non-positive pids are
/// rejected first because they have special meanings — 0 is "every process in
/// my group" and negative values are whole process groups, so passing one
/// through would probe something we never meant to ask about.
#[cfg(unix)]
pub fn process_alive(pid: i32) -> bool {
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    if pid <= 0 {
        return false;
    }
    #[allow(unsafe_code)]
    let rc = unsafe { kill(pid, 0) };
    rc == 0
}

/// Windows: whether `pid` is a process that hasn't exited.
///
/// Safety: the handle is checked for null before use and always closed; the
/// exit code is written into a local.
#[cfg(windows)]
pub fn process_alive(pid: i32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if pid <= 0 {
        return false;
    }
    #[allow(unsafe_code)]
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

/// Off macOS, what the system knows of process `pid` (one cross-platform
/// library: `/proc` on Linux, the process APIs on Windows).
#[cfg(not(target_os = "macos"))]
fn with_process<T>(pid: i32, f: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    if pid <= 0 {
        return None;
    }
    let pid = Pid::from_u32(pid as u32);
    let mut sys = System::new().ok()?;
    let kind = ProcessRefreshKind::nothing().with_memory().with_cpu().with_cwd(UpdateKind::Always);
    // Windows lists every process to find one (asking for just one there
    // found nothing on GitHub's runners); Linux reads /proc/<pid> alone.
    let which = if cfg!(windows) { ProcessesToUpdate::All } else { ProcessesToUpdate::Some(&[pid]) };
    sys.refresh_processes_specifics(which, true, kind);
    sys.process(pid).map(f)
}

/// The working directory of process `pid` (a terminal's shell, to follow
/// `cd`). macOS asks `lsof` (see `terminalpane`).
#[cfg(not(target_os = "macos"))]
pub fn process_cwd(pid: i32) -> Option<std::path::PathBuf> {
    with_process(pid, |p| p.cwd().map(|c| c.to_path_buf())).flatten()
}

/// What a terminal's shell `shell` is running now, where there's no
/// foreground process group to ask (Windows' consoles): the command you ran,
/// its newest child (not that command's own helpers — Claude Code's `rg`,
/// `git`), past the console host Windows puts beside it. `None`: just the
/// shell.
#[cfg(windows)]
pub fn foreground_process(shell: u32) -> Option<i32> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new().ok()?;
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let ignore = |p: &sysinfo::Process| {
        let n = p.name().to_string_lossy().to_lowercase();
        n == "conhost.exe" || n == "openconsole.exe"
    };
    let shell = Pid::from_u32(shell);
    sys.processes()
        .values()
        .filter(|p| p.parent() == Some(shell) && !ignore(p))
        .max_by_key(|p| p.start_time())
        .map(|p| p.pid().as_u32() as i32)
}

/// The short name of process `pid` — `node`, `cargo`, `vim` — used to label a
/// terminal tab by whatever it's running.
///
/// Safety: `proc_name` writes at most `buffersize` bytes into the buffer and
/// returns how many it wrote. The buffer is a live local array and its true
/// length is what's passed, so the call can't write past it. The return value
/// is clamped to the buffer length before it's used as a slice bound, in case
/// a future libc returns something larger, and the bytes are read with
/// `from_utf8_lossy` since a process name is not guaranteed to be UTF-8.
#[cfg(target_os = "macos")]
pub fn process_name(pid: i32) -> Option<String> {
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn proc_name(pid: i32, buffer: *mut std::ffi::c_void, buffersize: u32) -> i32;
    }
    if pid <= 0 {
        return None;
    }
    let mut buf = [0u8; 256];
    #[allow(unsafe_code)]
    let n = unsafe { proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let len = (n as usize).min(buf.len());
    Some(String::from_utf8_lossy(&buf[..len]).into_owned())
}

#[cfg(not(target_os = "macos"))]
pub fn process_name(pid: i32) -> Option<String> {
    with_process(pid, |p| {
        let name = p.name().to_string_lossy().into_owned();
        // Windows names end in .exe (or .EXE); the tab says "node", not "node.exe".
        match name.len().checked_sub(4) {
            Some(n) if name.is_char_boundary(n) && name[n..].eq_ignore_ascii_case(".exe") => name[..n].to_string(),
            _ => name,
        }
    })
}

/// Where the mouse pointer is, in points from the top-left of the window
/// whose content area is `inner` (egui's `ViewportInfo::inner_rect`).
///
/// Needed for Finder drops: during a drag macOS sends no mouse-moved events,
/// so egui's own pointer position is wherever it last left the window. These
/// AppKit calls are safe; they live here with the other OS calls.
#[cfg(all(target_os = "macos", not(test)))]
pub fn pointer_in_window(inner: eframe::egui::Rect) -> Option<eframe::egui::Pos2> {
    use objc2_app_kit::{NSEvent, NSScreen};
    let mtm = objc2::MainThreadMarker::new()?;
    // Cocoa's origin is the primary screen's bottom-left, y up; egui's (via
    // winit) is its top-left, y down.
    let primary = NSScreen::screens(mtm).firstObject()?.frame();
    let at = NSEvent::mouseLocation();
    let (x, y) = (at.x as f32, (primary.size.height - at.y) as f32);
    Some(eframe::egui::pos2(x - inner.left(), y - inner.top()))
}

/// Where a new window opens (egui points, from the primary screen's
/// top-left): 80% of the screen's width and height, centred in what the
/// menu bar and Dock leave free. The screen in use (with the key window).
#[cfg(target_os = "macos")]
pub fn new_window_rect() -> Option<eframe::egui::Rect> {
    use eframe::egui::{Rect, pos2, vec2};
    use objc2_app_kit::NSScreen;
    let mtm = objc2::MainThreadMarker::new()?;
    let primary = NSScreen::screens(mtm).firstObject()?.frame();
    let screen = NSScreen::mainScreen(mtm)?;
    // Cocoa's origin is the primary screen's bottom-left, y up.
    let flip = |r: objc2_foundation::NSRect| {
        Rect::from_min_size(
            pos2(r.origin.x as f32, (primary.size.height - r.origin.y - r.size.height) as f32),
            vec2(r.size.width as f32, r.size.height as f32),
        )
    };
    Some(window_rect(flip(screen.frame()), flip(screen.visibleFrame())))
}

#[cfg(not(target_os = "macos"))]
pub fn new_window_rect() -> Option<eframe::egui::Rect> {
    None
}

/// 80% of `screen` each way (no bigger than `visible`), centred in `visible`.
pub fn window_rect(screen: eframe::egui::Rect, visible: eframe::egui::Rect) -> eframe::egui::Rect {
    let size = (screen.size() * 0.8).min(visible.size());
    eframe::egui::Rect::from_center_size(visible.center(), size)
}

/// What process `pid` costs: memory as Activity Monitor's "Memory" column
/// counts it (the physical footprint), and CPU time used so far.
pub struct Usage {
    pub footprint_kb: u64,
    #[cfg_attr(not(test), allow(dead_code))] // the idle check reads it
    pub cpu: std::time::Duration,
}

/// Safety: `proc_pid_rusage` fills a caller-owned struct of the flavor asked
/// for. The buffer is a local `rusage_info_v2` laid out exactly as the C one
/// (a 16-byte uuid then eighteen `u64`s), and `RUSAGE_INFO_V2` (2) names that
/// flavor, so the kernel writes exactly its size. Non-positive pids are
/// rejected first, as in `process_alive`.
#[cfg(target_os = "macos")]
pub fn usage(pid: i32) -> Option<Usage> {
    #[repr(C)]
    struct RusageInfoV2 {
        uuid: [u8; 16],
        user_time: u64,
        system_time: u64,
        pkg_idle_wkups: u64,
        interrupt_wkups: u64,
        pageins: u64,
        wired_size: u64,
        resident_size: u64,
        phys_footprint: u64,
        rest: [u64; 10],
    }
    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV2) -> i32;
        fn mach_timebase_info(info: *mut [u32; 2]) -> i32;
    }
    if pid <= 0 {
        return None;
    }
    let mut info = RusageInfoV2 {
        uuid: [0; 16],
        user_time: 0,
        system_time: 0,
        pkg_idle_wkups: 0,
        interrupt_wkups: 0,
        pageins: 0,
        wired_size: 0,
        resident_size: 0,
        phys_footprint: 0,
        rest: [0; 10],
    };
    #[allow(unsafe_code)]
    let rc = unsafe { proc_pid_rusage(pid, 2, &mut info) };
    // CPU times are in Mach ticks; the timebase turns them into nanoseconds
    // (1:1 on Intel, 125:3 on Apple silicon).
    let mut base = [1u32, 1u32];
    #[allow(unsafe_code)]
    let _ = unsafe { mach_timebase_info(&mut base) };
    let ticks = info.user_time + info.system_time;
    let nanos = ticks as u128 * base[0] as u128 / base[1].max(1) as u128;
    (rc == 0).then(|| Usage {
        footprint_kb: info.phys_footprint / 1024,
        cpu: std::time::Duration::from_nanos(nanos as u64),
    })
}

#[cfg(not(target_os = "macos"))]
pub fn usage(pid: i32) -> Option<Usage> {
    with_process(pid, |p| Usage {
        footprint_kb: p.memory() / 1024,
        cpu: std::time::Duration::from_millis(p.accumulated_cpu_time()),
    })
}

/// Keep macOS **App Nap** off for the life of the process.
///
/// A backgrounded app gets its threads throttled and then suspended, PTY
/// readers included, so a terminal you're watching from another window stops
/// updating until you click back in (Oxru proved this with logs). Holding an
/// `NSProcessInfo` activity is the sanctioned fix, the one iTerm2 and
/// Terminal use. `LatencyCritical` also stops the run loop's redraw timer from
/// being coalesced to 1–2 s while the window is covered; idle sleep of the
/// whole machine is still allowed. Safe AppKit calls; here with the other OS
/// calls.
#[cfg(target_os = "macos")]
pub fn prevent_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let reason = NSString::from_str("Keeps embedded terminals live in the background");
    let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep
            | NSActivityOptions::LatencyCritical,
        &reason,
    );
    // Dropping the token would end the activity.
    std::mem::forget(token);
}

#[cfg(not(target_os = "macos"))]
pub fn prevent_app_nap() {}

/// The app menu's "Quit Roxaruu" (⌘Q) ends the app on the spot: AppKit
/// takes ⌘Q before the window sees the key, and the window is never asked.
/// Make it close the window instead (as its close button does), so the
/// window asks first ("Close this window?", `Ask::Quit`). Call once the
/// app has launched (the menu exists by then).
#[cfg(target_os = "macos")]
pub fn quit_closes_the_window() {
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(menu) = NSApplication::sharedApplication(mtm).mainMenu() else { return };
    for sub in menu.itemArray().iter().filter_map(|i| i.submenu()) {
        for item in sub.itemArray().iter().filter(|i| i.action() == Some(sel!(terminate:))) {
            // SAFETY: a real selector, sent to the key window through the
            // responder chain (no target), which every NSWindow answers.
            unsafe {
                item.setTarget(None);
                item.setAction(Some(sel!(performClose:)));
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn quit_closes_the_window() {}

/// A desktop notification. `osascript` needs no app bundle or permission
/// entitlement, so it works for the bare binary too.
// ponytail: clicking it opens Script Editor, not Roxaruu; UNUserNotificationCenter
// fixes that but only works from a signed .app bundle.
#[cfg(all(target_os = "macos", not(test)))]
pub fn notify(title: &str, body: &str) {
    let spawned = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv) sound name \"Glass\"",
            "-e",
            "end run",
            title,
            body,
        ])
        .spawn();
    if let Ok(mut child) = spawned {
        std::thread::spawn(move || child.wait());
    }
}

/// Tests record notifications instead of showing them.
#[cfg(test)]
pub static NOTIFIED: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub fn notify(title: &str, body: &str) {
    NOTIFIED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((title.into(), body.into()));
}

/// Off macOS: the desktop's own notifications (Windows' toasts, Linux's
/// notification service), with a sound, off the caller's thread.
#[cfg(all(not(target_os = "macos"), not(test)))]
pub fn notify(title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let sound = if cfg!(windows) { "Default" } else { "message-new-instant" };
        let _ = notify_rust::Notification::new().summary(&title).body(&body).sound_name(sound).show();
    });
}

/// Whether this app is the one in front, asked of macOS (any thread): the
/// window can't say once you've gone to another Space or app, since macOS
/// stops giving a window it can't show any frames. `None`: can't tell.
#[cfg(all(target_os = "macos", not(test)))]
pub fn app_active() -> Option<bool> {
    Some(objc2_app_kit::NSRunningApplication::currentApplication().isActive())
}

/// Windows: whether the window in front is one of this process's.
///
/// Safety: both calls only read: the foreground window's handle (null if
/// none), and the id of the process that owns it, written into a local.
#[cfg(all(windows, not(test)))]
pub fn app_active() -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    #[allow(unsafe_code)]
    unsafe {
        let w = GetForegroundWindow();
        if w.is_null() {
            return Some(false);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(w, &mut pid);
        Some(pid == std::process::id())
    }
}

/// Linux can't always say (Wayland keeps it private): the window's own
/// focus has to do.
#[cfg(all(not(target_os = "macos"), not(windows), not(test)))]
pub fn app_active() -> Option<bool> {
    None
}

/// Tests say whether the app is in front.
#[cfg(test)]
pub static APP_ACTIVE: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub fn app_active() -> Option<bool> {
    *APP_ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps the computer from sleeping while it's held (remote access: a
/// sleeping computer drops off the network). The display may still sleep.
pub struct Awake {
    #[allow(dead_code)] // held for its Drop
    child: Option<std::process::Child>,
}

impl Drop for Awake {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
        #[cfg(windows)]
        set_awake(false);
    }
}

/// Stay awake, until the returned value is dropped: macOS `caffeinate`,
/// Linux `systemd-inhibit`, Windows' own setting for this thread (the UI's,
/// which lives as long as the window).
pub fn stay_awake() -> Awake {
    let quiet = |c: &mut std::process::Command| {
        c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    };
    let pid = std::process::id().to_string();
    let child = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("/usr/bin/caffeinate");
        c.args(["-i", "-w", &pid]);
        quiet(&mut c);
        c.spawn().ok()
    } else if cfg!(windows) {
        #[cfg(windows)]
        set_awake(true);
        None
    } else {
        let mut c = std::process::Command::new("systemd-inhibit");
        c.args(["--what=idle:sleep", "--who=Roxaruu", "--why=Remote access is on", "--mode=block", "sleep", "infinity"]);
        quiet(&mut c);
        c.spawn().ok()
    };
    Awake { child }
}

/// Safety: `SetThreadExecutionState` only sets flags for the calling thread.
#[cfg(windows)]
fn set_awake(on: bool) {
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
    #[allow(unsafe_code)]
    unsafe {
        SetThreadExecutionState(if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS });
    }
}

/// The shell for a new terminal on Windows: Git Bash if it's installed (a
/// real Unix shell, the one Git for Windows and Claude Code use), else
/// PowerShell 7, else Windows PowerShell, else cmd. Elsewhere `None`: the
/// user's own login shell (`$SHELL`).
#[cfg(windows)]
pub fn windows_shell() -> (std::path::PathBuf, Vec<&'static str>, &'static str) {
    use std::path::PathBuf;
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    // Git for Windows' bash: its usual homes, then wherever `git` is on PATH
    // (…\Git\cmd\git.exe → …\Git\bin\bash.exe).
    let mut bash: Vec<PathBuf> = [env("ProgramFiles"), env("ProgramW6432"), env("ProgramFiles(x86)")]
        .into_iter()
        .flatten()
        .map(|p| p.join("Git").join("bin").join("bash.exe"))
        .chain(env("LOCALAPPDATA").map(|p| p.join("Programs").join("Git").join("bin").join("bash.exe")))
        .collect();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.join("git.exe").is_file()
                && let Some(git) = dir.parent()
            {
                bash.push(git.join("bin").join("bash.exe"));
            }
        }
    }
    if let Some(b) = bash.into_iter().find(|b| b.is_file()) {
        return (b, vec!["--login", "-i"], "bash");
    }
    let on_path = |exe: &str| {
        std::env::var_os("PATH")
            .and_then(|p| std::env::split_paths(&p).map(|d| d.join(exe)).find(|f| f.is_file()))
    };
    if let Some(p) = on_path("pwsh.exe") {
        return (p, vec!["-NoLogo"], "pwsh");
    }
    if let Some(p) = on_path("powershell.exe") {
        return (p, vec!["-NoLogo"], "powershell");
    }
    (env("ComSpec").unwrap_or_else(|| PathBuf::from("cmd.exe")), vec![], "cmd")
}

/// Bring the windows of process `pid` to the front.
#[cfg(target_os = "macos")]
pub fn activate(pid: i32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(|app| {
        app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows)
    })
}

#[cfg(not(target_os = "macos"))]
pub fn activate(_pid: i32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_windows_take_80_percent_centred() {
        use eframe::egui::{Rect, pos2};
        // A 1512×982 screen; the menu bar takes 33 at the top, the Dock 70
        // at the bottom.
        let screen = Rect::from_min_max(pos2(0.0, 0.0), pos2(1512.0, 982.0));
        let visible = Rect::from_min_max(pos2(0.0, 33.0), pos2(1512.0, 912.0));
        let r = window_rect(screen, visible);
        assert!((r.width() - 1209.6).abs() < 0.01 && (r.height() - 785.6).abs() < 0.01, "{r:?}");
        assert_eq!(r.center(), visible.center());
        // A tall Dock: never bigger than what's free.
        let small = Rect::from_min_max(pos2(0.0, 33.0), pos2(1512.0, 700.0));
        assert_eq!(window_rect(screen, small).height(), small.height());
    }

    /// Holding the activity is a plain AppKit call that can't fail.
    #[test]
    fn app_nap_prevention_runs() {
        prevent_app_nap();
    }

    /// This process has a footprint and has used some CPU; bogus pids give nothing.
    #[test]
    #[cfg(target_os = "macos")]
    fn usage_of_this_process() {
        let me = usage(std::process::id() as i32).unwrap();
        assert!(me.footprint_kb > 1024, "{} KB", me.footprint_kb);
        let spin = std::time::Instant::now();
        while spin.elapsed() < std::time::Duration::from_millis(50) {}
        let later = usage(std::process::id() as i32).unwrap();
        assert!(later.cpu > me.cpu && later.cpu - me.cpu < std::time::Duration::from_secs(1));
        assert!(usage(0).is_none() && usage(-3).is_none());
    }

    /// Our own process is alive; the pids that mean something else are not.
    #[test]
    fn liveness_rejects_the_special_pids() {
        assert!(!process_alive(0), "0 means \"my whole process group\"");
        assert!(!process_alive(-1), "negative pids are process groups");
        #[cfg(unix)]
        {
            let me = std::process::id() as i32;
            assert!(process_alive(me), "this process is running");
        }
    }

    /// A pid that can't exist gives no name rather than reading stale memory.
    #[test]
    fn naming_a_bogus_pid_yields_nothing() {
        assert_eq!(process_name(0), None);
        assert_eq!(process_name(-5), None);
    }

    /// Whatever this process is called, it's non-empty and printable — proof
    /// the length clamp and the lossy decode produce a sane string.
    #[test]
    #[cfg(target_os = "macos")]
    fn naming_this_process_returns_something_usable() {
        let me = std::process::id() as i32;
        let Some(name) = process_name(me) else {
            return; // sandboxed builds may refuse; not a failure of the wrapper
        };
        assert!(!name.is_empty());
        assert!(
            name.chars().all(|c| !c.is_control()),
            "got control chars: {name:?}"
        );
    }
}
