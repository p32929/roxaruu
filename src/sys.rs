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
#[cfg(target_os = "linux")]
fn with_process<T>(pid: i32, f: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    if pid <= 0 {
        return None;
    }
    let pid = Pid::from_u32(pid as u32);
    let mut sys = System::new().ok()?;
    let kind = ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_cwd(UpdateKind::Always)
        // For `process_name` (a fork not yet the shell).
        .with_exe(UpdateKind::Always);
    // /proc/<pid> alone.
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
    sys.process(pid).map(f)
}

#[cfg(windows)]
fn with_process<T>(pid: i32, f: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
    if pid <= 0 {
        return None;
    }
    with_processes(|sys| sys.process(sysinfo::Pid::from_u32(pid as u32)).map(f)).flatten()
}

/// Windows: every process, as listed at most a moment ago. Finding one
/// there means listing them all (asking for just one found nothing on
/// GitHub's runners), and each terminal asks a few times a second (what's
/// running, its name, its folder, the memory readout): one listing serves
/// them all, kept between calls (which also lets CPU use be measured).
#[cfg(windows)]
fn with_processes<T>(f: impl FnOnce(&sysinfo::System) -> T) -> Option<T> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    static LISTED: Mutex<Option<(Instant, System)>> = Mutex::new(None);
    let mut listed = LISTED.lock().ok()?;
    let stale = listed.as_ref().is_none_or(|(at, _)| at.elapsed() >= Duration::from_millis(250));
    if stale {
        let mut sys = match listed.take() {
            Some((_, sys)) => sys,
            None => System::new().ok()?,
        };
        // Not the program's path: asking every process for it would cost.
        let kind = ProcessRefreshKind::nothing().with_memory().with_cpu().with_cwd(UpdateKind::Always);
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        *listed = Some((Instant::now(), sys));
    }
    listed.as_ref().map(|(_, sys)| f(sys))
}

/// The working directory of process `pid` (a terminal's shell, to follow
/// `cd`). macOS asks `lsof` (see `terminalpane`).
#[cfg(not(target_os = "macos"))]
pub fn process_cwd(pid: i32) -> Option<std::path::PathBuf> {
    with_process(pid, |p| p.cwd().map(|c| c.to_path_buf())).flatten()
}

/// What a terminal's shell `shell` (named `name`) is running now, where
/// there's no foreground process group to ask (Windows' consoles): the
/// command you ran — going down past the shell itself (Git for Windows'
/// bash.exe is a launcher for the real bash) and the console host beside it,
/// to the newest child that isn't the shell, but not into that command's own
/// helpers (Claude Code's `rg`, `git`). `None`: just the shell.
#[cfg(windows)]
pub fn foreground_process(shell: u32, name: &str) -> Option<i32> {
    with_processes(|sys| foreground_in(sys, shell, name)).flatten()
}

#[cfg(windows)]
fn foreground_in(sys: &sysinfo::System, shell: u32, name: &str) -> Option<i32> {
    use sysinfo::Pid;
    let ignore = |p: &sysinfo::Process| {
        let n = p.name().to_string_lossy().to_lowercase();
        n == "conhost.exe" || n == "openconsole.exe"
    };
    let is_shell = |p: &sysinfo::Process| {
        let n = p.name().to_string_lossy().to_lowercase();
        n.strip_suffix(".exe").unwrap_or(&n) == name.to_lowercase()
    };
    let mut at = Pid::from_u32(shell);
    loop {
        let child = sys
            .processes()
            .values()
            .filter(|p| p.parent() == Some(at) && !ignore(p))
            .max_by_key(|p| p.start_time())?;
        if !is_shell(child) {
            return Some(child.pid().as_u32() as i32);
        }
        at = child.pid();
    }
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
    Some(String::from_utf8_lossy(buf.get(..len).unwrap_or_default()).into_owned())
}

