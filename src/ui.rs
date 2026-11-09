//! Drawing: title bar, status bar, welcome screen, code view and palette.
//! All chrome sizes scale with `uiFontSize` (14 = 1.0), so a bigger UI font
//! enlarges the chrome in proportion instead of overflowing it.

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke,
    StrokeKind, pos2, text::LayoutJob, vec2,
};

use crate::app::{Answer, App, Focus};
use crate::editor::{char_to_col, col_to_char};
use crate::fonts::{self, Weight};
use crate::keymap::Command;
use crate::palette::{Mode, Pick};
use crate::theme::{Colors, mix};

mod sidebar;

/// Nerd Font glyphs (VS Code codicons and friends).
pub mod icon {
    use crate::keymap::Command;

    pub const FOLDER: &str = "\u{ea83}";
    pub const FOLDER_OPEN: &str = "\u{eaf7}";
    pub const SIDEBAR: &str = "\u{ebf3}";
    pub const COLLAPSE_ALL: &str = "\u{eac5}";
    pub const FILE: &str = "\u{ea7b}";
    pub const HISTORY: &str = "\u{ea82}";
    pub const GEAR: &str = "\u{eb51}";
    pub const KEYBOARD: &str = "\u{f11c}";
    pub const QUIT: &str = "\u{ea6e}";
    pub const TERMINAL: &str = "\u{ea85}";
    pub const PULSE: &str = "\u{eb31}";
    pub const CHIP: &str = "\u{ec19}";
    pub const ERROR: &str = "\u{ea87}";
    pub const CHECK: &str = "\u{eab2}";
    pub const SEARCH: &str = "\u{ea6d}";
    pub const GO_TO_FILE: &str = "\u{ea94}";
    pub const GO_TO_LINE: &str = "\u{ea90}";
    pub const COMMENT: &str = "\u{ea6b}";
    pub const LINES: &str = "\u{eb17}";
    pub const THEME: &str = "\u{eb5c}";
    pub const FONT: &str = "\u{eb69}";
    pub const CLOSE: &str = "\u{ea76}";
    pub const PALETTE: &str = "\u{eb85}";
    pub const CHEVRON_RIGHT: &str = "\u{eab6}";
    pub const CHEVRON_DOWN: &str = "\u{eab4}";
    pub const CHEVRON_LEFT: &str = "\u{eab5}";
    pub const SAVE: &str = "\u{eb4b}";
    pub const SAVE_ALL: &str = "\u{eb49}";
    pub const CLOSE_ALL: &str = "\u{eac1}";
    pub const UNDO: &str = "\u{eae2}";
    pub const REDO: &str = "\u{ebb0}";
    pub const COPY: &str = "\u{ebcc}";
    pub const PASTE: &str = "\u{eac0}";
    pub const ARROW_LEFT: &str = "\u{ea9b}";
    pub const ARROW_RIGHT: &str = "\u{ea9c}";
    pub const DOT: &str = "\u{ea71}";
    pub const BELL: &str = "\u{eaa2}";
    pub const EDIT: &str = "\u{ea73}";
    pub const WARNING: &str = "\u{ea6c}";
    pub const ADD: &str = "\u{ea60}";
    pub const GRID: &str = "\u{ebeb}";
    pub const MAXIMIZE: &str = "\u{eb4c}";
    pub const RESTORE: &str = "\u{eb4d}";
    pub const PASS: &str = "\u{eba4}";
    pub const REPLACE: &str = "\u{eb3d}";
    pub const REPLACE_ALL: &str = "\u{eb3c}";
    pub const CLEAR: &str = "\u{eabf}";
    pub const CASE: &str = "\u{eab1}";
    pub const WHOLE_WORD: &str = "\u{eb7e}";
    pub const WRAP: &str = "\u{eb80}";
    pub const ARROW_UP: &str = "\u{eaa1}";
    pub const ARROW_DOWN: &str = "\u{ea9a}";
    pub const SPLIT: &str = "\u{eb56}";
    pub const CURSOR: &str = "\u{ec5c}";
    pub const NEW_FILE: &str = "\u{ea7f}";
    pub const NEW_FOLDER: &str = "\u{ea80}";
    pub const TRASH: &str = "\u{ea81}";
    pub const EYE: &str = "\u{ea70}";
    pub const LINK: &str = "\u{eb14}";
    pub const CIRCLE: &str = "\u{eabc}";
    pub const TASKS: &str = "\u{eb67}";
    pub const CLIPBOARD: &str = "\u{eac0}";
    pub const QUESTION: &str = "\u{eb32}";

    /// A file's type icon and its colour (Seti's, as VS Code shows them).
    /// `None`: no known type, the plain page in the caller's colour.
    pub fn for_file(name: &str) -> (&'static str, Option<eframe::egui::Color32>) {
        let rgb = |hex: u32| {
            Some(eframe::egui::Color32::from_rgb(
                (hex >> 16) as u8,
                (hex >> 8) as u8,
                hex as u8,
            ))
        };
        let (blue, green, yellow, orange, red, purple, pink, grey) = (
            0x519aba, 0x8dc149, 0xcbcb41, 0xe37933, 0xcc3e44, 0xa074c4, 0xf55385, 0x6d8086,
        );
        let lower = name.to_ascii_lowercase();
        // Whole names first: they say more than the extension.
        let by_name = match lower.as_str() {
            "dockerfile" | "containerfile" | ".dockerignore" => Some(("\u{e650}", blue)),
            "makefile" | "gnumakefile" | "justfile" | "cmakelists.txt" => {
                Some(("\u{e673}", orange))
            }
            ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => {
                Some(("\u{e702}", orange))
            }
            "package.json" | "package-lock.json" | ".npmrc" => Some(("\u{e71e}", red)),
            "yarn.lock" => Some(("\u{e6a7}", blue)),
            "pnpm-lock.yaml" => Some(("\u{e865}", yellow)),
            "tsconfig.json" | "jsconfig.json" => Some(("\u{e69d}", blue)),
            "cargo.toml" | "cargo.lock" | "rust-toolchain" | "rust-toolchain.toml" => {
                Some(("\u{e68b}", orange))
            }
            "go.mod" | "go.sum" | "go.work" => Some(("\u{e627}", blue)),
            "gemfile" | "gemfile.lock" | "rakefile" => Some(("\u{e605}", red)),
            "license" | "license.md" | "license.txt" | "licence" | "copying" => {
                Some(("\u{e60a}", yellow))
            }
            "jenkinsfile" => Some(("\u{e66e}", red)),
            "build.gradle" | "settings.gradle" | "build.gradle.kts" => Some(("\u{e660}", blue)),
            "pom.xml" => Some(("\u{e674}", red)),
            ".editorconfig" => Some(("\u{e652}", grey)),
            ".prettierrc" | ".prettierignore" => Some(("\u{e6b4}", yellow)),
            _ if lower.starts_with(".eslintrc") || lower.starts_with("eslint.config") => {
                Some(("\u{e655}", purple))
            }
            _ if lower.starts_with("webpack.config") => Some(("\u{e6a3}", blue)),
            _ if lower.starts_with("docker-compose") || lower.starts_with("compose.y") => {
                Some(("\u{e650}", blue))
            }
            _ if lower == ".env" || lower.starts_with(".env.") => Some(("\u{e615}", yellow)),
            _ if lower.starts_with("readme") => Some(("\u{e66a}", blue)),
            _ => None,
        };
        let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
        if let Some((glyph, hex)) = by_name {
            return (glyph, rgb(hex));
        }
        let (glyph, hex) = match ext {
            "rs" => ("\u{e68b}", orange),
            "py" | "pyi" | "pyw" => ("\u{e606}", blue),
            "ipynb" => ("\u{e678}", orange),
            "js" | "mjs" | "cjs" => ("\u{e60c}", yellow),
            "ts" | "mts" | "cts" => ("\u{e628}", blue),
            "jsx" | "tsx" => ("\u{e625}", blue),
            "vue" => ("\u{e6a0}", green),
            "svelte" => ("\u{e697}", red),
            "astro" => ("\u{e6b3}", orange),
            "html" | "htm" | "xhtml" => ("\u{e60e}", orange),
            "css" => ("\u{e614}", blue),
            "scss" | "sass" => ("\u{e603}", pink),
            "less" => ("\u{e60b}", blue),
            "json" | "jsonc" | "json5" => ("\u{eb0f}", yellow),
            "toml" => ("\u{e6b2}", grey),
            "yml" | "yaml" => ("\u{e6a8}", purple),
            "xml" | "plist" | "xsd" => ("\u{e619}", orange),
            "md" | "markdown" | "mdx" => ("\u{e609}", blue),
            "txt" | "text" | "log" => ("\u{e64e}", grey),
            "csv" | "tsv" => ("\u{e64a}", green),
            "go" => ("\u{e627}", blue),
            "c" => ("\u{e649}", blue),
            "h" => ("\u{e649}", purple),
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" => ("\u{e646}", blue),
            "cs" => ("\u{e648}", blue),
            "java" | "class" | "jar" => ("\u{e66d}", red),
            "kt" | "kts" => ("\u{e634}", orange),
            "scala" | "sc" => ("\u{e68e}", red),
            "swift" => ("\u{e699}", orange),
            "dart" => ("\u{e64c}", blue),
            "rb" | "erb" => ("\u{e605}", red),
            "php" => ("\u{e608}", purple),
            "lua" => ("\u{e620}", blue),
            "zig" => ("\u{e6a9}", orange),
            "nim" => ("\u{e677}", yellow),
            "hs" | "lhs" => ("\u{e61f}", purple),
            "ex" | "exs" => ("\u{e62d}", purple),
            "erl" | "hrl" => ("\u{e62d}", red),
            "clj" | "cljs" | "cljc" | "edn" => ("\u{e642}", green),
            "ml" | "mli" => ("\u{e67a}", orange),
            "elm" => ("\u{e62c}", blue),
            "jl" => ("\u{e624}", purple),
            "r" => ("\u{e68a}", blue),
            "pl" | "pm" => ("\u{e67e}", blue),
            "sh" | "bash" | "zsh" | "fish" | "ksh" => ("\u{e691}", green),
            "ps1" | "psm1" => ("\u{e683}", blue),
            "bat" | "cmd" => ("\u{e629}", green),
            "sql" | "db" | "sqlite" | "sqlite3" => ("\u{e64d}", pink),
            "graphql" | "gql" => ("\u{e662}", pink),
            "proto" => ("\u{e6a4}", grey),
            "tf" | "tfvars" | "hcl" => ("\u{e69a}", purple),
            "nix" => ("\u{e843}", blue),
            "vim" => ("\u{e62b}", green),
            "tex" | "bib" | "sty" => ("\u{e69b}", blue),
            "wasm" | "wat" => ("\u{e6a1}", purple),
            "lock" => ("\u{e672}", grey),
            "env" | "ini" | "cfg" | "conf" | "properties" => ("\u{e615}", grey),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "icns" | "tiff" | "avif" => {
                ("\u{e60d}", purple)
            }
            "svg" => ("\u{e698}", yellow),
            "pdf" => ("\u{e67d}", red),
            "ttf" | "otf" | "woff" | "woff2" => ("\u{e659}", red),
            "mp3" | "wav" | "flac" | "ogg" | "m4a" => ("\u{e638}", red),
            "mp4" | "mov" | "mkv" | "webm" | "avi" => ("\u{e69f}", pink),
            "zip" | "gz" | "tgz" | "tar" | "xz" | "bz2" | "7z" | "rar" | "zst" => {
                ("\u{e6aa}", grey)
            }
            "doc" | "docx" => ("\u{e6a5}", blue),
            "xls" | "xlsx" => ("\u{e6a6}", green),
            _ => return (FILE, None),
        };
        (glyph, rgb(hex))
    }

    /// The shortcuts sheet lists every command that has a key, once.
    #[test]
    fn shortcuts_sheet_lists_every_bound_command_once() {
        use crate::keymap::{COMMANDS, Command, Keymap};
        let (km, _) = Keymap::new(&[]);
        let listed: Vec<Command> = crate::ui::help_sections(&km)
            .into_iter()
            .flat_map(|(_, rows)| rows.into_iter().map(|(c, _)| c))
            .collect();
        for c in COMMANDS {
            let numbered = matches!(c.command, Command::JumpTerminal(n) | Command::GoToTab(n) if n > 1);
            if km.combo_for(c.command).is_some() && !numbered {
                let n = listed.iter().filter(|l| **l == c.command).count();
                assert_eq!(n, 1, "{} is listed {n} times", c.id);
            }
        }
    }

    #[test]
    fn file_icons_by_name_then_extension() {
        for name in [
            "Dockerfile",
            "Makefile",
            ".gitignore",
            "Cargo.toml",
            "README.md",
            ".env.local",
        ] {
            assert_ne!(for_file(name).0, FILE, "{name}");
        }
        assert_eq!(for_file("Cargo.toml").0, for_file("main.rs").0);
        assert_eq!(for_file("App.TSX").0, for_file("view.jsx").0);
        assert_eq!(for_file("unknown.xyz"), (FILE, None));
        assert!(for_file("LICENSE").1.is_some());
    }

    pub fn for_command(c: Command) -> &'static str {
        match c {
            Command::Palette => PALETTE,
            Command::GoToFile => GO_TO_FILE,
            Command::OpenFolder => FOLDER_OPEN,
            Command::RecentFolders => HISTORY,
            Command::CloseFolder => CLOSE,
            Command::SelectTheme => THEME,
            Command::SelectUiFont | Command::SelectCodeFont => FONT,
            Command::Settings
            | Command::OpenSettings
            | Command::OpenProjectSettings
            | Command::OpenConfigFolder => GEAR,
            Command::Keybindings | Command::OpenKeybindings => KEYBOARD,
            Command::Help => QUESTION,
            Command::Todos => TASKS,
            Command::ClipboardHistory => CLIPBOARD,
            Command::Save => SAVE,
            Command::SaveAll => SAVE_ALL,
            Command::CloseTab => CLOSE,
            Command::CloseAllTabs
            | Command::CloseOtherTabs
            | Command::CloseTabsToRight
            | Command::CloseTabsToLeft
            | Command::CloseSavedTabs => CLOSE_ALL,
            Command::ReopenTab => HISTORY,
            Command::NewFile => NEW_FILE,
            Command::NewFolder => NEW_FOLDER,
            Command::RenameFile => EDIT,
            Command::DeleteFile => TRASH,
            Command::RevealInFinder => FOLDER_OPEN,
            Command::CopyPath | Command::CopyRelativePath => COPY,
            Command::ToggleBuildFolders => EYE,
            Command::NextTab | Command::MoveTabRight => ARROW_RIGHT,
            Command::PrevTab | Command::MoveTabLeft => ARROW_LEFT,
            Command::Undo => UNDO,
            Command::Redo => REDO,
            Command::Copy | Command::Cut => COPY,
            Command::Paste => PASTE,
            Command::SelectAll => EDIT,
            Command::Find | Command::SearchInFiles => SEARCH,
            Command::Replace => REPLACE,
            Command::AddNextOccurrence | Command::AddCursorAbove | Command::AddCursorBelow => {
                CURSOR
            }
            Command::ToggleSplit => SPLIT,
            Command::ToggleWordWrap => WRAP,
            Command::ToggleSidebar => SIDEBAR,
            Command::GoToLine => GO_TO_LINE,
            Command::ToggleComment => COMMENT,
            Command::FormatDocument => EDIT,
            Command::CollapseFolders => COLLAPSE_ALL,
            Command::OpenTerminalLink => LINK,
            Command::ZoomIn | Command::ZoomOut | Command::ResetZoom => FONT,
            Command::FullScreen | Command::MaximizeWindow => MAXIMIZE,
            Command::MoveLineUp
            | Command::MoveLineDown
            | Command::DuplicateLineUp
            | Command::DuplicateLineDown
            | Command::DeleteLine
            | Command::InsertLineBelow
            | Command::InsertLineAbove
            | Command::IndentLines
            | Command::OutdentLines => LINES,
            Command::GoToTab(_) => FILE,
            Command::SwitchFocus => TERMINAL,
            Command::ClearTerminal => CLEAR,
            Command::FocusSidebar => FILE,
            Command::ToggleTerminal | Command::SwitchTerminal | Command::JumpTerminal(_) => {
                TERMINAL
            }
            Command::NewTerminal => ADD,
            Command::CloseTerminal => CLOSE,
            Command::ToggleTerminalGrid => GRID,
            Command::MaximizeTerminal => MAXIMIZE,
            Command::Quit => QUIT,
        }
    }
}

