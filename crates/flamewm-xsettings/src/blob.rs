//! Pure XSETTINGS blob encoding and serial bookkeeping. No X11 access here.
//!
//! Wire layout follows the XSETTINGS specification: a byte-order byte, three
//! pad bytes, the manager serial, the setting count, then each setting as a
//! type byte, pad byte, name length, padded name, last-change serial and a
//! padded integer or string value. Multi-byte fields use the byte order named
//! by the first byte; FlameWM always publishes little-endian (`l`).

use std::collections::BTreeMap;

use flamewm_api::{FlameError, FlameResult};

use crate::XSettingsValue;

pub const BYTE_ORDER_LITTLE: u8 = b'l';
pub const SETTING_TYPE_INTEGER: u8 = 0;
pub const SETTING_TYPE_STRING: u8 = 1;

/// One published setting with the manager serial at which it last changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedSetting {
    pub value: XSettingsValue,
    pub last_change_serial: u32,
}

/// The settings a manager currently publishes, plus its serial.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublishedState {
    pub serial: u32,
    pub settings: BTreeMap<String, PublishedSetting>,
}

/// Compute the next published state for `desired`.
///
/// Returns `None` when `desired` matches the current settings exactly (no
/// serial bump, nothing to re-publish). Otherwise the serial advances by one,
/// changed or added settings get the new serial, unchanged settings keep their
/// previous last-change serial, and settings absent from `desired` are dropped.
#[must_use]
pub fn next_publication(
    previous: &PublishedState,
    desired: &BTreeMap<String, XSettingsValue>,
) -> Option<PublishedState> {
    let unchanged = desired.len() == previous.settings.len()
        && desired.iter().all(|(name, value)| {
            previous
                .settings
                .get(name)
                .is_some_and(|setting| &setting.value == value)
        });
    if unchanged {
        return None;
    }
    let serial = previous.serial.wrapping_add(1);
    let settings = desired
        .iter()
        .map(|(name, value)| {
            let last_change_serial = match previous.settings.get(name) {
                Some(setting) if &setting.value == value => setting.last_change_serial,
                _ => serial,
            };
            (
                name.clone(),
                PublishedSetting {
                    value: value.clone(),
                    last_change_serial,
                },
            )
        })
        .collect();
    Some(PublishedState { serial, settings })
}

