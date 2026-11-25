//! What git says changed in the open folder, for the file tree's colours:
//! the folder's own repository (it may be inside one, or be one) and any
//! repositories in its folders a few levels down (a folder holding several
//! projects). Asked with `git status`, off the window's thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How a file differs from git's last commit; a folder takes the strongest
/// of what's in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    Untracked,
    Added,
    Modified,
    Conflict,
}

impl Change {
    /// The letter beside a file's name, as in VS Code.
    pub fn letter(self) -> &'static str {
        match self {
            Change::Untracked => "U",
            Change::Added => "A",
            Change::Modified => "M",
            Change::Conflict => "!",
        }
    }
}

/// How deep, below the open folder, repositories of their own are looked
/// for (a worktree in `project/.claude/worktrees/x` is 4 down), at most how
/// many folders are read doing it, and how many repositories are asked.
const REPO_DEPTH: usize = 4;
const MAX_LOOKED_IN: usize = 4000;
const MAX_REPOS: usize = 20;

/// The repositories the open folder's changes come from: its own (the
/// folder `prefix` down from its top), and those in its folders. Found
/// once per folder opened (or files changing); `status` asks them.
#[derive(Clone, Debug, Default)]
pub struct Repos {
    own: Option<String>,
    below: Vec<PathBuf>,
}

/// The repositories under (or around) `root`.
pub fn find(root: &Path) -> Repos {
    let own = crate::sys::git(root, &["rev-parse", "--show-prefix"]).map(|p| String::from_utf8_lossy(&p).trim().to_string());
    Repos { own, below: repos_below(root) }
}

/// Every changed file and folder under `root` (paths as the tree has them),
/// or `None` with no repository there (or no git).
pub fn status(root: &Path, repos: &Repos) -> Option<HashMap<PathBuf, Change>> {
    let mut found = false;
    let mut out = HashMap::new();
    // The folder's own repository: its paths come from the repository's top.
    if let Some(prefix) = &repos.own
        && let Some(text) = crate::sys::git(root, &["status", "--porcelain=v1", "-z", "-unormal", "--", "."])
    {
        found = true;
        for (rel, change) in parse(&text) {
            if let Some(rel) = rel.strip_prefix(prefix.as_str()) {
                mark(&mut out, root, rel, change);
            }
        }
    }
    for repo in &repos.below {
        let Ok(at) = repo.strip_prefix(root) else { continue };
        let at = at.to_string_lossy().replace('\\', "/");
        let Some(text) = crate::sys::git(repo, &["status", "--porcelain=v1", "-z", "-unormal"]) else { continue };
        found = true;
        for (rel, change) in parse(&text) {
            mark(&mut out, root, &format!("{at}/{rel}"), change);
        }
    }
    found.then_some(out)
}

/// `git status --porcelain=v1 -z`: each `XY path` (a rename or copy is
/// followed by the path it came from, skipped).
fn parse(text: &[u8]) -> Vec<(String, Change)> {
    let text = String::from_utf8_lossy(text);
    let mut parts = text.split('\0');
    let mut out = Vec::new();
    while let Some(entry) = parts.next() {
        let (Some(xy), Some(path)) = (entry.get(..2), entry.get(3..)) else { continue };
        let (x, y) = (xy.chars().next().unwrap_or(' '), xy.chars().nth(1).unwrap_or(' '));
        if matches!(x, 'R' | 'C') {
            parts.next();
        }
        let change = match (x, y) {
            ('?', '?') => Change::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => Change::Conflict,
            ('A', _) => Change::Added,
            _ => Change::Modified, // modified, renamed, deleted (its folders show it)
        };
        out.push((path.to_string(), change));
    }
    out
}

/// `rel` (with `/`, a folder's ending in one) and each folder it's in, the
/// strongest change kept.
fn mark(out: &mut HashMap<PathBuf, Change>, root: &Path, rel: &str, change: Change) {
    let mut at = root.to_path_buf();
    for part in rel.trim_end_matches('/').split('/').filter(|p| !p.is_empty()) {
        at.push(part);
        let was = out.entry(at.clone()).or_insert(change);
        *was = (*was).max(change);
    }
}

