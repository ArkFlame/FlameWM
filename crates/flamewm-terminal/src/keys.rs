//! Key events to the bytes a terminal child expects. Pure: the caller supplies the current cursor-key mode.
use flamewm_ui_x11::UiKeyEvent;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a String>) -> Self {
        let mut mods = Self::default();
        for name in names {
            match name.to_ascii_lowercase().as_str() {
                "shift" => mods.shift = true,
                "alt" | "mod1" => mods.alt = true,
                "ctrl" | "control" => mods.ctrl = true,
                _ => {}
            }
        }
        mods
    }

    /// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    fn xterm_param(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }
}

/// Shortcuts the terminal handles itself before any byte reaches the child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Copy,
    Paste,
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    Settings,
    FontLarger,
    FontSmaller,
}

pub fn shortcut(key: &str, mods: Mods) -> Option<Shortcut> {
    let name = key.to_ascii_lowercase();
    match (mods.ctrl, mods.shift, name.as_str()) {
        (true, true, "c") => Some(Shortcut::Copy),
        (true, true, "v") => Some(Shortcut::Paste),
        (true, true, "t") => Some(Shortcut::NewTab),
        (true, true, "w") => Some(Shortcut::CloseTab),
        (true, false, "tab") => Some(Shortcut::NextTab),
        (true, true, "tab") | (true, true, "iso_left_tab") => Some(Shortcut::PreviousTab),
        (true, false, ",") => Some(Shortcut::Settings),
        (true, false, "=") | (true, false, "+") => Some(Shortcut::FontLarger),
        (true, false, "-") => Some(Shortcut::FontSmaller),
        _ => None,
    }
}

fn named_key(name: &str) -> Option<&'static str> {
    Some(match name {
        "return" | "enter" | "kp_enter" => "return",
        "escape" | "esc" => "escape",
        "backspace" => "backspace",
        "tab" => "tab",
        "iso_left_tab" => "tab",
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        "home" => "home",
        "end" => "end",
        "insert" => "insert",
        "delete" => "delete",
        "prior" | "page_up" | "pageup" => "prior",
        "next" | "page_down" | "pagedown" => "next",
        "f1" => "f1",
        "f2" => "f2",
        "f3" => "f3",
        "f4" => "f4",
        "f5" => "f5",
        "f6" => "f6",
        "f7" => "f7",
        "f8" => "f8",
        "f9" => "f9",
        "f10" => "f10",
        "f11" => "f11",
        "f12" => "f12",
        _ => return None,
    })
}

fn csi_key(name: &str) -> Option<(&'static str, char)> {
    // (CSI parameter, final byte) for the keys that use tilde or letter encodings.
    Some(match name {
        "up" => ("", 'A'),
        "down" => ("", 'B'),
        "right" => ("", 'C'),
        "left" => ("", 'D'),
        "home" => ("", 'H'),
        "end" => ("", 'F'),
        "insert" => ("2", '~'),
        "delete" => ("3", '~'),
        "prior" => ("5", '~'),
        "next" => ("6", '~'),
        "f5" => ("15", '~'),
        "f6" => ("17", '~'),
        "f7" => ("18", '~'),
        "f8" => ("19", '~'),
        "f9" => ("20", '~'),
        "f10" => ("21", '~'),
        "f11" => ("23", '~'),
        "f12" => ("24", '~'),
        _ => return None,
    })
}

/// Bytes for one key press. `app_cursor` is DECCKM, which switches arrows and Home/End to SS3 form.
pub fn encode(event: &UiKeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = Mods::from_names(event.modifiers.iter());
    let name = event.key.to_ascii_lowercase();
    if let Some(named) = named_key(&name) {
        return Some(encode_named(named, mods, app_cursor));
    }
    // Printable text (layouts deliver the composed character here).
    if let Some(text) = event.text.as_deref().filter(|t| !t.is_empty() && !t.chars().any(char::is_control)) {
        let mut bytes = Vec::new();
        if mods.alt {
            bytes.push(0x1b);
        }
        bytes.extend_from_slice(text.as_bytes());
        return Some(bytes);
    }
    if mods.ctrl {
        let mut chars = name.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else { return None };
        let code = match c {
            'a'..='z' => c as u8 - b'a' + 1,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '/' | '7' => 0x1f,
            _ => return None,
        };
        return Some(if mods.alt { vec![0x1b, code] } else { vec![code] });
    }
    None
}

