//! Orbit Dark: the palette and metrics of `src/App.css` (:root and the shell
//! classes) and `src/lib/theme.ts`, as constants for our own views and as a
//! gpui-component theme for its widgets.
use gpui::{px, rgb, rgba, App, BoxShadow, Hsla, Pixels, Rgba};
use gpui_component::theme::{Theme, ThemeMode, ThemeRegistry};

pub const BG: u32 = 0x0d0f13;
pub const PANEL: u32 = 0x14171d;
pub const PANEL_RAISED: u32 = 0x1a1e26;
pub const PANEL_HOVER: u32 = 0x232936;
pub const BORDER: u32 = 0x232833;
pub const BORDER_STRONG: u32 = 0x2e3440;
pub const TEXT: u32 = 0xe6e8ec;
pub const TEXT_DIM: u32 = 0x9aa1ad;
pub const TEXT_FAINT: u32 = 0x6b7280;
pub const ACCENT: u32 = 0x4f7cf7;
pub const ACCENT_HOVER: u32 = 0x6d92ff;
pub const ON_ACCENT: u32 = 0xffffff;
pub const DANGER: u32 = 0xff5d5d;
pub const OK: u32 = 0x4ade80;
pub const WARN: u32 = 0xe3b341;

pub const MONO_FONT: &str = "DejaVu Sans Mono";
pub const UI_FONT: &str = "Noto Sans";

/// Shell metrics from App.css, in px.
pub mod size {
    /// `body { font-size }`.
    pub const FONT: f32 = 13.5;
    /// `.titlebar { height }`.
    pub const TITLEBAR: f32 = 38.;
    /// `.titlebar-btn`.
    pub const TITLEBAR_BTN_W: f32 = 28.;
    pub const TITLEBAR_BTN_H: f32 = 24.;
    /// `.tab-strip { height }` and `.tab { height; font-size }`.
    pub const TAB_STRIP: f32 = 42.;
    pub const TAB: f32 = 36.;
    pub const TAB_FONT: f32 = 12.5;
    /// `.context-menu-item { height }`, `.context-menu { padding; min-width }`.
    pub const MENU_ITEM: f32 = 30.;
    pub const MENU_PAD: f32 = 4.;
    pub const MENU_MIN_W: f32 = 200.;
    /// `--radius`.
    pub const RADIUS: f32 = 10.;
    /// `--control-height`.
    pub const CONTROL: f32 = 34.;
    pub const STATUS_BAR: f32 = 22.;
    /// `.sidebar-resizer` / `.pane-resizer` hit width.
    pub const RESIZER: f32 = 5.;
}

/// ANSI 0-7 then 8-15, Orbit Dark terminal theme.
pub const ANSI: [u32; 16] = [
    0x1a1e26, 0xff5d5d, 0x4ade80, 0xe3b341, 0x7aa7ff, 0xc792ea, 0x7fdbca, 0xe6e8ec, //
    0x4a5060, 0xff8080, 0x7ee2a8, 0xf2c96b, 0x9fb8ff, 0xd7a8f5, 0xa3eadb, 0xffffff,
];

pub fn c(hex: u32) -> Rgba {
    rgb(hex)
}

pub fn hsla(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// `--accent-soft`: the accent at 15%.
pub fn accent_soft() -> Rgba {
    rgba(0x4f7cf726)
}

/// `--danger-soft`: danger at 12%.
pub fn danger_soft() -> Rgba {
    rgba(0xff5d5d1f)
}

/// `.toast-success .toast-icon`: ok at 14%.
pub fn ok_soft() -> Rgba {
    rgba(0x4ade8024)
}

/// `.modal-overlay`: rgba(5, 6, 9, 0.55).
pub fn scrim() -> Rgba {
    rgba(0x0506098c)
}

/// `box-shadow: 0 <y>px <blur>px rgba(0, 0, 0, <alpha>)` of menus, toasts and modals.
pub fn shadow(y: f32, blur: f32, alpha: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Hsla { h: 0., s: 0., l: 0., a: alpha },
        offset: gpui::point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }]
}

pub fn radius() -> Pixels {
    px(size::RADIUS)
}

const ORBIT_THEME: &str = include_str!("orbit-dark.json");

/// Registers Orbit Dark with gpui-component and makes it the active theme.
pub fn init(cx: &mut App) {
    ThemeRegistry::global_mut(cx)
        .load_themes_from_str(ORBIT_THEME)
        .expect("orbit-dark.json is valid");
    let config = ThemeRegistry::global(cx).themes().get("Orbit Dark").cloned().expect("Orbit Dark registered");
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::global_mut(cx).apply_config(&config);
}
