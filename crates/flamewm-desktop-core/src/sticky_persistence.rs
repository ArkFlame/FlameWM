use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_api::{ErrorCode, FlameError, FlameResult, OutputId, Rect};

use crate::sticky::{MAX_TEXT_SIZE, MIN_TEXT_SIZE, Rgb, StickyNote, StickyNoteStore};

pub const STATE_FILE_NAME: &str = "sticky-notes.state";

pub fn state_path() -> FlameResult<PathBuf> {
    let base = match env::var_os("XDG_STATE_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(env::var_os("HOME").ok_or_else(|| invalid("HOME is not set"))?)
            .join(".local/state"),
    };
    Ok(base.join(STATE_FILE_NAME))
}

pub fn load(path: &Path) -> FlameResult<StickyNoteStore> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StickyNoteStore::default());
        }
        Err(error) => return Err(io_error("read sticky notes", error)),
    };
    parse(&text)
}

pub fn load_from_xdg() -> FlameResult<StickyNoteStore> {
    load(&state_path()?)
}

pub fn persist(path: &Path, store: &StickyNoteStore) -> FlameResult<()> {
    super::atomic_file::write_text(path, "sticky state", &serialize(store))
}

pub fn persist_to_xdg(store: &StickyNoteStore) -> FlameResult<()> {
    persist(&state_path()?, store)
}

pub fn serialize(store: &StickyNoteStore) -> String {
    let mut output = format!("v2\t{}\n", if store.enabled() { 1 } else { 0 });
    for note in store.notes() {
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            hex(note.id.as_bytes()),
            note.workspace,
            hex(note.output.as_str().as_bytes()),
            note.rect.x,
            note.rect.y,
            note.rect.width,
            note.rect.height,
            hex(note.text.as_bytes()),
            note.background.red,
            note.background.green,
            note.background.blue,
            note.foreground.red,
            note.foreground.green,
            note.foreground.blue,
            note.text_size,
            if note.bold { 1 } else { 0 }
        ));
    }
    output
}

pub fn parse(text: &str) -> FlameResult<StickyNoteStore> {
    let mut lines = text.lines();
    let header = lines.next().ok_or_else(|| invalid("empty sticky state"))?;
    let (version, enabled) = match header {
        "v1\t0" => (1_u8, false),
        "v1\t1" => (1_u8, true),
        "v2\t0" => (2_u8, false),
        "v2\t1" => (2_u8, true),
        _ => return Err(invalid("invalid sticky state header")),
    };
    let mut notes = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let expected = if version == 1 { 15 } else { 16 };
        if fields.len() != expected {
            return Err(invalid("invalid sticky state note"));
        }
        let bold = if version == 1 {
            false
        } else {
            match fields[15] {
                "0" => false,
                "1" => true,
                _ => return Err(invalid("invalid sticky state bold flag")),
            }
        };
        let note = StickyNote {
            id: string(&fields[0])?,
            workspace: number(&fields[1])?,
            output: OutputId::new(string(&fields[2])?),
            rect: Rect::new(
                number(&fields[3])?,
                number(&fields[4])?,
                number(&fields[5])?,
                number(&fields[6])?,
            ),
            text: string(&fields[7])?,
            background: Rgb::new(byte(&fields[8])?, byte(&fields[9])?, byte(&fields[10])?),
            foreground: Rgb::new(byte(&fields[11])?, byte(&fields[12])?, byte(&fields[13])?),
            text_size: number(&fields[14])?,
            bold,
        };
        if note.id.is_empty()
            || note.output.as_str().is_empty()
            || !note.rect.is_valid()
            || !(MIN_TEXT_SIZE..=MAX_TEXT_SIZE).contains(&note.text_size)
            || notes
                .iter()
                .any(|existing: &StickyNote| existing.id == note.id)
        {
            return Err(invalid("invalid sticky state note values"));
        }
        notes.push(note);
    }
    Ok(StickyNoteStore::from_persisted(enabled, notes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}
fn string(value: &str) -> FlameResult<String> {
    String::from_utf8(unhex(value)?).map_err(|_| invalid("sticky state text is not UTF-8"))
}
fn unhex(value: &str) -> FlameResult<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(invalid("invalid sticky state hex"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair =
                std::str::from_utf8(pair).map_err(|_| invalid("invalid sticky state hex"))?;
            u8::from_str_radix(pair, 16).map_err(|_| invalid("invalid sticky state hex"))
        })
        .collect()
}
fn number<T: std::str::FromStr>(value: &str) -> FlameResult<T> {
    value
        .parse()
        .map_err(|_| invalid("invalid sticky state number"))
}
fn byte(value: &str) -> FlameResult<u8> {
    number(value)
}
fn invalid(message: &str) -> FlameError {
    FlameError::new(ErrorCode::InvalidArgument, message)
}
fn io_error(operation: &str, error: std::io::Error) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, format!("{operation}: {error}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::OutputId;

    #[test]
    fn utf8_text_survives_edit_and_reload() {
        let path = std::env::temp_dir().join(format!("sticky-notes-{}.state", std::process::id()));
        let mut store = StickyNoteStore::default();
        assert!(store.create(StickyNote::new("note", OutputId::new("eDP-1"), 0, 10, 20)));
        assert!(store.set_text("note", "Žluťoučký 🦕"));
        persist(&path, &store).expect("persist");
        let loaded = load(&path).expect("reload");
        assert_eq!(loaded.get("note").expect("note").text, "Žluťoučký 🦕");
        let _ = fs::remove_file(path);
    }
}