#[cfg(not(target_os = "macos"))]
pub fn process_name(pid: i32) -> Option<String> {
    with_process(pid, |p| {
        // Just forked, not yet the shell, the child is a copy of this app,
        // named (on Linux) after the thread that started it: go by the program.
        if let (Some(exe), Ok(me)) = (p.exe(), std::env::current_exe())
            && exe == me
        {
            return me.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        }
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
pub fn pointer_in_window(inner: eframe::egui::Rect, _pixels_per_point: f32) -> Option<eframe::egui::Pos2> {
    use objc2_app_kit::{NSEvent, NSScreen};
    let mtm = objc2::MainThreadMarker::new()?;
    // Cocoa's origin is the primary screen's bottom-left, y up; egui's (via
    // winit) is its top-left, y down.
    let primary = NSScreen::screens(mtm).firstObject()?.frame();
    let at = NSEvent::mouseLocation();
    let (x, y) = (at.x as f32, (primary.size.height - at.y) as f32);
    Some(eframe::egui::pos2(x - inner.left(), y - inner.top()))
}

/// Windows: the same, for drops from Explorer (no pointer moves reach the
/// window mid-drag either). The cursor comes in screen pixels; egui's
/// window rect is in points.
///
/// Safety: `GetCursorPos` writes one point into a local.
#[cfg(all(windows, not(test)))]
pub fn pointer_in_window(inner: eframe::egui::Rect, pixels_per_point: f32) -> Option<eframe::egui::Pos2> {
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
    let mut at = POINT { x: 0, y: 0 };
    #[allow(unsafe_code)]
    if unsafe { GetCursorPos(&mut at) } == 0 {
        return None;
    }
    let ppp = pixels_per_point.max(0.1);
    Some(eframe::egui::pos2(at.x as f32 / ppp - inner.left(), at.y as f32 / ppp - inner.top()))
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

/// How a notification went, for the terminal's tab to say on hover: sent or
/// not, whether something is likely keeping it out of sight (`trouble`),
/// and what to do about it. Only what the system lets an app see for free:
/// macOS doesn't tell an app whether a Focus is on, so there it's a hint.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub sent: bool,
    pub trouble: bool,
    pub note: String,
}

impl Delivery {
    pub fn sent(note: impl Into<String>) -> Self {
        Self { sent: true, trouble: false, note: note.into() }
    }
    pub fn held_back(note: impl Into<String>) -> Self {
        Self { sent: true, trouble: true, note: note.into() }
    }
    // Only macOS can say it won't send one (Roxaruu's turned off there).
    #[cfg_attr(any(not(target_os = "macos"), test), allow(dead_code))]
    pub fn not_sent(note: impl Into<String>) -> Self {
        Self { sent: false, trouble: true, note: note.into() }
    }
}

/// What the system says about Roxaruu showing notifications. (Only macOS
/// ever says anything but yes.)
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(all(not(target_os = "macos"), not(test)), allow(dead_code))]
pub enum NotifyPermission {
    /// Roxaruu's own: its name and icon, and you can let it through a Focus.
    Own,
    /// Turned off for Roxaruu in the system's settings.
    Denied,
    /// Not Roxaruu's own (not the installed app, or macOS refused it):
    /// macOS shows them as Script Editor's, through `osascript`.
    Borrowed(String),
}

/// macOS: Roxaruu's own notifications (UserNotifications), when this is the
/// installed app and macOS allows them; else `osascript`'s, which needs
/// nothing but shows as Script Editor's (its name, and a Focus has to
/// allow Script Editor). Every one is logged, with how it went.
#[cfg(all(target_os = "macos", not(test)))]
mod mac_notes {
    use super::NotifyPermission;
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError, NSString};
    use super::Delivery;
    use objc2_user_notifications::{
        UNAlertStyle, UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
        UNNotificationRequest, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    };
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// Whether macOS lets Roxaruu show its own (asked at start, on Turn On,
    /// and again after each one, as you may change it meanwhile).
    static OWN: AtomicBool = AtomicBool::new(false);
    /// Turned off for Roxaruu in System Settings.
    static DENIED: AtomicBool = AtomicBool::new(false);
    /// Allowed, but set to show no banner (straight to Notification Center).
    static NO_BANNERS: AtomicBool = AtomicBool::new(false);

    /// Running as Roxaruu.app: the notification center exists only for an app
    /// bundle (asking for it from a bare binary throws, which aborts).
    fn bundled() -> bool {
        let bundle = NSBundle::mainBundle();
        bundle.bundleIdentifier().is_some_and(|id| id.to_string() == "com.p32929.roxaruu")
            && bundle.bundlePath().to_string().ends_with(".app")
    }

    fn center() -> Option<objc2::rc::Retained<UNUserNotificationCenter>> {
        bundled().then(UNUserNotificationCenter::currentNotificationCenter)
    }

    /// At start: whether Roxaruu's own are already allowed (asks nothing).
    pub fn check() {
        let Some(center) = center() else { return };
        let block = RcBlock::new(|settings: NonNull<UNNotificationSettings>| {
            // Safety: macOS hands the block a valid settings object.
            let settings = unsafe { settings.as_ref() };
            let status = settings.authorizationStatus();
            let own = status == UNAuthorizationStatus::Authorized || status == UNAuthorizationStatus::Provisional;
            let no_banners = own && settings.alertStyle() == UNAlertStyle::None;
            OWN.store(own, Ordering::Relaxed);
            DENIED.store(status == UNAuthorizationStatus::Denied, Ordering::Relaxed);
            NO_BANNERS.store(no_banners, Ordering::Relaxed);
            tracing::debug!(own, no_banners, status = status.0, "notifications: what macOS allows Roxaruu");
        });
        center.getNotificationSettingsWithCompletionHandler(&block);
    }

    /// Ask macOS (its prompt, the first time); `done` hears on another thread.
    pub fn ask(done: Box<dyn FnOnce(NotifyPermission) + Send>) {
        let Some(center) = center() else {
            tracing::info!("notifications: not the installed app, so osascript's");
            return done(NotifyPermission::Borrowed("not running as the installed Roxaruu.app".into()));
        };
        let done = std::sync::Mutex::new(Some(done));
        let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            // Safety: null, or an NSError macOS keeps alive for the call.
            let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
            let answer = match (granted.as_bool(), error) {
                (true, _) => NotifyPermission::Own,
                (false, None) => NotifyPermission::Denied,
                (false, Some(e)) => NotifyPermission::Borrowed(e),
            };
            OWN.store(answer == NotifyPermission::Own, Ordering::Relaxed);
            tracing::info!(?answer, "notifications: macOS answered");
            if let Some(done) = done.lock().unwrap_or_else(|e| e.into_inner()).take() {
                done(answer);
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &block,
        );
    }

    pub fn notify(title: &str, body: &str) -> Delivery {
        // Turned off for Roxaruu: respect it (not around it, as Script Editor's).
        if DENIED.load(Ordering::Relaxed) && bundled() {
            tracing::info!("notification not sent: off for Roxaruu in System Settings");
            return Delivery::not_sent(
                "Not sent: notifications for Roxaruu are off in System Settings › Notifications › Roxaruu.",
            );
        }
        let delivery = send(title, body);
        check(); // what macOS allows, fresh for the next one
        delivery
    }

    fn send(title: &str, body: &str) -> Delivery {
        if OWN.load(Ordering::Relaxed)
            && let Some(center) = center()
        {
            static N: AtomicU64 = AtomicU64::new(0);
            let content = UNMutableNotificationContent::new();
            content.setTitle(&NSString::from_str(title));
            content.setBody(&NSString::from_str(body));
            content.setSound(Some(&UNNotificationSound::defaultSound()));
            let id = format!("roxaruu-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
            let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&id), &content, None);
            let block = RcBlock::new(|error: *mut NSError| {
                // Safety: null, or an NSError macOS keeps alive for the call.
                match unsafe { error.as_ref() } {
                    None => tracing::debug!("notification shown (Roxaruu's own)"),
                    Some(e) => tracing::warn!(error = %e.localizedDescription(), "macOS refused a notification"),
                }
            });
            center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
            return if NO_BANNERS.load(Ordering::Relaxed) {
                Delivery::held_back(
                    "Sent, but Roxaruu's notifications are set to show no banner (System Settings › \
                     Notifications › Roxaruu), so it went quietly to Notification Center.",
                )
            } else {
                Delivery::sent(
                    "Sent. Didn't see it? A Focus or Do Not Disturb may have hidden it: allow Roxaruu in \
                     System Settings › Focus › (your Focus) › Allowed Apps.",
                )
            };
        }
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
        match spawned {
            Ok(mut child) => {
                tracing::debug!("notification shown (osascript's: Script Editor's)");
                std::thread::spawn(move || child.wait());
                Delivery::sent(
                    "Sent as Script Editor's notification (macOS didn't let Roxaruu use its own). Didn't \
                     see it? A Focus may have hidden it: allow Script Editor in System Settings › Focus › \
                     (your Focus) › Allowed Apps.",
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "couldn't run osascript for a notification");
                Delivery::not_sent(format!("Not sent: macOS's osascript couldn't run ({e})."))
            }
        }
    }
}

