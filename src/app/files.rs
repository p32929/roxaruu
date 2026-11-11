//! Working with files: new / rename / delete / reveal / copy path from the
//! file tree or its search (or on the open file), files dropped from Finder, files
//! pasted from Finder, and links clicked in a terminal.

use std::path::{Path, PathBuf};

use eframe::egui;

use super::{App, Focus, display_name, rel_path};
use crate::editor::Tab;
use crate::keymap::Command;
use crate::palette::Mode;

/// A question with a text field (or a yes/no) about a file or link.
#[derive(Clone, Debug)]
pub enum Ask {
    NewFile { dir: PathBuf, name: String },
    NewFolder { dir: PathBuf, name: String },
    Rename { path: PathBuf, name: String },
    Delete { path: PathBuf },
    OpenUrl(String),
    /// The password for turning remote access on (typed hidden).
    RemotePassword { password: String },
    /// Where to open the windows from a browser.
    RemoteAddress,
    /// ⌘Q or the close button: close this window? (What's running stops.)
    Quit,
}

impl App {
    /// What Rename, Delete, Reveal and Copy Path act on: in the file tree,
    /// its selected row (or chosen search match), else (from the editor or
    /// the command palette) the active tab's file.
    pub fn file_target(&self) -> Option<PathBuf> {
        match self.palette.as_ref().map(|p| p.mode) {
            // Searching: the chosen match (none, with nothing found).
            None if self.focus == Focus::Sidebar && !self.sidebar.query.is_empty() => {
                self.hit_path(self.sidebar.hit)
            }
            None if self.focus == Focus::Sidebar => self.sidebar.selected.clone(),
            None | Some(Mode::Commands) => self.active_tab().and_then(|t| t.path().cloned()),
            _ => None,
        }
    }

    /// Where New File / New Folder create: beside (or inside) the selected
    /// row in the file tree, else beside the active file, else the project root.
    fn new_file_dir(&self) -> Option<PathBuf> {
        let root = self.project.as_ref()?.root.clone();
        let target = self.file_target().filter(|p| p.starts_with(&root));
        let dir = match target {
            Some(p) if p.is_dir() => p,
            Some(p) => p.parent().map_or(root.clone(), Path::to_path_buf),
            None => root,
        };
        Some(dir)
    }

    pub fn run_file_command(&mut self, command: Command, ctx: &egui::Context) {
        if command == Command::ToggleBuildFolders {
            return self.toggle_build_folders(ctx);
        }
        if matches!(command, Command::NewFile | Command::NewFolder) {
            let Some(dir) = self.new_file_dir() else {
                return self.notify("Open a folder first");
            };
            let name = String::new();
            self.ask = Some(if command == Command::NewFile {
                Ask::NewFile { dir, name }
            } else {
                Ask::NewFolder { dir, name }
            });
            return;
        }
        let Some(path) = self.file_target() else {
            return self.notify("Select a file first");
        };
        let is_root = self.project.as_ref().is_some_and(|p| p.root == path);
        match command {
            Command::RenameFile | Command::DeleteFile if is_root => {
                self.notify("That's the project folder")
            }
            Command::RenameFile => {
                self.ask = Some(Ask::Rename {
                    name: display_name(&path),
                    path,
                })
            }
            Command::DeleteFile => self.ask = Some(Ask::Delete { path }),
            Command::RevealInFinder => {
                // The file manager opens its folder with it selected.
                if !os_reveal(&path) {
                    self.warn("Couldn't open the file manager");
                }
            }
            // A new terminal there (a file's: its folder), what runs in the
            // others left alone.
            Command::OpenInTerminal => {
                let dir = if path.is_dir() { path } else { path.parent().map_or(path.clone(), Path::to_path_buf) };
                self.new_terminal_in(Some(dir), ctx);
            }
            Command::CopyPath | Command::CopyRelativePath => {
                let text = match &self.project {
                    Some(p)
                        if command == Command::CopyRelativePath && path.starts_with(&p.root) =>
                    {
                        rel_path(&p.root, &path)
                    }
                    _ => path.to_string_lossy().into_owned(),
                };
                self.copy(ctx, text.clone());
                self.notify(format!("Copied {text}"));
            }
            _ => {}
        }
    }

