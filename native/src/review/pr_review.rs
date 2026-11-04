//! The PR review tab (`PrReviewPane.tsx`): the PR's header, overview and
//! files on the left; the overview or a file's diff with its review threads
//! on the right. Comments go out at once or wait in a pending review that is
//! submitted with a verdict.
use super::pr_ui::{self, lucide, tint, Tone};
use super::Load;
use crate::app::OrbitApp;
use crate::pages::{Host, PageView};
use crate::motion::skeleton;
use crate::probe::probe;
use crate::theme::{self, c};
use crate::toast::ToastKind;
use gpui::{
    div, prelude::*, px, AnyElement, App, Context, ElementId, Entity, FontWeight, MouseButton, SharedString, Subscription, Window,
};
use gpui_component::diff::{Diff, DiffAnnotation, DiffDecoration, DiffFile, DiffLinePosition, DiffLineRange, DiffMode, DiffSide, DiffState};
use gpui_component::highlighter::SyntaxHighlighter;
use gpui_component::input::{InputEvent, Rope, Textarea, TextareaState};
use gpui_component::text::TextView;
use gpui_component::ActiveTheme;
use orbit_core::github::{PrDetail, PullRequest};
use orbit_core::review::{DraftComment, ReviewData, ReviewThread};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const SIDE_W: f32 = 260.;

/// A pending comment, kept until the review is submitted.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Draft {
    pub id: String,
    pub path: String,
    /// RIGHT | LEFT
    pub side: String,
    pub line: u64,
    pub start_line: Option<u64>,
    pub body: String,
}

/// What survives a restart for one PR: viewed files and pending comments.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    viewed: Vec<String>,
    #[serde(default)]
    drafts: Vec<Draft>,
}

fn store_path() -> Option<PathBuf> {
    orbit_core::config::config_dir().ok().map(|d| d.join("pr-review.json"))
}

fn load_saved(key: &str) -> Saved {
    let Some(p) = store_path() else { return Saved::default() };
    let all: HashMap<String, Saved> = std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    all.get(key).cloned().unwrap_or_default()
}

fn save(key: &str, saved: &Saved) {
    let Some(p) = store_path() else { return };
    let mut all: HashMap<String, Saved> = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if saved.viewed.is_empty() && saved.drafts.is_empty() {
        all.remove(key);
    } else {
        all.insert(key.to_string(), saved.clone());
    }
    if let Ok(text) = serde_json::to_string_pretty(&all) {
        let _ = std::fs::write(p, text);
    }
}

