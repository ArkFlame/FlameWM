use super::*;

pub type WireDict = BTreeMap<String, WireValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireValue {
    Bool(bool),
    U32(u32),
    U64(u64),
    I32(i32),
    I64(i64),
    String(String),
    StringArray(Vec<String>),
    Array(Vec<WireValue>),
    Dict(WireDict),
}

impl WireValue {
    pub(crate) fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            Self::U32(value) => Some(u64::from(*value)),
            _ => None,
        }
    }

    pub(crate) fn as_u32(&self) -> Option<u32> {
        match self {
            Self::U32(value) => Some(*value),
            Self::U64(value) => u32::try_from(*value).ok(),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn as_i32(&self) -> Option<i32> {
        match self {
            Self::I32(value) => Some(*value),
            Self::I64(value) => i32::try_from(*value).ok(),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[WireValue]> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn as_string_array(&self) -> Option<&[String]> {
        match self {
            Self::StringArray(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn as_dict(&self) -> Option<&WireDict> {
        match self {
            Self::Dict(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct WireCall {
    pub interface: String,
    pub member: String,
    pub args: Vec<WireValue>,
    /// Monotonic clock supplied by the WM reactor; not part of the D-Bus ABI.
    pub now_ms: u64,
}

impl core::fmt::Debug for WireCall {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let args: Vec<_> = if self.interface == IFACE_SYSTEM && self.member == "SubmitNetworkSecret"
        {
            self.args
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if index == 2 {
                        WireValue::String("<redacted>".to_owned())
                    } else {
                        value.clone()
                    }
                })
                .collect()
        } else {
            self.args.clone()
        };
        f.debug_struct("WireCall")
            .field("interface", &self.interface)
            .field("member", &self.member)
            .field("args", &args)
            .field("now_ms", &self.now_ms)
            .finish()
    }
}

impl WireCall {
    #[must_use]
    pub fn new(
        interface: impl Into<String>,
        member: impl Into<String>,
        args: Vec<WireValue>,
    ) -> Self {
        Self {
            interface: interface.into(),
            member: member.into(),
            args,
            now_ms: 0,
        }
    }

    #[must_use]
    pub fn with_now_ms(mut self, now_ms: u64) -> Self {
        self.now_ms = now_ms;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WireReply {
    pub values: Vec<WireValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError {
    pub name: String,
    pub message: String,
}

impl From<ControlError> for WireError {
    fn from(value: ControlError) -> Self {
        Self {
            name: value.name.to_owned(),
            message: value.message,
        }
    }
}
