//! Updates, from GitHub's releases. Now and then a window asks GitHub for
//! the latest release (the windows share what one found, on disk). If it's
//! newer, the window downloads this computer's package, checks it against
//! the release's `SHA256SUMS`, and installs it when the last window closes,
//! or right away with Restart to Update.
//!
//! It costs nothing: no signing and no server of ours. A file the app
//! downloads itself carries no "from the internet" mark, so neither
//! Gatekeeper nor SmartScreen stops it.
//!
//! Files in `<local data>/Roxaruu/update/`:
//! - `checked`: when a window last asked GitHub (Unix seconds).
//! - `ready`: `<version> <file>`, downloaded and verified, waiting to install.
//! - `installed`: the version a window installed (the others restart into it).
//! - `reopen`: on Windows, the folder to reopen once the installer is done.
//! - `lock`: the pid of the window checking or downloading right now.
//! - the package itself.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, SystemTime};

const LATEST: &str = "https://api.github.com/repos/p32929/roxaruu/releases/latest";
/// The release page, for a copy that updates by hand.
pub(crate) const RELEASES: &str = "https://github.com/p32929/roxaruu/releases/latest";
/// How often a window asks GitHub. One window asks for all of them
/// (`checked`), well under GitHub's 60 requests an hour.
pub(crate) const EVERY: Duration = Duration::from_secs(6 * 3600);
/// A package bigger than this isn't ours (they're 15–40 MB).
const MAX_PACKAGE: u64 = 300 << 20;

/// How this copy was installed, and so how it updates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Install {
    /// macOS: this Roxaruu.app is replaced with the disk image's.
    MacApp(PathBuf),
    /// Windows: the installer runs silently, for this user.
    WindowsSetup,
    /// Linux: the .tar.xz is unpacked over this prefix (`~/.local`).
    LinuxTar(PathBuf),
    /// Linux: the .deb, through apt. That needs your password, so it runs
    /// in a terminal.
    LinuxDeb,
    /// Built from source, or the .zip: you update it yourself.
    ByHand,
}

/// How this copy updates.
pub(crate) fn this_install() -> Install {
    match std::env::current_exe().and_then(dunce::canonicalize) {
        Ok(exe) => install_of(std::env::consts::OS, &exe),
        Err(_) => Install::ByHand,
    }
}

/// How a copy at `exe` on `os` updates: from where its installer puts it.
pub(crate) fn install_of(os: &str, exe: &Path) -> Install {
    let up = |p: &Path, n: usize| (0..n).try_fold(p.to_path_buf(), |p, _| p.parent().map(Path::to_path_buf));
    match os {
        // …/Roxaruu.app/Contents/MacOS/roxaruu, in a folder we may write to.
        "macos" => match up(exe, 3) {
            Some(app)
                if app.extension().is_some_and(|e| e == "app")
                    && app.parent().is_some_and(writable) =>
            {
                Install::MacApp(app)
            }
            _ => Install::ByHand,
        },
        // The installer leaves its uninstaller beside the program; the .zip doesn't.
        "windows" if exe.with_file_name("unins000.exe").is_file() => Install::WindowsSetup,
        "linux" if exe == Path::new("/usr/bin/roxaruu") => Install::LinuxDeb,
        // <prefix>/bin/roxaruu, with the .tar.xz's menu entry beside it.
        "linux" => match up(exe, 2) {
            Some(prefix)
                if prefix.join("share/applications/roxaruu.desktop").is_file()
                    && exe.parent().is_some_and(writable) =>
            {
                Install::LinuxTar(prefix)
            }
            _ => Install::ByHand,
        },
        _ => Install::ByHand,
    }
}

/// Whether this user can write in `dir`. Making a file there is the only
/// honest test: permission bits don't know about groups or ACLs.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".roxaruu-update-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The release's file for an `install` on `arch`, at `version`.
pub(crate) fn package(install: &Install, arch: &str, version: &str) -> Option<String> {
    let v = version;
    match install {
        Install::MacApp(_) => {
            let chip = if arch == "aarch64" { "apple-silicon" } else { "intel" };
            Some(format!("roxaruu_v{v}_macos_{chip}.dmg"))
        }
        Install::WindowsSetup => Some(format!("roxaruu_v{v}_windows_x64_setup.exe")),
        Install::LinuxTar(_) => Some(format!("roxaruu_v{v}_linux_x64.tar.xz")),
        Install::LinuxDeb => Some(format!("roxaruu_v{v}_linux_x64.deb")),
        Install::ByHand => None,
    }
    // Linux and Windows releases are x86-64 only.
    .filter(|_| matches!(install, Install::MacApp(_)) || arch == "x86_64")
}

