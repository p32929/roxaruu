//! Tracks which folders are currently open across all running Oxru windows.
//!
//! Each window is its own process, so to know what's already open we keep a
//! directory of marker files — one per live process, named by PID, containing
//! that window's folder. A marker is only trusted if its PID is both alive
//! *and* still actually an Oxru process — a window that crashed or was force-
//! quit without cleaning up leaves its marker behind, and once the OS recycles
//! that PID for an unrelated process, a bare `kill(pid, 0)` liveness check
//! would wrongly treat the stale marker as a live window forever.

use std::path::{Path, PathBuf};

/// `<config_dir>/roxaruu/running/`: Roxaruu's own. (It once shared Oxru's,
/// which made folders open in Oxru look "already open" here.)
fn running_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("roxaruu").join("running"))
}

/// Whether process `pid` is still alive (so its marker isn't stale).
fn pid_alive(pid: i32) -> bool {
    crate::sys::process_alive(pid)
}

/// Whether `pid` is alive *and* actually running an Oxru binary — not just
/// some unrelated process that happens to have been assigned the same PID
/// after the original Oxru window died without unregistering.
///
/// Shared with [`crate::termbridge`], which reaps the bridge directories of
/// instances that died without cleaning up and needs the exact same "is this
/// PID still an Oxru?" question answered.
pub(crate) fn pid_is_oxru(pid: i32) -> bool {
    pid_runs(pid, &["oxru", "roxaruu"])
}

/// Whether `pid` is a live Roxaruu window.
fn pid_is_roxaruu(pid: i32) -> bool {
    pid_runs(pid, &["roxaruu"])
}

/// Whether `pid` is alive and its binary's name contains one of `names`.
fn pid_runs(pid: i32, names: &[&str]) -> bool {
    if !pid_alive(pid) {
        return false;
    }
    // Can't verify — fall back to the liveness check alone rather than
    // dropping every window's marker.
    let Some(name) = process_command(pid) else {
        return true;
    };
    let name = name.to_lowercase();
    names.iter().any(|n| name.contains(n))
}

/// The name `pid` was started by: `ps` on Unix (the name as run, a link's
/// too); Windows has no `ps` (Git Bash's counts processes its own way), so
/// the system's own name there.
fn process_command(pid: i32) -> Option<String> {
    if cfg!(windows) {
        return crate::sys::process_name(pid);
    }
    let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "comm="]).output().ok()?;
    let comm = String::from_utf8_lossy(&out.stdout);
    Some(comm.trim().rsplit('/').next().unwrap_or("").to_string())
}

/// Mark this process as having `folder` open.
pub fn register(folder: &Path) {
    if let Some(dir) = running_dir() {
        let canon = dunce::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        register_in(&dir, std::process::id(), &canon);
    }
}

/// Test-only: mark `pid` (another process) as the window with `folder`.
#[cfg(test)]
pub fn register_for(pid: u32, folder: &Path) {
    if let Some(dir) = running_dir() {
        register_in(&dir, pid, folder);
    }
}

/// Remove this process's marker (called on a clean exit).
pub fn unregister() {
    if let Some(dir) = running_dir() {
        let _ = std::fs::remove_file(dir.join(std::process::id().to_string()));
    }
}

/// Folders open in other live Roxaruu windows, with each window's pid
/// (stale markers pruned in passing).
pub fn open_folders() -> Vec<(i32, PathBuf)> {
    running_dir()
        .map(|d| open_folders_in(&d, std::process::id()))
        .unwrap_or_default()
}

// ---- testable cores (dir-injected) ------------------------------------

fn register_in(dir: &Path, pid: u32, folder: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(
        dir.join(pid.to_string()),
        folder.to_string_lossy().as_bytes(),
    );
}

fn open_folders_in(dir: &Path, me: u32) -> Vec<(i32, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid as u32 == me {
            continue;
        }
        if !pid_is_roxaruu(pid) {
            let _ = std::fs::remove_file(e.path()); // prune dead/stale window
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(e.path()) {
            let p = PathBuf::from(content.trim());
            if !p.as_os_str().is_empty() {
                out.push((pid, p));
            }
        }
    }
    out
}

// ---- remote access ------------------------------------------------------

/// A window reachable from the browser: where its private link listens
/// (localhost only), the token that proves a caller is the server, and what
/// to call it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct RemoteWindow {
    #[serde(default)]
    pub pid: i32,
    pub port: u16,
    pub token: String,
    pub name: String,
    pub path: String,
}

/// List this window for remote access (rewritten when its folder changes).
pub fn register_remote(window: &RemoteWindow) {
    if let Some(dir) = running_dir() {
        register_remote_in(&dir, std::process::id(), window);
    }
}

pub fn unregister_remote() {
    if let Some(dir) = running_dir() {
        let _ = std::fs::remove_file(dir.join(format!("{}.remote", std::process::id())));
    }
}

/// Every live window open to remote access, this one included, oldest first.
pub fn remote_windows() -> Vec<RemoteWindow> {
    running_dir()
        .map(|d| remote_windows_in(&d))
        .unwrap_or_default()
}

/// The window serving remote access: its pid and the port browsers use.
/// No secrets (the password lives only in that window's memory).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct RemoteServer {
    pub pid: i32,
    pub port: u16,
    /// Used only from browsers: every window here keeps minimized meanwhile
    /// (so this computer draws nothing; the browsers' devices do).
    #[serde(default)]
    pub browser_only: bool,
}

const SERVER_FILE: &str = "remote-server.json";

