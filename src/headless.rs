//! Headless walkthroughs: drive the real app with simulated input and render
//! frames off-screen to PNGs — no window, no focus stealing. Ignored by
//! default; run with a throwaway HOME so real config is never touched:
//!
//! ```sh
//! HOME=/tmp/rx-home SHOTS=/tmp/rx-shots cargo test --release headless -- --ignored
//! ```

use std::path::PathBuf;

use eframe::egui::{self, Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use crate::Root;
use crate::app::App;

fn harness(target: Option<PathBuf>) -> Harness<'static, Root> {
    Harness::builder()
        .with_size(egui::vec2(1280.0, 820.0))
        .wgpu()
        .build_eframe(move |cc| Root::new(App::new(&cc.egui_ctx, target)))
}

/// Run a few frames (and give background work a moment), then save a PNG.
fn shot(h: &mut Harness<'static, Root>, name: &str) {
    for _ in 0..4 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    let dir = PathBuf::from(std::env::var("SHOTS").unwrap_or_else(|_| "target/shots".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = h.render().unwrap();
    img.save(dir.join(format!("{name}.png"))).unwrap();
}

/// Every key on screen works from where the keyboard is now: pressing it
/// runs its command (a key shown but left to the editor or shell is a lie).
fn visible_keys_work(h: &mut Harness<'static, Root>, place: &str) {
    crate::keymap::record_shown(true);
    h.step();
    let mut shown = crate::keymap::record_shown(false);
    shown.sort_by_key(|c| format!("{c:?}"));
    shown.dedup();
    let mut dead = Vec::new();
    for command in shown {
        let Some((key, mods)) = app(h).config.keymap.combo_for(command).map(|c| c.press()) else {
            continue;
        };
        crate::app::dry_run(true);
        press(h, mods, key);
        if crate::app::dry_run(false) != [command] {
            dead.push(command);
        }
    }
    assert!(dead.is_empty(), "shown in {place} but their keys do nothing there: {dead:?}");
}

/// Reads of a browser's link (30 ms each) with nothing new before its
/// picture counts as settled. Drawing on the CPU (CI's Linux): past
/// `remote::SETTLED`, when the sharp frame follows the quick ones (and no
/// caret blinks there to keep frames coming).
fn quiet_reads(h: &Harness<'static, Root>) -> u32 {
    if h.state().1.software { (crate::remote::SETTLED.as_millis() / 30) as u32 + 10 } else { 10 }
}

fn press(h: &mut Harness<'static, Root>, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.step();
}

fn type_text(h: &mut Harness<'static, Root>, text: &str) {
    h.event(Event::Text(text.into()));
    h.step();
}

/// Where the editor's text area starts: under the title bar (32), the
/// tab strip (36) and its row of tab keys (26).
const EDITOR_TOP: f32 = 32.0 + 36.0 + 26.0;

/// The command key: ⌘ on macOS, Ctrl elsewhere (as the app's keys are).
const CMD: Modifiers = Modifiers {
    mac_cmd: cfg!(target_os = "macos"),
    ctrl: !cfg!(target_os = "macos"),
    command: true,
    ..Modifiers::NONE
};
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::NONE
};
const ALT: Modifiers = Modifiers {
    alt: true,
    ..Modifiers::NONE
};
const CMD_SHIFT: Modifiers = Modifiers {
    shift: true,
    ..CMD
};

/// A key as this system shows it (`"ctrl+o"`: ⌘O on macOS, Ctrl+O elsewhere).
fn label(spec: &str) -> String {
    crate::keymap::Combo::parse(spec).unwrap().label()
}

#[test]
#[ignore]
fn headless_welcome() {
    let mut h = harness(None);
    shot(&mut h, "01-welcome");
    visible_keys_work(&mut h, "the welcome page");
    // The real app icon is the welcome mark, not a stand-in.
    assert!(!app(&h).app_icon.is_empty(), "the app icon loaded");
}

/// `PROJECT` = a file inside a git repo (the walkthrough opens its repo).
#[test]
#[ignore]
fn headless_project_walkthrough() {
    let file = PathBuf::from(std::env::var("PROJECT").unwrap());
    let mut h = harness(Some(file));
    shot(&mut h, "10-code");

    // Go to File: browse tree, then search, then open.
    press(&mut h, ALT, Key::F);
    shot(&mut h, "11-files-tree");
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    shot(&mut h, "12-files-expanded");
    type_text(&mut h, "app");
    shot(&mut h, "13-files-search");
    press(&mut h, Modifiers::NONE, Key::Enter);
    shot(&mut h, "14-opened");
    let app = &h.state().0;
    assert!(
        app.active_tab()
            .is_some_and(|t| app.tab_rel(t).ends_with("app.rs"))
    );

    // Keyboard scrolling.
    for _ in 0..5 {
        press(&mut h, Modifiers::NONE, Key::PageDown);
    }
    shot(&mut h, "15-paged");

    // Command palette, then the theme picker with live preview and cancel.
    press(&mut h, CMD_SHIFT, Key::P);
    shot(&mut h, "16-palette");
    type_text(&mut h, "select theme");
    press(&mut h, Modifiers::NONE, Key::Enter);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    shot(&mut h, "17-theme-preview");
    assert_eq!(h.state().0.config.theme.name, "Oxide Light");
    press(&mut h, Modifiers::NONE, Key::Escape);
    shot(&mut h, "18-theme-cancelled");
    assert_eq!(h.state().0.config.theme.name, "Oxide");

    // Recent folders.
    press(&mut h, ALT, Key::O);
    shot(&mut h, "19-recent");
}

fn text_of(h: &Harness<'static, Root>) -> String {
    h.state()
        .0
        .active_tab()
        .map(|t| t.buf.rope.to_string())
        .unwrap_or_default()
}

/// Click (press + release) at a window position, optionally dragging to `to`.
fn click(h: &mut Harness<'static, Root>, at: egui::Pos2, to: Option<egui::Pos2>) {
    let button = |pos, pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    h.event(Event::PointerMoved(at));
    h.step();
    h.event(button(at, true));
    h.step();
    let end = to.unwrap_or(at);
    if let Some(to) = to {
        for k in 1..=4 {
            h.event(Event::PointerMoved(at + (to - at) * (k as f32 / 4.0)));
            h.step();
        }
    }
    h.event(button(end, false));
    h.step();
}

/// Step 3: editing, saving, undo, selection, clipboard, tabs, the unsaved
/// prompt, reopen, quit-with-unsaved, and session restore.
#[test]
#[ignore]
fn headless_editing_walkthrough() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join("a.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
    std::fs::write(root.join("b.rs"), "struct B;\n").unwrap();
    let a = root.join("a.rs");

    let mut h = harness(Some(a.clone()));
    no_sidebar(&mut h); // this walkthrough clicks at fixed spots
    shot(&mut h, "30-open");

    // Type, then save.
    type_text(&mut h, "// hello");
    press(&mut h, Modifiers::NONE, Key::Enter);
    shot(&mut h, "31-typed-dirty");
    assert!(h.state().0.active_tab().unwrap().buf.modified);
    press(&mut h, CMD, Key::S);
    shot(&mut h, "32-saved");
    assert!(
        std::fs::read_to_string(&a)
            .unwrap()
            .starts_with("// hello\n")
    );
    assert!(!h.state().0.active_tab().unwrap().buf.modified);

    // Undo a run of typing in one step; back to the saved text is clean again.
    type_text(&mut h, "abc");
    press(&mut h, CMD, Key::Z);
    assert!(text_of(&h).starts_with("// hello\nfn main"));
    assert!(
        !h.state().0.active_tab().unwrap().buf.modified,
        "undo to saved = clean"
    );
    press(&mut h, Modifiers { shift: true, ..CMD }, Key::Z);
    assert!(text_of(&h).contains("abcfn main"));
    press(&mut h, CMD, Key::Z);

    // Mouse: click places the caret, drag selects.
    let (row_h, x0, y0) = (24.0, 108.0, EDITOR_TOP + 12.0); // code size 15 → 24px rows
    click(&mut h, egui::pos2(x0 + 3.0 * 9.0, y0 + row_h), None);
    let caret = h.state().0.active_tab().unwrap().buf.cursor_row();
    assert_eq!(caret, 1, "click on the second row puts the caret there");
    click(
        &mut h,
        egui::pos2(x0, y0 + row_h * 2.0),
        Some(egui::pos2(x0 + 200.0, y0 + row_h * 2.0)),
    );
    shot(&mut h, "33-mouse-selection");
    assert!(
        h.state().0.active_tab().unwrap().buf.selection().is_some(),
        "drag selects"
    );

    // Keyboard selection, copy, paste.
    press(&mut h, CMD, Key::ArrowUp);
    for _ in 0..8 {
        press(&mut h, SHIFT, Key::ArrowRight);
    }
    h.event(Event::Copy);
    h.step();
    press(&mut h, CMD, Key::ArrowDown);
    h.event(Event::Paste("// hello".into()));
    h.step();
    assert!(text_of(&h).ends_with("}\n// hello"));
    shot(&mut h, "34-pasted");

    // A second file, as a tab.
    press(&mut h, ALT, Key::F);
    type_text(&mut h, "b.rs");
    shot(&mut h, "35-finder-b");
    press(&mut h, Modifiers::NONE, Key::Enter);
    type_text(&mut h, "x");
    shot(&mut h, "36-two-tabs");
    assert_eq!(h.state().0.tabs.len(), 2);

    // Close with unsaved changes: Esc cancels, D discards.
    press(&mut h, CMD, Key::W);
    shot(&mut h, "37-unsaved-prompt");
    assert!(h.state().0.prompt.is_some());
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(h.state().0.prompt.is_none());
    assert_eq!(h.state().0.tabs.len(), 2, "cancel keeps the tab");
    press(&mut h, CMD, Key::W);
    press(&mut h, Modifiers::NONE, Key::D);
    assert_eq!(h.state().0.tabs.len(), 1);
    assert_eq!(
        std::fs::read_to_string(root.join("b.rs")).unwrap(),
        "struct B;\n",
        "discard leaves disk alone"
    );

    // Reopen the closed tab.
    press(&mut h, Modifiers { shift: true, ..CMD }, Key::T);
    assert_eq!(h.state().0.tabs.len(), 2);
    shot(&mut h, "38-reopened");

    // Quit asks first (always), then about unsaved work; Save writes it.
    press(&mut h, CMD, Key::Q);
    assert!(matches!(h.state().0.ask, Some(crate::app::Ask::Quit)));
    press(&mut h, Modifiers::NONE, Key::Enter);
    shot(&mut h, "39-quit-prompt");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(h.state().0.prompt.is_none());
    assert!(
        std::fs::read_to_string(&a)
            .unwrap()
            .ends_with("}\n// hello")
    );
    drop(h);

    // Session: reopening the project brings the tabs back.
    let mut h = harness(Some(root.clone()));
    shot(&mut h, "40-session-restored");
    let names: Vec<String> = h.state().0.tabs.iter().map(|t| t.buf.name()).collect();
    assert_eq!(names.len(), 2, "tabs restored: {names:?}");
}

/// Typing latency in a big file: `BIG` = path to a large source file.
#[test]
#[ignore]
fn headless_big_file_typing() {
    let mut h = harness(Some(PathBuf::from(std::env::var("BIG").unwrap())));
    shot(&mut h, "50-big");
    press(&mut h, CMD, Key::ArrowDown);
    let mut worst = std::time::Duration::ZERO;
    let start = std::time::Instant::now();
    for ch in "let typed_here = 42;".chars() {
        let t = std::time::Instant::now();
        type_text(&mut h, &ch.to_string());
        worst = worst.max(t.elapsed());
    }
    eprintln!(
        "20 keystrokes: {:?} total, worst frame {:?}",
        start.elapsed(),
        worst
    );
    std::thread::sleep(std::time::Duration::from_millis(400)); // let the background highlight land
    shot(&mut h, "51-big-typed");
    assert!(
        worst < std::time::Duration::from_millis(50),
        "typing must stay responsive"
    );
}

/// Step until `done` holds (the shell runs in real time), or fail after 10s.
fn wait_for(h: &mut Harness<'static, Root>, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done(&h.state().0) {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            screen_text(h)
        );
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

fn screen_text(h: &Harness<'static, Root>) -> String {
    h.state()
        .0
        .terminal()
        .map(|t| t.pane.screen().contents())
        .unwrap_or_default()
}

fn screen_has(needle: &'static str) -> impl Fn(&App) -> bool {
    move |app| {
        app.terminal()
            .is_some_and(|t| t.pane.screen().contents().contains(needle))
    }
}

fn run(h: &mut Harness<'static, Root>, cmd: &str) {
    type_text(h, cmd);
    press(h, Modifiers::NONE, Key::Enter);
}

const CTRL: Modifiers = Modifiers {
    ctrl: true,
    ..Modifiers::NONE
};

/// Step 4: a real shell in the terminal panel.
#[test]
#[ignore]
fn headless_terminal_walkthrough() {
    // A quiet, predictable zsh: an empty .zshrc skips the new-user wizard.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join("notes.txt"), "draft\n").unwrap();

    let mut h = harness(Some(root.clone()));
    no_sidebar(&mut h); // this walkthrough clicks at fixed spots
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "the prompt", screen_has("$ "));
    run(&mut h, "echo hello-$((6*7))");
    wait_for(&mut h, "echo output", screen_has("hello-42"));
    shot(&mut h, "60-terminal");

    // Colours.
    run(
        &mut h,
        "printf '\\033[31mred \\033[32mgreen \\033[1;34mboldblue \\033[7minverse\\033[0m\\n'",
    );
    wait_for(&mut h, "colours", screen_has("boldblue"));
    shot(&mut h, "61-colours");

    // Scrollback by wheel.
    run(&mut h, "seq 1 300");
    wait_for(&mut h, "seq output", screen_has("300"));
    // A full screen of output must still draw well inside a frame.
    let t0 = std::time::Instant::now();
    for _ in 0..30 {
        h.step();
    }
    let per_frame = t0.elapsed() / 30;
    eprintln!("full terminal screen: {per_frame:?} per frame");
    assert!(
        per_frame < std::time::Duration::from_millis(8),
        "{per_frame:?}"
    );
    let over_terminal = egui::pos2(640.0, 650.0);
    h.event(Event::PointerMoved(over_terminal));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 400.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: Modifiers::NONE,
    });
    shot(&mut h, "62-scrolled-back");
    assert!(
        h.state().0.terminal().unwrap().pane.scroll_offset() > 0,
        "wheel scrolls history"
    );
    type_text(&mut h, " "); // typing snaps back to the live bottom
    press(&mut h, Modifiers::NONE, Key::Backspace);
    assert_eq!(h.state().0.terminal().unwrap().pane.scroll_offset(), 0);

    // A pager on the alternate screen.
    run(&mut h, "seq 1 500 | less");
    wait_for(&mut h, "less", |app| {
        app.terminal().is_some_and(|t| t.pane.on_alternate_screen())
    });
    shot(&mut h, "63-less");
    type_text(&mut h, "q");
    wait_for(&mut h, "less to quit", |app| {
        app.terminal()
            .is_some_and(|t| !t.pane.on_alternate_screen())
    });

    // vim: edit and save a file, keys and all.
    run(&mut h, "vim -u NONE -N notes.txt");
    wait_for(&mut h, "vim", |app| {
        app.terminal().is_some_and(|t| t.pane.on_alternate_screen())
    });
    wait_for(&mut h, "vim to draw", screen_has("notes.txt"));
    type_text(&mut h, "Otyped in vim");
    press(&mut h, Modifiers::NONE, Key::Escape);
    shot(&mut h, "64-vim");
    type_text(&mut h, ":wq");
    press(&mut h, Modifiers::NONE, Key::Enter);
    wait_for(&mut h, "vim to save", |_| {
        std::fs::read_to_string(root.join("notes.txt"))
            .unwrap_or_default() // not written yet: keep waiting
            .starts_with("typed in vim\n")
    });
    wait_for(&mut h, "vim to exit", |app| {
        app.terminal()
            .is_some_and(|t| !t.pane.on_alternate_screen())
    });

    // Ctrl+C reaches the program (and isn't taken as Copy).
    run(&mut h, "sleep 30");
    wait_for(&mut h, "sleep to start", |app| {
        app.terminal()
            .is_some_and(|t| t.pane.display_name().contains("sleep"))
    });
    shot(&mut h, "65-running");
    press(&mut h, CTRL, Key::C);
    wait_for(&mut h, "sleep to stop", |app| {
        app.terminal()
            .is_some_and(|t| !t.pane.display_name().contains("sleep"))
    });

    // Mouse selection, copied with ⌘C.
    run(&mut h, "clear; echo select-me-please");
    wait_for(&mut h, "text to select", screen_has("select-me-please"));
    shot(&mut h, "66-before-select");
    click(
        &mut h,
        egui::pos2(14.0, 548.0),
        Some(egui::pos2(400.0, 548.0)),
    );
    let selected = h
        .state_mut()
        .0
        .terminal_mut()
        .unwrap()
        .pane
        .selection_text();
    shot(&mut h, "67-selected");
    assert!(
        selected.is_some_and(|s| !s.trim().is_empty()),
        "drag selects terminal text"
    );

    // ⌥T hides (editor gets focus), ⌥T brings it back focused.
    press(&mut h, ALT, Key::T);
    assert!(!h.state().0.terminal_open);
    press(&mut h, ALT, Key::T);
    assert!(h.state().0.terminal_open && h.state().0.terminal_focused());

    // `exit` closes the panel.
    run(&mut h, "exit");
    wait_for(&mut h, "the shell to exit", |app| app.terminals.is_empty());
    shot(&mut h, "68-exited");
}

/// Step 5: several terminals — tabs, grid, names, switching, and the FPS cap
/// on unattended output.
#[test]
#[ignore]
fn headless_terminals_walkthrough() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let cfg = home.join(".config/oxru");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.json"), "{ terminalFps: 5 }").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("api");
    std::fs::create_dir_all(root.join(".git")).unwrap();

    let mut h = harness(Some(root.clone()));
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "first prompt", screen_has("$ "));
    for _ in 0..4 {
        press(&mut h, ALT, Key::N);
        wait_for(&mut h, "new prompt", screen_has("$ "));
    }
    assert_eq!(h.state().0.terminals.len(), 5);
    assert_eq!(
        h.state().0.active_terminal,
        4,
        "new terminals open beside the current one"
    );
    let names: Vec<String> = h
        .state()
        .0
        .terminals
        .iter()
        .map(|t| t.pane.display_name())
        .collect();
    let reports: Vec<String> = app(&h).terminals.iter().map(|t| t.pane.foreground_report()).collect();
    assert_eq!(
        crate::terminal::labels(&names).iter().filter(|n| n.contains(" · ")).count(),
        0,
        "{reports:#?}"
    );
    assert_eq!(
        crate::terminal::labels(&names),
        ["api", "api 2", "api 3", "api 4", "api 5"]
    );

    // Five "servers" logging away.
    for n in 1..=5u8 {
        press(
            &mut h,
            ALT,
            [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5][n as usize - 1],
        );
        assert_eq!(h.state().0.active_terminal, n as usize - 1, "⌥{n} jumps");
        run(
            &mut h,
            &format!("while :; do echo server-{n} tick $RANDOM; sleep 0.05; done"),
        );
        assert!(
            h.state().0.palette.is_none(),
            "Enter in a terminal must not reach the welcome card behind it"
        );
    }
    wait_for(&mut h, "logs", screen_has("server-5 tick"));
    shot(&mut h, "70-tabs-running");

    press(&mut h, ALT, Key::G);
    std::thread::sleep(std::time::Duration::from_millis(300));
    shot(&mut h, "71-grid");
    assert!(h.state().0.terminal_grid);

    // Record every redraw the app asks for, to see the FPS cap at work.
    let requests: std::sync::Arc<std::sync::Mutex<Vec<std::time::Duration>>> = Default::default();
    {
        let requests = requests.clone();
        h.ctx
            .set_request_repaint_callback(move |info| requests.lock().unwrap().push(info.delay));
    }
    let watch = |h: &mut Harness<'static, Root>, ms: u64| {
        requests.lock().unwrap().clear();
        std::thread::sleep(std::time::Duration::from_millis(ms));
        let got = requests.lock().unwrap().clone();
        h.step();
        got
    };

    // Unattended output is capped at 5fps: with all five printing every 50ms,
    // the app still never asks to redraw sooner than ~200ms out.
    std::thread::sleep(std::time::Duration::from_millis(800)); // leave the typing window
    h.step();
    // (egui only reports a request sooner than one already pending, so
    // retry until one is seen; what matters is none is sooner than the cap.)
    let mut idle = Vec::new();
    for _ in 0..10 {
        idle = watch(&mut h, 1000);
        if !idle.is_empty() {
            break;
        }
    }
    let soonest = idle.iter().min().copied().unwrap_or_default();
    eprintln!(
        "5 busy terminals at 5fps: {} wake requests in 1s, soonest {soonest:?}",
        idle.len()
    );
    // None seen at all (a busy machine, a sooner one already pending) proves
    // nothing either way; the cap itself is `terminal::tests::
    // idle_output_waits_but_typed_echo_is_immediate`'s.
    assert!(idle.is_empty() || soonest >= std::time::Duration::from_millis(150), "{idle:?}");

    // Clicking a tile focuses it (the middle tile of the top row).
    click(&mut h, egui::pos2(640.0, 600.0), None);
    shot(&mut h, "72-grid-clicked");
    assert_eq!(
        h.state().0.active_terminal,
        1,
        "a click moves focus to the clicked tile"
    );

    // (Typing redrawing immediately is checked in terminal::tests, on a
    // fresh context where no other redraw is already pending.)

    // Terminals off screen don't wake the UI at all.
    press(&mut h, ALT, Key::G); // back to tabs: only the active one shows
    press(&mut h, ALT, Key::T); // hide the panel entirely
    h.step();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let hidden = watch(&mut h, 1000);
    eprintln!("panel hidden: {} wake requests in 1s", hidden.len());
    assert!(
        hidden.len() <= 2,
        "hidden terminals shouldn't drive redraws: {hidden:?}"
    );
    press(&mut h, ALT, Key::T);
    press(&mut h, ALT, Key::G);

    // ⌥K quick switch.
    press(&mut h, ALT, Key::K);
    type_text(&mut h, "3 ");
    shot(&mut h, "73-switcher");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(h.state().0.active_terminal, 2);

    // Maximize, then close one.
    press(&mut h, ALT, Key::M);
    shot(&mut h, "74-maximized");
    assert!(h.state().0.terminal_max);
    press(&mut h, ALT, Key::W);
    assert_eq!(h.state().0.terminals.len(), 4);
    press(&mut h, ALT, Key::G);
    press(&mut h, ALT, Key::M);
    shot(&mut h, "75-tabs-after-close");

    // Maximized, the keyboard going elsewhere brings it back to its size:
    // to the editor (⌘J), the tree (⇧⌘E); the palette over it doesn't.
    for (mods, key) in [(CMD, Key::J), (CMD_SHIFT, Key::E)] {
        press(&mut h, ALT, Key::M);
        assert!(h.state().0.terminal_max && h.state().0.terminal_focused());
        press(&mut h, CMD_SHIFT, Key::P);
        assert!(h.state().0.terminal_max, "the palette leaves it maximized");
        press(&mut h, Modifiers::NONE, Key::Escape);
        press(&mut h, mods, key);
        assert!(!h.state().0.terminal_max, "{mods:?} {key:?} restores it");
        press(&mut h, CMD, Key::J); // back into the terminal
        assert!(h.state().0.terminal_focused());
    }
}