/// Menu entries in separated groups: the command, its label, and whether
/// it can run now.
type MenuGroups = Vec<Vec<(Command, &'static str, bool)>>;

/// A right-click menu of commands in separated groups, each with its
/// shortcut; returns the one picked.
fn command_menu(
    ui: &mut egui::Ui,
    groups: &[Vec<(Command, &'static str, bool)>],
    key: &dyn Fn(Command) -> Option<String>,
) -> Option<Command> {
    let mut pick = None;
    for (g, group) in groups.iter().enumerate() {
        if g > 0 {
            ui.separator();
        }
        for &(c, title, enabled) in group {
            let button = egui::Button::new(title).shortcut_text(key(c).unwrap_or_default());
            if ui.add_enabled(enabled, button).clicked() {
                pick = Some(c);
                ui.close();
            }
        }
    }
    pick
}

/// A dialog button: its label, the key hint shown on it, and the keys that press it.
struct Btn {
    label: &'static str,
    hint: &'static str,
    keys: &'static [Key],
    primary: bool,
}

/// Rows shown at once in a compact palette (pickers, Go to Line).
const PALETTE_ROWS: usize = 10;

/// A key hint: the keycaps, then what they do ("" for bare caps).
pub(crate) type Hint = (Vec<String>, &'static str);

/// Colours + scale: everything the shared widgets need. Holding a copy (not a
/// borrow of `App`) lets them draw while app state is mutably borrowed.
struct Paint {
    c: Colors,
    s: f32,
}

impl Paint {
    fn font(&self, size: f32, weight: Weight) -> FontId {
        fonts::ui(size * self.s, weight)
    }

    /// A label with the `matched` chars picked out in the accent.
    fn label(&self, label: &str, matched: &[usize], color: Color32) -> LayoutJob {
        let font = self.font(14.0, Weight::Medium);
        let mut job = LayoutJob::default();
        for (i, ch) in label.chars().enumerate() {
            let fmt = egui::TextFormat::simple(
                font.clone(),
                if matched.contains(&i) {
                    self.c.accent
                } else {
                    color
                },
            );
            job.append(ch.encode_utf8(&mut [0; 4]), 0.0, fmt);
        }
        job
    }

    /// One list row, shared by the welcome card and the palette: highlight
    /// (eased by `t`), icon, label, quieter detail, keycaps on the right.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        ui: &egui::Ui,
        p: &egui::Painter,
        row: Rect,
        t: f32,
        glyph: (&str, Option<Color32>),
        label: LayoutJob,
        detail: &str,
        caps: &[String],
        hints: &[Hint],
        indent: f32,
    ) {
        let (c, s) = (&self.c, self.s);
        p.rect_filled(row, 8.0 * s, c.hover.gamma_multiply(t));
        let x = row.left() + 14.0 * s + indent;
        let (glyph, tint) = glyph;
        let tint = tint.unwrap_or_else(|| mix(c.text_subtle, c.accent, t));
        p.text(
            pos2(x, row.center().y),
            Align2::LEFT_CENTER,
            glyph,
            self.font(16.0, Weight::Regular),
            tint,
        );
        let label = ui.fonts_mut(|f| f.layout_job(label));
        let lx = x + 30.0 * s;
        let label_w = label.size().x;
        p.galley(
            pos2(lx, row.center().y - label.size().y / 2.0),
            label,
            c.text,
        );
        let caps_w = if hints.is_empty() {
            self.keycaps(ui, p, caps, pos2(row.right() - 12.0 * s, row.center().y))
        } else {
            let w = self.key_hints(ui, None, hints, 12.0, Pos2::ZERO);
            let at = pos2(row.right() - 12.0 * s - w, row.center().y);
            self.key_hints(ui, Some(p), hints, 12.0, at)
        };
        if !detail.is_empty() {
            // One line, ending in "…" if it would run into the keycaps.
            let dx = lx + label_w + 10.0 * s;
            let mut job = LayoutJob::simple_singleline(
                detail.to_string(),
                self.font(12.5, Weight::Regular),
                c.text_subtle,
            );
            job.wrap = egui::text::TextWrapping {
                max_width: (row.right() - 12.0 * s - caps_w - dx).max(0.0),
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            let d = ui.fonts_mut(|f| f.layout_job(job));
            p.galley(
                pos2(dx, row.center().y - d.size().y / 2.0),
                d,
                c.text_subtle,
            );
        }
    }

    /// A small square icon button with a hover tint and tooltip.
    fn icon_button(&self, ui: &mut egui::Ui, glyph: &str, tip: &str, on: bool) -> bool {
        let size = 24.0 * self.s;
        let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
        if resp.hovered() || on {
            ui.painter().rect_filled(r, 5.0 * self.s, self.c.hover);
        }
        let color = if on {
            self.c.accent
        } else if resp.hovered() {
            self.c.text
        } else {
            self.c.text_muted
        };
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            glyph,
            self.font(14.0, Weight::Regular),
            color,
        );
        resp.on_hover_text(tip).clicked()
    }

    /// Keycaps drawn right-to-left, ending at `right_center`. Returns their width.
    fn keycaps(
        &self,
        ui: &egui::Ui,
        p: &egui::Painter,
        caps: &[String],
        right_center: Pos2,
    ) -> f32 {
        let (c, s) = (&self.c, self.s);
        let h = 22.0 * s;
        let mut x = right_center.x;
        for cap in caps.iter().rev() {
            let g = ui.fonts_mut(|f| {
                f.layout_no_wrap(cap.clone(), fonts::keycap(11.5 * s), c.text_muted)
            });
            let w = (g.size().x + 12.0 * s).max(h);
            let r = Rect::from_min_size(pos2(x - w, right_center.y - h / 2.0), vec2(w, h));
            p.rect(
                r,
                6.0 * s,
                c.background,
                Stroke::new(1.0, c.border),
                StrokeKind::Inside,
            );
            p.galley(r.center() - g.size() / 2.0, g, c.text_muted);
            x -= w + 4.0 * s;
        }
        right_center.x - x
    }

    /// As many of `hints` as fit in `room`, right-aligned, keeping their
    /// order: put the most useful first, so a narrow window drops the rest.
    fn hints_in(&self, ui: &egui::Ui, p: &egui::Painter, room: Rect, hints: &[Hint], size: f32) {
        let width = |n: usize| self.key_hints(ui, None, &hints[..n], size, Pos2::ZERO);
        let n = (0..=hints.len()).rev().find(|&n| width(n) <= room.width()).unwrap_or(0);
        if n > 0 {
            let w = width(n);
            self.key_hints(ui, Some(p), &hints[..n], size, pos2(room.right() - w, room.center().y));
        }
    }

    /// Key hints left to right from `left_center`: each one's caps, then
    /// what they do, in `size` text. Without a painter it only measures.
    /// Returns the width.
    fn key_hints(
        &self,
        ui: &egui::Ui,
        p: Option<&egui::Painter>,
        hints: &[Hint],
        size: f32,
        left_center: Pos2,
    ) -> f32 {
        let (c, s) = (&self.c, self.s);
        let h = (size + 9.0) * s;
        let mut x = left_center.x;
        for (n, (caps, what)) in hints.iter().enumerate() {
            if n > 0 {
                x += 14.0 * s;
            }
            for cap in caps {
                let g = ui.fonts_mut(|f| {
                    f.layout_no_wrap(cap.clone(), fonts::keycap((size - 0.5) * s), c.text_muted)
                });
                let w = (g.size().x + 10.0 * s).max(h);
                let r = Rect::from_min_size(pos2(x, left_center.y - h / 2.0), vec2(w, h));
                if let Some(p) = p {
                    p.rect(
                        r,
                        5.0 * s,
                        c.background,
                        Stroke::new(1.0, c.border),
                        StrokeKind::Inside,
                    );
                    p.galley(r.center() - g.size() / 2.0, g, c.text_muted);
                }
                x += w + 3.0 * s;
            }
            if !what.is_empty() {
                let g = ui.fonts_mut(|f| {
                    f.layout_no_wrap(
                        what.to_string(),
                        self.font(size, Weight::Regular),
                        c.text_muted,
                    )
                });
                x += 3.0 * s;
                if let Some(p) = p {
                    p.galley(
                        pos2(x, left_center.y - g.size().y / 2.0),
                        g.clone(),
                        c.text_muted,
                    );
                }
                x += g.size().x;
            } else {
                x -= 3.0 * s;
            }
        }
        x - left_center.x
    }
}

impl App {
    fn paint(&self) -> Paint {
        Paint {
            c: self.config.theme.colors.clone(),
            s: self.config.settings.ui_font_size / 14.0,
        }
    }

    /// The whole window, one frame.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // The app owns keyboard focus. A clicked button keeping egui focus
        // would "click" again on every ↵ or space typed into the terminal.
        if !ctx.text_edit_focused() {
            ctx.memory_mut(|m| m.focused().map(|id| m.surrender_focus(id)));
        }
        self.tick(&ctx);
        self.handle_keys(&ctx);
        self.terminal_rects.clear();

        let bare = egui::Frame::NONE;
        let bg = self.config.theme.colors.background;
        let toggle = egui::Panel::top("titlebar")
            .frame(bare)
            .show_separator_line(false)
            .show(ui, |ui| self.titlebar(ui))
            .inner;
        if toggle {
            self.toggle_sidebar(&ctx);
        }
        let terminals_up = self.terminal_open && !self.terminals.is_empty();
        let maximized = terminals_up && self.terminal_max;
        let hint = egui::Panel::bottom("status")
            .frame(bare)
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui))
            .inner;
        // The tree runs the full height; tabs and terminals sit beside it.
        if self.sidebar_shown() && !maximized {
            let s = self.paint().s;
            egui::Panel::left("sidebar")
                .frame(bare.fill(self.config.theme.colors.surface))
                .resizable(true)
                .show_separator_line(false)
                .default_size(260.0 * s)
                .size_range(170.0 * s..=(ui.available_width() * 0.5).max(170.0 * s))
                .show(ui, |ui| self.sidebar_ui(ui));
        }
        if !self.tabs.is_empty() && !maximized {
            egui::Panel::top("tabs")
                .frame(bare)
                .show_separator_line(false)
                .show(ui, |ui| self.tab_strip(ui));
        }
        if terminals_up && !maximized {
            egui::Panel::bottom("terminal")
                .frame(bare.fill(bg))
                .resizable(true)
                .show_separator_line(false)
                .default_size(320.0)
                .size_range(120.0..=ui.available_height() - 80.0)
                .show(ui, |ui| self.terminal_panel(ui));
        }
        let chosen = egui::CentralPanel::default()
            .frame(bare.fill(bg))
            .show(ui, |ui| {
                if maximized {
                    self.terminal_panel(ui);
                    None
                } else if self.active.is_some() {
                    let area = self.editors(ui);
                    if self.find.is_some() {
                        self.find_bar(ui.ctx(), area);
                    }
                    None
                } else {
                    self.welcome(ui)
                }
            })
            .inner;
        if let Some(command) = chosen.or(hint) {
            self.run(command, &ctx);
        }
        // A dialog opened from the palette shows on its own, then the palette returns.
        if self.palette.is_some() && !self.modal_open() {
            self.palette_ui(&ctx);
        }
        if self.prompt.is_some() {
            self.prompt_ui(&ctx);
        } else if self.conflict.is_some() {
            self.conflict_ui(&ctx);
        } else if self.ask.is_some() {
            self.ask_ui(&ctx);
        } else if self.help {
            self.help_ui(&ctx);
        }
        self.toasts_ui(&ctx);
        self.drop_overlay(&ctx);
        self.handle_drops(&ctx);

        // Idle costs next to nothing: egui repaints on input, plus one wake-up
        // a second for config reloads and the RAM/FPS readout.
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }

    /// While files are dragged in from Finder: outline where they'll land
    /// and say what happens there.
    fn drop_overlay(&self, ctx: &egui::Context) {
        let n = ctx.input(|i| i.raw.hovered_files.len());
        if n == 0 {
            return;
        }
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let target = self.drop_terminal(ctx);
        let (area, label) =
            match target.and_then(|t| self.terminal_rects.iter().find(|(i, _)| *i == t)) {
                Some((_, r)) => (
                    *r,
                    if n == 1 {
                        "Drop to type its path here".to_string()
                    } else {
                        format!("Drop to type {n} paths here")
                    },
                ),
                None => (
                    ctx.content_rect(),
                    if n == 1 {
                        "Drop to open".to_string()
                    } else {
                        format!("Drop to open {n} files")
                    },
                ),
            };
        let p = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("drop-overlay"),
        ));
        let area = area.shrink(6.0 * s);
        p.rect(
            area,
            10.0 * s,
            c.accent.gamma_multiply(0.08),
            Stroke::new(2.0, c.accent.gamma_multiply(0.8)),
            StrokeKind::Inside,
        );
        let g = p.layout_no_wrap(label, pt.font(14.0, Weight::Medium), c.accent_text);
        let pill = Rect::from_center_size(area.center(), g.size() + vec2(28.0 * s, 16.0 * s));
        p.rect_filled(pill, pill.height() / 2.0, c.accent);
        p.galley(pill.center() - g.size() / 2.0, g, c.accent_text);
        // No mouse-move events arrive during a drag, so poll to follow the pointer.
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    /// Open files as tabs: click to switch, × (or middle-click) to close. A
    /// dot marks unsaved changes; the active tab joins the editor below it.
    fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let height = 36.0 * s;
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let p = ui.painter().clone();
        p.rect_filled(bar, 0.0, c.surface);
        p.hline(
            bar.x_range(),
            bar.bottom() - 0.5,
            Stroke::new(1.0, c.border),
        );

        let names: Vec<String> = self.tabs.iter().map(|t| t.buf.name()).collect();
        let font = pt.font(13.0, Weight::Medium);
        // More tabs than fit: slide the strip so the active one is in view.
        let tab_w = |i: usize| {
            ui.fonts_mut(|f| f.layout_no_wrap(names[i].clone(), font.clone(), c.text))
                .size()
                .x
                + 68.0 * s
        };
        // In the editor, the tab keys get the strip's right end: room for at
        // least the first few is kept, however many tabs are open.
        let hints = if self.focus == Focus::Editor && self.palette.is_none() {
            self.tab_hints(false)
        } else {
            Vec::new()
        };
        let reserve = if hints.is_empty() {
            0.0
        } else {
            let first = &hints[..hints.len().min(3)];
            (pt.key_hints(ui, None, first, 11.5, Pos2::ZERO) + 32.0 * s).min(bar.width() * 0.45)
        };
        let limit = bar.right() - reserve;
        let marker_w = 56.0 * s; // "‹ N" / "N ›"
        let mut start = 0;
        if let Some(a) = self.active {
            let room = limit - bar.left() - 2.0 * marker_w;
            while start < a && (start..=a).map(&tab_w).sum::<f32>() > room {
                start += 1;
            }
        }
        let mut x = bar.left() + 8.0 * s;
        if start > 0 {
            // "‹ 3": tabs slid off to the left; click for the one before.
            let g = ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    format!("{} {start}", icon::CHEVRON_LEFT),
                    pt.font(12.0, Weight::Medium),
                    c.text_subtle,
                )
            });
            let r = Rect::from_min_size(
                pos2(x, bar.top()),
                vec2(g.size().x + 12.0 * s, height - 1.0),
            );
            let resp = ui.interact(r, egui::Id::unique("tabs-hidden-left"), Sense::click());
            if resp.hovered() {
                p.rect_filled(r, 0.0, c.hover);
            }
            p.galley(
                pos2(r.left() + 6.0 * s, r.center().y - g.size().y / 2.0),
                g,
                c.text_subtle,
            );
            let keys = self
                .config
                .keymap
                .combo_for(Command::PrevTab)
                .map(|k| k.caps().concat())
                .unwrap_or_default();
            if resp
                .on_hover_text(format!("{start} more tabs to the left · {keys} goes back"))
                .clicked()
            {
                self.select_tab(start - 1);
                return;
            }
            x = r.right();
        }
        let (mut select, mut close, mut menu_pick) = (None, None, None);
        let mut hidden_right = 0;
        let key = |c: Command| self.shortcut(c);
        for (i, tab) in self.tabs.iter().enumerate().skip(start) {
            // Same-named files get their folder alongside, to tell them apart.
            let dup = names.iter().filter(|n| **n == names[i]).count() > 1;
            let parent = tab
                .path()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            let detail = if dup {
                parent.unwrap_or_default()
            } else {
                String::new()
            };
            let active = self.active == Some(i);
            let mut job = LayoutJob::default();
            let mut fmt =
                egui::TextFormat::simple(font.clone(), if active { c.text } else { c.text_muted });
            if tab.gone {
                // Deleted on disk: struck through until saved back or closed.
                fmt.color = c.error.gamma_multiply(0.85);
                fmt.strikethrough = Stroke::new(1.5 * s, c.error.gamma_multiply(0.85));
            }
            job.append(&names[i], 0.0, fmt);
            if !detail.is_empty() {
                job.append(
                    &format!("  {detail}"),
                    0.0,
                    egui::TextFormat::simple(pt.font(12.0, Weight::Regular), c.text_subtle),
                );
            }
            let g = ui.fonts_mut(|f| f.layout_job(job));
            let (glyph, tint) = icon::for_file(&names[i]);
            let w = 14.0 * s + 20.0 * s + g.size().x + 34.0 * s;
            let rect = Rect::from_min_size(pos2(x, bar.top()), vec2(w, height - 1.0));
            let last = i + 1 == self.tabs.len();
            if !active && rect.right() > limit - if last { 0.0 } else { marker_w } {
                hidden_right = self.tabs.len() - i; // ⌃Tab and "N ›" reach them
                break;
            }
            let resp = ui.interact(rect, ui.scope_id().with(("tab", i)), Sense::click());
            let close_rect = Rect::from_center_size(
                pos2(rect.right() - 16.0 * s, rect.center().y),
                vec2(18.0 * s, 18.0 * s),
            );
            let over_close = resp.hovered()
                && ui.input(|inp| {
                    inp.pointer
                        .hover_pos()
                        .is_some_and(|ptr| close_rect.contains(ptr))
                });
            if resp.clicked() {
                if over_close {
                    close = Some(i)
                } else {
                    select = Some(i)
                }
            }
            if resp.middle_clicked() {
                close = Some(i);
            }
            if resp.secondary_clicked() {
                select = Some(i);
            }
            let jump = if i < 8 {
                key(Command::GoToTab(i as u8 + 1))
            } else {
                None
            };
            let tip = if over_close {
                format!(
                    "Close{}",
                    key(Command::CloseTab)
                        .map(|k| format!("  {k}"))
                        .unwrap_or_default()
                )
            } else {
                let path = if tab.path().is_some() {
                    self.tab_rel(tab)
                } else {
                    names[i].clone()
                };
                let gone = if tab.gone {
                    format!(
                        "\ndeleted on disk — {} saves it back",
                        key(Command::Save).unwrap_or_default()
                    )
                } else {
                    String::new()
                };
                match jump {
                    Some(k) => format!("{path}{gone}\n{k} to switch here · right-click for more"),
                    None => format!("{path}{gone}\nright-click for more"),
                }
            };
            let resp = resp.on_hover_text(tip);
            resp.context_menu(|ui| {
                menu_pick = menu_pick.or(command_menu(ui, &self.tab_menu(i), &key));
            });
            if active {
                p.rect_filled(rect, 0.0, c.background);
                p.hline(rect.x_range(), rect.top() + 1.0, Stroke::new(2.0, c.accent));
            } else if resp.hovered() {
                p.rect_filled(rect, 0.0, c.hover);
            }
            p.vline(
                rect.right(),
                rect.y_range().shrink(9.0 * s),
                Stroke::new(1.0, c.border),
            );
            let tint = tint.unwrap_or(c.text_subtle);
            p.text(
                pos2(rect.left() + 21.0 * s, rect.center().y),
                Align2::CENTER_CENTER,
                glyph,
                pt.font(14.0, Weight::Regular),
                if active {
                    tint
                } else {
                    tint.gamma_multiply(0.75)
                },
            );
            p.galley(
                pos2(rect.left() + 34.0 * s, rect.center().y - g.size().y / 2.0),
                g,
                c.text,
            );
            // Unsaved: a dot, which turns into × on hover (like VS Code).
            let marker = if tab.buf.modified && !resp.hovered() {
                Some(icon::DOT)
            } else if resp.hovered() || active {
                Some(icon::CLOSE)
            } else {
                None
            };
            if let Some(m) = marker {
                if over_close {
                    p.rect_filled(close_rect, 4.0 * s, c.hover);
                }
                let size = if m == icon::DOT { 9.0 } else { 13.0 };
                let color = if m == icon::DOT {
                    c.text_muted
                } else if over_close {
                    c.text
                } else {
                    c.text_subtle
                };
                p.text(
                    close_rect.center(),
                    Align2::CENTER_CENTER,
                    m,
                    pt.font(size, Weight::Regular),
                    color,
                );
            }
            x += w;
        }
        if hidden_right > 0 {
            // "3 ›": tabs past the right end; click for the next one.
            let g = ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    format!("{hidden_right} {}", icon::CHEVRON_RIGHT),
                    pt.font(12.0, Weight::Medium),
                    c.text_subtle,
                )
            });
            let r = Rect::from_min_size(pos2(x, bar.top()), vec2(g.size().x + 12.0 * s, height - 1.0));
            let resp = ui.interact(r, egui::Id::unique("tabs-hidden-right"), Sense::click());
            if resp.hovered() {
                p.rect_filled(r, 0.0, c.hover);
            }
            p.galley(pos2(r.left() + 6.0 * s, r.center().y - g.size().y / 2.0), g, c.text_subtle);
            let keys = key(Command::NextTab).unwrap_or_default();
            if resp
                .on_hover_text(format!("{hidden_right} more tabs to the right · {keys} goes on"))
                .clicked()
            {
                select = Some(self.tabs.len() - hidden_right);
            }
            x = r.right();
        }
        // The tab keys, in the room kept for them (and any left over).
        if !hints.is_empty() {
            let room = Rect::from_min_max(
                pos2(x + 20.0 * s, bar.top()),
                pos2(bar.right() - 12.0 * s, bar.bottom()),
            );
            pt.hints_in(ui, &p, room, &hints, 11.5);
        }
        if let Some(i) = select {
            self.select_tab(i);
        }
        if let Some(c) = menu_pick {
            self.focus = Focus::Editor; // the file actions take the active tab
            let ctx = ui.ctx().clone();
            self.run(c, &ctx);
        }
        if let Some(i) = close
            && let Some(path) = self.tabs.get(i).and_then(|t| t.path().cloned())
        {
            let ctx = ui.ctx().clone();
            self.begin_close(vec![path], crate::app::After::Nothing, &ctx);
        }
    }

    /// "Save changes?" for a tab being closed with unsaved work.
    fn prompt_ui(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &self.prompt else { return };
        let Some(path) = prompt.queue.first() else {
            return;
        };
        let left = prompt
            .queue
            .iter()
            .filter(|q| {
                self.tabs
                    .iter()
                    .any(|t| t.path() == Some(*q) && t.buf.modified)
            })
            .count();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let picked = self.modal(
            ctx,
            icon::WARNING,
            &format!("Save changes to {name}?"),
            "Your changes will be lost if you don't save them.",
            &[
                Btn {
                    label: "Don't Save",
                    hint: "D",
                    keys: &[Key::D, Key::N],
                    primary: false,
                },
                Btn {
                    label: "Cancel",
                    hint: "esc",
                    keys: &[Key::Escape],
                    primary: false,
                },
                Btn {
                    label: "Save",
                    hint: "↵",
                    keys: &[Key::Enter, Key::S],
                    primary: true,
                },
            ],
            None,
            // Several unsaved: settle them all at once.
            (left > 1).then_some((
                &format!("All {left} files:") as &str,
                &[
                    Btn {
                        label: "Discard All",
                        hint: "X",
                        keys: &[Key::X],
                        primary: false,
                    },
                    Btn {
                        label: "Save All",
                        hint: "A",
                        keys: &[Key::A],
                        primary: false,
                    },
                ][..],
            )),
        );
        let answer = picked.map(|i| {
            [
                Answer::Discard,
                Answer::Cancel,
                Answer::Save,
                Answer::DiscardAll,
                Answer::SaveAll,
            ][i]
        });
        if let Some(a) = answer {
            self.answer_prompt(a, ctx);
        }
    }

    /// This tab's file changed on disk (and it has unsaved edits): reload
    /// it, or keep ours. Or it was deleted: close the tab, or keep it.
    fn conflict_ui(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.conflict else { return };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let deleted = self.tabs.iter().any(|t| {
            t.path() == Some(path) && t.disk_news == Some(crate::buffer::DiskStatus::Deleted)
        });
        let save = self
            .config
            .keymap
            .combo_for(Command::Save)
            .map(|k| k.caps().concat())
            .unwrap_or_default();
        let picked = if deleted {
            self.modal(
                ctx,
                icon::TRASH,
                &format!("{name} was deleted"),
                &format!("Something outside Roxaruu deleted it. Keep the tab to go on reading or editing it ({save} saves it back), or close it."),
                &[
                    Btn { label: "Close Tab", hint: "C", keys: &[Key::C, Key::W], primary: false },
                    Btn { label: "Keep Open", hint: "↵", keys: &[Key::Enter, Key::K, Key::Escape], primary: true },
                ],
                None,
                None,
            )
        } else {
            self.modal(
                ctx,
                icon::WARNING,
                &format!("{name} changed on disk"),
                "You have unsaved changes here. Reload the version on disk (your changes are discarded), or keep yours?",
                &[
                    Btn { label: "Reload from Disk", hint: "R", keys: &[Key::R], primary: false },
                    Btn { label: "Keep Mine", hint: "↵", keys: &[Key::Enter, Key::K, Key::Escape], primary: true },
                ],
                None,
                None,
            )
        };
        if let Some(i) = picked {
            self.answer_conflict(i == 0);
        }
    }

    /// New file / folder, rename, move to Trash, open a link.
    fn ask_ui(&mut self, ctx: &egui::Context) {
        use crate::app::Ask;
        let Some(mut ask) = self.ask.take() else {
            return;
        };
        let rel = |dir: &std::path::Path| match &self.project {
            Some(p) => match dir.strip_prefix(&p.root) {
                Ok(r) if r.as_os_str().is_empty() => format!("{}/", p.name),
                Ok(r) => format!("{}/{}/", p.name, r.display()),
                Err(_) => tilde(&dir.to_string_lossy()),
            },
            None => tilde(&dir.to_string_lossy()),
        };
        let name_of = |p: &std::path::Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        const CANCEL: Btn = Btn {
            label: "Cancel",
            hint: "esc",
            keys: &[Key::Escape],
            primary: false,
        };
        let ok = |label| Btn {
            label,
            hint: "↵",
            keys: &[Key::Enter],
            primary: true,
        };
        let file = matches!(ask, Ask::NewFile { .. });
        let picked = match &mut ask {
            Ask::NewFile { dir, name } | Ask::NewFolder { dir, name } => {
                let (glyph, title) = if file {
                    (icon::NEW_FILE, "New File")
                } else {
                    (icon::NEW_FOLDER, "New Folder")
                };
                let body = format!("In {} — use / to make folders on the way", rel(dir));
                self.modal(
                    ctx,
                    glyph,
                    title,
                    &body,
                    &[CANCEL, ok("Create")],
                    Some((name, 0)),
                    None,
                )
            }
            Ask::Rename { path, name } => {
                let title = format!("Rename {}", name_of(path));
                let body = format!("In {}", rel(path.parent().unwrap_or(path)));
                // Select the name without its extension, like Finder.
                let stem = match name.rfind('.') {
                    Some(dot) if dot > 0 => name[..dot].chars().count(),
                    _ => name.chars().count(),
                };
                self.modal(
                    ctx,
                    icon::EDIT,
                    &title,
                    &body,
                    &[CANCEL, ok("Rename")],
                    Some((name, stem)),
                    None,
                )
            }
            Ask::Delete { path } => {
                let what = if path.is_dir() {
                    "This folder and everything in it go"
                } else {
                    "It goes"
                };
                let title = format!("Move {} to the Trash?", name_of(path));
                let body = format!("{what} to the Trash, so you can put it back from there.");
                self.modal(
                    ctx,
                    icon::TRASH,
                    &title,
                    &body,
                    &[CANCEL, ok("Move to Trash")],
                    None,
                    None,
                )
            }
            Ask::OpenUrl(url) => {
                let body = url.clone();
                self.modal(
                    ctx,
                    icon::LINK,
                    "Open this link in your browser?",
                    &body,
                    &[CANCEL, ok("Open")],
                    None,
                    None,
                )
            }
        };
        self.ask = Some(ask);
        if let Some(i) = picked {
            self.answer_ask(i == 1, ctx);
        }
    }

    /// A centred dialog over a dimmed window: title, body, and buttons (the
    /// first on the left, the rest on the right). Owns the keyboard while up.
    /// Returns the index of the button chosen by click or key.
    /// With `input`, a text field sits under the body; `select` is the char
    /// range selected when it first appears.
    #[allow(clippy::too_many_arguments)]
    fn modal(
        &self,
        ctx: &egui::Context,
        glyph: &str,
        title: &str,
        body: &str,
        buttons: &[Btn],
        input: Option<(&mut String, usize)>,
        more: Option<(&str, &[Btn])>,
    ) -> Option<usize> {
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let extra = more.map_or(&[][..], |m| m.1);
        let by_key = ctx.input_mut(|i| {
            buttons
                .iter()
                .chain(extra)
                .position(|b| b.keys.iter().any(|k| i.consume_key(Modifiers::NONE, *k)))
        });
        // Keys the dialog doesn't use must not reach anything behind it (with
        // a text field, they're its typing).
        if input.is_none() {
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_)
                    )
                })
            });
        }

        let screen = ctx.content_rect();
        let dim = Color32::from_black_alpha(if self.config.theme.dark { 110 } else { 50 });
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::unique("modal-backdrop"),
        ))
        .rect_filled(screen, 0.0, dim);

        // At most 70% of the window either way; a long body scrolls.
        let width = (460.0 * s).min(screen.width() * 0.7 - 2.0);
        let pos = pos2(
            screen.center().x - width / 2.0,
            screen.top() + (140.0 * s).min(screen.height() * 0.15),
        );
        // Title, field, buttons, second row.
        let chrome = (100.0
            + if input.is_some() { 50.0 } else { 0.0 }
            + if more.is_some() { 56.0 } else { 0.0 })
            * s;
        let body_max = (screen.height() * 0.7 - 40.0 * s - chrome).max(20.0 * s);
        let mut clicked = None;
        egui::Area::new(egui::Id::unique("modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(c.elevated)
                    .stroke(Stroke::new(1.0, c.border))
                    .corner_radius(12.0 * s)
                    .inner_margin(egui::Margin::same((20.0 * s) as i8))
                    .shadow(egui::Shadow {
                        offset: [0, 12],
                        blur: 40,
                        spread: 0,
                        color: Color32::from_black_alpha(80),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 40.0 * s);
                        ui.spacing_mut().item_spacing = vec2(8.0 * s, 6.0 * s);
                        ui.horizontal_top(|ui| {
                            ui.label(
                                egui::RichText::new(glyph)
                                    .font(pt.font(18.0, Weight::Regular))
                                    .color(c.accent),
                            );
                            // A long file name wraps; it never widens the dialog.
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(title)
                                        .font(pt.font(16.0, Weight::SemiBold))
                                        .color(c.text),
                                )
                                .wrap(),
                            );
                        });
                        if !body.is_empty() {
                            egui::ScrollArea::vertical()
                                .max_height(body_max)
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(body)
                                            .font(pt.font(13.0, Weight::Regular))
                                            .color(c.text_muted),
                                    );
                                });
                        }
                        if let Some((text, select)) = input {
                            ui.add_space(6.0 * s);
                            let id = ui.scope_id().with("modal-input");
                            let field = egui::Frame::NONE
                                .fill(c.background)
                                .stroke(Stroke::new(1.0, c.accent.gamma_multiply(0.8)))
                                .corner_radius(8.0 * s)
                                .inner_margin(egui::Margin::symmetric(
                                    (10.0 * s) as i8,
                                    (8.0 * s) as i8,
                                ));
                            field.show(ui, |ui| {
                                let edit = egui::TextEdit::singleline(text)
                                    .id(id)
                                    .frame(egui::Frame::NONE)
                                    .font(pt.font(14.0, Weight::Regular))
                                    .text_color(c.text)
                                    .desired_width(f32::INFINITY);
                                let resp = ui.add(edit);
                                // First frame: take the keyboard, select the name's stem.
                                if !resp.has_focus() {
                                    resp.request_focus();
                                    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
                                        let range = egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(0),
                                            egui::text::CCursor::new(select),
                                        );
                                        state.cursor.set_char_range(Some(range));
                                        state.store(ctx, id);
                                    }
                                }
                            });
                        }
                        let button = |ui: &mut egui::Ui, b: &Btn| {
                            let color = if b.primary { c.accent_text } else { c.text };
                            let text = egui::RichText::new(format!("{}  {}", b.label, b.hint))
                                .font(pt.font(13.0, Weight::Medium))
                                .color(color);
                            let widget = egui::Button::new(text)
                                .fill(if b.primary { c.accent } else { c.surface })
                                .stroke(Stroke::new(
                                    1.0,
                                    if b.primary { c.accent } else { c.border },
                                ))
                                .corner_radius(8.0 * s)
                                .min_size(vec2(0.0, 32.0 * s));
                            ui.add(widget).clicked()
                        };
                        ui.add_space(12.0 * s);
                        ui.horizontal(|ui| {

                            if let Some(first) = buttons.first()
                                && button(ui, first)
                            {
                                clicked = Some(0);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    for (i, b) in buttons.iter().enumerate().skip(1).rev() {
                                        if button(ui, b) {
                                            clicked = Some(i);
                                        }
                                    }
                                },
                            );
                        });
                        // A second row for choices that cover more than this one.
                        if let Some((caption, row)) = more {
                            ui.add_space(8.0 * s);
                            ui.separator();
                            let row_layout = egui::Layout::left_to_right(egui::Align::Center)
                                .with_main_wrap(true);
                            ui.with_layout(row_layout, |ui| {
                                ui.label(
                                    egui::RichText::new(caption)
                                        .font(pt.font(12.5, Weight::Regular))
                                        .color(c.text_subtle),
                                );
                                for (j, b) in row.iter().enumerate() {
                                    if button(ui, b) {
                                        clicked = Some(buttons.len() + j);
                                    }
                                }
                            });
                        }
                    });
            });
        by_key.or(clicked)
    }

    /// A slim, seamless bar: drag to move the window, double-click to zoom.
    /// Returns whether the sidebar button was clicked.
    fn titlebar(&self, ui: &mut egui::Ui) -> bool {
        let pt = self.paint();
        let c = &pt.c;
        let height = 32.0_f32.max(22.0 * pt.s);
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let p = ui.painter();
        p.rect_filled(rect, 0.0, c.surface);
        p.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, c.border),
        );

        // "project  ›  path/to/file": the project bright, the path quieter.
        let mut job = LayoutJob::default();
        let mut push = |s: &str, color: Color32, w: Weight| {
            job.append(s, 0.0, egui::TextFormat::simple(pt.font(12.5, w), color))
        };
        match &self.project {
            Some(proj) => {
                push(&proj.name, c.text, Weight::Medium);
                if let Some(t) = self.active_tab() {
                    push(
                        &format!("   {}   ", icon::CHEVRON_RIGHT),
                        c.text_subtle,
                        Weight::Regular,
                    );
                    push(&self.tab_rel(t), c.text_muted, Weight::Regular);
                }
            }
            None => push("Roxaruu", c.text_muted, Weight::Medium),
        }
        let g = ui.fonts_mut(|f| f.layout_job(job));
        let text = Rect::from_center_size(rect.center(), g.size());
        p.galley(text.min, g, c.text);
        if let Some(tip) = self.folder_tip() {
            ui.interact(text, egui::Id::unique("titlebar-folder"), Sense::hover())
                .on_hover_text(tip);
        }

        if resp.drag_started() {
            ui.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            crate::app::toggle_maximized(ui.ctx());
        }

        // The sidebar button, just right of the traffic lights.
        if self.project.is_none() {
            return false;
        }
        let x = rect.left() + if cfg!(target_os = "macos") { 78.0 } else { 8.0 };
        let size = 24.0 * pt.s;
        let r = Rect::from_min_size(pos2(x, rect.center().y - size / 2.0), vec2(size, size));
        let b = ui.interact(r, egui::Id::unique("sidebar-button"), Sense::click());
        let on = self.config.settings.sidebar;
        if b.hovered() {
            ui.painter().rect_filled(r, 5.0 * pt.s, c.hover);
        }
        let color = if b.hovered() {
            c.text
        } else if on {
            c.text_muted
        } else {
            c.text_subtle
        };
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            icon::SIDEBAR,
            pt.font(14.0, Weight::Regular),
            color,
        );
        let keys = self
            .config
            .keymap
            .combo_for(Command::ToggleSidebar)
            .map(|k| format!("  {}", k.caps().concat()))
            .unwrap_or_default();
        b.on_hover_text(format!(
            "{} File Tree{keys}",
            if on { "Hide" } else { "Show" }
        ))
        .clicked()
    }

    /// The keys that matter where the keyboard is now: `(keys, what, command)`.
    /// A command's keys are its current binding, so remaps show up here.
    fn status_hints(&self) -> Vec<(String, &'static str, Option<Command>)> {
        let km = &self.config.keymap;
        let cmd = |c: Command, what: &'static str| {
            km.combo_for(c).map(|k| (k.caps().concat(), what, Some(c)))
        };
        let raw = |keys: &str, what: &'static str| Some((keys.to_string(), what, None));
        // Each label says what it acts on, and follows the state ("show
        // terminal" while it's hidden, "hide terminal" while it's shown).
        let terminal_shown = self.terminal_open && !self.terminals.is_empty();
        let show_hide = if terminal_shown {
            "hide terminal"
        } else {
            "show terminal"
        };
        let list = if self.palette.is_some() || self.modal_open() {
            vec![]
        } else if self.terminal_focused() {
            vec![
                cmd(Command::SwitchFocus, "back to editor"),
                cmd(Command::NewTerminal, "new terminal"),
                cmd(Command::SwitchTerminal, "switch terminal"),
                cmd(Command::ClearTerminal, "clear terminal"),
                cmd(
                    Command::MaximizeTerminal,
                    if self.terminal_max {
                        "restore terminal"
                    } else {
                        "maximize terminal"
                    },
                ),
                cmd(Command::ToggleTerminal, show_hide),
            ]
        } else if self.focus == Focus::Sidebar && self.sidebar_shown() {
            vec![
                raw("↵", "open file"),
                raw("space", "preview file"),
                cmd(Command::NewFile, "new file"),
                cmd(Command::RenameFile, "rename"),
                cmd(Command::DeleteFile, "move to trash"),
                cmd(Command::CollapseFolders, "collapse all"),
                raw("esc", "back to editor"),
            ]
        } else if self.focus == Focus::Find {
            vec![
                raw("↵", "next match"),
                raw("⇧↵", "previous match"),
                raw("⇥", "go to replace"),
                raw("esc", "close find"),
            ]
        } else if self.active.is_some() {
            vec![
                cmd(Command::GoToFile, "go to file"),
                cmd(Command::Palette, "all commands"),
                cmd(Command::GoToLine, "go to line"),
                cmd(Command::FocusSidebar, "file tree"),
                cmd(
                    Command::SwitchFocus,
                    if terminal_shown {
                        "focus terminal"
                    } else {
                        "open terminal"
                    },
                ),
            ]
        } else if self.project.is_some() {
            vec![
                cmd(Command::GoToFile, "go to file"),
                cmd(Command::SearchInFiles, "search in files"),
                cmd(Command::FocusSidebar, "file tree"),
                cmd(
                    Command::SwitchFocus,
                    if terminal_shown {
                        "focus terminal"
                    } else {
                        "open terminal"
                    },
                ),
            ]
        } else {
            vec![
                cmd(Command::OpenFolder, "open folder"),
                cmd(Command::RecentFolders, "recent folders"),
                cmd(Command::Palette, "all commands"),
            ]
        };
        let mut out: Vec<_> = list.into_iter().flatten().collect();
        if !out.is_empty() {
            out.extend(cmd(Command::Help, "all shortcuts"));
        }
        out
    }

    /// Returns a command picked from the hints or the caret position.
    fn status_bar(&self, ui: &mut egui::Ui) -> Option<Command> {
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 26.0 * s), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 0.0, c.surface);
        p.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.border));
        let font = pt.font(12.0, Weight::Regular);

        let mut picked = None;
        let position = self.active_tab().map(|t| {
            let b = &t.buf;
            let selected = b
                .selection()
                .map(|(a, e)| format!("  ({} selected)", e - a))
                .unwrap_or_default();
            format!(
                "Ln {}, Col {}{selected}",
                b.cursor_row() + 1,
                b.cursor_col() + 1
            )
        });
        let lang = self
            .active_tab()
            .map(|t| t.buf.lang.map_or("Plain Text", |l| l.name).to_string());
        let stats = [
            Some(format!("v{}", crate::VERSION)),
            self.mem_kb
                .map(|kb| format!("{}  {:.0} MB", icon::CHIP, kb as f64 / 1024.0)),
            self.fps.map(|f| format!("{}  {f:.1} fps", icon::PULSE)),
            lang,
        ];
        // Right to left. Only the position does something on click, so only
        // it looks like a button.
        let mut x = rect.right() - 12.0 * s;
        for text in stats.into_iter().flatten() {
            let r = p.text(
                pos2(x, rect.center().y),
                Align2::RIGHT_CENTER,
                text,
                font.clone(),
                c.text_subtle,
            );
            x = r.left() - 20.0 * s;
        }
        if let Some(position) = position {
            let g = ui.fonts_mut(|f| f.layout_no_wrap(position, font.clone(), c.text_subtle));
            let r = Rect::from_min_size(
                pos2(x - g.size().x - 6.0 * s, rect.top() + 3.0 * s),
                vec2(g.size().x + 12.0 * s, rect.height() - 6.0 * s),
            );
            let keys = self
                .config
                .keymap
                .combo_for(Command::GoToLine)
                .map(|k| format!("  {}", k.caps().concat()))
                .unwrap_or_default();
            // Clicking "Ln 12, Col 4" is Go to Line, as in VS Code.
            let go = ui.interact(r, egui::Id::unique("status-position"), Sense::click());
            if go.hovered() {
                ui.painter().rect_filled(r, 4.0 * s, c.hover);
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            ui.painter().galley(
                pos2(r.left() + 6.0 * s, rect.center().y - g.size().y / 2.0),
                g,
                c.text_subtle,
            );
            if go.on_hover_text(format!("Go to Line{keys}")).clicked() {
                picked = Some(Command::GoToLine);
            }
            x = r.left();
        }
        // The key hints get everything left of the stats.
        let stats_left = x;

        let left = match self.status_line() {
            Some(msg) => Some((format!("{}  {msg}", icon::ERROR), c.error)),
            None if self.indexing() => Some(("Indexing files…".to_string(), c.text_subtle)),
            None => None,
        };
        if let Some((text, color)) = left {
            p.text(
                pos2(rect.left() + 12.0 * s, rect.center().y),
                Align2::LEFT_CENTER,
                text,
                font.clone(),
                color,
            );
        } else {
            // Key hints: a small keycap and what it does; click one to do it.
            let mut x = rect.left() + 10.0 * s;
            let cap_font = fonts::keycap(10.5 * s);
            for (i, (keys, what, command)) in self.status_hints().into_iter().enumerate() {
                let kg = ui.fonts_mut(|f| f.layout_no_wrap(keys, cap_font.clone(), c.text_muted));
                let wg = ui.fonts_mut(|f| {
                    f.layout_no_wrap(
                        what.to_string(),
                        pt.font(11.5, Weight::Regular),
                        c.text_subtle,
                    )
                });
                let cap_w = kg.size().x + 10.0 * s;
                let w = cap_w + 5.0 * s + wg.size().x;
                if x + w > stats_left - 16.0 * s {
                    break; // up to the stats on the right
                }
                let item = Rect::from_min_size(
                    pos2(x - 4.0 * s, rect.top() + 3.0 * s),
                    vec2(w + 8.0 * s, rect.height() - 6.0 * s),
                );
                let r = ui.interact(
                    item,
                    egui::Id::unique(("status-hint", i)),
                    if command.is_some() {
                        Sense::click()
                    } else {
                        Sense::hover()
                    },
                );
                let p = ui.painter();
                if r.hovered() && command.is_some() {
                    p.rect_filled(item, 4.0 * s, c.hover);
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                let cap =
                    Rect::from_min_size(pos2(x, rect.center().y - 8.5 * s), vec2(cap_w, 17.0 * s));
                p.rect(
                    cap,
                    4.0 * s,
                    c.background,
                    Stroke::new(1.0, c.border),
                    StrokeKind::Inside,
                );
                p.galley(cap.center() - kg.size() / 2.0, kg, c.text_muted);
                p.galley(
                    pos2(cap.right() + 5.0 * s, rect.center().y - wg.size().y / 2.0),
                    wg,
                    c.text_subtle,
                );
                if r.clicked() {
                    picked = command;
                }
                x += w + 14.0 * s;
            }
        }

        // Hover lists every config problem, not just the first.
        if self.errors().next().is_some() {
            resp.on_hover_ui(|ui| {
                for e in self.errors() {
                    ui.label(egui::RichText::new(e).color(c.error));
                }
            });
        }
        picked
    }

    /// The app icon as a texture (decoded once), mipmapped so the 1024px
    /// image stays crisp when drawn small. `None` if it can't be decoded.
    fn app_icon_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.app_icon.is_none() {
            let icon = eframe::icon_data::from_png_bytes(crate::ICON).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            let options = egui::TextureOptions {
                mipmap_mode: Some(egui::TextureFilter::Linear),
                ..egui::TextureOptions::LINEAR
            };
            self.app_icon = Some(ctx.load_texture("app-icon", image, options));
        }
        self.app_icon.as_ref().map(|t| t.id())
    }

    /// The start screen: brand (or the open project) and a card of actions
    /// that works with the mouse and the keyboard (↑/↓, Enter). Returns the
    /// command to run, if one was chosen.
    /// A folder open, no file: a quiet page, the folder and the keys for
    /// what usually comes next (the launcher card is for no folder).
    fn project_home(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        let project = self.project.as_ref()?;
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let terminal = if self.terminal_open && !self.terminals.is_empty() {
            "focus terminal"
        } else {
            "open terminal"
        };
        let actions = [
            (Command::GoToFile, "go to file"),
            (Command::SearchInFiles, "search in files"),
            (Command::FocusSidebar, "file tree"),
            (Command::SwitchFocus, terminal),
            (Command::Palette, "all commands"),
            (Command::Help, "all shortcuts"),
        ];
        let rows: Vec<(Command, Vec<String>, &str)> = actions
            .into_iter()
            .filter_map(|(c, what)| {
                Some((
                    c,
                    vec![self.config.keymap.combo_for(c)?.caps().concat()],
                    what,
                ))
            })
            .collect();
        let area = ui.max_rect();
        let p = ui.painter().clone();
        let layout = |text: String, font: FontId, color: Color32| {
            ui.fonts_mut(|f| f.layout_no_wrap(text, font, color))
        };
        let name = layout(
            project.name.clone(),
            pt.font(22.0, Weight::SemiBold),
            c.text_muted,
        );
        let path = layout(
            tilde(&project.root.to_string_lossy()),
            pt.font(12.5, Weight::Regular),
            c.text_subtle,
        );
        let row_h = 30.0 * s;
        let total = name.size().y + 6.0 * s + path.size().y + 28.0 * s + row_h * rows.len() as f32;
        let cx = area.center().x;
        let mut y = (area.center().y - total / 2.0).max(area.top() + 16.0);
        p.galley(
            pos2(cx - name.size().x / 2.0, y),
            name.clone(),
            c.text_muted,
        );
        y += name.size().y + 6.0 * s;
        p.galley(
            pos2(cx - path.size().x / 2.0, y),
            path.clone(),
            c.text_subtle,
        );
        y += path.size().y + 28.0 * s;
        // Two columns meeting in the middle: what it does | its keys.
        let mut chosen = None;
        for (command, caps, what) in rows {
            let mid = pos2(cx, y + row_h / 2.0);
            let label = layout(
                what.to_string(),
                pt.font(13.5, Weight::Regular),
                c.text_muted,
            );
            let caps_w = pt.key_hints(ui, None, &[(caps.clone(), "")], 12.5, Pos2::ZERO);
            let hit = Rect::from_min_max(
                pos2(cx - 12.0 * s - label.size().x, y),
                pos2(cx + 12.0 * s + caps_w, y + row_h),
            );
            let resp = ui.interact(hit, ui.scope_id().with(("home", what)), Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                chosen = Some(command);
            }
            let color = if resp.hovered() { c.text } else { c.text_muted };
            p.galley(
                pos2(
                    mid.x - 12.0 * s - label.size().x,
                    mid.y - label.size().y / 2.0,
                ),
                label,
                color,
            );
            pt.key_hints(
                ui,
                Some(&p),
                &[(caps, "")],
                12.5,
                pos2(mid.x + 12.0 * s, mid.y),
            );
            y += row_h;
        }
        chosen
    }

    fn welcome(&mut self, ui: &mut egui::Ui) -> Option<Command> {
        if self.project.is_some() {
            return self.project_home(ui);
        }
        let actions: &[Command] = &[
            Command::OpenFolder,
            Command::RecentFolders,
            Command::Palette,
            Command::Settings,
            Command::Quit,
        ];
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let n = actions.len();
        // Keys are the welcome card's only while nothing else has them (the
        // palette, a prompt, or a focused terminal below it).
        let keyboard = self.palette.is_none() && !self.modal_open() && self.focus == Focus::Editor;
        if keyboard {
            ui.input(|i| {
                if i.key_pressed(Key::ArrowDown) {
                    self.welcome_selected = (self.welcome_selected + 1) % n;
                }
                if i.key_pressed(Key::ArrowUp) {
                    self.welcome_selected = (self.welcome_selected + n - 1) % n;
                }
            });
        }
        self.welcome_selected %= n;
        let mut chosen = (keyboard && ui.input(|i| i.key_pressed(Key::Enter)))
            .then(|| actions[self.welcome_selected]);

        let area = ui.max_rect();
        let mark = 56.0 * s;
        let row_h = 40.0 * s;
        let pad = 6.0 * s;
        let card_w = (440.0 * s).min(area.width() - 32.0);
        let card_h = row_h * n as f32 + pad * 2.0;

        let (title, subtitle, tile_icon): (String, String, _) = (
            "Roxaruu".into(),
            "Your code and all its terminals, in one window.".into(),
            icon::TERMINAL,
        );
        let layout = |text: String, font: FontId, color: Color32| {
            ui.fonts_mut(|f| f.layout_no_wrap(text, font, color))
        };
        let wordmark = layout(title, pt.font(30.0, Weight::SemiBold), c.text);
        let tagline = layout(subtitle, pt.font(14.0, Weight::Regular), c.text_muted);
        let help = self
            .config
            .keymap
            .combo_for(Command::Help)
            .map(|k| k.caps().concat())
            .unwrap_or_else(|| "F1".into());
        let footer_text = format!(
            "{help} shows every shortcut   ·   {}  themes, fonts and keys live in ~/.config/oxru   ·   v{}",
            icon::GEAR,
            crate::VERSION
        );
        let footer = layout(footer_text, pt.font(12.0, Weight::Regular), c.text_subtle);

        let total =
            mark + 14.0 * s + tagline.size().y + 36.0 * s + card_h + 20.0 * s + footer.size().y;
        let cx = area.center().x;
        let mut y = (area.center().y - total / 2.0).max(area.top() + 16.0);
        let p = ui.painter().clone();

        // Brand: an accent tile, then the wordmark.
        let gap = 14.0 * s;
        let tile = Rect::from_min_size(
            pos2(cx - (mark + gap + wordmark.size().x) / 2.0, y),
            vec2(mark, mark),
        );
        match self.app_icon_texture(ui.ctx()) {
            // The icon's art fills 824 of its 1024 pixels (Apple's grid,
            // with room for the shadow): scale so the art is `mark` wide.
            Some(id) => {
                let full = Rect::from_center_size(tile.center(), tile.size() * (1024.0 / 824.0));
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                p.image(id, full, uv, Color32::WHITE);
            }
            None => {
                p.rect_filled(tile, 12.0 * s, c.accent);
                p.text(
                    tile.center(),
                    Align2::CENTER_CENTER,
                    tile_icon,
                    pt.font(22.0, Weight::Regular),
                    c.accent_text,
                );
            }
        }
        p.galley(
            pos2(
                tile.right() + gap,
                tile.center().y - wordmark.size().y / 2.0,
            ),
            wordmark,
            c.text,
        );
        y += mark + 14.0 * s;
        p.galley(
            pos2(cx - tagline.size().x / 2.0, y),
            tagline.clone(),
            c.text_muted,
        );
        y += tagline.size().y + 36.0 * s;

        let card = Rect::from_min_size(pos2(cx - card_w / 2.0, y), vec2(card_w, card_h));
        p.rect(
            card,
            12.0 * s,
            c.elevated,
            Stroke::new(1.0, c.border),
            StrokeKind::Inside,
        );
        for (i, command) in actions.iter().enumerate() {
            let row = Rect::from_min_size(
                pos2(card.left() + pad, card.top() + pad + row_h * i as f32),
                vec2(card_w - pad * 2.0, row_h),
            );
            let resp = ui.interact(row, ui.scope_id().with(("welcome", i)), Sense::click());
            if resp.hovered() && keyboard {
                self.welcome_selected = i;
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                chosen = Some(*command);
            }
            let t = ui
                .ctx()
                .animate_bool_with_time(resp.id, self.welcome_selected == i, 0.12);
            let caps = self
                .config
                .keymap
                .combo_for(*command)
                .map(|k| k.caps())
                .unwrap_or_default();
            let title = crate::keymap::COMMANDS
                .iter()
                .find(|ci| ci.command == *command)
                .map_or("", |ci| ci.title);
            pt.row(
                ui,
                &p,
                row,
                t,
                (icon::for_command(*command), None),
                pt.label(title, &[], c.text),
                "",
                &caps,
                &[],
                0.0,
            );
        }
        y += card_h + 20.0 * s;

        let footer_rect = Rect::from_min_size(pos2(cx - footer.size().x / 2.0, y), footer.size());
        let resp = ui.interact(
            footer_rect,
            ui.scope_id().with("welcome-footer"),
            Sense::click(),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            chosen = Some(Command::OpenConfigFolder);
        }
        p.galley(
            footer_rect.min,
            footer,
            if resp.hovered() {
                c.text_muted
            } else {
                c.text_subtle
            },
        );

        chosen
    }

    /// One tab's editor, filling `ui`: gutter, highlighted text, find
    /// matches, selections and carets, with typing (when `keyboard`) and the
    /// mouse. Lines are laid out as screen rows — a whole line each, or its
    /// wrapped pieces with word wrap on — and only visible rows are built, so
    /// a 100k-line file costs the same per frame as a 10-line one. Returns
    /// whether it was clicked (so split view can focus it).
    fn editor_tile(&mut self, ui: &mut egui::Ui, i: usize, keyboard: bool) -> bool {
        let c = self.config.theme.colors.clone();
        let size = self.config.settings.code_font_size;
        let font = FontId::monospace(size);
        let row_h = (size * 1.6).round();
        let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let view = ui.available_size();
        let page = ((view.y / row_h) as usize).saturating_sub(2).max(1);
        let now = ui.input(|inp| inp.time);
        let wrap = self.config.settings.word_wrap;
        let Some(tab) = self.tabs.get_mut(i) else {
            return false;
        };

        if keyboard {
            let events = ui.input(|inp| inp.events.clone());
            if events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                )
            }) {
                tab.input(&events, page);
                tab.reveal = true;
                tab.caret_epoch = now;
            }
        }
        tab.refresh(Some(ui.ctx()));

        let digits = tab.buf.line_count().to_string().len().max(3);
        let gutter = digits as f32 * char_w + 36.0;
        let text_x = gutter + 8.0;
        // Screen rows: (line, first char column, end char column).
        let wrap_cols = ((view.x - text_x - 24.0) / char_w).floor().max(20.0) as usize;
        let rows: Vec<(usize, usize, usize)> = if wrap {
            tab.wrapped_rows(wrap_cols).to_vec()
        } else {
            Vec::new()
        };
        let n_rows = if wrap {
            rows.len()
        } else {
            tab.buf.line_count()
        };
        let row_at = |r: usize| if wrap { rows[r] } else { (r, 0, usize::MAX) };
        let content_w = if wrap {
            view.x
        } else {
            text_x + tab.max_cols as f32 * char_w + 64.0
        };

        let b = &tab.buf;
        let line_text = |l: usize| {
            let s = b.rope.line(l).to_string();
            s.trim_end_matches(['\n', '\r']).to_string()
        };
        let crow = b.cursor_row();
        let ccol_char = b.cursor_col();
        // The screen row the caret is on.
        let caret_row = if wrap {
            let first = rows.partition_point(|r| r.0 < crow);
            (first..rows.len())
                .take_while(|&r| rows[r].0 == crow)
                .filter(|&r| rows[r].1 <= ccol_char)
                .last()
                .unwrap_or(first)
        } else {
            crow
        };
        let caret_x = {
            let (_, s0, _) = row_at(caret_row);
            let line = line_text(crow);
            let seg: String = line.chars().skip(s0).collect();
            text_x + char_to_col(&seg, ccol_char.saturating_sub(s0)) as f32 * char_w
        };

        // Keep the caret in view after keyboard edits and moves.
        let mut area = if wrap {
            egui::ScrollArea::vertical()
        } else {
            egui::ScrollArea::both()
        }
        .id_salt(("editor", tab.path().cloned(), wrap))
        .auto_shrink(false)
        .scroll_source(
            egui::scroll_area::ScrollSource::MOUSE_WHEEL
                | egui::scroll_area::ScrollSource::SCROLL_BAR,
        );
        if tab.reveal {
            tab.reveal = false;
            let (top, bottom) = (caret_row as f32 * row_h, (caret_row + 1) as f32 * row_h);
            let mut y = tab.scroll.y;
            if top < y {
                y = top;
            } else if bottom > y + view.y - row_h {
                y = bottom - view.y + row_h;
            }
            area = area.vertical_scroll_offset(y.max(0.0));
            if !wrap {
                let mut x = tab.scroll.x;
                if caret_x - text_x - 4.0 * char_w < x {
                    x = (caret_x - text_x - 4.0 * char_w).max(0.0);
                } else if caret_x + 4.0 * char_w > x + view.x {
                    x = caret_x + 4.0 * char_w - view.x;
                }
                area = area.horizontal_scroll_offset(x.max(0.0));
            }
        }

        // Caret: solid while typing, then blinking; none while the keyboard is elsewhere.
        let focused = ui.input(|inp| inp.viewport().focused.unwrap_or(true));
        let since = now - tab.caret_epoch;
        let caret_on = keyboard && (!focused || since < 0.5 || ((since / 0.53) as i64) % 2 == 0);
        if focused && keyboard {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(0.53 - since % 0.53));
        }

        let found: &[(usize, usize)] = match &self.find {
            Some(f) if !f.query.is_empty() && self.active == Some(i) => &f.matches,
            _ => &[],
        };
        let show_current_line = self.config.settings.current_line && b.selection().is_none();
        let show_guides = self.config.settings.indent_guides;
        let unit = b.indent_unit().max(1);
        let sel = b.selection();
        let extra: Vec<(usize, (usize, usize))> = b
            .extra_carets()
            .iter()
            .map(|k| (k.cursor, k.bounds()))
            .collect();
        let tab_str = " ".repeat(crate::app::TAB_WIDTH);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // Scroll past the end by half a screen, so the last line can sit
        // clear of the bottom edge (and of a horizontal scrollbar).
        let tail = ((view.y / row_h) as usize / 2).max(2);
        let out = area.show_rows(ui, row_h, n_rows + tail, |ui, visible| {
            ui.set_min_width(content_w);
            for r in visible {
                if r >= n_rows {
                    ui.allocate_exact_size(vec2(1.0, row_h), Sense::hover());
                    continue;
                }
                let (l, seg_start, seg_end) = row_at(r);
                let (rect, _) = ui.allocate_exact_size(
                    vec2(content_w.max(ui.available_width()), row_h),
                    Sense::hover(),
                );
                let p = ui.painter();
                let y = rect.center().y;
                let tx = rect.left() + text_x;
                let full = line_text(l);
                let full_len = full.chars().count();
                let (s0, e0) = (seg_start.min(full_len), seg_end.min(full_len));
                let last_piece = e0 >= full_len;
                // This row's slice of the line, as chars and as a byte range.
                let byte = |ch: usize| full.char_indices().nth(ch).map_or(full.len(), |(b, _)| b);
                let (b0, b1) = (byte(s0), byte(e0));
                let line = &full[b0..b1];
                let line_start = b.rope.line_to_char(l);
                let start = line_start + s0;
                let len = e0 - s0;

                if s0 == 0 {
                    let num_color = if l == crow {
                        c.text_muted
                    } else {
                        c.text_subtle
                    };
                    p.text(
                        pos2(rect.left() + gutter - 16.0, y),
                        Align2::RIGHT_CENTER,
                        (l + 1).to_string(),
                        font.clone(),
                        num_color,
                    );
                }
                if l == crow && show_current_line {
                    p.rect_filled(rect, 0.0, c.line_highlight);
                }
                if show_guides && s0 == 0 {
                    let level = guide_cols(b, l);
                    let mut col = unit;
                    while col < level {
                        let x = (tx + col as f32 * char_w).round() + 0.5;
                        p.vline(x, rect.y_range(), Stroke::new(1.0, c.indent_guide));
                        col += unit;
                    }
                }

                // Find matches on this row (they're sorted: skip to the first).
                let first = found.partition_point(|m| m.1 <= start);
                for &(a, e) in found[first..].iter().take_while(|m| m.0 <= start + len) {
                    let x0 = tx + char_to_col(line, a.max(start) - start) as f32 * char_w;
                    let x1 = tx + char_to_col(line, e.min(start + len) - start) as f32 * char_w;
                    let r =
                        Rect::from_x_y_ranges(x0..=x1.max(x0 + 2.0), rect.y_range().shrink(2.0));
                    p.rect(
                        r,
                        3.0,
                        c.accent.gamma_multiply(0.14),
                        Stroke::new(1.0, c.accent.gamma_multiply(0.45)),
                        StrokeKind::Inside,
                    );
                }

                // Selections (primary and extra carets) on this row.
                let ranges = sel
                    .into_iter()
                    .chain(extra.iter().map(|e| e.1).filter(|(a, e)| a != e));
                for (a, e) in ranges {
                    if e < start || a > start + len {
                        continue;
                    }
                    let (sa, se) = (
                        a.max(start) - start,
                        (e.min(start + len + 1) - start).min(len),
                    );
                    let x0 = tx + char_to_col(line, sa) as f32 * char_w;
                    // A selected line break shows as a little extra width.
                    let nl = if last_piece && e > start + len {
                        char_w * 0.6
                    } else {
                        0.0
                    };
                    let x1 = tx + char_to_col(line, se) as f32 * char_w + nl;
                    if x1 > x0 {
                        p.rect_filled(
                            Rect::from_x_y_ranges(x0..=x1, rect.y_range()),
                            3.0,
                            c.accent.gamma_multiply(0.28),
                        );
                    }
                }

                // Text, coloured by the (possibly a moment stale) highlight.
                let mut job = LayoutJob::default();
                let mut push = |s: &str, color: Color32| {
                    if !s.is_empty() {
                        job.append(
                            &s.replace('\t', &tab_str),
                            0.0,
                            egui::TextFormat::simple(font.clone(), color),
                        );
                    }
                };
                let mut at = b0;
                for (range, token) in tab.spans.get(l).map(Vec::as_slice).unwrap_or_default() {
                    let (s1, e1) = (range.start.clamp(b0, b1), range.end.clamp(b0, b1));
                    if s1 < at
                        || s1 >= e1
                        || !full.is_char_boundary(s1)
                        || !full.is_char_boundary(e1)
                    {
                        continue;
                    }
                    push(&full[at..s1], c.text);
                    push(&full[s1..e1], c.token(*token));
                    at = e1;
                }
                push(&full[at..b1], c.text);
                let g = ui.fonts_mut(|f| f.layout_job(job));
                p.galley(pos2(tx, y - g.size().y / 2.0), g, c.text);

                // Carets. One at the end of a wrapped piece belongs to the next row.
                if caret_on {
                    let carets = std::iter::once(b.cursor).chain(extra.iter().map(|e| e.0));
                    for pos in carets.filter(|&pos| {
                        pos >= start && (pos < start + len || (pos == start + len && last_piece))
                    }) {
                        let x = tx + char_to_col(line, pos - start) as f32 * char_w;
                        let h = row_h * 0.72;
                        p.rect_filled(
                            Rect::from_min_size(pos2(x - 1.0, y - h / 2.0), vec2(2.0, h)),
                            1.0,
                            c.accent,
                        );
                    }
                }
            }
        });
        tab.scroll = out.state.clamped_offset();

        // Mouse: click places the caret, drag selects, Shift+click extends,
        // double-click selects a word, triple-click a line.
        let viewport = out.inner_rect;
        let resp = ui.interact(
            viewport,
            ui.scope_id().with(("editor-mouse", i)),
            Sense::click_and_drag(),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        let Some(ptr) = ui.input(|inp| inp.pointer.interact_pos()) else {
            return false;
        };
        let pressed = resp.hovered() && ui.input(|inp| inp.pointer.primary_pressed());
        let dragging = resp.dragged_by(egui::PointerButton::Primary);
        if !(pressed || dragging || resp.double_clicked() || resp.triple_clicked()) {
            return false;
        }
        let r = (((ptr.y - viewport.top() + tab.scroll.y) / row_h)
            .floor()
            .max(0.0) as usize)
            .min(n_rows.saturating_sub(1));
        let (l, s0, e0) = row_at(r);
        let b = &mut tab.buf;
        let full = {
            let s = b
                .rope
                .line(l.min(b.line_count().saturating_sub(1)))
                .to_string();
            s.trim_end_matches(['\n', '\r']).to_string()
        };
        let piece: String = full.chars().skip(s0).take(e0.saturating_sub(s0)).collect();
        let col = (ptr.x - viewport.left() + tab.scroll.x - text_x) / char_w;
        let idx = b.rope.line_to_char(l) + s0 + col_to_char(&piece, col.max(0.0));
        let shift = ui.input(|inp| inp.modifiers.shift);
        if resp.triple_clicked() {
            b.select_line_at(idx);
        } else if resp.double_clicked() {
            b.select_word_at(idx);
        } else if pressed {
            b.clear_extra_carets();
            if shift {
                b.begin_selection();
                tab.drag_from = None;
            } else {
                b.clear_selection();
                tab.drag_from = Some(idx); // a drag selects from here
            }
            b.cursor = idx;
        } else if dragging {
            if let Some(from) = tab.drag_from {
                b.anchor = Some(from);
            }
            b.cursor = idx;
            // Dragging past an edge scrolls toward it.
            if !viewport.contains(ptr) {
                tab.reveal = true;
            }
        }
        let b = &mut tab.buf;
        b.goal_col = b.cursor_col();
        tab.caret_epoch = now;
        pressed
    }

    /// The keys for tabs, shown on the strip they act on: editor tabs, or
    /// (`terminal`) terminal tabs. Most useful first; only what applies.
    pub(crate) fn tab_hints(&self, terminal: bool) -> Vec<Hint> {
        let n = if terminal { self.terminals.len() } else { self.tabs.len() };
        let caps = |cs: &[Command]| -> Vec<String> { cs.iter().filter_map(|c| self.shortcut(*c)).collect() };
        let close = if terminal { Command::CloseTerminal } else { Command::CloseTab };
        let mut v: Vec<(Vec<String>, &'static str, bool)> = vec![
            (caps(&[close]), "close", n > 0),
            (caps(&[Command::CloseTabsToLeft, Command::CloseTabsToRight]), "close left / right", n > 1),
            (caps(&[Command::CloseOtherTabs]), "close others", n > 1),
        ];
        if terminal {
            v.extend([
                (caps(&[Command::NewTerminal]), "new", true),
                (caps(&[Command::NextTab]), "next", n > 1),
                (caps(&[Command::MoveTabLeft, Command::MoveTabRight]), "move", n > 1),
                (caps(&[Command::ToggleTerminalGrid]), "side by side", n > 1),
            ]);
        } else {
            v.extend([
                (caps(&[Command::CloseAllTabs]), "close all", n > 1),
                (caps(&[Command::CloseSavedTabs]), "close saved", self.available(Command::CloseSavedTabs) && n > 1),
                (caps(&[Command::NextTab]), "next", n > 1),
                (caps(&[Command::MoveTabLeft, Command::MoveTabRight]), "move", n > 1),
                (caps(&[Command::ReopenTab]), "reopen", self.available(Command::ReopenTab)),
            ]);
        }
        v.into_iter()
            .filter(|(caps, _, applies)| *applies && !caps.is_empty())
            .map(|(caps, what, _)| (caps, what))
            .collect()
    }

    /// The right-click menu of editor tab `i` (on its tab, or its title in
    /// split view). Right-click makes it the active tab, so these act on it.
    fn tab_menu(&self, i: usize) -> MenuGroups {
        let n = self.tabs.len();
        let has_path = self.tabs.get(i).is_some_and(|t| t.path().is_some());
        vec![
            vec![
                (Command::CloseTab, "Close", true),
                (Command::CloseOtherTabs, "Close Others", n > 1),
                (Command::CloseTabsToRight, "Close to the Right", i + 1 < n),
                (Command::CloseTabsToLeft, "Close to the Left", i > 0),
                (
                    Command::CloseSavedTabs,
                    "Close Saved",
                    self.tabs.iter().any(|t| !t.buf.modified),
                ),
                (Command::CloseAllTabs, "Close All", true),
            ],
            vec![
                (
                    Command::ReopenTab,
                    "Reopen Closed Tab",
                    self.available(Command::ReopenTab),
                ),
                (Command::MoveTabLeft, "Move Left", i > 0),
                (Command::MoveTabRight, "Move Right", i + 1 < n),
            ],
            vec![
                (Command::CopyPath, "Copy Path", has_path),
                (Command::CopyRelativePath, "Copy Relative Path", has_path),
                (Command::RevealInFinder, "Reveal in Finder", has_path),
                (Command::FocusSidebar, "Show in File Tree", has_path),
            ],
        ]
    }

    /// The right-click menu of terminal `i` (its tab, or its title side by
    /// side). Right-click focuses that terminal, so these act on it.
    fn terminal_menu(&self, i: usize) -> MenuGroups {
        let n = self.terminals.len();
        vec![
            vec![(Command::NewTerminal, "New Terminal", true)],
            vec![
                (Command::CloseTerminal, "Close", true),
                (Command::CloseOtherTabs, "Close Others", n > 1),
                (Command::CloseTabsToRight, "Close to the Right", i + 1 < n),
                (Command::CloseTabsToLeft, "Close to the Left", i > 0),
            ],
            vec![
                (Command::MoveTabLeft, "Move Left", i > 0),
                (Command::MoveTabRight, "Move Right", i + 1 < n),
            ],
            vec![
                (Command::ClearTerminal, "Clear", true),
                (Command::MaximizeTerminal, "Maximize / Restore", true),
                (Command::ToggleTerminalGrid, "Side by Side / as Tabs", n > 1),
            ],
        ]
    }

    /// The key that runs `c`, as keycaps ("⇧⌘W").
    fn shortcut(&self, c: Command) -> Option<String> {
        self.config.keymap.combo_for(c).map(|k| k.caps().concat())
    }

    /// The editor area: the active tab, or — in split view — every open tab
    /// tiled, the active one outlined. Returns the active tile's rect.
    fn editors(&mut self, ui: &mut egui::Ui) -> Rect {
        let keyboard = self.palette.is_none() && !self.modal_open() && self.focus == Focus::Editor;
        let area = ui.max_rect();
        let Some(active) = self.active else {
            return area;
        };
        if !(self.editor_grid && self.tabs.len() > 1) {
            if self.editor_tile(ui, active, keyboard) {
                self.focus = Focus::Editor;
            }
            return area;
        }
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let tiles = crate::terminal::grid_rects(area.shrink(6.0 * s), self.tabs.len(), 6.0 * s);
        let mut active_rect = area;
        let mut clicked = None;
        let mut tile_pick = None;
        for (i, tile) in tiles.into_iter().enumerate() {
            let is_active = i == active;
            let border = if is_active && self.focus == Focus::Editor {
                c.accent.gamma_multiply(0.7)
            } else {
                c.border
            };
            ui.painter().rect(
                tile,
                8.0 * s,
                c.background,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
            let title = Rect::from_min_size(tile.min, vec2(tile.width(), 26.0 * s));
            ui.painter().hline(
                title.x_range().shrink(1.0),
                title.bottom(),
                Stroke::new(1.0, c.border),
            );
            let title_resp = ui.interact(title, ui.scope_id().with(("tile-title", i)), Sense::click());
            if title_resp.clicked() || title_resp.secondary_clicked() {
                clicked = Some(i);
            }
            title_resp.context_menu(|ui| {
                tile_pick = tile_pick.or(command_menu(ui, &self.tab_menu(i), &|c| self.shortcut(c)));
            });
            let name = self.tabs[i].buf.name();
            let dot = if self.tabs[i].buf.modified {
                "  ●"
            } else {
                ""
            };
            ui.painter().with_clip_rect(title.shrink(4.0)).text(
                pos2(title.left() + 12.0 * s, title.center().y),
                Align2::LEFT_CENTER,
                format!("{name}{dot}"),
                pt.font(12.5, Weight::Medium),
                if is_active { c.text } else { c.text_muted },
            );
            let inner = Rect::from_min_max(
                pos2(tile.left() + 2.0, title.bottom() + 2.0),
                tile.max - vec2(2.0, 2.0),
            );
            if is_active {
                active_rect = inner;
            }
            let pressed = ui
                .scope_builder(
                    egui::UiBuilder::new().max_rect(inner).id_salt(("tile", i)),
                    |ui| {
                        ui.set_clip_rect(inner);
                        self.editor_tile(ui, i, keyboard && is_active)
                    },
                )
                .inner;
            if pressed {
                clicked = Some(i);
            }
        }
        if let Some(i) = clicked {
            self.active = Some(i);
            self.focus = Focus::Editor;
        }
        if let Some(c) = tile_pick {
            self.focus = Focus::Editor; // the menu acts on the active tab
            let ctx = ui.ctx().clone();
            self.run(c, &ctx);
        }
        active_rect
    }

    /// Find / replace, floating at the editor's top right. Enter / ⇧Enter step
    /// through matches (in the replace field: replace one / replace all),
    /// Tab swaps fields, Esc closes.
    fn find_bar(&mut self, ctx: &egui::Context, area: Rect) {
        let Some(i) = self.active else {
            return self.close_find();
        };
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let focused = self.focus == Focus::Find && self.palette.is_none() && !self.modal_open();
        let caps = |cmd: Command| {
            self.config
                .keymap
                .combo_for(cmd)
                .map(|k| format!("  ({})", k.caps().join("")))
                .unwrap_or_default()
        };
        let tip_replace = format!("Toggle replace{}", caps(Command::Replace));

        let (mut close, mut note) = (false, None);
        let (Some(f), Some(tab)) = (self.find.as_mut(), self.tabs.get_mut(i)) else {
            return;
        };
        if focused {
            let (enter, back, esc, tab_key) = ctx.input_mut(|inp| {
                // ⇧Enter first: egui's key matching ignores Shift unless asked,
                // so a plain-Enter check would swallow it.
                let back = inp.consume_key(
                    Modifiers {
                        shift: true,
                        ..Modifiers::NONE
                    },
                    Key::Enter,
                );
                (
                    !back && inp.consume_key(Modifiers::NONE, Key::Enter),
                    back,
                    inp.consume_key(Modifiers::NONE, Key::Escape),
                    inp.consume_key(Modifiers::NONE, Key::Tab),
                )
            });
            close = esc;
            if tab_key && f.replace_open {
                f.in_replace = !f.in_replace;
            }
            match (f.in_replace, enter, back) {
                (false, true, _) => f.step(&mut tab.buf, 1),
                (false, _, true) => f.step(&mut tab.buf, -1),
                (true, true, _) => f.replace_one(&mut tab.buf, i),
                (true, _, true) => {
                    note = Some(format!(
                        "Replaced {} matches",
                        f.replace_all(&mut tab.buf, i)
                    ))
                }
                _ => {}
            }
            if enter || back {
                tab.reveal = true;
            }
        }

        let width = 476.0 * s;
        let pos = pos2(area.right() - width - 22.0 * s, area.top() + 10.0 * s);
        let mut grab_focus = false;
        egui::Area::new(egui::Id::unique("find-bar"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(c.elevated)
                    .stroke(Stroke::new(1.0, c.border))
                    .corner_radius(10.0 * s)
                    .inner_margin(egui::Margin::same((6.0 * s) as i8))
                    .shadow(egui::Shadow {
                        offset: [0, 6],
                        blur: 20,
                        spread: 0,
                        color: Color32::from_black_alpha(60),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 12.0 * s);
                        ui.spacing_mut().item_spacing = vec2(4.0 * s, 4.0 * s);
                        let field = |ui: &mut egui::Ui,
                                     text: &mut String,
                                     hint: &str,
                                     active: bool| {
                            let edit = egui::TextEdit::singleline(text)
                                .font(pt.font(13.5, Weight::Regular))
                                .text_color(c.text)
                                .hint_text(egui::RichText::new(hint).color(c.text_subtle))
                                .background_color(c.background)
                                .margin(egui::Margin::symmetric((8.0 * s) as i8, (5.0 * s) as i8))
                                .desired_width(220.0 * s);
                            let resp = ui.add(edit);
                            if active {
                                resp.request_focus();
                            }
                            resp
                        };
                        ui.horizontal(|ui| {
                            let chevron = if f.replace_open {
                                icon::CHEVRON_DOWN
                            } else {
                                icon::CHEVRON_RIGHT
                            };
                            if pt.icon_button(ui, chevron, &tip_replace, false) {
                                f.replace_open = !f.replace_open;
                                f.in_replace = f.replace_open;
                                grab_focus = true;
                            }
                            let resp = field(ui, &mut f.query, "Find", focused && !f.in_replace);
                            if resp.changed() {
                                f.refresh(&tab.buf, i);
                                f.jump_from_origin(&mut tab.buf);
                                tab.reveal = true;
                            }
                            if resp.clicked() {
                                f.in_replace = false;
                                grab_focus = true;
                            }
                            let toggles = [
                                (icon::CASE, "Match case  ⌥⌘C", f.case),
                                (icon::WHOLE_WORD, "Whole word  ⌥⌘W", f.word),
                            ];
                            for (n, (glyph, tip, on)) in toggles.into_iter().enumerate() {
                                if pt.icon_button(ui, glyph, tip, on) {
                                    if n == 0 {
                                        f.case = !f.case;
                                    } else {
                                        f.word = !f.word;
                                    }
                                    f.refresh(&tab.buf, i);
                                    f.jump_from_origin(&mut tab.buf);
                                    tab.reveal = true;
                                    grab_focus = true;
                                }
                            }
                            let count = match (f.query.is_empty(), f.matches.len()) {
                                (true, _) => String::new(),
                                (false, 0) => "No results".to_string(),
                                (false, n) => format!("{} of {n}", f.current + 1),
                            };
                            let color = if !f.query.is_empty() && f.matches.is_empty() {
                                c.error
                            } else {
                                c.text_muted
                            };
                            let (r, _) =
                                ui.allocate_exact_size(vec2(74.0 * s, 24.0 * s), Sense::hover());
                            ui.painter().text(
                                r.center(),
                                Align2::CENTER_CENTER,
                                count,
                                pt.font(12.0, Weight::Regular),
                                color,
                            );
                            if pt.icon_button(ui, icon::ARROW_UP, "Previous match (⇧↵)", false)
                            {
                                f.step(&mut tab.buf, -1);
                                tab.reveal = true;
                            }
                            if pt.icon_button(ui, icon::ARROW_DOWN, "Next match (↵)", false) {
                                f.step(&mut tab.buf, 1);
                                tab.reveal = true;
                            }
                            if pt.icon_button(ui, icon::CLOSE, "Close (esc)", false) {
                                close = true;
                            }
                        });
                        if f.replace_open {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0 * s);
                                let resp =
                                    field(ui, &mut f.replace, "Replace", focused && f.in_replace);
                                if resp.clicked() {
                                    f.in_replace = true;
                                    grab_focus = true;
                                }
                                if pt.icon_button(
                                    ui,
                                    icon::REPLACE,
                                    "Replace (↵ in this field)",
                                    false,
                                ) {
                                    f.replace_one(&mut tab.buf, i);
                                    tab.reveal = true;
                                }
                                if pt.icon_button(
                                    ui,
                                    icon::REPLACE_ALL,
                                    "Replace all (⇧↵ in this field)",
                                    false,
                                ) {
                                    note = Some(format!(
                                        "Replaced {} matches",
                                        f.replace_all(&mut tab.buf, i)
                                    ));
                                }
                            });
                        }
                    });
            });
        if grab_focus {
            self.focus = Focus::Find;
        }
        if let Some(n) = note {
            self.notify(n);
        }
        if close {
            self.close_find();
        }
    }

    /// The terminal panel: a strip of terminal tabs with the panel's
    /// buttons, then the active terminal — or, in grid mode, all of them.
    fn terminal_panel(&mut self, ui: &mut egui::Ui) {
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let focused = self.focus == Focus::Terminal && self.palette.is_none() && !self.modal_open();

        // Header: terminal tabs, then buttons on the right.
        let (head, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 32.0 * s), Sense::hover());
        let p = ui.painter().clone();
        p.rect_filled(head, 0.0, c.surface);
        p.hline(head.x_range(), head.top() + 0.5, Stroke::new(1.0, c.border));
        p.hline(
            head.x_range(),
            head.bottom() - 0.5,
            Stroke::new(1.0, c.border),
        );

        let buttons: [(&str, &str, Command); 4] = [
            (
                icon::CHEVRON_DOWN,
                "Hide terminals",
                Command::ToggleTerminal,
            ),
            (
                if self.terminal_max {
                    icon::RESTORE
                } else {
                    icon::MAXIMIZE
                },
                "Maximize / restore",
                Command::MaximizeTerminal,
            ),
            (icon::GRID, "Grid / tabs", Command::ToggleTerminalGrid),
            (icon::ADD, "New terminal", Command::NewTerminal),
        ];
        let mut run = None;
        let mut bx = head.right() - 18.0 * s;
        for (n, (glyph, tip, command)) in buttons.iter().enumerate() {
            let r = Rect::from_center_size(pos2(bx, head.center().y), vec2(24.0 * s, 24.0 * s));
            bx -= 28.0 * s;
            let resp = ui.interact(r, ui.scope_id().with(("term-btn", n)), Sense::click());
            let on = *command == Command::ToggleTerminalGrid && self.terminal_grid;
            if resp.hovered() || on {
                p.rect_filled(r, 5.0 * s, c.hover);
            }
            let color = if on {
                c.accent
            } else if resp.hovered() {
                c.text
            } else {
                c.text_subtle
            };
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                *glyph,
                pt.font(14.0, Weight::Regular),
                color,
            );
            let keys = self
                .config
                .keymap
                .combo_for(*command)
                .map(|k| format!("  ({})", k.caps().join("")))
                .unwrap_or_default();
            if resp.on_hover_text(format!("{tip}{keys}")).clicked() {
                run = Some(*command);
            }
        }

        let names: Vec<String> = self
            .terminals
            .iter()
            .map(|t| t.pane.display_name())
            .collect();
        let labels = crate::terminal::labels(&names);
        let tabs_right = bx + 8.0 * s;
        // While a terminal has the keyboard, its tab keys keep some room.
        let hints = if focused && self.palette.is_none() {
            self.tab_hints(true)
        } else {
            Vec::new()
        };
        let reserve = if hints.is_empty() {
            0.0
        } else {
            let first = &hints[..hints.len().min(3)];
            (pt.key_hints(ui, None, first, 11.5, Pos2::ZERO) + 28.0 * s)
                .min((tabs_right - head.left()) * 0.45)
        };
        let mut x = head.left() + 6.0 * s;
        let (mut select, mut close, mut menu_pick) = (None, None, None);
        for (i, label) in labels.iter().enumerate() {
            let active = i == self.active_terminal;
            let number = if i < 9 {
                format!("{}  ", i + 1)
            } else {
                String::new()
            };
            let mut job = LayoutJob::default();
            job.append(
                &number,
                0.0,
                egui::TextFormat::simple(pt.font(11.5, Weight::Medium), c.text_subtle),
            );
            job.append(
                label,
                0.0,
                egui::TextFormat::simple(
                    pt.font(12.5, Weight::Medium),
                    if active { c.text } else { c.text_muted },
                ),
            );
            let g = ui.fonts_mut(|f| f.layout_job(job));
            let w = 12.0 * s + g.size().x + 30.0 * s;
            let rect = Rect::from_min_size(
                pos2(x, head.top() + 4.0 * s),
                vec2(w, head.height() - 8.0 * s),
            );
            if rect.right() > tabs_right - reserve && i != self.active_terminal {
                break; // more than fit; ⌥K and ⌥1-9 still reach them
            }
            let resp = ui.interact(rect, ui.scope_id().with(("term-tab", i)), Sense::click());
            let close_rect = Rect::from_center_size(
                pos2(rect.right() - 13.0 * s, rect.center().y),
                vec2(16.0 * s, 16.0 * s),
            );
            let over_close = resp.hovered()
                && ui.input(|inp| {
                    inp.pointer
                        .hover_pos()
                        .is_some_and(|ptr| close_rect.contains(ptr))
                });
            if resp.clicked() {
                if over_close {
                    close = Some(i)
                } else {
                    select = Some(i)
                }
            }
            if resp.middle_clicked() {
                close = Some(i);
            }
            if resp.secondary_clicked() {
                select = Some(i);
            }
            let key = |c: Command| self.shortcut(c);
            let tip = if over_close {
                format!(
                    "Close terminal{}",
                    key(Command::CloseTerminal)
                        .filter(|_| active)
                        .map(|k| format!("  {k}"))
                        .unwrap_or_default()
                )
            } else {
                let jump = if i < 9 {
                    key(Command::JumpTerminal(i as u8 + 1))
                } else {
                    None
                };
                let status = if self.terminals[i].pane.is_running() {
                    "running"
                } else {
                    "idle"
                };
                match jump {
                    Some(k) => format!(
                        "{label} · {status}\n{k} to jump here · {} for all",
                        key(Command::SwitchTerminal).unwrap_or_default()
                    ),
                    None => format!("{label} · {status}"),
                }
            };
            let resp = resp.on_hover_text(tip);
            // Right-click focuses the terminal, so these act on it.
            resp.context_menu(|ui| {
                menu_pick = menu_pick.or(command_menu(ui, &self.terminal_menu(i), &key));
            });
            if active {
                p.rect(
                    rect,
                    6.0 * s,
                    c.background,
                    Stroke::new(
                        1.0,
                        if focused {
                            c.accent.gamma_multiply(0.6)
                        } else {
                            c.border
                        },
                    ),
                    StrokeKind::Inside,
                );
            } else if resp.hovered() {
                p.rect_filled(rect, 6.0 * s, c.hover);
            }
            p.galley(
                pos2(rect.left() + 10.0 * s, rect.center().y - g.size().y / 2.0),
                g,
                c.text,
            );
            // Right slot: × on hover; else running (dot) or finished-unseen (✓).
            let t = &self.terminals[i];
            let (glyph, color, size) = if resp.hovered() {
                (
                    icon::CLOSE,
                    if over_close { c.text } else { c.text_subtle },
                    12.0,
                )
            } else if t.pane.attention {
                (icon::BELL, c.accent, 12.0)
            } else if t.pane.is_running() {
                (icon::DOT, c.accent, 8.0)
            } else if t.pane.finished_unseen() {
                (icon::PASS, c.accent, 12.0)
            } else {
                ("", c.text, 8.0)
            };
            if over_close {
                p.rect_filled(close_rect, 4.0 * s, c.hover);
            }
            p.text(
                close_rect.center(),
                Align2::CENTER_CENTER,
                glyph,
                pt.font(size, Weight::Regular),
                color,
            );
            x += w + 4.0 * s;
        }
        // Its tab keys, between the tabs and the buttons.
        if !hints.is_empty() {
            let room = Rect::from_min_max(
                pos2(x + 16.0 * s, head.top()),
                pos2(tabs_right - 12.0 * s, head.bottom()),
            );
            pt.hints_in(ui, &p, room, &hints, 11.5);
        }

        // Body.
        let (body, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let mut clicked = None;
        let mut tile_pick = None;
        if self.terminal_grid && self.terminals.len() > 1 {
            let tiles =
                crate::terminal::grid_rects(body.shrink(6.0 * s), self.terminals.len(), 6.0 * s);
            for (i, tile) in tiles.into_iter().enumerate() {
                let active = i == self.active_terminal;
                let border = if active && focused {
                    c.accent.gamma_multiply(0.7)
                } else {
                    c.border
                };
                p.rect(
                    tile,
                    8.0 * s,
                    c.background,
                    Stroke::new(1.0, border),
                    StrokeKind::Inside,
                );
                let title = Rect::from_min_size(tile.min, vec2(tile.width(), 24.0 * s));
                p.hline(
                    title.x_range().shrink(1.0),
                    title.bottom(),
                    Stroke::new(1.0, c.border),
                );
                let title_resp =
                    ui.interact(title, ui.scope_id().with(("term-title", i)), Sense::click());
                if title_resp.clicked() || title_resp.secondary_clicked() {
                    clicked = Some(i);
                }
                title_resp.context_menu(|ui| {
                    tile_pick =
                        tile_pick.or(command_menu(ui, &self.terminal_menu(i), &|c| self.shortcut(c)));
                });
                let label = format!("{}  {}", i + 1, labels[i]);
                p.with_clip_rect(title.shrink(4.0)).text(
                    pos2(title.left() + 10.0 * s, title.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    pt.font(12.0, Weight::Medium),
                    if active { c.text } else { c.text_muted },
                );
                let inner = Rect::from_min_max(
                    pos2(tile.left() + 2.0, title.bottom() + 1.0),
                    tile.max - vec2(2.0, 2.0),
                );
                if self.draw_terminal(ui, i, inner, focused && active) {
                    clicked = Some(i);
                }
            }
        } else {
            let i = self.active_terminal;
            if self.draw_terminal(ui, i, body, focused) {
                clicked = Some(i);
            }
        }
        for (i, t) in self.terminals.iter_mut().enumerate() {
            if self.terminal_grid || i == self.active_terminal {
                t.pane.mark_viewed();
            }
        }

        let ctx = ui.ctx().clone();
        if let Some(i) = clicked.or(select) {
            self.active_terminal = i;
            self.focus = Focus::Terminal;
        }
        if let Some(i) = close {
            self.close_terminal(i);
        }
        if let Some(command) = run {
            self.run(command, &ctx);
        }
        if let Some(command) = menu_pick.or(tile_pick) {
            self.focus = Focus::Terminal; // the menu acts on the active terminal
            self.run(command, &ctx);
        }
    }

    /// One terminal's screen in `rect`: fits the PTY to it, takes input when
    /// `keyboard`, handles the mouse, and draws it. Returns whether it was
    /// clicked (so the caller can focus it).
    fn draw_terminal(&mut self, ui: &mut egui::Ui, i: usize, rect: Rect, keyboard: bool) -> bool {
        let c = self.config.theme.colors.clone();
        let size = self.config.settings.code_font_size;
        let font = FontId::monospace(size);
        let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let row_h = (size * 1.4).round();
        let s = self.config.settings.ui_font_size / 14.0;
        self.terminal_rects.push((i, rect));
        // For the hint on a link: the key that lists them all.
        let all_links = self
            .shortcut(Command::OpenTerminalLink)
            .map(|k| format!("\n{k} lists every link here"))
            .unwrap_or_default();
        let Some(term) = self.terminals.get_mut(i) else {
            return false;
        };
        let p = ui.painter().clone();

        let resp = ui.interact(
            rect,
            ui.scope_id().with(("terminal-body", i)),
            Sense::click_and_drag(),
        );
        // ⌘ held over a link: underline it; ⌘+click offers to open it.
        let link_mod = ui.input(|inp| inp.modifiers.command || inp.modifiers.ctrl);
        let inner = rect.shrink2(vec2(10.0 * s, 6.0 * s));
        let rows = ((inner.height() / row_h).floor() as u16).max(1);
        let cols = ((inner.width() / char_w).floor() as u16).max(1);
        term.pane.resize(rows, cols);
        if keyboard {
            let events = ui.input(|inp| inp.events.clone());
            term.input(&events);
        }
        term.pane.pump();

        let cell_at = |pos: Pos2| {
            let r = ((pos.y - inner.top()) / row_h)
                .floor()
                .clamp(0.0, (rows - 1) as f32) as u16;
            let col = ((pos.x - inner.left()) / char_w)
                .floor()
                .clamp(0.0, (cols - 1) as f32) as u16;
            (r, col)
        };
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
            let (dy, shift, ptr) = ui.input(|inp| {
                (
                    inp.smooth_scroll_delta.y,
                    inp.modifiers.shift,
                    inp.pointer.hover_pos(),
                )
            });
            if dy != 0.0
                && let Some(ptr) = ptr
            {
                term.wheel(dy / row_h, cell_at(ptr), shift);
            }
        }
        let (pressed, released, shift, ptr) = ui.input(|inp| {
            (
                inp.pointer.primary_pressed(),
                inp.pointer.primary_released(),
                inp.modifiers.shift,
                inp.pointer.interact_pos(),
            )
        });
        let mut clicked = false;
        let mut link = None;
        let mut hover_link = None;
        let mut plain_link = None; // over a link without ⌘: say how to open it
        if let Some(ptr) = ptr {
            let (r, col) = cell_at(ptr);
            let pane = &mut term.pane;
            if resp.hovered() && pane.url_at(r, col).is_some() {
                if link_mod {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    hover_link = Some((r, col));
                } else {
                    plain_link = Some((r, col));
                }
            }
            if pressed && resp.hovered() && hover_link.is_some() {
                clicked = true;
                link = pane.url_at(r, col);
            } else if pressed && resp.hovered() {
                clicked = true;
                term.mouse_forward = pane.wants_mouse() && !shift;
                if term.mouse_forward {
                    pane.send_mouse(0, col, r, false);
                } else if shift {
                    pane.extend_selection(r, col);
                } else {
                    pane.begin_selection(r, col);
                }
            } else if resp.dragged_by(egui::PointerButton::Primary) {
                if term.mouse_forward {
                    if pane.wants_mouse_motion() {
                        pane.send_mouse(32, col, r, false);
                    }
                } else {
                    pane.update_selection(r, col);
                }
            }
            if released && term.mouse_forward {
                pane.send_mouse(0, col, r, true);
                term.mouse_forward = false;
            }
            if !term.mouse_forward {
                if resp.triple_clicked() {
                    pane.select_line_cell(r);
                } else if resp.double_clicked() {
                    pane.select_word_cell(r, col);
                }
            }
        }

        let pane = &term.pane;
        let screen = pane.screen();
        let color = |col: vt100::Color, fg: bool, bold: bool| -> Option<Color32> {
            match col {
                vt100::Color::Default => fg.then_some(c.text),
                // Bold with a basic colour brightens it, as xterm does.
                vt100::Color::Idx(i) if i < 8 && bold && fg => Some(c.ansi(i + 8)),
                vt100::Color::Idx(i) if i < 16 => Some(c.ansi(i)),
                vt100::Color::Idx(i) => Some(xterm_256(i)),
                vt100::Color::Rgb(r, g, b) => Some(Color32::from_rgb(r, g, b)),
            }
        };
        let sel_bg = c.accent.gamma_multiply(0.35);
        // Per row: resolve each cell's style, paint merged background runs,
        // then lay out merged text runs. Runs of same-styled ASCII share one
        // layout (a busy screen is thousands of cells); anything else gets its
        // own, placed at its cell so wide and fallback glyphs stay aligned.
        struct Cell {
            col: u16,
            text: String,
            fg: Color32,
            bg: Option<Color32>,
            italic: bool,
            underline: bool,
            wide: bool,
        }
        let mut cells: Vec<Cell> = Vec::with_capacity(cols as usize);
        for r in 0..rows {
            let y = inner.top() + r as f32 * row_h;
            cells.clear();
            for col in 0..cols {
                let Some(cell) = screen.cell(r, col) else {
                    break;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let (mut fg, mut bg) = (
                    color(cell.fgcolor(), true, cell.bold()).unwrap_or(c.text),
                    color(cell.bgcolor(), false, false),
                );
                if cell.inverse() {
                    let old_fg = fg;
                    fg = bg.unwrap_or(c.background);
                    bg = Some(old_fg);
                }
                if cell.dim() {
                    fg = fg.gamma_multiply(0.6);
                }
                if pane.is_selected(r, col) {
                    bg = Some(sel_bg);
                }
                let text = cell.contents();
                cells.push(Cell {
                    col,
                    text: if text.is_empty() {
                        " ".to_string()
                    } else {
                        text.to_string()
                    },
                    fg,
                    bg,
                    italic: cell.italic(),
                    underline: cell.underline(),
                    wide: cell.is_wide(),
                });
            }
            let x_of = |col: u16| inner.left() + col as f32 * char_w;
            // Backgrounds.
            let mut i = 0;
            while i < cells.len() {
                let mut j = i + 1;
                while j < cells.len() && cells[j].bg == cells[i].bg {
                    j += 1;
                }
                if let Some(bg) = cells[i].bg {
                    let end = cells[j - 1].col + if cells[j - 1].wide { 2 } else { 1 };
                    p.rect_filled(
                        Rect::from_x_y_ranges(x_of(cells[i].col)..=x_of(end), y..=y + row_h),
                        0.0,
                        bg,
                    );
                }
                i = j;
            }
            // Text.
            let mut i = 0;
            while i < cells.len() {
                let first = &cells[i];
                let simple = |k: &Cell| k.text.is_ascii() && !k.wide;
                let mut j = i + 1;
                if simple(first) {
                    while j < cells.len()
                        && simple(&cells[j])
                        && (cells[j].fg, cells[j].italic, cells[j].underline)
                            == (first.fg, first.italic, first.underline)
                    {
                        j += 1;
                    }
                }
                let text: String = cells[i..j].iter().map(|k| k.text.as_str()).collect();
                if !text.trim().is_empty() || first.underline {
                    let mut fmt = egui::TextFormat::simple(font.clone(), first.fg);
                    fmt.italics = first.italic;
                    if first.underline {
                        fmt.underline = Stroke::new(1.0, first.fg);
                    }
                    let g = ui.fonts_mut(|f| f.layout_job(LayoutJob::single_section(text, fmt)));
                    p.galley(
                        pos2(x_of(first.col), y + (row_h - g.size().y) / 2.0),
                        g,
                        first.fg,
                    );
                }
                i = j;
            }
        }
        if !screen.hide_cursor() && pane.scroll_offset() == 0 && !pane.copy_mode() {
            let (cr, cc) = screen.cursor_position();
            let r = Rect::from_min_size(
                pos2(
                    inner.left() + cc as f32 * char_w,
                    inner.top() + cr as f32 * row_h,
                ),
                vec2(char_w, row_h),
            );
            if keyboard {
                p.rect_filled(r, 1.0, c.accent.gamma_multiply(0.85));
                let ch = screen
                    .cell(cr, cc)
                    .map(|cell| cell.contents().to_string())
                    .unwrap_or_default();
                if !ch.is_empty() {
                    p.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        ch,
                        font.clone(),
                        c.background,
                    );
                }
            } else {
                p.rect_stroke(r, 1.0, Stroke::new(1.0, c.text_subtle), StrokeKind::Inside);
            }
        }
        if let Some((cr, cc)) = pane.copy_cursor() {
            let r = Rect::from_min_size(
                pos2(
                    inner.left() + cc as f32 * char_w,
                    inner.top() + cr as f32 * row_h,
                ),
                vec2(char_w, row_h),
            );
            p.rect_stroke(r, 1.0, Stroke::new(1.5, c.accent), StrokeKind::Inside);
        }
        // A link: faint underline, then (once the pointer rests) how to open
        // it; with ⌘ held, a bold one.
        let link_mod_key = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" };
        for ((r, col), held) in [(hover_link, true), (plain_link, false)]
            .into_iter()
            .filter_map(|(at, held)| Some((at?, held)))
        {
            let Some(span) = crate::terminalpane::url_cols_in_row(&screen, r)
                .into_iter()
                .find(|s| s.contains(&col))
            else {
                continue;
            };
            let y = inner.top() + (r + 1) as f32 * row_h - 2.0;
            let xs =
                inner.left() + span.start as f32 * char_w..=inner.left() + span.end as f32 * char_w;
            if held {
                p.hline(xs, y, Stroke::new(1.5, c.accent));
            } else {
                p.hline(xs, y, Stroke::new(1.0, c.accent.gamma_multiply(0.45)));
                if ui.input(|i| i.pointer.time_since_last_movement()) > 0.4 {
                    egui::Tooltip::always_open(
                        ui.ctx().clone(),
                        ui.layer_id(),
                        ui.scope_id().with("link-hint"),
                        egui::PopupAnchor::Pointer,
                    )
                    .show(|ui| ui.label(format!("{link_mod_key}-click to open this link{all_links}")));
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(450));
                }
            }
        }
        drop(screen);
        if let Some(url) = link {
            self.offer_url(url);
        }
        clicked
    }

    /// Keys that only mean something in one kind of list. Returns true when
    /// the palette closed.
    fn palette_mode_keys(&mut self, ctx: &egui::Context, mode: Mode) -> bool {
        // Keyboard Shortcuts waiting for a key: the next press is it.
        if self.capturing() {
            let press = ctx.input_mut(|i| {
                let press = i.events.iter().find_map(|e| match e {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some((*key, *physical_key, *modifiers)),
                    _ => None,
                });
                i.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_)
                    )
                });
                press
            });
            if let Some((key, physical, m)) = press
                && let Some(command) = self.palette.as_mut().and_then(|p| p.capture.take())
                && key != Key::Escape
            {
                self.record_binding(
                    command,
                    crate::keymap::Combo::from_press(key, physical, m),
                    ctx,
                );
            }
            return false;
        }
        let empty = self.palette.as_ref().is_some_and(|p| p.query.is_empty());
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        match mode {
            Mode::Todos => {
                // A pasted list: one task per line.
                let list = ctx.input_mut(|i| {
                    let at = i.events.iter().position(
                        |e| matches!(e, egui::Event::Paste(t) if t.trim().contains('\n')),
                    )?;
                    match i.events.remove(at) {
                        egui::Event::Paste(t) => Some(t),
                        _ => None,
                    }
                });
                if let Some(list) = list {
                    self.todo_add(&list);
                }
                let (clear, delete, edit) = ctx.input_mut(|i| {
                    (
                        i.consume_key(cmd_shift, Key::Backspace),
                        empty && i.consume_key(Modifiers::COMMAND, Key::Backspace),
                        i.consume_key(Modifiers::COMMAND, Key::E),
                    )
                });
                if clear || delete {
                    self.todo_remove(clear);
                }
                if edit {
                    self.todo_edit_file(ctx);
                    return true;
                }
            }
            Mode::Clipboard => {
                let (tick, clear) = ctx.input_mut(|i| {
                    (
                        i.consume_key(Modifiers::NONE, Key::Tab),
                        empty && i.consume_key(Modifiers::COMMAND, Key::Backspace),
                    )
                });
                if tick {
                    self.clip_tick();
                }
                if clear {
                    self.clip_clear();
                }
            }
            Mode::Settings => {
                let (left, right) = ctx.input_mut(|i| {
                    (
                        i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                        i.consume_key(Modifiers::NONE, Key::ArrowRight),
                    )
                });
                if let (true, Some(Pick::Setting(set))) = (
                    left || right,
                    self.palette
                        .as_ref()
                        .and_then(|p| p.selected_pick().cloned()),
                ) {
                    self.change_setting(set, if right { 1 } else { -1 }, ctx);
                }
            }
            Mode::Keybindings
                if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Backspace)) =>
            {
                self.reset_binding(ctx)
            }
            _ => {}
        }
        false
    }

    /// Notifications, stacked in the bottom-right corner; they fade out.
    fn toasts_ui(&self, ctx: &egui::Context) {
        // The F1 sheet reaches the corner; toasts would sit on top of it.
        if self.toasts.is_empty() || self.help {
            return;
        }
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let screen = ctx.content_rect();
        let p = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("toasts"),
        ));
        let mut bottom = screen.bottom() - 26.0 * s - 12.0 * s;
        let mut next_change = std::time::Duration::from_secs(60);
        for t in self.toasts.iter().rev() {
            let age = t.born.elapsed();
            let left = t.ttl().saturating_sub(age);
            let fade_in = (age.as_secs_f32() / 0.15).min(1.0);
            let fade_out = (left.as_secs_f32() / 0.4).min(1.0);
            let alpha = fade_in.min(fade_out);
            next_change = next_change.min(if alpha < 1.0 {
                std::time::Duration::ZERO
            } else {
                left.saturating_sub(std::time::Duration::from_millis(400))
            });

            let (glyph, tint) = if t.error {
                (icon::ERROR, c.error)
            } else {
                (icon::CHECK, c.accent)
            };
            let text = p.layout(
                t.msg.clone(),
                pt.font(13.0, Weight::Regular),
                c.text.gamma_multiply(alpha),
                360.0 * s,
            );
            let pad = vec2(14.0 * s, 10.0 * s);
            let size = vec2(text.size().x + 26.0 * s, text.size().y.max(18.0 * s)) + pad * 2.0;
            let rise = (1.0 - fade_in) * 8.0 * s;
            let rect = Rect::from_min_size(
                pos2(screen.right() - 16.0 * s - size.x, bottom - size.y + rise),
                size,
            );
            p.add(
                egui::Shadow {
                    offset: [0, 6],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha((60.0 * alpha) as u8),
                }
                .as_shape(rect, 10.0 * s),
            );
            p.rect(
                rect,
                10.0 * s,
                c.elevated.gamma_multiply(alpha),
                Stroke::new(
                    1.0,
                    if t.error { c.error } else { c.border }.gamma_multiply(alpha),
                ),
                StrokeKind::Inside,
            );
            p.text(
                pos2(rect.left() + pad.x, rect.top() + pad.y + 9.0 * s),
                Align2::LEFT_CENTER,
                glyph,
                pt.font(14.0, Weight::Regular),
                tint.gamma_multiply(alpha),
            );
            p.galley(
                pos2(rect.left() + pad.x + 26.0 * s, rect.top() + pad.y),
                text,
                c.text,
            );
            bottom = rect.top() - 8.0 * s;
        }
        ctx.request_repaint_after(next_change.max(std::time::Duration::from_millis(16)));
    }

    /// F1: every shortcut, with the ones for where you are first.
    fn help_ui(&mut self, ctx: &egui::Context) {
        let close_combo = self.config.keymap.combo_for(Command::Help);
        let close = ctx.input_mut(|i| {
            let hit = i.events.iter().any(|e| match e {
                egui::Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                } => true,
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    modifiers,
                    ..
                } => close_combo.is_some_and(|k| {
                    crate::keymap::Combo::from_press(*key, *physical_key, *modifiers) == k
                }),
                _ => false,
            });
            let enter = i.key_pressed(Key::Enter);
            i.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_)
                )
            });
            (hit, enter)
        });
        let (close, customize_key) = close;
        if customize_key {
            self.help = false;
            return self.open_palette(Mode::Keybindings, ctx);
        }
        if close {
            self.help = false;
            return;
        }
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let screen = ctx.content_rect();
        let (here, extra, groups) = self.help_context();
        let sections = help_sections(&self.config.keymap);
        let keys = |cmd: Command| {
            self.config
                .keymap
                .combo_for(cmd)
                .map(|k| k.caps())
                .unwrap_or_default()
        };

        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::unique("help-backdrop"),
        ))
        .rect_filled(
            screen,
            0.0,
            Color32::from_black_alpha(if self.config.theme.dark { 110 } else { 50 }),
        );
        // At most 70% of the window either way; the list scrolls.
        let width = (880.0 * s).min(screen.width() * 0.7 - 2.0);
        let height = screen.height() * 0.7 - 2.0 - 44.0 * s;
        let pos = pos2(
            screen.center().x - width / 2.0,
            screen.top() + screen.height() * 0.15,
        );
        let mut customize = false;
        egui::Area::new(egui::Id::unique("help"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(c.elevated)
                    .stroke(Stroke::new(1.0, c.border))
                    .corner_radius(14.0 * s)
                    .inner_margin(egui::Margin::same((22.0 * s) as i8))
                    .shadow(egui::Shadow {
                        offset: [0, 16],
                        blur: 48,
                        spread: 0,
                        color: Color32::from_black_alpha(90),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 44.0 * s);
                        ui.set_max_height(height);
                        ui.spacing_mut().item_spacing = vec2(8.0 * s, 4.0 * s);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(icon::KEYBOARD)
                                    .font(pt.font(18.0, Weight::Regular))
                                    .color(c.accent),
                            );
                            ui.label(
                                egui::RichText::new("Keyboard Shortcuts")
                                    .font(pt.font(17.0, Weight::SemiBold))
                                    .color(c.text),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("esc")
                                            .font(pt.font(12.0, Weight::Regular))
                                            .color(c.text_subtle),
                                    );
                                    let b = egui::Button::new(
                                        egui::RichText::new("Change shortcuts…  ↵")
                                            .font(pt.font(12.5, Weight::Medium))
                                            .color(c.text),
                                    )
                                    .fill(c.surface)
                                    .stroke(Stroke::new(1.0, c.border))
                                    .corner_radius(7.0 * s);
                                    customize = ui.add(b).clicked();
                                },
                            );
                        });
                        ui.add_space(10.0 * s);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                // Where you are: its own keys first, on a tinted card.
                                egui::Frame::NONE
                                    .fill(c.accent.gamma_multiply(0.07))
                                    .stroke(Stroke::new(1.0, c.accent.gamma_multiply(0.35)))
                                    .corner_radius(10.0 * s)
                                    .inner_margin(egui::Margin::same((14.0 * s) as i8))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "HERE · {}",
                                                here.to_uppercase()
                                            ))
                                            .font(pt.font(11.0, Weight::SemiBold))
                                            .color(c.accent),
                                        );
                                        ui.add_space(4.0 * s);
                                        let mut rows: Vec<(String, Vec<String>)> = extra
                                            .iter()
                                            .map(|(k, what)| {
                                                (what.to_string(), vec![k.to_string()])
                                            })
                                            .collect();
                                        for (title, list) in &sections {
                                            if groups.contains(title) {
                                                rows.extend(
                                                    list.iter().map(|(cmd, t)| {
                                                        (t.to_string(), keys(*cmd))
                                                    }),
                                                );
                                            }
                                        }
                                        help_rows(ui, &pt, &rows);
                                    });
                                ui.add_space(14.0 * s);
                                let (left, right): (Vec<_>, Vec<_>) =
                                    sections.iter().enumerate().partition(|(i, _)| i % 2 == 0);
                                ui.columns(2, |cols| {
                                    for (col, half) in cols.iter_mut().zip([left, right]) {
                                        for (_, (title, list)) in half {
                                            col.label(
                                                egui::RichText::new(title.to_uppercase())
                                                    .font(pt.font(11.0, Weight::SemiBold))
                                                    .color(c.text_subtle),
                                            );
                                            let rows: Vec<(String, Vec<String>)> = list
                                                .iter()
                                                .map(|(cmd, t)| (t.to_string(), keys(*cmd)))
                                                .collect();
                                            help_rows(col, &pt, &rows);
                                            col.add_space(14.0 * s);
                                        }
                                    }
                                });
                            });
                    });
            });
        if customize {
            self.help = false;
            self.open_palette(Mode::Keybindings, ctx);
        }
    }

    /// The F1 sheet's "here" card: a name for where the keyboard is, the keys
    /// that only work there, and which command sections belong with it.
    fn help_context(
        &self,
    ) -> (
        &'static str,
        Vec<(&'static str, &'static str)>,
        Vec<&'static str>,
    ) {
        let mode = self.palette.as_ref().map(|p| p.mode);
        match mode {
            Some(Mode::Files) => (
                "Go to File",
                vec![("↑ ↓ ← →", "Move, fold folders"), ("↵", "Open")],
                vec!["Files"],
            ),
            Some(Mode::Todos) => (
                "To-do list",
                vec![
                    ("↵", "Add what you typed, or tick the selected task"),
                    ("⌘⌫", "Delete the selected task"),
                    ("⇧⌘⌫", "Clear finished tasks"),
                    ("⌘E", "Edit the list as a text file"),
                    ("⌘V", "Paste a list — one task per line"),
                ],
                vec![],
            ),
            Some(Mode::Clipboard) => (
                "Clipboard history",
                vec![
                    ("↵", "Paste where you were"),
                    ("⇥", "Tick entries to paste several"),
                    ("⌘⌫", "Clear the history"),
                ],
                vec![],
            ),
            Some(Mode::Keybindings) => (
                "Keyboard Shortcuts",
                vec![
                    ("↵", "Record a new shortcut"),
                    ("⌘⌫", "Back to the default"),
                ],
                vec![],
            ),
            Some(Mode::Settings) => (
                "Settings",
                vec![
                    ("← →", "Change the value"),
                    ("↵", "Next value, or open the picker"),
                ],
                vec![],
            ),
            Some(_) => (
                "Palette",
                vec![("↑ ↓", "Move"), ("↵", "Choose"), ("esc", "Close")],
                vec![],
            ),
            None if self.terminal_focused() => (
                "Terminal",
                vec![
                    ("⌘-click", "Open a link (asks first)"),
                    ("⇧-drag", "Select, even when a program uses the mouse"),
                    ("⌘C  ⌘V", "Copy the selection, paste"),
                    ("⌃C  ⌃R …", "Go straight to the shell"),
                    ("drop a file", "Types its path"),
                ],
                vec!["Terminals"],
            ),
            None if self.focus == Focus::Sidebar && self.sidebar_shown() => (
                "Sidebar",
                vec![
                    ("↑ ↓", "Move"),
                    ("← →", "Fold / unfold a folder"),
                    ("↵", "Open the file and go to it"),
                    ("space", "Open the file, stay in the tree"),
                    ("right-click", "New, rename, reveal, copy path, trash"),
                    ("esc", "Back to the editor"),
                ],
                vec!["Files"],
            ),
            None if self.focus == Focus::Find => (
                "Find bar",
                vec![
                    ("↵  ⇧↵", "Next / previous match"),
                    ("↵  ⇧↵", "In Replace: replace one / all"),
                    ("⇥", "Switch fields"),
                    ("esc", "Close"),
                ],
                vec![],
            ),
            None if self.active.is_some() => (
                "Editor",
                vec![
                    ("⇧ + arrows", "Select"),
                    ("⌥ ← →", "Move by word"),
                    ("⌘ ← →", "Start / end of line"),
                    ("⇥  ⇧⇥", "Indent / outdent"),
                    ("double / triple click", "Select word / line"),
                    ("esc", "Back to one cursor"),
                ],
                vec!["Editing"],
            ),
            None => (
                "Welcome",
                vec![("↑ ↓  ↵", "Pick an action")],
                vec!["General"],
            ),
        }
    }

    /// The palette overlay: dimmed backdrop, search box, results, key hints.
    fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(mode) = self.palette.as_ref().map(|p| p.mode) else {
            return;
        };
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let screen = ctx.content_rect();
        let indexing = self.indexing();
        let searching = self.searching();
        let search_summary = self.search_summary();
        let build = self.project.as_ref().is_some_and(|p| p.build);
        let tree = mode == Mode::Files && self.palette.as_ref().is_some_and(|p| p.query.is_empty());
        let footer = self.palette_footer(mode, tree, build);
        let no_match_hint = match self.config.keymap.combo_for(Command::ToggleBuildFolders) {
            Some(k) => format!(
                "No matches — {} to include node_modules, target…",
                k.caps().concat()
            ),
            None => "No matches".to_string(),
        };

        if self.palette_mode_keys(ctx, mode) {
            return;
        }
        let todo_summary = self.todo_summary();
        let conflicts = if mode == Mode::Keybindings {
            crate::keymap::COMMANDS
                .iter()
                .filter(|c| !self.config.keymap.conflicts(c.command).is_empty())
                .count()
        } else {
            0
        };
        let capture_title = self
            .palette
            .as_ref()
            .and_then(|p| p.capture)
            .and_then(|c| crate::keymap::COMMANDS.iter().find(|i| i.command == c))
            .map(|i| i.title);

        // Navigation keys, taken before the search box can see them.
        let (up, down, enter, esc, left, right) = ctx.input_mut(|i| {
            let mut k = |key| i.consume_key(Modifiers::NONE, key);
            (
                k(Key::ArrowUp),
                k(Key::ArrowDown),
                k(Key::Enter),
                k(Key::Escape),
                tree && k(Key::ArrowLeft),
                tree && k(Key::ArrowRight),
            )
        });
        if esc {
            return self.close_palette(false, ctx);
        }
        if up || down {
            if let Some(p) = &mut self.palette {
                p.step(if up { -1 } else { 1 });
            }
            self.palette_moved(ctx);
        }
        if (left || right)
            && let Some(sel) = self.palette.as_ref().map(|p| p.selected)
        {
            self.palette_tree(sel, Some(right));
        }
        if enter {
            return self.palette_accept(ctx);
        }

        let backdrop =
            egui::LayerId::new(egui::Order::Middle, egui::Id::unique("palette-backdrop"));
        let dim = Color32::from_black_alpha(if self.config.theme.dark { 90 } else { 40 });
        ctx.layer_painter(backdrop).rect_filled(screen, 0.0, dim);

        // Lists you browse get most of the window (70% each way); short
        // pickers stay compact, so a theme or font previews around them.
        let big = !matches!(
            mode,
            Mode::Themes
                | Mode::UiFonts
                | Mode::CodeFonts
                | Mode::Line
                | Mode::Terminals
                | Mode::Links
        );
        let row_h = 36.0 * s;
        let (bar_h, foot_h) = (48.0 * s, 36.0 * s);
        // Never more than 70% of the window either way; the list scrolls.
        let max_h = screen.height() * 0.7 - 2.0; // the border is outside
        let (width, top) = if big {
            (
                screen.width() * 0.7 - 2.0,
                screen.top() + (screen.height() - max_h) / 2.0,
            )
        } else {
            (
                (620.0 * s).min(screen.width() * 0.7 - 2.0),
                screen.top() + (64.0 * s).min(screen.height() * 0.15),
            )
        };
        let pos = pos2(screen.center().x - width / 2.0, top);
        let (mut moved, mut accept, mut query_changed, mut toggle_row) =
            (false, false, false, None);

        let area = egui::Area::new(egui::Id::unique("palette"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos);
        let inner = area.show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(c.elevated)
                .stroke(Stroke::new(1.0, c.border))
                .corner_radius(12.0 * s)
                .shadow(egui::Shadow {
                    offset: [0, 12],
                    blur: 40,
                    spread: 0,
                    color: Color32::from_black_alpha(70),
                });
            frame.show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // Footer layout first: its height sets how many rows fit.
                // The keys, in a grid spread over the full width: as many
                // columns as fit, each as wide as its widest hint, the
                // spare room shared out between them.
                let hints: Vec<Hint> = footer.into_iter().filter(|h| h.1 != "│").collect();
                let room = width - 36.0 * s;
                let widths: Vec<f32> = hints
                    .iter()
                    .map(|h| pt.key_hints(ui, None, std::slice::from_ref(h), 12.0, Pos2::ZERO))
                    .collect();
                let col_widths = |cols: usize| -> Vec<f32> {
                    (0..cols)
                        .map(|c| {
                            widths
                                .iter()
                                .skip(c)
                                .step_by(cols)
                                .fold(0.0, |a: f32, w| a.max(*w))
                        })
                        .collect()
                };
                let min_gap = 24.0 * s;
                let cols = (1..=hints.len())
                    .rev()
                    .find(|&n| col_widths(n).iter().sum::<f32>() + min_gap * (n - 1) as f32 <= room)
                    .unwrap_or(1);
                // Balanced rows: 6 hints that fit 5 across go 3 and 3, not 5 and 1.
                let lines = hints.len().div_ceil(cols).max(1);
                let cols = hints.len().div_ceil(lines).max(1);
                let col_w = col_widths(cols);
                let used: f32 = col_w.iter().sum();
                let gap = if cols > 1 {
                    ((room - used) / (cols - 1) as f32).min(96.0 * s)
                } else {
                    0.0
                };
                let x0 = 18.0 * s + (room - used - gap * (cols - 1) as f32) / 2.0;
                let line_h = 28.0 * s;
                let foot_total = foot_h + line_h * (lines - 1) as f32;
                let fit = ((max_h - bar_h - foot_total - 12.0 * s) / row_h).floor().max(1.0) as usize;
                let rows = if big { fit } else { fit.min(PALETTE_ROWS) };
                let Some(pal) = &mut self.palette else { return };

                // Search box.
                let (bar, _) = ui.allocate_exact_size(vec2(width, bar_h), Sense::hover());
                let icon_pos = pos2(bar.left() + 18.0 * s, bar.center().y);
                ui.painter().text(
                    icon_pos,
                    Align2::LEFT_CENTER,
                    icon::SEARCH,
                    pt.font(16.0, Weight::Regular),
                    c.text_subtle,
                );
                // How many there are, at the search box's end.
                let noun = if tree { "items" } else { "results" };
                let count = if mode == Mode::Search && !pal.query.is_empty() && !searching {
                    search_summary.clone()
                } else if pal.items.len() > rows {
                    format!("{} {noun}", pal.items.len())
                } else {
                    String::new()
                };
                let count = match mode {
                    Mode::Files | Mode::Search if build => {
                        format!("{count}  + build folders").trim().to_string()
                    }
                    Mode::Todos => todo_summary.clone(),
                    Mode::Clipboard if !pal.ticked.is_empty() => {
                        format!("{} ticked", pal.ticked.len())
                    }
                    Mode::Settings => "saved to settings.json".into(),
                    Mode::Keybindings if conflicts > 0 => format!("{conflicts} clashing"),
                    _ => count,
                };
                let count_r = ui.painter().text(
                    pos2(bar.right() - 18.0 * s, bar.center().y),
                    Align2::RIGHT_CENTER,
                    count,
                    pt.font(12.5, Weight::Regular),
                    c.text_subtle,
                );
                let edit_rect = Rect::from_min_max(
                    pos2(bar.left() + 46.0 * s, bar.top()),
                    pos2(count_r.left() - 16.0 * s, bar.bottom()),
                );
                let hint = egui::RichText::new(mode.placeholder())
                    .color(c.text_subtle)
                    .font(pt.font(15.0, Weight::Regular));
                let edit = egui::TextEdit::singleline(&mut pal.query)
                    .frame(egui::Frame::NONE)
                    .font(pt.font(15.0, Weight::Regular))
                    .text_color(c.text)
                    .hint_text(hint)
                    .align(Align2::LEFT_CENTER)
                    .desired_width(edit_rect.width());
                if let Some(title) = capture_title {
                    // Recording a shortcut: the box becomes the prompt.
                    let msg = format!("Press the new shortcut for {title}…");
                    ui.painter().text(
                        edit_rect.left_center(),
                        Align2::LEFT_CENTER,
                        msg,
                        pt.font(15.0, Weight::Medium),
                        c.accent,
                    );
                } else {
                    let resp = ui.put(edit_rect, edit);
                    resp.request_focus();
                    if resp.changed() {
                        // To-do's box is for new tasks, not a filter: keep the selection.
                        if mode != Mode::Todos {
                            pal.selected = 0;
                            pal.scroll = 0;
                        }
                        query_changed = true;
                    }
                }
                ui.painter().hline(
                    bar.x_range(),
                    bar.bottom() - 0.5,
                    Stroke::new(1.0, c.border),
                );

                // Results.
                pal.follow(rows);
                let shown = pal.items.len().saturating_sub(pal.scroll).min(rows);
                // A big list keeps its size as results come and go.
                let list_h = if big { rows } else { shown.max(1) } as f32 * row_h;
                let (list, list_resp) =
                    ui.allocate_exact_size(vec2(width, list_h + 12.0 * s), Sense::hover());
                let list = list.shrink(6.0 * s);
                if list_resp.hovered() {
                    let dy = ui.input(|i| i.smooth_scroll_delta.y);
                    let max = pal.items.len().saturating_sub(rows) as f32;
                    if dy.abs() > 0.5 {
                        pal.scroll =
                            (pal.scroll as f32 - (dy / row_h).round()).clamp(0.0, max) as usize;
                    }
                }
                if pal.items.is_empty() {
                    let msg = match mode {
                        Mode::Todos => "Nothing to do — type a task and press ↵",
                        Mode::Clipboard if pal.query.is_empty() => {
                            "Nothing copied yet — what you copy here shows up in this list"
                        }
                        Mode::Files if indexing => "Indexing files…",
                        Mode::Files | Mode::Search
                            if !build && !pal.query.is_empty() && !searching =>
                        {
                            no_match_hint.as_str()
                        }
                        Mode::Search if pal.query.is_empty() => {
                            "Type to search every file in the project"
                        }
                        Mode::Search if searching => "Searching…",
                        _ => "No matches",
                    };
                    ui.painter().text(
                        list.center(),
                        Align2::CENTER_CENTER,
                        msg,
                        pt.font(13.0, Weight::Regular),
                        c.text_subtle,
                    );
                }
                let row_rect = |n: usize| {
                    Rect::from_min_size(
                        pos2(list.left(), list.top() + n as f32 * row_h),
                        vec2(list.width(), row_h),
                    )
                };
                let pointer_moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
                for (n, idx) in (pal.scroll..pal.scroll + shown).enumerate() {
                    let r = ui.interact(
                        row_rect(n),
                        ui.scope_id().with(("palette-row", idx)),
                        Sense::click(),
                    );
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                        // Only a moving pointer steals the selection from the keyboard.
                        if pointer_moved && pal.selected != idx {
                            pal.selected = idx;
                            moved = true;
                        }
                    }
                    if r.clicked() {
                        pal.selected = idx;
                        if pal.items[idx].expanded.is_some() {
                            toggle_row = Some(idx);
                        } else {
                            accept = true;
                        }
                    }
                }
                let painter = ui.painter().clone();
                for (n, idx) in (pal.scroll..pal.scroll + shown).enumerate() {
                    let item = &pal.items[idx];
                    let t = if idx == pal.selected { 1.0 } else { 0.0 };
                    let label = pt.label(
                        &item.label,
                        &item.matched,
                        if item.dim { c.text_subtle } else { c.text },
                    );
                    let glyph = match item.expanded {
                        Some(true) => (icon::CHEVRON_DOWN, None),
                        Some(false) => (icon::CHEVRON_RIGHT, None),
                        None => (
                            item.icon,
                            item.tint
                                .map(|t| if item.dim { t.gamma_multiply(0.5) } else { t }),
                        ),
                    };
                    let indent = item.depth as f32 * 16.0 * s;
                    let waiting = [String::from("press a key…")];
                    let caps = if capture_title.is_some() && idx == pal.selected {
                        &waiting[..]
                    } else {
                        &item.caps[..]
                    };
                    // The selected file or folder says what ↵ / → / ← do to it.
                    let hints: Vec<Hint> = match (mode, item.expanded) {
                        _ if mode != Mode::Files || idx != pal.selected => Vec::new(),
                        (_, Some(false)) => vec![(vec!["→".into()], "expand")],
                        (_, Some(true)) => vec![(vec!["←".into()], "collapse")],
                        (_, None) => vec![(vec!["↵".into()], "open")],
                    };
                    pt.row(
                        ui,
                        &painter,
                        row_rect(n),
                        t,
                        glyph,
                        label,
                        &item.detail,
                        caps,
                        &hints,
                        indent,
                    );
                }

                // Footer: key hints and the result count.
                let (foot, _) = ui.allocate_exact_size(vec2(width, foot_total), Sense::hover());
                ui.painter()
                    .hline(foot.x_range(), foot.top() + 0.5, Stroke::new(1.0, c.border));
                for (i, h) in hints.iter().enumerate() {
                    let (row, col) = (i / cols, i % cols);
                    let x = foot.left() + x0 + col_w[..col].iter().sum::<f32>() + gap * col as f32;
                    pt.key_hints(
                        ui,
                        Some(ui.painter()),
                        std::slice::from_ref(h),
                        12.0,
                        pos2(x, foot.top() + foot_h / 2.0 + line_h * row as f32),
                    );
                }
            });
        });

        // A click outside closes (and cancels any preview).
        let armed = match &mut self.palette {
            Some(p) => {
                p.armed |=
                    ctx.cumulative_pass_nr() > p.opened && ctx.input(|i| i.pointer.any_pressed());
                p.armed
            }
            None => false,
        };
        let outside = armed
            && ctx.input(|i| i.pointer.any_click())
            && ctx
                .pointer_interact_pos()
                .is_some_and(|p| !inner.response.rect.contains(p));

        if query_changed {
            self.refresh_items();
        }
        if query_changed || moved {
            self.palette_moved(ctx);
        }
        if let Some(i) = toggle_row {
            self.palette_tree(i, None);
        } else if accept {
            self.palette_accept(ctx);
        } else if outside {
            self.close_palette(false, ctx);
        }
    }
}

