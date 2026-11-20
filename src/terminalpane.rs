//! One embedded terminal: a PTY running the user's shell, with its output fed
//! through a `vt100` emulator so it can be rendered as a cell grid. A background
//! thread reads the PTY into a channel; the UI thread drains it in [`pump`].

// Ported whole from Oxru with its tests; link clicking (Step 8) and the
// finished-while-away tab marker (Step 5) are wired up later.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};

/// How often to re-check the foreground process for the tab label, for a
/// terminal you're looking at.
const PROC_POLL: Duration = Duration::from_millis(400);

/// The same for one out of sight (another tab, or the window in the
/// background): it still notices a command ending (the tab's "finished"
/// mark, agent alerts), just less often. Asking the system costs: on
/// Windows it lists every process there is.
const PROC_POLL_UNSEEN: Duration = Duration::from_secs(2);

/// Floor between working-directory reads for one pane. Slower than
/// [`PROC_POLL`] because each read forks `lsof`, and it only fires at all after
/// the pane has printed something (see `TerminalPane::cwd_dirty`), so a quiet
/// terminal never pays for it.
const CWD_POLL: Duration = Duration::from_millis(600);

/// Rows the emulator itself keeps above the screen: 32 bytes a cell, the full
/// width, however short the line (a busy 200-column terminal's 3,000 rows
/// were ~19 MB). Just a short live stretch; older rows go to the compact
/// history (`Alerts::history`).
const LIVE_HISTORY: usize = 200;

/// Lines of history a terminal keeps in all: the live stretch, then the
/// compact rest (~100–200 bytes a line, deflated: well under a MB here).
/// Scrolling back that far draws them again (`replay`).
const HISTORY: usize = 10_000;

/// Called from a terminal's reader thread whenever new output arrives, to wake
/// the GUI event loop so it redraws promptly (winit throttles its idle timer on
/// macOS, which otherwise leaves a running CLI looking "stuck" between inputs).
/// `true` (from the watcher) means wake even if this terminal is hidden:
/// it asked for the user.
pub type Waker = std::sync::Arc<dyn Fn(bool) + Send + Sync>;

/// A burst of output is one with gaps shorter than this.
const BURST_GAP: Duration = Duration::from_millis(1500);
/// An agent that printed for this long, then stopped for this long, has
/// finished or is waiting for an answer.
const AGENT_QUIET: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 3 });

/// What programs in the terminal asked to tell the user: the bell, and the
/// notification escapes — OSC 9 (iTerm2), OSC 777 (Ghostty, rxvt) and
/// OSC 99 (kitty) — which is what Claude Code and friends send.
pub struct Alerts {
    said: Vec<String>,
    /// The terminal's history past the emulator's live stretch, compact.
    history: crate::history::History,
}

impl Default for Alerts {
    fn default() -> Self {
        Self { said: Vec::new(), history: crate::history::History::new(HISTORY - LIVE_HISTORY) }
    }
}

impl vt100::Callbacks for Alerts {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.said.push(String::new());
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let Some(message) = osc_notification(params) {
            self.said.push(message);
        }
    }
}

/// Output into the emulator; the rows its live stretch let go of into the
/// compact history (each with its width first: it's drawn that wide again).
fn feed(p: &mut vt100::Parser<Alerts>, bytes: &[u8]) {
    p.process(bytes);
    for (width, line) in p.screen_mut().take_dropped_formatted() {
        let mut stored = width.to_le_bytes().to_vec();
        stored.extend(line);
        p.callbacks_mut().history.push(stored);
    }
}

/// The message of a notification OSC, `None` for any other OSC.
fn osc_notification(params: &[&[u8]]) -> Option<String> {
    let text = |p: &[&[u8]]| {
        p.iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<Vec<_>>()
            .join(";")
    };
    match params {
        // `9;4;…` is iTerm2's progress bar, not a notification.
        [b"9", b"4", ..] => None,
        [b"9", rest @ ..] => Some(text(rest)),
        [b"777", b"notify", title, body @ ..] => {
            let (title, body) = (text(&[title]), text(body));
            Some(match (title.is_empty(), body.is_empty()) {
                (false, false) => format!("{title}: {body}"),
                (true, _) => body,
                (_, true) => title,
            })
        }
        // kitty sends the title and the body as separate pieces; the body says it.
        [b"99", meta, body @ ..] if meta.windows(6).any(|w| w == b"p=body") => Some(text(body)),
        _ => None,
    }
}

/// What a running program is doing, told by its output: still printing,
/// running (printed a moment ago), or quiet this long — maybe waiting, maybe
/// stuck.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pulse {
    Printing,
    Running,
    Quiet(Duration),
}

/// The `terminalShell` setting, as the app last read it (`set_shell`).
static SHELL: Mutex<String> = Mutex::new(String::new());

/// What new terminals run (the `terminalShell` setting; the app keeps it current).
pub fn set_shell(choice: &str) {
    let mut s = SHELL.lock().unwrap_or_else(|e| e.into_inner());
    if *s != choice {
        *s = choice.to_string();
    }
}

/// Output this recent: still printing.
const PRINTING_WITHIN: Duration = Duration::from_millis(1500);
/// Nothing printed for this long while running: quiet. (Tests don't sit
/// through ten real seconds of silence.)
pub const QUIET_AFTER: Duration = Duration::from_secs(if cfg!(test) { 3 } else { 10 });

impl Pulse {
    /// For a terminal running a program or not, its output last `since` ago.
    fn of(running: bool, since: Option<Duration>) -> Option<Pulse> {
        if !running {
            return None;
        }
        Some(match since {
            Some(d) if d < PRINTING_WITHIN => Pulse::Printing,
            Some(d) if d >= QUIET_AFTER => Pulse::Quiet(d),
            _ => Pulse::Running,
        })
    }

    /// For the tab's tooltip and the status bar.
    pub fn label(self) -> String {
        match self {
            Pulse::Printing => "printing".into(),
            Pulse::Running => "running".into(),
            Pulse::Quiet(d) => {
                let s = d.as_secs();
                let ago = match s {
                    0..60 => format!("{s}s"),
                    60..3600 => format!("{}m", s / 60),
                    _ => format!("{}h {}m", s / 3600, s / 60 % 60),
                };
                format!("quiet for {ago}")
            }
        }
    }
}

/// When the current burst of output started and when output last came.
#[derive(Default)]
struct Activity {
    burst: Option<Instant>,
    last: Option<Instant>,
}

impl Activity {
    /// Whether a burst at least [`AGENT_QUIET`] long has just been followed
    /// by [`AGENT_QUIET`] of silence: for an agent, done or waiting. Reports
    /// each burst once.
    fn went_quiet(&mut self) -> bool {
        let (Some(start), Some(last)) = (self.burst, self.last) else {
            return false;
        };
        if last.elapsed() < AGENT_QUIET {
            return false;
        }
        self.burst = None;
        last - start >= AGENT_QUIET
    }
}

/// Programs whose falling silent after a stretch of output means "finished,
/// or waiting for you".
const AGENTS: &[&str] = &[
    "claude",
    "codex",
    "gemini",
    "opencode",
    "aider",
    "amp",
    "goose",
    "cursor-agent",
    "crush",
    "qwen",
    "droid",
    "copilot",
];

/// Whether the app window has the keyboard; set by the UI every frame. While
/// it doesn't, alerts also go to the OS as notifications.
pub static APP_FOCUSED: AtomicBool = AtomicBool::new(true);

/// The `notifications` setting, for the watchers (the window sets it).
pub static NOTIFY: AtomicBool = AtomicBool::new(true);

/// A terminal's alert state, shared with its watcher thread. The watcher
/// runs off the UI loop on purpose: macOS stops giving a hidden window
/// frames, and that's exactly when the user needs telling.
#[derive(Default)]
pub struct Watch {
    /// The foreground program is a coding agent.
    agent: AtomicBool,
    /// The user can see this terminal right now (set by the UI).
    pub looking: AtomicBool,
    /// "folder · program", for the notification's title.
    name: Mutex<String>,
    /// Alerts the UI hasn't shown yet.
    fired: Mutex<Vec<String>>,
    /// How the last notification went (the UI takes it with the alert).
    notified: Mutex<Option<crate::sys::Delivery>>,
    /// This terminal's inbox for what Claude Code's hooks say
    /// (`roxaruu-hook`, through `ROXARUU_TERM`).
    inbox: Mutex<Option<PathBuf>>,
}

/// Twice a second: anything asked for the user here? An agent gone quiet
/// counts unless it just said so itself (Claude Code's OSC 9 comes ~3s
/// before the silence does).
fn watch(
    parser: std::sync::Weak<Mutex<vt100::Parser<Alerts>>>,
    activity: std::sync::Weak<Mutex<Activity>>,
    w: std::sync::Weak<Watch>,
    waker: Option<Waker>,
) {
    // When something was last raised, and when Claude Code's hooks last
    // said something themselves.
    let (mut last, mut spoke): (Option<Instant>, Option<Instant>) = (None, None);
    loop {
        thread::sleep(Duration::from_millis(500));
        let (Some(parser), Some(activity), Some(w)) =
            (parser.upgrade(), activity.upgrade(), w.upgrade())
        else {
            return; // the terminal closed
        };
        let mut said = std::mem::take(
            &mut parser
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .callbacks_mut()
                .said,
        );
        let quiet = activity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .went_quiet();
        // What Claude Code's hooks left in this terminal's inbox: exactly
        // when it finished or wants you (taken whole: renamed, then read).
        let inbox = w.inbox.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(path) = inbox {
            let taken = path.with_extension("taken");
            if std::fs::rename(&path, &taken).is_ok() {
                let text = std::fs::read_to_string(&taken).unwrap_or_default();
                let _ = std::fs::remove_file(&taken);
                let lines: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
                // The hooks said it: "went quiet" would only repeat it. (A
                // bell or a printed notice earlier doesn't: the agent may
                // work on after it, then finish.)
                if !lines.is_empty() {
                    spoke = Some(Instant::now());
                }
                said.extend(lines);
            }
        }
        let recent = spoke.is_some_and(|t| t.elapsed() < Duration::from_secs(5));
        if said.is_empty() && quiet && !recent && w.agent.load(Ordering::Relaxed) {
            said.push("finished, or waiting for you".into());
        }
        // Gone to another app or Space: the window's last word on focus (and
        // so on what you're looking at) can be stale — macOS stops drawing a
        // window it can't show, so it never hears you left.
        let away = !APP_FOCUSED.load(Ordering::Relaxed) || crate::sys::app_active() == Some(false);
        // Seen as it happened; and a ringing bell is one alert, not twenty.
        if said.is_empty()
            || (w.looking.load(Ordering::Relaxed) && !away)
            || last.is_some_and(|t| t.elapsed() < Duration::from_millis(1500))
        {
            continue;
        }
        last = Some(Instant::now());
        let message = said
            .into_iter()
            .rfind(|m| !m.is_empty())
            .unwrap_or_else(|| "wants your attention".into());
        // Away: a notification (or why not), for the tab to say on hover.
        if away {
            let delivery = if NOTIFY.load(Ordering::Relaxed) {
                let name = w.name.lock().unwrap_or_else(|e| e.into_inner()).clone();
                crate::sys::notify(&format!("Roxaruu · {name}"), &message)
            } else {
                crate::sys::Delivery {
                    sent: false,
                    trouble: false,
                    note: "Not sent: notifications are off in Roxaruu's Settings.".into(),
                }
            };
            *w.notified.lock().unwrap_or_else(|e| e.into_inner()) = Some(delivery);
        }
        w.fired
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(message);
        if let Some(waker) = &waker {
            waker(true);
        }
    }
}

/// A read lock on a terminal's emulator that dereferences to its
/// [`vt100::Screen`], so callers keep writing `term.screen().cell(..)` even
/// though the parser now lives behind a mutex (written by the reader thread).
/// What a terminal shows: the emulator's screen, or (scrolled back past its
/// live stretch) those rows drawn again from the compact history.
pub enum ScreenGuard<'a> {
    Live(std::sync::MutexGuard<'a, vt100::Parser<Alerts>>),
    Replay(&'a vt100::Parser),
}

impl std::ops::Deref for ScreenGuard<'_> {
    type Target = vt100::Screen;
    fn deref(&self) -> &vt100::Screen {
        match self {
            ScreenGuard::Live(p) => p.screen(),
            ScreenGuard::Replay(p) => p.screen(),
        }
    }
}