/// A unified patch of `path` between two texts (git's own diff).
fn patch_of(path: &str, original: &str, modified: &str) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("orbit-pr-{}-{}", std::process::id(), pr_ui::now()));
    let (a, b) = (dir.join("base").join(path), dir.join("head").join(path));
    let write = |p: &Path, t: &str| -> std::io::Result<()> {
        std::fs::create_dir_all(p.parent().unwrap_or(&dir))?;
        std::fs::write(p, t)
    };
    write(&a, original).and_then(|_| write(&b, modified)).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("git")
        .args(["diff", "--no-index", "--no-color", "--no-ext-diff", "--unified=1000000", "--"])
        .arg(format!("base/{path}"))
        .arg(format!("head/{path}"))
        .current_dir(&dir)
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let out = out.map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .lines()
        .map(|l| {
            if l.starts_with("diff --git ") || l.starts_with("--- ") || l.starts_with("+++ ") {
                l.replace("a/base/", "a/").replace("b/head/", "b/")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n")
}

#[derive(Clone, Debug, PartialEq)]
enum Sel {
    Overview,
    File(String),
}

#[derive(Clone, Debug, PartialEq)]
enum ComposerKind {
    New { side: DiffSide, start: u64, end: u64 },
    Reply(String),
}

struct Composer {
    kind: ComposerKind,
    input: Entity<TextareaState>,
    _sub: Subscription,
}

const EVENTS: [(&str, &str, &str); 3] = [
    ("COMMENT", "Comment", "General feedback without explicit approval."),
    ("APPROVE", "Approve", "Give your approval to merge these changes."),
    ("REQUEST_CHANGES", "Request changes", "Feedback that must be addressed before merging."),
];

struct Submit {
    event: &'static str,
    body: Entity<TextareaState>,
}

pub struct PrReviewPage {
    host: Host,
    pub prs: Vec<PullRequest>,
    pub active: usize,
    pub detail: Load<PrDetail>,
    pub review: Option<ReviewData>,
    selected: Sel,
    pub diff: Load<Entity<DiffState>>,
    pub sel: crate::diff_select::Handle,
    split: bool,
    saved: Saved,
    hide_viewed: bool,
    composer: Option<Composer>,
    submit: Option<Submit>,
    busy: bool,
}

fn side_of(s: &str) -> DiffSide {
    if s == "LEFT" {
        DiffSide::Original
    } else {
        DiffSide::Modified
    }
}

fn side_name(s: DiffSide) -> &'static str {
    match s {
        DiffSide::Original => "LEFT",
        DiffSide::Modified => "RIGHT",
    }
}

impl PrReviewPage {
    pub fn new(host: Host, prs: Vec<PullRequest>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            host,
            prs,
            active: 0,
            detail: Load::Idle,
            review: None,
            selected: Sel::Overview,
            diff: Load::Idle,
            sel: crate::diff_select::new(),
            split: false,
            saved: Saved::default(),
            hide_viewed: false,
            composer: None,
            submit: None,
            busy: false,
        };
        page.load(window, cx);
        page
    }

    fn pr(&self) -> &PullRequest {
        &self.prs[self.active]
    }

    fn key(&self) -> String {
        format!("{}#{}", self.pr().owner_repo, self.pr().number)
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.detail = Load::Loading;
        self.review = None;
        self.selected = Sel::Overview;
        self.diff = Load::Idle;
        self.composer = None;
        self.saved = load_saved(&self.key());
        let (or, n) = (self.pr().owner_repo.clone(), self.pr().number);
        cx.spawn_in(window, async move |this, cx| {
            let d = cx.background_spawn(async move { orbit_core::github::pr_detail(&or, n) }).await;
            let _ = this.update(cx, |p, cx| {
                if let Err(e) = &d {
                    p.host.error("Couldn't load the pull request", e.clone(), cx);
                }
                p.detail = d.map_or_else(Load::Failed, Load::Ready);
                cx.notify();
            });
        })
        .detach();
        self.load_review(true, window, cx);
        cx.notify();
    }

    fn load_review(&mut self, with_lines: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (or, n) = (self.pr().owner_repo.clone(), self.pr().number);
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { orbit_core::review::review_data(&or, n, with_lines) }).await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(mut data) => {
                        if !with_lines {
                            if let Some(prev) = &p.review {
                                data.commentable = prev.commentable.clone();
                            }
                        }
                        p.review = Some(data);
                    }
                    Err(e) => p.host.error("Couldn't load review threads", e, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select(&mut self, sel: Sel, window: &mut Window, cx: &mut Context<Self>) {
        self.composer = None;
        self.selected = sel.clone();
        let (Sel::File(path), Some(detail)) = (sel, self.detail.ready()) else {
            self.diff = Load::Idle;
            return cx.notify();
        };
        self.diff = Load::Loading;
        let (or, head, base) = (self.pr().owner_repo.clone(), detail.head_sha.clone(), detail.base_sha.clone());
        let highlight_theme = cx.theme().highlight_theme.clone();
        let split = self.split;
        cx.spawn_in(window, async move |this, cx| {
            let p2 = path.clone();
            let patch = cx
                .background_spawn(async move {
                    let d = orbit_core::github::pr_file_diff(&or, &head, &base, &p2)?;
                    patch_of(&p2, &d.original, &d.modified)
                })
                .await;
            let _ = this.update(cx, |page, cx| {
                if page.selected != Sel::File(path.clone()) {
                    return;
                }
                let language = crate::lsp::file_language(Path::new(&path)).highlight;
                let files = patch.and_then(|p| DiffFile::parse(&p).map(|f| (f, p)).map_err(|e| e.message().to_string()));
                page.diff = match files {
                    Ok((files, patch)) => {
                        let files: Vec<DiffFile> = files.into_iter().map(|f| f.with_language(language)).collect();
                        page.sel.borrow_mut().set_files(&files, &patch, &[DiffSide::Original, DiffSide::Modified]);
                        let mode = if split { DiffMode::Split } else { DiffMode::Unified };
                        let theme = highlight_theme.clone();
                        Load::Ready(cx.new(|cx| {
                            DiffState::new(files, cx).with_mode(mode).with_syntax_highlighter(move |language, text| {
                                let mut h = SyntaxHighlighter::new(language);
                                h.update(None, &Rope::from(text), None);
                                h.styles(&(0..text.len()), &*theme)
                            })
                        }))
                    }
                    Err(e) => Load::Failed(e),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        self.split = split;
        if let Load::Ready(d) = &self.diff {
            d.update(cx, |d, cx| d.set_mode(if split { DiffMode::Split } else { DiffMode::Unified }, cx));
        }
        cx.notify();
    }

    fn persist(&self) {
        save(&self.key(), &self.saved);
    }

    fn mark_viewed(&mut self, path: &str, on: bool, advance: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.saved.viewed.retain(|p| p != path);
        if on {
            self.saved.viewed.push(path.to_string());
        }
        self.persist();
        if on && advance {
            let files: Vec<String> = self.detail.ready().map(|d| d.files.iter().map(|f| f.path.clone()).collect()).unwrap_or_default();
            if let Some(at) = files.iter().position(|f| f == path) {
                let next = files[at + 1..].iter().chain(&files[..at]).find(|f| !self.saved.viewed.contains(f)).cloned();
                if let Some(next) = next {
                    return self.select(Sel::File(next), window, cx);
                }
            }
        }
        cx.notify();
    }

    fn open_composer(&mut self, kind: ComposerKind, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = if matches!(kind, ComposerKind::Reply(_)) { "Reply…" } else { "Leave a comment" };
        let input = cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder).rows(3));
        let sub = cx.subscribe_in(&input, window, |_, _, _: &InputEvent, _, _| {});
        input.update(cx, |i, cx| i.focus(window, cx));
        self.composer = Some(Composer { kind, input, _sub: sub });
        cx.notify();
    }

    fn comment_on(&mut self, range: &DiffLineRange, window: &mut Window, cx: &mut Context<Self>) {
        let (start, end) = (range.start().min(range.end()) as u64, range.start().max(range.end()) as u64);
        self.open_composer(ComposerKind::New { side: range.side(), start, end }, window, cx);
    }

    fn composer_text(&self, cx: &App) -> Option<String> {
        let t = self.composer.as_ref()?.input.read(cx).value().trim().to_string();
        (!t.is_empty()).then_some(t)
    }

    fn add_draft(&mut self, cx: &mut Context<Self>) {
        let (Some(body), Sel::File(path)) = (self.composer_text(cx), self.selected.clone()) else { return };
        let Some(ComposerKind::New { side, start, end }) = self.composer.as_ref().map(|c| c.kind.clone()) else { return };
        self.saved.drafts.push(Draft {
            id: format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)),
            path,
            side: side_name(side).into(),
            line: end,
            start_line: (start < end).then_some(start),
            body,
        });
        self.persist();
        self.composer = None;
        cx.notify();
    }

    /// Posts the composer's comment (or reply) at once.
    fn comment_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(body) = self.composer_text(cx) else { return };
        let Some(kind) = self.composer.as_ref().map(|c| c.kind.clone()) else { return };
        let (or, n) = (self.pr().owner_repo.clone(), self.pr().number);
        let head = self.detail.ready().map(|d| d.head_sha.clone()).unwrap_or_default();
        let reply_to = match &kind {
            ComposerKind::Reply(tid) => self.review.as_ref().and_then(|r| r.threads.iter().find(|t| &t.id == tid)).and_then(|t| t.comments.first()).map(|c| c.id),
            _ => None,
        };
        let comment = match (&kind, &self.selected) {
            (ComposerKind::New { side, start, end }, Sel::File(path)) => Some(DraftComment {
                path: path.clone(),
                side: side_name(*side).into(),
                line: *end,
                start_line: (start < end).then_some(*start),
                body: body.clone(),
            }),
            _ => None,
        };
        self.busy = true;
        cx.spawn_in(window, async move |this, cx| {
            let r = cx
                .background_spawn(async move {
                    match (reply_to, comment) {
                        (Some(id), _) => orbit_core::review::reply(&or, n, id, &body),
                        (None, Some(c)) => orbit_core::review::add_comment(&or, n, &head, &c),
                        _ => Err("nothing to post".into()),
                    }
                })
                .await;
            let _ = this.update_in(cx, |p, window, cx| {
                p.busy = false;
                match r {
                    Ok(()) => {
                        p.composer = None;
                        p.host.toast(ToastKind::Success, "Comment posted", None, cx);
                        p.load_review(false, window, cx);
                    }
                    Err(e) => p.host.error("Couldn't post the comment", e, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn resolve(&mut self, thread: &ReviewThread, window: &mut Window, cx: &mut Context<Self>) {
        let (id, resolved) = (thread.id.clone(), !thread.is_resolved);
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { orbit_core::review::set_thread_resolved(&id, resolved) }).await;
            let _ = this.update_in(cx, |p, window, cx| match r {
                Ok(()) => p.load_review(false, window, cx),
                Err(e) => p.host.error("Couldn't update the thread", e, cx),
            });
        })
        .detach();
    }

    fn toggle_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submit.take().is_none() {
            let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("Leave a comment").rows(4));
            self.submit = Some(Submit { event: "COMMENT", body });
        }
        cx.notify();
    }

    fn submit_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(s), Some(detail)) = (&self.submit, self.detail.ready()) else { return };
        let (event, body) = (s.event, s.body.read(cx).value().to_string());
        let (or, n, head) = (self.pr().owner_repo.clone(), self.pr().number, detail.head_sha.clone());
        let comments: Vec<DraftComment> = self
            .saved
            .drafts
            .iter()
            .map(|d| DraftComment { path: d.path.clone(), side: d.side.clone(), line: d.line, start_line: d.start_line, body: d.body.clone() })
            .collect();
        let count = comments.len();
        let label = format!("{} #{}", self.pr().repo, n);
        self.busy = true;
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_spawn(async move { orbit_core::review::submit_review(&or, n, &head, event, &body, &comments) }).await;
            let _ = this.update_in(cx, |p, window, cx| {
                p.busy = false;
                match r {
                    Ok(()) => {
                        p.saved.drafts.clear();
                        p.persist();
                        p.submit = None;
                        let title = match event {
                            "APPROVE" => "Approved",
                            "REQUEST_CHANGES" => "Changes requested",
                            _ => "Review submitted",
                        };
                        let s = if count == 1 { "" } else { "s" };
                        let desc = if count > 0 { format!("{label} · {count} comment{s}") } else { label };
                        p.host.toast(ToastKind::Success, title, Some(desc), cx);
                        p.load_review(false, window, cx);
                    }
                    Err(e) => p.host.error("Couldn't submit the review", e, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn threads_of(&self, path: &str) -> Vec<&ReviewThread> {
        self.review.iter().flat_map(|r| &r.threads).filter(|t| t.path == path).collect()
    }

    // ---------- left side ----------

    fn render_side(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut side = div()
            .id("pr-side")
            .w(px(SIDE_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .bg(c(theme::PANEL))
            .border_r_1()
            .border_color(c(theme::BORDER));
        if self.prs.len() > 1 {
            side = side.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .px(px(8.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(
                        div()
                            .px(px(6.))
                            .pb(px(6.))
                            .flex()
                            .gap(px(4.))
                            .text_size(px(10.5))
                            .text_color(c(theme::TEXT_FAINT))
                            .child("REPOSITORIES")
                            .child(div().text_color(c(theme::TEXT_DIM)).child(self.prs.len().to_string())),
                    )
                    .children(self.prs.iter().enumerate().map(|(i, p)| {
                        let on = i == self.active;
                        div()
                            .id(SharedString::from(format!("pr-repo:{i}")))
                            .relative()
                            .h(px(28.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(px(6.))
                            .border_l_2()
                            .text_size(px(12.5))
                            .cursor_pointer()
                            .when(on, |d| d.bg(c(theme::BG)).border_color(c(theme::ACCENT)).text_color(c(theme::TEXT)).font_weight(FontWeight::SEMIBOLD))
                            .when(!on, |d| d.border_color(gpui::transparent_black()).text_color(c(theme::TEXT_DIM)).hover(|s| s.bg(c(theme::BG))))
                            .child(lucide("repo", 13., c(if on { theme::ACCENT } else { theme::TEXT_FAINT })))
                            .child(div().min_w_0().truncate().child(p.repo.clone()))
                            .child(div().ml_auto().text_size(px(11.)).font_weight(FontWeight::NORMAL).text_color(c(theme::TEXT_FAINT)).child(format!("#{}", p.number)))
                            .child(probe(format!("pr-repo:{i}")))
                            .on_click(cx.listener(move |p, _, window, cx| {
                                if p.active != i {
                                    p.active = i;
                                    p.load(window, cx);
                                }
                            }))
                    })),
            );
        }
        let Some(detail) = self.detail.ready() else {
            // `.pr-side-loading`.
            let bar = |w: f32, h: f32| skeleton(gpui::relative(w), h);
            return side.child(div().flex().flex_col().gap(px(8.)).p(px(14.)).child(bar(0.8, 14.)).child(bar(0.6, 10.)).child(bar(0.7, 11.)));
        };
        let pr = self.pr().clone();
        let unresolved = self.review.iter().flat_map(|r| &r.threads).filter(|t| !t.is_resolved).count();
        let decision = self.review.as_ref().and_then(|r| r.review_decision.as_deref()).and_then(pr_ui::decision);
        let branch_pill = |t: &str| {
            div()
                .min_w_0()
                .truncate()
                .px(px(6.))
                .py(px(1.))
                .rounded(px(4.))
                .bg(c(theme::BG))
                .border_1()
                .border_color(c(theme::BORDER))
                .font_family(theme::MONO_FONT)
                .text_color(c(theme::TEXT_DIM))
                .child(t.to_string())
        };
        let url = pr.url.clone();
        let header = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(12.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("pr-link")
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c(theme::ACCENT))
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child(format!("{} #{} ↗", pr.repo, pr.number))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .when_some(decision, |d, (label, tone)| d.child(pr_ui::decision_pill(label, tone))),
            )
            .child(div().text_size(px(13.5)).font_weight(FontWeight(650.)).line_height(px(19.)).line_clamp(2).child(detail.title.clone()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .text_size(px(11.))
                    .child(branch_pill(&detail.branch))
                    .child(div().flex_none().text_color(c(theme::TEXT_FAINT)).child("→"))
                    .child(branch_pill(&detail.base).flex_none()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(11.))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(c(theme::TEXT_DIM)).child(detail.author.clone()))
                    .child(pr_ui::stats(detail.additions, detail.deletions, 11.))
                    .when(unresolved > 0, |d| d.child(div().text_color(c(theme::WARN)).child(format!("{unresolved} unresolved")))),
            );
        let reviews = self.review.as_ref().map_or(0, |r| r.reviews.len());
        let viewed_count = detail.files.iter().filter(|f| self.saved.viewed.contains(&f.path)).count();
        let total = detail.files.len();
        let on_overview = self.selected == Sel::Overview;
        let hide = self.hide_viewed;
        let mut files = div()
            .flex()
            .flex_col()
            .p(px(6.))
            .child(
                div()
                    .id("pr-overview")
                    .relative()
                    .h(px(30.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(on_overview, |d| d.bg(theme::accent_soft()).text_color(c(theme::TEXT)))
                    .when(!on_overview, |d| d.text_color(c(theme::TEXT_DIM)).hover(|s| s.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT))))
                    .child(lucide("file-text", 13., c(if on_overview { theme::TEXT } else { theme::TEXT_DIM })))
                    .child(div().flex_1().child("Overview"))
                    .when(reviews > 0, |d| d.child(count_pill(reviews, false)))
                    .child(probe("pr-overview"))
                    .on_click(cx.listener(|p, _, window, cx| p.select(Sel::Overview, window, cx))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(theme::TEXT_FAINT))
                    .child("FILES")
                    .child(div().mr_auto().font_weight(FontWeight::MEDIUM).child(format!("{viewed_count}/{total} viewed")))
                    .child(pr_ui::tip(
                        div()
                            .id("pr-hide-viewed")
                            .relative()
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .when(hide, |d| d.bg(theme::accent_soft()))
                            .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                            .child(lucide("eye", 13., c(if hide { theme::ACCENT } else { theme::TEXT_FAINT })))
                            .child(probe("pr-hide-viewed"))
                            .on_click(cx.listener(|p, _, _, cx| {
                                p.hide_viewed = !p.hide_viewed;
                                cx.notify();
                            })),
                        if hide { "Show viewed files" } else { "Hide viewed files" },
                    )),
            )
            .child(
                div()
                    .h(px(2.))
                    .mx(px(10.))
                    .mt(px(2.))
                    .mb(px(6.))
                    .rounded(px(2.))
                    .bg(c(theme::BORDER))
                    // `.rv-progress > div`: `transition: width 0.25s ease`.
                    .child(crate::motion::fill(
                        "pr-viewed-fill",
                        if total > 0 { viewed_count as f32 / total as f32 } else { 0. },
                        crate::motion::RV_FILL,
                        crate::motion::ease(),
                        div().h_full().bg(c(theme::OK)),
                    )),
            );
        if hide && total > 0 && viewed_count == total {
            files = files.child(
                div().flex().items_center().gap(px(6.)).px(px(12.)).py(px(8.)).text_size(px(12.)).text_color(c(theme::OK)).child(lucide("check", 12., c(theme::OK))).child("All files viewed"),
            );
        }
        for (i, f) in detail.files.iter().enumerate() {
            let viewed = self.saved.viewed.contains(&f.path);
            let selected = self.selected == Sel::File(f.path.clone());
            if hide && viewed && !selected {
                continue;
            }
            let (name, dir) = match f.path.rsplit_once('/') {
                Some((d, n)) => (n.to_string(), Some(d.to_string())),
                None => (f.path.clone(), None),
            };
            let open = self.threads_of(&f.path).iter().filter(|t| !t.is_resolved).count();
            let drafts = self.saved.drafts.iter().filter(|d| d.path == f.path).count();
            let (p1, p2) = (f.path.clone(), f.path.clone());
            files = files.child(
                div()
                    .id(SharedString::from(format!("pr-file:{i}")))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(8.))
                    .h(px(28.))
                    .rounded(px(6.))
                    .when(selected, |d| d.bg(theme::accent_soft()))
                    .when(!selected, |d| d.hover(|s| s.bg(c(theme::PANEL_RAISED))))
                    .child(
                        checkbox(format!("pr-viewed:{i}"), viewed).on_click(cx.listener(move |p, _, window, cx| {
                            cx.stop_propagation();
                            p.mark_viewed(&p1, !viewed, false, window, cx)
                        })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("pr-file-btn:{i}")))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .pl(px(2.))
                            .pr(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .text_size(px(12.5))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_baseline()
                                    .overflow_hidden()
                                    .child(div().flex_none().text_color(c(if viewed { theme::TEXT_FAINT } else { theme::TEXT })).child(name))
                                    .when_some(dir, |d, dir| d.child(div().ml(px(6.)).min_w_0().truncate().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(dir))),
                            )
                            .when(open > 0, |d| d.child(count_pill(open, false)))
                            .when(drafts > 0, |d| d.child(count_pill(drafts, true)))
                            .child(pr_ui::stats(f.additions, f.deletions, 10.5))
                            .on_click(cx.listener(move |p, _, window, cx| p.select(Sel::File(p2.clone()), window, cx))),
                    )
                    .child(probe(format!("pr-file:{i}"))),
            );
        }
        side.child(header).child(files)
    }

    // ---------- right side ----------

    fn render_filebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path = match &self.selected {
            Sel::File(p) => Some(p.clone()),
            Sel::Overview => None,
        };
        let pending = self.saved.drafts.len();
        let ready = self.detail.ready().is_some();
        let mode_btn = |name: &'static str, label: &'static str, on: bool| {
            div()
                .id(name)
                .relative()
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |d| d.bg(c(theme::PANEL_RAISED)).text_color(c(theme::TEXT)))
                .when(!on, |d| d.text_color(c(theme::TEXT_DIM)).hover(|s| s.text_color(c(theme::TEXT))))
                .child(label)
                .child(probe(name))
        };
        let nav_btn = |name: &'static str, icon: &'static str| {
            div()
                .id(name)
                .relative()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .hover(|s| s.bg(c(theme::PANEL_RAISED)))
                .child(lucide(icon, 13., c(theme::TEXT_DIM)))
                .child(probe(name))
        };
        let mut actions = div().flex().flex_none().items_center().gap(px(10.));
        if let Some(path) = path.clone() {
            let viewed = self.saved.viewed.contains(&path);
            let split = self.split;
            actions = actions
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .child(pr_ui::tip(
                            nav_btn("pr-prev-change", "chevron-left").on_click(cx.listener(|p, _, _, cx| {
                                if let Load::Ready(d) = &p.diff {
                                    d.update(cx, |d, cx| d.previous_change(cx));
                                }
                            })),
                            "Previous change (Shift+F7)",
                        ))
                        .child(pr_ui::tip(
                            nav_btn("pr-next-change", "chevron-right").on_click(cx.listener(|p, _, _, cx| {
                                if let Load::Ready(d) = &p.diff {
                                    d.update(cx, |d, cx| d.next_change(cx));
                                }
                            })),
                            "Next change (F7)",
                        )),
                )
                .child(
                    div()
                        .flex()
                        .bg(c(theme::BG))
                        .border_1()
                        .border_color(c(theme::BORDER))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .child(mode_btn("pr-unified", "Unified", !split).on_click(cx.listener(|p, _, _, cx| p.set_split(false, cx))))
                        .child(pr_ui::tip(
                            mode_btn("pr-split", "Split", split).on_click(cx.listener(|p, _, _, cx| p.set_split(true, cx))),
                            "Side by side; lets you comment on removed lines",
                        )),
                )
                .child(pr_ui::tip(
                    div()
                        .id("pr-viewed-toggle")
                        .relative()
                        .h(px(26.))
                        .pl(px(6.))
                        .pr(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .border_1()
                        .rounded(px(6.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .when(viewed, |d| d.text_color(c(theme::OK)).border_color(tint(theme::OK, 0.35)).bg(tint(theme::OK, 0.08)))
                        .when(!viewed, |d| d.text_color(c(theme::TEXT_DIM)).border_color(c(theme::BORDER)).hover(|s| s.text_color(c(theme::TEXT))))
                        .child(check_box(viewed))
                        .child("Viewed")
                        .child(probe("pr-viewed-toggle"))
                        .on_click(cx.listener(move |p, _, window, cx| p.mark_viewed(&path, !viewed, true, window, cx))),
                    "Mark as viewed and open the next file",
                ));
        }
        actions = actions.child(
            div()
                .relative()
                .child(
                    div()
                        .id("pr-review-btn")
                        .relative()
                        .h(px(26.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded(px(6.))
                        .bg(c(theme::ACCENT))
                        .text_color(c(theme::ON_ACCENT))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .when(!ready, |d| d.opacity(0.5))
                        .when(ready, |d| d.cursor_pointer().hover(|s| s.bg(c(theme::ACCENT_HOVER))))
                        .child("Review")
                        .when(pending > 0, |d| {
                            d.child(
                                div()
                                    .min_w(px(18.))
                                    .px(px(6.))
                                    .rounded_full()
                                    .bg(tint(theme::ON_ACCENT, 0.25))
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(pending.to_string()),
                            )
                        })
                        .child(probe("pr-review-btn"))
                        .on_click(cx.listener(move |p, _, window, cx| {
                            if ready {
                                p.toggle_submit(window, cx)
                            }
                        })),
                )
                .when(self.submit.is_some(), |d| d.child(self.render_submit(cx))),
        );
        let title = match &path {
            Some(p) => {
                let (dir, name) = match p.rsplit_once('/') {
                    Some((d, n)) => (format!("{d}/"), n.to_string()),
                    None => (String::new(), p.clone()),
                };
                div()
                    .flex()
                    .min_w_0()
                    .overflow_hidden()
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.))
                    .child(div().min_w_0().truncate().text_color(c(theme::TEXT_FAINT)).child(dir))
                    .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(name))
            }
            None => div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).font_family(theme::MONO_FONT).text_color(c(theme::TEXT)).child("Overview"),
        };
        div()
            .flex_none()
            .min_h(px(30.))
            .px(px(10.))
            .py(px(3.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .bg(c(theme::PANEL))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(title)
            .child(actions)
    }

    fn render_submit(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(s) = &self.submit else { return div().id("pr-submit") };
        let own = self.review.as_ref().is_some_and(|r| r.viewer == r.pr_author);
        let pending = self.saved.drafts.len();
        let busy = self.busy;
        let needs_text = s.event != "APPROVE" && s.body.read(cx).value().trim().is_empty() && pending == 0;
        let s_ = if pending == 1 { "" } else { "s" };
        div()
            .id("pr-submit")
            .absolute()
            .right_0()
            .top(px(34.))
            .w(px(420.))
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .border_1()
            .border_color(c(theme::BORDER_STRONG))
            .rounded(px(12.))
            .bg(c(theme::PANEL))
            .shadow(theme::shadow(16., 40., 0.5))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().text_size(px(13.5)).font_weight(FontWeight(650.)).child("Finish your review"))
            .child(div().relative().child(Textarea::new(&s.body)).child(probe("pr-submit-body")))
            .child(div().flex().flex_col().gap(px(6.)).children(EVENTS.iter().map(|(event, title, hint)| {
                let on = s.event == *event;
                let disabled = own && *event != "COMMENT";
                let event = *event;
                div()
                    .id(SharedString::from(format!("pr-event:{event}")))
                    .relative()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(if on { tint(theme::ACCENT, 0.5) } else { c(theme::BORDER) })
                    .when(on, |d| d.bg(theme::accent_soft()))
                    .when(disabled, |d| d.opacity(0.45))
                    .when(!disabled, |d| d.cursor_pointer())
                    .child(check_box(on))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(*title))
                            .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(*hint)),
                    )
                    .child(probe(format!("pr-event:{event}")))
                    .on_click(cx.listener(move |p, _, _, cx| {
                        if let (Some(s), false) = (&mut p.submit, disabled) {
                            s.event = event;
                            cx.notify();
                        }
                    }))
            })))
            .when(own, |d| d.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child("Only Comment is available on your own PR.")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .when(pending > 0, |d| {
                        d.child(
                            div()
                                .id("pr-discard")
                                .text_size(px(12.))
                                .text_color(c(theme::DANGER))
                                .cursor_pointer()
                                .child(format!("Discard {pending} pending"))
                                .on_click(cx.listener(|p, _, _, cx| {
                                    p.saved.drafts.clear();
                                    p.persist();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().ml_auto().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(format!("{pending} pending comment{s_}")))
                    .child({
                        let b = pr_ui::button("pr-submit-go", if busy { "Submitting…" } else { "Submit to GitHub" }, true, 30.);
                        if busy || needs_text {
                            b.opacity(0.5)
                        } else {
                            b.on_click(cx.listener(|p, _, window, cx| p.submit_review(window, cx)))
                        }
                    }),
            )
    }

    fn render_overview(&self) -> AnyElement {
        let Some(detail) = self.detail.ready() else {
            return table_loading("pr-overview-spin", "Loading…");
        };
        let head = div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .gap(px(4.))
            .mb(px(12.))
            .pb(px(10.))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .text_size(px(13.))
            .text_color(c(theme::TEXT_DIM))
            .child(div().font_weight(FontWeight::BOLD).text_color(c(theme::TEXT)).child(detail.author.clone()))
            .child("wants to merge")
            .child(div().font_family(theme::MONO_FONT).text_size(px(12.)).child(detail.branch.clone()))
            .child("into")
            .child(div().font_family(theme::MONO_FONT).text_size(px(12.)).child(detail.base.clone()));
        let body = if detail.body.trim().is_empty() {
            div().text_size(px(13.)).text_color(c(theme::TEXT_FAINT)).child("No description provided.").into_any_element()
        } else {
            div().text_size(px(13.)).child(TextView::markdown("pr-body", detail.body.clone()).selectable(true)).into_any_element()
        };
        let h4 = |t: &str| {
            div().mb(px(10.)).text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT_FAINT)).child(t.to_uppercase())
        };
        let now = pr_ui::now();
        let reviews: AnyElement = match &self.review {
            None => skeleton(gpui::relative(0.5), 12.).into_any_element(),
            Some(r) if r.reviews.is_empty() => div()
                .text_size(px(13.))
                .text_color(c(theme::TEXT_FAINT))
                .child("No reviews yet. Comment on lines in the files, then press Review to submit.")
                .into_any_element(),
            Some(r) => div()
                .flex()
                .flex_col()
                .children(r.reviews.iter().enumerate().map(|(i, rv)| {
                    let (label, tone) = match rv.state.as_str() {
                        "APPROVED" => ("approved".to_string(), Tone::Ok),
                        "CHANGES_REQUESTED" => ("requested changes".into(), Tone::Bad),
                        "COMMENTED" => ("reviewed".into(), Tone::Muted),
                        "DISMISSED" => ("review dismissed".into(), Tone::Muted),
                        "PENDING" => ("has a pending review".into(), Tone::Muted),
                        s => (s.to_lowercase(), Tone::Muted),
                    };
                    div()
                        .flex()
                        .gap(px(10.))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(c(theme::BORDER))
                        .text_size(px(12.5))
                        .child(avatar(&rv.author))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(4.))
                                        .child(div().font_weight(FontWeight::BOLD).child(rv.author.clone()))
                                        .child(div().text_color(tone.color()).child(label))
                                        .when_some(rv.submitted_at.as_ref(), |d, at| {
                                            d.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(format!(" · {}", pr_ui::rel_time(at, now))))
                                        }),
                                )
                                .when(!rv.body.trim().is_empty(), |d| {
                                    d.child(TextView::markdown(SharedString::from(format!("pr-review-body:{i}")), rv.body.clone()))
                                }),
                        )
                }))
                .into_any_element(),
        };
        let threads: Vec<ReviewThread> = self.review.iter().flat_map(|r| r.threads.clone()).collect();
        div()
            .id("pr-overview-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(920.))
                    .px(px(24.))
                    .py(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(22.))
                    .child(div().border_1().border_color(c(theme::BORDER)).rounded(px(10.)).bg(c(theme::PANEL)).px(px(18.)).py(px(16.)).child(head).child(body))
                    .child(div().child(h4("Reviews")).child(reviews))
                    .when(!threads.is_empty(), |d| {
                        d.child(div().child(h4("Conversations")).children(threads.iter().map(|t| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .py(px(6.))
                                .text_size(px(12.5))
                                .child(if t.is_resolved {
                                    lucide("check", 12., c(theme::OK)).into_any_element()
                                } else {
                                    div().size(px(7.)).rounded_full().bg(c(theme::WARN)).into_any_element()
                                })
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(theme::MONO_FONT)
                                        .text_size(px(11.5))
                                        .text_color(c(theme::TEXT_DIM))
                                        .child(format!("{}{}", t.path, t.line.map(|l| format!(":{l}")).unwrap_or_default())),
                                )
                                .child(div().flex_1().min_w_0().truncate().child(t.comments.first().map(|c| c.body.lines().next().unwrap_or("").to_string()).unwrap_or_default()))
                                .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(t.comments.len().to_string()))
                        })))
                    }),
            )
            .into_any_element()
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let Sel::File(path) = &self.selected else { return div().into_any_element() };
        let state = match &self.diff {
            Load::Ready(s) => s.clone(),
            Load::Failed(e) => return div().p(px(20.)).text_size(px(12.5)).text_color(c(theme::DANGER)).child(e.clone()).into_any_element(),
            _ => return table_loading("pr-diff-spin", "Loading diff…"),
        };
        let diff_path: SharedString = state.read(cx).files().first().map(|f| f.path().clone()).unwrap_or_else(|| path.clone().into());
        let threads = self.threads_of(path);
        let placed: Vec<&ReviewThread> = threads.iter().copied().filter(|t| t.line.is_some() && (t.side == "RIGHT" || self.split)).collect();
        let off: Vec<ReviewThread> = threads.iter().filter(|t| !placed.iter().any(|p| p.id == t.id)).map(|t| (*t).clone()).collect();
        let mut annotations = vec![];
        let mut ranges = vec![];
        let mut lines: Vec<(DiffSide, u64)> = vec![];
        for t in &placed {
            lines.push((side_of(&t.side), t.line.unwrap_or(1)));
            if let (Some(s), Some(e)) = (t.start_line, t.line) {
                if s < e {
                    ranges.push((side_of(&t.side), s, e));
                }
            }
        }
        for d in self.saved.drafts.iter().filter(|d| &d.path == path && (d.side == "RIGHT" || self.split)) {
            lines.push((side_of(&d.side), d.line));
            if let Some(s) = d.start_line {
                ranges.push((side_of(&d.side), s, d.line));
            }
        }
        if let Some(ComposerKind::New { side, start, end }) = self.composer.as_ref().map(|c| &c.kind) {
            lines.push((*side, *end));
            ranges.push((*side, *start, *end));
        }
        lines.sort_by_key(|(s, l)| (*s == DiffSide::Modified, *l));
        lines.dedup();
        for (side, line) in &lines {
            annotations.push(DiffAnnotation::line(
                ElementId::Name(format!("{}:{line}", side_name(*side)).into()),
                DiffLinePosition::new(diff_path.clone(), *side, *line as usize),
            ));
        }
        let decorations: Vec<DiffDecoration> = ranges
            .into_iter()
            .map(|(side, s, e)| DiffDecoration::new(DiffLineRange::new(diff_path.clone(), side, s as usize, e as usize)).with_background(theme::accent_soft()))
            .collect();
        let (w1, w2) = (cx.entity().downgrade(), cx.entity().downgrade());
        let diff = Diff::new(&state)
            .size_full()
            .header_visible(false)
            .annotations(annotations)
            .decorations(decorations)
            .render_annotation(move |a, window, cx| {
                let Some(page) = w1.upgrade() else { return div().into_any_element() };
                let ElementId::Name(id) = a.id() else { return div().into_any_element() };
                let Some((side, line)) = id.split_once(':').and_then(|(s, l)| Some((s.to_string(), l.parse::<u64>().ok()?))) else {
                    return div().into_any_element();
                };
                page.update(cx, |p, cx| p.render_zone(&side, line, window, cx))
            })
;
        // Old lines take notes in split view only, as in the Tauri app.
        let sides: &[DiffSide] = if self.split { &[DiffSide::Original, DiffSide::Modified] } else { &[DiffSide::Modified] };
        let diff = crate::diff_select::attach(
            diff,
            &self.sel,
            &state,
            sides,
            Some(std::rc::Rc::new(move |range, window, cx| {
                let _ = w2.update(cx, |p, cx| p.comment_on(range, window, cx));
            })),
        );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when(!off.is_empty(), |d| {
                let outdated = off.iter().any(|t| t.line.is_none());
                d.child(
                    div()
                        .id("pr-offdiff")
                        .max_h(gpui::relative(0.38))
                        .overflow_y_scroll()
                        .px(px(14.))
                        .py(px(10.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .bg(c(theme::BG))
                        .border_b_1()
                        .border_color(c(theme::BORDER))
                        .child(
                            div()
                                .flex()
                                .gap(px(12.))
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c(theme::TEXT_FAINT))
                                .child(if outdated { "OUTDATED COMMENTS" } else { "COMMENTS ON REMOVED LINES" }),
                        )
                        .children(off.iter().map(|t| self.render_thread(t, cx))),
                )
            })
            .child(div().flex_1().min_h_0().child(diff))
            .into_any_element()
    }

    /// The threads, drafts and composer under one line.
    fn render_zone(&mut self, side: &str, line: u64, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Sel::File(path) = self.selected.clone() else { return div().into_any_element() };
        let threads: Vec<ReviewThread> =
            self.threads_of(&path).into_iter().filter(|t| t.side == side && t.line == Some(line)).cloned().collect();
        let drafts: Vec<Draft> = self.saved.drafts.iter().filter(|d| d.path == path && d.side == side && d.line == line).cloned().collect();
        let composer_here = matches!(self.composer.as_ref().map(|c| &c.kind), Some(ComposerKind::New { side: s, end, .. }) if side_name(*s) == side && *end == line);
        div()
            .mx(px(12.))
            .my(px(6.))
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .font_family(theme::UI_FONT)
            .children(threads.iter().map(|t| self.render_thread(t, cx)))
            .children(drafts.iter().map(|d| self.render_draft(d, cx)))
            .when(composer_here, |d| d.child(self.render_composer(cx)))
            .into_any_element()
    }

    fn render_thread(&self, t: &ReviewThread, cx: &mut Context<Self>) -> AnyElement {
        let now = pr_ui::now();
        let first = t.comments.first().map(|c| c.body.lines().next().unwrap_or("").to_string()).unwrap_or_default();
        let replying = matches!(self.composer.as_ref().map(|c| &c.kind), Some(ComposerKind::Reply(id)) if *id == t.id);
        let (t1, t2) = (t.clone(), t.id.clone());
        thread_card(false)
            .when(t.is_resolved, |d| d.opacity(0.85))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(7.))
                    .bg(c(theme::PANEL_RAISED))
                    .text_size(px(12.))
                    .text_color(c(theme::TEXT_DIM))
                    .child(div().flex_1().min_w_0().truncate().child(first))
                    .when(t.comments.len() > 1, |d| d.child(div().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child(t.comments.len().to_string())))
                    .when(t.is_outdated, |d| d.child(pr_ui::decision_pill("Outdated", Tone::Muted)))
                    .when(t.is_resolved, |d| d.child(pr_ui::decision_pill("Resolved", Tone::Ok))),
            )
            .children(t.comments.iter().enumerate().map(|(i, cm)| {
                div()
                    .flex()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .when(i > 0, |d| d.border_t_1().border_color(c(theme::BORDER)))
                    .child(avatar(&cm.author))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.))
                                    .child(div().font_weight(FontWeight::BOLD).child(cm.author.clone()))
                                    .child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(pr_ui::rel_time(&cm.created_at, now)))
                                    .when(cm.state == "PENDING", |d| d.child(pr_ui::decision_pill("Pending", Tone::Warn))),
                            )
                            .child(div().text_size(px(12.5)).child(TextView::markdown(SharedString::from(format!("pr-comment:{}", cm.id)), cm.body.clone()))),
                    )
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(c(theme::BORDER))
                    .bg(c(theme::BG))
                    .when(replying, |d| d.child(self.render_composer(cx)))
                    .when(!replying, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("pr-reply:{}", t.id)))
                                .flex_1()
                                .h(px(26.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(c(theme::BORDER))
                                .bg(c(theme::PANEL))
                                .text_size(px(12.))
                                .text_color(c(theme::TEXT_FAINT))
                                .cursor_text()
                                .child("Reply…")
                                .on_click(cx.listener(move |p, _, window, cx| p.open_composer(ComposerKind::Reply(t2.clone()), window, cx))),
                        )
                        .child(
                            pr_ui::btn_mini(format!("pr-resolve:{}", t.id), if t.is_resolved { "Unresolve" } else { "Resolve" })
                                .on_click(cx.listener(move |p, _, window, cx| p.resolve(&t1, window, cx))),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_draft(&self, d: &Draft, cx: &mut Context<Self>) -> AnyElement {
        let range = d.start_line.map(|s| format!("Lines {s}–{}", d.line));
        let viewer = self.review.as_ref().map(|r| r.viewer.clone()).unwrap_or_default();
        let id = d.id.clone();
        thread_card(true)
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .child(avatar(&viewer))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(12.))
                                    .child(div().font_weight(FontWeight::BOLD).child(viewer))
                                    .child(pr_ui::decision_pill("Pending", Tone::Warn))
                                    .when_some(range, |el, r| el.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(r)))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("pr-draft-del:{}", d.id)))
                                            .ml_auto()
                                            .cursor_pointer()
                                            .child(lucide("trash", 12., c(theme::TEXT_FAINT)))
                                            .on_click(cx.listener(move |p, _, _, cx| {
                                                p.saved.drafts.retain(|x| x.id != id);
                                                p.persist();
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(div().text_size(px(12.5)).child(TextView::markdown(SharedString::from(format!("pr-draft:{}", d.id)), d.body.clone()))),
                    ),
            )
            .into_any_element()
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(composer) = &self.composer else { return div().into_any_element() };
        let reply = matches!(composer.kind, ComposerKind::Reply(_));
        let label = match composer.kind {
            ComposerKind::New { start, end, .. } if start < end => format!("Comment on lines {start}–{end}"),
            ComposerKind::New { end, .. } => format!("Comment on line {end}"),
            ComposerKind::Reply(_) => String::new(),
        };
        let in_review = !self.saved.drafts.is_empty();
        let busy = self.busy;
        let inner = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(6.))
            .when(!label.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(c(theme::TEXT_FAINT)).child(label)))
            .child(div().relative().child(Textarea::new(&composer.input)).child(probe("pr-composer")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().mr_auto().text_size(px(11.)).text_color(c(theme::TEXT_FAINT)).child("Markdown"))
                    .child(pr_ui::btn_mini("pr-composer-cancel", "Cancel").on_click(cx.listener(|p, _, _, cx| {
                        p.composer = None;
                        cx.notify();
                    })))
                    .when(!reply, |d| {
                        d.child(
                            pr_ui::btn_mini("pr-composer-now", if in_review { "Add single comment" } else { "Comment now" })
                                .when(busy, |b| b.opacity(0.5))
                                .on_click(cx.listener(|p, _, window, cx| p.comment_now(window, cx))),
                        )
                        .child(
                            pr_ui::button("pr-composer-add", if in_review { "Add to review" } else { "Start a review" }, true, 24.)
                                .text_size(px(11.5))
                                .on_click(cx.listener(|p, _, _, cx| p.add_draft(cx))),
                        )
                    })
                    .when(reply, |d| {
                        d.child(
                            pr_ui::button("pr-composer-reply", "Reply", true, 24.)
                                .text_size(px(11.5))
                                .on_click(cx.listener(|p, _, window, cx| p.comment_now(window, cx))),
                        )
                    }),
            );
        if reply {
            return inner.into_any_element();
        }
        thread_card(false).border_color(tint(theme::ACCENT, 0.5)).px(px(12.)).py(px(10.)).child(inner).into_any_element()
    }
}

