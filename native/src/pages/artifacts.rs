//! CI artifacts of a workspace's branches (`ArtifactsSection.tsx`): the
//! latest run of each workflow per repo; Download extracts one into
//! `~/Downloads/Orbit/<workspace>/` and opens the folder.
use super::ui::{self, Tone};
use super::{Host, PageView};
use crate::review::Load;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, App, ClickEvent, Context, SharedString, Window};
use orbit_core::artifacts::{self, Artifact};
use std::collections::HashMap;
use std::fmt::Write as _;

pub struct Source {
    repo: String,
    owner_repo: Result<String, String>,
    branch: String,
    list: Load<Vec<Artifact>>,
}

pub struct ArtifactsPage {
    host: Host,
    workspace: String,
    sources: Vec<Source>,
    /// Download state per artifact id: false while downloading, true once done.
    busy: HashMap<u64, bool>,
}

fn size(bytes: u64) -> String {
    match bytes {
        1_073_741_824.. => format!("{:.1} GB", bytes as f64 / 1_073_741_824.),
        1_048_576.. => format!("{:.1} MB", bytes as f64 / 1_048_576.),
        1024.. => format!("{:.0} KB", bytes as f64 / 1024.),
        _ => format!("{bytes} B"),
    }
}

impl ArtifactsPage {
    pub fn new(host: Host, ws: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sources = ws
            .meta
            .repos
            .iter()
            .map(|r| Source { repo: r.clone(), owner_repo: Err(String::new()), branch: ws.meta.branch_of(r).to_string(), list: Load::Loading })
            .collect();
        let mut page = Self { host, workspace: ws.meta.name.clone(), sources, busy: HashMap::new() };
        page.load(window, cx);
        page
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let jobs: Vec<(String, String)> = self.sources.iter().map(|s| (s.repo.clone(), s.branch.clone())).collect();
        for s in &mut self.sources {
            s.list = Load::Loading;
        }
        ui::bg(
            cx,
            move || {
                // One gh round trip per repo, side by side.
                std::thread::scope(|scope| {
                    let handles: Vec<_> = jobs
                        .iter()
                        .map(|(repo, branch)| {
                            let ws = &ws;
                            scope.spawn(move || {
                                let owner = orbit_core::prs::owner_repo(ws, repo);
                                let list = owner.as_ref().map(|o| artifacts::branch_artifacts(o, branch)).map_err(Clone::clone);
                                (owner, list)
                            })
                        })
                        .collect();
                    handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (Err("thread panicked".into()), Err("thread panicked".into())))).collect::<Vec<_>>()
                })
            },
            |t: &mut Self, results, _, _| {
                for (s, (owner, list)) in t.sources.iter_mut().zip(results) {
                    s.owner_repo = owner;
                    s.list = list.map_or_else(Load::Failed, Load::Ready);
                }
            },
            window,
        );
    }

    fn download(&mut self, owner_repo: String, a: Artifact, window: &mut Window, cx: &mut Context<Self>) {
        self.busy.insert(a.id, false);
        let folder = self.workspace.clone();
        let (id, name, run_id) = (a.id, a.name.clone(), a.run_id);
        ui::bg(
            cx,
            move || artifacts::download(&owner_repo, run_id, &name, &folder),
            move |t: &mut Self, r, _, cx| match r {
                Ok(dir) => {
                    t.busy.insert(id, true);
                    t.host.toast(ToastKind::Success, format!("Downloaded {}", a.name), Some(dir.display().to_string()), cx);
                    if !crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                        cx.reveal_path(&dir);
                    }
                }
                Err(e) => {
                    t.busy.remove(&id);
                    t.host.error(format!("Couldn't download {}", a.name), e, cx);
                }
            },
            window,
        );
        cx.notify();
    }
}

impl Render for ArtifactsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = ui::page(
            "artifacts-body",
            format!("CI artifacts · {}", self.workspace),
            "Builds GitHub Actions already produced for this workspace's branches (the latest run of each workflow), to try a feature without compiling it.",
        )
        .child(div().child(ui::button("artifacts-refresh", "Refresh", Tone::Plain).on_click(cx.listener(|t, _: &ClickEvent, w, cx| t.load(w, cx)))));
        for s in &self.sources {
            page = page.child(ui::heading(format!("{} · {}", s.repo, s.branch)));
            let owner = match &s.owner_repo {
                Ok(o) => o.clone(),
                Err(e) if !e.is_empty() => {
                    page = page.child(ui::muted(e.clone()));
                    continue;
                }
                Err(_) => String::new(),
            };
            page = match &s.list {
                Load::Ready(list) if list.is_empty() => page.child(ui::muted("No artifacts from the latest runs of this branch.")),
                Load::Ready(list) => {
                    let mut card = ui::card();
                    for a in list {
                        let state = self.busy.get(&a.id).copied();
                        let (o, art) = (owner.clone(), a.clone());
                        card = card.child(
                            div()
                                .px_4()
                                .py_2()
                                .flex()
                                .items_center()
                                .gap_3()
                                .border_b_1()
                                .border_color(c(theme::BORDER))
                                .child(div().flex_1().min_w_0().flex().flex_col().child(div().text_sm().text_color(c(theme::TEXT)).child(a.name.clone())).child(
                                    ui::faint(format!("{} · {} · {} · {}", a.workflow, size(a.size_bytes), a.created_at, a.head_sha.chars().take(7).collect::<String>())),
                                ))
                                .child(
                                    ui::button(
                                        SharedString::from(format!("artifact:{}:{}", s.repo, a.name)),
                                        match state {
                                            Some(false) => "Downloading…",
                                            Some(true) => "Download again",
                                            None => "Download",
                                        },
                                        Tone::Plain,
                                    )
                                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| {
                                        if t.busy.get(&art.id) != Some(&false) {
                                            t.download(o.clone(), art.clone(), w, cx)
                                        }
                                    })),
                                ),
                        );
                    }
                    page.child(card)
                }
                Load::Failed(e) => page.child(ui::error(e.clone())),
                _ => page.child(ui::faint("Loading…")),
            };
        }
        page
    }
}

impl PageView for ArtifactsPage {
    fn title(&self) -> SharedString {
        format!("CI artifacts · {}", self.workspace).into()
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(window, cx);
    }

    fn dump(&self, out: &mut String, _: &App) {
        for s in &self.sources {
            let _ = writeln!(out, "  {} {:?} @ {}: {}", s.repo, s.owner_repo, s.branch, ui::load_label(&s.list));
            for a in s.list.ready().into_iter().flatten() {
                let st = match self.busy.get(&a.id) {
                    Some(false) => " downloading",
                    Some(true) => " downloaded",
                    None => "",
                };
                let _ = writeln!(out, "    {} run {} {} {}{st}", a.name, a.run_id, a.workflow, size(a.size_bytes));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_read_well() {
        assert_eq!(super::size(512), "512 B");
        assert_eq!(super::size(2048), "2 KB");
        assert_eq!(super::size(5 * 1_048_576), "5.0 MB");
    }
}
