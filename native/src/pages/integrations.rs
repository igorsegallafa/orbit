//! Integrations (`IntegrationsPage.tsx`): connect Shortcut, Linear and Figma
//! with an API token, validated against the provider before it is saved.
use super::ui::{self, Tone};
use super::{Host, PageView};
use crate::theme::{self, c};
use gpui::{div, prelude::*, App, ClickEvent, Context, Entity, SharedString, Window};
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

impl Render for IntegrationsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = ui::page("integrations-body", "Integrations", "Trackers and design tools Orbit reads cards and designs from. Tokens stay on this machine.");
        for (ix, (d, row)) in DEFS.iter().zip(&self.rows).enumerate() {
            let badge = match &row.status {
                None => ui::faint("…"),
                Some(s) if s.connected => div().flex().items_center().gap_1().child(ui::dot(c(theme::OK))).child(ui::muted("Connected")),
                Some(_) => div().flex().items_center().gap_1().child(ui::dot(c(theme::TEXT_FAINT))).child(ui::muted("Not connected")),
            };
            let mut card = ui::card().p_4().flex().flex_col().gap_2().child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().flex().flex_col().child(div().text_sm().text_color(c(theme::TEXT)).child(d.name)).child(ui::muted(d.description)))
                    .child(badge),
            );
            match &row.status {
                Some(s) if s.connected => {
                    card = card.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(ui::muted(s.account.as_ref().map_or("Token saved locally".to_string(), |a| format!("Signed in as {a}")))))
                            .child(
                                ui::button(format!("integration:{}:disconnect", d.slug), "Disconnect", Tone::Danger)
                                    .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.disconnect(ix, w, cx))),
                            ),
                    );
                }
                Some(_) => {
                    let url = d.token_url;
                    card = card
                        .child(
                            div().flex().items_center().gap_1().child(ui::muted("Create an API token at")).child(
                                ui::button(format!("integration:{}:token-page", d.slug), d.token_hint, Tone::Plain).on_click(move |_, _, cx| cx.open_url(url)),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(ui::input(format!("integration:{}:token", d.slug), &row.token))
                                .child(ui::button(format!("integration:{}:show", d.slug), if row.show { "Hide" } else { "Show" }, Tone::Plain).on_click(
                                    cx.listener(move |t, _: &ClickEvent, w, cx| {
                                        let row = &mut t.rows[ix];
                                        row.show = !row.show;
                                        let masked = !row.show;
                                        row.token.update(cx, |i, cx| i.set_masked(masked, w, cx));
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    ui::button(format!("integration:{}:connect", d.slug), if row.testing { "Testing…" } else { "Connect" }, Tone::Primary)
                                        .on_click(cx.listener(move |t, _: &ClickEvent, w, cx| t.connect(ix, w, cx))),
                                ),
                        )
                        .when_some(row.error.clone(), |d, e| d.child(ui::error(e)));
                }
                None => {}
            }
            page = page.child(card);
        }
        page
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
