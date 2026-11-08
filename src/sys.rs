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

#[cfg(not(unix))]
pub fn process_alive(_pid: i32) -> bool {
    false
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
pub fn process_name(_pid: i32) -> Option<String> {
    None
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
pub fn usage(_pid: i32) -> Option<Usage> {
    None
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

#[cfg(all(not(target_os = "macos"), not(test)))]
pub fn notify(_title: &str, _body: &str) {}

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
