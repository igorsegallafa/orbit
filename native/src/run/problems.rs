//! Compiler and linker messages in build output (`src/lib/problems.ts`):
//! GCC/Clang ("file:line:col: error: msg"), MSVC ("file(line,col): error
//! C2065: msg") and GNU ld ("undefined reference to …").
use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// Absolute path, or as printed when it can't be made absolute; None for
    /// messages without a location (linker errors).
    pub file: Option<String>,
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub message: String,
    /// e.g. "C2065", "-Wunused-variable".
    pub code: Option<String>,
}

impl Problem {
    /// Identity for de-duplication: a header's warning repeats for every file that includes it.
    pub fn key(&self) -> String {
        format!("{:?}:{}:{}:{:?}:{}", self.file, self.line, self.column, self.severity, self.message)
    }
}

fn re(s: &str) -> Regex {
    Regex::new(s).expect("valid regex")
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| re(r"\x1b\[[0-9;]*m"));
static GCC: LazyLock<Regex> = LazyLock::new(|| re(r"^(.*?):(\d+)(?::(\d+))?:\s+(fatal error|error|warning):\s+(.*)$"));
static MSVC: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(.*?)\((\d+)(?:,(\d+))?\)\s*:\s+(fatal error|error|warning)\s+([A-Z]+\d+)\s*:\s+(.*)$"));
static LD: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:^|\s)(?:/\S*/)?ld(?:\.\w+)?: (.*(?:undefined reference|multiple definition|cannot find).*)$"));
static LINK: LazyLock<Regex> =
    LazyLock::new(|| re(r"^\s*(?:LINK|\S+\.(?:obj|lib|exe|dll))\s*:\s+(fatal error|error|warning)\s+(LNK\d+)\s*:\s+(.*)$"));
static INCLUDED: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(In file included from|from)\b"));
static FLAG: LazyLock<Regex> = LazyLock::new(|| re(r"\s\[(-W[\w=+-]+)\]$"));
static ABSOLUTE: LazyLock<Regex> = LazyLock::new(|| re(r"^([A-Za-z]:[\\/]|/)"));

/// Colours removed (CMake passes them through from the compiler).
pub fn strip_ansi(s: &str) -> String {
    ANSI.replace_all(s, "").into_owned()
}

fn absolute(file: &str, base_dir: Option<&str>) -> String {
    let Some(base) = base_dir.filter(|_| !ABSOLUTE.is_match(file)) else { return file.to_string() };
    let sep = if base.contains('\\') { "\\" } else { "/" };
    let joined = format!("{base}{sep}{file}");
    let mut out: Vec<&str> = vec![];
    for p in joined.split(['\\', '/']) {
        if p == ".." {
            out.pop();
        } else if p != "." && (!p.is_empty() || out.is_empty()) {
            out.push(p);
        }
    }
    out.join(sep)
}

fn severity(s: &str) -> Severity {
    if s == "warning" {
        Severity::Warning
    } else {
        Severity::Error
    }
}