/// `.rv-thread` (`.rv-thread-draft` when pending).
fn thread_card(draft: bool) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(if draft { tint(theme::WARN, 0.4) } else { c(theme::BORDER_STRONG) })
        .rounded(px(8.))
        .bg(c(theme::PANEL))
        .overflow_hidden()
        .text_size(px(12.5))
        .text_color(c(theme::TEXT))
        .shadow(theme::shadow(4., 16., 0.25))
}

/// `.rv-avatar-fallback`: the initial in a circle.
fn avatar(name: &str) -> gpui::Div {
    div()
        .size(px(22.))
        .flex_none()
        .rounded_full()
        .bg(theme::accent_soft())
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(c(theme::INFO))
        .child(name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default())
}

/// `.rv-file-count`.
fn count_pill(n: usize, draft: bool) -> gpui::Div {
    div()
        .min_w(px(16.))
        .px(px(5.))
        .rounded_full()
        .bg(if draft { tint(theme::WARN, 0.18) } else { theme::accent_soft() })
        .text_color(c(if draft { theme::WARN } else { theme::INFO }))
        .text_size(px(10.5))
        .font_weight(FontWeight::BOLD)
        .flex()
        .justify_center()
        .child(n.to_string())
}

/// `CheckBox`: a 14px box with a check when on.
fn check_box(on: bool) -> gpui::Div {
    div()
        .size(px(14.))
        .flex_none()
        .rounded(px(3.))
        .border_1()
        .border_color(c(if on { theme::OK } else { theme::BORDER_STRONG }))
        .when(on, |d| d.bg(c(theme::OK)))
        .flex()
        .items_center()
        .justify_center()
        .when(on, |d| d.child(lucide("check", 10., c(theme::ON_ACCENT))))
}

