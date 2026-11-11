//! The file tree beside the editor. ⌘B shows or hides it, ⇧⌘E puts the
//! keyboard in it (↑↓ move, → ← open and close folders, ↵ opens, esc leaves).
//! Its search box is Go to File (⌥F): typing anywhere in the tree lists the
//! files that match in its place.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;

/// What a click or key in the sidebar asks for, done after drawing.
enum Act {
    /// Open the file; `true` moves the keyboard to it.
    Open(PathBuf, bool),
    Toggle(usize),
    Hide,
    Run(Command),
}

impl App {
    pub(crate) fn sidebar_shown(&self) -> bool {
        let view = self.view();
        self.project.is_some()
            && if view.compact {
                view.drawer
            } else {
                self.config.settings.sidebar
            }
    }

    pub fn toggle_sidebar(&mut self, ctx: &egui::Context) {
        self.sidebar.peek = false; // chosen, not a peek
        // A compact view's drawer is its own: the saved setting is the
        // full-size layout's, which a phone must not change.
        if self.view().compact {
            if self.sidebar_shown() {
                self.view_mut().drawer = false;
                if self.focus == Focus::Sidebar {
                    self.focus = Focus::Editor;
                }
            } else {
                self.focus_sidebar();
            }
            return;
        }
        let on = !self.config.settings.sidebar;
        if on {
            // Showing the tree is asking to use it: the keyboard goes there.
            self.focus_sidebar();
        } else {
            self.config.settings.sidebar = false;
            if self.focus == Focus::Sidebar {
                self.focus = Focus::Editor;
            }
        }
        self.save_choice_quietly("sidebar", on, ctx);
    }

    /// ⌘B: hidden → show it and go in; shown with the keyboard elsewhere →
    /// just go in; already in it → hide it.
    pub fn sidebar_key(&mut self, ctx: &egui::Context) {
        if self.sidebar_shown() && self.focus != Focus::Sidebar {
            self.focus_sidebar();
        } else {
            self.toggle_sidebar(ctx);
        }
    }

    /// Fold every folder in the tree, keeping the selection on a row that's
    /// still there.
    pub fn collapse_folders(&mut self) {
        let Some(p) = &mut self.project else { return };
        p.tree.collapse_all();
        self.sidebar.revealed = self.active_tab().and_then(|t| t.path().cloned());
    }

    /// Go to File: into the tree's search box, showing the tree if needed;
    /// again to go back to the editor.
    pub fn search_files(&mut self) {
        let shown = self.sidebar_shown();
        let in_tree = self.focus == Focus::Sidebar && shown;
        if in_tree && self.sidebar.searching {
            self.focus = Focus::Editor;
            return;
        }
        self.sidebar.query.clear();
        if !in_tree {
            self.focus_sidebar();
        }
        self.sidebar.searching = true;
        self.sidebar.from_tree = in_tree;
        self.sidebar.peek |= !shown && self.sidebar_shown();
    }

    /// Esc in the search box: back to the rows if the search began there,
    /// else to the editor.
    fn leave_search(&mut self) {
        self.sidebar.query.clear();
        self.sidebar.searching = false;
        if !self.sidebar.from_tree {
            self.focus = Focus::Editor;
        }
    }

    /// A tree shown only for a search goes away when the keyboard leaves it.
    pub(super) fn end_peek(&mut self) {
        if !self.sidebar.peek
            || self.focus == Focus::Sidebar
            || self.palette.is_some()
            || self.modal_open()
        {
            return;
        }
        self.sidebar.peek = false;
        self.sidebar.query.clear();
        self.sidebar.searching = false;
        if self.view().compact {
            self.view_mut().drawer = false;
        } else {
            self.config.settings.sidebar = false;
        }
    }

    /// ⇧⌘E: into the tree, on the active file (showing the sidebar if
    /// needed); again to go back to the editor.
    pub fn focus_sidebar(&mut self) {
        if self.focus == Focus::Sidebar && self.sidebar_shown() {
            self.focus = Focus::Editor;
            return;
        }
        if self.view().compact {
            self.view_mut().drawer = true;
        } else {
            self.config.settings.sidebar = true;
        }
        self.focus = Focus::Sidebar;
        self.sidebar.searching = false; // walking the rows; typing searches
        self.sidebar.revealed = None; // reveal the active file again
        if self.sidebar.selected.is_none() {
            self.sidebar.selected = self
                .project
                .as_ref()
                .and_then(|p| p.tree.entries.first())
                .map(|e| e.path.clone());
        }
        self.sidebar.scroll = true;
    }

