//! The bottom tool window (`BottomPanel.tsx`): Build output with links to
//! problems, the running programs' terminals, Problems and Debug; with more
//! than one run or debug session, a strip of instance tabs picks one. The shell
//! sizes it (`layout.panel_height`, resizable and saved) and shows it when
//! `layout.panel_open`.
use super::{open_location, store, BuildStatus, Stream};
use crate::app::OrbitApp;
use crate::debug::{self, view::DebugView, DebugStatus};
use crate::logview::{Link, LogView};
use crate::probe::probe;
use crate::run::problems::Severity;
use crate::theme::{self, c, size};
use gpui::{div, prelude::*, px, AnyElement, App, Context, Entity, Subscription, WeakEntity, Window};
use std::rc::Rc;

/// A tab of the instance strip: name, dot colour, selected, select, close (stop when live).
type Instance = (String, u32, bool, Rc<dyn Fn(&mut App)>, Rc<dyn Fn(&mut App)>);

fn debug_color(status: DebugStatus) -> u32 {
    match status {
        DebugStatus::Stopped => theme::WARN,
        DebugStatus::Starting | DebugStatus::Running => theme::INFO,
        _ => theme::TEXT_FAINT,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelView {
    Build,
    Run,
    Problems,
    Debug,
}

pub struct Panel {
    app: WeakEntity<OrbitApp>,
    debug: Entity<DebugView>,
    output: Entity<LogView>,
    /// The build the output shows.
    shown_build: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Panel {
    pub fn new(app: WeakEntity<OrbitApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&store(cx), |_, _, cx| cx.notify()), cx.observe(&debug::store(cx), |_, _, cx| cx.notify())];
        Self {
            debug: cx.new(|cx| DebugView::new(app.clone(), window, cx)),
            app,
            output: cx.new(|cx| LogView::new("build-output", super::MAX_LINES, cx)),
            shown_build: None,
            _subscriptions: subs,
        }
    }

    fn tab(&self, id: &'static str, view: PanelView, active: bool, label: AnyElement, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(id)
            .relative()
            .h_full()
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .cursor_pointer()
            .border_b_2()
            .border_color(if active { c(theme::ACCENT) } else { gpui::transparent_black().into() })
            .text_color(c(if active { theme::TEXT } else { theme::TEXT_DIM }))
            .hover(|s| s.text_color(c(theme::TEXT)))
            .child(label)
            .child(probe(id))
            .on_click(cx.listener(move |_, _, _, cx| store(cx).update(cx, |s, cx| s.set_view(view, cx))))
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (view, build, run, counts) = {
            let s = store(cx).read(cx);
            let run = s.shown().map(|r| (r.id(), r.name.clone(), r.exited));
            (s.view, s.build_status(), run, s.counts())
        };
        let debug_status = debug::active(cx).map_or(DebugStatus::Idle, |s| s.read(cx).status);
        // `.panel-status`; `-running` is info and pulses (`panel-pulse`, 1s).
        let dot = |id: &'static str, color: u32| -> gpui::AnyElement {
            let d = div().size(px(7.)).flex_none().rounded_full().bg(c(color));
            if color == theme::INFO { crate::motion::blink(d, id, crate::motion::PANEL_PULSE, 0.35) } else { d.into_any_element() }
        };
        let build_color = match build.map(|b| b.0) {
            Some(BuildStatus::Running) => theme::INFO,
            Some(BuildStatus::Ok) => theme::OK,
            Some(BuildStatus::Failed) => theme::DANGER,
            Some(BuildStatus::Cancelled) => theme::WARN,
            None => theme::TEXT_FAINT,
        };
        let run_label = match &run {
            Some((_, name, _)) => format!("Run · {name}"),
            None => "Run".into(),
        };
        let run_color = if store(cx).read(cx).is_running() { theme::INFO } else { theme::TEXT_FAINT };
        let debug_color = debug_color(debug_status);
        let count = |n: usize, color: u32| {
            (n > 0).then(|| div().px_1().rounded(px(8.)).text_xs().bg(c(color)).text_color(c(theme::BG)).child(n.to_string()))
        };
        let meta = match build {
            Some((BuildStatus::Running, _)) => "Building…".to_string(),
            Some((BuildStatus::Ok, d)) => format!("Build succeeded · {}s", d.map_or(0, |d| d.as_secs_f32().round() as u64)),
            Some((BuildStatus::Failed, _)) => "Build failed".into(),
            Some((BuildStatus::Cancelled, _)) => "Build cancelled".into(),
            None => String::new(),
        };
        let exited_run = run.filter(|r| r.2).map(|r| r.0);
        div()
            .h(px(32.))
            .flex_none()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(size::TAB_FONT))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(self.tab("panel-build", PanelView::Build, view == PanelView::Build, div().flex().items_center().gap_1p5().child(dot("panel-dot-build", build_color)).child("Build").into_any_element(), cx))
            .child(self.tab("panel-run", PanelView::Run, view == PanelView::Run, div().flex().items_center().gap_1p5().child(dot("panel-dot-run", run_color)).child(run_label).into_any_element(), cx))
            .child(self.tab("panel-debug", PanelView::Debug, view == PanelView::Debug, div().flex().items_center().gap_1p5().child(dot("panel-dot-debug", debug_color)).child("Debug").into_any_element(), cx))
            .child(self.tab(
                "panel-problems",
                PanelView::Problems,
                view == PanelView::Problems,
                div().flex().items_center().gap_1p5().child("Problems").children(count(counts.0, theme::DANGER)).children(count(counts.1, theme::WARN)).into_any_element(),
                cx,
            ))
            .child(div().flex_1())
            .when(view == PanelView::Build, |d| d.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(meta)))
            .when_some(exited_run.filter(|_| view == PanelView::Run), |d, id| {
                d.child(
                    div()
                        .id("panel-clear")
                        .relative()
                        .px_2()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(c(theme::TEXT_DIM))
                        .hover(|s| s.bg(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT)))
                        .child("Clear")
                        .child(probe("panel-clear"))
                        .on_click(cx.listener(move |_, _, _, cx| store(cx).update(cx, |s, cx| s.close_run(id, cx)))),
                )
            })
            .child(
                div()
                    .id("panel-close")
                    .relative()
                    .px_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(c(theme::TEXT_FAINT))
                    .hover(|s| s.bg(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT)))
                    .child("×")
                    .child(probe("panel-close"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let _ = this.app.update(cx, |a, cx| a.toggle_panel(cx));
                    })),
            )
    }

    /// The Run or Debug tab's instances, when there is more than one.
    fn render_instances(&self, view: PanelView, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items: Vec<Instance> = match view {
            PanelView::Run => {
                let s = store(cx).read(cx);
                let shown = s.shown().map(|r| r.id());
                s.runs
                    .iter()
                    .map(|r| {
                        let id = r.id();
                        let select: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| store(cx).update(cx, |s, cx| s.show_run(id, cx)));
                        let close: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| store(cx).update(cx, |s, cx| s.close_run(id, cx)));
                        (r.name.clone(), if r.exited { theme::TEXT_FAINT } else { theme::INFO }, Some(id) == shown, select, close)
                    })
                    .collect()
            }
            PanelView::Debug => {
                let d = debug::store(cx).read(cx);
                d.sessions
                    .iter()
                    .map(|e| {
                        let (a, b) = (e.clone(), e.clone());
                        let select: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| debug::store(cx).update(cx, |d, cx| d.activate(a.clone(), cx)));
                        let close: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| debug::store(cx).update(cx, |d, cx| d.close(b.clone(), cx)));
                        let s = e.read(cx);
                        (s.name.clone(), debug_color(s.status), d.active.as_ref() == Some(e), select, close)
                    })
                    .collect()
            }
            _ => return None,
        };
        if items.len() < 2 {
            return None;
        }
        let tabs = items.into_iter().enumerate().map(|(ix, (name, color, active, select, close))| {
            div()
                .id(("panel-instance", ix))
                .relative()
                .h_full()
                .pl_2()
                .pr_1()
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .border_b_2()
                .border_color(if active { c(theme::ACCENT) } else { gpui::transparent_black().into() })
                .text_color(c(if active { theme::TEXT } else { theme::TEXT_DIM }))
                .hover(|s| s.text_color(c(theme::TEXT)))
                .child(div().size(px(6.)).flex_none().rounded_full().bg(c(color)))
                .child(name.clone())
                .child(
                    div()
                        .id(("panel-instance-close", ix))
                        .relative()
                        .px_1()
                        .rounded(px(3.))
                        .text_color(c(theme::TEXT_FAINT))
                        .hover(|s| s.bg(c(theme::BORDER_STRONG)).text_color(c(theme::TEXT)))
                        .child("×")
                        .child(probe(format!("instance-close:{name}")))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            close(cx);
                        }),
                )
                .child(probe(format!("instance:{name}")))
                .on_click(move |_, _, cx| select(cx))
        });
        Some(
            div()
                .h(px(26.))
                .flex_none()
                .px_2()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .border_b_1()
                .border_color(c(theme::BORDER))
                .children(tabs)
                .into_any_element(),
        )
    }

    /// The output views, for the self-test dump.
    pub fn dump(&self, cx: &App) -> String {
        let d = self.debug.read(cx);
        format!("--- panel\n  {}\n  {}\n  debug tab {:?}\n", self.output.read(cx).dump(), d.console.read(cx).dump(), d.tab)
    }

    fn empty(text: &'static str) -> AnyElement {
        div().size_full().flex().items_center().justify_center().text_color(c(theme::TEXT_FAINT)).child(text).into_any_element()
    }

    fn render_output(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some((key, total, tail)) = store(cx).read(cx).build().map(|b| {
            let seen = if self.shown_build.as_deref() == Some(b.key.as_str()) { self.output.read(cx).seen() } else { 0 };
            let fresh = b.total.saturating_sub(seen).min(b.lines.len());
            let tail: Vec<(String, u32, Option<Link>)> = b.lines[b.lines.len() - fresh..]
                .iter()
                .map(|l| {
                    let problem = l.problem.and_then(|p| b.problems.get(p));
                    let color = match (l.stream, problem.map(|p| p.problem.severity)) {
                        (_, Some(Severity::Error)) => theme::DANGER,
                        (_, Some(Severity::Warning)) => theme::WARN,
                        (Stream::Info, _) => theme::TEXT_FAINT,
                        (Stream::Err, _) => theme::TEXT_DIM,
                        (Stream::Out, _) => theme::TEXT,
                    };
                    let link = problem.and_then(|p| Some((p.open.clone()?, p.problem.line, p.problem.column))).map(|(path, line, col)| {
                        Rc::new(move |_: &mut Window, cx: &mut App| open_location(path.clone(), line, col, cx)) as Link
                    });
                    (l.raw.to_string(), color, link)
                })
                .collect();
            (b.key.clone(), b.total, tail)
        }) else {
            return Self::empty("No build yet · Ctrl+F9 builds the selected configuration");
        };
        if self.shown_build.as_deref() != Some(key.as_str()) {
            self.shown_build = Some(key);
            self.output.update(cx, |v, cx| v.clear(cx));
        }
        // Build lines never change once printed: `tail` holds only the new ones.
        self.output.update(cx, |v, cx| v.sync(total, &tail, |(text, color, link)| (text.clone(), *color, link.clone()), cx));
        let output = div().flex_1().min_w_0().h_full().child(self.output.clone());
        div().size_full().flex().children(self.render_builds(cx)).child(output).into_any_element()
    }

    /// With several builds (a compound's), the list beside the output: status, time; a click shows its output.
    fn render_builds(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = store(cx).read(cx);
        if s.builds.len() < 2 {
            return None;
        }
        let shown = s.build().map(|b| b.key.clone());
        let several_repos = s.builds.iter().any(|b| b.target.repo != s.builds[0].target.repo);
        let rows = s.builds.iter().enumerate().map(|(ix, b)| {
            let (icon, color) = match b.status {
                BuildStatus::Running => ("●", theme::INFO),
                BuildStatus::Ok => ("✓", theme::OK),
                BuildStatus::Failed => ("⨯", theme::DANGER),
                BuildStatus::Cancelled => ("◌", theme::WARN),
            };
            let name = if b.all { format!("{} · all", b.target.repo) } else { b.target.name.clone() };
            let time = match b.elapsed {
                Some(d) => format!("{:.1}s", d.as_secs_f32()),
                None => format!("{}s", b.started.elapsed().as_secs()),
            };
            let (errors, _) = b.counts();
            let active = shown.as_ref() == Some(&b.key);
            let key = b.key.clone();
            div()
                .id(("build-item", ix))
                .relative()
                .px_2()
                .py(px(3.))
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .when(active, |d| d.bg(c(theme::ACCENT_SOFT)))
                .hover(|d| d.bg(c(theme::PANEL_HOVER)))
                .child(div().w(px(12.)).flex_none().text_color(c(color)).child(icon))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().text_color(c(if active { theme::TEXT } else { theme::TEXT_DIM })).child(name))
                        .when(several_repos, |d| d.child(div().truncate().text_xs().text_color(c(theme::TEXT_FAINT)).child(b.target.repo.clone()))),
                )
                .when(errors > 0, |d| d.child(div().text_xs().text_color(c(theme::DANGER)).child(errors.to_string())))
                .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(time))
                .child(probe(format!("build-item:{}", b.target.name)))
                .on_click(move |_, _, cx| store(cx).update(cx, |s, cx| s.show_build(key.clone(), cx)))
        });
        Some(
            div()
                .id("build-list")
                .w(px(220.))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .py_1()
                .text_size(px(12.5))
                .border_r_1()
                .border_color(c(theme::BORDER))
                .children(rows)
                .into_any_element(),
        )
    }

    fn render_problems(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = store(cx).read(cx);
        // Every build's, numbered across them.
        let all: Vec<(&super::Build, &super::BuildProblem)> = s.builds.iter().flat_map(|b| b.problems.iter().map(move |p| (b, p))).collect();
        if all.is_empty() {
            return Self::empty("No problems in the last build");
        }
        // Grouped by file, errors first.
        let mut order: Vec<usize> = (0..all.len()).collect();
        order.sort_by_key(|&i| all[i].1.problem.severity != Severity::Error);
        let mut groups: Vec<(String, Vec<usize>)> = vec![];
        for i in order {
            let (b, p) = all[i];
            let file = match &p.open {
                Some(o) => o.strip_prefix(&b.linked_root).map(|r| format!("{}/{}", b.target.repo, r.display())).unwrap_or_else(|_| o.display().to_string()),
                None => p.problem.file.clone().unwrap_or_else(|| "Linker".into()),
            };
            match groups.iter_mut().find(|(f, _)| *f == file) {
                Some((_, list)) => list.push(i),
                None => groups.push((file, vec![i])),
            }
        }
        let rows: Vec<AnyElement> = groups
            .into_iter()
            .flat_map(|(file, list)| {
                let header = div().px_3().pt_2().pb_1().text_xs().text_color(c(theme::TEXT_FAINT)).child(file).into_any_element();
                let items = list.into_iter().map(|i| {
                    let p = all[i].1;
                    let q = &p.problem;
                    let (icon, color) = if q.severity == Severity::Error { ("⨯", theme::DANGER) } else { ("⚠", theme::WARN) };
                    let target = p.open.clone().map(|o| (o, q.line, q.column));
                    div()
                        .id(("problem", i))
                        .relative()
                        .px_3()
                        .py(px(3.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_color(c(color)).child(icon))
                        .child(div().flex_1().truncate().text_color(c(theme::TEXT)).child(q.message.clone()))
                        .children(q.code.clone().map(|code| div().text_xs().text_color(c(theme::TEXT_FAINT)).child(code)))
                        .when(q.line > 0, |d| d.child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child(format!("{}:{}", q.line, q.column))))
                        .child(probe(format!("problem:{i}")))
                        .when_some(target, |d, (path, line, col)| {
                            d.cursor_pointer().hover(|s| s.bg(c(theme::PANEL_HOVER))).on_click(move |_, _, cx| open_location(path.clone(), line, col, cx))
                        })
                        .into_any_element()
                });
                std::iter::once(header).chain(items).collect::<Vec<_>>()
            })
            .collect();
        div().id("panel-problems-list").size_full().overflow_y_scroll().children(rows).into_any_element()
    }
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = store(cx).read(cx).view;
        let body = match view {
            PanelView::Build => self.render_output(cx),
            PanelView::Problems => self.render_problems(cx),
            PanelView::Run => match store(cx).read(cx).shown() {
                Some(r) => div().size_full().child(r.view.clone()).into_any_element(),
                None => Self::empty("Nothing running · Shift+F10 builds and runs the selected configuration"),
            },
            PanelView::Debug => self.debug.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::PANEL))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .child(self.render_header(cx))
            .children(self.render_instances(view, cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}
