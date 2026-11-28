//! Colour themes, as JSON. Built-ins ship in `assets/themes/`; users add their
//! own in `~/.config/roxaruu/themes/*.json`. A theme only has to list the colours
//! it changes — anything missing falls back to Oxide (dark) or Oxide Light,
//! by the theme's `"type"`.

use std::collections::BTreeMap;

use eframe::egui::Color32;
use serde::Deserialize;

/// Declares the colour slots once: the struct fields and their JSON keys.
macro_rules! colors {
    ($($field:ident = $key:literal),* $(,)?) => {
        #[derive(Clone, Default)]
        pub struct Colors { $(pub $field: Color32,)* }

        impl Colors {
            pub const KEYS: &[&str] = &[$($key),*];

            fn slot(&mut self, key: &str) -> Option<&mut Color32> {
                match key {
                    $($key => Some(&mut self.$field),)*
                    _ => None,
                }
            }

            /// Every slot with its JSON key, in `KEYS`' order.
            fn all(&self) -> [(&'static str, Color32); Self::KEYS.len()] {
                [$(($key, self.$field)),*]
            }
        }
    };
}

colors! {
    background = "background",
    surface = "surface",
    elevated = "elevated",
    border = "border",
    text = "text",
    text_muted = "textMuted",
    text_subtle = "textSubtle",
    accent = "accent",
    accent_text = "accentText",
    hover = "hover",
    error = "error",
    line_highlight = "lineHighlight",
    indent_guide = "indentGuide",
    token_comment = "tokenComment",
    token_keyword = "tokenKeyword",
    token_string = "tokenString",
    token_function = "tokenFunction",
    token_type = "tokenType",
    token_constant = "tokenConstant",
    token_property = "tokenProperty",
    token_attribute = "tokenAttribute",
    token_punctuation = "tokenPunctuation",
    token_variable = "tokenVariable",
    token_number = "tokenNumber",
    token_operator = "tokenOperator",
    token_tag = "tokenTag",
    token_heading = "tokenHeading",
    ansi_0 = "ansiBlack",
    ansi_1 = "ansiRed",
    ansi_2 = "ansiGreen",
    ansi_3 = "ansiYellow",
    ansi_4 = "ansiBlue",
    ansi_5 = "ansiMagenta",
    ansi_6 = "ansiCyan",
    ansi_7 = "ansiWhite",
    ansi_8 = "ansiBrightBlack",
    ansi_9 = "ansiBrightRed",
    ansi_10 = "ansiBrightGreen",
    ansi_11 = "ansiBrightYellow",
    ansi_12 = "ansiBrightBlue",
    ansi_13 = "ansiBrightMagenta",
    ansi_14 = "ansiBrightCyan",
    ansi_15 = "ansiBrightWhite",
}

impl Colors {
    pub fn token(&self, t: crate::syntax::Token) -> Color32 {
        use crate::syntax::Token;
        match t {
            Token::Comment => self.token_comment,
            Token::Keyword => self.token_keyword,
            Token::String => self.token_string,
            Token::Function => self.token_function,
            Token::Type => self.token_type,
            Token::Constant => self.token_constant,
            Token::Property => self.token_property,
            Token::Attribute => self.token_attribute,
            Token::Punctuation => self.token_punctuation,
            Token::Variable => self.token_variable,
            Token::Number => self.token_number,
            Token::Operator => self.token_operator,
            Token::Tag => self.token_tag,
            Token::Heading => self.token_heading,
        }
    }
}

impl Colors {
    /// One of the 16 terminal colours (0-7 normal, 8-15 bright).
    pub fn ansi(&self, i: u8) -> Color32 {
        let all = [
            self.ansi_0,
            self.ansi_1,
            self.ansi_2,
            self.ansi_3,
            self.ansi_4,
            self.ansi_5,
            self.ansi_6,
            self.ansi_7,
            self.ansi_8,
            self.ansi_9,
            self.ansi_10,
            self.ansi_11,
            self.ansi_12,
            self.ansi_13,
            self.ansi_14,
            self.ansi_15,
        ];
        all.get(usize::from(i & 15)).copied().unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    pub colors: Colors,
}

#[derive(Deserialize, Clone, Copy, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    #[default]
    Dark,
    Light,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    #[serde(rename = "type", default)]
    kind: Kind,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

const BUILTIN: &[&str] = &[
    include_str!("../assets/themes/oxide.json"),
    include_str!("../assets/themes/oxide-light.json"),
    include_str!("../assets/themes/midnight.json"),
    include_str!("../assets/themes/moss.json"),
];

pub const DEFAULT: &str = "Oxide";

impl Theme {
    /// Parse a theme. With no `base`, every colour must be present (that's how
    /// the two complete fallback themes are checked).
    fn parse(src: &str, base: Option<&Theme>) -> Result<Theme, String> {
        let file: ThemeFile = json5::from_str(src).map_err(|e| e.to_string())?;
        let mut colors = base.map(|b| b.colors.clone()).unwrap_or_default();
        for (key, hex) in &file.colors {
            let slot = colors
                .slot(key)
                .ok_or_else(|| format!("unknown colour \"{key}\""))?;
            *slot = parse_hex(hex).ok_or_else(|| {
                format!("\"{key}\": bad colour \"{hex}\" (use #rrggbb or #rrggbbaa)")
            })?;
        }
        if base.is_none()
            && let Some(missing) = Colors::KEYS.iter().find(|k| !file.colors.contains_key(**k))
        {
            return Err(format!("missing colour \"{missing}\""));
        }
        Ok(Theme {
            name: file.name,
            dark: file.kind == Kind::Dark,
            colors,
        })
    }
}

impl Theme {
    /// This theme as a complete theme file (every colour spelled out),
    /// named `name`: a user's own theme starts from it.
    pub fn to_file(&self, name: &str) -> String {
        let hex = |c: Color32| {
            let [r, g, b, a] = c.to_srgba_unmultiplied();
            if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
        };
        let colors: Vec<String> = self.colors.all().iter().map(|(k, c)| format!("    \"{k}\": \"{}\"", hex(*c))).collect();
        format!(
            "{{\n  \"name\": {},\n  \"type\": \"{}\",\n  \"colors\": {{\n{}\n  }}\n}}\n",
            serde_json::Value::from(name),
            if self.dark { "dark" } else { "light" },
            colors.join(",\n")
        )
    }
}

/// How far apart two colours' brightness is, as WCAG measures it: 1 (the
/// same) to 21 (black on white). Body text wants 4.5.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let ch = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `fg`, moved toward black (on a light `bg`) or white (on a dark one) just
/// far enough to reach `ratio` against `bg`: a terminal program's colours
/// made for the other kind of background stay readable (VS Code's and
/// iTerm2's "minimum contrast"). Readable already: unchanged.
pub fn readable(fg: Color32, bg: Color32, ratio: f32) -> Color32 {
    if contrast(fg, bg) >= ratio {
        return fg;
    }
    let solid = Color32::from_rgb(fg.r(), fg.g(), fg.b());
    let to = if contrast(bg, Color32::BLACK) > contrast(bg, Color32::WHITE) { Color32::BLACK } else { Color32::WHITE };
    // The least mix that's enough (contrast grows along the way).
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..8 {
        let mid = (lo + hi) / 2.0;
        if contrast(mix(solid, to, mid), bg) >= ratio { hi = mid } else { lo = mid }
    }
    let c = mix(solid, to, hi);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), fg.a())
}

