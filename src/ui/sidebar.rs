//! The file tree beside the editor. ⌘B shows or hides it, ⇧⌘E puts the
//! keyboard in it (↑↓ move, → ← open and close folders, ↵ opens, esc leaves).
//! The tree is the project's, the same one Go to File browses.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;

/// What a click or key in the sidebar asks for, done after drawing.
enum Act {
    /// Open the file; `true` moves the keyboard to it.
    Open(PathBuf, bool),
    Toggle(usize),
    CollapseAll,
    Run(Command),
}

impl App {
    pub(super) fn sidebar_shown(&self) -> bool {
        self.project.is_some() && self.config.settings.sidebar
    }

    pub fn toggle_sidebar(&mut self, ctx: &egui::Context) {
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

    /// ⇧⌘E: into the tree, on the active file (showing the sidebar if
    /// needed); again to go back to the editor.
    pub fn focus_sidebar(&mut self) {
        if self.focus == Focus::Sidebar && self.config.settings.sidebar {
            self.focus = Focus::Editor;
            return;
        }
        self.config.settings.sidebar = true;
        self.focus = Focus::Sidebar;
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
        if let Some(p) = &mut self.project
            && p.tree.reveal(&path).is_some()
        {
            self.sidebar.selected = Some(path);
            self.sidebar.scroll = true;
        }
    }

    fn sidebar_keys(&mut self, ctx: &egui::Context) -> Option<Act> {
        if self.focus != Focus::Sidebar || self.palette.is_some() || self.modal_open() {
            return None;
        }
        let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
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
            // ↵ opens and goes to the file; space opens it and stays here.
            match (at, entry) {
                (Some(i), Some(e)) if e.is_dir => act = Some(Act::Toggle(i)),
                (_, Some(e)) => act = Some(Act::Open(e.path.clone(), enter == Key::Enter)),
                _ => {}
            }
        } else if pressed(Key::Escape) {
            self.focus = Focus::Editor;
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
                        Act::Run(Command::ToggleSidebar),
                    ),
                    (
                        icon::COLLAPSE_ALL,
                        "Collapse Folders".to_string(),
                        Act::CollapseAll,
                    ),
                    (
                        icon::NEW_FOLDER,
                        format!("New Folder…{}", caps(Command::NewFolder)),
                        Act::Run(Command::NewFolder),
                    ),
                    (
                        icon::NEW_FILE,
                        format!("New File…{}", caps(Command::NewFile)),
                        Act::Run(Command::NewFile),
                    ),
                ];
                for (glyph, tip, a) in buttons {
                    if pt.icon_button(ui, glyph, &tip, false) {
                        act = Some(a);
                    }
                }
                let room = (ui.available_width() - 6.0 * s).max(0.0);
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
                if let Some(tip) = tip {
                    ui.interact(name, egui::Id::unique("sidebar-folder"), Sense::hover())
                        .on_hover_text(tip);
                }
            },
        );

        // The rows.
        let row_h = 26.0 * s;
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
            if top < off {
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
                if is_sel && focused {
                    p.rect(
                        bg,
                        6.0 * s,
                        c.accent.gamma_multiply(0.16),
                        Stroke::new(1.0, c.accent.gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                } else if is_sel || is_active {
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

        if let Some(path) = select_click {
            self.sidebar.selected = (!path.as_os_str().is_empty()).then_some(path);
            self.focus = Focus::Sidebar;
        }
        match act {
            Some(Act::Open(path, edit)) => {
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
            Some(Act::CollapseAll) => {
                if let Some(p) = &mut self.project {
                    p.tree.collapse_all();
                }
                self.sidebar.revealed = self.active_tab().and_then(|t| t.path().cloned());
            }
            Some(Act::Run(cmd)) => self.run(cmd, &ctx),
            None => {}
        }
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
                    "Open in Finder"
                } else {
                    "Reveal in Finder"
                },
            );
            item(ui, Command::CopyPath, "Copy Path");
            item(ui, Command::CopyRelativePath, "Copy Relative Path");
            ui.separator();
            item(ui, Command::DeleteFile, "Move to Trash");
        }
        chosen
    }
}
