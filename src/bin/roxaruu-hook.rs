//! `roxaruu-hook`: Roxaruu's helper for Claude Code, the same program on
//! macOS, Windows and Linux (no shell scripts).
//!
//! - Run as `claude` — Roxaruu's terminals find it first on `PATH` (a link
//!   to this program, see `termbridge`) — it starts the real Claude Code,
//!   the next `claude` on `PATH`, with two hooks added for that run
//!   (`--settings`; the user's own settings are never touched): `Stop` (it
//!   finished) and `Notification` (it wants you: a permission, a question).
//! - Run as `roxaruu-hook stop` / `roxaruu-hook notification` — what those
//!   hooks run — it puts the message in the terminal's inbox
//!   (`$ROXARUU_ALERTS/$ROXARUU_TERM`), which Roxaruu's watcher raises as a
//!   toast, a bell on the tab, and a notification if you're away.
//!
//! A console program on purpose: Roxaruu itself is a windowed app on
//! Windows, which a shell doesn't wait for.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    let invoked = args
        .first()
        .and_then(|a| Path::new(a).file_stem())
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if invoked == "claude" {
        std::process::exit(run_claude(args.get(1..).unwrap_or_default()));
    }
    match args.get(1).and_then(|a| a.to_str()) {
        Some(event @ ("stop" | "notification")) => {
            let mut input = String::new();
            let _ = std::io::stdin().take(1 << 20).read_to_string(&mut input);
            tell(event, &input);
        }
        _ => {
            eprintln!("roxaruu-hook: Roxaruu's helper for Claude Code (run by its hooks: stop | notification)");
            std::process::exit(2);
        }
    }
}

/// The real Claude Code: the first `claude` on `PATH` that isn't this
/// program (Windows' `.exe` and npm's `.cmd` included).
fn real_claude() -> Option<PathBuf> {
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    which::which_all("claude")
        .ok()?
        .find(|p| p.canonicalize().ok() != me)
}

/// This program by its own name, beside the `claude` link to it.
fn helper() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let h = me.with_file_name(format!("roxaruu-hook{}", std::env::consts::EXE_SUFFIX));
    h.exists().then_some(h)
}

/// The settings that add the two hooks, each running `helper <event>`.
fn hooks(helper: &Path) -> String {
    // Forward slashes: every shell Claude Code runs hooks with (bash, and Git
    // Bash on Windows) reads them as a path, backslashes not always.
    let exe = helper.to_string_lossy().replace('\\', "/");
    let run = |event: &str| {
        serde_json::json!([{ "hooks": [{ "type": "command", "command": format!("\"{exe}\" {event}") }] }])
    };
    serde_json::json!({ "hooks": { "Stop": run("stop"), "Notification": run("notification") } }).to_string()
}

/// `claude …`: the real one, with the hooks in a Roxaruu terminal.
fn run_claude(args: &[OsString]) -> i32 {
    let Some(real) = real_claude() else {
        eprintln!("claude: command not found");
        return 127;
    };
    let mut cmd = Command::new(&real);
    if std::env::var_os("ROXARUU_TERM").is_some()
        && let Some(h) = helper()
    {
        cmd.arg("--settings").arg(hooks(&h));
    }
    cmd.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec(); // only returns if it couldn't start
        eprintln!("claude: {err}");
        126
    }
    #[cfg(not(unix))]
    {
        // Ctrl+C is Claude Code's (it stops what it's doing): this waiting
        // parent mustn't die of it and hand the prompt back under it.
        ignore_ctrl_c();
        match cmd.status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => {
                eprintln!("claude: {e}");
                126
            }
        }
    }
}

/// Safety: `SetConsoleCtrlHandler(None, TRUE)` only sets a flag for this
/// process: Ctrl+C is ignored here (the child still gets it).
#[cfg(windows)]
#[allow(unsafe_code)]
fn ignore_ctrl_c() {
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 1);
    }
}

#[cfg(not(any(unix, windows)))]
fn ignore_ctrl_c() {}

/// What a hook says, for `event` and the JSON Claude Code hands it.
fn message(event: &str, input: &str) -> String {
    let said = serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string));
    match (event, said) {
        ("stop", _) => "Claude Code: finished".to_string(),
        (_, Some(m)) if !m.trim().is_empty() => format!("Claude Code: {m}"),
        _ => "Claude Code: needs your attention".to_string(),
    }
}

/// Put the message in this terminal's inbox. Never fails Claude Code: a
/// hook that can't tell just says nothing.
fn tell(event: &str, input: &str) {
    let (Some(dir), Some(term)) = (std::env::var_os("ROXARUU_ALERTS"), std::env::var_os("ROXARUU_TERM")) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(term)) {
        let _ = writeln!(f, "{}", message(event, input).replace('\n', " "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_run_this_program_for_both_events() {
        let json: serde_json::Value = serde_json::from_str(&hooks(Path::new(r"C:\Users\a b\roxaruu-hook.exe"))).unwrap();
        assert_eq!(json["hooks"]["Stop"][0]["hooks"][0]["command"], "\"C:/Users/a b/roxaruu-hook.exe\" stop");
        assert_eq!(json["hooks"]["Notification"][0]["hooks"][0]["command"], "\"C:/Users/a b/roxaruu-hook.exe\" notification");
        assert_eq!(json["hooks"]["Stop"][0]["hooks"][0]["type"], "command");
    }

    #[test]
    fn messages_say_what_happened() {
        assert_eq!(message("stop", "{}"), "Claude Code: finished");
        assert_eq!(
            message("notification", r#"{"message":"Claude needs your permission to use Bash"}"#),
            "Claude Code: Claude needs your permission to use Bash"
        );
        assert_eq!(message("notification", "not json"), "Claude Code: needs your attention");
    }
}
