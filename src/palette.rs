//! The palette: one searchable overlay list for commands, themes, fonts,
//! recent folders, files, the to-do list, clipboard history, settings and
//! keyboard shortcuts. This is just its state; `app` fills the items
//! and `ui` draws it.

use std::path::{Path, PathBuf};

use crate::config::Settings;
use crate::keymap::Command;
use crate::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Commands,
    Themes,
    UiFonts,
    CodeFonts,
    Recent,
    /// Open Folder: browse to any folder, by typing its path.
    Folders,
    Terminals,
    Search,
    Todos,
    Clipboard,
    Settings,
    Keybindings,
    Line,
    Links,
}

impl Mode {
    pub fn placeholder(self) -> &'static str {
        match self {
            Mode::Commands => "Type a command…",
            Mode::Themes => "Select a theme — ↑↓ to preview",
            Mode::UiFonts => "Select a UI font — ↑↓ to preview",
            Mode::CodeFonts => "Select a code font — ↑↓ to preview",
            Mode::Recent => "Open a recent folder…",
            Mode::Folders => "Type a folder's path…",
            Mode::Terminals => "Switch to a terminal…",
            Mode::Line => "Line number — or line:column",
            Mode::Search => "Search in files…",
            Mode::Todos => "Add a task…",
            Mode::Clipboard => "Filter what you've copied…",
            Mode::Settings => "Search settings…",
            Mode::Keybindings => "Search shortcuts by name…",
            Mode::Links => "Open a link from the terminal…",
        }
    }

    /// Lists where several rows can be ticked and done at once: paste
    /// several clips, open several folders.
    pub fn multi(self) -> bool {
        matches!(self, Mode::Clipboard | Mode::Recent)
    }
}

/// What choosing an item does.
#[derive(Clone, PartialEq, Debug)]
pub enum Pick {
    Command(Command),
    Theme(String),
    UiFont(String),
    CodeFont(String),
    Folder(PathBuf),
    Terminal(usize),
    /// A search hit: file, line, and the match's char columns.
    Hit(PathBuf, usize, usize, usize),
    /// An entry of the to-do list (index into `App::todos`).
    Todo(usize),
    /// An entry of the clipboard history (index into `App::clips`).
    Clip(usize),
    Setting(crate::app::Setting),
    /// A command's row in Keyboard Shortcuts.
    Binding(Command),
    /// Go to Line: line and column, from 0.
    Line(usize, usize),
    /// A link printed in the terminal.
    Url(String),
}

pub struct Item {
    pub icon: &'static str,
    pub label: String,
    /// Secondary text after the label (a folder path, a description).
    pub detail: String,
    /// Keycaps shown on the right.
    pub caps: Vec<String>,
    /// Char indices of `label` that matched the query.
    pub matched: Vec<usize>,
    /// Faded (gitignored).
    pub dim: bool,
    /// The icon's own colour (file types); otherwise the row's.
    pub tint: Option<eframe::egui::Color32>,
    pub pick: Pick,
}

