//! The pages' icons: Orbit's own stroke icons from `src/components/Icons.tsx`
//! (and `BrandIcons.tsx`, `CheckBox.tsx`), served as `wsui/<name>.svg`.
use gpui::{prelude::*, px, svg, Hsla, Svg};

const STROKE: &str = r#"fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round""#;

/// Body and viewBox/attributes per icon.
const ICONS: &[(&str, &str)] = &[
    ("satellite", r#"<path d="m13 7 4-4 4 4-4 4z"/><path d="m3 17 4-4 4 4-4 4z"/><path d="m11 13 2 2"/><path d="M15 9 9 15"/>"#),
    ("plus", r#"<path d="M12 5v14"/><path d="M5 12h14"/>"#),
    ("folder", r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#),
    ("code", r#"<path d="m8 9-4 3 4 3"/><path d="m16 9 4 3-4 3"/><path d="m13 5-2 14"/>"#),
    ("spark", r#"<path d="M12 3l1.9 5.7L19.6 10.6l-5.7 1.9L12 18.2l-1.9-5.7L4.4 10.6l5.7-1.9z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>"#),
    ("commit", r#"<circle cx="12" cy="12" r="9"/><path d="m8.5 12 2.5 2.5 4.5-5"/>"#),
    ("push", r#"<path d="M12 15V4"/><path d="m7 9 5-5 5 5"/><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/>"#),
    ("pull-request", r#"<circle cx="6" cy="5" r="2.2"/><circle cx="18" cy="19" r="2.2"/><circle cx="6" cy="19" r="2.2"/><path d="M6 7.2v9.6"/><path d="M18 16.8V9a3 3 0 0 0-3-3h-3"/><path d="m13 3-3 3 3 3"/>"#),
    ("race", r#"<path d="M3 6h11M12 4l2 2-2 2"/><path d="M3 12h17M18 10l2 2-2 2"/><path d="M3 18h7M8 16l2 2-2 2"/>"#),
    ("rebase", r#"<path d="M3 8h15l-3-3"/><path d="M21 16H6l3 3"/>"#),
    ("diff", r#"<path d="M12 3v18" stroke-dasharray="2 2"/><path d="M5 8h6M8 5v6"/><path d="M19 16h-6"/>"#),
    ("doc", r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z"/><path d="M14 2v5h6"/>"#),
    ("refresh", r#"<path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/>"#),
    ("trash", r#"<path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M6 6l1 14h10l1-14"/>"#),
    ("git", r#"<line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>"#),
    ("branch", r#"<circle cx="6" cy="6" r="2.2"/><circle cx="6" cy="18" r="2.2"/><circle cx="18" cy="8" r="2.2"/><path d="M6 8.2v7.6"/><path d="M18 10.2c0 4-4 4.3-9.8 6.6"/>"#),
    ("repo", r#"<path d="M5 19.5V5a2 2 0 0 1 2-2h12v15H7a2 2 0 0 0-2 2 2 2 0 0 0 2 2h12"/><path d="M9 3v7l2-1.5L13 10V3"/>"#),
    ("terminal", r#"<path d="m5 8 4 4-4 4"/><path d="M13 16h6"/>"#),
    ("stash", r#"<rect x="3" y="4" width="18" height="5" rx="1"/><path d="M5 9v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9"/><path d="M10 13h4"/>"#),
    ("undo", r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>"#),
    ("download", r#"<path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/>"#),
    ("search", r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#),
    ("chevron-down", r#"<path d="m6 9 6 6 6-6"/>"#),
    ("chevron-right", r#"<path d="M9 6l6 6-6 6"/>"#),
    ("arrow-right", r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#),
];

/// Icons with their own viewBox or fill.
const SPECIAL: &[(&str, &str)] = &[
    ("check", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m3 8.5 3.5 3.5L13 4.5"/></svg>"#),
    ("x", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m4 4 8 8M12 4l-8 8"/></svg>"#),
    ("circle", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2"><circle cx="8" cy="8" r="5.5"/></svg>"#),
    ("tick", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1.5 5.5L4 8L8.5 2"/></svg>"#),
    ("dash", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M2 5H8"/></svg>"#),
    ("disclosure", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><path d="M8.6 4.4c-.4-.4-1-.5-1.5-.2-.5.3-.8.8-.8 1.3v13c0 .5.3 1 .8 1.3.5.3 1.1.2 1.5-.2l8-6.5c.35-.3.55-.75.55-1.2s-.2-.9-.55-1.2z"/></svg>"#),
    ("spinner", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-opacity="0.25" stroke-width="2.5"/><path d="M12 3a9 9 0 0 1 9 9" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/></svg>"#),
    ("github", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor"><path d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z"/></svg>"#),
];

/// The SVG document for `name`, if it is one of ours.
pub fn load(name: &str) -> Option<String> {
    if let Some((_, doc)) = SPECIAL.iter().find(|(n, _)| *n == name) {
        return Some((*doc).to_string());
    }
    let (_, body) = ICONS.iter().find(|(n, _)| *n == name)?;
    Some(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" {STROKE}>{body}</svg>"#))
}

/// An icon at `size` px in `color` (GPUI's svg doesn't inherit the text colour).
pub fn icon(name: &'static str, size: f32, color: impl Into<Hsla>) -> Svg {
    svg().path(format!("wsui/{name}.svg")).size(px(size)).flex_none().text_color(color)
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_is_svg() {
        for (n, _) in super::ICONS.iter().chain(super::SPECIAL) {
            assert!(super::load(n).unwrap().starts_with("<svg"), "{n}");
        }
    }
}
