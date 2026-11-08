//! Recently opened folders, newest first. Shares Oxru's own list (same file,
//! same one-path-per-line format), so both apps remember the same projects.

use std::path::{Path, PathBuf};

const MAX: usize = 20;

fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("oxru").join("recent"))
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
}
