//! What git says changed in the open folder, and doing something about it:
//! the folder's own repository (it may be inside one, or be one) and any
//! repositories in its folders a few levels down (a folder holding several
//! projects, a worktree). One `git status` per repository gives both the
//! file tree's colours and the Changes view's lists; staging, committing,
//! pushing and the rest are plain `git` runs. All off the window's thread.

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

/// The repositories the open folder's changes come from: its own (its top
/// folder, and the open folder's path from there), and those in its
/// folders. Found once per folder opened (or files changing).
#[derive(Clone, Debug, Default)]
pub struct Repos {
    own: Option<(PathBuf, String)>,
    below: Vec<PathBuf>,
}

/// The repositories under (or around) `root`.
pub fn find(root: &Path) -> Repos {
    let own = crate::sys::git(root, &["rev-parse", "--show-toplevel", "--show-prefix"]).and_then(|out| {
        let out = String::from_utf8_lossy(&out).into_owned();
        let mut lines = out.lines();
        let top = PathBuf::from(lines.next()?.trim());
        Some((top, lines.next().unwrap_or("").trim().to_string()))
    });
    Repos { own, below: repos_below(root) }
}

/// One repository, as the Changes view shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Repo {
    /// Where git runs (its top folder).
    pub top: PathBuf,
    /// The open folder's path inside it ("" for all of it): what "everything"
    /// means for a folder opened inside a repository.
    pub scope: String,
    /// As shown: its path from the open folder, or the open folder's name.
    pub name: String,
    pub head: Head,
    pub files: Vec<File>,
}

/// The branch, and how it stands against the one it pushes to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Head {
    /// The branch's name ("HEAD (no branch)" for a detached one).
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// A changed file: where it is (as the tree has it, and as git names it),
/// and its state in the index (`staged`) and the folder (`unstaged`), each
/// as git's letter (`M` `A` `D` `R` `C` `T`; `?` untracked, `!` a conflict).
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub path: PathBuf,
    pub rel: String,
    pub staged: Option<char>,
    pub unstaged: Option<char>,
}

impl File {
    pub fn untracked(&self) -> bool {
        self.unstaged == Some('?')
    }
}

/// What `status` found: every changed file and folder (the tree's colours)
/// and each repository with changes or not.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub tree: HashMap<PathBuf, Change>,
    pub repos: Vec<Repo>,
}