impl App {
    /// The palette's footer: the keys that work here, with their current
    /// bindings. For files, those on the selected item come first, then a
    /// "│", then the ones that don't need one.
    fn palette_footer(&self, mode: Mode, tree: bool, build: bool) -> Vec<Hint> {
        let k = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let esc = (k("esc"), "close");
        match mode {
            Mode::Files => {
                let key = |c: Command| {
                    self.config
                        .keymap
                        .combo_for(c)
                        .map(|k| vec![k.caps().concat()])
                };
                let mut v = vec![(k("↵"), "open")];
                if tree {
                    v.push((k("→ ←"), "expand / collapse"));
                    if let Some(caps) = key(Command::CollapseFolders) {
                        v.push((caps, "collapse all"));
                    }
                }
                v.extend(
                    [
                        (Command::RenameFile, "rename"),
                        (Command::DeleteFile, "move to trash"),
                        (Command::RevealInFinder, "reveal in Finder"),
                    ]
                    .into_iter()
                    .filter_map(|(c, what)| Some((key(c)?, what))),
                );
                v.push((Vec::new(), "│"));
                v.extend(
                    [
                        (Command::NewFile, "new file"),
                        (Command::NewFolder, "new folder"),
                        (
                            Command::ToggleBuildFolders,
                            if build {
                                "skip build folders"
                            } else {
                                "include build folders"
                            },
                        ),
                    ]
                    .into_iter()
                    .filter_map(|(c, what)| Some((key(c)?, what))),
                );
                v.push(esc);
                v
            }
            Mode::Themes | Mode::UiFonts | Mode::CodeFonts => {
                vec![
                    (k("↑ ↓"), "preview"),
                    (k("↵"), "save"),
                    (k("esc"), "cancel"),
                ]
            }
            Mode::Todos => vec![
                (k("↵"), "add · tick"),
                (k("⌘⌫"), "delete"),
                (k("⇧⌘⌫"), "clear finished"),
                (k("⌘E"), "edit as text"),
                esc,
            ],
            Mode::Clipboard => vec![
                (k("↵"), "paste"),
                (k("⇥"), "tick to paste several"),
                (k("⌘⌫"), "clear history"),
                esc,
            ],
            Mode::Settings => vec![
                (k("← →"), "change"),
                (k("↵"), "next value · open picker"),
                esc,
            ],
            Mode::Keybindings if self.capturing() => {
                vec![
                    (Vec::new(), "Press any key combination"),
                    (k("esc"), "cancel"),
                ]
            }
            Mode::Keybindings => vec![
                (k("↵"), "change shortcut"),
                (k("⌘⌫"), "back to default"),
                esc,
            ],
            Mode::Commands => vec![(k("↑ ↓"), "move"), (k("↵"), "run"), esc],
            Mode::Recent => {
                let mut v = vec![(k("↑ ↓"), "move"), (k("↵"), "open folder")];
                if let Some(key) = self.config.keymap.combo_for(Command::OpenFolder) {
                    v.push((vec![key.caps().concat()], "open another folder…"));
                }
                v.push(esc);
                v
            }
            Mode::Search => vec![(k("↑ ↓"), "move"), (k("↵"), "open at match"), esc],
            Mode::Terminals => vec![(k("↑ ↓"), "move"), (k("↵"), "switch to it"), esc],
            Mode::Links => vec![(k("↑ ↓"), "move"), (k("↵"), "open in browser"), esc],
            Mode::Line => vec![(k("↵"), "go"), esc],
        }
    }
}

