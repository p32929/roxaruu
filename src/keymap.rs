//! Commands and the keys bound to them. Every action is a named command with
//! default keys; `keybindings.json` remaps them, VS Code style:
//!
//! ```json
//! [ { "key": "ctrl+shift+q", "command": "app.quit" } ]
//! ```
//!
//! Listing a command replaces *all* its default keys; `"key": ""` unbinds it.
//! `ctrl` also matches ⌘ on macOS (Oxru's rule: ⌘ works anywhere Ctrl does).

use eframe::egui::{Key, Modifiers};
use serde::Deserialize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    Palette,
    GoToFile,
    OpenFolder,
    RecentFolders,
    CloseFolder,
    SelectTheme,
    SelectUiFont,
    SelectCodeFont,
    Settings,
    Keybindings,
    OpenSettings,
    OpenKeybindings,
    OpenProjectSettings,
    Help,
    Todos,
    ClipboardHistory,
    OpenConfigFolder,
    Save,
    SaveAll,
    CloseTab,
    CloseAllTabs,
    ReopenTab,
    NewFile,
    NewFolder,
    RenameFile,
    DeleteFile,
    RevealInFinder,
    CopyPath,
    CopyRelativePath,
    ToggleBuildFolders,
    NextTab,
    PrevTab,
    MoveTabLeft,
    MoveTabRight,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Find,
    Replace,
    AddNextOccurrence,
    AddCursorAbove,
    AddCursorBelow,
    SearchInFiles,
    ToggleSplit,
    ToggleWordWrap,
    ToggleSidebar,
    FocusSidebar,
    GoToLine,
    ToggleComment,
    MoveLineUp,
    MoveLineDown,
    DuplicateLineUp,
    DuplicateLineDown,
    DeleteLine,
    InsertLineBelow,
    InsertLineAbove,
    IndentLines,
    OutdentLines,
    GoToTab(u8),
    SwitchFocus,
    ClearTerminal,
    ToggleTerminal,
    NewTerminal,
    CloseTerminal,
    ToggleTerminalGrid,
    MaximizeTerminal,
    SwitchTerminal,
    JumpTerminal(u8),
    Quit,
}

pub struct CommandInfo {
    pub command: Command,
    pub id: &'static str,
    pub title: &'static str,
    pub keys: &'static [&'static str],
}

macro_rules! commands {
    ($($command:ident $(($arg:literal))?, $id:literal, $title:literal, [$($key:literal),*];)*) => {
        pub const COMMANDS: &[CommandInfo] = &[
            $(CommandInfo { command: Command::$command $(($arg))?, id: $id, title: $title, keys: &[$($key),*] },)*
        ];
    };
}

