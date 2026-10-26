//! Colour themes as data. A palette is background, foreground, cursor, selection and the 16 ANSI colours, each an
//! RGB value. Built-in entries are read-only; user themes live in TOML files managed by `store`.
//! The built-in values are approximations of the named editor themes' published palettes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub selection: u32,
    pub ansi: [u32; 16],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub palette: Palette,
    pub builtin: bool,
}

/// Field names in the order `palette_fields` yields them. They are also the TOML keys of a user theme.
pub const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

/// Hex for background, foreground, cursor, selection, then the 16 ANSI colours. One line per theme.
const BUILTIN: &[(&str, &str)] = &[
    (
        "Dark+",
        "1e1e1e d4d4d4 aeafad 264f78 000000 cd3131 0dbc79 e5e510 2472c8 bc3fbc 11a8cd e5e5e5 666666 f14c4c 23d18b f5f543 3b8eea d670d6 29b8db ffffff",
    ),
    (
        "Light+",
        "ffffff 000000 000000 add6ff 000000 cd3131 00bc00 949800 0451a5 bc05bc 0598bc 555555 666666 cd3131 14ce14 b5ba00 0451a5 bc05bc 0598bc a5a5a5",
    ),
    (
        "Monokai",
        "272822 f8f8f2 f8f8f0 49483e 272822 f92672 a6e22e f4bf75 66d9ef ae81ff a1efe4 f8f8f2 75715e f92672 a6e22e f4bf75 66d9ef ae81ff a1efe4 f9f8f5",
    ),
    (
        "Solarized Dark",
        "002b36 839496 93a1a1 073642 073642 dc322f 859900 b58900 268bd2 d33682 2aa198 eee8d5 002b36 cb4b16 586e75 657b83 839496 6c71c4 93a1a1 fdf6e3",
    ),
    (
        "Solarized Light",
        "fdf6e3 657b83 586e75 eee8d5 073642 dc322f 859900 b58900 268bd2 d33682 2aa198 eee8d5 002b36 cb4b16 586e75 657b83 839496 6c71c4 93a1a1 fdf6e3",
    ),
    (
        "Dracula",
        "282a36 f8f8f2 f8f8f2 44475a 21222c ff5555 50fa7b f1fa8c bd93f9 ff79c6 8be9fd f8f8f2 6272a4 ff6e6e 69ff94 ffffa5 d6acff ff92df a4ffff ffffff",
    ),
    (
        "One Dark Pro",
        "282c34 abb2bf 528bff 3e4451 282c34 e06c75 98c379 e5c07b 61afef c678dd 56b6c2 abb2bf 5c6370 e06c75 98c379 e5c07b 61afef c678dd 56b6c2 ffffff",
    ),
    (
        "Nord",
        "2e3440 d8dee9 d8dee9 434c5e 3b4252 bf616a a3be8c ebcb8b 81a1c1 b48ead 88c0d0 e5e9f0 4c566a bf616a a3be8c ebcb8b 81a1c1 b48ead 8fbcbb eceff4",
    ),
    (
        "Gruvbox Dark",
        "282828 ebdbb2 ebdbb2 504945 282828 cc241d 98971a d79921 458588 b16286 689d6a a89984 928374 fb4934 b8bb26 fabd2f 83a598 d3869b 8ec07c ebdbb2",
    ),
    (
        "Gruvbox Light",
        "fbf1c7 3c3836 3c3836 d5c4a1 fbf1c7 cc241d 98971a d79921 458588 b16286 689d6a 7c6f64 928374 9d0006 79740e b57614 076678 8f3f71 427b58 3c3836",
    ),
    (
        "Tokyo Night",
        "1a1b26 c0caf5 c0caf5 33467c 15161e f7768e 9ece6a e0af68 7aa2f7 bb9af7 7dcfff a9b1d6 414868 f7768e 9ece6a e0af68 7aa2f7 bb9af7 7dcfff c0caf5",
    ),
    (
        "Catppuccin Mocha",
        "1e1e2e cdd6f4 f5e0dc 585b70 45475a f38ba8 a6e3a1 f9e2af 89b4fa f5c2e7 94e2d5 bac2de 585b70 f38ba8 a6e3a1 f9e2af 89b4fa f5c2e7 94e2d5 a6adc8",
    ),
    (
        "Catppuccin Latte",
        "eff1f5 4c4f69 dc8a78 acb0be 5c5f77 d20f39 40a02b df8e1d 1e66f5 ea76cb 179299 acb0be 6c6f85 d20f39 40a02b df8e1d 1e66f5 ea76cb 179299 bcc0cc",
    ),
    (
        "GitHub Dark",
        "0d1117 c9d1d9 c9d1d9 264f78 484f58 ff7b72 3fb950 d29922 58a6ff bc8cff 39c5cf b1bac4 6e7681 ffa198 56d364 e3b341 79c0ff d2a8ff 56d4dd f0f6fc",
    ),
    (
        "GitHub Light",
        "ffffff 24292f 24292f b6e3ff 24292f cf222e 116329 4d2d00 0969da 8250df 1b7c83 6e7781 57606a a40e26 1a7f37 633c01 218bff a475f9 3192aa 8c959f",
    ),
    (
        "Material Palenight",
        "292d3e a6accd ffcc00 464b5d 292d3e f07178 c3e88d ffcb6b 82aaff c792ea 89ddff d0d0d0 434758 ff8b92 ddffa7 ffe585 9cc4ff e1acff a3f7ff ffffff",
    ),
    (
        "Ayu Dark",
        "0b0e14 bfbdb6 e6b450 253340 01060e ea6c73 91b362 f9af4f 53bdfa fae994 90e1c6 c7c7c7 686868 f07178 c2d94c ffb454 59c2ff ffee99 95e6cb ffffff",
    ),
    (
        "Night Owl",
        "011627 d6deeb 80a4c2 1d3b53 011627 ef5350 22da6e addb67 82aaff c792ea 21c7a8 ffffff 575656 ef5350 22da6e ffeb95 82aaff c792ea 7fdbca ffffff",
    ),
    (
        "Cobalt2",
        "193549 ffffff ffc600 1f4662 000000 ff0000 38de21 ffe50a 1478db e350ba 00bbbb bbbbbb 555555 f40e17 3bd01d edc809 5555ff ff55ff 6febff ffffff",
    ),
    (
        "Synthwave '84",
        "262335 ffffff ff7edb 3e3762 262335 f97e72 72f1b8 fede5d 36f9f6 ff7edb 03edf9 ffffff 4d4d4d f97e72 72f1b8 fede5d 36f9f6 ff7edb 03edf9 ffffff",
    ),
];