/// Every available theme — built-ins, then the user's — plus any errors from
/// user theme files. A user theme with a built-in's name replaces it.
pub fn load_all(user_sources: &[(String, String)]) -> (Vec<Theme>, Vec<String>) {
    let mut errors = Vec::new();
    // The two complete themes; the built-in files are tested, so failing here
    // is impossible in practice, but degrade to an empty list rather than panic.
    let dark = BUILTIN.first().and_then(|s| Theme::parse(s, None).ok());
    let light = BUILTIN.get(1).and_then(|s| Theme::parse(s, None).ok());
    let base_for = |src: &str| -> Option<&Theme> {
        let light_kind = json5::from_str::<ThemeFile>(src).is_ok_and(|f| f.kind == Kind::Light);
        if light_kind {
            light.as_ref()
        } else {
            dark.as_ref()
        }
    };

    let mut themes: Vec<Theme> = Vec::new();
    let builtin = BUILTIN
        .iter()
        .map(|s| ("built-in".to_string(), s.to_string()));
    for (origin, src) in builtin.chain(user_sources.iter().cloned()) {
        match Theme::parse(&src, base_for(&src)) {
            Ok(t) => {
                themes.retain(|old| !old.name.eq_ignore_ascii_case(&t.name));
                themes.push(t);
            }
            Err(e) => errors.push(format!("{origin}: {e}")),
        }
    }
    (themes, errors)
}

/// `#rrggbb` or `#rrggbbaa`.
fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(
            byte(0)?,
            byte(2)?,
            byte(4)?,
            byte(6)?,
        )),
        _ => None,
    }
}

