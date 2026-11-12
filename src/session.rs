//! Per-project editing session: which files were open (and which was active),
//! so reopening a folder restores your tabs. Stored under the config dir, keyed
//! by a hash of the project root, separate from the project itself. And per
//! file, where you were in it (`Place`), so a file reopens there.

use std::path::{Path, PathBuf};

/// `<config dir>/oxru/sessions/<root-hash>`: Oxru's own session store, same
/// format, so both apps restore the same tabs.
fn session_path(root: &Path) -> Option<PathBuf> {
    let canon = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    dirs::config_dir().map(|d| {
        d.join("oxru")
            .join("sessions")
            .join(format!("{:016x}", hash(&canon)))
    })
}

fn hash(p: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    h.finish()
}

/// Save the open `files` (and the `active` index) for `root`.
pub fn save(root: &Path, files: &[PathBuf], active: usize) {
    if let Some(path) = session_path(root) {
        save_to(&path, files, active);
    }
}

/// Load the previously-open files for `root` (skipping any that no longer exist)
/// and the active index.
pub fn load(root: &Path) -> (Vec<PathBuf>, usize) {
    session_path(root)
        .map(|p| load_from(&p))
        .unwrap_or_default()
}

/// Where you were in a file: the caret's line and column, and how the view
/// was scrolled (points: ponytail — a different text size lands a little off;
/// the caret is still put back exactly).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Place {
    pub line: usize,
    pub col: usize,
    pub scroll: (f32, f32),
}

/// The most files whose places are kept (the least recent go first).
const PLACES_KEPT: usize = 1000;

fn places_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("oxru").join("places"))
}

/// Note where you are in `places`' files (kept across windows and launches).
pub fn remember(places: &[(PathBuf, Place)]) {
    if let Some(store) = places_path() {
        remember_in(&store, places);
    }
}

/// Where you last were in `file`, if anywhere.
pub fn recall(file: &Path) -> Option<Place> {
    recall_from(&places_path()?, file)
}

// ---- testable cores (path-injected) -----------------------------------

/// One line per file, most recent first: line, column, scroll x, y, path.
fn read_places(store: &Path) -> Vec<(PathBuf, Place)> {
    let text = std::fs::read_to_string(store).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(5, '\t');
            let (line, col, x, y) = (f.next()?, f.next()?, f.next()?, f.next()?);
            let place = Place {
                line: line.parse().ok()?,
                col: col.parse().ok()?,
                scroll: (x.parse().ok()?, y.parse().ok()?),
            };
            Some((PathBuf::from(f.next()?), place))
        })
        .collect()
}

fn remember_in(store: &Path, places: &[(PathBuf, Place)]) {
    // Read, merge, write: other windows remember their files here too.
    let mut all = read_places(store);
    all.retain(|(p, _)| !places.iter().any(|(q, _)| q == p));
    all.splice(0..0, places.iter().cloned());
    all.truncate(PLACES_KEPT);
    let body: String = all
        .iter()
        .map(|(p, pl)| format!("{}\t{}\t{}\t{}\t{}\n", pl.line, pl.col, pl.scroll.0, pl.scroll.1, p.to_string_lossy()))
        .collect();
    if let Some(parent) = store.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(store, body);
}

fn recall_from(store: &Path, file: &Path) -> Option<Place> {
    read_places(store).into_iter().find(|(p, _)| p == file).map(|(_, pl)| pl)
}

fn save_to(path: &Path, files: &[PathBuf], active: usize) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Line 0 is the active index; the rest are file paths, in tab order.
    let mut body = format!("{active}\n");
    for f in files {
        body.push_str(&f.to_string_lossy());
        body.push('\n');
    }
    let _ = std::fs::write(path, body);
}

fn load_from(path: &Path) -> (Vec<PathBuf>, usize) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Vec::new(), 0);
    };
    let mut lines = text.lines();
    let active: usize = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    let files: Vec<PathBuf> = lines
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect();
    let active = active.min(files.len().saturating_sub(1));
    (files, active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_roundtrips_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("sess");
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();

        save_to(&store, &[a.clone(), b.clone()], 1);
        let (files, active) = load_from(&store);
        assert_eq!(files, vec![a, b]);
        assert_eq!(active, 1);
    }

    #[test]
    fn places_are_remembered_per_file_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("places");
        let (a, b) = (PathBuf::from("/p/a b.rs"), PathBuf::from("/p/b.rs"));
        let at = |line, y| Place { line, col: 3, scroll: (0.0, y) };
        assert_eq!(recall_from(&store, &a), None);
        remember_in(&store, &[(a.clone(), at(10, 120.5)), (b.clone(), at(2, 0.0))]);
        remember_in(&store, &[(a.clone(), at(40, 900.0))]); // moved on: replaced
        assert_eq!(recall_from(&store, &a), Some(at(40, 900.0)));
        assert_eq!(recall_from(&store, &b), Some(at(2, 0.0)));
        assert_eq!(read_places(&store).len(), 2);
        // Only the most recent are kept.
        let many: Vec<_> = (0..PLACES_KEPT + 5).map(|i| (PathBuf::from(format!("/f{i}")), at(i, 0.0))).collect();
        remember_in(&store, &many);
        assert_eq!(read_places(&store).len(), PLACES_KEPT);
        assert_eq!(recall_from(&store, &a), None, "the oldest went");
    }

    #[test]
    fn missing_files_are_dropped_and_active_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("sess");
        let real = dir.path().join("real.rs");
        std::fs::write(&real, "").unwrap();
        let ghost = dir.path().join("ghost.rs");

        save_to(&store, &[ghost, real.clone()], 1);
        let (files, active) = load_from(&store);
        assert_eq!(files, vec![real]);
        assert_eq!(active, 0, "active index clamped to surviving files");
    }
}
