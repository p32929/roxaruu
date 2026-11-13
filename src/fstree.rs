//! The project's files, ported from Oxru: a lazily-expanded tree for browsing,
//! and a full file list for fuzzy search.
//!
//! Policy (VS Code-like): VCS/tool internals are always hidden; the heavy
//! build/dependency dirs (`node_modules`, `target`, …) show in the tree but are
//! pruned from search; gitignored files show everywhere, flagged so the UI can
//! fade them.

use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

const ALWAYS_HIDDEN: &[&str] = &[".git", ".hg", ".svn", ".roxaruu", ".oxru", ".DS_Store"];

const SEARCH_PRUNE: &[&str] = &[
    ".idea",
    "node_modules",
    "bower_components",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".cache",
    ".output",
    "coverage",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".dart_tool",
    ".gradle",
    "Pods",
    "DerivedData",
    ".terraform",
];

/// Cap on files collected for search, so a giant tree can't stall the app.
const MAX_FILES: usize = 50_000;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// Gitignored: shown, but faded.
    pub ignored: bool,
}

pub struct FileTree {
    pub entries: Vec<Entry>,
    root: PathBuf,
    gitignore: Option<Gitignore>,
}

impl FileTree {
    /// The root's contents at depth 0.
    pub fn new(root: &Path) -> Self {
        let mut b = GitignoreBuilder::new(root);
        b.add(root.join(".gitignore")); // a missing file is fine
        let mut tree = FileTree {
            entries: Vec::new(),
            root: root.to_path_buf(),
            gitignore: b.build().ok(),
        };
        tree.entries = tree.read_dir(root, 0);
        tree
    }

    /// Re-read from disk after files were added, renamed or deleted, keeping
    /// open folders open.
    pub fn refresh(&mut self) {
        let open: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|e| e.expanded)
            .map(|e| e.path.clone())
            .collect();
        *self = FileTree::new(&self.root.clone());
        for dir in open {
            if let Some(i) = self.index_of(&dir) {
                self.expand(i);
            }
        }
    }

    /// Expand the folders down to `path` and return its row.
    pub fn reveal(&mut self, path: &Path) -> Option<usize> {
        let rel = path.strip_prefix(&self.root).ok()?;
        let mut at = self.root.clone();
        for part in rel.parent().into_iter().flat_map(|p| p.components()) {
            at.push(part);
            let i = self.index_of(&at)?;
            self.expand(i);
        }
        self.index_of(path)
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.path == path)
    }

    /// Expand a collapsed folder or collapse an expanded one.
    pub fn toggle(&mut self, idx: usize) {
        let Some(e) = self.entries.get(idx) else {
            return;
        };
        if !e.is_dir {
            return;
        }
        if e.expanded {
            self.collapse(idx);
        } else {
            self.expand(idx);
        }
    }

    pub fn expand(&mut self, idx: usize) {
        let Some(e) = self.entries.get(idx).filter(|e| e.is_dir && !e.expanded) else {
            return;
        };
        let children = self.read_dir(&e.path.clone(), e.depth + 1);
        if let Some(e) = self.entries.get_mut(idx) {
            e.expanded = true;
        }
        self.entries.splice(idx + 1..idx + 1, children);
    }

    pub fn collapse(&mut self, idx: usize) {
        let Some(e) = self.entries.get(idx).filter(|e| e.is_dir && e.expanded) else {
            return;
        };
        let depth = e.depth;
        let end = self.entries.get(idx + 1..).unwrap_or_default()
            .iter()
            .position(|c| c.depth <= depth)
            .map_or(self.entries.len(), |p| idx + 1 + p);
        if let Some(e) = self.entries.get_mut(idx) {
            e.expanded = false;
        }
        self.entries.drain(idx + 1..end);
    }

    /// Back to just the top level.
    pub fn collapse_all(&mut self) {
        self.entries.retain(|e| e.depth == 0);
        self.entries.iter_mut().for_each(|e| e.expanded = false);
    }

    /// The index of `idx`'s parent folder, if it isn't top level.
    pub fn parent(&self, idx: usize) -> Option<usize> {
        let depth = self.entries.get(idx)?.depth;
        self.entries.get(..idx)?.iter().rposition(|e| e.depth + 1 == depth)
    }

    fn read_dir(&self, dir: &Path, depth: usize) -> Vec<Entry> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut entries: Vec<Entry> =
            rd.flatten()
                .filter_map(|de| {
                    let name = de.file_name().to_string_lossy().into_owned();
                    if ALWAYS_HIDDEN.contains(&name.as_str()) {
                        return None;
                    }
                    let path = de.path();
                    let is_dir = de.file_type().is_ok_and(|t| t.is_dir());
                    let ignored = self.gitignore.as_ref().is_some_and(|gi| {
                        gi.matched_path_or_any_parents(&path, is_dir).is_ignore()
                    });
                    Some(Entry {
                        path,
                        name,
                        depth,
                        is_dir,
                        expanded: false,
                        ignored,
                    })
                })
                .collect();
        // Folders first, then case-insensitive by name.
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        entries
    }
}