pub struct TerminalPane {
    /// The base label (project folder), e.g. `server_nestjs`.
    pub folder: String,
    /// The folder it's in, as last read (see `refresh_folder`): Recent
    /// Terminals keeps it.
    pub dir: PathBuf,
    /// The live foreground process name, e.g. `node` (empty when only the
    /// shell is running).
    proc: String,
    /// Basename of the user's shell, so we can tell "just a shell" from a
    /// running command.
    shell_name: String,
    last_proc_check: Instant,
    /// On screen in the window you're at: checked at `PROC_POLL`, else
    /// `PROC_POLL_UNSEEN` (set each frame, `App::poll_attention`).
    pub in_sight: bool,
    /// Set whenever the pane produces output, cleared once the working
    /// directory has been re-read. A shell that printed something is a shell
    /// that may have just `cd`'d; one sitting silent cannot have moved, so it
    /// costs nothing to leave alone — which matters because reading the cwd
    /// forks `lsof` (see [`process_cwd`]).
    cwd_dirty: bool,
    last_cwd_check: Instant,
    /// The vt100 emulator, written by the **reader thread** (so PTY output is
    /// consumed even when the UI event loop is parked) and read under the lock
    /// for rendering and queries.
    parser: Arc<Mutex<vt100::Parser<Alerts>>>,
    /// Output timing, kept by the reader thread.
    activity: Arc<Mutex<Activity>>,
    pub watch: Arc<Watch>,
    /// A program here asked for the user (bell, notification, an agent
    /// gone quiet) while nobody was looking; cleared by [`Self::mark_viewed`].
    pub attention: bool,
    /// How the notification for this terminal's last alert went, and when;
    /// gone once you've looked at it.
    pub notified: Option<(Instant, crate::sys::Delivery)>,
    /// Bytes the reader thread has parsed since the last [`pump`] — lets the UI
    /// know a redraw is warranted without re-draining anything itself.
    pending: Arc<AtomicU64>,
    /// Scrolled back past the emulator's live stretch: those rows drawn
    /// again from the compact history (what `screen()` shows meanwhile).
    replay: Option<vt100::Parser>,
    /// Latency probe: the reader stamps the first chunk of a fresh output burst
    /// here; [`pump`] takes it and logs if that output waited too long for a
    /// frame (i.e. the UI loop was parked) — the freeze, quantified.
    stamp: Arc<Mutex<Option<Instant>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Typed before Windows' console started (see `spawn`); `None` after.
    startup: Arc<Mutex<Option<Vec<u8>>>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    rows: u16,
    cols: u16,
    /// Scrollback offset in rows (0 = following the live bottom).
    scroll: usize,
    /// Rows that had ever gone into history at the last pump or scroll, so
    /// new output can be measured exactly (the scroll lock in [`pump`]).
    seen: u64,
    /// Selection anchor / live end as `(line_id, col)`, where `line_id` is the
    /// number of lines above the live bottom (0 = bottom row). Storing it this way
    /// (rather than visible coords) keeps a selection anchored to its text as the
    /// view scrolls, so it can span far more than one screen.
    sel_anchor: Option<(usize, u16)>,
    sel_cursor: Option<(usize, u16)>,
    /// Copy mode: a free-floating cursor + selection over the screen, decoupled
    /// from the shell — arrows move the cursor instead of being sent to the
    /// running program. Toggled with ⌥↑/⌥↓, exited with Esc.
    copy_mode: bool,
    /// Set when a foreground command finishes (`is_running()` goes
    /// true→false) while nobody's watching this tab; cleared by
    /// [`Self::mark_viewed`]. Lets the tab strip flag "something finished
    /// running here" separately from "something is running right now".
    finished_unseen: bool,
    /// Test-only recorder of every byte handed to `send_input`, so tests can
    /// assert the exact escape sequences a key/mouse action produces without
    /// racing the shell's echo.
    #[cfg(test)]
    sent: Vec<u8>,
}

