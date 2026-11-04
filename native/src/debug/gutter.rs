//! Breakpoints and the paused line in the editors (`src/lib/dap/gutter.ts`).
//!
//! gpui-component's editor has no glyph margin or gutter click hook. What it
//! has is a strip before the line numbers for `GutterDecoration` bars (meant
//! for VCS marks), and range decorations. So: breakpoints are red bars in
//! that strip (widened to 8 px), the paused line is a filled range plus an
//! amber bar, and a click in the strip toggles a breakpoint. The click comes
//! from one mouse-down listener on the editor's wrapper in `editor.rs`, which
//! runs after the editor's own handler has moved the caret to the clicked
//! line, so the line is the caret's.
use super::{store, DebugStatus};
use crate::editor::EditorTab;
use crate::theme::{self, hsla};
use gpui::{px, App, Context, Global, MouseDownEvent, WeakEntity, Window};
use gpui_component::input::{GutterDecoration, GutterDecorationCollection, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, RopeExt};
use std::path::{Path, PathBuf};

/// Width of the breakpoint strip; the editor adds a 4 px gap after it.
pub const STRIP: f32 = 8.;

#[derive(Clone)]
struct Deco {
    editor: WeakEntity<EditorTab>,
    breakpoints: GutterDecorationCollection,
    here_mark: GutterDecorationCollection,
    here_line: RangeDecorationCollection,
}

#[derive(Default)]
struct Decorated(Vec<Deco>);

impl Global for Decorated {}

/// Every editor gets the strip and its collections when it opens.
pub fn init(cx: &mut App) {
    cx.observe_new::<EditorTab>(|_, _, cx: &mut Context<EditorTab>| {
        let editor = cx.weak_entity();
        // Not now: the editor may be opening from inside a store update.
        cx.defer(move |cx| attach(editor, cx));
    })
    .detach();
}

fn attach(editor: WeakEntity<EditorTab>, cx: &mut App) {
    let Some(e) = editor.upgrade() else { return };
    let state = e.read(cx).state.clone();
    let (breakpoints, here_mark, here_line) = state.update(cx, |s, cx| {
        s.set_gutter_decorations_width(px(STRIP), cx);
        (
            s.create_gutter_decorations_collection(vec![], cx),
            s.create_gutter_decorations_collection(vec![], cx),
            s.create_range_decorations_collection(vec![], cx),
        )
    });
    cx.default_global::<Decorated>().0.push(Deco { editor, breakpoints, here_mark, here_line });
    refresh(cx);
    crate::run::apply_problems(cx);
}

/// Open editors (closed ones are forgotten).
pub fn editors(cx: &mut App) -> Vec<gpui::Entity<EditorTab>> {
    let list = cx.default_global::<Decorated>();
    list.0.retain(|d| d.editor.upgrade().is_some());
    list.0.iter().filter_map(|d| d.editor.upgrade()).collect()
}

/// The byte range of a 1-based line, or None past the end.
fn line_range(text: &gpui_component::input::Rope, line: u32) -> Option<std::ops::Range<usize>> {
    let row = (line as usize).checked_sub(1)?;
    (row < text.lines_len()).then(|| text.line_start_offset(row)..text.line_end_offset(row))
}

/// Redraws breakpoints and the paused line in every editor.
pub fn refresh(cx: &mut App) {
    editors(cx);
    let debug = store(cx);
    let d = debug.read(cx);
    // The paused line is the active session's.
    let here: Option<(PathBuf, u32)> = d.active.as_ref().map(|s| s.read(cx)).filter(|s| s.status == DebugStatus::Stopped).and_then(|s| {
        let f = s.frames.iter().find(|f| Some(f.id) == s.frame_id)?;
        Some((f.file.clone()?, f.line))
    });
    let marks: Vec<(Deco, Vec<u32>, Option<u32>)> = cx
        .global::<Decorated>()
        .0
        .iter()
        .filter_map(|deco| {
            let path = deco.editor.upgrade()?.read(cx).path.clone();
            let at = here.as_ref().filter(|(f, _)| same_file(f, &path)).map(|(_, l)| *l);
            Some((deco.clone(), d.breakpoints_in(&path), at))
        })
        .collect();
    for (deco, lines, at) in marks {
        let Some(e) = deco.editor.upgrade() else { continue };
        let text = e.read(cx).state.read(cx).text().clone();
        let red = hsla(theme::DANGER);
        deco.breakpoints.set(lines.iter().filter_map(|l| line_range(&text, *l)).map(|r| GutterDecoration::new(r).with_color(red)).collect(), cx);
        let range = at.and_then(|l| line_range(&text, l));
        let amber = hsla(theme::WARN);
        deco.here_mark.set(range.clone().map(|r| GutterDecoration::new(r).with_color(amber)).into_iter().collect(), cx);
        let fill = gpui::Hsla { a: 0.16, ..amber };
        deco.here_line.set(
            range.map(|r| RangeDecoration::new(r.start..r.end.max(r.start + 1)).with_style(RangeDecorationStyle::Fill).with_color(fill)).into_iter().collect(),
            cx,
        );
    }
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    a == b || (cfg!(windows) && a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()))
}

/// A left click on the editor: in the breakpoint strip, it toggles the caret's line.
pub fn on_mouse_down(editor: &mut EditorTab, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<EditorTab>) {
    let state = editor.state.read(cx);
    let left = state.input_bounds().origin.x;
    if ev.position.x < left || ev.position.x > left + px(STRIP + 4.) {
        return;
    }
    let line = state.cursor_position().line + 1;
    let path = editor.path.clone();
    cx.defer(move |cx| store(cx).update(cx, |d, cx| d.toggle_breakpoint(path, line, cx)));
}

/// What each editor shows (self-test dump): breakpoint and paused lines, 1-based.
pub fn dump(cx: &App) -> String {
    let Some(list) = cx.try_global::<Decorated>() else { return String::new() };
    let mut out = String::new();
    for d in &list.0 {
        let Some(e) = d.editor.upgrade() else { continue };
        let text = e.read(cx).state.read(cx).text().clone();
        let lines = |ranges: Vec<std::ops::Range<usize>>| -> Vec<u32> { ranges.iter().map(|r| text.offset_to_position(r.start).line + 1).collect() };
        out.push_str(&format!(
            "  gutter {}: breakpoints {:?}, paused mark {:?}, paused line {:?}\n",
            e.read(cx).path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            lines(d.breakpoints.get_ranges(cx)),
            lines(d.here_mark.get_ranges(cx)),
            lines(d.here_line.get_ranges(cx)),
        ));
    }
    out
}

/// Where a click on `line`'s breakpoint strip lands (self-test).
pub fn strip_point(editor: &EditorTab, line: u32, cx: &App) -> Option<gpui::Point<gpui::Pixels>> {
    let state = editor.state.read(cx);
    let text = state.text_bounds()?;
    let h = state.line_height()?;
    let x = state.input_bounds().origin.x + px(STRIP / 2.);
    Some(gpui::point(x, text.origin.y + h * (line as f32 - 0.5)))
}