fn encode_named(name: &str, mods: Mods, app_cursor: bool) -> Vec<u8> {
    let param = mods.xterm_param();
    let modified = param != 1;
    match name {
        "return" => vec![b'\r'],
        "tab" => {
            if mods.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }
        }
        "escape" => vec![0x1b],
        "backspace" => if mods.alt { vec![0x1b, 0x7f] } else { vec![0x7f] },
        "f1" | "f2" | "f3" | "f4" => {
            let letter = match name { "f1" => 'P', "f2" => 'Q', "f3" => 'R', _ => 'S' };
            if modified { format!("\x1b[1;{param}{letter}").into_bytes() } else { format!("\x1bO{letter}").into_bytes() }
        }
        other => {
            let Some((number, final_byte)) = csi_key(other) else { return Vec::new() };
            let arrowlike = matches!(final_byte, 'A' | 'B' | 'C' | 'D' | 'H' | 'F');
            let sequence = if modified {
                let number = if number.is_empty() { "1" } else { number };
                format!("\x1b[{number};{param}{final_byte}")
            } else if arrowlike && app_cursor {
                format!("\x1bO{final_byte}")
            } else if number.is_empty() {
                format!("\x1b[{final_byte}")
            } else {
                format!("\x1b[{number}{final_byte}")
            };
            sequence.into_bytes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, modifiers: &[&str], text: Option<&str>) -> UiKeyEvent {
        UiKeyEvent {
            phase: flamewm_ui_x11::UiKeyPhase::Press,
            modifiers: modifiers.iter().map(|m| (*m).to_owned()).collect(),
            key: name.to_owned(),
            text: text.map(str::to_owned),
        }
    }

    #[test]
    fn printable_text_and_return_and_backspace() {
        assert_eq!(encode(&key("a", &[], Some("a")), false), Some(b"a".to_vec()));
        assert_eq!(encode(&key("return", &[], Some("\r")), false), Some(b"\r".to_vec()));
        assert_eq!(encode(&key("backspace", &[], None), false), Some(vec![0x7f]));
        assert_eq!(encode(&key("e", &["alt"], Some("é")), false), Some("\x1bé".as_bytes().to_vec()));
    }

    #[test]
    fn ctrl_letters_become_c0_codes() {
        assert_eq!(encode(&key("c", &["ctrl"], None), false), Some(vec![0x03]));
        assert_eq!(encode(&key("d", &["ctrl"], None), false), Some(vec![0x04]));
        assert_eq!(encode(&key("[", &["ctrl"], None), false), Some(vec![0x1b]));
        assert_eq!(encode(&key("x", &["alt", "ctrl"], None), false), Some(vec![0x1b, 0x18]));
    }

    #[test]
    fn arrows_follow_decckm_and_modifiers() {
        assert_eq!(encode(&key("up", &[], None), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(&key("up", &[], None), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(encode(&key("right", &["ctrl"], None), true), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(encode(&key("left", &["shift"], None), false), Some(b"\x1b[1;2D".to_vec()));
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(encode(&key("delete", &[], None), false), Some(b"\x1b[3~".to_vec()));
        assert_eq!(encode(&key("page_up", &[], None), false), Some(b"\x1b[5~".to_vec()));
        assert_eq!(encode(&key("f1", &[], None), false), Some(b"\x1bOP".to_vec()));
        assert_eq!(encode(&key("f5", &[], None), false), Some(b"\x1b[15~".to_vec()));
        assert_eq!(encode(&key("tab", &["shift"], None), false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(encode(&key("home", &[], None), true), Some(b"\x1bOH".to_vec()));
    }

    #[test]
    fn shortcuts_are_claimed_before_encoding() {
        let ctrl_shift = Mods { shift: true, ctrl: true, alt: false };
        assert_eq!(shortcut("c", ctrl_shift), Some(Shortcut::Copy));
        assert_eq!(shortcut("v", ctrl_shift), Some(Shortcut::Paste));
        assert_eq!(shortcut("t", ctrl_shift), Some(Shortcut::NewTab));
        assert_eq!(shortcut(",", Mods { ctrl: true, ..Mods::default() }), Some(Shortcut::Settings));
        assert_eq!(shortcut("c", Mods { ctrl: true, ..Mods::default() }), None, "plain Ctrl+C reaches the child");
    }
}