/// `x.y.z` (a `v` before it is fine).
fn parse(v: &str) -> Option<[u64; 3]> {
    let mut parts = v.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok());
    let v = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(v)
}

/// Whether `version` is newer than `than`. A version that can't be read
/// is never newer.
pub(crate) fn newer(version: &str, than: &str) -> bool {
    matches!((parse(version), parse(than)), (Some(a), Some(b)) if a > b)
}

/// Where the releases come from: GitHub, or (in tests) a folder with a
/// `latest.json` like GitHub's and the files beside it.
#[derive(Clone, Debug)]
pub(crate) enum Source {
    GitHub,
    #[cfg(test)]
    Folder(PathBuf),
}

/// A release: its version and its files (name, where to get it).
#[derive(Debug, PartialEq)]
pub(crate) struct Release {
    pub(crate) version: String,
    files: Vec<(String, String)>,
}

impl Release {
    /// Out of GitHub's answer for the latest release.
    fn from_json(text: &str) -> Result<Release, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("GitHub's answer isn't JSON: {e}"))?;
        let field = |v: &serde_json::Value, k: &str| v.get(k).and_then(|f| f.as_str()).map(str::to_string);
        let version = field(&v, "tag_name").unwrap_or_default().trim_start_matches('v').to_string();
        if parse(&version).is_none() {
            return Err(format!("the latest release's version isn't x.y.z: {version:?}"));
        }
        let files = v
            .get("assets")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|f| Some((field(f, "name")?, field(f, "browser_download_url")?))).collect())
            .unwrap_or_default();
        Ok(Release { version, files })
    }

    fn url(&self, name: &str) -> Option<&str> {
        self.files.iter().find(|(n, _)| n == name).map(|(_, u)| u.as_str())
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .user_agent(format!("Roxaruu/{}", crate::VERSION))
        .build()
        .into()
}

/// A reader of `url`'s body, at most `limit` bytes.
fn open(source: &Source, url: &str, limit: u64) -> Result<Box<dyn Read + Send>, String> {
    match source {
        Source::GitHub => {
            let response = agent().get(url).call().map_err(|e| format!("couldn't reach {url}: {e}"))?;
            Ok(Box::new(response.into_body().into_with_config().limit(limit).reader()))
        }
        #[cfg(test)]
        Source::Folder(_) => Ok(Box::new(std::fs::File::open(url).map_err(|e| format!("{url}: {e}"))?.take(limit))),
    }
}

fn text(source: &Source, url: &str) -> Result<String, String> {
    let mut s = String::new();
    open(source, url, 1 << 20)?.read_to_string(&mut s).map_err(|e| format!("{url}: {e}"))?;
    Ok(s)
}

/// The latest release.
pub(crate) fn latest(source: &Source) -> Result<Release, String> {
    match source {
        Source::GitHub => Release::from_json(&text(source, LATEST)?),
        #[cfg(test)]
        Source::Folder(dir) => {
            let mut r = Release::from_json(&text(source, &dir.join("latest.json").to_string_lossy())?)?;
            for (name, url) in &mut r.files {
                *url = dir.join(&name).to_string_lossy().into_owned();
            }
            Ok(r)
        }
    }
}

/// `SHA256SUMS` (`<hex>  <name>` per line, as `sha256sum` writes it): the
/// sum for `name`.
fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hex, file) = l.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hex.trim().to_ascii_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The folder all this lives in (`<local data>/Roxaruu/update`).
pub(crate) fn dir() -> Option<PathBuf> {
    crate::log_path().and_then(|l| l.parent().map(|d| d.join("update")))
}

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether it's time to ask GitHub again (no window has in `EVERY`).
pub(crate) fn due(dir: &Path) -> bool {
    let at = std::fs::read_to_string(dir.join("checked")).ok().and_then(|s| s.trim().parse::<u64>().ok());
    at.is_none_or(|at| now().saturating_sub(at) >= EVERY.as_secs())
}

/// What's waiting, for a copy at `current`: a version installed by another
/// window (restart to use it), or one downloaded and ready to install.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Waiting {
    Installed(String),
    Ready { version: String, file: PathBuf },
}

