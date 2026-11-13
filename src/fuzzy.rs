//! VS Code-style fuzzy matching, ported from Oxru (itself a port of the
//! relevant parts of microsoft/vscode `fuzzyScorer.ts`). The filename is scored
//! separately from the full path, with big tier gaps so a filename match always
//! beats a path match and a filename *prefix* beats the rest.

use std::cmp::Ordering;

const PATH_IDENTITY_SCORE: i64 = 1 << 18;
const LABEL_PREFIX_SCORE_THRESHOLD: i64 = 1 << 17;
const LABEL_SCORE_THRESHOLD: i64 = 1 << 16;

/// Per-step recency boost (most recent first). Small next to the tier
/// thresholds, so it only reorders within a tier.
const RECENCY_BOOST_STEP: i64 = 96;

/// A project's file list, prepared once (chars + lowercase) so each keystroke
/// only scores, rather than re-lowercasing every path.
pub struct FileMatcher {
    prepared: Vec<Prepared>,
}

impl FileMatcher {
    pub fn new(entries: &[String]) -> Self {
        FileMatcher {
            prepared: entries.iter().map(|e| Prepared::new(e)).collect(),
        }
    }

    /// Entry indices matching `query`, best first. `recent(i)` is entry `i`'s
    /// most-recently-used position (`Some(0)` = latest). An empty query lists
    /// everything, recent first.
    pub fn rank(&self, query: &str, recent: impl Fn(usize) -> Option<usize>) -> Vec<usize> {
        if query.trim().is_empty() {
            let mut idx: Vec<usize> = (0..self.prepared.len()).collect();
            idx.sort_by(|&a, &b| cmp_recent(recent(a), recent(b)).then(a.cmp(&b)));
            return idx;
        }
        let by_path = query.contains('/');
        // (index, score, path length, matched the filename)
        let mut scored: Vec<(usize, i64, usize, bool)> = self
            .prepared
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                score_item(p, query, by_path).map(|(s, _, label)| (i, s, p.chars.len(), label))
            })
            .collect();
        // Every filename match is listed (mid-word ones ranked low), but pure
        // path-scatter matches are dropped when any filename match exists —
        // that's the random-looking noise.
        if scored.iter().any(|x| x.3) {
            scored.retain(|x| x.3);
        }
        scored.sort_by(|a, b| {
            let sa = a.1 + recency_boost(recent(a.0));
            let sb = b.1 + recency_boost(recent(b.0));
            sb.cmp(&sa).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0))
        });
        scored.into_iter().map(|x| x.0).collect()
    }
}

/// Score `query` against one short label (a command, a theme name). `None` if
/// it doesn't match; an empty query matches everything equally.
pub fn score(query: &str, text: &str) -> Option<i64> {
    if query.trim().is_empty() {
        return Some(0);
    }
    score_item(&Prepared::new(text), query, false).map(|(s, _, _)| s)
}

/// The char indices of `text` that `query` matched, for highlighting.
pub fn positions(query: &str, text: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    score_item(&Prepared::new(text), query, query.contains('/'))
        .map(|(_, p, _)| p)
        .unwrap_or_default()
}