impl TerminalPane {
    /// Spawn the user's `$SHELL` in `cwd` as a **login** shell. `folder` is the
    /// base tab label (usually the project directory name).
    ///
    /// `new_default_prog` runs `$SHELL` with argv0 prefixed by `-` (e.g. `-zsh`),
    /// which makes it a login shell — so it sources `~/.zprofile` / `~/.zlogin`
    /// (where macOS users usually set up PATH, Homebrew, and aliases), exactly
    /// like VSCode's integrated terminal. A plain non-login shell would skip
    /// those and end up missing commands and PATH entries.
    pub fn new(
        folder: impl Into<String>,
        rows: u16,
        cols: u16,
        cwd: &Path,
        waker: Option<Waker>,
    ) -> Result<Self> {
        // The `terminalShell` setting; by default Git Bash on Windows (else
        // WSL), your login shell elsewhere.
        let choice = SHELL.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (mut cmd, shell) = match crate::sys::shell_for(&choice) {
            Some((program, args, name)) => {
                let mut cmd = CommandBuilder::new(program);
                cmd.args(args);
                (cmd, Some(name))
            }
            None if cfg!(windows) => anyhow::bail!("terminals need Git Bash or WSL"),
            None => (CommandBuilder::new_default_prog(), None),
        };
        // Git Bash's login profile otherwise starts in your home folder.
        cmd.env("CHERE_INVOKING", "1");
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        // A fresh terminal, even in a Roxaruu started from inside Claude
        // Code: a `claude` here is no other Claude's (`roxaruu-hook`).
        cmd.env_remove("CLAUDECODE");
        // Route scripts that try to open new OS Terminal windows back into the app.
        for (k, v) in crate::termbridge::child_env() {
            cmd.env(k, v);
        }
        // Which terminal this is, for Claude Code's hooks (`roxaruu-hook`).
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        cmd.env("ROXARUU_TERM", &id);
        tracing::info!(cmd = ?cmd.get_argv(), cwd = %cwd.display(), id, "terminal");
        let mut pane = Self::spawn(folder, rows, cols, cmd, waker)
            .inspect_err(|e| tracing::warn!(error = %e, "terminal didn't start"))?;
        if let Some(dir) = crate::termbridge::alerts_dir() {
            *pane.watch.inbox.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.join(id));
        }
        if let Some(name) = shell {
            pane.shell_name = name; // what "nothing running" looks like
        }
        pane.dir = cwd.to_path_buf();
        Ok(pane)
    }

    /// The process a terminal is in front of: the foreground process group's
    /// leader (Unix), or on Windows, where a console has none, the shell's
    /// newest descendant (else the shell itself).
    #[cfg(unix)]
    fn foreground_pid(&self) -> Option<i32> {
        self.master.process_group_leader()
    }

    /// What the system says about the process in front, for a test's
    /// failure message: pid, short name, program, command line.
    #[cfg(test)]
    pub(crate) fn foreground_report(&self) -> String {
        let Some(pid) = self.foreground_pid() else { return "no foreground pid".into() };
        let read = |f: &str| std::fs::read_to_string(format!("/proc/{pid}/{f}")).unwrap_or_default().replace('\0', " ");
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
        format!(
            "pid {pid} (shell {:?}, me {}): name {:?}, comm {:?}, exe {exe:?}, cmdline {:?}",
            self.child.process_id(),
            std::process::id(),
            process_name(pid),
            read("comm").trim(),
            read("cmdline").trim()
        )
    }

    #[cfg(not(unix))]
    fn foreground_pid(&self) -> Option<i32> {
        let shell = self.child.process_id()?;
        crate::sys::foreground_process(shell, &self.shell_name).or(Some(shell as i32))
    }

    /// Whether the shell has exited (e.g. the user typed `exit`).
    pub fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// What the running program is doing, by its output (none: nothing runs).
    pub fn pulse(&self) -> Option<Pulse> {
        Pulse::of(self.is_running(), self.since_output())
    }

    /// When `pulse` next changes with no output to say so (to look again
    /// then, not every frame): printing turns running, running turns quiet,
    /// and quiet counts on. Never sooner than the 5fps unattended output gets.
    pub fn pulse_changes_in(&self) -> Option<Duration> {
        let since = self.since_output().filter(|_| self.is_running())?;
        let next = if since < PRINTING_WITHIN {
            PRINTING_WITHIN - since
        } else if since < QUIET_AFTER {
            QUIET_AFTER - since
        } else {
            Duration::from_secs(1)
        };
        Some(next.max(Duration::from_millis(200)))
    }

    fn since_output(&self) -> Option<Duration> {
        self.activity.lock().unwrap_or_else(|e| e.into_inner()).last.map(|t| t.elapsed())
    }

    /// Whether a foreground command (beyond the bare shell) is running — i.e.
    /// closing this terminal would interrupt something.
    pub fn is_running(&self) -> bool {
        !self.proc.is_empty()
    }

    /// Whether a foreground command finished here since this tab was last
    /// viewed — distinct from [`Self::is_running`], which is about *now*.
    pub fn finished_unseen(&self) -> bool {
        self.finished_unseen
    }

    /// Call when this terminal is the one actually on screen (the active tab
    /// of a focused terminal dialog) — clears the finished-unseen flag.
    pub fn mark_viewed(&mut self) {
        self.finished_unseen = false;
        self.attention = false;
        self.notified = None;
    }

    /// Alerts the watcher raised since the last call; marks the tab.
    pub fn take_fired(&mut self) -> Vec<String> {
        let fired =
            std::mem::take(&mut *self.watch.fired.lock().unwrap_or_else(|e| e.into_inner()));
        self.attention |= !fired.is_empty();
        if let Some(d) = self.watch.notified.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.notified = Some((Instant::now(), d));
        }
        fired
    }

    /// The shell's actual live working directory (reflects any `cd` the user
    /// has typed), best-effort. `None` if it can't be determined — the
    /// caller falls back to whatever cwd it already has on hand.
    pub fn current_dir(&self) -> Option<PathBuf> {
        let leader = self.foreground_pid()?;
        // Just after spawning, the child is still a copy of this app (not yet
        // the shell) and sits in the app's own directory — not an answer.
        if process_name(leader).is_some_and(|n| is_own_name(&n)) {
            return None;
        }
        process_cwd(leader)
    }

    /// Force `is_running()` to report true without actually spawning and
    /// waiting on a real foreground process — real detection depends on the
    /// shell's own OSC reporting and is already covered elsewhere; callers
    /// testing the *quit* flow just need a `RunningTerminal` blocker to exist.
    #[cfg(test)]
    pub fn set_running_for_test(&mut self) {
        self.proc = "sleep".to_string();
    }

    /// Force `finished_unseen()` to report true without needing a real
    /// running→idle process transition — for app/ui-level tests of the tab
    /// indicator; the transition logic itself is covered directly in this
    /// module's own tests.
    #[cfg(test)]
    pub fn set_finished_unseen_for_test(&mut self) {
        self.finished_unseen = true;
    }

    /// The URL under terminal cell `(row, col)`, if there is one.
    ///
    /// Long URLs almost always wrap, so the logical line is rebuilt first: a row
    /// whose last cell is occupied is treated as continuing into the next one,
    /// which is how a terminal wraps. Without that, clicking a wrapped link
    /// would hand back half of it.
    pub fn url_at(&self, row: u16, col: u16) -> Option<String> {
        let screen = self.screen();
        let (rows, cols) = screen.size();
        if row >= rows || col >= cols {
            return None;
        }
        let (joined, row_ranges) = wrapped_line(&screen, row);
        let at = row_ranges.into_iter().find(|(r, _)| *r == row)?.1.start + col as usize;
        url_span_at(&joined, at)
    }

    /// A `file:line[:col]` at a visible cell (a compiler's error, a stack
    /// trace) naming a file that exists, relative to the terminal's folder:
    /// the file, its line and column (from 1), and the cells it covers on
    /// that row.
    pub fn file_at(&self, row: u16, col: u16) -> Option<(PathBuf, usize, usize, std::ops::Range<u16>)> {
        let screen = self.screen();
        let (rows, cols) = screen.size();
        if row >= rows || col >= cols {
            return None;
        }
        let (joined, row_ranges) = wrapped_line(&screen, row);
        let range = row_ranges.into_iter().find(|(r, _)| *r == row)?.1;
        let (span, path, line, column) = file_span_at(&joined, range.start + col as usize)?;
        let path = crate::config::expand_home(&path);
        let path = if path.is_absolute() { path } else { self.dir.join(path) };
        if !path.is_file() {
            return None;
        }
        let start = span.start.max(range.start) - range.start;
        let end = span.end.min(range.end) - range.start;
        Some((path, line, column, start as u16..end as u16))
    }

    /// The tab label: the folder, plus the running command when one is active.
    pub fn display_name(&self) -> String {
        if self.proc.is_empty() {
            self.folder.clone()
        } else {
            format!("{} \u{00b7} {}", self.folder, self.proc)
        }
    }

    pub fn spawn(
        folder: impl Into<String>,
        rows: u16,
        cols: u16,
        cmd: CommandBuilder,
        waker: Option<Waker>,
    ) -> Result<Self> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let pty = NativePtySystem::default();
        let pair = pty.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let child = pair.slave.spawn_command(cmd)?;
        let pid = child.process_id();
        // Drop the slave so the master sees EOF when the child exits.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        // Shared with the reader, which answers the terminal's own questions.
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer()?));
        // Windows' console wants its cursor question answered before anything
        // typed: what's sent before it has spoken waits here, and the reader
        // sends it after the answer. (A Unix terminal just buffers it.)
        let startup: Arc<Mutex<Option<Vec<u8>>>> =
            Arc::new(Mutex::new(if cfg!(windows) { Some(Vec::new()) } else { None }));

        // The emulator lives behind a lock and is fed by the reader thread, NOT
        // the UI thread. This is the core of the no-freeze design: PTY output is
        // parsed into the screen the instant it arrives, so even if winit parks
        // the event loop (macOS does this during occlusion / resize / background),
        // the terminal contents stay current and there's never a catch-up burst —
        // the UI just renders the already-up-to-date screen whenever it next wakes.
        let parser = Arc::new(Mutex::new(new_parser(rows, cols)));
        let activity = Arc::new(Mutex::new(Activity::default()));
        let watch = Arc::new(Watch::default());
        {
            let (p, a, w, waker) = (
                Arc::downgrade(&parser),
                Arc::downgrade(&activity),
                Arc::downgrade(&watch),
                waker.clone(),
            );
            thread::spawn(move || self::watch(p, a, w, waker));
        }
        let pending = Arc::new(AtomicU64::new(0));
        let stamp: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        {
            let parser = parser.clone();
            let activity = activity.clone();
            let pending = pending.clone();
            let stamp = stamp.clone();
            let answer = writer.clone();
            let held = startup.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            tracing::info!(?pid, "terminal reader: EOF (shell exited)");
                            break;
                        }
                        Err(e) => {
                            tracing::info!(?pid, error = %e, "terminal reader: read error");
                            break;
                        }
                        Ok(n) => {
                            // read() never reports more than the buffer holds.
                            let chunk = buf.get(..n).unwrap_or_default();
                            // Parse off the UI thread so output is consumed even
                            // while the event loop is parked.
                            feed(&mut parser.lock().unwrap_or_else(|e| e.into_inner()), chunk);
                            // "Where's the cursor?" (DSR 6). Windows' console asks it
                            // as it starts and shows nothing until it's answered;
                            // programs anywhere may ask too. Answered here, at once.
                            let asks = chunk.windows(4).filter(|w| *w == b"\x1b[6n").count();
                            if asks > 0 {
                                let (row, col) = parser
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .screen()
                                    .cursor_position();
                                let reply = format!("\x1b[{};{}R", row + 1, col + 1).repeat(asks);
                                let mut w = answer.lock().unwrap_or_else(|e| e.into_inner());
                                let _ = w.write_all(reply.as_bytes()).and_then(|()| w.flush());
                            }
                            // It has started: its first output is its question
                            // (answered above), or a prompt if it never asks.
                            // What was typed meanwhile goes now, in order
                            // (sending waits on this lock).
                            let mut held = held.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(typed) = held.take().filter(|t| !t.is_empty())
                            {
                                let mut w = answer.lock().unwrap_or_else(|e| e.into_inner());
                                let _ = w.write_all(&typed).and_then(|()| w.flush());
                            }
                            // Burst timing, for the watcher's "agent went quiet".
                            {
                                let mut a = activity.lock().unwrap_or_else(|e| e.into_inner());
                                let now = Instant::now();
                                if a.burst.is_none() || a.last.is_none_or(|t| now - t > BURST_GAP) {
                                    a.burst = Some(now);
                                }
                                a.last = Some(now);
                            }
                            pending.fetch_add(n as u64, Ordering::Release);
                            // Stamp the first chunk of a burst so pump() can measure
                            // how long it waited to be shown (the freeze probe).
                            if let Ok(mut s) = stamp.lock()
                                && s.is_none()
                            {
                                *s = Some(Instant::now());
                            }
                            // Nudge the UI to redraw now that there's fresh output.
                            if let Some(w) = &waker {
                                w(false);
                            }
                        }
                    }
                }
            });
        }
        tracing::info!(?pid, rows, cols, "terminal spawned");

        let shell_name = std::env::var("SHELL")
            .ok()
            .and_then(|s| s.rsplit('/').next().map(|n| n.to_string()))
            .unwrap_or_else(|| "sh".to_string());

        Ok(TerminalPane {
            folder: folder.into(),
            dir: PathBuf::new(),
            proc: String::new(),
            shell_name,
            last_proc_check: Instant::now() - PROC_POLL,
            in_sight: true,
            // Check once as soon as the shell first prints its prompt.
            cwd_dirty: true,
            last_cwd_check: Instant::now() - CWD_POLL,
            parser,
            activity,
            watch,
            attention: false,
            notified: None,
            pending,
            replay: None,
            stamp,
            writer,
            startup,
            master: pair.master,
            child,
            rows,
            cols,
            scroll: 0,
            seen: 0,
            sel_anchor: None,
            sel_cursor: None,
            copy_mode: false,
            finished_unseen: false,
            #[cfg(test)]
            sent: Vec::new(),
        })
    }

    /// Feed any pending PTY output into the emulator. Returns whether any new
    /// bytes were processed (so callers can decide to redraw).
    pub fn pump(&mut self) -> usize {
        // The reader thread already parsed the bytes into the emulator; we only
        // settle the view and report how many arrived so the caller can redraw.
        let bytes = self.pending.swap(0, Ordering::Acquire) as usize;
        if bytes > 0 {
            // Latency probe: how long did this output wait for a frame? A large
            // value means the UI event loop was parked (the freeze) — log it so a
            // recurrence is captured as hard evidence, not a vague report.
            if let Some(t) = self.stamp.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let lag = t.elapsed();
                if lag >= Duration::from_millis(250) {
                    tracing::warn!(
                        lag_ms = lag.as_millis() as u64,
                        "terminal output waited for a frame (UI loop was parked)"
                    );
                }
            }
            // Scroll lock: hold the *text* still while output streams past.
            //
            // The offset is measured from the live bottom, so simply re-applying
            // it (which is what this did) drags the viewport down one row for
            // every row that arrives — the line you were reading marches off the
            // top and you end up at the bottom within seconds. Holding the
            // offset is not holding the position. Growing it by however much
            // history arrived keeps the same text on screen; at offset 0 the
            // pane is following the bottom and should keep following it.
            let parser = self.parser.clone();
            let mut p = parser.lock().unwrap_or_else(|e| e.into_inner());
            // Exactly how many rows went into history since: every row the
            // live stretch let go of is counted by the compact history, so
            // this holds once both are full too.
            let entered = Self::entered(&mut p);
            let arrived = entered.saturating_sub(self.seen) as usize;
            self.seen = entered;
            let len = Self::history_total(&mut p);
            if self.scroll > 0 {
                self.scroll = (self.scroll + arrived).min(len);
            }
            // A mark is stored the same bottom-relative way, so it needs the
            // same correction — otherwise it stays a fixed distance from the
            // bottom while the bottom moves, and the highlight visibly travels
            // down the screen onto whatever unrelated output has arrived since.
            // Applied whether or not the view is scrolled: at the bottom the
            // marked text is sliding up into history, and the mark has to
            // follow it there.
            if arrived > 0 {
                let ceiling = len + self.rows as usize;
                for (lid, _) in [&mut self.sel_anchor, &mut self.sel_cursor]
                    .into_iter()
                    .flatten()
                {
                    *lid = (*lid + arrived).min(ceiling);
                }
            }
            self.replay = Self::show(&mut p, self.scroll, self.rows, self.cols);
            self.cwd_dirty = true;
        }
        self.refresh_foreground();
        self.refresh_folder();
        bytes
    }

    // ---- scrollback ----------------------------------------------------

    /// Scroll the view by `delta` rows (positive = up into history).
    pub fn scroll_lines(&mut self, delta: i32) {
        let next = (self.scroll as i32 + delta).max(0) as usize;
        self.set_scroll(next);
        // In copy mode keep the free cursor on screen, so scrolling (PageUp/Dn)
        // and then moving it doesn't snap the view back to where it was.
        if self.copy_mode
            && let Some((lid, col)) = self.sel_cursor
        {
            let top = self.scroll + self.rows as usize - 1;
            self.sel_cursor = Some((lid.clamp(self.scroll, top), col));
        }
    }

    /// Scroll by (almost) a full screen; `dir` +1 = up into history, -1 = down.
    pub fn scroll_page(&mut self, dir: i32) {
        let page = (self.rows.saturating_sub(1)).max(1) as i32;
        self.scroll_lines(dir * page);
    }

    /// Jump back to the live bottom of the terminal.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
        self.replay = None;
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen_mut()
            .set_scrollback(0);
    }

    /// Current scrollback offset (0 = at the live bottom).
    pub fn scroll_offset(&self) -> usize {
        self.scroll
    }

    /// The scrollback offset and all the history there is (a scrollbar's).
    pub fn scroll_extent(&self) -> (usize, usize) {
        let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        (self.scroll, Self::history_total(&mut p))
    }

    /// Whether a full-screen program (vim, less, htop, …) owns the alternate
    /// screen. When it does, PageUp/PageDown belong to that program, not our
    /// scrollback — there's no scrollback on the alt screen anyway.
    pub fn on_alternate_screen(&self) -> bool {
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .alternate_screen()
    }

    // ---- selection (scrollback-aware) ----------------------------------
    //
    // Endpoints are `(line_id, col)` with `line_id` = lines above the live
    // bottom. Moving the cursor past a visible edge auto-scrolls to follow it, so
    // a selection (mouse or keyboard) can run across the whole scrollback.

    /// The line_id of visible `row` at the current scroll offset.
    fn cell_lineid(&self, row: u16) -> usize {
        let r = row.min(self.rows.saturating_sub(1));
        (self.rows - 1 - r) as usize + self.scroll
    }

    /// The visible row currently showing `lineid`, or `None` if it's off-screen.
    fn lineid_row(&self, lineid: usize) -> Option<u16> {
        let rows = self.rows as usize;
        if lineid >= self.scroll && lineid < self.scroll + rows {
            Some((rows - 1 - (lineid - self.scroll)) as u16)
        } else {
            None
        }
    }

    /// A reading-order scalar (top→bottom, left→right increasing) for range tests.
    fn ord(&self, lineid: usize, col: u16) -> i64 {
        ((1i64 << 40) - lineid as i64) * (self.cols as i64 + 1) + col as i64
    }

    /// Rows of history in the emulator's live stretch right now.
    ///
    /// vt100 has no getter for this, but `set_scrollback` clamps to it — so ask
    /// for an impossible offset and read back what you were given, then restore.
    fn history_len(p: &mut vt100::Parser<Alerts>) -> usize {
        let current = p.screen().scrollback();
        p.screen_mut().set_scrollback(usize::MAX);
        let len = p.screen().scrollback();
        p.screen_mut().set_scrollback(current);
        len
    }

    /// All the history there is: the live stretch and the compact rest.
    fn history_total(p: &mut vt100::Parser<Alerts>) -> usize {
        Self::history_len(p) + p.callbacks().history.len()
    }

    /// Every row that has ever gone into history (it only grows).
    fn entered(p: &mut vt100::Parser<Alerts>) -> u64 {
        Self::history_len(p) as u64 + p.callbacks().history.pushed()
    }

    /// Show history `scroll` rows up: the emulator's own offset while it's
    /// in the live stretch; past it, those rows drawn again (returned).
    fn show(p: &mut vt100::Parser<Alerts>, scroll: usize, rows: u16, cols: u16) -> Option<vt100::Parser> {
        let live = Self::history_len(p);
        if scroll <= live {
            p.screen_mut().set_scrollback(scroll);
            return None;
        }
        Some(replay(p, scroll, live, rows, cols))
    }

    /// Set the scrollback offset directly (clamped to the history there is).
    fn set_scroll(&mut self, s: usize) {
        let parser = self.parser.clone();
        let mut p = parser.lock().unwrap_or_else(|e| e.into_inner());
        self.scroll = s.min(Self::history_total(&mut p));
        // Re-baseline the history measurement at the moment the view moves.
        // The reader thread parses output continuously, so history can have
        // grown since the last pump; without this, that growth is counted as
        // "arrived while scrolled up" and the view jumps by however much had
        // landed *before* the user scrolled.
        self.seen = Self::entered(&mut p);
        self.replay = Self::show(&mut p, self.scroll, self.rows, self.cols);
    }

    /// Scroll so `lineid` is on screen; returns it clamped to reachable history.
    fn ensure_visible(&mut self, lineid: usize) -> usize {
        let rows = self.rows as usize;
        if lineid + 1 > self.scroll + rows {
            self.set_scroll(lineid + 1 - rows); // bring it to the top visible row
        } else if lineid < self.scroll {
            self.set_scroll(lineid); // bring it to the bottom visible row
        }
        lineid.min(self.scroll + rows - 1)
    }

    pub fn begin_selection(&mut self, row: u16, col: u16) {
        let lid = self.cell_lineid(row);
        self.sel_anchor = Some((lid, col));
        self.sel_cursor = Some((lid, col));
    }

    pub fn update_selection(&mut self, row: u16, col: u16) {
        if self.sel_anchor.is_none() {
            return;
        }
        // Dragging against an edge auto-scrolls so the selection can keep growing.
        if row == 0 {
            self.scroll_lines(1);
        } else if row + 1 >= self.rows {
            self.scroll_lines(-1);
        }
        let lid = self.cell_lineid(row);
        self.sel_cursor = Some((lid, col));
    }

    /// Extend the selection to visible `(row, col)` **without** moving the
    /// anchor — the Shift+Click endpoint. With no prior selection, the clicked
    /// cell becomes the anchor. Lets you click a start, wheel-scroll the end into
    /// view, then Shift+Click it to grab text spanning the whole scrollback.
    pub fn extend_selection(&mut self, row: u16, col: u16) {
        let lid = self.cell_lineid(row);
        if self.sel_anchor.is_none() {
            self.sel_anchor = self.sel_cursor.or(Some((lid, col)));
        }
        self.sel_cursor = Some((lid, col));
    }

    /// Whether a non-empty selection is currently held. A click that produced
    /// a selection was a drag, not a click on a link.
    pub fn selection_active(&self) -> bool {
        matches!((self.sel_anchor, self.sel_cursor), (Some(a), Some(c)) if a != c)
    }

    pub fn clear_selection(&mut self) {
        self.sel_anchor = None;
        self.sel_cursor = None;
    }

    /// Select the whole word under visible cell `(row, col)` — the double-click
    /// gesture.
    pub fn select_word_cell(&mut self, row: u16, col: u16) {
        let lid = self.cell_lineid(row);
        let left = self.word_boundary(row, col, -1);
        let right = self.word_boundary(row, col, 1);
        self.sel_anchor = Some((lid, left));
        self.sel_cursor = Some((lid, right));
    }

    /// Select the entire visible line at `row` — the triple-click gesture.
    pub fn select_line_cell(&mut self, row: u16) {
        let lid = self.cell_lineid(row);
        self.sel_anchor = Some((lid, 0));
        self.sel_cursor = Some((lid, self.cols));
    }

    /// Move the selection cursor to `(lineid, col)`, anchoring/dropping the mark
    /// per `select` and scrolling to keep it visible.
    fn extend(&mut self, lineid: usize, col: u16, select: bool) {
        if select {
            if self.sel_anchor.is_none() {
                self.sel_anchor = self.sel_cursor;
            }
        } else {
            self.sel_anchor = None;
        }
        let lid = self.ensure_visible(lineid);
        self.sel_cursor = Some((lid, col));
    }

    /// One cell step in line_id space (Shift+arrow, copy-mode arrows). `drow` −1 =
    /// up (older), +1 = down; `dcol` wraps to the adjacent line at row edges.
    fn step(&mut self, drow: i32, dcol: i32, select: bool) {
        let cols = self.cols as i32;
        let (lid0, col0) = self.sel_cursor.unwrap_or((self.scroll, 0));
        let mut lid = lid0 as i64;
        let mut c = col0 as i32;
        if dcol != 0 {
            c += dcol;
            if c < 0 {
                lid += 1; // wrap to the end of the older line
                c = cols - 1;
            } else if c >= cols {
                if lid > 0 {
                    lid -= 1; // wrap to the start of the newer line
                    c = 0;
                } else {
                    c = cols - 1;
                }
            }
        }
        lid -= drow as i64; // up (drow = −1) is older = a larger line_id
        let lid = lid.max(0) as usize;
        let c = c.clamp(0, cols - 1) as u16;
        self.extend(lid, c, select);
    }

    /// Move the cursor to the previous/next word boundary on its line.
    fn word_step(&mut self, dir: i32, select: bool) {
        let (lid, col) = self.sel_cursor.unwrap_or((self.scroll, 0));
        if let Some(row) = self.lineid_row(lid) {
            let nc = self.word_boundary(row, col, dir);
            self.extend(lid, nc, select);
        }
    }

    /// Seed the cursor at the shell cursor if there's no selection yet.
    fn start_from_shell_cursor(&mut self) {
        if self.sel_cursor.is_none() {
            let (r, c) = self
                .parser
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .screen()
                .cursor_position();
            self.sel_cursor = Some((self.cell_lineid(r), c));
        }
    }

    /// Shift+arrow quick-mark (without entering copy mode).
    pub fn select_key(&mut self, drow: i32, dcol: i32) {
        self.start_from_shell_cursor();
        self.step(drow, dcol, true);
    }

    /// Shift+Option+arrow quick word-mark.
    pub fn select_word(&mut self, dir: i32) {
        self.start_from_shell_cursor();
        self.word_step(dir, true);
    }

    /// The column of the word boundary reached by moving from `(row, col)` in
    /// `dir` (words are alphanumeric / underscore runs; everything else is a gap).
    fn word_boundary(&self, row: u16, col: u16, dir: i32) -> u16 {
        let screen = self.screen();
        let ch_at = |c: u16| -> char {
            screen
                .cell(row, c)
                .and_then(|cell| cell.contents().chars().next())
                .unwrap_or(' ')
        };
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        if dir < 0 {
            let mut c = col;
            while c > 0 && !is_word(ch_at(c - 1)) {
                c -= 1;
            }
            while c > 0 && is_word(ch_at(c - 1)) {
                c -= 1;
            }
            c
        } else {
            let mut c = col;
            while c < self.cols && !is_word(ch_at(c)) {
                c += 1;
            }
            while c < self.cols && is_word(ch_at(c)) {
                c += 1;
            }
            c
        }
    }

    // ---- copy mode -----------------------------------------------------

    /// Whether copy mode (free cursor + select, decoupled from the shell) is on.
    pub fn copy_mode(&self) -> bool {
        self.copy_mode
    }

    /// The copy-mode cursor cell `(row, col)` for drawing — `None` unless in copy
    /// mode and the cursor is currently on screen.
    pub fn copy_cursor(&self) -> Option<(u16, u16)> {
        if !self.copy_mode {
            return None;
        }
        let (lid, col) = self.sel_cursor?;
        self.lineid_row(lid).map(|r| (r, col))
    }

    /// Enter copy mode: park a free cursor at the shell cursor, no selection yet.
    pub fn enter_copy_mode(&mut self) {
        self.copy_mode = true;
        let (r, c) = self
            .parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .cursor_position();
        self.sel_cursor = Some((self.cell_lineid(r), c));
        self.sel_anchor = None;
    }

    /// Leave copy mode and drop any selection.
    pub fn exit_copy_mode(&mut self) {
        self.copy_mode = false;
        self.clear_selection();
    }

    /// Move the copy cursor by `(drow, dcol)` (auto-scrolling at edges). With
    /// `select` (Shift) the move marks/extends text; without it, it just moves.
    pub fn copy_move(&mut self, drow: i32, dcol: i32, select: bool) {
        if self.copy_mode {
            self.step(drow, dcol, select);
        }
    }

    /// Move the copy cursor by a word, marking when `select` (Shift) is held.
    pub fn copy_move_word(&mut self, dir: i32, select: bool) {
        if self.copy_mode {
            self.word_step(dir, select);
        }
    }

    /// Whether visible cell `(row, col)` falls inside the current selection
    /// (reading order, end-exclusive) — used to paint the highlight.
    pub fn is_selected(&self, row: u16, col: u16) -> bool {
        let (a, c) = match (self.sel_anchor, self.sel_cursor) {
            (Some(a), Some(c)) if a != c => (a, c),
            _ => return false,
        };
        let pos = self.ord(self.cell_lineid(row), col);
        let (pa, pc) = (self.ord(a.0, a.1), self.ord(c.0, c.1));
        pos >= pa.min(pc) && pos < pa.max(pc)
    }

    /// The selected text, gathered across scrollback (temporarily scrolls to read
    /// each line, then restores the view). `None` when nothing is selected.
    pub fn selection_text(&mut self) -> Option<String> {
        let (a, c) = match (self.sel_anchor, self.sel_cursor) {
            (Some(a), Some(c)) if a != c => (a, c),
            _ => return None,
        };
        // `start` is the earlier cell in reading order (top-left), `end` later.
        let (start, end) = if self.ord(a.0, a.1) <= self.ord(c.0, c.1) {
            (a, c)
        } else {
            (c, a)
        };
        let cols = self.cols;
        let saved = self.scroll;
        let mut out = String::new();
        let mut lid = start.0; // top line has the larger line_id
        loop {
            self.set_scroll(lid); // bring this line onto the bottom visible row
            let Some(r) = self.lineid_row(lid) else { break };
            let (c0, c1) = if lid == start.0 && lid == end.0 {
                (start.1, end.1)
            } else if lid == start.0 {
                (start.1, cols)
            } else if lid == end.0 {
                (0, end.1)
            } else {
                (0, cols)
            };
            let mut line = String::new();
            {
                let screen = self.screen();
                for cc in c0..c1 {
                    match screen.cell(r, cc) {
                        Some(cell) => {
                            let s = cell.contents();
                            line.push_str(if s.is_empty() { " " } else { s });
                        }
                        None => line.push(' '),
                    }
                }
            }
            out.push_str(line.trim_end());
            if lid == end.0 || lid == 0 {
                break;
            }
            out.push('\n');
            lid -= 1;
        }
        self.set_scroll(saved);
        Some(out)
    }

    /// Update the foreground-process label (throttled). Shows the running
    /// command, or nothing when only the shell is at the prompt.
    pub fn refresh_foreground(&mut self) {
        let every = if self.in_sight { PROC_POLL } else { PROC_POLL_UNSEEN };
        if self.last_proc_check.elapsed() < every {
            return;
        }
        self.last_proc_check = Instant::now();
        let pid = self.foreground_pid();
        // The shell itself in front is a bare prompt, whatever it's called:
        // for a moment after spawning, before it execs the shell, the child
        // is a copy of this process, named (on Linux) after the thread that
        // started it. ponytail: `exec prog` in the shell reads as idle too.
        let shell = pid.is_some() && pid == self.child.process_id().map(|p| p as i32);
        let name = pid.filter(|_| !shell).and_then(process_name).unwrap_or_default();
        // Treat the bare shell (login or not, e.g. "-zsh"/"zsh") as idle —
        // and our own name too (a fork, as above).
        let trimmed = name.trim_start_matches('-');
        let proc = if trimmed.is_empty() || trimmed == self.shell_name || is_own_name(trimmed) {
            String::new()
        } else {
            trimmed.to_string()
        };
        self.set_proc(proc);
    }

    /// What runs in it now, asked this moment (closing it: is something
    /// still running?), however recently it was.
    pub fn refresh_foreground_now(&mut self) {
        self.last_proc_check = Instant::now() - PROC_POLL_UNSEEN;
        self.refresh_foreground();
    }

    /// Re-read the shell's working directory so the tab follows `cd`.
    ///
    /// The label used to be frozen at spawn: `cd`ing to another repo left the
    /// tab still naming the folder the terminal started in, which is exactly
    /// backwards for a tab strip whose whole job is telling you which project
    /// each terminal is in.
    ///
    /// Skipped while a command is running, for two reasons: the process-group
    /// leader is then the command rather than the shell, so its cwd needn't be
    /// the shell's, and a build streaming output would otherwise re-fork `lsof`
    /// on every poll. The label catches up the moment the prompt returns.
    fn refresh_folder(&mut self) {
        if !self.cwd_dirty || self.is_running() {
            return;
        }
        if self.last_cwd_check.elapsed() < CWD_POLL {
            return;
        }
        self.last_cwd_check = Instant::now();
        self.cwd_dirty = false;
        if let Some(dir) = self.current_dir() {
            let name = folder_name(&dir);
            if name != self.folder {
                self.folder = name;
            }
            self.dir = dir;
        }
    }

    /// Apply a freshly-detected foreground process name, flagging
    /// finished-unseen on a running→idle edge. Split out from
    /// [`Self::refresh_foreground`] so tests can drive the transition
    /// directly instead of needing a real OS process check.
    fn set_proc(&mut self, proc: String) {
        let was_running = self.is_running();
        self.proc = proc;
        self.watch
            .agent
            .store(AGENTS.contains(&self.proc.as_str()), Ordering::Relaxed);
        *self.watch.name.lock().unwrap_or_else(|e| e.into_inner()) = self.display_name();
        if was_running && !self.is_running() {
            self.finished_unseen = true;
        }
    }

    /// Test-only: drain and return everything sent to the shell since the last
    /// call, so a test can assert exactly what a key/mouse action produced.
    #[cfg(test)]
    pub fn take_sent(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.sent)
    }

    /// Whether the running program has asked to receive mouse events (any xterm
    /// mouse mode is active). When true, the GUI forwards clicks/drags/wheel to
    /// it instead of using them for oxru's own selection & scrollback. Holding
    /// Shift bypasses this so local text selection still works.
    pub fn wants_mouse(&self) -> bool {
        self.parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .mouse_protocol_mode()
            != vt100::MouseProtocolMode::None
    }

    /// Whether the program reports motion (drag) events, not just press/release.
    pub fn wants_mouse_motion(&self) -> bool {
        use vt100::MouseProtocolMode as M;
        matches!(
            self.parser
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .screen()
                .mouse_protocol_mode(),
            M::ButtonMotion | M::AnyMotion
        )
    }

    /// Report a mouse event to the program in its requested xterm encoding.
    /// `cb` is the button byte (0 left, 1 middle, 2 right, 64/65 wheel up/down,
    /// with +32 OR'd in for motion). `(col,row)` are 0-based terminal cells;
    /// `release` marks a button-up. No-op if the program hasn't enabled mouse
    /// reporting.
    pub fn send_mouse(&mut self, cb: u8, col: u16, row: u16, release: bool) {
        use vt100::{MouseProtocolEncoding as E, MouseProtocolMode as M};
        let (mode, enc) = {
            let guard = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = guard.screen();
            (
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };
        if mode == M::None {
            return;
        }
        let cx = col as u32 + 1; // xterm mouse coords are 1-based
        let cy = row as u32 + 1;
        let seq: Vec<u8> = match enc {
            E::Sgr => {
                let fin = if release { 'm' } else { 'M' };
                format!("\x1b[<{cb};{cx};{cy}{fin}").into_bytes()
            }
            _ => {
                // Default / UTF-8 X10 encoding: ESC [ M (cb+32)(cx+32)(cy+32).
                // A release reports button 3; coords clamp at the 223 ceiling.
                let b = if release { 3 } else { cb };
                let off = |v: u32| -> u8 { (v.min(223) as u8).wrapping_add(32) };
                vec![0x1b, b'[', b'M', b.wrapping_add(32), off(cx), off(cy)]
            }
        };
        self.send_input(&seq);
    }

    /// Send raw bytes to the shell.
    pub fn send_input(&mut self, bytes: &[u8]) {
        #[cfg(test)]
        self.sent.extend_from_slice(bytes);
        // XOFF (Ctrl+S) suspends the program's output until XON — the classic
        // "terminal froze, a keypress un-froze it" cause. Log it so we can catch it.
        if bytes.contains(&0x13) {
            tracing::warn!("sending XOFF (Ctrl+S) to the shell — this suspends output");
        }
        let mut held = self.startup.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(typed) = held.as_mut() {
            // The console hasn't said anything yet: they go when it does.
            typed.extend_from_slice(bytes);
            return;
        }
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = w.write_all(bytes).and_then(|()| w.flush()) {
            tracing::warn!(error = %e, bytes = bytes.len(), "terminal write failed");
        }
    }

    /// Paste `text` into the shell. Snaps to the live bottom first, and — when
    /// the running program has enabled **bracketed paste** mode — wraps the text
    /// in the `ESC[200~ … ESC[201~` markers so it's handled as a paste (no
    /// auto-indent, no executing each newline) instead of typed keystrokes.
    pub fn paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.scroll_to_bottom();
        // Normalize newlines to CR, which is what a terminal paste delivers.
        let body = text.replace("\r\n", "\r").replace('\n', "\r");
        let bracketed = self
            .parser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .screen()
            .bracketed_paste();
        if bracketed {
            self.send_input(b"\x1b[200~");
            self.send_input(body.as_bytes());
            self.send_input(b"\x1b[201~");
        } else {
            self.send_input(body.as_bytes());
        }
    }

    /// Resize the PTY + emulator to fit a render area.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.lock().unwrap_or_else(|e| e.into_inner()).screen_mut().set_size(rows, cols);
        // A resize can change the scrollback bounds; keep our offset valid
        // (and draw a replayed view at the new size).
        self.set_scroll(self.scroll);
    }

    /// ⌘K: wipe the screen and scrollback, then have the shell redraw its
    /// prompt (Ctrl-L). Left alone while a full-screen program runs.
    pub fn clear(&mut self) -> bool {
        let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        if p.screen().alternate_screen() {
            return false;
        }
        *p = new_parser(self.rows, self.cols);
        drop(p);
        self.scroll = 0;
        self.seen = 0;
        self.replay = None;
        self.send_input(b"\x0c");
        true
    }

    /// How this terminal wants file paths typed (dropped or pasted): WSL its
    /// Linux view; Git Bash quoted at its prompt, as Windows writes them for
    /// a program running (Claude Code); macOS and Linux escaped.
    pub fn path_style(&self) -> PathStyle {
        if !cfg!(windows) {
            PathStyle::Posix
        } else if self.shell_name.eq_ignore_ascii_case("wsl") {
            PathStyle::Wsl
        } else if self.proc.is_empty() {
            PathStyle::GitBashPrompt
        } else {
            PathStyle::WindowsProgram
        }
    }

    /// `paths` typed into this terminal, space-separated.
    pub fn paths_text(&self, paths: &[std::path::PathBuf]) -> String {
        let style = self.path_style();
        let text: Vec<String> = paths.iter().map(|p| path_text(&p.to_string_lossy(), style)).collect();
        tracing::debug!(?style, n = paths.len(), "paths typed into a terminal");
        text.join(" ")
    }

    /// What its compact history takes in memory (for the log).
    pub fn history_bytes(&self) -> usize {
        self.parser.lock().unwrap_or_else(|e| e.into_inner()).callbacks().history.bytes()
    }

    pub fn screen(&self) -> ScreenGuard<'_> {
        match &self.replay {
            Some(replay) => ScreenGuard::Replay(replay),
            None => ScreenGuard::Live(self.parser.lock().unwrap_or_else(|e| e.into_inner())),
        }
    }
}

