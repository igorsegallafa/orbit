//! The app themes of `src/lib/theme.ts` (Orbit Dark, Orbit Light, Dracula,
//! Tokyo Night, Catppuccin Mocha, Nord, Solarized Light) and the shell
//! metrics of `src/App.css`.
//!
//! Colours are tokens: `c(theme::PANEL)` resolves `PANEL` against the active
//! theme when it is drawn, so `theme::set` switches every view live. A plain
//! `0xrrggbb` passed to `c()` is drawn as is. The same palette drives the
//! gpui-component theme (inputs, menus, scrollbars, editor highlighting).
use gpui::{px, rgb, rgba, App, BoxShadow, Hsla, Pixels, Rgba};
use gpui_component::theme::{Theme, ThemeMode, ThemeRegistry};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::RwLock;

/// Marks a token (real colours are 24-bit).
const TOKEN: u32 = 0x0100_0000;
const fn token(ix: u32) -> u32 {
    TOKEN | ix
}

// The CSS variables theme.ts sets (`--bg`, `--panel`, …).
pub const BG: u32 = token(0);
pub const PANEL: u32 = token(1);
pub const PANEL_RAISED: u32 = token(2);
pub const PANEL_HOVER: u32 = token(3);
pub const BORDER: u32 = token(4);
pub const BORDER_STRONG: u32 = token(5);
pub const TEXT: u32 = token(6);
pub const TEXT_DIM: u32 = token(7);
pub const TEXT_FAINT: u32 = token(8);
pub const ACCENT: u32 = token(9);
pub const ACCENT_HOVER: u32 = token(10);
pub const ON_ACCENT: u32 = token(11);
pub const DANGER: u32 = token(12);
pub const OK: u32 = token(13);
pub const WARN: u32 = token(14);
pub const INFO: u32 = token(15);
pub const PURPLE: u32 = token(16);
// Tints App.css derives from the palette.
/// `--accent-soft`: accent at 15%.
pub const ACCENT_SOFT: u32 = token(17);
/// `--danger-soft`: danger at 12%.
pub const DANGER_SOFT: u32 = token(18);
/// `color-mix(in srgb, var(--info) 40%, var(--text))`: active nav items.
pub const NAV_ACTIVE: u32 = token(19);
/// Ok at 14% (success toasts).
pub const OK_SOFT: u32 = token(20);
/// Info at 10% (the status bar's model chip).
pub const INFO_SOFT: u32 = token(21);
/// Accent at 24% (the selected row of the new-session menu).
pub const ACCENT_STRONG_SOFT: u32 = token(23);
/// Text at 8% (`.nav-section-add:hover`).
pub const TEXT_SOFT: u32 = token(24);
// Syntax colours (Monaco tokens).
pub const SYN_COMMENT: u32 = token(32);
pub const SYN_KEYWORD: u32 = token(33);
pub const SYN_STRING: u32 = token(34);
pub const SYN_NUMBER: u32 = token(35);
pub const SYN_TYPE: u32 = token(36);
pub const SYN_FUNCTION: u32 = token(37);
pub const SYN_NAMESPACE: u32 = token(38);
pub const SYN_PARAMETER: u32 = token(39);
pub const SYN_PROPERTY: u32 = token(40);
pub const SYN_CONSTANT: u32 = token(41);
const ANSI_BASE: u32 = 48;
const TOKENS: usize = 64;

/// The terminal's ANSI 0-7 then 8-15, of the active theme.
pub const ANSI: [u32; 16] = {
    let mut a = [0; 16];
    let mut i = 0;
    while i < 16 {
        a[i] = token(ANSI_BASE + i as u32);
        i += 1;
    }
    a
};

