//! Updates as a window sees them. Now and then it checks (on a thread),
//! says once when an update is ready, and handles the About dialog's line,
//! Check for Updates, and Restart to Update. The work itself is in
//! `crate::update`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;

use super::{App, Ask};
use crate::keymap::Command;
use crate::update::{self, Found, Install, Waiting};

/// Where this window's update stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Update {
    #[default]
    Unknown,
    Checking,
    UpToDate,
    Downloading(String),
    /// Downloaded and verified: it installs when the last window closes.
    Ready(String),
    /// Installed (by another window): restart to use it.
    Installed(String),
    /// Out, but this copy updates by hand (built from source, the .zip).
    ByHand(String),
    Failed(String),
}

#[derive(Default)]
pub(crate) struct Updates {
    /// Written by the checking thread too.
    pub(crate) state: Arc<Mutex<Update>>,
    /// When this window last looked at what the others found.
    looked: Option<Instant>,
    /// Asked for (Check for Updates): say how it went, whatever it was.
    asked: bool,
    /// What was last said (each is said once).
    said: Update,
    /// How this copy updates (asked once).
    install: Option<Install>,
}

/// How often a window looks at what the others found (some files).
const LOOK_EVERY: Duration = Duration::from_secs(600);

impl App {
    pub(crate) fn update_state(&self) -> Update {
        self.updates.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set_update(&self, u: Update) {
        if let Ok(mut s) = self.updates.state.lock() {
            *s = u;
        }
    }

    pub(crate) fn install(&mut self) -> Install {
        self.updates.install.get_or_insert_with(update::this_install).clone()
    }

    /// Per frame. Now and then, look at what the other windows found, and
    /// check when one is due (with the `updates` setting on). Then say
    /// what changed.
    pub(crate) fn poll_updates(&mut self, ctx: &egui::Context) {
        if !self.updates.looked.is_some_and(|t| t.elapsed() < LOOK_EVERY) {
            self.updates.looked = Some(Instant::now());
            let busy = matches!(self.update_state(), Update::Checking | Update::Downloading(_));
            if let Some(dir) = update::dir().filter(|_| !busy) {
                match update::waiting(&dir, crate::VERSION) {
                    Some(Waiting::Ready { version, .. }) => self.set_update(Update::Ready(version)),
                    Some(Waiting::Installed(v)) => self.set_update(Update::Installed(v)),
                    None => {}
                }
                // Tests never ask GitHub.
                if self.config.settings.updates && !cfg!(test) && update::due(&dir) {
                    self.check_for_updates(false, ctx);
                }
            }
        }
        let now = self.update_state();
        if now == self.updates.said {
            return;
        }
        self.updates.said = now.clone();
        let asked = std::mem::take(&mut self.updates.asked);
        let key = self
            .config
            .keymap
            .combo_for(Command::CheckForUpdates)
            .map(|k| format!(" — {}", k.label()))
            .unwrap_or_default();
        match now {
            Update::Checking | Update::Downloading(_) => self.updates.asked = asked,
            Update::Ready(v) if self.install() == Install::LinuxDeb => {
                self.notify(format!("Roxaruu {v} is downloaded{key} to install it"));
            }
            Update::Ready(v) => {
                self.notify(format!("Roxaruu {v} is ready: it installs when you close the last window{key} to restart now"));
            }
            Update::Installed(v) => self.notify(format!("Roxaruu {v} is installed: restart to use it{key}")),
            Update::ByHand(_) if asked => self.ask_about_update(),
            Update::ByHand(v) => self.notify(format!("Roxaruu {v} is out{key} to get it")),
            Update::UpToDate if asked => self.notify(format!("Roxaruu {} is up to date", crate::VERSION)),
            Update::Failed(e) if asked => self.warn(format!("Couldn't check for updates: {e}")),
            Update::Unknown | Update::UpToDate | Update::Failed(_) => {}
        }
    }

    /// Ask GitHub on a thread. `asked`: the user did, so say how it went.
    pub(crate) fn check_for_updates(&mut self, asked: bool, ctx: &egui::Context) {
        let Some(dir) = update::dir() else {
            return self.warn("Couldn't check for updates: no folder to download them into");
        };
        self.updates.asked |= asked;
        // Tests never ask GitHub (`update.rs` tests the checking itself).
        if cfg!(test) {
            return self.set_update(Update::Failed("tests don't ask GitHub".into()));
        }
        self.set_update(Update::Checking);
        let (state, install, ctx) = (self.updates.state.clone(), self.install(), ctx.clone());
        let set = move |u: Update| {
            if let Ok(mut s) = state.lock() {
                *s = u;
            }
            ctx.request_repaint();
        };
        let spawned = std::thread::Builder::new().name("update".into()).spawn(move || {
            let found = update::check(&update::Source::GitHub, &dir, (&install, std::env::consts::ARCH), crate::VERSION, &|v| {
                set(Update::Downloading(v.to_string()))
            });
            set(match found {
                Ok(Found::UpToDate(_)) => Update::UpToDate,
                Ok(Found::Ready(v)) => match update::waiting(&dir, crate::VERSION) {
                    Some(Waiting::Installed(v)) => Update::Installed(v),
                    _ => Update::Ready(v),
                },
                Ok(Found::ByHand(v)) => Update::ByHand(v),
                Ok(Found::Busy) => Update::Failed("another Roxaruu window is checking right now".into()),
                Err(e) => {
                    tracing::warn!(error = %e, "couldn't check for updates");
                    Update::Failed(e)
                }
            });
        });
        if let Err(e) = spawned {
            self.set_update(Update::Failed(e.to_string()));
        }
    }

    /// Check for Updates: what's waiting, or check now.
    pub(crate) fn run_check_for_updates(&mut self, ctx: &egui::Context) {
        match self.update_state() {
            Update::Ready(_) | Update::Installed(_) | Update::ByHand(_) => self.ask_about_update(),
            Update::Checking => self.notify("Checking for updates…"),
            Update::Downloading(v) => self.notify(format!("Downloading Roxaruu {v}…")),
            Update::Unknown | Update::UpToDate | Update::Failed(_) => {
                self.notify("Checking for updates…");
                self.check_for_updates(true, ctx);
            }
        }
    }

    fn ask_about_update(&mut self) {
        // Asked once, as it opens: counting windows looks at processes.
        let others = crate::instances::other_windows();
        self.ask = Some(Ask::Update { others });
    }

    /// The update dialog: its title, its text, and its button (none: it
    /// only says how it goes).
    pub(crate) fn update_dialog(&mut self, others: usize) -> (String, String, Option<&'static str>) {
        let losses = self.quit_losses().join("\n\n");
        let restart = |text: String| if losses.is_empty() { text } else { format!("{text}\n\n{losses}") };
        let install = self.install();
        match self.update_state() {
            Update::Ready(v) if install == Install::LinuxDeb => (
                format!("Roxaruu {v} is downloaded"),
                "Installing it needs your password, so it runs in a terminal here (apt). Then restart Roxaruu to use it."
                    .into(),
                Some("Install in Terminal"),
            ),
            // From a browser, a restart would end remote access.
            Update::Ready(v) if self.remote_pass => {
                (format!("Roxaruu {v} is ready"), "It installs when the last Roxaruu window closes.".into(), None)
            }
            // Windows' installer closes every open copy: it waits for the last.
            Update::Ready(v) if install == Install::WindowsSetup && others > 0 => (
                format!("Roxaruu {v} is ready"),
                format!(
                    "It installs when you close the last Roxaruu window ({others} other{} open).",
                    if others == 1 { " is" } else { "s are" }
                ),
                None,
            ),
            Update::Ready(v) => (
                format!("Roxaruu {v} is ready"),
                restart(
                    "It's downloaded and checked. It installs when you close the last Roxaruu window, or restart \
                     this one now to use it right away."
                        .into(),
                ),
                Some("Restart Now"),
            ),
            Update::Installed(v) if self.remote_pass => {
                (format!("Roxaruu {v} is installed"), "Open windows use it once they restart.".into(), None)
            }
            Update::Installed(v) => (
                format!("Roxaruu {v} is installed"),
                restart("Restart this window to use it.".into()),
                Some("Restart Now"),
            ),
            Update::ByHand(v) => (
                format!("Roxaruu {v} is out"),
                "This copy was built from source (or unzipped), so it updates by hand: the release page has \
                 every download."
                    .into(),
                Some("Open Release Page"),
            ),
            _ => (format!("Roxaruu {}", crate::VERSION), "It's up to date.".into(), None),
        }
    }

    /// The update dialog's button.
    pub(crate) fn answer_update(&mut self, ctx: &egui::Context) {
        let install = self.install();
        match self.update_state() {
            Update::Ready(_) if install == Install::LinuxDeb => {
                let Some(Waiting::Ready { file, .. }) = update::dir().and_then(|d| update::waiting(&d, crate::VERSION))
                else {
                    return self.warn("The download is gone: check for updates again");
                };
                self.new_terminal(ctx);
                if let Some(t) = self.terminal_mut() {
                    t.pane.send_input(format!("{}\r", update::apt_command(&file)).as_bytes());
                }
            }
            Update::Ready(_) | Update::Installed(_) => {
                self.begin_close(self.tab_paths(), super::After::Restart, ctx);
            }
            Update::ByHand(_) => self.open_url(update::RELEASES.into(), ctx),
            _ => {}
        }
    }
}
