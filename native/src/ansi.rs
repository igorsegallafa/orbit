//! ANSI escape codes in log text (build output, the debug console): SGR
//! colours and attributes become styled spans; every other escape sequence
//! and control character is dropped. The style carries over from one line to
//! the next, as in a terminal.
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// 0-15 are the theme's ANSI colours, 16-255 xterm's table.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

/// A line without its escapes, and the styles of its parts in order (they
/// cover the whole text).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub spans: Vec<Span>,
}

impl Line {
    fn push(&mut self, s: &str, style: Style) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        match self.spans.last_mut() {
            Some(last) if last.style == style && last.range.end == start => last.range.end = self.text.len(),
            _ => self.spans.push(Span { range: start..self.text.len(), style }),
        }
    }

    fn pop_char(&mut self) {
        if self.text.pop().is_none() {
            return;
        }
        let end = self.text.len();
        while let Some(last) = self.spans.last_mut() {
            last.range.end = last.range.end.min(end);
            if last.range.is_empty() {
                self.spans.pop();
            } else {
                break;
            }
        }
    }
}

const TAB: usize = 8;

/// Parses one line (no `\n`), starting from `style` and leaving it as the
/// line ends. A `\r` before the end starts the line over, as a progress bar
/// redrawing itself shows its last state.
pub fn parse(input: &str, style: &mut Style) -> Line {
    let mut line = Line::default();
    let mut chars = input.char_indices().peekable();
    let mut plain = String::new();
    let flush = |line: &mut Line, plain: &mut String, style: Style| {
        line.push(plain, style);
        plain.clear();
    };
    while let Some((_, ch)) = chars.next() {
        match ch {
            '\x1b' | '\u{9b}' => {
                flush(&mut line, &mut plain, *style);
                let csi = ch == '\u{9b}' || chars.next_if(|(_, c)| *c == '[').is_some();
                if csi {
                    let mut params = String::new();
                    let mut fin = None;
                    for (_, c) in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            fin = Some(c);
                            break;
                        }
                        params.push(c);
                    }
                    if fin == Some('m') && params.chars().all(|c| c.is_ascii_digit() || c == ';' || c == ':') {
                        sgr(&params, style);
                    }
                } else if chars.next_if(|(_, c)| *c == ']' || *c == 'P' || *c == '_' || *c == '^' || *c == 'X').is_some() {
                    // OSC / DCS / APC / PM / SOS: up to BEL or ST.
                    while let Some((_, c)) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if(|(_, c)| *c == '\\').is_some()) {
                            break;
                        }
                    }
                } else {
                    // ESC, intermediates (0x20-0x2f), then one final byte.
                    while chars.next_if(|(_, c)| ('\x20'..='\x2f').contains(c)).is_some() {}
                    chars.next();
                }
            }
            '\t' => {
                let col = line.text.chars().count() + plain.chars().count();
                plain.extend(std::iter::repeat_n(' ', TAB - col % TAB));
            }
            '\r' => {
                flush(&mut line, &mut plain, *style);
                if chars.peek().is_some() {
                    line = Line::default();
                }
            }
            '\x08' => {
                if plain.pop().is_none() {
                    line.pop_char();
                }
            }
            c if c.is_control() => {}
            c => plain.push(c),
        }
    }
    flush(&mut line, &mut plain, *style);
    line
}

/// `ESC [ params m`.
fn sgr(params: &str, style: &mut Style) {
    // `38;5;n` and `38:5:n` (and `38:2::r:g:b`) both: flatten, remembering the colon groups.
    let groups: Vec<Vec<u16>> = params
        .split(';')
        .map(|g| g.split(':').map(|n| n.parse::<u16>().unwrap_or(0)).collect())
        .collect();
    let mut i = 0;
    while i < groups.len() {
        let g = &groups[i];
        let code = g[0];
        // An extended colour, from its colon group or from the following `;` params.
        let mut extended = || -> Option<Color> {
            let sub: Vec<u16> = if g.len() > 1 {
                g[1..].to_vec()
            } else {
                let rest: Vec<u16> = groups[i + 1..].iter().map(|g| g[0]).collect();
                let take = match rest.first() {
                    Some(5) => 2,
                    Some(2) => 4,
                    _ => 0,
                };
                i += take;
                rest.into_iter().take(take).collect()
            };
            match sub.as_slice() {
                [5, n, ..] => Some(Color::Indexed(*n as u8)),
                // `38:2::r:g:b` has a colour space id before r.
                [2, _, r, g, b] => Some(Color::Rgb(*r as u8, *g as u8, *b as u8)),
                [2, r, g, b, ..] => Some(Color::Rgb(*r as u8, *g as u8, *b as u8)),
                _ => None,
            }
        };
        match code {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            7 => style.inverse = true,
            21 | 22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.inverse = false,
            30..=37 => style.fg = Some(Color::Indexed((code - 30) as u8)),
            38 => style.fg = extended().or(style.fg),
            39 => style.fg = None,
            40..=47 => style.bg = Some(Color::Indexed((code - 40) as u8)),
            48 => style.bg = extended().or(style.bg),
            49 => style.bg = None,
            90..=97 => style.fg = Some(Color::Indexed((code - 90 + 8) as u8)),
            100..=107 => style.bg = Some(Color::Indexed((code - 100 + 8) as u8)),
            _ => {}
        }
        i += 1;
    }
}