/// Step 6: a script that opens OS terminal windows gets tabs here instead.
/// The bridge's "real" osascript/open are swapped for a recording stub first,
/// so even a regression could never open a window on this machine.
#[test]
#[ignore]
fn headless_run_sh_walkthrough() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    for sub in ["web", "api", "worker"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    std::fs::create_dir_all(root.join(".git")).unwrap();
    // Every everyday way a script opens a terminal window.
    let script = r#"#!/bin/bash
cd "$(dirname "$0")"
osascript -e "tell application \"Terminal\" to do script \"cd '$PWD/web' && echo web-up\""
osascript <<END
tell application "Terminal"
  do script "cd '$PWD/api' && echo api-up"
end tell
END
osascript -l JavaScript -e "Application('Terminal').doScript(\"cd '$PWD/worker' && echo worker-up\")"
osascript -e 'tell application "Terminal" to do script ""'
open -a Terminal .
osascript -e 'display notification "not a terminal"'
echo run.sh-done
"#;
    std::fs::write(root.join("run.sh"), script).unwrap();

    let mut h = harness(Some(root.clone()));
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "prompt", screen_has("$ "));

    // Swap the fallback binaries for a stub that records what got through.
    let bridge = crate::termbridge::request_file()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let escaped = dir.path().join("escaped");
    let stub = dir.path().join("stub");
    std::fs::write(
        &stub,
        format!("#!/bin/bash\necho \"$0 $*\" >> '{}'\n", escaped.display()),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    for which in ["osascript", "open"] {
        let shim = bridge.join(which);
        let src = std::fs::read_to_string(&shim).unwrap();
        let real = format!("REAL=\"/usr/bin/{which}\"");
        assert!(src.contains(&real), "{which} shim has its fallback line");
        std::fs::write(
            &shim,
            src.replace(&real, &format!("REAL=\"{}\"", stub.display())),
        )
        .unwrap();
    }

    run(&mut h, "bash run.sh");
    wait_for(&mut h, "five new terminals", |app| app.terminals.len() == 6);
    // The last call (a notification) runs after the last terminal opens:
    // wait for the script to finish before checking what got through.
    wait_for(&mut h, "run.sh to finish", |app| {
        app.terminals[0].pane.screen().contents().contains("run.sh-done")
    });
    wait_for(&mut h, "the commands to run", |app| {
        let all: String = app
            .terminals
            .iter()
            .map(|t| t.pane.screen().contents())
            .collect();
        ["web-up", "api-up", "worker-up"]
            .iter()
            .all(|s| all.contains(s))
    });
    press(&mut h, ALT, Key::G);
    std::thread::sleep(std::time::Duration::from_millis(300));
    shot(&mut h, "80-run-sh-grid");

    let names: Vec<String> = h
        .state()
        .0
        .terminals
        .iter()
        .map(|t| t.pane.display_name())
        .collect();
    eprintln!("terminals: {names:?}");
    for want in ["web", "api", "worker"] {
        assert!(
            names.iter().any(|n| n.starts_with(want)),
            "a tab named for {want}: {names:?}"
        );
    }
    assert!(
        names.last().is_some_and(|n| n.starts_with("shop")),
        "`open -a Terminal .` opens in the script's folder: {names:?}"
    );
    let escaped = std::fs::read_to_string(&escaped).unwrap_or_default();
    eprintln!("let through to the real binary: {escaped:?}");
    assert!(
        !escaped.contains("do script") && !escaped.contains("Terminal"),
        "a terminal request escaped: {escaped}"
    );
    assert!(
        escaped.contains("display notification"),
        "non-terminal calls still pass through"
    );
}

/// Step 7: disk sync, find/replace, multi-cursor, search in files, split view, wrap.
#[test]
#[ignore]
fn headless_editor_features_walkthrough() {
    let _restore = keep_settings(); // ⌥Z below saves word wrap
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("proj");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let a = root.join("src/a.rs");
    std::fs::write(
        &a,
        "fn alpha() {\n    let cat = 1;\n    if cat > 0 {\n        let cat2 = cat;\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/b.rs"), "// needle here\nfn beta() {}\n").unwrap();
    std::fs::write(
        root.join("notes.md"),
        format!("{}\n", "long words ".repeat(40)),
    )
    .unwrap();
    let wait_disk = |h: &mut Harness<'static, Root>| {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        h.step();
        h.step();
    };

    let mut h = harness(Some(a.clone()));
    no_sidebar(&mut h); // this walkthrough clicks at fixed spots
    shot(&mut h, "90-guides");

    // A clean tab follows the file on disk.
    std::fs::write(&a, "fn alpha() {\n    let cat = 1;\n    if cat > 0 {\n        let cat2 = cat;\n    }\n}\n// from disk\n").unwrap();
    wait_disk(&mut h);
    assert!(text_of(&h).ends_with("// from disk\n"), "clean tab reloads");

    // With unsaved edits: ask. Keep mine, then (on the next change) reload.
    press(&mut h, CMD, Key::ArrowDown);
    type_text(&mut h, "// mine");
    std::fs::write(&a, "fn alpha() {}\n// changed again\n").unwrap();
    wait_disk(&mut h);
    shot(&mut h, "91-conflict");
    assert!(h.state().0.conflict.is_some());
    press(&mut h, Modifiers::NONE, Key::K);
    assert!(
        text_of(&h).ends_with("// mine"),
        "keep mine keeps the edits"
    );
    std::fs::write(
        &a,
        "fn alpha() {\n    let cat = 1;\n    if cat > 0 {\n        let cat2 = cat;\n    }\n}\n",
    )
    .unwrap();
    wait_disk(&mut h);
    press(&mut h, Modifiers::NONE, Key::R);
    assert!(
        !text_of(&h).contains("// mine"),
        "reload takes the disk version"
    );

    // Find: ⌘F, type, Enter steps; matches are counted.
    press(&mut h, CMD, Key::F);
    type_text(&mut h, "cat");
    shot(&mut h, "92-find");
    assert_eq!(
        h.state().0.find.as_ref().unwrap().matches.len(),
        4,
        "cat ×4 (cat2 contains one)"
    );
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(h.state().0.find.as_ref().unwrap().current, 1);

    // Replace all: ⌘R opens the replace field.
    press(&mut h, CMD, Key::R);
    type_text(&mut h, "dog");
    shot(&mut h, "93-replace");
    press(&mut h, SHIFT, Key::Enter);
    assert!(
        !text_of(&h).contains("cat") && text_of(&h).contains("dog2"),
        "{}",
        text_of(&h)
    );
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(h.state().0.find.is_none() && h.state().0.focus == crate::app::Focus::Editor);

    // Multi-cursor: select a word, ⌘D twice, type over all three.
    press(&mut h, CMD, Key::ArrowUp);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    for _ in 0..8 {
        press(&mut h, Modifiers::NONE, Key::ArrowRight);
    }
    press(&mut h, CMD, Key::D); // selects the word under the caret ("dog")
    press(&mut h, CMD, Key::D);
    press(&mut h, CMD, Key::D);
    shot(&mut h, "94-multi-cursor");
    assert!(h.state().0.active_tab().unwrap().buf.caret_count() >= 3);
    type_text(&mut h, "pet");
    assert_eq!(text_of(&h).matches("pet").count(), 3, "{}", text_of(&h));
    press(&mut h, Modifiers::NONE, Key::Escape);
    let alt_cmd = Modifiers { alt: true, ..CMD };
    press(&mut h, alt_cmd, Key::ArrowDown);
    assert_eq!(
        h.state().0.active_tab().unwrap().buf.caret_count(),
        2,
        "⌘⌥↓ adds a caret below"
    );
    press(&mut h, Modifiers::NONE, Key::Escape);

    // Search in files.
    press(&mut h, Modifiers { shift: true, ..CMD }, Key::F);
    type_text(&mut h, "needle");
    wait_for(&mut h, "search results", |app| {
        app.palette.as_ref().is_some_and(|p| !p.items.is_empty())
    });
    shot(&mut h, "95-search");
    press(&mut h, Modifiers::NONE, Key::Enter);
    let app = &h.state().0;
    assert_eq!(
        app.active_tab().map(|t| app.tab_rel(t)),
        Some("src/b.rs".to_string())
    );
    assert_eq!(
        app.active_tab().unwrap().buf.selected_text().as_deref(),
        Some("needle")
    );

    // Split view: every open tab tiled.
    press(&mut h, ALT, Key::F);
    type_text(&mut h, "notes");
    press(&mut h, Modifiers::NONE, Key::Enter);
    press(&mut h, CMD, Key::Backslash);
    shot(&mut h, "96-split");
    assert!(h.state().0.editor_grid);
    click(&mut h, egui::pos2(200.0, 200.0), None);
    let app = &h.state().0;
    assert_eq!(
        app.active_tab().map(|t| app.tab_rel(t)),
        Some("src/a.rs".to_string()),
        "clicking a tile focuses it"
    );
    press(&mut h, CMD, Key::Backslash);

    // Word wrap: ⌥Z, saved to settings.json.
    assert!(!h.state().0.config.settings.word_wrap, "starts off");
    press(&mut h, ALT, Key::F);
    type_text(&mut h, "notes");
    press(&mut h, Modifiers::NONE, Key::Enter);
    press(&mut h, ALT, Key::Z);
    press(&mut h, CMD, Key::ArrowDown);
    shot(&mut h, "97-wrapped");
    assert!(h.state().0.config.settings.word_wrap);
    let saved = std::fs::read_to_string(
        PathBuf::from(std::env::var("HOME").unwrap()).join(".config/oxru/settings.json"),
    )
    .unwrap();
    assert!(saved.contains("\"wordWrap\": true"), "{saved}");

    // Deleted on disk: keeps the text, marked unsaved.
    std::fs::remove_file(root.join("notes.md")).unwrap();
    wait_disk(&mut h);
    assert!(h.state().0.active_tab().unwrap().buf.modified);
    shot(&mut h, "98-deleted");
}

/// The shell is waiting at an empty prompt.
fn at_prompt(app: &App) -> bool {
    app.terminal().is_some_and(|t| {
        let screen = t.pane.screen();
        let row = screen.cursor_position().0 as usize;
        screen
            .rows(0, screen.size().1)
            .nth(row)
            .is_some_and(|l| l.trim_end().ends_with('$'))
    })
}

/// A file "dropped from Finder".
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn drop_file(h: &mut Harness<'static, Root>, at: egui::Pos2, path: &std::path::Path) {
    h.event(Event::PointerMoved(at));
    h.step();
    h.input_mut()
        .dropped_files
        .push(std::sync::Arc::new(Dropped(path.to_path_buf())));
    h.step();
}

/// Hide the file tree (in memory only), for walkthroughs that click at
/// fixed screen positions laid out without it.
fn no_sidebar(h: &mut Harness<'static, Root>) {
    h.state_mut().0.config.settings.sidebar = false;
    h.step();
}

fn app<'a>(h: &'a Harness<'static, Root>) -> &'a App {
    &h.state().0
}

/// Step 8: new / rename / delete / reveal / copy path from Go to File,
/// build folders in search, Finder drops and paste, ⌘+click links.
#[test]
#[ignore]
fn headless_files_walkthrough() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let root = base.join("proj");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/lib")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/old_name.rs"), "fn old() {}\n").unwrap();
    std::fs::write(root.join("node_modules/lib/index.js"), "x\n").unwrap();
    let outside = base.join("drop me.txt");
    std::fs::write(&outside, "dropped\n").unwrap();
    let rel = |h: &Harness<'static, Root>| {
        app(h)
            .active_tab()
            .map(|t| app(h).tab_rel(t))
            .unwrap_or_default()
    };

    let mut h = harness(Some(root.join("src/a.rs")));
    wait_for(&mut h, "indexing", |a| !a.indexing());

    // New file inside the selected folder; nested paths make folders.
    press(&mut h, ALT, Key::F);
    shot(&mut h, "a0-files-footer");
    press(&mut h, Modifiers::NONE, Key::ArrowDown); // node_modules → src
    press(&mut h, CMD, Key::N);
    type_text(&mut h, "util/helpers.rs");
    shot(&mut h, "a1-new-file");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(root.join("src/util/helpers.rs").is_file());
    assert_eq!(rel(&h), "src/util/helpers.rs", "the new file opens");
    assert!(app(&h).palette.is_none());

    // Rename: the tab follows; the stem is selected, so typing keeps ".rs".
    press(&mut h, ALT, Key::F);
    type_text(&mut h, "old_name");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(rel(&h), "src/old_name.rs");
    press(&mut h, ALT, Key::F);
    type_text(&mut h, "old_name");
    press(&mut h, Modifiers::NONE, Key::F2);
    shot(&mut h, "a2-rename");
    type_text(&mut h, "new_name");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(root.join("src/new_name.rs").is_file() && !root.join("src/old_name.rs").exists());
    assert_eq!(
        rel(&h),
        "src/new_name.rs",
        "the open tab follows the rename"
    );
    assert_eq!(app(&h).focus, crate::app::Focus::Sidebar, "the tree keeps the keyboard");
    assert!(app(&h).sidebar.query.is_empty());
    assert_eq!(
        app(&h).sidebar.selected.as_deref(),
        Some(root.join("src/new_name.rs").as_path()),
        "…on the renamed file, in the tree"
    );
    shot(&mut h, "a3-renamed");

    // Delete (⌘⌫ in the list): asks, then the file and its tab go.
    press(&mut h, CMD, Key::Backspace);
    shot(&mut h, "a4-trash");
    assert!(matches!(app(&h).ask, Some(crate::app::Ask::Delete { .. })));
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(!root.join("src/new_name.rs").exists());
    assert!(
        app(&h)
            .tabs
            .iter()
            .all(|t| !t.buf.name().contains("new_name")),
        "its tab closed"
    );
    press(&mut h, Modifiers::NONE, Key::Escape);

    // In the editor ⌘⌫ is still an editing key, never "delete the file" —
    // nor in the command palette, where Move to Trash is still listed.
    press(&mut h, CMD, Key::Backspace);
    assert!(app(&h).ask.is_none());
    press(&mut h, CMD_SHIFT, Key::P);
    type_text(&mut h, "trash");
    assert_eq!(
        app(&h).palette.as_ref().unwrap().items[0].label,
        "Move to Trash"
    );
    press(&mut h, CMD, Key::Backspace);
    assert!(app(&h).ask.is_none());
    press(&mut h, Modifiers::NONE, Key::Escape);

    // New folder beside the open file; reveal and copy path act on it too.
    press(&mut h, CMD_SHIFT, Key::N);
    type_text(&mut h, "docs");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(
        root.join("src/util/docs").is_dir(),
        "created beside the active file (src/util/helpers.rs)"
    );
    App::take_launched();
    press(&mut h, Modifiers { alt: true, ..CMD }, Key::R);
    let current = root.join(rel(&h));
    assert_eq!(
        App::take_launched(),
        [vec![
            "-R".to_string(),
            current.to_string_lossy().into_owned()
        ]]
    );
    press(
        &mut h,
        Modifiers {
            alt: true,
            shift: true,
            ..CMD
        },
        Key::C,
    );
    assert_eq!(
        app(&h).note_text(),
        Some(format!("Copied {}", rel(&h)).as_str())
    );

    // Build folders: skipped by search until ⌥H.
    press(&mut h, ALT, Key::F);
    type_text(&mut h, "index.js");
    assert!(app(&h).sidebar.hits.is_empty(), "node_modules is skipped");
    shot(&mut h, "a5-no-match-hint");
    press(&mut h, ALT, Key::H);
    wait_for(&mut h, "re-indexing with build folders", |a| !a.sidebar.hits.is_empty());
    shot(&mut h, "a5-build-folders-on");
    press(&mut h, ALT, Key::H);
    press(&mut h, Modifiers::NONE, Key::Escape);

    // Finder drop on the editor: opens. Hovering shows where it'll land.
    h.input_mut().hovered_files.push(egui::HoveredFile {
        path: Some(outside.clone()),
        mime: String::new(),
    });
    h.event(Event::PointerMoved(egui::pos2(640.0, 300.0)));
    shot(&mut h, "a6-drop-hover-editor");
    h.input_mut().hovered_files.clear();
    drop_file(&mut h, egui::pos2(640.0, 300.0), &outside);
    assert_eq!(
        app(&h).active_tab().map(|t| t.buf.name()).as_deref(),
        Some("drop me.txt")
    );

    // Finder drop on a terminal: types the path, escaped.
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "the prompt", screen_has("$ "));
    let term = app(&h).terminal_rects[0].1;
    h.input_mut().hovered_files.push(egui::HoveredFile {
        path: Some(outside.clone()),
        mime: String::new(),
    });
    h.event(Event::PointerMoved(term.center()));
    shot(&mut h, "a7-drop-hover-terminal");
    h.input_mut().hovered_files.clear();
    click(&mut h, egui::pos2(640.0, 300.0), None); // editor focus, to prove the pointer decides
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);
    drop_file(&mut h, term.center(), &outside);
    wait_for(&mut h, "dropped path", screen_has("drop\\ me.txt"));
    assert_eq!(app(&h).focus, crate::app::Focus::Terminal);
    press(&mut h, CTRL, Key::C);
    wait_for(&mut h, "a fresh prompt", at_prompt);

    // ⌘V with a file copied in Finder pastes its path.
    crate::app::set_clipboard_files(vec![root.join("src/a.rs")]);
    h.event(Event::Paste("a.rs".into()));
    h.step();
    wait_for(&mut h, "pasted path", |a| {
        a.terminal()
            .is_some_and(|t| t.pane.screen().contents().contains("proj/src/a.rs"))
    });
    press(&mut h, CTRL, Key::C);
    wait_for(&mut h, "a fresh prompt", at_prompt);
    crate::app::set_clipboard_files(Vec::new());

    // ⌘+click a link: asks first, then hands it to the browser.
    run(&mut h, "printf 'see https://example.com/PICKME here\\n'");
    let link_row = |a: &App| {
        a.terminal().and_then(|t| {
            let screen = t.pane.screen();
            screen
                .rows(0, screen.size().1)
                .position(|l| l.starts_with("see https"))
        })
    };
    wait_for(&mut h, "the link", |a| link_row(a).is_some());
    let (row, col) = (link_row(app(&h)).unwrap(), 8);
    let size = app(&h).config.settings.code_font_size;
    let s = app(&h).config.settings.ui_font_size / 14.0;
    let char_w = h
        .ctx
        .fonts_mut(|f| f.glyph_width(&egui::FontId::monospace(size), '0'));
    let row_h = (size * 1.4).round();
    let term = app(&h).terminal_rects[0].1;
    let at = egui::pos2(
        term.left() + 10.0 * s + (col as f32 + 0.5) * char_w,
        term.top() + 6.0 * s + (row as f32 + 0.5) * row_h,
    );
    // Resting on a link without ⌘ says how to open it.
    h.event(Event::PointerMoved(at));
    h.step();
    assert!(h.query_by_label_contains("-click to open this link").is_none());
    std::thread::sleep(std::time::Duration::from_millis(500));
    h.step();
    h.step();
    shot(&mut h, "a7-link-hint");
    assert!(h.query_by_label_contains("-click to open this link").is_some());
    h.event(Event::ModifiersChanged(CMD));
    h.event(Event::PointerMoved(at));
    shot(&mut h, "a8-link-hover");
    for pressed in [true, false] {
        h.event(Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: CMD,
        });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    shot(&mut h, "a9-open-link");
    assert!(
        matches!(&app(&h).ask, Some(crate::app::Ask::OpenUrl(u)) if u == "https://example.com/PICKME")
    );
    App::take_launched();
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(
        App::take_launched(),
        [vec!["https://example.com/PICKME".to_string()]]
    );
}