    fn sidebar_row(&self) -> Option<usize> {
        let p = self.project.as_ref()?;
        p.tree.index_of(self.sidebar.selected.as_deref()?)
    }

    fn select_row(&mut self, i: usize) {
        if let Some(e) = self.project.as_ref().and_then(|p| p.tree.entries.get(i)) {
            self.sidebar.selected = Some(e.path.clone());
            self.sidebar.scroll = true;
        }
    }

    /// Switching to a tab shows its file in the tree, once.
    fn follow_active(&mut self) {
        let Some(path) = self.active_tab().and_then(|t| t.path().cloned()) else {
            return;
        };
        if self.sidebar.revealed.as_ref() == Some(&path) {
            return;
        }
        self.sidebar.revealed = Some(path.clone());
        let Some(p) = &mut self.project else { return };
        // The same file by another name (through a symlink, /var and
        // /private/var) is still the one in the tree.
        let found = p.tree.reveal(&path).map(|_| path.clone()).or_else(|| {
            let real = path.canonicalize().ok()?;
            let there = p.root.join(real.strip_prefix(p.root.canonicalize().ok()?).ok()?);
            p.tree.reveal(&there).map(|_| there)
        });
        if let Some(path) = found {
            self.sidebar.selected = Some(path);
            self.sidebar.scroll = true;
            self.sidebar.center = true;
        }
    }

    /// Redo the matches when the query or the file list changed.
    fn update_hits(&mut self) {
        let Some(p) = &self.project else { return };
        let key = (self.sidebar.query.clone(), p.files.len());
        if self.sidebar.hits_for.as_ref() == Some(&key) {
            return;
        }
        let Some(m) = &p.matcher else {
            self.sidebar.hits.clear(); // still indexing: try again next frame
            return;
        };
        let recent = |i: usize| self.mru.iter().position(|r| *r == p.root.join(&p.files[i].0));
        self.sidebar.hits = if key.0.trim().is_empty() {
            Vec::new()
        } else {
            m.rank(&key.0, recent).into_iter().take(crate::app::MAX_RESULTS).collect()
        };
        self.sidebar.hit = 0;
        self.sidebar.scroll = true;
        self.sidebar.hits_for = Some(key);
    }

    /// The chosen match's path.
    pub(crate) fn hit_path(&self, n: usize) -> Option<PathBuf> {
        let p = self.project.as_ref()?;
        Some(p.root.join(&p.files.get(*self.sidebar.hits.get(n)?)?.0))
    }

