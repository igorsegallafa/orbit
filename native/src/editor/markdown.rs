//! Markdown preview for `.md` files (`MarkdownPreview.tsx`): the file bar's
//! Editor / Preview toggle swaps the editor for a rendered view that follows
//! the buffer. Links to the web open in the browser, relative ones open the
//! file in Orbit; images load from the file's folder or the web.
use super::{EditorEvent, EditorTab};
use crate::commands::{self, Command};
use crate::theme::{self, c};
use base64::Engine as _;
use gpui::{
    div, prelude::*, px, relative, App, Context, Entity, FocusHandle, FontWeight, Image, ImageFormat, ImageSource,
    ScrollHandle, SharedUri, StyleRefinement, Task, Window,
};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::text::{TextView, TextViewState, TextViewStyle};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Typing pauses this long before the preview re-renders.
const DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_IMAGE_BYTES: &str = "20M";

pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

pub fn register(cx: &mut App) {
    commands::register(
        cx,
        Command::new("markdown.openPreview", "Open Markdown Preview")
            .when(|app, cx| app.active_editor().is_some_and(|e| e.read(cx).markdown.is_some()))
            .run(|app, window, cx| {
                if let Some(e) = app.active_editor() {
                    e.update(cx, |e, cx| e.set_markdown_mode(Mode::Preview, window, cx));
                }
            }),
    );
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Editor,
    Preview,
}

pub struct Preview {
    pub mode: Mode,
    pub state: Entity<TextViewState>,
    scroll: ScrollHandle,
    images: Arc<Images>,
    _debounce: Task<()>,
    _images: Task<()>,
}

impl Preview {
    pub fn new(text: &str, cx: &mut Context<EditorTab>) -> Self {
        let state = cx.new(|cx| TextViewState::markdown(text, cx).selectable(true));
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        // A downloaded image repaints the preview.
        let images_task = cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while rx.next().await.is_some() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            mode: Mode::Editor,
            state,
            scroll: ScrollHandle::new(),
            images: Arc::new(Images { cache: Arc::default(), loaded: tx }),
            _debounce: Task::ready(()),
            _images: images_task,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle().clone()
    }

    fn rendered(&self, cx: &App) -> String {
        self.state.read(cx).rendered_text().as_str().to_string()
    }
}

impl EditorTab {
    pub fn markdown_mode(&self) -> Option<Mode> {
        self.markdown.as_ref().map(|m| m.mode)
    }

    /// The mode and the rendered text, for the self-test dump.
    pub fn markdown_text(&self, cx: &App) -> Option<String> {
        let md = self.markdown.as_ref()?;
        let text = md.rendered(cx);
        let mut out = format!("markdown: {:?}, scroll {:.0}px\n", md.mode, f32::from(-md.scroll.offset().y).max(0.));
        for line in text.lines().filter(|l| !l.trim().is_empty()).take(80) {
            out.push_str(&format!("  md| {line}\n"));
        }
        Some(out)
    }