commands! {
    Palette, "app.commandPalette", "Command Palette", ["ctrl+shift+p"];
    Help, "app.help", "Help: Keyboard Shortcuts", ["f1"];
    GoToFile, "file.goTo", "Go to File", ["alt+f", "ctrl+p"];
    OpenFolder, "folder.open", "Open Folder", ["ctrl+o"];
    RecentFolders, "folder.recent", "Recent Folders", ["alt+o"];
    CloseFolder, "folder.close", "Close Folder", [];
    SelectTheme, "theme.select", "Select Theme", [];
    SelectUiFont, "font.selectUi", "Select UI Font", [];
    SelectCodeFont, "font.selectCode", "Select Code Font", [];
    Settings, "app.settings", "Settings", ["ctrl+,"];
    Keybindings, "app.keybindings", "Edit Keyboard Shortcuts", [];
    OpenSettings, "app.openSettings", "Open Settings (JSON)", [];
    OpenKeybindings, "app.openKeybindings", "Open Keyboard Shortcuts (JSON)", [];
    OpenProjectSettings, "app.openProjectSettings", "Open Project Settings (JSON)", [];
    Todos, "todo.open", "To-do List", ["alt+d"];
    ClipboardHistory, "clipboard.history", "Clipboard History", ["alt+v"];
    OpenConfigFolder, "app.openConfigFolder", "Open Config Folder", [];
    Save, "file.save", "Save", ["ctrl+s"];
    SaveAll, "file.saveAll", "Save All", ["ctrl+shift+s"];
    CloseTab, "tab.close", "Close Tab", ["ctrl+w"];
    CloseAllTabs, "tab.closeAll", "Close All Tabs", ["ctrl+shift+w"];
    ReopenTab, "tab.reopen", "Reopen Closed Tab", ["ctrl+shift+t"];
    NewFile, "file.new", "New File…", ["ctrl+n"];
    NewFolder, "file.newFolder", "New Folder…", ["ctrl+shift+n"];
    RenameFile, "file.rename", "Rename File…", ["f2"];
    DeleteFile, "file.delete", "Move to Trash", ["ctrl+backspace"];
    RevealInFinder, "file.reveal", "Reveal in Finder", ["ctrl+alt+r"];
    CopyPath, "file.copyPath", "Copy Path", ["ctrl+alt+c"];
    CopyRelativePath, "file.copyRelativePath", "Copy Relative Path", ["ctrl+alt+shift+c"];
    ToggleBuildFolders, "search.toggleBuildFolders", "Search: Include / Skip Build Folders", ["alt+h"];
    NextTab, "tab.next", "Next Tab", ["ctrl+tab"];
    PrevTab, "tab.previous", "Previous Tab", ["ctrl+shift+tab"];
    MoveTabLeft, "tab.moveLeft", "Move Tab Left", ["ctrl+shift+,"];
    MoveTabRight, "tab.moveRight", "Move Tab Right", ["ctrl+shift+."];
    GoToTab(1), "tab.goTo1", "Go to Tab 1", ["ctrl+1"];
    GoToTab(2), "tab.goTo2", "Go to Tab 2", ["ctrl+2"];
    GoToTab(3), "tab.goTo3", "Go to Tab 3", ["ctrl+3"];
    GoToTab(4), "tab.goTo4", "Go to Tab 4", ["ctrl+4"];
    GoToTab(5), "tab.goTo5", "Go to Tab 5", ["ctrl+5"];
    GoToTab(6), "tab.goTo6", "Go to Tab 6", ["ctrl+6"];
    GoToTab(7), "tab.goTo7", "Go to Tab 7", ["ctrl+7"];
    GoToTab(8), "tab.goTo8", "Go to Tab 8", ["ctrl+8"];
    GoToTab(9), "tab.goTo9", "Go to Last Tab", ["ctrl+9"];
    Undo, "edit.undo", "Undo", ["ctrl+z"];
    Redo, "edit.redo", "Redo", ["ctrl+shift+z", "ctrl+y"];
    Copy, "edit.copy", "Copy", ["ctrl+c"];
    Cut, "edit.cut", "Cut", ["ctrl+x"];
    Paste, "edit.paste", "Paste", ["ctrl+v"];
    SelectAll, "edit.selectAll", "Select All", ["ctrl+a"];
    Find, "edit.find", "Find", ["ctrl+f"];
    Replace, "edit.replace", "Replace", ["ctrl+r"];
    AddNextOccurrence, "edit.addNextOccurrence", "Add Cursor at Next Match", ["ctrl+d"];
    AddCursorAbove, "edit.addCursorAbove", "Add Cursor Above", ["ctrl+alt+up"];
    AddCursorBelow, "edit.addCursorBelow", "Add Cursor Below", ["ctrl+alt+down"];
    GoToLine, "edit.goToLine", "Go to Line…", ["ctrl+l"];
    ToggleComment, "edit.toggleComment", "Comment / Uncomment Line", ["ctrl+/"];
    MoveLineUp, "edit.moveLineUp", "Move Line Up", ["alt+up"];
    MoveLineDown, "edit.moveLineDown", "Move Line Down", ["alt+down"];
    DuplicateLineUp, "edit.copyLineUp", "Duplicate Line Up", ["alt+shift+up"];
    DuplicateLineDown, "edit.copyLineDown", "Duplicate Line Down", ["alt+shift+down"];
    DeleteLine, "edit.deleteLine", "Delete Line", ["ctrl+shift+k"];
    InsertLineBelow, "edit.insertLineBelow", "New Line Below", ["ctrl+enter"];
    InsertLineAbove, "edit.insertLineAbove", "New Line Above", ["ctrl+shift+enter"];
    IndentLines, "edit.indent", "Indent Line", ["ctrl+]"];
    OutdentLines, "edit.outdent", "Outdent Line", ["ctrl+["];
    SearchInFiles, "search.inFiles", "Search in Files", ["ctrl+shift+f"];
    ToggleSplit, "view.split", "Split View On / Off (All Open Files)", ["ctrl+\\"];
    ToggleWordWrap, "view.wordWrap", "Word Wrap On / Off", ["alt+z"];
    ToggleSidebar, "view.sidebar", "Show / Hide File Tree", ["ctrl+b"];
    FocusSidebar, "view.focusSidebar", "Focus File Tree (on the Current File)", ["ctrl+shift+e"];
    SwitchFocus, "view.switchFocus", "Switch Between Editor and Terminal", ["ctrl+j"];
    ToggleTerminal, "terminal.toggle", "Show / Hide Terminal", ["alt+t"];
    NewTerminal, "terminal.new", "New Terminal", ["alt+n"];
    ClearTerminal, "terminal.clear", "Clear Terminal", ["ctrl+k"];
    CloseTerminal, "terminal.close", "Close Terminal", ["alt+w"];
    ToggleTerminalGrid, "terminal.grid", "Terminals Side by Side / as Tabs", ["alt+g"];
    MaximizeTerminal, "terminal.maximize", "Maximize / Restore Terminal", ["alt+m"];
    SwitchTerminal, "terminal.switch", "Switch to Terminal…", ["alt+k"];
    JumpTerminal(1), "terminal.jump1", "Go to Terminal 1", ["alt+1"];
    JumpTerminal(2), "terminal.jump2", "Go to Terminal 2", ["alt+2"];
    JumpTerminal(3), "terminal.jump3", "Go to Terminal 3", ["alt+3"];
    JumpTerminal(4), "terminal.jump4", "Go to Terminal 4", ["alt+4"];
    JumpTerminal(5), "terminal.jump5", "Go to Terminal 5", ["alt+5"];
    JumpTerminal(6), "terminal.jump6", "Go to Terminal 6", ["alt+6"];
    JumpTerminal(7), "terminal.jump7", "Go to Terminal 7", ["alt+7"];
    JumpTerminal(8), "terminal.jump8", "Go to Terminal 8", ["alt+8"];
    JumpTerminal(9), "terminal.jump9", "Go to Terminal 9", ["alt+9"];
    Quit, "app.quit", "Quit", ["ctrl+q"];
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    ctrl: bool,
    alt: bool,
    shift: bool,
    key: Key,
}

