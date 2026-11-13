//! Files too big to hold in memory, read straight from disk a screenful at a
//! time. One pass at open builds a sparse index (the byte offset of every
//! `STEP`th line); showing a line is then a seek to the mark before it and a
//! short read forward. Read-only: editing would need the whole file in memory
//! (`Command::EditWholeFile` does that on request).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::PathBuf;

use crate::syntax::LineSpans;

/// Files bigger than this open in the read-only view. Measured: whole, a
/// 50 MB file took 8 s and ~800 MB to open; this view opens it in well
/// under a second with a few MB.
pub const HUGE_BYTES: u64 = 8 << 20;
/// So do files with a line longer than this (minified code, data dumps):
/// the editor lays out whole lines, which this long are too slow to draw.
pub const LONG_LINE: usize = 20_000;
/// A mark every this many lines: showing any line reads at most this many
/// lines before it.
const STEP: usize = 1024;
/// How much of one line is kept for showing.
// ponytail: columns past 64 KB of a line aren't shown; read further on demand if that matters.
const LINE_CAP: usize = 64 << 10;
/// Search reads the file in pieces this big.
const CHUNK: u64 = 4 << 20;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct LineIndex {
    /// `marks[k]` is where line `k * STEP` starts.
    marks: Vec<u64>,
    /// Lines, counted like the editor does (a final line break starts an empty last line).
    pub lines: usize,
    pub bytes: u64,
    /// Bytes in the longest line.
    pub longest: usize,
}

impl LineIndex {
    pub fn build(mut r: impl Read) -> io::Result<Self> {
        let mut ix = LineIndex {
            marks: vec![0],
            lines: 1,
            ..Default::default()
        };
        let mut buf = vec![0u8; 1 << 20];
        let mut line_start = 0u64;
        loop {
            let n = match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            for i in memchr::memchr_iter(b'\n', buf.get(..n).unwrap_or_default()) {
                let end = ix.bytes + i as u64;
                ix.longest = ix.longest.max((end - line_start) as usize);
                line_start = end + 1;
                // `ix.lines` is the number of the line starting here.
                if ix.lines.is_multiple_of(STEP) {
                    ix.marks.push(line_start);
                }
                ix.lines += 1;
            }
            ix.bytes += n as u64;
        }
        ix.longest = ix.longest.max((ix.bytes - line_start) as usize);
        Ok(ix)
    }

    /// The nearest mark at or before `line`: `(its line, its offset)`.
    fn mark_before(&self, line: usize) -> (usize, u64) {
        let k = (line / STEP).min(self.marks.len().saturating_sub(1));
        (k * STEP, self.marks.get(k).copied().unwrap_or(0))
    }

