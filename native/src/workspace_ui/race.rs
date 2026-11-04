//! Race agents (`Race.tsx` `RaceModal`): the same task for several agents,
//! each in its own variant workspace, started as prompted sessions.
use super::icons::icon;
use super::kit::{self, Kind};
use super::{spawn, Modal, PageTab};
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{div, prelude::*, px, ClickEvent, Context, Entity, Focusable, FontWeight, Window};
use gpui_component::input::{Textarea, TextareaState};
use orbit_core::workspace::Workspace;
use std::collections::BTreeMap;
use std::fmt::Write as _;

const AGENTS: [(&str, &str); 3] = [("claude", "Claude Code"), ("opencode", "OpenCode"), ("omp", "omp")];

#[derive(Clone)]
pub struct Contestant {
    pub agent: String,
    pub model: String,
}

pub struct RaceForm {
    pub ws: Workspace,
    pub prompt: Entity<TextareaState>,
    pub contestants: Vec<Contestant>,
    pub models: BTreeMap<String, Vec<String>>,
}

impl RaceForm {
    pub fn dump(&self, out: &mut String, cx: &gpui::App) {
        let cs: Vec<String> = self.contestants.iter().map(|c| format!("{}:{}", c.agent, c.model)).collect();
        let _ = writeln!(out, "  race modal ({}): task {:?} agents {cs:?}", self.ws.name, self.prompt.read(cx).value());
    }
}

/// Unique, readable variant labels: claude, claude-2, opencode…
pub fn labels(cs: &[Contestant]) -> Vec<String> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    cs.iter()
        .map(|c| {
            let n = seen.entry(c.agent.as_str()).or_insert(0);
            *n += 1;
            if *n == 1 { c.agent.clone() } else { format!("{}-{n}", c.agent) }
        })
        .collect()
}

fn agent_label(agent: &str) -> &str {
    AGENTS.iter().find(|(v, _)| *v == agent).map_or(agent, |(_, l)| l)
}

impl PageTab {
    fn race_form(&mut self) -> Option<&mut RaceForm> {
        match &mut self.modal {
            Some(Modal::Race(f)) => Some(f),
            _ => None,
        }
    }

    pub fn open_race(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws_page().and_then(|p| p.meta.clone()) else {
            return self.toast(ToastKind::Info, "Still reading the workspace", None, window, cx);
        };
        let prompt = cx.new(|cx| TextareaState::new(window, cx).auto_grow(5, 10).placeholder("e.g. Add retry with exponential backoff to the HTTP client, with tests."));
        let handle = prompt.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        let ai = orbit_core::config::Config::load().map(|c| c.ai).ok();
        let contestants = match ai {
            Some(ai) => {
                let other = if ai.agent == "claude" { "opencode" } else { "claude" };
                vec![Contestant { agent: ai.agent, model: ai.model }, Contestant { agent: other.into(), model: String::new() }]
            }
            None => vec![Contestant { agent: "claude".into(), model: String::new() }, Contestant { agent: "opencode".into(), model: String::new() }],
        };
        self.modal = Some(Modal::Race(Box::new(RaceForm { ws, prompt, contestants, models: BTreeMap::new() })));
        spawn(
            window,
            cx,
            || AGENTS.iter().map(|(a, _)| (a.to_string(), orbit_core::agent::list_models(a))).collect::<BTreeMap<_, _>>(),
            |t, models, _, _| {
                if let Some(f) = t.race_form() {
                    f.models = models;
                }
            },
        );
        cx.notify();
    }

    fn race_pick(&mut self, i: usize, model: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.race_form() else { return };
        let Some(cur) = f.contestants.get(i) else { return };
        let options: Vec<(String, String)> = if model {
            std::iter::once((String::new(), "Pick a model".to_string()))
                .chain(f.models.get(&cur.agent).into_iter().flatten().map(|m| (m.clone(), m.clone())))
                .collect()
        } else {
            AGENTS.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect()
        };
        let page = cx.entity().downgrade();
        let items = options
            .into_iter()
            .map(|(value, label)| {
                let page = page.clone();
                crate::overlay::MenuItem::new(label, move |_: &mut crate::app::OrbitApp, _: &mut Window, cx: &mut Context<crate::app::OrbitApp>| {
                    let value = value.clone();
                    let _ = page.update(cx, |t, cx| {
                        if let Some(c) = t.race_form().and_then(|f| f.contestants.get_mut(i)) {
                            if model {
                                c.model = value;
                            } else if c.agent != value {
                                c.agent = value;
                                c.model.clear();
                            }
                        }
                        cx.notify();
                    });
                })
            })
            .collect();
        let at = window.mouse_position();
        self.with_app(window, cx, move |a, w, cx| a.open_menu(at, items, w, cx));
    }

