// Unit tests of a repo: which runners it has (cargo, CTest, vitest, jest),
// listing their tests and running them, with parsers for their output.
use crate::agent::runner::{self, Line};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cargo,
    Ctest,
    Vitest,
    Jest,
}

/// One runner of a repo.
#[derive(Clone, Debug)]
pub struct Project {
    pub kind: Kind,
    /// "cargo", "ctest build/linux-debug", "vitest".
    pub label: String,
    /// Where its commands run (CTest: the build folder).
    pub dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    /// Unique in its project.
    pub id: String,
    /// The group it shows under: a Rust module, a gtest suite, a JS file.
    pub suite: String,
    pub label: String,
    /// What the runner's filter takes: libtest's or CTest's name, a JS test's title.
    pub name: String,
    pub file: Option<String>,
    /// "path:line", relative to the repo (CTest's DEF_SOURCE_LINE, vitest's location).
    pub location: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub case: Case,
    /// None while only output has been seen.
    pub status: Option<Status>,
    pub output: Vec<String>,
    pub message: Option<String>,
}

pub struct RunEnd {
    pub success: bool,
    pub cancelled: bool,
    /// The JSON report vitest and jest write.
    pub report: Option<String>,
}

/// The runners found in `root`; CTest's build folder is the preset's (`run::cmake_build_dir`).
pub fn detect(root: &Path, preset: Option<&str>) -> Vec<Project> {
    let mut out = vec![];
    if root.join("Cargo.toml").is_file() {
        out.push(Project { kind: Kind::Cargo, label: "cargo".into(), dir: root.to_path_buf() });
    }
    if root.join("CMakeLists.txt").is_file() {
        if let Some(dir) = crate::run::cmake_build_dir(root, preset) {
            let shown = dir.strip_prefix(root).unwrap_or(&dir).display().to_string();
            out.push(Project { kind: Kind::Ctest, label: format!("ctest {shown}"), dir });
        }
    }
    let pkg: Value = std::fs::read_to_string(root.join("package.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let has = |dep: &str| ["dependencies", "devDependencies"].iter().any(|k| pkg.get(k).and_then(|d| d.get(dep)).is_some());
    for (dep, kind) in [("vitest", Kind::Vitest), ("jest", Kind::Jest)] {
        if has(dep) {
            out.push(Project { kind, label: dep.into(), dir: root.to_path_buf() });
        }
    }
    out
}

/// `program args…` with stderr merged into stdout, so cargo's "Running" lines
/// arrive in order with the tests they introduce.
fn merged(program: &str, args: &[String]) -> Command {
    #[cfg(unix)]
    {
        let mut c = crate::proc::cmd("sh");
        c.args(["-c", "exec \"$0\" \"$@\" 2>&1", program]).args(args);
        c
    }
    #[cfg(not(unix))]
    {
        let mut c = crate::proc::cmd(program);
        c.args(args);
        c
    }
}

/// The package manager's runner for a JS tool: `pnpm exec`, `yarn`, `npx`.
fn js(dir: &Path, tool: &str, args: Vec<String>) -> Command {
    let (pm, pre): (&str, &[&str]) = if dir.join("pnpm-lock.yaml").is_file() {
        ("pnpm", &["exec"])
    } else if dir.join("yarn.lock").is_file() {
        ("yarn", &[])
    } else {
        ("npx", &["--no-install"])
    };
    let all: Vec<String> = pre.iter().map(|s| s.to_string()).chain([tool.to_string()]).chain(args).collect();
    merged(pm, &all)
}

fn report_path(key: &str) -> PathBuf {
    let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    std::env::temp_dir().join(format!("orbit-tests-{}-{safe}.json", std::process::id()))
}

/// Runs `cmd`, streaming its lines; cancellable with `cancel(key)`.
fn stream(cmd: Command, dir: &Path, key: &str, mut on_line: impl FnMut(String)) -> Result<(bool, bool), String> {
    let o = runner::run_streaming(cmd, dir, TIMEOUT, Some(&format!("tests:{key}")), |l| match l {
        Line::Out(s) | Line::Err(s) => on_line(s),
    })?;
    Ok((o.success, o.cancelled))
}

pub fn cancel(key: &str) -> bool {
    runner::cancel(&format!("tests:{key}"))
}

/// Lists a project's tests (cargo builds them first). Jest can't list tests, only files: none.
pub fn list(p: &Project, key: &str) -> Result<Vec<Case>, String> {
    let mut lines = vec![];
    let report = report_path(key);
    let cmd = match p.kind {
        Kind::Cargo => merged("cargo", &args(&["test", "--workspace", "--", "--list", "--format", "terse"])),
        Kind::Ctest => {
            if !p.dir.join("CTestTestfile.cmake").is_file() {
                return Err(format!("{} has no CTest tests yet: build the project first", p.dir.display()));
            }
            merged("ctest", &args(&["--show-only=json-v1"]))
        }
        Kind::Vitest => js(&p.dir, "vitest", vec!["list".into(), format!("--json={}", report.display())]),
        Kind::Jest => return Ok(vec![]),
    };
    let (ok, cancelled) = stream(cmd, &p.dir, key, |l| lines.push(l))?;
    if cancelled {
        return Err("cancelled".into());
    }
    let text = lines.join("\n");
    let tail = || lines.iter().rev().take(12).rev().cloned().collect::<Vec<_>>().join("\n");
    match p.kind {
        Kind::Cargo if ok => Ok(parse_cargo_list(&text)),
        Kind::Ctest if ok => parse_ctest_list(&text).ok_or_else(|| "ctest printed no test list".into()),
        Kind::Vitest => {
            let raw = std::fs::read_to_string(&report).ok();
            let _ = std::fs::remove_file(&report);
            match raw.as_deref().and_then(|r| parse_vitest_list(r, &p.dir)) {
                Some(cases) => Ok(cases),
                None => Err(tail()),
            }
        }
        _ => Err(tail()),
    }
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Escapes regex metacharacters (CMake's and JavaScript's).
fn escape(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if "\\^$.|?*+()[]{}/".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The command running `cases` (all when None).
pub fn run_command(p: &Project, cases: Option<&[Case]>, report: &Path) -> Command {
    match p.kind {
        Kind::Cargo => {
            let mut a = args(&["test", "--workspace", "--no-fail-fast", "--", "--show-output"]);
            if let Some(cases) = cases {
                a.push("--exact".into());
                let mut names: Vec<String> = cases.iter().map(|c| c.name.clone()).collect();
                names.dedup();
                a.extend(names);
            }
            merged("cargo", &a)
        }
        Kind::Ctest => {
            let jobs = std::thread::available_parallelism().map_or(1, |n| n.get());
            let mut a = vec!["-V".to_string(), "-j".into(), jobs.to_string()];
            if let Some(cases) = cases {
                let names: Vec<String> = cases.iter().map(|c| escape(&c.name)).collect();
                a.extend(["-R".into(), format!("^({})$", names.join("|"))]);
            }
            merged("ctest", &a)
        }
        Kind::Vitest | Kind::Jest => {
            let vitest = p.kind == Kind::Vitest;
            let mut a: Vec<String> = if vitest {
                args(&["run", "--reporter=default", "--reporter=json"]).into_iter().chain([format!("--outputFile.json={}", report.display())]).collect()
            } else {
                vec!["--json".into(), format!("--outputFile={}", report.display())]
            };
            if let Some(cases) = cases {
                let mut files: Vec<String> = cases.iter().filter_map(|c| c.file.clone()).collect();
                files.sort();
                files.dedup();
                a.extend(files);
                let mut titles: Vec<String> = cases.iter().filter(|c| !c.name.is_empty()).map(|c| escape(&c.name)).collect();
                titles.dedup();
                if !titles.is_empty() {
                    a.extend(["-t".into(), titles.join("|")]);
                }
            }
            js(&p.dir, if vitest { "vitest" } else { "jest" }, a)
        }
    }
}

/// Runs `cases` (all when None), streaming the output; vitest's and jest's
/// results come back as their JSON report.
pub fn run(p: &Project, cases: Option<&[Case]>, key: &str, on_line: impl FnMut(String)) -> Result<RunEnd, String> {
    let report = report_path(key);
    let _ = std::fs::remove_file(&report);
    let (success, cancelled) = stream(run_command(p, cases, &report), &p.dir, key, on_line)?;
    let text = std::fs::read_to_string(&report).ok();
    let _ = std::fs::remove_file(&report);
    Ok(RunEnd { success, cancelled, report: text })
}

// ---------- listing ----------

/// `Running unittests src/lib.rs (target/debug/deps/orbit_core-1a2b)` → `orbit_core`;
/// `Doc-tests orbit_core` → `orbit_core`.
fn cargo_binary(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(name) = t.strip_prefix("Doc-tests ") {
        return Some(name.trim().to_string());
    }
    let inner = t.strip_prefix("Running ")?.rsplit_once('(')?.1.strip_suffix(')')?;
    let file = Path::new(inner).file_stem()?.to_string_lossy().to_string();
    Some(file.rsplit_once('-').map_or(file.clone(), |(n, _)| n.to_string()))
}

fn cargo_case(binary: &str, name: &str) -> Case {
    // Doc tests: "src/lib.rs - module::item (line 12)".
    let (suite, label) = if name.contains(" - ") && name.ends_with(')') {
        (format!("{binary} doc-tests"), name.to_string())
    } else {
        match name.rsplit_once("::") {
            Some((module, test)) => (format!("{binary}::{module}"), test.to_string()),
            None => (binary.to_string(), name.to_string()),
        }
    };
    Case { id: format!("{binary}::{name}"), suite, label, name: name.to_string(), file: None, location: None }
}

pub fn parse_cargo_list(text: &str) -> Vec<Case> {
    let mut binary = String::from("tests");
    let mut out = vec![];
    for line in text.lines() {
        if let Some(b) = cargo_binary(line) {
            binary = b;
        } else if let Some(name) = line.strip_suffix(": test") {
            out.push(cargo_case(&binary, name));
        }
    }
    out
}

/// gtest's `Suite.Test` groups by suite; other tests by their executable.
fn ctest_suite(name: &str, command: Option<&str>) -> String {
    match name.split_once('.') {
        Some((suite, rest)) if !suite.is_empty() && !rest.is_empty() && !suite.contains(' ') => suite.to_string(),
        _ => command.and_then(|c| Path::new(c).file_stem()).map_or("tests".into(), |s| s.to_string_lossy().to_string()),
    }
}

fn ctest_case(name: &str, command: Option<&str>, location: Option<String>) -> Case {
    let suite = ctest_suite(name, command);
    let label = name.strip_prefix(&format!("{suite}.")).unwrap_or(name).to_string();
    Case { id: name.to_string(), suite, label, name: name.to_string(), file: None, location }
}

/// `ctest --show-only=json-v1`.
pub fn parse_ctest_list(text: &str) -> Option<Vec<Case>> {
    let doc: Value = serde_json::from_str(&text[text.find('{')?..]).ok()?;
    let tests = doc.get("tests")?.as_array()?;
    Some(
        tests
            .iter()
            .filter_map(|t| {
                let name = t.get("name")?.as_str()?;
                let command = t.pointer("/command/0").and_then(Value::as_str);
                let props = t.get("properties").and_then(Value::as_array);
                let location = props
                    .into_iter()
                    .flatten()
                    .find(|p| p.get("name").and_then(Value::as_str) == Some("DEF_SOURCE_LINE"))
                    .and_then(|p| p.get("value")?.as_str().map(str::to_string));
                Some(ctest_case(name, command, location))
            })
            .collect(),
    )
}

fn js_case(dir: &Path, file: &str, path: &[&str]) -> Case {
    let suite = Path::new(file).strip_prefix(dir).map_or(file.to_string(), |p| p.display().to_string());
    let label = path.join(" > ");
    Case {
        id: format!("{file} > {label}"),
        suite,
        label,
        name: path.last().map_or(String::new(), |s| s.to_string()),
        file: Some(file.to_string()),
        location: None,
    }
}

/// `vitest list --json`: `[{"name": "suite > test", "file": "/abs/a.test.ts", "location": {"line": 4}}]`.
pub fn parse_vitest_list(text: &str, dir: &Path) -> Option<Vec<Case>> {
    let list: Vec<Value> = serde_json::from_str(text).ok()?;
    Some(
        list.iter()
            .filter_map(|t| {
                let (name, file) = (t.get("name")?.as_str()?, t.get("file")?.as_str()?);
                let mut case = js_case(dir, file, &name.split(" > ").collect::<Vec<_>>());
                case.location = t.pointer("/location/line").and_then(Value::as_u64).map(|line| format!("{}:{line}", case.suite));
                Some(case)
            })
            .collect(),
    )
}

// ---------- results ----------

/// Reads a run's output line by line; `results` grows as tests finish.
pub struct Parser {
    kind: Kind,
    binary: String,
    /// The test whose captured output is being read (cargo's `---- name stdout ----`).
    block: Option<usize>,
    /// CTest's test numbers.
    numbers: HashMap<u32, String>,
    pub results: Vec<Outcome>,
    index: HashMap<String, usize>,
}

impl Parser {
    pub fn new(kind: Kind) -> Self {
        Self { kind, binary: "tests".into(), block: None, numbers: HashMap::new(), results: vec![], index: HashMap::new() }
    }

    fn entry(&mut self, case: Case) -> usize {
        if let Some(&ix) = self.index.get(&case.id) {
            return ix;
        }
        self.index.insert(case.id.clone(), self.results.len());
        self.results.push(Outcome { case, status: None, output: vec![], message: None });
        self.results.len() - 1
    }

    pub fn feed(&mut self, line: &str) {
        match self.kind {
            Kind::Cargo => self.feed_cargo(line),
            Kind::Ctest => self.feed_ctest(line),
            Kind::Vitest | Kind::Jest => {}
        }
    }

    fn feed_cargo(&mut self, line: &str) {
        if let Some(b) = cargo_binary(line) {
            self.binary = b;
            self.block = None;
            return;
        }
        if let Some(name) = line.strip_prefix("---- ").and_then(|l| l.strip_suffix(" stdout ----")) {
            let case = cargo_case(&self.binary, name);
            self.block = Some(self.entry(case));
            return;
        }
        if let Some(rest) = line.strip_prefix("test ") {
            if let Some((name, result)) = rest.rsplit_once(" ... ") {
                let status = match result.split(',').next().unwrap_or_default().trim() {
                    "ok" => Status::Passed,
                    "FAILED" => Status::Failed,
                    "ignored" => Status::Skipped,
                    _ => return,
                };
                let ix = self.entry(cargo_case(&self.binary, name));
                self.results[ix].status = Some(status);
                return;
            }
        }
        if matches!(line, "failures:" | "successes:") || line.starts_with("test result:") {
            self.block = None;
        } else if let Some(ix) = self.block {
            self.results[ix].output.push(line.to_string());
        }
    }

    fn feed_ctest(&mut self, line: &str) {
        let t = line.trim_start();
        // "Start 12: name"
        if let Some((n, name)) = t.strip_prefix("Start ").and_then(|r| r.trim_start().split_once(": ")) {
            if let Ok(n) = n.parse() {
                self.numbers.insert(n, name.trim().to_string());
                return;
            }
        }
        // "3/10 Test  #3: name ...........***Failed    0.01 sec"
        if let Some((_, rest)) = t.split_once(" Test ").filter(|(head, _)| head.contains('/') && head.chars().all(|c| c.is_ascii_digit() || c == '/')) {
            let Some((_, rest)) = rest.trim_start().strip_prefix('#').and_then(|r| r.split_once(": ")) else { return };
            let Some(dots) = rest.find(" ..").or_else(|| rest.find(" .")) else { return };
            let name = rest[..dots].trim();
            let verdict = rest[dots..].trim_start_matches([' ', '.']).trim_start_matches('*');
            let status = if verdict.starts_with("Passed") {
                Status::Passed
            } else if verdict.starts_with("Skipped") || verdict.starts_with("Not Run") || verdict.starts_with("Disabled") {
                Status::Skipped
            } else {
                Status::Failed
            };
            let ix = self.entry(ctest_case(name, None, None));
            self.results[ix].status = Some(status);
            if status == Status::Failed {
                let why = verdict.split("  ").next().unwrap_or(verdict).trim();
                self.results[ix].message = Some(why.to_string());
            }
            return;
        }
        // "12: output of test 12"
        if let Some((n, text)) = line.split_once(": ") {
            if let Some(name) = n.parse::<u32>().ok().and_then(|n| self.numbers.get(&n)).cloned() {
                let ix = self.entry(ctest_case(&name, None, None));
                self.results[ix].output.push(text.to_string());
            }
        }
    }

    /// vitest's and jest's JSON report (jest's format).
    pub fn report(&mut self, text: &str, dir: &Path) {
        let Ok(doc) = serde_json::from_str::<Value>(text) else { return };
        for file in doc.get("testResults").and_then(Value::as_array).into_iter().flatten() {
            let name = file.get("name").and_then(Value::as_str).unwrap_or_default();
            for a in file.get("assertionResults").and_then(Value::as_array).into_iter().flatten() {
                let mut path: Vec<&str> = a.get("ancestorTitles").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                path.push(a.get("title").and_then(Value::as_str).unwrap_or_default());
                let status = match a.get("status").and_then(Value::as_str) {
                    Some("passed") => Status::Passed,
                    Some("failed") => Status::Failed,
                    _ => Status::Skipped,
                };
                let messages: Vec<&str> = a.get("failureMessages").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                let ix = self.entry(js_case(dir, name, &path));
                let r = &mut self.results[ix];
                r.status = Some(status);
                if !messages.is_empty() {
                    r.message = Some(messages.join("\n"));
                }
            }
            // A file that failed to load has no assertions, only a message.
            let message = file.get("message").and_then(Value::as_str).unwrap_or_default();
            if !message.is_empty() && file.get("assertionResults").and_then(Value::as_array).is_none_or(|a| a.is_empty()) {
                let ix = self.entry(js_case(dir, name, &["(file)"]));
                self.results[ix].status = Some(Status::Failed);
                self.results[ix].message = Some(message.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statuses(p: &Parser) -> Vec<(&str, Option<Status>)> {
        p.results.iter().map(|r| (r.case.id.as_str(), r.status)).collect()
    }

    #[test]
    fn cargo_list_groups_by_binary_and_module() {
        let text = "   Compiling orbit-core v0.1.0\n     Running unittests src/lib.rs (target/debug/deps/orbit_core-4f1e2a)\nrun::tests::merges: test\ntop_level: test\n     Running tests/cli.rs (target/debug/deps/cli-99aa)\nstarts: test\n   Doc-tests orbit_core\nsrc/lib.rs - run::setup (line 12): test\n";
        let cases = parse_cargo_list(text);
        let got: Vec<_> = cases.iter().map(|c| (c.id.as_str(), c.suite.as_str(), c.label.as_str(), c.name.as_str())).collect();
        assert_eq!(
            got,
            [
                ("orbit_core::run::tests::merges", "orbit_core::run::tests", "merges", "run::tests::merges"),
                ("orbit_core::top_level", "orbit_core", "top_level", "top_level"),
                ("cli::starts", "cli", "starts", "starts"),
                ("orbit_core::src/lib.rs - run::setup (line 12)", "orbit_core doc-tests", "src/lib.rs - run::setup (line 12)", "src/lib.rs - run::setup (line 12)"),
            ]
        );
    }

    #[test]
    fn cargo_run_reads_statuses_and_captured_output() {
        let text = "\
     Running unittests src/lib.rs (target/debug/deps/core-1)

running 3 tests
test a::ok_one ... ok
test a::bad ... FAILED
test a::later ... ignored, needs a server

successes:

---- a::ok_one stdout ----
hello from ok

successes:
    a::ok_one

failures:

---- a::bad stdout ----
thread 'a::bad' panicked at src/a.rs:3:5:
assertion `left == right` failed

failures:
    a::bad

test result: FAILED. 1 passed; 1 failed; 1 ignored
     Running unittests src/main.rs (target/debug/deps/app-2)
test a::bad ... ok
";
        let mut p = Parser::new(Kind::Cargo);
        text.lines().for_each(|l| p.feed(l));
        assert_eq!(
            statuses(&p),
            [
                ("core::a::ok_one", Some(Status::Passed)),
                ("core::a::bad", Some(Status::Failed)),
                ("core::a::later", Some(Status::Skipped)),
                ("app::a::bad", Some(Status::Passed)),
            ]
        );
        assert_eq!(p.results[0].output, ["hello from ok", ""]);
        assert_eq!(p.results[1].output, ["thread 'a::bad' panicked at src/a.rs:3:5:", "assertion `left == right` failed", ""]);
    }

    #[test]
    fn ctest_list_and_verbose_run() {
        let json = r#"{"kind":"ctestInfo","tests":[
            {"name":"Audio.Decodes","command":["/b/test/game/game_test","--gtest_filter=Audio.Decodes"],"properties":[{"name":"DEF_SOURCE_LINE","value":"test/game/audio.cxx:8"}]},
            {"name":"smoke","command":["/b/tools/smoke_check"]}]}"#;
        let cases = parse_ctest_list(json).unwrap();
        assert_eq!((cases[0].suite.as_str(), cases[0].label.as_str(), cases[0].location.as_deref()), ("Audio", "Decodes", Some("test/game/audio.cxx:8")));
        assert_eq!((cases[1].suite.as_str(), cases[1].label.as_str()), ("smoke_check", "smoke"));

        let run = "\
UpdateCTestConfiguration  from :/b/DartConfiguration.tcl
test 1
      Start  1: Audio.Decodes
1: Test command: /b/test/game/game_test \"--gtest_filter=Audio.Decodes\"
1: [ RUN      ] Audio.Decodes
1: [       OK ] Audio.Decodes (0 ms)
 1/3 Test  #1: Audio.Decodes ....................   Passed    0.01 sec
      Start  2: smoke
2: boom
 2/3 Test  #2: smoke ............................***Failed    0.02 sec
      Start 10: x.y
3/3 Test #10: x.y ..............................***Skipped   0.00 sec
";
        let mut p = Parser::new(Kind::Ctest);
        run.lines().for_each(|l| p.feed(l));
        assert_eq!(statuses(&p), [("Audio.Decodes", Some(Status::Passed)), ("smoke", Some(Status::Failed)), ("x.y", Some(Status::Skipped))]);
        assert_eq!(p.results[0].output.len(), 3);
        assert_eq!(p.results[1].output, ["boom"]);
        assert_eq!(p.results[1].message.as_deref(), Some("Failed"));
    }

    #[test]
    fn vitest_list_and_json_report() {
        let dir = Path::new("/w/web");
        let cases = parse_vitest_list(r#"[{"name":"math > adds","file":"/w/web/src/math.test.ts","location":{"line":4,"column":3}},{"name":"top","file":"/w/web/a.test.ts"}]"#, dir).unwrap();
        assert_eq!((cases[0].id.as_str(), cases[0].suite.as_str(), cases[0].label.as_str(), cases[0].name.as_str()), ("/w/web/src/math.test.ts > math > adds", "src/math.test.ts", "math > adds", "adds"));
        assert_eq!(cases[0].location.as_deref(), Some("src/math.test.ts:4"));
        assert_eq!(cases[1].label, "top");

        let report = r#"{"testResults":[
            {"name":"/w/web/src/math.test.ts","assertionResults":[
                {"ancestorTitles":["math"],"title":"adds","status":"passed","failureMessages":[]},
                {"ancestorTitles":["math"],"title":"divides","status":"failed","failureMessages":["AssertionError: expected 1 to be 2"]},
                {"ancestorTitles":[],"title":"later","status":"todo","failureMessages":[]}]},
            {"name":"/w/web/broken.test.ts","assertionResults":[],"message":"SyntaxError: x"}]}"#;
        let mut p = Parser::new(Kind::Vitest);
        p.report(report, dir);
        assert_eq!(
            statuses(&p),
            [
                ("/w/web/src/math.test.ts > math > adds", Some(Status::Passed)),
                ("/w/web/src/math.test.ts > math > divides", Some(Status::Failed)),
                ("/w/web/src/math.test.ts > later", Some(Status::Skipped)),
                ("/w/web/broken.test.ts > (file)", Some(Status::Failed)),
            ]
        );
        assert_eq!(p.results[1].message.as_deref(), Some("AssertionError: expected 1 to be 2"));
    }

    #[test]
    fn filters_select_the_cases() {
        let case = |name: &str, file: Option<&str>| Case { id: name.into(), suite: String::new(), label: name.into(), name: name.into(), file: file.map(str::to_string), location: None };
        let report = Path::new("/tmp/r.json");
        let line = |c: Command| c.get_args().map(|a| a.to_string_lossy().to_string()).collect::<Vec<_>>().join(" ");
        let p = |kind| Project { kind, label: String::new(), dir: PathBuf::from("/nowhere") };
        let cargo = line(run_command(&p(Kind::Cargo), Some(&[case("a::b", None)]), report));
        assert!(cargo.ends_with("cargo test --workspace --no-fail-fast -- --show-output --exact a::b"), "{cargo}");
        let ctest = line(run_command(&p(Kind::Ctest), Some(&[case("A.b", None), case("c(1)", None)]), report));
        assert!(ctest.ends_with(r"-R ^(A\.b|c\(1\))$"), "{ctest}");
        let vitest = line(run_command(&p(Kind::Vitest), Some(&[case("adds", Some("/w/a.test.ts"))]), report));
        assert!(vitest.ends_with("--outputFile.json=/tmp/r.json /w/a.test.ts -t adds"), "{vitest}");
    }
}
