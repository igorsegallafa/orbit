//! The editor's quick-pick overlay: go to line, file outline, project
//! symbols, references and rename all share it (a query line and a list).
use super::provider::{fuzzy_score, Target};
use crate::theme::{self, c};
use gpui::{div, prelude::*, px, Context, FocusHandle, KeyDownEvent, MouseButton, SharedString, Window};

/// Rows shown at once; the window follows the selection.
const VISIBLE: usize = 14;

#[derive(Clone, Debug, PartialEq)]
pub enum PickerKind {
    GoToLine,
    Outline,
    WorkspaceSymbols,
    /// References, implementations… (`title` says which).
    Locations { title: SharedString },
    Rename,
    CodeActions,
}

#[derive(Clone, Debug)]
pub struct PickItem {
    pub label: SharedString,
    /// Shown after the label, dimmed (kind, container, file:line).
    pub detail: SharedString,
    pub depth: usize,
    pub target: Option<Target>,
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub items: Vec<PickItem>,
    /// Indexes into `items` that match the query, best first.
    pub shown: Vec<usize>,
    pub selected: usize,
    /// A line under the query: loading, no results, hints.
    pub status: Option<SharedString>,
}

pub enum PickerKey {
    /// The query changed.
    Edited,
    Accept,
    Close,
    Handled,
    Ignored,
}

impl Picker {
    pub fn new(kind: PickerKind, query: String) -> Self {
        Self { kind, query, items: vec![], shown: vec![], selected: 0, status: None }
    }

    pub fn title(&self) -> SharedString {
        match &self.kind {
            PickerKind::GoToLine => "Go to line".into(),
            PickerKind::Outline => "Go to symbol in file".into(),
            PickerKind::WorkspaceSymbols => "Go to symbol in project".into(),
            PickerKind::Locations { title } => title.clone(),
            PickerKind::Rename => "Rename symbol".into(),
            PickerKind::CodeActions => "Quick fixes and refactorings".into(),
        }
    }

    /// Lists filter locally; project symbols ask the server instead.
    fn filters_locally(&self) -> bool {
        matches!(self.kind, PickerKind::Outline | PickerKind::Locations { .. } | PickerKind::CodeActions)
    }

    pub fn set_items(&mut self, items: Vec<PickItem>) {
        self.items = items;
        self.refilter();
    }

