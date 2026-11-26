//! App state and behaviour: config, the open project, tabs, the palette,
//! and running commands. Drawing lives in `ui`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;

use crate::config::{self, Config};
use crate::editor::Tab;
use crate::find::Find;
use crate::fonts;
use crate::fstree::{self, FileTree};
use crate::fuzzy::{self, FileMatcher};
use crate::keymap::{COMMANDS, Command};
use crate::palette::{Item, Mode, Palette, Pick, Revert};
use crate::terminal::{self, Terminal, Throttle};
use crate::ui::icon;

mod extras;
pub(crate) mod rox;
pub(crate) mod files;
mod updates;
pub use extras::{Setting, Toast};
pub use files::Ask;
pub(crate) use updates::Update;
#[cfg(test)]
pub use files::set_clipboard_files;

#[cfg(test)]
thread_local! {
    static DRY_RUN: std::cell::RefCell<Option<Vec<Command>>> = const { std::cell::RefCell::new(None) };
}

/// Test-only: while on, commands are recorded instead of run. Returns what
/// was recorded since the last call.
#[cfg(test)]
pub fn dry_run(on: bool) -> Vec<Command> {
    DRY_RUN.with(|d| std::mem::replace(&mut *d.borrow_mut(), on.then(Vec::new)).unwrap_or_default())
}

/// Search in Files: the last finished search, the one running, and when the
/// query last changed (searches start once typing pauses).
#[derive(Default)]
struct SearchState {
    query: String,
    hits: Vec<(PathBuf, crate::search::SearchMatch)>,
    truncated: bool,
    job: Option<(String, Receiver<crate::search::Results>)>,
    changed: Option<Instant>,
}

/// A project's files (relative, each with whether it's ignored) and their
/// matcher, from a background walk.
type Index = (Vec<(String, bool, bool)>, FileMatcher);
/// Git's answer: the repositories asked, and what changed (none: no repository).
type GitAnswer = (crate::git::Repos, Option<crate::git::Status>);

/// Search results shown at most — more is noise, and costs layout.
pub(crate) const MAX_RESULTS: usize = 300;

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub tree: FileTree,
    /// The tree search's list: (path from the root, ignored, is a folder).
    pub(crate) files: Vec<(String, bool, bool)>,
    pub(crate) matcher: Option<FileMatcher>,
    /// The walk's result, with its matcher built there too (a big repo's
    /// would hold the window up).
    indexing: Option<Receiver<Index>>,
    /// What git says changed (files and the folders they're in), for the
    /// tree's colours: asked off this thread (`git_job`), again when stale.
    pub(crate) git: std::collections::HashMap<PathBuf, crate::git::Change>,
    /// Each repository, its branch and changed files (the Changes view).
    pub(crate) repos: Vec<crate::git::Repo>,
    pub(crate) git_job: Option<Receiver<GitAnswer>>,
    git_stale: bool,
    /// The repositories git is asked about: found again only when files
    /// come or go (`index`), not on every look.
    git_repos: Option<crate::git::Repos>,
    /// When git was last asked, and whether there's a repository to ask.
    git_at: Option<Instant>,
    pub(crate) git_found: bool,
}

/// Closing tabs that may hold unsaved work: they're worked through one at a
/// time, asking about each modified one, then `after` runs.
pub struct Prompt {
    pub queue: Vec<PathBuf>,
    pub after: After,
    /// "Save All" / "Discard All": the answer for every file left.
    pub all: Option<Answer>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum After {
    Nothing,
    CloseFolder,
    /// Close the folder, then open `App::replace_with` here (the first; any
    /// others each in a new window).
    Replace,
    Quit,
    /// Quit, then start again on the update (`update::on_close`).
    Restart,
}

/// Where the keyboard goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Editor,
    Terminal,
    /// The find / replace bar's fields.
    Find,
    /// The file tree.
    Sidebar,
}

/// One view's layout: the window's own, or a browser's copy of it. Derived
/// from that view's size each frame and never saved, so a phone looking in
/// never changes how the Mac's window is laid out.
#[derive(Default, Clone, Copy)]
pub struct View {
    /// Too narrow for the tree beside the editor: it becomes a drawer.
    pub compact: bool,
    /// Too short for the key rows under the tab strips (`tab_hint_row`).
    pub short: bool,
    /// The tree's drawer is out (compact views only).
    pub drawer: bool,
    /// The editor's and the terminals' tab strips: scroll, drag.
    pub tabs: crate::ui::tabstrip::Strip,
    pub terms: crate::ui::tabstrip::Strip,
    /// The tree's width last frame: the status bar's tree section matches it.
    pub tree_w: f32,
}

/// The file tree's own state, and its search box (Go to File). The tree
/// itself is the project's.
#[derive(Default)]
pub struct Sidebar {
    /// The selected row's path (rows move as folders open and close).
    pub selected: Option<PathBuf>,
    /// The file last revealed, so switching tabs reveals the new one once.
    pub revealed: Option<PathBuf>,
    /// Scroll the selected row into view next frame; `center` it (the tree
    /// following the file in front, so what's around it shows too).
    pub scroll: bool,
    pub center: bool,
    /// The tree's scroll position and height last frame.
    pub offset: f32,
    pub view: f32,
    /// The search matches' own (a match chosen must not move the tree by them).
    pub hits_offset: f32,
    pub hits_view: f32,
    /// When the tree was last re-read from disk.
    pub read_at: Option<Instant>,
    /// The search box above the tree: typing there (or anywhere in the
    /// tree) lists matching files in place of the tree.
    pub query: String,
    /// The matches (indices into `Project::files`), the chosen one, and
    /// what they were found for (query, file count) so they're redone
    /// only when either changes.
    pub hits: Vec<usize>,
    pub hit: usize,
    pub hits_for: Option<(String, usize)>,
    /// With the keyboard in the tree: in its search box (Go to File, or
    /// typing), not walking the rows; and whether the search began in the
    /// rows (esc goes back there, else to the editor).
    pub searching: bool,
    pub from_tree: bool,
    /// Go to File showed a hidden tree just for the search: hide it again
    /// once the keyboard leaves.
    pub peek: bool,
}

/// The answer to "save changes?".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
    SaveAll,
    DiscardAll,
}

pub struct App {
    pub config: Config,
    /// Config file mtimes at the last load, polled to reload on save.
    fingerprint: Vec<(PathBuf, Option<SystemTime>)>,
    last_poll: Instant,
    /// Set by a watcher thread when a shared config file changes (another
    /// window saved a setting), which also wakes this window right away.
    config_changed: Arc<AtomicBool>,
    /// The (UI, code) fonts installed in egui, and any errors loading them.
    applied_fonts: Option<(String, String, bool)>,
    font_errors: Vec<String>,
    /// Notifications, oldest first.
    pub toasts: Vec<Toast>,
    /// Terminal alerts raised while the window was in the background,
    /// shown as toasts on coming back.
    missed: Vec<String>,
    /// Text copied in the app this session, newest first (⌥V).
    pub clips: Vec<String>,
    /// Whether the window had the keyboard last frame (coming back to it
    /// asks git again).
    was_focused: bool,
    /// The to-do list, as last read from (and written to) `todos.md`.
    pub todos: Vec<crate::todo::Entry>,
    /// The F1 shortcuts sheet is up.
    pub help: bool,
    pub welcome_selected: usize,
    /// The empty window's recent folders (with their branches), read when
    /// first shown and again on coming back to the window.
    pub welcome_recent: Option<Vec<(PathBuf, u64, Option<String>)>>,
    /// The app icon as a texture, per egui context (this window's, and the
    /// browser's copy of it), loaded the first time it's drawn.
    pub(crate) app_icon: Vec<(egui::Context, egui::TextureHandle)>,
    /// Remote access, as this window shows it.
    pub remote: crate::remote::Status,
    /// Turn remote access on (with a password) or off, on the next update.
    pub(crate) remote_want: Option<crate::remote::Want>,
    /// Set while drawing the browser's copy of the window (see `remote`).
    pub(crate) remote_pass: bool,
    /// Switch Window was run in a browser's copy: its page shows the list.
    pub(crate) remote_windows: bool,
    /// A browser opened a folder that's in another window: it goes there
    /// (`remote` tells the page, which switches once the window is listed).
    pub(crate) remote_follow: Option<String>,
    /// Alerts for the browsers looking at this window (title, message): each
    /// shows a notification there when that tab isn't the one in front.
    pub(crate) remote_notes: Vec<(String, String)>,
    /// Where this window's update stands (`app/updates.rs`).
    pub(crate) updates: updates::Updates,
    /// Picked from the editor's right-click menu, run once it's drawn.
    pub(crate) editor_pick: Option<Command>,
    /// Layout per view: `[the window, a browser's copy]` (see `View`).
    pub(crate) views: [View; 2],
    /// This window's own egui context: settings always go to it, even when
    /// a browser's pass is the one that notices they changed.
    main_ctx: egui::Context,
    pub project: Option<Project>,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    /// Recently closed files, newest last, for Reopen Closed Tab.
    closed: Vec<PathBuf>,
    pub prompt: Option<Prompt>,
    /// The folders to open once this one has closed (`After::Replace`).
    replace_with: Vec<PathBuf>,
    /// Whether `rox` is where a terminal finds it (else the start page
    /// offers to install it).
    pub rox_installed: bool,
    /// The frame a dialog came up in: it doesn't take that frame's keys,
    /// which opened it (egui may run a frame twice; both passes count).
    pub(crate) modal_since: std::cell::Cell<Option<u64>>,
    /// A modified tab whose file changed on disk, awaiting reload-or-keep.
    pub conflict: Option<PathBuf>,
    /// A file dialog (new / rename / delete) or "open this link?".
    pub ask: Option<Ask>,
    /// The system's answer to "may Roxaruu notify?", from whichever thread
    /// it came on (`poll_attention` takes it).
    pub(crate) notify_answer: std::sync::Arc<std::sync::Mutex<Option<crate::sys::NotifyPermission>>>,
    last_file_check: Instant,
    /// Hidden at the last frame (another Space, minimized): coming back is
    /// noticed (`came_back`).
    was_hidden: bool,
    /// Redraw until then, after coming back (`came_back`).
    pub(crate) redraw_until: Option<Instant>,
    /// Looks at the open files on disk so far (tests wait for the next one).
    pub(crate) file_checks: u64,
    /// Set once unsaved work is dealt with, so the window may really close.
    pub(crate) allow_close: bool,
    pub palette: Option<Palette>,
    pub find: Option<Find>,
    search: SearchState,
    pub terminals: Vec<Terminal>,
    pub active_terminal: usize,
    /// All terminals at once, tiled, instead of one tab at a time.
    pub terminal_grid: bool,
    /// The terminal panel fills the window (the editor steps aside).
    pub terminal_max: bool,
    /// This window's terminals' folders as last saved in the Open list.
    terms_saved: Vec<PathBuf>,
    /// All open files tiled side by side (split view).
    pub editor_grid: bool,
    throttle: std::sync::Arc<Throttle>,
    /// How far into the terminal bridge's request file we've read.
    request_offset: u64,
    /// Set by the thread watching that file (`terminal::watch_requests`)
    /// when it changed: read it then, not every frame.
    requests_changed: Option<Arc<AtomicBool>>,
    /// Whether the terminal panel is showing.
    pub terminal_open: bool,
    /// Counts folders opened: the terminal panel's id, so each starts at
    /// its first height, not where the last folder left it.
    pub(crate) terminal_layout: u32,
    /// Where each terminal was drawn last frame, for drops.
    pub terminal_rects: Vec<(usize, egui::Rect)>,
    pub sidebar: Sidebar,
    /// Source Control, shown in the sidebar's place.
    pub changes: crate::ui::changes::Changes,
    /// Commands picked in the palette, latest first (they lead its list).
    used: Vec<Command>,
    pub focus: Focus,
    /// Files opened this session, newest first (ranks them first in search).
    pub(crate) mru: Vec<PathBuf>,
    title: String,
    pub fps: Option<f32>,
    frames: u32,
    fps_window_start: Instant,
    pub mem_kb: Option<u64>,
    mem_last_check: Option<Instant>,
    /// When the memory breakdown was last logged.
    mem_logged: Option<Instant>,
}

