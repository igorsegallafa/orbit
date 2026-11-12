//! Review notes (REVIEW.md, the Tauri app's format through `orbit_core::notes`)
//! and where a note's code is now (`anchorNote` in `src/lib/notes.ts`).
use crate::overlay::MenuItem;
use crate::sessions;
use gpui::App;
use orbit_core::notes::{self, Note, NotesFile};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// REVIEW.md has no threads: a reply is a paragraph appended to the note, so
/// the agent reading the file and the Tauri app both see it.
pub const REPLY_PREFIX: &str = "Reply: ";

pub fn list(ws: &str) -> Result<NotesFile, String> {
    notes::list(ws)
}

pub fn add(ws: &str, repo: &str, path: &str, start: u32, end: u32, body: &str, snippet: String) -> Result<NotesFile, String> {
    notes::add(ws, repo.into(), path.into(), start, end, body.into(), snippet)
}

pub fn set_done(ws: &str, id: &str, done: bool) -> Result<NotesFile, String> {
    notes::update(ws, id, Some(done), None)
}

pub fn edit(ws: &str, id: &str, body: &str) -> Result<NotesFile, String> {
    notes::update(ws, id, None, Some(body.into()))
}

/// Appends a reply and reopens the note (a reply asks for more work).
pub fn reply(ws: &str, note: &Note, text: &str) -> Result<NotesFile, String> {
    if text.trim().is_empty() {
        return Err("the reply is empty".into());
    }
    let body = format!("{}\n\n{REPLY_PREFIX}{}", note.body.trim_end(), text.trim());
    let file = notes::update(ws, &note.id, None, Some(body.clone()))?;
    // The id follows the content: find the note again by its new body.
    match file.notes.iter().find(|n| n.body == body && n.path == note.path && n.done) {
        Some(n) => notes::update(ws, &n.id.clone(), Some(false), None),
        None => Ok(file),
    }
}

pub fn delete(ws: &str, ids: &[String]) -> Result<NotesFile, String> {
    notes::delete(ws, ids)
}

/// REVIEW.md's path and modification time, to reload when someone (an
/// agent ticking a box) changes it.
pub fn stamp(ws: &str) -> Option<(PathBuf, Option<SystemTime>)> {
    let path = notes::notes_path(ws).ok()?;
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    Some((path, mtime))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub start: u32,
    pub end: u32,
    /// The code moved since the note was left.
    pub moved: bool,
    /// The code isn't there any more: the note stays at its old lines.
    pub lost: bool,
}

/// Where a note's code is in `text` now: its snippet found again nearest to
/// the old lines.
pub fn anchor(note: &Note, text: Option<&str>) -> Anchor {
    let at = Anchor { start: note.start, end: note.end, moved: false, lost: false };
    let Some(text) = text.filter(|_| !note.snippet.trim().is_empty()) else { return at };
    let lines: Vec<&str> = text.split('\n').collect();
    let want: Vec<&str> = note.snippet.split('\n').collect();
    let fits = |from: usize| want.iter().enumerate().all(|(k, w)| lines.get(from + k).map_or("", |l| l).trim_end() == w.trim_end());
    let old = note.start as usize - 1;
    if fits(old) {
        return at;
    }
    let best = (0..(lines.len() + 1).saturating_sub(want.len())).filter(|&i| fits(i)).min_by_key(|&i| i.abs_diff(old));
    match best {
        Some(i) => Anchor { start: i as u32 + 1, end: (i + want.len()) as u32, moved: true, lost: false },
        None => {
            let last = lines.len().max(1) as u32;
            Anchor { start: note.start.min(last), end: note.end.min(last), moved: false, lost: true }
        }
    }
}

/// The code on lines `start..=end` (one-based) of `text`, kept with a note.
pub fn snippet(text: &str, start: u32, end: u32) -> String {
    text.split('\n').skip(start.saturating_sub(1) as usize).take((end + 1).saturating_sub(start) as usize).collect::<Vec<_>>().join("\n")
}

/// Prompt that has an agent work through the open notes (`notesPrompt`).
pub fn prompt(file: &Path) -> String {
    format!(
        "Read the review notes in {} and address every open item (\"- [ ]\"): each one points at a file and lines (relative to that file's folder), says what to change, and shows the code it was left on. Skip ticked items (\"- [x]\"): they were done in an earlier round. Tick each item (\"- [x]\") in that file as you finish it. Commit nothing unless asked.",
        file.display()
    )
}

pub fn open(file: Option<&NotesFile>) -> Vec<&Note> {
    file.iter().flat_map(|f| &f.notes).filter(|n| !n.done).collect()
}

/// "Fix N notes with agent": sends the prompt to one of workspace `ws`'s
/// agent sessions, or starts a new one with it.
pub fn fix_menu(ws: &str, file: &Path, count: usize, cx: &App) -> Vec<MenuItem> {
    let prompt = prompt(file);
    let mut items: Vec<MenuItem> = sessions::store(cx)
        .read(cx)
        .sessions()
        .filter(|(m, v)| m.workspace == ws && m.cmd.is_some() && v.is_some())
        .map(|(m, _)| {
            let (id, name, prompt) = (m.id.clone(), m.session_name.clone(), prompt.clone());
            MenuItem::new(format!("Send to {name}"), move |a, w, cx| {
                let Some(view) = sessions::store(cx).read(cx).view(&id) else { return };
                view.update(cx, |v, cx| v.send_prompt(&prompt, w, cx));
                let s = if count == 1 { "" } else { "s" };
                a.toast(crate::toast::ToastKind::Success, format!("Sent {count} note{s} to {name}"), None, cx);
            })
        })
        .collect();
    let ws = ws.to_string();
    items.push(MenuItem::new("New agent session", move |a, w, cx| {
        let bin = orbit_core::config::Config::load().map(|c| c.ai.agent_bin()).unwrap_or_else(|_| "claude".into());
        sessions::new_session_in(a, ws.clone(), Some(&bin), Some(prompt.clone()), w, cx);
    }));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: u32, end: u32, snippet: &str) -> Note {
        Note {
            id: "x".into(),
            done: false,
            repo: "r".into(),
            path: "a.rs".into(),
            start,
            end,
            body: "b".into(),
            snippet: snippet.into(),
        }
    }

    #[test]
    fn anchors_follow_moved_code_and_flag_lost_code() {
        let n = note(2, 3, "b\nc");
        assert_eq!(anchor(&n, Some("a\nb\nc\nd")), Anchor { start: 2, end: 3, moved: false, lost: false });
        assert_eq!(anchor(&n, Some("x\ny\na\nb\nc")), Anchor { start: 4, end: 5, moved: true, lost: false });
        assert_eq!(anchor(&n, Some("q")), Anchor { start: 1, end: 1, moved: false, lost: true });
        assert_eq!(anchor(&note(2, 2, ""), Some("q")).start, 2, "no snippet: kept as is");
        // The nearest copy wins.
        assert_eq!(anchor(&note(5, 5, "z"), Some("z\na\na\na\na\na\nz")).start, 7);
    }

    #[test]
    fn snippets_are_the_noted_lines() {
        assert_eq!(snippet("a\nb\nc\nd", 2, 3), "b\nc");
        assert_eq!(snippet("a", 1, 1), "a");
        assert_eq!(snippet("a", 3, 4), "");
    }
}