/// The UI font (`body { font-family }` resolves to the system sans on Linux).
#[cfg(not(target_os = "macos"))]
pub const UI_FONT: &str = "Noto Sans";
/// macOS has no Noto: GPUI's name for the system font (SF Pro).
#[cfg(target_os = "macos")]
pub const UI_FONT: &str = ".SystemUIFont";
/// `--mono` and the terminal's `ui-monospace, …, monospace`: the system monospace.
#[cfg(not(target_os = "macos"))]
pub const MONO_FONT: &str = "Noto Sans Mono";
#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = "Menlo";

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
    /// `.usage-bar { height }`, the window's bottom bar.
    pub const STATUS_BAR: f32 = 26.;
    /// `.sidebar-resizer` / `.dock-resizer` width.
    pub const RESIZER: f32 = 4.;
    /// `.nav-item` / `.nav-sub` heights.
    pub const NAV_ITEM: f32 = 32.;
    pub const NAV_SUB: f32 = 28.;
}

/// One theme of `THEMES` in theme.ts.
pub struct ThemeDef {
    pub id: &'static str,
    pub label: &'static str,
    pub dark: bool,
    /// bg, panel, panelRaised, panelHover, border, borderStrong, text, textDim,
    /// textFaint, accent, accentHover, onAccent, danger, ok, warn, info, purple.
    ui: [u32; 17],
    /// comment, keyword, string, number, type, function, namespace, parameter, property, constant.
    syntax: [u32; 10],
    ansi: [u32; 8],
    ansi_bright: [u32; 8],
}

