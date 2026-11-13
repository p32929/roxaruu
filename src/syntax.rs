//! Tree-sitter highlighting over a drop-in language registry (ported from
//! Oxru). Output is theme-independent — spans carry a [`Token`], and colours
//! are looked up at draw time — so switching theme never re-highlights.
//!
//! Adding a language: add its `tree-sitter-<lang>` crate, then one [`Lang`]
//! entry (and a sample in the `every_grammar_builds_and_highlights` test).
//! Anything unsupported or malformed falls back to plain text.
//!
//! A user can replace any language's highlighting rules, without a rebuild,
//! with `~/.config/roxaruu/queries/<id>/highlights.scm` (read at start-up).
//! Languages inside others — code in Markdown, CSS and JS in HTML — are
//! highlighted by their own grammar.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[derive(Debug)]
pub struct Lang {
    pub name: &'static str,
    /// Its name in injections ("```rust") and in `queries/<id>/`.
    pub id: &'static str,
    extensions: &'static [&'static str],
    /// What starts a line comment (⌘/), if the language has one.
    pub comment: Option<&'static str>,
    grammar: fn() -> Grammar,
}

/// A grammar and its queries, as shipped with its crate.
struct Grammar {
    language: tree_sitter::Language,
    highlights: String,
    injections: String,
    locals: &'static str,
}

fn grammar(
    language: impl Into<tree_sitter::Language>,
    highlights: &str,
    injections: &'static str,
    locals: &'static str,
) -> Grammar {
    Grammar {
        language: language.into(),
        highlights: highlights.to_string(),
        injections: injections.to_string(),
        locals,
    }
}

impl Lang {
    pub fn detect(path: &Path) -> Option<&'static Lang> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        LANGUAGES
            .iter()
            .find(|l| l.extensions.contains(&ext.as_str()))
    }

    /// The language an injection names: its id, its name, or an extension
    /// ("```py", "```TypeScript", "markdown_inline").
    fn named(name: &str) -> Option<&'static Lang> {
        let name = name.trim().to_lowercase();
        LANGUAGES.iter().find(|l| {
            l.id == name || l.name.eq_ignore_ascii_case(&name) || l.extensions.contains(&name.as_str())
        })
    }
}

