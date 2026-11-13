//! The embedded terminal's keyboard and mouse handling: egui events in, PTY
//! bytes out (ported from Oxru's terminal input). The pane itself — the PTY,
//! emulator, scrollback and selection — is [`TerminalPane`].

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Modifiers};

use crate::terminalpane::{TerminalPane, Waker};

/// How long after a keystroke terminal output still redraws immediately, so
/// the echo of what you type is never held back by the FPS cap.
pub(crate) const TYPING_WINDOW: Duration = Duration::from_millis(600);

/// Shared by all terminals: the FPS cap for unattended output, and when the
/// user last typed into any of them.
pub struct Throttle {
    pub fps: AtomicU32,
    last_input: Mutex<Option<Instant>>,
}

impl Throttle {
    pub fn new(fps: u32) -> Arc<Throttle> {
        Arc::new(Throttle {
            fps: AtomicU32::new(fps),
            last_input: Mutex::new(None),
        })
    }

    fn note_input(&self) {
        *self.last_input.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    /// How soon fresh output should be drawn: now while the user is typing,
    /// otherwise at the next frame the cap allows.
    pub fn delay(&self) -> Duration {
        let typing = self
            .last_input
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|t| t.elapsed() < TYPING_WINDOW);
        if typing {
            Duration::ZERO
        } else {
            Duration::from_micros(1_000_000 / self.fps.load(Ordering::Relaxed).max(1) as u64)
        }
    }
}

pub struct Terminal {
    pub pane: TerminalPane,
    throttle: Arc<Throttle>,
    /// Whether this terminal is on screen; output from hidden ones doesn't
    /// wake the UI (it's still parsed, and shows when you look).
    pub visible: Arc<AtomicBool>,
    /// Fractional wheel rows not yet applied.
    wheel: f32,
    /// A ⌥+key was just sent as Meta; drop the character it also typed.
    swallow_text: bool,
    /// The current mouse press is going to the program (mouse mode).
    pub mouse_forward: bool,
}

impl Terminal {
    pub fn spawn(
        cwd: &Path,
        rows: u16,
        cols: u16,
        ctx: &egui::Context,
        throttle: Arc<Throttle>,
    ) -> anyhow::Result<Terminal> {
        let ctx = ctx.clone();
        let visible = Arc::new(AtomicBool::new(true));
        let waker: Waker = {
            let (throttle, visible) = (throttle.clone(), visible.clone());
            Arc::new(move |urgent| {
                if urgent {
                    ctx.request_repaint();
                    crate::remote::soon(Duration::ZERO);
                } else if visible.load(Ordering::Relaxed) {
                    let delay = throttle.delay();
                    ctx.request_repaint_after(delay);
                    crate::remote::soon(delay);
                }
            })
        };
        let pane = TerminalPane::new(
            crate::terminalpane::folder_name(cwd),
            rows,
            cols,
            cwd,
            Some(waker),
        )?;
        Ok(Terminal {
            pane,
            throttle,
            visible,
            wheel: 0.0,
            swallow_text: false,
            mouse_forward: false,
        })
    }