impl Combo {
    /// Test-only: the key and modifiers that press this combo (`ctrl` as ⌘).
    #[cfg(test)]
    pub fn press(&self) -> (Key, Modifiers) {
        let m = Modifiers {
            alt: self.alt,
            shift: self.shift,
            mac_cmd: self.ctrl,
            command: self.ctrl,
            ..Default::default()
        };
        (self.key, m)
    }

    pub fn parse(s: &str) -> Result<Combo, String> {
        let mut combo = Combo {
            ctrl: false,
            alt: false,
            shift: false,
            key: Key::Escape,
        };
        let s = s.trim();
        // The key is whatever follows the last `+` — or `+` itself in "ctrl++".
        let (mods, last) = match s.strip_suffix("++") {
            Some(m) => (m, "+"),
            None => s.rsplit_once('+').unwrap_or(("", s)),
        };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "mod" => combo.ctrl = true,
                "alt" | "option" | "opt" => combo.alt = true,
                "shift" => combo.shift = true,
                other => return Err(format!("unknown modifier \"{other}\" in \"{s}\"")),
            }
        }
        let last = last.trim();
        // Symbols and aliases via `from_name`; names like "pageup" case-insensitively.
        let key = Key::from_name(last).or_else(|| {
            Key::ALL
                .iter()
                .copied()
                .find(|k| k.name().eq_ignore_ascii_case(last))
        });
        combo.key = key.ok_or_else(|| format!("unknown key \"{last}\" in \"{s}\""))?;
        Ok(combo)
    }

    /// The combo a key press makes, for recording a new shortcut. Uses the
    /// physical key when ⌥/⇧ change the character.
    pub fn from_press(key: Key, physical: Option<Key>, m: Modifiers) -> Combo {
        let key = if m.alt || m.shift {
            physical.unwrap_or(key)
        } else {
            key
        };
        Combo {
            ctrl: m.ctrl || m.mac_cmd,
            alt: m.alt,
            shift: m.shift,
            key,
        }
    }

    /// As written in `keybindings.json`: `"ctrl+shift+p"`.
    pub fn to_config(self) -> String {
        let mut parts: Vec<String> = [
            (self.ctrl, "ctrl"),
            (self.alt, "alt"),
            (self.shift, "shift"),
        ]
        .iter()
        .filter(|m| m.0)
        .map(|m| m.1.to_string())
        .collect();
        parts.push(self.key.name().to_ascii_lowercase());
        parts.join("+")
    }

    /// Does a key press match? `physical` rescues chords whose logical key is
    /// a different character: ⌥O types "ø" on macOS, ⇧, types "<".
    fn matches(&self, key: Key, physical: Option<Key>, m: Modifiers) -> bool {
        (key == self.key || ((m.alt || m.shift) && physical == Some(self.key)))
            && self.ctrl == (m.ctrl || m.mac_cmd)
            && self.alt == m.alt
            && self.shift == m.shift
    }

    /// The combo as keycaps: `["⇧", "⌘", "P"]` on macOS (Apple's order:
    /// ⌥ ⇧ ⌘), `["Ctrl", "Shift", "P"]` elsewhere.
    pub fn caps(&self) -> Vec<String> {
        let mac = cfg!(target_os = "macos");
        let mods: [(bool, &str, &str); 3] = if mac {
            [
                (self.alt, "⌥", ""),
                (self.shift, "⇧", ""),
                (self.ctrl, "⌘", ""),
            ]
        } else {
            [
                (self.ctrl, "", "Ctrl"),
                (self.alt, "", "Alt"),
                (self.shift, "", "Shift"),
            ]
        };
        let mut caps: Vec<String> = mods
            .iter()
            .filter(|m| m.0)
            .map(|m| if mac { m.1 } else { m.2 }.to_string())
            .collect();
        caps.push(match self.key {
            Key::Backspace if mac => "⌫".to_string(),
            Key::Delete if mac => "⌦".to_string(),
            Key::Enter => "↵".to_string(),
            Key::ArrowUp => "↑".to_string(),
            Key::ArrowDown => "↓".to_string(),
            Key::ArrowLeft => "←".to_string(),
            Key::ArrowRight => "→".to_string(),
            k => k.symbol_or_name().to_string(),
        });
        caps
    }
}