/// Parses a `#rrggbb` or `rrggbb` string.
#[must_use]
pub fn parse_hex(value: &str) -> Option<u32> {
    let digits = value.trim().strip_prefix('#').unwrap_or(value.trim());
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

#[must_use]
pub fn format_hex(value: u32) -> String {
    format!("#{:06x}", value & 0xff_ffff)
}

impl Palette {
    /// Colour slots in display order: background, foreground, cursor, selection, then the 16 ANSI names.
    #[must_use]
    pub fn slots(&self) -> Vec<(&'static str, u32)> {
        let mut slots = vec![
            ("background", self.background),
            ("foreground", self.foreground),
            ("cursor", self.cursor),
            ("selection", self.selection),
        ];
        slots.extend(ANSI_NAMES.iter().copied().zip(self.ansi.iter().copied()));
        slots
    }

    pub fn set_slot(&mut self, name: &str, value: u32) -> bool {
        match name {
            "background" => self.background = value,
            "foreground" => self.foreground = value,
            "cursor" => self.cursor = value,
            "selection" => self.selection = value,
            other => match ANSI_NAMES.iter().position(|n| *n == other) {
                Some(index) => self.ansi[index] = value,
                None => return false,
            },
        }
        true
    }

    fn from_hex_list(list: &str) -> Option<Self> {
        let values: Vec<u32> = list
            .split_whitespace()
            .map(|token| u32::from_str_radix(token, 16).ok())
            .collect::<Option<_>>()?;
        if values.len() != 20 {
            return None;
        }
        let mut ansi = [0_u32; 16];
        ansi.copy_from_slice(&values[4..]);
        Some(Self {
            background: values[0],
            foreground: values[1],
            cursor: values[2],
            selection: values[3],
            ansi,
        })
    }
}

/// The built-in themes, in picker order.
#[must_use]
pub fn builtin_themes() -> Vec<Theme> {
    BUILTIN
        .iter()
        .map(|(name, list)| Theme {
            name: (*name).to_owned(),
            palette: Palette::from_hex_list(list).expect("built-in theme table is well formed"),
            builtin: true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_twenty_builtin_themes_parse_with_full_palettes() {
        let themes = builtin_themes();
        assert_eq!(themes.len(), 20);
        let names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
        for expected in [
            "Dark+",
            "Light+",
            "Monokai",
            "Solarized Dark",
            "Solarized Light",
            "Dracula",
            "One Dark Pro",
            "Nord",
            "Gruvbox Dark",
            "Gruvbox Light",
            "Tokyo Night",
            "Catppuccin Mocha",
            "Catppuccin Latte",
            "GitHub Dark",
            "GitHub Light",
            "Material Palenight",
            "Ayu Dark",
            "Night Owl",
            "Cobalt2",
            "Synthwave '84",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
        assert!(themes.iter().all(|t| t.builtin));
    }

    #[test]
    fn hex_parsing_accepts_hash_and_rejects_garbage() {
        assert_eq!(parse_hex("#1E1E1E"), Some(0x001e_1e1e));
        assert_eq!(parse_hex("ff00aa"), Some(0x00ff_00aa));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
        assert_eq!(format_hex(0x000a_0b0c), "#0a0b0c");
    }

    #[test]
    fn slots_round_trip_through_set_slot() {
        let mut palette = builtin_themes()[0].palette.clone();
        assert_eq!(palette.slots().len(), 20);
        assert!(palette.set_slot("bright_red", 0x0012_3456));
        assert_eq!(palette.ansi[9], 0x0012_3456);
        assert!(!palette.set_slot("nope", 1));
    }
}