/// Step 9: to-do list, clipboard history, settings, shortcuts, F1 help,
/// toasts, project settings, other windows.
#[test]
#[ignore]
fn headless_extras_walkthrough() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let root = base.join("proj");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join(".oxru")).unwrap();
    std::fs::write(root.join(".oxru/settings.json"), "{ wordWrap: true }\n").unwrap();
    std::fs::write(root.join("a.rs"), "alpha\nbeta\n").unwrap();
    let cfg = home.join(".config/oxru");
    // Shared with Oxru: the to-do list. The open-window markers are Roxaruu's own.
    let state = dirs::config_dir().unwrap().join("oxru");
    let running = dirs::config_dir().unwrap().join("roxaruu/running");
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let _ = std::fs::remove_file(state.join("todos.md")); // start from an empty list

    let mut h = harness(Some(root.join("a.rs")));
    // Project settings win over the global ones.
    assert!(
        app(&h).config.settings.word_wrap,
        "the project turns word wrap on"
    );
    assert_eq!(app(&h).config.project_keys, ["wordWrap"]);
    // This window says which folder it has open.
    let marker = read(running.join(std::process::id().to_string()));
    assert_eq!(marker, root.to_string_lossy());

    // Toasts.
    press(&mut h, CMD, Key::S);
    shot(&mut h, "b0-toast");
    assert_eq!(app(&h).toasts.len(), 1);

    // To-do list: add, tick, paste a list, delete, clear finished.
    press(&mut h, ALT, Key::D);
    type_text(&mut h, "buy milk");
    press(&mut h, Modifiers::NONE, Key::Enter);
    type_text(&mut h, "ship step 9");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(
        read(state.join("todos.md")),
        "- [ ] buy milk\n- [ ] ship step 9\n"
    );
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(
        read(state.join("todos.md")),
        "- [x] buy milk\n- [ ] ship step 9\n"
    );
    h.event(Event::Paste("call sam\nwater plants\n".into()));
    h.step();
    shot(&mut h, "b1-todos");
    assert_eq!(app(&h).todos.len(), 4);
    press(&mut h, CMD, Key::Backspace); // the last one, selected after the paste
    press(&mut h, CMD_SHIFT, Key::Backspace);
    assert_eq!(
        read(state.join("todos.md")),
        "- [ ] ship step 9\n- [ ] call sam\n"
    );
    press(&mut h, CMD, Key::E);
    assert_eq!(
        app(&h).active_tab().map(|t| t.buf.name()).as_deref(),
        Some("todos.md"),
        "⌘E edits the file"
    );
    press(&mut h, CMD, Key::W);

    // Clipboard history: two copies, tick both, paste them together.
    press(&mut h, CMD, Key::ArrowUp);
    press(&mut h, CMD, Key::C); // no selection: the whole line
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, CMD, Key::C);
    assert_eq!(app(&h).clips, ["beta\n", "alpha\n"]);
    press(&mut h, ALT, Key::V);
    press(&mut h, Modifiers::NONE, Key::Tab);
    press(&mut h, Modifiers::NONE, Key::Tab);
    shot(&mut h, "b2-clipboard");
    press(&mut h, Modifiers::NONE, Key::Enter); // at the caret, start of line 2
    assert_eq!(
        text_of(&h),
        "alpha\nbeta\n\nalpha\nbeta\n",
        "newest first, one per line"
    );
    press(&mut h, CMD, Key::Z);

    // Settings: change a value live; a project-set key saves to the project.
    press(&mut h, CMD, Key::Comma);
    shot(&mut h, "b3-settings");
    type_text(&mut h, "code font size");
    let size = app(&h).config.settings.code_font_size;
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    assert_eq!(app(&h).config.settings.code_font_size, size + 1.0);
    assert!(read(cfg.join("settings.json")).contains(&format!("\"codeFontSize\": {}", size + 1.0)));
    press(&mut h, Modifiers::NONE, Key::Escape);
    let global_wrap = || -> Option<serde_json::Value> {
        json5::from_str::<serde_json::Value>(&read(cfg.join("settings.json")))
            .unwrap()
            .get("wordWrap")
            .cloned()
    };
    let wrap_before = global_wrap();
    press(&mut h, CMD, Key::Comma);
    type_text(&mut h, "word wrap");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(!app(&h).config.settings.word_wrap);
    assert!(read(root.join(".oxru/settings.json")).contains("\"wordWrap\": false"));
    assert_eq!(global_wrap(), wrap_before, "the global file is untouched");
    press(&mut h, Modifiers::NONE, Key::Escape);

    // Keyboard Shortcuts: rebind, see a clash, reset.
    press(&mut h, CMD_SHIFT, Key::P);
    type_text(&mut h, "edit keyboard shortcuts");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(
        app(&h).palette.as_ref().map(|p| p.mode),
        Some(crate::palette::Mode::Keybindings)
    );
    type_text(&mut h, "go to file");
    press(&mut h, Modifiers::NONE, Key::Enter);
    shot(&mut h, "b4-recording");
    assert!(app(&h).capturing());
    press(&mut h, ALT, Key::E);
    let km =
        |h: &Harness<'static, Root>, c| app(h).config.keymap.combo_for(c).map(|k| k.to_config());
    assert_eq!(
        km(&h, crate::keymap::Command::GoToFile).as_deref(),
        Some("alt+e")
    );
    assert!(
        read(cfg.join("keybindings.json"))
            .contains(r#"{ "key": "alt+e", "command": "file.goTo" },"#)
    );
    h.state_mut().0.palette.as_mut().unwrap().query = "split".into();
    h.state_mut().0.refresh_items();
    press(&mut h, Modifiers::NONE, Key::Enter);
    press(&mut h, ALT, Key::E);
    assert!(
        app(&h)
            .note_text()
            .is_some_and(|n| n.contains("also Go to File")),
        "{:?}",
        app(&h).note_text()
    );
    h.state_mut().0.palette.as_mut().unwrap().query = String::new();
    h.state_mut().0.refresh_items();
    shot(&mut h, "b5-clash");
    for q in ["split", "go to file"] {
        h.state_mut().0.palette.as_mut().unwrap().query = q.into();
        h.state_mut().0.refresh_items();
        press(&mut h, CMD, Key::Backspace);
    }
    assert_eq!(
        km(&h, crate::keymap::Command::GoToFile).as_deref(),
        Some("alt+f"),
        "back to the default"
    );
    assert!(
        app(&h)
            .config
            .keymap
            .conflicts(crate::keymap::Command::GoToFile)
            .is_empty()
    );
    press(&mut h, Modifiers::NONE, Key::Escape);

    // F1: the keys for where you are, then everything.
    press(&mut h, Modifiers::NONE, Key::F1);
    shot(&mut h, "b6-help-editor");
    assert!(app(&h).help);
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(!app(&h).help);
    press(&mut h, ALT, Key::F);
    press(&mut h, Modifiers::NONE, Key::F1);
    shot(&mut h, "b7-help-files");
    press(&mut h, Modifiers::NONE, Key::F1);
    assert!(
        !app(&h).help && app(&h).focus == crate::app::Focus::Sidebar,
        "F1 again closes help, back to the file search"
    );
    press(&mut h, Modifiers::NONE, Key::Escape);

    // A folder open in another window is marked and not opened twice.
    let other = base.join("other");
    std::fs::create_dir_all(&other).unwrap();
    // A live process named like Roxaruu (a symlink: a copied system binary
    // is killed by code signing).
    let fake = base.join("fake-roxaruu");
    std::os::unix::fs::symlink("/bin/sleep", &fake).unwrap();
    let mut child = std::process::Command::new(&fake).arg("30").spawn().unwrap();
    std::fs::create_dir_all(&running).unwrap();
    std::fs::write(
        running.join(child.id().to_string()),
        other.to_string_lossy().as_bytes(),
    )
    .unwrap();
    crate::recent::record(&other);
    press(&mut h, ALT, Key::O);
    shot(&mut h, "b8-recent-elsewhere");
    let pal = app(&h).palette.as_ref().unwrap();
    let row = pal.items.iter().position(|i| i.label == "other").unwrap();
    assert!(pal.items[row].detail.contains("open in another window"));
    h.state_mut().0.palette.as_mut().unwrap().selected = row;
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(
        app(&h).note_text(),
        Some("other is already open in another window")
    );
    assert!(
        App::take_launched().iter().all(|l| l[0] != "new window"),
        "no second window for it"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// Step 10: every shortcut on the F1 sheet reaches its command. Commands are
/// recorded rather than run (what each one does is covered by the
/// walkthroughs above); this checks nothing on the sheet is shadowed by the
/// editor, a terminal, the palette or another binding.
#[test]
#[ignore]
fn headless_every_shortcut() {
    use crate::keymap::Command;
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("proj");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    for f in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(root.join(f), "fn x() {}\n").unwrap();
    }

    // Somewhere everything applies: two terminals, two tabs (one unsaved),
    // a closed tab to reopen, the editor focused.
    let mut h = harness(Some(root.join("a.rs")));
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "first prompt", at_prompt);
    press(&mut h, ALT, Key::N);
    wait_for(&mut h, "second prompt", at_prompt);
    for f in ["b.rs", "c.rs"] {
        let path = root.join(f);
        h.state_mut().0.open_file(&path);
        h.step();
    }
    // In the editor: off macOS a terminal keeps Ctrl+W for its shell.
    h.state_mut().0.focus = crate::app::Focus::Editor;
    press(&mut h, CMD, Key::W);
    h.step();
    type_text(&mut h, "x");
    let a = app(&h);
    let modified = a.tabs.iter().any(|t| t.buf.modified);
    assert!(
        a.tabs.len() == 2 && a.terminals.len() == 2 && modified,
        "{} tabs, {} terminals, modified {modified}",
        a.tabs.len(),
        a.terminals.len()
    );

    visible_keys_work(&mut h, "the editor");
    let km = &app(&h).config.keymap;
    let sheet: Vec<_> = crate::ui::help_sections(km)
        .into_iter()
        .flat_map(|(_, rows)| rows)
        .map(|(command, title)| (command, title, km.combo_for(command).unwrap().press()))
        .collect();
    let mut done = vec![Command::DeleteFile]; // only in Go to File, below
    for &(command, title, (key, mods)) in &sheet {
        if done.contains(&command) || !app(&h).available(command) {
            continue; // a terminal's own (⌘K), checked from there
        }
        crate::app::dry_run(true);
        press(&mut h, mods, key);
        let ran = crate::app::dry_run(false);
        assert_eq!(ran, [command], "{title}: {mods:?}+{key:?}");
        done.push(command);
    }
    let from_editor = done.len() - 1;

    // From a terminal: everything that still applies there gets through too
    // (only real Ctrl chords are the shell's, and none of the defaults use one).
    h.state_mut().0.focus = crate::app::Focus::Terminal;
    h.step();
    visible_keys_work(&mut h, "a terminal");
    let mut from_terminal = 0;
    for &(command, title, (key, mods)) in &sheet {
        if command == Command::DeleteFile || !app(&h).available(command) {
            continue;
        }
        crate::app::dry_run(true);
        press(&mut h, mods, key);
        assert_eq!(
            crate::app::dry_run(false),
            [command],
            "from a terminal, {title}: {mods:?}+{key:?}"
        );
        assert!(app(&h).terminal_focused(), "{title}");
        from_terminal += 1;
        if !done.contains(&command) {
            done.push(command);
        }
    }
    eprintln!("{from_editor} work from the editor, {from_terminal} from a terminal");

    // From the file tree: its own keys get through.
    h.state_mut().0.focus_sidebar();
    h.step();
    visible_keys_work(&mut h, "the tree");
    for &(command, title, (key, mods)) in &sheet {
        if done.contains(&command) || !app(&h).available(command) {
            continue;
        }
        crate::app::dry_run(true);
        press(&mut h, mods, key);
        assert_eq!(crate::app::dry_run(false), [command], "from the tree, {title}");
        done.push(command);
    }
    h.state_mut().0.focus = crate::app::Focus::Editor;

    // Edit a Huge File: only with one in front (a long line makes it one).
    let wide = root.join("min.js");
    std::fs::write(&wide, "x".repeat(crate::huge::LONG_LINE + 1)).unwrap();
    h.state_mut().0.open_file(&wide);
    h.step();
    let (_, _, (key, mods)) = sheet
        .iter()
        .find(|s| s.0 == Command::EditWholeFile)
        .copied()
        .unwrap();
    crate::app::dry_run(true);
    press(&mut h, mods, key);
    assert_eq!(crate::app::dry_run(false), [Command::EditWholeFile]);
    done.push(Command::EditWholeFile);

    // Move to Trash, from Go to File.
    press(&mut h, ALT, Key::F);
    type_text(&mut h, "b.rs");
    let (_, _, (key, mods)) = sheet
        .iter()
        .find(|s| s.0 == Command::DeleteFile)
        .copied()
        .unwrap();
    crate::app::dry_run(true);
    press(&mut h, mods, key);
    assert_eq!(crate::app::dry_run(false), [Command::DeleteFile]);
    done.push(Command::DeleteFile);

    // Switch Window: in a browser's copy (the page's list; the Chrome test
    // opens it for real).
    press(&mut h, Modifiers::NONE, Key::Escape);
    let (_, _, (key, mods)) = sheet.iter().find(|s| s.0 == Command::SwitchWindow).copied().unwrap();
    h.state_mut().0.remote_pass = true;
    crate::app::dry_run(true);
    press(&mut h, mods, key);
    h.state_mut().0.remote_pass = false;
    assert_eq!(crate::app::dry_run(false), [Command::SwitchWindow]);
    done.push(Command::SwitchWindow);
    let missed: Vec<_> = sheet
        .iter()
        .filter(|s| !done.contains(&s.0))
        .map(|s| s.1)
        .collect();
    assert!(missed.is_empty(), "never reached: {missed:?}");
    eprintln!("all {} F1 shortcuts reach their command", sheet.len());
}

/// Step 10: how long a window takes to show a project, and what the app
/// costs while nobody touches it. Frames here are laid out but not drawn by
/// a GPU, so the CPU number is the app's own share.
#[test]
#[ignore]
fn headless_status_hint_click_opens_palette() {
    // The click that opens the palette ends outside it; that release must
    // not count as "clicked away".
    let mut h = harness(None);
    h.run();
    assert!(app(&h).palette.is_none());
    // The status bar's "all commands", wherever this system's labels put it.
    let hint = crate::ui::drawn_hint(crate::keymap::Command::Palette).expect("the status bar shows all commands");
    click_at(&mut h, hint.center(), egui::PointerButton::Primary);
    assert!(
        app(&h).palette.is_some(),
        "clicking a status-bar hint opens it"
    );
    for _ in 0..4 {
        h.step();
    }
    assert!(app(&h).palette.is_some(), "and it stays open");
    // A real click elsewhere (outside its 70% box) still closes it.
    click_at(
        &mut h,
        egui::pos2(1220.0, 760.0),
        egui::PointerButton::Primary,
    );
    assert!(app(&h).palette.is_none());
}

#[test]
#[ignore]
fn headless_idle_and_startup() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("proj");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(
        root.join("main.rs"),
        "fn main() {\n    println!(\"hi\");\n}\n".repeat(200),
    )
    .unwrap();
    let me = std::process::id() as i32;

    let start = std::time::Instant::now();
    let mut h = harness(Some(root.join("main.rs")));
    h.step();
    let startup = start.elapsed();
    shot(&mut h, "c0-idle");

    // Requested wake-ups and CPU over 3 quiet seconds, focused and not.
    let requests: std::sync::Arc<std::sync::Mutex<Vec<std::time::Duration>>> = Default::default();
    {
        let requests = requests.clone();
        h.ctx
            .set_request_repaint_callback(move |info| requests.lock().unwrap().push(info.delay));
    }
    let idle = |h: &mut Harness<'static, Root>, focused: bool| {
        h.input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(focused);
        requests.lock().unwrap().clear();
        h.step();
        let cpu0 = crate::sys::usage(me).unwrap().cpu;
        let (t0, mut frames) = (std::time::Instant::now(), 0);
        // Run a frame whenever one was asked for, as eframe would.
        while t0.elapsed() < std::time::Duration::from_secs(3) {
            let due = requests.lock().unwrap().drain(..).min();
            match due {
                Some(d) => {
                    std::thread::sleep(d.min(std::time::Duration::from_secs(1)));
                    h.step();
                    frames += 1;
                }
                None => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        let cpu = crate::sys::usage(me).unwrap().cpu - cpu0;
        (frames as f32 / 3.0, cpu.as_secs_f32() / 3.0 * 100.0)
    };
    let (fps_focused, cpu_focused) = idle(&mut h, true);
    let (fps_away, cpu_away) = idle(&mut h, false);
    let mb = crate::sys::usage(me).unwrap().footprint_kb / 1024;
    eprintln!("startup to first frame: {startup:?}");
    eprintln!("idle, focused (caret blinks): {fps_focused:.1} frames/s, {cpu_focused:.2}% CPU");
    eprintln!("idle, in the background:      {fps_away:.1} frames/s, {cpu_away:.2}% CPU");
    eprintln!("memory (test process, no GPU drawing): {mb} MB");
    assert!(startup < std::time::Duration::from_secs(2), "{startup:?}");
    assert!(
        fps_away <= 1.5,
        "in the background only the 1s config check wakes it: {fps_away}"
    );
    assert!(fps_focused <= 3.5, "{fps_focused}");
}

fn click_at(h: &mut Harness<'static, Root>, at: egui::Pos2, button: egui::PointerButton) {
    h.event(Event::PointerMoved(at));
    h.event(Event::PointerButton {
        pos: at,
        button,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.event(Event::PointerButton {
        pos: at,
        button,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.step();
    h.step();
}

/// The sidebar: the open file shown in the tree, ⌘B, ⇧⌘E and the arrow
/// keys, clicks, the right-click menu, and files made outside the app.
#[test]
#[ignore]
fn headless_sidebar_walkthrough() {
    use crate::app::Focus;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    for d in [".git", "src/cart", "docs", "target/debug"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(root.join(".gitignore"), "target\n").unwrap();
    for (f, body) in [
        (
            "src/main.rs",
            "mod cart;\n\nfn main() {\n    cart::checkout();\n}\n",
        ),
        ("src/cart/mod.rs", "pub fn checkout() {}\n"),
        ("src/cart/tax.rs", "pub const RATE: f32 = 0.2;\n"),
        ("docs/guide.md", "# Guide\n"),
        ("README.md", "# Shop\n"),
        ("Cargo.toml", "[package]\nname = \"shop\"\n"),
    ] {
        std::fs::write(root.join(f), body).unwrap();
    }
    let names = |h: &Harness<'static, Root>| -> Vec<String> {
        app(h)
            .project
            .as_ref()
            .unwrap()
            .tree
            .entries
            .iter()
            .map(|e| format!("{}{}", "  ".repeat(e.depth), e.name))
            .collect()
    };
    let selected = |h: &Harness<'static, Root>| {
        app(h).sidebar.selected.as_ref().map(|p| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
    };

    // Opening a file shows it in the tree, its folder open.
    let mut h = harness(Some(root.join("src/cart/tax.rs")));
    shot(&mut h, "d0-sidebar");
    assert!(app(&h).config.settings.sidebar);
    assert_eq!(selected(&h).as_deref(), Some("src/cart/tax.rs"));
    assert!(
        names(&h).contains(&"    tax.rs".to_string()),
        "{:?}",
        names(&h)
    );

    // ⌘B from the editor goes into the tree; from the tree it hides it (and
    // remembers); ⌘B brings it back.
    press(&mut h, CMD, Key::B);
    assert!(app(&h).config.settings.sidebar);
    assert_eq!(app(&h).focus, Focus::Sidebar);
    press(&mut h, CMD, Key::B);
    shot(&mut h, "d1-hidden");
    assert!(!app(&h).config.settings.sidebar);
    let saved = std::fs::read_to_string(crate::config::settings_path()).unwrap();
    assert!(saved.contains("\"sidebar\": false"), "{saved}");
    press(&mut h, CMD, Key::B);
    assert!(app(&h).config.settings.sidebar);
    // Showing it hands it the keyboard, on the open file.
    assert_eq!(app(&h).focus, Focus::Sidebar);
    assert_eq!(selected(&h).as_deref(), Some("src/cart/tax.rs"));
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(app(&h).focus, Focus::Editor);

    // The titlebar button toggles it too. ⇥ in the shell (completing
    // `./env.sh`) must not hand it egui focus for ↵ to "click".
    let button = egui::pos2(
        if cfg!(target_os = "macos") {
            90.0
        } else {
            20.0
        },
        16.0,
    );
    click_at(&mut h, button, egui::PointerButton::Primary);
    assert!(!app(&h).config.settings.sidebar);
    click_at(&mut h, button, egui::PointerButton::Primary);
    assert!(app(&h).config.settings.sidebar);
    assert_eq!(app(&h).focus, Focus::Sidebar);
    press(&mut h, ALT, Key::T);
    assert!(app(&h).terminal_focused());
    for _ in 0..4 {
        press(&mut h, Modifiers::NONE, Key::Tab);
        press(&mut h, Modifiers::NONE, Key::Enter);
    }
    assert!(
        app(&h).config.settings.sidebar,
        "⇥ ↵ in the terminal toggled the sidebar"
    );
    press(&mut h, ALT, Key::T);

    // ⇧⌘E puts the keyboard in the tree; the editor stops taking keys.
    press(&mut h, CMD_SHIFT, Key::E);
    assert_eq!(app(&h).focus, Focus::Sidebar);
    let before = app(&h).active_tab().unwrap().buf.rope.to_string();
    type_text(&mut h, "zz");
    assert_eq!(
        app(&h).active_tab().unwrap().buf.rope.to_string(),
        before,
        "typing in the tree doesn't edit"
    );
    assert_eq!(app(&h).sidebar.query, "zz", "it searches");
    press(&mut h, Modifiers::NONE, Key::Backspace);
    press(&mut h, Modifiers::NONE, Key::Backspace);
    assert_eq!(app(&h).focus, Focus::Sidebar, "back to the tree");
    press(&mut h, Modifiers::NONE, Key::ArrowUp);
    assert_eq!(selected(&h).as_deref(), Some("src/cart/mod.rs"));
    press(&mut h, Modifiers::NONE, Key::ArrowLeft); // to the folder
    assert_eq!(selected(&h).as_deref(), Some("src/cart"));
    press(&mut h, Modifiers::NONE, Key::ArrowLeft); // fold it
    assert!(!names(&h).contains(&"    tax.rs".to_string()));
    press(&mut h, Modifiers::NONE, Key::ArrowRight); // unfold
    press(&mut h, Modifiers::NONE, Key::ArrowRight); // into it
    assert_eq!(selected(&h).as_deref(), Some("src/cart/mod.rs"));
    press(&mut h, Modifiers::NONE, Key::Space); // open, stay in the tree
    assert_eq!(
        app(&h).tab_rel(app(&h).active_tab().unwrap()),
        "src/cart/mod.rs"
    );
    assert_eq!(app(&h).focus, Focus::Sidebar);
    shot(&mut h, "d2-keyboard");
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::Enter); // open and go to it
    assert_eq!(
        app(&h).tab_rel(app(&h).active_tab().unwrap()),
        "src/cart/tax.rs"
    );
    assert_eq!(app(&h).focus, Focus::Editor);

    // File actions act on the tree's row while it has the keyboard.
    press(&mut h, CMD_SHIFT, Key::E);
    press(&mut h, Modifiers::NONE, Key::Home);
    assert_eq!(selected(&h).as_deref(), Some("docs"));
    press(&mut h, Modifiers::NONE, Key::F2);
    assert!(
        matches!(&app(&h).ask, Some(crate::app::Ask::Rename { path, .. }) if path.ends_with("docs"))
    );
    press(&mut h, Modifiers::NONE, Key::Escape);
    press(&mut h, CMD, Key::Backspace);
    assert!(
        matches!(&app(&h).ask, Some(crate::app::Ask::Delete { path }) if path.ends_with("docs"))
    );
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(root.join("docs").exists());
    press(&mut h, CMD, Key::N); // a new file goes inside the selected folder
    assert!(
        matches!(&app(&h).ask, Some(crate::app::Ask::NewFile { dir, .. }) if dir.ends_with("docs"))
    );
    press(&mut h, Modifiers::NONE, Key::Escape);
    press(&mut h, Modifiers::NONE, Key::Escape); // back to the editor
    assert_eq!(app(&h).focus, Focus::Editor);

    // Clicks: a folder opens, a file opens in the editor.
    // Title bar, the tree's header, its search box, then the rows.
    let row = |i: usize| egui::pos2(80.0, 32.0 + 38.0 + 36.0 + 26.0 * i as f32 + 13.0);
    let at = |h: &Harness<'static, Root>, name: &str| {
        names(h).iter().position(|n| n.trim() == name).unwrap()
    };
    let docs = at(&h, "docs");
    click_at(&mut h, row(docs), egui::PointerButton::Primary);
    assert!(
        names(&h).contains(&"  guide.md".to_string()),
        "{:?}",
        names(&h)
    );
    let guide = at(&h, "guide.md");
    click_at(&mut h, row(guide), egui::PointerButton::Primary);
    assert_eq!(
        app(&h).tab_rel(app(&h).active_tab().unwrap()),
        "docs/guide.md"
    );
    assert_eq!(app(&h).focus, Focus::Editor);

    // Right-click: the file menu.
    let readme = at(&h, "README.md");
    click_at(&mut h, row(readme), egui::PointerButton::Secondary);
    shot(&mut h, "d3-menu");
    assert_eq!(selected(&h).as_deref(), Some("README.md"));
    assert_eq!(app(&h).focus, Focus::Sidebar);
    h.key_press(Key::Escape);
    h.step();

    // Open in Terminal: a folder's menu opens a new terminal there; ⌥⇧T on
    // a file (in the tree, or the one in front) opens one in its folder.
    let terminal_in = |h: &mut Harness<'static, Root>, dir: &std::path::Path| {
        let want = dir.canonicalize().unwrap();
        wait_for(h, "the new terminal's folder", |a| {
            a.terminal_focused() && a.terminal().and_then(|t| t.pane.current_dir()).as_deref() == Some(want.as_path())
        });
    };
    let before = app(&h).terminals.len();
    let docs = at(&h, "docs");
    click_at(&mut h, row(docs), egui::PointerButton::Secondary);
    h.get_by_label_contains("Open in Terminal").click();
    h.step();
    h.step();
    assert_eq!(app(&h).terminals.len(), before + 1);
    terminal_in(&mut h, &root.join("docs"));
    h.state_mut().0.focus = Focus::Editor; // the tab in front: docs/guide.md
    h.step();
    h.state_mut().0.open_file(&root.join("src/cart/mod.rs"));
    h.step();
    press(&mut h, Modifiers { alt: true, shift: true, ..Default::default() }, Key::T);
    assert_eq!(app(&h).terminals.len(), before + 2, "a new one, the others left as they were");
    terminal_in(&mut h, &root.join("src/cart"));
    press(&mut h, ALT, Key::T); // hide them again

    // A file made outside the app shows up on its own.
    std::fs::write(root.join("src/cart/coupon.rs"), "").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !names(&h).contains(&"    coupon.rs".to_string()) {
        assert!(std::time::Instant::now() < deadline, "{:?}", names(&h));
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Gitignored folders are listed, faded.
    assert!(
        app(&h)
            .project
            .as_ref()
            .unwrap()
            .tree
            .entries
            .iter()
            .any(|e| e.name == "target" && e.ignored)
    );
    shot(&mut h, "d4-final");
}

/// A folder open with no file: the quiet page, not the launcher. Go to
/// File fills most of the window and its selected row says what ↵ → ← do.
#[test]
#[ignore]
fn headless_project_home_and_file_browser() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    for d in [".git", "src/cart", "docs"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    for f in [
        "src/main.rs",
        "src/cart/mod.rs",
        "docs/guide.md",
        "README.md",
    ] {
        std::fs::write(root.join(f), "x\n").unwrap();
    }
    let mut h = harness(Some(root.clone()));
    h.state_mut().0.config.settings.sidebar = false;
    shot(&mut h, "k0-project-home");
    assert!(app(&h).active_tab().is_none());
    visible_keys_work(&mut h, "the empty-folder page");

    // Its keys work from the page: ⌥F shows the tree, keyboard in its
    // search box; with nothing typed the arrows walk the tree.
    press(&mut h, ALT, Key::F);
    shot(&mut h, "k1-files");
    assert!(app(&h).palette.is_none(), "no dialog: the tree is the finder");
    assert!(app(&h).sidebar_shown());
    assert_eq!(app(&h).focus, crate::app::Focus::Sidebar);
    let label = |h: &Harness<'static, Root>| {
        let a = app(h);
        a.sidebar.selected.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string())
    };
    while label(&h).as_deref() != Some("src") {
        press(&mut h, Modifiers::NONE, Key::ArrowDown);
    }
    shot(&mut h, "k2-folder-selected");
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    shot(&mut h, "k3-folder-expanded");
    let tree = &app(&h).project.as_ref().unwrap().tree;
    assert!(tree.entries.iter().any(|e| e.name == "main.rs" && e.depth == 1));
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    shot(&mut h, "k4-file-selected");
    assert_eq!(label(&h).as_deref(), Some("main.rs"));
    assert!(app(&h).sidebar.query.is_empty(), "arrows don't type");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);
    assert!(
        app(&h)
            .tab_rel(app(&h).active_tab().unwrap())
            .ends_with("main.rs")
    );

    // Typing searches, from the box or anywhere in the tree: matches replace
    // the tree, ↓ picks one, ↵ opens it and the tree comes back.
    press(&mut h, ALT, Key::F);
    type_text(&mut h, ".md");
    shot(&mut h, "k4b-file-search");
    let hits = |h: &Harness<'static, Root>| app(h).sidebar.hits.len();
    assert_eq!(hits(&h), 2, "README.md and docs/guide.md");
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    let chosen = app(&h).file_target().unwrap();
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(app(&h).active_tab().and_then(|t| t.path().cloned()), Some(chosen));
    assert!(app(&h).sidebar.query.is_empty());
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);
    press(&mut h, CMD_SHIFT, Key::E); // into the rows, not the box
    assert!(!app(&h).sidebar.searching, "⇧⌘E browses");
    shot(&mut h, "k4c-browsing");
    visible_keys_work(&mut h, "the tree's rows");
    type_text(&mut h, "guide");
    assert_eq!(app(&h).sidebar.query, "guide", "typing in the tree searches");
    assert_eq!(hits(&h), 1);
    // Esc: back to the rows the search began in, then to the editor.
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(app(&h).sidebar.query.is_empty());
    assert_eq!(app(&h).focus, crate::app::Focus::Sidebar);
    assert!(!app(&h).sidebar.searching);
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);
    // From the box with nothing typed, ↓ goes down into the rows.
    press(&mut h, ALT, Key::F);
    assert!(app(&h).sidebar.searching);
    shot(&mut h, "k4d-search-box");
    visible_keys_work(&mut h, "the tree's search box");
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert!(!app(&h).sidebar.searching && app(&h).focus == crate::app::Focus::Sidebar);
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);
    // ⌥F twice: in and back out.
    press(&mut h, ALT, Key::F);
    press(&mut h, ALT, Key::F);
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);

    // The welcome launcher is for no folder only.
    let mut h = harness(None);
    shot(&mut h, "k5-no-folder");
}