/// What's waiting on disk. Anything not newer than `current` is done with,
/// and cleared away.
pub(crate) fn waiting(dir: &Path, current: &str) -> Option<Waiting> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    if let Some(v) = read("installed").map(|s| s.trim().to_string()) {
        if newer(&v, current) {
            return Some(Waiting::Installed(v));
        }
        let _ = std::fs::remove_file(dir.join("installed"));
    }
    let ready = read("ready")?;
    let (version, file) = ready.trim().split_once(' ')?;
    let file = dir.join(file);
    if newer(version, current) && file.is_file() {
        return Some(Waiting::Ready { version: version.to_string(), file });
    }
    clear_ready(dir);
    None
}

/// Forget what was downloaded: the `ready` note and every package.
fn clear_ready(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("ready"));
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with("roxaruu_v") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// One window at a time checks and downloads: the one holding `lock`.
/// A lock left by a window that's gone is taken over.
struct Lock(PathBuf);

impl Lock {
    fn take(dir: &Path) -> Option<Lock> {
        let path = dir.join("lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = write!(f, "{}", std::process::id());
                    return Some(Lock(path));
                }
                Err(_) => {
                    let holder = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<i32>().ok());
                    if holder.is_some_and(crate::sys::process_alive) {
                        return None;
                    }
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        None
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What a check found.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Found {
    UpToDate(String),
    /// Downloaded and verified (`waiting` has it).
    Ready(String),
    /// Out, but this copy updates by hand (or has no package for this computer).
    ByHand(String),
    /// Another window is checking right now.
    Busy,
}

/// Ask `source` for the latest release and, if it's newer than `current`,
/// download this copy's package (for `arch`) into `dir` and verify it.
/// `progress` hears the version once it's downloading.
pub(crate) fn check(
    source: &Source,
    dir: &Path,
    (install, arch): (&Install, &str),
    current: &str,
    progress: &dyn Fn(&str),
) -> Result<Found, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    let Some(_lock) = Lock::take(dir) else { return Ok(Found::Busy) };
    let release = latest(source)?;
    let _ = std::fs::write(dir.join("checked"), now().to_string());
    let version = release.version.clone();
    tracing::info!(latest = %version, current, ?install, "checked for updates");
    if !newer(&version, current) {
        return Ok(Found::UpToDate(version));
    }
    if let Some(Waiting::Ready { version: v, .. } | Waiting::Installed(v)) = waiting(dir, current)
        && v == version
    {
        return Ok(Found::Ready(version));
    }
    let Some(name) = package(install, arch, &version).filter(|n| release.url(n).is_some()) else {
        return Ok(Found::ByHand(version));
    };
    // Nothing that isn't in the release's sums is installed.
    let sums = release.url("SHA256SUMS").ok_or("the release has no SHA256SUMS to check the download against")?;
    let want = sum_for(&text(source, sums)?, &name).ok_or_else(|| format!("SHA256SUMS has no line for {name}"))?;
    progress(&version);
    clear_ready(dir); // an older one, or half of one
    let url = release.url(&name).unwrap_or_default();
    let part = dir.join(format!("{name}.part"));
    let got = download(source, url, &part)?;
    if got != want {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the download of {name} doesn't match its checksum; nothing was installed"));
    }
    std::fs::rename(&part, dir.join(&name)).map_err(|e| format!("couldn't keep {name}: {e}"))?;
    std::fs::write(dir.join("ready"), format!("{version} {name}")).map_err(|e| format!("couldn't note the download: {e}"))?;
    tracing::info!(%version, file = %name, "update downloaded and verified");
    Ok(Found::Ready(version))
}

/// Download `url` to `to`, returning its SHA-256.
fn download(source: &Source, url: &str, to: &Path) -> Result<String, String> {
    let mut body = open(source, url, MAX_PACKAGE)?;
    let mut file = std::fs::File::create(to).map_err(|e| format!("couldn't write {}: {e}", to.display()))?;
    let mut sha = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("the download stopped: {e}"))?;
        let Some(chunk) = buf.get(..n).filter(|c| !c.is_empty()) else { break };
        sha.update(chunk);
        file.write_all(chunk).map_err(|e| format!("couldn't write {}: {e}", to.display()))?;
    }
    file.sync_all().map_err(|e| format!("couldn't write {}: {e}", to.display()))?;
    Ok(hex(sha.finish().as_ref()))
}

/// Run `program`, saying what went wrong if it didn't work.
fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("{program} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
}