    fn toggle_build_folders(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.project else { return };
        p.build = !p.build;
        let msg = if p.build {
            "Search now includes node_modules, target and other build folders"
        } else {
            "Search skips node_modules, target and other build folders"
        };
        self.notify(msg);
        self.index(ctx);
        self.search.query.clear(); // run the current search again
        self.search.changed = Some(std::time::Instant::now());
    }

    /// Enter / a button on the file dialog: `ok` false cancels.
    pub fn answer_ask(&mut self, ok: bool, ctx: &egui::Context) {
        let Some(ask) = self.ask.take() else { return };
        if !ok {
            return;
        }
        let result = match ask {
            Ask::NewFile { dir, name } => self.create(&dir, name.trim(), false, ctx),
            Ask::NewFolder { dir, name } => self.create(&dir, name.trim(), true, ctx),
            Ask::Rename { path, name } => self.rename(&path, name.trim(), ctx),
            Ask::Delete { path } => self.delete(&path, ctx),
            Ask::OpenUrl(url) => {
                self.open_url(url, ctx);
                Ok(())
            }
            Ask::RemotePassword { password } => self.start_remote(&password),
            Ask::Quit => {
                // Then "save changes?" for each unsaved file, then close.
                self.begin_close(self.tab_paths(), crate::app::After::Quit, ctx);
                Ok(())
            }
            Ask::RemoteAddress => {
                // "Copy Address".
                if let Some(url) = self.remote.url.clone() {
                    self.copy(ctx, url);
                    self.notify("Address copied");
                }
                Ok(())
            }
        };
        if let Err(e) = result {
            self.warn(e);
        }
    }

