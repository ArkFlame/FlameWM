//! Byte-level VT/xterm state machine (ground, escape, CSI, OSC, ignored DCS/SOS/PM/APC strings, UTF-8 text).
//! The parser keeps its state across `advance` calls, so sequences split across PTY reads decode correctly.
//! Parameters are groups separated by `;`; `:` subparameters stay inside their group (for `38:2::r:g:b`).

/// Receives decoded input. The screen implements this; tests use a recording implementation.
pub trait Perform {
    fn print(&mut self, c: char);
    fn execute(&mut self, byte: u8);
    /// `prefix` is the private marker (`?`, `>`, `<`, `=`), `params` the `;` groups, `action` the final byte.
    fn csi(&mut self, prefix: Option<u8>, params: &[Vec<u16>], intermediates: &[u8], action: u8);
    fn esc(&mut self, intermediates: &[u8], final_byte: u8);
    fn osc(&mut self, segments: &[Vec<u8>]);
}

const OSC_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    OscEscape,
    StringIgnore,
    StringIgnoreEscape,
}

#[derive(Debug)]
pub struct Parser {
    state: State,
    params: Vec<Vec<u16>>,
    in_number: bool,
    intermediates: Vec<u8>,
    prefix: Option<u8>,
    osc: Vec<Vec<u8>>,
    utf8: Vec<u8>,
    utf8_need: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            params: vec![Vec::new()],
            in_number: false,
            intermediates: Vec::new(),
            prefix: None,
            osc: Vec::new(),
            utf8: Vec::with_capacity(4),
            utf8_need: 0,
        }
    }

    pub fn advance(&mut self, bytes: &[u8], performer: &mut impl Perform) {
        for &byte in bytes {
            self.byte(byte, performer);
        }
    }

    fn clear_csi(&mut self) {
        self.params.clear();
        self.params.push(Vec::new());
        self.in_number = false;
        self.intermediates.clear();
        self.prefix = None;
    }

    fn finish_value(&mut self) {
        if !self.in_number {
            if let Some(group) = self.params.last_mut() {
                group.push(0);
            }
        }
        self.in_number = false;
    }

    fn digit(&mut self, value: u8) {
        if !self.in_number {
            if let Some(group) = self.params.last_mut() {
                group.push(0);
            }
            self.in_number = true;
        }
        if let Some(last) = self.params.last_mut().and_then(|group| group.last_mut()) {
            *last = last.saturating_mul(10).saturating_add(u16::from(value));
        }
    }

    fn flush_utf8(&mut self, performer: &mut impl Perform) {
        if self.utf8_need > 0 {
            performer.print(char::REPLACEMENT_CHARACTER);
            self.utf8.clear();
            self.utf8_need = 0;
        }
    }

    fn byte(&mut self, byte: u8, performer: &mut impl Perform) {
        if self.utf8_need > 0 && !(0x80..=0xbf).contains(&byte) {
            self.flush_utf8(performer);
        }
        match self.state {
            State::OscString => match byte {
                0x07 => self.finish_osc(performer, State::Ground),
                0x1b => self.state = State::OscEscape,
                0x18 | 0x1a => self.state = State::Ground,
                b';' => {
                    if self.osc.len() < OSC_LIMIT {
                        self.osc.push(Vec::new());
                    }
                }
                0x00..=0x1f | 0x7f => {}
                _ => {
                    if self.osc.is_empty() {
                        self.osc.push(Vec::new());
                    }
                    if let Some(last) = self.osc.last_mut() {
                        if last.len() < OSC_LIMIT {
                            last.push(byte);
                        }
                    }
                }
            },
            State::OscEscape => {
                if byte == b'\\' {
                    self.finish_osc(performer, State::Ground);
                } else {
                    self.finish_osc(performer, State::Escape);
                    self.byte(byte, performer);
                }
            }
            State::StringIgnore => match byte {
                0x1b => self.state = State::StringIgnoreEscape,
                0x18 | 0x1a => self.state = State::Ground,
                _ => {}
            },
            State::StringIgnoreEscape => {
                if byte == b'\\' {
                    self.state = State::Ground;
                } else {
                    self.state = State::Escape;
                    self.byte(byte, performer);
                }
            }
            _ => {
                if matches!(byte, 0x18 | 0x1a) {
                    self.state = State::Ground;
                    return;
                }
                if byte == 0x1b {
                    self.clear_csi();
                    self.state = State::Escape;
                    return;
                }
                match self.state {
                    State::Ground => self.ground(byte, performer),
                    State::Escape => self.escape(byte, performer),
                    State::EscapeIntermediate => self.escape_intermediate(byte, performer),
                    State::CsiEntry => self.csi_entry(byte, performer),
                    State::CsiParam => self.csi_param(byte, performer),
                    State::CsiIntermediate => self.csi_intermediate(byte, performer),
                    State::CsiIgnore => self.csi_ignore(byte, performer),
                    State::OscString
                    | State::OscEscape
                    | State::StringIgnore
                    | State::StringIgnoreEscape => {}
                }
            }
        }
    }

    fn finish_osc(&mut self, performer: &mut impl Perform, next: State) {
        let segments = std::mem::take(&mut self.osc);
        if !segments.is_empty() {
            performer.osc(&segments);
        }
        self.state = next;
    }

    fn ground(&mut self, byte: u8, performer: &mut impl Perform) {
        match byte {
            0x00..=0x1f => performer.execute(byte),
            0x20..=0x7e => performer.print(char::from(byte)),
            0x7f => {}
            0x80..=0xbf => {
                if self.utf8_need == 0 {
                    performer.print(char::REPLACEMENT_CHARACTER);
                } else {
                    self.utf8.push(byte);
                    if self.utf8.len() == self.utf8_need {
                        let decoded = std::str::from_utf8(&self.utf8)
                            .ok()
                            .and_then(|text| text.chars().next());
                        performer.print(decoded.unwrap_or(char::REPLACEMENT_CHARACTER));
                        self.utf8.clear();
                        self.utf8_need = 0;
                    }
                }
            }
            0xc0..=0xf7 => {
                self.utf8.clear();
                self.utf8.push(byte);
                self.utf8_need = match byte {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
            }
            _ => performer.print(char::REPLACEMENT_CHARACTER),
        }
    }

    fn escape(&mut self, byte: u8, performer: &mut impl Perform) {
        match byte {
            0x00..=0x1f => performer.execute(byte),
            0x20..=0x2f => {
                self.intermediates.push(byte);
                self.state = State::EscapeIntermediate;
            }
            b'[' => {
                self.clear_csi();
                self.state = State::CsiEntry;
            }
            b']' => {
                self.osc.clear();
                self.state = State::OscString;
            }
            b'P' | b'X' | b'^' | b'_' => self.state = State::StringIgnore,
            0x30..=0x7e => {
                performer.esc(&[], byte);
                self.state = State::Ground;
            }
            _ => {}
        }
    }

    fn escape_intermediate(&mut self, byte: u8, performer: &mut impl Perform) {
        match byte {
            0x00..=0x1f => performer.execute(byte),
            0x20..=0x2f => self.intermediates.push(byte),
            0x30..=0x7e => {
                let intermediates = std::mem::take(&mut self.intermediates);
                performer.esc(&intermediates, byte);
                self.state = State::Ground;
            }
            _ => {}
        }
    }

    fn csi_entry(&mut self, byte: u8, performer: &mut impl Perform) {
        match byte {
            0x00..=0x1f => performer.execute(byte),
            0x30..=0x39 => {
                self.digit(byte - b'0');
                self.state = State::CsiParam;
            }
            b';' | b':' => {
                self.separator(byte);
                self.state = State::CsiParam;
            }
            0x3c..=0x3f => {
                self.prefix = Some(byte);
                self.state = State::CsiParam;
            }
            0x20..=0x2f => {
                self.intermediates.push(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => self.dispatch_csi(byte, performer),
            _ => {}
        }
    }

    fn separator(&mut self, byte: u8) {
        self.finish_value();
        if byte == b';' {
            self.params.push(Vec::new());
        }
    }

    fn csi_param(&mut self, byte: u8, performer: &mut impl Perform) {
        match byte {
            0x00..=0x1f => performer.execute(byte),
            0x30..=0x39 => self.digit(byte - b'0'),
            b';' | b':' => self.separator(byte),
            0x3c..=0x3f => self.state = State::CsiIgnore,
            0x20..=0x2f => {
                self.finish_value();
                self.intermediates.push(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => self.dispatch_csi(byte, performer),
            _ => {}
        }
    }

    fn csi_intermediate(&mut self, byte: u8, performer: &mut impl Perform) {
        match byte {
            0x00..=0x1f => performer.execute(byte),
            0x20..=0x2f => self.intermediates.push(byte),
            0x30..=0x3f => self.state = State::CsiIgnore,
            0x40..=0x7e => self.dispatch_csi(byte, performer),
            _ => {}
        }
    }

    fn csi_ignore(&mut self, byte: u8, performer: &mut impl Perform) {
        match byte {
            0x00..=0x1f => performer.execute(byte),
            0x40..=0x7e => self.state = State::Ground,
            _ => {}
        }
    }

    fn dispatch_csi(&mut self, action: u8, performer: &mut impl Perform) {
        // An omitted final value is a default (0), but a value already closed by an intermediate is kept as is.
        if !self.in_number && self.params.last().is_none_or(Vec::is_empty) {
            self.finish_value();
        }
        let params = std::mem::replace(&mut self.params, vec![Vec::new()]);
        let intermediates = std::mem::take(&mut self.intermediates);
        performer.csi(self.prefix.take(), &params, &intermediates, action);
        self.in_number = false;
        self.state = State::Ground;
    }
}

#[cfg(test)]
pub(crate) mod recording {
    use super::Perform;

    pub type CsiRecord = (Option<u8>, Vec<Vec<u16>>, Vec<u8>, u8);

    #[derive(Debug, Default, PartialEq)]
    pub struct Recorder {
        pub printed: String,
        pub executed: Vec<u8>,
        pub csi: Vec<CsiRecord>,
        pub esc: Vec<(Vec<u8>, u8)>,
        pub osc: Vec<Vec<Vec<u8>>>,
    }

    impl Perform for Recorder {
        fn print(&mut self, c: char) {
            self.printed.push(c);
        }
        fn execute(&mut self, byte: u8) {
            self.executed.push(byte);
        }
        fn csi(
            &mut self,
            prefix: Option<u8>,
            params: &[Vec<u16>],
            intermediates: &[u8],
            action: u8,
        ) {
            self.csi
                .push((prefix, params.to_vec(), intermediates.to_vec(), action));
        }
        fn esc(&mut self, intermediates: &[u8], final_byte: u8) {
            self.esc.push((intermediates.to_vec(), final_byte));
        }
        fn osc(&mut self, segments: &[Vec<u8>]) {
            self.osc.push(segments.to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::recording::Recorder;
    use super::*;

    fn parse(bytes: &[u8]) -> Recorder {
        let mut parser = Parser::new();
        let mut recorder = Recorder::default();
        parser.advance(bytes, &mut recorder);
        recorder
    }

    #[test]
    fn plain_text_and_controls_are_dispatched_in_order() {
        let recorder = parse(b"ab\r\nc");
        assert_eq!(recorder.printed, "abc");
        assert_eq!(recorder.executed, vec![b'\r', b'\n']);
    }

    #[test]
    fn csi_params_split_on_semicolons_and_default_to_zero() {
        let recorder = parse(b"\x1b[12;;5H\x1b[H");
        assert_eq!(
            recorder.csi[0],
            (None, vec![vec![12], vec![0], vec![5]], vec![], b'H')
        );
        assert_eq!(recorder.csi[1], (None, vec![vec![0]], vec![], b'H'));
    }

    #[test]
    fn private_marker_and_intermediates_are_reported() {
        let recorder = parse(b"\x1b[?1049h\x1b[3 q");
        assert_eq!(
            recorder.csi[0],
            (Some(b'?'), vec![vec![1049]], vec![], b'h')
        );
        assert_eq!(recorder.csi[1], (None, vec![vec![3]], vec![b' '], b'q'));
    }

    #[test]
    fn colon_subparameters_stay_in_one_group() {
        let recorder = parse(b"\x1b[38:2::10:20:30m");
        assert_eq!(recorder.csi[0].1, vec![vec![38, 2, 0, 10, 20, 30]]);
    }

    #[test]
    fn sequences_split_across_reads_decode_the_same() {
        let mut parser = Parser::new();
        let mut recorder = Recorder::default();
        parser.advance(b"\x1b[3", &mut recorder);
        parser.advance(b"1;4", &mut recorder);
        parser.advance(b"2m", &mut recorder);
        assert_eq!(
            recorder.csi,
            vec![(None, vec![vec![31], vec![42]], vec![], b'm')]
        );
    }

    #[test]
    fn osc_ends_with_bel_or_string_terminator() {
        let bel = parse(b"\x1b]2;hello\x07");
        assert_eq!(bel.osc, vec![vec![b"2".to_vec(), b"hello".to_vec()]]);
        let st = parse(b"\x1b]0;title\x1b\\x");
        assert_eq!(st.osc, vec![vec![b"0".to_vec(), b"title".to_vec()]]);
        assert_eq!(st.printed, "x");
    }

    #[test]
    fn utf8_multibyte_is_decoded_and_broken_sequences_become_replacement() {
        let recorder = parse("héllo ☃".as_bytes());
        assert_eq!(recorder.printed, "héllo ☃");
        let broken = parse(&[b'a', 0xe2, b'b']);
        assert_eq!(broken.printed, "a\u{fffd}b");
    }

    #[test]
    fn escape_sequences_reach_esc_dispatch_with_intermediates() {
        let recorder = parse(b"\x1b7\x1b(0\x1bM");
        assert_eq!(
            recorder.esc,
            vec![(vec![], b'7'), (vec![b'('], b'0'), (vec![], b'M')]
        );
    }

    #[test]
    fn dcs_payload_is_ignored_until_terminator() {
        let recorder = parse(b"\x1bPq#0;2;0;0;0\x1b\\ok");
        assert_eq!(recorder.printed, "ok");
        assert!(recorder.csi.is_empty());
    }
}