pub static THEMES: [ThemeDef; 7] = [
    ThemeDef {
        id: "orbit-dark",
        label: "Orbit Dark",
        dark: true,
        ui: [
            0x0d0f13, 0x14171d, 0x1a1e26, 0x232936, 0x232833, 0x2e3440, 0xe6e8ec, 0x9aa1ad, 0x6b7280, 0x4f7cf7, 0x6d92ff, 0xffffff,
            0xff5d5d, 0x4ade80, 0xe3b341, 0x7aa7ff, 0xb48cff,
        ],
        syntax: [0x6b7280, 0xc792ea, 0xa5d6a7, 0xf78c6c, 0x7aa7ff, 0x82aaff, 0x7fdbca, 0xe6c07b, 0xb4c5e4, 0xf7b267],
        ansi: [0x1a1e26, 0xff5d5d, 0x4ade80, 0xe3b341, 0x7aa7ff, 0xc792ea, 0x7fdbca, 0xe6e8ec],
        ansi_bright: [0x4a5060, 0xff8080, 0x7ee2a8, 0xf2c96b, 0x9fb8ff, 0xd7a8f5, 0xa3eadb, 0xffffff],
    },
    ThemeDef {
        id: "orbit-light",
        label: "Orbit Light",
        dark: false,
        ui: [
            0xf6f7f9, 0xffffff, 0xf0f2f5, 0xe7eaef, 0xe1e4ea, 0xcfd4dc, 0x1d2129, 0x555d6b, 0x8a919e, 0x3b6ef5, 0x2f5fe0, 0xffffff,
            0xd83b3b, 0x1f9d55, 0xb7791f, 0x2f6fd6, 0x7c4dda,
        ],
        syntax: [0x8a919e, 0x8e3fb5, 0x2e7d32, 0xc05621, 0x1f62c4, 0x3056c9, 0x0f7b75, 0x9a6700, 0x3b5b8c, 0xb35c00],
        ansi: [0x1d2129, 0xd83b3b, 0x1f9d55, 0xb7791f, 0x3b6ef5, 0x8e3fb5, 0x0f7b75, 0xc9ced6],
        ansi_bright: [0x555d6b, 0xe55b5b, 0x2fb86a, 0xd19a2a, 0x5b86f7, 0xa660cc, 0x1a9990, 0xf6f7f9],
    },
    ThemeDef {
        id: "dracula",
        label: "Dracula",
        dark: true,
        ui: [
            0x282a36, 0x21222c, 0x2f3141, 0x3a3d4f, 0x343746, 0x44475a, 0xf8f8f2, 0xbfc2d4, 0x6272a4, 0xbd93f9, 0xcaa9fa, 0x282a36,
            0xff5555, 0x50fa7b, 0xf1fa8c, 0x8be9fd, 0xff79c6,
        ],
        syntax: [0x6272a4, 0xff79c6, 0xf1fa8c, 0xbd93f9, 0x8be9fd, 0x50fa7b, 0x8be9fd, 0xffb86c, 0xf8f8f2, 0xbd93f9],
        ansi: [0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2],
        ansi_bright: [0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff],
    },
    ThemeDef {
        id: "tokyo-night",
        label: "Tokyo Night",
        dark: true,
        ui: [
            0x1a1b26, 0x16161e, 0x1f2335, 0x292e42, 0x232433, 0x3b4261, 0xc0caf5, 0xa9b1d6, 0x565f89, 0x7aa2f7, 0x89b4fa, 0x1a1b26,
            0xf7768e, 0x9ece6a, 0xe0af68, 0x7dcfff, 0xbb9af7,
        ],
        syntax: [0x565f89, 0xbb9af7, 0x9ece6a, 0xff9e64, 0x2ac3de, 0x7aa2f7, 0x7dcfff, 0xe0af68, 0x73daca, 0xff9e64],
        ansi: [0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6],
        ansi_bright: [0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5],
    },
    ThemeDef {
        id: "catppuccin-mocha",
        label: "Catppuccin Mocha",
        dark: true,
        ui: [
            0x1e1e2e, 0x181825, 0x24273a, 0x313244, 0x313244, 0x45475a, 0xcdd6f4, 0xa6adc8, 0x6c7086, 0x89b4fa, 0xb4befe, 0x1e1e2e,
            0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89dceb, 0xcba6f7,
        ],
        syntax: [0x6c7086, 0xcba6f7, 0xa6e3a1, 0xfab387, 0xf9e2af, 0x89b4fa, 0x94e2d5, 0xeba0ac, 0xb4befe, 0xfab387],
        ansi: [0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de],
        ansi_bright: [0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8],
    },
    ThemeDef {
        id: "nord",
        label: "Nord",
        dark: true,
        ui: [
            0x2e3440, 0x292e39, 0x343a47, 0x3b4252, 0x3b4252, 0x4c566a, 0xeceff4, 0xc0c8d6, 0x7b88a1, 0x88c0d0, 0xa3d3df, 0x2e3440,
            0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead,
        ],
        syntax: [0x616e88, 0x81a1c1, 0xa3be8c, 0xb48ead, 0x8fbcbb, 0x88c0d0, 0x8fbcbb, 0xd8dee9, 0xd8dee9, 0xd08770],
        ansi: [0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0],
        ansi_bright: [0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4],
    },
    ThemeDef {
        id: "solarized-light",
        label: "Solarized Light",
        dark: false,
        ui: [
            0xfdf6e3, 0xeee8d5, 0xf5efdc, 0xe6dfca, 0xddd6c1, 0xc9c2ab, 0x073642, 0x586e75, 0x93a1a1, 0x268bd2, 0x1f76b5, 0xfdf6e3,
            0xdc322f, 0x859900, 0xb58900, 0x2aa198, 0x6c71c4,
        ],
        syntax: [0x93a1a1, 0x859900, 0x2aa198, 0xd33682, 0xb58900, 0x268bd2, 0xcb4b16, 0x657b83, 0x6c71c4, 0xcb4b16],
        ansi: [0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5],
        ansi_bright: [0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3],
    },
];

/// Used when nothing was chosen and the Tauri app has no choice either: the user's theme.
pub const DEFAULT: &str = "nord";
/// Follows the OS: Orbit Dark or Orbit Light.
pub const SYSTEM: &str = "system";

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU32 = AtomicU32::new(0);
/// The active theme's tokens as 0xRRGGBBAA.
static ACTIVE: [AtomicU32; TOKENS] = [ZERO; TOKENS];
static CURRENT: AtomicUsize = AtomicUsize::new(usize::MAX);
static PREF: RwLock<String> = RwLock::new(String::new());

/// `a` blended over `b` by `t` (theme.ts `mix`), 0xRRGGBB.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |v: u32, s: u32| ((v >> s) & 0xff) as f32;
    [16, 8, 0].iter().fold(0, |acc, &s| acc | (((ch(a, s) * t + ch(b, s) * (1. - t)).round() as u32) << s))
}