    /// New file or folder `name` (may be a nested path like `src/a.rs`) in `dir`.
    fn create(
        &mut self,
        dir: &Path,
        name: &str,
        folder: bool,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if name.is_empty() {
            return Ok(());
        }
        let path = dir.join(name);
        if folder {
            std::fs::create_dir_all(&path).map_err(|e| format!("Couldn't create {name}: {e}"))?;
            self.files_changed(Some(&path), ctx);
            self.notify(format!("Created {name}/"));
            return Ok(());
        }
        if path.is_dir() {
            return Err(format!("{name} is a folder"));
        }
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Couldn't create {name}: {e}"))?;
            }
            std::fs::write(&path, "").map_err(|e| format!("Couldn't create {name}: {e}"))?;
        }
        self.files_changed(None, ctx);
        self.close_palette(true, ctx);
        self.open_file(&path);
        self.focus = Focus::Editor;
        Ok(())
    }

    fn rename(&mut self, from: &Path, name: &str, ctx: &egui::Context) -> Result<(), String> {
        if name.is_empty() || name.contains('/') {
            return Err("A name can't be empty or contain /".into());
        }
        let to = from.with_file_name(name);
        if to == from {
            return Ok(());
        }
        // Never rename over something — except the same file in another case
        // ("readme" → "README" on a case-insensitive disk).
        if to.exists() && !same_file(from, &to) {
            return Err(format!("{name} already exists"));
        }
        std::fs::rename(from, &to).map_err(|e| format!("Couldn't rename: {e}"))?;
        // Open tabs follow the file (or everything under a renamed folder).
        for tab in &mut self.tabs {
            if let Some(rest) = tab.path().and_then(|p| p.strip_prefix(from).ok()) {
                let moved = if rest.as_os_str().is_empty() {
                    to.clone()
                } else {
                    to.join(rest)
                };
                tab.retarget(moved);
            }
        }
        self.closed.retain(|p| !p.starts_with(from));
        self.save_session();
        self.files_changed(Some(&to), ctx);
        self.notify(format!("Renamed to {name}"));
        Ok(())
    }

    /// Move to the Trash (recoverable), closing its tabs.
    fn delete(&mut self, path: &Path, ctx: &egui::Context) -> Result<(), String> {
        trash(path)
            .map_err(|e| format!("Couldn't move {} to the Trash: {e}", display_name(path)))?;
        // Tabs with unsaved work stay open (marked unsaved, so it can be saved back).
        while let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.path().is_some_and(|p| p.starts_with(path)) && !t.buf.modified)
        {
            self.close_tab_now(i);
        }
        for t in &mut self.tabs {
            if t.path().is_some_and(|p| p.starts_with(path)) {
                t.gone = true; // struck through; no need to ask, you did it
                t.buf.forget_disk();
            }
        }
        self.closed.retain(|p| !p.starts_with(path));
        self.files_changed(None, ctx);
        self.notify(format!("Moved {} to the Trash", display_name(path)));
        Ok(())
    }

    /// Files were added, renamed or deleted: re-read the tree (keeping folders
    /// open), re-index for search, and select `select` in the tree.
    fn files_changed(&mut self, select: Option<&Path>, ctx: &egui::Context) {
        let Some(p) = &mut self.project else { return };
        p.tree.refresh();
        let row = select.and_then(|s| p.tree.reveal(s));
        if row.is_some() {
            self.sidebar.selected = select.map(Path::to_path_buf);
            self.sidebar.scroll = true;
        }
        self.index(ctx);
        if row.is_some() {
            self.sidebar.query.clear(); // show it in the tree
            self.sidebar.searching = false;
        }
    }

    // ---- drag & drop, paste ---------------------------------------------

    /// Files dropped from Finder: onto a terminal, their paths are typed
    /// there (shell-quoted, like Terminal.app); anywhere else they open.
    pub fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        if let Some(i) = self.drop_terminal(ctx) {
            let text = shell_args(&dropped) + " ";
            self.active_terminal = i;
            self.focus = Focus::Terminal;
            self.terminals[i].pane.paste(&text);
            return;
        }
        for path in dropped {
            if path.is_dir() {
                self.open_folder(path, ctx);
            } else {
                self.open_file(&path);
                self.focus = Focus::Editor;
            }
        }
    }

    /// The terminal under the pointer during a drag from Finder, if any.
    pub fn drop_terminal(&self, ctx: &egui::Context) -> Option<usize> {
        let at = drop_pos(ctx)?;
        self.terminal_rects
            .iter()
            .find(|(_, r)| r.contains(at))
            .map(|(i, _)| *i)
    }

    /// ⌘V with files copied in Finder pastes their paths — shell-quoted into a
    /// terminal, one per line elsewhere — instead of the bare file names macOS
    /// also puts on the clipboard as text.
    pub fn paste_files_as_paths(&self, ctx: &egui::Context) {
        if !ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))) {
            return;
        }
        if let Some(text) = self.clipboard_paths() {
            ctx.input_mut(|i| {
                for e in &mut i.events {
                    if let egui::Event::Paste(t) = e {
                        *t = text.clone();
                    }
                }
            });
        }
    }

    /// The clipboard's files as text for where the keyboard is, if it holds files.
    pub fn clipboard_paths(&self) -> Option<String> {
        let files = clipboard_files()?;
        let shell = self.terminal_focused() && self.palette.is_none();
        Some(if shell {
            shell_args(&files)
        } else {
            files
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// Open a link in the browser of whoever asked: the Mac's, or (asked
    /// from a browser looking in) that browser.
    pub fn open_url(&mut self, url: String, ctx: &egui::Context) {
        if self.remote_pass {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        } else if !os_open(&[url.as_ref()]) {
            self.warn("Couldn't open your browser");
        }
    }

    /// ⌘+click on a link in a terminal: ask before opening it.
    pub fn offer_url(&mut self, url: String) {
        self.ask = Some(Ask::OpenUrl(url));
    }

    /// Re-open what a test made the app hand to the OS (Finder, browser).
    #[cfg(test)]
    pub fn take_launched() -> Vec<Vec<String>> {
        LAUNCHED.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }
}

impl Tab {
    /// The file was renamed or moved: follow it (the language may change).
    pub fn retarget(&mut self, path: PathBuf) {
        self.buf.lang = crate::syntax::Lang::detect(&path);
        if let Some(h) = &mut self.huge {
            h.path = path.clone(); // it reads from there
        }
        self.buf.path = Some(path);
        self.invalidate_highlight();
    }
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
}

/// Windows: the same file by its full real path (no inode numbers to ask).
#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Paths as shell arguments, backslash-escaped like Terminal.app does.
fn shell_args(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| crate::terminalpane::shell_escape(p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the pointer is during a drag from Finder (see `sys::pointer_in_window`).
#[cfg(all(target_os = "macos", not(test)))]
fn drop_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let inner = ctx.input(|i| i.viewport().inner_rect)?;
    crate::sys::pointer_in_window(inner)
}

/// Tests (and other platforms) go by egui's pointer.
#[cfg(any(not(target_os = "macos"), test))]
fn drop_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    ctx.input(|i| i.pointer.latest_pos())
}

#[cfg(test)]
thread_local! {
    static LAUNCHED: std::cell::RefCell<Vec<Vec<String>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// What the "system clipboard" holds as files, for tests.
    static CLIPBOARD_FILES: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only: put files on the "clipboard", as copying them in Finder does.
#[cfg(test)]
pub fn set_clipboard_files(files: Vec<PathBuf>) {
    CLIPBOARD_FILES.with(|c| *c.borrow_mut() = files);
}

/// Hand `args` to `open` (Finder, the browser). Tests only record it — they
/// must never open windows.
pub fn os_open(args: &[&std::ffi::OsStr]) -> bool {
    #[cfg(test)]
    {
        LAUNCHED.with(|l| {
            l.borrow_mut().push(
                args.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
            )
        });
        true
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        std::process::Command::new("open").args(args).status().is_ok_and(|s| s.success())
    }
    // Elsewhere: the system's default app (Windows' shell, Linux's
    // xdg-open), for the one thing asked.
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        args.first().is_some_and(|a| opener::open(a).is_ok())
    }
}

/// Show `path` selected in the file manager (Finder, File Explorer, the
/// Linux desktop's). Tests only record it.
pub fn os_reveal(path: &Path) -> bool {
    #[cfg(any(test, target_os = "macos"))]
    {
        os_open(&["-R".as_ref(), path.as_os_str()])
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        opener::reveal(path).is_ok()
    }
}

/// Open `dir` in a new window (its own process). Tests only record it.
pub fn spawn_window(dir: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    {
        LAUNCHED.with(|l| {
            l.borrow_mut().push(vec![
                "new window".into(),
                dir.to_string_lossy().into_owned(),
            ])
        });
        Ok(())
    }
    #[cfg(not(test))]
    {
        std::process::Command::new(std::env::current_exe()?)
            .arg(dir)
            .spawn()
            .map(|_| ())
    }
}

#[cfg(not(test))]
fn clipboard_files() -> Option<Vec<PathBuf>> {
    let files = arboard::Clipboard::new().ok()?.get().file_list().ok()?;
    (!files.is_empty()).then_some(files)
}

#[cfg(test)]
fn clipboard_files() -> Option<Vec<PathBuf>> {
    let files = CLIPBOARD_FILES.with(|c| c.borrow().clone());
    (!files.is_empty()).then_some(files)
}

/// Move to the Trash. Tests delete outright, so nothing lands in the real Trash.
#[cfg(not(test))]
fn trash(path: &Path) -> std::io::Result<()> {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("/usr/bin/trash", &[])
    } else {
        ("gio", &["trash"])
    };
    let out = std::process::Command::new(cmd)
        .args(args)
        .arg(path)
        .output()?;
    if out.status.success() && !path.exists() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(std::io::Error::other(if err.is_empty() {
            "the trash command failed".to_string()
        } else {
            err
        }))
    }
}

#[cfg(test)]
fn trash(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_args_escape_spaces() {
        assert_eq!(
            shell_args(&[PathBuf::from("/a b/c.txt"), PathBuf::from("/d")]),
            "/a\\ b/c.txt /d"
        );
    }

    #[test]
    fn same_file_sees_through_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::write(&a, "").unwrap();
        std::fs::write(dir.path().join("b"), "").unwrap();
        assert!(same_file(&a, &a));
        assert!(!same_file(&a, &dir.path().join("b")));
    }
}