/// Mix `a` toward `b` by `t` in `[0, 1]`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Light grey on a light background darkens until readable; text that
    /// reads already is left alone; on a dark background it lightens.
    #[test]
    fn terminal_colours_stay_readable() {
        let light = Color32::from_rgb(0xfa, 0xf9, 0xf7);
        let grey = Color32::from_rgb(0xdd, 0xdd, 0xdd);
        assert!(contrast(grey, light) < 2.0);
        let fixed = readable(grey, light, 4.5);
        assert!(contrast(fixed, light) >= 4.5, "{fixed:?}");
        assert!(fixed.r() < grey.r(), "darker on light");
        assert!(contrast(fixed, light) < 6.0, "only as far as needed");
        let black = Color32::from_rgb(0x1c, 0x1b, 0x19);
        assert_eq!(readable(black, light, 4.5), black, "readable already");
        let dark = Color32::from_rgb(0x16, 0x16, 0x18);
        let navy = Color32::from_rgb(0x20, 0x20, 0x60);
        let up = readable(navy, dark, 4.5);
        assert!(contrast(up, dark) >= 4.5 && up.r() > navy.r(), "lighter on dark");
    }

    /// A theme written out (New Theme) reads back as the same theme.
    #[test]
    fn a_theme_round_trips_through_its_file() {
        let (themes, _) = load_all(&[]);
        for t in &themes {
            let back = Theme::parse(&t.to_file("Mine"), None).unwrap();
            assert_eq!(back.name, "Mine");
            assert_eq!(back.dark, t.dark);
            assert_eq!(back.colors.all(), t.colors.all(), "{}", t.name);
        }
    }

    #[test]
    fn builtins_all_load_and_fallbacks_are_complete() {
        let (themes, errors) = load_all(&[]);
        assert!(errors.is_empty(), "{errors:?}");
        let names: Vec<_> = themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Oxide", "Oxide Light", "Midnight", "Moss"]);
        assert!(Theme::parse(BUILTIN[0], None).is_ok());
        assert!(Theme::parse(BUILTIN[1], None).is_ok());
    }

    /// A syntax colour added to `colors!` must be set in every built-in
    /// theme, not quietly borrowed from Oxide's palette.
    #[test]
    fn builtins_set_every_syntax_colour() {
        for src in BUILTIN {
            let v: serde_json::Value = json5::from_str(src).unwrap();
            let colors = v["colors"].as_object().unwrap();
            for key in Colors::KEYS.iter().filter(|k| k.starts_with("token")) {
                assert!(colors.contains_key(*key), "{} lacks {key}", v["name"]);
            }
        }
    }

    #[test]
    fn partial_user_theme_falls_back_by_type() {
        let src = r##"{ name: "Mine", type: "light", colors: { accent: "#ff0000", }, // comments ok
        }"##;
        let (themes, errors) = load_all(&[("mine.json".into(), src.into())]);
        assert!(errors.is_empty(), "{errors:?}");
        let mine = themes.iter().find(|t| t.name == "Mine").unwrap();
        assert!(!mine.dark);
        assert_eq!(mine.colors.accent, Color32::from_rgb(255, 0, 0));
        assert_eq!(mine.colors.background, Color32::from_rgb(0xfa, 0xf9, 0xf7));
    }

    #[test]
    fn user_theme_errors_are_reported_not_fatal() {
        let bad_key = r##"{ "name": "X", "colors": { "acent": "#fff000" } }"##;
        let bad_hex = r##"{ "name": "Y", "colors": { "accent": "red" } }"##;
        let (themes, errors) = load_all(&[
            ("x.json".into(), bad_key.into()),
            ("y.json".into(), bad_hex.into()),
        ]);
        assert_eq!(themes.len(), 4);
        assert!(errors[0].contains("x.json") && errors[0].contains("acent"));
        assert!(errors[1].contains("y.json") && errors[1].contains("red"));
    }

    #[test]
    fn user_theme_replaces_builtin_of_same_name() {
        let src = r##"{ "name": "oxide", "colors": { "accent": "#00ff00" } }"##;
        let (themes, _) = load_all(&[("o.json".into(), src.into())]);
        assert_eq!(themes.len(), 4);
        assert!(
            themes
                .iter()
                .any(|t| t.colors.accent == Color32::from_rgb(0, 255, 0))
        );
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(
            parse_hex("#102030"),
            Some(Color32::from_rgb(0x10, 0x20, 0x30))
        );
        assert_eq!(
            parse_hex("#10203040"),
            Some(Color32::from_rgba_unmultiplied(0x10, 0x20, 0x30, 0x40))
        );
        assert_eq!(parse_hex("102030"), None);
        assert_eq!(parse_hex("#12345"), None);
    }
}