static LANGUAGES: &[Lang] = &[
    Lang {
        name: "Rust",
        id: "rust",
        extensions: &["rs"],
        comment: Some("//"),
        grammar: || {
            let mut g = grammar(
                tree_sitter_rust::LANGUAGE,
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                tree_sitter_rust::INJECTIONS_QUERY,
                "",
            );
            // Its rules leave plain locals uncoloured. First, as a fallback:
            // every more specific rule (calls, types, constants) still wins.
            g.highlights.insert_str(0, "(identifier) @variable\n");
            g
        },
    },
    Lang {
        name: "JavaScript",
        id: "javascript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        comment: Some("//"),
        grammar: || {
            grammar(
                tree_sitter_javascript::LANGUAGE,
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            )
        },
    },
    Lang {
        name: "TypeScript",
        id: "typescript",
        extensions: &["ts", "mts", "cts"],
        comment: Some("//"),
        grammar: || typescript(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
    },
    Lang {
        name: "TSX",
        id: "tsx",
        extensions: &["tsx"],
        comment: Some("//"),
        grammar: || typescript(tree_sitter_typescript::LANGUAGE_TSX),
    },
    Lang {
        name: "Python",
        id: "python",
        extensions: &["py", "pyw", "pyi"],
        comment: Some("#"),
        grammar: || grammar(tree_sitter_python::LANGUAGE, tree_sitter_python::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "JSON",
        id: "json",
        extensions: &["json", "jsonc", "json5"],
        comment: Some("//"),
        grammar: || grammar(tree_sitter_json::LANGUAGE, tree_sitter_json::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "Go",
        id: "go",
        extensions: &["go"],
        comment: Some("//"),
        grammar: || grammar(tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "C",
        id: "c",
        extensions: &["c", "h"],
        comment: Some("//"),
        grammar: || grammar(tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY, "", ""),
    },
    Lang {
        name: "C++",
        id: "cpp",
        extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        comment: Some("//"),
        // C++'s highlights extend C's.
        grammar: || {
            let mut g = grammar(tree_sitter_cpp::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY, "", "");
            g.highlights.push('\n');
            g.highlights.push_str(tree_sitter_cpp::HIGHLIGHT_QUERY);
            g
        },
    },
    Lang {
        name: "C#",
        id: "c_sharp",
        extensions: &["cs"],
        comment: Some("//"),
        grammar: || grammar(tree_sitter_c_sharp::LANGUAGE, tree_sitter_c_sharp::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "Java",
        id: "java",
        extensions: &["java"],
        comment: Some("//"),
        grammar: || grammar(tree_sitter_java::LANGUAGE, tree_sitter_java::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "Swift",
        id: "swift",
        extensions: &["swift"],
        comment: Some("//"),
        grammar: || grammar(tree_sitter_swift::LANGUAGE, tree_sitter_swift::HIGHLIGHTS_QUERY, "", tree_sitter_swift::LOCALS_QUERY),
    },
    Lang {
        name: "PHP",
        id: "php",
        extensions: &["php"],
        comment: Some("//"),
        grammar: || grammar(tree_sitter_php::LANGUAGE_PHP, tree_sitter_php::HIGHLIGHTS_QUERY, tree_sitter_php::INJECTIONS_QUERY, ""),
    },
    Lang {
        name: "Ruby",
        id: "ruby",
        extensions: &["rb", "rake", "gemspec"],
        comment: Some("#"),
        grammar: || grammar(tree_sitter_ruby::LANGUAGE, tree_sitter_ruby::HIGHLIGHTS_QUERY, "", tree_sitter_ruby::LOCALS_QUERY),
    },
    Lang {
        name: "Lua",
        id: "lua",
        extensions: &["lua"],
        comment: Some("--"),
        grammar: || grammar(tree_sitter_lua::LANGUAGE, tree_sitter_lua::HIGHLIGHTS_QUERY, tree_sitter_lua::INJECTIONS_QUERY, tree_sitter_lua::LOCALS_QUERY),
    },
    Lang {
        name: "SQL",
        id: "sql",
        extensions: &["sql"],
        comment: Some("--"),
        grammar: || grammar(tree_sitter_sequel::LANGUAGE, tree_sitter_sequel::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "HTML",
        id: "html",
        extensions: &["html", "htm"],
        comment: None,
        grammar: || grammar(tree_sitter_html::LANGUAGE, tree_sitter_html::HIGHLIGHTS_QUERY, tree_sitter_html::INJECTIONS_QUERY, ""),
    },
    Lang {
        name: "CSS",
        id: "css",
        extensions: &["css", "scss"],
        comment: None,
        grammar: || grammar(tree_sitter_css::LANGUAGE, tree_sitter_css::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "Shell",
        id: "bash",
        extensions: &["sh", "bash", "zsh"],
        comment: Some("#"),
        grammar: || grammar(tree_sitter_bash::LANGUAGE, tree_sitter_bash::HIGHLIGHT_QUERY, "", ""),
    },
    Lang {
        name: "TOML",
        id: "toml",
        extensions: &["toml"],
        comment: Some("#"),
        grammar: || grammar(tree_sitter_toml_ng::LANGUAGE, tree_sitter_toml_ng::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "YAML",
        id: "yaml",
        extensions: &["yml", "yaml"],
        comment: Some("#"),
        grammar: || grammar(tree_sitter_yaml::LANGUAGE, tree_sitter_yaml::HIGHLIGHTS_QUERY, "", ""),
    },
    Lang {
        name: "Markdown",
        id: "markdown",
        extensions: &["md", "markdown"],
        comment: None,
        grammar: || {
            let mut g = grammar(
                tree_sitter_md::LANGUAGE,
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                tree_sitter_md::INJECTION_QUERY_BLOCK,
                "",
            );
            // A fenced block is coloured by its own language (plain when
            // unknown), not painted as one string around it.
            g.highlights = g
                .highlights
                .replace("  (fenced_code_block)\n", "")
                .replace("(code_fence_content) @none\n", "");
            // Inline text keeps its child nodes (the ` and * delimiters),
            // or the inline grammar never sees a whole `code` or *emphasis*.
            g.injections = g.injections.replace(
                "(#set! injection.language \"markdown_inline\"))",
                "(#set! injection.language \"markdown_inline\")\n  (#set! injection.include-children))",
            );
            g
        },
    },
    // Markdown's inline text (emphasis, links, `code`); injected, never a file.
    Lang {
        name: "Markdown Inline",
        id: "markdown_inline",
        extensions: &[],
        comment: None,
        grammar: || {
            grammar(
                tree_sitter_md::INLINE_LANGUAGE,
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                tree_sitter_md::INJECTION_QUERY_INLINE,
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
    Variable,
    Number,
    Operator,
    Tag,
    Heading,
}

/// Capture names we ask tree-sitter for; the index is the `Highlight` id.
/// A grammar's capture matches the longest of these it starts with, so
/// "keyword.function" lands on "keyword".
const CAPTURES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "conditional",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "field",
    "float",
    "function",
    "include",
    "keyword",
    "label",
    "method",
    "module",
    "namespace",
    "number",
    "operator",
    "parameter",
    "preproc",
    "property",
    "punctuation",
    "repeat",
    "storageclass",
    "string",
    "string.escape",
    "tag",
    "text.literal",
    "text.reference",
    "text.title",
    "text.uri",
    "type",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
];

fn token_for(capture: &str) -> Option<Token> {
    Some(match capture {
        "comment" => Token::Comment,
        "keyword" | "conditional" | "repeat" | "storageclass" | "include" | "preproc"
        | "variable.builtin" => Token::Keyword,
        "string" | "character" | "text.literal" => Token::String,
        "function" | "method" => Token::Function,
        "type" | "constructor" | "module" | "namespace" | "text.uri" | "text.reference" => {
            Token::Type
        }
        "constant" | "constant.builtin" | "boolean" | "escape" | "string.escape" => {
            Token::Constant
        }
        "number" | "float" => Token::Number,
        "property" | "field" | "variable.member" => Token::Property,
        "variable" | "variable.parameter" | "parameter" => Token::Variable,
        "attribute" | "label" => Token::Attribute,
        "operator" => Token::Operator,
        "punctuation" => Token::Punctuation,
        "tag" => Token::Tag,
        "text.title" => Token::Heading,
        _ => return None,
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

/// `queries/README.md` in the config folder: how to change a language's
/// colouring rules, and the ids to use.
pub fn queries_readme() -> String {
    let ids: Vec<String> = LANGUAGES
        .iter()
        .map(|l| format!("- `{}` — {}", l.id, l.name))
        .collect();
    format!(
        "# Highlighting rules\n\n\
         Each language is coloured by tree-sitter rules. To change one, put your own\n\
         rules in `<id>/highlights.scm` here (it replaces the built-in rules; read at\n\
         start-up). A file that doesn't compile is ignored.\n\n\
         Capture names pick the colour: `@keyword`, `@string`, `@function`, `@type`,\n\
         `@constant`, `@number`, `@variable`, `@property`, `@attribute`, `@operator`,\n\
         `@punctuation`, `@comment`, `@tag`, `@text.title` (theme keys `tokenKeyword`…).\n\n\
         The built-in rules are the grammar's own `queries/highlights.scm`, e.g.\n\
         https://github.com/tree-sitter/tree-sitter-rust/blob/master/queries/highlights.scm\n\n\
         Ids:\n\n{}\n",
        ids.join("\n")
    )
}

/// Highlight configs, built once per language on first use and kept for
/// the life of the app (so injections can borrow them from any thread).
fn config(lang: &'static Lang) -> Option<&'static HighlightConfiguration> {
    static CONFIGS: OnceLock<Mutex<HashMap<&'static str, Option<&'static HighlightConfiguration>>>> =
        OnceLock::new();
    let mut map = CONFIGS.get_or_init(Default::default).lock().ok()?;
    *map.entry(lang.id).or_insert_with(|| build(lang).map(|c| &*Box::leak(Box::new(c))))
}

/// The user's `queries/<id>/highlights.scm` if there is one (and it
/// compiles), else the grammar's own.
fn build(lang: &Lang) -> Option<HighlightConfiguration> {
    let mine = std::fs::read_to_string(
        crate::config::dir()
            .join("queries")
            .join(lang.id)
            .join("highlights.scm"),
    );
    build_with(lang, mine.ok())
}

fn build_with(lang: &Lang, mine: Option<String>) -> Option<HighlightConfiguration> {
    let g = (lang.grammar)();
    let make = |highlights: &str| {
        let mut cfg = HighlightConfiguration::new(
            g.language.clone(),
            lang.id,
            highlights,
            &g.injections,
            g.locals,
        )
        .ok()?;
        cfg.configure(CAPTURES);
        Some(cfg)
    };
    mine.and_then(|q| make(&q)).or_else(|| make(&g.highlights))
}

fn spans_for(lang: &'static Lang, source: &str) -> Option<Vec<LineSpans>> {
    let cfg = config(lang)?;
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(cfg, source.as_bytes(), None, |name| {
            Lang::named(name).and_then(config)
        })
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
                let token = stack.last().and_then(|&i| CAPTURES.get(i)).and_then(|c| token_for(c));
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
}

/// TypeScript's highlights are an addition to JavaScript's.
fn typescript(language: impl Into<tree_sitter::Language>) -> Grammar {
    let highlights = format!(
        "{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    );
    grammar(language, &highlights, "", tree_sitter_typescript::LOCALS_QUERY)
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
            ("x.rs", "fn main() {}\n"),
            ("x.cpp", "class A { public: int x; };\n"),
            ("x.cs", "class A { int x = 1; }\n"),
            ("x.java", "class A { int x = 1; }\n"),
            ("x.swift", "let x = 1\n"),
            ("x.php", "<?php echo 1; ?>\n"),
            ("x.rb", "def f; 1; end\n"),
            ("x.lua", "local x = 1\n"),
            ("x.sql", "SELECT 1 FROM t;\n"),
            ("x.yaml", "a: 1\n"),
            ("x.md", "# Title\n"),
        ];
        for (file, src) in samples {
            assert!(!tokens(file, src).is_empty(), "{file} produced no tokens");
        }
        // And every language in the table has a sample above.
        for l in LANGUAGES.iter().filter(|l| !l.extensions.is_empty()) {
            assert!(
                samples.iter().any(|(f, _)| Lang::detect(Path::new(f)).map(|d| d.id) == Some(l.id)),
                "{} has no sample",
                l.name
            );
        }
    }

    #[test]
    fn variables_numbers_and_operators_have_colours() {
        let t = tokens("m.py", "def f(count):\n    total = count + 42\n");
        assert!(t.contains(&("count".into(), Token::Variable)), "{t:?}");
        assert!(t.contains(&("42".into(), Token::Number)), "{t:?}");
        assert!(t.contains(&("+".into(), Token::Operator)), "{t:?}");
        let t = tokens("m.rs", "fn f() { let total = Some(1); total.max(2) }\n");
        assert!(t.contains(&("total".into(), Token::Variable)), "{t:?}");
        assert!(t.contains(&("f".into(), Token::Function)), "definitions win: {t:?}");
        assert!(t.contains(&("max".into(), Token::Function)), "{t:?}");
        let t = tokens("m.html", "<p class=\"a\">hi</p>\n");
        assert!(t.contains(&("p".into(), Token::Tag)), "{t:?}");
    }

    #[test]
    fn markdown_headings_and_code_inside_it() {
        let t = tokens("a.md", "# Title\n\n```rust\nfn main() {}\n```\n\nSome `code` here\n");
        assert!(t.iter().any(|(s, k)| s.contains("Title") && *k == Token::Heading), "{t:?}");
        // The fenced block is highlighted as Rust.
        assert!(t.contains(&("fn".into(), Token::Keyword)), "{t:?}");
        assert!(!t.iter().any(|(s, k)| s.contains("main") && *k == Token::String), "not one string: {t:?}");
        assert!(t.iter().any(|(s, k)| s.contains("code") && *k == Token::String), "{t:?}");
    }

    #[test]
    fn injections_find_languages_by_id_name_or_extension() {
        for (name, id) in [("rust", "rust"), ("TypeScript", "typescript"), ("py", "python"), ("markdown_inline", "markdown_inline")] {
            assert_eq!(Lang::named(name).map(|l| l.id), Some(id), "{name}");
        }
        assert!(Lang::named("klingon").is_none());
    }

    #[test]
    fn a_users_query_replaces_the_builtin_one_and_a_broken_one_is_ignored() {
        let rust = Lang::named("rust").unwrap();
        let builtin = build_with(rust, None).unwrap().query.pattern_count();
        let mine = build_with(rust, Some("(identifier) @keyword".into())).unwrap();
        assert!(mine.query.pattern_count() < builtin);
        let broken = build_with(rust, Some("(no_such_node) @x (".into())).unwrap();
        assert_eq!(broken.query.pattern_count(), builtin, "fell back to the built-in rules");
    }

    #[test]
    fn every_capture_we_ask_for_has_a_colour() {
        for c in CAPTURES {
            assert!(token_for(c).is_some(), "{c}");
        }
    }

    #[test]
    fn the_queries_readme_names_every_language() {
        let readme = queries_readme();
        for l in LANGUAGES {
            assert!(readme.contains(&format!("`{}`", l.id)), "{}", l.id);
        }
    }

    #[test]
    fn line_comments_per_language() {
        let comment = |f: &str| Lang::detect(Path::new(f)).and_then(|l| l.comment);
        assert_eq!(comment("a.rs"), Some("//"));
        assert_eq!(comment("a.py"), Some("#"));
        assert_eq!(comment("a.sql"), Some("--"));
        assert_eq!(comment("a.html"), None);
    }

    #[test]
    fn plain_text_keeps_lines() {
        let h = highlight("a\r\nb\n", None);
        assert_eq!(h.spans.len(), 3);
        assert!(h.spans.iter().all(Vec::is_empty));
    }
}