fn checkbox(name: String, on: bool) -> gpui::Stateful<gpui::Div> {
    div().id(SharedString::from(name.clone())).relative().flex_none().cursor_pointer().child(check_box(on)).child(probe(name))
}

impl Render for PrReviewPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main = match self.selected {
            Sel::Overview => self.render_overview(),
            Sel::File(_) => self.render_diff(cx),
        };
        div()
            .size_full()
            .flex()
            .font_family(theme::UI_FONT)
            .text_color(c(theme::TEXT))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|p, _, _, cx| {
                    if p.submit.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(self.render_side(cx))
            .child(div().flex_1().min_w_0().h_full().flex().flex_col().child(self.render_filebar(cx)).child(main))
    }
}

impl PageView for PrReviewPage {
    fn title(&self) -> SharedString {
        format!("#{}", self.pr().number).into()
    }

    fn dump(&self, out: &mut String, _: &App) {
        let pr = self.pr();
        let _ = writeln!(out, "  pr review {} #{}: detail {} selected {:?}", pr.repo, pr.number, crate::pages::ui::load_label(&self.detail), self.selected);
        if let Some(d) = self.detail.ready() {
            let _ = writeln!(out, "    {} files, {} viewed, {} drafts", d.files.len(), self.saved.viewed.len(), self.saved.drafts.len());
        }
        if let Some(r) = &self.review {
            let _ = writeln!(out, "    review: viewer {} {} threads {} reviews", r.viewer, r.threads.len(), r.reviews.len());
        }
        let _ = writeln!(out, "    diff: {}", crate::pages::ui::load_label(&self.diff));
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_review(false, window, cx);
    }
}

