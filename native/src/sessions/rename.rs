//! Renaming a session inline, in its tab or sidebar row (App.tsx `renameTab`):
//! Enter or leaving the field saves, Esc cancels, an empty name restores the codename.
use crate::app::OrbitApp;
use crate::theme::{self, c};
use gpui::{actions, div, prelude::*, px, AnyElement, App, Context, Entity, Focusable, KeyBinding, MouseButton, Subscription, Window};
use gpui_component::input::{Input, InputEvent, InputState};

actions!(sessions, [CancelRename]);

const CONTEXT: &str = "SessionRename";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", CancelRename, Some("SessionRename > Input"))]);
}

/// Where the field is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    Tab,
    Sidebar,
}

pub struct Rename {
    /// The session's key (`SessionMeta::id`).
    pub key: String,
    pub place: Place,
    input: Entity<InputState>,
    _sub: Subscription,
}

impl OrbitApp {
    pub fn start_session_rename(&mut self, key: &str, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = super::store(cx).read(cx).view(key) else { return };
        let name = view.read(cx).meta.session_name.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name.clone()));
        input.update(cx, |i, cx| i.set_selected_range(0..name.len(), cx));
        let sub = cx.subscribe_in(&input, window, |a, _, ev: &InputEvent, w, cx| match ev {
            InputEvent::PressEnter { .. } | InputEvent::Blur => a.finish_session_rename(true, w, cx),
            _ => {}
        });
        window.focus(&input.focus_handle(cx), cx);
        self.chrome.session_rename = Some(Rename { key: key.to_string(), place, input, _sub: sub });
        cx.notify();
    }

    fn finish_session_rename(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.chrome.session_rename.take() else { return };
        if save {
            let name = r.input.read(cx).value().to_string();
            if let Err(e) = super::store(cx).update(cx, |s, cx| s.rename(&r.key, &name, cx)) {
                self.toast_error(e, cx);
            }
        }
        // Enter / Esc: back to the tab. A blur keeps the focus where it went.
        if r.input.focus_handle(cx).is_focused(window) {
            self.restore_focus(window, cx);
        }
        cx.notify();
    }

    /// The field, when session `key` is being renamed at `place`.
    pub fn session_rename_field(&self, key: &str, place: Place, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.chrome.session_rename.as_ref().filter(|r| r.key == key && r.place == place)?;
        Some(
            // `.tab-rename`: 22px high, 120px wide, 12px, radius 4, accent border.
            div()
                .key_context(CONTEXT)
                .on_action(cx.listener(|a, _: &CancelRename, w, cx| a.finish_session_rename(false, w, cx)))
                // Clicks in the field stay out of the tab / row under it.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .map(|d| if place == Place::Tab { d.flex_none().w(px(120.)) } else { d.flex_1().min_w_0() })
                .h(px(22.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(c(theme::ACCENT))
                .bg(c(theme::BG))
                .child(Input::new(&r.input).appearance(false).text_size(px(12.)))
                .child(crate::probe::probe("session-rename"))
                .into_any_element(),
        )
    }
}
