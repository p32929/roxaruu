//! Terminal bridge: makes scripts that open new OS Terminal windows open
//! *inside* Oxru instead.
//!
//! Many macOS scripts spawn extra windows with
//! `osascript … tell application "Terminal" … do script "CMD"`. When Oxru spawns
//! an embedded terminal it prepends a shim directory to the child's `PATH`. The
//! shim's fake `osascript` detects those "do script" calls, appends the command
//! to a request file, and exits without opening a window. Oxru polls that file
//! and opens a new embedded terminal per request. Every other `osascript` use
//! falls through to the real binary, so nothing else changes.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct Bridge {
    dir: PathBuf,
    request_file: PathBuf,
    /// A `ZDOTDIR` of zsh startup files that source the user's real config and
    /// then re-prepend `dir` to `PATH` (so the shim wins even after a login
    /// shell's `path_helper` reorders things). `None` if it couldn't be written.
    zdotdir: Option<PathBuf>,
    /// The POSIX snippet that does that re-prepending — sourced by the zsh
    /// startup files above and handed to bash as `BASH_ENV`, so one copy of
    /// the logic covers every shell kind. See [`write_path_guard`].
    path_guard: Option<PathBuf>,
}

static BRIDGE: OnceLock<Option<Bridge>> = OnceLock::new();

/// Where bridge directories live: Oxru's own `<config dir>/oxru/bridge`, so
/// either app reaps the other's leftovers. Tests use the build directory, so
/// a `cargo test` never writes into the real one (and, like the real one, it
/// isn't under `$TMPDIR`, which macOS sweeps).
fn bridge_root() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("test-bridge"),
        );
    }
    dirs::config_dir().map(|d| d.join("oxru").join("bridge"))
}

/// The fake `osascript`. Routes terminal-opening AppleScript to Oxru, runs the
/// real `osascript` for anything else.
///
/// Recognises the three shapes a script can arrive in — `-e` arguments, a
/// script on stdin, and a `.scpt`/`.applescript` file argument — and both the
/// AppleScript (`do script "…"`, iTerm's `write text "…"`) and JavaScript
/// (`doScript("…")`) spellings. Anything it declines is logged to
/// `$OXRU_REQUEST_FILE.passthrough` with its arguments, so a call that still
/// escapes and opens a window can be identified instead of guessed at.
const OSASCRIPT_SHIM: &str = r##"#!/bin/bash
# Oxru osascript shim — see src/termbridge.rs.
REAL="/usr/bin/osascript"
[ -n "$OXRU_REQUEST_FILE" ] || exec "$REAL" "$@"

# Reconstruct the source: concatenated -e args, a script file argument, or stdin.
src=""
have_e=0
file=""
argv=("$@")
n=${#argv[@]}
i=0
while [ $i -lt $n ]; do
  a="${argv[$i]}"
  case "$a" in
    -e) have_e=1; i=$((i+1)); src="$src${argv[$i]}
" ;;
    -*) : ;;                       # -l, -s, … — flags we don't need to read
    *) [ -z "$file" ] && [ -f "$a" ] && file="$a" ;;
  esac
  i=$((i+1))
done
if [ $have_e -eq 0 ]; then
  if [ -n "$file" ]; then
    src="$(cat -- "$file" 2>/dev/null)"
  else
    src="$(cat)"
  fi
fi

# Pull out every command that would open a terminal: AppleScript `do script`,
# iTerm's `write text`, and the JavaScript-for-Automation `doScript(...)`.
# `do script ""` — an EMPTY command — still opens a window; it is how a script
# asks for a bare shell (run.sh-style scratch windows do exactly this). An
# empty capture used to print a blank line, which the command substitution
# below stripped, leaving $cmds empty and indistinguishable from "found
# nothing" — so the one call that *always* opens a window was the one that
# escaped. Emit the same sentinel `open -a Terminal` uses instead.
cmds="$(printf '%s' "$src" | perl -0777 -ne '
  sub emit { my $c=shift; $c=~s/\s*\n\s*/ /g; print($c =~ /\S/ ? "$c\n" : "@SHELL_REQUEST@\n"); }
  while (/(?:do\s+script|write\s+text)\s+"((?:[^"\\]|\\.)*)"/gis) { my $c=$1; $c=~s/\\"/"/g; emit($c); }
  while (/doScript\s*\(\s*"((?:[^"\\]|\\.)*)"/gs)   { my $c=$1; $c=~s/\\"/"/g; emit($c); }
  while (/doScript\s*\(\s*'"'"'((?:[^'"'"'\\]|\\.)*)'"'"'/gs) { emit($1); }
')"

if [ -n "$cmds" ]; then
  printf '%s\n' "$cmds" >> "$OXRU_REQUEST_FILE"
  exit 0
fi

# Not a terminal-opening script: record it, then run the real osascript.
printf 'osascript %s\n' "$*" >> "$OXRU_REQUEST_FILE.passthrough" 2>/dev/null
if [ $have_e -eq 1 ] || [ -n "$file" ]; then
  exec "$REAL" "$@"
else
  printf '%s' "$src" | "$REAL"
  exit $?
fi
"##;

/// The fake `open`. `open -a Terminal`, `open -b com.apple.Terminal` and
/// `open foo.command` are the other everyday ways a script starts a terminal,
/// and none of them go anywhere near `osascript` — so the `osascript` shim
/// alone left them opening real windows.
const OPEN_SHIM: &str = r##"#!/bin/bash
# Oxru open shim — see src/termbridge.rs.
REAL="/usr/bin/open"
[ -n "$OXRU_REQUEST_FILE" ] || exec "$REAL" "$@"

app=""
bundle=""
files=()
argv=("$@")
n=${#argv[@]}
i=0
while [ $i -lt $n ]; do
  a="${argv[$i]}"
  case "$a" in
    -a) i=$((i+1)); app="${argv[$i]}" ;;
    -b) i=$((i+1)); bundle="${argv[$i]}" ;;
    --args) break ;;               # everything after --args belongs to the app
    -*) : ;;
    *) files+=("$a") ;;
  esac
  i=$((i+1))
done

is_terminal_app() {
  case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" in
    *terminal*|*iterm*|*alacritty*|*kitty*|*wezterm*|*ghostty*|*hyper*|*warp*) return 0 ;;
  esac
  return 1
}

# Paths are queued absolute: the app reads the queue from its own directory,
# not the script's, so a relative `.` would point somewhere else entirely.
abs_path() {
  if [ -d "$1" ]; then (cd "$1" && pwd); else printf '%s/%s' "$(cd "$(dirname "$1")" && pwd)" "$(basename "$1")"; fi
}

