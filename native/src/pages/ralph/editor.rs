//! The PRD editor (`RalphView`'s editing mode): the rules every story
//! follows, and per story its title, description, acceptance criteria,
//! notes and done flag; reorder, delete and add stories. Save writes
//! `scripts/ralph/prd.json` through `ralph::save_prd`.
use super::super::kit::{self as pkit, Kind};
use super::super::ui;
use super::feed::story_id;
use super::{watch, RalphPage};
use crate::probe::probe;
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, App, ClickEvent, Context, Entity, FontWeight, SharedString, Window};
use gpui_component::input::{InputState, Textarea, TextareaState};
use orbit_core::ralph::prd::{Prd, Story};
use std::fmt::Write as _;

pub struct EditStory {
    base: Story,
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    criteria: Entity<TextareaState>,
    notes: Entity<InputState>,
    passes: bool,
}

pub struct PrdEditor {
    base: Prd,
    rules: Entity<TextareaState>,
    stories: Vec<EditStory>,
}

fn textarea(text: String, rows: usize, placeholder: &'static str, window: &mut Window, cx: &mut Context<RalphPage>) -> Entity<TextareaState> {
    let input = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx).auto_grow(rows, rows + 6).placeholder(placeholder);
        s.set_value(text, window, cx);
        s
    });
    watch(&input, window, cx);
    input
}

fn input(text: String, placeholder: &'static str, window: &mut Window, cx: &mut Context<RalphPage>) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(text));
    watch(&input, window, cx);
    input
}

fn edit_story(s: Story, window: &mut Window, cx: &mut Context<RalphPage>) -> EditStory {
    EditStory {
        title: input(s.title.clone(), "Story title", window, cx),
        description: textarea(s.description.clone(), 3, "As a …, I want … so that …", window, cx),
        criteria: textarea(s.acceptance_criteria.join("\n"), 5, "", window, cx),
        notes: input(s.notes.clone(), "Optional", window, cx),
        passes: s.passes,
        base: s,
    }
}

impl PrdEditor {
    pub fn new(prd: &Prd, window: &mut Window, cx: &mut Context<RalphPage>) -> Self {
        let mut stories = prd.user_stories.clone();
        stories.sort_by_key(|s| s.priority);
        Self {
            rules: textarea(prd.description.clone(), 3, "Context and constraints every story must follow", window, cx),
            stories: stories.into_iter().map(|s| edit_story(s, window, cx)).collect(),
            base: prd.clone(),
        }
    }

    /// The edited PRD: stories in their order (priority 1…n), criteria one per line.
    pub fn prd(&self, cx: &App) -> Prd {
        let mut prd = self.base.clone();
        prd.description = self.rules.read(cx).value().to_string();
        prd.user_stories = self
            .stories
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut st = s.base.clone();
                st.title = s.title.read(cx).value().to_string();
                st.description = s.description.read(cx).value().to_string();
                st.acceptance_criteria = s.criteria.read(cx).value().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
                st.notes = s.notes.read(cx).value().to_string();
                st.passes = s.passes;
                st.priority = i as i64 + 1;
                st
            })
            .collect();
        prd
    }

    pub fn dump(&self, out: &mut String, cx: &App) {
        let prd = self.prd(cx);
        let _ = writeln!(out, "  editing: rules {:?}", prd.description);
        for s in &prd.user_stories {
            let _ = writeln!(out, "    edit {} p{} [{}] {:?} · {:?} · criteria {:?} · notes {:?}", s.id, s.priority, if s.passes { "x" } else { " " }, s.title, s.description, s.acceptance_criteria, s.notes);
        }
    }
}

/// `.rv-field`: a small label over its control.
fn labeled(label: &'static str, control: impl IntoElement) -> gpui::Div {
    div().flex().flex_col().gap(px(5.)).child(div().text_size(px(11.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_FAINT)).child(label)).child(control)
}

fn area(id: String, state: &Entity<TextareaState>) -> gpui::Stateful<gpui::Div> {
    div().id(SharedString::from(id.clone())).relative().text_size(px(12.5)).child(Textarea::new(state)).child(probe(id))
}

/// A 24px square icon button (`.icon-button`), dimmed when `off`.
fn icon_btn(id: String, glyph: &'static str, off: bool, danger: bool) -> gpui::Stateful<gpui::Div> {
    let el = div()
        .id(SharedString::from(id.clone()))
        .relative()
        .flex_none()
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(c(theme::TEXT_DIM))
        .child(glyph)
        .child(probe(id));
    if off {
        el.opacity(0.35)
    } else {
        el.cursor_pointer().hover(move |s| s.bg(c(theme::PANEL_RAISED)).text_color(c(if danger { theme::DANGER } else { theme::TEXT })))
    }
}