impl Drop for TerminalPane {
    fn drop(&mut self) {
        // Kill the shell when its terminal is closed.
        let pid = self.child.process_id();
        if let Err(e) = self.child.kill() {
            tracing::debug!(?pid, error = %e, "terminal kill on close failed");
        } else {
            tracing::info!(?pid, "terminal closed");
        }
    }
}

/// Whether a process name is this app's own (a just-forked child that hasn't
/// exec'd the shell yet).
fn new_parser(rows: u16, cols: u16) -> vt100::Parser<Alerts> {
    vt100::Parser::new_with_callbacks(rows, cols, LIVE_HISTORY, Alerts::default())
}

/// The screen `scroll` rows up, past the emulator's live stretch (`live`
/// rows): each row drawn again from where it is now — the screen, the live
/// stretch, or the compact history. As wide as its widest row: a row from
/// when the terminal was wider runs off the edge rather than wrapping
/// (vt100 can't turn wrapping off); the pane draws only its own width.
fn replay(p: &mut vt100::Parser<Alerts>, scroll: usize, live: usize, rows: u16, cols: u16) -> vt100::Parser {
    let rows_n = usize::from(rows);
    let held = p.callbacks().history.len();
    let lines: Vec<Option<(u16, Vec<u8>)>> = (0..rows_n)
        .map(|r| {
            // Lines above the live bottom (0 = its bottom row), as the
            // selection counts them.
            let lineid = scroll + rows_n - 1 - r;
            if lineid < rows_n {
                p.screen_mut().set_scrollback(0);
                return p.screen().rows_formatted(0, cols).nth(rows_n - 1 - lineid).map(|l| (cols, l));
            }
            let k = lineid - rows_n; // history rows back, 0 = the newest
            if k < live {
                p.screen_mut().set_scrollback(k + 1);
                return p.screen().rows_formatted(0, cols).next().map(|l| (cols, l));
            }
            let stored = p.callbacks().history.get(held.checked_sub(k - live + 1)?)?;
            let (width, line) = stored.split_first_chunk::<2>()?;
            Some((u16::from_le_bytes(*width), line.to_vec()))
        })
        .collect();
    p.screen_mut().set_scrollback(0);
    let wide = lines.iter().flatten().map(|(w, _)| *w).fold(cols, u16::max);
    let mut out = vt100::Parser::new(rows, wide, 0);
    out.process(b"\x1b[?25l"); // no cursor in history
    for (r, line) in lines.into_iter().enumerate() {
        out.process(format!("\x1b[{};1H\x1b[m", r + 1).as_bytes());
        if let Some((_, line)) = line {
            out.process(&line);
        }
    }
    out
}

