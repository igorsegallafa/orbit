//! Keystrokes and mouse events as the bytes a terminal application expects.
use alacritty_terminal::term::TermMode;
use gpui::{Keystroke, Modifiers, MouseButton};

pub fn key_to_bytes(ks: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = ks.modifiers;
    let modcode = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let csi = |final_: char, ss3: bool| -> Vec<u8> {
        if modcode > 1 {
            format!("\x1b[1;{modcode}{final_}").into_bytes()
        } else if ss3 {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modcode > 1 { format!("\x1b[{n};{modcode}~") } else { format!("\x1b[{n}~") }.into_bytes()
    };
    let bytes = match ks.key.as_str() {
        // ESC+CR is how Claude Code and other line editors read a newline.
        "enter" if m.shift || m.alt => b"\x1b\r".to_vec(),
        "enter" => b"\r".to_vec(),
        "backspace" if m.control => vec![0x08],
        "backspace" => vec![0x7f],
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => vec![0x1b],
        "up" => csi('A', app_cursor),
        "down" => csi('B', app_cursor),
        "right" => csi('C', app_cursor),
        "left" => csi('D', app_cursor),
        "home" => csi('H', app_cursor),
        "end" => csi('F', app_cursor),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => csi('P', true),
        "f2" => csi('Q', true),
        "f3" => csi('R', true),
        "f4" => csi('S', true),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        key if m.control && !m.platform => {
            let ch = key.chars().next()?;
            let code = match ch {
                'a'..='z' => ch as u8 - b'a' + 1,
                ' ' | '2' | '@' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '6' => 0x1e,
                '/' | '7' | '-' => 0x1f,
                _ => return None,
            };
            let mut v = if m.alt { vec![0x1b] } else { vec![] };
            v.push(code);
            v
        }
        "space" => {
            let mut v = if m.alt { vec![0x1b] } else { vec![] };
            v.push(b' ');
            v
        }
        _ => {
            let text = ks.key_char.as_ref()?;
            let mut v = if m.alt { vec![0x1b] } else { vec![] };
            v.extend_from_slice(text.as_bytes());
            v
        }
    };
    Some(bytes)
}

/// What a mouse event means to an application that asked for mouse reports.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MouseReport {
    Press(MouseButton),
    Release(MouseButton),
    /// Motion with a button held (or none, in any-motion mode).
    Motion(Option<MouseButton>),
    WheelUp,
    WheelDown,
}

/// The report for `event` at 0-based cell `(col, row)`, or None when the
/// current mode doesn't ask for it.
pub fn mouse_report(mode: TermMode, event: MouseReport, col: usize, row: usize, mods: Modifiers) -> Option<Vec<u8>> {
    let button_code = |b: MouseButton| match b {
        MouseButton::Left => Some(0u8),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    };
    let (mut code, release) = match event {
        MouseReport::Press(b) => (button_code(b)?, false),
        MouseReport::Release(b) => (button_code(b)?, true),
        MouseReport::Motion(Some(b)) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => (button_code(b)? + 32, false),
        MouseReport::Motion(None) if mode.contains(TermMode::MOUSE_MOTION) => (3 + 32, false),
        MouseReport::Motion(_) => return None,
        MouseReport::WheelUp => (64, false),
        MouseReport::WheelDown => (65, false),
    };
    code += 4 * mods.shift as u8 + 8 * mods.alt as u8 + 16 * mods.control as u8;
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    // X10/normal encoding: releases don't say which button, and coordinates stop at 222.
    if release {
        code = 3 + (code & !3);
    }
    if col > 222 || row > 222 {
        return None;
    }
    Some(vec![0x1b, b'[', b'M', 32 + code, 33 + col as u8, 33 + row as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_or_alt_enter_sends_esc_cr() {
        let ks = |k: &'static str| Keystroke::parse(k).unwrap();
        assert_eq!(key_to_bytes(&ks("enter"), false).unwrap(), b"\r");
        assert_eq!(key_to_bytes(&ks("shift-enter"), false).unwrap(), b"\x1b\r");
        assert_eq!(key_to_bytes(&ks("alt-enter"), false).unwrap(), b"\x1b\r");
    }

    #[test]
    fn sgr_reports_press_release_and_wheel() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let m = Modifiers::default();
        assert_eq!(mouse_report(mode, MouseReport::Press(MouseButton::Left), 4, 2, m).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(mouse_report(mode, MouseReport::Release(MouseButton::Left), 4, 2, m).unwrap(), b"\x1b[<0;5;3m");
        assert_eq!(mouse_report(mode, MouseReport::WheelDown, 0, 0, m).unwrap(), b"\x1b[<65;1;1M");
        // Click-only mode doesn't report drags.
        assert_eq!(mouse_report(mode, MouseReport::Motion(Some(MouseButton::Left)), 1, 1, m), None);
    }

    #[test]
    fn normal_encoding_offsets_by_32() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        let ctrl = Modifiers { control: true, ..Default::default() };
        assert_eq!(mouse_report(mode, MouseReport::Press(MouseButton::Right), 0, 0, ctrl).unwrap(), vec![0x1b, b'[', b'M', 32 + 2 + 16, 33, 33]);
        assert_eq!(mouse_report(mode, MouseReport::Release(MouseButton::Left), 0, 0, Modifiers::default()).unwrap()[3], 32 + 3);
    }
}