/// Agents asking for you: a notification escape or the bell from a terminal
/// you can't see becomes a toast (with the key to get there) and a bell on
/// its tab; with the window in the background, an OS notification too. An
/// agent that worked a while and went quiet counts without saying anything.
#[test]
#[ignore]
fn headless_agent_alerts() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    // An "agent": bash, under the name claude (re-signed, or macOS kills the copy).
    std::fs::copy("/bin/bash", root.join("claude")).unwrap();
    if cfg!(target_os = "macos") {
        let signed = std::process::Command::new("codesign")
            .args(["-s", "-", "-f"])
            .arg(root.join("claude"))
            .output()
            .unwrap();
        assert!(signed.status.success());
    }
    let toast =
        |needle: &'static str| move |app: &App| app.toasts.iter().any(|t| t.msg.contains(needle));
    let notified = |needle: &str| {
        crate::sys::NOTIFIED
            .lock()
            .unwrap()
            .iter()
            .any(|(_, body)| body.contains(needle))
    };

    let mut h = harness(Some(root.clone()));
    no_sidebar(&mut h);
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "the prompt", screen_has("$ "));

    // On screen: you saw it, nothing to raise.
    run(&mut h, "printf '\\033]9;seen it\\007'; echo printed");
    wait_for(&mut h, "the printf", screen_has("printed"));
    for _ in 0..30 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!toast("seen it")(app(&h)));
    assert!(!app(&h).terminals[0].pane.attention);

    // Hidden: a toast with what it said and the key to switch, a bell on its tab.
    run(
        &mut h,
        "sleep 1; printf '\\033]9;Claude Code: needs your permission\\007'",
    );
    press(&mut h, ALT, Key::T);
    assert!(!app(&h).terminal_open);
    wait_for(&mut h, "the toast", toast("needs your permission"));
    let msg = app(&h).toasts.last().unwrap().msg.clone();
    assert!(msg.contains("to switch"), "{msg}");
    assert!(app(&h).terminals[0].pane.attention);
    assert!(!notified("needs your permission"), "the window had focus");

    // In the background: the OS is told too; the toast waits for your return.
    press(&mut h, ALT, Key::T);
    assert!(!app(&h).terminals[0].pane.attention, "seen now");
    run(&mut h, "sleep 1; printf '\\a'");
    press(&mut h, ALT, Key::T);
    h.input_mut().focused = false;
    h.step();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !notified("wants your attention") {
        assert!(std::time::Instant::now() < deadline, "no OS notification");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let title = crate::sys::NOTIFIED
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .0
        .clone();
    assert!(title.starts_with("Roxaruu · "), "{title}");
    assert!(!toast("wants your attention")(app(&h)));
    h.input_mut().focused = true;
    wait_for(&mut h, "the toast on return", toast("wants your attention"));

    // An agent that worked, then went quiet without a word.
    press(&mut h, ALT, Key::T);
    run(
        &mut h,
        "./claude -c 'for i in $(seq 1 8); do echo working $i; sleep 0.25; done; sleep 30; true'",
    );
    wait_for(&mut h, "the agent", screen_has("working 2"));
    press(&mut h, ALT, Key::T);
    wait_for(
        &mut h,
        "the quiet toast",
        toast("finished, or waiting for you"),
    );
    let msg = app(&h).toasts.last().unwrap().msg.clone();
    assert!(msg.contains("claude"), "names the agent: {msg}");
    shot(&mut h, "a0-agent-toast");

    // On screen in the window, but you've gone to another Space or app:
    // you're not looking, so the OS is told (with its sound) when the agent
    // finishes — even though the window, given no frames once it can't be
    // seen, still believes it has the focus (only macOS knows you left).
    h.state_mut().0.terminal_mut().unwrap().pane.send_input(&[3]); // ⌃C the last agent
    wait_for(&mut h, "it stopped", |a| !a.terminals[0].pane.is_running());
    press(&mut h, ALT, Key::T); // on screen
    assert!(app(&h).terminal_open);
    run(
        &mut h,
        "./claude -c 'for i in $(seq 1 8); do echo working $i; sleep 0.25; done; sleep 30; true'",
    );
    wait_for(&mut h, "the agent working", screen_has("working 2"));
    *crate::sys::APP_ACTIVE.lock().unwrap() = Some(false); // macOS: another app in front
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !crate::sys::NOTIFIED.lock().unwrap().iter().any(|(t, b)| t.contains("claude") && b.contains("finished, or waiting")) {
        assert!(std::time::Instant::now() < deadline, "no notification for the agent on screen in a window you left");
        h.step(); // the window still thinks it's focused
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    *crate::sys::APP_ACTIVE.lock().unwrap() = None;
    press(&mut h, ALT, Key::T); // hidden again, as the next part starts

    // A browser looking in (remote access): its copy of the window always
    // counts itself focused, but you're not at the Mac's window — the OS is
    // still told. (Its passes used to tell the watchers you were looking.)
    use std::sync::atomic::Ordering::Relaxed;
    press(&mut h, ALT, Key::T); // the terminal on screen, in both
    h.state_mut().0.terminal_mut().unwrap().pane.send_input(&[3]); // ⌃C the agent
    wait_for(&mut h, "the agent stopped", |a| !a.terminals[0].pane.is_running());
    let both = |h: &mut Harness<'static, Root>| {
        h.input_mut().focused = false; // the Mac's window, in the background
        h.step();
        h.state_mut().0.remote_pass = true; // then the browser's pass
        h.input_mut().focused = true;
        h.step();
        h.state_mut().0.remote_pass = false;
    };
    run(&mut h, "sleep 1; printf '\\033]9;remote-time ping\\007'");
    both(&mut h);
    assert!(!crate::terminalpane::APP_FOCUSED.load(Relaxed), "the browser's pass doesn't count as you at the Mac");
    assert!(!app(&h).terminals[0].pane.watch.looking.load(Relaxed));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !notified("remote-time ping") {
        assert!(std::time::Instant::now() < deadline, "no OS notification with a browser looking in");
        both(&mut h);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    h.input_mut().focused = true;
}

/// ⌘B moves the keyboard into a tree that's already showing; only from
/// inside does it hide it. A list's own key closes it. ⌥O offers a folder
/// that isn't in the list.
#[test]
#[ignore]
fn headless_focus_keys() {
    use crate::app::Focus;
    use crate::palette::{Mode, Pick};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    let mut h = harness(Some(root.join("main.rs")));
    h.state_mut().0.config.settings.sidebar = true;
    h.step();

    press(&mut h, ALT, Key::T);
    assert_eq!(app(&h).focus, Focus::Terminal);
    press(&mut h, CMD, Key::B);
    assert!(app(&h).config.settings.sidebar, "still shown");
    assert_eq!(app(&h).focus, Focus::Sidebar, "and the keyboard is in it");
    press(&mut h, CMD, Key::B);
    assert!(!app(&h).config.settings.sidebar, "from inside, ⌘B hides it");
    assert_eq!(app(&h).focus, Focus::Editor);
    press(&mut h, CMD, Key::B);
    assert!(app(&h).config.settings.sidebar);
    assert_eq!(app(&h).focus, Focus::Sidebar);
    press(&mut h, Modifiers::NONE, Key::Escape);

    // ⌥O: the recent folders, then a way to any other folder, with its key.
    press(&mut h, ALT, Key::O);
    let p = app(&h).palette.as_ref().unwrap();
    assert_eq!(p.mode, Mode::Recent);
    let last = p.items.last().unwrap();
    assert_eq!(last.pick, Pick::Command(crate::keymap::Command::OpenFolder));
    assert_eq!(crate::keymap::joined(&last.caps), label("ctrl+o"));
    shot(&mut h, "f0-recent");
    press(&mut h, ALT, Key::O);
    assert!(app(&h).palette.is_none(), "⌥O again closes it");
    press(&mut h, ALT, Key::F);
    press(&mut h, ALT, Key::F);
    assert!(app(&h).palette.is_none(), "so does ⌥F");
}

/// Open Folder is our own list, on every system (the system's picker is
/// missing on a server, and out of sight of remote access): it starts at
/// home, lists the folders in the one typed (hidden ones only once a `.` is
/// typed), narrows by name, ⇥ goes in, ⇧⇥ up, ↵ opens.
#[test]
#[ignore]
fn headless_open_folder_list() {
    use crate::palette::{Mode, Pick};
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    for d in ["alpha", "shop/src", ".hidden"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    std::fs::write(base.join("notes.txt"), "not a folder").unwrap();
    let mut h = harness(None);
    let rows = |h: &Harness<'static, Root>| -> Vec<String> {
        app(h).palette.as_ref().unwrap().items.iter().map(|i| i.label.clone()).collect()
    };
    let query = |h: &Harness<'static, Root>| app(h).palette.as_ref().unwrap().query.clone();

    press(&mut h, CMD, Key::O);
    let p = app(&h).palette.as_ref().expect("Open Folder opens the list");
    assert_eq!(p.mode, Mode::Folders);
    let home = crate::config::home().unwrap();
    assert_eq!(p.query, crate::app::folder_text(&home), "it starts at home");
    assert_eq!(p.items[0].pick, Pick::Folder(home.clone()));

    let p = h.state_mut().0.palette.as_mut().unwrap();
    p.query = crate::app::folder_text(&base);
    p.to_end = true;
    h.state_mut().0.refresh_items();
    h.step();
    assert_eq!(rows(&h), ["Open This Folder", "alpha", "shop"], "folders only, hidden ones left out");
    shot(&mut h, "o0-folders");
    type_text(&mut h, ".");
    assert_eq!(rows(&h), [".hidden"]);
    press(&mut h, Modifiers::NONE, Key::Backspace);
    type_text(&mut h, "sh");
    assert_eq!(rows(&h), ["shop"]);

    press(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(query(&h), crate::app::folder_text(&base.join("shop")), "⇥ goes in");
    assert_eq!(rows(&h), ["Open This Folder", "src"]);
    type_text(&mut h, "s");
    assert!(query(&h).ends_with("s"), "and typing goes on at the end: {}", query(&h));
    press(&mut h, Modifiers::SHIFT, Key::Tab);
    assert_eq!(query(&h), crate::app::folder_text(&base.join("shop")), "⇧⇥ drops a half-typed name");
    press(&mut h, Modifiers::SHIFT, Key::Tab);
    assert_eq!(query(&h), crate::app::folder_text(&base), "then goes up");

    type_text(&mut h, "sh");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(app(&h).palette.is_none());
    assert_eq!(app(&h).project.as_ref().map(|p| p.root.clone()), Some(base.join("shop")), "↵ opens it");

    press(&mut h, CMD, Key::O);
    press(&mut h, CMD, Key::O);
    assert!(app(&h).palette.is_none(), "its key closes it again");
}

/// Recent Folders, several at once: space / ⇥ tick one, ⇧↓ ticks a run,
/// ⌘-click and ⇧-click too; ticks survive the search; ↵ opens them all,
/// the first here, the rest each in a window of its own.
#[test]
#[ignore]
fn headless_open_several_folders() {
    use crate::palette::Pick;
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    for n in names.iter().rev() {
        std::fs::create_dir_all(base.join(n).join(".git")).unwrap();
        crate::recent::record(&base.join(n)); // alpha ends up first
    }
    let mut h = harness(None);
    press(&mut h, ALT, Key::O);
    let rows = |h: &Harness<'static, Root>| -> Vec<String> {
        app(h).palette.as_ref().unwrap().items.iter().map(|i| i.label.clone()).collect()
    };
    assert_eq!(&rows(&h)[..5], &names);
    let ticked = |h: &Harness<'static, Root>| -> Vec<String> {
        let mut v: Vec<String> = app(h)
            .ticked()
            .iter()
            .filter_map(|p| if let Pick::Folder(d) = p { Some(d.file_name().unwrap().to_string_lossy().to_string()) } else { None })
            .collect();
        v.sort();
        v
    };

    // Space ticks without moving (and types nothing); ⇧↓ ticks a run.
    press(&mut h, Modifiers::NONE, Key::Space);
    assert_eq!(ticked(&h), ["alpha"]);
    assert_eq!(app(&h).palette.as_ref().unwrap().query, "");
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::SHIFT, Key::ArrowDown);
    assert_eq!(ticked(&h), ["alpha", "charlie", "delta"]);
    assert_eq!(app(&h).palette.as_ref().unwrap().selected, 3);
    // ⇥ flips it and moves on: delta off, now on echo.
    press(&mut h, Modifiers::NONE, Key::Tab);
    assert_eq!(ticked(&h), ["alpha", "charlie"]);
    assert_eq!(app(&h).palette.as_ref().unwrap().selected, 4);
    shot(&mut h, "o1-ticked");

    // Narrowing the list keeps the ticks.
    type_text(&mut h, "brav");
    assert_eq!(rows(&h)[0], "bravo");
    assert_eq!(ticked(&h), ["alpha", "charlie"]);
    for _ in 0..4 {
        press(&mut h, Modifiers::NONE, Key::Backspace);
    }

    // The mouse: ⌘-click ticks one, ⇧-click a run (here: bravo to echo,
    // after the last one ticked). Rows are found by hovering.
    let row = |h: &mut Harness<'static, Root>, i: usize| {
        (0..800)
            .step_by(6)
            .map(|y| egui::pos2(640.0, y as f32))
            .find(|p| {
                h.event(Event::PointerMoved(*p));
                h.step();
                app(h).palette.as_ref().unwrap().selected == i
            })
            .unwrap()
    };
    let click = |h: &mut Harness<'static, Root>, at: egui::Pos2, m: Modifiers| {
        h.event(Event::ModifiersChanged(m));
        click_at(h, at, egui::PointerButton::Primary);
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.step();
    };
    let bravo = row(&mut h, 1);
    click(&mut h, bravo, Modifiers::COMMAND);
    assert!(app(&h).palette.is_some(), "a ⌘-click doesn't open it");
    assert_eq!(ticked(&h), ["alpha", "bravo", "charlie"]);
    let echo = row(&mut h, 4);
    click(&mut h, echo, Modifiers::SHIFT);
    assert_eq!(ticked(&h), ["alpha", "bravo", "charlie", "delta", "echo"]);
    click(&mut h, echo, Modifiers::COMMAND);
    click(&mut h, bravo, Modifiers::COMMAND);
    assert_eq!(ticked(&h), ["alpha", "charlie", "delta"]);
    shot(&mut h, "o2-ticked-by-mouse");

    // ↵: all of them. This window had none, so the first one opens here.
    App::take_launched();
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(app(&h).palette.is_none());
    let here = app(&h).project.as_ref().map(|p| p.name.clone()).unwrap();
    let mut elsewhere: Vec<String> = App::take_launched()
        .into_iter()
        .filter(|l| l[0] == "new window")
        .map(|l| std::path::Path::new(&l[1]).file_name().unwrap().to_string_lossy().to_string())
        .collect();
    elsewhere.push(here);
    elsewhere.sort();
    assert_eq!(elsewhere, ["alpha", "charlie", "delta"]);
    assert!(
        app(&h).toasts.iter().any(|t| t.msg.starts_with("Opened 3 folders")),
        "one toast for them all"
    );
    shot(&mut h, "o3-opened");
}

/// Many tabs: the strip scrolls (wheel, the "‹ N" end), keeps the active
/// tab in view, and a tab drags to a new place; terminals' tabs too.
#[test]
#[ignore]
fn headless_tab_strips_scroll_and_drag() {
    use crate::ui::tabstrip::laid_out;
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let names: Vec<String> = (0..24).map(|i| format!("module_{i:02}.rs")).collect();
    for n in &names {
        std::fs::write(root.join(n), "fn x() {}\n").unwrap();
    }
    let mut h = harness(Some(root.join(&names[0])));
    no_sidebar(&mut h);
    for n in &names[1..] {
        h.state_mut().0.open_file(&root.join(n));
    }
    h.step();
    // The editor's strip: (room, tab rects) from the last frame.
    let strip = |h: &mut Harness<'static, Root>, terminal: bool| {
        laid_out();
        h.step();
        laid_out().into_iter().rfind(|(room, _)| (room.top() > 300.0) == terminal).unwrap()
    };
    let shown = |room: egui::Rect, r: egui::Rect| r.left() >= room.left() - 0.5 && r.right() <= room.right() + 0.5;

    // The active (last opened) tab is in view, the first ones scrolled off.
    let (room, rects) = strip(&mut h, false);
    assert_eq!(app(&h).active, Some(23));
    assert!(shown(room, rects[23]), "the active tab is in view");
    assert!(rects[0].right() < room.left(), "the first ones are scrolled off");
    shot(&mut h, "s0-many-tabs");

    // The wheel over the strip scrolls it back (up/down go sideways).
    let to_start = |h: &mut Harness<'static, Root>| {
        h.event(Event::PointerMoved(room.center()));
        for _ in 0..40 {
            h.event(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 400.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel });
            h.step();
        }
    };
    to_start(&mut h);
    let (room, rects) = strip(&mut h, false);
    assert!(shown(room, rects[0]), "scrolled to the start: {:?} in {room:?}", rects[0]);
    assert_eq!(app(&h).active, Some(23), "scrolling selects nothing");
    shot(&mut h, "s1-scrolled");

    // "N ›" at the right end brings the next hidden tab in.
    let first_hidden = rects.iter().position(|r| r.right() > room.right() + 1.0).unwrap();
    click_at(&mut h, egui::pos2(room.right() - 10.0, room.center().y), egui::PointerButton::Primary);
    let (room, rects) = strip(&mut h, false);
    assert!(shown(room, rects[first_hidden]), "tab {first_hidden} came in");

    // Switching tabs from the keyboard scrolls the new one into view: the
    // previous tab (⌃⇧Tab), a numbered one, the last (⌘9) even when it was
    // already the active one, scrolled away.
    let keys: [(Modifiers, Key, usize); 4] = [
        (Modifiers { shift: true, ..CTRL }, Key::Tab, 22),
        (CMD, Key::Num9, 23),
        (CMD, Key::Num5, 4),
        (CTRL, Key::Tab, 5),
    ];
    for (mods, key, want) in keys {
        to_start(&mut h);
        h.event(Event::PointerMoved(egui::pos2(640.0, 400.0))); // off the strip
        press(&mut h, mods, key);
        assert_eq!(app(&h).active, Some(want), "{mods:?} {key:?}");
        h.step();
        let (room, rects) = strip(&mut h, false);
        assert!(shown(room, rects[want]), "{mods:?} {key:?}: tab {want} at {:?} in {room:?}", rects[want]);
        // To the right, the "N ›" end doesn't cover it either.
        let end = 70.0;
        assert!(rects[want].right() <= room.right() - if want < 23 { end } else { 0.0 } + 0.5, "{want} under the end marker");
    }

    // The strip changing under it (tabs growing, the window narrowing) keeps
    // the active tab in view; scrolling by hand then isn't undone.
    press(&mut h, CMD, Key::Num8); // tab 7, mid-strip
    h.set_size(egui::vec2(700.0, 820.0));
    h.step();
    let (room, rects) = strip(&mut h, false);
    assert!(shown(room, rects[7]), "still in view at a narrower width: {:?} in {room:?}", rects[7]);
    h.set_size(egui::vec2(1280.0, 820.0));
    h.step();
    to_start(&mut h);
    h.step();
    let (room, rects) = strip(&mut h, false);
    assert!(shown(room, rects[0]), "scrolled by hand, it stays where it was put");

    // Opening a file that's already open (the tree, Go to File) shows its tab.
    to_start(&mut h);
    h.event(Event::PointerMoved(egui::pos2(640.0, 400.0)));
    h.state_mut().0.open_file(&root.join(&names[18]));
    h.step();
    let (room, rects) = strip(&mut h, false);
    assert!(shown(room, rects[18]), "an already-open file's tab comes into view");

    // Drag tab 0 right, past two tabs: it lands third, and is the active one.
    to_start(&mut h);
    let (room, rects) = strip(&mut h, false);
    let order = |h: &Harness<'static, Root>| -> Vec<String> { app(h).tabs.iter().map(|t| t.buf.name()).collect() };
    let first = order(&h)[0].clone();
    let drag = |h: &mut Harness<'static, Root>, from: egui::Pos2, to: egui::Pos2| {
        h.event(Event::PointerMoved(from));
        h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        for k in 1..=12 {
            h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 12.0)));
            h.step();
        }
        shot(h, "s2-dragging");
        h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.step();
        h.step();
    };
    let target = egui::pos2(rects[2].center().x + rects[2].width() * 0.2, rects[0].center().y);
    assert!(shown(room, rects[0]) && shown(room, rects[2]));
    drag(&mut h, rects[0].center(), target);
    assert_eq!(order(&h)[2], first, "{:?}", order(&h));
    assert_eq!(app(&h).active, Some(2), "the moved tab is the active one");
    assert_eq!(order(&h).len(), 24);

    // Terminals: three, drag the last to the front.
    for _ in 0..3 {
        press(&mut h, ALT, Key::N);
        wait_for(&mut h, "a prompt", at_prompt);
    }
    let (_, rects) = strip(&mut h, true);
    assert_eq!(rects.len(), 3);
    // Which terminal is which: its shared state stays put as it moves.
    let id = |h: &Harness<'static, Root>, i: usize| std::sync::Arc::as_ptr(&app(h).terminals[i].visible) as usize;
    let last = id(&h, 2);
    drag(&mut h, rects[2].center(), egui::pos2(rects[0].left() + 4.0, rects[0].center().y));
    assert_eq!(id(&h, 0), last, "the last terminal is first now");
    assert_eq!(app(&h).active_terminal, 0, "and active");
}