    /// Lines `range` (clamped to the file), each without its line break and
    /// at most `LINE_CAP` bytes of it.
    pub fn read_lines<R: Read + Seek>(&self, r: R, range: Range<usize>) -> io::Result<Vec<String>> {
        let end = range.end.min(self.lines);
        let mut out = Vec::with_capacity(end.saturating_sub(range.start));
        if range.start >= end {
            return Ok(out);
        }
        let (mut line, at) = self.mark_before(range.start);
        let mut r = BufReader::with_capacity(256 << 10, r);
        r.seek(SeekFrom::Start(at))?;
        let mut cur = Vec::new();
        while line < end {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                // The last line has no break after it.
                out.push(text(&cur));
                break;
            }
            let (take, done) = match memchr::memchr(b'\n', buf) {
                Some(i) => (i, true),
                None => (buf.len(), false),
            };
            if line >= range.start {
                let room = LINE_CAP.saturating_sub(cur.len());
                cur.extend_from_slice(buf.get(..take.min(room)).unwrap_or_default());
            }
            r.consume(take + usize::from(done));
            if done {
                if line >= range.start {
                    out.push(text(&cur));
                }
                cur.clear();
                line += 1;
            }
        }
        Ok(out)
    }

    /// Where `line` starts.
    pub fn line_start<R: Read + Seek>(&self, r: R, line: usize) -> io::Result<u64> {
        let (mut at_line, mut pos) = self.mark_before(line);
        let mut r = BufReader::with_capacity(256 << 10, r);
        r.seek(SeekFrom::Start(pos))?;
        while at_line < line {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let (used, done) = match memchr::memchr(b'\n', buf) {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            r.consume(used);
            pos += used as u64;
            at_line += usize::from(done);
        }
        Ok(pos)
    }

    /// The line holding byte `offset`, and where that line starts.
    pub fn line_at<R: Read + Seek>(&self, r: R, offset: u64) -> io::Result<(usize, u64)> {
        let k = self.marks.partition_point(|&m| m <= offset).saturating_sub(1);
        let (mut line, mut pos) = (k * STEP, self.marks.get(k).copied().unwrap_or(0));
        let mut start = pos;
        let mut r = BufReader::with_capacity(256 << 10, r);
        r.seek(SeekFrom::Start(pos))?;
        while pos < offset {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let n = buf.len().min((offset - pos) as usize);
            for i in memchr::memchr_iter(b'\n', buf.get(..n).unwrap_or_default()) {
                line += 1;
                start = pos + i as u64 + 1;
            }
            r.consume(n);
            pos += n as u64;
        }
        Ok((line, start))
    }

    /// Where `needle` next occurs from byte `from` on (`forward`), or last
    /// occurs before it, going round the end of the file once. ASCII case is
    /// ignored unless `case`.
    pub fn find<R: Read + Seek>(
        &self,
        mut r: R,
        needle: &str,
        from: u64,
        forward: bool,
        case: bool,
    ) -> io::Result<Option<u64>> {
        let from = from.min(self.bytes);
        let (ahead, behind) = (from..self.bytes, 0..from);
        let order = if forward { [ahead, behind] } else { [behind, ahead] };
        for starts in order {
            if let Some(at) = self.search(&mut r, needle, starts, forward, case)? {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }

    /// The first (or last) match starting inside `starts`.
    fn search<R: Read + Seek>(
        &self,
        r: &mut R,
        needle: &str,
        starts: Range<u64>,
        forward: bool,
        case: bool,
    ) -> io::Result<Option<u64>> {
        let fold = |b: &mut [u8]| {
            if !case {
                b.make_ascii_lowercase();
            }
        };
        let mut pat = needle.as_bytes().to_vec();
        fold(&mut pat);
        if pat.is_empty() || starts.is_empty() {
            return Ok(None);
        }
        let n_chunks = (starts.end - starts.start).div_ceil(CHUNK);
        let mut buf = Vec::new();
        for k in 0..n_chunks {
            let k = if forward { k } else { n_chunks - 1 - k };
            let s = starts.start + k * CHUNK;
            let own = (starts.end - s).min(CHUNK) as usize;
            // Read a needle's length more, for a match that starts in this piece.
            let len = (own + pat.len() - 1).min((self.bytes - s) as usize);
            buf.resize(len, 0);
            r.seek(SeekFrom::Start(s))?;
            r.read_exact(&mut buf)?;
            fold(&mut buf);
            let hit = if forward {
                memchr::memmem::find_iter(&buf, &pat).find(|&i| i < own)
            } else {
                memchr::memmem::rfind_iter(&buf, &pat).find(|&i| i < own)
            };
            if let Some(i) = hit {
                return Ok(Some(s + i as u64));
            }
        }
        Ok(None)
    }
}

fn text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// A huge file's tab: where it's scrolled to and the lines on screen.
pub struct HugeView {
    pub path: PathBuf,
    pub index: LineIndex,
    /// The lines last read: `(first line, lines)`.
    cache: (usize, Vec<String>),
    /// Highlighting of the lines on screen: `(first line, count, spans)`.
    pub spans: Option<(usize, usize, Vec<LineSpans>)>,
    /// First line shown.
    pub top: usize,
    /// The line the caret is on.
    pub cursor: usize,
    /// First column shown (horizontal scroll), and how many fit.
    pub col: usize,
    pub cols_shown: usize,
    /// Mouse-wheel lines not scrolled yet.
    pub wheel: f32,
    /// The last find: `(line, first char, end char, byte offset)`.
    pub found: Option<(usize, usize, usize, u64)>,
}

impl HugeView {
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let index = LineIndex::build(File::open(&path)?)?;
        Ok(HugeView {
            path,
            index,
            cache: (0, Vec::new()),
            spans: None,
            top: 0,
            cursor: 0,
            col: 0,
            cols_shown: 80,
            wheel: 0.0,
            found: None,
        })
    }

    pub fn lines(&self) -> usize {
        self.index.lines
    }

    /// Lines `range` for drawing, from disk only when they aren't the ones
    /// read last (a screen's worth either side is read along with them).
    pub fn shown(&mut self, range: Range<usize>) -> io::Result<&[String]> {
        let end = range.end.min(self.lines());
        let (first, have) = (&self.cache.0, &self.cache.1);
        if !(range.start >= *first && end <= first + have.len()) {
            let pad = range.len();
            let from = range.start.saturating_sub(pad);
            let lines = self.index.read_lines(File::open(&self.path)?, from..end + pad)?;
            self.cache = (from, lines);
        }
        let (first, have) = (self.cache.0, &self.cache.1);
        let a = range.start.saturating_sub(first).min(have.len());
        let b = end.saturating_sub(first).clamp(a, have.len());
        Ok(have.get(a..b).unwrap_or_default())
    }

    /// One line, read now (for previews and copying).
    pub fn line(&self, l: usize) -> String {
        File::open(&self.path)
            .and_then(|f| self.index.read_lines(f, l..l + 1))
            .ok()
            .and_then(|v| v.into_iter().next())
            .unwrap_or_default()
    }

    /// Scroll so the caret's line is on screen (`rows` lines tall).
    pub fn keep_cursor_in_view(&mut self, rows: usize) {
        self.cursor = self.cursor.min(self.lines().saturating_sub(1));
        let rows = rows.max(1);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + rows {
            self.top = self.cursor + 1 - rows;
        }
    }

    /// Put the caret on `line`, a few lines below the top of the screen.
    pub fn go_to(&mut self, line: usize) {
        self.cursor = line.min(self.lines().saturating_sub(1));
        self.top = self.cursor.saturating_sub(5);
    }

    /// Find `needle`: from the start of the caret's line (`step` None), or
    /// after / before the last match. Moves the caret to what it finds.
    pub fn find(&mut self, needle: &str, case: bool, step: Option<bool>) -> io::Result<()> {
        let file = File::open(&self.path)?;
        let (from, forward) = match (step, self.found) {
            (Some(true), Some(f)) => (f.3 + 1, true),
            (Some(false), Some(f)) => (f.3, false),
            (step, _) => (self.index.line_start(&file, self.cursor)?, step != Some(false)),
        };
        self.found = None;
        let Some(at) = self.index.find(&file, needle, from, forward, case)? else {
            return Ok(());
        };
        let (line, start) = self.index.line_at(&file, at)?;
        let text = self.line(line);
        let before = ((at - start) as usize).min(text.len());
        let col = String::from_utf8_lossy(text.as_bytes().get(..before).unwrap_or_default()).chars().count();
        self.found = Some((line, col, col + needle.chars().count(), at));
        self.go_to(line);
        if col < self.col || col + needle.len() > self.col + self.cols_shown {
            self.col = col.saturating_sub(8);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn numbered(n: usize) -> String {
        (0..n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn the_index_counts_lines_like_the_editor() {
        for (text, lines) in [("", 1), ("a", 1), ("a\n", 2), ("a\nb", 2), ("\n\n", 3)] {
            let ix = LineIndex::build(Cursor::new(text)).unwrap();
            assert_eq!(ix.lines, ropey::Rope::from_str(text).len_lines(), "{text:?}");
            assert_eq!(ix.lines, lines);
        }
        let ix = LineIndex::build(Cursor::new("ab\nabcdef\nx")).unwrap();
        assert_eq!(ix.longest, 6);
    }

    #[test]
    fn any_lines_read_back_across_marks() {
        let text = numbered(5000);
        let ix = LineIndex::build(Cursor::new(&text)).unwrap();
        assert_eq!(ix.marks.len(), 5, "a mark every {STEP} lines");
        let want: Vec<String> = text.lines().map(String::from).collect();
        for range in [0..3, 1020..1030, 4990..5001, 2048..2049, 3000..3000] {
            let got = ix.read_lines(Cursor::new(&text), range.clone()).unwrap();
            let end = range.end.min(5000);
            let mut expect = want[range.start.min(end)..end].to_vec();
            if range.end > 5000 {
                expect.push(String::new()); // after the final line break
            }
            assert_eq!(got, expect, "{range:?}");
        }
        // No final line break, CRLF.
        let crlf = "a\r\nb\r\nc";
        let ix = LineIndex::build(Cursor::new(crlf)).unwrap();
        assert_eq!(ix.read_lines(Cursor::new(crlf), 0..9).unwrap(), ["a", "b", "c"]);
    }

    #[test]
    fn long_lines_are_capped() {
        let text = format!("{}\nnext", "x".repeat(LINE_CAP * 2));
        let ix = LineIndex::build(Cursor::new(&text)).unwrap();
        let got = ix.read_lines(Cursor::new(&text), 0..2).unwrap();
        assert_eq!(got[0].len(), LINE_CAP);
        assert_eq!(got[1], "next");
    }

    #[test]
    fn offsets_map_to_lines_and_back() {
        let text = numbered(3000);
        let ix = LineIndex::build(Cursor::new(&text)).unwrap();
        for line in [0, 1, 1023, 1024, 2999] {
            let at = ix.line_start(Cursor::new(&text), line).unwrap();
            assert!(text[at as usize..].starts_with(&format!("line {line}\n")));
            assert_eq!(ix.line_at(Cursor::new(&text), at + 3).unwrap(), (line, at));
        }
    }

    #[test]
    fn find_goes_both_ways_and_wraps() {
        let text = "alpha\nBeta\nalpha beta\n";
        let ix = LineIndex::build(Cursor::new(text)).unwrap();
        let find = |from, forward, case| ix.find(Cursor::new(text), "beta", from, forward, case).unwrap();
        assert_eq!(find(0, true, false), Some(6));
        assert_eq!(find(7, true, false), Some(17));
        assert_eq!(find(18, true, false), Some(6), "wraps round to the top");
        assert_eq!(find(0, true, true), Some(17), "match case");
        assert_eq!(find(17, false, false), Some(6), "backwards: before the last match");
        assert_eq!(find(6, false, false), Some(17), "and round the bottom");
        assert_eq!(ix.find(Cursor::new(text), "gamma", 0, true, false).unwrap(), None);
    }

    #[test]
    fn find_sees_matches_across_chunk_edges() {
        let mut text = "x".repeat(CHUNK as usize - 2);
        text.push_str("needle");
        let ix = LineIndex::build(Cursor::new(&text)).unwrap();
        let at = CHUNK - 2;
        assert_eq!(ix.find(Cursor::new(&text), "needle", 0, true, false).unwrap(), Some(at));
        assert_eq!(ix.find(Cursor::new(&text), "needle", ix.bytes, false, false).unwrap(), Some(at));
    }
}