    fn sidebar_keys(&mut self, ctx: &egui::Context) -> Option<Act> {
        if self.focus != Focus::Sidebar || self.palette.is_some() || self.modal_open() {
            return None;
        }
        let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        // Searching: the arrows choose a match; everything else types.
        // Nothing typed yet, ↓ goes down into the rows.
        if self.sidebar.searching && self.sidebar.query.is_empty() {
            if pressed(Key::ArrowDown) {
                self.sidebar.searching = false;
                self.sidebar.scroll = true;
            } else if pressed(Key::Escape) {
                self.leave_search();
            }
            return None;
        }
        if !self.sidebar.query.is_empty() {
            let n = self.sidebar.hits.len();
            if pressed(Key::ArrowDown) {
                self.sidebar.hit = (self.sidebar.hit + 1).min(n.saturating_sub(1));
                self.sidebar.scroll = true;
            } else if pressed(Key::ArrowUp) {
                self.sidebar.hit = self.sidebar.hit.saturating_sub(1);
                self.sidebar.scroll = true;
            } else if pressed(Key::Enter) {
                return self.hit_path(self.sidebar.hit).map(|p| Act::Open(p, true));
            } else if pressed(Key::Escape) {
                self.leave_search();
            }
            return None;
        }
        let p = self.project.as_ref()?;
        let n = p.tree.entries.len();
        if n == 0 {
            return None;
        }
        let at = self.sidebar_row();
        let entry = at.and_then(|i| p.tree.entries.get(i));
        let mut go = None;
        let mut act = None;
        if pressed(Key::ArrowDown) {
            go = Some(at.map_or(0, |i| (i + 1).min(n - 1)));
        } else if pressed(Key::ArrowUp) {
            go = Some(at.map_or(0, |i| i.saturating_sub(1)));
        } else if pressed(Key::Home) {
            go = Some(0);
        } else if pressed(Key::End) {
            go = Some(n - 1);
        } else if pressed(Key::ArrowRight) {
            match (at, entry) {
                (Some(i), Some(e)) if e.is_dir && !e.expanded => act = Some(Act::Toggle(i)),
                (Some(i), Some(e)) if e.is_dir => go = Some((i + 1).min(n - 1)),
                _ => {}
            }
        } else if pressed(Key::ArrowLeft) {
            match (at, entry) {
                (Some(i), Some(e)) if e.is_dir && e.expanded => act = Some(Act::Toggle(i)),
                (Some(i), _) => go = p.tree.parent(i),
                _ => {}
            }
        } else if let Some(enter) = [Key::Enter, Key::Space].into_iter().find(|k| pressed(*k)) {
            // Space here isn't the start of a search.
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == " ")));
            // ↵ opens and goes to the file; space opens it and stays here.
            match (at, entry) {
                (Some(i), Some(e)) if e.is_dir => act = Some(Act::Toggle(i)),
                (_, Some(e)) => act = Some(Act::Open(e.path.clone(), enter == Key::Enter)),
                _ => {}
            }
        } else if pressed(Key::Escape) {
            self.focus = Focus::Editor;
        } else if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Paste(_)))) {
            // Typing in the rows searches (the box has the keys all along).
            self.sidebar.searching = true;
            self.sidebar.from_tree = true;
        }
        if let Some(i) = go {
            self.select_row(i);
        }
        act
    }

    pub(super) fn sidebar_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let tip = self.folder_tip();
        // ponytail: polls the open folders every 2s; a file watcher if trees get huge.
        if self
            .sidebar
            .read_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.sidebar.read_at = Some(Instant::now());
            if let Some(p) = &mut self.project {
                p.tree.refresh();
            }
        }
        self.follow_active();
        let mut act = self.sidebar_keys(&ctx);

        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let full = ui.max_rect();
        ui.painter().vline(
            full.right() - 0.5,
            full.y_range(),
            Stroke::new(1.0, c.border),
        );
        let focused = self.focus == Focus::Sidebar && self.palette.is_none() && !self.modal_open();
        let Some(project) = &self.project else { return };

        // Header: the project, and its actions.
        let (head, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 38.0 * s), Sense::hover());
        let caps = |cmd: Command| {
            self.config
                .keymap
                .combo_for(cmd)
                .map(|k| format!("  {}", k.caps().concat()))
                .unwrap_or_default()
        };
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(head.shrink2(vec2(8.0 * s, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 2.0 * s;
                let buttons = [
                    (
                        icon::SIDEBAR,
                        format!("Hide File Tree{}", caps(Command::ToggleSidebar)),
                        Act::Hide,
                        Command::ToggleSidebar,
                    ),
                    (
                        icon::COLLAPSE_ALL,
                        format!("Collapse Folders{}", caps(Command::CollapseFolders)),
                        Act::Run(Command::CollapseFolders),
                        Command::CollapseFolders,
                    ),
                    (
                        icon::NEW_FOLDER,
                        format!("New Folder…{}", caps(Command::NewFolder)),
                        Act::Run(Command::NewFolder),
                        Command::NewFolder,
                    ),
                    (
                        icon::NEW_FILE,
                        format!("New File…{}", caps(Command::NewFile)),
                        Act::Run(Command::NewFile),
                        Command::NewFile,
                    ),
                ];
                // With the keyboard here, each button wears its key.
                for (glyph, tip, a, cmd) in buttons {
                    let key = focused.then(|| self.shortcut(cmd)).flatten();
                    if pt.icon_button_key(ui, glyph, &tip, key.as_deref()) {
                        act = Some(a);
                    }
                }
                // The key that brings the keyboard here, after the name.
                let go: Vec<Hint> = self.go_hint(Focus::Sidebar).map(|(caps, _)| (caps, "")).into_iter().collect();
                let go_w = pt.key_hints(ui, None, &go, 10.5, Pos2::ZERO);
                let room = (ui.available_width() - 6.0 * s - go_w - 8.0 * s).max(0.0);
                let mut job = LayoutJob::simple_singleline(
                    project.name.to_uppercase(),
                    pt.font(11.5, Weight::SemiBold),
                    c.text_muted,
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: room,
                    max_rows: 1,
                    break_anywhere: true,
                    overflow_character: Some('…'),
                };
                let g = ui.fonts_mut(|f| f.layout_job(job));
                let at = pos2(head.left() + 16.0 * s, head.center().y - g.size().y / 2.0);
                let name = Rect::from_min_size(at, g.size());
                ui.painter().galley(at, g, c.text_muted);
                let p = ui.painter().clone();
                pt.key_hints(ui, Some(&p), &go, 10.5, pos2(name.right() + 8.0 * s, head.center().y));
                if let Some(tip) = tip {
                    ui.interact(name, egui::Id::unique("sidebar-folder"), Sense::hover())
                        .on_hover_text(tip);
                }
            },
        );

        // The search box: Go to File. It has the keys whenever the tree
        // does, so typing in the rows searches; it looks it (lit, a caret)
        // only while searching.
        let typing = focused && self.sidebar.searching;
        let go_to = caps(Command::GoToFile);
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0 * s), Sense::hover());
        let field = bar.shrink2(vec2(10.0 * s, 3.0 * s));
        ui.painter().rect(
            field,
            7.0 * s,
            c.hover.gamma_multiply(if typing { 1.0 } else { 0.6 }),
            Stroke::new(1.0, if typing { c.accent.gamma_multiply(0.7) } else { c.border }),
            StrokeKind::Inside,
        );
        ui.painter().text(
            pos2(field.left() + 12.0 * s, field.center().y),
            Align2::CENTER_CENTER,
            icon::SEARCH,
            pt.font(12.5, Weight::Regular),
            c.text_subtle,
        );
        let hint = egui::RichText::new(format!("Search files{go_to}"))
            .color(c.text_subtle)
            .font(pt.font(12.5, Weight::Regular));
        let had = !self.sidebar.query.is_empty();
        let edit = egui::TextEdit::singleline(&mut self.sidebar.query)
            .id(egui::Id::unique("sidebar-search"))
            .frame(egui::Frame::NONE)
            .font(pt.font(13.0, Weight::Regular))
            .text_color(c.text)
            .hint_text(hint)
            .align(Align2::LEFT_CENTER)
            .desired_width(field.width() - 30.0 * s);
        let caret = ui.visuals().text_cursor.clone();
        if !typing {
            ui.visuals_mut().text_cursor.stroke.color = Color32::TRANSPARENT;
        }
        let resp = ui.put(field.with_min_x(field.left() + 24.0 * s), edit);
        ui.visuals_mut().text_cursor = caret;
        if resp.clicked() {
            self.sidebar.searching = true;
            self.sidebar.from_tree = false;
        }
        if focused {
            resp.request_focus();
        } else if resp.has_focus() {
            resp.surrender_focus();
        }
        if resp.clicked() || resp.gained_focus() {
            self.focus = Focus::Sidebar;
        }
        // Typed in the rows and erased again: back to walking them.
        if had && self.sidebar.query.is_empty() && self.sidebar.from_tree {
            self.sidebar.searching = false;
        }
        self.sidebar.searching |= !self.sidebar.query.is_empty();
        self.update_hits(); // for what was just typed
        let center = std::mem::take(&mut self.sidebar.center);
        let Some(project) = &self.project else { return };

        // The rows.
        // Rows a finger can hit, in the drawer of a touch-sized view.
        let row_h = if self.view().compact { 34.0 } else { 26.0 } * s;
        if !self.sidebar.query.is_empty() {
            if let Some(a) = self.hit_rows(ui, row_h, focused) {
                act = Some(a);
            }
            return self.sidebar_act(act, None, &ctx);
        }
        let entries = &project.tree.entries;
        let selected = self.sidebar.selected.as_deref();
        let active = self
            .active_tab()
            .and_then(|t| t.path())
            .map(PathBuf::as_path);
        let unsaved: Vec<&Path> = self
            .tabs
            .iter()
            .filter(|t| t.buf.modified)
            .filter_map(|t| t.path().map(PathBuf::as_path))
            .collect();
        let mut area = egui::ScrollArea::vertical()
            .auto_shrink(false)
            .id_salt("sidebar-rows");
        if self.sidebar.scroll
            && let Some(i) = selected.and_then(|p| project.tree.index_of(p))
        {
            let (top, bottom) = (i as f32 * row_h, (i + 1) as f32 * row_h);
            let (off, view) = (self.sidebar.offset, self.sidebar.view);
            let out_of_view = top < off || (bottom > off + view && view > 0.0);
            if out_of_view && center {
                area = area.vertical_scroll_offset((top + row_h / 2.0 - view / 2.0).max(0.0));
            } else if top < off {
                area = area.vertical_scroll_offset((top - row_h).max(0.0));
            } else if bottom > off + view && view > 0.0 {
                area = area.vertical_scroll_offset(bottom - view + row_h);
            }
        }
        let mut select_click = None;
        let out = area.show_rows(ui, row_h, entries.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let e = &entries[i];
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                let p = ui.painter();
                let is_sel = selected == Some(e.path.as_path());
                let is_active = active == Some(e.path.as_path());
                let bg = r.shrink2(vec2(6.0 * s, 1.0 * s));
                if is_sel && focused && !self.sidebar.searching {
                    p.rect(
                        bg,
                        6.0 * s,
                        c.accent.gamma_multiply(0.16),
                        Stroke::new(1.0, c.accent.gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                } else if is_active {
                    // The file in front: marked whatever has the keyboard.
                    p.rect_filled(bg, 6.0 * s, c.accent.gamma_multiply(0.12));
                    p.rect_filled(
                        Rect::from_min_size(bg.left_top(), vec2(2.5 * s, bg.height())).shrink2(vec2(0.0, 3.0 * s)),
                        1.5 * s,
                        c.accent,
                    );
                } else if is_sel {
                    p.rect_filled(bg, 6.0 * s, c.hover);
                } else if resp.hovered() {
                    p.rect_filled(bg, 6.0 * s, c.hover.gamma_multiply(0.6));
                }
                let indent = 14.0 * s;
                let x0 = r.left() + 14.0 * s;
                // Indent guides, one per level.
                for d in 0..e.depth {
                    let x = x0 + d as f32 * indent + 6.0 * s;
                    p.vline(x, r.y_range(), Stroke::new(1.0, c.indent_guide));
                }
                let x = x0 + e.depth as f32 * indent;
                let small = pt.font(12.0, Weight::Regular);
                if e.is_dir {
                    let chevron = if e.expanded {
                        icon::CHEVRON_DOWN
                    } else {
                        icon::CHEVRON_RIGHT
                    };
                    p.text(
                        pos2(x + 6.0 * s, r.center().y),
                        Align2::CENTER_CENTER,
                        chevron,
                        small.clone(),
                        c.text_subtle,
                    );
                }
                let (glyph, tint) = match (e.is_dir, e.expanded) {
                    (true, true) => (icon::FOLDER_OPEN, c.accent),
                    (true, false) => (icon::FOLDER, c.accent.gamma_multiply(0.85)),
                    _ => match icon::for_file(&e.name) {
                        (glyph, Some(tint)) => (
                            glyph,
                            if e.ignored {
                                tint.gamma_multiply(0.5)
                            } else {
                                tint
                            },
                        ),
                        (glyph, None) => (glyph, if is_active { c.accent } else { c.text_subtle }),
                    },
                };
                p.text(
                    pos2(x + 24.0 * s, r.center().y),
                    Align2::CENTER_CENTER,
                    glyph,
                    pt.font(14.0, Weight::Regular),
                    tint,
                );
                let color = if e.ignored {
                    c.text_subtle
                } else if is_active || is_sel {
                    c.text
                } else {
                    c.text_muted
                };
                let weight = if is_active {
                    Weight::Medium
                } else {
                    Weight::Regular
                };
                let lx = x + 36.0 * s;
                let dot = unsaved.contains(&e.path.as_path());
                let room = (r.right() - 14.0 * s - lx - if dot { 16.0 * s } else { 0.0 }).max(0.0);
                let mut job =
                    LayoutJob::simple_singleline(e.name.clone(), pt.font(13.0, weight), color);
                job.wrap = egui::text::TextWrapping {
                    max_width: room,
                    max_rows: 1,
                    break_anywhere: true,
                    overflow_character: Some('…'),
                };
                let g = ui.fonts_mut(|f| f.layout_job(job));
                p.galley(pos2(lx, r.center().y - g.size().y / 2.0), g, color);
                if dot {
                    p.circle_filled(
                        pos2(r.right() - 20.0 * s, r.center().y),
                        3.5 * s,
                        c.text_muted,
                    );
                }

                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    select_click = Some(e.path.clone());
                    act = Some(if e.is_dir {
                        Act::Toggle(i)
                    } else {
                        Act::Open(e.path.clone(), true)
                    });
                }
                if resp.secondary_clicked() {
                    select_click = Some(e.path.clone());
                }
                resp.context_menu(|ui| {
                    if let Some(cmd) = self.sidebar_menu(ui, e.is_dir) {
                        act = Some(Act::Run(cmd));
                    }
                });
            }
            // Right-click below the rows: new things at the top level.
            let rest = ui.available_rect_before_wrap();
            if rest.height() > 0.0 {
                let resp = ui.allocate_rect(rest, Sense::click());
                if resp.secondary_clicked() || resp.clicked() {
                    select_click = Some(PathBuf::new());
                }
                resp.context_menu(|ui| {
                    if let Some(cmd) = self.sidebar_menu(ui, true) {
                        act = Some(Act::Run(cmd));
                    }
                });
            }
        });
        if entries.is_empty() {
            ui.painter().text(
                head.center_bottom() + vec2(0.0, 24.0 * s),
                Align2::CENTER_CENTER,
                "This folder is empty",
                pt.font(12.5, Weight::Regular),
                c.text_subtle,
            );
        }
        self.sidebar.offset = out.state.clamped_offset().y;
        self.sidebar.view = out.inner_rect.height();
        self.sidebar.scroll = false;

        self.sidebar_act(act, select_click, &ctx);
    }

    /// Do what a click or key in the sidebar asked for.
    fn sidebar_act(&mut self, act: Option<Act>, select_click: Option<PathBuf>, ctx: &egui::Context) {
        if let Some(path) = select_click {
            self.sidebar.selected = (!path.as_os_str().is_empty()).then_some(path);
            self.focus = Focus::Sidebar;
            self.sidebar.searching = false;
        }
        match act {
            Some(Act::Open(path, edit)) => {
                self.sidebar.query.clear();
                self.sidebar.searching = false;
                self.open_file(&path);
                if edit {
                    self.focus = Focus::Editor;
                }
            }
            Some(Act::Toggle(i)) => {
                if let Some(p) = &mut self.project {
                    p.tree.toggle(i);
                }
                self.select_row(i);
            }
            Some(Act::Hide) => self.toggle_sidebar(ctx),
            Some(Act::Run(cmd)) => self.run(cmd, ctx),
            None => {}
        }
    }

    /// The matches for the search box, best first, in place of the tree.
    fn hit_rows(&mut self, ui: &mut egui::Ui, row_h: f32, focused: bool) -> Option<Act> {
        let pt = self.paint();
        let (c, s) = (&pt.c, pt.s);
        let Some(p) = &self.project else { return None };
        let query = &self.sidebar.query;
        let mut act = None;
        let mut clicked = None;
        let mut area = egui::ScrollArea::vertical()
            .auto_shrink(false)
            .id_salt("sidebar-hits");
        if self.sidebar.scroll {
            let i = self.sidebar.hit;
            let (top, bottom) = (i as f32 * row_h, (i + 1) as f32 * row_h);
            let (off, view) = (self.sidebar.offset, self.sidebar.view);
            if top < off {
                area = area.vertical_scroll_offset(top);
            } else if bottom > off + view && view > 0.0 {
                area = area.vertical_scroll_offset(bottom - view);
            }
        }
        let out = area.show_rows(ui, row_h, self.sidebar.hits.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for n in range {
                let (rel, ignored) = &p.files[self.sidebar.hits[n]];
                let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                let chosen = n == self.sidebar.hit;
                let bg = r.shrink2(vec2(6.0 * s, 1.0 * s));
                if chosen && focused {
                    ui.painter().rect(
                        bg,
                        6.0 * s,
                        c.accent.gamma_multiply(0.16),
                        Stroke::new(1.0, c.accent.gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                } else if chosen || resp.hovered() {
                    ui.painter().rect_filled(bg, 6.0 * s, c.hover);
                }
                let (glyph, tint) = icon::for_file(name);
                let tint = tint.map(|t| if *ignored { t.gamma_multiply(0.5) } else { t });
                let mut label = pt.label(
                    name,
                    &crate::palette::label_positions(rel, &crate::fuzzy::positions(query, rel)),
                    if *ignored { c.text_subtle } else { c.text },
                );
                for s in &mut label.sections {
                    s.format.font_id.size = pt.font(13.0, Weight::Regular).size;
                }
                // The chosen match says what ↵ does.
                let open: Vec<Hint> = if chosen && focused { vec![(vec!["↵".into()], "open")] } else { Vec::new() };
                pt.row(ui, ui.painter(), bg, 0.0, (glyph, tint), label, dir, &[], &open, -4.0 * s);
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    act = self.hit_path(n).map(|p| Act::Open(p, true));
                }
                if resp.secondary_clicked() {
                    clicked = Some(n);
                }
                resp.context_menu(|ui| {
                    if let Some(cmd) = self.sidebar_menu(ui, false) {
                        act = Some(Act::Run(cmd));
                    }
                });
            }
        });
        if self.sidebar.hits.is_empty() {
            let indexing = p.matcher.is_none();
            let build = self
                .config
                .keymap
                .combo_for(Command::ToggleBuildFolders)
                .filter(|_| !p.build)
                .map(|k| format!("{} to include build folders", k.caps().concat()));
            let msg = if indexing { "Indexing files…".to_string() } else { "No matches".to_string() };
            let at = out.inner_rect.center_top() + vec2(0.0, 24.0 * s);
            ui.painter().text(at, Align2::CENTER_CENTER, msg, pt.font(12.5, Weight::Regular), c.text_subtle);
            if let Some(build) = build.filter(|_| !indexing) {
                ui.painter().text(
                    at + vec2(0.0, 20.0 * s),
                    Align2::CENTER_CENTER,
                    build,
                    pt.font(11.5, Weight::Regular),
                    c.text_subtle,
                );
            }
        }
        self.sidebar.offset = out.state.clamped_offset().y;
        self.sidebar.view = out.inner_rect.height();
        self.sidebar.scroll = false;
        if let Some(n) = clicked {
            self.sidebar.hit = n;
            self.focus = Focus::Sidebar;
        }
        act
    }

    /// The right-click menu: file actions on the row, with their keys.
    fn sidebar_menu(&self, ui: &mut egui::Ui, dir: bool) -> Option<Command> {
        let mut chosen = None;
        let mut item = |ui: &mut egui::Ui, cmd: Command, title: &str| {
            let keys = self
                .config
                .keymap
                .combo_for(cmd)
                .map(|k| k.caps().concat())
                .unwrap_or_default();
            if ui
                .add(egui::Button::new(title).shortcut_text(keys))
                .clicked()
            {
                chosen = Some(cmd);
                ui.close();
            }
        };
        item(ui, Command::NewFile, "New File…");
        item(ui, Command::NewFolder, "New Folder…");
        if self.sidebar.selected.is_some() {
            ui.separator();
            item(ui, Command::RenameFile, "Rename…");
            item(
                ui,
                Command::RevealInFinder,
                if dir {
                    crate::keymap::OPEN_FOLDER_TITLE
                } else {
                    crate::keymap::REVEAL_TITLE
                },
            );
            item(ui, Command::OpenInTerminal, "Open in Terminal");
            item(ui, Command::CopyPath, "Copy Path");
            item(ui, Command::CopyRelativePath, "Copy Relative Path");
            ui.separator();
            item(ui, Command::DeleteFile, "Move to Trash");
        }
        chosen
    }
}
