//! Integrations (`IntegrationsPage.tsx`): connect Shortcut, Linear and Figma
//! with an API token, validated against the provider before it is saved.
use super::kit::{self, Kind};
use super::ui;
use super::{Host, PageView};
use crate::theme::{self, c};
use gpui::{div, img, prelude::*, px, App, ClickEvent, Context, Entity, FontWeight, Image, ImageFormat, SharedString, Window};
use std::sync::{Arc, OnceLock};
use gpui_component::input::{InputEvent, InputState};
use orbit_core::integrations::{self, IntegrationStatus, TrackerKind};
use std::fmt::Write as _;

struct Def {
    kind: TrackerKind,
    slug: &'static str,
    name: &'static str,
    description: &'static str,
    token_url: &'static str,
    token_hint: &'static str,
}

const DEFS: &[Def] = &[
    Def {
        kind: TrackerKind::Shortcut,
        slug: "shortcut",
        name: "Shortcut",
        description: "Track feature cards, stories and workflows.",
        token_url: "https://app.shortcut.com/settings/account/api-tokens",
        token_hint: "Shortcut → Settings → API Tokens",
    },
    Def {
        kind: TrackerKind::Linear,
        slug: "linear",
        name: "Linear",
        description: "Track issues, cycles and project status.",
        token_url: "https://linear.app/settings/api",
        token_hint: "Linear → Settings → API",
    },
    Def {
        kind: TrackerKind::Figma,
        slug: "figma",
        name: "Figma",
        description: "Preview designs attached to feature cards.",
        token_url: "https://www.figma.com/settings",
        token_hint: "Figma → Settings → Personal access tokens",
    },
];

struct Row {
    status: Option<IntegrationStatus>,
    token: Entity<InputState>,
    show: bool,
    testing: bool,
    error: Option<String>,
}

pub struct IntegrationsPage {
    host: Host,
    rows: Vec<Row>,
}

impl IntegrationsPage {
    pub fn new(host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rows = DEFS
            .iter()
            .enumerate()
            .map(|(ix, d)| {
                let token = cx.new(|cx| InputState::new(window, cx).placeholder(format!("{} API token", d.name)).masked(true));
                cx.subscribe_in(&token, window, move |t: &mut Self, _, ev: &InputEvent, w, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. }) {
                        t.connect(ix, w, cx);
                    }
                })
                .detach();
                Row { status: None, token, show: false, testing: false, error: None }
            })
            .collect();
        let mut page = Self { host, rows };
        page.load(window, cx);
        page
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ui::bg(
            cx,
            || DEFS.iter().map(|d| integrations::status(d.kind)).collect::<Vec<_>>(),
            |t: &mut Self, list, _, _| {
                for (row, st) in t.rows.iter_mut().zip(list) {
                    row.status = Some(st);
                }
            },
            window,
        );
    }

    fn connect(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let row = &mut self.rows[ix];
        let token = row.token.read(cx).value().trim().to_string();
        if token.is_empty() || row.testing {
            return;
        }
        row.testing = true;
        row.error = None;
        let kind = DEFS[ix].kind;
        ui::bg(
            cx,
            move || integrations::connect(kind, &token),
            move |t: &mut Self, r, window, cx| {
                let row = &mut t.rows[ix];
                row.testing = false;
                match r {
                    Ok(account) => {
                        row.status = Some(IntegrationStatus { kind: DEFS[ix].slug.into(), connected: true, account: Some(account) });
                        row.token.update(cx, |i, cx| i.set_value("", window, cx));
                    }
                    Err(e) => row.error = Some(e),
                }
            },
            window,
        );
        cx.notify();
    }

    fn disconnect(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let kind = DEFS[ix].kind;
        ui::bg(
            cx,
            move || integrations::remove_token(kind),
            move |t: &mut Self, r, _, cx| match r {
                Ok(()) => t.rows[ix].status = Some(IntegrationStatus { kind: DEFS[ix].slug.into(), connected: false, account: None }),
                Err(e) => t.host.error("Couldn't disconnect", e, cx),
            },
            window,
        );
    }
}