/// 0xRRGGBB at opacity `a`, as 0xRRGGBBAA.
fn with_alpha(hex: u32, a: f32) -> u32 {
    (hex << 8) | (a * 255.).round() as u32
}

impl ThemeDef {
    fn tokens(&self) -> [u32; TOKENS] {
        let mut t = [0u32; TOKENS];
        for (i, v) in self.ui.iter().enumerate() {
            t[i] = with_alpha(*v, 1.);
        }
        let ui = |tok: u32| self.ui[(tok & 0xff) as usize];
        let soft = |tok: u32, a: f32| with_alpha(ui(tok), a);
        t[17] = soft(ACCENT, 0.15);
        t[18] = soft(DANGER, 0.12);
        t[19] = with_alpha(mix(ui(INFO), ui(TEXT), 0.4), 1.);
        t[20] = soft(OK, 0.14);
        t[21] = soft(INFO, 0.10);
        t[23] = soft(ACCENT, 0.24);
        t[24] = soft(TEXT, 0.08);
        for (i, v) in self.syntax.iter().enumerate() {
            t[32 + i] = with_alpha(*v, 1.);
        }
        for (i, v) in self.ansi.iter().chain(self.ansi_bright.iter()).enumerate() {
            t[ANSI_BASE as usize + i] = with_alpha(*v, 1.);
        }
        t
    }

    /// A token of this theme as 0xRRGGBB.
    pub fn hex(&self, tok: u32) -> u32 {
        self.tokens()[(tok & 0xff) as usize] >> 8
    }
}

/// A colour: a token of the active theme, or a literal 0xRRGGBB.
pub fn c(hex: u32) -> Rgba {
    if hex & TOKEN != 0 {
        rgba(ACTIVE[(hex & 0xff) as usize % TOKENS].load(Ordering::Relaxed))
    } else {
        rgb(hex)
    }
}

pub fn hsla(hex: u32) -> Hsla {
    c(hex).into()
}

/// A colour at opacity `a` (0..1), e.g. `theme::alpha(theme::WARN, 0.12)`.
pub fn alpha(hex: u32, a: f32) -> Rgba {
    let mut v = c(hex);
    v.a *= a;
    v
}

/// The active theme's value of a token as 0xRRGGBB.
pub fn hex(tok: u32) -> u32 {
    if tok & TOKEN != 0 {
        ACTIVE[(tok & 0xff) as usize % TOKENS].load(Ordering::Relaxed) >> 8
    } else {
        tok
    }
}

/// `--accent-soft`.
pub fn accent_soft() -> Rgba {
    c(ACCENT_SOFT)
}

/// `--danger-soft`.
pub fn danger_soft() -> Rgba {
    c(DANGER_SOFT)
}

/// `.toast-success .toast-icon`.
pub fn ok_soft() -> Rgba {
    c(OK_SOFT)
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

/// Every theme, in the order of theme.ts (Settings → Appearance lists these, plus "system").
pub fn list() -> &'static [ThemeDef] {
    &THEMES
}

/// The applied theme.
pub fn current() -> &'static ThemeDef {
    &THEMES[CURRENT.load(Ordering::Relaxed).min(THEMES.len() - 1)]
}

/// The chosen theme id, or "system" (what is saved).
pub fn pref() -> String {
    let p = PREF.read().map(|p| p.clone()).unwrap_or_default();
    if p.is_empty() {
        current().id.to_string()
    } else {
        p
    }
}

fn find(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.id == name || t.label.eq_ignore_ascii_case(name))
}