impl App {
    /// `target` is the command-line path: a folder to open, or a file to open
    /// along with its folder.
    pub fn new(ctx: &egui::Context, target: Option<PathBuf>) -> Self {
        config::adopt_oxru_folders();
        config::ensure_files();
        let mut app = Self {
            config: config::load(None, None),
            fingerprint: config::fingerprint(None),
            last_poll: Instant::now(),
            config_changed: watch_config(ctx),
            applied_fonts: None,
            font_errors: Vec::new(),
            toasts: Vec::new(),
            missed: Vec::new(),
            clips: Vec::new(),
            was_focused: true,
            todos: Vec::new(),
            help: false,
            welcome_selected: 0,
            welcome_recent: None,
            app_icon: Vec::new(),
            remote: Default::default(),
            remote_want: None,
            remote_pass: false,
            remote_windows: false,
            remote_follow: None,
            remote_notes: Vec::new(),
            updates: Default::default(),
            editor_pick: None,
            views: Default::default(),
            main_ctx: ctx.clone(),
            project: None,
            tabs: Vec::new(),
            active: None,
            closed: Vec::new(),
            prompt: None,
            replace_with: Vec::new(),
            rox_installed: crate::app::rox::installed(),
            modal_since: std::cell::Cell::new(None),
            conflict: None,
            ask: None,
            notify_answer: Default::default(),
            last_file_check: Instant::now(),
            was_hidden: false,
            redraw_until: None,
            file_checks: 0,
            allow_close: false,
            palette: None,
            find: None,
            search: SearchState::default(),
            terminals: Vec::new(),
            active_terminal: 0,
            terminal_grid: false,
            terminal_max: false,
            terms_saved: Vec::new(),
            editor_grid: false,
            throttle: Throttle::new(60),
            // Requests queued before this window existed aren't ours.
            request_offset: crate::termbridge::request_file()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len()),
            requests_changed: None,
            terminal_open: false,
            terminal_layout: 0,
            terminal_rects: Vec::new(),
            sidebar: Sidebar::default(),
            changes: Default::default(),
            used: Vec::new(),
            focus: Focus::Editor,
            mru: Vec::new(),
            title: String::new(),
            fps: None,
            frames: 0,
            fps_window_start: Instant::now(),
            mem_kb: None,
            mem_last_check: None,
            mem_logged: None,
        };
        app.apply(ctx);
        if let Some(path) = target.and_then(|p| dunce::canonicalize(p).ok()) {
            if path.is_dir() {
                app.open_folder(path, ctx);
            } else if let Some(dir) = path.parent() {
                app.open_folder(project_root(dir), ctx);
                app.open_file(&path);
            }
        }
        app
    }

    // ---- config ---------------------------------------------------------

    /// Push the config into egui: fonts (only when they changed — rebuilding
    /// the atlas isn't free) and visuals.
    pub fn apply(&mut self, _ctx: &egui::Context) {
        let ctx = &self.main_ctx.clone();
        let s = &self.config.settings;
        crate::buffer::set_indent_settings(s.tab_size, s.insert_spaces);
        let wanted = (s.ui_font.clone(), s.code_font.clone(), self.font_picker_open());
        if self.applied_fonts.as_ref() != Some(&wanted) {
            fonts::unpack(&wanted.0, &wanted.1);
            self.font_errors = fonts::install(ctx, &wanted.0, &wanted.1, wanted.2);
            self.applied_fonts = Some(wanted);
        }
        let c = &self.config.theme.colors;
        let mut v = if self.config.theme.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = c.background;
        v.window_fill = c.elevated;
        v.window_stroke.color = c.border;
        v.override_text_color = Some(c.text);
        v.text_cursor.stroke.color = c.accent;
        v.selection.bg_fill = c.accent.gamma_multiply(0.35);
        v.selection.stroke.color = c.text;
        ctx.set_visuals(v);
    }

    fn poll_config(&mut self, ctx: &egui::Context) {
        if !self.config_changed.swap(false, Ordering::Relaxed)
            && self.last_poll.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.last_poll = Instant::now();
        let fp = config::fingerprint(self.root());
        if fp == self.fingerprint {
            return;
        }
        let before: Vec<String> = self.errors().cloned().collect();
        self.reload_config(ctx);
        // A new mistake says so where you'll see it, not only in the bar.
        let first = self.errors().next().cloned();
        match first {
            None => self.notify("Config reloaded"),
            Some(first) if !before.contains(&first) => {
                self.warn(format!("{first} — that change waits until it's fixed; the rest still applies"))
            }
            Some(_) => {}
        }
    }

    pub fn errors(&self) -> impl Iterator<Item = &String> {
        self.config.errors.iter().chain(&self.font_errors)
    }

    /// Re-read the config (global + this project's) and apply it.
    pub fn reload_config(&mut self, ctx: &egui::Context) {
        let root = self.root().map(Path::to_path_buf);
        let ignored = self.config.settings.search_ignored;
        self.config = config::load(Some(&self.config), root.as_deref());
        if self.config.settings.search_ignored != ignored {
            // What search covers changed (⌥H, Settings, another window).
            self.index(ctx);
            self.search.query.clear(); // run the current search again
            self.search.changed = Some(Instant::now());
        }
        self.fingerprint = config::fingerprint(root.as_deref());
        tracing::debug!(errors = ?self.config.errors, "config read");
        for e in &self.config.errors {
            tracing::warn!(error = %e, "config");
        }
        self.apply(ctx);
    }

    /// A font picker is up: every font is installed, to show itself.
    pub(crate) fn font_picker_open(&self) -> bool {
        self.palette.as_ref().is_some_and(|p| matches!(p.mode, Mode::UiFonts | Mode::CodeFonts))
    }

    /// The open project's folder.
    pub fn root(&self) -> Option<&Path> {
        self.project.as_ref().map(|p| p.root.as_path())
    }

    /// The status bar's message: the first config error (errors stay until
    /// fixed), with a count of the rest.
    pub fn status_line(&self) -> Option<String> {
        let mut errors = self.errors();
        let first = errors.next()?;
        let more = errors.count();
        Some(if more > 0 {
            format!("{first}  (+{more} more)")
        } else {
            first.clone()
        })
    }

    // ---- frame ----------------------------------------------------------

    /// Per-frame housekeeping before drawing.
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.expire_toasts();
        self.poll_git(ctx);
        self.poll_git_ops();
        // FPS is this window's own: a browser's frames don't count.
        self.frames += u32::from(!self.remote_pass);
        let elapsed = self.fps_window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.fps = Some(self.frames as f32 / elapsed.as_secs_f32());
            self.frames = 0;
            self.fps_window_start = Instant::now();
        }
        if !self
            .mem_last_check
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            self.mem_last_check = Some(Instant::now());
            self.mem_kb = crate::sys::usage(std::process::id() as i32).map(|u| u.footprint_kb);
            // Now and then, where it goes: a report of "it uses N MB" can
            // then be answered from the log.
            if !self.mem_logged.is_some_and(|t| t.elapsed() < Duration::from_secs(600)) {
                self.mem_logged = Some(Instant::now());
                let history_kb: usize = self.terminals.iter().map(|t| t.pane.history_bytes()).sum::<usize>() / 1024;
                tracing::info!(
                    mb = self.mem_kb.map(|kb| kb / 1024),
                    terminals = self.terminals.len(),
                    history_kb,
                    tabs = self.tabs.len(),
                    tabs_read = self.tabs.iter().filter(|t| t.is_loaded()).count(),
                    "memory"
                );
            }
        }
        self.poll_config(ctx);
        self.poll_index();
        if !self.remote_pass {
            self.poll_updates(ctx);
        }

        // The OS close button (or ⌘Q from the app menu): ask about unsaved
        // work first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_quit(ctx);
        }
        if let Some(i) = self.active {
            self.load_tab(i);
        }
        if let Some(t) = self.active_tab_mut() {
            t.refresh(Some(ctx));
        }
        if let (Some(f), Some(i)) = (&mut self.find, self.active)
            && let Some(t) = self.tabs.get(i)
        {
            f.refresh(&t.buf, i);
        }
        // Coming back to the window (its focus, or seen again: another
        // Space, un-minimized) is when files most likely changed, and when
        // what ran meanwhile has to show.
        if !self.remote_pass && self.came_back(ctx) {
            self.last_file_check = Instant::now() - Duration::from_secs(5);
            // While hidden, macOS gives the window no surface and its frames
            // are skipped (no work while away). The frame asked for on coming
            // back can be skipped too, if the surface isn't ready that moment,
            // and nothing asks for another: the old picture stayed until the
            // mouse moved. A few more, briefly, make sure one draws.
            self.redraw_until = Some(Instant::now() + Duration::from_millis(600));
            tracing::debug!("back to the window: redrawing");
        }
        if let Some(until) = self.redraw_until {
            if Instant::now() < until {
                ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                self.redraw_until = None;
            }
        }
        self.poll_file_changes();
        // Shells that exited (`exit`, ^D) close their terminal; the last one
        // closes the panel.
        let before = self.terminals.len();
        self.terminals.retain_mut(|t| !t.pane.exited());
        if self.terminals.len() != before {
            self.after_terminal_removed();
        }
        self.remember_terminals(false);
        // Syntax trees are kept only while a tab is being edited.
        let active = self.active;
        for (i, t) in self.tabs.iter_mut().enumerate() {
            if Some(i) != active || t.last_edit.elapsed() > Duration::from_secs(60) {
                t.forget_parse();
            }
        }
        self.poll_terminal_requests(ctx);
        self.poll_attention(ctx);
        self.poll_search(ctx);
        self.throttle.fps.store(
            self.config.settings.terminal_fps,
            std::sync::atomic::Ordering::Relaxed,
        );
        crate::terminalpane::set_shell(&self.config.settings.terminal_shell);
        // Only terminals on screen wake the UI for their output.
        let shown = self.terminal_open;
        for (i, t) in self.terminals.iter().enumerate() {
            let on_screen = shown && (self.terminal_grid || i == self.active_terminal);
            t.visible
                .store(on_screen, std::sync::atomic::Ordering::Relaxed);
        }

        let title = match (&self.project, self.active_tab()) {
            (Some(p), Some(t)) => format!(
                "{}{} — {}",
                if t.buf.modified { "● " } else { "" },
                self.tab_rel(t),
                p.name
            ),
            (Some(p), None) => p.name.clone(),
            _ => "Roxaruu".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Run the commands bound to this frame's key presses. Swallows the text
    /// a chord would also type (⌥F types "ƒ" on macOS) so it can't land in
    /// the palette's search box.
    pub fn handle_keys(&mut self, ctx: &egui::Context) {
        // The prompt owns the keyboard while it's up (and Keyboard Shortcuts,
        // while it waits for a new key).
        if self.modal_open() || self.capturing() {
            return;
        }
        self.paste_files_as_paths(ctx);
        self.find_toggle_keys(ctx);
        let keymap = &self.config.keymap;
        // With the terminal focused, Ctrl chords are the shell's (^C, ^R…).
        let shell = self.focus == Focus::Terminal && self.terminal_open && self.palette.is_none();
        // (Off macOS a few of ours get past it: `passes_the_shell`.)
        let bound = |e: &egui::Event| match e {
            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } => keymap
                .command_for(*key, *physical_key, *modifiers)
                .filter(|c| !(shell && terminal::belongs_to_shell(*key, *modifiers)) || terminal::passes_the_shell(*c)),
            // ⌘C/⌘X arrive as clipboard events, not keys (⌘V too, as Paste
            // with the text, which the editor or search box takes directly).
            egui::Event::Copy => Some(Command::Copy),
            egui::Event::Cut => Some(Command::Cut),
            _ => None,
        };
        // A command that can't run now leaves its key to the editor. Move to
        // Trash's key (⌘⌫) works only in the file tree (and its search box):
        // elsewhere, even in the command palette's search box, it deletes to
        // the line start.
        let in_files = self.palette.is_none() && self.focus == Focus::Sidebar;
        let command_for = |e: &egui::Event| {
            bound(e).filter(|c| self.available(*c) && (*c != Command::DeleteFile || in_files))
        };
        let (pressed, chord): (Vec<Command>, bool) = ctx.input(|i| {
            (
                i.events.iter().filter_map(command_for).collect(),
                i.events.iter().any(|e| bound(e).is_some()),
            )
        });
        if !chord {
            return;
        }
        // Take the chord's events (and the text it typed, like ⌥F's "ƒ",
        // which comes right after its key) so nothing downstream acts on
        // them too. Other typing in the same frame stays: "ab" then ⌘S
        // landing together (a slow frame, a remote batch) still types "ab";
        // so does a chord whose command can't run here (⌥A's "å" in the
        // editor, Stage All being Source Control's).
        ctx.input_mut(|i| {
            let mut after_chord = false;
            i.events.retain(|e| {
                let keep = command_for(e).is_none()
                    && !(after_chord && matches!(e, egui::Event::Text(_)));
                after_chord = command_for(e).is_some();
                keep
            })
        });
        for command in pressed {
            // The palette's search box handles its own copy/paste.
            if self.palette.is_some()
                && matches!(
                    command,
                    Command::Copy | Command::Cut | Command::Paste | Command::SelectAll
                )
            {
                continue;
            }
            self.run(command, ctx);
        }
    }

    pub fn run(&mut self, command: Command, ctx: &egui::Context) {
        tracing::debug!(?command, "run");
        #[cfg(test)]
        if DRY_RUN.with(|d| {
            d.borrow_mut()
                .as_mut()
                .map(|ran| ran.push(command))
                .is_some()
        }) {
            return;
        }
        // The key that opened a list closes it again.
        let list = match command {
            Command::Palette => Some(Mode::Commands),
            Command::OpenFolder => Some(Mode::Open),
            Command::SearchInFiles => Some(Mode::Search),
            Command::GoToLine => Some(Mode::Line),
            Command::SwitchTerminal => Some(Mode::Terminals),
            Command::OpenTerminalLink => Some(Mode::Links),
            Command::Settings => Some(Mode::Settings),
            Command::Keybindings => Some(Mode::Keybindings),
            Command::ClipboardHistory => Some(Mode::Clipboard),
            _ => None,
        };
        if list.is_some() && self.palette.as_ref().map(|p| p.mode) == list {
            return self.close_palette(false, ctx);
        }
        match command {
            Command::Palette => self.open_palette(Mode::Commands, ctx),
            Command::GoToFile => {
                if self.project.is_some() {
                    self.close_palette(false, ctx);
                    self.search_files()
                } else {
                    self.notify("Open a folder first")
                }
            }
            // Our own picker, never the system's: that one is missing on a
            // server and opens out of sight of a browser using remote access.
            Command::OpenFolder => self.open_palette(Mode::Open, ctx),
            // An empty one: no folder, its terminals start at home.
            Command::InstallRox => self.install_rox(),
            Command::NewWindow => {
                if let Err(e) = files::spawn_empty_window() {
                    self.warn(format!("Couldn't open a new window: {e}"));
                }
            }
            Command::CloseFolder => self.begin_close(self.tab_paths(), After::CloseFolder, ctx),
            Command::SelectTheme => self.open_palette(Mode::Themes, ctx),
            Command::SelectUiFont => self.open_palette(Mode::UiFonts, ctx),
            Command::SelectCodeFont => self.open_palette(Mode::CodeFonts, ctx),
            Command::Quit => self.request_quit(ctx),
            Command::Save => self.save_active(),
            Command::SaveAll => self.save_all(),
            Command::CloseTab => {
                let path = self.active_tab().and_then(|t| t.path().cloned());
                self.begin_close(path.into_iter().collect(), After::Nothing, ctx);
            }
            Command::CloseAllTabs => {
                // Pinned ones stay (⌘W on one closes it).
                let paths = self.tabs.iter().filter(|t| !t.pinned).filter_map(|t| t.path().cloned()).collect();
                self.begin_close(paths, After::Nothing, ctx);
            }
            Command::TogglePinTab => self.toggle_pin(self.terminal_focused()),
            Command::SourceControl => self.source_control(),
            Command::GitStage => {
                if let Some(row) = self.chosen_row() {
                    self.stage_chosen(row, ctx);
                }
            }
            Command::GitStageAll => self.stage_everything(ctx),
            Command::GitDiscard => self.ask_discard(),
            Command::GitCommit => self.commit(false, ctx),
            Command::GitCommitAll => self.commit(true, ctx),
            Command::GitPush => self.sync(crate::git::Op::Push, false, ctx),
            Command::GitPushAll => self.sync(crate::git::Op::Push, true, ctx),
            Command::GitPull => self.sync(crate::git::Op::Pull, false, ctx),
            Command::GitPullAll => self.sync(crate::git::Op::Pull, true, ctx),
            Command::GitOpenFile => self.open_changed_file(),
            Command::CloseOtherTabs
            | Command::CloseTabsToRight
            | Command::CloseTabsToLeft
            | Command::CloseSavedTabs => self.close_around(command, ctx),
            Command::ReopenTab => {
                if let Some(path) = self.closed.pop() {
                    self.open_file(&path);
                }
            }
            // With a terminal focused, tab keys work on terminals.
            Command::NextTab | Command::PrevTab if self.terminal_focused() => {
                let n = self.terminals.len() as isize;
                let step = if command == Command::NextTab { 1 } else { -1 };
                self.active_terminal =
                    (self.active_terminal as isize + step).rem_euclid(n) as usize;
            }
            Command::MoveTabLeft | Command::MoveTabRight if self.terminal_focused() => {
                let to = self.active_terminal as isize + if command == Command::MoveTabRight { 1 } else { -1 };
                if to >= 0 {
                    self.reorder(true, self.active_terminal, to as usize);
                }
            }
            Command::NextTab => self.cycle_tab(1),
            Command::PrevTab => self.cycle_tab(-1),
            Command::MoveTabLeft => self.move_tab(-1),
            Command::MoveTabRight => self.move_tab(1),
            Command::Copy | Command::Cut | Command::Paste if self.terminal_focused() => {
                self.terminal_clipboard(command, ctx)
            }
            Command::Undo | Command::Redo | Command::SelectAll if self.terminal_focused() => {}
            Command::Undo
            | Command::Redo
            | Command::Copy
            | Command::Cut
            | Command::Paste
            | Command::SelectAll => self.edit_command(command, ctx),
            Command::Find | Command::Replace => self.open_find(command == Command::Replace),
            Command::EditWholeFile => {
                if let Some(t) = self.active_tab_mut() {
                    let line = t.caret().0;
                    match t.load(true) {
                        Ok(()) => {
                            t.go_to(line, 0..0);
                            let name = t.name();
                            self.notify(format!("{name} is open for editing"));
                        }
                        Err(e) => self.warn(e),
                    }
                }
            }
            Command::AddNextOccurrence | Command::AddCursorAbove | Command::AddCursorBelow => {
                if let Some(t) = self.active_tab_mut() {
                    match command {
                        Command::AddNextOccurrence => {
                            t.buf.add_next_occurrence();
                        }
                        Command::AddCursorAbove => t.buf.add_caret_above(),
                        _ => t.buf.add_caret_below(),
                    }
                    t.reveal = true;
                    self.focus = Focus::Editor;
                }
            }
            Command::SearchInFiles => self.open_palette(Mode::Search, ctx),
            Command::ToggleSplit => self.editor_grid = !self.editor_grid,
            Command::ToggleSidebar => self.sidebar_key(ctx),
            Command::GoToLine => self.open_palette(Mode::Line, ctx),
            Command::ToggleComment
            | Command::MoveLineUp
            | Command::MoveLineDown
            | Command::DuplicateLineUp
            | Command::DuplicateLineDown
            | Command::DeleteLine
            | Command::InsertLineBelow
            | Command::InsertLineAbove
            | Command::IndentLines
            | Command::OutdentLines => self.line_command(command),
            Command::GoToTab(n) => {
                // ⌘9 is the last tab, as in browsers.
                let i = if n == 9 {
                    self.tabs.len().saturating_sub(1)
                } else {
                    n as usize - 1
                };
                if i < self.tabs.len() {
                    self.select_tab(i);
                    self.focus = Focus::Editor;
                }
            }
            Command::SwitchFocus => self.switch_focus(ctx),
            Command::ClearTerminal => {
                if let Some(t) = self.terminals.get_mut(self.active_terminal)
                    && !t.pane.clear()
                {
                    self.notify("Not while a full-screen program is running");
                }
            }
            Command::CopyTerminalOutput => {
                let text = self.terminals.get_mut(self.active_terminal).and_then(|t| t.pane.last_output());
                match text {
                    Some(text) if !text.is_empty() => {
                        let n = text.lines().count();
                        self.copy(ctx, text);
                        self.notify(format!("Copied the last command's output ({n} line{})", if n == 1 { "" } else { "s" }));
                    }
                    _ => self.notify("The last command printed nothing"),
                }
            }
            Command::FocusSidebar => self.focus_sidebar(),
            Command::CollapseFolders => self.collapse_folders(),
            Command::ZoomIn => self.zoom(Some(1.0), ctx),
            Command::ZoomOut => self.zoom(Some(-1.0), ctx),
            Command::ResetZoom => self.zoom(None, ctx),
            Command::FullScreen => {
                let on = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!on));
            }
            Command::MaximizeWindow => toggle_maximized(ctx),
            Command::MinimizeWindow => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Command::ToggleWordWrap => {
                let wrap = !self.config.settings.word_wrap;
                self.config.settings.word_wrap = wrap;
                self.save_choice_quietly("wordWrap", wrap, ctx);
                self.notify(if wrap {
                    "Word wrap on"
                } else {
                    "Word wrap off"
                });
            }
            Command::ToggleTerminal => self.toggle_terminal(ctx),
            Command::NewTerminal => self.new_terminal(ctx),
            Command::CloseTerminal => {
                let ids = self.terminals.get(self.active_terminal).map(|t| t.id);
                self.begin_close_terminals(ids.into_iter().collect());
            }
            Command::ToggleTerminalGrid => {
                self.terminal_grid = !self.terminal_grid;
                self.show_terminals(ctx);
            }
            Command::MaximizeTerminal => {
                // Hidden, or none yet: open it maximized. Else maximize / restore.
                let max = !(self.terminal_max && self.terminal_open);
                self.show_terminals(ctx);
                self.terminal_max = max && !self.terminals.is_empty();
            }
            Command::SwitchTerminal => self.open_palette(Mode::Terminals, ctx),
            Command::OpenTerminalLink => self.open_palette(Mode::Links, ctx),
            Command::ToggleRemoteAccess => self.toggle_remote(ctx),
            Command::ShowRemoteAddress => {
                self.close_palette(true, ctx);
                self.ask = Some(Ask::RemoteAddress);
            }
            Command::SwitchWindow => self.remote_windows = true,
            Command::JumpTerminal(n) => {
                let i = n as usize - 1;
                if i < self.terminals.len() {
                    self.active_terminal = i;
                    self.show_terminals(ctx);
                }
            }
            Command::NewFile
            | Command::NewFolder
            | Command::RenameFile
            | Command::DeleteFile
            | Command::RevealInFinder
            | Command::OpenInDefaultApp
            | Command::OpenInTerminal
            | Command::CopyPath
            | Command::CopyRelativePath
            | Command::ToggleBuildFolders => self.run_file_command(command, ctx),
            Command::Settings => self.open_palette(Mode::Settings, ctx),
            Command::Keybindings => self.open_palette(Mode::Keybindings, ctx),
            Command::Todos => self.open_todos(ctx),
            Command::ClipboardHistory => self.open_palette(Mode::Clipboard, ctx),
            Command::Help => self.help = !self.help,
            Command::About => self.ask = Some(Ask::About),
            Command::CheckForUpdates => self.run_check_for_updates(ctx),
            Command::OpenNotificationSettings => self.open_notification_settings(),
            Command::OpenProjectSettings => self.open_project_settings(ctx),
            // The config files open right here; saving applies them.
            Command::OpenSettings | Command::OpenKeybindings => {
                let path = if command == Command::OpenSettings {
                    config::settings_path()
                } else {
                    config::keybindings_path()
                };
                self.close_palette(true, ctx);
                self.open_file(&path);
                self.focus = Focus::Editor;
            }
            Command::OpenConfigFolder => self.open_external(&config::dir()),
        }
    }

    /// Whether a command makes sense right now (e.g. Go to File needs a folder).
    pub fn available(&self, command: Command) -> bool {
        match command {
            Command::TogglePinTab => {
                if self.terminal_focused() { !self.terminals.is_empty() } else { self.active.is_some() }
            }
            Command::GoToFile
            | Command::CloseFolder
            | Command::NewFile
            | Command::NewFolder
            | Command::ToggleBuildFolders
            | Command::OpenProjectSettings
            | Command::ToggleSidebar
            | Command::FocusSidebar => self.project.is_some(),
            Command::RenameFile
            | Command::RevealInFinder
            | Command::OpenInTerminal
            | Command::CopyPath
            | Command::CopyRelativePath => self.file_target().is_some(),
            // A file, and on this computer: a browser's copy would open it
            // on the computer Roxaruu runs on, out of sight.
            Command::OpenInDefaultApp => !self.remote_pass && self.file_target().is_some_and(|p| p.is_file()),
            // Only with a list open: in the editor ⌘⌫ deletes to the line start.
            Command::DeleteFile => {
                (self.palette.is_some() || self.focus == Focus::Sidebar)
                    && self.file_target().is_some()
            }
            Command::CloseTab | Command::CloseAllTabs | Command::Copy => {
                self.active.is_some() || self.terminal_focused()
            }
            Command::Save
            | Command::Undo
            | Command::Redo
            | Command::Cut
            | Command::Paste
            | Command::SelectAll => self.editable() || self.terminal_focused(),
            Command::Find => self.active.is_some(),
            Command::Replace
            | Command::AddNextOccurrence
            | Command::AddCursorAbove
            | Command::AddCursorBelow => self.editable(),
            Command::EditWholeFile => self.active_tab().is_some_and(|t| t.huge.is_some()),
            // Line edits are the editor's: in a terminal ⌥↑ and ⌘↵ stay the shell's.
            Command::GoToLine => self.active.is_some() && self.focus == Focus::Editor,
            Command::ToggleComment
            | Command::MoveLineUp
            | Command::MoveLineDown
            | Command::DuplicateLineUp
            | Command::DuplicateLineDown
            | Command::DeleteLine
            | Command::InsertLineBelow
            | Command::InsertLineAbove
            | Command::IndentLines
            | Command::OutdentLines => self.editable() && self.focus == Focus::Editor,
            Command::GoToTab(n) => {
                n == 9 && !self.tabs.is_empty() || (n as usize) <= self.tabs.len()
            }
            Command::ClearTerminal => self.terminal_focused(),
            Command::CopyTerminalOutput => self.terminal_focused() && self.terminal().is_some_and(|t| t.pane.has_run()),
            Command::SourceControl => self.project.is_some(),
            Command::GitStage => self.can_stage(),
            Command::GitDiscard => self.can_discard(),
            Command::GitOpenFile => self.changed_file_target(),
            // The repository you're on (the Changes view's row, the file in front's).
            Command::GitCommit => self.current_repo().and_then(|r| self.project.as_ref()?.repos.get(r)).is_some_and(|r| !r.files.is_empty()),
            Command::GitPush | Command::GitPull => self.current_repo().is_some(),
            // Only there: elsewhere ⌥A types (å on a Mac).
            Command::GitStageAll => self.changes_focused() && self.project.as_ref().is_some_and(|p| p.repos.iter().any(|r| r.files.iter().any(|f| f.unstaged.is_some()))),
            // "All": more than one repository to do it in.
            Command::GitCommitAll => self.project.as_ref().is_some_and(|p| p.repos.len() > 1 && p.repos.iter().any(|r| !r.files.is_empty())),
            Command::GitPushAll | Command::GitPullAll => self.project.as_ref().is_some_and(|p| p.repos.len() > 1),
            Command::SwitchFocus => {
                self.active.is_some() || !self.terminals.is_empty() || self.project.is_some()
            }
            Command::SearchInFiles => self.project.is_some(),
            Command::ToggleSplit => self.tabs.len() > 1,
            Command::SaveAll => self.tabs.iter().any(|t| t.buf.modified),
            Command::CloseSavedTabs => self.tabs.iter().any(|t| !t.buf.modified),
            // Its key shows on the tree's header whatever has the keyboard,
            // so it works from anywhere (its key clashes with no editor key).
            Command::CollapseFolders => self.project.is_some(),
            Command::NextTab
            | Command::PrevTab
            | Command::MoveTabLeft
            | Command::MoveTabRight
            | Command::CloseOtherTabs
            | Command::CloseTabsToRight
            | Command::CloseTabsToLeft => {
                self.tabs.len() > 1 || (self.terminal_focused() && self.terminals.len() > 1)
            }
            Command::CloseTerminal | Command::ToggleTerminalGrid => {
                !self.terminals.is_empty()
            }
            Command::SwitchTerminal => self.terminals.len() > 1,
            Command::OpenTerminalLink => self.terminal_open && self.terminal().is_some(),
            Command::JumpTerminal(n) => (n as usize) <= self.terminals.len(),
            Command::ReopenTab => !self.closed.is_empty(),
            // The page's window list: nothing to switch to on the Mac.
            Command::SwitchWindow => self.remote_pass,
            // The window's own: a browser's copy has none to move.
            Command::MinimizeWindow | Command::MaximizeWindow => !self.remote_pass,
            _ => true,
        }
    }

    // ---- folders & files ------------------------------------------------

    /// From a browser (its copy of this window): it should show the window
    /// that has `dir`, as the computer would bring that window to the front.
    fn follow_in_browser(&mut self, dir: &Path) {
        if self.remote_pass {
            self.remote_follow = Some(crate::ui::tilde(&dir.to_string_lossy()));
        }
    }

    /// Open `dir` here, or — if this window already has a project, or its
    /// terminals are in use (opened before any folder: they're yours, not
    /// the folder's) — in a new window, like Oxru: one project per window.
    pub fn open_folder(&mut self, dir: PathBuf, ctx: &egui::Context) {
        // One spelling per folder: `~/code/` and `~/code` are the same one.
        let dir: PathBuf = dir.components().collect();
        tracing::info!(dir = %dir.display(), "open folder");
        if self.project.as_ref().is_some_and(|p| p.root == dir) {
            return self.notify(format!("{} is already open in this window", display_name(&dir)));
        }
        if let Some(pid) = self.open_elsewhere(&dir) {
            // One window per folder: go to the one that has it.
            let shown = crate::sys::activate(pid);
            self.follow_in_browser(&dir);
            return self.notify(format!(
                "{} is already open in another window{}",
                display_name(&dir),
                if shown { ", switched to it" } else { "" }
            ));
        }
        if self.project.is_some() || !self.terminals.is_empty() {
            let why = if self.project.is_some() { "" } else { " (this one keeps its terminals)" };
            return self.folder_in_new_window(&dir, why);
        }
        self.folder_here(dir, ctx);
    }

    /// `dir` in a window of its own; `why` ends the note.
    fn folder_in_new_window(&mut self, dir: &Path, why: &str) {
        match files::spawn_window(dir) {
            Ok(_) => {
                self.follow_in_browser(dir);
                self.notify(format!("Opened {} in a new window{why}", display_name(dir)));
            }
            Err(e) => self.warn(format!("Couldn't open a new window: {e}")),
        }
    }

    /// `dir` as this window's folder (none open now; terminals stay).
    fn folder_here(&mut self, dir: PathBuf, ctx: &egui::Context) {
        crate::recent::record(std::slice::from_ref(&dir));
        crate::instances::register(&dir);
        self.project = Some(Project {
            name: display_name(&dir),
            tree: FileTree::new(&dir),
            root: dir,
            files: Vec::new(),
            matcher: None,
            indexing: None,
            git: Default::default(),
            repos: Vec::new(),
            git_job: None,
            git_stale: true,
            git_repos: None,
            git_at: None,
            git_found: false,
        });
        self.index(ctx);
        self.reload_config(ctx); // with the project's own settings
        self.tabs.clear();
        self.active = None;
        self.closed.clear();
        self.mru.clear();
        self.welcome_selected = 0;
        // Bring back the tabs this project had open last time.
        let root = self.project.as_ref().map(|p| p.root.clone());
        if let Some(root) = root {
            let (files, active) = crate::session::load(&root);
            // Each is read when it's first shown, not now: fifty restored
            // tabs cost what the one in front does.
            self.tabs.extend(files.iter().map(|(f, pinned)| {
                let mut t = Tab::lazy(f);
                t.pinned = *pinned;
                t
            }));
            if !self.tabs.is_empty() {
                self.active = Some(active.min(self.tabs.len() - 1));
            }
        }
        // A folder always opens the same way, whatever the last one left:
        // the tree, and a terminal below at its first size.
        if !self.config.settings.sidebar && !self.view().compact {
            self.save_choice_quietly("sidebar", true, ctx);
        }
        self.terminal_max = false;
        self.terminal_layout += 1;
        if self.terminals.is_empty() {
            let focus = self.focus;
            self.new_terminal_in(None, ctx);
            self.focus = focus;
        } else {
            self.terminal_open = true;
        }
    }

    /// Folders you chose (the Open list, dropped on the window): those open
    /// already are gone to; the rest open here if this window has nothing
    /// open, else it asks: here (in place of its folder, beside its
    /// terminals), or in new windows (`Ask::OpenFolders`).
    pub fn open_folders(&mut self, dirs: Vec<PathBuf>, ctx: &egui::Context) {
        let mut fresh: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            let dir: PathBuf = dir.components().collect();
            let open = self.project.as_ref().is_some_and(|p| p.root == dir) || self.open_elsewhere(&dir).is_some();
            if open {
                self.open_folder(dir, ctx); // says so, or goes there
            } else if !fresh.contains(&dir) {
                fresh.push(dir);
            }
        }
        if fresh.is_empty() {
            return;
        }
        if self.project.is_some() || !self.terminals.is_empty() {
            self.ask = Some(Ask::OpenFolders { dirs: fresh });
        } else {
            self.open_each(fresh, ctx);
        }
    }

    /// `Ask::OpenFolders`' answer: here (the first, in place of this
    /// window's folder once its tabs are closed — unsaved ones asked about —
    /// the others each in a new window), or all in new windows.
    pub(crate) fn open_folders_answered(&mut self, dirs: Vec<PathBuf>, here: bool, ctx: &egui::Context) {
        if !here {
            for dir in &dirs {
                self.folder_in_new_window(dir, "");
            }
            if dirs.len() > 1 {
                self.toasts.retain(|t| t.error);
                self.notify(format!("Opened {} folders, each in its own window", dirs.len()));
            }
        } else if self.project.is_some() {
            self.replace_with = dirs;
            self.begin_close(self.tab_paths(), After::Replace, ctx);
        } else {
            self.open_each(dirs, ctx);
        }
    }

    /// The first of `dirs` here, the rest each in a window of its own. One
    /// toast for them all.
    fn open_each(&mut self, dirs: Vec<PathBuf>, ctx: &egui::Context) {
        let n = dirs.len();
        let mut dirs = dirs.into_iter();
        let Some(first) = dirs.next() else { return };
        let before: Vec<String> = self.toasts.iter().map(|t| t.msg.clone()).collect();
        self.folder_here(first, ctx);
        for dir in dirs {
            self.folder_in_new_window(&dir, "");
        }
        if n > 1 {
            // What went wrong still shows.
            self.toasts.retain(|t| t.error || before.contains(&t.msg));
            self.notify(format!("Opened {n} folders: the first here, the others each in its own window"));
        }
    }

    /// (Re)list the project's files for search, in the background.
    fn index(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.project else { return };
        let (tx, rx) = channel();
        let (root, build, ctx2) = (p.root.clone(), self.config.settings.search_ignored, ctx.clone());
        std::thread::spawn(move || {
            let files = fstree::with_folders(
                fstree::collect_files_marked(&root, build)
                    .into_iter()
                    .map(|(path, ignored)| (rel_path(&root, &path), ignored))
                    .collect(),
            );
            let names: Vec<String> = files.iter().map(|(f, _, _)| f.clone()).collect();
            let matcher = FileMatcher::new(&names);
            drop(names);
            let _ = tx.send((files, matcher));
            ctx2.request_repaint();
        });
        p.indexing = Some(rx);
        p.git_stale = true; // files came or went: the repositories too, maybe
        p.git_repos = None;
    }

    /// Ask git again soon (a save, a command finished): the tree's colours.
    pub(crate) fn git_changed(&mut self) {
        if let Some(p) = &mut self.project {
            p.git_stale = true;
        }
    }

    /// `git_changed`, looking for the repositories again too (one cloned
    /// into the folder since).
    pub(crate) fn git_rescan(&mut self) {
        if let Some(p) = &mut self.project {
            p.git_stale = true;
            p.git_repos = None;
        }
    }

    /// Git's answer in, or asked again: when something may have changed it
    /// (`git_stale`), on coming back to the window, and every `GIT_EVERY`
    /// while you're at it (an agent in a terminal edits behind its back).
    /// One ask at a time; none while there's no repository but on those.
    fn poll_git(&mut self, ctx: &egui::Context) {
        const GIT_EVERY: Duration = Duration::from_secs(10);
        let focused = !self.remote_pass && ctx.input(|i| i.focused);
        let back = focused && !std::mem::replace(&mut self.was_focused, focused);
        if back {
            self.welcome_recent = None;
        }
        if self.remote_pass {
            self.was_focused = true;
        }
        let Some(p) = &mut self.project else { return };
        if let Some((repos, answer)) = p.git_job.as_ref().and_then(|rx| rx.try_recv().ok()) {
            p.git_job = None;
            p.git_found = answer.is_some();
            let answer = answer.unwrap_or_default();
            p.git = answer.tree;
            p.repos = answer.repos;
            p.git_repos = Some(repos);
        }
        let due = focused && p.git_found && p.git_at.is_none_or(|t| t.elapsed() >= GIT_EVERY);
        if p.git_job.is_some() || !(p.git_stale || back || due) {
            if focused && p.git_found {
                ctx.request_repaint_after(GIT_EVERY);
            }
            return;
        }
        p.git_stale = false;
        p.git_at = Some(Instant::now());
        // The answer shows at the next frame (a second at most, idle): no
        // frame of its own, so asking costs nothing on screen.
        let (tx, rx) = channel();
        let (root, repos) = (p.root.clone(), p.git_repos.clone());
        std::thread::spawn(move || {
            let repos = repos.unwrap_or_else(|| crate::git::find(&root));
            let answer = crate::git::status(&root, &repos);
            let _ = tx.send((repos, answer));
        });
        p.git_job = Some(rx);
    }

    /// Pick up the background file walk when it finishes.
    fn poll_index(&mut self) {
        let Some(p) = &mut self.project else { return };
        let Some((files, matcher)) = p.indexing.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        p.indexing = None;
        p.files = files;
        p.matcher = Some(matcher);
        self.sidebar.hits_for = None; // search the new list
    }

    /// Open `path` in a tab, or switch to it if it's already open.
    pub fn open_file(&mut self, path: &Path) {
        // An image, a PDF, a program: its own app opens it (here, on this
        // computer; a browser's copy can't show it).
        if crate::buffer::is_binary_file(path) {
            if self.remote_pass {
                return self.warn(crate::buffer::unsupported_reason(path).unwrap_or_default());
            }
            tracing::debug!(path = %path.display(), "open in its own app");
            return self.open_external(path);
        }
        self.mru.retain(|p| p != path);
        self.mru.insert(0, path.to_path_buf());
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.path().is_some_and(|p| p == path))
        {
            self.active = Some(i);
            // Already open (from the tree, Go to File): show its tab.
            self.views.iter_mut().for_each(|v| v.tabs.reveal_again());
            self.sidebar.revealed = None;
            return;
        }
        tracing::debug!(path = %path.display(), "open file");
        match Tab::open(path) {
            Ok(mut tab) => {
                recall_place(&mut tab);
                // New tabs open beside the current one.
                let at = self.active.map_or(self.tabs.len(), |a| a + 1).max(self.pinned(false));
                self.tabs.insert(at, tab);
                self.active = Some(at);
                self.save_session();
            }
            Err(e) => self.warn(e),
        }
    }

    /// Read tab `i`'s file if it isn't yet: it's being shown.
    pub(crate) fn load_tab(&mut self, i: usize) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        let fresh = !tab.is_loaded();
        if let Err(e) = tab.ensure_loaded()
            && !tab.gone
        {
            self.warn(e);
        } else if fresh {
            recall_place(tab);
        }
    }

    /// Whether the active tab's text can be edited (a huge file's can't).
    pub(crate) fn editable(&self) -> bool {
        self.active_tab().is_some_and(|t| t.huge.is_none() && t.diff.is_none())
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active?)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active?)
    }

    /// A tab's path relative to the project (or its file name).
    pub fn tab_rel(&self, tab: &Tab) -> String {
        match (tab.path(), &self.project) {
            (Some(path), Some(p)) => rel_path(&p.root, path),
            (Some(path), None) => display_name(path),
            (None, _) => tab.name(),
        }
    }

    fn tab_paths(&self) -> Vec<PathBuf> {
        self.tabs.iter().filter_map(|t| t.path().cloned()).collect()
    }

    /// Remote access on or off, from any window. Turning it on always asks
    /// for a password (kept in memory only, until it's turned off); off stops
    /// the server wherever it runs.
    pub fn toggle_remote(&mut self, ctx: &egui::Context) {
        self.close_palette(true, ctx);
        if self.remote.on {
            self.remote_want = Some(crate::remote::Want::Off);
            self.notify("Remote access off");
        } else {
            self.ask = Some(Ask::RemotePassword { password: String::new() });
        }
    }

    /// Remote access with `password`: then whether to use it only from
    /// browsers (`Ask::RemoteHere`).
    pub fn start_remote(&mut self, password: &str) -> Result<(), String> {
        if password.is_empty() {
            return Err("The password can't be empty".into());
        }
        self.ask = Some(Ask::RemoteHere { password: password.to_string() });
        Ok(())
    }

    /// Serve remote access; `browser_only` minimizes the windows here
    /// meanwhile. Then show where to open it.
    pub fn serve_remote(&mut self, password: String, browser_only: bool) {
        self.remote_want = Some(crate::remote::Want::On(password, browser_only));
        self.ask = Some(Ask::RemoteAddress);
    }

    /// Text size: one step bigger / smaller (`Some(±1)`), or back to the
    /// defaults (`None`). UI and code text move together, saved like any
    /// setting so every window follows.
    fn zoom(&mut self, step: Option<f32>, ctx: &egui::Context) {
        let (now, d) = (&self.config.settings, config::Settings::default());
        let (ui, code) = match step {
            Some(s) => (
                (now.ui_font_size + s).clamp(9.0, 32.0),
                (now.code_font_size + s).clamp(9.0, 40.0),
            ),
            None => (d.ui_font_size, d.code_font_size),
        };
        self.save_choice_quietly("uiFontSize", ui, ctx);
        self.save_choice_quietly("codeFontSize", code, ctx);
        self.notify(format!("Text size {ui} · code {code}"));
    }

    pub fn save_session(&self) {
        if let Some(p) = &self.project {
            let files: Vec<(PathBuf, bool)> =
                self.tabs.iter().filter_map(|t| t.path().map(|p| (p.clone(), t.pinned))).collect();
            crate::session::save(&p.root, &files, self.active.unwrap_or(0));
        }
        remember_places(self.tabs.iter());
    }

    fn save_active(&mut self) {
        let Some(i) = self.active else { return };
        self.save_tab(i);
    }

    fn save_tab(&mut self, i: usize) -> bool {
        let Some(tab) = self.tabs.get_mut(i) else {
            return false;
        };
        let name = tab.name();
        match tab.buf.save() {
            Ok(()) => {
                tab.gone = false; // saved back
                tab.disk_news = None;
                self.notify(format!("Saved {name}"));
                self.git_changed();
                true
            }
            Err(e) => {
                self.warn(format!("Couldn't save {name}: {e:#}"));
                false
            }
        }
    }

    fn save_all(&mut self) {
        let dirty: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| self.tabs.get(i).is_some_and(|t| t.buf.modified))
            .collect();
        let saved = dirty.iter().filter(|&&i| self.save_tab(i)).count();
        if saved == dirty.len() {
            self.notify(format!(
                "Saved {saved} file{}",
                if saved == 1 { "" } else { "s" }
            ));
        }
    }

    pub fn select_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = Some(i);
            // Chosen: in view, even if it already was the active one; and
            // shown in the tree again, even if it's where it was.
            self.views.iter_mut().for_each(|v| v.tabs.reveal_again());
            self.sidebar.revealed = None;
            self.save_session();
        }
    }

    fn cycle_tab(&mut self, delta: isize) {
        let (Some(a), n) = (self.active, self.tabs.len() as isize) else {
            return;
        };
        if n > 0 {
            self.select_tab((a as isize + delta).rem_euclid(n) as usize);
        }
    }

    fn move_tab(&mut self, delta: isize) {
        let Some(a) = self.active else { return };
        let to = a as isize + delta;
        if to >= 0 {
            self.reorder(false, a, to as usize);
        }
    }

    /// Move editor tab (or terminal) `from` to `to`, the active one going
    /// with it: ⇧⌘, / ⇧⌘. or a drag.
    pub fn reorder(&mut self, terminal: bool, from: usize, to: usize) {
        // Pinned ones stay at the left, the rest after them.
        let (pinned, n) = (self.pinned(terminal), if terminal { self.terminals.len() } else { self.tabs.len() });
        let is_pinned = if terminal { self.terminals.get(from).is_some_and(|t| t.pinned) } else { self.tabs.get(from).is_some_and(|t| t.pinned) };
        let to = if is_pinned { to.min(pinned.saturating_sub(1)) } else { to.max(pinned).min(n.saturating_sub(1)) };
        self.move_raw(terminal, from, to);
    }

    /// How many tabs (or terminals) are pinned: the first that many.
    pub(crate) fn pinned(&self, terminal: bool) -> usize {
        if terminal {
            self.terminals.iter().filter(|t| t.pinned).count()
        } else {
            self.tabs.iter().filter(|t| t.pinned).count()
        }
    }

    /// Pin the tab (or terminal) in front, or unpin it: to the end of the
    /// pinned ones, or just after them.
    fn toggle_pin(&mut self, terminal: bool) {
        let i = if terminal { self.active_terminal } else { self.active.unwrap_or(usize::MAX) };
        let flip = |p: &mut bool| {
            *p = !*p;
        };
        if terminal {
            let Some(t) = self.terminals.get_mut(i) else { return };
            flip(&mut t.pinned);
        } else {
            let Some(t) = self.tabs.get_mut(i) else { return };
            flip(&mut t.pinned);
        }
        let others = if terminal {
            self.terminals.iter().enumerate().filter(|(j, t)| *j != i && t.pinned).count()
        } else {
            self.tabs.iter().enumerate().filter(|(j, t)| *j != i && t.pinned).count()
        };
        self.move_raw(terminal, i, others);
        if !terminal {
            self.save_session();
        }
    }

    fn move_raw(&mut self, terminal: bool, from: usize, to: usize) {
        use crate::ui::tabstrip::moved_index;
        if terminal {
            if from < self.terminals.len() && to < self.terminals.len() {
                let t = self.terminals.remove(from);
                self.terminals.insert(to, t);
                self.active_terminal = moved_index(self.active_terminal, from, to);
            }
        } else if from < self.tabs.len() && to < self.tabs.len() {
            let t = self.tabs.remove(from);
            self.tabs.insert(to, t);
            self.active = self.active.map(|a| moved_index(a, from, to));
            self.save_session();
        }
    }

    /// What ⌘V pastes: files copied in Finder as paths, else the text.
    fn paste_text(&self) -> Option<String> {
        self.clipboard_paths().or_else(|| {
            arboard::Clipboard::new()
                .and_then(|mut c| c.get_text())
                .ok()
        })
    }

    fn edit_command(&mut self, command: Command, ctx: &egui::Context) {
        let pasted = if command == Command::Paste {
            self.paste_text()
        } else {
            None
        };
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        match command {
            Command::Undo => {
                tab.buf.undo();
            }
            Command::Redo => {
                tab.buf.redo();
            }
            Command::SelectAll => tab.buf.select_all(),
            Command::Copy => {
                let text = tab.copy_text();
                return self.copy(ctx, text);
            }
            Command::Cut => {
                let text = tab.cut();
                tab.reveal = true;
                return self.copy(ctx, text);
            }
            Command::Paste => match pasted {
                Some(text) => tab.paste(&text),
                None => return self.notify("Nothing to paste"),
            },
            _ => return,
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.reveal = true;
        }
    }

    /// Open the find bar on the active tab (with the replace field if asked).
    fn open_find(&mut self, replace: bool) {
        let Some(i) = self.active else { return };
        let Some(tab) = self.tabs.get_mut(i) else { return };
        let previous = self.find.take();
        let mut f = Find::open(&tab.buf, i, replace, previous);
        f.jump_from_origin(&mut tab.buf);
        tab.reveal = true;
        if let Some(h) = &mut tab.huge {
            // Read-only: find, no replace; it searches the file on disk.
            f.replace_open = false;
            f.in_replace = false;
            if !f.query.is_empty() {
                let _ = h.find(&f.query, f.case, None);
            }
        }
        self.find = Some(f);
        self.focus = Focus::Find;
    }

    pub fn close_find(&mut self) {
        self.find = None;
        if self.focus == Focus::Find {
            self.focus = Focus::Editor;
        }
    }

    /// Whether a dialog (save changes? / changed on disk?) owns the keyboard.
    /// The layout of the view being drawn (or the window's, between frames).
    pub(crate) fn view(&self) -> View {
        let [window, remote] = self.views;
        if self.remote_pass { remote } else { window }
    }

    pub(crate) fn view_mut(&mut self) -> &mut View {
        let [window, remote] = &mut self.views;
        if self.remote_pass { remote } else { window }
    }

    pub fn modal_open(&self) -> bool {
        self.prompt.is_some() || self.conflict.is_some() || self.ask.is_some() || self.help
    }

    pub fn terminal_focused(&self) -> bool {
        self.focus == Focus::Terminal && self.terminal_open && !self.terminals.is_empty()
    }

    /// The active terminal.
    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminals.get(self.active_terminal)
    }

    pub fn terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.terminals.get_mut(self.active_terminal)
    }

    /// Show the panel and give the active terminal the keyboard.
    fn show_terminals(&mut self, ctx: &egui::Context) {
        if self.terminals.is_empty() {
            return self.new_terminal(ctx);
        }
        self.terminal_open = true;
        self.focus = Focus::Terminal;
        self.views.iter_mut().for_each(|v| v.terms.reveal_again());
    }

    /// In the find bar, ⌥⌘C and ⌥⌘W toggle match case and whole word (as in
    /// VS Code), ahead of whatever else those keys do.
    fn find_toggle_keys(&mut self, ctx: &egui::Context) {
        if self.focus != Focus::Find || self.palette.is_some() {
            return;
        }
        let (Some(f), Some(i)) = (self.find.as_mut(), self.active) else {
            return;
        };
        let m = egui::Modifiers {
            alt: true,
            mac_cmd: crate::keymap::mac_keys(),
            command: true,
            ctrl: !crate::keymap::mac_keys(),
            ..Default::default()
        };
        let (case, word) = ctx.input_mut(|inp| {
            (
                inp.consume_key(m, egui::Key::C),
                inp.consume_key(m, egui::Key::W),
            )
        });
        if !(case || word) {
            return;
        }
        f.case ^= case;
        f.word ^= word;
        if let Some(t) = self.tabs.get_mut(i) {
            f.refresh(&t.buf, i);
            f.jump_from_origin(&mut t.buf);
            t.reveal = true;
        }
    }

    /// ⌘J: between the editor and the terminals, leaving both on screen
    /// (opening a terminal the first time).
    fn switch_focus(&mut self, ctx: &egui::Context) {
        if self.terminal_focused() {
            self.focus = Focus::Editor; // the file, or the welcome card's actions
        } else {
            self.show_terminals(ctx);
        }
    }

    /// The whole-line edits, on the active tab.
    fn line_command(&mut self, command: Command) {
        let Some(t) = self.active_tab_mut() else {
            return;
        };
        let b = &mut t.buf;
        match command {
            Command::ToggleComment => {
                let token = match b.lang.filter(|l| l.id != crate::syntax::GENERIC.id) {
                    Some(l) => l.comment,
                    // No grammar here: guess from the extension.
                    None => match b
                        .path
                        .as_ref()
                        .and_then(|p| p.extension())
                        .and_then(|e| e.to_str())
                    {
                        Some("conf" | "ini" | "env" | "r" | "pl" | "mk") => Some("#"),
                        Some("kt" | "dart" | "scala" | "zig") => Some("//"),
                        Some("hs") => Some("--"),
                        _ => None,
                    },
                };
                match token {
                    Some(token) => {
                        b.toggle_comment(token);
                    }
                    None => return self.notify("This file type has no line comments"),
                }
            }
            Command::MoveLineUp => b.move_lines(false),
            Command::MoveLineDown => b.move_lines(true),
            Command::DuplicateLineUp => b.duplicate_lines(false),
            Command::DuplicateLineDown => b.duplicate_lines(true),
            Command::DeleteLine => b.delete_lines(),
            Command::InsertLineBelow => b.open_line(true),
            Command::InsertLineAbove => b.open_line(false),
            Command::IndentLines => b.indent_selection(),
            Command::OutdentLines => b.outdent_selection(),
            _ => return,
        }
        t.reveal = true;
        t.edited();
    }

    /// Go to Line's choices: the line typed (`42` or `42:7`), with its text.
    fn line_items(&self, query: &str) -> Vec<Item> {
        let Some(t) = self.active_tab() else {
            return Vec::new();
        };
        let n = t.line_count();
        let (l, c) = query.trim().split_once(':').unwrap_or((query.trim(), ""));
        let Ok(line) = l.trim().parse::<usize>() else {
            let (row, col) = t.caret();
            let row = row + 1;
            return vec![
                Item::new(
                    icon::GO_TO_LINE,
                    format!("Type a line number, 1 to {n}"),
                    Pick::Line(row - 1, col),
                )
                .detail(format!("now on line {row}")),
            ];
        };
        let line = line.clamp(1, n.max(1));
        let col = c.trim().parse::<usize>().unwrap_or(1).max(1);
        let text: String = t
            .line_text(line - 1)
            .trim()
            .chars()
            .take(80)
            .collect();
        let label = if c.is_empty() {
            format!("Go to line {line}")
        } else {
            format!("Go to line {line}, column {col}")
        };
        vec![Item::new(icon::GO_TO_LINE, label, Pick::Line(line - 1, col - 1)).detail(text)]
    }

    /// Put the caret at `line`, `col` (from 0) and show it.
    fn go_to(&mut self, line: usize, col: usize) {
        if let Some(t) = self.active_tab_mut() {
            t.go_to(line, col..col);
        }
        self.focus = Focus::Editor;
    }

    /// Show and focus the terminals (starting one the first time); if they
    /// already have focus, hide them.
    fn toggle_terminal(&mut self, ctx: &egui::Context) {
        if self.terminal_focused() {
            self.terminal_open = false;
            self.focus = Focus::Editor;
        } else {
            self.show_terminals(ctx);
        }
    }

    /// Notice files changed by something else (a `git checkout`, a code generator,
    /// vim in a terminal): a clean tab just reloads. One with unsaved edits,
    /// or whose file was deleted, gets a question — when that tab is in front
    /// and the editor has the keyboard, never by jumping to it. A deleted
    /// file keeps its text, struck through and marked unsaved, so saving
    /// brings it back. A cheap `stat` per tab, once a second.
    /// Whether you've just come back to this window: it got the keyboard, or
    /// can be seen again (back from another Space, un-minimized).
    fn came_back(&mut self, ctx: &egui::Context) -> bool {
        let (focused, hidden) = ctx.input(|i| {
            let v = i.viewport();
            (
                i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true))),
                v.occluded.unwrap_or(false) || v.minimized.unwrap_or(false),
            )
        });
        let seen_again = self.was_hidden && !hidden;
        self.was_hidden = hidden;
        focused || seen_again
    }

    fn poll_file_changes(&mut self) {
        use crate::buffer::DiskStatus;
        if !self.modal_open()
            && self.palette.is_none()
            && self.focus == Focus::Editor
            && let Some(t) = self.active_tab()
            && t.disk_news.is_some()
        {
            self.conflict = t.path().cloned();
        }
        if self.prompt.is_some()
            || self.conflict.is_some()
            || self.last_file_check.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.last_file_check = Instant::now();
        self.file_checks += 1;
        for i in 0..self.tabs.len() {
            let Some(tab) = self.tabs.get_mut(i) else { continue };
            let name = tab.name();
            if !tab.is_loaded() {
                // Not read yet: whether it's still there is all that matters.
                let there = tab.path().is_some_and(|p| p.exists());
                if tab.gone == there {
                    tab.gone = !there;
                    tab.disk_news = (!there).then_some(DiskStatus::Deleted);
                    if !there {
                        self.warn(format!("{name} was deleted on disk"));
                    }
                }
                continue;
            }
            if tab.gone {
                // Back again (git checkout, put back from the Trash)?
                if tab.path().is_some_and(|p| p.exists()) {
                    tab.gone = false;
                    tab.disk_news = None;
                    tab.buf.rebaseline_disk();
                    if !tab.buf.is_clean() {
                        tab.disk_news = Some(DiskStatus::Modified);
                    } else if let Err(e) = tab.reload() {
                        self.warn(format!("Couldn't reload {name}: {e:#}"));
                    }
                }
                continue;
            }
            if tab.disk_news.is_some() {
                continue; // still waiting for an answer
            }
            match tab.buf.disk_status() {
                DiskStatus::Unchanged => {}
                DiskStatus::Deleted => {
                    tab.gone = true;
                    // Its text is ours to save back (a huge file's isn't held).
                    tab.buf.modified = tab.buf.is_loaded();
                    tab.buf.forget_disk(); // report once
                    tab.disk_news = Some(DiskStatus::Deleted);
                    self.warn(format!("{name} was deleted on disk"));
                }
                DiskStatus::Modified if tab.buf.modified => {
                    tab.disk_news = Some(DiskStatus::Modified);
                }
                DiskStatus::Modified => match tab.reload() {
                    Ok(()) => self.notify(format!("Reloaded {name} (changed on disk)")),
                    Err(e) => self.warn(format!("Couldn't reload {name}: {e:#}")),
                },
            }
        }
    }

    /// The answer to a change on disk. Changed: take the disk version
    /// (`first`), or keep ours and stop asking about that change. Deleted:
    /// close the tab (`first`), or keep it open to save back.
    pub fn answer_conflict(&mut self, first: bool) {
        use crate::buffer::DiskStatus;
        let Some(path) = self.conflict.take() else {
            return;
        };
        let Some(i) = self.tabs.iter().position(|t| t.path() == Some(&path)) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(i) else { return };
        let news = tab.disk_news.take();
        if news == Some(DiskStatus::Deleted) {
            if first {
                self.close_tab_now(i);
            }
            return;
        }
        if !first {
            return tab.buf.rebaseline_disk();
        }
        let name = tab.name();
        match tab.reload() {
            Ok(()) => self.notify(format!("Reloaded {name}")),
            Err(e) => self.warn(format!("Couldn't reload {name}: {e:#}")),
        }
    }

    /// Open what scripts asked for: each queued `do script` / `open -a
    /// Terminal` from a shell inside the app becomes a terminal tab here
    /// instead of an OS window (Oxru's terminal bridge). Only whole lines are
    /// taken, so a half-written request waits for the next frame.
    fn poll_terminal_requests(&mut self, ctx: &egui::Context) {
        use std::io::{Read, Seek, SeekFrom};
        if self.terminals.is_empty() {
            return; // only shells we started can queue anything
        }
        let Some(path) = crate::termbridge::request_file() else {
            return;
        };
        let changed = self.requests_changed.get_or_insert_with(|| terminal::watch_requests(ctx));
        if !changed.swap(false, Ordering::Relaxed) {
            return;
        }
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if len < self.request_offset {
            self.request_offset = 0; // truncated
        }
        if len == self.request_offset {
            return;
        }
        let mut buf = String::new();
        let read = std::fs::File::open(&path).and_then(|mut f| {
            f.seek(SeekFrom::Start(self.request_offset))
                .and_then(|_| f.read_to_string(&mut buf))
        });
        let Some(end) = read.ok().and_then(|_| buf.rfind('\n')) else {
            return;
        };
        self.request_offset += (end + 1) as u64;
        for cmd in buf[..=end].lines().map(str::trim).filter(|l| !l.is_empty()) {
            if cmd == crate::termbridge::SHELL_REQUEST {
                self.new_terminal(ctx); // asked for a plain shell
            } else {
                self.open_terminal_with_command(cmd, ctx);
            }
        }
        // Calls the shims let through (dialogs, other apps) aren't ours;
        // drain the record so it can't grow without bound.
        if let Some(p) = crate::termbridge::passthrough_file() {
            let _ = std::fs::write(p, b"");
        }
    }

    /// A new terminal running `cmd`, started in the folder the command `cd`s
    /// into (so the tab is named right from the first frame).
    fn open_terminal_with_command(&mut self, cmd: &str, ctx: &egui::Context) {
        let before = self.terminals.len();
        let target = terminal::cd_target(cmd).filter(|p| p.is_dir());
        self.new_terminal_in(target, ctx);
        if self.terminals.len() == before {
            return; // at the cap — never type into some other terminal
        }
        if let Some(t) = self.terminal_mut() {
            t.pane.folder = terminal::short_title(cmd);
            t.pane.send_input(format!("{cmd}\n").as_bytes());
        }
        self.notify(format!("Opened terminal: {}", terminal::short_title(cmd)));
    }

    /// A new shell beside the current one, in the current one's folder (or
    /// the project's).
    pub fn new_terminal(&mut self, ctx: &egui::Context) {
        self.new_terminal_in(None, ctx);
    }

    fn new_terminal_in(&mut self, cwd: Option<PathBuf>, ctx: &egui::Context) {
        const MAX_TERMINALS: usize = 20;
        if self.terminals.len() >= MAX_TERMINALS {
            return self.warn(format!(
                "Too many terminals open (max {MAX_TERMINALS}) — close some first"
            ));
        }
        if crate::sys::no_terminal_shell() {
            self.ask = Some(Ask::NeedShell);
            return;
        }
        let cwd = cwd
            .or_else(|| self.terminal().and_then(|t| t.pane.current_dir()))
            .or_else(|| self.project.as_ref().map(|p| p.root.clone()))
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        match Terminal::spawn(&cwd, 24, 80, ctx, self.throttle.clone()) {
            Ok(t) => {
                let at = if self.terminals.is_empty() {
                    0
                } else {
                    (self.active_terminal + 1).max(self.pinned(true))
                };
                self.terminals.insert(at, t);
                self.active_terminal = at;
                self.terminal_open = true;
                self.focus = Focus::Terminal;
            }
            Err(e) => self.warn(format!("Couldn't start a shell: {e:#}")),
        }
    }

    /// Close (kill) terminal `i`.
    /// Close these terminals; first, if one still runs something, ask
    /// whether to stop it (unless `confirmCloseTerminal` is off).
    pub(crate) fn begin_close_terminals(&mut self, ids: Vec<u64>) {
        for t in self.terminals.iter_mut().filter(|t| ids.contains(&t.id)) {
            t.pane.refresh_foreground_now();
        }
        let running = self.terminals.iter().any(|t| ids.contains(&t.id) && t.pane.is_running());
        if running && self.config.settings.confirm_close_terminal {
            self.ask = Some(Ask::CloseTerminals { ids });
        } else {
            self.close_terminals(&ids);
        }
    }

    /// Those still here (one may have exited meanwhile).
    pub(crate) fn close_terminals(&mut self, ids: &[u64]) {
        while let Some(i) = self.terminals.iter().position(|t| ids.contains(&t.id)) {
            self.close_terminal(i);
        }
    }

    /// "A terminal is still running (…): it will be stopped." for these.
    pub(crate) fn running_line(&self, ids: Option<&[u64]>) -> Option<String> {
        let running: Vec<String> = self
            .terminals
            .iter()
            .filter(|t| t.pane.is_running() && ids.is_none_or(|ids| ids.contains(&t.id)))
            .map(|t| t.pane.display_name())
            .collect();
        match running.as_slice() {
            [] => None,
            [one] => Some(format!("A terminal is still running ({one}): it will be stopped.")),
            many => Some(format!("{} terminals are still running ({}): they will be stopped.", many.len(), running.join(", "))),
        }
    }

    pub fn close_terminal(&mut self, i: usize) {
        if i < self.terminals.len() {
            self.terminals.remove(i);
            if i < self.active_terminal {
                self.active_terminal -= 1;
            }
            self.after_terminal_removed();
        }
    }

    /// Close Others / to the Right / to the Left / Saved, around the active
    /// tab: the terminal's when one is focused (all but Saved).
    fn close_around(&mut self, command: Command, ctx: &egui::Context) {
        let hit = |i: usize, a: usize| match command {
            Command::CloseOtherTabs => i != a,
            Command::CloseTabsToRight => i > a,
            Command::CloseTabsToLeft => i < a,
            _ => false,
        };
        if self.terminal_focused() && command != Command::CloseSavedTabs {
            let a = self.active_terminal;
            let ids = self.terminals.iter().enumerate().filter(|&(i, t)| hit(i, a) && !t.pinned).map(|(_, t)| t.id).collect();
            return self.begin_close_terminals(ids);
        }
        let a = self.active.unwrap_or(0);
        let paths = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(i, t)| !t.pinned && match command {
                Command::CloseSavedTabs => !t.buf.modified,
                _ => hit(*i, a),
            })
            .filter_map(|(_, t)| t.path().cloned())
            .collect();
        self.begin_close(paths, After::Nothing, ctx);
    }

    /// A maximized terminal is for working in it: once the keyboard goes
    /// elsewhere (the editor, a file opened), it's back to its size. The
    /// tree beside it isn't elsewhere (a compact view's drawer over it is).
    pub(crate) fn end_maximize(&mut self) {
        let in_tree = self.focus == Focus::Sidebar && self.sidebar_shown() && !self.view().compact;
        if self.terminal_max && !self.terminal_focused() && !in_tree && self.palette.is_none() && !self.modal_open() {
            self.terminal_max = false;
        }
    }

    fn after_terminal_removed(&mut self) {
        if self.terminals.is_empty() {
            self.terminal_open = false;
            self.terminal_max = false;
            self.focus = Focus::Editor;
        }
        self.active_terminal = self
            .active_terminal
            .min(self.terminals.len().saturating_sub(1));
    }

    /// Copy / paste while the terminal has focus: copy its selection, paste
    /// into the shell (bracketed when the program asks for it).
    fn terminal_clipboard(&mut self, command: Command, ctx: &egui::Context) {
        let pasted = if command == Command::Paste {
            self.paste_text()
        } else {
            None
        };
        let Some(t) = self.terminal_mut() else { return };
        match command {
            Command::Copy | Command::Cut => {
                if let Some(text) = t.pane.selection_text() {
                    self.copy(ctx, text);
                }
            }
            Command::Paste => {
                if let Some(text) = pasted {
                    t.pane.paste(&text);
                }
            }
            _ => {}
        }
    }

    /// Quit, asking about unsaved work first.
    /// ⌘Q or the close button: always asked first, since closing ends
    /// whatever runs in the terminals (and remote access, if it runs here).
    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.prompt.is_some() {
            return; // already on its way, asking about unsaved files
        }
        self.close_palette(true, ctx);
        self.ask = Some(Ask::Quit);
    }

    /// Close a window nobody can see (on a virtual screen, remote access
    /// now off) — unless that would lose unsaved files, which wait for the
    /// next time someone looks in.
    pub(crate) fn close_unseen(&mut self, ctx: &egui::Context) {
        if self.tabs.iter().any(|t| t.buf.modified) {
            tracing::warn!("remote access is off; this window on a virtual screen stays, it has unsaved files");
            return;
        }
        tracing::info!("remote access is off and nobody can see this window: closing it");
        self.save_session();
        self.remember_terminals(true);
        self.allow_close = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// What closing this window would stop, one line each, for its dialog.
    pub(crate) fn quit_losses(&self) -> Vec<String> {
        let mut v: Vec<String> = self.running_line(None).into_iter().collect();
        match self.terminals.len() {
            _ if !v.is_empty() => {}
            0 => {}
            1 => v.push("Its terminal closes.".into()),
            n => v.push(format!("Its {n} terminals close.")),
        }
        match self.tabs.iter().filter(|t| t.buf.modified).count() {
            0 => {}
            1 => v.push("A file has unsaved changes: you'll be asked whether to save it.".into()),
            n => v.push(format!("{n} files have unsaved changes: you'll be asked about each.")),
        }
        if self.remote.here {
            v.push("Remote access turns off, for every window (it runs from this one).".into());
        } else {
            match self.remote.clients {
                0 => {}
                1 => v.push("A browser looking at it disconnects.".into()),
                n => v.push(format!("{n} browsers looking at it disconnect.")),
            }
        }
        v
    }

    /// Close the tabs for `paths`, asking about each modified one, then do
    /// `after`.
    pub fn begin_close(&mut self, paths: Vec<PathBuf>, after: After, ctx: &egui::Context) {
        self.prompt = Some(Prompt {
            queue: paths,
            after,
            all: None,
        });
        self.advance_close(ctx);
    }

    /// Work through the queue until a modified tab needs an answer (the
    /// prompt stays up for it) or the queue is done. Closing tabs removes
    /// them; quitting or closing the folder only settles unsaved work and
    /// leaves the tab list alone, so the session reopens as it was.
    fn advance_close(&mut self, ctx: &egui::Context) {
        while let Some((path, after)) = self
            .prompt
            .as_ref()
            .and_then(|p| Some((p.queue.first().cloned()?, p.after)))
        {
            let idx = self.tabs.iter().position(|t| t.path() == Some(&path));
            match idx {
                Some(i) if self.tabs.get(i).is_some_and(|t| t.buf.modified) => {
                    self.active = Some(i);
                    if let Some(all) = self.prompt.as_ref().and_then(|p| p.all) {
                        return self.answer_prompt(all, ctx);
                    }
                    return; // wait for the answer
                }
                Some(i) if after == After::Nothing => self.close_tab_now(i),
                _ => {}
            }
            if let Some(p) = &mut self.prompt {
                p.queue.remove(0);
            }
        }
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        match prompt.after {
            After::Nothing => {}
            After::CloseFolder | After::Replace => {
                self.save_session();
                self.tabs.clear();
                self.active = None;
                self.project = None;
                self.sidebar = Sidebar::default();
                if self.focus == Focus::Sidebar {
                    self.focus = Focus::Editor;
                }
                self.closed.clear();
                crate::instances::unregister();
                self.reload_config(ctx); // without the project's settings
                if prompt.after == After::Replace {
                    let dirs = std::mem::take(&mut self.replace_with);
                    self.open_each(dirs, ctx);
                }
            }
            After::Quit | After::Restart => {
                if prompt.after == After::Restart {
                    crate::update::restart_after_close(self.project.as_ref().map(|p| p.root.clone()));
                }
                self.save_session();
                self.remember_terminals(true);
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The user answered "save changes?" for the tab at the front of the queue.
    pub fn answer_prompt(&mut self, answer: Answer, ctx: &egui::Context) {
        let Some((path, after)) = self
            .prompt
            .as_ref()
            .and_then(|p| Some((p.queue.first().cloned()?, p.after)))
        else {
            return;
        };
        let idx = self.tabs.iter().position(|t| t.path() == Some(&path));
        let answer = match answer {
            Answer::SaveAll | Answer::DiscardAll => {
                let each = if answer == Answer::SaveAll {
                    Answer::Save
                } else {
                    Answer::Discard
                };
                if let Some(p) = &mut self.prompt {
                    p.all = Some(each);
                }
                each
            }
            a => a,
        };
        match (answer, idx) {
            (Answer::Cancel, _) => {
                self.prompt = None;
                return;
            }
            // A failed save keeps the tab (and the prompt, asking again): never lose work.
            (Answer::Save, Some(i)) if !self.save_tab(i) => {
                if let Some(p) = &mut self.prompt {
                    p.all = None;
                }
                return;
            }
            (_, Some(i)) if after == After::Nothing => self.close_tab_now(i),
            _ => {}
        }
        if let Some(p) = &mut self.prompt {
            p.queue.remove(0);
        }
        self.advance_close(ctx);
    }

    fn close_tab_now(&mut self, i: usize) {
        let tab = self.tabs.remove(i);
        remember_places(std::iter::once(&tab)); // for ⇧⌘T, or opening it again
        if let Some(path) = tab.path() {
            self.closed.retain(|p| p != path);
            self.closed.push(path.clone());
        }
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) => Some(a.min(self.tabs.len() - 1)),
            None => None,
        };
        self.save_session();
    }

    /// Open a file or folder with the system's default app (settings.json and
    /// friends, until the editor can edit them itself).
    fn open_external(&mut self, path: &Path) {
        // No app registered for .json on macOS: fall back to the text editor.
        let ok = files::os_open(&[path.as_os_str()])
            || (cfg!(target_os = "macos") && files::os_open(&["-t".as_ref(), path.as_os_str()]));
        if !ok {
            self.warn(format!("Couldn't open {}", path.display()));
        }
    }

    // ---- palette --------------------------------------------------------

    pub fn open_palette(&mut self, mode: Mode, ctx: &egui::Context) {
        self.close_palette(false, ctx);
        if mode == Mode::Commands {
            self.used = crate::recent::commands().iter().filter_map(|id| COMMANDS.iter().find(|i| i.id == id.as_str())).map(|i| i.command).collect();
        }
        let mut p = Palette::new(mode);
        p.opened = ctx.cumulative_frame_nr();
        p.revert = match mode {
            Mode::Themes => Some(Revert::Theme(self.config.theme.clone())),
            Mode::UiFonts | Mode::CodeFonts => Some(Revert::Settings(self.config.settings.clone())),
            _ => None,
        };
        if mode == Mode::Folders {
            // Start beside the folder that's open, else at home.
            let start = self.project.as_ref().and_then(|p| p.root.parent()).map(PathBuf::from).or_else(crate::config::home);
            if let Some(start) = start {
                p.query = folder_text(&start);
                p.to_end = true;
            }
        }
        self.palette = Some(p);
        if self.font_picker_open() {
            self.apply(ctx); // each font, to draw its name in
        }
        self.refresh_items();
        // Start the pickers on the current choice.
        let current = match mode {
            Mode::Themes => Some(Pick::Theme(self.config.theme.name.clone())),
            Mode::UiFonts => Some(Pick::UiFont(self.config.settings.ui_font.clone())),
            Mode::CodeFonts => Some(Pick::CodeFont(self.config.settings.code_font.clone())),
            _ => None,
        };
        if let (Some(cur), Some(p)) = (current, &mut self.palette)
            && let Some(i) = p.items.iter().position(|it| it.pick == cur)
        {
            p.selected = i;
        }
    }

    /// Close the palette; without `keep`, undo any live preview.
    pub fn close_palette(&mut self, keep: bool, ctx: &egui::Context) {
        let Some(p) = self.palette.take() else { return };
        if keep {
            if matches!(p.mode, Mode::UiFonts | Mode::CodeFonts) {
                self.apply(ctx); // the fonts not chosen go
            }
            return;
        }
        match p.revert {
            Some(Revert::Theme(t)) => self.config.theme = t,
            Some(Revert::Settings(s)) => self.config.settings = s,
            None => return,
        }
        self.apply(ctx);
    }

    /// Rebuild the palette's items for its mode and query.
    pub fn refresh_items(&mut self) {
        let Some(p) = &mut self.palette else { return };
        // A path typed into Open…: browse there.
        if p.mode == Mode::Open && (p.query.starts_with('~') || Path::new(&p.query).is_absolute()) {
            p.mode = Mode::Folders;
        }
        let (mode, query) = (p.mode, p.query.clone());
        let items = match mode {
            Mode::Commands => self.command_items(&query),
            Mode::Themes => {
                let names: Vec<String> =
                    self.config.themes.iter().map(|t| t.name.clone()).collect();
                simple_items(&query, names, icon::THEME, Pick::Theme)
            }
            Mode::UiFonts => simple_items(
                &query,
                fonts::ui_names().map(String::from).collect(),
                icon::FONT,
                Pick::UiFont,
            ),
            Mode::CodeFonts => simple_items(
                &query,
                fonts::code_names().map(String::from).collect(),
                icon::FONT,
                Pick::CodeFont,
            ),
            Mode::Open => self.open_items(&query),
            Mode::Folders => folder_items(&query),
            Mode::Terminals => self.terminal_items(&query),
            Mode::Links => self.link_items(&query),
            Mode::Search => {
                if query != self.search.query {
                    self.search.changed = Some(Instant::now());
                }
                self.search_items(&query)
            }
            Mode::Todos => self.todo_items(),
            Mode::Clipboard => self.clip_items(&query),
            Mode::Settings => self.setting_items(&query),
            Mode::Keybindings => self.binding_items(&query),
            Mode::Line => self.line_items(&query),
        };
        if let Some(p) = &mut self.palette {
            p.items = items;
            p.selected = p.selected.min(p.items.len().saturating_sub(1));
        }
    }

    fn command_items(&self, query: &str) -> Vec<Item> {
        let mut scored: Vec<(i64, Item)> = COMMANDS
            .iter()
            .filter(|c| c.command != Command::Palette && self.available(c.command))
            .filter_map(|c| {
                let score = fuzzy::score(query, c.title)?;
                let recent = self.used.iter().position(|u| *u == c.command);
                let item = Item {
                    icon: icon::for_command(c.command),
                    label: c.title.to_string(),
                    detail: if recent.is_some() && query.is_empty() {
                        "recently used".into()
                    } else {
                        String::new()
                    },
                    caps: self
                        .config
                        .keymap
                        .combo_for(c.command)
                        .map(|k| k.caps())
                        .unwrap_or_default(),
                    matched: fuzzy::positions(query, c.title),
                    dim: false,
                    tint: None,
                    pick: Pick::Command(c.command),
                };
                // With nothing typed, the last few picked come first; typing,
                // they win over as good a match.
                let boost = match recent {
                    Some(r) if query.is_empty() => 1000 - r as i64,
                    Some(r) => (16 - r as i64) * 96,
                    _ => 0,
                };
                Some((score + boost, item))
            })
            .collect();
        scored.sort_by_key(|(s, _)| -s); // stable: ties keep COMMANDS order
        scored.into_iter().map(|(_, i)| i).collect()
    }

    /// Start a search once typing has paused, and take results when done.
    fn poll_search(&mut self, ctx: &egui::Context) {
        if let Some((q, rx)) = &self.search.job
            && let Ok((files, truncated)) = rx.try_recv()
        {
            self.search.query = q.clone();
            self.search.hits = files
                .into_iter()
                .flat_map(|f| f.matches.into_iter().map(move |m| (f.path.clone(), m)))
                .collect();
            self.search.truncated = truncated;
            self.search.job = None;
            self.refresh_items();
        }
        let Some(pal) = &self.palette else { return };
        let (Some(changed), Some(project)) = (self.search.changed, &self.project) else {
            return;
        };
        if pal.mode != Mode::Search || self.search.job.is_some() || pal.query == self.search.query {
            return;
        }
        let wait = crate::search::SEARCH_DEBOUNCE.saturating_sub(changed.elapsed());
        if !wait.is_zero() {
            return ctx.request_repaint_after(wait);
        }
        let (tx, rx) = channel();
        let (root, q, build, ctx2) = (
            project.root.clone(),
            pal.query.clone(),
            self.config.settings.search_ignored,
            ctx.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(crate::search::search_project(&root, &q, build));
            ctx2.request_repaint();
        });
        self.search.job = Some((pal.query.clone(), rx));
    }

    /// Whether a search is waiting to start or running (for "Searching…").
    pub fn searching(&self) -> bool {
        self.search.job.is_some()
            || self
                .palette
                .as_ref()
                .is_some_and(|p| p.mode == Mode::Search && p.query != self.search.query)
    }

    fn search_items(&self, query: &str) -> Vec<Item> {
        if query.is_empty() || query != self.search.query {
            return Vec::new();
        }
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .unwrap_or_default();
        self.search
            .hits
            .iter()
            .take(MAX_RESULTS)
            .map(|(path, m)| {
                // Trim the indent off the preview, keeping the match columns aligned.
                let indent = m.preview.chars().take_while(|c| c.is_whitespace()).count();
                // A long line is cut around the match, so the file and line
                // number after it stay in view.
                const KEEP: usize = 64;
                let (mut from, len) = (indent, m.preview.chars().count());
                if m.preview_col > from + 24 {
                    from = m.preview_col - 20;
                }
                let to = len.min(from + KEEP);
                let lead = if from > indent { "…" } else { "" };
                let tail = if to < len { "…" } else { "" };
                let label = format!(
                    "{lead}{}{tail}",
                    m.preview
                        .chars()
                        .skip(from)
                        .take(to - from)
                        .collect::<String>()
                );
                let shift = lead.chars().count();
                let matched = (m.preview_col.saturating_sub(from) + shift
                    ..(m.preview_end_col.min(to)).saturating_sub(from) + shift)
                    .collect();
                Item {
                    icon: icon::SEARCH,
                    label,
                    detail: format!("{}:{}", rel_path(&root, path), m.line + 1),
                    caps: Vec::new(),
                    matched,
                    dim: false,
                    tint: None,
                    pick: Pick::Hit(path.to_path_buf(), m.line, m.col, m.end_col),
                }
            })
            .collect()
    }

    /// "1,234 results in 56 files" for the search footer.
    pub fn search_summary(&self) -> String {
        let n = self.search.hits.len();
        let files = self
            .search
            .hits
            .iter()
            .map(|h| &h.0)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let capped = if self.search.truncated {
            " (capped)"
        } else {
            ""
        };
        let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        format!(
            "{} in {}{capped}",
            plural(n, "result"),
            plural(files, "file")
        )
    }

    /// The links on the active terminal's screen, newest first.
    fn link_items(&self, query: &str) -> Vec<Item> {
        let urls = self
            .terminal()
            .map(|t| crate::terminalpane::urls_on_screen(&t.pane.screen()))
            .unwrap_or_default();
        urls.into_iter()
            .filter(|u| fuzzy::score(query, u).is_some())
            .map(|u| {
                let mut it = Item::new(icon::LINK, u.clone(), Pick::Url(u.clone()));
                it.matched = fuzzy::positions(query, &u);
                it
            })
            .collect()
    }

    /// Each editor tab's folders that tell it from same-named files.
    pub(crate) fn tab_details(&self) -> Vec<String> {
        let names: Vec<String> = self.tabs.iter().map(|t| t.name()).collect();
        let items: Vec<(&str, Option<&Path>)> =
            self.tabs.iter().zip(&names).map(|(t, n)| (n.as_str(), t.path().and_then(|p| p.parent()))).collect();
        fstree::tell_apart(&items)
    }

    /// Each terminal's name, its folder told apart from same-named folders
    /// elsewhere ("Codes/thr"); the same folder twice is numbered (`labels`).
    pub(crate) fn terminal_names(&self) -> Vec<String> {
        let folders: Vec<String> = self.terminals.iter().map(|t| t.pane.folder_name()).collect();
        let items: Vec<(&str, Option<&Path>)> =
            self.terminals.iter().zip(&folders).map(|(t, f)| (f.as_str(), t.pane.dir.parent())).collect();
        let apart = fstree::tell_apart(&items);
        self.terminals.iter().zip(&apart).map(|(t, a)| t.pane.display_name_in(a)).collect()
    }

    fn terminal_items(&self, query: &str) -> Vec<Item> {
        let names = self.terminal_names();
        // Led by the terminal's number (as on its tab and its ⌥N key), which,
        // unlike a name, doesn't shift as commands start and stop.
        terminal::labels(&names)
            .into_iter()
            .enumerate()
            .map(|(i, label)| (i, format!("{}  {label}", i + 1)))
            .filter(|(_, label)| fuzzy::score(query, label).is_some())
            .map(|(i, label)| Item {
                icon: icon::TERMINAL,
                matched: fuzzy::positions(query, &label),
                label,
                detail: self.terminals.get(i).and_then(|t| t.pane.pulse()).map(|p| p.label()).unwrap_or_default(),
                caps: if i < 9 {
                    self.config
                        .keymap
                        .combo_for(Command::JumpTerminal(i as u8 + 1))
                        .map(|k| k.caps())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                dim: false,
                tint: None,
                pick: Pick::Terminal(i),
            })
            .collect()
    }

    /// Open…: every folder you've opened, as a project or with a terminal
    /// in it, once, newest first or A to Z (⌘S, remembered); then browsing
    /// to any other, and a new window.
    fn open_items(&self, query: &str) -> Vec<Item> {
        let mut folders = crate::recent::list();
        if self.config.settings.open_list_by_name {
            folders.sort_by_cached_key(|(d, _)| (display_name(d).to_lowercase(), d.clone()));
        }
        let now = crate::update::now();
        let caps = |c: Command| self.config.keymap.combo_for(c).map(|k| k.caps()).unwrap_or_default();
        let found = folders.into_iter().filter_map(|(dir, at)| {
            let name = display_name(&dir);
            if fuzzy::score(query, &name).is_none() && fuzzy::score(query, &dir.to_string_lossy()).is_none() {
                return None;
            }
            let path = crate::ui::tilde(&dir.to_string_lossy());
            let elsewhere = self.open_elsewhere(&dir).is_some();
            let mut detail = vec![path];
            if self.project.as_ref().is_some_and(|p| p.root == dir) {
                detail.insert(0, "open in this window".into());
            } else if elsewhere {
                detail.insert(0, "open in another window".into());
            }
            // Folders from before times were kept have none to show.
            if at > 0 {
                detail.push(crate::recent::ago(now.saturating_sub(at)));
            }
            Some(Item {
                matched: fuzzy::positions(query, &name),
                dim: elsewhere,
                ..Item::new(icon::FOLDER, name, Pick::Folder(dir)).detail(detail.join("  ·  "))
            })
        });
        let browse = Item::new(icon::FOLDER_OPEN, "Browse…", Pick::Browse).detail("to any other folder, by its path");
        let window = Item {
            caps: caps(Command::NewWindow),
            ..Item::new(icon::WINDOW_RESTORE, "New Window", Pick::Command(Command::NewWindow)).detail("an empty one, no folder")
        };
        found.chain([browse, window]).collect()
    }

    /// Open…'s ⌘S: sort by name, or by when, and keep that.
    pub(crate) fn sort_open_list(&mut self, ctx: &egui::Context) {
        let by_name = !self.config.settings.open_list_by_name;
        self.config.settings.open_list_by_name = by_name;
        self.save_choice_quietly("openListByName", by_name, ctx);
        self.refresh_items();
    }

    /// Open…'s and Browse's ⌘↵: a terminal in the ticked folders (in
    /// ticking order), else in the chosen one.
    pub(crate) fn open_as_terminals(&mut self, ctx: &egui::Context) {
        let mut dirs: Vec<PathBuf> = self
            .ticked()
            .into_iter()
            .filter_map(|t| if let Pick::Folder(d) = t { Some(d) } else { None })
            .collect();
        if dirs.is_empty()
            && let Some(Pick::Folder(d)) = self.palette.as_ref().and_then(|p| p.selected_pick().cloned())
        {
            dirs.push(d);
        }
        if dirs.is_empty() {
            return;
        }
        self.close_palette(true, ctx);
        self.open_terminals(dirs, ctx);
    }

    /// A terminal in each of `dirs`, in that order, side by side. A window
    /// with nothing open takes them, filling it; one with things open asks,
    /// even about one: here, or a new window (`Ask::OpenTerminals`).
    pub fn open_terminals(&mut self, dirs: Vec<PathBuf>, ctx: &egui::Context) {
        let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| d.is_dir()).collect();
        if dirs.is_empty() {
            return;
        }
        tracing::info!(n = dirs.len(), "open terminals");
        let busy = self.project.is_some() || !self.terminals.is_empty();
        if busy {
            self.ask = Some(Ask::OpenTerminals { dirs });
        } else {
            self.terminals_here(dirs, ctx);
        }
    }

    /// `open_terminals` in this window: after the one in front, side by
    /// side, the first of them in front; filling a window with nothing else.
    pub(crate) fn terminals_here(&mut self, dirs: Vec<PathBuf>, ctx: &egui::Context) {
        let (empty, before) = (self.project.is_none() && self.terminals.is_empty(), self.terminals.len());
        let first = if empty { 0 } else { self.active_terminal + 1 };
        for d in dirs {
            self.new_terminal_in(Some(d), ctx);
        }
        if self.terminals.len() > before {
            self.active_terminal = first.min(self.terminals.len() - 1);
            self.terminal_grid = true;
            self.terminal_max |= empty;
        }
    }

    /// `open_terminals` in a window of their own.
    pub(crate) fn terminals_in_new_window(&mut self, dirs: &[PathBuf]) {
        let n = dirs.len();
        match files::spawn_terminals_window(dirs) {
            Ok(()) => self.notify(format!("Opened {n} terminal{} in a new window", if n == 1 { "" } else { "s" })),
            Err(e) => self.warn(format!("Couldn't open a new window: {e}")),
        }
    }

    /// Keep this window's terminals' folders in the Open list when they
    /// change; `closing`: now, whether or not they did.
    fn remember_terminals(&mut self, closing: bool) {
        let dirs: Vec<PathBuf> = self.terminals.iter().map(|t| t.pane.dir.clone()).filter(|d| !d.as_os_str().is_empty()).collect();
        if dirs.is_empty() || (!closing && dirs == self.terms_saved) {
            return;
        }
        crate::recent::record(&dirs);
        self.terms_saved = dirs;
    }

    /// Whether the file index is still being built.
    pub fn indexing(&self) -> bool {
        self.project.as_ref().is_some_and(|p| p.indexing.is_some())
    }

    /// The selection moved: preview theme / font choices live.
    pub fn palette_moved(&mut self, ctx: &egui::Context) {
        let Some(pick) = self
            .palette
            .as_ref()
            .and_then(|p| p.selected_pick().cloned())
        else {
            return;
        };
        match pick {
            Pick::Theme(name) => {
                if let Some(t) = self.config.themes.iter().find(|t| t.name == name) {
                    self.config.theme = t.clone();
                }
            }
            Pick::UiFont(name) => self.config.settings.ui_font = name,
            Pick::CodeFont(name) => self.config.settings.code_font = name,
            _ => return,
        }
        self.apply(ctx);
    }

    /// Enter (or a click) on the selected item.
    pub fn palette_accept(&mut self, ctx: &egui::Context) {
        // To-do: Enter with something typed adds it.
        if let Some(p) = self
            .palette
            .as_ref()
            .filter(|p| p.mode == Mode::Todos && !p.query.trim().is_empty())
        {
            let text = p.query.clone();
            return self.todo_add(&text);
        }
        let Some(pick) = self
            .palette
            .as_ref()
            .and_then(|p| p.selected_pick().cloned())
        else {
            return;
        };
        match pick {
            Pick::Todo(i) => self.todo_toggle(i),
            Pick::Clip(_) => self.clip_paste(ctx),
            Pick::Setting(s) => self.activate_setting(s, ctx),
            Pick::Binding(c) => {
                if let Some(p) = &mut self.palette {
                    p.capture = Some(c);
                }
            }
            Pick::Hit(path, line, col, end) => {
                self.close_palette(true, ctx);
                self.open_file(&path);
                if let Some(i) = self.active {
                    self.load_tab(i);
                }
                if let Some(t) = self.active_tab_mut() {
                    t.go_to(line, col..end);
                }
                self.focus = Focus::Editor;
            }
            Pick::Line(line, col) => {
                self.close_palette(true, ctx);
                self.go_to(line, col);
            }
            Pick::Url(url) => {
                self.close_palette(true, ctx);
                // Chosen by name from the list: no "open this link?" step.
                self.open_url(url, ctx);
            }
            Pick::Terminal(i) => {
                self.close_palette(true, ctx);
                self.active_terminal = i.min(self.terminals.len().saturating_sub(1));
                self.show_terminals(ctx);
            }
            Pick::Browse => self.open_palette(Mode::Folders, ctx),
            Pick::Folder(dir) => {
                // The ticked ones, if any; else the one chosen.
                let mut dirs: Vec<PathBuf> = self
                    .ticked()
                    .into_iter()
                    .filter_map(|t| if let Pick::Folder(d) = t { Some(d) } else { None })
                    .collect();
                if dirs.is_empty() {
                    dirs.push(dir);
                }
                self.close_palette(true, ctx);
                self.open_folders(dirs, ctx);
            }
            Pick::Command(c) => {
                self.close_palette(true, ctx);
                // Remembered for every window, and the next run.
                if let Some(info) = COMMANDS.iter().find(|i| i.command == c) {
                    crate::recent::record_command(info.id);
                }
                self.used.retain(|u| *u != c);
                self.used.insert(0, c);
                self.run(c, ctx);
            }
            Pick::Theme(name) => self.save_choice("theme", name, ctx),
            Pick::UiFont(name) => self.save_choice("uiFont", name, ctx),
            Pick::CodeFont(name) => self.save_choice("codeFont", name, ctx),
        }
    }

    /// Persist a setting without a "saved" note (toggles announce themselves).
    pub(crate) fn save_choice_quietly(
        &mut self,
        key: &str,
        value: impl Into<serde_json::Value>,
        ctx: &egui::Context,
    ) {
        let root = self.root().map(Path::to_path_buf);
        match config::save_setting(root.as_deref(), key, value) {
            Ok(()) => self.reload_config(ctx),
            Err(e) => self.warn(format!("Couldn't save: {e}")),
        }
    }

    /// Persist a picker choice to settings.json (keeping the preview applied).
    fn save_choice(&mut self, key: &str, value: String, ctx: &egui::Context) {
        self.close_palette(true, ctx);
        let root = self.root().map(Path::to_path_buf);
        match config::save_setting(root.as_deref(), key, value.as_str()) {
            Ok(()) => {
                // Take our own write now, so the file watcher doesn't report it
                // as an outside change.
                self.reload_config(ctx);
                self.notify(format!("{value} — saved to settings"));
            }
            Err(e) => self.warn(format!("Couldn't save: {e}")),
        }
    }
}