/// Note that this window now serves remote access on `port`; every other
/// window watches for it (see [`remote_server`]).
pub fn register_remote_server(port: u16, browser_only: bool) {
    if let Some(dir) = running_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let server = RemoteServer { pid: std::process::id() as i32, port, browser_only };
        if let Ok(json) = serde_json::to_string(&server) {
            let _ = std::fs::write(dir.join(SERVER_FILE), json);
        }
    }
}

/// Remove the server marker: only if it's this window's when `mine_only`
/// (stopping here), or whoever's it is (turning remote access off from
/// another window, which the serving window notices and stops).
pub fn clear_remote_server(mine_only: bool) {
    if let Some(dir) = running_dir() {
        clear_remote_server_in(&dir, mine_only.then(|| std::process::id() as i32));
    }
}

/// The live server, if any: its marker, with its pid still a Roxaruu window
/// (a crashed one's is stale, removed in passing).
pub fn remote_server() -> Option<RemoteServer> {
    remote_server_in(&running_dir()?, std::process::id() as i32)
}

fn clear_remote_server_in(dir: &Path, only: Option<i32>) {
    let path = dir.join(SERVER_FILE);
    let theirs = || {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<RemoteServer>(&s).ok())
            .is_some_and(|s| Some(s.pid) != only)
    };
    if only.is_none() || !theirs() {
        let _ = std::fs::remove_file(&path);
    }
}

fn remote_server_in(dir: &Path, me: i32) -> Option<RemoteServer> {
    let path = dir.join(SERVER_FILE);
    let server: RemoteServer = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    if server.pid != me && !pid_is_roxaruu(server.pid) {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(server)
}

fn register_remote_in(dir: &Path, pid: u32, window: &RemoteWindow) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("{pid}.remote"));
    let tmp = dir.join(format!("{pid}.remote.tmp"));
    let Ok(json) = serde_json::to_string(window) else {
        return;
    };
    // Holds the token: readable by this user only.
    if std::fs::write(&tmp, json).is_ok() {
        #[cfg(unix)]
        let _ = std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600));
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn remote_windows_in(dir: &Path) -> Vec<RemoteWindow> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(std::time::SystemTime, RemoteWindow)> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_suffix(".remote").and_then(|p| p.parse::<i32>().ok()) else {
            continue;
        };
        if !pid_is_roxaruu(pid) {
            let _ = std::fs::remove_file(e.path()); // a window that died
            continue;
        }
        let Some(mut w) = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|s| serde_json::from_str::<RemoteWindow>(&s).ok())
        else {
            continue;
        };
        w.pid = pid;
        let born = e.metadata().and_then(|m| m.created()).unwrap_or(std::time::UNIX_EPOCH);
        out.push((born, w));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.pid.cmp(&b.1.pid)));
    out.into_iter().map(|(_, w)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_process_is_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("proj");
        std::fs::create_dir(&folder).unwrap();
        register_in(tmp.path(), std::process::id(), &folder);
        assert_eq!(
            open_folders_in(tmp.path(), 0),
            vec![(std::process::id() as i32, folder)]
        );
        assert!(
            open_folders_in(tmp.path(), std::process::id()).is_empty(),
            "not this window's own"
        );
    }

    #[test]
    fn dead_process_marker_is_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        // A PID that's essentially never alive.
        register_in(tmp.path(), 0x7fff_fffe, Path::new("/somewhere"));
        assert!(open_folders_in(tmp.path(), 0).is_empty());
        // The stale marker file was removed.
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }

    #[test]
    fn remote_windows_are_listed_privately_and_dead_ones_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let me = RemoteWindow {
            pid: 0,
            port: 50123,
            token: "secret".into(),
            name: "shop".into(),
            path: "/x/shop".into(),
        };
        register_remote_in(tmp.path(), std::process::id(), &me);
        register_remote_in(tmp.path(), 0x7fff_fffe, &me); // a dead window
        let listed = remote_windows_in(tmp.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].pid, std::process::id() as i32);
        assert_eq!(listed[0].port, 50123);
        assert!(!tmp.path().join("2147483646.remote").exists(), "pruned");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = tmp.path().join(format!("{}.remote", std::process::id()));
            let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the token is private");
        }
    }

    #[test]
    fn a_crashed_servers_marker_is_ignored_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join(SERVER_FILE);
        std::fs::write(&marker, r#"{"pid":2147483646,"port":7878}"#).unwrap();
        assert_eq!(remote_server_in(tmp.path(), 1), None);
        assert!(!marker.exists(), "pruned");
        // A live one is the server; this window's own needs no process check.
        let me = std::process::id() as i32;
        std::fs::write(&marker, format!(r#"{{"pid":{me},"port":4321}}"#)).unwrap();
        assert_eq!(remote_server_in(tmp.path(), 1), Some(RemoteServer { pid: me, port: 4321, browser_only: false }));
        assert_eq!(remote_server_in(tmp.path(), me).map(|s| s.port), Some(4321));
        // Garbage isn't a server.
        std::fs::write(&marker, "nonsense").unwrap();
        assert_eq!(remote_server_in(tmp.path(), me), None);
    }

    #[test]
    fn stopping_removes_only_your_own_marker_turning_off_removes_any() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join(SERVER_FILE);
        std::fs::write(&marker, r#"{"pid":42,"port":7878}"#).unwrap();
        clear_remote_server_in(tmp.path(), Some(7));
        assert!(marker.exists(), "another window's server is left alone");
        clear_remote_server_in(tmp.path(), Some(42));
        assert!(!marker.exists());
        std::fs::write(&marker, r#"{"pid":42,"port":7878}"#).unwrap();
        clear_remote_server_in(tmp.path(), None);
        assert!(!marker.exists(), "turned off from anywhere");
    }
}