    /// Apply this frame's typing to the shell.
    pub fn input(&mut self, events: &[Event]) {
        for e in events {
            match e {
                Event::Text(t) if !std::mem::take(&mut self.swallow_text) => {
                    self.send_typed(t.as_bytes())
                }
                Event::Ime(egui::ImeEvent::Commit(t)) => self.send_typed(t.as_bytes()),
                Event::Paste(t) => {
                    self.throttle.note_input();
                    self.pane.paste(t);
                }
                Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Only the text right after a Meta chord is its own: a
                    // dead ⌥E types none, and mustn't eat the next key's.
                    self.swallow_text = false;
                    self.key(*key, *physical_key, *modifiers)
                }
                _ => {}
            }
        }
    }

    /// Typing drops any mark and snaps back to the live bottom, like every terminal.
    fn send_typed(&mut self, bytes: &[u8]) {
        self.throttle.note_input();
        let p = &mut self.pane;
        p.clear_selection();
        p.scroll_to_bottom();
        p.send_input(bytes);
    }

    fn key(&mut self, key: Key, physical: Option<Key>, m: Modifiers) {
        let p = &mut self.pane;
        // Copy mode owns the keys: a free cursor for selecting, nothing reaches the shell.
        if p.copy_mode() {
            match key {
                Key::Escape | Key::Q => p.exit_copy_mode(),
                Key::ArrowLeft if m.alt => p.copy_move_word(-1, m.shift),
                Key::ArrowRight if m.alt => p.copy_move_word(1, m.shift),
                Key::ArrowLeft | Key::H => p.copy_move(0, -1, m.shift),
                Key::ArrowRight | Key::L => p.copy_move(0, 1, m.shift),
                Key::ArrowUp | Key::K => p.copy_move(-1, 0, m.shift),
                Key::ArrowDown | Key::J => p.copy_move(1, 0, m.shift),
                Key::PageUp => p.scroll_page(1),
                Key::PageDown => p.scroll_page(-1),
                _ => {}
            }
            return;
        }
        // macOS line editing on ⌘, mapped to readline: start/end of line, kill line.
        if m.mac_cmd {
            let seq: &[u8] = match key {
                Key::ArrowLeft => &[0x01],
                Key::ArrowRight => &[0x05],
                Key::Backspace => &[0x15],
                _ => return,
            };
            return self.send_typed(seq);
        }
        match key {
            // Shift+PageUp/Down page the scrollback; Shift+arrows mark text.
            Key::PageUp if m.shift => return p.scroll_page(1),
            Key::PageDown if m.shift => return p.scroll_page(-1),
            Key::ArrowUp if m.shift => return p.select_key(-1, 0),
            Key::ArrowDown if m.shift => return p.select_key(1, 0),
            Key::ArrowLeft if m.shift && m.alt => return p.select_word(-1),
            Key::ArrowRight if m.shift && m.alt => return p.select_word(1),
            Key::ArrowLeft if m.shift => return p.select_key(0, -1),
            Key::ArrowRight if m.shift => return p.select_key(0, 1),
            // ⌥/Ctrl+←/→ jump by word (readline meta-b / meta-f).
            Key::ArrowLeft if m.alt || m.ctrl => return self.send_typed(&[0x1b, b'b']),
            Key::ArrowRight if m.alt || m.ctrl => return self.send_typed(&[0x1b, b'f']),
            // ⌥↑/↓ enter copy mode.
            Key::ArrowUp | Key::ArrowDown if m.alt => {
                p.enter_copy_mode();
                return p.copy_move(if key == Key::ArrowUp { -1 } else { 1 }, 0, false);
            }
            // fn+↑/↓ (PageUp/Down) scroll our scrollback at the shell, but belong
            // to a full-screen program (vim, less) on the alternate screen.
            Key::PageUp | Key::PageDown if !p.on_alternate_screen() => {
                return p.scroll_page(if key == Key::PageUp { 1 } else { -1 });
            }
            _ => {}
        }
        // ⌥+letter/digit is Meta: ESC + the key (Option as Meta), not the
        // composed character macOS would type (⌥F → "ƒ").
        if m.alt && !m.ctrl {
            let k = physical.unwrap_or(key);
            if let Some(c) = key_char(k, m.shift) {
                self.swallow_text = true;
                return self.send_typed(&[0x1b, c as u8]);
            }
        }
        if let Some(bytes) = key_bytes(key, m) {
            self.send_typed(&bytes);
        }
    }

    /// Wheel over the terminal. A program in mouse mode gets wheel events; a
    /// pager on the alternate screen gets arrow keys; otherwise the wheel
    /// scrolls history. Shift always scrolls history.
    pub fn wheel(&mut self, dy_rows: f32, cell: (u16, u16), shift: bool) {
        self.wheel += dy_rows;
        let lines = self.wheel.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.wheel -= lines as f32;
        let p = &mut self.pane;
        let (up, n) = (lines > 0, lines.unsigned_abs().min(10));
        if !shift && p.wants_mouse() {
            for _ in 0..n {
                p.send_mouse(if up { 64 } else { 65 }, cell.1, cell.0, false);
            }
        } else if !shift && p.on_alternate_screen() {
            let seq: &[u8] = if up { b"\x1b[A" } else { b"\x1b[B" };
            for _ in 0..n {
                p.send_input(seq);
            }
        } else {
            p.scroll_lines(lines);
        }
    }
}