/// Folders up to `REPO_DEPTH` below `root` that are repositories (`.git`, a
/// folder or a worktree's file), not looking inside version control's own
/// or build folders.
// ponytail: stops after `MAX_LOOKED_IN` folders (a home folder opened):
// repositories past that get no colours.
fn repos_below(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut looked = 0;
    let mut level = vec![root.to_path_buf()];
    for _ in 0..REPO_DEPTH {
        let mut next = Vec::new();
        for dir in &level {
            looked += 1;
            if looked > MAX_LOOKED_IN {
                return out;
            }
            let Ok(read) = std::fs::read_dir(dir) else { continue };
            for e in read.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if crate::fstree::is_skipped(&name) || !e.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let path = e.path();
                if path.join(".git").exists() {
                    out.push(path.clone());
                    if out.len() >= MAX_REPOS {
                        return out;
                    }
                }
                next.push(path);
            }
        }
        level = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_read_and_folders_marked() {
        let text = b" M src/a.rs\0?? new/\0R  b2.rs\0b.rs\0A  src/deep/c.rs\0UU d.rs\0 D gone/x.rs\0";
        let got = parse(text);
        assert_eq!(
            got,
            [
                ("src/a.rs".to_string(), Change::Modified),
                ("new/".to_string(), Change::Untracked),
                ("b2.rs".to_string(), Change::Modified),
                ("src/deep/c.rs".to_string(), Change::Added),
                ("d.rs".to_string(), Change::Conflict),
                ("gone/x.rs".to_string(), Change::Modified),
            ]
        );
        let root = Path::new("/r");
        let mut out = HashMap::new();
        for (rel, change) in got {
            mark(&mut out, root, &rel, change);
        }
        assert_eq!(out.get(&root.join("src")), Some(&Change::Modified), "the strongest inside");
        assert_eq!(out.get(&root.join("src").join("deep")), Some(&Change::Added));
        assert_eq!(out.get(&root.join("new")), Some(&Change::Untracked));
        assert_eq!(out.get(&root.join("gone")), Some(&Change::Modified), "a deleted file's folder");
        assert_eq!(out.get(&root.join("b.rs")), None, "where a rename came from");
    }

    /// Against real git, if there is one: the folder's own repository, and
    /// one in a folder below a folder that isn't one.
    #[test]
    fn real_repositories_own_and_below() {
        if which::which("git").is_err() {
            return;
        }
        let git = |dir: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success();
            assert!(ok, "git {args:?}");
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        let app = root.join("app");
        std::fs::create_dir_all(app.join("src")).unwrap();
        git(&app, &["init", "-q"]);
        git(&app, &["config", "user.email", "t@t"]);
        git(&app, &["config", "user.name", "t"]);
        std::fs::write(app.join("src/kept.rs"), "1").unwrap();
        git(&app, &["add", "."]);
        git(&app, &["commit", "-qm", "one"]);
        std::fs::write(app.join("src/kept.rs"), "2").unwrap();
        std::fs::write(app.join("fresh.rs"), "").unwrap();
        // A worktree tucked away in a hidden folder, 4 down from `root`.
        git(&app, &["worktree", "add", "-q", "-b", "wt", ".claude/worktrees/x"]);
        let wt = app.join(".claude/worktrees/x");
        std::fs::write(wt.join("src/kept.rs"), "3").unwrap();
        let nothing = dir.path().join("nothing");
        std::fs::create_dir_all(&nothing).unwrap();
        assert_eq!(status(&nothing, &find(&nothing)), None, "no repository");
        let got = status(&root, &find(&root)).unwrap();
        assert_eq!(got.get(&wt.join("src").join("kept.rs")), Some(&Change::Modified), "in the worktree");
        assert_eq!(got.get(&app.join("src").join("kept.rs")), Some(&Change::Modified));
        assert_eq!(got.get(&app.join("src")), Some(&Change::Modified));
        assert_eq!(got.get(&app), Some(&Change::Modified));
        assert_eq!(got.get(&app.join("fresh.rs")), Some(&Change::Untracked));
        // Opened at a folder inside the repository: its paths from there.
        let inner = status(&app.join("src"), &find(&app.join("src"))).unwrap();
        assert_eq!(inner.get(&app.join("src").join("kept.rs")), Some(&Change::Modified));
        assert_eq!(inner.get(&app.join("fresh.rs")), None, "outside the open folder");
    }
}
