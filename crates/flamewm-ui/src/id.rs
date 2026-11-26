use std::fmt;
use std::sync::Arc;

/// Stable identifiers are shared handles rather than owned strings: the tree, an event and a
/// widget all name the same id, so cloning one on dispatch is a refcount bump instead of a heap
/// copy.
macro_rules! stable_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Arc<str>);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                let value = value.into();
                Self(Arc::from(value))
            }
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
            #[must_use]
            pub fn is_valid(&self) -> bool {
                !self.0.is_empty()
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(Arc::from(value))
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(Arc::from(value))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

stable_id!(WidgetId);
stable_id!(ActionId);