/// Items for a plain list of names (themes, fonts).
fn simple_items(
    query: &str,
    names: Vec<String>,
    icon: &'static str,
    pick: fn(String) -> Pick,
) -> Vec<Item> {
    names
        .into_iter()
        .filter(|n| fuzzy::score(query, n).is_some())
        .map(|name| Item {
            icon,
            matched: fuzzy::positions(query, &name),
            label: name.clone(),
            detail: String::new(),
            caps: Vec::new(),
            dim: false,
            tint: None,
            pick: pick(name),
        })
        .collect()
}

/// The project a file belongs to: its nearest enclosing git repository, or
/// just its folder when it isn't in one.
fn project_root(dir: &Path) -> PathBuf {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(dir)
        .to_path_buf()
}

pub(crate) fn display_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// `path` relative to `root`, with `/` separators.
fn rel_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        // Outside the project: the full path.
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Note where you are in each of `tabs` (only ones read: an unread tab's
/// caret is just the top), so each file reopens there.
fn remember_places<'a>(tabs: impl Iterator<Item = &'a Tab>) {
    let places: Vec<_> =
        tabs.filter(|t| t.is_loaded()).filter_map(|t| Some((t.path()?.clone(), t.place()))).collect();
    if !places.is_empty() {
        crate::session::remember(&places);
    }
}

