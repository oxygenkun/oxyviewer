//! Error types and result model (F5).

use std::fmt;

/// All errors produced by SiftX.
#[derive(Debug)]
pub enum Error {
    /// Underlying I/O error.
    Io(std::io::Error),
    /// Data does not match the expected format (e.g., bad magic bytes, invalid structure).
    Format(String),
    /// Data was cut short unexpectedly.
    Truncated { needed: usize, available: usize },
    /// The format or feature is recognized but not yet supported.
    Unsupported(String),
    /// An offset or reference has already been visited (circular structure).
    Cycle(u64),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Format(msg) => write!(f, "format error: {msg}"),
            Error::Truncated { needed, available } => {
                write!(
                    f,
                    "truncated: need {needed} bytes, only {available} available"
                )
            }
            Error::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            Error::Cycle(offset) => write!(f, "cycle detected at offset {offset}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_format() {
        let e = Error::Format("bad marker".into());
        assert_eq!(e.to_string(), "format error: bad marker");
    }

    #[test]
    fn display_truncated() {
        let e = Error::Truncated {
            needed: 10,
            available: 4,
        };
        assert_eq!(e.to_string(), "truncated: need 10 bytes, only 4 available");
    }

    #[test]
    fn display_cycle() {
        let e = Error::Cycle(0x1234);
        assert_eq!(e.to_string(), "cycle detected at offset 4660");
    }

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let e: Error = io_err.into();
        assert!(matches!(e, Error::Io(_)));
    }
}
