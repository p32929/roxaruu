//! Format Document through the formatter you already use for a language
//! (rustfmt, ruff, biome, taplo — mostly Rust-written — gofmt, clang-format,
//! shfmt, prettier). The text goes in on stdin and comes back on stdout, so
//! unsaved edits are formatted too. Which command runs for which extension
//! is `"formatters"` in settings.json; updating a formatter is updating the
//! tool (`brew upgrade`, `rustup update`), nothing in here.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Built-in commands by extension; `{file}` becomes the file's path (so
/// tools that care about the name, like biome and prettier, know the
/// language). A user's `"formatters"` entry for an extension replaces these;
/// an empty string turns formatting off for it.
const DEFAULTS: &[(&[&str], &str)] = &[
    (&["rs"], "rustfmt --edition 2024"),
    (&["py", "pyi"], "ruff format --stdin-filename {file} -"),
    (
        &["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "json", "jsonc", "css"],
        "biome format --stdin-file-path={file}",
    ),
    (&["toml"], "taplo fmt -"),
    (&["go"], "gofmt"),
    (&["c", "h", "cc", "cpp", "hpp"], "clang-format --assume-filename={file}"),
    (&["sh", "bash", "zsh"], "shfmt -filename {file}"),
    (&["html", "htm", "md", "markdown", "yaml", "yml", "scss", "less"], "prettier --stdin-filepath {file}"),
];

/// How to install each built-in formatter, for the "not installed" note.
const INSTALL: &[(&str, &str)] = &[
    ("rustfmt", "rustup component add rustfmt"),
    ("ruff", "brew install ruff"),
    ("biome", "brew install biome"),
    ("taplo", "brew install taplo"),
    ("gofmt", "brew install go"),
    ("clang-format", "brew install clang-format"),
    ("shfmt", "brew install shfmt"),
    ("prettier", "npm install -g prettier"),
];

/// A formatter taking longer than this is stopped (the text is left alone).
const TIMEOUT: Duration = Duration::from_secs(10);

/// The command that formats `path`: the user's entry for its extension,
/// else the built-in one. `None` when there's none (or it's turned off).
pub fn command_for(user: &BTreeMap<String, String>, path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let cmd = match user.get(&ext) {
        Some(c) => c.clone(),
        None => DEFAULTS
            .iter()
            .find(|(exts, _)| exts.contains(&ext.as_str()))?
            .1
            .to_string(),
    };
    (!cmd.trim().is_empty()).then_some(cmd)
}

/// `cmd` as program + arguments, with `{file}` filled in. Split on spaces
/// only (no shell), so a path with spaces stays one argument.
fn argv(cmd: &str, path: &Path) -> Vec<String> {
    let file = path.to_string_lossy();
    cmd.split_whitespace()
        .map(|a| a.replace("{file}", &file))
        .collect()
}

/// PATH plus where formatters usually live: an app opened from the Dock
/// gets only the system folders.
fn search_path() -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let mut dirs: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .map(String::from)
        .collect();
    for extra in [
        home.join(".cargo/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        home.join(".local/bin"),
        home.join("go/bin"),
        "/usr/local/go/bin".into(),
    ] {
        let extra = extra.to_string_lossy().into_owned();
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    dirs.join(":")
}

/// Run `cmd` on `text` (the contents of `path`) and return the formatted
/// text, or a message saying what went wrong.
pub fn run(cmd: &str, path: &Path, text: &str) -> Result<String, String> {
    let args = argv(cmd, path);
    let Some((program, rest)) = args.split_first() else {
        return Err("the formatter command is empty".into());
    };
    let mut child = Command::new(program)
        .args(rest)
        .env("PATH", search_path())
        .current_dir(path.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => not_installed(program),
            _ => format!("Couldn't run {program}: {e}"),
        })?;
    // Write and read on their own threads: a big file would fill a pipe
    // and stall both sides.
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        return Err(format!("Couldn't talk to {program}"));
    };
    let input = text.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes()); // dropped: EOF
    });
    let read = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = r.read_to_end(&mut out);
            out
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} took too long; nothing changed"));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let _ = writer.join();
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if !status.success() {
        let why = String::from_utf8_lossy(&err);
        let why = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("it failed");
        return Err(format!("{program}: {}", why.trim()));
    }
    let formatted = String::from_utf8(out).map_err(|_| format!("{program} gave back non-UTF-8 text"))?;
    // A formatter that prints nothing for non-empty input didn't format.
    if formatted.is_empty() && !text.trim().is_empty() {
        return Err(format!("{program} returned nothing; nothing changed"));
    }
    Ok(formatted)
}

fn not_installed(program: &str) -> String {
    let name = Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string());
    match INSTALL.iter().find(|(tool, _)| *tool == name) {
        Some((_, how)) => format!("{name} isn't installed — install it with: {how}"),
        None => format!("{name} isn't installed (or isn't on PATH)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_command_by_extension() {
        let none = BTreeMap::new();
        let p = Path::new("/x/main.RS");
        assert_eq!(command_for(&none, p).as_deref(), Some("rustfmt --edition 2024"));
        assert!(command_for(&none, Path::new("notes.txt")).is_none());
        assert!(command_for(&none, Path::new("Makefile")).is_none());
        // Yours wins; an empty one turns it off; new extensions work too.
        let mut mine = BTreeMap::new();
        mine.insert("rs".to_string(), "rustfmt --edition 2021".to_string());
        mine.insert("py".to_string(), " ".to_string());
        mine.insert("txt".to_string(), "fmt -w 80".to_string());
        assert_eq!(command_for(&mine, p).as_deref(), Some("rustfmt --edition 2021"));
        assert!(command_for(&mine, Path::new("a.py")).is_none());
        assert_eq!(command_for(&mine, Path::new("a.txt")).as_deref(), Some("fmt -w 80"));
    }

    #[test]
    fn file_placeholder_stays_one_argument() {
        let a = argv("biome format --stdin-file-path={file}", Path::new("/my dir/a b.ts"));
        assert_eq!(a, ["biome", "format", "--stdin-file-path=/my dir/a b.ts"]);
    }

    #[test]
    fn every_default_tool_says_how_to_install_it() {
        for (_, cmd) in DEFAULTS {
            let tool = cmd.split_whitespace().next().unwrap();
            assert!(INSTALL.iter().any(|(t, _)| *t == tool), "{tool}");
        }
    }

    #[test]
    fn runs_through_stdin_and_reports_failures() {
        let p = Path::new("/tmp/x.txt");
        // `tr` stands in for a formatter: it reads stdin, writes stdout.
        assert_eq!(run("tr a-z A-Z", p, "hello\n").unwrap(), "HELLO\n");
        // A big file doesn't stall on full pipes.
        let big = "x".repeat(1 << 20);
        assert_eq!(run("cat", p, &big).unwrap().len(), big.len());
        let missing = run("no-such-formatter-xyz", p, "a").unwrap_err();
        assert!(missing.contains("isn't installed"), "{missing}");
        let failed = run("sh -c exit_3", p, "a").unwrap_err();
        assert!(failed.starts_with("sh:"), "{failed}");
        assert!(run("true", p, "text").unwrap_err().contains("returned nothing"));
        assert!(not_installed("rustfmt").contains("rustup component add rustfmt"));
    }
}
