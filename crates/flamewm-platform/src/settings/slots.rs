//! Typed bridges between `ProductSettings` fields and settings text and API values.

use super::*;

/// Typed bridge between one `ProductSettings` field and its file text, API value, and apply input.
pub(super) trait Slot: Sized + Clone {
    fn to_value(&self) -> SettingValue;
    fn from_text(text: &str, key: &str) -> FlameResult<Self>;
    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self>;
}

/// Text-valued slots read `SettingValue::Text` through their own `from_text`.
pub(super) fn from_text_value<T: Slot>(value: &SettingValue, key: &str) -> FlameResult<T> {
    match value {
        SettingValue::Text(text) => T::from_text(text, key),
        _ => invalid("setting requires text"),
    }
}

impl Slot for String {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.clone())
    }

    fn from_text(text: &str, _key: &str) -> FlameResult<Self> {
        Ok(text.to_owned())
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        from_text_value(value, key)
    }
}

impl Slot for bool {
    fn to_value(&self) -> SettingValue {
        SettingValue::Boolean(*self)
    }

    fn from_text(text: &str, key: &str) -> FlameResult<Self> {
        parse_bool(text, key)
    }

    fn from_value(value: &SettingValue, _key: &str) -> FlameResult<Self> {
        match value {
            SettingValue::Boolean(on) => Ok(*on),
            _ => invalid("setting requires boolean"),
        }
    }
}

impl Slot for u8 {
    fn to_value(&self) -> SettingValue {
        SettingValue::Percent(*self)
    }

    fn from_text(text: &str, key: &str) -> FlameResult<Self> {
        parse(text, key)
    }

    fn from_value(value: &SettingValue, _key: &str) -> FlameResult<Self> {
        match value {
            SettingValue::Percent(percent) => Ok(*percent),
            _ => invalid("setting requires percent"),
        }
    }
}

macro_rules! integer_slot {
    ($($ty:ty),*) => {$(
        impl Slot for $ty {
            fn to_value(&self) -> SettingValue {
                SettingValue::Integer(i64::from(*self))
            }

            fn from_text(text: &str, key: &str) -> FlameResult<Self> {
                parse(text, key)
            }

            fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
                match value {
                    SettingValue::Integer(number) => (*number).try_into().map_err(|_| {
                        FlameError::new(ErrorCode::InvalidArgument, format!("{key} is out of range"))
                    }),
                    _ => invalid("setting requires integer"),
                }
            }
        }
    )*};
}

integer_slot!(i8, u16);

pub(super) fn gated_animation(enabled: bool, style: AnimationStyle) -> AnimationStyle {
    if enabled { style } else { AnimationStyle::None }
}

pub(super) fn backdrop(opacity_percent: u8, blur_percent: u8) -> Backdrop {
    let radius_px = if opacity_percent >= 100 {
        0.0
    } else {
        f32::from(blur_percent) / 100.0 * 40.0
    };
    Backdrop {
        opacity_percent,
        blur_percent,
        radius_px,
    }
}

impl Slot for AnimationStyle {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.as_str().to_owned())
    }

    fn from_text(text: &str, key: &str) -> FlameResult<Self> {
        Self::parse_for(key, text).map_or_else(|| invalid_owned(format!("bad {key}")), Ok)
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        from_text_value(value, key)
    }
}

macro_rules! text_enum_slot {
    ($($ty:ty),*) => {$(
        impl Slot for $ty {
            fn to_value(&self) -> SettingValue {
                SettingValue::Text(self.as_str().to_owned())
            }

            fn from_text(text: &str, key: &str) -> FlameResult<Self> {
                Self::parse(text).map_or_else(|| invalid_owned(format!("bad {key}")), Ok)
            }

            fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
                from_text_value(value, key)
            }
        }
    )*};
}

text_enum_slot!(
    LockClockFormat,
    TaskbarStyle,
    TaskbarFill,
    TaskbarGradientEdge,
    DockPosition,
    BackgroundMode,
    WallpaperFit,
    ScreensaverMode,
    HotCornerAction,
    NightShiftMode
);

/// Comma-separated app ids. Each id is validated; duplicates are dropped; capped at `DOCK_PINNED_MAX`.
impl Slot for Vec<String> {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.join(","))
    }

    fn from_text(text: &str, key: &str) -> FlameResult<Self> {
        let mut ids: Vec<String> = Vec::new();
        for raw in text.split(',').map(str::trim).filter(|id| !id.is_empty()) {
            if !is_valid_dock_app_id(raw) {
                return invalid_owned(format!("bad {key} app id"));
            }
            if !ids.iter().any(|id| id == raw) && ids.len() < DOCK_PINNED_MAX {
                ids.push(raw.to_owned());
            }
        }
        Ok(ids)
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        match value {
            SettingValue::Text(text) => Self::from_text(text, key),
            _ => invalid("setting requires text"),
        }
    }
}

/// Desktop-entry ids: ASCII letters, digits, `.`, `-`, `_`; 1..=128 bytes.
#[must_use]
pub fn is_valid_dock_app_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

impl Slot for AppearanceMode {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.as_str().to_owned())
    }

    fn from_text(text: &str, _key: &str) -> FlameResult<Self> {
        AppearanceMode::parse(text).ok_or_else(|| FlameError::invalid("bad appearance"))
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        from_text_value(value, key)
    }
}