#[cfg(all(target_os = "macos", not(test)))]
pub use mac_notes::notify;

/// At start: whether Roxaruu may show its own notifications (asks nothing);
/// on Windows, its name for them registered.
pub fn check_notifications() {
    #[cfg(all(target_os = "macos", not(test)))]
    mac_notes::check();
    #[cfg(all(windows, not(test)))]
    register_app_id();
}

/// Windows: the id Roxaruu's notifications go under, so Windows shows them
/// as Roxaruu's (with its icon), not PowerShell's.
#[cfg(windows)]
pub const APP_ID: &str = "p32929.Roxaruu";

/// Windows: register `APP_ID` for this user (HKCU, no admin; however it was
/// installed: the installer, the zip, install-local.sh), with Windows' own
/// `reg.exe`, once at start and off the main thread, no console flashing.
/// The installer's uninstaller and uninstall.sh remove it.
#[cfg(all(windows, not(test)))]
fn register_app_id() {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW
    std::thread::spawn(|| {
        let key = format!(r"HKCU\Software\Classes\AppUserModelId\{APP_ID}");
        let mut values = vec![("DisplayName", "Roxaruu".to_string())];
        // The installer puts the icon beside the program.
        if let Some(icon) = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("roxaruu.ico"))).filter(|i| i.is_file()) {
            values.push(("IconUri", icon.to_string_lossy().into_owned()));
        }
        for (name, value) in values {
            let done = std::process::Command::new("reg")
                .args(["add", &key, "/v", name, "/t", "REG_SZ", "/d", &value, "/f"])
                .creation_flags(NO_WINDOW)
                .output();
            match done {
                Ok(out) if out.status.success() => {}
                Ok(out) => tracing::warn!(name, err = %String::from_utf8_lossy(&out.stderr).trim(), "couldn't register Roxaruu's notification id"),
                Err(e) => tracing::warn!(name, error = %e, "couldn't run reg.exe"),
            }
        }
        tracing::debug!("notification id registered");
    });
}

