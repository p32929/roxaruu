//! Find / replace in the open file (Oxru's rules): case-insensitive,
//! non-overlapping matches as char ranges. Refining the query keeps the
//! selection near where find was opened instead of walking down the file.

use crate::buffer::Buffer;

#[derive(Default)]
pub struct Find {
    pub query: String,
    pub replace: String,
    pub replace_open: bool,
    /// Keyboard is in the replace field (else the query field).
    pub in_replace: bool,
    /// Char ranges of every match, in order.
    pub matches: Vec<(usize, usize)>,
    pub current: usize,
    /// The caret when find opened: refining jumps to the first match from here.
    origin: usize,
    /// Match case (⌥⌘C) and whole words only (⌥⌘W).
    pub case: bool,
    pub word: bool,
    /// Text of (tab, revision) the matches were computed over.
    hay: Option<(usize, u64, Vec<char>)>,
    /// (tab, revision, query, case, word) the matches are for.
    key: Option<(usize, u64, String, bool, bool)>,
}

impl Find {
    /// Open on `b` (tab `tab`), seeding the query from a one-line selection.
    pub fn open(b: &Buffer, tab: usize, replace: bool, previous: Option<Find>) -> Find {
        let mut f = previous.unwrap_or_default();
        if let Some(sel) = b
            .selected_text()
            .filter(|t| !t.is_empty() && !t.contains('\n'))
        {
            f.query = sel;
        }
        f.replace_open |= replace;
        f.in_replace = replace;
        f.origin = b.selection().map_or(b.cursor, |(s, _)| s);
        f.key = None;
        f.refresh(b, tab);
        f
    }

    /// Recompute matches if the text or query changed. Returns whether they did.
    pub fn refresh(&mut self, b: &Buffer, tab: usize) -> bool {
        let key = (tab, b.revision(), self.query.clone(), self.case, self.word);
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        if !matches!(&self.hay, Some((t, r, _)) if *t == tab && *r == b.revision()) {
            self.hay = Some((tab, b.revision(), b.rope.chars().collect()));
        }
        let needle: Vec<char> = self.query.chars().collect();
        self.matches = match &self.hay {
            Some((_, _, hay)) => find_all(hay, &needle, self.case, self.word),
            None => Vec::new(),
        };
        self.key = Some(key);
        self.current = self.current.min(self.matches.len().saturating_sub(1));
        true
    }

    /// Select the first match at or after where find opened.
    pub fn jump_from_origin(&mut self, b: &mut Buffer) {
        let origin = self.origin;
        self.current = self
            .matches
            .iter()
            .position(|(s, _)| *s >= origin)
            .unwrap_or(0);
        self.select(b);
    }

    pub fn step(&mut self, b: &mut Buffer, delta: isize) {
        let n = self.matches.len() as isize;
        if n > 0 {
            self.current = (self.current as isize + delta).rem_euclid(n) as usize;
            if let Some(m) = self.matches.get(self.current) {
                self.origin = m.0;
            }
            self.select(b);
        }
    }

    fn select(&self, b: &mut Buffer) {
        if let Some(&(s, e)) = self.matches.get(self.current) {
            b.clear_extra_carets();
            b.select_range(s, e);
        }
    }

    /// Replace the current match, then move to the next one.
    pub fn replace_one(&mut self, b: &mut Buffer, tab: usize) {
        let Some(&(s, e)) = self.matches.get(self.current) else {
            return;
        };
        b.select_range(s, e);
        b.insert_str(&self.replace.clone());
        self.origin = b.cursor;
        self.refresh(b, tab);
        if !self.matches.is_empty() {
            self.jump_from_origin(b);
        }
    }

    /// Replace every match, back to front so earlier offsets stay valid.
    pub fn replace_all(&mut self, b: &mut Buffer, tab: usize) -> usize {
        let n = self.matches.len();
        let with = self.replace.clone();
        for &(s, e) in self.matches.iter().rev() {
            b.select_range(s, e);
            b.insert_str(&with);
        }
        self.refresh(b, tab);
        n
    }
}

