//! Orbit Dark: the palette of `src/App.css` (:root) and `src/lib/theme.ts`,
//! as constants for our own views and as a gpui-component theme for its widgets.
use gpui::{rgb, rgba, App, Hsla, Rgba};
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
pub const DANGER: u32 = 0xff5d5d;
pub const OK: u32 = 0x4ade80;
pub const WARN: u32 = 0xe3b341;

pub const MONO_FONT: &str = "DejaVu Sans Mono";
pub const UI_FONT: &str = "Noto Sans";

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