/// Encode a published state as a little-endian XSETTINGS blob.
pub fn encode_blob(state: &PublishedState) -> FlameResult<Vec<u8>> {
    let count = u32::try_from(state.settings.len())
        .map_err(|_| FlameError::unavailable("too many XSettings entries"))?;
    let mut out = Vec::new();
    out.push(BYTE_ORDER_LITTLE);
    out.extend_from_slice(&[0, 0, 0]);
    push_u32(&mut out, state.serial);
    push_u32(&mut out, count);
    for (name, setting) in &state.settings {
        let name_len = u16::try_from(name.len())
            .map_err(|_| FlameError::unavailable("XSettings name too long"))?;
        let type_byte = match setting.value {
            XSettingsValue::Integer(_) => SETTING_TYPE_INTEGER,
            XSettingsValue::Text(_) => SETTING_TYPE_STRING,
        };
        out.push(type_byte);
        out.push(0);
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        pad4(&mut out, name.len());
        push_u32(&mut out, setting.last_change_serial);
        match &setting.value {
            XSettingsValue::Integer(value) => out.extend_from_slice(&value.to_le_bytes()),
            XSettingsValue::Text(text) => {
                let text_len = u32::try_from(text.len())
                    .map_err(|_| FlameError::unavailable("XSettings string too long"))?;
                push_u32(&mut out, text_len);
                out.extend_from_slice(text.as_bytes());
                pad4(&mut out, text.len());
            }
        }
    }
    Ok(out)
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn pad4(out: &mut Vec<u8>, len: usize) {
    let padding = (4 - (len % 4)) % 4;
    out.resize(out.len() + padding, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desired(entries: &[(&str, XSettingsValue)]) -> BTreeMap<String, XSettingsValue> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn blob_matches_byte_exact_integer_vector() {
        let state = PublishedState {
            serial: 1,
            settings: BTreeMap::from([(
                "Gtk/PreferDarkTheme".to_owned(),
                PublishedSetting {
                    value: XSettingsValue::Integer(1),
                    last_change_serial: 1,
                },
            )]),
        };
        let mut expected = vec![
            b'l', 0, 0, 0, // byte order + pad
            1, 0, 0, 0, // serial
            1, 0, 0, 0, // setting count
            0, 0, // type integer, pad
            19, 0, // name length "Gtk/PreferDarkTheme"
        ];
        expected.extend_from_slice(b"Gtk/PreferDarkTheme");
        expected.push(0); // name pad to 20
        expected.extend_from_slice(&[1, 0, 0, 0]); // last-change serial
        expected.extend_from_slice(&[1, 0, 0, 0]); // value
        assert_eq!(encode_blob(&state).expect("encode"), expected);
    }

    #[test]
    fn blob_matches_byte_exact_string_vector() {
        let state = PublishedState {
            serial: 2,
            settings: BTreeMap::from([(
                "A".to_owned(),
                PublishedSetting {
                    value: XSettingsValue::Text("xy".to_owned()),
                    last_change_serial: 2,
                },
            )]),
        };
        let expected: Vec<u8> = vec![
            b'l', 0, 0, 0, //
            2, 0, 0, 0, // serial
            1, 0, 0, 0, // count
            1, 0, // type string, pad
            1, 0, // name length
            b'A', 0, 0, 0, // name padded to 4
            2, 0, 0, 0, // last-change serial
            2, 0, 0, 0, // string length
            b'x', b'y', 0, 0, // value padded to 4
        ];
        assert_eq!(encode_blob(&state).expect("encode"), expected);
    }

    #[test]
    fn first_publication_starts_serial_at_one() {
        let want = desired(&[("Gtk/PreferDarkTheme", XSettingsValue::Integer(1))]);
        let next = next_publication(&PublishedState::default(), &want).expect("change");
        assert_eq!(next.serial, 1);
        assert_eq!(next.settings["Gtk/PreferDarkTheme"].last_change_serial, 1);
    }

    #[test]
    fn serial_does_not_advance_on_no_op() {
        let want = desired(&[("Xft/DPI", XSettingsValue::Integer(98_304))]);
        let first = next_publication(&PublishedState::default(), &want).expect("change");
        assert!(next_publication(&first, &want).is_none());
        assert_eq!(first.serial, 1);
    }

    #[test]
    fn serial_increments_only_changed_settings_and_keeps_others() {
        let base = desired(&[
            ("Net/ThemeName", XSettingsValue::Text("Flame-Dark".to_owned())),
            ("Xft/Antialias", XSettingsValue::Integer(1)),
        ]);
        let first = next_publication(&PublishedState::default(), &base).expect("first");
        let changed = desired(&[
            ("Net/ThemeName", XSettingsValue::Text("Flame-Light".to_owned())),
            ("Xft/Antialias", XSettingsValue::Integer(1)),
        ]);
        let second = next_publication(&first, &changed).expect("theme change");
        assert_eq!(second.serial, 2);
        assert_eq!(second.settings["Net/ThemeName"].last_change_serial, 2);
        assert_eq!(second.settings["Xft/Antialias"].last_change_serial, 1);
    }

    #[test]
    fn removed_setting_bumps_serial_and_is_dropped() {
        let first = next_publication(
            &PublishedState::default(),
            &desired(&[
                ("Xft/Antialias", XSettingsValue::Integer(1)),
                ("Xft/Hinting", XSettingsValue::Integer(1)),
            ]),
        )
        .expect("first");
        let second = next_publication(
            &first,
            &desired(&[("Xft/Antialias", XSettingsValue::Integer(1))]),
        )
        .expect("removal");
        assert_eq!(second.serial, 2);
        assert!(!second.settings.contains_key("Xft/Hinting"));
    }

    #[test]
    fn four_byte_name_and_string_value_take_no_padding() {
        let state = PublishedState {
            serial: 3,
            settings: BTreeMap::from([(
                "Xft/RGBA".to_owned(),
                PublishedSetting {
                    value: XSettingsValue::Text("rgb".to_owned()),
                    last_change_serial: 3,
                },
            )]),
        };
        let mut expected = vec![
            b'l', 0, 0, 0, // byte order + pad
            3, 0, 0, 0, // serial
            1, 0, 0, 0, // count
            1, 0, // type string, pad
            8, 0, // name length 8: already 4-byte aligned, no pad
        ];
        expected.extend_from_slice(b"Xft/RGBA");
        expected.extend_from_slice(&[3, 0, 0, 0]); // last-change serial
        expected.extend_from_slice(&[3, 0, 0, 0]); // string length
        expected.extend_from_slice(b"rgb");
        expected.push(0); // value padded to 4
        assert_eq!(encode_blob(&state).expect("encode"), expected);
    }

    #[test]
    fn four_byte_integer_name_and_negative_value_are_exact() {
        let state = PublishedState {
            serial: 1,
            settings: BTreeMap::from([(
                "ABCD".to_owned(),
                PublishedSetting {
                    value: XSettingsValue::Integer(-1),
                    last_change_serial: 1,
                },
            )]),
        };
        let mut expected = vec![
            b'l', 0, 0, 0, //
            1, 0, 0, 0, // serial
            1, 0, 0, 0, // count
            0, 0, // type integer, pad
            4, 0, // name length 4
        ];
        expected.extend_from_slice(b"ABCD");
        expected.extend_from_slice(&[1, 0, 0, 0]); // last-change serial
        expected.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]); // value -1
        assert_eq!(encode_blob(&state).expect("encode"), expected);
    }

    #[test]
    fn keys_encode_in_sorted_name_order() {
        let state = PublishedState {
            serial: 1,
            settings: BTreeMap::from([
                (
                    "B".to_owned(),
                    PublishedSetting {
                        value: XSettingsValue::Integer(2),
                        last_change_serial: 1,
                    },
                ),
                (
                    "A".to_owned(),
                    PublishedSetting {
                        value: XSettingsValue::Integer(1),
                        last_change_serial: 1,
                    },
                ),
            ]),
        };
        let expected: Vec<u8> = vec![
            b'l', 0, 0, 0, //
            1, 0, 0, 0, // serial
            2, 0, 0, 0, // count
            0, 0, 1, 0, b'A', 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, // entry "A" = 1
            0, 0, 1, 0, b'B', 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, // entry "B" = 2
        ];
        assert_eq!(encode_blob(&state).expect("encode"), expected);
    }

    #[test]
    fn serial_wraps_to_zero_after_u32_max() {
        let previous = PublishedState {
            serial: u32::MAX,
            settings: BTreeMap::from([(
                "A".to_owned(),
                PublishedSetting {
                    value: XSettingsValue::Integer(1),
                    last_change_serial: 7,
                },
            )]),
        };
        let next = next_publication(
            &previous,
            &desired(&[("A", XSettingsValue::Integer(2))]),
        )
        .expect("change");
        assert_eq!(next.serial, 0);
        assert_eq!(next.settings["A"].last_change_serial, 0);
    }

    #[test]
    fn same_value_with_different_type_counts_as_change() {
        let first = next_publication(
            &PublishedState::default(),
            &desired(&[("Xft/RGBA", XSettingsValue::Integer(1))]),
        )
        .expect("first");
        let second = next_publication(
            &first,
            &desired(&[("Xft/RGBA", XSettingsValue::Text("1".to_owned()))]),
        )
        .expect("type change");
        assert_eq!(second.serial, 2);
    }
}
