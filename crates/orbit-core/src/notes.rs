// Review notes: comments left on a workspace's local changes, kept as a
// checklist in REVIEW.md so any agent (a Claude already running in a
// terminal included) can read them, fix the code and tick them off.
//
// The file is the source of truth: Orbit reads it fresh for every change,
// and an agent editing it (ticking a box) shows up in the UI through the
// file watcher.
//
// Format (one item per note; paths relative to the file's folder):
//
//   - [ ] `api/src/auth.rs:42-58`
//     This unwrap can panic with an empty token.
//     ```rs
//     let claims = decode(token).unwrap();
//     ```
use serde::Serialize;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use crate::workspace;

pub const FILE: &str = "REVIEW.md";

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// Stable while location and text stay the same (ticking keeps it).
    pub id: String,
    pub done: bool,
    pub repo: String,
    pub path: String,
    pub start: u32,
    pub end: u32,
    pub body: String,
    /// The code the note was left on, to find it again after edits.
    pub snippet: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NotesFile {
    /// Absolute path of REVIEW.md (to hand to an agent).
    pub file: String,
    pub notes: Vec<Note>,
}

/// REVIEW.md of a workspace: at its root, next to the repos. A repo scope
/// keeps it in Orbit's folder for the repo, so nothing lands in the clone.
pub fn notes_path(ws: &str) -> Result<PathBuf, String> {
    if ws.starts_with("dir:") {
        return Err("review notes need a workspace or a repository".into());
    }
    Ok(match workspace::repo_scope(ws) {
        Some(_) => workspace::orbit_dir(ws)?.join(FILE),
        None => workspace::ws_dir(ws)?.join(FILE),
    })
}

fn note_id(repo: &str, path: &str, start: u32, end: u32, body: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (repo, path, start, end, body.trim()).hash(&mut h);
    format!("{:012x}", h.finish() & 0xffff_ffff_ffff)
}

const HEADER: &str = "# Review notes

Review comments left in Orbit on this code. Each item points at
`<file>:<lines>` (relative to this file's folder), then the comment, then the
code it was left on. Line numbers are from when the note was written; if the
file changed since, find the code by its snippet.

Work through the open items. When one is done, tick its box (`- [x]`).
Don't reword or remove items.
";

const SCOPE_HEADER_NOTE: &str = "
Paths are relative to the repository root";

/// `scope_repo`: the repo of a repo scope, whose paths carry no repo prefix.
pub fn parse(text: &str, scope_repo: Option<&str>) -> Vec<Note> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some((done, loc)) = item_head(lines[i]) else {
            i += 1;
            continue;
        };
        i += 1;
        let mut block = Vec::new();
        while i < lines.len() && (lines[i].trim().is_empty() || lines[i].starts_with("  ")) {
            if item_head(lines[i]).is_some() {
                break;
            }
            block.push(lines[i].strip_prefix("  ").unwrap_or(lines[i].trim()));
            i += 1;
        }
        let Some((full, start, end)) = parse_loc(loc) else { continue };
        let (repo, path) = match scope_repo {
            Some(r) => (r.to_string(), full.to_string()),
            None => match full.split_once('/') {
                Some((r, p)) => (r.to_string(), p.to_string()),
                None => continue,
            },
        };
        let (body, snippet) = split_block(&block);
        out.push(Note { id: note_id(&repo, &path, start, end, &body), done, repo, path, start, end, body, snippet });
    }
    out
}

/// "- [ ] `loc`" → (done, loc)
fn item_head(line: &str) -> Option<(bool, &str)> {
    let rest = line.strip_prefix("- [")?;
    let mut chars = rest.chars();
    let mark = chars.next()?;
    let rest = chars.as_str().strip_prefix("] ")?.trim();
    let loc = rest.strip_prefix('`')?.split('`').next()?;
    let done = match mark {
        ' ' => false,
        'x' | 'X' => true,
        _ => return None,
    };
    Some((done, loc))
}

/// "path/to/file.rs:42-58" → (path, 42, 58)
fn parse_loc(loc: &str) -> Option<(&str, u32, u32)> {
    let (path, lines) = loc.rsplit_once(':')?;
    let (a, b) = match lines.split_once('-') {
        Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
        None => {
            let n = lines.parse().ok()?;
            (n, n)
        }
    };
    (!path.is_empty() && a > 0 && b >= a).then_some((path, a, b))
}

/// Item lines → (comment, first fenced code block).
fn split_block(block: &[&str]) -> (String, String) {
    let mut body = Vec::new();
    let mut code = Vec::new();
    let mut fence: Option<String> = None;
    let mut have_code = false;
    for line in block {
        let t = line.trim_start();
        match &fence {
            Some(f) if t.starts_with(f.as_str()) && t.trim() == f.as_str() => fence = None,
            Some(_) => code.push(*line),
            None if !have_code && t.starts_with("```") => {
                let ticks = t.chars().take_while(|c| *c == '`').count();
                fence = Some("`".repeat(ticks));
                have_code = true;
            }
            None => body.push(*line),
        }
    }
    (body.join("\n").trim().to_string(), code.join("\n"))
}

