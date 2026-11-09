//! User configuration, VS Code style, in `~/.config/oxru/`:
//! `settings.json`, `keybindings.json` and `themes/*.json`. JSON5 is accepted,
//! so comments and trailing commas are fine. Mistakes never stop the app:
//! they're collected as messages and the defaults stand in. A project's own
//! `.oxru/settings.json` overrides the global settings, key by key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::fonts;
use crate::keymap::{self, Keymap, UserBinding};
use crate::theme::{self, Theme};

#[derive(Deserialize, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub theme: String,
    pub ui_font: String,
    pub code_font: String,
    pub ui_font_size: f32,
    pub code_font_size: f32,
    /// Redraw rate for terminal output nobody is typing into (typing is
    /// always instant). One of [`TERMINAL_FPS`].
    pub terminal_fps: u32,
    /// Tint the line the caret is on.
    pub current_line: bool,
    /// Faint vertical lines at each indent level.
    pub indent_guides: bool,
    /// Wrap long lines instead of scrolling sideways (⌥Z).
    pub word_wrap: bool,
    /// The file tree beside the editor (⌘B).
    pub sidebar: bool,
    /// Formatter command per file extension, over the built-in ones (see
    /// `format.rs`); `""` turns formatting off for that extension.
    pub formatters: BTreeMap<String, String>,
    /// Run the formatter on save.
    pub format_on_save: bool,
}

/// The offered terminal frame rates (Oxru's).
pub const TERMINAL_FPS: &[u32] = &[1, 5, 10, 20, 30, 60];

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: theme::DEFAULT.into(),
            ui_font: fonts::DEFAULT_UI.into(),
            code_font: fonts::DEFAULT_CODE.into(),
            ui_font_size: 14.0,
            code_font_size: 15.0,
            terminal_fps: 60,
            current_line: true,
            indent_guides: true,
            word_wrap: false,
            sidebar: true,
            formatters: BTreeMap::new(),
            format_on_save: false,
        }
    }
}

/// Everything loaded from the config folder.
pub struct Config {
    pub settings: Settings,
    pub theme: Theme,
    /// Every available theme, for the picker.
    pub themes: Vec<Theme>,
    pub keymap: Keymap,
    /// The user's parsed bindings, kept so a broken edit can fall back to them.
    bindings: Vec<UserBinding>,
    /// Settings keys the open project's `.oxru/settings.json` sets.
    pub project_keys: Vec<String>,
    pub errors: Vec<String>,
}

pub fn dir() -> PathBuf {
    expand_home("~/.config/oxru")
}

pub fn settings_path() -> PathBuf {
    dir().join("settings.json")
}

pub fn keybindings_path() -> PathBuf {
    dir().join("keybindings.json")
}

/// A project's own settings, layered over the global ones.
pub fn project_settings_path(root: &Path) -> PathBuf {
    root.join(".oxru").join("settings.json")
}

fn themes_dir() -> PathBuf {
    dir().join("themes")
}

pub fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

/// Write commented starter files the first time, so there's something to
/// edit. Never overwrites.
pub fn ensure_files() {
    let _ = std::fs::create_dir_all(themes_dir());
    let _ = std::fs::create_dir_all(dir().join("queries"));
    let starters = [
        (settings_path(), settings_template()),
        (keybindings_path(), keymap::template()),
        (themes_dir().join("README.md"), themes_readme()),
        (dir().join("queries").join("README.md"), crate::syntax::queries_readme()),
    ];
    for (path, content) in starters {
        if !path.exists() {
            let _ = std::fs::write(path, content);
        }
    }
}

/// Load everything, with `project`'s settings over the global ones. A file
/// that fails to parse keeps `prev`'s version of it, so a half-typed edit
/// doesn't flash the whole UI back to defaults.
pub fn load(prev: Option<&Config>, project: Option<&Path>) -> Config {
    load_in(&dir(), prev, project)
}