fn is_own_name(name: &str) -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .as_deref()
        == Some(name.trim_start_matches('-'))
}

/// A folder's display name for the tab label ("shell" for `/`).
pub fn folder_name(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "shell".to_string())
}

/// The short name of process `pid`, for labelling a terminal tab by whatever
/// it's running. The platform call lives in [`crate::sys`], which is where all
/// of Oxru's unsafe code is kept.
fn process_name(pid: i32) -> Option<String> {
    crate::sys::process_name(pid)
}

/// Rebuilds the logical line containing `row` by walking back/forward across
/// wrapped continuation rows (a row whose last cell is occupied is treated as
/// continuing into the next one, which is how a terminal wraps), returning the
/// joined text and each contributing row's (row number, char-offset range)
/// within it. Shared by [`TerminalPane::url_at`] and [`url_cols_in_row`].
fn wrapped_line(screen: &vt100::Screen, row: u16) -> (String, Vec<(u16, std::ops::Range<usize>)>) {
    let (rows, cols) = screen.size();
    let text_of = |r: u16| -> String {
        (0..cols)
            .map(|c| {
                screen
                    .cell(r, c)
                    .map(|cell| {
                        let s = cell.contents();
                        if s.is_empty() {
                            " ".to_string()
                        } else {
                            s.to_string()
                        }
                    })
                    .unwrap_or_else(|| " ".to_string())
            })
            .collect()
    };
    let is_full = |r: u16| -> bool {
        screen
            .cell(r, cols - 1)
            .map(|c| !c.contents().trim().is_empty())
            .unwrap_or(false)
    };

    let mut first = row;
    while first > 0 && is_full(first - 1) {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < rows && is_full(last) {
        last += 1;
    }

    let mut joined = String::new();
    let mut row_ranges = Vec::with_capacity((last - first + 1) as usize);
    for r in first..=last {
        let t = text_of(r);
        let start = joined.chars().count();
        // Only a *wrapped* row runs straight into the next; a short row ended
        // on its own, so keep them separated or two unrelated lines would glue
        // into one bogus link.
        joined.push_str(t.trim_end());
        row_ranges.push((r, start..joined.chars().count()));
        if !is_full(r) {
            joined.push(' ');
        }
    }
    (joined, row_ranges)
}

/// Column ranges within row `row` of `screen` that fall inside a URL — for the
/// renderer to underline, independent of (and possibly wider than) any styling
/// the shell itself applied to that text.
///
/// Takes the [`vt100::Screen`] directly, rather than being a `TerminalPane`
/// method that locks its own screen, so a caller already holding a
/// [`ScreenGuard`] (as the renderer does, for every row) can call it without
/// re-entering the same mutex.
/// Every link on screen, newest (lowest) first, each once — wrapped ones
/// whole. What "Open a Link from the Terminal…" offers.
pub fn urls_on_screen(screen: &vt100::Screen) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for row in (0..screen.size().0).rev() {
        let (joined, ranges) = wrapped_line(screen, row);
        let Some(start) = ranges.iter().find(|(r, _)| *r == row).map(|(_, r)| r.start) else {
            continue;
        };
        for span in url_cols_in_row(screen, row) {
            if let Some(url) = url_span_at(&joined, start + span.start as usize)
                && !out.contains(&url)
            {
                out.push(url);
            }
        }
    }
    out
}

pub fn url_cols_in_row(screen: &vt100::Screen, row: u16) -> Vec<std::ops::Range<u16>> {
    let (rows, _) = screen.size();
    if row >= rows {
        return Vec::new();
    }
    let (joined, row_ranges) = wrapped_line(screen, row);
    let Some(row_range) = row_ranges
        .into_iter()
        .find(|(r, _)| *r == row)
        .map(|(_, r)| r)
    else {
        return Vec::new();
    };
    all_url_spans(&joined)
        .into_iter()
        .filter_map(|span| {
            let start = span.start.max(row_range.start);
            let end = span.end.min(row_range.end);
            (start < end).then(|| (start - row_range.start) as u16..(end - row_range.start) as u16)
        })
        .collect()
}

/// The `path:line[:col]` covering char `at` in `line`: its char span, the
/// path as written, the line, and the column (1 when none). Whatever's
/// around it — quotes, brackets, a trailing `:` or `.` — is left out.
pub fn file_span_at(line: &str, at: usize) -> Option<(std::ops::Range<usize>, String, usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let edge = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '<' | '>' | '{' | '}' | ',' | ';' | '|');
    if chars.get(at).is_none_or(|&c| edge(c)) {
        return None;
    }
    let start = chars.get(..at)?.iter().rposition(|&c| edge(c)).map_or(0, |i| i + 1);
    let end = chars.get(at..)?.iter().position(|&c| edge(c)).map_or(chars.len(), |i| at + i);
    let mut token: String = chars.get(start..end)?.iter().collect();
    while token.ends_with([':', '.']) {
        token.pop();
    }
    // path:line or path:line:col (the path may itself hold a `:`, as C:\ does).
    let mut parts = token.rsplitn(3, ':');
    let (a, b, c) = (parts.next()?, parts.next(), parts.next());
    let number = |s: &str| s.parse::<usize>().ok().filter(|n| *n > 0);
    let (path, line, col) = match (number(a), b.and_then(number), c) {
        (Some(col), Some(line), Some(path)) => (path.to_string(), line, col),
        (Some(line), _, _) => (token.rsplit_once(':')?.0.to_string(), line, 1),
        _ => return None,
    };
    // A name, not a time (12:30) or a URL's port.
    let has_name = path.chars().any(char::is_alphabetic) && !path.contains("://");
    has_name.then(|| (start..start + token.chars().count(), path, line, col))
}

/// Find the URL covering character index `at` in `line`, if any.
///
/// Only `http://` and `https://` are recognised. That's a security choice, not
/// a shortcut: terminal output is untrusted — any repo you build, any log you
/// tail, can print whatever it likes — and honouring schemes like `file://`,
/// `javascript:` or a custom app handler would turn "click a link" into "run
/// something the output chose". A confirmation prompt guards the click; the
/// scheme allow-list guards what a confirmation can even be for.
pub fn url_span_at(line: &str, at: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    // Work in char positions, not byte offsets, so a line with any
    // non-ASCII in it doesn't shift every URL's bounds.
    for cstart in 0..chars.len() {
        let rest = chars.get(cstart..).unwrap_or_default();
        if !starts_scheme(rest) {
            continue;
        }
        let len = rest.iter().take_while(|&&c| !is_url_terminator(c)).count();
        let len = trim_trailing(rest.get(..len).unwrap_or_default());
        let end = cstart + len;
        if at >= cstart && at < end && len > SCHEME_MIN {
            return Some(rest.iter().take(len).collect());
        }
    }
    None
}

/// Every URL span in `line`, as char-offset ranges — the renderer's
/// underline pass over a whole row, so unlike [`url_span_at`] (which stops at
/// the first span containing a given position) this walks the line once.
fn all_url_spans(line: &str) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while let Some(rest) = chars.get(i..).filter(|r| !r.is_empty()) {
        if starts_scheme(rest) {
            let len = rest.iter().take_while(|&&c| !is_url_terminator(c)).count();
            let end = trim_trailing(rest.get(..len).unwrap_or_default()) + i;
            if end - i > SCHEME_MIN {
                spans.push(i..end);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    spans
}

/// Shortest thing that could still be a URL: `http://` plus one character.
const SCHEME_MIN: usize = 8;

fn starts_scheme(rest: &[char]) -> bool {
    let s: String = rest.iter().take(8).collect();
    s.starts_with("http://") || s.starts_with("https://")
}

/// A URL ends at whitespace, or at a character a terminal would never carry
/// inside one (quotes and angle brackets, which shells and logs wrap URLs in).
fn is_url_terminator(c: char) -> bool {
    // A backslash is never part of a URL: it's an escape (`…/two\n`).
    c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '|' | '\\')
}

/// Drop trailing punctuation that belongs to the surrounding prose rather than
/// the URL — "see https://example.com/x." ends in a full stop, not a path.
/// A closing bracket is only dropped when it has no opener inside the URL, so
/// a real `…/Foo_(bar)` link survives.
fn trim_trailing(url: &[char]) -> usize {
    let mut end = url.len();
    while let Some((&c, head)) = url.get(..end).and_then(<[char]>::split_last) {
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            ')' => {
                head.iter().filter(|&&c| c == '(').count()
                    <= head.iter().filter(|&&c| c == ')').count()
            }
            ']' => {
                head.iter().filter(|&&c| c == '[').count()
                    <= head.iter().filter(|&&c| c == ']').count()
            }
            _ => false,
        };
        if !drop {
            break;
        }
        end -= 1;
    }
    end
}

/// The live working directory of `pid`, via `lsof` (no stable libproc struct
/// layout to lean on across macOS versions, so we shell out like
/// [`read_git_status`](crate::app) already does for `git`).
#[cfg(target_os = "macos")]
fn process_cwd(pid: i32) -> Option<PathBuf> {
    if pid <= 0 {
        return None;
    }
    let out = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix('n'))
        .map(PathBuf::from)
}

#[cfg(not(target_os = "macos"))]
fn process_cwd(pid: i32) -> Option<PathBuf> {
    crate::sys::process_cwd(pid)
}

/// Quote `path` so a shell receives it as **one** argument.
///
/// The single place Oxru turns a path into shell text — both dropping a file
/// onto a terminal and pasting one there go through here, so the two can't
/// disagree about what happens to a name with a space in it.
///
/// Backslash-escaping rather than quoting, which is what macOS Terminal and
/// iTerm do when you drop a file: the result stays readable and can be edited
/// in place afterwards, where `'...'` would have to be unwrapped first.
/// Anything outside a conservative safe set is escaped, so a name can't smuggle
/// in a `;` or a `$(...)` that the shell would act on.
pub fn shell_escape(path: &std::path::Path) -> String {
    escape_posix(&path.to_string_lossy())
}

/// How a terminal wants a file's path typed into it (dropped, or pasted
/// after copying files): what the program in it reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathStyle {
    /// macOS and Linux: backslash-escaped, as Terminal.app does.
    Posix,
    /// WSL: the Linux view of a Windows path (`C:\a` → `/mnt/c/a`), escaped.
    Wsl,
    /// A Git Bash prompt: the Windows path in single quotes (bash would eat
    /// its backslashes otherwise; Git Bash's programs read `C:\…` fine).
    GitBashPrompt,
    /// A Windows program running (Claude Code, say): the path as Windows
    /// writes it, in double quotes when it has a space, as Windows Terminal
    /// drops it (a program isn't a shell: it would keep `\`-escapes).
    WindowsProgram,
}

/// `raw` (a path as the OS gave it) typed the way `style` wants.
pub fn path_text(raw: &str, style: PathStyle) -> String {
    if raw.is_empty() {
        return String::new();
    }
    match style {
        PathStyle::Posix => escape_posix(raw),
        PathStyle::Wsl => escape_posix(&wsl_path(raw)),
        PathStyle::GitBashPrompt => format!("'{}'", raw.replace('\'', "'\\''")),
        PathStyle::WindowsProgram if raw.contains([' ', '&', '(', ')', '^', ',', ';', '=']) => format!("\"{raw}\""),
        PathStyle::WindowsProgram => raw.to_string(),
    }
}

