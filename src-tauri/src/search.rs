// Find in Files across a workspace's repos (IntelliJ-style). Each repo is a
// git worktree, so `git grep` does the work: .gitignore respected, untracked
// files included, binaries skipped, no extra dependency. Repos run in
// parallel; output is streamed and cut at a total cap so a broad query
// can't flood the UI.
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::Stdio;

const MAX_MATCHES: usize = 2000;
const MAX_LINE: usize = 400;

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Comma/space separated globs; `!glob` excludes (e.g. "*.ts, !*.test.ts").
    #[serde(default)]
    pub mask: String,
    /// Empty = every repo of the workspace.
    #[serde(default)]
    pub repos: Vec<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub repo: String,
    pub path: String,
    pub line: u64,
    /// 1-based column of the first match on the line (in `text` coordinates).
    pub col: u64,
    pub text: String,
}

#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

/// File mask → git pathspecs. Bare names match at any depth.
fn pathspecs(mask: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut positive = false;
    for tok in mask.split(|c: char| c == ',' || c.is_whitespace()).map(str::trim).filter(|t| !t.is_empty()) {
        let (exclude, glob) = match tok.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, tok),
        };
        if glob.is_empty() {
            continue;
        }
        let glob = if glob.contains('/') { glob.trim_start_matches('/').to_string() } else { format!("**/{glob}") };
        out.push(if exclude { format!(":(exclude,glob){glob}") } else { format!(":(glob){glob}") });
        positive |= !exclude;
    }
    // Excludes alone select nothing: pair them with "everything".
    if !out.is_empty() && !positive {
        out.insert(0, ".".into());
    }
    out
}

/// `path\0line\0col\0text` (git grep -z -n --column).
fn parse_line(repo: &str, raw: &[u8]) -> Option<SearchMatch> {
    let s = String::from_utf8_lossy(raw);
    let mut parts = s.splitn(4, '\0');
    let path = parts.next()?.to_string();
    let line = parts.next()?.parse().ok()?;
    let byte_col: usize = parts.next()?.parse().ok()?;
    let mut text = parts.next()?.trim_end_matches(['\r', '\n']).to_string();
    // git reports a byte offset; the UI works in characters.
    let mut col = text.as_bytes().get(..byte_col.saturating_sub(1)).map_or(1, |b| String::from_utf8_lossy(b).chars().count() as u64 + 1);
    // Minified files: keep a window around the match instead of the whole line.
    if text.chars().count() > MAX_LINE {
        let chars: Vec<char> = text.chars().collect();
        let start = (col as usize).saturating_sub(1).saturating_sub(MAX_LINE / 4).min(chars.len());
        let end = (start + MAX_LINE).min(chars.len());
        text = chars[start..end].iter().collect();
        col -= start as u64;
    }
    Some(SearchMatch { repo: repo.to_string(), path, line, col, text })
}

fn grep_repo(repo: &str, dir: &Path, q: &SearchQuery, specs: &[String], limit: usize) -> Result<(Vec<SearchMatch>, bool), String> {
    let mut args: Vec<String> = ["grep", "--untracked", "-n", "--column", "-I", "--no-color", "-z"].map(String::from).to_vec();
    if !q.case_sensitive {
        args.push("-i".into());
    }
    if q.whole_word {
        args.push("-w".into());
    }
    args.push(if q.regex { "-P".into() } else { "-F".into() });
    args.push("-e".into());
    args.push(q.query.clone());
    if !specs.is_empty() {
        args.push("--".into());
        args.extend(specs.iter().cloned());
    }
    let mut child = crate::proc::cmd("git")
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let mut matches = Vec::new();
    let mut truncated = false;
    if let Some(out) = child.stdout.take() {
        for raw in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            if matches.len() >= limit {
                truncated = true;
                break;
            }
            if let Some(m) = parse_line(repo, &raw) {
                matches.push(m);
            }
        }
    }
    if truncated {
        let _ = child.kill();
        let _ = child.wait();
        return Ok((matches, true));
    }
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    // 1 = no match; anything else with nothing found is a real error (bad regex...).
    if !status.success() && status.code() != Some(1) && matches.is_empty() {
        let msg = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("git grep failed").trim().trim_start_matches("fatal: ");
        // "-e option, '<pattern>': missing closing parenthesis" → the part that matters.
        return Err(match (msg.starts_with("-e option"), msg.rsplit_once("': ")) {
            (true, Some((_, why))) => format!("Invalid regex: {why}"),
            _ => msg.to_string(),
        });
    }
    Ok((matches, false))
}