/// One entry of `keybindings.json`.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct UserBinding {
    key: String,
    command: String,
}

pub struct Keymap {
    bindings: Vec<(Combo, Command)>,
    /// Commands whose keys come from `keybindings.json`.
    custom: Vec<Command>,
}

impl Keymap {
    /// Defaults with the user's bindings applied, plus any errors in them.
    pub fn new(user: &[UserBinding]) -> (Keymap, Vec<String>) {
        let mut errors = Vec::new();
        let mut bindings = Vec::new();
        let mut custom = Vec::new();
        for info in COMMANDS {
            let overrides: Vec<_> = user.iter().filter(|b| b.command == info.id).collect();
            if !overrides.is_empty() {
                custom.push(info.command);
            }
            let keys: Vec<&str> = if overrides.is_empty() {
                info.keys.to_vec()
            } else {
                overrides
                    .iter()
                    .map(|b| b.key.as_str())
                    .filter(|k| !k.trim().is_empty())
                    .collect()
            };
            for k in keys {
                match Combo::parse(k) {
                    Ok(c) => bindings.push((c, info.command)),
                    Err(e) => errors.push(e),
                }
            }
        }
        for b in user {
            if !COMMANDS.iter().any(|c| c.id == b.command) {
                errors.push(format!("unknown command \"{}\"", b.command));
            }
        }
        (Keymap { bindings, custom }, errors)
    }