/// ⌘Q (or the window's close button) always asks first, saying what would
/// stop: running terminals, unsaved files. Esc keeps the window; ↵ closes.
#[test]
#[ignore]
fn headless_quit_asks_first() {
    use crate::app::Ask;
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
    let mut h = harness(Some(root.join("a.rs")));

    // Nothing running, nothing unsaved: it still asks.
    press(&mut h, CMD, Key::Q);
    assert!(matches!(app(&h).ask, Some(Ask::Quit)), "always asks");
    assert!(app(&h).quit_losses().is_empty());
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(app(&h).ask.is_none() && !app(&h).allow_close, "esc keeps the window");

    // The close button too.
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    h.step();
    assert!(matches!(app(&h).ask, Some(Ask::Quit)), "the close button asks too");
    press(&mut h, Modifiers::NONE, Key::Escape);

    // A program running in a terminal, and an unsaved file: it says so.
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", at_prompt);
    run(&mut h, "sleep 30");
    wait_for(&mut h, "sleep running", |a| a.terminals.iter().any(|t| t.pane.is_running()));
    h.state_mut().0.focus = crate::app::Focus::Editor;
    type_text(&mut h, "x");
    press(&mut h, CMD, Key::Q);
    let losses = app(&h).quit_losses().join(" ");
    assert!(losses.contains("still running") && losses.contains("sleep"), "{losses}");
    assert!(losses.contains("unsaved"), "{losses}");
    shot(&mut h, "q0-quit-confirm");

    // ↵: on to "save changes?", then the window closes.
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(app(&h).prompt.is_some(), "then asked about the unsaved file");
    press(&mut h, Modifiers::NONE, Key::D); // don't save
    assert!(app(&h).allow_close, "and it closes");
}

/// A file reopens where you were in it: the caret's line and column, and the
/// scroll; after closing its tab (⇧⌘T) and after a restart.
#[test]
#[ignore]
fn headless_files_reopen_where_you_were() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let text: String = (0..400).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("long.rs"), &text).unwrap();
    std::fs::write(root.join("other.rs"), "fn x() {}\n").unwrap();
    let mut h = harness(Some(root.join("long.rs")));
    let ctx = h.ctx.clone();
    h.state_mut().0.run(crate::keymap::Command::GoToLine, &ctx);
    h.step();
    type_text(&mut h, "250:4");
    press(&mut h, Modifiers::NONE, Key::Enter);
    for _ in 0..3 {
        h.step();
    }
    let place = app(&h).active_tab().unwrap().place();
    assert_eq!((place.line, place.col), (249, 3));
    assert!(place.scroll.1 > 1000.0, "scrolled down to it: {place:?}");
    shot(&mut h, "p0-at-line-250");

    // Close it, open something else, ⇧⌘T: back at line 250, scrolled as it was.
    h.state_mut().0.open_file(&root.join("other.rs"));
    h.step();
    h.state_mut().0.select_tab(0);
    h.step();
    press(&mut h, CMD, Key::W);
    assert_eq!(app(&h).tabs.len(), 1);
    press(&mut h, CMD_SHIFT, Key::T);
    for _ in 0..3 {
        h.step();
    }
    let back = app(&h).active_tab().unwrap().place();
    assert_eq!((back.line, back.col), (249, 3), "⇧⌘T");
    assert!((back.scroll.1 - place.scroll.1).abs() < 1.0, "scrolled as it was: {back:?} vs {place:?}");
    drop(h);

    // Restart: the tab comes back, and so does where you were in it.
    let mut h = harness(Some(root.clone()));
    let i = app(&h).tabs.iter().position(|t| t.buf.name() == "long.rs").unwrap();
    h.state_mut().0.select_tab(i);
    for _ in 0..3 {
        h.step();
    }
    let again = app(&h).active_tab().unwrap().place();
    assert_eq!((again.line, again.col), (249, 3), "after a restart");
    assert!((again.scroll.1 - place.scroll.1).abs() < 1.0, "{again:?}");
    shot(&mut h, "p1-restored");
}

/// The tree follows the file in front: switching tabs (or opening one, from
/// anywhere) opens its folders, selects it and scrolls to it, while the
/// tree stays where the keyboard isn't.
#[test]
#[ignore]
fn headless_tree_follows_the_active_file() {
    use crate::app::Focus;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    // Enough rows that the tree scrolls.
    for d in 0..30 {
        std::fs::create_dir_all(root.join(format!("pkg{d:02}/src/deep"))).unwrap();
        std::fs::write(root.join(format!("pkg{d:02}/src/deep/mod.rs")), "x\n").unwrap();
    }
    std::fs::write(root.join("zz_last.md"), "x\n").unwrap();
    let files = [root.join("pkg03/src/deep/mod.rs"), root.join("pkg27/src/deep/mod.rs"), root.join("zz_last.md")];
    let mut h = harness(Some(files[0].clone()));
    h.state_mut().0.config.settings.sidebar = true;
    for f in &files[1..] {
        h.state_mut().0.open_file(f);
    }
    h.state_mut().0.focus = Focus::Editor;
    h.step();
    let row_visible = |h: &Harness<'static, Root>, path: &std::path::Path| {
        let a = app(h);
        let p = a.project.as_ref().unwrap();
        let i = p.tree.index_of(path).unwrap_or(usize::MAX);
        let row = 26.0;
        let (top, bottom) = (i as f32 * row, (i + 1) as f32 * row);
        a.sidebar.selected.as_deref() == Some(path) && top >= a.sidebar.offset - 0.5 && bottom <= a.sidebar.offset + a.sidebar.view + 0.5
    };
    for (n, f) in [(0, &files[0]), (1, &files[1]), (2, &files[2]), (0, &files[0])] {
        // Folders folded by hand in between.
        let ctx = h.ctx.clone();
        h.state_mut().0.run(crate::keymap::Command::CollapseFolders, &ctx);
        h.step();
        h.state_mut().0.select_tab(n);
        for _ in 0..4 {
            h.step();
        }
        assert_eq!(app(&h).focus, Focus::Editor, "the keyboard stays");
        assert!(row_visible(&h, f), "{} selected and in view: {:?} offset {}", f.display(), app(&h).sidebar.selected, app(&h).sidebar.offset);
    }
    shot(&mut h, "tf0-tree-follows");

    // The tree scrolled away by hand, then the same tab chosen again: back.
    h.event(Event::PointerMoved(egui::pos2(120.0, 400.0)));
    for _ in 0..20 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        h.step();
    }
    assert!(!row_visible(&h, &files[0]), "scrolled away");
    h.event(Event::PointerMoved(egui::pos2(700.0, 400.0)));
    h.state_mut().0.select_tab(0);
    for _ in 0..4 {
        h.step();
    }
    assert!(row_visible(&h, &files[0]), "the same tab again shows it again");

    // A file opened by another name for the same place (a symlinked folder).
    let link = dir.path().canonicalize().unwrap().join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    h.state_mut().0.open_file(&link.join("zz_last.md"));
    for _ in 0..4 {
        h.step();
    }
    assert!(row_visible(&h, &files[2]), "through a symlink: {:?}", app(&h).sidebar.selected);
    shot(&mut h, "tf1-tree-follows-again");
}

/// A terminal's tab says what its program is doing: printing, running,
/// quiet a while (maybe stuck), and the tab list and status bar say so too.
#[test]
#[ignore]
fn headless_terminal_says_whether_it_is_printing() {
    use crate::terminalpane::{Pulse, QUIET_AFTER};
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let mut h = harness(None);
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", at_prompt);
    assert_eq!(app(&h).terminal().unwrap().pane.pulse(), None, "a bare prompt: nothing running");
    // Prints for a few seconds, then goes silent.
    run(&mut h, "for i in 1 2 3 4 5 6 7 8; do echo tick $i; sleep 0.4; done; sleep 60");
    wait_for(&mut h, "printing", |a| a.terminal().unwrap().pane.pulse() == Some(Pulse::Printing));
    shot(&mut h, "tp0-printing");
    let deadline = std::time::Instant::now() + QUIET_AFTER + std::time::Duration::from_secs(10);
    let quiet = loop {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Some(p @ Pulse::Quiet(_)) = app(&h).terminal().unwrap().pane.pulse() {
            break p;
        }
        assert!(std::time::Instant::now() < deadline, "never went quiet: {:?}", app(&h).terminal().unwrap().pane.pulse());
    };
    assert_eq!(quiet.label(), format!("quiet for {}s", QUIET_AFTER.as_secs()));
    shot(&mut h, "tp1-quiet");
    // Stopped: nothing running any more.
    h.state_mut().0.terminal_mut().unwrap().pane.send_input(&[3]); // ⌃C
    wait_for(&mut h, "stopped", |a| a.terminal().unwrap().pane.pulse().is_none());
}

/// A window with no folder: opening one replaces it, unless you have
/// terminals there (opened before any folder) — then the folder gets a
/// window of its own and these stay as they were.
#[test]
#[ignore]
fn headless_folder_from_a_window_with_terminals() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let ctx = |h: &Harness<'static, Root>| h.ctx.clone();

    // No terminals: the folder opens right here.
    let mut h = harness(None);
    App::take_launched();
    let c = ctx(&h);
    h.state_mut().0.open_folder(root.clone(), &c);
    h.step();
    assert_eq!(app(&h).project.as_ref().map(|p| p.root.clone()), Some(root.clone()));
    assert!(App::take_launched().is_empty(), "no new window");
    drop(h);

    // A terminal (even idle, even hidden): a new window for the folder.
    let mut h = harness(None);
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", at_prompt);
    press(&mut h, ALT, Key::T); // hidden still counts
    App::take_launched();
    let c = ctx(&h);
    h.state_mut().0.open_folder(root.clone(), &c);
    h.step();
    assert!(app(&h).project.is_none(), "this window stays folder-less");
    assert_eq!(app(&h).terminals.len(), 1, "its terminal kept");
    let launched = App::take_launched();
    assert!(
        launched.iter().any(|l| l[0] == "new window" && l[1] == root.to_string_lossy()),
        "{launched:?}"
    );
    assert!(app(&h).toasts.iter().any(|t| t.msg.contains("keeps its terminals")));
}

/// Claude Code in a Roxaruu terminal says when it's done through its own
/// hooks (added by `roxaruu-hook`, which `claude` is in Roxaruu's
/// terminals): a stand-in `claude` that runs the `Stop` hook it was given,
/// as Claude Code does, raises "finished" here.
#[test]
#[ignore]
fn headless_claude_hooks_say_when_it_is_done() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().canonicalize().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // The stand-in: shows what it was given, then runs the Stop hook's
    // command from the settings it was handed the way Claude Code does — in
    // a session of its own, with no terminal.
    std::fs::write(
        bin.join("claude"),
        "#!/bin/bash\n[ \"$1\" = --settings ] || { echo \"args: $*\"; exit 0; }\n\
         echo \"args: --settings … ${*:3}\"\n\
         cmd=$(printf '%s' \"$2\" | python3 -c 'import json,sys; print(json.load(sys.stdin)[\"hooks\"][\"Stop\"][0][\"hooks\"][0][\"command\"])')\n\
         echo '{}' | perl -MPOSIX -e 'POSIX::setsid(); exec \"/bin/bash\", \"-c\", $ARGV[0]' \"$cmd\"\n\
         echo stand-in done\n",
    )
    .unwrap();
    std::process::Command::new("chmod").arg("+x").arg(bin.join("claude")).status().unwrap();
    let mut h = harness(None);
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", at_prompt);
    press(&mut h, ALT, Key::T); // hidden: so the alert is raised, not just seen
    h.state_mut().0.focus = crate::app::Focus::Editor;
    h.step();
    // Typed into the hidden terminal straight.
    // The shim first (as it always is), then the stand-in before any real one.
    let line = format!("PATH=\"$OXRU_BRIDGE_DIR:{}:$PATH\" claude --resume abc\r", bin.display());
    h.state_mut().0.terminals[0].pane.send_input(line.as_bytes());
    wait_for(&mut h, "the stand-in ran", screen_has("stand-in done"));
    let screen = app(&h).terminals[0].pane.screen().contents();
    assert!(screen.contains("args: --settings … --resume abc"), "the hooks added, the rest passed on: {screen}");
    wait_for(&mut h, "the toast", |a| a.toasts.iter().any(|t| t.msg.contains("Claude Code: finished")));
    assert!(app(&h).terminals[0].pane.attention, "a bell on its tab");
}

/// Right-click a tab: close others / to the right / to the left / saved,
/// for editor tabs and terminal tabs alike.
#[test]
#[ignore]
fn headless_tab_menus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let files = ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"];
    for f in files {
        std::fs::write(root.join(f), "fn x() {}\n").unwrap();
    }
    let mut h = harness(Some(root.join("a.rs")));
    no_sidebar(&mut h);
    for f in &files[1..] {
        h.state_mut().0.open_file(&root.join(f));
    }
    let names = |h: &Harness<'static, Root>| -> Vec<String> {
        app(h).tabs.iter().map(|t| t.buf.name()).collect()
    };
    let tab = |i: usize| egui::pos2(45.0 + 92.0 * i as f32, 49.0);
    let pick = |h: &mut Harness<'static, Root>, label: &str| {
        h.get_by_label_contains(label).click();
        h.step();
        h.step();
    };

    click_at(&mut h, tab(3), egui::PointerButton::Secondary);
    shot(&mut h, "t1-tab-menu");
    assert_eq!(app(&h).active, Some(3));
    pick(&mut h, "Close to the Right");
    assert_eq!(names(&h), ["a.rs", "b.rs", "c.rs", "d.rs"]);

    click_at(&mut h, tab(1), egui::PointerButton::Secondary);
    pick(&mut h, "Close to the Left");
    assert_eq!(names(&h), ["b.rs", "c.rs", "d.rs"]);

    h.state_mut().0.tabs[1].buf.modified = true; // c.rs
    click_at(&mut h, tab(0), egui::PointerButton::Secondary);
    pick(&mut h, "Close Saved");
    assert_eq!(names(&h), ["c.rs"]);

    h.state_mut().0.open_file(&root.join("a.rs"));
    h.state_mut().0.open_file(&root.join("e.rs"));
    click_at(&mut h, tab(1), egui::PointerButton::Secondary);
    pick(&mut h, "Close Others");
    // c.rs is unsaved: asked first, nothing lost.
    assert!(app(&h).prompt.is_some());
    h.state_mut().0.answer_prompt(crate::app::Answer::Discard, &egui::Context::default());
    h.step();
    assert_eq!(names(&h), ["a.rs"]);
    // Reopen brings the last one closed back.
    click_at(&mut h, tab(0), egui::PointerButton::Secondary);
    pick(&mut h, "Reopen Closed Tab");
    assert_eq!(names(&h).len(), 2);

    // Terminal tabs: the same menu, on terminals.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "first prompt", screen_has("$ "));
    for _ in 0..3 {
        press(&mut h, ALT, Key::N);
        wait_for(&mut h, "new prompt", screen_has("$ "));
    }
    assert_eq!(app(&h).terminals.len(), 4);
    let terms = |h: &Harness<'static, Root>| -> Vec<String> {
        app(h).terminals.iter().map(|t| t.pane.display_name()).collect()
    };
    let before = terms(&h);
    press(&mut h, CMD, Key::J); // back to the editor: the menu still acts on terminals
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);
    click_at(&mut h, egui::pos2(228.0, 490.0), egui::PointerButton::Secondary);
    shot(&mut h, "t2-terminal-menu");
    assert_eq!(app(&h).active_terminal, 2);
    pick(&mut h, "Close to the Right");
    assert_eq!(terms(&h), before[..3]);
    assert_eq!(names(&h).len(), 2, "editor tabs untouched");
    click_at(&mut h, egui::pos2(128.0, 490.0), egui::PointerButton::Secondary);
    pick(&mut h, "Close to the Left");
    assert_eq!(terms(&h), before[1..3]);
    click_at(&mut h, egui::pos2(40.0, 490.0), egui::PointerButton::Secondary);
    pick(&mut h, "Close Others");
    assert_eq!(terms(&h), before[1..2]);
    assert_eq!(names(&h).len(), 2);
    shot(&mut h, "t3-final");

    // Side by side: each terminal's title bar has the same menu.
    press(&mut h, ALT, Key::N);
    wait_for(&mut h, "new prompt", screen_has("$ "));
    press(&mut h, ALT, Key::G);
    h.step();
    let second = app(&h).terminal_rects.iter().find(|(i, _)| *i == 1).unwrap().1;
    click_at(
        &mut h,
        egui::pos2(second.left() + 40.0, second.top() - 12.0),
        egui::PointerButton::Secondary,
    );
    shot(&mut h, "t4-grid-title-menu");
    assert_eq!(app(&h).active_terminal, 1);
    pick(&mut h, "Close Others");
    assert_eq!(app(&h).terminals.len(), 1);

    // Editor split view: the tiles' title bars too.
    press(&mut h, ALT, Key::T); // hide the terminal
    h.state_mut().0.open_file(&root.join("c.rs"));
    let ctx = h.ctx.clone();
    h.state_mut().0.run(crate::keymap::Command::ToggleSplit, &ctx);
    shot(&mut h, "t5-split");
    // e.rs's title bar: the split view's top row, 19px into the editor area.
    click_at(&mut h, egui::pos2(700.0, EDITOR_TOP + 19.0), egui::PointerButton::Secondary);
    assert_eq!(names(&h)[app(&h).active.unwrap()], "e.rs");
    pick(&mut h, "Close to the Right");
    assert_eq!(names(&h), ["a.rs", "e.rs"]);
}

/// Every dialog — palettes, pickers, shortcuts sheet, confirmations — stays
/// within 70% of the window both ways, at any window size.
#[test]
#[ignore]
fn headless_dialogs_fit() {
    use crate::keymap::Command;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    for i in 0..60 {
        std::fs::write(root.join(format!("file{i:02}.rs")), "fn x() {}\n").unwrap();
    }
    // A very long name makes the delete dialog's body long.
    let deep = "a_rather_long_folder_name_here/".repeat(10);
    std::fs::create_dir_all(root.join(&deep)).unwrap();
    let long = format!("{deep}{}.rs", "very_long_name_".repeat(15));
    std::fs::write(root.join(&long), "").unwrap();
    let mut h = harness(Some(root.join("file00.rs")));
    let opens = [
        Command::Palette,
        Command::RecentFolders,
        Command::SearchInFiles,
        Command::GoToLine,
        Command::Settings,
        Command::Keybindings,
        Command::ClipboardHistory,
        Command::Todos,
        Command::SelectTheme,
        Command::SelectUiFont,
        Command::SelectCodeFont,
        Command::Help,
    ];
    let check = |h: &mut Harness<'static, Root>, what: &str, size: egui::Vec2| {
        for _ in 0..3 {
            h.step();
        }
        let rect = ["palette", "help", "modal"]
            .iter()
            .find_map(|id| h.ctx.memory(|m| m.area_rect(egui::Id::unique(*id))))
            .unwrap_or_else(|| panic!("{what}: no dialog"));
        assert!(
            rect.width() <= size.x * 0.7 + 1.0 && rect.height() <= size.y * 0.7 + 1.0,
            "{what} at {size:?} is {:?}",
            rect.size()
        );
        assert!(rect.min.y >= 0.0 && rect.max.y <= size.y, "{what} off screen: {rect:?}");
    };
    for size in [
        egui::vec2(1280.0, 820.0),
        egui::vec2(760.0, 460.0),
        egui::vec2(2400.0, 1500.0),
    ] {
        h.set_size(size);
        h.step();
        for c in opens {
            let ctx = h.ctx.clone();
            h.state_mut().0.run(c, &ctx);
            check(&mut h, &format!("{c:?}"), size);
            if size.y < 500.0 {
                shot(&mut h, &format!("fit-{c:?}"));
            }
            // Fresh areas each time: forget the last one's rect.
            let ctx = h.ctx.clone();
            let app = &mut h.state_mut().0;
            app.palette = None;
            app.help = false;
            ctx.memory_mut(|m| *m.areas_mut() = Default::default());
            h.step();
        }
        // Confirmations: quit, delete (long name), then save-changes.
        let ctx = h.ctx.clone();
        h.state_mut().0.ask = Some(crate::app::Ask::Quit);
        check(&mut h, "quit", size);
        h.state_mut().0.ask = None;
        ctx.memory_mut(|m| *m.areas_mut() = Default::default());
        h.state_mut().0.ask = Some(crate::app::Ask::Delete { path: root.join(&long) });
        check(&mut h, "delete", size);
        if size.y < 500.0 {
            shot(&mut h, "fit-delete");
        }
        h.state_mut().0.ask = None;
        ctx.memory_mut(|m| *m.areas_mut() = Default::default());
        h.state_mut().0.tabs[0].buf.modified = true;
        h.state_mut().0.run(Command::CloseTab, &ctx);
        check(&mut h, "save changes", size);
        h.state_mut().0.prompt = None;
        h.state_mut().0.tabs[0].buf.modified = false;
        ctx.memory_mut(|m| *m.areas_mut() = Default::default());
        h.step();
    }
}

