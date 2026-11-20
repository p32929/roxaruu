//! Recently opened folders, newest first, one path per line; and the
//! folders each window's terminals were in (Recent Terminals).

use std::path::{Path, PathBuf};

const MAX: usize = 20;

fn file() -> Option<PathBuf> {
    crate::config::state_dir().map(|d| d.join("recent"))
}

/// Move `folder` to the front of the list. Silent on I/O errors.
pub fn record(folder: &Path) {
    if let Some(path) = file() {
        record_to(&path, folder);
    }
}

/// The list, dropping folders that no longer exist.
pub fn list() -> Vec<PathBuf> {
    file().map(|p| list_from(&p)).unwrap_or_default()
}

fn list_from(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect()
}

fn record_to(path: &Path, folder: &Path) {
    let mut list = list_from(path);
    list.retain(|p| p != folder);
    list.insert(0, folder.to_path_buf());
    list.truncate(MAX);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body: Vec<String> = list
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let _ = std::fs::write(path, body.join("\n"));
}

/// One window's terminals: the folders they were in when they last changed.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct Group {
    /// Which window, so it keeps one entry while it changes.
    pub window: String,
    /// Its project's name; empty without one.
    pub name: String,
    /// When, in seconds since 1970.
    pub at: u64,
    /// In the terminals' order.
    pub dirs: Vec<PathBuf>,
}

fn terminals_file() -> Option<PathBuf> {
    crate::config::state_dir().map(|d| d.join("terminals.json"))
}

/// Save this window's terminals, first in the list. Silent on I/O errors.
pub fn record_terminals(group: Group) {
    if let Some(path) = terminals_file() {
        record_group_to(&path, group);
    }
}

/// The windows' terminals, newest first, without folders that are gone.
pub fn terminal_groups() -> Vec<Group> {
    terminals_file().map(|p| groups_from(&p)).unwrap_or_default()
}

/// Every folder in `groups` once, with when it was last used, newest first.
pub fn terminal_folders(groups: &[Group]) -> Vec<(PathBuf, u64)> {
    let mut out: Vec<(PathBuf, u64)> = Vec::new();
    for g in groups {
        for d in &g.dirs {
            if !out.iter().any(|(o, _)| o == d) {
                out.push((d.clone(), g.at));
            }
        }
    }
    out
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago".
pub fn ago(secs: u64) -> String {
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        86400..172800 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86400),
    }
}

fn groups_from(path: &Path) -> Vec<Group> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut groups: Vec<Group> = serde_json::from_str(&text).unwrap_or_default();
    for g in &mut groups {
        g.dirs.retain(|d| d.is_dir());
    }
    groups.retain(|g| !g.dirs.is_empty());
    groups
}

// ponytail: windows writing at the same moment can lose one's update; it's
// written again at its next change.
fn record_group_to(path: &Path, group: Group) {
    let mut list = groups_from(path);
    list.retain(|g| g.window != group.window);
    list.insert(0, group);
    list.truncate(MAX);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&list) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_moves_to_front_dedupes_and_drops_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let file = dir.path().join("recent");
        record_to(&file, &a);
        record_to(&file, &b);
        record_to(&file, &a);
        assert_eq!(list_from(&file), [a.clone(), b.clone()]);
        std::fs::remove_dir(&b).unwrap();
        assert_eq!(list_from(&file), [a]);
    }

    #[test]
    fn terminal_groups_one_per_window_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let file = dir.path().join("terminals.json");
        let g = |window: &str, at, dirs: &[&PathBuf]| Group {
            window: window.into(),
            name: String::new(),
            at,
            dirs: dirs.iter().map(|d| d.to_path_buf()).collect(),
        };
        record_group_to(&file, g("w1", 1, &[&a]));
        record_group_to(&file, g("w2", 2, &[&b, &a]));
        record_group_to(&file, g("w1", 3, &[&a, &b]));
        let groups = groups_from(&file);
        assert_eq!(groups.iter().map(|g| g.window.as_str()).collect::<Vec<_>>(), ["w1", "w2"]);
        assert_eq!(terminal_folders(&groups), [(a.clone(), 3), (b.clone(), 3)]);
        // A folder that's gone is left out, and a window left with none.
        std::fs::remove_dir(&b).unwrap();
        record_group_to(&file, g("w3", 4, &[&b]));
        let groups = groups_from(&file);
        assert_eq!(groups.iter().map(|g| g.window.as_str()).collect::<Vec<_>>(), ["w1", "w2"]);
        assert_eq!(groups[0].dirs, [a]);
    }

    #[test]
    fn ago_reads_like_speech() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(600), "10 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(90000), "yesterday");
        assert_eq!(ago(4 * 86400), "4 days ago");
    }
}
