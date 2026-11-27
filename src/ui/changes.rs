//! Source Control (⇧⌘G): in the file tree's place, each repository in the
//! open folder (its own, and those in its folders) with its branch, how far
//! ahead or behind, its staged and unstaged files, and its recent commits.
//! One commit message at the top: ⇧⌘M commits the repository you're on,
//! ⌥⇧M every repository with changes; ⌥P / ⌥L push and pull, ⇧ for all.
//! Their keys work only here (a terminal's ⌥ chords are its program's).
//! Rows: ↑↓ move, space stages or unstages, ↵ shows a file's changes (a
//! read-only diff tab) or opens a repository's history, ← → fold; typing
//! types the message. Git runs off the window's thread (`git::apply`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use super::*;
use crate::git::{Commit, Op, Repo};

/// How each repository's git run went: its name, what was done, and git's answer.
type Done = Vec<(String, Op, Result<String, String>)>;

/// Commits shown in a repository's history.
const HISTORY: usize = 50;

/// The Changes view's own state.
#[derive(Default)]
pub struct Changes {
    /// The sidebar shows this, not the file tree.
    pub on: bool,
    pub message: String,
    /// With the keyboard here: in the message box, not walking the rows.
    pub typing: bool,
    /// The chosen row, by what it is (rows come and go as git answers),
    /// and where it was (the one there now, if it's gone).
    selected: Option<RowKey>,
    at: usize,
    /// Repositories folded shut, and those whose history is open (with it).
    folded: HashSet<PathBuf>,
    history: HashMap<PathBuf, Vec<Commit>>,
    /// Git at work: what it's doing, and its answers, one per repository.
    busy: Option<(String, Receiver<Done>)>,
    /// Scroll the chosen row into view; the list's scroll last frame.
    scroll: bool,
    offset: f32,
}

/// A row of the list: a repository, its staged / unstaged heading, a file
/// in one of them, its history heading, a commit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Row {
    Repo(usize),
    Section(usize, bool),
    File(usize, bool, usize),
    History(usize),
    Commit(usize, usize),
}

/// A row by what it shows, to find it again among new rows.
#[derive(Clone, Debug, PartialEq)]
enum RowKey {
    Repo(PathBuf),
    Section(PathBuf, bool),
    File(PathBuf, bool, String),
    History(PathBuf),
    Commit(PathBuf, String),
}

/// What a click or key in the list asks for, done after drawing.
enum Act {
    Select(usize),
    Open(usize),
    Stage(usize),
    Fold(usize, bool),
    Run(Command),
    /// A row's own button: the row, then the command on it.
    RowRun(usize, Command),
}

impl App {
    pub(crate) fn repos(&self) -> &[Repo] {
        self.project.as_ref().map_or(&[], |p| p.repos.as_slice())
    }

    /// Whether Source Control is on screen (in the sidebar), wherever the
    /// keyboard is: its keys work from the editor and a terminal then.
    pub(crate) fn changes_shown(&self) -> bool {
        self.changes.on && self.sidebar_shown()
    }

    /// Commit ▾'s actions, grouped, each with whether it can run now: the
    /// card's menu and its key's list (`Mode::Git`).
    pub(crate) fn git_menu(&self) -> crate::ui::MenuGroups {
        let busy = self.changes.busy.is_some();
        let repos = self.repos();
        let can = |cmd: Command| !busy && self.git_ready(cmd);
        let ahead: u32 = repos.iter().map(|r| r.head.ahead).sum();
        let here_ahead = self.current_repo().and_then(|r| repos.get(r)).map_or(0, |r| r.head.ahead);
        let staged_all = repos.iter().any(|r| r.files.iter().any(|f| f.unstaged.is_some()));
        vec![
            vec![(Command::GitCommit, "Commit", can(Command::GitCommit)), (Command::GitCommitAll, "Commit All", can(Command::GitCommitAll))],
            vec![
                (Command::GitPush, "Push", !busy && here_ahead > 0),
                (Command::GitPushAll, "Push All", can(Command::GitPushAll) && ahead > 0),
                (Command::GitPull, "Pull", can(Command::GitPull)),
                (Command::GitPullAll, "Pull All", can(Command::GitPullAll)),
            ],
            vec![(Command::GitStageAll, "Stage All", !busy && staged_all)],
        ]
    }

    /// Whether the keyboard is in the Changes view.
    pub(crate) fn changes_focused(&self) -> bool {
        self.changes.on && self.focus == Focus::Sidebar && self.sidebar_shown()
    }

    /// Whether commit / push / pull (or their "all") has something to do,
    /// wherever the keyboard is: Source Control's buttons show by this, its
    /// keys (`available`) only while it has the keyboard too.
    pub(crate) fn git_ready(&self, c: Command) -> bool {
        let Some(p) = self.project.as_ref() else { return false };
        match c {
            // The repository you're on (the row, the file in front's).
            Command::GitCommit => self.current_repo().and_then(|r| p.repos.get(r)).is_some_and(|r| !r.files.is_empty()),
            Command::GitPush | Command::GitPull => self.current_repo().is_some(),
            // "All": more than one repository to do it in.
            Command::GitCommitAll => p.repos.len() > 1 && p.repos.iter().any(|r| !r.files.is_empty()),
            Command::GitPushAll | Command::GitPullAll => p.repos.len() > 1,
            _ => false,
        }
    }

