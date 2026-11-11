//! The extras: toasts, clipboard history (⌥V), the to-do list (⌥D), the
//! Settings and Keyboard Shortcuts screens, and knowing which folders other
//! windows have open.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui;

use super::{App, Focus};
use crate::config::{self, TERMINAL_FPS};
use crate::fonts;
use crate::fuzzy;
use crate::keymap::{COMMANDS, Combo, Command};
use crate::palette::{Item, Mode, Pick};
use crate::todo::{self, Entry};
use crate::ui::icon;

/// A notification in the corner.
pub struct Toast {
    pub msg: String,
    pub error: bool,
    pub born: Instant,
}

impl Toast {
    /// Errors stay up longer: they're the ones worth reading.
    pub fn ttl(&self) -> Duration {
        Duration::from_millis(if self.error { 6000 } else { 3500 })
    }
}

/// Toasts shown at once; older ones make way.
const MAX_TOASTS: usize = 4;
/// Clipboard history kept (memory only).
const MAX_CLIPS: usize = 50;

/// A row of the Settings screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    Theme,
    UiFont,
    CodeFont,
    UiFontSize,
    CodeFontSize,
    TerminalFps,
    WordWrap,
    CurrentLine,
    IndentGuides,
    Sidebar,
}

impl Setting {
    pub const ALL: [Setting; 10] = [
        Setting::Theme,
        Setting::UiFont,
        Setting::CodeFont,
        Setting::UiFontSize,
        Setting::CodeFontSize,
        Setting::TerminalFps,
        Setting::WordWrap,
        Setting::CurrentLine,
        Setting::IndentGuides,
        Setting::Sidebar,
    ];

    /// Its key in `settings.json`.
    pub fn key(self) -> &'static str {
        match self {
            Setting::Theme => "theme",
            Setting::UiFont => "uiFont",
            Setting::CodeFont => "codeFont",
            Setting::UiFontSize => "uiFontSize",
            Setting::CodeFontSize => "codeFontSize",
            Setting::TerminalFps => "terminalFps",
            Setting::WordWrap => "wordWrap",
            Setting::CurrentLine => "currentLine",
            Setting::IndentGuides => "indentGuides",
            Setting::Sidebar => "sidebar",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Setting::Theme => "Theme",
            Setting::UiFont => "UI font",
            Setting::CodeFont => "Code font",
            Setting::UiFontSize => "UI font size",
            Setting::CodeFontSize => "Code font size",
            Setting::TerminalFps => "Terminal frame rate (when you aren't typing)",
            Setting::WordWrap => "Word wrap",
            Setting::CurrentLine => "Highlight the current line",
            Setting::IndentGuides => "Indent guides",
            Setting::Sidebar => "Show file tree",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Setting::Theme => icon::THEME,
            Setting::UiFont | Setting::CodeFont | Setting::UiFontSize | Setting::CodeFontSize => {
                icon::FONT
            }
            Setting::TerminalFps => icon::PULSE,
            Setting::WordWrap => icon::WRAP,
            Setting::CurrentLine | Setting::IndentGuides => icon::EDIT,
            Setting::Sidebar => icon::SIDEBAR,
        }
    }
}

impl App {
    /// Terminals asking for the user (see `terminalpane::watch`): tell the
    /// watchers who can see what, and show what they raised: a toast with
    /// the key to get there, and a bell on the terminal's tab.
    pub(crate) fn poll_attention(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering::Relaxed;
        let focused = ctx.input(|i| i.focused);
        // Whether you're at this window is the window's own say: a browser's
        // copy always counts itself focused, and while one was open it kept
        // telling the watchers you were looking, so nothing was raised.
        let own = !self.remote_pass;
        if own {
            crate::terminalpane::APP_FOCUSED.store(focused, Relaxed);
        }
        for i in 0..self.terminals.len() {
            let shown = self.terminal_open && (self.terminal_grid || i == self.active_terminal);
            let t = &mut self.terminals[i];
            t.pane.refresh_foreground();
            if own {
                t.pane.watch.looking.store(focused && shown, Relaxed);
            }
            let name = t.pane.display_name();
            for message in t.pane.take_fired() {
                let key = self
                    .config
                    .keymap
                    .combo_for(Command::SwitchTerminal)
                    .map(|k| format!(" — {} to switch", k.caps().concat()))
                    .unwrap_or_default();
                self.missed.push(format!("{name}: {message}{key}"));
            }
        }
        if focused {
            for msg in std::mem::take(&mut self.missed) {
                self.notify(msg);
            }
        }
    }