# `open -a Terminal <dir>` opens a shell there; with no file it's just a shell.
if { [ -n "$app" ] && is_terminal_app "$app"; } || { [ -n "$bundle" ] && is_terminal_app "$bundle"; }; then
  target="${files[0]}"
  if [ -n "$target" ] && [ -d "$target" ]; then
    printf 'cd %q\n' "$(abs_path "$target")" >> "$OXRU_REQUEST_FILE"
  elif [ -n "$target" ] && [ -f "$target" ]; then
    printf '%q\n' "$(abs_path "$target")" >> "$OXRU_REQUEST_FILE"
  else
    printf '@SHELL_REQUEST@\n' >> "$OXRU_REQUEST_FILE"
  fi
  exit 0
fi

# A double-clickable shell script is a terminal window by another name.
for f in "${files[@]}"; do
  case "$f" in
    *.command|*.terminal|*.tool)
      printf '%q\n' "$(abs_path "$f")" >> "$OXRU_REQUEST_FILE"
      exit 0
      ;;
  esac
done

printf 'open %s\n' "$*" >> "$OXRU_REQUEST_FILE.passthrough" 2>/dev/null
exec "$REAL" "$@"
"##;

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn init() -> Option<Bridge> {
    let root = bridge_root()?;
    // Clear out the directories of instances that died without `cleanup()`
    // before claiming our own, so they don't accumulate orphaned shims.
    reap_stale_dirs(&reap_roots(&root));

    let dir = root.join(bridge_dir_name(std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;

    let request_file = dir.join("requests");
    // Unconditionally empty: nothing queued before this process started is
    // ours to act on. The directory name makes a collision impossible anyway
    // (see `bridge_dir_name`), so this is belt-and-braces rather than the
    // load-bearing guard it would be under a bare `oxru-<pid>`.
    std::fs::write(&request_file, b"").ok()?;

    let (path_guard, zdotdir) = write_bridge_files(&dir)?;

    Some(Bridge {
        dir,
        request_file,
        zdotdir,
        path_guard,
    })
}

/// Write (or rewrite) every file the bridge needs into `dir`, returning the
/// path guard and the zsh wrapper. Split out of [`init`] so [`ensure_intact`]
/// can restore them without rebuilding the whole [`Bridge`].
fn write_bridge_files(dir: &Path) -> Option<(Option<PathBuf>, Option<PathBuf>)> {
    // Every system: `claude` is Roxaruu's helper, which adds the hooks.
    link_claude_helper(dir);
    // The rest catch scripts opening Terminal.app, through Unix shells.
    if !cfg!(unix) {
        return Some((None, None));
    }
    let path_guard = write_path_guard(dir)?;

    let shim = dir.join("osascript");
    std::fs::write(&shim, shim_source(OSASCRIPT_SHIM)).ok()?;
    make_executable(&shim).ok()?;

    let open_shim = dir.join("open");
    std::fs::write(&open_shim, shim_source(OPEN_SHIM)).ok()?;
    make_executable(&open_shim).ok()?;

    Some((Some(path_guard), write_zdotdir(dir)))
}

/// The helper program (`roxaruu-hook`, src/bin) beside Roxaruu's own — and,
/// for tests, in the build folder above theirs.
fn helper_program() -> Option<PathBuf> {
    let name = format!("roxaruu-hook{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().ok()?;
    // Installed: beside the app. A test binary sits deeper under target/
    // (deps/, or build/<pkg>/<hash>/out/ since Cargo 1.100) than the
    // helper `cargo build` made.
    let up = if cfg!(test) { 6 } else { 1 };
    exe.ancestors().skip(1).take(up).map(|d| d.join(&name)).find(|p| p.is_file())
}

/// `claude` (and `roxaruu-hook`, which its hooks run) in the bridge
/// directory, first on every terminal's `PATH`: the helper, which starts the
/// real Claude Code with hooks that say when it's done or needs you. A link
/// (Unix) or a hard link, else a copy (Windows: a shell runs `claude.exe`).
fn link_claude_helper(dir: &Path) {
    let Some(helper) = helper_program() else {
        tracing::warn!("roxaruu-hook isn't beside Roxaruu: Claude Code's hooks are off");
        return;
    };
    for name in ["claude", "roxaruu-hook"] {
        let to = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let _ = std::fs::remove_file(&to);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&helper, &to);
        #[cfg(not(unix))]
        let made = std::fs::hard_link(&helper, &to).or_else(|_| std::fs::copy(&helper, &to).map(|_| ()));
        if let Err(e) = made {
            tracing::warn!(error = %e, to = %to.display(), "couldn't put the Claude Code helper in place");
        }
    }
}

/// Where Claude Code's hooks leave messages for a terminal: one file per
/// terminal, named by its `ROXARUU_TERM` (see `terminalpane`).
pub fn alerts_dir() -> Option<PathBuf> {
    bridge().map(|b| b.dir.join("alerts"))
}

/// A shim script with its placeholders filled in, so the sentinel the shims
/// queue is [`SHELL_REQUEST`] itself rather than a third hand-typed copy of
/// the literal that could drift from it.
fn shim_source(body: &str) -> String {
    body.replace("@SHELL_REQUEST@", SHELL_REQUEST)
}

/// Put the shims back if something removed them under us.
///
/// Defence in depth for a failure that is total and completely silent: `PATH`
/// still points at the directory, so `osascript` just quietly resolves to
/// `/usr/bin` again and every script opens a real window with nothing logged.
/// macOS's `dirhelper` sweep used to cause exactly this (see
/// [`crate::paths::bridge_root`]); moving out of `$TMPDIR` fixes that cause,
/// and this covers whatever else empties a directory we don't own the
/// lifetime of.
pub fn ensure_intact() {
    let Some(b) = bridge() else {
        return;
    };
    // One `exists()` per tick, next to the `metadata()` the request poll
    // already does. The shim is the file that matters; if it went, assume the
    // rest did too and rewrite the lot.
    let kept = if cfg!(unix) { "osascript" } else { "roxaruu-hook.exe" };
    if b.dir.join(kept).exists() || helper_program().is_none() {
        return;
    }
    tracing::warn!(
        dir = %b.dir.display(),
        "terminal-bridge files vanished — rewriting them"
    );
    let _ = std::fs::create_dir_all(&b.dir);
    let _ = write_bridge_files(&b.dir);
}

/// Name of this process's bridge directory.
///
/// The PID alone is **not** unique: macOS recycles PIDs, so an instance that
/// died without `cleanup()` (crash, force-quit, `kill -9`) left
/// `/tmp/oxru-<pid>` behind for a future instance to adopt — inheriting its
/// leftover `requests` file and spawning tabs for commands queued in a
/// previous session, and letting any still-living shell from that session
/// (whose `OXRU_REQUEST_FILE` points there) open tabs in the new window. A
/// per-process suffix makes the path unguessable from the PID alone, so a
/// recycled PID can neither adopt an old directory nor be fed by an orphaned
/// shell.
fn bridge_dir_name(pid: u32) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("oxru-{pid}-{nonce:x}")
}

/// The PID a bridge directory belongs to, for both the current
/// `oxru-<pid>-<nonce>` form and the bare `oxru-<pid>` older builds left
/// behind (which is exactly the litter most in need of reaping).
fn dir_owner_pid(name: &str) -> Option<i32> {
    let rest = name.strip_prefix("oxru-")?;
    let pid = rest.split('-').next()?;
    pid.parse().ok()
}

/// Delete the bridge directories of instances that are no longer running.
///
/// Without this, every crashed or force-quit instance — and every `cargo test`
/// run, which builds a real bridge to exercise the shims — leaves a directory
/// in `/tmp` holding a working `osascript` shim, a `ZDOTDIR`, and whatever was
/// queued in its `requests` file.
/// Where stale bridge directories can be found: our own home, plus the
/// `$TMPDIR` that earlier versions used — an upgrade otherwise leaves working
/// shims there for any shell still holding the old `PATH`.
fn reap_roots(root: &Path) -> Vec<PathBuf> {
    vec![root.to_path_buf(), std::env::temp_dir()]
}

fn reap_stale_dirs(roots: &[PathBuf]) {
    let me = std::process::id() as i32;
    for e in roots
        .iter()
        .filter_map(|r| std::fs::read_dir(r).ok())
        .flatten()
        .flatten()
    {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(dir_owner_pid) else {
            continue;
        };
        // Our own directory, and anything still owned by a live Oxru, stay.
        // `pid_is_oxru` answers "can't tell" as `true`, so an unreadable
        // process table leaves the litter alone rather than deleting the
        // bridge out from under a running window.
        // ponytail: one `ps` per candidate dir, once at startup — fine for
        // the handful of dirs this accumulates; batch the `ps` if it ever
        // grows into the hundreds.
        if pid == me || crate::instances::pid_is_oxru(pid) {
            continue;
        }
        let _ = std::fs::remove_dir_all(e.path());
    }
}

/// Marker dropped into every wrapper we write, so a nested Oxru can recognise
/// another instance's `ZDOTDIR` and refuse to chain to it.
const WRAPPER_MARKER: &str = ".oxru-zdotdir";

/// Resolve the value to export as `OXRU_REAL_ZDOTDIR` — the user's *own* zsh
/// config directory, never another Oxru's wrapper.
///
/// The bug this exists for: launching Oxru from inside an Oxru terminal used to
/// set `OXRU_REAL_ZDOTDIR` to the outer instance's wrapper, whose own startup
/// files then read `${OXRU_REAL_ZDOTDIR:-$HOME}` — still pointing at
/// themselves — and sourced themselves until zsh gave up with "recursion limit
/// exceeded", leaving the shell with none of the user's config loaded.
///
/// Returns an empty string when there's nothing real to point at; the wrapper's
/// `${OXRU_REAL_ZDOTDIR:-$HOME}` treats empty as unset and falls back to `$HOME`.
/// It's always exported, even empty, so a value inherited from an outer Oxru
/// can't leak into the child.
fn resolve_real_zdotdir(
    oxru_real: Option<String>,
    zdotdir: Option<String>,
    is_ours: impl Fn(&Path) -> bool,
) -> String {
    // An outer Oxru already did this resolution; pass its answer through
    // unchanged rather than resolving again on top of it.
    if let Some(v) = oxru_real.filter(|v| !v.is_empty()) {
        if !is_ours(Path::new(&v)) {
            return v;
        }
        // An outer instance that pointed at a wrapper (older build, or a
        // deeper nesting) — drop it rather than propagate the loop.
        return String::new();
    }
    match zdotdir.filter(|v| !v.is_empty()) {
        // Running inside another Oxru terminal: its ZDOTDIR is a wrapper of
        // ours, and chaining to it is exactly the recursion above.
        Some(z) if is_ours(Path::new(&z)) => String::new(),
        Some(z) => z,
        None => String::new(),
    }
}

/// Whether `dir` is a wrapper written by some Oxru instance.
///
/// The marker file alone isn't enough: a wrapper left by an *older* build
/// doesn't have one, and that's precisely the case that matters — you upgrade,
/// launch the new Oxru from a terminal of the still-running old one, and the
/// new instance has to recognise the old wrapper to avoid chaining to it. So
/// fall back to the content signature every version of the wrapper has: its
/// `.zshenv` references `OXRU_REAL_ZDOTDIR`, which nobody else's would.
fn is_oxru_zdotdir(dir: &Path) -> bool {
    if dir.join(WRAPPER_MARKER).exists() {
        return true;
    }
    std::fs::read_to_string(dir.join(".zshenv")).is_ok_and(|t| t.contains("OXRU_REAL_ZDOTDIR"))
}

/// The one copy of "put the shim directory back at the front of `PATH`".
///
/// POSIX `sh`, so the same file can be sourced by the zsh startup files *and*
/// handed to bash as `BASH_ENV` — one implementation for every shell kind
/// rather than a zsh-only one plus nothing for everyone else.
///
/// The `-d` test matters: once a dead instance's directory has been reaped,
/// a surviving shell stops putting a path that no longer exists back on
/// `PATH`.
const PATH_GUARD: &str = r##"# Oxru PATH guard — see src/termbridge.rs.
# We occupy $BASH_ENV to get here, so run whatever the user had there first.
if [ -n "$OXRU_REAL_BASH_ENV" ] && [ -f "$OXRU_REAL_BASH_ENV" ]; then
  . "$OXRU_REAL_BASH_ENV"
fi

# macOS's path_helper (/etc/zprofile, /etc/profile) rebuilds PATH for every
# *login* shell and demotes our shim directory behind /usr/bin, which is how a
# script reached the real osascript and opened a real Terminal window. Put it
# back, in whichever startup file the shell actually reads.
if [ -n "$OXRU_BRIDGE_DIR" ] && [ -d "$OXRU_BRIDGE_DIR" ]; then
  case ":$PATH:" in
    ":$OXRU_BRIDGE_DIR:"*) ;;                      # already first: nothing to do
    *) PATH="$OXRU_BRIDGE_DIR:$PATH"; export PATH ;;
  esac
