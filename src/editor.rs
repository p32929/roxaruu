//! An open file (a tab): its [`Buffer`], highlighting kept in step with
//! edits, and the editing keys. Commands (save, undo, clipboard…) live in the
//! keymap; this handles typing and caret movement.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers};

use crate::app::TAB_WIDTH;
use crate::buffer::{self, Buffer};
use crate::huge::{self, HugeView};
use crate::syntax::{self, LineSpans};

/// Files up to this many lines re-highlight on every edit (a few ms);
/// bigger ones re-highlight in the background once typing pauses.
const SYNC_HIGHLIGHT_LINES: usize = 2000;
const ASYNC_HIGHLIGHT_DELAY: Duration = Duration::from_millis(120);
/// Files up to this many lines are highlighted, wrapped and measured whole.
/// Past it only the lines around the view are (`around`): the cost then
/// follows the screen, not the file.
const WHOLE_FILE_LINES: usize = 10_000;
/// Around the view: this many lines either side, in whole blocks so a
/// little scrolling doesn't redo anything.
const MARGIN: usize = 1000;
const BLOCK: usize = 500;

/// One screen row: `(line, start, end)`, `start..end` being char columns of
/// that line (the whole line when not wrapping).
pub type Row = (usize, usize, usize);

/// The part of a big file worth highlighting or laying out for `view`:
/// `MARGIN` lines either side of it, in whole `BLOCK`s, within `0..lines`.
pub fn around(view: &Range<usize>, lines: usize) -> Range<usize> {
    let start = view.start.saturating_sub(MARGIN) / BLOCK * BLOCK;
    let end = (view.end + MARGIN).div_ceil(BLOCK) * BLOCK;
    start.min(lines)..end.min(lines)
}

/// Whether `have` (from `around`) still serves `view`: it reaches half a
/// margin past it on both sides (or the file's ends).
pub fn serves(have: &Range<usize>, view: &Range<usize>, lines: usize) -> bool {
    have.start <= view.start.saturating_sub(MARGIN / 2)
        && have.end >= (view.end + MARGIN / 2).min(lines)
}

/// Word-wrapped screen rows for lines `from..to`; every other line of the
/// file counts as one row (laid out properly once scrolled near).
#[derive(Clone, Default)]
pub struct WrapLayout {
    pub from: usize,
    pub to: usize,
    pub rows: Vec<Row>,
    pub lines: usize,
}

impl WrapLayout {
    pub fn len(&self) -> usize {
        self.from + self.rows.len() + self.lines.saturating_sub(self.to)
    }

    pub fn row(&self, r: usize) -> Row {
        if r < self.from {
            return (r, 0, usize::MAX);
        }
        match self.rows.get(r - self.from) {
            Some(row) => *row,
            None => (r - self.from - self.rows.len() + self.to, 0, usize::MAX),
        }
    }

    /// The first row of `line`.
    pub fn first_row(&self, line: usize) -> usize {
        if line < self.from {
            line
        } else if line < self.to {
            self.from + self.rows.partition_point(|r| r.0 < line)
        } else {
            self.from + self.rows.len() + (line - self.to)
        }
    }

    /// The row showing char column `col` of `line`.
    pub fn row_of(&self, line: usize, col: usize) -> usize {
        let first = self.first_row(line);
        if !(self.from..self.to).contains(&line) {
            return first;
        }
        (first..self.from + self.rows.len())
            .take_while(|&r| self.rows[r - self.from].0 == line)
            .filter(|&r| self.rows[r - self.from].1 <= col)
            .last()
            .unwrap_or(first)
    }
}

/// Display columns of a line's text (a tab is `TAB_WIDTH`).
pub fn display_cols(chars: impl Iterator<Item = char>) -> usize {
    chars
        .map(|c| match c {
            '\t' => TAB_WIDTH,
            '\n' | '\r' => 0,
            _ => 1,
        })
        .sum()
}