/// The brand marks of `BrandIcons.tsx` (multi-colour, so drawn as images).
fn logo(kind: TrackerKind) -> gpui::Img {
    static LOGOS: OnceLock<[Arc<Image>; 3]> = OnceLock::new();
    let logos = LOGOS.get_or_init(|| {
        let svg = |body: &str| Arc::new(Image::from_bytes(ImageFormat::Svg, format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 32 32" fill="none">{body}</svg>"#).into_bytes()));
        [
            svg(r##"<rect width="32" height="32" rx="8" fill="#57B5E1"/><path d="M20.5 11.2c-1.2-1-3-1.6-4.7-1.6-2.6 0-4.4 1.2-4.4 3 0 1.7 1.4 2.5 3.9 3 2.8.6 4.7 1.3 4.7 3.5 0 2.3-2.1 3.7-5 3.7-2 0-4-.7-5.3-1.9" stroke="#fff" stroke-width="2.2" stroke-linecap="round" fill="none"/>"##),
            svg(r##"<rect width="32" height="32" rx="8" fill="#5E6AD2"/><path d="M15.4 22.5V9.5h5.1v2.9h-5.1" fill="#fff"/><path d="M15.4 22.5c-3 0-5.1-1.9-5.1-4.7v-2.9h5.1" fill="#fff" opacity="0.6"/>"##),
            svg(r##"<rect width="32" height="32" rx="8" fill="#fff"/><path d="M11.5 24a3.5 3.5 0 0 0 3.5-3.5V17h-3.5a3.5 3.5 0 0 0 0 7z" fill="#0acf83"/><path d="M11.5 10a3.5 3.5 0 0 1 3.5 3.5V17h-3.5a3.5 3.5 0 0 1 0-7z" fill="#a259ff"/><path d="M15 10h3.5a3.5 3.5 0 0 1 0 7H15V10z" fill="#f7266f"/><path d="M15 17h3.5a3.5 3.5 0 0 1 0 7H15v-7z" fill="#ff7262"/><path d="M15 3a3.5 3.5 0 0 0-3.5 3.5V10H15a3.5 3.5 0 0 0 0-7z" fill="#1abcfe"/>"##),
        ]
    });
    let ix = match kind {
        TrackerKind::Shortcut => 0,
        TrackerKind::Linear => 1,
        _ => 2,
    };
    img(logos[ix].clone()).size(px(28.)).flex_none()
}

impl Render for IntegrationsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = div()
            .id("integrations-body")
            .size_full()
            .overflow_y_scroll()
            .px(px(44.))
            .py(px(28.))
            .flex()
            .flex_col()
            .child(div().mb(px(18.)).text_size(px(19.)).font_weight(FontWeight(650.)).text_color(c(theme::TEXT)).child("Integrations"))
            .child(
                div()
                    .mt(px(-8.))
                    .mb(px(18.))
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .text_color(c(theme::TEXT_FAINT))
                    .child("Connect your tools so Orbit can pull card and design context. Tokens are stored locally and never leave your machine."),
            );
        for (ix, (d, row)) in DEFS.iter().zip(&self.rows).enumerate() {
            let badge = |on: bool| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(12.))
                    .py(px(4.))
                    .rounded_full()
                    .border_1()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(on, |d| d.text_color(c(theme::OK)).bg(kit::alpha(theme::OK, 0.08)).border_color(kit::alpha(theme::OK, 0.3)))
                    .when(!on, |d| d.text_color(c(theme::TEXT_FAINT)).bg(c(theme::PANEL_RAISED)).border_color(c(theme::BORDER)))
                    .child(div().size(px(7.)).rounded_full().bg(c(if on { theme::OK } else { theme::TEXT_FAINT })))
                    .child(if on { "Connected" } else { "Not connected" })
            };
            let status = match &row.status {
                None => kit::skeleton(px(90.), 18.).pill().into_any_element(),
                Some(s) => badge(s.connected).into_any_element(),
            };
            let mut card = kit::card().mb(px(14.)).px(px(20.)).py(px(16.)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .child(logo(d.kind))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().text_size(px(14.5)).font_weight(FontWeight::SEMIBOLD).text_color(c(theme::TEXT)).child(d.name))
                            .child(div().text_size(px(12.)).text_color(c(theme::TEXT_FAINT)).child(d.description)),
                    )
                    .child(status),
            );
            match &row.status {
                Some(s) if s.connected => {
                    card = card.child(
                        div()
                            .mt(px(14.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(c(theme::TEXT_DIM))
                                    .child(s.account.as_ref().map_or("Token saved locally".to_string(), |a| format!("Signed in as {a}"))),
                            )
                            .child(
                                kit::button(format!("integration:{}:disconnect", d.slug), "Disconnect", Kind::DangerOutline)
                                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.disconnect(ix, w, cx))),
                            ),
                    );
                }
                Some(_) => {
                    let url = d.token_url;
                    let empty = row.token.read(cx).value().trim().is_empty();
                    card = card
                        .child(
                            div()
                                .mt(px(14.))
                                .mb(px(12.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(12.5))
                                .text_color(c(theme::TEXT_FAINT))
                                .child("Create an API token at")
                                .child(kit::link(format!("integration:{}:token-page", d.slug), d.token_hint).on_click(move |_, _, cx| cx.open_url(url)))
                                .child("and paste it below."),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(kit::field(format!("integration:{}:token", d.slug), &row.token, window, cx))
                                .child(kit::button(format!("integration:{}:show", d.slug), if row.show { "Hide" } else { "Show" }, Kind::Secondary).on_click(
                                    cx.listener(move |t, _: &ClickEvent, w, cx| {
                                        let row = &mut t.rows[ix];
                                        row.show = !row.show;
                                        let masked = !row.show;
                                        row.token.update(cx, |i, cx| i.set_masked(masked, w, cx));
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    kit::disabled(
                                        kit::button_busy(format!("integration:{}:connect", d.slug), if row.testing { "Testing…" } else { "Connect" }, Kind::Primary, row.testing),
                                        row.testing || empty,
                                    )
                                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.connect(ix, w, cx))),
                                ),
                        )
                        .when_some(row.error.clone(), |d, e| d.child(div().mt(px(10.)).text_size(px(12.5)).text_color(c(theme::DANGER)).child(e)));
                }
                None => {}
            }
            page = page.child(card);
        }
        crate::motion::page_in(page, "integrations-in")
    }
}

impl PageView for IntegrationsPage {
    fn title(&self) -> SharedString {
        "Integrations".into()
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(window, cx);
    }

    fn dump(&self, out: &mut String, cx: &App) {
        for (d, row) in DEFS.iter().zip(&self.rows) {
            let status = match &row.status {
                None => "…".to_string(),
                Some(s) if s.connected => format!("connected as {:?}", s.account),
                Some(_) => "not connected".into(),
            };
            let typed = row.token.read(cx).value().chars().count();
            let _ = writeln!(
                out,
                "  {} {status} · token typed {typed} chars{}{}",
                d.slug,
                if row.testing { " · testing" } else { "" },
                row.error.as_ref().map(|e| format!(" · error {e}")).unwrap_or_default()
            );
        }
    }
}
