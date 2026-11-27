//! Keyboard input for the terminal: GPUI keystrokes to the bytes a shell
//! expects (xterm encoding), and pasted text.
//!
//! Kept free of views so it can be tested on its own. The terminal view
//! handles its own shortcuts (copy, paste, scrolling, jumping between
//! commands) before falling back to [`encode_key`].

use ferro_services::TerminalModes;
use gpui_kit::{Keystroke, Modifiers};

/// The xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(m: &Modifiers) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control)
}

fn has_modifiers(m: &Modifiers) -> bool {
    m.shift || m.alt || m.control
}

/// The bytes for `keystroke`, or `None` when the terminal has no use for
/// it (e.g. a bare modifier, or a platform shortcut).
pub fn encode_key(keystroke: &Keystroke, modes: TerminalModes) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    if m.platform || m.function {
        return None;
    }
    let key = keystroke.key.as_str();
    let csi = |final_byte: char| -> Vec<u8> {
        if has_modifiers(m) {
            format!("\x1b[1;{}{final_byte}", modifier_param(m)).into_bytes()
        } else if modes.app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if has_modifiers(m) {
            format!("\x1b[{code};{}~", modifier_param(m)).into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };
    let ss3 = |final_byte: char| -> Vec<u8> {
        if has_modifiers(m) {
            format!("\x1b[1;{}{final_byte}", modifier_param(m)).into_bytes()
        } else {
            format!("\x1bO{final_byte}").into_bytes()
        }
    };
    let bytes = match key {
        "enter" => {
            if m.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        "backspace" => {
            let base: &[u8] = if m.control { b"\x08" } else { b"\x7f" };
            if m.alt {
                [b"\x1b", base].concat()
            } else {
                base.to_vec()
            }
        }
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => csi('A'),
        "down" => csi('B'),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if m.control => vec![0],
        _ => return encode_char(keystroke),
    };
    Some(bytes)
}

/// A printable key, with Ctrl turning letters into control codes and Alt
/// prefixing ESC (meta).
fn encode_char(keystroke: &Keystroke) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    // AltGr arrives as Ctrl+Alt with the composed character (Polish ą, ł…).
    if m.control
        && m.alt
        && let Some(text) = keystroke.key_char.as_deref()
        && !text.is_empty()
    {
        return Some(text.as_bytes().to_vec());
    }
    if m.control {
        let key = keystroke.key.as_str();
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return None;
        };
        let code = match c.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            '@' | '2' | ' ' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' => 0x1f,
            '/' => 0x1f,
            '8' | '?' => 0x7f,
            _ => return None,
        };
        return Some(if m.alt { vec![0x1b, code] } else { vec![code] });
    }
    let text = keystroke
        .key_char
        .clone()
        .or_else(|| (keystroke.key == "space").then(|| " ".to_string()))
        .or_else(|| (keystroke.key.chars().count() == 1).then(|| keystroke.key.clone()))?;
    if text.is_empty() {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() + 1);
    if m.alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

/// Pasted text as the shell should receive it: line breaks become carriage
/// returns, and bracketed paste wraps it so shells do not run it line by
/// line.
pub fn encode_paste(text: &str, modes: TerminalModes) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste {
        // A pasted end marker would end the paste early.
        let clean = normalized.replace("\x1b[201~", "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        normalized.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(spec: &'static str) -> Keystroke {
        Keystroke::parse(spec).map_err(|e| e.to_string()).unwrap()
    }

    fn typed(spec: &'static str, ch: &str) -> Keystroke {
        let mut k = key(spec);
        k.key_char = Some(ch.into());
        k
    }

    fn enc(k: &Keystroke) -> Vec<u8> {
        encode_key(k, TerminalModes::default()).unwrap()
    }

    #[test]
    fn printable_and_control_keys() {
        assert_eq!(enc(&typed("a", "a")), b"a");
        assert_eq!(enc(&typed("shift-a", "A")), b"A");
        assert_eq!(enc(&key("ctrl-c")), [3]);
        assert_eq!(enc(&key("ctrl-d")), [4]);
        assert_eq!(enc(&key("ctrl-[")), [0x1b]);
        assert_eq!(enc(&key("ctrl-space")), [0]);
        assert_eq!(enc(&typed("alt-b", "b")), b"\x1bb");
        assert_eq!(enc(&typed("ctrl-alt-a", "ą")), "ą".as_bytes());
        assert_eq!(enc(&key("enter")), b"\r");
        assert_eq!(enc(&key("backspace")), b"\x7f");
        assert_eq!(enc(&key("shift-tab")), b"\x1b[Z");
        assert_eq!(enc(&key("space")), b" ");
        assert!(encode_key(&key("cmd-c"), TerminalModes::default()).is_none());
    }

    #[test]
    fn cursor_keys_follow_the_mode_and_modifiers() {
        assert_eq!(enc(&key("up")), b"\x1b[A");
        let app = TerminalModes {
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(encode_key(&key("up"), app).unwrap(), b"\x1bOA");
        assert_eq!(enc(&key("ctrl-left")), b"\x1b[1;5D");
        assert_eq!(enc(&key("shift-end")), b"\x1b[1;2F");
        assert_eq!(enc(&key("delete")), b"\x1b[3~");
        assert_eq!(enc(&key("ctrl-delete")), b"\x1b[3;5~");
        assert_eq!(enc(&key("f1")), b"\x1bOP");
        assert_eq!(enc(&key("f12")), b"\x1b[24~");
    }

    #[test]
    fn paste_uses_carriage_returns_and_brackets() {
        let plain = TerminalModes::default();
        assert_eq!(encode_paste("a\r\nb\nc", plain), b"a\rb\rc");
        let bracketed = TerminalModes {
            bracketed_paste: true,
            ..Default::default()
        };
        assert_eq!(
            encode_paste("ls\x1b[201~", bracketed),
            b"\x1b[200~ls\x1b[201~"
        );
    }
}