    pub fn set_markdown_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(md) = &mut self.markdown else { return };
        md.mode = mode;
        match mode {
            Mode::Preview => {
                // Catch up at once rather than after the debounce.
                md._debounce = Task::ready(());
                let text = self.state.read(cx).text().to_string();
                md.state.update(cx, |s, cx| s.set_text(&text, cx));
                window.focus(&md.focus_handle(cx), cx);
            }
            Mode::Editor => self.focus_editor(window, cx),
        }
        cx.notify();
    }

    /// The buffer changed: re-render once typing pauses (entering the
    /// preview catches up, so the editor side skips it).
    pub(super) fn schedule_markdown(&mut self, cx: &mut Context<Self>) {
        let Some(md) = self.markdown.as_mut().filter(|m| m.mode == Mode::Preview) else { return };
        md._debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                let text = this.state.read(cx).text().to_string();
                if let Some(md) = &this.markdown {
                    md.state.update(cx, |s, cx| s.set_text(&text, cx));
                }
            });
        });
    }

    fn repo_root(&self) -> Option<PathBuf> {
        self.repo.as_ref().map(|(_, root)| root.clone())
    }

    pub fn open_markdown_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.repo_root();
        match resolve_link(url, &self.path, root.as_deref()) {
            Link::External(url) => {
                // A self-test records it instead of starting a browser.
                if !crate::probe::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
                    cx.open_url(&url);
                }
                self.message = Some(format!("Opened {url}").into());
            }
            Link::Anchor(anchor) => {
                if !self.reveal_heading(&anchor, cx) {
                    self.message = Some(format!("No heading #{anchor}").into());
                }
            }
            Link::Local { path, line } => {
                if path.is_dir() {
                    crate::file_ops::reveal_in_tree(path, window, cx);
                } else if path.is_file() {
                    let range = line.map(|l| {
                        let at = lsp_types::Position::new(l.saturating_sub(1), 0);
                        lsp_types::Range::new(at, at)
                    });
                    if path == self.path {
                        if let Some(range) = range {
                            self.reveal(range, window, cx);
                        }
                    } else {
                        cx.emit(EditorEvent::OpenLocation { path, range });
                    }
                } else {
                    self.message = Some(format!("Not found: {}", self.rel_path(&path)).into());
                }
            }
        }
        cx.notify();
    }

    /// Scrolls the preview to the heading whose GitHub anchor is `anchor`.
    fn reveal_heading(&mut self, anchor: &str, cx: &mut Context<Self>) -> bool {
        let Some(md) = &self.markdown else { return false };
        let source = self.state.read(cx).text().to_string();
        let Some(range) = heading_range(&source, anchor) else { return false };
        md.state.update(cx, |s, cx| {
            let text = s.rendered_text();
            text.range_for_source(range).is_some_and(|r| s.reveal_range(r.start..r.start, cx).is_ok())
        })
    }

    pub(super) fn render_markdown_toggle(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let mode = self.markdown_mode()?;
        let button = |m: Mode, id: &'static str, icon: &'static str, tip: &'static str| {
            let on = mode == m;
            div()
                .id(id)
                .relative()
                .h(px(22.))
                .w(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(on, |d| d.bg(theme::accent_soft()))
                .child(crate::icons::orbit_icon(icon, 13., c(if on { theme::NAV_ACTIVE } else { theme::TEXT_FAINT })))
                .child(crate::probe::probe(id))
                .tooltip(move |w, cx| gpui_component::tooltip::Tooltip::new(tip).build(w, cx))
                .on_click(cx.listener(move |this, _, window, cx| this.set_markdown_mode(m, window, cx)))
        };
        // `.mode-toggle`.
        Some(
            div()
                .flex()
                .flex_none()
                .bg(c(theme::BG))
                .border_1()
                .border_color(c(theme::BORDER))
                .rounded(px(6.))
                .overflow_hidden()
                .child(button(Mode::Editor, "md-mode:editor", "code-view", "Editor"))
                .child(button(Mode::Preview, "md-mode:preview", "eye", "Preview")),
        )
    }

    pub(super) fn render_markdown(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let md = self.markdown.as_ref().filter(|m| m.mode == Mode::Preview)?;
        let this = cx.weak_entity();
        let file = self.path.clone();
        let root = self.repo_root();
        let images = md.images.clone();
        let scroll = md.scroll.clone();
        let view = TextView::new(&md.state)
            .style(preview_style())
            .selectable(true)
            .on_link_click(move |url, _, window, cx| {
                let _ = this.update(cx, |e, cx| e.open_markdown_link(url, window, cx));
            })
            .image_source(move |uri| images.source(uri, &file, root.as_deref()))
            .on_reveal(move |bounds, _, _| {
                // Put the line at the top of the viewport.
                let top = scroll.bounds().top();
                let offset = scroll.offset();
                scroll.set_offset(gpui::point(offset.x, offset.y - (bounds.top() - top) + px(12.)));
            })
            .inline_code_style(StyleRefinement::default().bg(c(theme::PANEL_RAISED)))
            .blockquote_style({
                let mut s = StyleRefinement::default().bg(c(theme::PANEL)).text_color(c(theme::TEXT_DIM)).border_color(c(theme::ACCENT));
                s.border_widths.left = Some(px(3.).into());
                s
            });
        // `.md-preview` / `.md-body`.
        Some(
            div().relative().flex_1().min_h_0().bg(c(theme::BG)).child(crate::probe::probe("md-preview")).child(
                div()
                    .id("md-preview")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&md.scroll)
                    .vertical_scrollbar(&md.scroll)
                    .child(
                        div().w_full().flex().justify_center().child(
                            div()
                                .w_full()
                                .max_w(px(860.))
                                .px(px(40.))
                                .pt(px(28.))
                                .pb(px(60.))
                                .text_size(px(14.))
                                .line_height(relative(1.65))
                                .text_color(c(theme::TEXT))
                                .child(view),
                        ),
                    ),
            ),
        )
    }
}

