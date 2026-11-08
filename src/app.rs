//! App state and behaviour: config, the open project, tabs, the palette,
//! and running commands. Drawing lives in `ui`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;

use crate::config::{self, Config};
use crate::editor::Tab;
use crate::find::Find;
use crate::fonts;
use crate::fstree::{self, FileTree};
use crate::fuzzy::{self, FileMatcher};
use crate::keymap::{COMMANDS, Command};
use crate::palette::{Item, Mode, Palette, Pick, Revert, label_positions};
use crate::terminal::{self, Terminal, Throttle};
use crate::ui::icon;

mod extras;
mod files;
pub use extras::{Setting, Toast};
pub use files::Ask;
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

/// Search results shown at most — more is noise, and costs layout.
const MAX_RESULTS: usize = 300;

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub tree: FileTree,
    /// Project-relative paths (with `/`) and their gitignored flag, once the
    /// background walk finishes.
    files: Vec<(String, bool)>,
    matcher: Option<FileMatcher>,
    indexing: Option<Receiver<Vec<(PathBuf, bool)>>>,
    /// Search includes the build/dependency folders (node_modules, target…).
    pub build: bool,
}

/// Closing tabs that may hold unsaved work: they're worked through one at a
/// time, asking about each modified one, then `after` runs.
pub struct Prompt {
    pub queue: Vec<PathBuf>,
    pub after: After,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum After {
    Nothing,
    CloseFolder,
    Quit,
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

/// The file tree's own state (the tree itself is the project's, shared with
/// Go to File).
#[derive(Default)]
pub struct Sidebar {
    /// The selected row's path (rows move as folders open and close).
    pub selected: Option<PathBuf>,
    /// The file last revealed, so switching tabs reveals the new one once.
    pub revealed: Option<PathBuf>,
    /// Scroll the selected row into view next frame.
    pub scroll: bool,
    /// The list's scroll position and height last frame.
    pub offset: f32,
    pub view: f32,
    /// When the tree was last re-read from disk.
    pub read_at: Option<Instant>,
}

/// The answer to "save changes?".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

pub const TAB_WIDTH: usize = 4;

pub struct App {
    pub config: Config,
    /// Config file mtimes at the last load, polled to reload on save.
    fingerprint: Vec<(PathBuf, Option<SystemTime>)>,
    last_poll: Instant,
    /// The (UI, code) fonts installed in egui, and any errors loading them.
    applied_fonts: Option<(String, String)>,
    font_errors: Vec<String>,
    /// Notifications, oldest first.
    pub toasts: Vec<Toast>,
    /// Terminal alerts raised while the window was in the background,
    /// shown as toasts on coming back.
    missed: Vec<String>,
    /// Text copied in the app this session, newest first (⌥V).
    pub clips: Vec<String>,
    /// The to-do list, as last read from (and written to) `todos.md`.
    pub todos: Vec<crate::todo::Entry>,
    /// The F1 shortcuts sheet is up.
    pub help: bool,
    pub welcome_selected: usize,
    pub project: Option<Project>,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    /// Recently closed files, newest last, for Reopen Closed Tab.
    closed: Vec<PathBuf>,
    pub prompt: Option<Prompt>,
    /// A modified tab whose file changed on disk, awaiting reload-or-keep.
    pub conflict: Option<PathBuf>,
    /// A file dialog (new / rename / delete) or "open this link?".
    pub ask: Option<Ask>,
    last_file_check: Instant,
    /// Set once unsaved work is dealt with, so the window may really close.
    allow_close: bool,
    pub palette: Option<Palette>,
    pub find: Option<Find>,
    search: SearchState,
    pub terminals: Vec<Terminal>,
    pub active_terminal: usize,
    /// All terminals at once, tiled, instead of one tab at a time.
    pub terminal_grid: bool,
    /// The terminal panel fills the window (the editor steps aside).
    pub terminal_max: bool,
    /// All open files tiled side by side (split view).
    pub editor_grid: bool,
    throttle: std::sync::Arc<Throttle>,
    /// How far into the terminal bridge's request file we've read.
    request_offset: u64,
    watching_requests: bool,
    /// Whether the terminal panel is showing.
    pub terminal_open: bool,
    /// Where each terminal was drawn last frame, for drops.
    pub terminal_rects: Vec<(usize, egui::Rect)>,
    pub sidebar: Sidebar,
    /// Commands picked in the palette, latest first (they lead its list).
    used: Vec<Command>,
    pub focus: Focus,
    /// Files opened this session, newest first (ranks them first in search).
    mru: Vec<PathBuf>,
    title: String,
    pub fps: Option<f32>,
    frames: u32,
    fps_window_start: Instant,
    pub mem_kb: Option<u64>,
    mem_last_check: Option<Instant>,
}

impl App {
    /// `target` is the command-line path: a folder to open, or a file to open
    /// along with its folder.
    pub fn new(ctx: &egui::Context, target: Option<PathBuf>) -> Self {
        config::ensure_files();
        let mut app = Self {
            config: config::load(None, None),
            fingerprint: config::fingerprint(None),
            last_poll: Instant::now(),
            applied_fonts: None,
            font_errors: Vec::new(),
            toasts: Vec::new(),
            missed: Vec::new(),
            clips: Vec::new(),
            todos: Vec::new(),
            help: false,
            welcome_selected: 0,
            project: None,
            tabs: Vec::new(),
            active: None,
            closed: Vec::new(),
            prompt: None,
            conflict: None,
            ask: None,
            last_file_check: Instant::now(),
            allow_close: false,
            palette: None,
            find: None,
            search: SearchState::default(),
            terminals: Vec::new(),
            active_terminal: 0,
            terminal_grid: false,
            terminal_max: false,
            editor_grid: false,
            throttle: Throttle::new(60),
            // Requests queued before this window existed aren't ours.
            request_offset: crate::termbridge::request_file()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len()),
            watching_requests: false,
            terminal_open: false,
            terminal_rects: Vec::new(),
            sidebar: Sidebar::default(),
            used: Vec::new(),
            focus: Focus::Editor,
            mru: Vec::new(),
            title: String::new(),
            fps: None,
            frames: 0,
            fps_window_start: Instant::now(),
            mem_kb: None,
            mem_last_check: None,
        };
        app.apply(ctx);
        if let Some(path) = target.and_then(|p| p.canonicalize().ok()) {
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
    pub fn apply(&mut self, ctx: &egui::Context) {
        let s = &self.config.settings;
        let wanted = (s.ui_font.clone(), s.code_font.clone());
        if self.applied_fonts.as_ref() != Some(&wanted) {
            self.font_errors = fonts::install(ctx, &wanted.0, &wanted.1);
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
        if self.last_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_poll = Instant::now();
        let fp = config::fingerprint(self.root());
        if fp == self.fingerprint {
            return;
        }
        self.reload_config(ctx);
        if self.errors().next().is_none() {
            self.notify("Config reloaded");
        }
    }

    pub fn errors(&self) -> impl Iterator<Item = &String> {
        self.config.errors.iter().chain(&self.font_errors)
    }

    /// Re-read the config (global + this project's) and apply it.
    pub fn reload_config(&mut self, ctx: &egui::Context) {
        let root = self.root().map(Path::to_path_buf);
        self.config = config::load(Some(&self.config), root.as_deref());
        self.fingerprint = config::fingerprint(root.as_deref());
        self.apply(ctx);
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
        self.frames += 1;
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
        }
        self.poll_config(ctx);
        self.poll_index();

        // The OS close button (or ⌘Q from the app menu): ask about unsaved
        // work first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_quit(ctx);
        }
        if let Some(t) = self.active_tab_mut() {
            t.refresh(Some(ctx));
        }
        if let (Some(f), Some(i)) = (&mut self.find, self.active) {
            f.refresh(&self.tabs[i].buf, i);
        }
        // Coming back to the window is when files most likely changed.
        if ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(true)))
        }) {
            self.last_file_check = Instant::now() - Duration::from_secs(5);
        }
        self.poll_file_changes();
        // Shells that exited (`exit`, ^D) close their terminal; the last one
        // closes the panel.
        let before = self.terminals.len();
        self.terminals.retain_mut(|t| !t.pane.exited());
        if self.terminals.len() != before {
            self.after_terminal_removed();
        }
        self.poll_terminal_requests(ctx);
        self.poll_attention(ctx);
        self.poll_search(ctx);
        self.throttle.fps.store(
            self.config.settings.terminal_fps,
            std::sync::atomic::Ordering::Relaxed,
        );
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
        let bound = |e: &egui::Event| match e {
            egui::Event::Key { key, modifiers, .. }
                if shell && terminal::belongs_to_shell(*key, *modifiers) =>
            {
                None
            }
            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } => keymap.command_for(*key, *physical_key, *modifiers),
            // ⌘C/⌘X arrive as clipboard events, not keys (⌘V too, as Paste
            // with the text, which the editor or search box takes directly).
            egui::Event::Copy => Some(Command::Copy),
            egui::Event::Cut => Some(Command::Cut),
            _ => None,
        };
        // A command that can't run now leaves its key to the editor. Move to
        // Trash's key (⌘⌫) works only in Go to File: elsewhere, even in the
        // command palette's search box, it deletes to the line start.
        let in_files = self
            .palette
            .as_ref()
            .map_or(self.focus == Focus::Sidebar, |p| p.mode == Mode::Files);
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
        // Take the chord's events (and any text it typed, like ⌥F's "ƒ") so
        // nothing downstream acts on them too.
        ctx.input_mut(|i| {
            i.events
                .retain(|e| command_for(e).is_none() && !matches!(e, egui::Event::Text(_)))
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
            Command::GoToFile => Some(Mode::Files),
            Command::RecentFolders => Some(Mode::Recent),
            Command::SearchInFiles => Some(Mode::Search),
            Command::GoToLine => Some(Mode::Line),
            Command::SwitchTerminal => Some(Mode::Terminals),
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
                    self.open_palette(Mode::Files, ctx)
                } else {
                    self.notify("Open a folder first")
                }
            }
            Command::OpenFolder => {
                // The Finder panel takes over: nothing should be left open behind it.
                self.close_palette(false, ctx);
                if let Some(dir) = pick_folder() {
                    self.open_folder(dir, ctx);
                }
            }
            Command::RecentFolders => self.open_palette(Mode::Recent, ctx),
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
            Command::CloseAllTabs => self.begin_close(self.tab_paths(), After::Nothing, ctx),
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
                let to = self.active_terminal as isize
                    + if command == Command::MoveTabRight {
                        1
                    } else {
                        -1
                    };
                if to >= 0 && (to as usize) < self.terminals.len() {
                    self.terminals.swap(self.active_terminal, to as usize);
                    self.active_terminal = to as usize;
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
                    self.active = Some(i);
                    self.focus = Focus::Editor;
                    self.save_session();
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
            Command::FocusSidebar => self.focus_sidebar(),
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
            Command::CloseTerminal => self.close_terminal(self.active_terminal),
            Command::ToggleTerminalGrid => {
                self.terminal_grid = !self.terminal_grid;
                self.show_terminals(ctx);
            }
            Command::MaximizeTerminal => {
                self.terminal_max = !self.terminal_max;
                self.show_terminals(ctx);
            }
            Command::SwitchTerminal => self.open_palette(Mode::Terminals, ctx),
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
            | Command::CopyPath
            | Command::CopyRelativePath
            | Command::ToggleBuildFolders => self.run_file_command(command, ctx),
            Command::Settings => self.open_palette(Mode::Settings, ctx),
            Command::Keybindings => self.open_palette(Mode::Keybindings, ctx),
            Command::Todos => self.open_todos(ctx),
            Command::ClipboardHistory => self.open_palette(Mode::Clipboard, ctx),
            Command::Help => self.help = !self.help,
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
            | Command::CopyPath
            | Command::CopyRelativePath => self.file_target().is_some(),
            // Only with a list open: in the editor ⌘⌫ deletes to the line start.
            Command::DeleteFile => {
                (self.palette.is_some() || self.focus == Focus::Sidebar)
                    && self.file_target().is_some()
            }
            Command::Save
            | Command::CloseTab
            | Command::CloseAllTabs
            | Command::Undo
            | Command::Redo
            | Command::Copy
            | Command::Cut
            | Command::Paste
            | Command::SelectAll => self.active.is_some() || self.terminal_focused(),
            Command::Find
            | Command::Replace
            | Command::AddNextOccurrence
            | Command::AddCursorAbove
            | Command::AddCursorBelow => self.active.is_some(),
            // Line edits are the editor's: in a terminal ⌥↑ and ⌘↵ stay the shell's.
            Command::GoToLine
            | Command::ToggleComment
            | Command::MoveLineUp
            | Command::MoveLineDown
            | Command::DuplicateLineUp
            | Command::DuplicateLineDown
            | Command::DeleteLine
            | Command::InsertLineBelow
            | Command::InsertLineAbove
            | Command::IndentLines
            | Command::OutdentLines => self.active.is_some() && self.focus == Focus::Editor,
            Command::GoToTab(n) => {
                n == 9 && !self.tabs.is_empty() || (n as usize) <= self.tabs.len()
            }
            Command::ClearTerminal => self.terminal_focused(),
            Command::SwitchFocus => {
                self.active.is_some() || !self.terminals.is_empty() || self.project.is_some()
            }
            Command::SearchInFiles => self.project.is_some(),
            Command::ToggleSplit => self.tabs.len() > 1,
            Command::SaveAll => self.tabs.iter().any(|t| t.buf.modified),
            Command::NextTab | Command::PrevTab | Command::MoveTabLeft | Command::MoveTabRight => {
                self.tabs.len() > 1 || (self.terminal_focused() && self.terminals.len() > 1)
            }
            Command::CloseTerminal | Command::ToggleTerminalGrid | Command::MaximizeTerminal => {
                !self.terminals.is_empty()
            }
            Command::SwitchTerminal => self.terminals.len() > 1,
            Command::JumpTerminal(n) => (n as usize) <= self.terminals.len(),
            Command::ReopenTab => !self.closed.is_empty(),
            _ => true,
        }
    }

    // ---- folders & files ------------------------------------------------

    /// Open `dir` here, or — if this window already has a project — in a new
    /// window, like Oxru: one project per window.
    pub fn open_folder(&mut self, dir: PathBuf, ctx: &egui::Context) {
        if self.project.as_ref().is_some_and(|p| p.root == dir) {
            return;
        }
        if let Some(pid) = self.open_elsewhere(&dir) {
            // One window per folder: go to the one that has it.
            let shown = crate::sys::activate(pid);
            return self.notify(format!(
                "{} is already open in another window{}",
                display_name(&dir),
                if shown { ", switched to it" } else { "" }
            ));
        }
        if self.project.is_some() {
            match files::spawn_window(&dir) {
                Ok(_) => self.notify(format!("Opened {} in a new window", display_name(&dir))),
                Err(e) => self.warn(format!("Couldn't open a new window: {e}")),
            }
            return;
        }
        crate::recent::record(&dir);
        crate::instances::register(&dir);
        self.project = Some(Project {
            name: display_name(&dir),
            tree: FileTree::new(&dir),
            root: dir,
            files: Vec::new(),
            matcher: None,
            indexing: None,
            build: false,
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
            for f in &files {
                if let Ok(tab) = Tab::open(f) {
                    self.tabs.push(tab);
                }
            }
            if !self.tabs.is_empty() {
                self.active = Some(active.min(self.tabs.len() - 1));
            }
        }
    }

    /// (Re)list the project's files for search, in the background.
    fn index(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.project else { return };
        let (tx, rx) = channel();
        let (root, build, ctx2) = (p.root.clone(), p.build, ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(fstree::collect_files_marked(&root, build));
            ctx2.request_repaint();
        });
        p.indexing = Some(rx);
    }

    /// Pick up the background file walk when it finishes.
    fn poll_index(&mut self) {
        let Some(p) = &mut self.project else { return };
        let Some(files) = p.indexing.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        p.indexing = None;
        p.files = files
            .into_iter()
            .map(|(path, ignored)| (rel_path(&p.root, &path), ignored))
            .collect();
        let names: Vec<String> = p.files.iter().map(|(f, _)| f.clone()).collect();
        p.matcher = Some(FileMatcher::new(&names));
        if self
            .palette
            .as_ref()
            .is_some_and(|pal| pal.mode == Mode::Files)
        {
            self.refresh_items();
        }
    }

    /// Open `path` in a tab, or switch to it if it's already open.
    pub fn open_file(&mut self, path: &Path) {
        self.mru.retain(|p| p != path);
        self.mru.insert(0, path.to_path_buf());
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.path().is_some_and(|p| p == path))
        {
            self.active = Some(i);
            return;
        }
        match Tab::open(path) {
            Ok(tab) => {
                // New tabs open beside the current one.
                let at = self.active.map_or(self.tabs.len(), |a| a + 1);
                self.tabs.insert(at, tab);
                self.active = Some(at);
                self.save_session();
            }
            Err(e) => self.warn(e),
        }
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
            (None, _) => tab.buf.name(),
        }
    }

    fn tab_paths(&self) -> Vec<PathBuf> {
        self.tabs.iter().filter_map(|t| t.path().cloned()).collect()
    }

    pub fn save_session(&self) {
        if let Some(p) = &self.project {
            crate::session::save(&p.root, &self.tab_paths(), self.active.unwrap_or(0));
        }
    }

    fn save_active(&mut self) {
        let Some(i) = self.active else { return };
        self.save_tab(i);
    }

    fn save_tab(&mut self, i: usize) -> bool {
        let Some(tab) = self.tabs.get_mut(i) else {
            return false;
        };
        let name = tab.buf.name();
        match tab.buf.save() {
            Ok(()) => {
                self.notify(format!("Saved {name}"));
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
            .filter(|&i| self.tabs[i].buf.modified)
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
        if to >= 0 && (to as usize) < self.tabs.len() {
            self.tabs.swap(a, to as usize);
            self.active = Some(to as usize);
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
        let previous = self.find.take();
        let mut f = Find::open(&self.tabs[i].buf, i, replace, previous);
        f.jump_from_origin(&mut self.tabs[i].buf);
        self.tabs[i].reveal = true;
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
            mac_cmd: cfg!(target_os = "macos"),
            command: true,
            ctrl: !cfg!(target_os = "macos"),
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
                let token = match b.lang.map(|l| l.name) {
                    Some("Python" | "Shell" | "TOML") => Some("#"),
                    Some("Rust" | "JavaScript" | "TypeScript" | "TSX" | "Go" | "C" | "JSON") => {
                        Some("//")
                    }
                    Some(_) => None,
                    // No language: guess from the extension.
                    None => match b
                        .path
                        .as_ref()
                        .and_then(|p| p.extension())
                        .and_then(|e| e.to_str())
                    {
                        Some(
                            "yml" | "yaml" | "rb" | "sh" | "zsh" | "conf" | "ini" | "env" | "r"
                            | "pl",
                        ) => Some("#"),
                        Some(
                            "java" | "kt" | "swift" | "cs" | "cpp" | "hpp" | "cc" | "dart"
                            | "scala" | "php" | "jsonc" | "json5" | "zig",
                        ) => Some("//"),
                        Some("sql" | "lua" | "hs") => Some("--"),
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
        let n = t.buf.line_count();
        let (l, c) = query.trim().split_once(':').unwrap_or((query.trim(), ""));
        let Ok(line) = l.trim().parse::<usize>() else {
            let row = t.buf.cursor_row() + 1;
            return vec![
                Item::new(
                    icon::GO_TO_LINE,
                    format!("Type a line number, 1 to {n}"),
                    Pick::Line(row - 1, t.buf.cursor_col()),
                )
                .detail(format!("now on line {row}")),
            ];
        };
        let line = line.clamp(1, n.max(1));
        let col = c.trim().parse::<usize>().unwrap_or(1).max(1);
        let text: String = t
            .buf
            .rope
            .line(line - 1)
            .to_string()
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
            let b = &mut t.buf;
            let line = line.min(b.line_count().saturating_sub(1));
            let at = b.rope.line_to_char(line) + col.min(b.line_len_chars(line));
            b.clear_extra_carets();
            b.select_range(at, at);
            b.clear_selection();
            t.reveal = true;
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

    /// Notice files changed by something else (a `git checkout`, a formatter,
    /// vim in a terminal): a clean tab just reloads; one with unsaved edits
    /// asks. A deleted file keeps its text and is marked unsaved, so saving
    /// brings it back. A cheap `stat` per tab, once a second.
    fn poll_file_changes(&mut self) {
        use crate::buffer::DiskStatus;
        if self.prompt.is_some()
            || self.conflict.is_some()
            || self.last_file_check.elapsed() < Duration::from_secs(1)
        {
            return;
        }
        self.last_file_check = Instant::now();
        for i in 0..self.tabs.len() {
            let tab = &mut self.tabs[i];
            let name = tab.buf.name();
            match tab.buf.disk_status() {
                DiskStatus::Unchanged => {}
                DiskStatus::Deleted => {
                    tab.buf.modified = true;
                    tab.buf.forget_disk(); // report once
                    self.notify(format!("{name} was deleted on disk — save to restore it"));
                }
                DiskStatus::Modified if tab.buf.modified => {
                    self.conflict = tab.path().cloned();
                    self.active = Some(i);
                    return; // one question at a time
                }
                DiskStatus::Modified => match tab.reload() {
                    Ok(()) => self.notify(format!("Reloaded {name} (changed on disk)")),
                    Err(e) => self.warn(format!("Couldn't reload {name}: {e:#}")),
                },
            }
        }
    }

    /// The answer to "changed on disk": take the disk version, or keep ours
    /// (and stop asking about that change).
    pub fn answer_conflict(&mut self, reload: bool) {
        let Some(path) = self.conflict.take() else {
            return;
        };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.path() == Some(&path)) else {
            return;
        };
        if !reload {
            return tab.buf.rebaseline_disk();
        }
        let name = tab.buf.name();
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
        crate::termbridge::ensure_intact();
        let Some(path) = crate::termbridge::request_file() else {
            return;
        };
        if !self.watching_requests {
            self.watching_requests = true;
            terminal::watch_requests(ctx);
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
                    self.active_terminal + 1
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
    pub fn close_terminal(&mut self, i: usize) {
        if i < self.terminals.len() {
            self.terminals.remove(i);
            if i < self.active_terminal {
                self.active_terminal -= 1;
            }
            self.after_terminal_removed();
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
    fn request_quit(&mut self, ctx: &egui::Context) {
        self.begin_close(self.tab_paths(), After::Quit, ctx);
    }

    /// Close the tabs for `paths`, asking about each modified one, then do
    /// `after`.
    pub fn begin_close(&mut self, paths: Vec<PathBuf>, after: After, ctx: &egui::Context) {
        self.prompt = Some(Prompt {
            queue: paths,
            after,
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
                Some(i) if self.tabs[i].buf.modified => {
                    self.active = Some(i);
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
            After::CloseFolder => {
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
            }
            After::Quit => {
                self.save_session();
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
        match (answer, idx) {
            (Answer::Cancel, _) => {
                self.prompt = None;
                return;
            }
            // A failed save keeps the tab (and the prompt): never lose work.
            (Answer::Save, Some(i)) if !self.save_tab(i) => return,
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
        let mut p = Palette::new(mode);
        p.opened = ctx.cumulative_pass_nr();
        p.revert = match mode {
            Mode::Themes => Some(Revert::Theme(self.config.theme.clone())),
            Mode::UiFonts | Mode::CodeFonts => Some(Revert::Settings(self.config.settings.clone())),
            _ => None,
        };
        self.palette = Some(p);
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
        let Some(p) = &self.palette else { return };
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
            Mode::Recent => self.recent_items(&query),
            Mode::Terminals => self.terminal_items(&query),
            Mode::Search => {
                if query != self.search.query {
                    self.search.changed = Some(Instant::now());
                }
                self.search_items(&query)
            }
            Mode::Files => self.file_items(&query),
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
                    depth: 0,
                    dim: false,
                    expanded: None,
                    tint: None,
                    pick: Pick::Command(c.command),
                };
                // With nothing typed, the last few picked come first.
                let boost = match recent {
                    Some(r) if query.is_empty() => 1000 - r as i64,
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
            project.build,
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
                    depth: 0,
                    dim: false,
                    expanded: None,
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

    fn terminal_items(&self, query: &str) -> Vec<Item> {
        let names: Vec<String> = self
            .terminals
            .iter()
            .map(|t| t.pane.display_name())
            .collect();
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
                detail: if self.terminals[i].pane.is_running() {
                    "running".into()
                } else {
                    String::new()
                },
                caps: if i < 9 {
                    self.config
                        .keymap
                        .combo_for(Command::JumpTerminal(i as u8 + 1))
                        .map(|k| k.caps())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                depth: 0,
                dim: false,
                expanded: None,
                tint: None,
                pick: Pick::Terminal(i),
            })
            .collect()
    }

    fn recent_items(&self, query: &str) -> Vec<Item> {
        let home = std::env::var("HOME").unwrap_or_default();
        crate::recent::list()
            .into_iter()
            .filter_map(|dir| {
                let name = display_name(&dir);
                fuzzy::score(query, &name)?;
                let full = dir.to_string_lossy();
                let detail = match full.strip_prefix(&home) {
                    Some(rest) if !home.is_empty() => format!("~{rest}"),
                    _ => full.into_owned(),
                };
                let here = self.project.as_ref().is_some_and(|p| p.root == dir);
                let elsewhere = self.open_elsewhere(&dir).is_some();
                let detail = if here {
                    format!("open in this window  ·  {detail}")
                } else if elsewhere {
                    format!("open in another window  ·  {detail}")
                } else {
                    detail
                };
                Some(Item {
                    icon: icon::FOLDER,
                    matched: fuzzy::positions(query, &name),
                    label: name,
                    detail,
                    caps: Vec::new(),
                    depth: 0,
                    dim: elsewhere,
                    expanded: None,
                    tint: None,
                    pick: Pick::Folder(dir),
                })
            })
            // Last, whatever was typed: a folder that isn't in the list.
            .chain(std::iter::once(Item {
                icon: icon::FOLDER_OPEN,
                label: "Open Another Folder…".into(),
                detail: "choose it in Finder".into(),
                caps: self
                    .config
                    .keymap
                    .combo_for(Command::OpenFolder)
                    .map(|k| k.caps())
                    .unwrap_or_default(),
                matched: Vec::new(),
                depth: 0,
                dim: false,
                expanded: None,
                tint: None,
                pick: Pick::Command(Command::OpenFolder),
            }))
            .collect()
    }

    fn file_items(&self, query: &str) -> Vec<Item> {
        let Some(p) = &self.project else {
            return Vec::new();
        };
        if query.trim().is_empty() {
            // Browse: the tree.
            return p
                .tree
                .entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let (icon, tint) = if e.is_dir {
                        (icon::FOLDER, None)
                    } else {
                        icon::for_file(&e.name)
                    };
                    Item {
                        icon,
                        label: e.name.clone(),
                        detail: String::new(),
                        caps: Vec::new(),
                        matched: Vec::new(),
                        depth: e.depth,
                        dim: e.ignored,
                        expanded: e.is_dir.then_some(e.expanded),
                        tint,
                        pick: Pick::Tree(i),
                    }
                })
                .collect();
        }
        let Some(m) = &p.matcher else {
            return Vec::new();
        };
        let recent = |i: usize| {
            self.mru
                .iter()
                .position(|r| *r == p.root.join(&p.files[i].0))
        };
        m.rank(query, recent)
            .into_iter()
            .take(MAX_RESULTS)
            .map(|i| {
                let (rel, ignored) = &p.files[i];
                let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
                let (icon, tint) = icon::for_file(name);
                Item {
                    icon,
                    tint,
                    label: name.to_string(),
                    detail: dir.to_string(),
                    caps: Vec::new(),
                    matched: label_positions(rel, &fuzzy::positions(query, rel)),
                    depth: 0,
                    dim: *ignored,
                    expanded: None,
                    pick: Pick::File(p.root.join(rel)),
                }
            })
            .collect()
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
            Pick::Tree(i) => {
                let entry = self
                    .project
                    .as_ref()
                    .and_then(|p| p.tree.entries.get(i).cloned());
                match entry {
                    Some(e) if e.is_dir => self.palette_tree(i, None),
                    Some(e) => {
                        self.close_palette(true, ctx);
                        self.open_file(&e.path);
                    }
                    None => {}
                }
            }
            Pick::File(path) => {
                self.close_palette(true, ctx);
                self.open_file(&path);
            }
            Pick::Hit(path, line, col, end) => {
                self.close_palette(true, ctx);
                self.open_file(&path);
                if let Some(t) = self.active_tab_mut() {
                    let at = t
                        .buf
                        .rope
                        .line_to_char(line.min(t.buf.line_count().saturating_sub(1)));
                    t.buf.clear_extra_carets();
                    t.buf.select_range(at + col, at + end);
                    t.reveal = true;
                }
                self.focus = Focus::Editor;
            }
            Pick::Line(line, col) => {
                self.close_palette(true, ctx);
                self.go_to(line, col);
            }
            Pick::Terminal(i) => {
                self.close_palette(true, ctx);
                self.active_terminal = i.min(self.terminals.len().saturating_sub(1));
                self.show_terminals(ctx);
            }
            Pick::Folder(dir) => {
                self.close_palette(true, ctx);
                self.open_folder(dir, ctx);
            }
            Pick::Command(c) => {
                self.close_palette(true, ctx);
                self.used.retain(|u| *u != c);
                self.used.insert(0, c);
                self.used.truncate(5);
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

    /// Fold or unfold the tree row at `i`: `Some(true)` expands, `Some(false)`
    /// collapses (or jumps to the parent if already collapsed), `None` toggles.
    pub fn palette_tree(&mut self, i: usize, expand: Option<bool>) {
        let Some(project) = &mut self.project else {
            return;
        };
        let Some(e) = project.tree.entries.get(i) else {
            return;
        };
        let mut select = i;
        match (expand, e.is_dir, e.expanded) {
            (None, true, _) => project.tree.toggle(i),
            (Some(true), true, false) => project.tree.expand(i),
            (Some(false), true, true) => project.tree.collapse(i),
            (Some(false), _, _) => select = project.tree.parent(i).unwrap_or(i),
            _ => {}
        }
        self.refresh_items();
        if let Some(p) = &mut self.palette {
            p.selected = select;
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
            depth: 0,
            dim: false,
            expanded: None,
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

fn display_name(p: &Path) -> String {
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

#[cfg(all(target_os = "macos", not(test)))]
fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open Folder")
        .pick_folder()
}

/// No native picker wired up off macOS yet; Recent Folders and the CLI path
/// work. Tests never open the real panel.
#[cfg(any(not(target_os = "macos"), test))]
fn pick_folder() -> Option<PathBuf> {
    None
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
