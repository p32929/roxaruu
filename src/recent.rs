//! Recently opened folders, newest first, one path per line; and the
//! folders terminals were in (Recent Terminals).

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

fn terminals_file() -> Option<PathBuf> {
    crate::config::state_dir().map(|d| d.join("terminal-folders.json"))
}

/// The folders terminals were in (Recent Terminals): `dirs` move to the
/// front, as used now. Silent on I/O errors.
pub fn record_terminals(dirs: &[PathBuf]) {
    if let Some(path) = terminals_file() {
        record_folders_to(&path, dirs, crate::update::now());
    }
}

/// Every folder a terminal was in, with when (seconds since 1970), newest
/// first, without those that are gone.
pub fn terminal_folders() -> Vec<(PathBuf, u64)> {
    terminals_file().map(|p| folders_from(&p)).unwrap_or_default()
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

fn folders_from(path: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut list: Vec<(PathBuf, u64)> = serde_json::from_str(&text).unwrap_or_default();
    list.retain(|(d, _)| d.is_dir());
    list
}

// ponytail: windows writing at the same moment can lose one's update; it's
// written again at its next change.
fn record_folders_to(path: &Path, dirs: &[PathBuf], at: u64) {
    let mut list = folders_from(path);
    list.retain(|(d, _)| !dirs.contains(d));
    let mut now: Vec<(PathBuf, u64)> = Vec::new();
    for d in dirs {
        if !now.iter().any(|(n, _)| n == d) {
            now.push((d.clone(), at));
        }
    }
    list.splice(0..0, now);
    list.truncate(TERMINALS_MAX);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&list) {
        let _ = std::fs::write(path, json);
    }
}

const TERMINALS_MAX: usize = 50;

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
    fn terminal_folders_once_each_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let [a, b, c] = ["a", "b", "c"].map(|n| dir.path().join(n));
        for d in [&a, &b, &c] {
            std::fs::create_dir(d).unwrap();
        }
        let file = dir.path().join("terminal-folders.json");
        record_folders_to(&file, std::slice::from_ref(&a), 1);
        record_folders_to(&file, &[b.clone(), c.clone(), b.clone()], 2);
        record_folders_to(&file, std::slice::from_ref(&a), 3);
        assert_eq!(folders_from(&file), [(a.clone(), 3), (b.clone(), 2), (c.clone(), 2)]);
        std::fs::remove_dir(&b).unwrap();
        assert_eq!(folders_from(&file), [(a, 3), (c, 2)], "a folder that's gone is left out");
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