/// Commands grouped for the F1 sheet, by their id's prefix.
pub(crate) fn help_sections(
    km: &crate::keymap::Keymap,
) -> Vec<(&'static str, Vec<(Command, &'static str)>)> {
    let section = |id: &str| match id.split('.').next().unwrap_or("") {
        "file" | "folder" => "Files",
        "tab" => "Tabs",
        "edit" => "Editing",
        "search" => "Search",
        "view" => "View",
        "terminal" => "Terminals",
        _ => "General",
    };
    let mut out: Vec<(&'static str, Vec<(Command, &'static str)>)> = [
        "General",
        "Files",
        "Tabs",
        "Editing",
        "Search",
        "View",
        "Terminals",
    ]
    .iter()
    .map(|t| (*t, Vec::new()))
    .collect();
    for c in crate::keymap::COMMANDS {
        // Only what has a key; ⌥1…⌥9 make one row.
        if km.combo_for(c.command).is_none()
            || matches!(c.command, Command::JumpTerminal(n) | Command::GoToTab(n) if n > 1)
        {
            continue;
        }
        let title = match c.command {
            Command::JumpTerminal(_) => "Go to terminal 1…9",
            Command::GoToTab(_) => "Go to tab 1…8, last",
            _ => c.title,
        };
        if let Some((_, list)) = out.iter_mut().find(|(t, _)| *t == section(c.id)) {
            list.push((c.command, title));
        }
    }
    out.retain(|(_, l)| !l.is_empty());
    out
}