/// A recent entry (`Some`, lower = newer) sorts before a non-recent one.
fn cmp_recent(a: Option<usize>, b: Option<usize>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn recency_boost(rank: Option<usize>) -> i64 {
    match rank {
        Some(r) if r < 256 => (256 - r as i64) * RECENCY_BOOST_STEP,
        _ => 0,
    }
}

struct Prepared {
    chars: Vec<char>,
    lower: Vec<char>,
    /// Where the filename (as opposed to its folders) begins.
    label_start: usize,
}

impl Prepared {
    fn new(full: &str) -> Self {
        let chars: Vec<char> = full.chars().collect();
        let lower: Vec<char> = full.to_lowercase().chars().collect();
        let label_len = full.rsplit('/').next().unwrap_or(full).chars().count();
        // Clamped to both: lowercasing can (rarely) change the char count.
        let label_start = chars.len().saturating_sub(label_len).min(lower.len());
        Prepared {
            chars,
            lower,
            label_start,
        }
    }
}

fn is_path_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// VS Code `scoreSeparatorAtPos`.
fn separator_bonus(prev: char) -> i64 {
    match prev {
        '/' | '\\' => 5,
        '_' | '-' | '.' | ' ' | '\'' | '"' | ':' => 4,
        _ => 0,
    }
}

/// VS Code `computeCharScore`.
fn char_score(
    q_char: char,
    q_lower: char,
    target: &[char],
    target_lower: &[char],
    ti: usize,
    seq_len: i64,
) -> i64 {
    let (Some(&t), Some(&t_lower)) = (target.get(ti), target_lower.get(ti)) else {
        return 0;
    };
    if q_lower != t_lower && !(is_path_sep(q_lower) && is_path_sep(t_lower)) {
        return 0;
    }
    let mut score = 1;
    if seq_len > 0 {
        score += seq_len.min(3) * 6 + (seq_len - 3).max(0) * 3;
    }
    if q_char == t {
        score += 1;
    }
    match ti.checked_sub(1).and_then(|i| target.get(i)) {
        None => score += 8,
        Some(&prev) => {
            let sep = separator_bonus(prev);
            if sep != 0 {
                score += sep;
            } else if t.is_uppercase() && seq_len == 0 {
                score += 2; // camelCase hump
            }
        }
    }
    score
}

/// VS Code `doScoreFuzzy` DP: (score, matched positions).
fn score_fuzzy(
    target: &[char],
    target_lower: &[char],
    query: &[char],
    query_lower: &[char],
) -> (i64, Vec<usize>) {
    let (tl, ql) = (target.len(), query.len());
    if tl == 0 || ql == 0 || tl < ql {
        return (0, Vec::new());
    }
    let mut scores = vec![0i64; ql * tl];
    let mut matches = vec![0i64; ql * tl];
    for (qi, (&q_char, &q_lower)) in query.iter().zip(query_lower).enumerate() {
        let q_off = qi * tl;
        for ti in 0..tl {
            let cur = q_off + ti;
            let left = if ti > 0 { scores.get(cur - 1).copied().unwrap_or(0) } else { 0 };
            let (diag, seq_len) = if qi > 0 && ti > 0 {
                let d = q_off - tl + ti - 1;
                (scores.get(d).copied().unwrap_or(0), matches.get(d).copied().unwrap_or(0))
            } else {
                (0, 0)
            };
            // Past the first query char, only score in sequence.
            let score = if diag == 0 && qi > 0 {
                0
            } else {
                char_score(q_char, q_lower, target, target_lower, ti, seq_len)
            };
            let (m, s) = if score != 0 && diag + score >= left {
                (seq_len + 1, diag + score)
            } else {
                (0, left)
            };
            if let (Some(mc), Some(sc)) = (matches.get_mut(cur), scores.get_mut(cur)) {
                *mc = m;
                *sc = s;
            }
        }
    }
    let mut positions = Vec::new();
    let (mut qi, mut ti) = (ql as isize - 1, tl as isize - 1);
    while qi >= 0 && ti >= 0 {
        if matches.get(qi as usize * tl + ti as usize).is_none_or(|&m| m == 0) {
            ti -= 1;
        } else {
            positions.push(ti as usize);
            qi -= 1;
            ti -= 1;
        }
    }
    positions.reverse();
    (scores.last().copied().unwrap_or(0), positions)
}

/// One query piece: filename first (prefix > boundary > anywhere), then the
/// full path as a weak fallback. The bool is "matched the filename".
fn score_piece(p: &Prepared, query: &str, prefer_label: bool) -> Option<(i64, Vec<usize>, bool)> {
    let q: Vec<char> = query.chars().collect();
    let q_lower: Vec<char> = query.to_lowercase().chars().collect();
    if prefer_label {
        let lab = p.chars.get(p.label_start..).unwrap_or_default();
        let lab_lower = p.lower.get(p.label_start..).unwrap_or_default();
        let (ls, lpos) = score_fuzzy(lab, lab_lower, &q, &q_lower);
        if ls > 0 {
            let base = if lab_lower.starts_with(&q_lower) {
                // Shorter filenames win among prefix matches: window.ts > windowActions.ts.
                LABEL_PREFIX_SCORE_THRESHOLD
                    + ((q.len() as f64 / lab.len().max(1) as f64) * 100.0).round() as i64
            } else {
                LABEL_SCORE_THRESHOLD
            };
            return Some((
                base + ls,
                lpos.into_iter().map(|i| i + p.label_start).collect(),
                true,
            ));
        }
    }
    let (ps, ppos) = score_fuzzy(&p.chars, &p.lower, &q, &q_lower);
    (ps > 0).then_some((ps, ppos, false))
}

/// A whole query: identity first, then every space-separated piece must match.
/// `by_path` (query has a `/`) matches against the full path.
fn score_item(p: &Prepared, query: &str, by_path: bool) -> Option<(i64, Vec<usize>, bool)> {
    if p.lower.len() == query.chars().count()
        && p.lower.iter().copied().eq(query.to_lowercase().chars())
    {
        return Some((PATH_IDENTITY_SCORE, (0..p.chars.len()).collect(), true));
    }
    let mut total = 0;
    let mut positions = Vec::new();
    let mut label = true;
    let mut any = false;
    for piece in query.split_whitespace() {
        any = true;
        let (s, pos, lm) = score_piece(p, piece, !by_path)?;
        total += s;
        positions.extend(pos);
        label &= lm;
    }
    if !any {
        return None;
    }
    positions.sort_unstable();
    positions.dedup();
    Some((total, positions, label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank(entries: &[&str], query: &str) -> Vec<String> {
        let entries: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
        FileMatcher::new(&entries)
            .rank(query, |_| None)
            .into_iter()
            .map(|i| entries[i].clone())
            .collect()
    }

    #[test]
    fn positions_cover_the_match() {
        let text: Vec<char> = "src/domain.rs".chars().collect();
        let hit: String = positions("man", "src/domain.rs")
            .iter()
            .map(|&i| text[i])
            .collect();
        assert_eq!(hit, "man");
        assert!(positions("zzz", "src/domain.rs").is_empty());
    }

    #[test]
    fn filename_match_ranks_above_path_only_match() {
        assert_eq!(
            rank(&["main/lib.rs", "src/main.rs"], "main")[0],
            "src/main.rs"
        );
    }

    #[test]
    fn path_scatter_noise_is_excluded() {
        let got = rank(
            &[
                "server_nestjs/src/sessions/create-session.dto.ts",
                "thrive_shared/lib/src/widgets/app_button.dart",
                "thrive_shared/lib/src/constants/colors.dart",
            ],
            "sess dto",
        );
        assert_eq!(got, ["server_nestjs/src/sessions/create-session.dto.ts"]);
    }

    #[test]
    fn slash_query_matches_the_path() {
        let got = rank(
            &["src/components/Button.tsx", "src/widgets/Button.tsx"],
            "comp/button",
        );
        assert_eq!(got, ["src/components/Button.tsx"]);
    }

    #[test]
    fn prefix_beats_mid_name_beats_path() {
        let got = rank(
            &["window.ts", "windowActions.ts", "lib/createWindow.ts"],
            "window",
        );
        assert_eq!(
            got,
            ["window.ts", "windowActions.ts", "lib/createWindow.ts"]
        );
    }

    #[test]
    fn boundary_and_camelcase_rank_above_mid_word() {
        let got = rank(&["crowbar.rs", "AppBar.tsx", "bar.rs"], "bar");
        let pos = |s: &str| got.iter().position(|x| x == s).unwrap();
        assert!(pos("bar.rs") < pos("crowbar.rs"));
        assert!(pos("AppBar.tsx") < pos("crowbar.rs"));
    }

    #[test]
    fn vscode_constants_ordering() {
        let got = rank(
            &[
                "thrive_shared/lib/src/constants/colors.dart",
                "trainer_flutter/lib/config/constants.dart",
                "thrive_shared/lib/src/constants/app_constants.dart",
                "client_flutter/lib/config/constants.dart",
            ],
            "constants",
        );
        assert_eq!(
            got,
            [
                "client_flutter/lib/config/constants.dart",
                "trainer_flutter/lib/config/constants.dart",
                "thrive_shared/lib/src/constants/app_constants.dart",
            ]
        );
    }

    #[test]
    fn recent_file_ranks_first() {
        let entries: Vec<String> = ["src/user.rs", "src/user_service.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let m = FileMatcher::new(&entries);
        assert_eq!(m.rank("user", |_| None)[0], 0);
        assert_eq!(m.rank("user", |i| (i == 1).then_some(0))[0], 1);
        // Empty query: recent first, then natural order.
        assert_eq!(m.rank("", |i| (i == 1).then_some(0)), [1, 0]);
    }

    #[test]
    fn short_label_scoring() {
        assert!(score("thm", "Select Theme").is_some());
        assert!(score("xyz", "Select Theme").is_none());
        assert!(score("", "anything").is_some());
    }
}