/// Ask every repository in `repos`; `None` with no repository there (or no git).
pub fn status(root: &Path, repos: &Repos) -> Option<Status> {
    let mut out = Status::default();
    let name = root.file_name().map_or_else(|| root.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
    // The folder's own repository: its paths come from the repository's top.
    if let Some((top, prefix)) = &repos.own
        && let Some(text) = crate::sys::git(root, &["status", "--porcelain=v1", "-z", "-b", "-unormal", "--", "."])
    {
        let (head, entries) = parse(&text);
        let mut repo = Repo { top: top.clone(), scope: prefix.trim_end_matches('/').to_string(), name: name.clone(), head, files: Vec::new() };
        for (rel, x, y) in entries {
            let Some(inside) = rel.strip_prefix(prefix.as_str()) else { continue };
            let path = under(root, inside);
            mark(&mut out.tree, root, inside, change(x, y));
            repo.files.push(File { path, rel, staged: staged(x, y), unstaged: unstaged(x, y) });
        }
        out.repos.push(repo);
    }
    for top in &repos.below {
        let Ok(at) = top.strip_prefix(root) else { continue };
        let at = at.to_string_lossy().replace('\\', "/");
        let Some(text) = crate::sys::git(top, &["status", "--porcelain=v1", "-z", "-b", "-unormal"]) else { continue };
        let (head, entries) = parse(&text);
        let mut repo = Repo { top: top.clone(), scope: String::new(), name: at.clone(), head, files: Vec::new() };
        for (rel, x, y) in entries {
            mark(&mut out.tree, root, &format!("{at}/{rel}"), change(x, y));
            repo.files.push(File { path: under(top, &rel), rel, staged: staged(x, y), unstaged: unstaged(x, y) });
        }
        out.repos.push(repo);
    }
    (!out.repos.is_empty()).then_some(out)
}

fn under(dir: &Path, rel: &str) -> PathBuf {
    let mut p = dir.to_path_buf();
    p.extend(rel.trim_end_matches('/').split('/').filter(|s| !s.is_empty()));
    p
}

fn conflict(x: char, y: char) -> bool {
    x == 'U' || y == 'U' || (x, y) == ('A', 'A') || (x, y) == ('D', 'D')
}

/// The tree's one word for a file's state.
fn change(x: char, y: char) -> Change {
    match (x, y) {
        ('?', '?') => Change::Untracked,
        _ if conflict(x, y) => Change::Conflict,
        ('A', _) => Change::Added,
        _ => Change::Modified, // modified, renamed, deleted (its folders show it)
    }
}

fn staged(x: char, y: char) -> Option<char> {
    (!matches!(x, ' ' | '?') && !conflict(x, y)).then_some(x)
}

fn unstaged(x: char, y: char) -> Option<char> {
    if conflict(x, y) {
        Some('!')
    } else {
        (y != ' ').then_some(y)
    }
}

/// `git status --porcelain=v1 -z -b`: the branch line, then each `XY path`
/// (a rename or copy is followed by the path it came from, skipped).
fn parse(text: &[u8]) -> (Head, Vec<(String, char, char)>) {
    let text = String::from_utf8_lossy(text);
    let mut parts = text.split('\0');
    let mut head = Head::default();
    let mut out = Vec::new();
    while let Some(entry) = parts.next() {
        if let Some(b) = entry.strip_prefix("## ") {
            head = branch_line(b);
            continue;
        }
        let (Some(xy), Some(path)) = (entry.get(..2), entry.get(3..)) else { continue };
        let (x, y) = (xy.chars().next().unwrap_or(' '), xy.chars().nth(1).unwrap_or(' '));
        if matches!(x, 'R' | 'C') {
            parts.next();
        }
        out.push((path.to_string(), x, y));
    }
    (head, out)
}

/// `main...origin/main [ahead 1, behind 2]`, `main`, `No commits yet on
/// main`, `HEAD (no branch)`.
fn branch_line(b: &str) -> Head {
    let (names, counts) = b.split_once(" [").unwrap_or((b, ""));
    let names = names.strip_prefix("No commits yet on ").or_else(|| names.strip_prefix("Initial commit on ")).unwrap_or(names);
    let (branch, upstream) = match names.split_once("...") {
        Some((b, u)) => (b.to_string(), Some(u.to_string())),
        None => (names.to_string(), None),
    };
    let count = |what: &str| {
        counts.trim_end_matches(']').split(", ").find_map(|c| c.strip_prefix(what)?.trim().parse().ok()).unwrap_or(0)
    };
    // "[gone]": the branch it pushed to was deleted.
    let upstream = upstream.filter(|_| !counts.starts_with("gone"));
    Head { branch, upstream, ahead: count("ahead "), behind: count("behind ") }
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

// ---- doing things ------------------------------------------------------

/// Something to do in one repository.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Into the index; out of it again.
    Stage(Vec<String>),
    Unstage(Vec<String>),
    /// Back to the last commit (untracked files go to the trash).
    Discard(Vec<String>),
    /// Every change in the repository (in the open folder's part of it).
    StageAll,
    /// What's staged, or everything if nothing is.
    Commit(String),
    Push,
    Pull,
}

impl Op {
    /// As a toast says it was done.
    pub fn done(&self) -> &'static str {
        match self {
            Op::Stage(_) | Op::StageAll => "staged",
            Op::Unstage(_) => "unstaged",
            Op::Discard(_) => "discarded",
            Op::Commit(_) => "committed",
            Op::Push => "pushed",
            Op::Pull => "pulled",
        }
    }

    /// Whether `repo` has anything for this to do (the "all" versions skip
    /// the rest).
    pub fn applies(&self, repo: &Repo) -> bool {
        match self {
            Op::StageAll | Op::Commit(_) => !repo.files.is_empty(),
            Op::Push => repo.head.ahead > 0 || repo.head.upstream.is_none(),
            _ => true,
        }
    }
}