/// Turning notifications on: ask the system where it asks (macOS shows its
/// prompt the first time); `done` hears the answer, maybe on another thread.
pub fn ask_to_notify(done: Box<dyn FnOnce(NotifyPermission) + Send>) {
    #[cfg(all(target_os = "macos", not(test)))]
    mac_notes::ask(done);
    // Windows and Linux don't ask an app first.
    #[cfg(all(not(target_os = "macos"), not(test)))]
    done(NotifyPermission::Own);
    // Tests hear what `PERMISSION_ANSWER` says (yes, unless a test says otherwise).
    #[cfg(test)]
    done(PERMISSION_ANSWER.lock().unwrap_or_else(|e| e.into_inner()).clone());
}

/// What the "system" answers a test that turns notifications on.
#[cfg(test)]
pub static PERMISSION_ANSWER: std::sync::Mutex<NotifyPermission> = std::sync::Mutex::new(NotifyPermission::Own);

/// A message that waits to be read, for when there's no window of ours to
/// show it in (it couldn't open): the system's own alert, and stderr.
pub fn alert(title: &str, body: &str) {
    eprintln!("{title}\n\n{body}");
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("osascript")
        .args(["-e", "on run argv", "-e", "display alert (item 1 of argv) message (item 2 of argv) as critical", "-e", "end run", title, body])
        .status();
    // Safety: a modal box with two NUL-terminated wide strings that outlive the call.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (t, b) = (wide(title), wide(body));
        MessageBoxW(std::ptr::null_mut(), b.as_ptr(), t.as_ptr(), MB_OK | MB_ICONERROR);
    }
    // Linux: whichever dialog program the desktop has (a server has none).
    #[cfg(not(any(target_os = "macos", windows)))]
    for (cmd, args) in [
        ("zenity", vec!["--error".to_string(), format!("--title={title}"), format!("--text={body}")]),
        ("kdialog", vec!["--title".into(), title.into(), "--error".into(), body.into()]),
        ("xmessage", vec![format!("{title}\n\n{body}")]),
    ] {
        if std::process::Command::new(cmd).args(&args).status().is_ok() {
            break;
        }
    }
}

/// The CPU time this thread has used: the work done, whatever else the
/// machine is busy with (speed checks in tests don't trip on a loaded one).
///
/// Safety: `clock_gettime` only writes the local `timespec`.
#[cfg(all(test, unix))]
pub fn thread_cpu() -> std::time::Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    #[allow(unsafe_code)]
    let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } == 0;
    if !ok {
        return std::time::Duration::ZERO;
    }
    std::time::Duration::new(u64::try_from(ts.tv_sec).unwrap_or(0), u32::try_from(ts.tv_nsec).unwrap_or(0))
}

