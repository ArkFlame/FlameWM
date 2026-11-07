//! StatusNotifierItem identity, registration-string parsing and item properties. Pure: no bus access.

use std::collections::BTreeMap;
use std::fmt;

pub const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
pub const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
const MAX_NAME_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationError {
    Empty,
    TooLong,
    InvalidService,
    InvalidPath,
    MissingSender,
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "registration string is empty",
            Self::TooLong => "registration string exceeds 255 bytes",
            Self::InvalidService => "invalid bus name",
            Self::InvalidPath => "invalid object path",
            Self::MissingSender => "relative registration without a sender",
        })
    }
}

/// Item identity: bus service plus object path. Two registrations with the same pair are one item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ItemKey {
    service: String,
    path: String,
}

impl ItemKey {
    pub fn new(service: &str, path: &str) -> Result<Self, RegistrationError> {
        if !valid_service(service) {
            return Err(RegistrationError::InvalidService);
        }
        if !valid_path(path) {
            return Err(RegistrationError::InvalidPath);
        }
        Ok(Self {
            service: service.to_owned(),
            path: path.to_owned(),
        })
    }

    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The `RegisteredStatusNotifierItems` form: service followed by object path.
    #[must_use]
    pub fn registry_string(&self) -> String {
        format!("{}{}", self.service, self.path)
    }
}

/// Parses the argument of `RegisterStatusNotifierItem`. Accepted forms: `service`,
/// `service/path`, and `/path` (service taken from the calling sender).
pub fn parse_registration(
    argument: &str,
    sender: Option<&str>,
) -> Result<ItemKey, RegistrationError> {
    if argument.is_empty() {
        return Err(RegistrationError::Empty);
    }
    if argument.len() > MAX_NAME_BYTES {
        return Err(RegistrationError::TooLong);
    }
    if argument.starts_with('/') {
        let sender = sender.ok_or(RegistrationError::MissingSender)?;
        return ItemKey::new(sender, argument);
    }
    match argument.find('/') {
        Some(index) => ItemKey::new(&argument[..index], &argument[index..]),
        None => ItemKey::new(argument, DEFAULT_ITEM_PATH),
    }
}

fn valid_service(service: &str) -> bool {
    if service.len() > MAX_NAME_BYTES {
        return false;
    }
    if let Some(unique) = service.strip_prefix(':') {
        // Unique connection names: `:` followed by dot-separated numeric components.
        return !unique.is_empty()
            && unique.contains('.')
            && unique
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.');
    }
    // Well-known names: at least two elements, none empty or starting with a digit.
    let elements: Vec<&str> = service.split('.').collect();
    elements.len() >= 2
        && elements.iter().all(|element| {
            element
                .bytes()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
                && element
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_NAME_BYTES
        && (path.len() == 1 || !path.ends_with('/'))
        && !path.contains("//")
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'/')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

impl Status {
    /// Unknown status strings are `None`; callers keep the default rather than guessing.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Passive" => Some(Self::Passive),
            "Active" => Some(Self::Active),
            "NeedsAttention" => Some(Self::NeedsAttention),
            _ => None,
        }
    }
}

/// One `IconPixmap` entry: `ARGB32`, network byte order, row-major, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconPixmap {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,
}

/// A property value as received from `GetAll`, reduced to what the tray consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropValue {
    Str(String),
    Bool(bool),
    /// `ToolTip` `(sa(iiay)ss)`: only the title (element 2) is kept.
    Tooltip(String),
    /// `IconPixmap` `a(iiay)` with malformed or oversized entries already dropped.
    Pixmaps(Vec<IconPixmap>),
    Other,
}

/// The subset of `org.kde.StatusNotifierItem` properties the tray renders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemProps {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub icon_name: String,
    pub tooltip_title: String,
    pub menu_path: Option<String>,
    pub item_is_menu: bool,
    pub icon_pixmaps: Vec<IconPixmap>,
}

