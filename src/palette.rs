//! The palette: one searchable overlay list for commands, themes, fonts,
//! recent folders, files, the to-do list, clipboard history, settings and
//! keyboard shortcuts. This is just its state; `app` fills the items
//! and `ui` draws it.

use std::path::PathBuf;

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
    Files,
    Terminals,
    Search,
    Todos,
    Clipboard,
    Settings,
    Keybindings,
    Line,
}

impl Mode {
    pub fn placeholder(self) -> &'static str {
        match self {
            Mode::Commands => "Type a command…",
            Mode::Themes => "Select a theme — ↑↓ to preview",
            Mode::UiFonts => "Select a UI font — ↑↓ to preview",
            Mode::CodeFonts => "Select a code font — ↑↓ to preview",
            Mode::Recent => "Open a recent folder…",
            Mode::Files => "Search files by name — empty to browse",
            Mode::Terminals => "Switch to a terminal…",
            Mode::Line => "Line number — or line:column",
            Mode::Search => "Search in files…",
            Mode::Todos => "Add a task…",
            Mode::Clipboard => "Filter what you've copied…",
            Mode::Settings => "Search settings…",
            Mode::Keybindings => "Search shortcuts by name…",
        }
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
    File(PathBuf),
    Terminal(usize),
    /// A search hit: file, line, and the match's char columns.
    Hit(PathBuf, usize, usize, usize),
    /// A row of the browse tree (index into `FileTree::entries`).
    Tree(usize),
    /// An entry of the to-do list (index into `App::todos`).
    Todo(usize),
    /// An entry of the clipboard history (index into `App::clips`).
    Clip(usize),
    Setting(crate::app::Setting),
    /// A command's row in Keyboard Shortcuts.
    Binding(Command),
    /// Go to Line: line and column, from 0.
    Line(usize, usize),
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
    /// Tree indent level.
    pub depth: usize,
    /// Faded (gitignored).
    pub dim: bool,
    /// Open folder (tree rows): draws an open chevron.
    pub expanded: Option<bool>,
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
            depth: 0,
            dim: false,
            expanded: None,
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
    /// Clipboard history: entries ticked for a multi-paste.
    pub ticked: Vec<usize>,
    /// Keyboard Shortcuts: waiting for the new key for this command.
    pub capture: Option<Command>,
    /// A mouse button went down while it was open. Until then a release
    /// outside is the end of the click that opened it, not a click away.
    pub armed: bool,
    /// The egui pass it opened in; presses from that pass don't arm it.
    pub opened: u64,
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
            capture: None,
            armed: false,
            opened: 0,
        }
    }

    pub fn selected_pick(&self) -> Option<&Pick> {
        self.items.get(self.selected).map(|i| &i.pick)
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
                depth: 0,
                dim: false,
                expanded: None,
                tint: None,
                pick: Pick::Command(Command::Quit),
            });
        }
        p
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
    fn label_positions_shift_to_the_filename() {
        assert_eq!(label_positions("src/main.rs", &[0, 4, 5]), [0, 1]);
    }
}