fi
"##;

/// Write [`PATH_GUARD`] into the bridge directory and return its path.
fn write_path_guard(bridge_dir: &Path) -> Option<PathBuf> {
    let p = bridge_dir.join("path.sh");
    std::fs::write(&p, PATH_GUARD).ok()?;
    Some(p)
}

/// Whether `path` is a guard *we* wrote — recognised by its content, the same
/// way [`is_oxru_zdotdir`] recognises our `ZDOTDIR`.
///
/// Needed because `BASH_ENV` is inherited: launching Oxru from inside an Oxru
/// terminal hands the new instance the outer one's guard, and chaining to it
/// (see `OXRU_REAL_BASH_ENV`) would re-prepend a directory belonging to a
/// different instance.
fn is_oxru_path_guard(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains("Oxru PATH guard"))
}

/// The value to export as `OXRU_REAL_BASH_ENV`: the user's own `BASH_ENV`,
/// never another Oxru's guard. Empty when there is nothing real to chain to.
fn resolve_real_bash_env(inherited: Option<String>, is_ours: impl Fn(&Path) -> bool) -> String {
    match inherited.filter(|v| !v.is_empty()) {
        Some(v) if is_ours(Path::new(&v)) => String::new(),
        Some(v) => v,
        None => String::new(),
    }
}

