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
        .build_eframe(move |cc| Root(App::new(&cc.egui_ctx, target)))
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

fn press(h: &mut Harness<'static, Root>, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.step();
}

fn type_text(h: &mut Harness<'static, Root>, text: &str) {
    h.event(Event::Text(text.into()));
    h.step();
}

const CMD: Modifiers = Modifiers {
    mac_cmd: true,
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
    mac_cmd: true,
    command: true,
    shift: true,
    ..Modifiers::NONE
};

#[test]
#[ignore]
fn headless_welcome() {
    let mut h = harness(None);
    shot(&mut h, "01-welcome");
    // The real app icon is the welcome mark, not a stand-in.
    assert!(app(&h).app_icon.is_some(), "the app icon loaded");
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
    let (row_h, x0, y0) = (24.0, 108.0, 32.0 + 36.0 + 12.0); // code size 15 → 24px rows
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

    // Quit with unsaved work asks; Save writes it.
    press(&mut h, CMD, Key::Q);
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
    for _ in 0..5 {
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
    assert!(
        !idle.is_empty() && soonest >= std::time::Duration::from_millis(150),
        "{idle:?}"
    );

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
    let pal = app(&h).palette.as_ref().expect("Go to File stays open");
    assert_eq!(
        pal.items[pal.selected].label, "new_name.rs",
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
    assert!(
        app(&h).palette.as_ref().unwrap().items.is_empty(),
        "node_modules is skipped"
    );
    shot(&mut h, "a5-no-match-hint");
    press(&mut h, ALT, Key::H);
    wait_for(&mut h, "re-indexing with build folders", |a| {
        a.palette.as_ref().is_some_and(|p| !p.items.is_empty())
    });
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
        !app(&h).help && app(&h).palette.is_some(),
        "F1 again closes help, back to the list"
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
    press(&mut h, CMD, Key::W);
    h.state_mut().0.focus = crate::app::Focus::Editor;
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

    // From the file tree: its own keys (⌘← collapse all) get through.
    h.state_mut().0.focus_sidebar();
    h.step();
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
    let hint = egui::pos2(155.0, 807.0); // "⌥O recent"
    click_at(&mut h, hint, egui::PointerButton::Primary);
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
    let row = |i: usize| egui::pos2(80.0, 32.0 + 38.0 + 26.0 * i as f32 + 13.0);
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
    use crate::palette::Mode;
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

    // Its keys work from the page: ⌥F browses the tree.
    press(&mut h, ALT, Key::F);
    shot(&mut h, "k1-files");
    assert_eq!(app(&h).palette.as_ref().map(|p| p.mode), Some(Mode::Files));
    let label = |h: &Harness<'static, Root>| {
        let p = app(h).palette.as_ref().unwrap();
        p.items[p.selected].label.clone()
    };
    while label(&h) != "src" {
        press(&mut h, Modifiers::NONE, Key::ArrowDown);
    }
    shot(&mut h, "k2-folder-selected");
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    shot(&mut h, "k3-folder-expanded");
    let p = app(&h).palette.as_ref().unwrap();
    assert!(p.items.iter().any(|i| i.label == "main.rs" && i.depth == 1));
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    shot(&mut h, "k4-file-selected");
    assert_eq!(label(&h), "main.rs");
    press(&mut h, Modifiers::NONE, Key::Enter);
    assert!(app(&h).palette.is_none());
    assert!(
        app(&h)
            .tab_rel(app(&h).active_tab().unwrap())
            .ends_with("main.rs")
    );

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
    let signed = std::process::Command::new("codesign")
        .args(["-s", "-", "-f"])
        .arg(root.join("claude"))
        .output()
        .unwrap();
    assert!(signed.status.success());
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
        "./claude -c 'for i in $(seq 1 16); do echo working $i; sleep 0.25; done; sleep 30'",
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
    assert_eq!(last.caps.concat(), "⌘O");
    shot(&mut h, "f0-recent");
    press(&mut h, ALT, Key::O);
    assert!(app(&h).palette.is_none(), "⌥O again closes it");
    press(&mut h, ALT, Key::F);
    press(&mut h, ALT, Key::F);
    assert!(app(&h).palette.is_none(), "so does ⌥F");
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
    click_at(&mut h, egui::pos2(700.0, 87.0), egui::PointerButton::Secondary); // e.rs's title
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
        Command::GoToFile,
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
        // Confirmations: delete (long name), then save-changes.
        let ctx = h.ctx.clone();
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

/// ⇧⌥F formats with the language's tool (rustfmt here), as one undo step;
/// `"formatters"` adds or replaces tools, `"formatOnSave"` runs it on ⌘S,
/// and a missing tool says how to install it.
#[test]
#[ignore]
fn headless_format_document() {
    const SHIFT_ALT: Modifiers = Modifiers {
        shift: true,
        alt: true,
        ..Modifiers::NONE
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join(".oxru")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let messy = "fn  main(){let x=1;println!(\"{}\",x);}\n";
    std::fs::write(root.join("main.rs"), messy).unwrap();
    std::fs::write(root.join("notes.txt"), "shout\n").unwrap();
    std::fs::write(root.join("a.md"), "# hi\n").unwrap();
    std::fs::write(
        root.join(".oxru/settings.json"),
        r#"{ "formatters": { "txt": "tr a-z A-Z", "md": "no-such-formatter-xyz" }, "formatOnSave": true }"#,
    )
    .unwrap();
    let last_toast = |h: &Harness<'static, Root>| {
        app(h).toasts.last().map(|t| (t.msg.clone(), t.error)).unwrap_or_default()
    };
    let mut h = harness(Some(root.join("main.rs")));

    if which("rustfmt") {
        press(&mut h, SHIFT_ALT, Key::F);
        assert_eq!(
            text_of(&h),
            "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n",
            "{:?}",
            last_toast(&h)
        );
        shot(&mut h, "f0-formatted");
        press(&mut h, CMD, Key::Z);
        assert_eq!(text_of(&h), messy, "one undo step");
    } else {
        eprintln!("rustfmt not installed: skipped the real-tool step");
    }

    // Your own formatter, and format on save.
    h.state_mut().0.open_file(&root.join("notes.txt"));
    h.step();
    press(&mut h, CMD, Key::ArrowDown);
    type_text(&mut h, "more");
    press(&mut h, CMD, Key::S);
    assert_eq!(std::fs::read_to_string(root.join("notes.txt")).unwrap(), "SHOUT\nMORE");
    assert!(!app(&h).active_tab().unwrap().buf.modified, "saved what was formatted");

    // A tool that isn't there: told, text untouched.
    h.state_mut().0.open_file(&root.join("a.md"));
    h.step();
    press(&mut h, SHIFT_ALT, Key::F);
    let (msg, error) = last_toast(&h);
    assert!(error && msg.contains("isn't installed"), "{msg}");
    assert_eq!(text_of(&h), "# hi\n");
    shot(&mut h, "f1-missing-tool");
}

fn which(tool: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .env("PATH", format!("{}:/opt/homebrew/bin:{}/.cargo/bin", std::env::var("PATH").unwrap_or_default(), std::env::var("HOME").unwrap_or_default()))
        .output()
        .is_ok_and(|o| o.status.success())
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
fn keep_settings() -> impl Drop {
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

    // ⌘← collapses the tree only while the tree has the keyboard.
    assert!(open_dirs(&h) > 0, "the open file's folders start expanded");
    press(&mut h, CMD, Key::ArrowLeft); // in the editor: line start, tree untouched
    assert!(open_dirs(&h) > 0);
    press(&mut h, CMD_SHIFT, Key::E);
    assert_eq!(app(&h).focus, Focus::Sidebar);
    shot(&mut h, "k0-tree-hints");
    press(&mut h, CMD, Key::ArrowLeft);
    assert_eq!(open_dirs(&h), 0);
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
    assert!(hints.contains(&(vec!["⌥⌘[".into(), "⌥⌘]".into()], "close left / right")), "{hints:?}");
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
    assert!(hints.iter().any(|(c, w)| *w == "close" && c == &vec!["⌥W".to_string()]), "{hints:?}");
    assert!(hints.iter().any(|(_, w)| *w == "close left / right"));
    shot(&mut h, "z2-terminal-tab-hints");
}
