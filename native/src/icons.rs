//! Icons: Lucide for the UI (gpui-component's `IconName`, from
//! gpui-kit-assets) and Orbit's own file-type glyphs, ported from
//! `src/components/FileIcons.tsx` with the same shapes and colours.
use gpui::{div, prelude::*, px, svg, AnyElement, AssetSource, FontWeight, Result, SharedString};
use std::borrow::Cow;

/// gpui-kit-assets plus the file-type SVGs under `filetypes/`.
pub struct Assets(gpui_kit_assets::Assets);

impl Assets {
    pub fn new() -> Self {
        Self(gpui_kit_assets::Assets::new(""))
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path.strip_prefix("filetypes/").and_then(|p| p.strip_suffix(".svg")) {
            return Ok(glyph(name).map(|body| Cow::Owned(svg_doc(body).into_bytes())));
        }
        if let Some(doc) = path.strip_prefix("wsui/").and_then(|p| p.strip_suffix(".svg")).and_then(crate::workspace_ui::icons::load) {
            return Ok(Some(Cow::Owned(doc.into_bytes())));
        }
        if let Some(name) = path.strip_prefix("orbit/").and_then(|p| p.strip_suffix(".svg")) {
            return Ok(orbit_icons::ICONS.iter().find(|(n, _)| *n == name).map(|(_, doc)| Cow::Borrowed(doc.as_bytes())));
        }
        self.0.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = self.0.list(path)?;
        all.extend(GLYPHS.iter().map(|(n, _)| SharedString::from(format!("filetypes/{n}.svg"))).filter(|p| p.starts_with(path)));
        all.extend(orbit_icons::ICONS.iter().map(|(n, _)| SharedString::from(format!("orbit/{n}.svg"))).filter(|p| p.starts_with(path)));
        Ok(all)
    }
}

#[path = "orbit_icons.rs"]
mod orbit_icons;

/// An icon of `src/components/Icons.tsx` by its kebab-case name (`satellite`,
/// `home`, `git`, `settings`, `plug`, `repo`, `bell`, `panel-left`, `chevron-right`…).
pub fn orbit_icon(name: &'static str, size: f32, color: impl Into<gpui::Hsla>) -> gpui::Svg {
    svg().path(format!("orbit/{name}.svg")).size(px(size)).flex_none().text_color(color)
}

fn svg_doc(body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    )
}

const OUTLINE: &str = r#"<path d="M6 3h8l4 4v14H6z"/>"#;
const CODE_BOX: &str = r#"<path d="M4 4h16v14H4z"/><path d="m9 9-2 3 2 3"/><path d="m14 9 2 3-2 3"/>"#;
const YAML: &str = r#"<path d="M4 4h16v14H4z"/><path d="M8 9v6"/><path d="M12 9v6"/><path d="M16 9v6"/>"#;

