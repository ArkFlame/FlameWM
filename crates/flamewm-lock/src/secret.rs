/// Typed password entry. The text never leaves this type except through
/// [`PasswordBuffer::as_str`] for the PAM call, and is wiped on clear and drop.
#[derive(Default)]
pub struct PasswordBuffer {
    text: String,
}

impl PasswordBuffer {
    pub fn push(&mut self, character: char) {
        if !character.is_control() {
            self.text.push(character);
        }
    }

    /// Removes the last character; a no-op when empty.
    pub fn backspace(&mut self) {
        self.text.pop();
    }

    pub fn clear(&mut self) {
        wipe(&mut self.text);
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// One mask dot per entered character; the secret itself is never rendered.
    pub fn masked(&self) -> String {
        "\u{2022}".repeat(self.len())
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl Drop for PasswordBuffer {
    fn drop(&mut self) {
        self.clear();
    }
}

fn wipe(text: &mut String) {
    // Overwrite the bytes in place before releasing the allocation.
    let mut bytes = std::mem::take(text).into_bytes();
    bytes.iter_mut().for_each(|byte| *byte = 0);
    std::hint::black_box(&bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_one_dot_per_character() {
        let mut buffer = PasswordBuffer::default();
        buffer.push('a');
        buffer.push('\u{e9}');
        buffer.push('3');
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.masked(), "\u{2022}\u{2022}\u{2022}");
        assert!(!buffer.masked().contains('a'));
    }

    #[test]
    fn backspace_and_clear_and_control_chars() {
        let mut buffer = PasswordBuffer::default();
        buffer.push('x');
        buffer.push('\n');
        assert_eq!(buffer.as_str(), "x");
        buffer.backspace();
        buffer.backspace();
        assert!(buffer.is_empty());
        buffer.push('y');
        buffer.clear();
        assert_eq!(buffer.masked(), "");
    }
}