pub struct Tab {
    pub buf: Buffer,
    /// A file too big to hold: shown straight from disk, read-only.
    pub huge: Option<HugeView>,
    /// Whether the file was read yet: a restored tab isn't until it's first
    /// shown (`ensure_loaded`), and meanwhile its `buf` is empty.
    loaded: bool,
    /// Highlighting of lines `spans_from..` for the text at `spans_rev` (may
    /// briefly lag the buffer). Read it with `line_spans`.
    spans: Vec<LineSpans>,
    spans_from: usize,
    spans_rev: Option<u64>,
    job: Option<(u64, usize, Receiver<Vec<LineSpans>>)>,
    /// Widest line in display columns, for horizontal scrolling.
    pub max_cols: usize,
    cols_rev: Option<u64>,
    /// Where each view is scrolled to: the window's, a browser's (`App::views`).
    pub scroll: [egui::Vec2; 2],
    /// Scroll the caret into view on the next frame (of every view).
    pub reveal: bool,
    /// Scroll each view to its `scroll` on its next frame (put back where
    /// you were, `go_to_place`), per view until it has drawn.
    pub restore_in: [bool; 2],
    /// `reveal`, per view, until that view has drawn: a phone looking in
    /// follows the caret too, whichever view draws first.
    pub reveal_in: [bool; 2],
    pub last_edit: Instant,
    /// When the caret last moved (egui time), so it stays solid while typing.
    pub caret_epoch: f64,
    /// Where a mouse drag started, so dragging selects from there.
    pub drag_from: Option<usize>,
    /// The lines drawn last frame: what a big file highlights and wraps around.
    pub view: Range<usize>,
    /// Word-wrap layout for (revision, width).
    wrap: Option<(u64, usize, WrapLayout)>,
    /// The file was deleted on disk (the tab shows it struck through).
    pub gone: bool,
    /// A change on disk not answered yet: asked when this tab is in front.
    pub disk_news: Option<buffer::DiskStatus>,
}

impl Tab {
    /// A tab for `path` that reads nothing until it's shown.
    pub fn lazy(path: &Path) -> Tab {
        Tab {
            buf: Buffer::unloaded(path),
            huge: None,
            loaded: false,
            spans: Vec::new(),
            spans_from: 0,
            spans_rev: None,
            job: None,
            max_cols: 0,
            cols_rev: None,
            scroll: [egui::Vec2::ZERO; 2],
            reveal_in: [false; 2],
            reveal: false,
            restore_in: [false; 2],
            last_edit: Instant::now(),
            caret_epoch: 0.0,
            drag_from: None,
            view: 0..0,
            wrap: None,
            gone: false,
            disk_news: None,
        }
    }

