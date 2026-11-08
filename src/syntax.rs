//! Tree-sitter highlighting over a drop-in language registry (ported from
//! Oxru). Output is theme-independent — spans carry a [`Token`], and colours
//! are looked up at draw time — so switching theme never re-highlights.
//!
//! Adding a language: add its `tree-sitter-<lang>` crate, then one [`Lang`]
//! entry. Anything unsupported or malformed falls back to plain text.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[derive(Debug)]
pub struct Lang {
    pub name: &'static str,
    extensions: &'static [&'static str],
    build: fn() -> Option<HighlightConfiguration>,
}

impl Lang {
    pub fn detect(path: &Path) -> Option<&'static Lang> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        LANGUAGES
            .iter()
            .find(|l| l.extensions.contains(&ext.as_str()))
    }
}

static LANGUAGES: &[Lang] = &[
    Lang {
        name: "Rust",
        extensions: &["rs"],
        build: || {
            make(
                tree_sitter_rust::LANGUAGE.into(),
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                "",
                "",
            )
        },
    },
    Lang {
        name: "JavaScript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        build: || {
            make(
                tree_sitter_javascript::LANGUAGE.into(),
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            )
        },
    },
    Lang {
        name: "TypeScript",
        extensions: &["ts", "mts", "cts"],
        build: || {
            typescript(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
            )
        },
    },
    Lang {
        name: "TSX",
        extensions: &["tsx"],
        build: || typescript(tree_sitter_typescript::LANGUAGE_TSX.into(), "tsx"),
    },
    Lang {
        name: "Python",
        extensions: &["py", "pyw", "pyi"],
        build: || {
            make(
                tree_sitter_python::LANGUAGE.into(),
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
                "",
            )
        },
    },
    Lang {
        name: "JSON",
        extensions: &["json", "jsonc", "json5"],
        build: || {
            make(
                tree_sitter_json::LANGUAGE.into(),
                "json",
                tree_sitter_json::HIGHLIGHTS_QUERY,
                "",
                "",
            )
        },
    },
    Lang {
        name: "Go",
        extensions: &["go"],
        build: || {
            make(
                tree_sitter_go::LANGUAGE.into(),
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
                "",
            )
        },
    },
    Lang {
        name: "C",
        extensions: &["c", "h"],
        build: || {
            make(
                tree_sitter_c::LANGUAGE.into(),
                "c",
                tree_sitter_c::HIGHLIGHT_QUERY,
                "",
                "",
            )
        },
    },
    Lang {
        name: "HTML",
        extensions: &["html", "htm"],
        build: || {
            make(
                tree_sitter_html::LANGUAGE.into(),
                "html",
                tree_sitter_html::HIGHLIGHTS_QUERY,
                tree_sitter_html::INJECTIONS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "CSS",
        extensions: &["css"],
        build: || {
            make(
                tree_sitter_css::LANGUAGE.into(),
                "css",
                tree_sitter_css::HIGHLIGHTS_QUERY,
                "",
                "",
            )
        },
    },
    Lang {
        name: "Shell",
        extensions: &["sh", "bash", "zsh"],
        build: || {
            make(
                tree_sitter_bash::LANGUAGE.into(),
                "bash",
                tree_sitter_bash::HIGHLIGHT_QUERY,
                "",
                "",
            )
        },
    },
    Lang {
        name: "TOML",
        extensions: &["toml"],
        build: || {
            make(
                tree_sitter_toml_ng::LANGUAGE.into(),
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
                "",
            )
        },
    },
];

/// The token kinds a theme colours. Captures map onto these in [`token_for`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    Comment,
    Keyword,
    String,
    Function,
    Type,
    Constant,
    Property,
    Attribute,
    Punctuation,
}

/// Capture names we ask tree-sitter for; the index is the `Highlight` id.
const CAPTURES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "function",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

fn token_for(capture: &str) -> Option<Token> {
    Some(match capture {
        "comment" => Token::Comment,
        "keyword" | "variable.builtin" => Token::Keyword,
        "string" => Token::String,
        "function" | "function.macro" | "function.method" => Token::Function,
        "type" | "type.builtin" => Token::Type,
        "constant" | "constant.builtin" | "number" => Token::Constant,
        "property" | "variable.parameter" => Token::Property,
        "attribute" | "label" => Token::Attribute,
        "operator" | "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => {
            Token::Punctuation
        }
        _ => return None, // plain variables keep the text colour
    })
}

/// A highlighted file: per line (split on `\n`), styled byte ranges into
/// that line.
pub struct Highlighted {
    pub spans: Vec<LineSpans>,
}

/// Styled byte ranges within one line.
pub type LineSpans = Vec<(Range<usize>, Token)>;

pub fn highlight(source: &str, lang: Option<&'static Lang>) -> Highlighted {
    let spans = lang
        .and_then(|l| spans_for(l, source))
        .unwrap_or_else(|| vec![Vec::new(); source.split('\n').count()]);
    Highlighted { spans }
}

