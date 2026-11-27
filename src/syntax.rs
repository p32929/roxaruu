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

/// Files that are words, not code: no colours guessed for them.
const PROSE: &[&str] = &["txt", "text", "log", "csv", "tsv", "rtf"];

/// Any other code: coloured by `generic`, from what nearly every language
/// shares, so Dart, Kotlin, Zig… aren't plain text without a grammar here.
/// Never built into a tree-sitter config (`config`, `spans_for`).
pub static GENERIC: Lang = Lang {
    name: "Code",
    id: "generic",
    extensions: &[],
    comment: None,
    grammar: || grammar(tree_sitter_json::LANGUAGE, "", "", ""),
};

/// A diff (what git says changed, a `.patch`): its lines coloured as the
/// code they are (`generic`), its headers dimmed (`diff_spans`). The editor
/// tints added and removed lines.
pub static DIFF: Lang = Lang {
    name: "Diff",
    id: "diff",
    extensions: &["diff", "patch"],
    comment: None,
    grammar: || grammar(tree_sitter_json::LANGUAGE, "", "", ""),
};

impl Lang {
    pub fn detect(path: &Path) -> Option<&'static Lang> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        LANGUAGES
            .iter()
            .chain([&DIFF])
            .find(|l| l.extensions.contains(&ext.as_str()))
            .or_else(|| (!PROSE.contains(&ext.as_str())).then_some(&GENERIC))
    }

    /// What the status bar calls a file's language: a generic one by its
    /// extension ("Dart").
    pub fn label(lang: Option<&Lang>, path: Option<&Path>) -> String {
        match lang {
            Some(l) if l.id == GENERIC.id => {
                let ext = path.and_then(|p| p.extension()).and_then(|e| e.to_str()).unwrap_or(l.name);
                let mut c = ext.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
            }
            Some(l) => l.name.to_string(),
            None => "Plain Text".to_string(),
        }
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

/// Styled byte ranges within one line: `u32`s, half the size (a line that
/// long is a huge file's, shown without highlighting).
pub type LineSpans = Vec<(Range<u32>, Token)>;

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
    if lang.id == GENERIC.id || lang.id == DIFF.id {
        return None;
    }
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
    if lang.id == GENERIC.id {
        return Some(generic(source));
    }
    if lang.id == DIFF.id {
        return Some(diff_spans(source));
    }
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
                        let at = |b: usize| u32::try_from(b - line_start).unwrap_or(u32::MAX);
                        line.push((at(pos)..at(stop), t));
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

/// Words most languages reserve (a union: one list for every language).
const KEYWORDS: &[&str] = &[
    "abstract", "and", "as", "assert", "async", "await", "break", "case", "catch", "class", "const", "continue",
    "covariant", "def", "default", "defer", "del", "delete", "deferred", "do", "elif", "else", "elsif", "enum",
    "except", "export", "extends", "extension", "extern", "external", "factory", "final", "finally", "fn", "for",
    "foreach", "from", "fun", "func", "function", "go", "goto", "guard", "hide", "if", "impl", "implements",
    "import", "in", "include", "inline", "interface", "internal", "is", "late", "let", "library", "loop", "match",
    "mixin", "mod", "module", "mut", "namespace", "new", "not", "on", "operator", "or", "override", "package",
    "part", "private", "protected", "pub", "public", "raise", "readonly", "required", "rescue", "return", "sealed",
    "self", "show", "static", "struct", "super", "switch", "sync", "this", "throw", "throws", "trait", "try",
    "typedef", "typeof", "union", "unless", "unsafe", "until", "use", "using", "val", "var", "virtual", "volatile",
    "when", "where", "while", "with", "yield",
];
const CONSTANTS: &[&str] = &["true", "false", "null", "nil", "None", "True", "False", "undefined", "NaN"];
const PRIMITIVES: &[&str] = &[
    "int", "float", "double", "bool", "boolean", "char", "string", "str", "byte", "long", "short", "void",
    "num", "dynamic", "any", "unknown", "never", "usize", "isize", "u8", "u16", "u32", "u64", "i8", "i16",
    "i32", "i64", "f32", "f64",
];
const OPERATORS: &[u8] = b"+-*/%=<>!&|^~?:";

/// A diff: each changed or kept line's code (after its `+` `-` or space)
/// coloured by `generic`, the rest (`diff --git`, `@@ … @@`, a commit's
/// header) dimmed.
fn diff_spans(source: &str) -> Vec<LineSpans> {
    let code = |l: &str| matches!(l.as_bytes().first(), Some(b'+' | b'-' | b' ')) && !l.starts_with("+++ ") && !l.starts_with("--- ");
    // The marks become spaces: same columns, nothing for `generic` to read.
    let stripped: String = source
        .split('\n')
        .map(|l| if code(l) { format!(" {}", l.get(1..).unwrap_or("")) } else { String::new() })
        .collect::<Vec<_>>()
        .join("\n");
    let mut spans = generic(&stripped);
    for (line, l) in source.split('\n').enumerate() {
        if let (false, Some(s), Ok(n)) = (code(l), spans.get_mut(line), u32::try_from(l.len())) {
            *s = if n > 0 { vec![(0..n, if l.starts_with("@@") { Token::Attribute } else { Token::Comment })] } else { Vec::new() };
        }
    }
    spans
}

/// Colours for code in a language without a grammar here, from what
/// nearly all share: comments (`//`, `/* */`, `#`, `--`), strings
/// (`"…"`, `'…'`, `` `…` ``, triple-quoted), numbers, `@annotations`,
/// common keywords, calls (`name(`), types (`Capitalised`), constants
/// (`ALL_CAPS`), `.fields`, operators and punctuation. One pass.
// ponytail: a guess, not a parse: `#` starts a comment unless a word or
// `[` follows it (`#include`, `#[..]`), `--` only before a space.
fn generic(source: &str) -> Vec<LineSpans> {
    let b = source.as_bytes();
    let len = b.len();
    let starts: Vec<usize> = std::iter::once(0).chain(source.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let mut out: Vec<LineSpans> = vec![Vec::new(); starts.len()];
    // `from..to` (byte offsets), split into each line it covers.
    let mut push = |from: usize, to: usize, t: Token| {
        let mut line = starts.partition_point(|&s| s <= from).saturating_sub(1);
        let mut pos = from;
        while pos < to {
            let Some(&start) = starts.get(line) else { break };
            let end = starts.get(line + 1).map_or(len, |&s| s - 1);
            let stop = to.min(end);
            if stop > pos
                && let (Some(spans), Ok(a), Ok(z)) =
                    (out.get_mut(line), u32::try_from(pos - start), u32::try_from(stop - start))
            {
                spans.push((a..z, t));
            }
            line += 1;
            pos = starts.get(line).copied().unwrap_or(len);
        }
    };
    let at = |i: usize| b.get(i).copied();
    let has = |i: usize, s: &str| b.get(i..).is_some_and(|r| r.starts_with(s.as_bytes()));
    let find = |i: usize, s: &str| source.get(i..).and_then(|r| r.find(s)).map(|k| i + k);
    let line_end = |i: usize| find(i, "\n").unwrap_or(len);
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80;
    let word_end = |mut j: usize| {
        while at(j).is_some_and(ident) {
            j += 1;
        }
        j
    };
    let mut i = 0;
    while let Some(c) = at(i) {
        let next = at(i + 1);
        if has(i, "/*") {
            let end = find(i + 2, "*/").map_or(len, |e| e + 2);
            push(i, end, Token::Comment);
            i = end;
        } else if has(i, "//")
            || (c == b'#' && !next.is_some_and(|n| n.is_ascii_alphabetic() || n == b'['))
            || (has(i, "--") && at(i + 2).is_none_or(|n| n == b' ' || n == b'\n' || n == b'-'))
        {
            let end = line_end(i);
            push(i, end, Token::Comment);
            i = end;
        } else if c == b'#' || (c == b'@' && next.is_some_and(|n| n.is_ascii_alphabetic())) {
            let end = if next == Some(b'[') {
                find(i, "]").filter(|&e| e < line_end(i)).map_or(i + 1, |e| e + 1)
            } else {
                word_end(i + 1)
            };
            push(i, end, Token::Attribute);
            i = end;
        } else if let Some(q) = ["\"\"\"", "'''"].into_iter().find(|q| has(i, q)) {
            let end = find(i + 3, q).map_or(len, |e| e + 3);
            push(i, end, Token::String);
            i = end;
        } else if (c == b'"' || c == b'`' || c == b'\'') && !(c == b'\'' && i > 0 && at(i - 1).is_some_and(ident)) {
            // To the closing quote; `"` and `'` end with their line.
            let mut j = i + 1;
            while let Some(d) = at(j) {
                if d == b'\\' {
                    j += 2;
                    continue;
                }
                if d == c {
                    j += 1;
                    break;
                }
                if d == b'\n' && c != b'`' {
                    break;
                }
                j += 1;
            }
            let end = j.min(len);
            push(i, end, Token::String);
            i = end;
        } else if c.is_ascii_digit() {
            let mut j = i + 1;
            while at(j).is_some_and(ident) || (at(j) == Some(b'.') && at(j + 1).is_some_and(|d| d.is_ascii_digit())) {
                j += 1;
            }
            push(i, j, Token::Number);
            i = j;
        } else if ident(c) {
            let j = word_end(i);
            let word = source.get(i..j).unwrap_or("");
            let mut k = j;
            while matches!(at(k), Some(b' ' | b'\t')) {
                k += 1;
            }
            let after_dot = source.get(..i).is_some_and(|s| s.trim_end_matches([' ', '\t']).ends_with('.'));
            let mut letters = word.chars().filter(|c| c.is_alphabetic()).peekable();
            let all_caps = word.len() > 1 && letters.peek().is_some() && letters.all(char::is_uppercase);
            let token = if KEYWORDS.contains(&word) {
                Some(Token::Keyword)
            } else if CONSTANTS.contains(&word) {
                Some(Token::Constant)
            } else if PRIMITIVES.contains(&word) {
                Some(Token::Type)
            } else if at(k) == Some(b'(') {
                Some(Token::Function)
            } else if all_caps {
                Some(Token::Constant)
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                Some(Token::Type)
            } else if after_dot {
                Some(Token::Property)
            } else {
                None
            };
            if let Some(t) = token {
                push(i, j, t);
            }
            i = j;
        } else if OPERATORS.contains(&c) {
            let mut j = i + 1;
            while at(j).is_some_and(|d| OPERATORS.contains(&d)) && !has(j, "//") && !has(j, "/*") {
                j += 1;
            }
            push(i, j, Token::Operator);
            i = j;
        } else if b"(){}[],;.".contains(&c) {
            push(i, i + 1, Token::Punctuation);
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

/// The pieces of a language that colour the same alone as in their file
/// (no rule looks outside them), so typing in one re-colours just it.
/// Languages whose rules follow names across the file (`locals` queries:
/// JavaScript, TypeScript, Swift, Ruby, Lua) aren't here, nor any that
/// `incremental_matches_whole_file` finds colouring differently (C#: a
/// half-typed line can recolour the lines above its piece): they're
/// coloured whole, as before.
fn units(lang: &Lang) -> &'static [&'static str] {
    match lang.id {
        "rust" => &[
            "function_item",
            "impl_item",
            "struct_item",
            "enum_item",
            "trait_item",
            "mod_item",
            "const_item",
            "static_item",
            "macro_definition",
            "type_item",
            "use_declaration",
        ],
        "go" => &["function_declaration", "method_declaration", "type_declaration"],
        "c" => &["function_definition"],
        "cpp" => &["function_definition"],
        "java" => &["method_declaration", "constructor_declaration", "class_declaration"],
        "css" => &["rule_set", "media_statement"],
        _ => &[],
    }
}

/// Highlighting kept up to date as a file is edited: its syntax tree is
/// re-parsed only where it changed, and only the smallest piece around the
/// change (a function, a block: `units`) is coloured again; the rest keeps
/// its colours. Anything else (a language without `units`, a change that
/// isn't inside one, code that doesn't parse there) is coloured whole.
pub struct Incremental {
    pub lang: &'static Lang,
    parser: tree_sitter::Parser,
    tree: tree_sitter::Tree,
    text: String,
}

impl Incremental {
    /// For `text` in `lang`, if `lang` can be coloured piece by piece.
    pub fn new(lang: &'static Lang, text: String) -> Option<Incremental> {
        if units(lang).is_empty() {
            return None;
        }
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&config(lang)?.language).ok()?;
        let tree = parser.parse(&text, None)?;
        Some(Incremental { lang, parser, tree, text })
    }

    /// The file is `text` now: bring `spans` (this file's, as it was) up to
    /// date with it. True when just a piece was coloured again.
    pub fn update(&mut self, text: String, spans: &mut Vec<LineSpans>) -> bool {
        let old = std::mem::replace(&mut self.text, text);
        if old == self.text && spans.len() == self.text.split('\n').count() {
            return true;
        }
        let piece = self.update_piece(&old, spans);
        if !piece {
            *spans = highlight(&self.text, Some(self.lang)).spans;
            if let Some(tree) = self.parser.parse(&self.text, None) {
                self.tree = tree;
            }
        }
        piece
    }

    /// `update` for one piece; false when the file must be coloured whole.
    fn update_piece(&mut self, old: &str, spans: &mut Vec<LineSpans>) -> bool {
        let new = self.text.as_str();
        if spans.len() != old.split('\n').count() {
            return false;
        }
        // What changed: the bytes between what's the same at both ends.
        let (ob, nb) = (old.as_bytes(), new.as_bytes());
        let mut start = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
        while !new.is_char_boundary(start) || !old.is_char_boundary(start) {
            start -= 1;
        }
        let room = ob.len().min(nb.len()) - start;
        let mut same_end = ob.iter().rev().zip(nb.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
        while !new.is_char_boundary(nb.len() - same_end) || !old.is_char_boundary(ob.len() - same_end) {
            same_end -= 1;
        }
        let (old_end, new_end) = (ob.len() - same_end, nb.len() - same_end);
        let point = |s: &str, at: usize| {
            let before = s.get(..at).unwrap_or_default();
            let row = before.matches('\n').count();
            tree_sitter::Point::new(row, at - before.rfind('\n').map_or(0, |i| i + 1))
        };
        self.tree.edit(&tree_sitter::InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            start_position: point(old, start),
            old_end_position: point(old, old_end),
            new_end_position: point(new, new_end),
        });
        let Some(tree) = self.parser.parse(new, Some(&self.tree)) else { return false };
        // Where the syntax changed too (a `"` opened can change what follows).
        let (mut a, mut b) = (start, new_end);
        for r in self.tree.changed_ranges(&tree) {
            a = a.min(r.start_byte);
            b = b.max(r.end_byte);
        }
        self.tree = tree;
        let Some(unit) = self.unit(a, b) else { return false };
        // Its lines, coloured alone, in place of theirs.
        let (first, last) = (unit.start_position().row, unit.end_position().row);
        let line_start = |row: usize| {
            if row == 0 { 0 } else { new.match_indices('\n').nth(row - 1).map_or(new.len(), |(i, _)| i + 1) }
        };
        let (from, to) = (line_start(first), line_start(last + 1));
        let Some(piece) = new.get(from..to) else { return false };
        let piece = piece.strip_suffix('\n').unwrap_or(piece);
        let Some(colored) = spans_for(self.lang, piece) else { return false };
        let new_lines = new.split('\n').count();
        let Some(old_last) = (last + spans.len()).checked_sub(new_lines) else { return false };
        if colored.len() != last - first + 1 || old_last + 1 < first || old_last >= spans.len() {
            return false;
        }
        spans.splice(first..=old_last, colored);
        spans.len() == new_lines
    }

    /// The smallest piece around bytes `a..b` that colours alone: one of
    /// `units`, on lines of its own, that parsed cleanly.
    fn unit(&self, a: usize, b: usize) -> Option<tree_sitter::Node<'_>> {
        let text = self.text.as_str();
        let kinds = units(self.lang);
        let own_lines = |n: &tree_sitter::Node| {
            let (Some(before), Some(after)) = (text.get(..n.start_byte()), text.get(n.end_byte()..)) else { return false };
            let lead = before.rsplit('\n').next().unwrap_or_default();
            let tail = after.split('\n').next().unwrap_or_default();
            lead.trim().is_empty() && tail.trim().is_empty()
        };
        let mut node = self.tree.root_node().descendant_for_byte_range(a, b)?;
        loop {
            if node.start_byte() <= a && b <= node.end_byte() && kinds.contains(&node.kind()) && own_lines(&node) {
                // Broken there (a `"` left open): coloured alone it can come
                // out other than in the whole file, so the whole file is.
                return (!node.has_error()).then_some(node);
            }
            node = node.parent()?;
        }
    }
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

    /// What `generic` calls each of `words` in `source` (where it's first seen).
    fn generic_tokens(source: &str, words: &[&str]) -> Vec<Option<Token>> {
        let spans = generic(source);
        words
            .iter()
            .map(|w| {
                let (n, line) = source.split('\n').enumerate().find(|(_, l)| l.contains(w)).unwrap();
                let at = line.find(w).unwrap() as u32;
                spans[n].iter().find(|(r, _)| r.start <= at && at < r.end).map(|(_, t)| *t)
            })
            .collect()
    }

    #[test]
    fn any_other_code_gets_colours() {
        let dart = "/// Shared widgets\nlibrary;\n\nimport 'package:flutter/material.dart';\n@override\nWidget build(BuildContext context) {\n  final count = 42; // the answer\n  return Text(\"hi $count\", style: theme.body);\n}\n/* a\n   block */\nconst MAX_ITEMS = null;";
        let words = [
            "Shared", "library", "package:flutter", "@override", "Widget", "build", "final", "42", "the answer",
            "return", "\"hi", "body", "block */", "MAX_ITEMS", "null", "count =",
        ];
        use Token::*;
        assert_eq!(
            generic_tokens(dart, &words),
            [
                Some(Comment), Some(Keyword), Some(String), Some(Attribute), Some(Type), Some(Function),
                Some(Keyword), Some(Number), Some(Comment), Some(Keyword), Some(String), Some(Property),
                Some(Comment), Some(Constant), Some(Constant), None,
            ]
        );
        // Python's and the shell's comments, strings over lines, `#include`,
        // an apostrophe in a word.
        let py = "# note\nx = \"\"\"one\ntwo\"\"\"\ny = don't\n#include <a.h>";
        assert_eq!(generic_tokens(py, &["note", "two", "#include"]), [Some(Comment), Some(String), Some(Attribute)]);
        assert!(generic(py)[3].iter().all(|(_, t)| *t != String), "don't isn't a string");
        // Every span within its line, in order, on character boundaries.
        let odd = "é = \"ü\\\"x\" // ñ\n'unclosed\n`multi\nline`";
        let spans = generic(odd);
        assert_eq!(spans.len(), 4);
        for (spans, line) in spans.iter().zip(odd.split('\n')) {
            let mut last = 0;
            for (r, _) in spans {
                let (a, z) = (r.start as usize, r.end as usize);
                assert!(r.start >= last && z <= line.len() && line.is_char_boundary(a) && line.is_char_boundary(z), "{line:?} {r:?}");
                last = r.end;
            }
        }
    }

    /// A diff: headers dimmed, the code in it coloured, at its own columns.
    #[test]
    fn diffs_coloured() {
        use Token::*;
        let d = "diff --git a/x b/x\n--- a/x\n@@ -1 +1 @@\n-let a = 1;\n+let a = \"b\";";
        let spans = highlight(d, Lang::detect(Path::new("fix.patch"))).spans;
        let first = |line: usize| spans[line].first().map(|(r, t)| (r.start, *t));
        assert_eq!(first(0), Some((0, Comment)));
        assert_eq!(first(1), Some((0, Comment)), "--- a/x is a header, not a removed line");
        assert_eq!(first(2), Some((0, Attribute)));
        assert!(spans[4].iter().any(|(r, t)| *t == String && r.start == 9), "{:?}", spans[4]);
        assert!(spans[3].iter().all(|(r, _)| r.start >= 1), "the - isn't code");
    }

    #[test]
    fn code_without_a_grammar_is_generic_prose_is_plain() {
        assert_eq!(Lang::detect(Path::new("a.dart")).map(|l| l.id), Some("generic"));
        assert_eq!(Lang::detect(Path::new("a.rs")).map(|l| l.id), Some("rust"));
        assert!(Lang::detect(Path::new("notes.txt")).is_none() && Lang::detect(Path::new("Makefile")).is_none());
        assert_eq!(Lang::label(Lang::detect(Path::new("x.dart")), Some(Path::new("x.dart"))), "Dart");
        assert_eq!(Lang::label(None, None), "Plain Text");
        assert!(!highlight("final x = 1;", Lang::detect(Path::new("a.kt"))).spans[0].is_empty());
    }

    /// Colouring piece by piece (`Incremental`) gives exactly what
    /// colouring the whole file does, after every kind of edit, one after
    /// another (as typing goes): letters, new lines, an opened brace,
    /// quote or comment, a deleted character or line, a pasted block, undo.
    #[test]
    fn incremental_matches_whole_file() {
        let samples: &[(&str, &str)] = &[
            ("x.rs", include_str!("editor.rs")),
            ("x.rs", include_str!("keymap.rs")),
            ("x.rs", include_str!("fuzzy.rs")),
            ("x.go", "package main\n\nimport \"fmt\"\n\ntype Point struct {\n\tX, Y int\n}\n\nfunc (p Point) Sum() int {\n\treturn p.X + p.Y // add\n}\n\nfunc main() {\n\tp := Point{1, 2}\n\tfmt.Println(\"sum\", p.Sum())\n\tfor i := 0; i < 3; i++ {\n\t\tfmt.Println(i)\n\t}\n}\n"),
            ("x.c", "#include <stdio.h>\n\nstatic int add(int a, int b) {\n    return a + b; /* sum */\n}\n\nint main(void) {\n    char *s = \"hi\";\n    for (int i = 0; i < 3; i++) {\n        printf(\"%s %d\\n\", s, add(i, 1));\n    }\n    return 0;\n}\n"),
            ("x.cpp", "#include <vector>\n\nnamespace app {\nclass Box {\npublic:\n    int size() const { return n; }\nprivate:\n    int n = 0;\n};\n\nint total(const std::vector<int>& v) {\n    int t = 0;\n    for (auto x : v) { t += x; }\n    return t; // done\n}\n}\n"),
            ("x.java", "package a;\n\nimport java.util.List;\n\npublic class Shop {\n    private int count = 0;\n\n    public Shop(int c) {\n        this.count = c;\n    }\n\n    public String name() {\n        return \"shop\" + count; // label\n    }\n\n    static int sum(List<Integer> xs) {\n        int t = 0;\n        for (int x : xs) t += x;\n        return t;\n    }\n}\n"),
            // Coloured whole (no `units`): checked all the same.
            ("x.cs", "using System;\n\nnamespace App {\n    public class Shop {\n        private int count = 0;\n\n        public Shop(int c) {\n            count = c;\n        }\n\n        public string Name() {\n            return $\"shop {count}\"; // label\n        }\n    }\n}\n"),
            ("x.css", "body {\n  margin: 0;\n  color: #333;\n}\n\n@media (max-width: 600px) {\n  .a { display: none; }\n}\n\n.b:hover {\n  color: red; /* hot */\n}\n"),
        ];
        let edits: &[&str] = &["x", "\n", "{", "\"", "/*", "//", "fn a() {}\n", "}", "'", "#"];
        for (file, text) in samples {
            let lang = Lang::detect(Path::new(file)).unwrap();
            let mut text = text.to_string();
            let mut spans = highlight(&text, Some(lang)).spans;
            let Some(mut inc) = Incremental::new(lang, text.clone()) else { continue };
            let step = (text.len() / 40).max(1);
            let (mut n, mut pieces) = (0, 0);
            let mut at = 7;
            while at < text.len() {
                let before = text.clone();
                let at_char = (0..=at).rev().find(|&i| text.is_char_boundary(i)).unwrap();
                match n % 4 {
                    0 | 1 => text.insert_str(at_char, edits[n % edits.len()]),
                    // A character gone.
                    2 => {
                        if let Some(c) = text[at_char..].chars().next() {
                            text.replace_range(at_char..at_char + c.len_utf8(), "");
                        }
                    }
                    // Its line gone.
                    _ => {
                        let s = text[..at_char].rfind('\n').map_or(0, |i| i + 1);
                        let e = text[at_char..].find('\n').map_or(text.len(), |i| at_char + i + 1);
                        text.replace_range(s..e, "");
                    }
                }
                pieces += usize::from(inc.update(text.clone(), &mut spans));
                assert_eq!(spans, highlight(&text, Some(lang)).spans, "{file}: edit {n} at {at_char}");
                // Every few: undone, as ⌘Z would.
                if n % 5 == 4 {
                    text = before;
                    inc.update(text.clone(), &mut spans);
                    assert_eq!(spans, highlight(&text, Some(lang)).spans, "{file}: undo {n}");
                }
                n += 1;
                at += step;
            }
            eprintln!("{file}: {pieces} of {n} edits coloured just their piece");
        }
        // Typing, a character at a time, at the start of lines inside
        // bodies: the half-typed states don't parse.
        let sample = |f: &str| samples.iter().find(|s| s.0 == f).map(|s| s.1).unwrap();
        let typed: &[(&str, &str, &str)] = &[
            ("x.rs", include_str!("editor.rs"), "let total = items.iter().map(|x| x * 2).sum::<u32>(); // \"done\"\n"),
            ("x.rs", include_str!("fuzzy.rs"), "if let Some(x) = self.find('a') { return Err(format!(\"{x}\")); }\n"),
            ("x.rs", include_str!("recent.rs"), "#[test] fn t() { let r#raw = b'\\n'; }\n"),
            ("x.go", sample("x.go"), "total := add(p.X, 2) // \"x\"\n"),
            ("x.c", sample("x.c"), "int total = add(1, 2); /* x */\n"),
            ("x.cpp", sample("x.cpp"), "auto total = v.size() + 1; // x\n"),
            ("x.java", sample("x.java"), "int total = sum(List.of(1, 2)); // \"x\"\n"),
            ("x.css", sample("x.css"), "border: 1px solid #fff; /* x */\n"),
        ];
        for (file, text, line) in typed {
            let lang = Lang::detect(Path::new(file)).unwrap();
            let mut text = text.to_string();
            let mut spans = highlight(&text, Some(lang)).spans;
            let mut inc = Incremental::new(lang, text.clone()).unwrap();
            let (mut pieces, mut n) = (0, 0);
            // Lines that start with indentation: inside something.
            let indent = if *file == "x.css" { "\n  " } else if *file == "x.go" { "\n\t" } else { "\n    " };
            let starts: Vec<usize> = text.match_indices(indent).map(|(i, _)| i + indent.len()).step_by(3).take(6).collect();
            for start in starts.into_iter().rev() {
                let mut at = start;
                for c in line.chars() {
                    text.insert(at, c);
                    at += c.len_utf8();
                    pieces += usize::from(inc.update(text.clone(), &mut spans));
                    n += 1;
                    assert_eq!(spans, highlight(&text, Some(lang)).spans, "{file}: typing {:?} at {start}", &text[start..at]);
                }
            }
            eprintln!("{file}: {pieces} of {n} keystrokes coloured just their piece");
        }
    }

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
                    .map(move |(r, t)| (line[r.start as usize..r.end as usize].to_string(), *t))
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

    /// How long a keystroke's colouring takes, piece by piece and whole
    /// (`cargo test --release -- --ignored --nocapture keystroke_cost`).
    #[test]
    #[ignore]
    fn keystroke_cost() {
        let lang = Lang::detect(Path::new("x.rs")).unwrap();
        let full: String = include_str!("ui.rs").lines().take(2000).collect::<Vec<_>>().join("\n");
        let mut text = full.clone();
        let mut spans = highlight(&text, Some(lang)).spans;
        let mut inc = Incremental::new(lang, text.clone()).unwrap();
        let at = text.match_indices("\n        ").nth(400).unwrap().0 + 9;
        let (mut piece, mut whole) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for (i, c) in "let total = 1;".chars().enumerate() {
            text.insert(at + i, c);
            let t = std::time::Instant::now();
            inc.update(text.clone(), &mut spans);
            piece += t.elapsed();
            let t = std::time::Instant::now();
            let _ = highlight(&text, Some(lang));
            whole += t.elapsed();
        }
        eprintln!("2000-line Rust file, per keystroke: piece {:?}, whole {:?}", piece / 14, whole / 14);
    }
}