/// Do `op` in `repo`: what to say, or what went wrong (git's own words).
pub fn apply(repo: &Repo, op: &Op) -> Result<String, String> {
    let git = |args: &[&str]| crate::sys::git_run(&repo.top, args);
    let with = |args: &[&str], rels: &[String]| {
        let mut all: Vec<&str> = args.to_vec();
        all.push("--");
        all.extend(rels.iter().map(String::as_str));
        git(&all)
    };
    let scope = if repo.scope.is_empty() { ".".to_string() } else { repo.scope.clone() };
    match op {
        Op::Stage(rels) => with(&["add", "-A"], rels).map(drop),
        // `reset` works before the first commit too (`restore --staged` doesn't).
        Op::Unstage(rels) => with(&["reset", "-q"], rels).map(drop),
        Op::Discard(rels) => {
            let (untracked, tracked): (Vec<&File>, Vec<&File>) =
                repo.files.iter().filter(|f| rels.contains(&f.rel)).partition(|f| f.untracked() && f.staged.is_none());
            if !tracked.is_empty() {
                let rels: Vec<String> = tracked.iter().map(|f| f.rel.clone()).collect();
                // Index and folder both back to the last commit; a file added
                // since (none there) is just taken out of the index.
                let (new, old): (Vec<String>, Vec<String>) =
                    rels.into_iter().partition(|r| tracked.iter().any(|f| &f.rel == r && f.staged == Some('A')));
                if !old.is_empty() {
                    with(&["checkout", "-q", "HEAD"], &old)?;
                }
                if !new.is_empty() {
                    with(&["rm", "-q", "--cached", "-r"], &new)?;
                }
            }
            for f in untracked {
                crate::app::files::trash(&f.path).map_err(|e| format!("couldn't move {} to the trash: {e}", f.rel))?;
            }
            Ok(())
        }
        Op::StageAll => with(&["add", "-A"], &[scope]).map(drop),
        Op::Commit(message) => {
            // ponytail: a folder opened inside a repository commits whatever
            // else is staged in it too, as git itself would.
            if !repo.files.iter().any(|f| f.staged.is_some()) {
                with(&["add", "-A"], &[scope])?;
            }
            git(&["commit", "-q", "-m", message]).map(drop)
        }
        Op::Push if repo.head.upstream.is_some() => git(&["push", "-q"]).map(drop),
        Op::Push => {
            // No branch to push to yet: one of the same name on its remote.
            let remotes = git(&["remote"])?;
            let remote = remotes.lines().find(|r| *r == "origin").or_else(|| remotes.lines().next());
            let Some(remote) = remote else { return Err("no remote to push to (git remote add …)".into()) };
            git(&["push", "-q", "-u", remote, "HEAD"]).map(drop)
        }
        Op::Pull => git(&["pull", "-q"]).map(drop),
    }
    .map(|()| op.done().to_string())
}

/// The change to one file, as a unified diff: what's staged, or what isn't
/// (an untracked file: all of it, added).
pub fn diff(repo: &Repo, file: &File, staged: bool) -> Result<String, String> {
    if !staged && file.untracked() {
        let text = std::fs::read_to_string(&file.path).map_err(|e| format!("couldn't read {}: {e}", file.rel))?;
        let n = text.lines().count();
        let mut out = format!("new file\n@@ -0,0 +1,{n} @@\n");
        for l in text.lines() {
            out.push('+');
            out.push_str(l);
            out.push('\n');
        }
        return Ok(out);
    }
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", file.rel.as_str()]);
    crate::sys::git_run(&repo.top, &args)
}