    fn race_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.race_form() else { return };
        let prompt = f.prompt.read(cx).value().trim().to_string();
        let cs = f.contestants.clone();
        if prompt.is_empty() || cs.len() < 2 || cs.iter().any(|c| c.model.trim().is_empty()) {
            return;
        }
        let ws = f.ws.clone();
        let names = labels(&cs);
        // The agent runs at the workspace root; the repos are subfolders of it.
        let task = format!(
            "{prompt}\n\n(Repositories live in these subfolders: {}. Only change files inside them.)",
            ws.repos.iter().map(|r| format!("{r}/")).collect::<Vec<_>>().join(", ")
        );
        self.close_modal(cx);
        self.toast(ToastKind::Info, format!("Starting a race in {}…", ws.name), Some(format!("{} agents", cs.len())), window, cx);
        let parent = ws.name.clone();
        spawn(
            window,
            cx,
            move || {
                cs.iter()
                    .zip(&names)
                    .map(|(c, label)| (c.clone(), label.clone(), orbit_core::workspace::create_variant(&parent, label, &format!("{} · {}", c.agent, c.model))))
                    .collect::<Vec<_>>()
            },
            move |t, results, window, cx| {
                let mut started = vec![];
                for (c, label, res) in results {
                    match res {
                        Ok(v) => started.push((v.name, c, label)),
                        Err(e) => t.toast(ToastKind::Error, format!("Couldn't start {label}"), Some(e), window, cx),
                    }
                }
                let n = started.len();
                let task = task.clone();
                t.with_app(window, cx, move |a, w, cx| {
                    super::reload_workspaces(a, None, cx);
                    for (name, c, label) in &started {
                        crate::sessions::new_prompted_session(a, name, &c.agent, &c.model, &task, Some(label), true, w, cx);
                    }
                });
                if n > 0 {
                    t.toast(ToastKind::Success, format!("Race started: {}", super::plural(n, "agent")), Some("Compare them in the workspace when they finish".into()), window, cx);
                }
                t.reload(false, window, cx);
            },
        );
    }

    pub(super) fn render_race(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Modal::Race(f)) = &self.modal else { return div().into_any_element() };
        let ready = !f.prompt.read(cx).value().trim().is_empty() && f.contestants.len() >= 2 && f.contestants.iter().all(|c| !c.model.trim().is_empty());
        let select = |id: String, label: String, w: Option<f32>| {
            div()
                .id(gpui::ElementId::Name(id.clone().into()))
                .relative()
                .when_some(w, |d, w| d.w(px(w)))
                .when(w.is_none(), |d| d.flex_1())
                .h(px(34.))
                .px(px(10.))
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(6.))
                .border_1()
                .border_color(c(theme::BORDER_STRONG))
                .bg(c(theme::BG))
                .text_size(px(13.))
                .text_color(c(theme::TEXT))
                .cursor_pointer()
                .hover(|s| s.border_color(c(theme::ACCENT)))
                .child(div().truncate().child(label))
                .child(div().text_size(px(9.)).text_color(c(theme::TEXT_FAINT)).child("▾"))
                .child(probe(id))
        };
        let rows = f.contestants.iter().enumerate().map(|(i, ct)| {
            let can_remove = f.contestants.len() > 2;
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    select(format!("race-agent:{i}"), agent_label(&ct.agent).to_string(), Some(160.))
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.race_pick(i, false, w, cx))),
                )
                .child(
                    select(format!("race-model:{i}"), if ct.model.is_empty() { "Pick a model".into() } else { ct.model.clone() }, None)
                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.race_pick(i, true, w, cx))),
                )
                .child(
                    kit::hint(kit::disabled(kit::icon_button(format!("race-remove:{i}"), "trash", 28., 13., true), !can_remove), "Remove agent").when(can_remove, |b| {
                        b.on_click(cx.listener(move |t, _: &ClickEvent, _, cx| {
                            if let Some(f) = t.race_form() {
                                f.contestants.remove(i);
                            }
                            cx.notify();
                        }))
                    }),
                )
        });
        let body = div()
            .id("race-body")
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(16.))
            .py(px(18.))
            .overflow_y_scroll()
            .child(div().text_size(px(13.)).line_height(px(19.)).text_color(c(theme::TEXT_DIM)).child(format!(
                "Each agent gets its own copy of this workspace ({}), starting from the current {}, and works on the same task. Compare what they did, then merge the winner back. Claude agents auto-approve file edits; commands still ask.",
                f.ws.repos.join(", "),
                f.ws.branch
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_DIM)).child("Task"))
                    .child(div().id("race-task").relative().child(Textarea::new(&f.prompt)).child(probe("race-task"))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(div().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_DIM)).child("AGENTS"))
                    .children(rows)
                    .when(f.contestants.len() < 4, |d| {
                        d.child(
                            div()
                                .id("race-add")
                                .relative()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .py(px(4.))
                                .text_size(px(13.))
                                .text_color(c(theme::ACCENT))
                                .cursor_pointer()
                                .child(icon("plus", 12., c(theme::ACCENT)))
                                .child("Add agent")
                                .child(probe("race-add"))
                                .on_click(cx.listener(|t, _: &ClickEvent, _, cx| {
                                    if let Some(f) = t.race_form() {
                                        f.contestants.push(Contestant { agent: "claude".into(), model: String::new() });
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            );
        kit::modal("race-modal", 600.)
            .child(kit::modal_header(format!("Race agents · {}", f.ws.name)))
            .child(body)
            .child(
                kit::modal_footer()
                    .child(kit::button("modal-cancel", "Cancel", Kind::Secondary, kit::NORMAL).on_click(cx.listener(|t, _: &ClickEvent, _, cx| t.close_modal(cx))))
                    .child(
                        kit::disabled(kit::button("race-start", format!("Start {} agents", f.contestants.len()), Kind::Primary, kit::NORMAL), !ready)
                            .when(ready, |b| b.on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.race_start(w, cx)))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_unique() {
        let c = |a: &str| Contestant { agent: a.into(), model: "m".into() };
        assert_eq!(labels(&[c("claude"), c("opencode"), c("claude")]), ["claude", "opencode", "claude-2"]);
    }
}
