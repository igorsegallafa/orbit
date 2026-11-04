//! Lays out and paints the visible grid: background runs as quads, text as one
//! shaped run per style change, positioned on the cell grid so fallback glyphs
//! can't drift the columns.
use super::graphics::{self, PlaceholderCell, PLACEHOLDER};
use super::{GridSize, Shared, LINE_HEIGHT, PADDING};
use crate::theme::{self, hsla};
use alacritty_terminal::{
    event::WindowSize,
    index::{Column, Line, Point as GridPoint},
    term::cell::Flags,
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
use gpui::{
    font, point, px, size, Bounds, FontStyle, FontWeight, Hsla, Pixels, RenderImage, ShapedLine, SharedString, TextRun, Window,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Frame statistics of every terminal canvas (layout + paint), for the self-test.
pub static FRAMES: AtomicU64 = AtomicU64::new(0);
pub static FRAME_US_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static FRAME_US_MAX: AtomicU64 = AtomicU64::new(0);

pub fn record_frame(us: u64) {
    FRAMES.fetch_add(1, Ordering::Relaxed);
    FRAME_US_TOTAL.fetch_add(us, Ordering::Relaxed);
    FRAME_US_MAX.fetch_max(us, Ordering::Relaxed);
}

/// Where the grid was last painted, to map mouse positions to cells.
#[derive(Clone, Copy)]
pub struct Geometry {
    pub origin: gpui::Point<Pixels>,
    pub cell_w: Pixels,
    pub line_h: Pixels,
    pub size: GridSize,
}

impl Geometry {
    /// The viewport cell under `pos` (column, row), clamped to the grid, and
    /// whether `pos` is on the cell's right half.
    pub fn cell_at(&self, pos: gpui::Point<Pixels>) -> (usize, usize, bool) {
        let x = f32::from(pos.x - self.origin.x) / f32::from(self.cell_w);
        let y = f32::from(pos.y - self.origin.y) / f32::from(self.line_h);
        let col = (x.max(0.) as usize).min(self.size.cols - 1);
        let row = (y.max(0.) as usize).min(self.size.rows - 1);
        (col, row, x.fract() >= 0.5)
    }

    pub fn cell_center(&self, col: usize, row: usize) -> gpui::Point<Pixels> {
        self.origin + point(self.cell_w * (col as f32 + 0.5), self.line_h * (row as f32 + 0.5))
    }
}

fn rgb_hsla(rgb: Rgb) -> Hsla {
    hsla(((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32)
}

/// xterm's 256-colour table beyond the 16 themed ones.
fn indexed(ix: u8) -> Hsla {
    match ix {
        0..=15 => hsla(theme::ANSI[ix as usize]),
        16..=231 => {
            let i = ix - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            rgb_hsla(Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) })
        }
        _ => {
            let v = 8 + (ix - 232) * 10;
            rgb_hsla(Rgb { r: v, g: v, b: v })
        }
    }
}

fn resolve(color: Color, colors: &alacritty_terminal::term::color::Colors, bold: bool) -> Hsla {
    match color {
        Color::Spec(rgb) => rgb_hsla(rgb),
        Color::Indexed(ix) => match colors[ix as usize] {
            Some(rgb) => rgb_hsla(rgb),
            None if bold && ix < 8 => indexed(ix + 8),
            None => indexed(ix),
        },
        Color::Named(named) => {
            if let Some(rgb) = colors[named] {
                return rgb_hsla(rgb);
            }
            let ix = named as usize;
            match named {
                NamedColor::Foreground | NamedColor::BrightForeground => hsla(theme::TEXT),
                NamedColor::DimForeground => hsla(theme::TEXT_DIM),
                NamedColor::Background => hsla(theme::BG),
                NamedColor::Cursor => hsla(theme::ACCENT),
                _ if ix < 8 && bold => hsla(theme::ANSI[ix + 8]),
                _ if ix < 16 => hsla(theme::ANSI[ix]),
                // Dim variants (DimBlack..DimWhite).
                _ => {
                    let base = ix.saturating_sub(NamedColor::DimBlack as usize).min(7);
                    let mut h = hsla(theme::ANSI[base]);
                    h.a = 0.7;
                    h
                }
            }
        }
    }
}

pub struct Frame {
    pub rects: Vec<(Bounds<Pixels>, Hsla)>,
    pub lines: Vec<(gpui::Point<Pixels>, ShapedLine)>,
    pub cursor: Option<(Bounds<Pixels>, CursorShape)>,
    pub line_height: Pixels,
    /// Kitty pictures: the cells they show in, the whole picture's bounds, the picture.
    pub images: Vec<(Bounds<Pixels>, Bounds<Pixels>, Arc<RenderImage>)>,
}

/// A row's run of placeholder cells showing consecutive columns of one picture.
struct ImageRun {
    line: i32,
    col: usize,
    count: usize,
    at: PlaceholderCell,
}

type Run = Option<(i32, usize, String, (Hsla, bool, bool))>;

pub fn layout(shared: &Shared, bounds: Bounds<Pixels>, window: &mut Window) -> Frame {
    let base_font = font(theme::MONO_FONT);
    let font_size = px(13.);
    let line_height = px(LINE_HEIGHT);
    let text_system = window.text_system().clone();
    let font_id = text_system.resolve_font(&base_font);
    let cell_w = text_system.advance(font_id, font_size, 'm').map(|s| s.width).unwrap_or(px(8.));

    let origin = bounds.origin + point(px(PADDING), px(PADDING));
    let cols = (((bounds.size.width - px(2. * PADDING)) / cell_w).floor() as usize).max(2);
    let rows = (((bounds.size.height - px(2. * PADDING)) / line_height).floor() as usize).max(1);
    let new_size = GridSize { cols, rows };
    {
        let mut size = shared.size.lock().unwrap();
        if *size != new_size {
            *size = new_size;
            shared.term.lock().resize(new_size);
            let pty_size = orbit_core::pty::Size {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: (f32::from(cell_w) * cols as f32) as u16,
                pixel_height: (LINE_HEIGHT * rows as f32) as u16,
            };
            *super::LAST_SIZE.lock().unwrap() = Some(pty_size);
            let _ = orbit_core::pty::resize(shared.pty_id, pty_size);
            *shared.window_size.lock().unwrap() = WindowSize {
                num_lines: rows as u16,
                num_cols: cols as u16,
                cell_width: f32::from(cell_w) as u16,
                cell_height: LINE_HEIGHT as u16,
            };
        }
    }
    *shared.geometry.lock().unwrap() = Some(Geometry { origin, cell_w, line_h: line_height, size: new_size });

    for image in std::mem::take(&mut shared.graphics.lock().unwrap().retired) {
        let _ = window.drop_image(image);
    }
    let term = shared.term.lock();
    let content = term.renderable_content();
    let mut runs: Vec<ImageRun> = Vec::new();
    let mut prev_placeholder: Option<(i32, usize, PlaceholderCell)> = None;
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let default_bg = hsla(theme::BG);
    let mut selection_bg = hsla(theme::ACCENT);
    selection_bg.a = 0.4;
    let cell_origin = |line: i32, col: usize| origin + point(cell_w * col as f32, line_height * (line + offset) as f32);

    let mut rects: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
    let mut lines = Vec::new();
    // Pending text run: (line, start column, text, style key).
    let mut run: Run = None;
    let flush = |run: &mut Run, lines: &mut Vec<(gpui::Point<Pixels>, ShapedLine)>| {
        if let Some((line, col, text, (color, bold, italic))) = run.take() {
            let trimmed = text.trim_end();
            if trimmed.is_empty() {
                return;
            }
            let mut f = base_font.clone();
            if bold {
                f.weight = FontWeight::BOLD;
            }
            if italic {
                f.style = FontStyle::Italic;
            }
            let text: SharedString = trimmed.to_string().into();
            let shaped = text_system.shape_line(
                text.clone(),
                font_size,
                &[TextRun { len: text.len(), font: f, color, background_color: None, underline: None, strikethrough: None }],
                None,
            );
            lines.push((cell_origin(line, col), shaped));
        }
    };

    for cell in content.display_iter {
        let GridPoint { line: Line(line), column: Column(col) } = cell.point;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let bold = cell.flags.contains(Flags::BOLD);
        let mut fg = resolve(cell.fg, content.colors, bold);
        let mut bg = resolve(cell.bg, content.colors, false);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::DIM) {
            fg.a *= 0.66;
        }
        if selection.is_some_and(|s| s.contains(cell.point)) {
            bg = selection_bg;
        }
        let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        if bg != default_bg {
            let b = Bounds::new(cell_origin(line, col), size(cell_w * width as f32, line_height));
            match rects.last_mut() {
                Some((last, color)) if *color == bg && last.top() == b.top() && last.right() == b.left() => {
                    last.size.width += b.size.width;
                }
                _ => rects.push((b, bg)),
            }
        }
        if cell.c == PLACEHOLDER {
            let prev = prev_placeholder.filter(|(l, c, _)| *l == line && c + 1 == col).map(|p| p.2);
            let at = graphics::read_placeholder(cell.zerowidth().unwrap_or(&[]), graphics::colour_id(cell.fg), prev);
            prev_placeholder = Some((line, col, at));
            match runs.last_mut() {
                Some(r) if r.line == line && r.col + r.count == col && r.at.id == at.id && r.at.row == at.row && r.at.col + r.count as u32 == at.col => {
                    r.count += 1
                }
                _ => runs.push(ImageRun { line, col, count: 1, at }),
            }
            flush(&mut run, &mut lines);
            continue;
        }
        let ch = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
        let style = (fg, bold, cell.flags.contains(Flags::ITALIC));
        let simple = ch.is_ascii() && width == 1;
        let continues = matches!(&run, Some((l, start, text, s)) if *l == line && *s == style && start + text.len() == col);
        if !(simple && continues) || ch == ' ' && run.is_none() {
            flush(&mut run, &mut lines);
        }
        if ch == ' ' && run.is_none() {
            continue;
        }
        let mut text = String::new();
        text.push(ch);
        if let Some(zw) = cell.zerowidth() {
            text.extend(zw);
        }
        match &mut run {
            Some((_, _, t, _)) if simple && continues => t.push_str(&text),
            _ => {
                run = Some((line, col, text, style));
                // Non-ASCII and wide glyphs stand alone so they can't shift later cells.
                if !simple {
                    flush(&mut run, &mut lines);
                }
            }
        }
    }
    flush(&mut run, &mut lines);

    let cursor = (content.cursor.shape != CursorShape::Hidden).then(|| {
        let p = content.cursor.point;
        (Bounds::new(cell_origin(p.line.0, p.column.0), size(cell_w, line_height)), content.cursor.shape)
    });
    let mut images = Vec::new();
    if !runs.is_empty() {
        let store = shared.graphics.lock().unwrap();
        let cell = (f32::from(cell_w), LINE_HEIGHT);
        for r in runs {
            let Some((image, (w, h), (cols, rows))) = store.lookup(r.at.id, cell) else { continue };
            // Fit in the placement's box, centred, as the React port does.
            let (box_w, box_h) = (cols as f32 * cell.0, rows as f32 * cell.1);
            let scale = (box_w / w).min(box_h / h);
            let (iw, ih) = (w * scale, h * scale);
            let cells = cell_origin(r.line, r.col);
            let top_left = cells - point(px(r.at.col as f32 * cell.0), px(r.at.row as f32 * cell.1));
            let image_bounds = Bounds::new(top_left + point(px((box_w - iw) / 2.), px((box_h - ih) / 2.)), size(px(iw), px(ih)));
            let clip = Bounds::new(cells, size(cell_w * r.count as f32, line_height));
            images.push((clip, image_bounds, image));
        }
    }
    Frame { rects, lines, cursor, line_height, images }
}