/// The preview's look, from the theme tokens (`.md-body` in `App.css`).
pub(crate) fn preview_style() -> TextViewStyle {
    let mut code = StyleRefinement::default().bg(c(theme::PANEL)).border_1().border_color(c(theme::BORDER)).px(px(16.)).py(px(14.));
    code.corner_radii.top_left = Some(px(8.).into());
    code.corner_radii.top_right = Some(px(8.).into());
    code.corner_radii.bottom_left = Some(px(8.).into());
    code.corner_radii.bottom_right = Some(px(8.).into());
    let mut table = StyleRefinement::default().border_color(c(theme::BORDER));
    table.overflow.x = Some(gpui::Overflow::Scroll);
    TextViewStyle::default()
        .paragraph_gap(gpui::rems(0.8))
        .heading_font_size(|level, _| px(match level {
            1 => 26.,
            2 => 21.,
            3 => 17.,
            4 => 15.,
            _ => 14.,
        }))
        .code_block(code)
        .table(table)
        .table_head(StyleRefinement::default().bg(c(theme::PANEL)).text_color(c(theme::TEXT)).font_weight(FontWeight::SEMIBOLD))
        .table_cell(StyleRefinement::default().px(px(12.)).py(px(6.)))
}

// ---------- links and images ----------

#[derive(Debug, PartialEq)]
pub enum Link {
    /// A web (or mail) address, for the browser.
    External(String),
    /// `#heading` in this document.
    Anchor(String),
    /// A file or folder, with GitHub's `#L12` line.
    Local { path: PathBuf, line: Option<u32> },
}

/// What a link in `file` points to. `/x` is from the repo root, as on GitHub.
pub fn resolve_link(url: &str, file: &Path, root: Option<&Path>) -> Link {
    let url = url.trim();
    if let Some(anchor) = url.strip_prefix('#') {
        return Link::Anchor(percent_decode(anchor));
    }
    if let Some(scheme) = scheme(url) {
        if scheme.eq_ignore_ascii_case("file") {
            if let Some(path) = url::Url::parse(url).ok().and_then(|u| u.to_file_path().ok()) {
                return Link::Local { path, line: None };
            }
        }
        return Link::External(url.to_string());
    }
    let (rest, fragment) = url.split_once('#').map_or((url, None), |(p, f)| (p, Some(f)));
    let rest = rest.split_once('?').map_or(rest, |(p, _)| p);
    let line = fragment.and_then(|f| f.strip_prefix('L')).and_then(|f| f.split(['-', 'C']).next()).and_then(|n| n.parse().ok());
    Link::Local { path: local_path(rest, file, root), line }
}

#[derive(Debug, PartialEq)]
pub enum ImageRef {
    Remote(String),
    Data(String),
    Local(PathBuf),
}

/// Where an image in `file` loads from: relative paths from the file's folder.
pub fn resolve_image(uri: &str, file: &Path, root: Option<&Path>) -> ImageRef {
    let uri = uri.trim();
    match scheme(uri) {
        Some(s) if s.eq_ignore_ascii_case("data") => ImageRef::Data(uri.to_string()),
        Some(s) if s.eq_ignore_ascii_case("file") => match url::Url::parse(uri).ok().and_then(|u| u.to_file_path().ok()) {
            Some(path) => ImageRef::Local(path),
            None => ImageRef::Remote(uri.to_string()),
        },
        Some(_) => ImageRef::Remote(uri.to_string()),
        None => {
            let rest = uri.split(['?', '#']).next().unwrap_or_default();
            ImageRef::Local(local_path(rest, file, root))
        }
    }
}

