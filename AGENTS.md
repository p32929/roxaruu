# Roxaruu — working rules

A keyboard-first editor with built-in terminals, on egui (Rust). Beautiful by
default; every action is a named command with a remappable key; settings,
keys and themes are JSON in `~/.config/oxru/`. Mouse works too, but the
keyboard comes first: wherever an action can happen, show its shortcut.
Anything the mouse can do, a key can do: every button, menu item and
click-only gesture is a named command (so it's in the palette, the F1
sheet, and remappable), and every command ships with a default key that
clashes with nothing (`every_command_has_a_default_key`,
`defaults_never_conflict`, `headless_every_shortcut`). A key shown on
screen works from wherever the keyboard is (`visible_keys_work`): never
gate a command on focus while its key is drawn elsewhere, and never show
one that can't run now. The keys between the editor, the tree and the
terminal come from `focus_hint`, shown in the status bar and on the area
they go to (`headless_focus_keys_everywhere`).

These rules apply to every change, however small.

## 1. A change is finished only when everything connected to it is

Before editing, find every place the thing you're changing is used
(`grep` the name, the command, the settings key). Update them all in the
same change. Never leave "the menu still says the old thing" or "the other
window doesn't get it" for the user to discover.

Checklists for the usual changes (the guard tests named here fail if a
step is missed — keep them passing, and add one when you find a new kind
of "forgot to update X"):

**A new command** (`keymap.rs`)
- `Command` enum + one `COMMANDS` row: id `<section>.<name>` (the section
  — `file`, `folder`, `tab`, `edit`, `search`, `view`, `terminal`, `app` —
  puts it in the right group of the F1 shortcuts sheet), title, default
  keys (`defaults_never_conflict`, `command_ids_are_unique_and_titled`).
- `icon::for_command` in `ui.rs` (the match is exhaustive).
- `App::available` (when it can run) and `App::run` (what it does) in `app.rs`.
- Show its key where it applies, always, for where the keyboard is now:
  status bar hints for the focused area (`status_hints`), the tab strips
  for tab actions (`tab_hints`, drawn with `Paint::hints_in`, which keeps
  the most useful when room is short), palette footers (`palette_footer`),
  right-click menus (`tab_menu`, `terminal_menu`, `sidebar_menu`), dialog
  buttons, and the empty-folder page (`project_home`).
- If it's tab-like, make it work for both editor tabs and terminals (see
  `close_around`, `NextTab`/`MoveTabLeft` with a terminal focused). Both
  strips scroll and drag through `ui/tabstrip.rs`; moving goes through
  `App::reorder` (`headless_tab_strips_scroll_and_drag`).

**A new setting** (`config.rs` + `app/extras.rs`)
- `Settings` field + `Default` + the commented line in `settings_template`.
- If it belongs on the Settings screen: `Setting` enum, `Setting::ALL`,
  `key`, `title`, `icon`, `setting_value`, `change_setting`
  (`every_setting_is_a_real_documented_key`).
- Saving goes through `save_choice` / `save_choice_quietly`, so every open
  window picks it up (`watch_config` wakes them). Never keep a setting only
  in memory.
- A project's `.oxru/settings.json` overrides the global file key by key.

**A new theme colour** (`theme.rs`)
- The `colors!` list, `Colors::token` if it's a syntax colour, and a value
  in **every** `assets/themes/*.json` (`builtins_set_every_syntax_colour`).

**A new language** (`syntax.rs`)
- Its `tree-sitter-<lang>` crate, one `LANGUAGES` entry (id, extensions,
  comment marker, grammar), a sample in `every_grammar_builds_and_highlights`.
- A file icon in `icon::for_file` (`ui.rs`).

**A new dialog or overlay**
- Use `App::modal` (title, body, buttons with their keys; a second row for
  "all" choices). Never wider or taller than 70% of the window; content
  scrolls inside (`headless_dialogs_fit`). It must own the keyboard while
  up (`modal_open`) and show the key for every button.

**A new palette mode** (`palette.rs`, `ui.rs`)
- `Mode` + `placeholder`, items in `App::refresh_items`, its footer hints in
  `palette_footer`, and the key that opens it also closes it (`run`).

**Remote access** (`remote/`)
- The browser's copy of a window is the same `App::frame`, run on a second
  egui context (`app.remote_pass` is set meanwhile). Anything tied to the
  window's own context must use `main_ctx` or be per-context (like the app
  icon texture); anything platform-only (window buttons, viewport commands)
  must check `remote_pass`.
- Lifecycle (see `remote/mod.rs`): off at every launch, never a setting.
  Turning it on (`ToggleRemoteAccess`) always asks for a password, held
  only in memory by the window that turned it on, which runs the server
  (`hub`) and writes `remote-server.json` (pid + port, no secrets) in the
  running dir. Other windows follow that marker: live → open their private
  link; gone or its pid dead → close it. Turning off anywhere removes the
  marker; the serving window stops when the marker goes or when it's
  dropped (window closed / app quit). The page says when its link is gone
  (a dialog after a moment down, retrying until it's back; a note when the
  window it showed closed): `headless_remote_in_chrome` turns access off
  under it. Nothing secret ever goes to disk
  (`headless_remote_access` checks); an old `remote.json` is deleted and an
  old `"remoteAccess"` settings key ignored (`headless_remote_stops_with_the_window`).
- Every security check has a test in `remote/hub.rs` / `auth.rs`: keep them.
- `headless_remote_access` drives it end to end; `headless_remote_in_chrome`
  runs the real page in headless Chrome (`tests/remote_browser.mjs`, needs
  Node + Chrome; it skips itself otherwise). `serve_remotely` turns it on
  as a user would (command, then password).

