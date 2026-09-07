//! One error type for the whole engine.

use core::fmt;

/// What went wrong, in a form a caller can branch on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    /// The file does not begin with the XISF signature.
    NotXisf,
    /// The file is shorter than its own structure claims.
    Truncated,
    /// The XML header could not be parsed.
    BadHeader,
    /// An attribute's value does not match the grammar the spec gives it.
    BadAttribute,
    /// A value is well-formed but not one the spec allows here.
    Unsupported,
    /// A checksum did not match.
    ChecksumMismatch,
    /// Decompression failed.
    Compression,
    /// The caller asked for something that is not there.
    NotFound,
    /// An argument was out of range or otherwise unusable.
    InvalidArgument,
    /// An I/O error.
    Io,
}

impl ErrorKind {
    /// A short name, stable enough to appear in messages and tests.
    pub fn name(self) -> &'static str {
        match self {
            ErrorKind::NotXisf => "not-xisf",
            ErrorKind::Truncated => "truncated",
            ErrorKind::BadHeader => "bad-header",
            ErrorKind::BadAttribute => "bad-attribute",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::ChecksumMismatch => "checksum-mismatch",
            ErrorKind::Compression => "compression",
            ErrorKind::NotFound => "not-found",
            ErrorKind::InvalidArgument => "invalid-argument",
            ErrorKind::Io => "io",
        }
    }
}

/// An engine error: a kind, and a message saying what specifically failed.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.name(), self.message)
    }
}

impl core::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::new(ErrorKind::Io, e.to_string())
    }
}

/// The engine's result type.
pub type Result<T> = core::result::Result<T, Error>;

/// Build an [`Error`] with a formatted message.
#[macro_export]
macro_rules! err {
    ($kind:ident, $($arg:tt)*) => {
        $crate::error::Error::new($crate::error::ErrorKind::$kind, format!($($arg)*))
    };
}