fn resolve(pref: &str, cx: &App) -> usize {
    if pref == SYSTEM {
        let dark = matches!(cx.window_appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
        return find(if dark { "orbit-dark" } else { "orbit-light" }).unwrap_or(0);
    }
    find(pref).unwrap_or(0)
}

/// Switches to theme `name` (an id or label from [`list`], or "system") and
/// redraws every window. Unknown names are refused. The app's state save picks the choice up.
pub fn set(name: &str, cx: &mut App) -> bool {
    if name != SYSTEM && find(name).is_none() {
        return false;
    }
    let ix = resolve(name, cx);
    if let Ok(mut p) = PREF.write() {
        *p = if name == SYSTEM { SYSTEM.into() } else { THEMES[ix].id.into() };
    }
    apply(ix, cx);
    true
}

fn apply(ix: usize, cx: &mut App) {
    let def = &THEMES[ix];
    for (slot, v) in ACTIVE.iter().zip(def.tokens()) {
        slot.store(v, Ordering::Relaxed);
    }
    CURRENT.store(ix, Ordering::Relaxed);
    let json = component_theme(def).to_string();
    if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(&json) {
        eprintln!("theme {}: {e}", def.id);
        return;
    }
    let Some(config) = ThemeRegistry::global(cx).themes().get(def.label).cloned() else { return };
    let mode = if def.dark { ThemeMode::Dark } else { ThemeMode::Light };
    if !cx.has_global::<Theme>() {
        Theme::change(mode, None, cx);
    }
    Theme::update(cx, |t| {
        t.apply_config(&config);
        t.mode = mode;
    });
    vscode_scrollbars(cx);
    cx.refresh_windows();
}

/// VS Code's scrollbar thumb colours (rest, hover, dragging): translucent, so
/// the change and problem marks under it stay visible.
pub fn scrollbar_thumb_colors() -> (Hsla, Hsla, Hsla) {
    let base = hsla(TEXT_FAINT);
    (base.opacity(0.4), base.opacity(0.6), base.opacity(0.75))
}

/// The component scrollbars (editor, lists) as VS Code's: a 14px square thumb
/// filling the track, shown while the mouse is over the view.
fn vscode_scrollbars(cx: &mut App) {
    let (rest, hover, active) = scrollbar_thumb_colors();
    let thumb = |s: gpui_base::ScrollbarThumbStyle, bg: Hsla| s.width(px(14.)).inset(px(1.)).radius(px(0.)).bg(bg);
    let base = &mut gpui_base::Theme::global_mut(cx).scrollbar;
    let styles = base
        .styles()
        .clone()
        .thumb(|s| thumb(s, rest))
        .thumb_hover(|s| thumb(s, hover))
        .thumb_active(|s| thumb(s, active));
    *base = gpui_base::ScrollbarTheme::new()
        .with_mode(gpui_base::ScrollbarMode::Hover)
        .with_motion(base.motion())
        .with_styles(styles);
}

/// Applies the saved choice; with none, the Tauri app's (its localStorage), else Nord.
pub fn init(cx: &mut App, saved: Option<&str>) {
    let pick = saved
        .filter(|s| *s == SYSTEM || find(s).is_some())
        .map(str::to_string)
        .or_else(tauri_theme)
        .unwrap_or_else(|| DEFAULT.into());
    if !set(&pick, cx) {
        set(DEFAULT, cx);
    }
}

/// `orbit.theme` from the Tauri app's webview storage (WebKitGTK keeps
/// localStorage in SQLite; the value is UTF-16 right after its key). Read-only.
fn tauri_theme() -> Option<String> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))?;
    let db = base.join("com.nsx001220.orbit/localstorage/tauri_localhost_0.localstorage");
    // The write-ahead log holds the newest value.
    [db.with_extension("localstorage-wal"), db]
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .find_map(|bytes| stored_theme(&bytes))
}

fn stored_theme(bytes: &[u8]) -> Option<String> {
    let key = b"orbit.theme";
    let ids = THEMES.iter().map(|t| t.id).chain([SYSTEM]);
    let utf16 = |s: &str| s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>();
    let candidates: Vec<(String, Vec<u8>)> = ids.map(|id| (id.to_string(), utf16(id))).collect();
    let mut found = None;
    let mut at = 0;
    while let Some(p) = bytes[at..].windows(key.len()).position(|w| w == key) {
        let rest = &bytes[at + p + key.len()..];
        // The longest id that matches ("orbit-dark" vs a hypothetical "orbit-dark-x").
        if let Some((id, _)) = candidates.iter().filter(|(_, enc)| rest.starts_with(enc)).max_by_key(|(_, enc)| enc.len()) {
            found = Some(id.clone());
        }
        at += p + key.len();
    }
    found
}