/// What a commit changed, as `git show` has it.
pub fn show(repo: &Repo, hash: &str) -> Result<String, String> {
    crate::sys::git_run(&repo.top, &["show", "--no-color", "--no-ext-diff", "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n%w(0,4,4)%B", hash])
}

/// One commit in a repository's history.
#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub author: String,
    pub when: String,
}

/// The most recent commits (the open folder's part of it).
pub fn log(repo: &Repo, n: usize) -> Vec<Commit> {
    let count = format!("-n{n}");
    let mut args = vec!["log", count.as_str(), "--format=%h%x1f%s%x1f%an%x1f%ar%x1e"];
    if !repo.scope.is_empty() {
        args.extend(["--", repo.scope.as_str()]);
    }
    let text = crate::sys::git_run(&repo.top, &args).unwrap_or_default();
    text.split('\x1e')
        .filter_map(|c| {
            let mut f = c.trim().split('\x1f');
            Some(Commit { hash: f.next()?.to_string(), subject: f.next()?.to_string(), author: f.next()?.to_string(), when: f.next()?.to_string() })
        })
        .filter(|c| !c.hash.is_empty())
        .collect()
}

/// Where each line of a diff is in the file now (`None`: a removed line, or
/// a header), so a line of a diff can open the file there.
pub fn new_lines(diff: &str) -> Vec<Option<usize>> {
    let mut at: Option<usize> = None;
    diff.lines()
        .map(|l| {
            if let Some(h) = l.strip_prefix("@@ ") {
                // "@@ -a,b +c,d @@": the next line is line c.
                at = h.split(' ').find_map(|p| p.strip_prefix('+')).and_then(|p| p.split(',').next()?.parse::<usize>().ok()).map(|c| c.saturating_sub(1));
                return None;
            }
            let n = at?;
            match l.chars().next() {
                Some('-') => None,
                Some('+' | ' ') => {
                    at = Some(n + 1);
                    Some(n)
                }
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_read_and_folders_marked() {
        let text = b"## main...origin/main [ahead 2, behind 1]\0 M src/a.rs\0?? new/\0R  b2.rs\0b.rs\0A  src/deep/c.rs\0UU d.rs\0 D gone/x.rs\0MM both.rs\0";
        let (head, got) = parse(text);
        assert_eq!(head, Head { branch: "main".into(), upstream: Some("origin/main".into()), ahead: 2, behind: 1 });
        let names: Vec<&str> = got.iter().map(|g| g.0.as_str()).collect();
        assert_eq!(names, ["src/a.rs", "new/", "b2.rs", "src/deep/c.rs", "d.rs", "gone/x.rs", "both.rs"]);
        let state: Vec<(Option<char>, Option<char>)> = got.iter().map(|&(_, x, y)| (staged(x, y), unstaged(x, y))).collect();
        assert_eq!(
            state,
            [(None, Some('M')), (None, Some('?')), (Some('R'), None), (Some('A'), None), (None, Some('!')), (None, Some('D')), (Some('M'), Some('M'))]
        );
        let root = Path::new("/r");
        let mut out = HashMap::new();
        for (rel, x, y) in got {
            mark(&mut out, root, &rel, change(x, y));
        }
        assert_eq!(out.get(&root.join("src")), Some(&Change::Modified), "the strongest inside");
        assert_eq!(out.get(&root.join("src").join("deep")), Some(&Change::Added));
        assert_eq!(out.get(&root.join("new")), Some(&Change::Untracked));
        assert_eq!(out.get(&root.join("d.rs")), Some(&Change::Conflict));
        assert_eq!(out.get(&root.join("gone")), Some(&Change::Modified), "a deleted file's folder");
        assert_eq!(out.get(&root.join("b.rs")), None, "where a rename came from");
    }

    #[test]
    fn branch_lines() {
        let b = |s: &str| branch_line(s);
        assert_eq!(b("main"), Head { branch: "main".into(), ..Default::default() });
        assert_eq!(b("No commits yet on dev"), Head { branch: "dev".into(), ..Default::default() });
        assert_eq!(b("HEAD (no branch)").branch, "HEAD (no branch)");
        assert_eq!(b("f...origin/f [behind 3]"), Head { branch: "f".into(), upstream: Some("origin/f".into()), ahead: 0, behind: 3 });
        assert_eq!(b("f...origin/f [gone]").upstream, None, "its remote branch was deleted");
    }

    #[test]
    fn diff_lines_to_file_lines() {
        let d = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,3 +3,4 @@ fn\n ctx\n-old\n+new\n+more\n ctx2\n";
        assert_eq!(new_lines(d), [None, None, None, None, Some(2), None, Some(3), Some(4), Some(5)]);
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A repository with one commit, its name and email set.
    pub(crate) fn repo_at(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        sh(dir, &["init", "-q", "-b", "main"]);
        sh(dir, &["config", "user.email", "t@t"]);
        sh(dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("kept.rs"), "1\n").unwrap();
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-qm", "one"]);
    }

    /// Against real git, if there is one: the folder's own repository, and
    /// one in a folder below a folder that isn't one.
    #[test]
    fn real_repositories_own_and_below() {
        if which::which("git").is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        let app = root.join("app");
        repo_at(&app);
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::write(app.join("src/kept.rs"), "1").unwrap();
        sh(&app, &["add", "."]);
        sh(&app, &["commit", "-qm", "two"]);
        std::fs::write(app.join("src/kept.rs"), "2").unwrap();
        std::fs::write(app.join("fresh.rs"), "").unwrap();
        // A worktree tucked away in a hidden folder, 4 down from `root`.
        sh(&app, &["worktree", "add", "-q", "-b", "wt", ".claude/worktrees/x"]);
        let wt = app.join(".claude/worktrees/x");
        std::fs::write(wt.join("src/kept.rs"), "3").unwrap();
        let nothing = dir.path().join("nothing");
        std::fs::create_dir_all(&nothing).unwrap();
        assert!(status(&nothing, &find(&nothing)).is_none(), "no repository");
        let got = status(&root, &find(&root)).unwrap();
        assert_eq!(got.tree.get(&wt.join("src").join("kept.rs")), Some(&Change::Modified), "in the worktree");
        assert_eq!(got.tree.get(&app.join("src").join("kept.rs")), Some(&Change::Modified));
        assert_eq!(got.tree.get(&app.join("src")), Some(&Change::Modified));
        assert_eq!(got.tree.get(&app), Some(&Change::Modified));
        assert_eq!(got.tree.get(&app.join("fresh.rs")), Some(&Change::Untracked));
        let names: Vec<&str> = got.repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["app", "app/.claude/worktrees/x"], "the worktree a repository of its own");
        assert_eq!(got.repos[1].head.branch, "wt");
        // Opened at a folder inside the repository: its paths from there.
        let inner = status(&app.join("src"), &find(&app.join("src"))).unwrap();
        assert_eq!(inner.tree.get(&app.join("src").join("kept.rs")), Some(&Change::Modified));
        assert_eq!(inner.tree.get(&app.join("fresh.rs")), None, "outside the open folder");
        assert_eq!(inner.repos[0].scope, "src");
        assert_eq!(inner.repos[0].files.len(), 1);
    }

    /// Stage, unstage, discard, commit (what's staged, or everything), push
    /// to a branch that isn't there yet, pull: against a bare repository
    /// standing in for the remote.
    #[test]
    fn stage_commit_push_pull() {
        if which::which("git").is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (origin, a, b) = (dir.path().join("origin.git"), dir.path().join("a"), dir.path().join("b"));
        sh(dir.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        repo_at(&a);
        sh(&a, &["remote", "add", "origin", origin.to_str().unwrap()]);
        let now = |root: &Path| status(root, &find(root)).unwrap().repos.remove(0);

        let r = now(&a);
        assert_eq!((r.head.branch.as_str(), r.head.upstream.as_deref()), ("main", None));
        assert_eq!(apply(&r, &Op::Push), Ok("pushed".into()), "no upstream yet: made");
        assert_eq!(now(&a).head.upstream.as_deref(), Some("origin/main"));

        std::fs::write(a.join("kept.rs"), "2\n").unwrap();
        std::fs::write(a.join("new.rs"), "n\n").unwrap();
        let r = now(&a);
        assert_eq!(r.files.len(), 2);
        apply(&r, &Op::Stage(vec!["new.rs".into()])).unwrap();
        let r = now(&a);
        let new = r.files.iter().find(|f| f.rel == "new.rs").unwrap();
        assert_eq!((new.staged, new.unstaged), (Some('A'), None));
        apply(&r, &Op::Unstage(vec!["new.rs".into()])).unwrap();
        assert!(now(&a).files.iter().all(|f| f.staged.is_none()));

        // Nothing staged: commit takes everything.
        apply(&now(&a), &Op::Commit("both".into())).unwrap();
        let r = now(&a);
        assert!(r.files.is_empty());
        assert_eq!(r.head.ahead, 1);
        assert_eq!(log(&r, 5).iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["both", "one"]);
        assert!(show(&r, &log(&r, 1)[0].hash).unwrap().contains("+n"));
        apply(&r, &Op::Push).unwrap();
        assert_eq!(now(&a).head.ahead, 0);

        // Something staged: only that.
        std::fs::write(a.join("kept.rs"), "3\n").unwrap();
        std::fs::write(a.join("new.rs"), "n2\n").unwrap();
        apply(&now(&a), &Op::Stage(vec!["new.rs".into()])).unwrap();
        let d = diff(&now(&a), now(&a).files.iter().find(|f| f.rel == "new.rs").unwrap(), true).unwrap();
        assert!(d.contains("-n\n+n2"), "{d}");
        apply(&now(&a), &Op::Commit("just new".into())).unwrap();
        assert_eq!(now(&a).files.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(), ["kept.rs"]);

        // Discard: back to the last commit; an untracked file to the trash
        // (in tests, deleted).
        std::fs::write(a.join("junk.rs"), "j").unwrap();
        apply(&now(&a), &Op::Discard(vec!["kept.rs".into(), "junk.rs".into()])).unwrap();
        assert!(now(&a).files.is_empty());
        assert!(!a.join("junk.rs").exists());
        assert_eq!(std::fs::read_to_string(a.join("kept.rs")).unwrap(), "2\n");
        // An added file, discarded: out of the index, still on disk.
        std::fs::write(a.join("added.rs"), "x").unwrap();
        apply(&now(&a), &Op::Stage(vec!["added.rs".into()])).unwrap();
        apply(&now(&a), &Op::Discard(vec!["added.rs".into()])).unwrap();
        assert!(now(&a).files.iter().all(|f| f.untracked()));
        std::fs::remove_file(a.join("added.rs")).unwrap();

        // Another clone pushes; this one is behind, then pulls.
        apply(&now(&a), &Op::Push).unwrap();
        sh(dir.path(), &["clone", "-q", origin.to_str().unwrap(), b.to_str().unwrap()]);
        sh(&b, &["config", "user.email", "t@t"]);
        sh(&b, &["config", "user.name", "t"]);
        std::fs::write(b.join("from_b.rs"), "b").unwrap();
        apply(&now(&b), &Op::Commit("from b".into())).unwrap();
        apply(&now(&b), &Op::Push).unwrap();
        sh(&a, &["fetch", "-q"]);
        assert_eq!(now(&a).head.behind, 1);
        assert_eq!(apply(&now(&a), &Op::Pull), Ok("pulled".into()));
        assert!(a.join("from_b.rs").exists());
        // A failure says git's own words.
        let err = apply(&now(&a), &Op::Commit("nothing".into())).unwrap_err();
        assert!(!err.is_empty());
    }
}