/// Every searchable file under `root`, each flagged `true` if gitignored.
/// Computed by diffing the full walk against the gitignore-respecting one
/// (which handles nested `.gitignore`s correctly).
/// `build` includes the heavy build/dependency folders.
pub fn collect_files_marked(root: &Path, build: bool) -> Vec<(PathBuf, bool)> {
    let tracked: std::collections::HashSet<PathBuf> = walk(root, true, build).into_iter().collect();
    walk(root, false, build)
        .into_iter()
        .map(|p| (p.clone(), !tracked.contains(&p)))
        .collect()
}

/// The one definition of "a project file", shared by Go to File and Search
/// in Files so they can never disagree: dotfiles included, the project's own
/// ignore files honoured (optionally), never the user's global or a parent
/// repo's, and VCS internals plus (unless `build`) the heavy build/dep dirs pruned.
pub fn walk_builder(root: &Path, respect_gitignore: bool, build: bool) -> ignore::WalkBuilder {
    let mut wb = ignore::WalkBuilder::new(root);
    wb.hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(false)
        .git_exclude(respect_gitignore)
        .ignore(respect_gitignore)
        .parents(false)
        .require_git(false)
        .filter_entry(move |e| {
            let name = e.file_name().to_str().unwrap_or("");
            !ALWAYS_HIDDEN.contains(&name) && (build || !SEARCH_PRUNE.contains(&name))
        });
    wb
}

fn walk(root: &Path, respect_gitignore: bool, build: bool) -> Vec<PathBuf> {
    let wb = walk_builder(root, respect_gitignore, build);
    let mut out: Vec<PathBuf> = wb
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .take(MAX_FILES)
        .map(|e| e.into_path())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("README.md"), "# hi").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("target/junk"), "x").unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        dir
    }

    #[test]
    fn tree_lists_folders_first_and_hides_vcs() {
        let dir = make_tree();
        let tree = FileTree::new(dir.path());
        let names: Vec<_> = tree.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "target", "README.md"]);
    }

    #[test]
    fn expand_collapse_and_parent() {
        let dir = make_tree();
        let mut tree = FileTree::new(dir.path());
        tree.toggle(0);
        let names: Vec<_> = tree.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "lib.rs", "main.rs", "target", "README.md"]);
        assert_eq!(tree.entries[1].depth, 1);
        assert_eq!(tree.parent(2), Some(0));
        assert_eq!(tree.parent(0), None);
        tree.toggle(0);
        assert_eq!(tree.entries.len(), 3);
    }

    #[test]
    fn search_prunes_build_dirs_and_flags_gitignored() {
        let dir = make_tree();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
        fs::write(root.join("secret.txt"), "").unwrap();
        fs::create_dir_all(root.join("pkg/gen")).unwrap();
        fs::write(root.join("pkg/.gitignore"), "gen/\n").unwrap();
        fs::write(root.join("pkg/gen/out.rs"), "").unwrap();

        assert!(
            collect_files_marked(root, true)
                .iter()
                .any(|(p, _)| p.starts_with(root.join("target"))),
            "unless asked for"
        );
        let files = collect_files_marked(root, false);
        let flag = |s: &str| files.iter().find(|(p, _)| p.ends_with(s)).map(|(_, f)| *f);
        assert_eq!(flag("src/main.rs"), Some(false));
        assert_eq!(
            flag("secret.txt"),
            Some(true),
            "gitignored: listed, flagged"
        );
        assert_eq!(
            flag("pkg/gen/out.rs"),
            Some(true),
            "nested .gitignore applies"
        );
        assert_eq!(flag(".gitignore"), Some(false), "dotfiles are searchable");
        assert!(
            !files
                .iter()
                .any(|(p, _)| p.starts_with(root.join("target"))),
            "build dirs pruned"
        );
    }

    #[test]
    fn refresh_keeps_folders_open_and_reveal_expands() {
        let dir = make_tree();
        let root = dir.path();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::write(root.join("src/deep/x.rs"), "").unwrap();
        let mut tree = FileTree::new(root);
        tree.toggle(0);
        fs::write(root.join("src/new.rs"), "").unwrap();
        tree.refresh();
        assert!(tree.entries[0].expanded);
        assert!(tree.entries.iter().any(|e| e.name == "new.rs"));
        let i = tree.reveal(&root.join("src/deep/x.rs")).unwrap();
        assert_eq!(
            (tree.entries[i].name.as_str(), tree.entries[i].depth),
            ("x.rs", 2)
        );
    }

    #[test]
    fn gitignored_entry_is_flagged_in_tree() {
        let dir = make_tree();
        fs::write(dir.path().join(".gitignore"), ".env\n").unwrap();
        fs::write(dir.path().join(".env"), "").unwrap();
        let tree = FileTree::new(dir.path());
        assert!(
            tree.entries
                .iter()
                .find(|e| e.name == ".env")
                .unwrap()
                .ignored
        );
        assert!(
            !tree
                .entries
                .iter()
                .find(|e| e.name == "README.md")
                .unwrap()
                .ignored
        );
    }
}
