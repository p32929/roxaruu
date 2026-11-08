# Roxaruu (was reoxru) — rebuilding Oxru in egui

Source of truth for behaviour: `../oxru` (README.md, FEATURES.md).

## Ground rules

- One step at a time. A step is done only when every **Check** box is ticked.
- Next step starts only when Fayaz says **"next"**. Until then, we fix the current step.
- Every step: Claude builds + clippy + tests + a **headless** walkthrough (off-screen render, simulated keys, isolated HOME — never a real window); Fayaz does the "try this" checks.
- Headless run: `CARGO_HOME=~/.cargo RUSTUP_HOME=~/.rustup HOME=/tmp/rx-home SHOTS=/tmp/rx-shots PROJECT=<file in a repo> cargo test --release headless -- --ignored --test-threads=1`
- Windowed only (no `--term`). Same default shortcuts as Oxru.
- Reuse Oxru's non-UI modules (buffer, syntax, search, fstree, termbridge, …); rewrite only the UI.

## Design goals

- **Looks amazing out of the box.** Fresh design, not a copy of Oxru's terminal-grid look.
- **Two font roles:** a UI font (Inter, Geist) and a code font (JetBrains Mono, Fira Code, IBM Plex Mono, Geist Mono), or any `.ttf`/`.otf` by path.
- **Themes** ship built in (Oxide, Oxide Light, Midnight, Moss), and users add their own as JSON.
- **Keyboard first:** every action is a named command with a shortcut, and every shortcut can be remapped.
- **VS Code-style config** in `~/.config/oxru/`: `settings.json`, `keybindings.json`, `themes/*.json` (comments and trailing commas allowed). Saved edits apply instantly, and mistakes show in the status bar instead of breaking the app.

---

## Step 1 — The app opens
**Result:** `cargo run` opens a polished window: a seamless title bar, a welcome screen you can drive by keyboard or mouse, and a status bar with live RAM and FPS. Themes, fonts and shortcuts come from JSON files and change live.

- [ ] Looks great: seamless title bar, welcome card, keycap-style shortcuts
- [ ] Welcome actions work by mouse (hover/click) and keyboard (↑/↓/Enter)
- [ ] Status bar shows RAM and FPS
- [ ] `settings.json`: switching theme / UI font / code font / sizes applies on save
- [ ] `keybindings.json`: remapping a shortcut applies on save, and the welcome keycaps update
- [ ] Custom theme in `themes/` shows up and can be selected
- [ ] A broken JSON edit shows an error in the status bar, and the app keeps running
- [ ] Starts in under a second, idle CPU ≈ 0%
- [ ] `Ctrl+Q` / `⌘Q` quits

## Step 2 — Open a project and read code
**Result:** open a folder (CLI arg, `Ctrl+O` picker, recent folders), find any file with `Alt+F`, and read it highlighted with line numbers.

- [ ] Command palette (`Ctrl+Shift+P`): every command, with its shortcut, searchable
- [ ] Theme and font pickers in the palette, with live preview, saved to `settings.json`
- [ ] Open folder via CLI path, `Ctrl+O`, and `Alt+O` recent list
- [ ] `Alt+F` fuzzy finder ranks like Oxru
- [ ] Build dirs (`node_modules`, `target`…) left out of search; `.gitignore`d files shown faded (like Oxru/VS Code)
- [ ] Syntax highlighting + line numbers; keyboard scrolling (arrows, PageUp/Down, ⌘↑↓)
- [ ] Large repo opens fast; 10k-line file scrolls smoothly

## Step 3 — A basic editor you could use every day
**Result:** type, select, undo/redo, copy/paste, save, tabs, unsaved dot, close prompts, tabs restored on reopen.