**Files and tabs**
- Anything that changes files on disk refreshes the tree and the search
  index (`files_changed`). Tabs follow outside changes in
  `poll_file_changes` (reload, ask, or strike through when deleted).
- A tab may not hold its text. Restored tabs are `Tab::lazy` and read
  when first shown (`App::load_tab`, called by `editor_tile` and `tick`);
  a file over `huge::HUGE_BYTES` (or with a line over `LONG_LINE`) is a
  read-only `tab.huge` view that reads only the lines on screen. Read a
  tab through `Tab` (`line_count`, `line_text`, `caret`, `go_to`,
  `copy_text`), gate edits on `App::editable`; an unread `buf` refuses to
  save (`a_lazy_tab_reads_nothing_until_loaded`,
  `headless_restored_tabs_load_when_shown`, `headless_huge_file`).
- Work per edit or per frame follows the screen, not the file: past
  `WHOLE_FILE_LINES`, highlighting, word wrap and widths cover only the
  lines `around` the view (`headless_big_file_works_around_the_view`).

## 2. Don't repeat yourself

Look for an existing helper before writing one; extend it rather than
copying it. The shared pieces:

- `App::modal`, `command_menu` + `MenuGroups` (right-click menus),
  `App::shortcut` (a command's keycaps), `Paint::key_hints` / `Paint::row`
  (key hints and list rows), `help_sections` (F1 sheet, built from the
  keymap — never a hand-kept list).
- `App::begin_close` (closing with "save changes?"), `close_around`
  (others / left / right / saved), `save_choice[_quietly]` (persist a
  setting), `files_changed`, `notify` / `warn` (toasts).
- `crate::terminal::grid_rects` (tiling), `fuzzy::score` / `positions`,
  `config::set_key` (edit JSON keeping comments).

Data lives in one place and everything else is derived from it: a
language's comment marker lives on its `Lang`, a command's key in the
keymap, a setting's key in `Setting::key`. If you're about to write the
same list twice, make one table and read it from both places.

## 3. Tests: everything has one, and they all pass

- Every pure function gets a unit test next to it (`#[cfg(test)] mod tests`)
  — the smallest test that fails if the logic breaks. Every bug fix gets a
  test that would have caught it.
- UI behaviour is tested headless in `src/headless.rs` (egui_kittest):
  simulated keys and clicks, assertions on app state, PNG screenshots to
  look at. **Never open a real window** (it steals focus from the user).
- Guard tests protect the connections above; extend them, don't delete them.
- Before calling anything done, run all of it:

```sh
cargo clippy --release --all-targets          # must be clean
SP=<scratch dir>
HOME=$SP/home SHOTS=$SP/shots PROJECT=<a file inside a git repo> BIG=<a big source file> \
  cargo test --release -- --include-ignored --test-threads=1
```

  (On this machine prefix with `CARGO_HOME=$HOME/.cargo RUSTUP_HOME=$HOME/.rustup`.)
  `HOME` must point at a scratch folder: tests write settings, and must
  never touch the real `~/.config/oxru`. A test that changes shared files
  restores them (see `headless_settings_reach_all_windows`). Timing-based
  terminal tests can be flaky under load: re-run alone before blaming the
  change, and fix the test if it's racy.

## 4. Ground rules

- Don't run `install-local.sh` or `run.sh` without `--temp`: replacing the
  installed app closes the user's running windows. Build and test only.
- Commit and push only when asked. Bump the version when asked: `./install-local.sh --vup [patch|minor|major]` (patch by default) bumps `Cargo.toml` and installs; the version shows in the status bar.
- Comments explain why, briefly; match the surrounding style. A deliberate
  shortcut with a known limit gets a `ponytail:` comment naming the limit.
- Report honestly: what was verified (and how), what wasn't, and why.

## Map

| File | What's there |
|---|---|
| `main.rs` | start-up, CLI (`roxaruu [path]`, `rox`) |
| `app.rs` | app state, commands (`run`, `available`), tabs, closing, config reload |
| `app/files.rs` | new / rename / delete / reveal / copy path, drops |
| `app/extras.rs` | toasts, clipboard history, to-dos, Settings screen, key editor, agent alerts |
| `ui.rs` | all drawing: title/status bars, tabs, editor, terminals, palette, dialogs, F1 sheet |
| `ui/sidebar.rs` | the file tree and its search box (Go to File) |
| `ui/tabstrip.rs` | tab strips that scroll sideways and drag tabs to a new place |
| `keymap.rs` | commands and keys |
| `config.rs` | settings / keybindings / themes files, JSON editing |
| `theme.rs` | colour themes (`assets/themes/*.json`) |
| `syntax.rs` | tree-sitter highlighting, languages |
| `buffer.rs` / `editor.rs` | text, undo, carets / a tab: buffer + highlighting |
| `huge.rs` | files too big to hold: line index, reading lines and searching on disk |
| `terminal.rs` / `terminalpane.rs` | terminal input / one PTY + screen, agent watching |
| `termbridge.rs` | scripts' "open a Terminal" requests become tabs here |
| `palette.rs`, `fuzzy.rs`, `search.rs`, `fstree.rs` | lists, matching, search in files, the tree |
| `remote/` | remote access: lifecycle (`mod`), in-memory password + sessions (`auth`), HTTPS server for all windows, run by the window that turned it on (`hub`), a window's private link and its flow control (`endpoint`), browser input (`input`), off-screen drawing (`render`), what changed as lossless WebP regions, on its own thread (`frames`), waking the window at full rate even hidden (`clock`), the page (`page.html`: its window list is the name's button, or Switch Window's key; `login.html`) |
| `sys.rs` | all `unsafe` and OS calls |
| `headless.rs` | headless UI walkthroughs |