    pub fn refilter(&mut self) {
        let q = self.query.trim();
        if !self.filters_locally() || q.is_empty() {
            self.shown = (0..self.items.len()).collect();
        } else {
            let mut scored: Vec<(i32, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(ix, item)| {
                    let label = fuzzy_score(q, &item.label);
                    let detail = fuzzy_score(q, &item.detail).map(|s| s - 500);
                    Some((label.max(detail)?, ix))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.shown = scored.into_iter().map(|(_, ix)| ix).collect();
        }
        self.selected = 0;
    }

    pub fn selected_item(&self) -> Option<&PickItem> {
        self.items.get(self.selected_index()?)
    }

    /// The selected row's index in `items`.
    pub fn selected_index(&self) -> Option<usize> {
        self.shown.get(self.selected).copied()
    }

    /// Typing, editing and list keys. The query line is our own: a full
    /// text input would swallow up/down/escape.
    pub fn on_key(&mut self, event: &KeyDownEvent, cx: &mut gpui::App) -> PickerKey {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let n = self.shown.len();
        match ks.key.as_str() {
            "escape" => return PickerKey::Close,
            "enter" => return PickerKey::Accept,
            "up" if n > 0 => self.selected = (self.selected + n - 1) % n,
            "down" if n > 0 => self.selected = (self.selected + 1) % n,
            "pageup" => self.selected = self.selected.saturating_sub(VISIBLE),
            "pagedown" if n > 0 => self.selected = (self.selected + VISIBLE).min(n - 1),
            "backspace" if m.control => {
                self.query.clear();
                return self.edited();
            }
            "backspace" => {
                self.query.pop();
                return self.edited();
            }
            "v" if m.control => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                self.query.push_str(text.lines().next().unwrap_or_default());
                return self.edited();
            }
            _ if m.control || m.alt || m.platform => return PickerKey::Ignored,
            _ => match ks.key_char.as_deref() {
                Some(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                    self.query.push_str(t);
                    return self.edited();
                }
                _ => return PickerKey::Ignored,
            },
        }
        PickerKey::Handled
    }

    fn edited(&mut self) -> PickerKey {
        self.refilter();
        PickerKey::Edited
    }

    pub fn render<T: 'static>(
        &self,
        focus: &FocusHandle,
        on_key: impl Fn(&mut T, &KeyDownEvent, &mut Window, &mut Context<T>) + 'static,
        on_pick: impl Fn(&mut T, usize, &mut Window, &mut Context<T>) + 'static + Clone,
        cx: &mut Context<T>,
    ) -> impl IntoElement {
        let start = self.selected.saturating_sub(VISIBLE - 1).min(self.shown.len().saturating_sub(VISIBLE));
        let rows = self.shown.iter().enumerate().skip(start).take(VISIBLE).map(|(pos, &ix)| {
            let item = &self.items[ix];
            let selected = pos == self.selected;
            let on_pick = on_pick.clone();
            div()
                .id(("pick", pos))
                .px_3()
                .py(px(3.))
                .pl(px(12. + item.depth as f32 * 14.))
                .flex()
                .gap_2()
                .overflow_hidden()
                .whitespace_nowrap()
                .cursor_pointer()
                .when(selected, |d| d.bg(theme::accent_soft()))
                .hover(|s| s.bg(c(theme::PANEL_HOVER)))
                .child(div().flex_none().text_color(c(theme::TEXT)).child(item.label.clone()))
                .child(div().min_w_0().overflow_hidden().text_color(c(theme::TEXT_FAINT)).child(item.detail.clone()))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| on_pick(this, pos, window, cx)))
        });
        let placeholder = match self.kind {
            PickerKind::GoToLine => "line[:column]",
            PickerKind::Rename => "new name",
            _ => "filter",
        };
        div()
            .absolute()
            .top(px(6.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("editor-picker")
                    .track_focus(focus)
                    .key_context("OrbitPicker")
                    .on_key_down(cx.listener(on_key))
                    .w(px(620.))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .bg(c(theme::PANEL_RAISED))
                    .border_1()
                    .border_color(c(theme::BORDER_STRONG))
                    .rounded_md()
                    .shadow_lg()
                    .text_sm()
                    .child(
                        div()
                            .px_3()
                            .pt_2()
                            .text_xs()
                            .text_color(c(theme::TEXT_FAINT))
                            .child(self.title()),
                    )
                    .child(
                        div()
                            .mx_2()
                            .my_1()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(c(theme::BG))
                            .border_1()
                            .border_color(c(theme::ACCENT))
                            .flex()
                            .child(if self.query.is_empty() {
                                div().text_color(c(theme::TEXT_FAINT)).child(placeholder).into_any_element()
                            } else {
                                div().text_color(c(theme::TEXT)).child(self.query.clone()).into_any_element()
                            })
                            .child(div().w(px(1.)).h(px(16.)).bg(c(theme::TEXT))),
                    )
                    .when_some(self.status.clone(), |d, s| d.child(div().px_3().py_1().text_xs().text_color(c(theme::TEXT_DIM)).child(s)))
                    .child(div().pb_1().flex().flex_col().children(rows)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, detail: &str) -> PickItem {
        PickItem { label: label.to_string().into(), detail: detail.to_string().into(), depth: 0, target: None }
    }

    #[test]
    fn filters_and_ranks_by_label_then_detail() {
        let mut p = Picker::new(PickerKind::Outline, String::new());
        p.set_items(vec![item("Update", "method"), item("Initialize", "method"), item("m_init", "field Init")]);
        assert_eq!(p.shown, [0, 1, 2]);
        p.query = "init".into();
        p.refilter();
        assert_eq!(p.shown, [1, 2]);
        assert_eq!(p.selected_item().unwrap().label, "Initialize");
    }

    #[test]
    fn project_symbols_are_not_filtered_locally() {
        let mut p = Picker::new(PickerKind::WorkspaceSymbols, "zzz".into());
        p.set_items(vec![item("Game", "")]);
        assert_eq!(p.shown, [0]);
    }
}
