use std::fmt;

/// Raw DeckLink HRESULT. Unknown values are preserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hresult(pub i32);

impl Hresult {
    pub const OK: Self = Self(decklink_sys::S_OK);
    pub const FALSE: Self = Self(decklink_sys::S_FALSE);
    pub const NOT_IMPL: Self = Self(decklink_sys::E_NOTIMPL);
    pub const OUT_OF_MEMORY: Self = Self(decklink_sys::E_OUTOFMEMORY);
    pub const INVALID_ARG: Self = Self(decklink_sys::E_INVALIDARG);
    pub const NO_INTERFACE: Self = Self(decklink_sys::E_NOINTERFACE);
    pub const FAIL: Self = Self(decklink_sys::E_FAIL);
    pub const ACCESS_DENIED: Self = Self(decklink_sys::E_ACCESSDENIED);

    pub fn is_success(self) -> bool {
        decklink_sys::succeeded(self.0)
    }

    pub fn is_failure(self) -> bool {
        decklink_sys::failed(self.0)
    }

    pub fn name(self) -> Option<&'static str> {
        match self.0 {
            decklink_sys::S_OK => Some("S_OK"),
            decklink_sys::S_FALSE => Some("S_FALSE"),
            decklink_sys::E_NOTIMPL => Some("E_NOTIMPL"),
            decklink_sys::E_OUTOFMEMORY => Some("E_OUTOFMEMORY"),
            decklink_sys::E_INVALIDARG => Some("E_INVALIDARG"),
            decklink_sys::E_NOINTERFACE => Some("E_NOINTERFACE"),
            decklink_sys::E_FAIL => Some("E_FAIL"),
            decklink_sys::E_ACCESSDENIED => Some("E_ACCESSDENIED"),
            _ => None,
        }
    }
}

impl fmt::Display for Hresult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} (0x{:08X})", self.0 as u32),
            None => write!(f, "HRESULT(0x{:08X})", self.0 as u32),
        }
    }
}

/// High-level failure class. The original HRESULT stays in [`Error::hresult`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Sdk,
    InvalidState,
    Unsupported,
    QueueOverflow,
    Disconnected,
    Cancelled,
    ShutdownTimeout,
    AbiMismatch,
    DriverNotFound,
}

/// Recoverable crate error.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{kind:?} during {operation}: {message}")]
pub struct Error {
    pub kind: ErrorKind,
    pub operation: &'static str,
    pub message: String,
    pub hresult: Option<Hresult>,
}

impl Error {
    pub fn new(kind: ErrorKind, operation: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            operation,
            message: message.into(),
            hresult: None,
        }
    }

    pub fn sdk(operation: &'static str, hr: Hresult) -> Self {
        let kind = match hr.0 {
            decklink_sys::E_NOTIMPL | decklink_sys::E_NOINTERFACE => ErrorKind::Unsupported,
            decklink_sys::E_INVALIDARG => ErrorKind::InvalidState,
            decklink_sys::E_ACCESSDENIED => ErrorKind::InvalidState,
            _ => ErrorKind::Sdk,
        };
        Self {
            kind,
            operation,
            message: hr.to_string(),
            hresult: Some(hr),
        }
    }

    pub fn check(operation: &'static str, hr: i32) -> Result<()> {
        let hr = Hresult(hr);
        if hr.is_failure() {
            Err(Self::sdk(operation, hr))
        } else {
            Ok(())
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
