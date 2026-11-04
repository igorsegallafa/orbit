//! TODO(theme): stands in for `theme::list()` / `theme::set(name)` until the
//! theme port lands. The palettes are `src/lib/theme.ts`'s, for the previews.
use std::sync::Mutex;

/// The colours a theme card's preview draws (`ThemePreview` in `AppearanceSection.tsx`).
#[derive(Clone, Copy, Debug)]
pub struct Preview {
    pub bg: u32,
    pub panel: u32,
    pub panel_raised: u32,
    pub border: u32,
    pub text: u32,
    pub text_dim: u32,
    pub text_faint: u32,
    pub accent: u32,
    pub ok: u32,
    pub warn: u32,
    pub danger: u32,
    pub purple: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ThemeInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub dark: bool,
    pub preview: Preview,
}

const fn p(c: [u32; 12]) -> Preview {
    Preview {
        bg: c[0],
        panel: c[1],
        panel_raised: c[2],
        border: c[3],
        text: c[4],
        text_dim: c[5],
        text_faint: c[6],
        accent: c[7],
        ok: c[8],
        warn: c[9],
        danger: c[10],
        purple: c[11],
    }
}

// bg, panel, panelRaised, border, text, textDim, textFaint, accent, ok, warn, danger, purple
const THEMES: &[ThemeInfo] = &[
    ThemeInfo {
        id: "orbit-dark",
        label: "Orbit Dark",
        dark: true,
        preview: p([0x0d0f13, 0x14171d, 0x1a1e26, 0x232833, 0xe6e8ec, 0x9aa1ad, 0x6b7280, 0x4f7cf7, 0x4ade80, 0xe3b341, 0xff5d5d, 0xb48cff]),
    },
    ThemeInfo {
        id: "dracula",
        label: "Dracula",
        dark: true,
        preview: p([0x282a36, 0x21222c, 0x2f3141, 0x343746, 0xf8f8f2, 0xbfc2d4, 0x6272a4, 0xbd93f9, 0x50fa7b, 0xf1fa8c, 0xff5555, 0xff79c6]),
    },
    ThemeInfo {
        id: "tokyo-night",
        label: "Tokyo Night",
        dark: true,
        preview: p([0x1a1b26, 0x16161e, 0x1f2335, 0x232433, 0xc0caf5, 0xa9b1d6, 0x565f89, 0x7aa2f7, 0x9ece6a, 0xe0af68, 0xf7768e, 0xbb9af7]),
    },
    ThemeInfo {
        id: "catppuccin-mocha",
        label: "Catppuccin Mocha",
        dark: true,
        preview: p([0x1e1e2e, 0x181825, 0x24273a, 0x313244, 0xcdd6f4, 0xa6adc8, 0x6c7086, 0x89b4fa, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0xcba6f7]),
    },
    ThemeInfo {
        id: "nord",
        label: "Nord",
        dark: true,
        preview: p([0x2e3440, 0x292e39, 0x343a47, 0x3b4252, 0xeceff4, 0xc0c8d6, 0x7b88a1, 0x88c0d0, 0xa3be8c, 0xebcb8b, 0xbf616a, 0xb48ead]),
    },
    ThemeInfo {
        id: "orbit-light",
        label: "Orbit Light",
        dark: false,
        preview: p([0xf6f7f9, 0xffffff, 0xf0f2f5, 0xe1e4ea, 0x1d2129, 0x555d6b, 0x8a919e, 0x3b6ef5, 0x1f9d55, 0xb7791f, 0xd83b3b, 0x7c4dda]),
    },
    ThemeInfo {
        id: "solarized-light",
        label: "Solarized Light",
        dark: false,
        preview: p([0xfdf6e3, 0xeee8d5, 0xf5efdc, 0xddd6c1, 0x073642, 0x586e75, 0x93a1a1, 0x268bd2, 0x859900, 0xb58900, 0xdc322f, 0x6c71c4]),
    },
];

/// The themes, dark ones first (as Appearance lists them).
pub fn list() -> &'static [ThemeInfo] {
    THEMES
}

static CURRENT: Mutex<Option<String>> = Mutex::new(None);

/// The chosen theme id, or `system`.
pub fn current() -> String {
    CURRENT.lock().unwrap().clone().unwrap_or_else(|| "orbit-dark".into())
}

pub fn set(id: &str) {
    *CURRENT.lock().unwrap() = Some(id.to_string());
}

/// What `system` resolves to: the OS is dark here unless told otherwise.
pub fn system() -> &'static ThemeInfo {
    &THEMES[0]
}