impl Item {
    /// A plain row; set the other fields as needed.
    pub fn new(icon: &'static str, label: impl Into<String>, pick: Pick) -> Item {
        Item {
            icon,
            label: label.into(),
            detail: String::new(),
            caps: Vec::new(),
            matched: Vec::new(),
            dim: false,
            tint: None,
            pick,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Item {
        self.detail = detail.into();
        self
    }
}

/// What a preview changed, so Esc can put it back.
pub enum Revert {
    Theme(Theme),
    Settings(Settings),
}

pub struct Palette {
    pub mode: Mode,
    pub query: String,
    pub selected: usize,
    /// First visible row.
    pub scroll: usize,
    pub items: Vec<Item>,
    pub revert: Option<Revert>,
    /// Rows ticked to do at once (see `Mode::multi`), by what they pick,
    /// so they stay ticked while the search narrows the list; in ticking order.
    pub ticked: Vec<Pick>,
    /// The row last ticked or unticked: where a ⇧-click's range starts.
    pub tick_from: Option<usize>,
    /// Keyboard Shortcuts: waiting for the new key for this command.
    pub capture: Option<Command>,
    /// A mouse button went down while it was open. Until then a release
    /// outside is the end of the click that opened it, not a click away.
    pub armed: bool,
    /// The egui pass it opened in; presses from that pass don't arm it.
    pub opened: u64,
    /// The query was set for you (Open Folder's ⇥): put the cursor at its end.
    pub to_end: bool,
}

impl Palette {
    pub fn new(mode: Mode) -> Self {
        Palette {
            mode,
            query: String::new(),
            selected: 0,
            scroll: 0,
            items: Vec::new(),
            revert: None,
            ticked: Vec::new(),
            tick_from: None,
            capture: None,
            armed: false,
            opened: 0,
            to_end: false,
        }
    }

    pub fn selected_pick(&self) -> Option<&Pick> {
        self.items.get(self.selected).map(|i| &i.pick)
    }

    /// Whether row `i` is ticked.
    pub fn is_ticked(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(|it| self.ticked.contains(&it.pick))
    }

    /// Tick or untick row `i`, if it's a row that can be (not "Open Another
    /// Folder…").
    pub fn set_tick(&mut self, i: usize, on: bool) {
        let Some(pick) = self.items.get(i).map(|it| it.pick.clone()) else { return };
        if !matches!(pick, Pick::Clip(_) | Pick::Folder(_)) {
            return;
        }
        self.ticked.retain(|t| *t != pick);
        if on {
            self.ticked.push(pick);
        }
        self.tick_from = Some(i);
    }

    /// Space, Tab, ⌘-click: flip row `i`.
    pub fn toggle_tick(&mut self, i: usize) {
        self.set_tick(i, !self.is_ticked(i));
    }

    /// ⇧-click: tick every row from the last one ticked (or the selected
    /// one) to `i`.
    pub fn tick_range(&mut self, i: usize) {
        let from = self.tick_from.unwrap_or(self.selected);
        for r in from.min(i)..=from.max(i) {
            self.set_tick(r, true);
        }
        self.tick_from = Some(i);
    }

    /// ⇧↑ / ⇧↓: tick this row and the next one up or down, moving there.
    pub fn extend_tick(&mut self, delta: isize) {
        self.set_tick(self.selected, true);
        self.step(delta);
        self.set_tick(self.selected, true);
    }

    /// Move the selection by `delta`, wrapping.
    pub fn step(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// Keep the selection inside the `visible`-row window.
    pub fn follow(&mut self, visible: usize) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        self.scroll = self.scroll.min(self.items.len().saturating_sub(visible));
    }
}

/// The char indices of `full` that fall in its last path segment, shifted to
/// be relative to that segment — for highlighting matches in a file's name.
pub fn label_positions(full: &str, positions: &[usize]) -> Vec<usize> {
    let label_chars = full.rsplit('/').next().unwrap_or(full).chars().count();
    let start = full.chars().count() - label_chars;
    positions
        .iter()
        .filter(|&&p| p >= start)
        .map(|p| p - start)
        .collect()
}

/// Open Folder's query, a path: the folder whose folders to list, and the
/// start of a name to narrow them by. `~/code/` lists ~/code; `~/code/ap`
/// lists ~/code's folders matching "ap". `~` is the home folder, and a path
/// that doesn't start at a root starts there too.
pub fn folder_query(query: &str, home: &Path) -> (PathBuf, String) {
    let sep = |c: char| std::path::is_separator(c);
    let rest = match query.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(sep) => rest.trim_start_matches(sep),
        _ => query,
    };
    // Split the text, not a `Path`: that drops a trailing `.`, the start of a
    // hidden folder's name.
    let (dir, name) = match rest.rfind(sep) {
        Some(i) => rest.split_at(i + 1),
        None => ("", rest),
    };
    (home.join(dir), name.to_string()) // `dir` from a root replaces `home`
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(n: usize) -> Palette {
        let mut p = Palette::new(Mode::Commands);
        for i in 0..n {
            p.items.push(Item {
                icon: "",
                label: i.to_string(),
                detail: String::new(),
                caps: Vec::new(),
                matched: Vec::new(),
                dim: false,
                tint: None,
                pick: Pick::Command(Command::Quit),
            });
        }
        p
    }

    #[test]
    fn folder_query_splits_a_typed_path() {
        let home = Path::new("/home/me");
        let q = |s: &str| folder_query(s, home);
        // Paths compare by component: the same on every system.
        let s = |d: &str, f: &str| (PathBuf::from(d), f.to_string());
        assert_eq!(q(""), s("/home/me", ""));
        assert_eq!(q("~"), s("/home/me", ""));
        assert_eq!(q("~/"), s("/home/me", ""));
        assert_eq!(q("~/code/"), s("/home/me/code/", ""));
        assert_eq!(q("~/code/ap"), s("/home/me/code", "ap"));
        assert_eq!(q("code"), s("/home/me", "code"));
        assert_eq!(q("~me"), s("/home/me", "~me"));
        assert_eq!(q("/"), s("/", ""));
        assert_eq!(q("/usr/lo"), s("/usr", "lo"));
        assert_eq!(q("~/code/."), s("/home/me/code", "."));
    }

    #[test]
    fn step_wraps_and_follow_scrolls() {
        let mut p = palette(20);
        p.step(-1);
        assert_eq!(p.selected, 19);
        p.follow(8);
        assert_eq!(p.scroll, 12);
        p.step(1);
        p.follow(8);
        assert_eq!((p.selected, p.scroll), (0, 0));
    }

    #[test]
    fn ticking_rows() {
        let mut p = palette(6);
        for (i, it) in p.items.iter_mut().enumerate() {
            it.pick = Pick::Folder(PathBuf::from(i.to_string()));
        }
        p.items[5].pick = Pick::Command(Command::OpenFolder);
        p.toggle_tick(1);
        p.toggle_tick(3);
        p.toggle_tick(1);
        assert!(!p.is_ticked(1) && p.is_ticked(3));
        // ⇧-click: from the last one ticked or unticked (1) to here.
        p.tick_range(0);
        assert_eq!((0..5).filter(|i| p.is_ticked(*i)).collect::<Vec<_>>(), [0, 1, 3]);
        p.tick_range(4);
        assert_eq!((0..5).filter(|i| p.is_ticked(*i)).count(), 5);
        // "Open Another Folder…" is never ticked.
        p.toggle_tick(5);
        assert!(!p.is_ticked(5));
        // ⇧↓ ticks where it starts and where it lands; ticks follow the
        // pick, not the row, when the list changes.
        p.ticked.clear();
        p.selected = 3;
        p.extend_tick(1);
        assert_eq!(p.selected, 4);
        assert!(p.is_ticked(3) && p.is_ticked(4));
        p.items.remove(0);
        assert!(p.is_ticked(2) && p.is_ticked(3) && !p.is_ticked(1));
    }

    #[test]
    fn label_positions_shift_to_the_filename() {
        assert_eq!(label_positions("src/main.rs", &[0, 4, 5]), [0, 1]);
    }
}