    // ---- toasts ---------------------------------------------------------

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.toast(msg.into(), false);
    }

    /// A notification about something that went wrong.
    pub fn warn(&mut self, msg: impl Into<String>) {
        self.toast(msg.into(), true);
    }

    fn toast(&mut self, msg: String, error: bool) {
        self.toasts.retain(|t| t.msg != msg);
        self.toasts.push(Toast {
            msg,
            error,
            born: Instant::now(),
        });
        let excess = self.toasts.len().saturating_sub(MAX_TOASTS);
        self.toasts.drain(..excess);
    }

    pub fn expire_toasts(&mut self) {
        self.toasts.retain(|t| t.born.elapsed() < t.ttl());
    }

    #[cfg(test)]
    pub fn note_text(&self) -> Option<&str> {
        self.toasts.last().map(|t| t.msg.as_str())
    }

    // ---- clipboard history ------------------------------------------------

    /// Put `text` on the clipboard and remember it for ⌥V. Every copy in the
    /// app goes through here.
    pub fn copy(&mut self, ctx: &egui::Context, text: String) {
        if text.is_empty() {
            return;
        }
        ctx.copy_text(text.clone());
        if !text.trim().is_empty() {
            self.clips.retain(|c| *c != text);
            self.clips.insert(0, text);
            self.clips.truncate(MAX_CLIPS);
        }
    }

    pub(super) fn clip_items(&self, query: &str) -> Vec<Item> {
        let q = query.to_lowercase();
        self.clips
            .iter()
            .enumerate()
            .filter(|(_, c)| q.is_empty() || c.to_lowercase().contains(&q))
            .map(|(i, text)| {
                let mut it = Item::new(icon::COPY, preview(text), Pick::Clip(i));
                let lines = text.lines().count();
                if lines > 1 {
                    it.detail = format!("{lines} lines");
                }
                it
            })
            .collect()
    }

    /// The open list's ticked rows (see `Palette::ticked`).
    pub(crate) fn ticked(&self) -> Vec<Pick> {
        self.palette.as_ref().map(|p| p.ticked.clone()).unwrap_or_default()
    }

    /// Enter: paste the ticked entries (top to bottom, one per line), or the
    /// selected one, where the keyboard was — and make it the clipboard.
    pub fn clip_paste(&mut self, ctx: &egui::Context) {
        let Some(pal) = &self.palette else { return };
        let mut picks: Vec<usize> = pal
            .ticked
            .iter()
            .filter_map(|t| if let Pick::Clip(i) = t { Some(*i) } else { None })
            .collect();
        picks.sort();
        if picks.is_empty()
            && let Some(Pick::Clip(i)) = pal.selected_pick()
        {
            picks.push(*i);
        }
        let text = picks
            .iter()
            .filter_map(|i| self.clips.get(*i).cloned())
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            return;
        }
        self.close_palette(true, ctx);
        if self.terminal_focused() {
            if let Some(t) = self.terminal_mut() {
                t.pane.paste(&text);
            }
        } else if let Some(tab) = self.active_tab_mut() {
            tab.paste(&text);
            tab.reveal = true;
        } else {
            return self.notify("Open a file or a terminal to paste into");
        }
        if picks.len() > 1 {
            self.notify(format!("Pasted {} entries", picks.len()));
        }
        self.copy(ctx, text);
    }

    pub fn clip_clear(&mut self) {
        self.clips.clear();
        if let Some(p) = &mut self.palette {
            p.ticked.clear();
        }
        self.refresh_items();
        self.notify("Clipboard history cleared");
    }

    // ---- to-do list -------------------------------------------------------

    /// ⌥D: the list, fresh from `todos.md` (which you may have edited).
    pub fn open_todos(&mut self, ctx: &egui::Context) {
        self.todos = todo::load_from(&todo::todos_path());
        self.open_palette(Mode::Todos, ctx);
    }

    pub(super) fn todo_items(&self) -> Vec<Item> {
        self.todos
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                Entry::Task { done, text } => {
                    let mut it = Item::new(
                        if *done { icon::PASS } else { icon::CIRCLE },
                        text.clone(),
                        Pick::Todo(i),
                    );
                    it.dim = *done;
                    Some(it)
                }
                // Headings and notes show (they're your structure), blank lines don't.
                Entry::Raw(line) if !line.trim().is_empty() => {
                    let mut it = Item::new(
                        "",
                        line.trim_start_matches('#').trim().to_string(),
                        Pick::Todo(i),
                    );
                    it.dim = true;
                    Some(it)
                }
                Entry::Raw(_) => None,
            })
            .collect()
    }

    /// Add every line of `text` as a task (a pasted list becomes several).
    pub fn todo_add(&mut self, text: &str) {
        for e in todo::parse(text) {
            if let Entry::Task { text, .. } = e {
                todo::add(&mut self.todos, &text);
            }
        }
        self.save_todos();
        if let Some(p) = &mut self.palette {
            p.query.clear();
        }
        self.refresh_items();
        if let Some(p) = &mut self.palette {
            p.selected = p.items.len().saturating_sub(1);
        }
    }

    pub fn todo_toggle(&mut self, i: usize) {
        todo::toggle(&mut self.todos, i);
        self.save_todos();
    }

    /// ⌘⌫ removes the selected task; ⇧⌘⌫ every finished one.
    pub fn todo_remove(&mut self, all_done: bool) {
        if all_done {
            let n = todo::clear_done(&mut self.todos);
            self.notify(format!(
                "Cleared {n} finished task{}",
                if n == 1 { "" } else { "s" }
            ));
        } else if let Some(Pick::Todo(i)) = self
            .palette
            .as_ref()
            .and_then(|p| p.selected_pick().cloned())
        {
            todo::remove(&mut self.todos, i);
        }
        self.save_todos();
    }

    /// ⌘E: the list as a plain markdown file, in a tab.
    pub fn todo_edit_file(&mut self, ctx: &egui::Context) {
        let path = todo::todos_path();
        if !path.exists() {
            self.save_todos();
        }
        self.close_palette(true, ctx);
        self.open_file(&path);
        self.focus = Focus::Editor;
    }

    fn save_todos(&mut self) {
        if let Err(e) = todo::save_to(&todo::todos_path(), &self.todos) {
            self.warn(format!("Couldn't save the to-do list: {e}"));
        }
        self.refresh_items();
    }

    /// "2 of 5 done", for the footer.
    pub fn todo_summary(&self) -> String {
        let (done, total) = todo::counts(&self.todos);
        if total == 0 {
            String::new()
        } else {
            format!("{done} of {total} done")
        }
    }

    // ---- settings ---------------------------------------------------------

    pub(super) fn setting_items(&self, query: &str) -> Vec<Item> {
        let mut items: Vec<Item> = Setting::ALL
            .iter()
            .filter(|s| fuzzy::score(query, s.title()).is_some())
            .map(|&s| {
                let mut it = Item::new(s.icon(), s.title(), Pick::Setting(s));
                it.matched = fuzzy::positions(query, s.title());
                it.caps = vec![self.setting_value(s)];
                // The toggles with a key of their own say so.
                let key = match s {
                    Setting::WordWrap => Some(Command::ToggleWordWrap),
                    Setting::Sidebar => Some(Command::ToggleSidebar),
                    _ => None,
                };
                let key = key
                    .and_then(|c| self.config.keymap.combo_for(c))
                    .map(|k| format!("also {}", k.caps().concat()));
                let project = self
                    .config
                    .project_keys
                    .iter()
                    .any(|k| k == s.key())
                    .then(|| "this project".to_string());
                it.detail = [project, key]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                it
            })
            .collect();
        let mut links = vec![
            Command::ToggleRemoteAccess,
            Command::ShowRemoteAddress,
            Command::Keybindings,
            Command::OpenSettings,
        ];
        if self.project.is_some() {
            links.push(Command::OpenProjectSettings);
        }
        for c in links {
            let title = COMMANDS
                .iter()
                .find(|i| i.command == c)
                .map_or("", |i| i.title);
            if fuzzy::score(query, title).is_some() {
                let mut it = Item::new(icon::for_command(c), title, Pick::Command(c));
                it.matched = fuzzy::positions(query, title);
                it.caps = self
                    .config
                    .keymap
                    .combo_for(c)
                    .map(|k| k.caps())
                    .unwrap_or_default();
                items.push(it);
            }
        }
        items
    }

    fn setting_value(&self, s: Setting) -> String {
        let st = &self.config.settings;
        let on = |b: bool| if b { "On" } else { "Off" }.to_string();
        match s {
            Setting::Theme => self.config.theme.name.clone(),
            Setting::UiFont => display_font(&st.ui_font),
            Setting::CodeFont => display_font(&st.code_font),
            Setting::UiFontSize => format!("{}", st.ui_font_size),
            Setting::CodeFontSize => format!("{}", st.code_font_size),
            Setting::TerminalFps => format!("{} fps", st.terminal_fps),
            Setting::WordWrap => on(st.word_wrap),
            Setting::CurrentLine => on(st.current_line),
            Setting::IndentGuides => on(st.indent_guides),
            Setting::Sidebar => on(st.sidebar),
        }
    }

    /// ← / → on a setting: the previous / next value, applied and saved.
    pub fn change_setting(&mut self, s: Setting, delta: i32, ctx: &egui::Context) {
        let st = self.config.settings.clone();
        let cycle = |names: Vec<String>, current: &str| {
            let at = names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(current))
                .unwrap_or(0) as i32;
            names
                .get((at + delta).rem_euclid(names.len().max(1) as i32) as usize)
                .cloned()
                .unwrap_or_default()
        };
        let value: serde_json::Value = match s {
            Setting::Theme => {
                let names = self.config.themes.iter().map(|t| t.name.clone()).collect();
                cycle(names, &self.config.theme.name).into()
            }
            Setting::UiFont => {
                cycle(fonts::ui_names().map(String::from).collect(), &st.ui_font).into()
            }
            Setting::CodeFont => cycle(
                fonts::code_names().map(String::from).collect(),
                &st.code_font,
            )
            .into(),
            Setting::UiFontSize => (st.ui_font_size + delta as f32).clamp(9.0, 32.0).into(),
            Setting::CodeFontSize => (st.code_font_size + delta as f32).clamp(9.0, 40.0).into(),
            Setting::TerminalFps => {
                let at = TERMINAL_FPS
                    .iter()
                    .position(|f| *f == st.terminal_fps)
                    .unwrap_or(0) as i32;
                TERMINAL_FPS[(at + delta).clamp(0, TERMINAL_FPS.len() as i32 - 1) as usize].into()
            }
            Setting::WordWrap => (!st.word_wrap).into(),
            Setting::CurrentLine => (!st.current_line).into(),
            Setting::IndentGuides => (!st.indent_guides).into(),
            Setting::Sidebar => (!st.sidebar).into(),
        };
        self.save_choice_quietly(s.key(), value, ctx);
        self.refresh_items();
    }

    /// Enter on a setting: themes and fonts open their (previewing) picker;
    /// the rest step to the next value.
    pub fn activate_setting(&mut self, s: Setting, ctx: &egui::Context) {
        match s {
            Setting::Theme => self.open_palette(Mode::Themes, ctx),
            Setting::UiFont => self.open_palette(Mode::UiFonts, ctx),
            Setting::CodeFont => self.open_palette(Mode::CodeFonts, ctx),
            _ => self.change_setting(s, 1, ctx),
        }
    }

    /// Create (if needed) and open this project's `.oxru/settings.json`.
    pub fn open_project_settings(&mut self, ctx: &egui::Context) {
        let Some(root) = self.root().map(Path::to_path_buf) else {
            return;
        };
        let path = config::project_settings_path(&root);
        if !path.exists() {
            let starter = "// Settings for this project only. Any key from the global settings.json works here,\n\
                           // e.g. \"wordWrap\": true or \"codeFontSize\": 16.\n{\n}\n";
            let made = path
                .parent()
                .map(std::fs::create_dir_all)
                .transpose()
                .and_then(|_| std::fs::write(&path, starter));
            if let Err(e) = made {
                return self.warn(format!("Couldn't create {}: {e}", path.display()));
            }
        }
        self.close_palette(true, ctx);
        self.open_file(&path);
        self.focus = Focus::Editor;
    }

    // ---- keyboard shortcuts -----------------------------------------------

    pub(super) fn binding_items(&self, query: &str) -> Vec<Item> {
        let km = &self.config.keymap;
        let title_of = |c: Command| {
            COMMANDS
                .iter()
                .find(|i| i.command == c)
                .map_or("", |i| i.title)
        };
        COMMANDS
            .iter()
            .filter_map(|info| {
                let combos = km.combos_for(info.command);
                let keys: Vec<String> = combos.iter().map(|k| k.caps().concat()).collect();
                let by_key =
                    !query.is_empty() && keys.iter().any(|k| k.eq_ignore_ascii_case(query));
                if !by_key && fuzzy::score(query, info.title).is_none() {
                    return None;
                }
                let conflicts = km.conflicts(info.command);
                let mut it = Item::new(
                    icon::for_command(info.command),
                    info.title,
                    Pick::Binding(info.command),
                );
                it.matched = fuzzy::positions(query, info.title);
                it.caps = combos.first().map(|k| k.caps()).unwrap_or_default();
                it.detail = if !conflicts.is_empty() {
                    it.icon = icon::WARNING;
                    let names: Vec<&str> = conflicts.iter().map(|c| title_of(*c)).collect();
                    format!("same key as {}", names.join(", "))
                } else if combos.is_empty() {
                    "no shortcut".into()
                } else if keys.len() > 1 {
                    format!("also {}", keys[1..].join(", "))
                } else if km.is_custom(info.command) {
                    "custom".into()
                } else {
                    String::new()
                };
                Some(it)
            })
            .collect()
    }

    /// Whether Keyboard Shortcuts is waiting for a new key.
    pub fn capturing(&self) -> bool {
        self.palette.as_ref().is_some_and(|p| p.capture.is_some())
    }

    /// The key pressed while capturing becomes `command`'s only shortcut,
    /// saved to `keybindings.json`. A clash is allowed but called out.
    pub fn record_binding(&mut self, command: Command, combo: Combo, ctx: &egui::Context) {
        self.write_binding(command, Some(combo), ctx);
    }

    /// ⌘⌫ in Keyboard Shortcuts: back to the default keys.
    pub fn reset_binding(&mut self, ctx: &egui::Context) {
        if let Some(Pick::Binding(c)) = self
            .palette
            .as_ref()
            .and_then(|p| p.selected_pick().cloned())
        {
            self.write_binding(c, None, ctx);
        }
    }

    fn write_binding(&mut self, command: Command, combo: Option<Combo>, ctx: &egui::Context) {
        let Some(info) = COMMANDS.iter().find(|i| i.command == command) else {
            return;
        };
        if let Err(e) = config::save_binding(info.id, combo.map(Combo::to_config).as_deref()) {
            return self.warn(format!("Couldn't save the shortcut: {e}"));
        }
        self.reload_config(ctx);
        self.refresh_items();
        let keys = self
            .config
            .keymap
            .combo_for(command)
            .map(|k| k.caps().concat())
            .unwrap_or_else(|| "no key".into());
        let clash: Vec<&str> = self
            .config
            .keymap
            .conflicts(command)
            .iter()
            .filter_map(|c| COMMANDS.iter().find(|i| i.command == *c).map(|i| i.title))
            .collect();
        if clash.is_empty() {
            self.notify(format!("{}: {keys}", info.title));
        } else {
            self.warn(format!(
                "{keys} is also {} — change one of them",
                clash.join(", ")
            ));
        }
    }

    // ---- other windows ----------------------------------------------------

    /// Whether another Oxru window has `dir` open.
    pub fn open_elsewhere(&self, dir: &Path) -> Option<i32> {
        let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let dir = canon(dir);
        crate::instances::open_folders()
            .into_iter()
            .find(|(_, o)| canon(o) == dir)
            .map(|(pid, _)| pid)
    }
}