/// Tests record notifications instead of showing them.
#[cfg(test)]
pub static NOTIFIED: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub fn notify(title: &str, body: &str) -> Delivery {
    NOTIFIED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((title.into(), body.into()));
    DELIVERY.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| Delivery::sent("Sent."))
}

/// How the "system" says a test's notification went (sent, unless set).
#[cfg(test)]
pub static DELIVERY: std::sync::Mutex<Option<Delivery>> = std::sync::Mutex::new(None);

/// Off macOS: the desktop's own notifications (Windows' toasts, Linux's
/// notification service), with a sound, off the caller's thread.
#[cfg(all(not(target_os = "macos"), not(test)))]
pub fn notify(title: &str, body: &str) -> Delivery {
    let delivery = match held_back_by() {
        Some(why) => Delivery::held_back(why),
        None if cfg!(windows) => Delivery::sent(
            "Sent. Didn't see it? Windows' Do Not Disturb (Focus Assist) may have hidden it: \
             Settings › System › Notifications.",
        ),
        None => Delivery::sent("Sent. Didn't see it? Your desktop's Do Not Disturb may have hidden it."),
    };
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let sound = if cfg!(windows) { "Default" } else { "message-new-instant" };
        let mut note = notify_rust::Notification::new();
        note.summary(&title).body(&body).sound_name(sound);
        // As Roxaruu's (registered at start), not PowerShell's.
        #[cfg(windows)]
        note.app_id(APP_ID);
        match note.show() {
            Ok(_) => tracing::debug!("notification shown"),
            Err(e) => tracing::warn!(error = %e, "couldn't show a notification"),
        }
    });
    delivery
}

/// Windows: what it says about taking notifications now (Do Not Disturb /
/// Focus Assist's quiet hours, a presentation, a full-screen game).
///
/// Safety: `SHQueryUserNotificationState` writes one value into a local.
#[cfg(all(windows, not(test)))]
fn held_back_by() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{
        QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_QUIET_TIME, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
    };
    let mut state = 0;
    #[allow(unsafe_code)]
    if unsafe { SHQueryUserNotificationState(&mut state) } != 0 {
        return None;
    }
    let why = match state {
        QUNS_QUIET_TIME => "Do Not Disturb (Focus Assist) is on",
        QUNS_PRESENTATION_MODE => "a presentation is on",
        QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN => "a full-screen app is in front",
        _ => return None,
    };
    tracing::info!(why, "Windows is holding notifications back");
    Some(format!("Sent, but Windows is holding notifications back: {why}."))
}

/// Linux (GNOME): Do Not Disturb is on when banners are off.
#[cfg(all(not(target_os = "macos"), not(windows), not(test)))]
fn held_back_by() -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.notifications", "show-banners"])
        .output()
        .ok()?;
    (String::from_utf8_lossy(&out.stdout).trim() == "false").then(|| {
        tracing::info!("GNOME's Do Not Disturb is on");
        "Sent, but Do Not Disturb is on, so it won't pop up.".to_string()
    })
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
/// `None` when the system wouldn't say yes (nothing else breaks).
pub type Awake = Option<keepawake::KeepAwake>;

/// Stay awake until the returned value is dropped, on every system
/// (`keepawake`: IOKit, systemd-logind, `SetThreadExecutionState`).
pub fn stay_awake() -> Awake {
    keepawake::Builder::default()
        .idle(true)
        .reason("Remote access is on")
        .app_name("Roxaruu")
        .app_reverse_domain("io.github.p32929.roxaruu")
        .create()
        .ok()
}

/// What a new terminal runs for the `terminalShell` setting `choice`:
/// `(program, arguments, name)`, or `None` for the user's own login shell
/// (`$SHELL`). On Windows a terminal is Git Bash or WSL, never PowerShell
/// or cmd: Git Bash unless "wsl" is chosen, each standing in for the other;
/// `None` there means neither is installed (`no_terminal_shell`).
pub fn shell_for(choice: &str) -> Option<(std::path::PathBuf, Vec<String>, String)> {
    let named = |p: std::path::PathBuf, args: &[&str], name: &str| {
        Some((p, args.iter().map(|a| a.to_string()).collect::<Vec<String>>(), name.to_string()))
    };
    let choice = choice.trim();
    #[cfg(windows)]
    {
        let bash = || git_bash().and_then(|p| named(p, &["--login", "-i"], "bash"));
        let wsl = || wsl().and_then(|p| named(p, &[], "wsl"));
        if choice.eq_ignore_ascii_case("wsl") { wsl().or_else(bash) } else { bash().or_else(wsl) }
    }
    #[cfg(not(windows))]
    {
        let _ = named;
        if choice.is_empty() {
            return None;
        }
        any_shell(choice)
    }
}