/// Write a `ZDOTDIR` whose startup files source the user's real zsh config and
/// then put `bridge_dir` first on `PATH`. Returns the dir, or `None` on failure.
///
/// The guard is sourced from **all four** startup files, because which ones zsh
/// reads depends on the shell kind and the PATH prepend has to survive
/// `path_helper` in each: `.zshenv` always, `.zprofile`/`.zlogin` for login
/// shells (and they run *after* `/etc/zprofile`'s path_helper), `.zshrc` for
/// interactive ones. It used to live in `.zshrc` alone — so `zsh -lc` and any
/// other non-interactive shell, which is how plenty of `run.sh` wrappers and
/// npm scripts invoke their work, never got it and resolved `osascript` to
/// `/usr/bin`.
fn write_zdotdir(bridge_dir: &Path) -> Option<PathBuf> {
    let zdir = bridge_dir.join("zdotdir");
    std::fs::create_dir_all(&zdir).ok()?;
    // Lets a nested Oxru tell this directory apart from the user's own.
    std::fs::write(zdir.join(WRAPPER_MARKER), b"").ok()?;
    let real = "${OXRU_REAL_ZDOTDIR:-$HOME}";
    let me = zdir.display();
    let guard = bridge_dir.join("path.sh").display().to_string();
    // Always last in the file: the user's own config may rebuild PATH too, and
    // whoever writes it last wins.
    let apply = format!("[ -f \"{guard}\" ] && . \"{guard}\"\n");

    // .zshenv runs first, for every shell kind. Re-pin ZDOTDIR to ours in case
    // the user's .zshenv changed it, so the files below are guaranteed to run.
    std::fs::write(
        zdir.join(".zshenv"),
        format!("[ -f \"{real}/.zshenv\" ] && source \"{real}/.zshenv\"\nexport ZDOTDIR=\"{me}\"\n{apply}"),
    )
    .ok()?;
    // .zprofile / .zlogin: login shells, interactive or not — and the only
    // hook that runs after /etc/zprofile's path_helper for a `zsh -lc`.
    std::fs::write(
        zdir.join(".zprofile"),
        format!("[ -f \"{real}/.zprofile\" ] && source \"{real}/.zprofile\"\n{apply}"),
    )
    .ok()?;
    std::fs::write(
        zdir.join(".zlogin"),
        format!("[ -f \"{real}/.zlogin\" ] && source \"{real}/.zlogin\"\n{apply}"),
    )
    .ok()?;
    // .zshrc runs last for interactive shells: load the user's, then re-apply.
    std::fs::write(
        zdir.join(".zshrc"),
        format!("[ -f \"{real}/.zshrc\" ] && source \"{real}/.zshrc\"\n{apply}"),
    )
    .ok()?;
    Some(zdir)
}

fn bridge() -> Option<&'static Bridge> {
    BRIDGE.get_or_init(init).as_ref()
}

/// Environment to inject into a spawned terminal so child processes route
/// Terminal-opening calls back to Oxru. Empty if the bridge can't be set up.
pub fn child_env() -> Vec<(String, String)> {
    let Some(b) = bridge() else {
        return Vec::new();
    };
    // First on PATH, joined the system's way (`;` on Windows).
    let path = std::env::join_paths(
        std::iter::once(b.dir.clone()).chain(std::env::var_os("PATH").iter().flat_map(std::env::split_paths)),
    )
    .map(|p| p.to_string_lossy().into_owned())
    .unwrap_or_else(|_| b.dir.display().to_string());
    let mut env = vec![
        ("PATH".to_string(), path),
        (
            "OXRU_REQUEST_FILE".to_string(),
            b.request_file.display().to_string(),
        ),
        ("OXRU_TERMINAL".to_string(), "1".to_string()),
        ("ROXARUU_ALERTS".to_string(), b.dir.join("alerts").display().to_string()),
        // Read by `path.sh` — the guard is written once and needs to be told
        // which directory to re-prepend rather than having it baked in.
        ("OXRU_BRIDGE_DIR".to_string(), b.dir.display().to_string()),
        // bash reads `$BASH_ENV` for non-interactive shells, *after* the
        // profile files have run path_helper — so this is what keeps the shim
        // ahead of `/usr/bin` for `bash -c` and `bash -lc` alike. zsh gets the
        // same guard through its startup files (see `write_zdotdir`).
        //
        // Not `ENV`: macOS's `/bin/sh` honours neither hook for `sh -c`, so
        // setting it would be cargo-cult. A plain `sh -c` child inherits the
        // PATH its parent already fixed, which covers it; `sh -lc` re-runs
        // path_helper with no per-user hook to undo it and stays a known gap,
        // alongside a script calling `/usr/bin/osascript` by absolute path.
    ];
    if let Some(guard) = &b.path_guard {
        env.push(("BASH_ENV".to_string(), guard.display().to_string()));
        // Taking over BASH_ENV must not cost the user theirs; the guard
        // sources this first. Always exported, even empty, so a value
        // inherited from an outer Oxru can't leak into the child.
        env.push((
            "OXRU_REAL_BASH_ENV".to_string(),
            resolve_real_bash_env(std::env::var("BASH_ENV").ok(), is_oxru_path_guard),
        ));
    }

    // For zsh, point it at our ZDOTDIR so the shim stays first on PATH even
    // after the login shell's path_helper runs (see write_zdotdir). Preserve a
    // pre-existing ZDOTDIR so our files can still source the user's real config.
    let is_zsh = std::env::var("SHELL")
        .map(|s| s.rsplit('/').next().unwrap_or("").starts_with("zsh"))
        .unwrap_or(false);
    if is_zsh && let Some(zdir) = &b.zdotdir {
        let real = resolve_real_zdotdir(
            std::env::var("OXRU_REAL_ZDOTDIR").ok(),
            std::env::var("ZDOTDIR").ok(),
            is_oxru_zdotdir,
        );
        // Always exported, even empty: without it a value inherited from an
        // outer Oxru survives into the child and re-creates the loop.
        env.push(("OXRU_REAL_ZDOTDIR".to_string(), real));
        env.push(("ZDOTDIR".to_string(), zdir.display().to_string()));
    }
    env
}