/// Non-overlapping occurrences of `needle` in `hay`: ignoring ASCII case
/// unless `case`, and only as a whole word (not inside a longer
/// identifier) with `word`.
fn find_all(hay: &[char], needle: &[char], case: bool, word: bool) -> Vec<(usize, usize)> {
    let n = needle.len();
    let mut out = Vec::new();
    if n == 0 || hay.len() < n {
        return out;
    }
    let same = |a: &char, b: &char| {
        if case {
            a == b
        } else {
            a.eq_ignore_ascii_case(b)
        }
    };
    let ident = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let mut i = 0;
    while i + n <= hay.len() {
        if hay.get(i..i + n).is_some_and(|w| w.iter().zip(needle).all(|(a, b)| same(a, b)))
            && (!word
                || !ident(i.checked_sub(1).and_then(|j| hay.get(j))) && !ident(hay.get(i + n)))
        {
            out.push((i, i + n));
            i += n;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> (tempfile::TempDir, Buffer) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, text).unwrap();
        let b = Buffer::from_path(&p).unwrap();
        (dir, b)
    }

    fn find(b: &mut Buffer, q: &str) -> Find {
        let mut f = Find::open(b, 0, false, None);
        f.query = q.into();
        f.refresh(b, 0);
        f.jump_from_origin(b);
        f
    }

    #[test]
    fn case_insensitive_non_overlapping() {
        let (_d, mut b) = buf("Foo foo FOO fooo");
        let f = find(&mut b, "foo");
        assert_eq!(f.matches, [(0, 3), (4, 7), (8, 11), (12, 15)]);
        assert_eq!(
            find_all(&['a'; 4], &['a', 'a'], false, false),
            [(0, 2), (2, 4)]
        );
        let hay: Vec<char> = "Cat cat catalog".chars().collect();
        let cat: Vec<char> = "cat".chars().collect();
        assert_eq!(find_all(&hay, &cat, false, false).len(), 3);
        assert_eq!(find_all(&hay, &cat, true, false), [(4, 7), (8, 11)]);
        assert_eq!(
            find_all(&hay, &cat, false, true),
            [(0, 3), (4, 7)],
            "not inside catalog"
        );
    }

    #[test]
    fn next_prev_wrap_and_select() {
        let (_d, mut b) = buf("a x a x a");
        let mut f = find(&mut b, "a");
        assert_eq!(b.selection(), Some((0, 1)));
        f.step(&mut b, -1);
        assert_eq!(
            b.selection(),
            Some((8, 9)),
            "prev from the first wraps to the last"
        );
        f.step(&mut b, 1);
        assert_eq!(b.selection(), Some((0, 1)));
    }

    #[test]
    fn starts_from_where_it_was_opened() {
        let (_d, mut b) = buf("one two one two");
        b.cursor = 5;
        let f = find(&mut b, "one");
        assert_eq!(
            f.current, 1,
            "the match after the caret, not the first in the file"
        );
    }

    #[test]
    fn seeds_the_query_from_a_selection() {
        let (_d, mut b) = buf("hello world");
        b.select_range(6, 11);
        let f = Find::open(&b, 0, false, None);
        assert_eq!(f.query, "world");
    }

    #[test]
    fn replace_one_then_all() {
        let (_d, mut b) = buf("cat cat cat");
        let mut f = find(&mut b, "cat");
        f.replace = "dog".into();
        f.replace_one(&mut b, 0);
        assert_eq!(b.rope.to_string(), "dog cat cat");
        assert_eq!(f.replace_all(&mut b, 0), 2);
        assert_eq!(b.rope.to_string(), "dog dog dog");
        assert!(f.matches.is_empty());
    }

    #[test]
    fn edits_refresh_the_matches() {
        let (_d, mut b) = buf("ab");
        let mut f = find(&mut b, "ab");
        assert_eq!(f.matches.len(), 1);
        b.clear_selection();
        b.cursor = 2;
        b.insert_str(" ab");
        assert!(f.refresh(&b, 0));
        assert_eq!(f.matches.len(), 2);
    }
}