    pub fn open(path: &Path) -> Result<Tab, String> {
        let mut tab = Tab::lazy(path);
        tab.load(false)?;
        Ok(tab)
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Read the file if that hasn't happened yet (a restored tab coming into
    /// view). Tried once: an error leaves the tab empty and says why.
    pub fn ensure_loaded(&mut self) -> Result<(), String> {
        if self.loaded {
            return Ok(());
        }
        self.load(false)
    }

    /// Read the file: into memory, or (too big, unless `whole`) as the
    /// read-only view that reads only what's on screen.
    pub fn load(&mut self, whole: bool) -> Result<(), String> {
        self.loaded = true;
        let Some(path) = self.buf.path.clone() else {
            return Ok(());
        };
        if let Some(reason) = buffer::unsupported_reason(&path) {
            return Err(reason);
        }
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        if !whole && size > huge::HUGE_BYTES {
            return self.open_huge(path);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Can't open {} — {e}", self.buf.name()))?;
        let mut widest = (0, 0); // (bytes, display columns)
        for line in text.split('\n') {
            widest = (widest.0.max(line.len()), widest.1.max(display_cols(line.chars())));
        }
        if !whole && widest.0 > huge::LONG_LINE {
            return self.open_huge(path);
        }
        let (id, lang) = (self.buf.id, self.buf.lang);
        self.buf = Buffer::from_text(&path, &text);
        (self.buf.id, self.buf.lang) = (id, lang);
        self.huge = None;
        self.max_cols = widest.1;
        self.cols_rev = Some(self.buf.revision());
        self.wrap = None;
        self.invalidate_highlight();
        if self.buf.line_count() <= SYNC_HIGHLIGHT_LINES {
            self.refresh(None); // coloured from the first frame
        }
        Ok(())
    }

    fn open_huge(&mut self, path: PathBuf) -> Result<(), String> {
        let mut view =
            HugeView::open(path).map_err(|e| format!("Can't open {} — {e}", self.buf.name()))?;
        // Keep the place when it's re-read after a change on disk.
        if let Some(old) = &self.huge {
            view.top = old.top.min(view.lines().saturating_sub(1));
            view.cursor = old.cursor.min(view.lines().saturating_sub(1));
        }
        if let Some(path) = self.buf.path.clone() {
            let lang = self.buf.lang;
            self.buf = Buffer::unloaded(&path);
            self.buf.lang = lang;
        }
        self.buf.rebaseline_disk(); // so changes on disk are noticed
        self.huge = Some(view);
        self.invalidate_highlight();
        Ok(())
    }

    /// Re-highlight from scratch (e.g. the language changed).
    pub fn invalidate_highlight(&mut self) {
        self.spans_rev = None;
        self.job = None;
        if let Some(h) = &mut self.huge {
            h.spans = None;
        }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.buf.path.as_ref()
    }

    /// Lines in the file (a huge one's too).
    pub fn line_count(&self) -> usize {
        self.huge.as_ref().map_or(self.buf.line_count(), HugeView::lines)
    }

    /// The caret's line and column.
    /// Where you are in it (the window's view), to come back to.
    pub fn place(&self) -> crate::session::Place {
        let (line, col) = self.caret();
        crate::session::Place { line, col, scroll: (self.scroll[0].x, self.scroll[0].y) }
    }

    /// Back where you were: the caret, and the view scrolled as it was (not
    /// to the caret), in every view.
    pub fn go_to_place(&mut self, place: crate::session::Place) {
        self.go_to(place.line, place.col..place.col);
        self.scroll = [egui::vec2(place.scroll.0, place.scroll.1); 2];
        self.reveal = false;
        self.reveal_in = [false; 2];
        self.restore_in = [true; 2];
    }

    pub fn caret(&self) -> (usize, usize) {
        match &self.huge {
            Some(h) => (h.cursor, 0),
            None => (self.buf.cursor_row(), self.buf.cursor_col()),
        }
    }

    /// The text of line `l`, without its line break.
    pub fn line_text(&self, l: usize) -> String {
        match &self.huge {
            Some(h) => h.line(l),
            None if l < self.buf.line_count() => self
                .buf
                .rope
                .line(l)
                .to_string()
                .trim_end_matches(['\n', '\r'])
                .to_string(),
            None => String::new(),
        }
    }

    /// Put the caret on `line`, selecting char columns `cols` of it (an
    /// empty range: just the caret, there), and show it.
    pub fn go_to(&mut self, line: usize, cols: Range<usize>) {
        if let Some(h) = &mut self.huge {
            h.go_to(line);
            return;
        }
        let b = &mut self.buf;
        let line = line.min(b.line_count().saturating_sub(1));
        let at = b.rope.line_to_char(line);
        let len = b.line_len_chars(line);
        b.clear_extra_carets();
        b.select_range(at + cols.start.min(len), at + cols.end.min(len));
        if cols.is_empty() {
            b.clear_selection();
        }
        self.reveal = true;
    }

    /// Highlighting for line `l` (empty until it's been worked out).
    pub fn line_spans(&self, l: usize) -> &[(Range<usize>, crate::syntax::Token)] {
        l.checked_sub(self.spans_from)
            .and_then(|i| self.spans.get(i))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Bring highlighting and the width cache up to date with the text.
    /// `ctx` is for waking the UI when background work finishes.
    pub fn refresh(&mut self, ctx: Option<&egui::Context>) {
        if !self.buf.is_loaded() {
            return; // not read yet, or a huge file (drawn its own way)
        }
        let rev = self.buf.revision();
        let lines = self.buf.line_count();
        let whole = lines <= WHOLE_FILE_LINES;
        if self.cols_rev != Some(rev) {
            let cols = |l: usize| display_cols(self.buf.rope.line(l).chars());
            self.max_cols = if whole {
                (0..lines).map(cols).max().unwrap_or(0)
            } else {
                // A big file: only lines being edited can have grown, and
                // they're on screen or under a caret.
                // ponytail: never shrinks until the file is re-read.
                let carets = std::iter::once(self.buf.cursor)
                    .chain(self.buf.extra_carets().iter().map(|c| c.cursor))
                    .map(|c| self.buf.rope.char_to_line(c));
                let shown = self.view.start.min(lines)..self.view.end.min(lines);
                carets.chain(shown).map(cols).fold(self.max_cols, usize::max)
            };
            self.cols_rev = Some(rev);
        }

        if let Some((job_rev, from, rx)) = &self.job
            && let Ok(spans) = rx.try_recv()
        {
            self.spans = spans;
            self.spans_from = *from;
            self.spans_rev = Some(*job_rev);
            self.job = None;
        }
        let want = if whole {
            0..lines
        } else {
            around(&self.view, lines)
        };
        let have = self.spans_from..self.spans_from + self.spans.len();
        let current = self.spans_rev == Some(rev) && (whole || serves(&have, &self.view, lines));
        let coming = self
            .job
            .as_ref()
            .is_some_and(|(r, from, _)| *r == rev && *from == want.start);
        if current || coming {
            return;
        }
        let lang = self.buf.lang;
        let text = |b: &Buffer| {
            if whole {
                return b.rope.to_string();
            }
            let at = |l: usize| {
                if l < lines {
                    b.rope.line_to_char(l)
                } else {
                    b.rope.len_chars()
                }
            };
            b.rope.slice(at(want.start)..at(want.end)).to_string()
        };
        if lines <= SYNC_HIGHLIGHT_LINES || ctx.is_none() {
            self.spans = syntax::highlight(&text(&self.buf), lang).spans;
            self.spans_from = want.start;
            self.spans_rev = Some(rev);
            return;
        }
        // Big file: wait for a pause in typing, then highlight off-thread.
        let Some(ctx) = ctx else { return };
        let idle = self.last_edit.elapsed();
        if idle < ASYNC_HIGHLIGHT_DELAY {
            ctx.request_repaint_after(ASYNC_HIGHLIGHT_DELAY - idle);
            return;
        }
        let (tx, rx) = channel();
        let (text, ctx) = (text(&self.buf), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(syntax::highlight(&text, lang).spans);
            ctx.request_repaint();
        });
        self.job = Some((rev, want.start, rx));
    }

    /// The screen rows when wrapping at `width` columns, cached until the
    /// text or the width changes: every line of a small file, only those
    /// around `near` of a big one. Also how many rows the line at the top of
    /// the view moved when that part changed, so the view can stay put.
    pub fn wrap_layout(&mut self, width: usize, near: Range<usize>) -> (&WrapLayout, isize) {
        let rev = self.buf.revision();
        let lines = self.buf.line_count();
        let whole = lines <= WHOLE_FILE_LINES;
        let fits = |l: &WrapLayout| whole || serves(&(l.from..l.to), &near, lines);
        let mut shift = 0;
        if !matches!(&self.wrap, Some((r, w, l)) if *r == rev && *w == width && fits(l)) {
            let span = if whole { 0..lines } else { around(&near, lines) };
            let rows = span
                .clone()
                .flat_map(|l| {
                    self.buf
                        .wrap_line_ranges(l, width)
                        .into_iter()
                        .map(move |(s, e)| (l, s, e))
                })
                .collect();
            let layout = WrapLayout {
                from: span.start,
                to: span.end,
                rows,
                lines,
            };
            if !whole
                && let Some((_, w, old)) = &self.wrap
                && *w == width
            {
                let top = self.view.start;
                shift = layout.first_row(top) as isize - old.first_row(top) as isize;
            }
            self.wrap = Some((rev, width, layout));
        }
        let laid = self.wrap.get_or_insert_with(|| (rev, width, WrapLayout::default()));
        (&laid.2, shift)
    }

    /// Take the file's current contents from disk (not read yet: nothing to do).
    pub fn reload(&mut self) -> anyhow::Result<()> {
        if !self.loaded {
            return Ok(());
        }
        if self.huge.is_some() {
            self.load(false).map_err(anyhow::Error::msg)?;
        } else {
            self.buf.reload_from_disk()?;
        }
        self.reveal = true;
        Ok(())
    }

    /// The text to copy: the selection, or the whole current line (with its
    /// newline) when nothing is selected — like VS Code.
    pub fn copy_text(&self) -> String {
        if let Some(h) = &self.huge {
            return h.line(h.cursor) + "\n";
        }
        if let Some(s) = self.buf.selected_text() {
            return s;
        }
        let line = self.buf.rope.line(self.buf.cursor_row()).to_string();
        if line.ends_with('\n') {
            line
        } else {
            line + "\n"
        }
    }

    /// Cut: the selection, or the whole current line.
    pub fn cut(&mut self) -> String {
        let text = self.copy_text();
        if self.buf.selection().is_none() {
            let row = self.buf.cursor_row();
            let start = self.buf.rope.line_to_char(row);
            let end = if row + 1 < self.buf.line_count() {
                self.buf.rope.line_to_char(row + 1)
            } else {
                self.buf.rope.len_chars()
            };
            self.buf.select_range(start, end);
        }
        self.buf.delete_selection();
        self.edited();
        text
    }

    pub fn paste(&mut self, text: &str) {
        self.buf.insert_str(&text.replace("\r\n", "\n"));
        self.edited();
    }

    pub fn edited(&mut self) {
        self.last_edit = Instant::now();
        self.reveal = true;
    }

    /// Apply this frame's typing and caret keys. `page` is rows per screen.
    pub fn input(&mut self, events: &[Event], page: usize) {
        for e in events {
            match e {
                Event::Text(t) => self.type_text(t),
                Event::Ime(egui::ImeEvent::Commit(t)) => self.type_text(t),
                Event::Paste(t) => self.paste(t),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => self.key(*key, *modifiers, page),
                _ => {}
            }
        }
    }

    fn type_text(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if !text.is_empty() {
            self.buf.insert_str(&text);
            self.edited();
        }
    }

    fn key(&mut self, key: Key, m: Modifiers, page: usize) {
        // ⌘ works anywhere Ctrl does (Oxru's rule).
        let cmd = m.ctrl || m.mac_cmd;
        let b = &mut self.buf;
        let mv = |b: &mut Buffer, f: &dyn Fn(&mut Buffer)| {
            if m.shift {
                b.begin_selection();
            } else {
                b.clear_selection();
            }
            f(b);
        };
        match key {
            Key::ArrowLeft if cmd => mv(b, &Buffer::move_home),
            Key::ArrowRight if cmd => mv(b, &Buffer::move_end),
            Key::ArrowLeft if m.alt => mv(b, &Buffer::move_word_left),
            Key::ArrowRight if m.alt => mv(b, &Buffer::move_word_right),
            Key::ArrowLeft => mv(b, &Buffer::move_left),
            Key::ArrowRight => mv(b, &Buffer::move_right),
            Key::ArrowUp if cmd => mv(b, &Buffer::move_doc_start),
            Key::ArrowDown if cmd => mv(b, &Buffer::move_doc_end),
            Key::ArrowUp => mv(b, &Buffer::move_up),
            Key::ArrowDown => mv(b, &Buffer::move_down),
            Key::Home if cmd => mv(b, &Buffer::move_doc_start),
            Key::End if cmd => mv(b, &Buffer::move_doc_end),
            Key::Home => mv(b, &Buffer::move_home),
            Key::End => mv(b, &Buffer::move_end),
            Key::PageUp => mv(b, &|b| (0..page).for_each(|_| b.move_up())),
            Key::PageDown => mv(b, &|b| (0..page).for_each(|_| b.move_down())),
            Key::Escape => {
                b.clear_extra_carets();
                b.clear_selection();
            }
            _ if cmd && !matches!(key, Key::Backspace | Key::Delete) => return, // commands, not typing
            Key::Backspace if m.alt => b.delete_word_left(),
            Key::Backspace if cmd => b.delete_to_line_start(),
            Key::Backspace => b.backspace(),
            Key::Delete => b.delete(),
            Key::Enter => {
                // Keep the current line's indentation.
                let line = b.rope.line(b.cursor_row()).to_string();
                let indent: String = line
                    .chars()
                    .take(b.cursor_col())
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                b.insert_str(&format!("\n{indent}"));
            }
            Key::Tab if m.shift => b.outdent_selection(),
            Key::Tab
                if b.selection()
                    .is_some_and(|(s, e)| b.rope.char_to_line(s) != b.rope.char_to_line(e)) =>
            {
                b.indent_selection()
            }
            Key::Tab => {
                let unit = b.indent_unit().max(1);
                b.insert_str(&" ".repeat(unit - b.cursor_col() % unit));
            }
            _ => return,
        }
        self.edited();
    }
}

/// The char offset within `line` closest to display column `col`.
pub fn col_to_char(line: &str, col: f32) -> usize {
    let mut x = 0.0;
    for (i, c) in line.trim_end_matches(['\n', '\r']).chars().enumerate() {
        let w = if c == '\t' { TAB_WIDTH as f32 } else { 1.0 };
        if col < x + w / 2.0 {
            return i;
        }
        x += w;
    }
    line.trim_end_matches(['\n', '\r']).chars().count()
}

/// The display column of char offset `ch` within `line`.
pub fn char_to_col(line: &str, ch: usize) -> usize {
    line.chars()
        .take(ch)
        .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(text: &str) -> (tempfile::TempDir, Tab) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.rs");
        std::fs::write(&path, text).unwrap();
        let t = Tab::open(&path).unwrap();
        (dir, t)
    }

    fn key(k: Key) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    fn key_with(k: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn typing_inserts_and_marks_modified() {
        let (_d, mut t) = tab("fn x() {}\n");
        t.input(&[Event::Text("ab".into())], 10);
        assert_eq!(t.buf.rope.to_string(), "abfn x() {}\n");
        assert!(t.buf.modified);
    }

    #[test]
    fn enter_keeps_indentation() {
        let (_d, mut t) = tab("    let x = 1;");
        t.input(
            &[key(Key::End), key(Key::Enter), Event::Text("y".into())],
            10,
        );
        assert_eq!(t.buf.rope.to_string(), "    let x = 1;\n    y");
    }

    #[test]
    fn shift_arrows_select_and_typing_replaces() {
        let (_d, mut t) = tab("hello world");
        let shift = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        t.input(
            &[
                key_with(Key::ArrowRight, shift),
                key_with(Key::ArrowRight, shift),
            ],
            10,
        );
        assert_eq!(t.buf.selected_text().as_deref(), Some("he"));
        t.input(&[Event::Text("J".into())], 10);
        assert_eq!(t.buf.rope.to_string(), "Jllo world");
    }

    #[test]
    fn cmd_letters_are_not_typed() {
        let (_d, mut t) = tab("x");
        let cmd = Modifiers {
            mac_cmd: true,
            command: true,
            ..Modifiers::NONE
        };
        t.input(&[key_with(Key::S, cmd)], 10);
        assert!(!t.buf.modified);
    }

    #[test]
    fn copy_and_cut_whole_line_without_selection() {
        let (_d, mut t) = tab("one\ntwo\nthree");
        t.input(&[key(Key::ArrowDown)], 10);
        assert_eq!(t.copy_text(), "two\n");
        assert_eq!(t.cut(), "two\n");
        assert_eq!(t.buf.rope.to_string(), "one\nthree");
    }

    #[test]
    fn save_ends_the_undo_group() {
        let (_d, mut t) = tab("x");
        t.input(&[Event::Text("ab".into())], 10);
        t.buf.save().unwrap();
        t.input(&[Event::Text("cd".into())], 10);
        t.buf.undo();
        assert_eq!(t.buf.rope.to_string(), "abx");
        assert!(!t.buf.modified, "undo back to the saved text is clean");
    }

    #[test]
    fn paste_normalises_crlf() {
        let (_d, mut t) = tab("");
        t.paste("a\r\nb");
        assert_eq!(t.buf.rope.to_string(), "a\nb");
    }

    #[test]
    fn tab_key_indents_to_the_next_stop() {
        let (_d, mut t) = tab("  x\n    y\n");
        // Detected unit is 2 here: from column 0, Tab inserts 2 spaces.
        t.input(&[key(Key::Tab)], 10);
        assert!(t.buf.rope.to_string().starts_with("    x"));
    }

    #[test]
    fn highlighting_follows_edits() {
        let (_d, mut t) = tab("let x = 1;");
        t.refresh(None);
        assert!(!t.line_spans(0).is_empty());
        t.input(
            &[key(Key::End), key(Key::Enter), Event::Text("fn".into())],
            10,
        );
        t.refresh(None);
        assert_eq!(t.spans.len(), 2);
    }

    #[test]
    fn column_mapping_counts_tabs() {
        assert_eq!(char_to_col("\tab", 2), TAB_WIDTH + 1);
        assert_eq!(col_to_char("\tab", (TAB_WIDTH + 1) as f32), 2);
        assert_eq!(col_to_char("ab\n", 9.0), 2);
    }

    #[test]
    fn wrapped_rows_split_long_lines_only() {
        let (_d, mut t) = tab("short\nthe quick brown fox jumps\n");
        let rows = t.wrap_layout(10, 0..2).0.rows.clone();
        assert_eq!(rows[0], (0, 0, 5));
        assert!(
            rows.iter().filter(|r| r.0 == 1).count() > 1,
            "the long line wraps: {rows:?}"
        );
        assert_eq!(
            rows.last().unwrap().0,
            2,
            "the empty last line still gets a row"
        );
    }

    #[test]
    fn around_the_view_in_whole_blocks() {
        assert_eq!(around(&(0..40), 100_000), 0..1500);
        assert_eq!(around(&(5200..5240), 100_000), 4000..6500);
        assert_eq!(around(&(5200..5240), 5300), 4000..5300, "clamped to the file");
        let have = around(&(5200..5240), 100_000);
        assert!(serves(&have, &(5300..5340), 100_000), "a little scrolling keeps it");
        assert!(!serves(&have, &(6200..6240), 100_000), "scrolling near its edge doesn't");
        assert!(serves(&(4000..5300), &(5250..5300), 5300), "the file's end counts as covered");
    }

    #[test]
    fn a_partial_wrap_layout_counts_other_lines_as_one_row() {
        // Lines 10..12 laid out (line 10 in two rows), 20 lines in all.
        let l = WrapLayout {
            from: 10,
            to: 12,
            rows: vec![(10, 0, 5), (10, 5, 9), (11, 0, 3)],
            lines: 20,
        };
        assert_eq!(l.len(), 21);
        assert_eq!(l.row(3), (3, 0, usize::MAX));
        assert_eq!(l.row(11), (10, 5, 9));
        assert_eq!(l.row(13), (12, 0, usize::MAX));
        assert_eq!(l.row(20), (19, 0, usize::MAX));
        assert_eq!((l.first_row(5), l.first_row(11), l.first_row(19)), (5, 12, 20));
        assert_eq!((l.row_of(10, 2), l.row_of(10, 7), l.row_of(15, 3)), (10, 11, 16));
    }

    #[test]
    fn a_lazy_tab_reads_nothing_until_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        let mut t = Tab::lazy(&path);
        assert!(!t.is_loaded() && t.buf.rope.len_chars() == 0);
        assert_eq!(t.buf.name(), "t.rs", "named without reading");
        assert!(t.buf.lang.is_some());
        assert!(t.buf.save().is_err(), "an unread tab never writes over its file");
        t.reload().unwrap();
        assert!(!t.is_loaded(), "a change on disk doesn't read it either");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() {}\n");
        t.ensure_loaded().unwrap();
        assert!(t.is_loaded());
        assert_eq!(t.buf.rope.to_string(), "fn main() {}\n");
        assert!(!t.line_spans(0).is_empty(), "highlighted from the start");
        // A file gone before it was shown: tried once, says why.
        let mut t = Tab::lazy(&dir.path().join("gone.rs"));
        assert!(t.ensure_loaded().is_err());
        assert!(t.ensure_loaded().is_ok() && t.is_loaded());
    }

    #[test]
    fn too_big_or_too_wide_opens_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let wide = dir.path().join("min.js");
        std::fs::write(&wide, format!("a\n{}\nb", "x".repeat(huge::LONG_LINE + 1))).unwrap();
        let t = Tab::open(&wide).unwrap();
        assert_eq!(t.huge.as_ref().map(HugeView::lines), Some(3));
        assert!(!t.buf.is_loaded() && t.line_count() == 3);
        assert_eq!(t.line_text(2), "b");
        let mut t = t;
        t.load(true).unwrap();
        assert!(t.huge.is_none() && t.buf.is_loaded(), "unless asked for whole");
        assert_eq!(t.line_count(), 3);
    }

    #[test]
    fn a_big_file_highlights_around_the_view() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.rs");
        let text: String = (0..30_000).map(|i| format!("let x{i} = {i};\n")).collect();
        std::fs::write(&path, text).unwrap();
        let mut t = Tab::open(&path).unwrap();
        t.view = 20_000..20_050;
        t.refresh(None);
        assert!(t.line_spans(0).is_empty(), "far from the view: not highlighted");
        assert!(!t.line_spans(20_010).is_empty());
        assert!(t.spans.len() < 4000, "only around the view: {}", t.spans.len());
        // Typing on a wide line widens without measuring every line.
        t.buf.cursor = t.buf.rope.line_to_char(20_010);
        t.buf.insert_str(&"y".repeat(500));
        t.refresh(None);
        assert!(t.max_cols >= 500);
    }

    #[test]
    fn binary_files_refuse_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.png");
        std::fs::write(&p, "x").unwrap();
        assert!(Tab::open(&p).is_err());
    }
}
