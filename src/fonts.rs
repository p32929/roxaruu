//! Bundled fonts in two roles: a UI font (three weights) for the chrome, and a
//! code font for code and terminals. Either can also be any `.ttf` / `.otf` on
//! disk. JuliaMono (huge Unicode coverage) and the Nerd symbols font sit at
//! the end of every chain as glyph fallbacks.

use std::sync::OnceLock;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId};

/// A bundled font, deflated in the binary (see `build.rs`): the bytes once
/// first asked for, inflated then.
struct Packed {
    deflated: &'static [u8],
    bytes: OnceLock<&'static [u8]>,
}

impl Packed {
    fn bytes(&self) -> &'static [u8] {
        self.bytes.get_or_init(|| {
            use std::io::Read;
            let mut out = Vec::new();
            let _ = flate2::read::ZlibDecoder::new(self.deflated).read_to_end(&mut out);
            Box::leak(out.into_boxed_slice())
        })
    }

    fn data(&self) -> FontData {
        FontData::from_static(self.bytes())
    }
}

macro_rules! font {
    ($file:literal) => {
        Packed {
            deflated: include_bytes!(concat!(env!("OUT_DIR"), "/", $file, ".z")),
            bytes: OnceLock::new(),
        }
    };
}

struct UiFont {
    name: &'static str,
    regular: Packed,
    medium: Packed,
    semibold: Packed,
}

static UI_FONTS: [UiFont; 2] = [
    UiFont {
        name: "Inter",
        regular: font!("Inter-Regular.ttf"),
        medium: font!("Inter-Medium.ttf"),
        semibold: font!("Inter-SemiBold.ttf"),
    },
    UiFont {
        name: "Geist",
        regular: font!("Geist-Regular.ttf"),
        medium: font!("Geist-Medium.ttf"),
        semibold: font!("Geist-SemiBold.ttf"),
    },
];

static CODE_FONTS: [(&str, Packed); 4] = [
    (
        "JetBrains Mono",
        font!("JetBrainsMono-Regular.ttf"),
    ),
    (
        "Fira Code",
        font!("FiraCode-Regular.ttf"),
    ),
    (
        "IBM Plex Mono",
        font!("IBMPlexMono-Regular.ttf"),
    ),
    (
        "Geist Mono",
        font!("GeistMono-Regular.ttf"),
    ),
];

static FALLBACK: Packed = font!("JuliaMono.ttf");
static SYMBOLS: Packed = font!("SymbolsNerdFontMono.ttf");

/// Inflate the fonts a window starts with, side by side, before it needs them.
pub fn unpack(ui_font: &str, code_font: &str) {
    let ui = UI_FONTS.iter().find(|f| f.name.eq_ignore_ascii_case(ui_font)).unwrap_or(&UI_FONTS[0]);
    let code = CODE_FONTS.iter().find(|f| f.0.eq_ignore_ascii_case(code_font)).map_or(&CODE_FONTS[0].1, |f| &f.1);
    let all = [&ui.regular, &ui.medium, &ui.semibold, &UI_FONTS[0].medium, code, &FALLBACK, &SYMBOLS];
    let started = std::time::Instant::now();
    std::thread::scope(|s| {
        for f in all {
            s.spawn(|| f.bytes());
        }
    });
    tracing::debug!(ms = started.elapsed().as_millis(), "fonts unpacked");
}

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
        Some(f) => [&f.regular, &f.medium, &f.semibold].map(Packed::data),
        None => match read_font_file(ui_font) {
            // One file: the same face serves all three weights.
            Ok(bytes) => [(); 3].map(|_| FontData::from_owned(bytes.clone())),
            Err(e) => {
                errors.push(format!("uiFont: {e} (choose {})", choices(ui_names())));
                let f = &UI_FONTS[0];
                [&f.regular, &f.medium, &f.semibold].map(Packed::data)
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
        Some(f) => f.1.data(),
        None => match read_font_file(code_font) {
            Ok(bytes) => FontData::from_owned(bytes),
            Err(e) => {
                errors.push(format!("codeFont: {e} (choose {})", choices(code_names())));
                CODE_FONTS[0].1.data()
            }
        },
    };
    add("code", code);
    add("keycap", UI_FONTS[0].medium.data());
    add("fallback", FALLBACK.data());
    add("symbols", SYMBOLS.data());

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_fonts_come_back_whole() {
        unpack(DEFAULT_UI, DEFAULT_CODE);
        for f in UI_FONTS.iter().flat_map(|f| [&f.regular, &f.medium, &f.semibold]).chain(CODE_FONTS.iter().map(|f| &f.1)).chain([&FALLBACK, &SYMBOLS]) {
            let b = f.bytes();
            assert!(b.len() > 50_000 && b[..4] == [0, 1, 0, 0], "a TrueType font, whole");
        }
    }
}