/// Opens (or shows) the review tab of a group of PRs.
pub fn open(app: &mut OrbitApp, _host: Host, prs: Vec<PullRequest>, window: &mut Window, cx: &mut Context<OrbitApp>) {
    let Some(first) = prs.first() else { return };
    let key = format!("page:pr-review:{}#{}", first.owner_repo, first.number);
    crate::pages::open(app, key, move |h, w, cx| PrReviewPage::new(h, prs, w, cx), window, cx);
}

/// `.table-loading`: a spinner and a faint label, centered.
pub(crate) fn table_loading(id: &'static str, label: &'static str) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(28.))
        .text_color(c(theme::TEXT_FAINT))
        .child(crate::workspace_ui::kit::spinner(id, 22., c(theme::ACCENT)))
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_two_texts_under_the_file_path() {
        let p = patch_of("src/a.rs", "one\ntwo\n", "one\nTWO\nthree\n").unwrap();
        assert!(p.contains("--- a/src/a.rs"), "{p}");
        assert!(p.contains("+++ b/src/a.rs"), "{p}");
        let files = DiffFile::parse(&p).unwrap();
        assert_eq!(files[0].path().as_ref(), "src/a.rs");
        assert_eq!((files[0].additions(), files[0].deletions()), (2, 1));
    }
}

