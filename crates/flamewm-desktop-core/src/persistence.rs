use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use flamewm_api::{ErrorCode, FlameError, FlameResult, OutputId};

use crate::layout::Cell;
use crate::model::StoredPosition;

pub struct LayoutStore;

impl LayoutStore {
    #[must_use]
    pub fn serialize(positions: &BTreeMap<PathBuf, StoredPosition>) -> String {
        positions
            .iter()
            .map(|(path, position)| {
                format!(
                    "{}\t{}\t{}\t{}",
                    hex(path.as_os_str().as_bytes()),
                    hex(position.output.as_str().as_bytes()),
                    position.cell.column,
                    position.cell.row
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    pub fn parse(text: &str) -> FlameResult<BTreeMap<PathBuf, StoredPosition>> {
        let mut positions = BTreeMap::new();
        for (line_number, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 4 {
                return Err(FlameError::new(
                    ErrorCode::InvalidArgument,
                    format!("bad desktop layout line {}", line_number + 1),
                ));
            }
            let decode = |value| unhex(value, "hex field has odd length", "invalid hex field");
            let path = PathBuf::from(OsString::from_vec(decode(fields[0])?));
            let output = String::from_utf8(decode(fields[1])?).map_err(|_| {
                FlameError::new(ErrorCode::InvalidArgument, "desktop output id is not UTF-8")
            })?;
            let column = fields[2]
                .parse::<i32>()
                .map_err(|_| FlameError::invalid("bad desktop layout column"))?;
            let row = fields[3]
                .parse::<i32>()
                .map_err(|_| FlameError::invalid("bad desktop layout row"))?;
            if output.is_empty() || column < 0 || row < 0 {
                return Err(FlameError::invalid("desktop layout position is invalid"));
            }
            positions.insert(
                path,
                StoredPosition {
                    output: OutputId::new(output),
                    cell: Cell::new(column, row),
                },
            );
        }
        Ok(positions)
    }

    pub fn persist(path: &Path, positions: &BTreeMap<PathBuf, StoredPosition>) -> FlameResult<()> {
        super::atomic_file::write_text(path, "desktop layout", &Self::serialize(positions))
    }
}

#[must_use]
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn unhex(
    value: &str,
    odd_length_error: &str,
    invalid_digit_error: &str,
) -> FlameResult<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(FlameError::invalid(odd_length_error));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = nibble(pair[0]).ok_or_else(|| FlameError::invalid(invalid_digit_error))?;
            let low = nibble(pair[1]).ok_or_else(|| FlameError::invalid(invalid_digit_error))?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips_path_output_and_cell() {
        let positions = BTreeMap::from([(
            PathBuf::from("/home/juan/Desktop/My File"),
            StoredPosition {
                output: OutputId::new("eDP-1"),
                cell: Cell::new(3, 4),
            },
        )]);
        let parsed = LayoutStore::parse(&LayoutStore::serialize(&positions)).expect("round trip");
        assert_eq!(parsed, positions);
    }

    #[test]
    fn layout_serialization_keeps_lowercase_hex_bytes() {
        let positions = BTreeMap::from([(
            PathBuf::from("/a"),
            StoredPosition {
                output: OutputId::new("B"),
                cell: Cell::new(1, 2),
            },
        )]);
        assert_eq!(LayoutStore::serialize(&positions), "2f61\t42\t1\t2\n");
    }

    #[test]
    fn layout_hex_errors_keep_their_existing_messages() {
        assert_eq!(
            unhex("0", "hex field has odd length", "invalid hex field").unwrap_err(),
            FlameError::invalid("hex field has odd length")
        );
        assert_eq!(
            unhex("gg", "hex field has odd length", "invalid hex field").unwrap_err(),
            FlameError::invalid("invalid hex field")
        );
        assert_eq!(
            unhex("AF", "odd", "invalid").expect("uppercase hex"),
            vec![0xaf]
        );
    }
}