pub fn search(workspace: &str, q: &SearchQuery) -> Result<SearchResult, String> {
    if q.query.is_empty() {
        return Ok(SearchResult::default());
    }
    let all = crate::workspace::scope_repos(workspace)?;
    let mut repos: Vec<(&String, std::path::PathBuf)> = Vec::new();
    for r in all.iter().filter(|r| q.repos.is_empty() || q.repos.contains(r)) {
        repos.push((r, crate::workspace::repo_path(workspace, r)?));
    }
    let specs = pathspecs(&q.mask);
    let results: Vec<Result<(Vec<SearchMatch>, bool), String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = repos
            .iter()
            .map(|(repo, dir)| {
                let specs = &specs;
                scope.spawn(move || if dir.exists() { grep_repo(repo, dir, q, specs, MAX_MATCHES) } else { Ok((vec![], false)) })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("search thread panicked".into()))).collect()
    });
    let mut out = SearchResult::default();
    for r in results {
        let (mut m, t) = r?;
        out.truncated |= t;
        out.matches.append(&mut m);
    }
    if out.matches.len() > MAX_MATCHES {
        out.matches.truncate(MAX_MATCHES);
        out.truncated = true;
    }
    Ok(out)
}

#[tauri::command]
pub async fn search_workspace(workspace: String, query: SearchQuery) -> Result<SearchResult, String> {
    tauri::async_runtime::spawn_blocking(move || search(&workspace, &query))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_become_pathspecs() {
        assert_eq!(pathspecs("*.ts, !*.test.ts"), [":(glob)**/*.ts", ":(exclude,glob)**/*.test.ts"]);
        assert_eq!(pathspecs("!dist/**"), [".", ":(exclude,glob)dist/**"]);
        assert!(pathspecs("  ").is_empty());
    }

    #[test]
    fn parses_grep_lines_and_windows_long_ones() {
        let m = parse_line("web", b"src/a.ts\x0012\x005\x00  let x = 1;\r").unwrap();
        assert_eq!(m, SearchMatch { repo: "web".into(), path: "src/a.ts".into(), line: 12, col: 5, text: "  let x = 1;".into() });
        let long = format!("p\x001\x00900\x00{}needle{}", "a".repeat(899), "b".repeat(500));
        let m = parse_line("r", long.as_bytes()).unwrap();
        assert!(m.text.chars().count() <= MAX_LINE);
        assert_eq!(&m.text[(m.col as usize - 1)..(m.col as usize + 5)], "needle");
        let m = parse_line("r", "p\x001\x008\x00ção = x".as_bytes()).unwrap();
        assert_eq!(m.col, 6, "byte offset 8 is the 6th character after two 2-byte chars");
    }

    #[test]
    fn greps_tracked_and_untracked_files_with_options() {
        let dir = std::env::temp_dir().join(format!("orbit-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| crate::proc::run("git", args, Some(&dir)).unwrap();
        git(&["init", "-q"]);
        std::fs::write(dir.join("a.rs"), "fn Foo() {}\nlet foo = 1;\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.rs\n").unwrap();
        git(&["add", "-A"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
        std::fs::write(dir.join("new.ts"), "const foo = 2;\n").unwrap();
        std::fs::write(dir.join("ignored.rs"), "foo\n").unwrap();

        let q = |query: &str, case: bool, word: bool, regex: bool, mask: &str| SearchQuery {
            query: query.into(),
            case_sensitive: case,
            whole_word: word,
            regex,
            mask: mask.into(),
            repos: vec![],
        };
        let run = |q: &SearchQuery| grep_repo("r", &dir, q, &pathspecs(&q.mask), 100).unwrap().0;
        assert_eq!(run(&q("foo", false, false, false, "")).len(), 3, "case-insensitive, untracked included, ignored skipped");
        assert_eq!(run(&q("foo", true, false, false, "")).len(), 2);
        assert_eq!(run(&q("foo", false, false, false, "*.ts")).len(), 1);
        assert_eq!(run(&q(r"fo+\s*=", false, false, true, "")).len(), 2);
        let err = grep_repo("r", &dir, &q("(", false, false, true, ""), &[], 100).unwrap_err();
        assert!(err.starts_with("Invalid regex:"), "{err}");
        let (m, truncated) = grep_repo("r", &dir, &q("foo", false, false, false, ""), &[], 1).unwrap();
        assert!(truncated && m.len() == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