pub fn render(notes: &[Note], scope_repo: Option<&str>) -> String {
    let mut s = String::from(HEADER);
    if scope_repo.is_some() {
        s.push_str(SCOPE_HEADER_NOTE);
        s.push_str(".\n");
    }
    for n in notes {
        let loc = match scope_repo {
            Some(_) => n.path.clone(),
            None => format!("{}/{}", n.repo, n.path),
        };
        let lines = if n.start == n.end { n.start.to_string() } else { format!("{}-{}", n.start, n.end) };
        s.push_str(&format!("\n- [{}] `{loc}:{lines}`\n", if n.done { "x" } else { " " }));
        for l in n.body.trim().lines() {
            s.push_str(&format!("  {l}\n").replace("  \n", "\n"));
        }
        if !n.snippet.trim().is_empty() {
            let fence = if n.snippet.contains("```") { "````" } else { "```" };
            let lang = n.path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
            s.push_str(&format!("  {fence}{lang}\n"));
            for l in n.snippet.lines() {
                s.push_str(&format!("  {l}\n").replace("  \n", "\n"));
            }
            s.push_str(&format!("  {fence}\n"));
        }
    }
    s
}

fn load(ws: &str) -> Result<(PathBuf, Vec<Note>), String> {
    let path = notes_path(ws)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    Ok((path, parse(&text, workspace::repo_scope(ws))))
}

fn save(ws: &str, path: &PathBuf, notes: &[Note]) -> Result<(), String> {
    if notes.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, render(notes, workspace::repo_scope(ws))).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Reads, changes and writes back the notes in one go.
fn change(ws: &str, f: impl FnOnce(&mut Vec<Note>) -> Result<(), String>) -> Result<NotesFile, String> {
    let (path, mut notes) = load(ws)?;
    f(&mut notes)?;
    save(ws, &path, &notes)?;
    // Ids follow the content: read back what was written.
    list(ws)
}

pub fn list(ws: &str) -> Result<NotesFile, String> {
    let (path, notes) = load(ws)?;
    Ok(NotesFile { file: path.to_string_lossy().to_string(), notes })
}

pub fn add(ws: &str, repo: String, path: String, start: u32, end: u32, body: String, snippet: String) -> Result<NotesFile, String> {
    if body.trim().is_empty() {
        return Err("the note is empty".into());
    }
    let (start, end) = (start.max(1), end.max(start.max(1)));
    change(ws, |notes| {
        let id = note_id(&repo, &path, start, end, &body);
        notes.push(Note { id, done: false, repo, path, start, end, body: body.trim().to_string(), snippet });
        Ok(())
    })
}

pub fn update(ws: &str, id: &str, done: Option<bool>, body: Option<String>) -> Result<NotesFile, String> {
    change(ws, |notes| {
        let n = notes.iter_mut().find(|n| n.id == id).ok_or("that note changed meanwhile; try again")?;
        if let Some(d) = done {
            n.done = d;
        }
        if let Some(b) = body.filter(|b| !b.trim().is_empty()) {
            n.body = b.trim().to_string();
        }
        Ok(())
    })
}

pub fn delete(ws: &str, ids: &[String]) -> Result<NotesFile, String> {
    change(ws, |notes| {
        notes.retain(|n| !ids.contains(&n.id));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(repo: &str, path: &str, start: u32, end: u32, body: &str, snippet: &str, done: bool) -> Note {
        Note { id: note_id(repo, path, start, end, body), done, repo: repo.into(), path: path.into(), start, end, body: body.into(), snippet: snippet.into() }
    }

    #[test]
    fn round_trips_through_markdown() {
        let notes = vec![
            note("api", "src/auth.rs", 42, 58, "This unwrap can panic.\n\nUse `?` instead.", "let c = decode(t).unwrap();\n\n  nested", false),
            note("web", "src/Login.tsx", 10, 10, "Rename to useSession", "", true),
            note("api", "README.md", 3, 4, "Fence inside", "```sh\nmake\n```", false),
        ];
        let md = render(&notes, None);
        assert!(md.contains("- [ ] `api/src/auth.rs:42-58`"));
        assert!(md.contains("- [x] `web/src/Login.tsx:10`"));
        assert_eq!(parse(&md, None), notes);
    }

    #[test]
    fn repo_scope_paths_have_no_repo_prefix() {
        let notes = vec![note("api", "src/main.rs", 1, 2, "x", "fn main() {}", false)];
        let md = render(&notes, Some("api"));
        assert!(md.contains("- [ ] `src/main.rs:1-2`"));
        assert_eq!(parse(&md, Some("api")), notes);
    }

    #[test]
    fn agent_ticks_keep_the_id_and_odd_lines_are_ignored() {
        let md = render(&[note("api", "a.rs", 5, 5, "fix", "x", false)], None);
        let ticked = md.replace("- [ ]", "- [X]") + "\nSome trailing text an agent added.\n- [ ] not a note\n";
        let before = parse(&md, None);
        let after = parse(&ticked, None);
        assert_eq!(after.len(), 1);
        assert!(after[0].done);
        assert_eq!(after[0].id, before[0].id);
    }
}