/// The gpui-component theme for `t`, mapped as `orbit-dark.json` maps Orbit Dark.
fn component_theme(t: &ThemeDef) -> Value {
    let h = |tok: u32| format!("#{:06x}", t.hex(tok));
    let ha = |tok: u32, a: f32| format!("#{:06x}{:02x}", t.hex(tok), (a * 255.).round() as u32);
    let mixed = |a: u32, b: u32, f: f32| format!("#{:06x}", mix(t.hex(a), t.hex(b), f));
    let s = |tok: u32| json!({ "color": h(tok) });
    json!({
        "name": "Orbit",
        "author": "Orbit",
        "themes": [{
            "name": t.label,
            "mode": if t.dark { "dark" } else { "light" },
            "font.family": UI_FONT,
            "font.size": size::FONT,
            "mono_font.family": MONO_FONT,
            "mono_font.size": 13,
            "radius": 6,
            "radius.lg": 10,
            "shadow": false,
            "colors": {
                "background": h(BG),
                "foreground": h(TEXT),
                "border": h(BORDER),
                "input.border": h(BORDER_STRONG),
                "ring": h(ACCENT),
                "caret": h(TEXT),
                "selection.background": ha(ACCENT, 0.4),
                "accent.background": h(PANEL_HOVER),
                "accent.foreground": h(TEXT),
                "muted.background": h(PANEL_RAISED),
                "muted.foreground": h(TEXT_DIM),
                "popover.background": h(PANEL_RAISED),
                "popover.foreground": h(TEXT),
                "primary.background": h(ACCENT),
                "primary.hover.background": h(ACCENT_HOVER),
                "primary.active.background": h(ACCENT),
                "primary.foreground": h(ON_ACCENT),
                "secondary.background": h(PANEL_RAISED),
                "secondary.hover.background": h(PANEL_HOVER),
                "secondary.active.background": h(BORDER_STRONG),
                "secondary.foreground": h(TEXT),
                "danger.background": h(DANGER),
                "danger.foreground": h(DANGER),
                "success.background": h(OK),
                "success.foreground": h(OK),
                "warning.background": h(WARN),
                "warning.foreground": h(WARN),
                "info.background": h(INFO),
                "info.foreground": h(INFO),
                "link.foreground": h(INFO),
                "link.hover.foreground": h(ACCENT_HOVER),
                "link.active.foreground": h(INFO),
                "list.background": h(PANEL),
                "list.hover.background": h(PANEL_HOVER),
                "list.active.background": ha(ACCENT, 0.15),
                "list.active.border": h(ACCENT),
                "list.even.background": h(PANEL),
                "list.head.background": h(PANEL),
                "scrollbar.background": "#00000000",
                "scrollbar.thumb.background": h(BORDER_STRONG),
                "scrollbar.thumb.hover.background": mixed(TEXT_FAINT, BORDER_STRONG, 0.5),
                "sidebar.background": h(PANEL),
                "sidebar.border": h(BORDER),
                "sidebar.foreground": h(TEXT),
                "sidebar.accent.background": h(PANEL_HOVER),
                "sidebar.accent.foreground": h(TEXT),
                "tab.background": "#00000000",
                "tab.active.background": h(BG),
                "tab.active.foreground": h(TEXT),
                "tab.foreground": h(TEXT_DIM),
                "tab_bar.background": h(PANEL),
                "title_bar.background": h(BG),
                "title_bar.border": h(BORDER),
                "status_bar.background": h(PANEL),
                "status_bar.border": h(BORDER),
                "window.border": h(BORDER),
                "overlay": "#00000066",
                "base.red": h(DANGER),
                "base.green": h(OK),
                "base.blue": h(INFO),
                "base.yellow": h(WARN),
                "base.magenta": h(PURPLE),
                "base.cyan": h(SYN_NAMESPACE)
            },
            "highlight": {
                "editor.foreground": h(TEXT),
                "editor.background": h(BG),
                "editor.active_line.background": h(PANEL),
                "editor.line_number": mixed(TEXT_FAINT, BG, 0.6),
                "editor.active_line_number": h(TEXT_DIM),
                "editor.invisible": ha(TEXT_FAINT, 0.4),
                "conflict": h(WARN),
                "created": h(OK),
                "created.background": ha(OK, 0.12),
                "deleted": h(DANGER),
                "deleted.background": ha(DANGER, 0.12),
                "modified": h(WARN),
                "modified.background": ha(WARN, 0.12),
                "error.background": ha(DANGER, 0.12),
                "error.border": h(DANGER),
                "warning.background": ha(WARN, 0.12),
                "warning.border": h(WARN),
                "info.background": ha(INFO, 0.12),
                "info.border": h(INFO),
                "hint": h(TEXT_DIM),
                "hint.background": ha(TEXT_DIM, 0.12),
                "hint.border": h(TEXT_FAINT),
                "success.background": ha(OK, 0.12),
                "predictive": h(TEXT_FAINT),
                "hidden": h(TEXT_FAINT),
                "syntax": {
                    "attribute": s(SYN_PROPERTY),
                    "boolean": s(SYN_CONSTANT),
                    "comment": { "color": h(SYN_COMMENT), "font_style": "italic" },
                    "comment.doc": { "color": h(SYN_COMMENT), "font_style": "italic" },
                    "constant": s(SYN_CONSTANT),
                    "constructor": s(SYN_FUNCTION),
                    "embedded": s(TEXT),
                    "emphasis": { "font_style": "italic" },
                    "emphasis.strong": { "font_weight": 700 },
                    "function": s(SYN_FUNCTION),
                    "keyword": s(SYN_KEYWORD),
                    "label": s(TEXT_DIM),
                    "link_text": { "color": h(INFO), "font_style": "normal" },
                    "link_uri": { "color": h(INFO), "font_style": "italic" },
                    "namespace": s(SYN_NAMESPACE),
                    "number": s(SYN_NUMBER),
                    "operator": s(TEXT_DIM),
                    "preproc": s(SYN_NUMBER),
                    "property": s(SYN_PROPERTY),
                    "punctuation": s(TEXT_DIM),
                    "string": s(SYN_STRING),
                    "string.escape": s(SYN_NAMESPACE),
                    "string.regex": s(SYN_NAMESPACE),
                    "string.special": s(SYN_STRING),
                    "tag": s(SYN_TYPE),
                    "text.literal": s(SYN_STRING),
                    "title": { "color": h(SYN_CONSTANT), "font_weight": 600 },
                    "type": s(SYN_TYPE),
                    "variable": s(TEXT),
                    "variable.parameter": s(SYN_PARAMETER),
                    "variable.special": s(SYN_CONSTANT)
                }
            }
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_resolve_per_theme() {
        let nord = &THEMES[find("nord").unwrap()];
        assert_eq!(nord.hex(PANEL), 0x292e39);
        assert_eq!(nord.hex(ANSI[8]), 0x4c566a);
        // color-mix(in srgb, #81a1c1 40%, #eceff4) ≈ rgb(193, 208, 224).
        assert_eq!(nord.hex(NAV_ACTIVE), 0xc1d0e0);
        assert_eq!(nord.tokens()[(ACCENT_SOFT & 0xff) as usize], 0x88c0d026);
        assert_eq!(find("Tokyo Night"), find("tokyo-night"));
        // Literal colours pass through.
        assert_eq!(c(0x123456), rgb(0x123456));
    }

    #[test]
    fn every_theme_makes_a_component_theme() {
        for t in &THEMES {
            let v = component_theme(t);
            assert_eq!(v["themes"][0]["name"], t.label);
            assert!(v["themes"][0]["colors"]["background"].as_str().unwrap().starts_with('#'));
        }
    }

    #[test]
    fn reads_the_tauri_choice() {
        let mut db = b"\x00\x13orbit.settings-section".to_vec();
        db.extend(b"orbit.theme");
        db.extend("nord".encode_utf16().flat_map(|u| u.to_le_bytes()));
        db.extend(b"orbit.session");
        assert_eq!(stored_theme(&db).as_deref(), Some("nord"));
        assert_eq!(stored_theme(b"orbit.themenothing"), None);
    }
}