/// A Windows path as WSL's Linux sees it: a drive under `/mnt`, a file from
/// a WSL distro's own folder (`\\wsl$\Ubuntu\home\…`) at its own path.
fn wsl_path(raw: &str) -> String {
    for prefix in ["\\\\wsl$\\", "\\\\wsl.localhost\\"] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            // Past the distro's name.
            let within = rest.split_once('\\').map_or("", |(_, p)| p);
            return format!("/{}", within.replace('\\', "/"));
        }
    }
    let mut chars = raw.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            format!("/mnt/{}{}", drive.to_ascii_lowercase(), chars.as_str().replace('\\', "/"))
        }
        _ => raw.replace('\\', "/"),
    }
}

fn escape_posix(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || "._-+/@:,=%".contains(c);
    // A non-ASCII character (an accent, CJK, an emoji in a filename) is passed
    // through unescaped: a backslash in front of one means nothing to the
    // shell, and these are never metacharacters.
    if raw.chars().all(|c| safe(c) || !c.is_ascii()) {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len() + 8);
    for c in raw.chars() {
        if !safe(c) && c.is_ascii() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// How long a test may wait for a real shell to do something before giving up.
///
/// A test's wait that hasn't happened in 2 minutes fails, saying what it
/// waited for (a stuck one used to hang until CI's hour was up; waits still
/// go on the moment the thing happens).
#[cfg(test)]
pub(crate) fn stuck(what: &str, since: Instant) {
    assert!(since.elapsed() < Duration::from_secs(120), "waited 120s for `{what}`: it never happened");
}

#[cfg(test)]
/// Whether `needle` has arrived anywhere in the output, history included
/// (the view is put back exactly where it was).
fn has_output(term: &TerminalPane, needle: &str) -> bool {
    let mut p = term.parser.lock().unwrap_or_else(|e| e.into_inner());
    let (keep, rows) = (p.screen().scrollback(), usize::from(p.screen().size().0).max(1));
    p.screen_mut().set_scrollback(usize::MAX);
    let max = p.screen().scrollback();
    let mut found = false;
    for at in (0..=max).step_by(rows).chain([max]) {
        p.screen_mut().set_scrollback(at);
        if p.screen().contents().contains(needle) {
            found = true;
            break;
        }
    }
    p.screen_mut().set_scrollback(keep);
    // …and the compact history past the live stretch.
    let history = &p.callbacks().history;
    found || (0..history.len()).any(|i| history.get(i).is_some_and(|l| String::from_utf8_lossy(&l).contains(needle)))
}

#[cfg(test)]
/// Pump until `last`, the end of the output just asked for, has arrived —
/// however long the shell takes — and once more to take it in.
fn settle(term: &mut TerminalPane, last: &str) {
    let waiting_since = std::time::Instant::now();
    while !has_output(term, last) {
        crate::terminalpane::stuck(r##"!has_output(term, last)"##, waiting_since);
        term.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    term.pump();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Out of sight, a terminal is asked what runs in it every
    /// `PROC_POLL_UNSEEN`, in sight every `PROC_POLL`; closing asks now.
    #[cfg(unix)]
    #[test]
    fn terminals_out_of_sight_are_asked_less_often() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = TerminalPane::new("t", 24, 80, dir.path(), None).unwrap();
        let just_after = |t: &mut TerminalPane| t.last_proc_check = Instant::now() - PROC_POLL * 2;
        t.in_sight = false;
        just_after(&mut t);
        let before = t.last_proc_check;
        t.refresh_foreground();
        assert_eq!(t.last_proc_check, before, "out of sight: not yet");
        t.in_sight = true;
        t.refresh_foreground();
        assert!(t.last_proc_check > before, "in sight: asked");
        t.in_sight = false;
        let asked = t.last_proc_check;
        t.refresh_foreground_now();
        assert!(t.last_proc_check > asked, "closing asks now, whatever");
    }

    #[test]
    fn file_positions_are_found_in_what_tools_print() {
        let at = |line: &str, word: &str| file_span_at(line, line.find(word).unwrap()).map(|(_, p, l, c)| (p, l, c));
        assert_eq!(at("  --> src/app.rs:42:7", "app"), Some(("src/app.rs".into(), 42, 7)));
        assert_eq!(at("error at main.go:9.", "main"), Some(("main.go".into(), 9, 1)));
        assert_eq!(at("at Object.<anonymous> (/home/me/x.js:3:15)", "home"), Some(("/home/me/x.js".into(), 3, 15)));
        assert_eq!(at(r"C:\code\a.rs:5:2 oops", "code"), Some((r"C:\code\a.rs".into(), 5, 2)));
        assert_eq!(at("see https://x.org:8080/a", "x.org"), None, "a URL's port isn't a line");
        assert_eq!(at("12:30 done", "30"), None, "a time isn't a file");
        assert_eq!(at("main.rs and more", "main"), None, "a name without a line");
        assert_eq!(at("Makefile:12: *** missing separator", "Make"), Some(("Makefile".into(), 12, 1)));
        // The span is the token, not its brackets or trailing colon.
        let line = "(src/a.rs:3):";
        let (span, ..) = file_span_at(line, 3).unwrap();
        assert_eq!(&line[span], "src/a.rs:3");
    }

    /// "Where's the cursor?" gets an answer (Windows' console waits for one
    /// before it shows anything; programs ask it everywhere): bash asks, reads
    /// the reply, and prints it.
    #[cfg(unix)]
    #[test]
    fn the_cursor_position_question_is_answered() {
        let mut cmd = CommandBuilder::new("bash");
        cmd.args(["--norc", "--noprofile"]);
        cmd.cwd(std::env::temp_dir());
        let mut t = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();
        t.send_input(b"printf '\\033[6n'; IFS= read -rs -d R pos; echo \"at:${pos#*[}:\"\r");
        let answered = |t: &TerminalPane| {
            let s = t.screen().contents();
            // (The reply may share the line with its own echo, if it came
            // before `read` turned echo off.)
            s.lines().any(|l| {
                l.rsplit_once("at:").and_then(|(_, r)| r.strip_suffix(':')).is_some_and(|p| {
                    p.split(';').count() == 2 && p.split(';').all(|n| n.parse::<u16>().is_ok_and(|n| n > 0))
                })
            })
        };
        let waiting_since = std::time::Instant::now();
        while !answered(&t) {
            crate::terminalpane::stuck(r##"!answered(&t)"##, waiting_since);
            t.pump();
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Claude Code's hooks reach the terminal they ran in, on every system:
    /// what they run (`roxaruu-hook`, first on the terminal's PATH) leaves its
    /// message in the terminal's inbox, and the terminal raises it.
    /// (Needs the helper built: `cargo build` builds it beside Roxaruu.)
    #[test]
    fn a_claude_hook_reaches_its_terminal() {
        let _turn = crate::termbridge::TEST_FILES.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir();
        let mut t = TerminalPane::new("t", 24, 100, &dir, None).expect("a shell starts");
        assert!(t.watch.inbox.lock().unwrap().is_some(), "the terminal has an inbox");
        t.send_input(b"echo x | roxaruu-hook stop\r");
        let waiting_since = std::time::Instant::now();
        loop {
            crate::terminalpane::stuck("Claude Code: finished", waiting_since);
            t.pump();
            if t.take_fired().iter().any(|m| m == "Claude Code: finished") {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Windows: a terminal runs Git Bash (on GitHub's runners too), runs
    /// what's typed, and names the program running in it, then nothing once it ends.
    #[cfg(windows)]
    #[test]
    fn a_windows_terminal_runs_commands_and_names_them() {
        let dir = std::env::temp_dir();
        let mut t = TerminalPane::new("t", 24, 100, &dir, None).expect("a shell starts");
        let wait = |t: &mut TerminalPane, what: &str, done: &dyn Fn(&mut TerminalPane) -> bool| {
            let waiting_since = std::time::Instant::now();
            while !done(t) {
                crate::terminalpane::stuck(r##"!done(t)"##, waiting_since);
                t.pump();
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        t.send_input(b"echo roxaruu-says-hi\r");
        wait(&mut t, "the echo", &|t| t.screen().contents().matches("roxaruu-says-hi").count() >= 2);
        // ping runs for a while in every Windows shell.
        t.send_input(b"ping -n 30 127.0.0.1\r");
        wait(&mut t, "ping named", &|t| {
            t.last_proc_check = Instant::now() - PROC_POLL * 2;
            t.refresh_foreground();
            t.proc.eq_ignore_ascii_case("ping")
        });
        assert!(t.is_running());
        t.send_input(&[3]); // Ctrl+C
        wait(&mut t, "back to the prompt", &|t| {
            t.last_proc_check = Instant::now() - PROC_POLL * 2;
            t.refresh_foreground();
            !t.is_running()
        });
    }

    #[test]
    fn pulse_from_output() {
        let s = Duration::from_secs;
        assert_eq!(Pulse::of(false, Some(s(0))), None, "nothing running");
        assert_eq!(Pulse::of(true, Some(Duration::from_millis(200))), Some(Pulse::Printing));
        assert_eq!(Pulse::of(true, Some(QUIET_AFTER - Duration::from_millis(1))), Some(Pulse::Running));
        assert_eq!(Pulse::of(true, Some(QUIET_AFTER)), Some(Pulse::Quiet(QUIET_AFTER)));
        assert_eq!(Pulse::of(true, None), Some(Pulse::Running), "nothing printed yet");
        assert_eq!(Pulse::Quiet(s(42)).label(), "quiet for 42s");
        assert_eq!(Pulse::Quiet(s(150)).label(), "quiet for 2m");
        assert_eq!(Pulse::Quiet(s(3 * 3600 + 120)).label(), "quiet for 3h 2m");
    }
    use std::time::{Duration, Instant};

    fn idle_term() -> TerminalPane {
        let dir = std::env::temp_dir();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.cwd(&dir);
        TerminalPane::spawn("t", 24, 80, cmd, None).unwrap()
    }

    /// The tab has to follow `cd`. It used to name the folder the terminal was
    /// spawned in, forever.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn the_folder_label_follows_the_shells_working_directory() {
        // A real shell, told to cd somewhere else, then left at a prompt.
        let start = std::env::temp_dir();
        let target = start.join(format!("oxru-cwd-test-{}", std::process::id()));
        std::fs::create_dir_all(&target).unwrap();

        // The user's real shell, as `App::new_terminal` spawns: the pane
        // recognises `$SHELL` as "idle", and anything else as a running
        // command — which would keep the cwd read permanently deferred.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(&start);
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();
        assert_eq!(term.folder, "t", "starts with the label it was given");

        term.send_input(format!("cd '{}'\n", target.display()).as_bytes());

        // Pump until the label catches up (the read is throttled and only
        // happens once the shell has printed its prompt back).
        let expect = target.file_name().unwrap().to_string_lossy().into_owned();
        let waiting_since = std::time::Instant::now();
        while term.folder != expect {
            crate::terminalpane::stuck(r##"term.folder != expect"##, waiting_since);
            term.pump();
            std::thread::sleep(Duration::from_millis(50));
        }
        std::fs::remove_dir_all(&target).ok();

        // `process_cwd` is a no-op off macOS, where there's nothing to assert.
        if cfg!(target_os = "macos") {
            assert_eq!(
                term.folder, expect,
                "the tab should name where the shell now is"
            );
        }
    }

    /// The read forks `lsof`, so it must not fire for a terminal that hasn't
    /// printed anything, nor while a command is running.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn the_working_directory_is_only_re_read_when_it_could_have_changed() {
        let mut term = idle_term();

        term.cwd_dirty = false;
        term.last_cwd_check = Instant::now() - CWD_POLL * 2;
        term.refresh_folder();
        assert!(
            !term.cwd_dirty,
            "no output since the last read: nothing to do"
        );

        // Fresh output while a command runs: still skipped, so a build
        // streaming output can't re-fork `lsof` on every poll.
        term.cwd_dirty = true;
        term.set_proc("cargo".to_string());
        term.refresh_folder();
        assert!(
            term.cwd_dirty,
            "deferred while a command holds the terminal"
        );

        // Back at the prompt: now it reads, and clears the flag.
        term.set_proc(String::new());
        term.refresh_folder();
        assert!(!term.cwd_dirty, "read once the shell is idle again");
    }

    #[test]
    fn finds_the_url_under_the_clicked_column() {
        let line = "see https://example.com/a/b for details";
        let at = line.find("example").unwrap();
        assert_eq!(
            url_span_at(line, at).as_deref(),
            Some("https://example.com/a/b")
        );
        // Clicking outside the span finds nothing.
        assert_eq!(url_span_at(line, 0), None);
        assert_eq!(url_span_at(line, line.chars().count() - 1), None);
    }

    #[test]
    fn trailing_prose_punctuation_is_not_part_of_the_link() {
        for (line, want) in [
            ("go to https://example.com/x.", "https://example.com/x"),
            ("https://example.com/x, then", "https://example.com/x"),
            ("(https://example.com/x)", "https://example.com/x"),
            ("[https://example.com/x]", "https://example.com/x"),
        ] {
            let at = line.find("http").unwrap() + 10;
            assert_eq!(url_span_at(line, at).as_deref(), Some(want), "{line}");
        }
    }

    #[test]
    fn brackets_that_belong_to_the_url_survive() {
        // A wiki-style link really does end in ')'.
        let line = "https://en.wikipedia.org/wiki/Foo_(bar)";
        assert_eq!(url_span_at(line, 10).as_deref(), Some(line));
    }

    /// The renderer underlines a URL by column range, independent of the
    /// click-detection path (`url_at`) that shares the same wrapped-line
    /// logic — this pins the range itself, not just whether *a* URL exists.
    #[test]
    fn url_cols_in_row_spans_the_whole_url_and_nothing_else() {
        let mut parser = vt100::Parser::new(3, 80, 0);
        parser.process(b"see https://example.com/x here\r\n");
        let spans = url_cols_in_row(parser.screen(), 0);
        let start = "see ".chars().count() as u16;
        let end = start + "https://example.com/x".chars().count() as u16;
        assert_eq!(spans, vec![start..end]);
    }

    #[test]
    fn urls_on_screen_newest_first_once_and_whole() {
        let mut parser = vt100::Parser::new(6, 20, 0);
        parser.process(b"old https://a.com/1\r\n");
        // Wraps past 20 columns: still one link.
        parser.process(b"https://example.com/a/long/path\r\n");
        parser.process(b"again https://a.com/1 ftp://no\r\n");
        assert_eq!(
            urls_on_screen(parser.screen()),
            ["https://a.com/1", "https://example.com/a/long/path"]
        );
        let mut echoed = vt100::Parser::new(3, 40, 0);
        echoed.process(b"printf 'https://a.com/x\\n'\r\n");
        assert_eq!(urls_on_screen(echoed.screen()), ["https://a.com/x"]);
        let empty = vt100::Parser::new(3, 20, 0);
        assert!(urls_on_screen(empty.screen()).is_empty());
    }

    #[test]
    fn url_cols_in_row_is_empty_for_plain_text() {
        let mut parser = vt100::Parser::new(3, 80, 0);
        parser.process(b"nothing to see here\r\n");
        assert_eq!(url_cols_in_row(parser.screen(), 0), Vec::new());
    }

    #[test]
    fn quotes_and_angle_brackets_delimit_it() {
        for line in ["\"https://example.com/x\"", "<https://example.com/x>"] {
            assert_eq!(
                url_span_at(line, 10).as_deref(),
                Some("https://example.com/x"),
                "{line}"
            );
        }
    }

    /// Terminal output is untrusted: anything you build or tail can print a
    /// line. Only http(s) is offered, so a click can never be turned into
    /// "open this local file" or "hand this to some app's URL handler".
    #[test]
    fn only_http_and_https_are_recognised() {
        for line in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "vscode://file/etc/passwd",
            "ftp://example.com/x",
            "data:text/html;base64,AAAA",
        ] {
            assert_eq!(url_span_at(line, 6), None, "{line} must not be offered");
        }
    }

    #[test]
    fn a_line_with_two_urls_picks_the_one_clicked() {
        let line = "https://one.example/a  https://two.example/b";
        let first = line.find("one").unwrap();
        let second = line.find("two").unwrap();
        assert_eq!(
            url_span_at(line, first).as_deref(),
            Some("https://one.example/a")
        );
        assert_eq!(
            url_span_at(line, second).as_deref(),
            Some("https://two.example/b")
        );
        // The gap between them isn't a link.
        assert_eq!(url_span_at(line, line.find("  ").unwrap()), None);
    }

    #[test]
    fn non_ascii_earlier_in_the_line_does_not_shift_the_span() {
        // Byte offsets and char positions diverge here; the scanner works in
        // chars, which is what a click's column is.
        let line = "→ ✓ https://example.com/x done";
        let at = line.chars().position(|c| c == 'e').unwrap();
        assert_eq!(
            url_span_at(line, at).as_deref(),
            Some("https://example.com/x")
        );
    }

    /// A broad table of real-world lines, each clicked at *every* column: the
    /// answer must be the same wherever inside a URL you click, and `None`
    /// everywhere outside it. The per-case tests above pin individual rules;
    /// this pins them all holding at once, which is what actually clicking
    /// around in a terminal exercises.
    #[test]
    fn every_column_of_a_line_resolves_consistently() {
        let cases: &[(&str, Option<&str>)] = &[
            ("https://example.com", Some("https://example.com")),
            ("http://example.com/a/b/c", Some("http://example.com/a/b/c")),
            (
                "https://example.com/a?x=1&y=2#f",
                Some("https://example.com/a?x=1&y=2#f"),
            ),
            (
                "Docs live at https://example.com/guide.",
                Some("https://example.com/guide"),
            ),
            (
                "See https://example.com/guide, then run.",
                Some("https://example.com/guide"),
            ),
            (
                "Ends in a colon: https://example.com/x:",
                Some("https://example.com/x"),
            ),
            (
                "(https://example.com/inside-parens)",
                Some("https://example.com/inside-parens"),
            ),
            (
                "[https://example.com/inside-brackets]",
                Some("https://example.com/inside-brackets"),
            ),
            (
                "\"https://example.com/inside-quotes\"",
                Some("https://example.com/inside-quotes"),
            ),
            (
                "<https://example.com/inside-angles>",
                Some("https://example.com/inside-angles"),
            ),
            (
                "https://en.wikipedia.org/wiki/Rust_(language)",
                Some("https://en.wikipedia.org/wiki/Rust_(language)"),
            ),
            (
                "ERROR  build failed, see https://example.com/logs/42 for output",
                Some("https://example.com/logs/42"),
            ),
            (
                "→ ✓ https://example.com/after-non-ascii is still correct",
                Some("https://example.com/after-non-ascii"),
            ),
            ("file:///etc/passwd", None),
            ("javascript:alert(document.cookie)", None),
            ("vscode://file/etc/passwd", None),
            ("ftp://example.com/pub/file.txt", None),
            ("data:text/html;base64,PHNjcmlwdD4=", None),
            ("example.com/no-scheme", None),
            ("ssh://git@example.com/repo.git", None),
            ("plain text with no link at all", None),
        ];
        let mut bad = 0;
        for (line, want) in cases {
            // Click every column; the span must be `want` inside it, None outside.
            let hits: Vec<String> = (0..line.chars().count())
                .filter_map(|i| url_span_at(line, i))
                .collect();
            let got = hits.first().map(String::as_str);
            let uniform = hits.iter().all(|h| Some(h.as_str()) == got);
            if got != *want || !uniform {
                println!("MISMATCH {line:?}\n   want {want:?}  got {got:?} uniform={uniform}");
                bad += 1;
            }
        }
        assert_eq!(bad, 0, "{bad} lines didn't resolve as expected");
    }

    /// Scrolled up, the text must stay put while output streams past — the
    /// whole point of scrolling up in a log. It used to slide one row per
    /// arriving row, so a busy terminal dragged you back to the bottom within
    /// seconds.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn scrolling_up_holds_the_text_still_while_output_arrives() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();

        // A block of numbered lines to scroll back into.
        term.send_input(b"for i in $(seq 1 60); do echo \"LINE-$i\"; done\n");
        let waiting_since = std::time::Instant::now();
        while !screen_contains(&term, "LINE-60") {
            crate::terminalpane::stuck(r##"!screen_contains(&term, "LINE-60")"##, waiting_since);
            term.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            screen_contains(&term, "LINE-60"),
            "seed output never arrived"
        );

        // Scroll up into the history and remember exactly what's on screen.
        term.scroll_lines(15);
        assert!(term.scroll_offset() > 0, "should be scrolled up");
        let before = visible_text(&term);
        let offset_before = term.scroll_offset();

        // Now stream a lot more output past it.
        term.send_input(b"for i in $(seq 1 80); do echo \"NOISE-$i\"; done\n");
        settle(&mut term, "NOISE-80");

        assert_eq!(
            visible_text(&term),
            before,
            "the view moved while the user was scrolled up"
        );
        assert!(
            term.scroll_offset() > offset_before,
            "the offset must grow as history arrives, or the text can't have held still"
        );
    }

    /// Same, once the history is full (a long agent session): old lines
    /// then fall off the top as new ones arrive, so the history stops
    /// growing — the view must still hold the text still.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn scroll_lock_holds_with_a_full_history() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();
        let fill = HISTORY + 200;
        term.send_input(format!("for i in $(seq 1 {fill}); do echo \"LINE-$i\"; done\n").as_bytes());
        let last = format!("LINE-{fill}");
        let waiting_since = std::time::Instant::now();
        while !screen_contains(&term, &last) {
            crate::terminalpane::stuck(r##"!screen_contains(&term, &last)"##, waiting_since);
            term.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(screen_contains(&term, &last), "seed output never arrived");

        term.scroll_lines(15);
        let before = visible_text(&term);
        term.send_input(b"for i in $(seq 1 80); do echo \"NOISE-$i\"; done\n");
        settle(&mut term, "NOISE-80");
        assert_eq!(visible_text(&term), before, "the view moved with a full history");
    }

    /// The rows on screen, each trimmed.
    fn visible_rows(t: &TerminalPane) -> Vec<String> {
        let screen = t.screen();
        (0..t.rows)
            .map(|r| (0..t.cols).map(|c| screen.cell(r, c).map(|x| x.contents()).unwrap_or_default()).collect::<String>().trim_end().to_string())
            .collect()
    }

    /// A program that prints `lines` red lines `L1`…, then `DONE`, then waits.
    fn printed(lines: usize, rows: u16, cols: u16) -> TerminalPane {
        let mut cmd = CommandBuilder::new("/bin/sh");
        let script = format!("i=1; while [ $i -le {lines} ]; do printf '\\033[31mL%d\\033[m\\n' $i; i=$((i+1)); done; echo DONE; sleep 60");
        cmd.args(["-c", &script]);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", rows, cols, cmd, None).unwrap();
        settle(&mut term, "DONE");
        term
    }

    /// Far past the emulator's live stretch, history is drawn again from the
    /// compact store: the right lines, in their colours.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn deep_history_is_drawn_again() {
        let mut term = printed(1_000, 24, 80);
        // On screen: L979…L1000, DONE, the cursor's empty row (lines above
        // the bottom: 0 the empty row, 1 DONE, n ≥ 2 is L(1002 - n)).
        term.scroll_lines(700);
        assert!(term.replay.is_some(), "past the live {LIVE_HISTORY} rows");
        let rows = visible_rows(&term);
        assert_eq!(rows.first().map(String::as_str), Some("L279"), "{rows:?}");
        assert_eq!(rows.last().map(String::as_str), Some("L302"), "{rows:?}");
        let red = term.screen().cell(0, 0).map(|c| c.fgcolor());
        assert_eq!(red, Some(vt100::Color::Idx(1)), "its colour comes back too");
        // Back to the live stretch, then the bottom: the emulator's own again.
        term.scroll_lines(-650);
        assert!(term.replay.is_none());
        assert_eq!(visible_rows(&term).last().map(String::as_str), Some("L952"));
        term.scroll_to_bottom();
        assert!(visible_rows(&term).contains(&"DONE".to_string()));
        // The far end: the oldest lines kept.
        term.scroll_lines(100_000);
        assert_eq!(visible_rows(&term).first().map(String::as_str), Some("L1"));
    }

    /// Scrolled deep, the view holds still while output pushes more rows
    /// through the live stretch into the compact history.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn scroll_lock_holds_deep_in_history() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();
        term.send_input(b"for i in $(seq 1 1500); do echo \"LINE-$i\"; done\n");
        settle(&mut term, "LINE-1500");
        term.scroll_lines(900);
        assert!(term.replay.is_some());
        let before = visible_rows(&term);
        term.send_input(b"for i in $(seq 1 400); do echo \"NOISE-$i\"; done\n");
        settle(&mut term, "NOISE-400");
        assert_eq!(visible_rows(&term), before, "the view moved deep in history");
    }

    /// A selection from deep history down into the live part copies every
    /// line between, in order.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn a_selection_spans_the_compact_history_and_the_live_part() {
        let mut term = printed(600, 24, 80);
        term.scroll_lines(400); // L179… on top: in the compact history
        term.begin_selection(0, 0);
        term.scroll_to_bottom();
        term.update_selection(21, 79); // L600 (DONE is on 22, the cursor's row 23)
        let text = term.selection_text().unwrap_or_default();
        let numbers: Vec<usize> = text.lines().filter_map(|l| l.trim().strip_prefix('L')?.parse().ok()).collect();
        let first = *numbers.first().unwrap();
        assert_eq!(numbers, (first..first + numbers.len()).collect::<Vec<_>>(), "no gaps, in order");
        assert!(first < 250 && numbers.last() == Some(&600), "from deep history to the bottom: {first}…{:?}", numbers.last());
    }

    /// Rows from when the terminal was wider come back as they were, one
    /// per row (not wrapped onto the next).
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn rows_from_a_wider_terminal_stay_one_row_each() {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "i=1; while [ $i -le 400 ]; do printf 'W%03d%0105d\\n' $i 0; i=$((i+1)); done; echo DONE; sleep 60"]);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", 24, 120, cmd, None).unwrap();
        settle(&mut term, "DONE");
        term.resize(24, 60);
        term.scroll_lines(300);
        assert!(term.replay.is_some());
        let rows = visible_rows(&term);
        assert!(rows.iter().all(|r| r.starts_with('W')), "{rows:#?}");
    }

    /// …and at the bottom it must still follow, or the terminal would freeze.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn at_the_bottom_the_view_keeps_following_new_output() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();
        // (The output line, not the command echoing it.)
        let printed = |t: &TerminalPane, m: &str| visible_text(t).lines().any(|l| l.trim_end() == m);
        term.send_input(b"echo FIRST-MARKER\n");
        let waiting_since = std::time::Instant::now();
        while !printed(&term, "FIRST-MARKER") {
            crate::terminalpane::stuck(r##"!printed(&term, "FIRST-MARKER")"##, waiting_since);
            term.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(term.scroll_offset(), 0, "starts at the bottom");

        term.send_input(b"echo SECOND-MARKER\n");
        let waiting_since = std::time::Instant::now();
        while !printed(&term, "SECOND-MARKER") {
            crate::terminalpane::stuck(r##"!printed(&term, "SECOND-MARKER")"##, waiting_since);
            term.pump();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(term.scroll_offset(), 0, "and still pinned there");
    }

    fn visible_text(t: &TerminalPane) -> String {
        let screen = t.screen();
        let (rows, cols) = screen.size();
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| screen.cell(r, c).map(|x| x.contents()).unwrap_or_default())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn screen_contains(t: &TerminalPane, needle: &str) -> bool {
        visible_text(t).contains(needle)
    }

    /// Marking text while scrolled up must not restart the auto-scroll.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn a_selection_does_not_break_scroll_lock() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();
        term.send_input(b"for i in $(seq 1 60); do echo \"LINE-$i\"; done\n");
        settle(&mut term, "LINE-60");

        term.scroll_lines(15);
        // Mark a couple of rows, the way a mouse drag does.
        term.begin_selection(5, 0);
        term.update_selection(7, 20);
        let before = visible_text(&term);
        assert!(term.selection_text().is_some(), "something is selected");
        let after_copy = visible_text(&term);
        assert_eq!(after_copy, before, "reading the selection moved the view");

        term.send_input(b"for i in $(seq 1 80); do echo \"NOISE-$i\"; done\n");
        settle(&mut term, "NOISE-80");
        assert_eq!(
            visible_text(&term),
            before,
            "the view scrolled while text was selected and output arrived"
        );
    }

    /// A mark must stay on the text it was made on. Both the mark and the
    /// scroll offset are stored as a distance from the live bottom, so output
    /// arriving moves them both off their content — the highlight slides down
    /// the screen onto unrelated output, which reads as the terminal scrolling
    /// even though the view itself is holding still.
    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn a_mark_stays_on_the_text_it_was_made_on() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(std::env::temp_dir());
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();
        term.send_input(b"for i in $(seq 1 60); do echo \"LINE-$i\"; done\n");
        settle(&mut term, "LINE-60");

        term.scroll_lines(15);
        term.begin_selection(5, 0);
        term.update_selection(5, 30);
        let before = term.selection_text().unwrap_or_default();
        assert!(
            before.starts_with("LINE-"),
            "seeded on a known line, got {before:?}"
        );

        term.send_input(b"for i in $(seq 1 80); do echo \"NOISE-$i\"; done\n");
        settle(&mut term, "NOISE-80");

        assert_eq!(
            term.selection_text().unwrap_or_default(),
            before,
            "the mark drifted onto different text as output arrived"
        );
    }

    /// What Claude Code and other tools send, per channel; progress and
    /// other OSCs aren't notifications.
    #[test]
    fn notification_escapes() {
        let said = |bytes: &[u8]| {
            let mut p = new_parser(3, 80);
            p.process(bytes);
            std::mem::take(&mut p.callbacks_mut().said)
        };
        assert_eq!(
            said(b"\x1b]9;Claude Code: waiting for input\x07"),
            ["Claude Code: waiting for input"]
        );
        assert_eq!(said(b"\x1b]9;4;1;50\x07"), Vec::<String>::new());
        assert_eq!(
            said(b"\x1b]777;notify;Claude Code;Task done\x1b\\"),
            ["Claude Code: Task done"]
        );
        assert_eq!(
            said(b"\x1b]99;i=1:d=0:p=title;Claude Code\x1b\\\x1b]99;i=1:p=body;All done\x1b\\"),
            ["All done"]
        );
        assert_eq!(said(b"hi\x07"), [""]);
        assert_eq!(
            said(b"\x1b]0;a title\x07\x1b]7;file:///tmp\x07"),
            Vec::<String>::new()
        );
    }

    /// Only a burst that went on a while, then stopped, counts; once.
    #[test]
    fn quiet_after_work() {
        // In multiples of AGENT_QUIET: work as long as it, then as much silence.
        let ago = |q: f32| Some(Instant::now() - AGENT_QUIET.mul_f32(q));
        let mut a = Activity {
            burst: ago(3.0),
            last: ago(1.3),
        };
        assert!(a.went_quiet());
        assert!(!a.went_quiet(), "reported once");
        let mut a = Activity {
            burst: ago(1.6),
            last: ago(1.3),
        };
        assert!(!a.went_quiet(), "a blip isn't work");
        let mut a = Activity {
            burst: ago(3.0),
            last: ago(0.3),
        };
        assert!(!a.went_quiet(), "still going");
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn finished_unseen_starts_false() {
        let term = idle_term();
        assert!(!term.finished_unseen());
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn finished_unseen_flags_on_running_to_idle_transition() {
        let mut term = idle_term();
        term.set_proc("sleep".to_string());
        assert!(
            !term.finished_unseen(),
            "still running, nothing finished yet"
        );
        term.set_proc(String::new());
        assert!(
            term.finished_unseen(),
            "sleep -> idle should flag finished-unseen"
        );
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn finished_unseen_stays_false_while_command_changes_but_keeps_running() {
        let mut term = idle_term();
        term.set_proc("sleep".to_string());
        term.set_proc("vim".to_string());
        assert!(
            !term.finished_unseen(),
            "one command replacing another isn't 'finished'"
        );
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn mark_viewed_clears_finished_unseen() {
        let mut term = idle_term();
        term.set_proc("sleep".to_string());
        term.set_proc(String::new());
        assert!(term.finished_unseen());
        term.mark_viewed();
        assert!(!term.finished_unseen());
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn captures_command_output() {
        let dir = std::env::temp_dir();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "printf OXRUOK"]);
        cmd.cwd(&dir);
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();

        let mut found = false;
        let waiting_since = std::time::Instant::now();
        while !found {
            crate::terminalpane::stuck(r##"!found"##, waiting_since);
            term.pump();
            let text: String = (0..24)
                .flat_map(|r| (0..80).filter_map(move |c| (r, c).into()))
                .filter_map(|(r, c): (u16, u16)| {
                    term.screen()
                        .cell(r, c)
                        .map(|cell| cell.contents().to_string())
                })
                .collect();
            if text.contains("OXRUOK") {
                found = true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(found, "expected the command output to reach the emulator");
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn paste_delivers_text_to_shell() {
        // Pasting "printf PASTED\n" should reach the shell and run, just like
        // typing it — newlines normalized to CR so the line executes.
        let dir = std::env::temp_dir();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.cwd(&dir);
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();

        term.paste("printf PASTED\n");
        let mut found = false;
        let waiting_since = std::time::Instant::now();
        while !found {
            crate::terminalpane::stuck(r##"!found"##, waiting_since);
            term.pump();
            let text: String = (0..24)
                .flat_map(|r| (0..80).filter_map(move |c| (r, c).into()))
                .filter_map(|(r, c): (u16, u16)| {
                    term.screen()
                        .cell(r, c)
                        .map(|cell| cell.contents().to_string())
                })
                .collect();
            if text.contains("PASTED") {
                found = true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(found, "pasted command should reach and run in the shell");
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn selection_reads_across_scrollback() {
        // Mark text that runs off the top of the screen into scrollback and copy
        // it — the selection must span more than one screen.
        let dir = std::env::temp_dir();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "for i in $(seq 1 60); do echo L$i; done; sleep 5"]);
        cmd.cwd(&dir);
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();

        let mut got = false;
        let waiting_since = std::time::Instant::now();
        while !got {
            crate::terminalpane::stuck(r##"!got"##, waiting_since);
            term.pump();
            let screen: String = (0..24)
                .flat_map(|r| (0..80).map(move |c| (r, c)))
                .filter_map(|(r, c)| term.screen().cell(r, c).map(|x| x.contents().to_string()))
                .collect();
            got = screen.contains("L60");
            if !got {
                thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(got, "expected the loop output on screen");

        // Shift+Up far enough to auto-scroll into history while marking.
        term.enter_copy_mode();
        for _ in 0..40 {
            term.copy_move(-1, 0, true);
        }
        let text = term.selection_text().unwrap_or_default();
        let lines = text
            .lines()
            .filter(|l| l.trim_start().starts_with('L'))
            .count();
        assert!(
            lines >= 5,
            "selection across scrollback should capture many lines, got {lines}: {text:?}"
        );
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn shift_click_extends_selection_across_scroll() {
        // Simulate: click a start point, wheel-scroll up into history, then
        // Shift+Click a point now on screen. The selection must span the lines
        // scrolled past — proving long-text selection works without dragging.
        let dir = std::env::temp_dir();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "for i in $(seq 1 60); do echo L$i; done; sleep 5"]);
        cmd.cwd(&dir);
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();

        let mut got = false;
        let waiting_since = std::time::Instant::now();
        while !got {
            crate::terminalpane::stuck(r##"!got"##, waiting_since);
            term.pump();
            let screen: String = (0..24)
                .flat_map(|r| (0..80).map(move |c| (r, c)))
                .filter_map(|(r, c)| term.screen().cell(r, c).map(|x| x.contents().to_string()))
                .collect();
            got = screen.contains("L60");
            if !got {
                thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(got, "expected the loop output on screen");

        // Click near the bottom (anchor), scroll up a screenful, Shift+Click the
        // top — exactly the click → scroll → shift-click flow in the GUI.
        term.begin_selection(22, 0);
        term.scroll_lines(20);
        term.extend_selection(0, 2);
        let text = term.selection_text().unwrap_or_default();
        let lines = text
            .lines()
            .filter(|l| l.trim_start().starts_with('L'))
            .count();
        assert!(
            lines >= 5,
            "shift-click after scroll should capture many lines, got {lines}: {text:?}"
        );
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn detects_mouse_reporting_request() {
        // A program that turns on xterm mouse tracking (1000) with SGR encoding
        // (1006) must flip wants_mouse() — that's what makes the GUI forward the
        // wheel/clicks to it instead of scrolling oxru's own scrollback.
        let dir = std::env::temp_dir();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "printf '\\033[?1000h\\033[?1006h'; sleep 5"]);
        cmd.cwd(&dir);
        let mut term = TerminalPane::spawn("t", 24, 80, cmd, None).unwrap();

        let waiting_since = std::time::Instant::now();
        while !term.wants_mouse() {
            crate::terminalpane::stuck(r##"!term.wants_mouse()"##, waiting_since);
            term.pump();
            thread::sleep(Duration::from_millis(20));
        }
        assert!(term.wants_mouse(), "should detect xterm mouse-mode enable");
    }

    #[test]
    #[cfg_attr(not(unix), ignore = "needs a Unix shell")]
    fn display_name_reflects_running_command() {
        // Use a plain `/bin/sh` (not the user's login shell) so the test doesn't
        // depend on a slow/noisy `~/.zprofile`; we only need the foreground-proc
        // label logic, which is shell-agnostic.
        let dir = std::env::temp_dir();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.cwd(&dir);
        let mut term = TerminalPane::spawn("proj", 24, 80, cmd, None).unwrap();

        // Run a long-lived command; the label should pick it up.
        term.send_input(b"sleep 5\n");
        let waiting_since = std::time::Instant::now();
        while !term.display_name().contains("sleep") {
            crate::terminalpane::stuck(r##"!term.display_name().contains("sleep")"##, waiting_since);
            term.pump();
            thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod escape_tests {
    use super::{PathStyle, path_text, shell_escape};
    use std::path::Path;

    /// A file dropped (or pasted) on Windows, typed as what's in the
    /// terminal reads it: Claude Code (a Windows program) gets the path as
    /// Windows writes it — `\\`-escaped, as before, it saw a broken path.
    #[test]
    fn a_windows_path_is_typed_as_each_terminal_reads_it() {
        let shot = r"C:\Users\me\Pictures\shot.png";
        let spaced = r"C:\Users\me\My Pictures\a shot.png";
        assert_eq!(path_text(shot, PathStyle::WindowsProgram), shot);
        assert_eq!(path_text(spaced, PathStyle::WindowsProgram), format!("\"{spaced}\""));
        assert_eq!(path_text(r"D:\a&b\c.png", PathStyle::WindowsProgram), r#""D:\a&b\c.png""#);
        // Git Bash's prompt: quoted, so bash keeps the backslashes.
        assert_eq!(path_text(shot, PathStyle::GitBashPrompt), format!("'{shot}'"));
        assert_eq!(path_text(r"C:\it's.png", PathStyle::GitBashPrompt), r"'C:\it'\''s.png'");
        // WSL: the Linux view, escaped like any Linux path.
        assert_eq!(path_text(spaced, PathStyle::Wsl), r"/mnt/c/Users/me/My\ Pictures/a\ shot.png");
        assert_eq!(path_text(r"\\wsl$\Ubuntu\home\me\a.png", PathStyle::Wsl), "/home/me/a.png");
        assert_eq!(path_text(r"\\wsl.localhost\Ubuntu\home\me\a.png", PathStyle::Wsl), "/home/me/a.png");
        // macOS and Linux: as before.
        assert_eq!(path_text("/a b/c.txt", PathStyle::Posix), r"/a\ b/c.txt");
        assert_eq!(path_text("", PathStyle::WindowsProgram), "");
    }

    #[test]
    fn a_plain_path_is_left_alone() {
        assert_eq!(
            shell_escape(Path::new("/Users/mac/src/main.rs")),
            "/Users/mac/src/main.rs"
        );
        assert_eq!(
            shell_escape(Path::new("a-b_c.2+x@y:z,w=1%2")),
            "a-b_c.2+x@y:z,w=1%2"
        );
    }

    #[test]
    fn spaces_are_escaped_so_the_path_stays_one_argument() {
        assert_eq!(
            shell_escape(Path::new("/Users/mac/My Docs/a.txt")),
            "/Users/mac/My\\ Docs/a.txt"
        );
    }

    /// The point of escaping: a filename can't smuggle a command past the shell.
    #[test]
    fn metacharacters_cannot_reach_the_shell() {
        for (raw, want) in [
            ("a;rm -rf b", r"a\;rm\ -rf\ b"),
            ("$(whoami)", "\\$\\(whoami\\)"),
            ("`id`", "\\`id\\`"),
            ("a|b", "a\\|b"),
            ("a&b", "a\\&b"),
            ("a>b", "a\\>b"),
            ("it's", "it\\'s"),
            ("a\"b", "a\\\"b"),
            ("a\\b", "a\\\\b"),
            ("*.rs", "\\*.rs"),
            ("a$HOME", "a\\$HOME"),
        ] {
            assert_eq!(shell_escape(Path::new(raw)), want, "escaping {raw:?}");
        }
    }

    /// A backslash before a non-ASCII char means nothing to the shell, and
    /// these are never metacharacters — so they pass through intact.
    #[test]
    fn non_ascii_names_are_not_mangled() {
        assert_eq!(
            shell_escape(Path::new("/tmp/résumé.txt")),
            "/tmp/résumé.txt"
        );
        assert_eq!(shell_escape(Path::new("/tmp/日本語.md")), "/tmp/日本語.md");
        // …but a space beside one still is escaped.
        assert_eq!(
            shell_escape(Path::new("/tmp/my résumé.txt")),
            "/tmp/my\\ résumé.txt"
        );
    }

    #[test]
    fn an_empty_path_yields_nothing() {
        assert_eq!(shell_escape(Path::new("")), "");
    }
}
