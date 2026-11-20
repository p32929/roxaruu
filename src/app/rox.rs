//! The `rox` command (and `roxaruu`) for terminals: `rox` opens the folder
//! you're in, `rox <path>` another. The installers put it on the PATH; a
//! copy dragged from the disk image or unpacked by hand has it put there
//! by Install the rox Terminal Command.

use std::path::{Path, PathBuf};

use super::App;

/// Where an installed `rox` would be.
fn places(home: &Path, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    if cfg!(windows) {
        // Beside the program, as the installer puts it.
        return exe_dir.map(|d| vec![d.join("rox.cmd")]).unwrap_or_default();
    }
    // Tests look only in their own scratch home.
    let shared: &[&str] = if cfg!(test) { &[] } else { &["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"] };
    shared
        .iter()
        .map(PathBuf::from)
        .chain([".local/bin", "bin", ".cargo/bin"].map(|d| home.join(d)))
        .map(|d| d.join("rox"))
        .collect()
}

/// Whether `rox` is installed (a cheap look, at start-up).
pub fn installed() -> bool {
    let exe = std::env::current_exe().ok();
    let home = crate::config::home().unwrap_or_default();
    places(&home, exe.as_deref().and_then(Path::parent)).iter().any(|p| p.exists())
}

/// Write `rox` (and `roxaruu`) into `dir`, running `exe`. Windows: `rox.cmd`,
/// which runs the `roxaruu.exe` beside it, so `dir` is the program's own.
fn install_in(dir: &Path, exe: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if cfg!(windows) {
        return std::fs::write(dir.join("rox.cmd"), include_str!("../../packaging/rox.cmd"));
    }
    // A script, not a link: `rox` keeps working when the app is replaced.
    let rox = dir.join("rox");
    std::fs::write(&rox, format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", exe.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&rox, std::fs::Permissions::from_mode(0o755))?;
        let long = dir.join("roxaruu");
        let _ = std::fs::remove_file(&long);
        std::os::unix::fs::symlink(exe, &long)?;
    }
    Ok(())
}

/// Where to put it: a folder every terminal already looks in if this user
/// may write there, else `~/.local/bin`.
fn target(home: &Path, exe_dir: Option<&Path>) -> Option<PathBuf> {
    if cfg!(windows) {
        return exe_dir.map(Path::to_path_buf);
    }
    if cfg!(test) {
        return Some(home.join(".local/bin")); // never the computer's own
    }
    let shared = Path::new("/usr/local/bin");
    let probe = shared.join(".roxaruu-can-write");
    let writable = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    Some(if writable { shared.to_path_buf() } else { home.join(".local/bin") })
}

/// The PATH a new terminal gets: the login shell's (an app opened from the
/// Dock has a shorter one). Windows: this process's.
fn terminal_path() -> String {
    if !cfg!(windows)
        && let Ok(shell) = std::env::var("SHELL")
        && let Ok(out) = std::process::Command::new(shell)
            .args(["-l", "-c", "printf %s \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .output()
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout).into_owned();
    }
    std::env::var("PATH").unwrap_or_default()
}

impl App {
    /// Install the rox Terminal Command: `rox` where terminals find it, saying where,
    /// and what to add to the PATH if that folder isn't on it.
    pub(crate) fn install_rox(&mut self) {
        let (Ok(exe), Some(home)) = (std::env::current_exe(), crate::config::home()) else {
            return self.warn("Couldn't find where Roxaruu is, to point rox at it");
        };
        let Some(dir) = target(&home, exe.parent()) else { return };
        if let Err(e) = install_in(&dir, &exe) {
            tracing::warn!(error = %e, dir = %dir.display(), "install rox");
            return self.warn(format!("Couldn't install rox in {}: {e}", dir.display()));
        }
        tracing::info!(dir = %dir.display(), "rox installed");
        self.rox_installed = true;
        // Its row on the start card goes: the highlight mustn't land on Quit.
        self.welcome_selected = 0;
        let shown = crate::ui::tilde(&dir.to_string_lossy());
        let on_path = std::env::split_paths(&terminal_path()).any(|p| p == dir);
        if on_path {
            self.notify(format!("Installed rox in {shown}: in a terminal, rox opens the folder you're in"));
        } else if cfg!(windows) {
            self.notify(format!("Installed rox in {shown}: add that folder to your PATH to use it"));
        } else {
            self.notify(format!(
                "Installed rox in {shown}: add it to your PATH (export PATH=\"{shown}:$PATH\" in your shell's profile), then open a new terminal"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rox` runs the program with whatever it's given; found once there.
    #[cfg(unix)]
    #[test]
    fn rox_runs_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(".local/bin");
        assert!(!places(dir.path(), None).iter().any(|p| p.starts_with(dir.path()) && p.exists()));
        install_in(&bin, Path::new("/bin/echo")).unwrap();
        let out = std::process::Command::new(bin.join("rox")).args(["a b", "c"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a b c\n");
        assert_eq!(std::fs::read_link(bin.join("roxaruu")).unwrap(), Path::new("/bin/echo"));
        assert!(places(dir.path(), None).iter().any(|p| p.starts_with(dir.path()) && p.exists()), "found");
        // Again: replaced, not an error.
        install_in(&bin, Path::new("/bin/echo")).unwrap();
    }
}