/// Request-file line meaning "open a terminal with no command" — what a bare
/// `open -a Terminal` asks for. A blank line can't carry that: the drain
/// ignores blank lines, and rightly so, or a stray newline would spawn a
/// terminal.
pub const SHELL_REQUEST: &str = ":oxru-shell:";

/// Where the shims record calls they decided *not* to intercept. Read into
/// Oxru's log so a script that still manages to open a real window can be
/// identified from the log instead of guessed at.
pub fn passthrough_file() -> Option<PathBuf> {
    bridge().map(|b| {
        let mut p = b.request_file.clone().into_os_string();
        p.push(".passthrough");
        PathBuf::from(p)
    })
}

/// The request file Oxru polls for queued terminal-open commands.
pub fn request_file() -> Option<PathBuf> {
    bridge().map(|b| b.request_file.clone())
}

/// Remove the shim directory (best-effort, on exit).
pub fn cleanup() {
    if let Some(b) = bridge() {
        let _ = std::fs::remove_dir_all(&b.dir);
    }
}

/// Tests that take the bridge's files away, or need them there, take turns:
/// a rewrite under a running shell briefly has no `roxaruu-hook`.
#[cfg(test)]
pub(crate) static TEST_FILES: std::sync::Mutex<()> = std::sync::Mutex::new(());

// The Terminal.app catching is for Unix shells.
#[cfg(all(test, unix))]
mod tests {

    use super::*;
    use std::process::Command;

    /// The reported bug: `./run-local.sh --gui .` from inside an Oxru terminal
    /// produced, in every terminal of the new instance:
    ///
    /// ```text
    /// /T/oxru-26888/zdotdir/.zshenv:1: job table full or recursion limit exceeded
    /// ```
    ///
    /// because the new instance pointed `OXRU_REAL_ZDOTDIR` at the *outer*
    /// instance's wrapper, whose own `${OXRU_REAL_ZDOTDIR:-$HOME}` then
    /// resolved back to itself.
    #[test]
    fn a_nested_oxru_never_chains_to_another_instances_wrapper() {
        let outer = PathBuf::from("/tmp/oxru-26888/zdotdir");
        // `Path::starts_with` matches whole components, so compare as text.
        let ours = |p: &Path| p.to_string_lossy().contains("/oxru-");

        // Inside an Oxru terminal, with no OXRU_REAL_ZDOTDIR yet.
        assert_eq!(
            resolve_real_zdotdir(None, Some(outer.display().to_string()), ours),
            "",
            "must fall back to $HOME, not point at the outer wrapper"
        );

        // Deeper nesting: an outer instance already set it to a wrapper.
        assert_eq!(
            resolve_real_zdotdir(Some(outer.display().to_string()), None, ours),
            "",
            "a wrapper handed down from an outer instance is dropped, not propagated"
        );
    }

    #[test]
    fn a_real_zdotdir_is_still_honoured_and_passed_down() {
        // `Path::starts_with` matches whole components, so compare as text.
        let ours = |p: &Path| p.to_string_lossy().contains("/oxru-");

        // The ordinary case: the user really does set ZDOTDIR.
        assert_eq!(
            resolve_real_zdotdir(None, Some("/Users/me/.config/zsh".into()), ours),
            "/Users/me/.config/zsh"
        );
        // An outer Oxru already resolved it — pass that through untouched, so
        // the user's config still loads at any nesting depth.
        assert_eq!(
            resolve_real_zdotdir(
                Some("/Users/me/.config/zsh".into()),
                Some("/tmp/oxru-1/zdotdir".into()),
                ours
            ),
            "/Users/me/.config/zsh"
        );
        // Nothing set anywhere: empty, which the wrapper reads as $HOME.
        assert_eq!(resolve_real_zdotdir(None, None, ours), "");
        assert_eq!(
            resolve_real_zdotdir(Some(String::new()), Some(String::new()), ours),
            ""
        );
    }

    /// A wrapper written by an older build has no marker file, and that's the
    /// case that actually bites: upgrade, then launch the new Oxru from a
    /// terminal of the still-running old one.
    #[test]
    fn a_wrapper_from_an_older_build_is_still_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let zdir = dir.path().join("zdotdir");
        std::fs::create_dir_all(&zdir).unwrap();
        // Exactly what the previous version wrote — no marker file.
        std::fs::write(
            zdir.join(".zshenv"),
            "[ -f \"${OXRU_REAL_ZDOTDIR:-$HOME}/.zshenv\" ] && source \"${OXRU_REAL_ZDOTDIR:-$HOME}/.zshenv\"\n",
        )
        .unwrap();
        assert!(
            !zdir.join(WRAPPER_MARKER).exists(),
            "no marker, as an old build left it"
        );
        assert!(
            is_oxru_zdotdir(&zdir),
            "recognised by its content signature"
        );