fn load_in(base: &Path, prev: Option<&Config>, project: Option<&Path>) -> Config {
    use serde_json::Value;
    let mut errors = Vec::new();

    // Each file is checked on its own (so errors name the right one), then
    // the project's keys are laid over the global ones.
    let mut checked = |path: &Path, label: String| -> Option<Value> {
        let v: Value = match std::fs::read_to_string(path) {
            Err(_) => return Some(Value::Object(Default::default())),
            Ok(src) => json5::from_str(&src)
                .map_err(|e| errors.push(format!("{label}: {e}")))
                .ok()?,
        };
        serde_json::from_value::<Settings>(v.clone())
            .map_err(|e| errors.push(format!("{label}: {e}")))
            .ok()?;
        Some(v)
    };
    let global = checked(&base.join("settings.json"), "settings.json".into());
    let local = project.and_then(|root| {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        checked(
            &project_settings_path(root),
            format!("{name}/.oxru/settings.json"),
        )
    });
    let project_keys: Vec<String> = local
        .as_ref()
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let mut settings: Settings = global
        .map(|mut g| {
            if let (Some(g), Some(Value::Object(l))) = (g.as_object_mut(), local) {
                g.extend(l);
            }
            serde_json::from_value(g).unwrap_or_default()
        })
        .or_else(|| prev.map(|p| p.settings.clone()))
        .unwrap_or_default();
    settings.ui_font_size = settings.ui_font_size.clamp(9.0, 32.0);
    settings.code_font_size = settings.code_font_size.clamp(9.0, 40.0);
    // Snap a hand-typed rate to the nearest offered one.
    let fps = settings.terminal_fps;
    settings.terminal_fps = TERMINAL_FPS
        .iter()
        .copied()
        .min_by_key(|f| f.abs_diff(fps))
        .unwrap_or(60);

    let bindings: Vec<UserBinding> = read_json(&base.join("keybindings.json"), &mut errors)
        .or_else(|| prev.map(|p| p.bindings.clone()))
        .unwrap_or_default();
    let (keymap, key_errors) = Keymap::new(&bindings);
    errors.extend(
        key_errors
            .into_iter()
            .map(|e| format!("keybindings.json: {e}")),
    );

    let (themes, theme_errors) = theme::load_all(&user_theme_sources(&base.join("themes")));
    errors.extend(theme_errors);
    let find = |name: &str| {
        themes
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .cloned()
    };
    let theme = match find(&settings.theme) {
        Some(t) => t,
        None => {
            let names: Vec<_> = themes.iter().map(|t| format!("\"{}\"", t.name)).collect();
            errors.push(format!(
                "settings.json: no theme \"{}\" (have {})",
                settings.theme,
                names.join(", ")
            ));
            prev.map(|p| p.theme.clone())
                .or_else(|| find(theme::DEFAULT))
                .or_else(|| themes.first().cloned())
                .unwrap_or_else(fallback_theme)
        }
    };

    Config {
        settings,
        theme,
        themes,
        keymap,
        bindings,
        project_keys,
        errors,
    }
}

/// Set one key in `settings.json` — the project's, if it sets that key —
/// keeping the user's comments and layout: replace the key's line if it has
/// one, else add a line after the `{`. If the result doesn't parse to exactly
/// the intended settings (say, several keys on one line), rewrite the file as
/// plain JSON instead.
pub fn save_setting(
    project: Option<&Path>,
    key: &str,
    value: impl Into<serde_json::Value>,
) -> Result<(), String> {
    let local = project.map(project_settings_path).filter(|p| {
        let src = std::fs::read_to_string(p).unwrap_or_default();
        json5::from_str::<serde_json::Value>(&src).is_ok_and(|v| v.get(key).is_some())
    });
    let path = local.unwrap_or_else(settings_path);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|_| "{\n}\n".into());
    let out = set_key(&src, key, &value.into())?;
    std::fs::write(&path, out).map_err(|e| format!("can't write {}: {e}", file_name(&path)))
}

/// Set (`Some`) or reset to its default (`None`) a command's key in
/// `keybindings.json`, keeping comments: its active lines are dropped and the
/// new one goes right after the `[`. Rewritten as plain JSON if that edit
/// wouldn't give exactly the intended list.
pub fn save_binding(command: &str, key: Option<&str>) -> Result<(), String> {
    let path = keybindings_path();
    let src = std::fs::read_to_string(&path).unwrap_or_else(|_| "[\n]\n".into());
    let out = set_binding(&src, command, key)?;
    std::fs::write(&path, out).map_err(|e| format!("can't write keybindings.json: {e}"))
}

fn set_binding(src: &str, command: &str, key: Option<&str>) -> Result<String, String> {
    use serde_json::{Value, json};
    let mut want: Vec<Value> =
        json5::from_str(src).map_err(|e| format!("fix keybindings.json first ({e})"))?;
    want.retain(|b| b.get("command").and_then(Value::as_str) != Some(command));
    let entry = key.map(|k| json!({ "key": k, "command": command }));
    if let Some(e) = &entry {
        want.insert(0, e.clone());
    }
    let id = format!("\"{command}\"");
    let mut lines: Vec<String> = src
        .lines()
        .filter(|l| l.trim_start().starts_with("//") || !l.contains(&id))
        .map(String::from)
        .collect();
    if let Some(e) = &entry {
        let at = lines
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map_or(0, |i| i + 1);
        lines.insert(
            at,
            format!("  {{ \"key\": {}, \"command\": {id} }},", e["key"]),
        );
    }
    let edited = lines.join("\n") + "\n";
    match json5::from_str::<Vec<Value>>(&edited) {
        Ok(v) if v == want => Ok(edited),
        _ => serde_json::to_string_pretty(&want)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string()),
    }
}