    /// The list's rows, top to bottom.
    pub(crate) fn change_rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for (r, repo) in self.repos().iter().enumerate() {
            out.push(Row::Repo(r));
            if self.changes.folded.contains(&repo.top) {
                continue;
            }
            for staged in [true, false] {
                let files: Vec<usize> = (0..repo.files.len())
                    .filter(|&i| in_section(repo, i, staged))
                    .collect();
                if files.is_empty() {
                    continue;
                }
                out.push(Row::Section(r, staged));
                out.extend(files.into_iter().map(|i| Row::File(r, staged, i)));
            }
            out.push(Row::History(r));
            if let Some(log) = self.changes.history.get(&repo.top) {
                out.extend((0..log.len()).map(|i| Row::Commit(r, i)));
            }
        }
        out
    }

    fn row_key(&self, row: Row) -> Option<RowKey> {
        let repos = self.repos();
        Some(match row {
            Row::Repo(r) => RowKey::Repo(repos.get(r)?.top.clone()),
            Row::Section(r, s) => RowKey::Section(repos.get(r)?.top.clone(), s),
            Row::File(r, s, i) => {
                let repo = repos.get(r)?;
                RowKey::File(repo.top.clone(), s, repo.files.get(i)?.rel.clone())
            }
            Row::History(r) => RowKey::History(repos.get(r)?.top.clone()),
            Row::Commit(r, i) => {
                let repo = repos.get(r)?;
                RowKey::Commit(
                    repo.top.clone(),
                    self.changes.history.get(&repo.top)?.get(i)?.hash.clone(),
                )
            }
        })
    }

    /// The chosen row's place in `rows`: where it is now, or the one in its
    /// old place if it's gone (staged: the next file down).
    fn chosen(&self, rows: &[Row]) -> Option<usize> {
        if rows.is_empty() {
            return None;
        }
        let key = self.changes.selected.as_ref();
        rows.iter()
            .position(|r| key.is_some() && self.row_key(*r).as_ref() == key)
            .or(Some(self.changes.at.min(rows.len() - 1)))
    }

    pub(crate) fn chosen_row(&self) -> Option<Row> {
        let rows = self.change_rows();
        self.chosen(&rows).and_then(|i| rows.get(i).copied())
    }

    /// Whether every repository is folded (with one, it counts too).
    pub(crate) fn all_folded(&self) -> bool {
        let repos = self.repos();
        !repos.is_empty() && repos.iter().all(|r| self.changes.folded.contains(&r.top))
    }

    /// Collapse All for Source Control: fold every repository (and close
    /// the histories); when all already are, open them all again.
    pub(crate) fn fold_all_repos(&mut self) {
        if self.all_folded() {
            self.changes.folded.clear();
        } else {
            let tops: Vec<PathBuf> = self.repos().iter().map(|r| r.top.clone()).collect();
            self.changes.folded.extend(tops);
            self.changes.history.clear();
        }
        let rows = self.change_rows();
        if let Some(up) = self.chosen_row().and_then(row_repo).and_then(|r| rows.iter().position(|x| *x == Row::Repo(r))) {
            self.choose(&rows, up);
        }
    }

    fn choose(&mut self, rows: &[Row], i: usize) {
        let Some(&row) = rows.get(i) else { return };
        self.changes.selected = self.row_key(row);
        self.changes.at = i;
        self.changes.scroll = true;
    }

    /// The repository git commands act on: the chosen row's in the Changes
    /// view, else the one the file in front is in, else the first with
    /// changes, else the first.
    pub(crate) fn current_repo(&self) -> Option<usize> {
        let repos = self.repos();
        if self.changes_focused()
            && let Some(
                Row::Repo(r)
                | Row::Section(r, _)
                | Row::File(r, _, _)
                | Row::History(r)
                | Row::Commit(r, _),
            ) = self.chosen_row()
        {
            return Some(r);
        }
        let file = self
            .active_tab()
            .and_then(|t| t.path().cloned().or_else(|| t.diff.as_ref()?.file.clone()));
        let inside = |repo: &Repo| {
            file.as_ref().is_some_and(|f| {
                repo.files.iter().any(|x| &x.path == f) || f.starts_with(&repo.top)
            })
        };
        // The deepest repository holding it (a worktree inside another).
        repos
            .iter()
            .enumerate()
            .filter(|(_, r)| inside(r))
            .max_by_key(|(_, r)| r.top.components().count())
            .map(|(i, _)| i)
            .or_else(|| repos.iter().position(|r| !r.files.is_empty()))
            .or_else(|| (!repos.is_empty()).then_some(0))
    }

    /// Into the Changes view (showing the sidebar if need be); again to go
    /// back to the editor.
    pub fn source_control(&mut self) {
        if self.changes_focused() {
            self.focus = Focus::Editor;
            return;
        }
        self.changes.on = true;
        if self.view().compact {
            self.view_mut().drawer = true;
        } else {
            self.config.settings.sidebar = true;
        }
        self.focus = Focus::Sidebar;
        self.sidebar.searching = false;
        self.changes.scroll = true;
        self.git_rescan();
    }

    /// Stage or unstage the row: a file, a heading's files, a repository's.
    pub(crate) fn stage_chosen(&mut self, row: Row, ctx: &egui::Context) {
        let Some(repo) = row_repo(row).and_then(|r| self.repos().get(r)).cloned() else {
            return;
        };
        let rels = |staged: bool| -> Vec<String> {
            (0..repo.files.len())
                .filter(|&i| in_section(&repo, i, staged))
                .filter_map(|i| Some(repo.files.get(i)?.rel.clone()))
                .collect()
        };
        let op = match row {
            Row::File(_, staged, i) => {
                let Some(f) = repo.files.get(i) else { return };
                if staged {
                    Op::Unstage(vec![f.rel.clone()])
                } else {
                    Op::Stage(vec![f.rel.clone()])
                }
            }
            Row::Section(_, true) => Op::Unstage(rels(true)),
            Row::Section(_, false) => Op::Stage(rels(false)),
            // A repository: all of it in, or (all in already) all out.
            Row::Repo(_) if rels(false).is_empty() => Op::Unstage(rels(true)),
            Row::Repo(_) => Op::StageAll,
            Row::History(_) | Row::Commit(..) => return,
        };
        self.run_git("Staging", vec![(repo, op)], ctx);
    }

    /// The files a discard would put back, and what to call them.
    fn discard_target(&self) -> Option<(Repo, Vec<String>, String)> {
        // From the editor, with Source Control showing: the file in front
        // (or the one its diff is of).
        if !self.changes_focused() {
            if !self.changes_shown() {
                return None;
            }
            let file = self.active_tab().and_then(|t| t.path().cloned().or_else(|| t.diff.as_ref()?.file.clone()))?;
            let repo = self.repos().iter().find(|r| r.files.iter().any(|f| f.path == file))?.clone();
            let rel = repo.files.iter().find(|f| f.path == file)?.rel.clone();
            return Some((repo, vec![rel.clone()], rel));
        }
        let row = self.chosen_row()?;
        let repo = self.repos().get(row_repo(row)?)?.clone();
        let (rels, what): (Vec<String>, String) = match row {
            Row::File(_, _, i) => {
                let f = repo.files.get(i)?;
                (vec![f.rel.clone()], f.rel.clone())
            }
            Row::Section(_, staged) => {
                let rels: Vec<String> = (0..repo.files.len())
                    .filter(|&i| in_section(&repo, i, staged))
                    .filter_map(|i| Some(repo.files.get(i)?.rel.clone()))
                    .collect();
                let what = format!(
                    "{} file{}",
                    rels.len(),
                    if rels.len() == 1 { "" } else { "s" }
                );
                (rels, what)
            }
            Row::Repo(_) => {
                let rels: Vec<String> = repo.files.iter().map(|f| f.rel.clone()).collect();
                let what = format!(
                    "every change in {} ({} file{})",
                    repo.name,
                    rels.len(),
                    if rels.len() == 1 { "" } else { "s" }
                );
                (rels, what)
            }
            Row::History(_) | Row::Commit(..) => return None,
        };
        (!rels.is_empty()).then_some((repo, rels, what))
    }

    /// Discard Changes…: asks first (`Ask::Discard`).
    pub(crate) fn ask_discard(&mut self) {
        if let Some((repo, rels, what)) = self.discard_target() {
            let untracked = repo
                .files
                .iter()
                .filter(|f| rels.contains(&f.rel) && f.untracked() && f.staged.is_none())
                .count();
            self.ask = Some(crate::app::Ask::Discard {
                repo: Box::new(repo),
                rels,
                what,
                untracked,
            });
        }
    }

    pub(crate) fn discard(&mut self, repo: Repo, rels: Vec<String>, ctx: &egui::Context) {
        self.run_git("Discarding", vec![(repo, Op::Discard(rels))], ctx);
    }

    /// Commit the current repository (⇧⌘M), or every one with changes (⌥⇧M),
    /// with the message at the top; none typed yet: there to type it.
    pub(crate) fn commit(&mut self, all: bool, ctx: &egui::Context) {
        let message = self.changes.message.trim().to_string();
        if message.is_empty() {
            if !self.changes_focused() {
                self.source_control();
            }
            self.changes.typing = true;
            let key = self
                .shortcut(if all {
                    Command::GitCommitAll
                } else {
                    Command::GitCommit
                })
                .unwrap_or_default();
            return self.notify(format!("Type a commit message, then {key}"));
        }
        let op = Op::Commit(message);
        let jobs = self.jobs(&op, all);
        if jobs.is_empty() {
            return self.notify("Nothing to commit");
        }
        self.run_git("Committing", jobs, ctx);
    }

    /// Push or pull the current repository, or (`all`) every one.
    pub(crate) fn sync(&mut self, op: Op, all: bool, ctx: &egui::Context) {
        let what = if op == Op::Push { "Pushing" } else { "Pulling" };
        let jobs = self.jobs(&op, all);
        if jobs.is_empty() {
            return self.notify(if op == Op::Push {
                "Nothing to push"
            } else {
                "Nothing to pull"
            });
        }
        self.run_git(what, jobs, ctx);
    }

    /// Every repository's changes into its index.
    pub(crate) fn stage_everything(&mut self, ctx: &egui::Context) {
        let jobs = self.jobs(&Op::StageAll, true);
        self.run_git("Staging", jobs, ctx);
    }

    /// `op` for the current repository, or every one it applies to.
    fn jobs(&self, op: &Op, all: bool) -> Vec<(Repo, Op)> {
        let repos = self.repos();
        let picked: Vec<&Repo> = if all {
            repos.iter().collect()
        } else {
            self.current_repo()
                .and_then(|r| repos.get(r))
                .into_iter()
                .collect()
        };
        picked
            .into_iter()
            .filter(|r| op.applies(r))
            .map(|r| (r.clone(), op.clone()))
            .collect()
    }

    /// Run `jobs` off this thread, one repository each, side by side;
    /// `poll_git_ops` says how it went.
    fn run_git(&mut self, what: &str, jobs: Vec<(Repo, Op)>, ctx: &egui::Context) {
        if jobs.is_empty() {
            return;
        }
        if let Some((doing, _)) = &self.changes.busy {
            return self.notify(format!("Still {}…", doing.to_lowercase()));
        }
        tracing::info!(what, repos = ?jobs.iter().map(|(r, _)| r.name.as_str()).collect::<Vec<_>>(), "git");
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let handles: Vec<_> = jobs
                .into_iter()
                .map(|(repo, op)| {
                    std::thread::spawn(move || {
                        let got = crate::git::apply(&repo, &op);
                        (repo.name, op, got)
                    })
                })
                .collect();
            let out: Vec<_> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
            let _ = tx.send(out);
            ctx.request_repaint();
        });
        self.changes.busy = Some((what.to_string(), rx));
    }

    /// Git's answers in: say how it went, clear a message that's been
    /// committed, ask git what's what now.
    pub(crate) fn poll_git_ops(&mut self) {
        let Some(done) = self
            .changes
            .busy
            .as_ref()
            .and_then(|(_, rx)| rx.try_recv().ok())
        else {
            return;
        };
        self.changes.busy = None;
        self.git_changed();
        let ok: Vec<&(String, Op, Result<String, String>)> =
            done.iter().filter(|d| d.2.is_ok()).collect();
        let failed: Vec<&(String, Op, Result<String, String>)> =
            done.iter().filter(|d| d.2.is_err()).collect();
        let committed = done.iter().any(|d| matches!(d.1, Op::Commit(_)));
        if committed && failed.is_empty() {
            self.changes.message.clear();
        }
        // Histories open show the new commits.
        let open: Vec<PathBuf> = self.changes.history.keys().cloned().collect();
        for top in open {
            if let Some(repo) = self.repos().iter().find(|r| r.top == top).cloned() {
                self.changes
                    .history
                    .insert(top, crate::git::log(&repo, HISTORY));
            }
        }
        if let Some((_, op, _)) = done.first().filter(|_| !ok.is_empty()) {
            let done_word = op.done();
            let mut word = done_word.chars();
            let cap: String = word
                .next()
                .map(|f| f.to_uppercase().chain(word).collect())
                .unwrap_or_default();
            self.notify(match (ok.as_slice(), done.len()) {
                ([one], 1) => format!("{cap} {}", one.0),
                _ => format!("{cap} {} of {} repositories", ok.len(), done.len()),
            });
        }
        for (name, _, err) in failed {
            let err = err.as_ref().err().map(String::as_str).unwrap_or_default();
            // Git says the useful part first; its hints after.
            let said: Vec<&str> = err
                .lines()
                .filter(|l| !l.starts_with("hint:"))
                .take(3)
                .collect();
            self.warn(format!("{name}: {}", said.join(" ")));
        }
    }

    /// A file's changes (or a commit's) in a read-only tab: one per thing
    /// shown, reused when shown again.
    fn show_row(&mut self, row: Row) {
        let Some(repo) = row_repo(row).and_then(|r| self.repos().get(r)).cloned() else {
            return;
        };
        let (title, file, got) = match row {
            Row::File(_, staged, i) => {
                let Some(f) = repo.files.get(i) else { return };
                let name = f
                    .path
                    .file_name()
                    .map_or_else(|| f.rel.clone(), |n| n.to_string_lossy().into_owned());
                let title = format!("{name} ({})", if staged { "staged" } else { "changes" });
                (
                    title,
                    Some(f.path.clone()),
                    crate::git::diff(&repo, f, staged),
                )
            }
            Row::Commit(_, i) => {
                let Some(c) = self
                    .changes
                    .history
                    .get(&repo.top)
                    .and_then(|h| h.get(i))
                    .cloned()
                else {
                    return;
                };
                (
                    format!("{} {}", c.hash, c.subject),
                    None,
                    crate::git::show(&repo, &c.hash),
                )
            }
            _ => return,
        };
        match got {
            Ok(text) if text.trim().is_empty() => self.notify("No changes to show"),
            Ok(text) => self.show_diff(&title, file, &text),
            Err(e) => self.warn(format!("{}: {e}", repo.name)),
        }
    }

    fn show_diff(&mut self, title: &str, file: Option<PathBuf>, text: &str) {
        let tab = crate::editor::Tab::diff(title, file, text);
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.diff.as_ref().is_some_and(|d| d.title == title))
        {
            if let Some(t) = self.tabs.get_mut(i) {
                *t = tab;
            }
            self.active = Some(i);
        } else {
            let at = self
                .active
                .map_or(self.tabs.len(), |a| a + 1)
                .max(self.pinned(false));
            self.tabs.insert(at, tab);
            self.active = Some(at);
        }
        self.views.iter_mut().for_each(|v| v.tabs.reveal_again());
    }

    /// Open the file behind the chosen row, or behind the diff in front (at
    /// the line the caret's on).
    /// The changed file on the row you're on, if it's one.
    pub(crate) fn chosen_file(&self) -> Option<std::path::PathBuf> {
        let Some(Row::File(r, _, i)) = self.chosen_row() else { return None };
        self.repos().get(r)?.files.get(i).map(|f| f.path.clone())
    }

    pub(crate) fn open_changed_file(&mut self) {
        if self.changes_focused() {
            if let Some(path) = self.chosen_file() {
                self.open_file(&path);
                self.focus = Focus::Editor;
            }
            return;
        }
        let Some((file, line)) = self.active_tab().and_then(|t| {
            let d = t.diff.as_ref()?;
            // The caret's line, or the nearest one below it that's in the file.
            let row = t.buf.cursor_row();
            let line = d
                .lines
                .iter()
                .skip(row)
                .flatten()
                .next()
                .or_else(|| d.lines.iter().take(row).flatten().last())
                .copied();
            Some((d.file.clone()?, line))
        }) else {
            return;
        };
        self.open_file(&file);
        if let (Some(line), Some(t)) = (line, self.active_tab_mut()) {
            t.go_to(line, 0..0);
        }
        self.focus = Focus::Editor;
    }

    pub(crate) fn changed_file_target(&self) -> bool {
        if self.changes_focused() {
            return matches!(self.chosen_row(), Some(Row::File(..)));
        }
        self.active_tab()
            .is_some_and(|t| t.diff.as_ref().is_some_and(|d| d.file.is_some()))
    }

    pub(crate) fn can_stage(&self) -> bool {
        self.changes_focused()
            && match self.chosen_row() {
                Some(Row::File(..) | Row::Section(..)) => true,
                Some(Row::Repo(r)) => self.repos().get(r).is_some_and(|r| !r.files.is_empty()),
                _ => false,
            }
    }

    pub(crate) fn can_discard(&self) -> bool {
        self.discard_target().is_some()
    }

    /// Keys in the list (the message box has its own while typing).
    fn changes_keys(&mut self, ctx: &egui::Context) -> Option<Act> {
        if !self.changes_focused() || self.palette.is_some() || self.modal_open() {
            return None;
        }
        let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        if self.changes.typing {
            if pressed(Key::ArrowDown) || pressed(Key::Escape) {
                self.changes.typing = false;
                self.changes.scroll = true;
            }
            return None;
        }
        let rows = self.change_rows();
        let at = self.chosen(&rows);
        let row = at.and_then(|i| rows.get(i).copied());
        let last = rows.len().saturating_sub(1);
        if pressed(Key::ArrowDown) {
            return Some(Act::Select(at.map_or(0, |i| (i + 1).min(last))));
        }
        if pressed(Key::ArrowUp) {
            if at.is_none_or(|i| i == 0) {
                self.changes.typing = true;
                return None;
            }
            return Some(Act::Select(at.map_or(0, |i| i.saturating_sub(1))));
        }
        if pressed(Key::Home) {
            return Some(Act::Select(0));
        }
        if pressed(Key::End) {
            return Some(Act::Select(last));
        }
        if pressed(Key::ArrowRight) {
            return at.map(|i| Act::Fold(i, false));
        }
        if pressed(Key::ArrowLeft) {
            return at.map(|i| Act::Fold(i, true));
        }
        if pressed(Key::Space) {
            ctx.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Text(t) if t == " "))
            });
            return at.map(Act::Stage);
        }
        if pressed(Key::Enter) {
            return at.map(Act::Open);
        }
        if pressed(Key::Escape) {
            self.focus = Focus::Editor;
            return None;
        }
        // Typing in the rows types the message: what was typed goes in now
        // (the box has the keys from the next frame on).
        let typed: String = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) | egui::Event::Paste(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect()
        });
        if row.is_some() && !typed.is_empty() {
            self.changes.message.push_str(&typed);
            self.changes.typing = true;
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_) | egui::Event::Paste(_))));
        }
        None
    }

    fn changes_act(&mut self, act: Option<Act>, ctx: &egui::Context) {
        let rows = self.change_rows();
        match act {
            Some(Act::Select(i)) => self.choose(&rows, i),
            Some(Act::Open(i)) => {
                self.choose(&rows, i);
                match rows.get(i).copied() {
                    Some(row @ (Row::File(..) | Row::Commit(..))) => self.show_row(row),
                    Some(Row::Repo(_) | Row::History(_)) => {
                        let shut = self.is_folded(rows.get(i).copied());
                        self.changes_act(Some(Act::Fold(i, !shut)), ctx);
                    }
                    _ => {}
                }
            }
            Some(Act::Stage(i)) => {
                self.choose(&rows, i);
                if let Some(row) = rows.get(i).copied() {
                    self.stage_chosen(row, ctx);
                }
            }
            Some(Act::Fold(i, shut)) => {
                let Some(row) = rows.get(i).copied() else {
                    return;
                };
                let Some(repo) = row_repo(row).and_then(|r| self.repos().get(r)).cloned() else {
                    return;
                };
                match row {
                    Row::Repo(_) if shut => {
                        self.changes.folded.insert(repo.top);
                    }
                    Row::Repo(_) => {
                        self.changes.folded.remove(&repo.top);
                    }
                    Row::History(_) if shut => {
                        self.changes.history.remove(&repo.top);
                    }
                    Row::History(_) => {
                        let log = crate::git::log(&repo, HISTORY);
                        self.changes.history.insert(repo.top, log);
                    }
                    // Elsewhere ← goes up to the repository.
                    _ if shut => {
                        if let Some(up) = rows
                            .iter()
                            .position(|r| *r == Row::Repo(row_repo(row).unwrap_or_default()))
                        {
                            self.choose(&rows, up);
                        }
                    }
                    _ => {}
                }
            }
            Some(Act::Run(c)) => self.run(c, ctx),
            Some(Act::RowRun(i, c)) => {
                self.choose(&rows, i);
                self.run(c, ctx);
            }
            None => {}
        }
    }

    fn is_folded(&self, row: Option<Row>) -> bool {
        let top = |r: usize| {
            self.repos()
                .get(r)
                .map(|r| r.top.clone())
                .unwrap_or_default()
        };
        match row {
            Some(Row::Repo(r)) => self.changes.folded.contains(&top(r)),
            Some(Row::History(r)) => !self.changes.history.contains_key(&top(r)),
            _ => false,
        }
    }

    /// The view: the message box, then the list.
    pub(super) fn changes_ui(&mut self, ui: &mut egui::Ui, focused: bool) {
        let ctx = ui.ctx().clone();
        self.poll_git_ops();
        let mut act = self.changes_keys(&ctx);
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);

        // The compose card, as a chat box: the message on top; under it
        // what a commit takes in, Commit (its ▾: the rest, with their keys)
        // and, once there's something to push, ↑ and how many commits.
        // The message has the keys whenever the view does and you're
        // typing; typing in the rows lands here.
        let typing = focused && self.changes.typing;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), 88.0 * s), Sense::hover());
        let card = Rect::from_min_max(area.min + vec2(10.0 * s, 3.0 * s), area.max - vec2(10.0 * s, 9.0 * s));
        ui.painter().rect(
            card,
            9.0 * s,
            c.background,
            Stroke::new(1.0, if typing { c.accent.gamma_multiply(0.7) } else { c.border }),
            StrokeKind::Inside,
        );
        let field = card.with_max_y(card.top() + 34.0 * s);
        ui.painter().text(
            pos2(field.left() + 14.0 * s, field.center().y),
            Align2::CENTER_CENTER,
            icon::GIT_COMMIT,
            pt.font(12.5, Weight::Regular),
            c.text_subtle,
        );
        let busy = self
            .changes
            .busy
            .as_ref()
            .map(|(what, _)| format!("{what}…"));
        let hint = egui::RichText::new("Commit message")
            .color(c.text_subtle)
            .font(pt.font(12.5, Weight::Regular));
        let edit = egui::TextEdit::singleline(&mut self.changes.message)
            .id(egui::Id::unique("commit-message"))
            .frame(egui::Frame::NONE)
            .font(pt.font(13.0, Weight::Regular))
            .text_color(c.text)
            .hint_text(hint)
            .align(Align2::LEFT_CENTER)
            .desired_width(field.width() - 36.0 * s);
        let caret = ui.visuals().text_cursor.clone();
        if !typing {
            ui.visuals_mut().text_cursor.stroke.color = Color32::TRANSPARENT;
        }
        let resp = ui.put(field.with_min_x(field.left() + 26.0 * s).shrink2(vec2(0.0, 4.0 * s)), edit);
        ui.visuals_mut().text_cursor = caret;
        if typing {
            resp.request_focus();
        } else if resp.has_focus() {
            resp.surrender_focus();
        }
        if resp.clicked() || resp.gained_focus() {
            self.focus = Focus::Sidebar;
            self.changes.typing = true;
        }
        ui.painter().hline(card.x_range().shrink(1.0), field.bottom(), Stroke::new(1.0, c.border));

        let repos = self.repos().to_vec();
        let here = self.current_repo().and_then(|r| repos.get(r));
        let tip = |cmd: Command| self.tip_keys(cmd);
        let can = |cmd: Command| busy.is_none() && self.git_ready(cmd);
        let changed = repos.iter().filter(|r| !r.files.is_empty()).count();
        let files: usize = repos.iter().map(|r| r.files.len()).sum();
        let ahead: u32 = repos.iter().map(|r| r.head.ahead).sum();
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let info = match &busy {
            Some(b) => b.clone(),
            None if changed == 0 => "Nothing to commit".into(),
            None if repos.len() == 1 => plural(files, "file", "files"),
            None => format!("{} · {}", plural(changed, "repository", "repositories"), plural(files, "file", "files")),
        };
        // Short of room: just the files.
        let short = if changed > 0 && busy.is_none() { plural(files, "file", "files") } else { info.clone() };
        // Commit: the repository you're on (its name in the tip), or, with
        // nothing there, every one.
        let commit = if can(Command::GitCommit) || !can(Command::GitCommitAll) { Command::GitCommit } else { Command::GitCommitAll };
        let commit_tip = match (commit, here) {
            (Command::GitCommit, Some(r)) => format!("Commit {}{}", r.name, tip(commit)),
            _ => format!("Commit every repository with changes ({changed}){}", tip(commit)),
        };
        // Push: the one you're on when only it has commits to push, else all.
        let here_ahead = here.map_or(0, |r| r.head.ahead);
        let push = if here_ahead == ahead || repos.len() < 2 { Command::GitPush } else { Command::GitPushAll };
        let push_tip = match push {
            Command::GitPush => format!("Push {} ({}){}", here.map_or("", |r| r.name.as_str()), plural(ahead as usize, "commit", "commits"), tip(push)),
            _ => format!("Push every repository with commits to push ({}){}", plural(ahead as usize, "commit", "commits"), tip(push)),
        };
        let foot = card.with_min_y(field.bottom()).shrink2(vec2(6.0 * s, 0.0));
        let h = 26.0 * s;
        let y = foot.center().y - h / 2.0;
        let r = (7.0 * s) as u8;
        let label = |text: String, color: Color32| egui::RichText::new(text).font(pt.font(12.5, Weight::Medium)).color(color);
        let width = |ui: &egui::Ui, text: &str| ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), pt.font(12.5, Weight::Medium), c.text).size().x);
        let mut right = foot.right();
        if ahead > 0 && busy.is_none() {
            let text = format!("↑ {ahead}");
            let w = width(ui, &text) + 20.0 * s;
            let at = Rect::from_min_size(pos2(right - w, y), vec2(w, h));
            let b = egui::Button::new(label(text, c.text)).fill(c.surface).stroke(Stroke::new(1.0, c.border)).corner_radius(r);
            let resp = ui.put(at, b);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if push == Command::GitPush { "Push" } else { "Push All" }));
            if resp.on_hover_text(push_tip).clicked() {
                act = Some(Act::Run(push));
            }
            right = at.left() - 6.0 * s;
        }
        let title = if commit == Command::GitCommit { "Commit" } else { "Commit All" };
        let caret_w = 22.0 * s;
        let w = width(ui, title) + 24.0 * s;
        let main = Rect::from_min_size(pos2(right - caret_w - w, y), vec2(w, h));
        let more = Rect::from_min_size(pos2(main.right(), y), vec2(caret_w, h));
        let on = can(commit);
        let fill = if on { c.accent } else { c.surface };
        let ink = if on { c.accent_text } else { c.text_subtle };
        let b = egui::Button::new(label(title.into(), ink))
            .fill(fill)
            .stroke(Stroke::new(1.0, if on { c.accent } else { c.border }))
            .corner_radius(egui::CornerRadius { nw: r, sw: r, ne: 0, se: 0 });
        if ui.add_enabled_ui(on, |ui| ui.put(main, b)).inner.on_hover_text(commit_tip).clicked() {
            act = Some(Act::Run(commit));
        }
        let b = egui::Button::new(label("▾".into(), ink))
            .fill(fill)
            .stroke(Stroke::new(1.0, if on { c.accent } else { c.border }))
            .corner_radius(egui::CornerRadius { nw: 0, sw: 0, ne: r, se: r });
        let caret_resp = ui.put(more, b);
        caret_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "More Git Actions"));
        // A line between the two halves.
        ui.painter().vline(more.left(), more.y_range().shrink(5.0 * s), Stroke::new(1.0, if on { c.accent_text.gamma_multiply(0.4) } else { c.border }));
        let groups = self.git_menu();
        let key = |c: Command| self.config.keymap.combos(c).next().map(|k| k.label());
        let more_tip = format!("More: commit all, push, pull, stage all{}", self.tip_keys(Command::GitMenu));
        if let Some(picked) = egui::Popup::menu(&caret_resp.on_hover_text(more_tip))
            .show(|ui| crate::ui::command_menu(ui, &groups, &key))
            .and_then(|r| r.inner)
        {
            act = Some(Act::Run(picked));
        }
        // What a commit takes in, in what room is left.
        let room = (main.left() - foot.left() - 12.0 * s).max(0.0);
        let fits = ui.fonts_mut(|f| f.layout_no_wrap(info.clone(), pt.font(11.5, Weight::Regular), c.text_subtle).size().x) <= room;
        let info = if fits { info } else { short };
        let mut job = LayoutJob::simple_singleline(info, pt.font(11.5, Weight::Regular), c.text_subtle);
        job.wrap = egui::text::TextWrapping { max_width: room, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(pos2(foot.left() + 6.0 * s, foot.center().y - g.size().y / 2.0), g, c.text_subtle);
        // (`put` left the cursor under the last button, inside the card.)
        ui.advance_cursor_after_rect(area);
        let bottom = 0.0;

        let rows = self.change_rows();
        let chosen = self.chosen(&rows);
        if repos.is_empty() {
            let found = self
                .project
                .as_ref()
                .is_some_and(|p| p.git_found || p.git_job.is_some());
            let note = if found {
                "Asking git…"
            } else {
                "No git repository in this folder"
            };
            ui.add_space(16.0 * s);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(note)
                        .font(pt.font(12.5, Weight::Regular))
                        .color(c.text_subtle),
                );
            });
            return self.changes_act(act, &ctx);
        }
        let row_h = if self.view().compact { 34.0 } else { 26.0 } * s;
        let mut area = egui::ScrollArea::vertical()
            .auto_shrink(false)
            .max_height(ui.available_height() - bottom)
            .id_salt("changes-rows");
        if self.changes.scroll
            && let Some(i) = chosen
        {
            let (top, bottom) = (i as f32 * row_h, (i + 1) as f32 * row_h);
            let (off, view) = (self.changes.offset, ui.available_height() - bottom);
            if top < off {
                area = area.vertical_scroll_offset(top);
            } else if bottom > off + view && view > 0.0 {
                area = area.vertical_scroll_offset(bottom - view);
            }
        }
        let histories = self.changes.history.clone();
        let folded = self.changes.folded.clone();
        let walking = focused && !self.changes.typing;
        let out = area.show_rows(ui, row_h, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let Some(&row) = rows.get(i) else { continue };
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                let p = ui.painter();
                let bg = r.shrink2(vec2(6.0 * s, 1.0 * s));
                let is_sel = chosen == Some(i);
                if is_sel && walking {
                    p.rect(
                        bg,
                        6.0 * s,
                        c.accent.gamma_multiply(0.16),
                        Stroke::new(1.0, c.accent.gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                } else if is_sel {
                    p.rect_filled(bg, 6.0 * s, c.hover);
                } else if resp.hovered() {
                    p.rect_filled(bg, 6.0 * s, c.hover.gamma_multiply(0.6));
                }
                let x0 = r.left() + 14.0 * s;
                let y = r.center().y;
                let small = pt.font(12.0, Weight::Regular);
                let text = |p: &egui::Painter,
                            x: f32,
                            t: &str,
                            font: FontId,
                            color: Color32,
                            room: f32| {
                    let mut job = LayoutJob::simple_singleline(t.to_string(), font, color);
                    job.wrap = egui::text::TextWrapping {
                        max_width: room.max(0.0),
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let g = ui.fonts_mut(|f| f.layout_job(job));
                    let w = g.size().x;
                    p.galley(pos2(x, y - g.size().y / 2.0), g, color);
                    w
                };
                let right = |p: &egui::Painter, t: &str, color: Color32| {
                    p.text(
                        pos2(r.right() - 14.0 * s, y),
                        Align2::RIGHT_CENTER,
                        t,
                        small.clone(),
                        color,
                    )
                    .width()
                };
                // The row under the mouse (or the keyboard's) shows what can
                // be done to it as buttons, as VS Code has them, beside its
                // letter or count; their keys are in their tips.
                let staged_row = matches!(row, Row::File(_, true, _) | Row::Section(_, true));
                let with_files = row_repo(row).and_then(|n| repos.get(n)).is_some_and(|r| !r.files.is_empty());
                let stage = if staged_row { (icon::REMOVE, "Unstage") } else { (icon::ADD, "Stage") };
                let acts: Vec<(Command, &str, &str)> = match row {
                    Row::File(..) if staged_row => vec![(Command::GitOpenFile, icon::GO_TO_FILE, "Open File"), (Command::GitStage, stage.0, stage.1)],
                    Row::File(..) => vec![(Command::GitOpenFile, icon::GO_TO_FILE, "Open File"), (Command::GitDiscard, icon::UNDO, "Discard Changes"), (Command::GitStage, stage.0, stage.1)],
                    Row::Section(_, true) => vec![(Command::GitStage, stage.0, "Unstage All")],
                    Row::Section(..) => vec![(Command::GitDiscard, icon::UNDO, "Discard All Changes"), (Command::GitStage, stage.0, "Stage All")],
                    Row::Repo(_) if with_files => vec![(Command::GitDiscard, icon::UNDO, "Discard All Changes"), (Command::GitStage, icon::ADD, "Stage All")],
                    _ => Vec::new(),
                };
                let lit = ui.rect_contains_pointer(r) || (is_sel && walking);
                // The name leaves them room.
                let reserve = if lit && !acts.is_empty() { acts.len() as f32 * 22.0 * s + 6.0 * s } else { 0.0 };
                // How wide the row's right-hand text is (letter, count).
                let mut tail = 0.0;
                match row {
                    Row::Repo(n) => {
                        let Some(repo) = repos.get(n) else { continue };
                        let shut = folded.contains(&repo.top);
                        p.text(
                            pos2(x0 + 6.0 * s, y),
                            Align2::CENTER_CENTER,
                            if shut {
                                icon::CHEVRON_RIGHT
                            } else {
                                icon::CHEVRON_DOWN
                            },
                            small.clone(),
                            c.text_subtle,
                        );
                        p.text(
                            pos2(x0 + 24.0 * s, y),
                            Align2::CENTER_CENTER,
                            icon::REPO,
                            pt.font(14.0, Weight::Regular),
                            c.accent,
                        );
                        let mut status = String::new();
                        if repo.head.ahead > 0 {
                            status.push_str(&format!("↑{} ", repo.head.ahead));
                        }
                        if repo.head.behind > 0 {
                            status.push_str(&format!("↓{} ", repo.head.behind));
                        }
                        let count = if repo.files.is_empty() {
                            "clean".to_string()
                        } else {
                            repo.files.len().to_string()
                        };
                        status.push_str(&count);
                        let w = right(
                            p,
                            &status,
                            if repo.files.is_empty() {
                                c.text_subtle
                            } else {
                                c.text_muted
                            },
                        );
                        tail = w;
                        let lx = x0 + 36.0 * s;
                        let room = r.right() - 14.0 * s - w - 10.0 * s - lx - reserve;
                        let nw = text(
                            p,
                            lx,
                            &repo.name,
                            pt.font(13.0, Weight::Medium),
                            c.text,
                            room,
                        );
                        text(
                            p,
                            lx + nw + 8.0 * s,
                            &repo.head.branch,
                            small.clone(),
                            c.text_subtle,
                            room - nw - 8.0 * s,
                        );
                        resp.clone().on_hover_text(format!(
                            "{}\n{}{}",
                            crate::ui::tilde(&repo.top.to_string_lossy()),
                            repo.head.branch,
                            repo.head
                                .upstream
                                .as_ref()
                                .map(|u| format!(" → {u}"))
                                .unwrap_or_else(|| " (not pushed yet)".into())
                        ));
                    }
                    Row::Section(n, staged) => {
                        let Some(repo) = repos.get(n) else { continue };
                        let count = (0..repo.files.len())
                            .filter(|&f| in_section(repo, f, staged))
                            .count();
                        let label = if staged { "STAGED" } else { "CHANGES" };
                        text(
                            p,
                            x0 + 20.0 * s,
                            label,
                            pt.font(11.0, Weight::SemiBold),
                            c.text_subtle,
                            r.width(),
                        );
                        tail = right(p, &count.to_string(), c.text_subtle);
                    }
                    Row::File(n, staged, f) => {
                        let Some(file) = repos.get(n).and_then(|repo| repo.files.get(f)) else {
                            continue;
                        };
                        let letter =
                            if staged { file.staged } else { file.unstaged }.unwrap_or(' ');
                        let tint = match letter {
                            '?' | 'A' => c.ansi_2,
                            'D' => c.ansi_1,
                            '!' => c.error,
                            _ => c.ansi_3,
                        };
                        let (dir, name) = file
                            .rel
                            .trim_end_matches('/')
                            .rsplit_once('/')
                            .unwrap_or(("", file.rel.as_str()));
                        let (glyph, gtint) = icon::for_file(name);
                        p.text(
                            pos2(x0 + 30.0 * s, y),
                            Align2::CENTER_CENTER,
                            glyph,
                            pt.font(14.0, Weight::Regular),
                            gtint.unwrap_or(c.text_subtle),
                        );
                        let shown = if letter == '?' { 'U' } else { letter };
                        let w = right(p, &shown.to_string(), tint);
                        tail = w;
                        let lx = x0 + 42.0 * s;
                        let room = r.right() - 14.0 * s - w - 10.0 * s - lx - reserve;
                        let struck = if letter == 'D' { c.text_muted } else { c.text };
                        let nw = text(p, lx, name, pt.font(13.0, Weight::Regular), struck, room);
                        text(
                            p,
                            lx + nw + 6.0 * s,
                            dir,
                            small.clone(),
                            c.text_subtle,
                            room - nw - 6.0 * s,
                        );
                    }
                    Row::History(n) => {
                        let Some(repo) = repos.get(n) else { continue };
                        let open = histories.contains_key(&repo.top);
                        p.text(
                            pos2(x0 + 20.0 * s, y),
                            Align2::CENTER_CENTER,
                            if open {
                                icon::CHEVRON_DOWN
                            } else {
                                icon::CHEVRON_RIGHT
                            },
                            small.clone(),
                            c.text_subtle,
                        );
                        text(
                            p,
                            x0 + 32.0 * s,
                            "HISTORY",
                            pt.font(11.0, Weight::SemiBold),
                            c.text_subtle,
                            r.width(),
                        );
                    }
                    Row::Commit(n, k) => {
                        let Some(commit) = repos
                            .get(n)
                            .and_then(|repo| histories.get(&repo.top))
                            .and_then(|h| h.get(k))
                        else {
                            continue;
                        };
                        p.text(
                            pos2(x0 + 30.0 * s, y),
                            Align2::CENTER_CENTER,
                            icon::GIT_COMMIT,
                            pt.font(13.0, Weight::Regular),
                            c.text_subtle,
                        );
                        let w = right(p, &commit.when, c.text_subtle);
                        let lx = x0 + 42.0 * s;
                        text(
                            p,
                            lx,
                            &commit.subject,
                            pt.font(13.0, Weight::Regular),
                            c.text_muted,
                            r.right() - 14.0 * s - w - 10.0 * s - lx,
                        );
                        resp.clone().on_hover_text(format!(
                            "{} · {} · {}",
                            commit.hash, commit.author, commit.when
                        ));
                    }
                }
                if lit && !acts.is_empty() {
                    let size = 22.0 * s;
                    let end = r.right() - 14.0 * s - tail - 6.0 * s;
                    let start = end - acts.len() as f32 * size;
                    for (k, (cmd, glyph, title)) in acts.iter().enumerate() {
                        let at = Rect::from_center_size(pos2(start + (k as f32 + 0.5) * size, y), vec2(size, size));
                        let b = ui.interact(at, egui::Id::unique(("change-act", i, k)), Sense::click());
                        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, *title));
                        if b.hovered() {
                            p.rect_filled(at, 5.0 * s, c.surface);
                        }
                        p.text(at.center(), Align2::CENTER_CENTER, *glyph, pt.font(14.0, Weight::Regular), if b.hovered() { c.text } else { c.text_muted });
                        let key = self.shortcut(*cmd).or_else(|| (*cmd == Command::GitStage).then(|| "Space".to_string()));
                        if b.on_hover_text(format!("{title}{}", key.map(|k| format!("  {k}")).unwrap_or_default())).clicked() {
                            self.focus = Focus::Sidebar;
                            self.changes.typing = false;
                            act = Some(Act::RowRun(i, *cmd));
                        }
                    }
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    self.focus = Focus::Sidebar;
                    self.changes.typing = false;
                    act = Some(Act::Open(i));
                }
                if resp.secondary_clicked() {
                    self.focus = Focus::Sidebar;
                    self.changes.typing = false;
                    act = Some(Act::Select(i));
                }
                resp.context_menu(|ui| {
                    if let Some(cmd) = self.changes_menu(ui, row) {
                        act = Some(Act::Run(cmd));
                    }
                });
            }
        });
        self.changes.offset = out.state.clamped_offset().y;
        self.changes.scroll = false;
        self.changes_act(act, &ctx);
    }

    /// The right-click menu: what can be done to the row, with keys.
    fn changes_menu(&self, ui: &mut egui::Ui, row: Row) -> Option<Command> {
        let mut chosen = None;
        let mut item = |ui: &mut egui::Ui, cmd: Command, title: &str| {
            let keys = self
                .config
                .keymap
                .combo_for(cmd)
                .map(|k| k.label())
                .unwrap_or_else(|| {
                    if cmd == Command::GitStage {
                        "Space".into()
                    } else {
                        String::new()
                    }
                });
            if ui
                .add(egui::Button::new(title).shortcut_text(keys))
                .clicked()
            {
                chosen = Some(cmd);
                ui.close();
            }
        };
        let staged = matches!(row, Row::File(_, true, _) | Row::Section(_, true));
        match row {
            Row::File(..) => {
                item(ui, Command::GitOpenFile, "Open File");
                item(
                    ui,
                    Command::GitStage,
                    if staged { "Unstage" } else { "Stage" },
                );
                item(ui, Command::GitDiscard, "Discard Changes…");
            }
            Row::Section(..) => {
                item(
                    ui,
                    Command::GitStage,
                    if staged { "Unstage All" } else { "Stage All" },
                );
                item(ui, Command::GitDiscard, "Discard All…");
            }
            Row::Repo(_) => {
                item(ui, Command::GitCommit, "Commit");
                item(ui, Command::GitPush, "Push");
                item(ui, Command::GitPull, "Pull");
                item(ui, Command::GitStage, "Stage / Unstage All");
                item(ui, Command::GitDiscard, "Discard All…");
            }
            Row::History(_) | Row::Commit(..) => return None,
        }
        chosen
    }
}

fn row_repo(row: Row) -> Option<usize> {
    match row {
        Row::Repo(r)
        | Row::Section(r, _)
        | Row::File(r, _, _)
        | Row::History(r)
        | Row::Commit(r, _) => Some(r),
    }
}

/// Whether `repo`'s file `i` is listed as staged (`true`) or as a change.
fn in_section(repo: &Repo, i: usize, staged: bool) -> bool {
    repo.files.get(i).is_some_and(|f| {
        if staged {
            f.staged.is_some()
        } else {
            f.unstaged.is_some()
        }
    })
}