/// Label-left, keycaps-right rows for the F1 sheet.
fn help_rows(ui: &mut egui::Ui, pt: &Paint, rows: &[(String, Vec<String>)]) {
    let (c, s) = (&pt.c, pt.s);
    for (label, caps) in rows {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 26.0 * s), Sense::hover());
        let p = ui.painter();
        p.text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            label,
            pt.font(13.0, Weight::Regular),
            c.text_muted,
        );
        // Commands get keycaps; free-form hints ("⌘-click") read better as text.
        if caps.len() == 1 && caps[0].chars().count() > 3 {
            p.text(
                rect.right_center(),
                Align2::RIGHT_CENTER,
                &caps[0],
                pt.font(12.5, Weight::Medium),
                c.text,
            );
        } else {
            pt.keycaps(ui, p, caps, rect.right_center());
        }
    }
}

impl App {
    /// Where the open folder is, and how to change it: the title bar's and
    /// the sidebar header's tooltip.
    pub(crate) fn folder_tip(&self) -> Option<String> {
        let p = self.project.as_ref()?;
        let key = |c: Command| {
            self.config
                .keymap
                .combo_for(c)
                .map(|k| k.caps().concat())
                .unwrap_or_default()
        };
        Some(format!(
            "Folder open in this window:\n{}\n\n{}  recent folders   {}  open another (new window)",
            tilde(&p.root.to_string_lossy()),
            key(Command::RecentFolders),
            key(Command::OpenFolder),
        ))
    }
}

/// `/Users/me/x` → `~/x`.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

/// How many columns of indent guides line `l` gets. A blank line carries on
/// the surrounding block (the shallower of its neighbours), so guides run
/// unbroken through gaps in nested code.
fn guide_cols(b: &crate::buffer::Buffer, l: usize) -> usize {
    let (w, content) = b.indent_width(l);
    if content {
        return w;
    }
    let near = |dir: isize| {
        (1..50)
            .map(|d| l as isize + d * dir)
            .take_while(|&i| i >= 0 && (i as usize) < b.line_count())
            .find_map(|i| {
                let (w, content) = b.indent_width(i as usize);
                content.then_some(w)
            })
    };
    match (near(-1), near(1)) {
        (Some(a), Some(b)) => a.min(b),
        _ => 0,
    }
}

/// Colours 16-255 of the xterm palette: a 6×6×6 cube, then a grey ramp.
fn xterm_256(i: u8) -> Color32 {
    if i >= 232 {
        let v = 8 + (i - 232) * 10;
        return Color32::from_rgb(v, v, v);
    }
    let i = i - 16;
    let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
    Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
}