/// The directory a launched command `cd`s into first (`cd '<dir>' && …`),
/// ported from Oxru: anchored at the start, both quote styles, unquoted paths
/// up to the first separator.
pub fn cd_target(cmd: &str) -> Option<std::path::PathBuf> {
    let rest = cmd.trim_start().strip_prefix("cd ")?.trim_start();
    let path = match rest.chars().next()? {
        q @ ('\'' | '"') => rest[1..].split(q).next()?,
        _ => rest.split([' ', '\t', ';', '&', '|']).next()?,
    };
    (!path.is_empty()).then(|| std::path::PathBuf::from(path))
}

/// A short tab name for a launched command: its `cd` target's folder, else
/// the command's first word.
pub fn short_title(cmd: &str) -> String {
    match cd_target(cmd) {
        Some(dir) => crate::terminalpane::folder_name(&dir),
        None => cmd.split_whitespace().next().unwrap_or("term").to_string(),
    }
}

/// Wake the UI when a script queues a terminal request — a cheap file-size
/// check a few times a second, so requests land promptly without redrawing
/// while nothing happens.
pub fn watch_requests(ctx: &egui::Context) {
    let Some(path) = crate::termbridge::request_file() else {
        return;
    };
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let mut last = 0;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if len != last {
                last = len;
                ctx.request_repaint();
            }
        }
    });
}

/// Tab labels: `folder · command`, with duplicates numbered (Oxru's rule).
/// The number goes on the folder — `api 2 · sleep` — so it can't be read as
/// part of the command.
pub fn labels(names: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    names
        .iter()
        .map(|n| {
            let count = seen.entry(n).or_insert(0);
            *count += 1;
            match (*count, n.split_once(" \u{00b7} ")) {
                (1, _) => n.clone(),
                (c, Some((folder, cmd))) => format!("{folder} {c} \u{00b7} {cmd}"),
                (c, None) => format!("{n} {c}"),
            }
        })
        .collect()
}

/// Tiles for the grid view: about as many columns as rows, filled left to
/// right, top to bottom; a short last row stretches to the full width.
pub fn grid_rects(area: egui::Rect, n: usize, gap: f32) -> Vec<egui::Rect> {
    if n == 0 {
        return Vec::new();
    }
    let cols = (n as f32).sqrt().ceil() as usize;
    let rows = n.div_ceil(cols);
    let h = (area.height() - gap * (rows - 1) as f32) / rows as f32;
    let mut out = Vec::with_capacity(n);
    for r in 0..rows {
        let in_row = cols.min(n - r * cols);
        let w = (area.width() - gap * (in_row - 1) as f32) / in_row as f32;
        for c in 0..in_row {
            let min = egui::pos2(
                area.left() + c as f32 * (w + gap),
                area.top() + r as f32 * (h + gap),
            );
            out.push(egui::Rect::from_min_size(min, egui::vec2(w, h)));
        }
    }
    out
}

/// The ASCII character a letter/digit key types (for Meta chords).
fn key_char(key: Key, shift: bool) -> Option<char> {
    let name = key.name();
    let c = name.chars().next()?;
    if name.len() != 1 || !c.is_ascii_alphanumeric() {
        return None;
    }
    Some(if shift {
        c.to_ascii_uppercase()
    } else {
        c.to_ascii_lowercase()
    })
}

/// The bytes a non-text key sends to a PTY. Plain characters arrive as text
/// events, so only control chords and special keys are handled here.
pub fn key_bytes(key: Key, m: Modifiers) -> Option<Vec<u8>> {
    // Soft newlines for CLIs that take them (⌥Enter = ESC CR, ⇧Enter = LF).
    match key {
        Key::Enter if m.alt => return Some(vec![0x1b, b'\r']),
        Key::Enter if m.shift => return Some(vec![b'\n']),
        Key::Backspace if m.alt => return Some(vec![0x1b, 0x7f]),
        _ => {}
    }
    if m.ctrl
        && let Some(c) = key_char(key, false).filter(char::is_ascii_alphabetic)
    {
        return Some(vec![c as u8 - b'a' + 1]);
    }
    Some(match key {
        Key::Enter => vec![b'\r'],
        Key::Backspace => vec![0x7f],
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        Key::ArrowLeft => b"\x1b[D".to_vec(),
        Key::ArrowRight => b"\x1b[C".to_vec(),
        Key::ArrowUp => b"\x1b[A".to_vec(),
        Key::ArrowDown => b"\x1b[B".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        _ => return None,
    })
}