/// xterm's colour for indices 16-255 (0-15 come from the theme).
pub fn xterm_rgb(ix: u8) -> (u8, u8, u8) {
    match ix {
        16..=231 => {
            let i = ix - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = 8 + (ix - 232) * 10;
            (v, v, v)
        }
        _ => (0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(line: &Line) -> Vec<(&str, Style)> {
        line.spans.iter().map(|s| (&line.text[s.range.clone()], s.style)).collect()
    }

    #[test]
    fn plain_text_is_one_default_span() {
        let mut st = Style::default();
        let l = parse("hello world", &mut st);
        assert_eq!(l.text, "hello world");
        assert_eq!(styled(&l), vec![("hello world", Style::default())]);
    }

    #[test]
    fn sixteen_colours_bold_and_reset() {
        let mut st = Style::default();
        let l = parse("\x1b[32mINFO\x1b[0m ok \x1b[1;31mERROR\x1b[0m \x1b[93mbright", &mut st);
        assert_eq!(l.text, "INFO ok ERROR bright");
        let green = Style { fg: Some(Color::Indexed(2)), ..Default::default() };
        let bold_red = Style { fg: Some(Color::Indexed(1)), bold: true, ..Default::default() };
        let bright_yellow = Style { fg: Some(Color::Indexed(11)), ..Default::default() };
        assert_eq!(
            styled(&l),
            vec![("INFO", green), (" ok ", Style::default()), ("ERROR", bold_red), (" ", Style::default()), ("bright", bright_yellow)]
        );
        // The style is still on at the end of the line, for the next one.
        assert_eq!(st, bright_yellow);
        let next = parse("more\x1b[39m!", &mut st);
        assert_eq!(styled(&next), vec![("more", bright_yellow), ("!", Style::default())]);
    }

    #[test]
    fn indexed_and_truecolor_in_both_notations() {
        let mut st = Style::default();
        let l = parse("\x1b[38;5;208ma\x1b[48;2;1;2;3mb\x1b[38:2::10:20:30mc\x1b[38:5:99md\x1b[0m", &mut st);
        assert_eq!(l.text, "abcd");
        let s: Vec<Style> = l.spans.iter().map(|s| s.style).collect();
        assert_eq!(s[0].fg, Some(Color::Indexed(208)));
        assert_eq!(s[1].bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(s[1].fg, Some(Color::Indexed(208)));
        assert_eq!(s[2].fg, Some(Color::Rgb(10, 20, 30)));
        assert_eq!(s[3].fg, Some(Color::Indexed(99)));
        assert_eq!(st, Style::default());
        // `38;2;r;g;b` followed by more params keeps going after the colour.
        let l = parse("\x1b[38;2;9;8;7;1mx", &mut st);
        assert_eq!(l.spans[0].style, Style { fg: Some(Color::Rgb(9, 8, 7)), bold: true, ..Default::default() });
    }

    #[test]
    fn other_escapes_and_controls_are_dropped() {
        let mut st = Style::default();
        let l = parse("\x1b]0;title\x07a\x1b[2Kb\x1b[?25lc\x1b(Bd\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\\x00e", &mut st);
        assert_eq!(l.text, "abcdlinke");
        assert_eq!(st, Style::default());
    }

    #[test]
    fn tabs_carriage_returns_and_backspaces() {
        let mut st = Style::default();
        assert_eq!(parse("a\tb", &mut st).text, "a       b");
        assert_eq!(parse("10%\r50%\r100%", &mut st).text, "100%");
        assert_eq!(parse("done\r", &mut st).text, "done");
        assert_eq!(parse("abc\x08d", &mut st).text, "abd");
    }

    #[test]
    fn a_truncated_sequence_does_not_eat_the_line() {
        let mut st = Style::default();
        assert_eq!(parse("ok\x1b[3", &mut st).text, "ok");
        assert_eq!(parse("\x1b", &mut st).text, "");
    }

    #[test]
    fn xterm_table() {
        assert_eq!(xterm_rgb(16), (0, 0, 0));
        assert_eq!(xterm_rgb(208), (255, 135, 0));
        assert_eq!(xterm_rgb(255), (238, 238, 238));
    }
}