fn set_key(src: &str, key: &str, value: &serde_json::Value) -> Result<String, String> {
    use serde_json::Value;
    let mut want: Value =
        json5::from_str(src).map_err(|e| format!("fix settings.json first ({e})"))?;
    want.as_object_mut()
        .ok_or("settings.json isn't a JSON object")?
        .insert(key.into(), value.clone());

    let entry = format!("\"{key}\": {value},");
    let quoted = format!("\"{key}\"");
    let is_key_line = |l: &str| {
        let t = l.trim_start();
        [quoted.as_str(), key].iter().any(|k| {
            t.strip_prefix(k)
                .is_some_and(|r| r.trim_start().starts_with(':'))
        })
    };
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    if let Some(i) = lines.iter().position(|l| is_key_line(l)) {
        let indent = lines[i].len() - lines[i].trim_start().len();
        lines[i] = format!("{}{entry}", &lines[i][..indent]);
    } else if let Some(i) = lines.iter().position(|l| l.trim_start().starts_with('{')) {
        lines.insert(i + 1, format!("  {entry}"));
    }
    let edited = lines.join("\n") + "\n";
    match json5::from_str::<Value>(&edited) {
        Ok(v) if v == want => Ok(edited),
        _ => serde_json::to_string_pretty(&want)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string()),
    }
}

/// Only reachable if every built-in theme failed to parse, which the tests rule
/// out; exists so that path degrades instead of panicking.
fn fallback_theme() -> Theme {
    use eframe::egui::Color32;
    let colors = theme::Colors {
        background: Color32::from_gray(20),
        text: Color32::from_gray(230),
        ..Default::default()
    };
    Theme {
        name: "Fallback".into(),
        dark: true,
        colors,
    }
}

/// Read and parse a JSON5 file. A missing file means defaults; a broken one
/// is reported and gives `None`.
fn read_json<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
    errors: &mut Vec<String>,
) -> Option<T> {
    let Ok(src) = std::fs::read_to_string(path) else {
        return Some(T::default());
    };
    json5::from_str(&src)
        .map_err(|e| errors.push(format!("{}: {e}", file_name(path))))
        .ok()
}

fn user_theme_sources(dir: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = theme_files(dir)
        .into_iter()
        .filter_map(|p| {
            Some((
                format!("themes/{}", file_name(&p)),
                std::fs::read_to_string(&p).ok()?,
            ))
        })
        .collect();
    out.sort();
    out
}

fn theme_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect()
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Modification times of every config file; when this changes, reload.
pub fn fingerprint(project: Option<&Path>) -> Vec<(PathBuf, Option<SystemTime>)> {
    let mut paths = vec![settings_path(), keybindings_path()];
    paths.extend(project.map(project_settings_path));
    paths.extend(theme_files(&themes_dir()));
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            (p, t)
        })
        .collect()
}