/// Files deleted or changed by something else: a deleted file's tab is
/// struck through and kept; the question comes when you switch to the tab,
/// never by jumping to it.
#[test]
#[ignore]
fn headless_disk_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    for f in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(root.join(f), format!("// {f}\n")).unwrap();
    }
    let wait_disk = |h: &mut Harness<'static, Root>| {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        h.step();
        h.step();
    };
    let mut h = harness(Some(root.join("a.rs")));
    no_sidebar(&mut h);
    h.state_mut().0.open_file(&root.join("b.rs"));
    h.state_mut().0.open_file(&root.join("c.rs"));
    press(&mut h, CMD, Key::ArrowDown);
    type_text(&mut h, "// mine"); // c.rs has unsaved edits
    h.state_mut().0.select_tab(0);
    h.step();
    assert_eq!(app(&h).focus, crate::app::Focus::Editor);

    std::fs::remove_file(root.join("b.rs")).unwrap();
    std::fs::write(root.join("c.rs"), "// changed elsewhere\n").unwrap();
    wait_disk(&mut h);
    assert!(app(&h).tabs[1].gone, "b.rs is marked deleted");
    assert_eq!(app(&h).active, Some(0), "no jumping to the changed tabs");
    assert!(app(&h).conflict.is_none(), "nothing asked about other tabs");
    shot(&mut h, "x0-struck-through");

    // Switch to the deleted one: asked; keep it.
    h.state_mut().0.select_tab(1);
    h.step();
    h.step();
    shot(&mut h, "x1-deleted-dialog");
    assert!(app(&h).conflict.is_some());
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(app(&h).conflict.is_none() && app(&h).tabs.len() == 3);
    assert!(app(&h).tabs[1].gone && app(&h).tabs[1].buf.modified);
    wait_disk(&mut h);
    assert!(app(&h).conflict.is_none(), "asked once");

    // The changed one with unsaved edits: asked; reload.
    h.state_mut().0.select_tab(2);
    h.step();
    h.step();
    shot(&mut h, "x2-changed-dialog");
    assert!(app(&h).conflict.is_some());
    press(&mut h, Modifiers::NONE, Key::R);
    assert_eq!(text_of(&h), "// changed elsewhere\n");

    // Saving the deleted one brings it back.
    h.state_mut().0.select_tab(1);
    h.step();
    press(&mut h, CMD, Key::S);
    assert!(root.join("b.rs").exists() && !app(&h).tabs[1].gone);

    // Deleted while you're looking at it: asked right away; close the tab.
    std::fs::remove_file(root.join("b.rs")).unwrap();
    wait_disk(&mut h);
    h.step();
    assert!(app(&h).conflict.is_some());
    press(&mut h, Modifiers::NONE, Key::C);
    assert_eq!(app(&h).tabs.len(), 2);

    // Deleted, then put back (a git checkout): a clean tab just follows it.
    h.state_mut().0.select_tab(1);
    h.step();
    std::fs::remove_file(root.join("a.rs")).unwrap();
    wait_disk(&mut h);
    assert!(app(&h).tabs[0].gone);
    std::fs::write(root.join("a.rs"), "// restored\n").unwrap();
    wait_disk(&mut h);
    let a = &app(&h).tabs[0];
    assert!(!a.gone && a.disk_news.is_none() && !a.buf.modified);
    h.state_mut().0.select_tab(0);
    h.step();
    h.step();
    assert!(app(&h).conflict.is_none());
    assert_eq!(text_of(&h), "// restored\n");
}

/// A setting changed in one window reaches every other open window.
#[test]
#[ignore]
fn headless_settings_reach_all_windows() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (dir.path().join("one"), dir.path().join("two"));
    for d in [&one, &two] {
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join("main.rs"), "fn main() {}\n").unwrap();
    }
    let mut a = harness(Some(one.join("main.rs")));
    let mut b = harness(Some(two.join("main.rs")));
    let _restore = keep_settings();
    let theme = |h: &Harness<'static, Root>| app(h).config.theme.name.clone();
    let start = theme(&b);
    // The idle window is woken as soon as the file changes, not on a timer.
    let woken: std::sync::Arc<std::sync::Mutex<Vec<std::time::Duration>>> = Default::default();
    {
        let woken = woken.clone();
        b.ctx
            .set_request_repaint_callback(move |info| woken.lock().unwrap().push(info.delay));
    }
    b.step();
    woken.lock().unwrap().clear();
    let other = if start == "Moss" { "Midnight" } else { "Moss" };
    let ctx = a.ctx.clone();
    a.state_mut().0.run(crate::keymap::Command::SelectTheme, &ctx);
    a.step();
    type_text(&mut a, other);
    press(&mut a, Modifiers::NONE, Key::Enter);
    assert_eq!(theme(&a), other);
    let ctx = a.ctx.clone();
    a.state_mut().0.run(crate::keymap::Command::ToggleWordWrap, &ctx);
    let wrap = app(&a).config.settings.word_wrap;
    std::thread::sleep(std::time::Duration::from_millis(900));
    assert!(
        woken.lock().unwrap().iter().any(|d| d.is_zero()),
        "the other window was woken right away: {:?}",
        woken.lock().unwrap()
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    while theme(&b) != other || app(&b).config.settings.word_wrap != wrap {
        assert!(std::time::Instant::now() < deadline, "the other window never caught up");
        b.step();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Closing several unsaved files: Save All / Discard All settle the rest
/// in one go.
#[test]
#[ignore]
fn headless_save_all_discard_all() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let files = ["a.rs", "b.rs", "c.rs"];
    let edit_all = |h: &mut Harness<'static, Root>| {
        for f in files {
            h.state_mut().0.open_file(&root.join(f));
            h.step();
            press(h, CMD, Key::ArrowDown);
            type_text(h, "// edited");
        }
    };
    for f in files {
        std::fs::write(root.join(f), "// start\n").unwrap();
    }
    let mut h = harness(Some(root.join("a.rs")));
    no_sidebar(&mut h);
    edit_all(&mut h);
    press(&mut h, CMD_SHIFT, Key::W);
    shot(&mut h, "s0-save-all");
    assert!(h.query_by_label_contains("All 3 files").is_some());
    press(&mut h, Modifiers::NONE, Key::A);
    assert!(app(&h).prompt.is_none() && app(&h).tabs.is_empty());
    for f in files {
        assert_eq!(std::fs::read_to_string(root.join(f)).unwrap(), "// start\n// edited");
    }

    for f in files {
        std::fs::write(root.join(f), "// start\n").unwrap();
    }
    edit_all(&mut h);
    press(&mut h, CMD_SHIFT, Key::W);
    // Answer the first one by itself, then discard the rest.
    press(&mut h, Modifiers::NONE, Key::S);
    assert!(h.query_by_label_contains("All 2 files").is_some());
    press(&mut h, Modifiers::NONE, Key::X);
    assert!(app(&h).prompt.is_none() && app(&h).tabs.is_empty());
    let saved: Vec<String> = files
        .iter()
        .map(|f| std::fs::read_to_string(root.join(f)).unwrap())
        .collect();
    assert_eq!(saved.iter().filter(|t| t.ends_with("edited")).count(), 1);
}


/// Highlighting across languages: a look at variables, numbers, operators,
/// Markdown headings and code fences in the real editor.
#[test]
#[ignore]
fn headless_highlighting() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("langs");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(
        root.join("main.rs"),
        "use std::collections::HashMap;\n\n/// Counts words.\n#[derive(Debug)]\nstruct Counter {\n    seen: HashMap<String, usize>,\n}\n\nfn count(text: &str, limit: usize) -> usize {\n    let mut total = 0;\n    for word in text.split_whitespace() {\n        if total >= limit * 2 + 1 {\n            break;\n        }\n        total += word.len();\n    }\n    println!(\"{total} words\");\n    total\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("notes.md"),
        "# Release notes\n\nSome *emphasis*, **strong**, `inline code` and a [link](https://example.com).\n\n- one\n- two\n\n```python\ndef area(radius):\n    return 3.14 * radius ** 2\n```\n",
    )
    .unwrap();
    let mut h = harness(Some(root.join("main.rs")));
    no_sidebar(&mut h);
    shot(&mut h, "h0-rust");
    h.state_mut().0.open_file(&root.join("notes.md"));
    shot(&mut h, "h1-markdown");
    assert_eq!(app(&h).active_tab().unwrap().buf.lang.map(|l| l.name), Some("Markdown"));
}

/// Put the shared settings.json back when this goes out of scope, pass or
/// fail: for tests that change settings.
fn keep_settings() -> impl Drop + 'static {
    struct Restore(PathBuf, String);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, &self.1);
        }
    }
    let path = crate::config::settings_path();
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    Restore(path, before)
}

/// The keys for what used to need the mouse: collapse the tree, text size,
/// links in the terminal, and "Change shortcuts…" on the F1 sheet.
/// From the editor, the tree or the terminal, the keys to the other two
/// show (the tree's and the terminal's in the status bar, the editor's on
/// its own row), once each, and take you there.
#[test]
#[ignore]
fn headless_focus_keys_everywhere() {
    use crate::app::Focus;
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let _restore = keep_settings();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    let mut h = harness(Some(root.join("main.rs")));
    h.state_mut().0.config.settings.sidebar = true;
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", at_prompt);
    let place = |a: &App| {
        if a.terminal_focused() {
            Focus::Terminal
        } else {
            a.focus
        }
    };
    let set = |h: &mut Harness<'static, Root>, f: Focus| {
        h.state_mut().0.focus = f;
        h.step();
    };
    for from in [Focus::Editor, Focus::Sidebar, Focus::Terminal] {
        for to in [Focus::Editor, Focus::Sidebar, Focus::Terminal] {
            set(&mut h, from);
            assert_eq!(place(app(&h)), from);
            let hint = app(&h).focus_hint(to);
            if to == from {
                assert!(hint.is_none(), "{from:?} to itself");
                continue;
            }
            let (command, what) = hint.unwrap_or_else(|| panic!("no key from {from:?} to {to:?}"));
            // To the tree or the terminal: in the status bar, under them.
            // To the editor: on its own row (`go_hint`, below).
            assert!(
                to == Focus::Editor
                    || app(&h).status_hints().iter().any(|(_, w, c)| *c == Some(command) && *w == what),
                "the status bar in {from:?} shows {what}"
            );
            assert!(to != Focus::Editor || app(&h).go_hint(to).is_some(), "drawn on {to:?} from {from:?}");
            let (key, mods) = app(&h).config.keymap.combo_for(command).unwrap().press();
            press(&mut h, mods, key);
            assert_eq!(place(app(&h)), to, "{what} from {from:?}");
        }
        set(&mut h, from);
        each_key_once(&format!("{from:?}"));
        visible_keys_work(&mut h, &format!("{from:?}"));
    }
    set(&mut h, Focus::Sidebar);
    shot(&mut h, "f1-focus-keys-from-tree");
    set(&mut h, Focus::Terminal);
    shot(&mut h, "f2-focus-keys-from-terminal");
    set(&mut h, Focus::Editor);
    shot(&mut h, "f0-focus-keys-from-editor");
}

/// A window on a virtual screen (`rox --remote on` on a server) closes once
/// remote access is off, as nobody can see it — but not over unsaved files.
#[test]
#[ignore]
fn headless_an_unseen_window_closes_when_remote_access_goes() {
    let _restore = keep_settings();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "hi\n").unwrap();
    let mut h = harness(Some(file));
    h.step();
    // Unsaved: it stays, so the edit isn't lost.
    h.state_mut().0.tabs[0].buf.modified = true;
    h.state_mut().2 = Some(true); // remote access was on; now it isn't
    h.step();
    assert!(!app(&h).allow_close, "kept over an unsaved file");
    // Nothing unsaved: it goes.
    h.state_mut().0.tabs[0].buf.modified = false;
    h.state_mut().2 = Some(true);
    h.step();
    assert!(app(&h).allow_close, "closed once nobody could see it");
    // A window on a real screen never closes by itself.
    let mut h = harness(None);
    h.step();
    assert!(!app(&h).allow_close);
}

/// No shortcut is drawn twice on the screen: each shows in the one row it
/// belongs to (the keys every row has, like ↵ and esc, aside).
fn each_key_once(when: &str) {
    const ANY_ROW: &[&str] = &["↵", "esc", "space", "↑↓", "↓", "↑↓←→", "⇥", "⇧↵", "type", "right-click"];
    let keys = crate::ui::drawn_keys();
    let mut seen = std::collections::HashSet::new();
    for k in keys.iter().filter(|k| !ANY_ROW.contains(&k.as_str())) {
        assert!(seen.insert(k), "{k} is shown twice ({when}): {keys:?}");
    }
}

#[test]
#[ignore]
fn headless_keyboard_for_everything() {
    use crate::app::Focus;
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let _restore = keep_settings();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    for d in [".git", "src/cart", "docs"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(root.join("src/cart/tax.rs"), "fn tax() {}\n").unwrap();
    let mut h = harness(Some(root.join("src/cart/tax.rs")));
    let open_dirs = |h: &Harness<'static, Root>| {
        let tree = &app(h).project.as_ref().unwrap().tree;
        tree.entries.iter().filter(|e| e.is_dir && e.expanded).count()
    };

    // ⌥⇧C collapses the tree wherever the keyboard is: its key shows on the
    // tree's header, so it can't be the tree's only (it used to be ⌘←, which
    // the editor kept as "line start").
    let alt_shift = Modifiers { alt: true, shift: true, ..Default::default() };
    assert!(open_dirs(&h) > 0, "the open file's folders start expanded");
    assert_eq!(app(&h).focus, Focus::Editor);
    press(&mut h, alt_shift, Key::C);
    assert_eq!(open_dirs(&h), 0, "from the editor");
    assert_eq!(app(&h).focus, Focus::Editor);
    press(&mut h, CMD_SHIFT, Key::E);
    assert_eq!(app(&h).focus, Focus::Sidebar);
    shot(&mut h, "k0-tree-hints");
    press(&mut h, Modifiers::NONE, Key::ArrowRight); // open a folder again
    assert!(open_dirs(&h) > 0);
    press(&mut h, alt_shift, Key::C);
    assert_eq!(open_dirs(&h), 0, "from the tree");
    press(&mut h, Modifiers::NONE, Key::Escape);

    // Text size: bigger, smaller, back to the default — saved, so every
    // window follows.
    let sizes = |h: &Harness<'static, Root>| {
        let s = &app(h).config.settings;
        (s.ui_font_size, s.code_font_size)
    };
    let start = sizes(&h);
    press(&mut h, CMD, Key::Equals);
    assert_eq!(sizes(&h), (start.0 + 1.0, start.1 + 1.0));
    press(&mut h, CMD, Key::Minus);
    press(&mut h, CMD, Key::Minus);
    assert_eq!(sizes(&h), (start.0 - 1.0, start.1 - 1.0));
    press(&mut h, CMD, Key::Num0);
    let d = crate::config::Settings::default();
    assert_eq!(sizes(&h), (d.ui_font_size, d.code_font_size));
    assert!(std::fs::read_to_string(crate::config::settings_path()).unwrap().contains("uiFontSize"));

    // Links in the terminal: ⇧⌘O lists them, newest first; ↵ opens one.
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", screen_has("$ "));
    run(&mut h, "printf 'docs https://example.com/one and https://example.com/two\\n'");
    wait_for(&mut h, "the links", screen_has("example.com/two"));
    press(&mut h, CMD_SHIFT, Key::O);
    shot(&mut h, "k1-links");
    let labels: Vec<String> = app(&h).palette.as_ref().unwrap().items.iter().map(|i| i.label.clone()).collect();
    assert!(labels.contains(&"https://example.com/two".to_string()), "{labels:?}");
    App::take_launched();
    type_text(&mut h, "two");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(App::take_launched(), [vec!["https://example.com/two".to_string()]]);

    // F1, then ↵: straight to changing shortcuts.
    press(&mut h, Modifiers::NONE, Key::F1);
    assert!(app(&h).help);
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(!app(&h).help);
    assert_eq!(app(&h).palette.as_ref().map(|p| p.mode), Some(crate::palette::Mode::Keybindings));
}

/// The closing keys, for real: ⌥⌘] right, ⌥⌘[ left, ⌥⌘S saved, ⌥⌘T others.
#[test]
#[ignore]
fn headless_close_keys() {
    const CMD_ALT: Modifiers = Modifiers {
        mac_cmd: true,
        command: true,
        alt: true,
        ..Modifiers::NONE
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let files = ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs"];
    for f in files {
        std::fs::write(root.join(f), "\n").unwrap();
    }
    let mut h = harness(Some(root.join("a.rs")));
    for f in &files[1..] {
        h.state_mut().0.open_file(&root.join(f));
    }
    let names = |h: &Harness<'static, Root>| -> Vec<String> {
        app(h).tabs.iter().map(|t| t.buf.name()).collect()
    };
    h.state_mut().0.select_tab(4); // e.rs
    h.step();
    // The tab keys are on show where the tabs are.
    let hints = app(&h).tab_hints(false);
    assert!(hints.contains(&(vec![label("ctrl+alt+["), label("ctrl+alt+]")], "close left / right")), "{hints:?}");
    assert!(hints.iter().any(|(_, w)| *w == "close others"));
    shot(&mut h, "z0-tab-hints");
    h.set_size(egui::vec2(900.0, 600.0));
    shot(&mut h, "z1-tab-hints-narrow");
    for _ in 0..3 {
        // Many tabs, narrow window: the hints keep their room.
        for f in ["g.rs", "h.rs", "i.rs", "j.rs"] {
            std::fs::write(root.join(f), "\n").unwrap();
            h.state_mut().0.open_file(&root.join(f));
        }
    }
    h.state_mut().0.select_tab(1);
    shot(&mut h, "z1b-many-tabs");
    for f in ["g.rs", "h.rs", "i.rs", "j.rs"] {
        let i = app(&h).tabs.iter().position(|t| t.buf.name() == f).unwrap();
        h.state_mut().0.tabs.remove(i);
    }
    h.state_mut().0.select_tab(4);
    h.set_size(egui::vec2(1280.0, 820.0));
    press(&mut h, CMD_ALT, Key::CloseBracket);
    assert_eq!(names(&h), ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"]);
    h.state_mut().0.select_tab(1); // b.rs
    h.step();
    press(&mut h, CMD_ALT, Key::OpenBracket);
    assert_eq!(names(&h), ["b.rs", "c.rs", "d.rs", "e.rs"]);
    h.state_mut().0.tabs[2].buf.modified = true; // d.rs unsaved
    press(&mut h, CMD_ALT, Key::S);
    assert_eq!(names(&h), ["d.rs"]);
    h.state_mut().0.tabs[0].buf.modified = false;
    h.state_mut().0.open_file(&root.join("a.rs"));
    press(&mut h, CMD_ALT, Key::T);
    assert_eq!(names(&h), ["a.rs"]);
}

/// A terminal's tab keys show on its tab bar while it has the keyboard.
#[test]
#[ignore]
fn headless_terminal_tab_hints() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("a.rs"), "\n").unwrap();
    let mut h = harness(Some(root.join("a.rs")));
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", screen_has("$ "));
    press(&mut h, ALT, Key::N);
    wait_for(&mut h, "a prompt", screen_has("$ "));
    let hints = app(&h).tab_hints(true);
    assert!(hints.iter().any(|(c, w)| *w == "close" && c == &vec![label("alt+w")]), "{hints:?}");
    assert!(hints.iter().any(|(_, w)| *w == "close left / right"));
    shot(&mut h, "z2-terminal-tab-hints");
}