/// The problem a line reports, if any. Relative paths resolve against
/// `base_dir` (the build folder: Ninja prints paths relative to it).
pub fn parse(raw: &str, base_dir: Option<&str>) -> Option<Problem> {
    let line = strip_ansi(raw);
    let num = |m: Option<regex::Match>| m.and_then(|m| m.as_str().parse().ok()).unwrap_or(1);
    if let Some(m) = MSVC.captures(&line) {
        return Some(Problem {
            file: Some(absolute(m[1].trim(), base_dir)),
            line: num(m.get(2)),
            column: num(m.get(3)),
            severity: severity(&m[4]),
            code: Some(m[5].to_string()),
            message: m[6].trim().to_string(),
        });
    }
    if let Some(m) = GCC.captures(&line).filter(|m| !INCLUDED.is_match(&m[1])) {
        let text = &m[5];
        let flag = FLAG.captures(text);
        return Some(Problem {
            file: Some(absolute(&m[1], base_dir)),
            line: num(m.get(2)),
            column: num(m.get(3)),
            severity: severity(&m[4]),
            code: flag.as_ref().map(|f| f[1].to_string()),
            message: match &flag {
                Some(f) => text[..f.get(0).map_or(text.len(), |g| g.start())].to_string(),
                None => text.to_string(),
            },
        });
    }
    if let Some(m) = LINK.captures(&line) {
        return Some(Problem {
            file: None,
            line: 0,
            column: 0,
            severity: severity(&m[1]),
            code: Some(m[2].to_string()),
            message: m[3].trim().to_string(),
        });
    }
    LD.captures(&line).map(|m| Problem {
        file: None,
        line: 0,
        column: 0,
        severity: Severity::Error,
        code: None,
        message: m[1].trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gcc_and_clang_diagnostics() {
        assert_eq!(
            parse("/src/engine/src/game/Game.cpp:42:7: error: 'foo' was not declared in this scope", None),
            Some(Problem {
                file: Some("/src/engine/src/game/Game.cpp".into()),
                line: 42,
                column: 7,
                severity: Severity::Error,
                code: None,
                message: "'foo' was not declared in this scope".into(),
            })
        );
        let w = parse(
            "\x1b[1m../../src/game/Game.h:10:3: \x1b[35mwarning: \x1b[0munused variable 'x' [-Wunused-variable]",
            Some("/src/engine/build/linux"),
        )
        .unwrap();
        assert_eq!(w.file.as_deref(), Some("/src/engine/src/game/Game.h"));
        assert_eq!((w.line, w.column, w.severity), (10, 3, Severity::Warning));
        assert_eq!(w.code.as_deref(), Some("-Wunused-variable"));
        assert_eq!(w.message, "unused variable 'x'");
        let f = parse("x.cpp:3: fatal error: y.h: No such file or directory", None).unwrap();
        assert_eq!((f.line, f.column, f.severity), (3, 1, Severity::Error));
    }

    #[test]
    fn reads_msvc_diagnostics() {
        assert_eq!(
            parse(r"C:\src\engine\src\game\Game.cpp(42,7): error C2065: 'foo': undeclared identifier", None),
            Some(Problem {
                file: Some(r"C:\src\engine\src\game\Game.cpp".into()),
                line: 42,
                column: 7,
                severity: Severity::Error,
                code: Some("C2065".into()),
                message: "'foo': undeclared identifier".into(),
            })
        );
        let w = parse("  Game.cpp(9): warning C4996: 'strcpy': This function may be unsafe.", Some(r"C:\b")).unwrap();
        assert_eq!(w.file.as_deref(), Some(r"C:\b\Game.cpp"));
        assert_eq!((w.line, w.severity), (9, Severity::Warning));
    }

    #[test]
    fn reads_linker_errors_without_a_location() {
        let ld = parse("/usr/bin/ld: Game.cpp.o: in function `Game::Init()': undefined reference to `Foo::Bar()'", None).unwrap();
        assert_eq!((ld.file, ld.severity), (None, Severity::Error));
        let link = parse("LINK : fatal error LNK1104: cannot open file 'game.exe'", None).unwrap();
        assert_eq!((link.file, link.code.as_deref()), (None, Some("LNK1104")));
    }

    #[test]
    fn ignores_everything_else() {
        assert_eq!(parse("[12/340] Building CXX object src/game/CMakeFiles/game.dir/Game.cpp.o", None), None);
        assert_eq!(parse("In file included from /src/a.h:3:", None), None);
        assert_eq!(parse("/src/a.cpp:5:3: note: declared here", None), None);
    }

    #[test]
    fn keys_identify_duplicates() {
        let a = parse("/a.h:1:2: warning: w", None).unwrap();
        let b = parse("/a.h:1:2: warning: w", None).unwrap();
        assert_eq!(a.key(), b.key());
        assert_ne!(a.key(), parse("/a.h:1:3: warning: w", None).unwrap().key());
    }
}