thread_local! {
    /// Highlight configs, built once per language on first use.
    static CONFIGS: RefCell<HashMap<&'static str, Option<HighlightConfiguration>>> = RefCell::new(HashMap::new());
}

fn spans_for(lang: &'static Lang, source: &str) -> Option<Vec<LineSpans>> {
    CONFIGS.with(|cache| {
        let mut map = cache.borrow_mut();
        let cfg = map.entry(lang.name).or_insert_with(lang.build).as_ref()?;
        let mut highlighter = Highlighter::new();
        let events = highlighter
            .highlight(cfg, source.as_bytes(), None, |_| None)
            .ok()?;

        let mut out: Vec<LineSpans> = vec![Vec::new()];
        let mut line_start = 0; // byte offset of the current line in `source`
        let mut stack: Vec<usize> = Vec::new();
        for event in events {
            match event.ok()? {
                HighlightEvent::HighlightStart(h) => stack.push(h.0),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let token = stack.last().and_then(|&i| token_for(CAPTURES[i]));
                    // Split the run at newlines into per-line ranges.
                    let mut pos = start;
                    while pos < end {
                        let nl = source[pos..end].find('\n').map(|i| pos + i);
                        let stop = nl.unwrap_or(end);
                        if let Some(t) = token
                            && stop > pos
                            && let Some(line) = out.last_mut()
                        {
                            line.push((pos - line_start..stop - line_start, t));
                        }
                        match nl {
                            Some(n) => {
                                out.push(Vec::new());
                                line_start = n + 1;
                                pos = n + 1;
                            }
                            None => pos = end,
                        }
                    }
                }
            }
        }
        Some(out)
    })
}

fn make(
    language: tree_sitter::Language,
    name: &str,
    highlights: &str,
    injections: &str,
    locals: &str,
) -> Option<HighlightConfiguration> {
    let mut cfg =
        HighlightConfiguration::new(language, name, highlights, injections, locals).ok()?;
    cfg.configure(CAPTURES);
    Some(cfg)
}

/// TypeScript's highlights are an addition to JavaScript's.
fn typescript(language: tree_sitter::Language, name: &str) -> Option<HighlightConfiguration> {
    let highlights = format!(
        "{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    );
    make(
        language,
        name,
        &highlights,
        "",
        tree_sitter_typescript::LOCALS_QUERY,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(file: &str, src: &str) -> Vec<(String, Token)> {
        let h = highlight(src, Lang::detect(Path::new(file)));
        let lines: Vec<&str> = src.split('\n').collect();
        assert_eq!(lines.len(), h.spans.len(), "{file}: one span list per line");
        h.spans
            .iter()
            .zip(lines)
            .flat_map(|(spans, line)| {
                spans
                    .iter()
                    .map(move |(r, t)| (line[r.clone()].to_string(), *t))
            })
            .collect()
    }

    #[test]
    fn detects_by_extension() {
        assert_eq!(
            Lang::detect(Path::new("a.rs")).map(|l| l.name),
            Some("Rust")
        );
        assert_eq!(
            Lang::detect(Path::new("a.TSX")).map(|l| l.name),
            Some("TSX")
        );
        assert!(Lang::detect(Path::new("README")).is_none());
    }

    #[test]
    fn rust_keywords_strings_and_multiline_comments() {
        let t = tokens("m.rs", "fn main() {\n    /* a\n b */ let s = \"hi\";\n}\n");
        assert!(t.contains(&("fn".into(), Token::Keyword)));
        assert!(t.contains(&("\"hi\"".into(), Token::String)));
        // A comment spanning lines is split per line.
        assert!(t.contains(&("/* a".into(), Token::Comment)));
        assert!(t.contains(&(" b */".into(), Token::Comment)));
    }

    #[test]
    fn every_grammar_builds_and_highlights() {
        let samples = [
            ("x.py", "def f():\n    return 1\n"),
            ("x.js", "const x = 1;\n"),
            ("x.ts", "let x: number = 1;\n"),
            ("x.tsx", "const a = <div />;\n"),
            ("x.go", "package main\nfunc main() {}\n"),
            ("x.c", "int main(void) { return 0; }\n"),
            ("x.json", "{\n  \"a\": 1\n}\n"),
            ("x.css", "a { color: red; }\n"),
            ("x.html", "<p>hi</p>\n"),
            ("x.sh", "echo hi\n"),
            ("x.toml", "a = 1\n"),
        ];
        for (file, src) in samples {
            assert!(!tokens(file, src).is_empty(), "{file} produced no tokens");
        }
    }

    #[test]
    fn plain_text_keeps_lines() {
        let h = highlight("a\r\nb\n", None);
        assert_eq!(h.spans.len(), 3);
        assert!(h.spans.iter().all(Vec::is_empty));
    }
}
