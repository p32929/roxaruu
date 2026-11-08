//! Roxaruu — a keyboard-first editor and terminals, on egui.
//!
//! **No `unwrap` / `expect` / `panic!` in shipped code** (Oxru's house rule): a
//! crash in an editor loses unsaved work, so fallible paths degrade instead.

#![deny(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

mod app;
mod buffer;
mod config;
mod editor;
mod find;
mod fonts;
mod fstree;
mod fuzzy;
#[cfg(test)]
mod headless;
mod instances;
mod keymap;
mod palette;
mod recent;
mod search;
mod session;
mod syntax;
mod sys;
mod termbridge;
mod terminal;
mod terminalpane;
mod theme;
mod todo;
mod ui;
mod wrap;

use eframe::egui;

const ICON: &[u8] = include_bytes!("../assets/icon.png");

fn main() -> eframe::Result {
    // `roxaruu [folder | file]`
    let target = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Roxaruu")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([520.0, 420.0]);
    // macOS: our own bar replaces the title bar; the traffic lights float over it.
    if cfg!(target_os = "macos") {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
    }
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON) {
        viewport = viewport.with_icon(icon);
    }
    // Terminals keep streaming while the window is in the background.
    sys::prevent_app_nap();
    let result = eframe::run_native(
        "Roxaruu",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(Root(app::App::new(&cc.egui_ctx, target))))),
    );
    // Remove this window's terminal-bridge shims and its open-folder marker.
    termbridge::cleanup();
    instances::unregister();
    result
}

struct Root(app::App);

impl eframe::App for Root {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.frame(ui);
    }
}