/// Install what's downloaded in `dir`, for `install`. Windows' installer
/// starts on its own and finishes after this process has gone, since it
/// replaces the running program; `relaunch` has it open Roxaruu again
/// (`--after-update`). Returns the version installed, or `None` when
/// nothing is waiting.
pub(crate) fn install_ready(dir: &Path, install: &Install, current: &str, relaunch: bool) -> Result<Option<String>, String> {
    let Some(Waiting::Ready { version, file }) = waiting(dir, current) else {
        return Ok(None);
    };
    tracing::info!(%version, ?install, "installing the update");
    match install {
        Install::MacApp(app) => replace_app(&file, app, &dir.join("mount"))?,
        Install::LinuxTar(prefix) => unpack(&file, prefix)?,
        Install::WindowsSetup => {
            let mut c = std::process::Command::new(&file);
            c.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER"]);
            if relaunch {
                c.arg("/RELAUNCH=1");
            }
            c.spawn().map_err(|e| format!("couldn't start the installer: {e}"))?;
            // The package stays until a start at its version clears it
            // (`waiting`): if the installer failed, the next close tries again.
            return Ok(Some(version));
        }
        Install::LinuxDeb | Install::ByHand => return Err("this copy can't install updates by itself".into()),
    }
    let _ = std::fs::write(dir.join("installed"), &version);
    clear_ready(dir);
    tracing::info!(%version, "update installed");
    Ok(Some(version))
}