/// Put a just-read tab back where you last were in its file.
fn recall_place(tab: &mut Tab) {
    if let Some(place) = tab.path().and_then(|p| crate::session::recall(p)) {
        tab.go_to_place(place);
    }
}

/// A folder as Open Folder's query shows it: `~` for home, ending in a
/// separator, so its folders are listed.
pub fn folder_text(dir: &Path) -> String {
    let text = crate::ui::tilde(&dir.to_string_lossy());
    if text.ends_with(std::path::is_separator) {
        text
    } else {
        format!("{text}{}", std::path::MAIN_SEPARATOR)
    }
}

/// Open Folder's list: the folder typed, then the folders in it (see
/// `palette::folder_query`); hidden ones only once a `.` is typed.
/// Open…'s Browse: the folders in the one typed.
fn folder_items(query: &str) -> Vec<Item> {
    let home = crate::config::home().unwrap_or_default();
    let (dir, filter) = crate::palette::folder_query(query, &home);
    let mut found: Vec<(i64, String)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir()) // links to folders too
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !filter.starts_with('.') {
                return None;
            }
            Some((fuzzy::score(&filter, &name)?, name))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let here = (filter.is_empty() && dir.is_dir()).then(|| {
        Item::new(icon::FOLDER_OPEN, "Open This Folder", Pick::Folder(dir.clone()))
            .detail(crate::ui::tilde(&dir.to_string_lossy()))
    });
    here.into_iter()
        .chain(found.into_iter().take(MAX_RESULTS).map(|(_, name)| Item {
            matched: fuzzy::positions(&filter, &name),
            ..Item::new(icon::FOLDER, name.clone(), Pick::Folder(dir.join(&name)))
        }))
        .collect()
}

