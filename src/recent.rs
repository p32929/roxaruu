//! The folders you've opened, as a project or with a terminal in it, with
//! when, newest first (the Open list).

use std::path::{Path, PathBuf};

const MAX: usize = 50;
/// Commands remembered as picked in the palette.
const MAX_COMMANDS: usize = 8;

fn dir() -> Option<PathBuf> {
    crate::config::state_dir()
}

/// `folders` move to the front, as used now. Silent on I/O errors.
pub fn record(folders: &[PathBuf]) {
    if let Some(d) = dir() {
        record_to(&d, folders, crate::update::now());
    }
}

/// Every folder, with when (seconds since 1970), newest first, without
/// those that are gone.
pub fn list() -> Vec<(PathBuf, u64)> {
    dir().map(|d| list_from(&d)).unwrap_or_default()
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

/// From `dir/folders.json`; before there was one, from the two lists it
/// replaced (terminals' folders with their times, then the old recent
/// folders, one path per line, with none).
fn list_from(dir: &Path) -> Vec<(PathBuf, u64)> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let mut list: Vec<(PathBuf, u64)> = match read("folders.json") {
        Some(text) => serde_json::from_str(&text).unwrap_or_default(),
        None => {
            let mut old: Vec<(PathBuf, u64)> =
                read("terminal-folders.json").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            for line in read("recent").unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()) {
                if !old.iter().any(|(d, _)| d.as_path() == Path::new(line)) {
                    old.push((PathBuf::from(line), 0));
                }
            }
            old
        }
    };
    list.retain(|(d, _)| d.is_dir());
    list
}

/// The commands last picked in the palette (their ids), newest first, from
/// every window and earlier runs.
pub fn commands() -> Vec<String> {
    dir().map(|d| commands_from(&d)).unwrap_or_default()
}

/// `id` was picked in the palette now. Silent on I/O errors.
pub fn record_command(id: &str) {
    if let Some(d) = dir() {
        record_command_to(&d, id);
    }
}

fn commands_from(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("commands.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn record_command_to(dir: &Path, id: &str) {
    let mut list = commands_from(dir);
    list.retain(|c| c != id);
    list.insert(0, id.to_string());
    list.truncate(MAX_COMMANDS);
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_string(&list) {
        let _ = std::fs::write(dir.join("commands.json"), json);
    }
}

// ponytail: windows writing at the same moment can lose one's update; it's
// written again at its next change.
fn record_to(dir: &Path, folders: &[PathBuf], at: u64) {
    let mut list = list_from(dir);
    list.retain(|(d, _)| !folders.contains(d));
    let mut now: Vec<(PathBuf, u64)> = Vec::new();
    for d in folders {
        if !now.iter().any(|(n, _)| n == d) {
            now.push((d.clone(), at));
        }
    }
    list.splice(0..0, now);
    list.truncate(MAX);
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_string(&list) {
        let _ = std::fs::write(dir.join("folders.json"), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_once_each_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let [a, b, c] = ["a", "b", "c"].map(|n| dir.path().join(n));
        for d in [&a, &b, &c] {
            std::fs::create_dir(d).unwrap();
        }
        let state = dir.path().join("state");
        record_to(&state, std::slice::from_ref(&a), 1);
        record_to(&state, &[b.clone(), c.clone(), b.clone()], 2);
        record_to(&state, std::slice::from_ref(&a), 3);
        assert_eq!(list_from(&state), [(a.clone(), 3), (b.clone(), 2), (c.clone(), 2)]);
        std::fs::remove_dir(&b).unwrap();
        assert_eq!(list_from(&state), [(a, 3), (c, 2)], "a folder that's gone is left out");
    }

    #[test]
    fn starts_from_the_lists_it_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let [a, b] = ["a", "b"].map(|n| dir.path().join(n));
        for d in [&a, &b] {
            std::fs::create_dir(d).unwrap();
        }
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::write(state.join("terminal-folders.json"), serde_json::to_string(&[(&a, 5)]).unwrap()).unwrap();
        std::fs::write(state.join("recent"), format!("{}\n{}", b.display(), a.display())).unwrap();
        assert_eq!(list_from(&state), [(a.clone(), 5), (b.clone(), 0)]);
        record_to(&state, std::slice::from_ref(&b), 9);
        assert_eq!(list_from(&state), [(b, 9), (a, 5)], "kept once written");
    }

    #[test]
    fn commands_newest_first_once_each() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["a", "b", "a"] {
            record_command_to(dir.path(), id);
        }
        assert_eq!(commands_from(dir.path()), ["a", "b"]);
        for i in 0..20 {
            record_command_to(dir.path(), &i.to_string());
        }
        assert_eq!(commands_from(dir.path()).len(), MAX_COMMANDS);
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