/// Glyph bodies by name (FileIcons.tsx `FILE_ICONS`; `outline` holds the labeled ones' C/C+/H).
const GLYPHS: &[(&str, &str)] = &[
    ("outline", OUTLINE),
    ("file", r#"<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/>"#),
    ("cmake", r#"<path d="M12 3 3 20h18z"/><path d="M12 3v17"/>"#),
    ("ts", CODE_BOX),
    ("js", r#"<path d="M4 4h16v14H4z"/><path d="M9 15c-.6.6-1.4.8-2.2.6"/><path d="M15 9.5c-1.8-.8-3.6.5-3.6 2.2 0 1.9 1.9 3 3.6 2.2"/>"#),
    ("json", r#"<path d="M8 4H6a2 2 0 0 0-2 2v3a2 2 0 0 1-2 2 2 2 0 0 1 2 2v3a2 2 0 0 0 2 2h2"/><path d="M16 4h2a2 2 0 0 1 2 2v3a2 2 0 0 0 2 2 2 2 0 0 0-2 2v3a2 2 0 0 1-2 2h-2"/>"#),
    ("md", r#"<path d="M4 4h16v14H4z"/><path d="M8 10l2 3 2-3"/><path d="M15 10v6"/>"#),
    ("rs", r#"<circle cx="12" cy="12" r="7"/>"#),
    ("go", r#"<circle cx="12" cy="12" r="7"/><path d="M9 10h2.5l1 3 1-3H16"/>"#),
    ("py", r#"<path d="M9 4h8v4H9z"/><path d="M7 8h10v12H7z"/><circle cx="13" cy="14" r="1.2"/>"#),
    ("css", r#"<path d="M5 4h14l-1.5 14L12 20l-5.5-2z"/><path d="M9 8l.7 7"/><path d="m12.4 8-.4 7 3.4-1"/>"#),
    ("html", r#"<path d="M5 4h14l-1.5 14L12 20l-5.5-2z"/><path d="M9 8l.5 7L12 16l2.5-1"/>"#),
    ("yaml", YAML),
    ("sql", r#"<ellipse cx="12" cy="6" rx="7" ry="2.5"/><path d="M5 6v12c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5V6"/>"#),
    ("lock", r#"<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>"#),
    ("sh", r#"<path d="m8 9-3 3 3 3"/><path d="M13 15h6"/>"#),
    ("env", r#"<path d="M5 5h14v14H5z"/><path d="M9 10h.01M15 10h.01M9 14h.01M15 14h.01"/>"#),
    ("toml", r#"<path d="M4 5h16v14H4z"/><path d="M4 9h16M9 5v14"/>"#),
    ("folder", r#"<path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#),
    ("folder-open", r#"<path d="M3 6a1 1 0 0 1 1-1h4l2 2h10a1 1 0 0 1 1 1v2H3z"/>"#),
    // Tab icons from Icons.tsx (DocIcon, TerminalIcon, DiffIcon).
    ("doc", r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z"/><path d="M14 2v5h6"/>"#),
    ("terminal", r#"<path d="m5 8 4 4-4 4"/><path d="M13 16h6"/>"#),
    // "↺" of a recent session (Lucide rotate-ccw).
    ("rotate-ccw", r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"#),
    // The status indicator's arc: an 11px ring's coloured top border (`.status-work`).
    ("arc-top", r#"<path d="M5.07 5.07A9.8 9.8 0 0 1 18.93 5.07" stroke-width="4.36" stroke-linecap="butt"/>"#),
    ("diff", r#"<path d="M12 3v18" stroke-dasharray="2 2"/><path d="M5 8h6M8 5v6"/><path d="M19 16h-6"/>"#),
    // Build/Run/Debug (Lucide: hammer, play, bug, square, loader-circle, pause, redo-dot, arrow-down-to-dot, arrow-up-from-dot).
    ("hammer", r#"<path d="m15 12-9.373 9.373a1 1 0 0 1-3.001-3L12 9"/><path d="m18 15 4-4"/><path d="m21.5 11.5-1.914-1.914A2 2 0 0 1 19 8.172v-.344a2 2 0 0 0-.586-1.414l-1.657-1.657A6 6 0 0 0 12.516 3H9l1.243 1.243A6 6 0 0 1 12 8.485V10l2 2h1.172a2 2 0 0 1 1.414.586L18.5 14.5"/>"#),
    ("play", r#"<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>"#),
    ("bug", r#"<path d="M12 20v-9"/><path d="M14 7a4 4 0 0 1 4 4v3a6 6 0 0 1-12 0v-3a4 4 0 0 1 4-4z"/><path d="M14.12 3.88 16 2"/><path d="M21 21a4 4 0 0 0-3.81-4"/><path d="M21 5a4 4 0 0 1-3.55 3.97"/><path d="M22 13h-4"/><path d="M3 21a4 4 0 0 1 3.81-4"/><path d="M3 5a4 4 0 0 0 3.55 3.97"/><path d="M6 13H2"/><path d="m8 2 1.88 1.88"/><path d="M9 7.13V6a3 3 0 1 1 6 0v1.13"/>"#),
    ("stop", r#"<rect width="14" height="14" x="5" y="5" rx="2"/>"#),
    ("spinner", r#"<path d="M21 12a9 9 0 1 1-6.219-8.56"/>"#),
    ("pause", r#"<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>"#),
    ("step-over", r#"<circle cx="12" cy="17" r="1"/><path d="M21 7v6h-6"/><path d="M3 17a9 9 0 0 1 9-9 9 9 0 0 1 6 2.3l3 2.7"/>"#),
    ("step-into", r#"<path d="M12 2v14"/><path d="m19 9-7 7-7-7"/><circle cx="12" cy="21" r="1"/>"#),
    ("step-out", r#"<path d="m5 9 7-7 7 7"/><path d="M12 16V2"/><circle cx="12" cy="21" r="1"/>"#),
];

/// One of Orbit's own UI glyphs (`doc`, `terminal`, `diff`), tinted by the text colour.
/// GPUI's `svg()` doesn't inherit the text colour, so it is passed in.
pub fn ui_icon(name: &'static str, size: f32, color: impl Into<gpui::Hsla>) -> gpui::Svg {
    svg().path(format!("filetypes/{name}.svg")).size(px(size)).flex_none().text_color(color)
}

fn glyph(name: &str) -> Option<&'static str> {
    GLYPHS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// How a file name is drawn: a glyph, or the page outline with a short label.
#[derive(Debug, PartialEq)]
pub enum FileGlyph {
    Glyph(&'static str),
    Label(&'static str),
}

pub const FOLDER_COLOR: u32 = 0xdcb67a;
pub const DEFAULT_COLOR: u32 = 0x8a919e;

/// Glyph and colour for a file name (FileIcons.tsx `FILE_ICONS` / `ICON_COLORS`).
pub fn file_glyph(name: &str) -> (FileGlyph, u32) {
    let ext = if name == "CMakeLists.txt" {
        "cmake".to_string()
    } else {
        name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
    };
    use FileGlyph::*;
    match ext.as_str() {
        "c" => (Label("C"), 0xa8b9cc),
        "cpp" | "cc" | "cxx" => (Label("C+"), 0xf34b7d),
        "h" | "hpp" | "hh" | "hxx" | "inl" => (Label("H"), 0xa074c4),
        "cmake" => (Glyph("cmake"), 0x3fb950),
        "ts" | "tsx" => (Glyph("ts"), 0x519aba),
        "js" => (Glyph("js"), 0xf1e05a),
        "json" => (Glyph("json"), 0xcbcb41),
        "md" => (Glyph("md"), 0x519aba),
        "rs" => (Glyph("rs"), 0xdea584),
        "go" => (Glyph("go"), 0x00add8),
        "py" => (Glyph("py"), 0x3572a5),
        "css" => (Glyph("css"), 0x563d7c),
        "html" => (Glyph("html"), 0xe34c26),
        "yaml" | "yml" => (Glyph("yaml"), 0xa074c4),
        "sql" => (Glyph("sql"), 0xe38c00),
        "lock" => (Glyph("lock"), 0x8b8b8b),
        "sh" => (Glyph("sh"), 0x89e051),
        "env" => (Glyph("env"), 0xe0d050),
        "toml" => (Glyph("toml"), 0x9c4221),
        _ => (Glyph("file"), DEFAULT_COLOR),
    }
}

fn glyph_svg(name: &str, size: f32, color: u32) -> gpui::Svg {
    svg().path(format!("filetypes/{name}.svg")).size(px(size)).flex_none().text_color(gpui::rgb(color))
}

/// The tree's icon for a file.
pub fn file_icon(name: &str, size: f32) -> AnyElement {
    match file_glyph(name) {
        (FileGlyph::Glyph(g), color) => glyph_svg(g, size, color).into_any_element(),
        (FileGlyph::Label(text), color) => div()
            .relative()
            .flex_none()
            .size(px(size))
            .child(glyph_svg("outline", size, color))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(size * 0.42))
                    .flex()
                    .justify_center()
                    .text_size(px(if text.len() > 1 { size * 0.36 } else { size * 0.46 }))
                    .line_height(px(size * 0.4))
                    .font_weight(FontWeight::BOLD)
                    .text_color(gpui::rgb(color))
                    .child(text),
            )
            .into_any_element(),
    }
}

/// The tree's icon for a folder.
pub fn folder_icon(open: bool, size: f32) -> AnyElement {
    glyph_svg(if open { "folder-open" } else { "folder" }, size, FOLDER_COLOR).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_names() {
        assert_eq!(file_glyph("Game.CPP"), (FileGlyph::Label("C+"), 0xf34b7d));
        assert_eq!(file_glyph("CMakeLists.txt").0, FileGlyph::Glyph("cmake"));
        assert_eq!(file_glyph("Makefile"), (FileGlyph::Glyph("file"), DEFAULT_COLOR));
        assert_eq!(file_glyph("a.yml").0, FileGlyph::Glyph("yaml"));
    }

    #[test]
    fn every_glyph_is_served() {
        let assets = Assets::new();
        for (name, _) in GLYPHS {
            let bytes = assets.load(&format!("filetypes/{name}.svg")).unwrap().unwrap();
            assert!(std::str::from_utf8(&bytes).unwrap().starts_with("<svg"));
        }
        assert!(assets.load("icons/panel-left.svg").unwrap().is_some());
    }
}