/// Whether a key chord should go to the shell rather than the app's keymap
/// while the terminal has focus: physical Ctrl chords are the shell's (^C,
/// ^R, ^L…) — except Ctrl+Tab, which switches tabs. ⌘ chords stay the app's.
pub fn belongs_to_shell(key: Key, m: Modifiers) -> bool {
    let mac = crate::keymap::mac_keys();
    // Off macOS, Ctrl doubles as the command key: only unshifted Ctrl chords
    // go to the shell, so Ctrl+Shift+C/V can still copy and paste.
    m.ctrl && key != Key::Tab && (mac || !m.shift)
}

/// Off macOS, Roxaruu's keys are Ctrl chords too, the shell's own. These few
/// still reach the app from a terminal (like VS Code's "skip the shell"
/// list): getting out of it and between places, tabs, saving, quitting, text
/// size. The shell keeps the rest (^C ^R ^W ^K ^A ^E…). On a Mac they're ⌘
/// chords, which never clash.
pub fn passes_the_shell(c: crate::keymap::Command) -> bool {
    use crate::keymap::Command::*;
    matches!(
        c,
        SwitchFocus | GoToTab(_) | ToggleSidebar | FocusSidebar | Save | SaveAll | Quit | ZoomIn | ZoomOut | ResetZoom
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    const ALT: Modifiers = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };

    #[test]
    fn ctrl_letter_is_control_code() {
        assert_eq!(key_bytes(Key::C, CTRL), Some(vec![3]));
        assert_eq!(key_bytes(Key::R, CTRL), Some(vec![18]));
    }

    #[test]
    fn special_keys_are_escape_sequences() {
        assert_eq!(
            key_bytes(Key::ArrowUp, Modifiers::NONE),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_bytes(Key::Delete, Modifiers::NONE),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(key_bytes(Key::Tab, SHIFT), Some(b"\x1b[Z".to_vec()));
        assert_eq!(
            key_bytes(Key::A, Modifiers::NONE),
            None,
            "plain letters come as text"
        );
    }

    #[test]
    fn enter_variants() {
        assert_eq!(key_bytes(Key::Enter, Modifiers::NONE), Some(vec![b'\r']));
        assert_eq!(key_bytes(Key::Enter, ALT), Some(vec![0x1b, b'\r']));
        assert_eq!(key_bytes(Key::Enter, SHIFT), Some(vec![b'\n']));
        assert_eq!(key_bytes(Key::Backspace, ALT), Some(vec![0x1b, 0x7f]));
    }

    #[test]
    fn meta_chars() {
        assert_eq!(key_char(Key::F, false), Some('f'));
        assert_eq!(key_char(Key::F, true), Some('F'));
        assert_eq!(key_char(Key::Num3, false), Some('3'));
        assert_eq!(key_char(Key::ArrowUp, false), None);
    }

    #[test]
    fn ctrl_chords_go_to_the_shell_but_not_ctrl_tab() {
        assert!(belongs_to_shell(Key::C, CTRL));
        assert!(!belongs_to_shell(Key::Tab, CTRL));
        let cmd = Modifiers {
            mac_cmd: true,
            command: true,
            ..Modifiers::NONE
        };
        assert!(!belongs_to_shell(Key::C, cmd));
    }

    #[test]
    fn cd_targets_and_titles() {
        assert_eq!(
            cd_target("cd '/a b/web' && npm run dev"),
            Some("/a b/web".into())
        );
        assert_eq!(cd_target("cd \"/srv\"; make"), Some("/srv".into()));
        assert_eq!(cd_target("cd /api&&cargo run"), Some("/api".into()));
        assert_eq!(
            cd_target("make && cd build"),
            None,
            "only a leading cd counts"
        );
        assert_eq!(short_title("cd '/x/server' && go run ."), "server");
        assert_eq!(short_title("npm run dev"), "npm");
    }

    #[test]
    fn throttle_caps_unattended_output_but_not_typing() {
        let t = Throttle::new(5);
        assert_eq!(t.delay(), Duration::from_millis(200));
        t.note_input();
        assert_eq!(t.delay(), Duration::ZERO);
    }

    /// Duplicate names get a number: "api", "api 2", "api 3".
    #[test]
    fn labels_disambiguate_duplicates() {
        let names = ["api", "web", "api", "api", "api · node", "api · node"].map(String::from);
        assert_eq!(
            labels(&names),
            ["api", "web", "api 2", "api 3", "api · node", "api 2 · node"]
        );
    }

    #[test]
    fn grid_is_left_right_then_full_width_below() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        assert_eq!(grid_rects(area, 1, 0.0), [area]);
        let r2 = grid_rects(area, 2, 0.0);
        assert!(
            r2[0].left() < r2[1].left() && r2[0].top() == r2[1].top(),
            "left | right"
        );
        let r3 = grid_rects(area, 3, 0.0);
        assert!(
            r3[0].top() == r3[1].top() && r3[2].top() > r3[0].top(),
            "two on top, one below"
        );
        assert_eq!(r3[2].width(), 100.0, "the last row stretches to full width");
        assert_eq!(grid_rects(area, 4, 0.0).len(), 4);
    }

    /// Output nobody is typing into waits for the FPS cap; the echo of typing
    /// asks for an immediate redraw.
    #[test]
    fn idle_output_waits_but_typed_echo_is_immediate() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let requests: Arc<Mutex<Vec<Duration>>> = Default::default();
        {
            let requests = requests.clone();
            ctx.set_request_repaint_callback(move |info| requests.lock().unwrap().push(info.delay));
        }
        let mut t = Terminal::spawn(dir.path(), 24, 80, &ctx, Throttle::new(5)).unwrap();
        while requests.lock().unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            requests.lock().unwrap()[0],
            Duration::from_millis(200),
            "the prompt arrived unattended"
        );
        // Type at the prompt, as a person would: a Windows shell prints well
        // before it reads input, and an echo that late isn't typing's any more.
        let mut shown = String::new();
        let mut since = Instant::now();
        while shown.trim().is_empty() || since.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(50));
            let now = t.pane.screen().contents();
            if now != shown {
                (shown, since) = (now, Instant::now());
            }
        }
        t.input(&[Event::Text("echo typed".into())]);
        while !requests.lock().unwrap().contains(&Duration::ZERO) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A real shell: what's typed runs and its output reaches the screen.
    #[test]
    fn a_typed_command_runs_and_prints() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut t = Terminal::spawn(dir.path(), 24, 80, &ctx, Throttle::new(60)).unwrap();
        t.input(&[Event::Text("echo hi-from-$((40+2))".into())]);
        t.input(&[Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        loop {
            t.pane.pump();
            if t.pane.screen().contents().contains("hi-from-42") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// ⌥E is a dead key on a Mac: Meta-e goes to the shell and no text
    /// follows, so the next key's text must not be swallowed in its place.
    #[test]
    // ponytail: on GitHub's Windows runners this terminal shows nothing at all
    // after an Alt+key sent as it starts (plain typing works there, see
    // a_typed_command_runs_and_prints); pending a look on a real Windows PC.
    #[cfg_attr(windows, ignore = "pending on Windows: blank console after an early Alt+key")]
    fn a_dead_meta_key_doesnt_eat_the_next_letter() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut t = Terminal::spawn(dir.path(), 24, 80, &ctx, Throttle::new(60)).unwrap();
        let key = |key, modifiers| Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        };
        t.input(&[key(Key::E, ALT)]);
        t.input(&[key(Key::Z, Modifiers::NONE), Event::Text("zqzq".into())]);
        // (However long the shell takes to start: Git Bash on Windows is slow.)
        while !t.pane.screen().contents().contains("zqzq") {
            t.pane.pump();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // ⌥F with its "ƒ" in one frame: the "ƒ" is still swallowed.
        t.input(&[key(Key::F, ALT), Event::Text("ƒ".into())]);
        assert!(!t.swallow_text, "the ƒ was taken");
    }
}