/// A URL scheme (`https`, `mailto`), not a Windows drive letter.
fn scheme(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    let valid = scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then_some(scheme)
}

fn local_path(rel: &str, file: &Path, root: Option<&Path>) -> PathBuf {
    let rel = percent_decode(rel);
    let dir = file.parent().unwrap_or(Path::new("/"));
    let joined = match rel.strip_prefix('/') {
        Some(from_root) => root.unwrap_or(Path::new("/")).join(from_root),
        None if rel.is_empty() => file.to_path_buf(),
        None => dir.join(&rel),
    };
    normalize(&joined)
}

/// Resolves `.` and `..` without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// GitHub's heading anchor: lowercase, punctuation dropped, spaces as `-`.
pub fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// The source range of the ATX heading whose slug is `anchor`.
fn heading_range(source: &str, anchor: &str) -> Option<std::ops::Range<usize>> {
    let anchor = anchor.to_lowercase();
    let mut at = 0;
    let mut fenced = false;
    for line in source.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if !(1..=6).contains(&hashes) {
            continue;
        }
        let text = trimmed[hashes..].trim().trim_end_matches('#').trim();
        if slug(&text.replace(['`', '*'], "")) == anchor {
            return Some(start..start + line.trim_end().len());
        }
    }
    None
}

/// Images the preview has loaded: data URLs decoded, web ones downloaded.
struct Images {
    /// `None` while downloading or after it failed.
    cache: Arc<Mutex<HashMap<String, Option<Arc<Image>>>>>,
    loaded: futures::channel::mpsc::UnboundedSender<()>,
}

impl Images {
    fn source(&self, uri: &SharedUri, file: &Path, root: Option<&Path>) -> ImageSource {
        match resolve_image(uri.as_ref(), file, root) {
            ImageRef::Local(path) => path.into(),
            ImageRef::Data(url) => {
                let mut cache = self.cache.lock().unwrap();
                let image = cache.entry(url).or_insert_with_key(|url| decode_data_url(url)).clone();
                image.map_or_else(pending, ImageSource::Image)
            }
            ImageRef::Remote(url) => {
                let mut cache = self.cache.lock().unwrap();
                if let Some(image) = cache.get(&url) {
                    return image.clone().map_or_else(pending, ImageSource::Image);
                }
                cache.insert(url.clone(), None);
                self.download(url);
                pending()
            }
        }
    }

    /// Fetched with curl (as `orbit-core` does); the preview repaints when it lands.
    fn download(&self, url: String) {
        let tx = self.loaded.clone();
        let images = Arc::downgrade(&self.cache);
        std::thread::spawn(move || {
            let out = orbit_core::proc::cmd("curl")
                .args(["-sSfL", "--max-time", "20", "--max-filesize", MAX_IMAGE_BYTES, &url])
                .stdin(std::process::Stdio::null())
                .output();
            let image = out.ok().filter(|o| o.status.success()).and_then(|o| {
                let format = sniff(&o.stdout)?;
                Some(Arc::new(Image::from_bytes(format, o.stdout)))
            });
            if let Some(images) = images.upgrade() {
                images.lock().unwrap().insert(url, image);
                let _ = tx.unbounded_send(());
            }
        });
    }
}

fn pending() -> ImageSource {
    ImageSource::Custom(Arc::new(|_, _| None))
}

fn decode_data_url(url: &str) -> Option<Arc<Image>> {
    let (meta, data) = url.strip_prefix("data:")?.split_once(',')?;
    let bytes = if meta.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?
    } else {
        percent_decode(data).into_bytes()
    };
    Some(Arc::new(Image::from_bytes(sniff(&bytes)?, bytes)))
}