/// Maximize the window, or restore it (the title bar's double-click).
pub fn toggle_maximized(ctx: &egui::Context) {
    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
}

/// Watch the config files every window shares (settings, keybindings,
/// themes): on a change, flag it and wake the window, so a setting saved in
/// one window shows in the others at once, even idle ones in the background.
fn watch_config(ctx: &egui::Context) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let (ctx, out) = (ctx.clone(), flag.clone());
    // If no thread can start, the 1s check in `poll_config` still catches it.
    let _ = std::thread::Builder::new()
        .name("config-watch".into())
        .spawn(move || {
            let mut last = config::fingerprint(None);
            // Ends with the window (the flag's only other owner).
            while Arc::strong_count(&out) > 1 {
                std::thread::sleep(Duration::from_millis(400));
                let now = config::fingerprint(None);
                if now != last {
                    last = now;
                    out.store(true, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }
        });
    flag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_uses_slashes() {
        assert_eq!(
            rel_path(Path::new("/a/b"), Path::new("/a/b/src/main.rs")),
            "src/main.rs"
        );
        assert_eq!(
            rel_path(Path::new("/a/b"), Path::new("/c/d.txt")),
            "/c/d.txt"
        );
    }

    #[test]
    fn a_file_opens_its_repo_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        assert_eq!(project_root(&dir.path().join("src/deep")), dir.path());
        let loose = tempfile::tempdir().unwrap();
        assert_eq!(project_root(loose.path()), loose.path());
    }
}