- [ ] Typing, keyboard + mouse selection
- [ ] Undo / redo (typing grouped)
- [ ] System clipboard copy / cut / paste
- [ ] Save / save all, unsaved dot, revert-to-saved clears it
- [ ] Tabs: open, switch, move, close, close all, reopen closed
- [ ] Unsaved-close prompt
- [ ] Session restore per project
- [ ] Quit / close folder with unsaved work asks first (Save / Don't Save / Cancel)
- [ ] An afternoon of real editing with no lost work

## Step 4 — One terminal that just works
**Result:** `Alt+T` opens a shell in-app; colours, vim, htop, less and mouse-aware apps all behave.

- [ ] Shell runs, typing feels instant
- [ ] Readable ANSI colours (VSCode palette)
- [ ] vim / htop / less work (alt screen, mouse modes)
- [ ] Scrollback, selection, copy, copy mode
- [ ] `npm run dev` runs fine
- [ ] Ctrl chords reach the shell (^C, ^R); ⌘C/⌘V copy and paste; `exit` closes the panel

## Step 5 — All of a project's terminals in one place
**Result:** many terminals as tabs or grid, named `folder · command`, quick-switch, FPS throttle.

- [ ] New / close / move terminals
- [ ] Tabs ↔ grid (`Alt+G`), click tile to focus
- [ ] `folder · command` names
- [ ] `Alt+K` quick switch, `Alt+1`–`9`
- [ ] 5 servers in grid with low CPU while logs scroll (`terminalFps` in settings.json; typing always instant)
- [ ] `⌥M` maximize, × / middle-click closes a terminal, running / finished markers on tabs

## Step 6 — `run.sh` opens tabs, not windows (the core feature)
**Result:** scripts that spawn OS terminal windows get tabs inside roxaruu instead.

- [ ] AppleScript `do script`, `open -a Terminal`, iTerm etc. intercepted
- [ ] Your real multi-window `run.sh` → N tabs, zero Terminal.app windows
- [ ] Tabs named for the folder each command `cd`s into; `open -a Terminal .` opens in the script's folder

## Step 7 — The editor features you use often
**Result:** find/replace, multi-cursor, word wrap, split view, project search, indent guides, disk-change sync.

- [ ] Find / replace (`Ctrl+F`, `Ctrl+R`)
- [ ] Multi-cursor (`Ctrl+D`, `Ctrl+Alt+↑/↓`)
- [ ] Word wrap (`Alt+Z`, saved to settings), split view (`Ctrl+\` tiles all open files)
- [ ] Project search (`Ctrl+Shift+F`)
- [ ] Current-line highlight + indent guides
- [ ] File edited from terminal updates the tab (conflict prompt if unsaved); deleted file keeps its text, marked unsaved

## Step 8 — Working with files
**Result:** picker create / rename / delete, reveal in Finder, Finder drag & paste, Ctrl+click links.

- [x] New file / folder (⌘N / ⇧⌘N, nested paths), rename (F2, tabs follow), Move to Trash (⌘⌫ in Go to File), Reveal in Finder (⌥⌘R), Copy Path / Relative Path (⌥⌘C / ⌥⇧⌘C), build folders in search (⌥H) — all remappable
- [x] Drop on terminal → path typed (the terminal under the pointer); drop on editor → opens tab (a folder opens as a project); overlay shows where it lands
- [x] Paste a Finder file → path (shell-quoted in a terminal, plain in the editor)
- [x] Ctrl/⌘+click link → confirm → browser (http/https only); ⌘-hover underlines it

## Step 9 — The extras
**Result:** to-do list, clipboard history, settings and shortcut screens, F1 help, toasts, per-project settings, multi-window awareness.

- [x] `Alt+D` to-do list over Oxru's `todos.md` (shared with Oxru) (add, tick, paste a list, delete, clear finished, ⌘E edit as text)
- [x] `Alt+V` clipboard history (memory only; ⇥ ticks several for one paste)
- [x] `Ctrl+,` settings screen (theme, fonts, sizes, FPS, wrap, current line, guides): ← → live, saved to `settings.json`
- [x] Keyboard-shortcuts screen: see and rebind any command (↵ then press keys), ⌘⌫ resets, clashes flagged
- [x] F1 context help (keys for where you are first, then everything), toasts
- [x] Per-project `.oxru/settings.json` overrides the global one (changes to a key it sets are saved there)
- [x] Folders open in other windows detected (marked in Recent, not opened twice)

## Step 10 — Ready to replace Oxru
**Result:** roxaruu becomes the daily driver.

- [ ] Side-by-side vs Oxru: RAM, idle CPU, startup time. Headless: first frame in ~90 ms; idle wakes 1×/s in the background, ~3×/s focused (caret), ~0.15% CPU for the app's own work; RAM readout no longer forks `ps`. Oxru idles at ~7% CPU, ~37 MB. **Still to do: the same numbers from a real window** (status bar shows MB)
- [x] Every F1 shortcut works: all 47 reach their command from the editor, 46 of them from a terminal too (`headless_every_shortcut`)
- [ ] Backgrounded 10 min, terminals don't freeze (App Nap): activity held for the process's life + `NSAppSleepDisabled`; terminal output is parsed off the UI thread. **Still to do: a real 10-minute backgrounded run**
- [x] Install script / release build: `./install-local.sh` → `roxaruu` command + `Roxaruu.app` (icon, signed, never touches Oxru); stripped release build

## After the plan
- [x] Sidebar file tree when a project is open (⌘B show/hide, remembered; ⇧⌘E into it with ↑↓←→ ↵ space esc; click to open; right-click for file actions; follows the active file; picks up outside changes; drag to resize)