impl RalphPage {
    pub(super) fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prd) = self.state.ready().and_then(|s| s.prd.clone()) else { return };
        if self.running() {
            return;
        }
        self.editor = Some(PrdEditor::new(&prd, window, cx));
        cx.notify();
    }

    pub(super) fn save_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = &self.editor else { return };
        let prd = ed.prd(cx);
        let (scope, repo) = (self.scope.clone(), self.repo.clone());
        ui::bg(
            cx,
            move || orbit_core::ralph::save_prd(scope, repo, prd),
            |t: &mut Self, r, w, cx| match r {
                Ok(()) => {
                    t.editor = None;
                    t.load(false, w, cx);
                }
                Err(e) => t.host.error("Couldn't save the PRD", e, cx),
            },
            window,
        );
    }

    fn move_story(&mut self, ix: usize, down: bool, cx: &mut Context<Self>) {
        let Some(ed) = &mut self.editor else { return };
        let j = if down { ix + 1 } else { ix.wrapping_sub(1) };
        if j < ed.stories.len() {
            ed.stories.swap(ix, j);
        }
        cx.notify();
    }

    fn add_story(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(n) = self.editor.as_ref().map(|e| e.stories.len() + 1) else { return };
        let story = Story {
            id: format!("US-{n:03}"),
            title: String::new(),
            description: String::new(),
            acceptance_criteria: vec!["Typecheck passes".into()],
            priority: n as i64,
            passes: false,
            notes: String::new(),
            extra: Default::default(),
        };
        let st = edit_story(story, window, cx);
        if let Some(ed) = &mut self.editor {
            ed.stories.push(st);
        }
        cx.notify();
    }

    pub(super) fn render_editor(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let Some(ed) = &self.editor else { return div() };
        let count = ed.stories.len();
        let cards = ed.stories.iter().enumerate().map(|(ix, s)| {
            let id = s.base.id.clone();
            let p = |what: &str| format!("ralph-edit:{id}:{what}");
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(c(theme::BORDER))
                .bg(c(theme::BG))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(story_id(id.clone()))
                        .child(div().flex_1().min_w_0().mx(px(4.)).font_weight(FontWeight::SEMIBOLD).child(pkit::field_sized(p("title"), &s.title, 30., 12.5, window, cx)))
                        .child(icon_btn(p("up"), "↑", ix == 0, false).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.move_story(ix, false, cx))))
                        .child(icon_btn(p("down"), "↓", ix + 1 == count, false).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| t.move_story(ix, true, cx))))
                        .child(icon_btn(p("delete"), "✕", false, true).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                            if let Some(ed) = &mut t.editor {
                                ed.stories.remove(ix);
                            }
                            cx.notify();
                        }))),
                )
                .child(labeled("Description", area(p("desc"), &s.description)))
                .child(labeled("Acceptance criteria, one per line", area(p("criteria"), &s.criteria)))
                .child(labeled("Notes for the agent", pkit::field_sized(p("notes"), &s.notes, 30., 12.5, window, cx)))
                .child(pkit::check(p("done"), "Done, Ralph skips it", s.passes).on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                    if let Some(s) = t.editor.as_mut().and_then(|e| e.stories.get_mut(ix)) {
                        s.passes = !s.passes;
                    }
                    cx.notify();
                })))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(6.))
            .child(labeled("Rules for every story", area("ralph-edit-rules".into(), &ed.rules)))
            .children(cards)
            .child(
                div()
                    .id("ralph-add-story")
                    .relative()
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .border_1()
                    .border_dashed()
                    .border_color(c(theme::BORDER_STRONG))
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_DIM))
                    .cursor_pointer()
                    .hover(|s| s.text_color(c(theme::TEXT)).border_color(c(theme::ACCENT)).bg(theme::accent_soft()))
                    .child("+ Add story")
                    .child(probe("ralph-add-story"))
                    .on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.add_story(w, cx))),
            )
    }

    /// The column head's actions while editing.
    pub(super) fn editor_actions(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap(px(6.))
            .child(pkit::mini("ralph-edit-discard", "Discard", Kind::Secondary).on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                t.editor = None;
                cx.notify();
            })))
            .child(pkit::mini("ralph-edit-save", "Save", Kind::Primary).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.save_editing(w, cx))))
    }
}