/// Windows without Git Bash or WSL: there's no shell for a terminal (the
/// app asks you to install one instead).
pub fn no_terminal_shell() -> bool {
    cfg!(windows) && shell_for("").is_none()
}

/// A shell by name (on `PATH`) or path; a login shell where that's a flag
/// it knows.
#[cfg(not(windows))]
fn any_shell(choice: &str) -> Option<(std::path::PathBuf, Vec<String>, String)> {
    let path = which::which(choice).ok().or_else(|| {
        let p = std::path::PathBuf::from(choice);
        p.is_file().then_some(p)
    })?;
    let name = path.file_stem()?.to_string_lossy().to_lowercase();
    let login = ["bash", "zsh", "fish", "sh", "ksh", "dash"].contains(&name.as_str());
    let args = if login { vec!["-l".to_string()] } else { Vec::new() };
    Some((path, args, name))
}

/// The choices for the Settings screen: the system's own ("") first, then
/// what this computer has.
pub fn shell_choices() -> Vec<&'static str> {
    let mut v = vec![""];
    #[cfg(windows)]
    {
        if git_bash().is_some() {
            v.push("gitbash");
        }
        if wsl().is_some() {
            v.push("wsl");
        }
    }
    #[cfg(not(windows))]
    v.extend(["zsh", "bash", "fish"].into_iter().filter(|s| which::which(s).is_ok()));
    v
}

/// How Settings shows a choice.
pub fn shell_title(choice: &str) -> String {
    match choice.trim().to_ascii_lowercase().as_str() {
        "" if cfg!(windows) => "Git Bash (WSL without it)".into(),
        "" => "Your login shell".into(),
        "gitbash" => "Git Bash".into(),
        "wsl" => "WSL (Linux)".into(),
        _ => choice.to_string(),
    }
}

/// WSL, when a Linux is installed in it (`wsl.exe` alone comes with every
/// Windows 11, and only offers to install one). Each installed Linux is a
/// key under Lxss.
#[cfg(windows)]
fn wsl() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryInfoKeyW};
    let root = std::env::var_os("SystemRoot").map(std::path::PathBuf::from)?;
    let p = root.join("System32").join("wsl.exe");
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss\0".encode_utf16().collect();
    let mut h = std::ptr::null_mut();
    let mut linuxes = 0u32;
    // SAFETY: a NUL-terminated key name and out-pointers that live through
    // the calls; the key is closed once read.
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_READ, &mut h) != 0 {
            return None;
        }
        use std::ptr::{null, null_mut};
        RegQueryInfoKeyW(h, null_mut(), null_mut(), null(), &mut linuxes, null_mut(), null_mut(), null_mut(), null_mut(), null_mut(), null_mut(), null_mut());
        RegCloseKey(h);
    }
    (linuxes > 0 && p.is_file()).then_some(p)
}

/// Git for Windows' bash, when it's installed: its usual homes, then
/// wherever `git` is on PATH (…\\Git\\cmd\\git.exe → …\\Git\\bin\\bash.exe).
#[cfg(windows)]
fn git_bash() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut bash: Vec<PathBuf> = [env("ProgramFiles"), env("ProgramW6432"), env("ProgramFiles(x86)")]
        .into_iter()
        .flatten()
        .map(|p| p.join("Git").join("bin").join("bash.exe"))
        .chain(env("LOCALAPPDATA").map(|p| p.join("Programs").join("Git").join("bin").join("bash.exe")))
        .collect();
    if let Ok(git) = which::which("git")
        && let Some(home) = git.parent().and_then(|d| d.parent())
    {
        bash.push(home.join("bin").join("bash.exe"));
    }
    bash.into_iter().find(|b| b.is_file())
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

    /// Windows terminals are Git Bash (GitHub's runners have it), whatever
    /// was chosen before: never PowerShell or cmd.
    #[cfg(windows)]
    #[test]
    fn windows_terminals_run_git_bash() {
        for choice in ["", "powershell", "cmd", "pwsh", "gitbash"] {
            let (path, _, name) = shell_for(choice).expect("Git Bash is installed");
            assert_eq!(name, "bash", "{choice:?} ran {path:?}");
        }
        assert!(!no_terminal_shell());
        assert!(!shell_choices().iter().any(|c| ["cmd", "pwsh", "powershell"].contains(c)));
    }

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