/// Remote access, end to end: turned on with a password, a browser signs in, sees this window in the list, gets frames at its own
/// size, and its typing, shortcuts and copying act on the same live window.
#[test]
#[ignore]
fn headless_remote_access() {
    use crate::remote::testing::{https, link, sign_in};
    use tungstenite::Message;
    let (mut h, port) = serve_remotely("open sesame", "fn main() {}\n");

    // Nothing secret reached the disk: no password file, and the password
    // in no file of ours (the marker holds only the pid and port).
    assert!(!crate::remote::old_secrets_path().exists());
    let running = dirs::config_dir().unwrap().join("roxaruu").join("running");
    for dir in [crate::config::dir(), running.clone()] {
        let leaks = files_containing(&dir, "open sesame");
        assert!(leaks.is_empty(), "the password is on disk: {leaks:?}");
    }
    let marker = std::fs::read_to_string(running.join("remote-server.json")).unwrap();
    assert_eq!(marker, format!(r#"{{"pid":{},"port":{port}}}"#, std::process::id()));

    // Sign in; this window is listed.
    assert!(sign_in(port, "wrong").is_none());
    let session = sign_in(port, "open sesame").expect("signed in");
    let me = std::process::id() as i32;
    let list = https(
        port,
        &format!("GET /api/windows HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {session}\r\n\r\n"),
    );
    assert!(list.contains(&format!("\"id\":{me}")) && list.contains("\"name\":\"shop\""), "{list}");

    // Link to it, say how big the page is: frames come at that size.
    let mut ws = link(port, me, &session);
    ws.get_mut()
        .sock
        .set_read_timeout(Some(std::time::Duration::from_millis(30)))
        .unwrap();
    let send = |ws: &mut tungstenite::WebSocket<_>, json: &str| ws.send(Message::text(json)).unwrap();
    send(&mut ws, r#"{"t":"size","w":900,"h":560,"dpr":1}"#);
    // Pump the window and the link, drawing (and acking, as the page does)
    // every frame; the picture once they go quiet.
    let mut texts = Vec::new();
    let mut screen = crate::remote::frames::Screen::default();
    let mut next_frame = |h: &mut Harness<'static, Root>, ws: &mut tungstenite::WebSocket<_>, texts: &mut Vec<String>| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut got = false;
        let mut quiet = 0;
        while !got || quiet < quiet_reads(h) {
            assert!(std::time::Instant::now() < deadline, "no frame came");
            h.step();
            match ws.read() {
                Ok(Message::Binary(b)) => {
                    assert!(screen.apply(&b), "a frame");
                    ws.send(Message::text(crate::remote::ACK)).unwrap();
                    got = true;
                    quiet = 0;
                }
                Ok(Message::Text(t)) => texts.push(t.to_string()),
                _ => quiet += 1,
            }
        }
        screen.image()
    };
    let frame = next_frame(&mut h, &mut ws, &mut texts);
    assert_eq!((frame.width(), frame.height()), (900, 560), "laid out at the browser's size");
    let shots = PathBuf::from(std::env::var("SHOTS").unwrap_or_else(|_| "target/shots".into()));
    std::fs::create_dir_all(&shots).unwrap();
    frame.save(shots.join("r0-remote-frame.png")).unwrap();

    // One session: typing in the browser edits the window's file.
    send(&mut ws, r#"{"t":"text","s":"// from the browser\n"}"#);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !text_of(&h).contains("// from the browser") {
        assert!(std::time::Instant::now() < deadline, "the typing never landed: {:?}", text_of(&h));
        h.step();
    }
    // ⌥T from a Mac browser: the press types "†" too (sent with it, as the
    // page does). It opens the terminal and the "†" goes nowhere — not into
    // the file, not into the terminal — exactly as on the Mac.
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let before = text_of(&h);
    send(&mut ws, r#"{"t":"key","k":"†","code":"KeyT","down":true,"m":{"alt":true},"text":"†"}"#);
    send(&mut ws, r#"{"t":"key","k":"†","code":"KeyT","down":false,"m":{"alt":true}}"#);
    while !app(&h).terminal_open {
        assert!(std::time::Instant::now() < deadline, "⌥T never opened the terminal");
        h.step();
    }
    wait_for(&mut h, "a prompt", screen_has("$ "));
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(text_of(&h), before, "⌥T typed into the file");
    let screen = app(&h).terminal().unwrap().pane.screen().contents();
    assert!(!screen.contains('†'), "⌥T typed into the terminal: {screen:?}");
    // And a plain letter still types.
    send(&mut ws, r#"{"t":"key","k":"x","code":"KeyX","down":true,"text":"x"}"#);
    wait_for(&mut h, "x in the terminal", screen_has("$ x"));
    send(&mut ws, r#"{"t":"key","k":"Backspace","code":"Backspace","down":true}"#);
    send(&mut ws, r#"{"t":"key","k":"Backspace","code":"Backspace","down":false}"#);
    // Let it land in the terminal before ⌘J moves the keyboard to the editor
    // (in the same frame, the editor could get it).
    wait_for(&mut h, "x erased", |a| !screen_has("$ x")(a));
    send(&mut ws, r#"{"t":"key","k":"j","code":"KeyJ","down":true,"m":{"meta":true}}"#); // back to the editor
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app(&h).focus != crate::app::Focus::Editor {
        assert!(std::time::Instant::now() < deadline, "⌘J never went back to the editor");
        h.step();
    }

    // A shortcut from the browser: ⌘⇧P opens the command palette.
    send(&mut ws, r#"{"t":"key","k":"p","code":"KeyP","down":true,"m":{"meta":true,"shift":true}}"#);
    send(&mut ws, r#"{"t":"key","k":"p","code":"KeyP","down":false,"m":{"meta":true,"shift":true}}"#);
    while app(&h).palette.is_none() {
        assert!(std::time::Instant::now() < deadline, "the shortcut never ran");
        h.step();
    }
    let frame = next_frame(&mut h, &mut ws, &mut texts);
    frame.save(shots.join("r1-remote-palette.png")).unwrap();
    send(&mut ws, r#"{"t":"key","k":"Escape","code":"Escape","down":true}"#);
    while app(&h).palette.is_some() {
        assert!(std::time::Instant::now() < deadline, "esc never closed the palette");
        h.step();
    }

    // Copying in the browser lands in the browser's clipboard.
    send(&mut ws, r#"{"t":"key","k":"a","code":"KeyA","down":true,"m":{"meta":true}}"#);
    for _ in 0..5 {
        h.step();
    }
    send(&mut ws, r#"{"t":"copy"}"#);
    while !texts.iter().any(|t| t.contains("\"t\":\"clip\"") && t.contains("from the browser")) {
        assert!(std::time::Instant::now() < deadline, "no clipboard text: {texts:?}");
        h.step();
        if let Ok(Message::Text(t)) = ws.read() {
            texts.push(t.to_string());
        }
    }

    // Where to open it, with a QR code for phones.
    let ctx = h.ctx.clone();
    h.state_mut().0.run(crate::keymap::Command::ShowRemoteAddress, &ctx);
    shot(&mut h, "r3-address-dialog");
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(app(&h).ask.is_none(), "esc closes it");

    // Turned off: the server stops, the browser's link closes, and this
    // window is no longer listed.
    h.state_mut().0.run(crate::keymap::Command::ToggleRemoteAccess, &ctx);
    assert!(app(&h).ask.is_none(), "turning off asks nothing");
    wait_link_closed(&mut ws, || h.step());
    wait_port_closed(port, || h.step());
    h.step();
    assert!(!app(&h).remote.on && crate::instances::remote_server().is_none());
    assert!(crate::instances::remote_windows().is_empty());

    // Turning on again asks for a password again (typed hidden).
    h.state_mut().0.run(crate::keymap::Command::ToggleRemoteAccess, &ctx);
    h.step(); // the field takes the keyboard as the dialog appears
    type_text(&mut h, "secret");
    shot(&mut h, "r2-password-dialog");
    assert!(
        matches!(&app(&h).ask, Some(crate::app::Ask::RemotePassword { password }) if password == "secret")
    );
    press(&mut h, Modifiers::NONE, Key::Escape);
    h.step();
    assert!(!app(&h).remote.on, "cancelled: still off");
}

/// Remote access never outlives its window: off at launch whatever an old
/// build saved (and that build's password file deleted), stopped when
/// turned off from another window, and stopped when the window closes.
#[test]
#[ignore]
fn headless_remote_stops_with_the_window() {
    use crate::remote::testing::{link, sign_in};
    let _restore = keep_settings();
    std::fs::create_dir_all(crate::config::dir()).unwrap();
    std::fs::write(crate::config::settings_path(), "{ \"remoteAccess\": true }").unwrap();
    std::fs::write(crate::remote::old_secrets_path(), r#"{"salt":"00","hash":"00","iterations":1,"key":"00"}"#)
        .unwrap();
    let mut h = harness(None);
    for _ in 0..3 {
        h.step();
    }
    assert!(!crate::remote::old_secrets_path().exists(), "the old password file is gone");
    assert!(app(&h).config.errors.is_empty(), "{:?}", app(&h).config.errors);
    assert!(!app(&h).remote.on && crate::instances::remote_server().is_none(), "off at launch");
    drop(h);

    // Turned off from another window: it removes the marker, and the
    // serving window stops.
    let (mut h, port) = serve_remotely("pw", "");
    let session = sign_in(port, "pw").expect("signed in");
    let mut ws = link(port, std::process::id() as i32, &session);
    crate::instances::clear_remote_server(false);
    wait_link_closed(&mut ws, || {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    });
    wait_port_closed(port, || h.step());
    drop(h);

    // The serving window closes: the server stops with it.
    let (h, port) = serve_remotely("pw", "");
    let session = sign_in(port, "pw").expect("signed in");
    let mut ws = link(port, std::process::id() as i32, &session);
    drop(h);
    assert!(crate::instances::remote_server().is_none(), "the marker went with the window");
    assert!(crate::instances::remote_windows().is_empty());
    wait_link_closed(&mut ws, || {});
    wait_port_closed(port, || {});
}

/// Files under `dir` holding `needle`.
fn files_containing(dir: &std::path::Path, needle: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        if path.is_dir() {
            out.extend(files_containing(&path, needle));
        } else if std::fs::read(&path).is_ok_and(|b| String::from_utf8_lossy(&b).contains(needle)) {
            out.push(path);
        }
    }
    out
}

/// Wait (calling `step`) until the browser's live link is closed.
fn wait_link_closed(ws: &mut tungstenite::WebSocket<crate::remote::testing::Tls>, mut step: impl FnMut()) {
    ws.get_mut()
        .sock
        .set_read_timeout(Some(std::time::Duration::from_millis(30)))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(std::time::Instant::now() < deadline, "the browser is still linked");
        step();
        match ws.read() {
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// Wait (calling `step`) until nothing listens on `port`.
fn wait_port_closed(port: u16, mut step: impl FnMut()) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(std::time::Instant::now() < deadline, "still serving on {port}");
        step();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// A window with project `shop/a.rs` (holding `text`), remote access turned
/// on as a user would (the command, then the password), serving on a free
/// port once this returns.
fn serve_remotely(password: &str, text: &str) -> (Harness<'static, Root>, u16) {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("a.rs"), text).unwrap();
    let mut h = harness(Some(root.join("a.rs")));
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    h.state_mut().1.port = port;
    h.step();
    let ctx = h.ctx.clone();
    h.state_mut().0.run(crate::keymap::Command::ToggleRemoteAccess, &ctx);
    h.step(); // the field takes the keyboard as the dialog appears
    assert!(matches!(&app(&h).ask, Some(crate::app::Ask::RemotePassword { password }) if password.is_empty()));
    type_text(&mut h, password);
    press(&mut h, Modifiers::NONE, Key::Enter);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "the server never started");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(app(&h).remote.on && app(&h).remote.problem.is_none(), "{:?}", app(&h).remote);
    assert!(matches!(app(&h).ask, Some(crate::app::Ask::RemoteAddress)), "then where to open it");
    press(&mut h, Modifiers::NONE, Key::Escape);
    (h, port)
}

/// The real page in a real (headless) Chrome: sign in through the form, see
/// the window drawn on the canvas at the page's size, and type into it.
/// Skipped when Node or Chrome isn't installed.
#[test]
#[ignore]
fn headless_remote_in_chrome() {
    let Some((node, chrome)) = node_and_chrome() else {
        eprintln!("skipped: needs Node and Google Chrome");
        return;
    };
    let (mut h, port) = serve_remotely("open sesame", "fn main() {}\n");
    let shots = PathBuf::from(std::env::var("SHOTS").unwrap_or_else(|_| "target/shots".into()));
    std::fs::create_dir_all(&shots).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/remote_browser.mjs");
    let out_path = shots.join("chrome-run.log");
    let mut child = std::process::Command::new(node)
        .arg(&script)
        .arg(port.to_string())
        .arg("open sesame")
        .arg(&shots)
        .arg(chrome)
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    // Keep the window running while Chrome drives the page; turn remote
    // access off when it asks (to see the page say it's disconnected).
    let mut off = false;
    let status = loop {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(15));
        if !off && std::fs::read_to_string(&out_path).unwrap_or_default().contains("STEP turn-off") {
            off = true;
            let ctx = h.ctx.clone();
            h.state_mut().0.run(crate::keymap::Command::ToggleRemoteAccess, &ctx);
        }
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
    };
    let out = std::fs::read_to_string(&out_path).unwrap_or_default();
    assert!(off, "the page never got to remote access turning off: {out}");
    assert!(status.success() && out.lines().any(|l| l.starts_with("OK ")), "{out}");
    assert!(out.contains("listed=shop"), "{out}");
    // Chrome's keystrokes landed in the window's editor; its ⌥T opened the
    // terminal without typing "†" anywhere.
    for _ in 0..10 {
        h.step();
    }
    assert!(text_of(&h).contains("zq"), "{:?}", text_of(&h));
    assert!(app(&h).terminal_open, "⌥T from Chrome opened the terminal");
    assert!(!text_of(&h).contains('†'), "{:?}", text_of(&h));
    let screen = app(&h).terminal().map(|t| t.pane.screen().contents()).unwrap_or_default();
    assert!(!screen.contains('†'), "{screen:?}");
}

/// Node and Google Chrome, for the tests that drive the real page.
fn node_and_chrome() -> Option<(&'static str, &'static str)> {
    let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    let node = ["/opt/homebrew/bin/node", "/usr/local/bin/node"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())?;
    std::path::Path::new(chrome).exists().then_some((node, chrome))
}

/// Real key, mouse, wheel and touch events from Chrome (CDP's
/// `Input.dispatch*Event`, so through the page's real handlers) reach the
/// window as on the Mac: typing with ⇧, ⌥ accents and dead keys, ⌥T not
/// typing "†", ⌘P opening Go to File, the wheel and finger drags scrolling
/// (a flick coasting further), clicks, ⇧-click, triple-click and ⌘C/⌘X/⌘V
/// with this browser's clipboard, right-click and long-press menus, ⌃C in a
/// shell, links opening in the browser, the phone keyboard (input methods,
/// autocorrect, Backspace) and an edge swipe for the drawer. The script
/// prints `STEP <name>` once each part has landed; each is checked here.
#[test]
#[ignore]
fn headless_remote_input_in_chrome() {
    use crate::app::Focus;
    use std::io::BufRead;
    let Some((node, chrome)) = node_and_chrome() else {
        eprintln!("skipped: needs Node and Google Chrome");
        return;
    };
    let text: String = (0..300).map(|i| format!("row {i}\n")).collect();
    let (mut h, port) = serve_remotely("open sesame", &text);
    // No CJK font is bundled: the input method's "日本" draws as boxes.
    h.ctx.set_missing_glyph_policy(egui::MissingGlyphPolicy::Tofu);
    let shots = PathBuf::from(std::env::var("SHOTS").unwrap_or_else(|_| "target/shots".into()));
    std::fs::create_dir_all(&shots).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/remote_browser.mjs");
    let mut child = std::process::Command::new(node)
        .args([script.as_os_str(), port.to_string().as_ref(), "open sesame".as_ref(), shots.as_os_str()])
        .args([chrome, "input"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let lines = |h: &Harness<'static, Root>| -> Vec<String> { text_of(h).lines().map(String::from).collect() };
    // The row number on the line typed `suffix` at the end of ("row 12W").
    let row_with = |h: &Harness<'static, Root>, suffix: &str| -> Option<usize> {
        lines(h).iter().find_map(|l| l.strip_prefix("row ")?.strip_suffix(suffix)?.parse().ok())
    };
    let menu_open = |h: &Harness<'static, Root>| h.state().1.ctx().any_popup_open();
    let (mut out, mut seen, mut dragged_to) = (Vec::new(), Vec::new(), 0);
    let mut before_pinch = String::new();
    let mut caret_before = (0, 0);
    let status = loop {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(15));
        let Ok(line) = rx.try_recv() else {
            match child.try_wait().unwrap() {
                Some(status) if rx.try_recv().is_err() => break status,
                _ => continue,
            }
        };
        out.push(line.clone());
        let Some(step) = line.strip_prefix("STEP ") else { continue };
        for _ in 0..3 {
            h.step();
        }
        seen.push(step.to_string());
        let a = app(&h);
        let screen = a.terminal().map(|t| t.pane.screen().contents()).unwrap_or_default();
        let first = lines(&h).first().cloned().unwrap_or_default();
        let what = format!("at {step}: {first:?}");
        match step {
            "typed" => assert_eq!(first, "Hiéå row 0", "⇧H, ⌥E E, ⌥A, a Backspace {what}"),
            "lines" => {
                let second = &lines(&h)[1];
                assert!(second.ends_with("trrr") && second.starts_with(char::is_whitespace), "{second:?}")
            }
            "wheeled" => {
                let w: Vec<_> = lines(&h).into_iter().filter(|l| l.contains('W')).collect();
                assert!(row_with(&h, "W").is_some_and(|n| n >= 5), "the wheel scrolled {what} {w:?}")
            }
            "cut" => {
                let clip = out.iter().rev().find_map(|l| l.strip_prefix("CLIP2 ")).unwrap_or_default();
                assert!(clip.contains("trrr") && !text_of(&h).contains("trrr"), "⇧-click + ⌘X cut {clip} {what}");
            }
            "pasted" => assert!(text_of(&h).contains("P!"), "{what}"),
            "refocused" => {
                assert!(text_of(&h).contains("P!k"), "⇧ let go while away {what}");
                before_pinch = text_of(&h);
            }
            "pinched" => assert_eq!(text_of(&h), before_pinch, "a pinch types nothing"),
            "menu" | "long-pressed" => assert!(menu_open(&h), "a menu is up {what}"),
            "menu-closed" => assert!(!menu_open(&h), "{what}"),
            "palette" => assert_eq!(a.focus, crate::app::Focus::Sidebar, "⌘P went to the file search"),
            "palette-closed" => {
                assert_eq!(a.focus, crate::app::Focus::Editor);
                caret_before = a.active_tab().unwrap().caret();
            }
            // The page's window list kept its keys (↓, esc) and the click
            // that closed it: the editor saw none of them.
            "windows" => {
                assert_eq!(a.focus, crate::app::Focus::Editor, "{what}");
                assert!(a.palette.is_none() && !menu_open(&h), "{what}");
                assert_eq!(a.active_tab().unwrap().caret(), caret_before, "{what}");
            }
            "terminal" => {
                assert!(screen.contains("done-2"), "⌃C interrupted, then typed: {screen:?}");
                assert!(!screen.contains('†') && !text_of(&h).contains('†'), "{screen:?}");
            }
            "link-opened" => assert!(a.ask.is_none(), "answered"),
            "editor-again" => assert_eq!(a.focus, Focus::Editor),
            "dragged" => {
                dragged_to = row_with(&h, "A").unwrap_or(0);
                assert!(dragged_to >= 3, "a finger's drag scrolled {what}");
            }
            "flicked" => {
                let to = row_with(&h, "F").unwrap_or(0);
                assert!(to > dragged_to, "a flick coasts further: {to} vs {dragged_to}");
            }
            "tapped" => assert!(first.ends_with('T'), "a tap clicked {what}"),
            "after-pinch" => assert!(first.ends_with("TZ"), "taps work after a page pinch {what}"),
            "soft-keyboard" => {
                assert!(first.ends_with("hello日本th"), "phone keyboard, input method, autocorrect, Backspace {what}");
                assert!(!text_of(&h).contains("にほ") && !text_of(&h).contains("teh"), "{what}");
                assert!(lines(&h)[1].starts_with("row "), "Enter, then Backspace joined the line back");
            }
            "swiped" => assert!(a.views[1].compact && a.views[1].drawer, "an edge swipe opened the phone's drawer"),
            _ => {}
        }
    };
    let out = out.join("\n");
    assert!(status.success() && out.contains("\nOK "), "{out}");
    for step in ["typed", "wheeled", "menu", "palette", "windows", "terminal", "link-opened", "flicked", "long-pressed", "soft-keyboard", "swiped"] {
        assert!(seen.iter().any(|s| s == step), "{step} never ran: {out}");
    }
    let clip = |n| out.lines().find_map(|l| l.strip_prefix(&format!("CLIP{n} ")).map(String::from)).unwrap_or_default();
    assert!(clip(1).contains("Hiéå row 0"), "triple-click + ⌘C reached the browser's clipboard: {}", clip(1));
    assert!(clip(2).contains("trrr"), "⇧-click + ⌘X: {}", clip(2));
    assert!(!text_of(&h).contains("trrr"), "and it was cut");
    assert!(out.contains("OPENED \"https://example.com/rox\""), "the link opened in the browser: {out}");
    assert!(App::take_launched().is_empty(), "nothing opened on the Mac");
}

/// A small project with a file open, a terminal and the tree: what the
/// small-screen tests look at.
fn small_screen_project() -> (tempfile::TempDir, PathBuf, Harness<'static, Root>) {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    for d in [".git", "src/cart", "docs"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    for (f, body) in [
        ("src/main.rs", "mod cart;\n\n/// Where it all starts.\nfn main() {\n    let total = cart::checkout(&[3, 4, 5]);\n    println!(\"total: {total}\");\n}\n"),
        ("src/cart/mod.rs", "pub fn checkout(items: &[u32]) -> u32 {\n    items.iter().sum()\n}\n"),
        ("src/cart/tax.rs", "pub const RATE: f32 = 0.2;\n"),
        ("docs/guide.md", "# Guide\n"),
        ("README.md", "# Shop\n"),
        ("Cargo.toml", "[package]\nname = \"shop\"\n"),
    ] {
        std::fs::write(root.join(f), body).unwrap();
    }
    let mut h = harness(Some(root.join("src/main.rs")));
    h.state_mut().0.open_file(&root.join("src/cart/mod.rs"));
    h.state_mut().0.open_file(&root.join("README.md"));
    h.state_mut().0.select_tab(0);
    (dir, root, h)
}

/// Phones and tablets: the window laid out at their sizes.
#[test]
#[ignore]
fn headless_small_screens() {
    let (_dir, _root, mut h) = small_screen_project();
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", screen_has("$ "));
    press(&mut h, CMD, Key::J); // back to the editor
    for (name, w, ht) in [
        ("phone", 390.0, 844.0),
        ("phone-wide", 430.0, 932.0),
        ("phone-landscape", 844.0, 390.0),
        ("tablet", 768.0, 1024.0),
        ("tablet-landscape", 1024.0, 768.0),
        ("desktop", 1280.0, 820.0),
    ] {
        h.set_size(egui::vec2(w, ht));
        shot(&mut h, &format!("s-{name}"));
    }

    // The tree, on a phone: a drawer over the editor, its own to this view.
    h.set_size(egui::vec2(390.0, 844.0));
    h.step();
    let saved = app(&h).config.settings.sidebar;
    press(&mut h, CMD, Key::B);
    shot(&mut h, "s-phone-drawer");
    assert!(app(&h).views[0].drawer && app(&h).focus == crate::app::Focus::Sidebar);
    assert_eq!(app(&h).config.settings.sidebar, saved, "the drawer isn't the setting");
    // A tap beside it puts it away.
    click(&mut h, egui::pos2(370.0, 400.0), None);
    shot(&mut h, "s-phone-drawer-closed");
    assert!(!app(&h).views[0].drawer && app(&h).focus == crate::app::Focus::Editor);
    // Opening a file from it puts it away too.
    press(&mut h, CMD, Key::B);
    press(&mut h, Modifiers::NONE, Key::End);
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(!app(&h).views[0].drawer);
    assert!(app(&h).active_tab().is_some_and(|t| t.buf.name() == "README.md"), "opened from the drawer");
    press(&mut h, CMD_SHIFT, Key::P);
    shot(&mut h, "s-phone-palette");
    press(&mut h, Modifiers::NONE, Key::Escape);
    h.state_mut().0.tabs[0].buf.modified = true;
    h.state_mut().0.select_tab(0);
    let ctx = h.ctx.clone();
    h.state_mut().0.run(crate::keymap::Command::CloseTab, &ctx);
    shot(&mut h, "s-phone-dialog");
    press(&mut h, Modifiers::NONE, Key::Escape);
    h.state_mut().0.tabs[0].buf.modified = false;
    // Wide again: the tree is back beside the editor as it was.
    h.set_size(egui::vec2(1280.0, 820.0));
    shot(&mut h, "s-desktop-after");
    assert_eq!(app(&h).config.settings.sidebar, saved);
}

/// The other screens at phone size: start page, a folder's home page, the
/// shortcuts sheet, Go to File.
#[test]
#[ignore]
fn headless_small_screen_pages() {
    let mut h = harness(None);
    h.set_size(egui::vec2(390.0, 844.0));
    shot(&mut h, "s-phone-welcome");
    let (_dir, _root, mut h) = small_screen_project();
    h.set_size(egui::vec2(390.0, 844.0));
    press(&mut h, Modifiers::NONE, Key::F1);
    shot(&mut h, "s-phone-help");
    press(&mut h, Modifiers::NONE, Key::Escape);
    press(&mut h, ALT, Key::F);
    shot(&mut h, "s-phone-goto");
    press(&mut h, Modifiers::NONE, Key::Escape);
    h.state_mut().0.tabs.clear();
    h.state_mut().0.active = None;
    shot(&mut h, "s-phone-home");
}

/// A phone looking in gets its own compact layout — the tree as a drawer —
/// while the Mac's window, the saved settings and the window's own drawer
/// stay exactly as they were.
#[test]
#[ignore]
fn headless_remote_phone_layout() {
    use crate::remote::testing::{link, sign_in};
    use tungstenite::Message;
    let (mut h, port) = serve_remotely("open sesame", "fn main() {}\n");
    h.state_mut().0.config.settings.sidebar = true;
    let settings = std::fs::read_to_string(crate::config::settings_path()).unwrap_or_default();
    let session = sign_in(port, "open sesame").expect("signed in");
    let mut ws = link(port, std::process::id() as i32, &session);
    ws.get_mut()
        .sock
        .set_read_timeout(Some(std::time::Duration::from_millis(30)))
        .unwrap();
    let send = |ws: &mut tungstenite::WebSocket<_>, json: &str| ws.send(Message::text(json)).unwrap();
    send(&mut ws, r#"{"t":"size","w":390,"h":844,"dpr":1}"#);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !app(&h).views[1].compact {
        assert!(std::time::Instant::now() < deadline, "the phone's copy never laid out");
        h.step();
    }
    assert!(!app(&h).views[0].compact, "the Mac's window is still full size");
    // ⌘B on the phone: its drawer comes out; the Mac keeps its tree column.
    send(&mut ws, r#"{"t":"key","k":"b","code":"KeyB","down":true,"m":{"meta":true}}"#);
    send(&mut ws, r#"{"t":"key","k":"b","code":"KeyB","down":false,"m":{"meta":true}}"#);
    while !app(&h).views[1].drawer {
        assert!(std::time::Instant::now() < deadline, "⌘B never opened the phone's drawer");
        h.step();
    }
    // What the phone sees: every frame drawn (and acked), once they go quiet.
    let (mut screen, mut got, mut quiet) = (crate::remote::frames::Screen::default(), false, 0);
    while !got || quiet < quiet_reads(&h) {
        assert!(std::time::Instant::now() < deadline, "no frame came");
        h.step();
        match ws.read() {
            Ok(Message::Binary(b)) => {
                assert!(screen.apply(&b), "a frame");
                ws.send(Message::text(crate::remote::ACK)).unwrap();
                (got, quiet) = (true, 0);
            }
            _ => quiet += 1,
        }
    }
    let frame = screen.image();
    assert_eq!((frame.width(), frame.height()), (390, 844));
    let shots = PathBuf::from(std::env::var("SHOTS").unwrap_or_else(|_| "target/shots".into()));
    std::fs::create_dir_all(&shots).unwrap();
    frame.save(shots.join("s-remote-phone-drawer.png")).unwrap();
    shot(&mut h, "s-remote-mac-unchanged");
    let a = app(&h);
    assert!(!a.views[0].drawer && !a.views[0].compact);
    assert!(a.config.settings.sidebar, "the Mac's tree is still shown");
    assert_eq!(
        std::fs::read_to_string(crate::config::settings_path()).unwrap_or_default(),
        settings,
        "nothing saved"
    );
    drop(ws);
}

/// Keys typed and a shortcut landing in the same frame (a slow frame, or a
/// browser's batch): the shortcut runs and swallows only its own text (⌥T's
/// "†", right after its key); the typing before it still lands.
#[test]
#[ignore]
fn headless_chord_keeps_other_typing_in_its_frame() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join("a.rs"), "").unwrap();
    let mut h = harness(Some(root.join("a.rs")));
    let k = |key, modifiers| Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers };
    // "ab", then ⌘S, in one frame.
    h.event(Event::Text("ab".into()));
    h.event(k(Key::S, CMD));
    h.step();
    assert_eq!(text_of(&h), "ab");
    assert!(!h.state().0.active_tab().unwrap().buf.modified, "⌘S ran");
    // "c", then ⌥T with its "†", in one frame.
    h.event(Event::Text("c".into()));
    h.event(k(Key::T, ALT));
    h.event(Event::Text("†".into()));
    h.step();
    assert_eq!(text_of(&h), "abc", "the c stays, the † goes");
    assert!(app(&h).terminal_open, "⌥T ran");
}

/// egui reports ⇧ ⌃ ⌥ ⌘ pressed on their own as keys (`ShiftLeft`…), on
/// the Mac and from a browser alike. They aren't typing: pressing ⇧ to
/// ⇧-click after scrolling away keeps the view where it is; and recording
/// a shortcut records the chord, not the first modifier pressed.
#[test]
#[ignore]
fn headless_modifier_keys_are_not_typing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    let text: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("a.rs"), text).unwrap();
    let mut h = harness(Some(root.join("a.rs")));
    let modifier = |key, pressed, modifiers| Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers };
    h.event(Event::PointerMoved(egui::pos2(700.0, 400.0)));
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -600.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    });
    for _ in 0..30 {
        h.step();
    }
    let scrolled = app(&h).active_tab().unwrap().scroll[0].y;
    assert!(scrolled > 300.0, "{scrolled}");
    h.event(modifier(Key::ShiftLeft, true, Modifiers::NONE));
    h.event(Event::ModifiersChanged(SHIFT));
    h.step();
    h.step();
    assert_eq!(app(&h).active_tab().unwrap().scroll[0].y, scrolled, "⇧ alone jumped back to the caret");
    h.event(modifier(Key::ShiftLeft, false, SHIFT));
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();

    // Record ⌥E for Go to File, pressed as a person does: ⌥, then E.
    let bindings = crate::config::dir().join("keybindings.json");
    let before = std::fs::read_to_string(&bindings).ok();
    press(&mut h, CMD_SHIFT, Key::P);
    type_text(&mut h, "edit keyboard shortcuts");
    press(&mut h, Modifiers::NONE, Key::Enter);
    type_text(&mut h, "go to file");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(app(&h).capturing());
    h.event(modifier(Key::AltLeft, true, Modifiers::NONE));
    h.event(Event::ModifiersChanged(ALT));
    h.step();
    assert!(app(&h).capturing(), "⌥ alone isn't a shortcut");
    h.event(modifier(Key::E, true, ALT));
    h.step();
    let combo = app(&h).config.keymap.combo_for(crate::keymap::Command::GoToFile).map(|k| k.to_config());
    match before {
        Some(b) => std::fs::write(&bindings, b).unwrap(),
        None => {
            let _ = std::fs::remove_file(&bindings);
        }
    }
    assert_eq!(combo.as_deref(), Some("alt+e"));
}

/// A key that opens a list or a dialog never also acts inside it. From the
/// start screen: ↓ to Recent Folders, ↵ opens the list — it doesn't also
/// open the first folder in it. And a dialog that comes up in the same frame
/// as an ↵ isn't answered by that ↵.
#[test]
#[ignore]
fn headless_opening_key_stays_with_its_opener() {
    use crate::palette::Mode;
    // A recent folder to (wrongly) open.
    let dir = tempfile::tempdir().unwrap();
    let recent = dir.path().canonicalize().unwrap().join("old-project");
    std::fs::create_dir_all(recent.join(".git")).unwrap();
    crate::recent::record(&recent);

    let mut h = harness(None);
    h.step();
    press(&mut h, Modifiers::NONE, Key::ArrowDown); // Recent Folders
    press(&mut h, Modifiers::NONE, Key::Enter);
    h.step();
    assert_eq!(app(&h).palette.as_ref().map(|p| p.mode), Some(Mode::Recent), "the list opened");
    assert!(app(&h).project.is_none(), "and nothing in it was opened");
    shot(&mut h, "k-recent-list");
    // From there, ↵ is the list's: it opens the selected folder.
    press(&mut h, Modifiers::NONE, Key::Enter);
    h.step();
    assert!(app(&h).palette.is_none());

    // A dialog opened in the same frame as an ↵: the ↵ doesn't answer it.
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("a.rs"), "\n").unwrap();
    let mut h = harness(Some(root.join("a.rs")));
    h.step();
    // ⌘N (New File…) and ↵ in the very same frame (the harness's own key
    // helpers spread presses over frames, so put them in by hand).
    for (key, modifiers) in [(Key::N, CMD), (Key::Enter, Modifiers::NONE)] {
        for pressed in [true, false] {
            h.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        }
    }
    h.step();
    h.step();
    assert!(
        matches!(app(&h).ask, Some(crate::app::Ask::NewFile { .. })),
        "the dialog waits for a real answer"
    );
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(app(&h).ask.is_none());
}

/// Time and memory to open `MEASURE` (a file, or a folder whose `*.rs`
/// files become its saved session of tabs). Skipped unless `MEASURE` is
/// set; run one size per process so earlier allocations don't blur it:
/// `MEASURE=/path/big.rs cargo test --release headless_measure_open -- --ignored --nocapture`
#[test]
#[ignore]
fn headless_measure_open() {
    let Ok(target) = std::env::var("MEASURE") else { return };
    let target = PathBuf::from(target).canonicalize().unwrap();
    if target.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&target)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .collect();
        files.sort();
        crate::session::save(&target, &files, 0);
    }
    let footprint = || crate::sys::usage(std::process::id() as i32).unwrap().footprint_kb / 1024;
    let before = footprint();
    let start = std::time::Instant::now();
    let mut h = harness(Some(target.clone()));
    h.step();
    h.step();
    let opened = start.elapsed();
    let at_open = footprint();
    // Let background work (highlighting) finish, then look again.
    let settle = std::time::Instant::now();
    while settle.elapsed() < std::time::Duration::from_secs(4) {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!(
        "MEASURE {}: {} tabs, shown in {opened:?}, footprint +{} MB at open, +{} MB after 4 s",
        target.display(),
        app(&h).tabs.len(),
        at_open.saturating_sub(before),
        footprint().saturating_sub(before),
    );
}

/// A project whose saved session has `n` tabs (`f0.rs`…), the first active.
fn project_with_session(n: usize) -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("lazy");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let files: Vec<PathBuf> = (0..n)
        .map(|i| {
            let f = root.join(format!("f{i}.rs"));
            std::fs::write(&f, format!("fn f{i}() {{}}\n")).unwrap();
            f
        })
        .collect();
    crate::session::save(&root, &files, 0);
    (dir, root, files)
}

/// Restored tabs read nothing until they're shown, yet everything about
/// tabs works on them: names, switching, disk changes, split view, closing.
#[test]
#[ignore]
fn headless_restored_tabs_load_when_shown() {
    let (_dir, root, files) = project_with_session(4);
    let mut h = harness(Some(root.clone()));
    no_sidebar(&mut h);
    h.step();
    let loaded = |h: &Harness<'static, Root>| -> Vec<bool> {
        app(h).tabs.iter().map(|t| t.is_loaded()).collect()
    };
    assert_eq!(loaded(&h), [true, false, false, false], "only the tab in front was read");
    let names: Vec<String> = app(&h).tabs.iter().map(|t| t.buf.name()).collect();
    assert_eq!(names, ["f0.rs", "f1.rs", "f2.rs", "f3.rs"], "named without reading");
    assert!(app(&h).tabs[1].buf.lang.is_some(), "and their icons know the language");
    shot(&mut h, "lazy-0-restored");

    // Deleted before it was ever shown: struck through, nothing read.
    std::fs::remove_file(&files[3]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    h.step();
    h.step();
    assert!(app(&h).tabs[3].gone && !app(&h).tabs[3].is_loaded());

    // Switching to one reads it, and it's an ordinary tab.
    press(&mut h, CMD, Key::Num2);
    assert_eq!(app(&h).active, Some(1));
    assert_eq!(loaded(&h), [true, true, false, false]);
    assert_eq!(app(&h).tabs[1].buf.rope.to_string(), "fn f1() {}\n");
    type_text(&mut h, "x");
    assert!(app(&h).tabs[1].buf.modified);
    press(&mut h, CMD, Key::Z);

    // Go to File on a tab not read yet just switches to it (and reads it).
    h.state_mut().0.open_file(&files[2]);
    h.step();
    assert_eq!(app(&h).active, Some(2));
    assert_eq!(app(&h).tabs.len(), 4);
    assert!(app(&h).tabs[2].is_loaded());
    shot(&mut h, "lazy-1-shown");

    // Close All: nothing unsaved, so no questions, and no file was touched.
    press(&mut h, CMD_SHIFT, Key::W);
    h.step();
    assert!(app(&h).prompt.is_none());
    assert!(app(&h).tabs.is_empty());
    assert_eq!(std::fs::read_to_string(&files[1]).unwrap(), "fn f1() {}\n");
}

/// Split view shows every tab, so it reads each one it shows.
#[test]
#[ignore]
fn headless_split_view_loads_its_tiles() {
    let (_dir, root, _files) = project_with_session(3);
    let mut h = harness(Some(root));
    no_sidebar(&mut h);
    h.step();
    assert!(!app(&h).tabs[2].is_loaded());
    press(&mut h, CMD, Key::Backslash);
    h.step();
    assert!(app(&h).editor_grid);
    assert!(app(&h).tabs.iter().all(|t| t.is_loaded()));
    shot(&mut h, "lazy-2-split");
}

/// `lines` lines of `line n` text, with `marker` on line `marker_at`.
fn write_numbered(path: &std::path::Path, lines: usize, marker_at: usize) {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for n in 0..lines {
        if n == marker_at {
            writeln!(f, "let found_me_here = {n}; // marker").unwrap();
        } else {
            writeln!(f, "let value_{n} = compute({n}, \"text\"); // line {n}").unwrap();
        }
    }
}

/// A file far past `HUGE_BYTES` opens at once, read-only, holding only the
/// lines on screen; the keyboard reaches its end, Go to Line and Find work.
#[test]
#[ignore]
fn headless_huge_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("logs");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let path = root.join("huge.rs");
    let lines = 2_400_000; // ~120 MB
    write_numbered(&path, lines, 2_000_000);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > 100 << 20);

    let footprint = || crate::sys::usage(std::process::id() as i32).unwrap().footprint_kb / 1024;
    let before = footprint();
    let start = std::time::Instant::now();
    let mut h = harness(Some(path.clone()));
    h.step();
    let opened = start.elapsed();
    let grew = footprint().saturating_sub(before);
    eprintln!("huge file ({} MB): shown in {opened:?}, footprint +{grew} MB", size >> 20);
    assert!(opened < std::time::Duration::from_secs(3), "{opened:?}");
    assert!(grew < 100, "only what's on screen is held: +{grew} MB");
    let view = |h: &Harness<'static, Root>| {
        let t = app(h).active_tab().unwrap();
        let v = t.huge.as_ref().expect("opened as the huge-file view");
        (v.top, v.cursor, v.lines())
    };
    assert_eq!(view(&h).2, lines + 1, "the empty line after the last break counts");
    shot(&mut h, "huge-0-open");

    // To the end and back, by keyboard.
    press(&mut h, CMD, Key::ArrowDown);
    let (top, cursor, n) = view(&h);
    assert_eq!(cursor, n - 1);
    assert!(top <= cursor && cursor < top + 60);
    let t = app(&h).active_tab().unwrap();
    assert_eq!(t.line_text(lines - 1), format!("let value_{0} = compute({0}, \"text\"); // line {0}", lines - 1));
    shot(&mut h, "huge-1-end");
    press(&mut h, Modifiers::NONE, Key::PageUp);
    assert!(view(&h).1 < n - 10);

    // Read-only: typing changes nothing, and nothing can be saved over it.
    type_text(&mut h, "oops");
    press(&mut h, CMD, Key::S);
    assert!(!app(&h).active_tab().unwrap().buf.modified);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
    assert!(!app(&h).available(crate::keymap::Command::Paste));
    assert!(app(&h).available(crate::keymap::Command::EditWholeFile));

    // Go to Line.
    press(&mut h, CMD, Key::L);
    type_text(&mut h, "1234567");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert_eq!(view(&h).1, 1_234_566);
    assert_eq!(
        app(&h).status_position().as_deref(),
        Some("Ln 1234567 of 2400001  ·  read-only")
    );

    // Find searches the file on disk.
    press(&mut h, CMD, Key::F);
    type_text(&mut h, "FOUND_ME");
    h.step();
    let t = app(&h).active_tab().unwrap();
    let found = t.huge.as_ref().unwrap().found;
    assert_eq!(found.map(|f| (f.0, f.1)), Some((2_000_000, 4)), "case ignored");
    assert_eq!(view(&h).1, 2_000_000);
    shot(&mut h, "huge-2-found");
    press(&mut h, Modifiers::NONE, Key::Enter);
    let t = app(&h).active_tab().unwrap();
    assert_eq!(t.huge.as_ref().unwrap().found.map(|f| f.0), Some(2_000_000), "the only one: wraps to itself");
    press(&mut h, Modifiers::NONE, Key::Escape);
}