        // Someone's genuine ZDOTDIR must not be mistaken for ours.
        let real = dir.path().join("myzsh");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join(".zshenv"), "export EDITOR=vim\n").unwrap();
        assert!(!is_oxru_zdotdir(&real));
    }

    /// The marker is what makes the detection above work on a real directory.
    #[test]
    fn every_wrapper_we_write_is_recognisable_as_ours() {
        let dir = tempfile::tempdir().unwrap();
        let zdir = write_zdotdir(dir.path()).expect("wrapper written");
        assert!(is_oxru_zdotdir(&zdir), "our own wrapper must be detectable");
        assert!(
            !is_oxru_zdotdir(dir.path()),
            "an unrelated directory must not be"
        );
    }

    /// Drive a shim with `args` in an isolated dir, with the "real" binary
    /// replaced by a stub, and return `(captured requests, escaped calls)`.
    fn run_shim(which: &str, body: &str, args: &[&str], stdin: Option<&str>) -> (String, String) {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("fake-real");
        std::fs::write(
            &real,
            format!(
                "#!/bin/bash\necho \"$*\" >> {}\n",
                dir.path().join("escaped").display()
            ),
        )
        .unwrap();
        let shim = dir.path().join(which);
        // Point the shim's REAL at the stub so a test can never open a window.
        let patched = shim_source(body).replace(
            &format!("REAL=\"/usr/bin/{which}\""),
            &format!("REAL=\"{}\"", real.display()),
        );
        std::fs::write(&shim, patched).unwrap();
        make_executable(&shim).unwrap();
        make_executable(&real).unwrap();

        let requests = dir.path().join("requests");
        std::fs::write(&requests, b"").unwrap();
        let mut child = Command::new(&shim)
            .args(args)
            .env("OXRU_REQUEST_FILE", &requests)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        child.wait().unwrap();
        (
            std::fs::read_to_string(&requests).unwrap_or_default(),
            std::fs::read_to_string(dir.path().join("escaped")).unwrap_or_default(),
        )
    }

    fn osa(args: &[&str], stdin: Option<&str>) -> (String, String) {
        run_shim("osascript", OSASCRIPT_SHIM, args, stdin)
    }
    fn open_(args: &[&str]) -> (String, String) {
        run_shim("open", OPEN_SHIM, args, None)
    }

    /// Every way a script opens a terminal has to be caught, or it opens a real
    /// window — which is the whole thing the bridge exists to prevent.
    #[test]
    fn every_terminal_opening_form_is_captured() {
        // AppleScript via -e, the common case.
        let (req, esc) = osa(
            &["-e", r#"tell application "Terminal" to do script "echo A""#],
            None,
        );
        assert!(req.contains("echo A"), "-e form");
        assert!(esc.is_empty(), "must not reach the real binary");

        // Multi-line AppleScript on stdin (the heredoc form).
        let (req, _) = osa(
            &[],
            Some("tell application \"Terminal\"\n activate\n do script \"echo B\"\nend tell\n"),
        );
        assert!(req.contains("echo B"), "stdin heredoc form");

        // JavaScript for Automation — a different verb entirely.
        let (req, _) = osa(
            &[
                "-l",
                "JavaScript",
                "-e",
                r#"Application("Terminal").doScript("echo C")"#,
            ],
            None,
        );
        assert!(req.contains("echo C"), "JXA doScript form");

        // iTerm2 writes into a session rather than "do script".
        let (req, _) = osa(
            &[
                "-e",
                r#"tell application "iTerm" to tell current session to write text "echo D""#,
            ],
            None,
        );
        assert!(req.contains("echo D"), "iTerm write text form");
    }

    /// The reported bug, reduced: a project's `run.sh` created its window with
    /// `do script ""` and that one call opened a REAL Terminal window while
    /// every other call in the same script was captured correctly.
    ///
    /// An empty command still opens a window — it is how a script asks for a
    /// bare shell — but the empty capture printed a blank line, which the
    /// shim's command substitution stripped, leaving it indistinguishable from
    /// "no terminal call in this script".
    #[test]
    fn an_empty_do_script_asks_for_a_shell_instead_of_escaping() {
        for src in [
            r#"tell application "Terminal" to do script """#,
            // Whitespace-only is the same request with a typo's worth of space.
            r#"tell application "Terminal" to do script "   ""#,
        ] {
            let (req, esc) = osa(&["-e", src], None);
            assert_eq!(req.trim(), SHELL_REQUEST, "{src}: must queue a plain shell");
            assert!(esc.is_empty(), "{src}: must NOT reach the real osascript");
        }

        // The exact heredoc shape run.sh uses, Cmd-T repeats and all.
        let (req, esc) = osa(
            &[],
            Some(
                "tell application \"Terminal\"\n  activate\n  do script \"\"\n  \
                 delay 0.4\n  set w to id of front window\nend tell\n\
                 repeat 5 times\n  tell application \"System Events\" to \
                 keystroke \"t\" using command down\n  delay 0.4\nend repeat\nreturn w\n",
            ),
        );
        assert_eq!(req.trim(), SHELL_REQUEST, "the scratch-window heredoc");
        assert!(
            esc.is_empty(),
            "no real window, and no Cmd-T keystrokes either"
        );
    }

    /// The sentinel the shims write has to be the one the drain recognises, or
    /// an empty request silently spawns nothing.
    #[test]
    fn the_shims_queue_the_sentinel_the_drain_understands() {
        for body in [OSASCRIPT_SHIM, OPEN_SHIM] {
            assert!(
                !shim_source(body).contains("@SHELL_REQUEST@"),
                "placeholder left unsubstituted"
            );
        }
        assert!(shim_source(OPEN_SHIM).contains(SHELL_REQUEST));
        assert!(shim_source(OSASCRIPT_SHIM).contains(SHELL_REQUEST));
    }

    /// A real command must still be forwarded verbatim — the empty-command
    /// handling above must not swallow ordinary scripts.
    #[test]
    fn a_non_empty_command_is_unaffected_by_the_empty_case() {
        let (req, esc) = osa(
            &[
                "-e",
                r#"tell application "Terminal" to do script "cd '/x/y' && npm run dev" in tab 1 of window id 42"#,
            ],
            None,
        );
        assert_eq!(req.trim(), "cd '/x/y' && npm run dev");
        assert!(esc.is_empty());
    }

    #[test]
    fn a_script_file_argument_is_read_rather_than_run() {
        let dir = tempfile::tempdir().unwrap();
        let scpt = dir.path().join("s.scpt");
        std::fs::write(
            &scpt,
            "tell application \"Terminal\" to do script \"echo E\"\n",
        )
        .unwrap();
        let (req, esc) = osa(&[scpt.to_str().unwrap()], None);
        assert!(
            req.contains("echo E"),
            "a .scpt file must be inspected, not executed"
        );
        assert!(esc.is_empty());
    }

    /// `open -a Terminal` never goes near osascript, so the osascript shim
    /// alone left this opening real windows.
    #[test]
    fn the_open_command_cannot_start_a_terminal_either() {
        let (req, esc) = open_(&["-a", "Terminal"]);
        assert!(
            req.contains(SHELL_REQUEST),
            "a bare terminal launch asks for a plain shell"
        );
        assert!(esc.is_empty());

        let (req, _) = open_(&["-a", "Terminal", "/tmp"]);
        assert!(req.contains("cd /tmp"), "a directory argument becomes a cd");

        let (req, _) = open_(&["-b", "com.apple.Terminal"]);
        assert!(req.contains(SHELL_REQUEST), "bundle id form");

        let (req, _) = open_(&["-a", "iTerm"]);
        assert!(req.contains(SHELL_REQUEST), "other terminal emulators too");
    }

    /// The app reads the queue from its own directory, so a relative path
    /// (`open -a Terminal .` in a script) must arrive absolute — it used to
    /// open the terminal in the app's folder instead of the script's.
    #[test]
    fn a_relative_folder_is_queued_as_an_absolute_path() {
        // `cargo test` runs from the crate root, where `src/` exists.
        let (req, _) = open_(&["-a", "Terminal", "src"]);
        let want = std::env::current_dir().unwrap().join("src");
        assert_eq!(req.trim(), format!("cd {}", want.display()), "got {req:?}");
    }

    #[test]
    fn a_double_clickable_script_counts_as_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = dir.path().join("go.command");
        std::fs::write(&cmd, "#!/bin/sh\necho hi\n").unwrap();
        let (req, _) = open_(&[cmd.to_str().unwrap()]);
        assert!(
            req.contains("go.command"),
            ".command files open a terminal window"
        );
    }

    /// Just as important: everything else must behave exactly as before, or the
    /// shims would break unrelated automation.
    #[test]
    fn non_terminal_calls_pass_straight_through() {
        let (req, esc) = osa(&["-e", r#"display dialog "hi""#], None);
        assert!(req.is_empty(), "a dialog is not a terminal");
        assert!(
            esc.contains("display dialog"),
            "and must still run for real"
        );

        let (req, esc) = open_(&["-a", "Simulator"]);
        assert!(req.is_empty(), "the iOS Simulator is not a terminal");
        assert!(esc.contains("Simulator"));

        let (req, esc) = open_(&["https://example.com"]);
        assert!(req.is_empty(), "a URL is not a terminal");
        assert!(esc.contains("example.com"));
    }

    /// The bug: the PATH prepend lived in `.zshrc` alone, which zsh reads only
    /// for **interactive** shells — so `zsh -lc` (how plenty of `run.sh`
    /// wrappers and npm scripts invoke their work) resolved `osascript` to
    /// `/usr/bin` and opened a real Terminal window. macOS's `path_helper`
    /// makes this worse than a plain omission: for a *login* shell it actively
    /// demotes our directory behind `/usr/bin` rather than dropping it, so the
    /// shim is on `PATH` and still loses.
    ///
    /// Every shell kind has to resolve to the shim, so this runs all four.
    #[test]
    fn zdotdir_keeps_shim_ahead_of_usr_bin_in_every_shell_kind() {
        // zsh-only mechanism.
        if !std::env::var("SHELL").unwrap_or_default().contains("zsh") {
            return;
        }
        let Some(req) = request_file() else {
            return;
        };
        let bridge_dir = req.parent().unwrap().to_path_buf();
        let zdotdir = bridge_dir.join("zdotdir");
        if !zdotdir.exists() || !Path::new("/bin/zsh").exists() {
            return;
        }

        // A controlled HOME whose .zshrc prepends dirs (like a real user setup),
        // to prove our shim still wins after macOS's path_helper + the user rc.
        let fake = tempfile::tempdir().unwrap();
        std::fs::write(
            fake.path().join(".zshrc"),
            "export PATH=\"/tmp/foo:/tmp/bar:$PATH\"\n",
        )
        .unwrap();

        for args in [
            vec!["-l", "-i", "-c"], // login + interactive: a normal Oxru terminal
            vec!["-l", "-c"],       // login, non-interactive: `zsh -lc ./run.sh`
            vec!["-i", "-c"],       // interactive, non-login
            vec!["-c"],             // the plainest subshell
        ] {
            let mut argv = args.clone();
            argv.push("command -v osascript");
            let out = Command::new("/bin/zsh")
                .args(&argv)
                .env_clear()
                .env("HOME", fake.path())
                .env("TERM", "xterm-256color")
                .env("ZDOTDIR", &zdotdir)
                .env("OXRU_REQUEST_FILE", &req)
                .env("OXRU_BRIDGE_DIR", &bridge_dir)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .output()
                .expect("run zsh");
            let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(
                resolved,
                bridge_dir.join("osascript").to_string_lossy(),
                "zsh {args:?}: osascript must resolve to the bridge shim, not /usr/bin"
            );
        }
    }

    /// bash gets the same guard through `BASH_ENV`, which it reads *after* the
    /// profile files have run `path_helper` — the zsh `ZDOTDIR` mechanism has
    /// no bash equivalent, so without this a bash user had no protection at
    /// all.
    #[test]
    fn bash_env_keeps_shim_ahead_of_usr_bin() {
        let Some(req) = request_file() else {
            return;
        };
        let bridge_dir = req.parent().unwrap().to_path_buf();
        let guard = bridge_dir.join("path.sh");
        if !guard.exists() || !Path::new("/bin/bash").exists() {
            return;
        }
        for args in [vec!["-c"], vec!["-l", "-c"]] {
            let mut argv = args.clone();
            argv.push("command -v osascript");
            let out = Command::new("/bin/bash")
                .args(&argv)
                .env("BASH_ENV", &guard)
                .env("OXRU_BRIDGE_DIR", &bridge_dir)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .output()
                .expect("run bash");
            let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(
                resolved,
                bridge_dir.join("osascript").to_string_lossy(),
                "bash {args:?}: osascript must resolve to the bridge shim"
            );
        }
    }

    /// Occupying `BASH_ENV` must not cost the user whatever they had there.
    #[test]
    fn the_guard_still_runs_the_users_own_bash_env() {
        let dir = tempfile::tempdir().unwrap();
        let guard = write_path_guard(dir.path()).expect("guard written");
        let theirs = dir.path().join("mine.sh");
        std::fs::write(&theirs, "export USER_BASH_ENV_RAN=yes\n").unwrap();

        let out = Command::new("/bin/bash")
            .args(["-c", "echo \"$USER_BASH_ENV_RAN $PATH\""])
            .env("BASH_ENV", &guard)
            .env("OXRU_REAL_BASH_ENV", &theirs)
            .env("OXRU_BRIDGE_DIR", dir.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run bash");
        let got = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(
            got,
            format!("yes {}:/usr/bin:/bin", dir.path().display()),
            "the user's BASH_ENV must run, and ours must still win on PATH"
        );
    }

    /// Launching Oxru from inside an Oxru terminal inherits the outer
    /// instance's guard through `BASH_ENV`; chaining to it would re-prepend a
    /// directory belonging to a different instance.
    #[test]
    fn a_nested_oxru_never_chains_to_another_instances_guard() {
        let dir = tempfile::tempdir().unwrap();
        let outer = write_path_guard(dir.path()).expect("guard written");
        assert!(
            is_oxru_path_guard(&outer),
            "our own guard must be detectable"
        );

        assert_eq!(
            resolve_real_bash_env(Some(outer.display().to_string()), is_oxru_path_guard),
            "",
            "an outer instance's guard is dropped, not chained to"
        );

        let theirs = dir.path().join("mine.sh");
        std::fs::write(&theirs, "export EDITOR=vim\n").unwrap();
        assert!(
            !is_oxru_path_guard(&theirs),
            "a user's own file is not ours"
        );
        assert_eq!(
            resolve_real_bash_env(Some(theirs.display().to_string()), is_oxru_path_guard),
            theirs.display().to_string(),
            "a real BASH_ENV is passed through"
        );
        assert_eq!(resolve_real_bash_env(None, is_oxru_path_guard), "");
    }

    /// The bridge used to live in `$TMPDIR`, where macOS's `dirhelper` sweep
    /// deleted the write-once shims out from under a long-running instance
    /// while the actively-appended `requests` file survived — leaving `PATH`
    /// pointing at an empty directory and every script opening a real window
    /// with nothing logged.
    #[test]
    fn the_bridge_does_not_live_in_the_temp_directory() {
        let Some(root) = bridge_root() else {
            return;
        };
        assert!(
            !root.starts_with(std::env::temp_dir()),
            "{} is inside $TMPDIR, which the OS prunes",
            root.display()
        );
        if let Some(req) = request_file() {
            assert!(
                req.starts_with(&root),
                "the live bridge must sit under {}",
                root.display()
            );
        }
    }

    /// Whatever removes them, the shims coming back is what keeps the feature
    /// from failing silently and permanently.
    #[test]
    fn vanished_shims_are_rewritten() {
        let _turn = TEST_FILES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(req) = request_file() else {
            return;
        };
        let dir = req.parent().unwrap().to_path_buf();
        let shim = dir.join("osascript");
        assert!(
            shim.exists(),
            "precondition: the shim is there to begin with"
        );

        // Exactly what the OS sweep did: delete the write-once files, keep the
        // directory and the request file.
        std::fs::remove_file(&shim).unwrap();
        std::fs::remove_file(dir.join("open")).unwrap();
        std::fs::remove_file(dir.join("zdotdir").join(".zshrc")).unwrap();
        // No `assert!(!shim.exists())` here: tests share one bridge and run in
        // parallel, so another test's `ensure_intact` may legitimately have put
        // it back already. What matters is the state *after* the call below.

        ensure_intact();

        assert!(shim.exists(), "the osascript shim must be restored");
        assert!(dir.join("open").exists(), "the open shim too");
        assert!(
            dir.join("zdotdir").join(".zshrc").exists(),
            "and the zsh wrapper"
        );
        // Restored, not just touched: it has to be a working shim.
        let text = std::fs::read_to_string(&shim).unwrap();
        assert!(
            text.contains("OXRU_REQUEST_FILE"),
            "restored file is the real shim"
        );
        assert!(!text.contains("@SHELL_REQUEST@"), "and fully substituted");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&shim).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "and executable");
        }
    }

    /// The guard must be a no-op once the directory is gone, so a shell that
    /// outlives its Oxru stops putting a reaped path back on `PATH`.
    #[test]
    fn the_path_guard_ignores_a_directory_that_no_longer_exists() {
        let dir = tempfile::tempdir().unwrap();
        let guard = write_path_guard(dir.path()).expect("guard written");
        let out = Command::new("/bin/sh")
            .args(["-c", &format!(". {} ; echo $PATH", guard.display())])
            .env("OXRU_BRIDGE_DIR", "/tmp/oxru-does-not-exist")
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "/usr/bin:/bin",
            "a vanished bridge dir must not be re-added"
        );
    }

    /// Sourcing the guard repeatedly must not keep growing `PATH`.
    #[test]
    fn the_path_guard_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let guard = write_path_guard(dir.path()).expect("guard written");
        let script = format!(". {g} ; . {g} ; . {g} ; echo $PATH", g = guard.display());
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("OXRU_BRIDGE_DIR", dir.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run sh");
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(
            path,
            format!("{}:/usr/bin:/bin", dir.path().display()),
            "the dir goes on once, however many times the guard runs"
        );
    }

    /// The reported bug: a command run in a terminal *outside* Oxru opened tabs
    /// *inside* it. macOS recycles PIDs, so `/tmp/oxru-<pid>` left behind by an
    /// instance that died without `cleanup()` got adopted by a later one —
    /// along with whatever was still queued in its `requests` file, and any
    /// orphaned shell still pointing `OXRU_REQUEST_FILE` at it.
    #[test]
    fn a_bridge_directory_is_never_reused_by_a_recycled_pid() {
        let pid = std::process::id();
        let a = bridge_dir_name(pid);
        let b = bridge_dir_name(pid);
        assert_ne!(a, b, "the same PID must not name the same directory twice");
        assert_eq!(
            dir_owner_pid(&a),
            Some(pid as i32),
            "the owner stays readable"
        );
    }

    /// Older builds named the directory `oxru-<pid>` with no suffix, and that
    /// litter is exactly what most needs reaping after an upgrade.
    #[test]
    fn the_old_directory_name_is_still_attributable() {
        assert_eq!(dir_owner_pid("oxru-26888"), Some(26888));
        assert_eq!(dir_owner_pid("oxru-26888-1f3a9b"), Some(26888));
        assert_eq!(dir_owner_pid("oxru-zdotdir"), None, "not a pid");
        assert_eq!(dir_owner_pid("something-else"), None);
    }

    /// Dead instances' directories have to go, or `/tmp` accumulates working
    /// `osascript` shims pointed at request files nobody drains.
    #[test]
    fn stale_bridge_directories_are_reaped_but_live_ones_survive() {
        // `reap_stale_dirs` scans the real temp dir, so drive it there — using
        // a PID that is essentially never alive, as `instances` does.
        let tmp = std::env::temp_dir();
        let dead = tmp.join("oxru-2147483646-deadbeef");
        std::fs::create_dir_all(&dead).unwrap();
        std::fs::write(dead.join("requests"), b"echo SHOULD_NEVER_RUN\n").unwrap();

        // Ours must survive: it is the bridge this process is using.
        let ours = request_file().map(|r| r.parent().unwrap().to_path_buf());

        let mut roots = vec![tmp.clone()];
        if let Some(o) = ours.as_ref().and_then(|o| o.parent()) {
            roots.push(o.to_path_buf());
        }
        reap_stale_dirs(&roots);

        assert!(
            !dead.exists(),
            "a dead instance's directory must be removed"
        );
        if let Some(ours) = ours {
            assert!(ours.exists(), "our own live bridge must not be reaped");
        }
    }

    #[test]
    fn shim_routes_terminal_do_script_to_request_file() {
        let Some(req) = request_file() else {
            return; // non-unix: no bridge
        };
        let shim = req.parent().unwrap().join("osascript");
        assert!(shim.exists(), "shim osascript should exist");

        // Point the shim at a throwaway request file rather than the real one:
        // this process is not an editor, so anything queued here would be read
        // by whichever Oxru next adopted the directory. That adoption is fixed
        // (see `a_bridge_directory_is_never_reused_by_a_recycled_pid`), but a
        // test has no business leaving a live command in a real queue.
        let scratch = tempfile::tempdir().unwrap();
        let req_out = scratch.path().join("requests");
        let out = Command::new(&shim)
            .arg("-e")
            .arg(r#"tell application "Terminal" to do script "echo HELLO_OXRU_TEST""#)
            .env("OXRU_REQUEST_FILE", &req_out)
            .output()
            .expect("run shim");
        assert!(out.status.success(), "shim should exit 0");

        let contents = std::fs::read_to_string(&req_out).unwrap_or_default();
        assert!(
            contents.contains("echo HELLO_OXRU_TEST"),
            "shim should queue the command, got: {contents:?}"
        );
    }
}