/// macOS: swap `app` for the one in the disk image `dmg`. The old one is
/// put back if the new one can't go in. A running copy carries on: it
/// still has its files open.
fn replace_app(dmg: &Path, app: &Path, mount: &Path) -> Result<(), String> {
    let _ = run("hdiutil", &["detach".as_ref(), "-quiet".as_ref(), "-force".as_ref(), mount.as_os_str()]);
    std::fs::create_dir_all(mount).map_err(|e| format!("couldn't make {}: {e}", mount.display()))?;
    run(
        "hdiutil",
        &["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet", "-mountpoint"]
            .map(std::ffi::OsStr::new)
            .into_iter()
            .chain([mount.as_os_str(), dmg.as_os_str()])
            .collect::<Vec<_>>(),
    )?;
    let new = app.with_extension("app.new");
    let _ = std::fs::remove_dir_all(&new);
    let copied = run("ditto", &[mount.join("Roxaruu.app").as_os_str(), new.as_os_str()]);
    let _ = run("hdiutil", &["detach".as_ref(), "-quiet".as_ref(), mount.as_os_str()]);
    copied?;
    let old = app.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(app, &old).map_err(|e| format!("couldn't move {} aside: {e}", app.display()))?;
    if let Err(e) = std::fs::rename(&new, app) {
        let _ = std::fs::rename(&old, app);
        return Err(format!("couldn't put the new Roxaruu.app in place: {e}"));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Linux: unpack the .tar.xz over `prefix`, then point its menu entry and
/// `rox` at this copy, as `install.sh` did (the package's point at /usr/bin).
fn unpack(tar: &Path, prefix: &Path) -> Result<(), String> {
    run("tar", &["-xJf".as_ref(), tar.as_os_str(), "-C".as_ref(), prefix.as_os_str()])?;
    let exe = prefix.join("bin").join("roxaruu");
    let desktop = prefix.join("share/applications/roxaruu.desktop");
    if let Ok(entry) = std::fs::read_to_string(&desktop) {
        let _ = std::fs::write(&desktop, entry.replace("/usr/bin/roxaruu", &exe.to_string_lossy()));
    }
    let rox = prefix.join("bin").join("rox");
    let _ = std::fs::write(&rox, format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", exe.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&rox, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// The shell command that installs a downloaded .deb (it asks for your password).
pub(crate) fn apt_command(deb: &Path) -> String {
    format!("sudo apt-get install -y '{}'", deb.to_string_lossy().replace('\'', r"'\''"))
}

/// This window asked to restart into the update (Restart to Update).
static RESTART: AtomicBool = AtomicBool::new(false);
/// The folder to reopen after the restart.
static REOPEN: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Whether this window will restart into the update as it closes.
#[cfg(test)]
pub(crate) fn restarting() -> bool {
    RESTART.load(Relaxed)
}

/// Restart into the update once this window has closed, reopening `folder`.
pub(crate) fn restart_after_close(folder: Option<PathBuf>) {
    RESTART.store(true, Relaxed);
    if let Ok(mut f) = REOPEN.lock() {
        *f = folder;
    }
}

/// The window has closed. Install what's waiting if this was the last
/// window, or this one restarts into it (on Windows, only as the last:
/// its installer closes every running copy). Then restart, if asked.
pub(crate) fn on_close(others: usize) {
    let restart = RESTART.load(Relaxed);
    let folder = REOPEN.lock().ok().and_then(|mut f| f.take());
    let Some(dir) = dir() else { return };
    let install = this_install();
    let windows = matches!(install, Install::WindowsSetup);
    let now = others == 0 || (restart && !windows);
    let mut relaunch = restart;
    if now && matches!(install, Install::MacApp(_) | Install::LinuxTar(_) | Install::WindowsSetup) {
        if windows && restart {
            let _ = std::fs::write(dir.join("reopen"), folder.as_ref().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default());
        }
        match install_ready(&dir, &install, crate::VERSION, restart) {
            // Windows' installer opens the new one itself.
            Ok(Some(_)) if windows => relaunch = false,
            Ok(_) => {}
            Err(e) => tracing::error!(error = %e, "couldn't install the update"),
        }
    }
    if relaunch {
        let Ok(exe) = std::env::current_exe() else { return };
        let mut c = std::process::Command::new(exe);
        c.arg("--foreground").args(folder).stdin(std::process::Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        match c.spawn() {
            Ok(_) => tracing::info!("restarted"),
            Err(e) => tracing::error!(error = %e, "couldn't restart"),
        }
    }
}

/// `--after-update` (Windows' installer starting it again): the folder the
/// window had open, said once.
pub(crate) fn take_reopen() -> Option<PathBuf> {
    let file = dir()?.join("reopen");
    let folder = std::fs::read_to_string(&file).ok();
    let _ = std::fs::remove_file(&file);
    folder.filter(|f| !f.trim().is_empty()).map(|f| PathBuf::from(f.trim()))
}

/// `rox --update`: check, download and install now, saying how it went.
pub(crate) fn from_command_line(others: usize) -> i32 {
    let Some(dir) = dir() else {
        eprintln!("roxaruu: no folder to download into");
        return 1;
    };
    let install = this_install();
    println!("Roxaruu {}: checking for a newer version…", crate::VERSION);
    let found = check(&Source::GitHub, &dir, (&install, std::env::consts::ARCH), crate::VERSION, &|v| {
        println!("Downloading Roxaruu {v}…");
    });
    let version = match found {
        Err(e) => {
            eprintln!("roxaruu: {e}");
            return 1;
        }
        Ok(Found::UpToDate(_)) => {
            println!("Roxaruu is up to date.");
            return 0;
        }
        Ok(Found::Busy) => {
            println!("A Roxaruu window is downloading the update right now; try again in a moment.");
            return 1;
        }
        Ok(Found::ByHand(v)) => {
            println!("Roxaruu {v} is out. This copy updates by hand: {RELEASES}");
            return 0;
        }
        Ok(Found::Ready(v)) => v,
    };
    let file = match waiting(&dir, crate::VERSION) {
        Some(Waiting::Ready { file, .. }) => file,
        _ => {
            println!("Roxaruu {version} is already installed: restart Roxaruu to use it.");
            return 0;
        }
    };
    match install {
        Install::LinuxDeb => {
            let ok = std::process::Command::new("sudo")
                .args(["apt-get", "install", "-y"])
                .arg(&file)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                eprintln!("roxaruu: apt couldn't install it");
                return 1;
            }
            clear_ready(&dir);
        }
        Install::WindowsSetup if others > 0 => {
            println!("Roxaruu {version} is downloaded. It installs when you close the last Roxaruu window ({others} open).");
            return 0;
        }
        Install::WindowsSetup => {
            let ok = std::process::Command::new(&file)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER"])
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                eprintln!("roxaruu: the installer failed");
                return 1;
            }
            clear_ready(&dir);
        }
        _ => {
            if let Err(e) = install_ready(&dir, &install, crate::VERSION, false) {
                eprintln!("roxaruu: {e}");
                return 1;
            }
        }
    }
    let _ = std::fs::write(dir.join("installed"), &version);
    println!("Installed Roxaruu {version}.{}", if others > 0 { " Open windows use it once they restart." } else { "" });
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linux and Windows packages are x86-64 only: the tests' (any test
    /// machine's) arch.
    const ARCH: &str = "x86_64";

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.2.63", "0.2.62"));
        assert!(newer("v0.10.0", "0.9.99"), "by number, not by text");
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.2.62", "0.2.62"));
        assert!(!newer("0.2.61", "0.2.62"));
        assert!(!newer("0.3", "0.2.62"), "not x.y.z: never newer");
        assert!(!newer("0.2.63-beta", "0.2.62"));
        assert!(!newer("0.2.63", "nonsense"));
    }

    #[test]
    fn each_install_gets_its_own_package() {
        let mac = Install::MacApp("/Applications/Roxaruu.app".into());
        assert_eq!(package(&mac, "aarch64", "1.2.3").as_deref(), Some("roxaruu_v1.2.3_macos_apple-silicon.dmg"));
        assert_eq!(package(&mac, "x86_64", "1.2.3").as_deref(), Some("roxaruu_v1.2.3_macos_intel.dmg"));
        assert_eq!(package(&Install::WindowsSetup, "x86_64", "1.2.3").as_deref(), Some("roxaruu_v1.2.3_windows_x64_setup.exe"));
        assert_eq!(package(&Install::LinuxTar("/x".into()), "x86_64", "1.2.3").as_deref(), Some("roxaruu_v1.2.3_linux_x64.tar.xz"));
        assert_eq!(package(&Install::LinuxDeb, "x86_64", "1.2.3").as_deref(), Some("roxaruu_v1.2.3_linux_x64.deb"));
        assert_eq!(package(&Install::LinuxDeb, "aarch64", "1.2.3"), None, "no Linux build for ARM yet");
        assert_eq!(package(&Install::ByHand, "x86_64", "1.2.3"), None);
    }

    #[test]
    fn how_a_copy_updates_follows_where_its_installer_put_it() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // macOS: inside an app bundle, in a folder we can write to.
        let exe = root.join("Apps/Roxaruu.app/Contents/MacOS/roxaruu");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        assert_eq!(install_of("macos", &exe), Install::MacApp(root.join("Apps/Roxaruu.app")));
        assert_eq!(install_of("macos", &root.join("target/release/roxaruu")), Install::ByHand, "built from source");
        // Windows: the installer's uninstaller beside it; the .zip has none.
        let win = root.join("Programs/Roxaruu/roxaruu.exe");
        std::fs::create_dir_all(win.parent().unwrap()).unwrap();
        assert_eq!(install_of("windows", &win), Install::ByHand);
        std::fs::write(win.with_file_name("unins000.exe"), "").unwrap();
        assert_eq!(install_of("windows", &win), Install::WindowsSetup);
        // Linux: the .deb's, the .tar.xz's (with its menu entry), or by hand.
        assert_eq!(install_of("linux", Path::new("/usr/bin/roxaruu")), Install::LinuxDeb);
        let local = root.join(".local");
        let tar = local.join("bin/roxaruu");
        std::fs::create_dir_all(tar.parent().unwrap()).unwrap();
        assert_eq!(install_of("linux", &tar), Install::ByHand, "cargo's bin has no menu entry");
        std::fs::create_dir_all(local.join("share/applications")).unwrap();
        std::fs::write(local.join("share/applications/roxaruu.desktop"), "").unwrap();
        assert_eq!(install_of("linux", &tar), Install::LinuxTar(local));
    }

    #[test]
    fn sums_are_read_as_sha256sum_writes_them() {
        let sums = "ABC123  roxaruu_v1_linux_x64.deb\nfff *roxaruu_v1_macos_intel.dmg\n";
        assert_eq!(sum_for(sums, "roxaruu_v1_linux_x64.deb").as_deref(), Some("abc123"));
        assert_eq!(sum_for(sums, "roxaruu_v1_macos_intel.dmg").as_deref(), Some("fff"), "binary mode's *");
        assert_eq!(sum_for(sums, "roxaruu_v1_linux_x64.tar.xz"), None);
    }

    #[test]
    fn githubs_answer_is_read() {
        let json = r#"{"tag_name":"v0.3.0","assets":[{"name":"SHA256SUMS","browser_download_url":"https://x/SHA256SUMS"}]}"#;
        let r = Release::from_json(json).unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(r.url("SHA256SUMS"), Some("https://x/SHA256SUMS"));
        assert!(Release::from_json(r#"{"tag_name":"nightly"}"#).is_err());
        assert!(Release::from_json("not json").is_err());
    }

    /// A fake release, in a folder: `latest.json`, the package for `install`
    /// with `body`, and its SHA256SUMS (`sum` overrides the right one).
    fn release(dir: &Path, version: &str, install: &Install, body: &[u8], sum: Option<&str>) -> String {
        let name = package(install, ARCH, version).unwrap();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(&name), body).unwrap();
        let right = hex(ring::digest::digest(&ring::digest::SHA256, body).as_ref());
        std::fs::write(dir.join("SHA256SUMS"), format!("{}  {name}\n", sum.unwrap_or(&right))).unwrap();
        let json = serde_json::json!({
            "tag_name": format!("v{version}"),
            "assets": [
                {"name": name, "browser_download_url": "x"},
                {"name": "SHA256SUMS", "browser_download_url": "x"},
            ],
        });
        std::fs::write(dir.join("latest.json"), json.to_string()).unwrap();
        name
    }

    #[test]
    fn a_newer_release_is_downloaded_verified_and_waits() {
        let d = tempfile::tempdir().unwrap();
        let (rel, dir) = (d.path().join("release"), d.path().join("update"));
        let install = Install::LinuxTar(d.path().join("prefix"));
        let source = Source::Folder(rel.clone());
        let name = release(&rel, "9.0.0", &install, b"package", None);
        let heard = std::cell::RefCell::new(Vec::new());
        let progress = |v: &str| heard.borrow_mut().push(v.to_string());
        // Already at it (or past it): nothing to do.
        assert_eq!(check(&source, &dir, (&install, ARCH), "9.0.0", &progress), Ok(Found::UpToDate("9.0.0".into())));
        assert!(!due(&dir), "a check was just made: the windows wait EVERY for the next");
        // Behind: downloaded, verified, waiting.
        assert_eq!(check(&source, &dir, (&install, ARCH), "1.0.0", &progress), Ok(Found::Ready("9.0.0".into())));
        assert_eq!(*heard.borrow(), ["9.0.0"]);
        let file = dir.join(&name);
        assert_eq!(std::fs::read(&file).unwrap(), b"package");
        assert_eq!(waiting(&dir, "1.0.0"), Some(Waiting::Ready { version: "9.0.0".into(), file: file.clone() }));
        // Asked again: not downloaded twice.
        assert_eq!(check(&source, &dir, (&install, ARCH), "1.0.0", &progress), Ok(Found::Ready("9.0.0".into())));
        assert_eq!(heard.borrow().len(), 1);
        // Once this copy is at it, it's cleared away.
        assert_eq!(waiting(&dir, "9.0.0"), None);
        assert!(!file.exists() && !dir.join("ready").exists());
        // By hand: told, nothing downloaded.
        assert_eq!(check(&source, &dir, (&Install::ByHand, ARCH), "1.0.0", &progress), Ok(Found::ByHand("9.0.0".into())));
        assert!(!file.exists());
    }

    #[test]
    fn a_download_that_doesnt_match_its_sum_is_thrown_away() {
        let d = tempfile::tempdir().unwrap();
        let (rel, dir) = (d.path().join("release"), d.path().join("update"));
        let install = Install::LinuxTar(d.path().join("prefix"));
        let name = release(&rel, "9.0.0", &install, b"tampered", Some(&"0".repeat(64)));
        let e = check(&Source::Folder(rel.clone()), &dir, (&install, ARCH), "1.0.0", &|_| {}).unwrap_err();
        assert!(e.contains("checksum"), "{e}");
        assert!(!dir.join(&name).exists() && !dir.join(format!("{name}.part")).exists());
        assert_eq!(waiting(&dir, "1.0.0"), None);
        // No SHA256SUMS at all: nothing is downloaded.
        std::fs::remove_file(rel.join("SHA256SUMS")).unwrap();
        let json = serde_json::json!({"tag_name": "v9.0.0", "assets": [{"name": name, "browser_download_url": "x"}]});
        std::fs::write(rel.join("latest.json"), json.to_string()).unwrap();
        let e = check(&Source::Folder(rel), &dir, (&install, ARCH), "1.0.0", &|_| {}).unwrap_err();
        assert!(e.contains("SHA256SUMS"), "{e}");
    }

    #[test]
    fn one_window_checks_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let first = Lock::take(d.path()).unwrap();
        assert!(Lock::take(d.path()).is_none(), "held by a live process (this one)");
        drop(first);
        assert!(Lock::take(d.path()).is_some(), "let go");
        // Left behind by a window that's gone: taken over.
        std::fs::write(d.path().join("lock"), "999999999").unwrap();
        assert!(Lock::take(d.path()).is_some());
    }

    #[test]
    fn a_version_another_window_installed_says_restart() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("installed"), "2.0.0").unwrap();
        assert_eq!(waiting(d.path(), "1.0.0"), Some(Waiting::Installed("2.0.0".into())));
        assert_eq!(waiting(d.path(), "2.0.0"), None, "restarted into it");
        assert!(!d.path().join("installed").exists());
    }

    /// Linux's .tar.xz: unpacked over the prefix, its menu entry and `rox`
    /// pointing at this copy, as install.sh left them.
    #[cfg(unix)]
    #[test]
    fn a_tar_install_is_updated_in_place() {
        let d = tempfile::tempdir().unwrap();
        let (rel, dir, prefix, pkg) =
            (d.path().join("release"), d.path().join("update"), d.path().join("local"), d.path().join("pkg"));
        for sub in ["bin", "share/applications"] {
            std::fs::create_dir_all(pkg.join(sub)).unwrap();
            std::fs::create_dir_all(prefix.join(sub)).unwrap();
        }
        std::fs::write(pkg.join("bin/roxaruu"), "new").unwrap();
        std::fs::write(pkg.join("bin/rox"), "#!/bin/sh\nexec /usr/bin/roxaruu \"$@\"\n").unwrap();
        std::fs::write(pkg.join("share/applications/roxaruu.desktop"), "Exec=/usr/bin/roxaruu %F\n").unwrap();
        std::fs::write(prefix.join("bin/roxaruu"), "old").unwrap();
        let tar = d.path().join("p.tar.xz");
        let ok = std::process::Command::new("tar").arg("-cJf").arg(&tar).arg("-C").arg(&pkg).arg(".").status().unwrap();
        assert!(ok.success());
        let install = Install::LinuxTar(prefix.clone());
        release(&rel, "9.0.0", &install, &std::fs::read(&tar).unwrap(), None);
        check(&Source::Folder(rel), &dir, (&install, ARCH), "1.0.0", &|_| {}).unwrap();
        assert_eq!(install_ready(&dir, &install, "1.0.0", false), Ok(Some("9.0.0".into())));
        assert_eq!(std::fs::read_to_string(prefix.join("bin/roxaruu")).unwrap(), "new");
        let exe = prefix.join("bin/roxaruu").to_string_lossy().into_owned();
        assert!(std::fs::read_to_string(prefix.join("share/applications/roxaruu.desktop")).unwrap().contains(&exe));
        assert!(std::fs::read_to_string(prefix.join("bin/rox")).unwrap().contains(&exe));
        assert_eq!(waiting(&dir, "1.0.0"), Some(Waiting::Installed("9.0.0".into())), "the other windows: restart");
        assert_eq!(install_ready(&dir, &install, "1.0.0", false), Ok(None), "nothing left to install");
    }

    /// macOS: the app in the disk image replaces this one.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_app_is_swapped_for_the_disk_images() {
        let d = tempfile::tempdir().unwrap();
        let (rel, dir, apps, src) =
            (d.path().join("release"), d.path().join("update"), d.path().join("Apps"), d.path().join("src"));
        let app = apps.join("Roxaruu.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/roxaruu"), "old").unwrap();
        std::fs::create_dir_all(src.join("Roxaruu.app/Contents/MacOS")).unwrap();
        std::fs::write(src.join("Roxaruu.app/Contents/MacOS/roxaruu"), "new").unwrap();
        let dmg = d.path().join("r.dmg");
        let ok = std::process::Command::new("hdiutil")
            .args(["create", "-quiet", "-fs", "HFS+", "-format", "UDZO", "-srcfolder"])
            .arg(&src)
            .arg(&dmg)
            .status()
            .unwrap();
        assert!(ok.success(), "hdiutil create");
        let install = Install::MacApp(app.clone());
        release(&rel, "9.0.0", &install, &std::fs::read(&dmg).unwrap(), None);
        check(&Source::Folder(rel), &dir, (&install, ARCH), "1.0.0", &|_| {}).unwrap();
        assert_eq!(install_ready(&dir, &install, "1.0.0", false), Ok(Some("9.0.0".into())));
        assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/roxaruu")).unwrap(), "new");
        assert!(!apps.join("Roxaruu.app.old").exists() && !apps.join("Roxaruu.app.new").exists());
        assert!(!dir.join("mount/Roxaruu.app").exists(), "the disk image is let go");
    }

    #[test]
    fn apt_gets_the_path_quoted() {
        assert_eq!(apt_command(Path::new("/h/it's/r.deb")), r"sudo apt-get install -y '/h/it'\''s/r.deb'");
    }
}