/// The image format from the bytes (servers often send the wrong type).
fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    let head = &bytes[..bytes.len().min(512)];
    if head.starts_with(b"\x89PNG") {
        Some(ImageFormat::Png)
    } else if head.starts_with(&[0xff, 0xd8]) {
        Some(ImageFormat::Jpeg)
    } else if head.starts_with(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if head.len() > 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if head.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if String::from_utf8_lossy(head).contains("<svg") {
        Some(ImageFormat::Svg)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "/ws/repo/docs/guide/README.md";
    const ROOT: &str = "/ws/repo";

    fn link(url: &str) -> Link {
        resolve_link(url, Path::new(FILE), Some(Path::new(ROOT)))
    }

    fn local(path: &str, line: Option<u32>) -> Link {
        Link::Local { path: PathBuf::from(path), line }
    }

    #[test]
    fn web_links_open_externally() {
        assert_eq!(link("https://github.com/x/y"), Link::External("https://github.com/x/y".into()));
        assert_eq!(link("mailto:a@b.c"), Link::External("mailto:a@b.c".into()));
        assert_eq!(link(" http://x.io "), Link::External("http://x.io".into()));
    }

    #[test]
    fn relative_links_resolve_from_the_file() {
        assert_eq!(link("setup.md"), local("/ws/repo/docs/guide/setup.md", None));
        assert_eq!(link("./img/../setup.md"), local("/ws/repo/docs/guide/setup.md", None));
        assert_eq!(link("../../src/main.rs#L42"), local("/ws/repo/src/main.rs", Some(42)));
        assert_eq!(link("../../src/main.rs#L10-L20"), local("/ws/repo/src/main.rs", Some(10)));
        assert_eq!(link("../api/"), local("/ws/repo/docs/api", None));
        assert_eq!(link("my%20notes.md?plain=1"), local("/ws/repo/docs/guide/my notes.md", None));
    }

    #[test]
    fn root_links_resolve_from_the_repo() {
        assert_eq!(link("/PLAN.md"), local("/ws/repo/PLAN.md", None));
        assert_eq!(resolve_link("/etc/x.md", Path::new("/tmp/a.md"), None), local("/etc/x.md", None));
    }

    #[test]
    fn anchors_and_file_urls() {
        assert_eq!(link("#task-lists"), Link::Anchor("task-lists".into()));
        assert_eq!(link("file:///tmp/a%20b.md"), local("/tmp/a b.md", None));
        assert_eq!(link("setup.md#install"), local("/ws/repo/docs/guide/setup.md", None));
    }

    #[test]
    fn images_resolve_from_the_file() {
        let img = |u: &str| resolve_image(u, Path::new(FILE), Some(Path::new(ROOT)));
        assert_eq!(img("shot.png"), ImageRef::Local("/ws/repo/docs/guide/shot.png".into()));
        assert_eq!(img("../../assets/logo.svg?raw=true"), ImageRef::Local("/ws/repo/assets/logo.svg".into()));
        assert_eq!(img("/assets/a%20b.png"), ImageRef::Local("/ws/repo/assets/a b.png".into()));
        assert_eq!(img("https://img.shields.io/badge/x-y-blue"), ImageRef::Remote("https://img.shields.io/badge/x-y-blue".into()));
        assert_eq!(img("file:///tmp/x.png"), ImageRef::Local("/tmp/x.png".into()));
        assert!(matches!(img("data:image/png;base64,AAAA"), ImageRef::Data(_)));
    }

    #[test]
    fn windows_drive_is_not_a_scheme() {
        assert_eq!(scheme("C:/x.png"), None);
        assert_eq!(scheme("https://x"), Some("https"));
    }

    #[test]
    fn data_urls_decode() {
        let svg = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=";
        assert_eq!(decode_data_url(svg).map(|i| i.format), Some(ImageFormat::Svg));
        assert!(decode_data_url("data:text/plain,hello").is_none());
    }

    #[test]
    fn headings_have_github_anchors() {
        assert_eq!(slug("Task lists & tables"), "task-lists--tables");
        assert_eq!(slug("Stage 3: sessions"), "stage-3-sessions");
        let src = "# Title\n\n```\n# not a heading\n```\n\n## Code `blocks`\ntext\n";
        assert_eq!(heading_range(src, "code-blocks").map(|r| &src[r]), Some("## Code `blocks`"));
        assert_eq!(heading_range(src, "not-a-heading"), None);
    }
}