    pub fn command_for(&self, key: Key, physical: Option<Key>, m: Modifiers) -> Option<Command> {
        self.bindings
            .iter()
            .find(|(c, _)| c.matches(key, physical, m))
            .map(|(_, cmd)| *cmd)
    }

    /// Every key bound to `command`.
    pub fn combos_for(&self, command: Command) -> Vec<Combo> {
        self.bindings
            .iter()
            .filter(|(_, c)| *c == command)
            .map(|(combo, _)| *combo)
            .collect()
    }

    /// The other commands sharing a key with `command`.
    pub fn conflicts(&self, command: Command) -> Vec<Command> {
        let mine = self.combos_for(command);
        let mut out: Vec<Command> = Vec::new();
        for (combo, c) in &self.bindings {
            if *c != command && mine.contains(combo) && !out.contains(c) {
                out.push(*c);
            }
        }
        out
    }

    pub fn is_custom(&self, command: Command) -> bool {
        self.custom.contains(&command)
    }

    /// The first key bound to `command`, for showing in the UI.
    pub fn combo_for(&self, command: Command) -> Option<Combo> {
        self.bindings
            .iter()
            .find(|(_, c)| *c == command)
            .map(|(combo, _)| *combo)
    }
}

/// A fresh `keybindings.json`: instructions plus every default, commented out,
/// so remapping is "uncomment and edit".
pub fn template() -> String {
    let mut s = String::from(
        "// Oxru keyboard shortcuts. Saved changes apply instantly.\n\
         //\n\
         // Each entry replaces ALL default keys of its command; use \"key\": \"\" to unbind.\n\
         // Modifiers: ctrl (also matches ⌘ on macOS), alt, shift. Example: \"ctrl+shift+p\".\n\
         //\n\
         // Defaults — uncomment a line to change it:\n[\n",
    );
    for c in COMMANDS {
        let keys = if c.keys.is_empty() { &[""][..] } else { c.keys };
        for key in keys {
            let key = serde_json::to_string(key).unwrap_or_default(); // escapes `\\`
            s += &format!(
                "  // {{ \"key\": {key}, \"command\": \"{}\" }}, // {}\n",
                c.id, c.title
            );
        }
    }
    s + "]\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(key: &str, command: &str) -> UserBinding {
        UserBinding {
            key: key.into(),
            command: command.into(),
        }
    }

    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    const CMD: Modifiers = Modifiers {
        mac_cmd: true,
        command: true,
        ..Modifiers::NONE
    };

    #[test]
    fn defaults_never_conflict() {
        let (km, errors) = Keymap::new(&[]);
        assert!(errors.is_empty(), "{errors:?}");
        for c in COMMANDS {
            assert!(
                km.conflicts(c.command).is_empty(),
                "{} shares a key with {:?}",
                c.id,
                km.conflicts(c.command)
            );
        }
    }

    #[test]
    fn combos_round_trip_through_config_text() {
        for c in COMMANDS {
            for k in c.keys {
                let combo = Combo::parse(k).unwrap();
                assert_eq!(Combo::parse(&combo.to_config()), Ok(combo), "{k}");
            }
        }
        let m = Modifiers {
            mac_cmd: true,
            command: true,
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            Combo::from_press(Key::Num1, Some(Key::Num1), m).to_config(),
            "ctrl+alt+1"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn keycaps_in_apple_order() {
        assert_eq!(
            Combo::parse("ctrl+alt+shift+c").unwrap().caps().concat(),
            "⌥⇧⌘C"
        );
        assert_eq!(
            Combo::parse("ctrl+backspace").unwrap().caps().concat(),
            "⌘⌫"
        );
    }

    #[test]
    fn parses_combos() {
        let c = Combo::parse("ctrl+shift+p").unwrap();
        assert!(c.ctrl && c.shift && !c.alt && c.key == Key::P);
        assert_eq!(Combo::parse("ctrl+,").unwrap().key, Key::Comma);
        assert_eq!(Combo::parse("F1").unwrap().key, Key::F1);
        assert_eq!(Combo::parse("alt+escape").unwrap().key, Key::Escape);
        assert_eq!(Combo::parse("ctrl++").unwrap().key, Key::Plus);
        assert!(Combo::parse("hyper+x").is_err());
        assert!(Combo::parse("ctrl+nope").is_err());
    }

    #[test]
    fn ctrl_also_matches_cmd() {
        let (km, _) = Keymap::new(&[]);
        assert_eq!(km.command_for(Key::Q, None, CTRL), Some(Command::Quit));
        assert_eq!(km.command_for(Key::Q, None, CMD), Some(Command::Quit));
        assert_eq!(km.command_for(Key::Q, None, Modifiers::NONE), None);
    }

    #[test]
    fn option_letter_matches_by_physical_key() {
        let (km, _) = Keymap::new(&[]);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        // ⌥O on macOS: no logical letter, physical O.
        assert_eq!(
            km.command_for(Key::Escape, Some(Key::O), alt),
            Some(Command::RecentFolders)
        );
        assert_eq!(km.command_for(Key::F, None, alt), Some(Command::GoToFile));
    }

    #[test]
    fn user_binding_replaces_defaults_and_can_unbind() {
        let (km, errors) = Keymap::new(&[
            user("ctrl+shift+x", "app.quit"),
            user("", "app.openSettings"),
        ]);
        assert!(errors.is_empty());
        assert_eq!(km.command_for(Key::Q, None, CTRL), None);
        let shift = Modifiers {
            shift: true,
            ..CTRL
        };
        assert_eq!(km.command_for(Key::X, None, shift), Some(Command::Quit));
        assert_eq!(km.combo_for(Command::OpenSettings), None);
    }

    #[test]
    fn errors_are_reported() {
        let (km, errors) =
            Keymap::new(&[user("ctrl+q", "app.qiut"), user("ctrl+bogus", "app.quit")]);
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().any(|e| e.contains("app.qiut")));
        assert!(errors.iter().any(|e| e.contains("bogus")));
        // Other defaults still work.
        assert_eq!(
            km.combo_for(Command::OpenFolder),
            Combo::parse("ctrl+o").ok()
        );
    }

    #[test]
    fn template_parses_and_every_default_key_is_valid() {
        let parsed: Vec<UserBinding> = json5::from_str(&template()).unwrap();
        assert!(parsed.is_empty());
        // Uncommented, every line is a valid binding — including `ctrl+\\`.
        let live = template().replace("  // {", "  {");
        let parsed: Vec<UserBinding> = json5::from_str(&live).unwrap();
        assert!(parsed.iter().any(|b| b.key == "ctrl+\\"));
        for c in COMMANDS {
            for k in c.keys {
                assert!(Combo::parse(k).is_ok(), "{k}");
            }
        }
    }
}
