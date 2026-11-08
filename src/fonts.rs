//! Bundled fonts in two roles: a UI font (three weights) for the chrome, and a
//! code font for code and terminals. Either can also be any `.ttf` / `.otf` on
//! disk. JuliaMono (huge Unicode coverage) and the Nerd symbols font sit at
//! the end of every chain as glyph fallbacks.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};

struct UiFont {
    name: &'static str,
    regular: &'static [u8],
    medium: &'static [u8],
    semibold: &'static [u8],
}

const UI_FONTS: &[UiFont] = &[
    UiFont {
        name: "Inter",
        regular: include_bytes!("../assets/fonts/Inter-Regular.ttf"),
        medium: include_bytes!("../assets/fonts/Inter-Medium.ttf"),
        semibold: include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    },
    UiFont {
        name: "Geist",
        regular: include_bytes!("../assets/fonts/Geist-Regular.ttf"),
        medium: include_bytes!("../assets/fonts/Geist-Medium.ttf"),
        semibold: include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    },
];

const CODE_FONTS: &[(&str, &[u8])] = &[
    (
        "JetBrains Mono",
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    ),
    (
        "Fira Code",
        include_bytes!("../assets/fonts/FiraCode-Regular.ttf"),
    ),
    (
        "IBM Plex Mono",
        include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    ),
    (
        "Geist Mono",
        include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    ),
];

const FALLBACK: &[u8] = include_bytes!("../assets/fonts/JuliaMono.ttf");
const SYMBOLS: &[u8] = include_bytes!("../assets/fonts/SymbolsNerdFontMono.ttf");

pub const DEFAULT_UI: &str = "Inter";
pub const DEFAULT_CODE: &str = "JetBrains Mono";

pub fn ui_names() -> impl Iterator<Item = &'static str> {
    UI_FONTS.iter().map(|f| f.name)
}

pub fn code_names() -> impl Iterator<Item = &'static str> {
    CODE_FONTS.iter().map(|f| f.0)
}

#[derive(Clone, Copy)]
pub enum Weight {
    Regular,
    Medium,
    SemiBold,
}

/// A UI-font `FontId` at `size` points.
pub fn ui(size: f32, weight: Weight) -> FontId {
    match weight {
        Weight::Regular => FontId::proportional(size),
        Weight::Medium => FontId::new(size, FontFamily::Name("ui-medium".into())),
        Weight::SemiBold => FontId::new(size, FontFamily::Name("ui-semibold".into())),
    }
}

/// Keycap labels always use Inter: some UI fonts (Geist) draw ⌘ ⌥ ⇧ tiny.
pub fn keycap(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("keycap".into()))
}

/// Install the chosen fonts into egui. Unknown names or unreadable files fall
/// back to the defaults; the returned messages say why.
pub fn install(ctx: &egui::Context, ui_font: &str, code_font: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let mut defs = FontDefinitions::empty();
    let mut add = |name: &str, data: FontData| {
        defs.font_data.insert(name.to_string(), data.into());
    };

    let [regular, medium, semibold] = match UI_FONTS
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(ui_font))
    {
        Some(f) => [f.regular, f.medium, f.semibold].map(FontData::from_static),
        None => match read_font_file(ui_font) {
            // One file: the same face serves all three weights.
            Ok(bytes) => [(); 3].map(|_| FontData::from_owned(bytes.clone())),
            Err(e) => {
                errors.push(format!("uiFont: {e} (choose {})", choices(ui_names())));
                let f = &UI_FONTS[0];
                [f.regular, f.medium, f.semibold].map(FontData::from_static)
            }
        },
    };
    add("ui-regular", regular);
    add("ui-medium", medium);
    add("ui-semibold", semibold);

    let code = match CODE_FONTS
        .iter()
        .find(|f| f.0.eq_ignore_ascii_case(code_font))
    {
        Some(f) => FontData::from_static(f.1),
        None => match read_font_file(code_font) {
            Ok(bytes) => FontData::from_owned(bytes),
            Err(e) => {
                errors.push(format!("codeFont: {e} (choose {})", choices(code_names())));
                FontData::from_static(CODE_FONTS[0].1)
            }
        },
    };
    add("code", code);
    add("keycap", FontData::from_static(UI_FONTS[0].medium));
    add("fallback", FontData::from_static(FALLBACK));
    add("symbols", FontData::from_static(SYMBOLS));

    let chain = |first: &str| {
        vec![
            first.to_string(),
            "fallback".to_string(),
            "symbols".to_string(),
        ]
    };
    defs.families
        .insert(FontFamily::Proportional, chain("ui-regular"));
    defs.families
        .insert(FontFamily::Name("ui-medium".into()), chain("ui-medium"));
    defs.families
        .insert(FontFamily::Name("ui-semibold".into()), chain("ui-semibold"));
    defs.families.insert(FontFamily::Monospace, chain("code"));
    defs.families
        .insert(FontFamily::Name("keycap".into()), chain("keycap"));
    ctx.set_fonts(defs);
    errors
}

/// A font given as a path rather than a bundled name.
fn read_font_file(spec: &str) -> Result<Vec<u8>, String> {
    let lower = spec.to_ascii_lowercase();
    if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) {
        return Err(format!("unknown font \"{spec}\""));
    }
    let path = crate::config::expand_home(spec);
    std::fs::read(&path).map_err(|e| format!("can't read {}: {e}", path.display()))
}

fn choices(names: impl Iterator<Item = &'static str>) -> String {
    let names: Vec<_> = names.map(|n| format!("\"{n}\"")).collect();
    format!("{}, or a .ttf/.otf path", names.join(", "))
}