impl ItemProps {
    #[must_use]
    pub fn from_props(props: &BTreeMap<String, PropValue>) -> Self {
        let text = |key: &str| match props.get(key) {
            Some(PropValue::Str(value)) => value.clone(),
            _ => String::new(),
        };
        let status = match props.get("Status") {
            Some(PropValue::Str(value)) => Status::parse(value).unwrap_or_default(),
            _ => Status::default(),
        };
        let menu_path = Some(text("Menu")).filter(|path| !path.is_empty() && path != "/");
        let item_is_menu = matches!(props.get("ItemIsMenu"), Some(PropValue::Bool(true)));
        let tooltip_title = match props.get("ToolTip") {
            Some(PropValue::Tooltip(title)) => title.clone(),
            _ => String::new(),
        };
        let icon_pixmaps = match props.get("IconPixmap") {
            Some(PropValue::Pixmaps(pixmaps)) => pixmaps.clone(),
            _ => Vec::new(),
        };
        Self {
            id: text("Id"),
            title: text("Title"),
            status,
            icon_name: text("IconName"),
            tooltip_title,
            menu_path,
            item_is_menu,
            icon_pixmaps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_service_uses_default_path() {
        let key = parse_registration("org.example.Tray-42-1", Some(":1.9")).unwrap();
        assert_eq!(key.service(), "org.example.Tray-42-1");
        assert_eq!(key.path(), DEFAULT_ITEM_PATH);
        assert_eq!(
            key.registry_string(),
            "org.example.Tray-42-1/StatusNotifierItem"
        );
    }

    #[test]
    fn service_with_path_splits_at_first_slash() {
        let key = parse_registration(":1.57/org/ayatana/NotificationItem/x", None).unwrap();
        assert_eq!(key.service(), ":1.57");
        assert_eq!(key.path(), "/org/ayatana/NotificationItem/x");
    }

    #[test]
    fn bare_path_takes_sender_as_service() {
        let key = parse_registration("/Item", Some(":1.12")).unwrap();
        assert_eq!(key.service(), ":1.12");
        assert_eq!(key.path(), "/Item");
        assert_eq!(
            parse_registration("/Item", None),
            Err(RegistrationError::MissingSender)
        );
    }

    #[test]
    fn rejects_empty_long_and_malformed_names() {
        assert_eq!(parse_registration("", None), Err(RegistrationError::Empty));
        let long = format!("org.{}", "a".repeat(260));
        assert_eq!(
            parse_registration(&long, None),
            Err(RegistrationError::TooLong)
        );
        assert_eq!(
            parse_registration("nodots", None),
            Err(RegistrationError::InvalidService)
        );
        assert_eq!(
            parse_registration("org.example.1bad", None),
            Err(RegistrationError::InvalidService)
        );
        assert_eq!(
            parse_registration(":1.x", None),
            Err(RegistrationError::InvalidService)
        );
        assert_eq!(
            parse_registration("org.example/bad path", None),
            Err(RegistrationError::InvalidPath)
        );
        assert_eq!(
            parse_registration("org.example/a//b", None),
            Err(RegistrationError::InvalidPath)
        );
        assert_eq!(
            parse_registration("org.example/a/", None),
            Err(RegistrationError::InvalidPath)
        );
    }

    #[test]
    fn status_parse_is_exact() {
        assert_eq!(
            Status::parse("NeedsAttention"),
            Some(Status::NeedsAttention)
        );
        assert_eq!(Status::parse("passive"), None);
        assert_eq!(Status::default(), Status::Active);
    }

    #[test]
    fn props_fill_fields_and_drop_empty_menu() {
        let mut props = BTreeMap::new();
        props.insert("Id".to_owned(), PropValue::Str("nm-applet".into()));
        props.insert("Title".to_owned(), PropValue::Str("Network".into()));
        props.insert("Status".to_owned(), PropValue::Str("NeedsAttention".into()));
        props.insert(
            "IconName".to_owned(),
            PropValue::Str("network-wired".into()),
        );
        props.insert("Menu".to_owned(), PropValue::Str("/MenuBar".into()));
        props.insert("ItemIsMenu".to_owned(), PropValue::Bool(true));
        let parsed = ItemProps::from_props(&props);
        assert_eq!(parsed.id, "nm-applet");
        assert_eq!(parsed.status, Status::NeedsAttention);
        assert_eq!(parsed.menu_path.as_deref(), Some("/MenuBar"));
        assert!(parsed.item_is_menu);

        props.insert("Menu".to_owned(), PropValue::Str("/".into()));
        props.insert("Status".to_owned(), PropValue::Str("Bogus".into()));
        props.insert("Title".to_owned(), PropValue::Other);
        let parsed = ItemProps::from_props(&props);
        assert_eq!(parsed.menu_path, None);
        assert_eq!(parsed.status, Status::Active);
        assert_eq!(parsed.title, "");
    }
}