/// A big file that still fits in memory edits as ever, with highlighting,
/// wrapping and widths worked out only around the view.
#[test]
#[ignore]
fn headless_big_file_works_around_the_view() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("big");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let path = root.join("big.rs");
    write_numbered(&path, 60_000, 50_000); // ~3 MB
    let mut h = harness(Some(path.clone()));
    no_sidebar(&mut h);
    h.state_mut().0.config.settings.word_wrap = true;
    h.step();
    assert!(app(&h).active_tab().unwrap().huge.is_none());

    press(&mut h, CMD, Key::L);
    type_text(&mut h, "50001");
    press(&mut h, Modifiers::NONE, Key::Enter);
    type_text(&mut h, "/*x*/ ");
    let t = app(&h).active_tab().unwrap();
    assert!(t.line_text(50_000).starts_with("/*x*/ let found_me_here"));
    assert!(t.buf.modified);
    // Highlighting arrives for the lines on screen, not for the whole file.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app(&h).active_tab().unwrap().line_spans(50_000).is_empty() {
        assert!(std::time::Instant::now() < deadline, "no highlighting near the view");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let t = app(&h).active_tab().unwrap();
    assert!(t.line_spans(10).is_empty(), "far from the view: not worked out");
    assert!(t.view.contains(&50_000), "the caret's line is on screen: {:?}", t.view);
    shot(&mut h, "big-around-view");
    press(&mut h, CMD, Key::S);
    assert!(std::fs::read_to_string(&path).unwrap().contains("/*x*/ let found_me_here"));
}

/// The remote picture, measured in a real (headless) Chrome at a retina
/// laptop's size: key-to-frame latency, frame rates and bytes while typing,
/// scrolling and streaming a terminal, and quiet when idle. Prints the
/// numbers (`--nocapture`); fails only when something is far off.
#[test]
#[ignore]
fn headless_remote_frames_in_chrome() {
    let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    let node = ["/opt/homebrew/bin/node", "/usr/local/bin/node"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    let (Some(node), true) = (node, std::path::Path::new(chrome).exists()) else {
        eprintln!("skipped: needs Node and Google Chrome");
        return;
    };
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let (mut h, port) = serve_remotely("open sesame", include_str!("app.rs"));
    let shots = PathBuf::from(std::env::var("SHOTS").unwrap_or_else(|_| "target/shots".into()));
    std::fs::create_dir_all(&shots).unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/remote_frames.mjs");
    let out_path = shots.join("chrome-frames.log");
    let mut child = std::process::Command::new(node)
        .arg(&script)
        .arg(port.to_string())
        .arg("open sesame")
        .arg(&shots)
        .arg(chrome)
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    // The window's event loop, as eframe would run it: a pass whenever
    // something asks (here: as often as the harness can).
    let status = loop {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(1));
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
    };
    let out = std::fs::read_to_string(&out_path).unwrap_or_default();
    println!("{out}");
    let (enc, remote) = (h.state().1.encode_stats(), &h.state().1);
    println!(
        "SERVER frames={} half-res={} kb/frame={:.1} encode ms/frame={:.2} render ms/frame={:.2}",
        enc.frames,
        enc.half,
        enc.bytes as f64 / enc.frames.max(1) as f64 / 1024.0,
        enc.encode_secs * 1000.0 / enc.frames.max(1) as f64,
        remote.render_secs * 1000.0 / remote.renders.max(1) as f64,
    );
    assert!(status.success() && out.contains("\nOK"), "{out}");
    let metric = |name: &str| -> serde_json::Value {
        out.lines()
            .filter_map(|l| l.strip_prefix("METRIC "))
            .filter_map(|j| serde_json::from_str::<serde_json::Value>(j).ok())
            .find(|m| m["name"] == name)
            .unwrap_or_else(|| panic!("no {name} metric: {out}"))
    };
    let p50 = metric("key-to-frame")["arrivedMs"]["p50"].as_f64().unwrap();
    assert!(p50 < 150.0, "a key's frame took {p50} ms");
    let exact = metric("exactness");
    assert!(exact["reconnected"].as_bool().unwrap(), "{out}");
    assert!(
        exact["differ"].as_f64().unwrap() < exact["pixels"].as_f64().unwrap() * 0.002,
        "what the page built differs from the whole frame: {exact}"
    );
    for name in ["typing", "scrolling", "terminal"] {
        assert!(metric(name)["frames"].as_f64().unwrap() >= 5.0, "{name}: too few frames: {out}");
    }
}

/// Wherever the keyboard is, every command that can run there shows its key
/// somewhere on screen (the status bar, the tab rows, buttons, the editor's
/// right-click menu); the rest are app-wide, listed by the palette (⇧⌘P)
/// and the F1 sheet, whose keys always show, or live in a right-click menu
/// of a tab or the tree.
#[test]
#[ignore]
fn headless_every_key_that_applies_shows() {
    use crate::keymap::Command::*;
    let elsewhere = [
        // App-wide: the palette and F1.
        SelectTheme, SelectUiFont, SelectCodeFont, Settings, Keybindings, OpenSettings, OpenKeybindings,
        OpenProjectSettings, OpenConfigFolder, Todos, ClipboardHistory, ZoomIn, ZoomOut, ResetZoom, FullScreen,
        MaximizeWindow, MinimizeWindow, ToggleRemoteAccess, Quit, CloseFolder, ToggleBuildFolders, ToggleWordWrap, NewTerminal,
        ToggleTerminal, Palette, SearchInFiles, SwitchFocus,
        // A tab's or the tree's right-click menu.
        RenameFile, RevealInFinder, OpenInTerminal, CopyPath, CopyRelativePath,
        // On each tab's tooltip (⌘1–8); ⌘9 is "the last".
        GoToTab(9),
        // ⌘S always saves; its hint shows while there's something to save.
        Save,
    ];
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    std::fs::write(home.join(".zshrc"), "PROMPT='%1~ $ '\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    for f in ["a.rs", "b.rs"] {
        std::fs::write(root.join(f), "fn x() {}\n").unwrap();
    }
    let check = |h: &mut Harness<'static, Root>, place: &str| {
        h.step(); // settled: a new window's first frame is still laying out
        crate::keymap::record_shown(true);
        h.step();
        let shown = crate::keymap::record_shown(false);
        let missing: Vec<_> = crate::keymap::COMMANDS
            .iter()
            .map(|c| c.command)
            .filter(|c| app(h).available(*c) && !shown.contains(c) && !elsewhere.contains(c))
            .collect();
        assert!(missing.is_empty(), "in {place}, these run but show their key nowhere: {missing:?} (status: {:?})", app(h).status_line());
    };
    let mut h = harness(None);
    check(&mut h, "the welcome page");
    let mut h = harness(Some(root.clone()));
    check(&mut h, "the empty-folder page");
    h.state_mut().0.open_file(&root.join("a.rs"));
    h.state_mut().0.open_file(&root.join("b.rs"));
    h.step();
    check(&mut h, "the editor");
    type_text(&mut h, "x");
    check(&mut h, "the editor, unsaved");
    h.state_mut().0.select_tab(0);
    type_text(&mut h, "y");
    check(&mut h, "the editor, two unsaved");
    shot(&mut h, "keys-editor-unsaved");
    // Saving's keys, by the editor: this file's, and all of them.
    let row = app(&h).editor_hints();
    for what in ["save", "save all"] {
        assert!(row.iter().any(|(_, w)| *w == what), "{what} on the editor's row: {row:?}");
    }

    // The editor's right-click menu: its items with their keys, and they run.
    let editor = egui::pos2(700.0, 300.0);
    click_at(&mut h, editor, egui::PointerButton::Secondary);
    shot(&mut h, "keys-editor-menu");
    let keys = |c| app(&h).config.keymap.combo_for(c).unwrap().label();
    for c in [crate::keymap::Command::Undo, crate::keymap::Command::Redo] {
        assert!(h.query_all_by_label_contains(&keys(c)).count() >= 1, "{c:?} shows its key");
    }
    h.get_by_label_contains("Select All").click();
    h.step();
    h.step();
    assert!(app(&h).active_tab().unwrap().buf.selection().is_some(), "Select All from the menu");
    click_at(&mut h, editor, egui::PointerButton::Secondary);
    h.get_by_label_contains("Save All").click();
    h.step();
    h.step();
    assert!(app(&h).tabs.iter().all(|t| !t.buf.modified), "Save All from the menu");
    press(&mut h, CMD, Key::F);
    check(&mut h, "the find bar");
    press(&mut h, Modifiers::NONE, Key::Escape);
    h.state_mut().0.focus_sidebar();
    h.step();
    check(&mut h, "the tree");
    press(&mut h, ALT, Key::T);
    wait_for(&mut h, "a prompt", at_prompt);
    check(&mut h, "a terminal");
    shot(&mut h, "keys-terminal");
}
