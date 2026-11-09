//! An open file (a tab): its [`Buffer`], highlighting kept in step with
//! edits, and the editing keys. Commands (save, undo, clipboard…) live in the
//! keymap; this handles typing and caret movement.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers};

use crate::app::TAB_WIDTH;
use crate::buffer::{self, Buffer};
use crate::syntax::{self, LineSpans};

/// Files up to this many lines re-highlight on every edit (a few ms);
/// bigger ones re-highlight in the background once typing pauses.
const SYNC_HIGHLIGHT_LINES: usize = 2000;
const ASYNC_HIGHLIGHT_DELAY: Duration = Duration::from_millis(120);

/// One screen row: `(line, start, end)`, `start..end` being char columns of
/// that line (the whole line when not wrapping).
pub type Row = (usize, usize, usize);

pub struct Tab {
    pub buf: Buffer,
    /// Highlighting for the text at `spans_rev` (may briefly lag the buffer).
    pub spans: Vec<LineSpans>,
    spans_rev: Option<u64>,
    job: Option<(u64, Receiver<Vec<LineSpans>>)>,
    /// Widest line in display columns, for horizontal scrolling.
    pub max_cols: usize,
    cols_rev: Option<u64>,
    pub scroll: egui::Vec2,
    /// Scroll the caret into view on the next frame.
    pub reveal: bool,
    pub last_edit: Instant,
    /// When the caret last moved (egui time), so it stays solid while typing.
    pub caret_epoch: f64,
    /// Where a mouse drag started, so dragging selects from there.
    pub drag_from: Option<usize>,
    /// Word-wrap layout for (revision, width).
    wrap: Option<(u64, usize, Vec<Row>)>,
    /// The file was deleted on disk (the tab shows it struck through).
    pub gone: bool,
    /// A change on disk not answered yet: asked when this tab is in front.
    pub disk_news: Option<buffer::DiskStatus>,
}

impl Tab {
    pub fn open(path: &Path) -> Result<Tab, String> {
        if let Some(reason) = buffer::unsupported_reason(path) {
            return Err(reason);
        }
        let buf = Buffer::from_path(path).map_err(|e| format!("{e:#}"))?;
        let mut tab = Tab {
            buf,
            spans: Vec::new(),
            spans_rev: None,
            job: None,
            max_cols: 0,
            cols_rev: None,
            scroll: egui::Vec2::ZERO,
            reveal: false,
            last_edit: Instant::now(),
            caret_epoch: 0.0,
            drag_from: None,
            wrap: None,
            gone: false,
            disk_news: None,
        };
        tab.refresh(None);
        Ok(tab)
    }

    /// Re-highlight from scratch (e.g. the language changed).
    pub fn invalidate_highlight(&mut self) {
        self.spans_rev = None;
        self.job = None;
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.buf.path.as_ref()
    }

    /// Bring highlighting and the width cache up to date with the text.
    /// `ctx` is for waking the UI when background work finishes.
    pub fn refresh(&mut self, ctx: Option<&egui::Context>) {
        let rev = self.buf.revision();
        if self.cols_rev != Some(rev) {
            let width = |c: char| match c {
                '\t' => TAB_WIDTH,
                '\n' | '\r' => 0,
                _ => 1,
            };
            self.max_cols = self
                .buf
                .rope
                .lines()
                .map(|l| l.chars().map(width).sum())
                .max()
                .unwrap_or(0);
            self.cols_rev = Some(rev);
        }

        if let Some((job_rev, rx)) = &self.job
            && let Ok(spans) = rx.try_recv()
        {
            self.spans = spans;
            self.spans_rev = Some(*job_rev);
            self.job = None;
        }
        if self.spans_rev == Some(rev) || self.job.as_ref().is_some_and(|(r, _)| *r == rev) {
            return;
        }
        let lang = self.buf.lang;
        if self.buf.line_count() <= SYNC_HIGHLIGHT_LINES || ctx.is_none() {
            self.spans = syntax::highlight(&self.buf.rope.to_string(), lang).spans;
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
        let (text, ctx) = (self.buf.rope.to_string(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(syntax::highlight(&text, lang).spans);
            ctx.request_repaint();
        });
        self.job = Some((rev, rx));
    }

    /// The screen rows when wrapping at `width` columns, cached until the
    /// text or the width changes.
    pub fn wrapped_rows(&mut self, width: usize) -> &[Row] {
        let rev = self.buf.revision();
        if !matches!(&self.wrap, Some((r, w, _)) if *r == rev && *w == width) {
            let rows = (0..self.buf.line_count())
                .flat_map(|l| {
                    self.buf
                        .wrap_line_ranges(l, width)
                        .into_iter()
                        .map(move |(s, e)| (l, s, e))
                })
                .collect();
            self.wrap = Some((rev, width, rows));
        }
        self.wrap
            .as_ref()
            .map(|w| w.2.as_slice())
            .unwrap_or_default()
    }

    /// Take the file's current contents from disk.
    pub fn reload(&mut self) -> anyhow::Result<()> {
        self.buf.reload_from_disk()?;
        self.reveal = true;
        Ok(())
    }

    /// The text to copy: the selection, or the whole current line (with its
    /// newline) when nothing is selected — like VS Code.
    pub fn copy_text(&self) -> String {
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
        assert!(!t.spans[0].is_empty());
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
        let rows = t.wrapped_rows(10).to_vec();
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
    fn binary_files_refuse_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.png");
        std::fs::write(&p, "x").unwrap();
        assert!(Tab::open(&p).is_err());
    }
}
