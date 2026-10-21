use std::fmt;

use crate::uri::VfsUri;

/// Everything that can go wrong in the core, with enough context to show in the UI.
#[derive(Debug)]
pub enum Error {
    NotFound {
        uri: VfsUri,
    },
    NotADirectory {
        uri: VfsUri,
    },
    PermissionDenied {
        uri: VfsUri,
    },
    AlreadyExists {
        uri: VfsUri,
    },
    /// The backend exists in the type system but is not wired up yet.
    Unsupported {
        scheme: String,
        operation: &'static str,
    },
    Cancelled,
    Io {
        context: String,
        source: std::io::Error,
    },
    Other(String),
}

impl Error {
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }

    /// Map a raw `std::io::Error` onto a structured error with the uri attached.
    pub fn from_io(uri: &VfsUri, source: std::io::Error) -> Self {
        use std::io::ErrorKind;
        match source.kind() {
            ErrorKind::NotFound => Error::NotFound { uri: uri.clone() },
            ErrorKind::PermissionDenied => Error::PermissionDenied { uri: uri.clone() },
            ErrorKind::AlreadyExists => Error::AlreadyExists { uri: uri.clone() },
            _ => Error::Io {
                context: uri.to_string(),
                source,
            },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound { uri } => write!(f, "Not found: {uri}"),
            Error::NotADirectory { uri } => write!(f, "Not a directory: {uri}"),
            Error::PermissionDenied { uri } => write!(f, "Permission denied: {uri}"),
            Error::AlreadyExists { uri } => write!(f, "Already exists: {uri}"),
            Error::Unsupported { scheme, operation } => {
                write!(f, "{operation} is not supported for {scheme} yet")
            }
            Error::Cancelled => f.write_str("Cancelled"),
            Error::Io { context, source } => write!(f, "{context}: {source}"),
            Error::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io {
            context: "io".to_string(),
            source,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_io_error_kinds() {
        let uri = VfsUri::local("/tmp/x");
        let err = Error::from_io(&uri, std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(matches!(err, Error::NotFound { .. }));
        assert!(err.to_string().contains("/tmp/x"));

        let err = Error::from_io(
            &uri,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(matches!(err, Error::PermissionDenied { .. }));
    }
}