/// A font setting for display: a path shows as its file name.
fn display_font(f: &str) -> String {
    Path::new(f)
        .file_stem()
        .filter(|_| f.contains('/'))
        .map_or_else(|| f.to_string(), |s| s.to_string_lossy().into_owned())
}

/// One line for the clipboard list: the first non-blank line, squeezed.
fn preview(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let squeezed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    match squeezed.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &squeezed[..at]),
        None => squeezed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_one_squeezed_line() {
        assert_eq!(preview("\n   fn  main() {\n}\n"), "fn main() {");
        assert!(preview(&"x".repeat(300)).ends_with('…'));
    }

    /// Every row of the Settings screen is a real settings.json key, and the
    /// starter settings.json documents it.
    #[test]
    fn every_setting_is_a_real_documented_key() {
        let template = crate::config::settings_template();
        for s in Setting::ALL {
            let one = format!("{{ \"{}\": null }}", s.key());
            let parsed: Result<crate::config::Settings, _> = json5::from_str(&one);
            // `null` is the wrong type for every key; an unknown key says so.
            let err = parsed.err().map(|e| e.to_string()).unwrap_or_default();
            assert!(!err.contains("unknown field"), "{}: {err}", s.key());
            assert!(template.contains(&format!("\"{}\"", s.key())), "{} isn't in the template", s.key());
        }
    }

    #[test]
    fn font_paths_show_as_names() {
        assert_eq!(display_font("/fonts/My Mono.ttf"), "My Mono");
        assert_eq!(display_font("Inter"), "Inter");
    }
}