pub(crate) fn settings_template() -> String {
    let quote = |names: Vec<String>| {
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (themes, _) = theme::load_all(&[]);
    let d = Settings::default();
    format!(
        "// Oxru settings. Saved changes apply instantly.\n\
         // Uncomment a line to change it.\n\
         {{\n  \
           // {}, or the \"name\" of any theme in themes/\n  \
           // \"theme\": \"{}\",\n\n  \
           // UI font: {}, or a path to a .ttf/.otf\n  \
           // \"uiFont\": \"{}\",\n\n  \
           // Code font: {}, or a path to a .ttf/.otf\n  \
           // \"codeFont\": \"{}\",\n\n  \
           // \"uiFontSize\": {},\n  \
           // \"codeFontSize\": {},\n\n  \
           // Redraws per second for terminal output you aren't typing into: {}\n  \
           // \"terminalFps\": {},\n\n  \
           // \"currentLine\": {},\n  \
           // \"indentGuides\": {},\n  \
           // \"wordWrap\": {},\n  \
           // \"sidebar\": {},\n\n  \
           // Format Document (⇧⌥F) runs rustfmt, ruff, biome, taplo, gofmt, clang-format,\n  \
           // shfmt or prettier by extension. Use your own (stdin in, stdout out; {{file}}\n  \
           // is the file's path), or \"\" to turn one off:\n  \
           // \"formatters\": {{ \"rs\": \"rustfmt --edition 2021\", \"md\": \"\" }},\n  \
           // \"formatOnSave\": {},\n\
         }}\n",
        quote(themes.into_iter().map(|t| t.name).collect()),
        d.theme,
        quote(fonts::ui_names().map(String::from).collect()),
        d.ui_font,
        quote(fonts::code_names().map(String::from).collect()),
        d.code_font,
        d.ui_font_size,
        d.code_font_size,
        TERMINAL_FPS
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        d.terminal_fps,
        d.current_line,
        d.indent_guides,
        d.word_wrap,
        d.sidebar,
        d.format_on_save,
    )
}

fn themes_readme() -> String {
    format!(
        "# Oxru themes\n\n\
         Drop a `.json` file here and select it by its `\"name\"` in `settings.json`.\n\
         List only the colours you want to change. Everything else comes from Oxide\n\
         (or Oxide Light when `\"type\": \"light\"`). Colours are `#rrggbb` or `#rrggbbaa`.\n\n\
         The default theme, as a starting point:\n\n```json\n{}```\n",
        include_str!("../assets/themes/oxide.json")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_template_parses_to_defaults() {
        let s: Settings = json5::from_str(&settings_template()).unwrap();
        assert!(s == Settings::default());
    }

    #[test]
    fn set_key_keeps_comments_and_replaces_or_inserts() {
        let src = settings_template();
        let out = set_key(&src, "theme", &"Moss".into()).unwrap();
        assert!(out.contains("// UI font"), "comments survive");
        assert!(out.contains("\"theme\": \"Moss\","));
        let s: Settings = json5::from_str(&out).unwrap();
        assert_eq!(s.theme, "Moss");

        // Setting it again replaces the line rather than adding another.
        let out = set_key(&out, "theme", &"Midnight".into()).unwrap();
        assert_eq!(
            out.lines()
                .filter(|l| l.trim_start().starts_with("\"theme\""))
                .count(),
            1
        );
        let s: Settings = json5::from_str(&out).unwrap();
        assert_eq!(s.theme, "Midnight");
    }

    #[test]
    fn set_key_unquoted_and_compact_files() {
        let out = set_key(
            "{\n  theme: \"Moss\", // mine\n}\n",
            "theme",
            &"Oxide".into(),
        )
        .unwrap();
        assert_eq!(json5::from_str::<Settings>(&out).unwrap().theme, "Oxide");
        // Several keys on one line: the line edit can't be trusted, so rewrite.
        let out = set_key(
            "{ theme: \"Moss\", uiFont: \"Geist\" }",
            "theme",
            &"Oxide".into(),
        )
        .unwrap();
        let s: Settings = json5::from_str(&out).unwrap();
        assert_eq!((s.theme.as_str(), s.ui_font.as_str()), ("Oxide", "Geist"));
    }

    #[test]
    fn set_binding_keeps_comments_replaces_and_resets() {
        let src = keymap::template();
        let out = set_binding(&src, "file.goTo", Some("ctrl+f9")).unwrap();
        assert!(out.contains("// Defaults"), "comments survive");
        let parsed: Vec<UserBinding> = json5::from_str(&out).unwrap();
        assert_eq!(parsed.len(), 1);
        let out = set_binding(&out, "file.goTo", Some("alt+f9")).unwrap();
        assert_eq!(out.matches("alt+f9").count(), 1);
        assert!(!out.contains("ctrl+f9"), "the old key is replaced");
        let out = set_binding(&out, "file.goTo", None).unwrap();
        assert!(
            json5::from_str::<Vec<UserBinding>>(&out)
                .unwrap()
                .is_empty(),
            "reset to default"
        );
        assert!(out.contains("// Defaults"));
    }

    #[test]
    fn project_settings_override_global_ones() {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".oxru")).unwrap();
        std::fs::write(
            project_settings_path(root.path()),
            "{ wordWrap: true, codeFontSize: 18 }",
        )
        .unwrap();
        let c = load_in(home.path(), None, Some(root.path()));
        assert!(c.settings.word_wrap && c.settings.code_font_size == 18.0);
        assert_eq!(c.project_keys.len(), 2);
        assert_eq!(c.settings.theme, theme::DEFAULT, "the rest stays global");
        std::fs::write(project_settings_path(root.path()), "{ wordWrp: true }").unwrap();
        let c = load_in(home.path(), None, Some(root.path()));
        assert!(
            c.errors.iter().any(|e| e.contains(".oxru/settings.json")),
            "{:?}",
            c.errors
        );
        assert!(!c.settings.word_wrap);
    }

    #[test]
    fn set_key_refuses_to_overwrite_a_broken_file() {
        assert!(set_key("{ theme: ", "theme", &"Moss".into()).is_err());
    }

    #[test]
    fn settings_reject_typos_and_keep_partial_values() {
        assert!(json5::from_str::<Settings>(r#"{ "thme": "Moss" }"#).is_err());
        let s: Settings = json5::from_str(r#"{ theme: "Moss", }"#).unwrap();
        assert_eq!(s.theme, "Moss");
        assert_eq!(s.ui_font, fonts::DEFAULT_UI);
    }
}
