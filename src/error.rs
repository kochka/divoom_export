//! Error model and process exit codes.

use std::path::PathBuf;

/// Library-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Typed error categories required by the specification (section 18).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("authentication failed: {0}")]
    Authentication(String),

    #[error("Divoom API error on {endpoint}: ReturnCode {return_code}{}", fmt_message(.message))]
    Api {
        endpoint: String,
        return_code: i64,
        message: Option<String>,
    },

    #[error("rate limited by the Divoom API (HTTP 429) after {attempts} attempts")]
    RateLimited { attempts: u32 },

    #[error("network error: {0}")]
    Network(String),

    #[error("invalid metadata: {0}")]
    InvalidMetadata(String),

    #[error("download exceeds the {limit} byte limit")]
    DownloadTooLarge { limit: u64 },

    #[error("unsupported Divoom format {0}")]
    UnsupportedFormat(u8),

    #[error("unsupported dimensions {width}x{height} (only 16x16 is supported)")]
    UnsupportedDimensions { width: u32, height: u32 },

    #[error("invalid container: {0}")]
    InvalidContainer(String),

    #[error("decryption failed: {0}")]
    Decrypt(String),

    #[error("decompression failed: {0}")]
    Decompress(String),

    #[error("WebP encoding failed: {0}")]
    WebpEncode(String),

    #[error("manifest error: {0}")]
    Manifest(String),

    #[error("I/O error{}: {source}", fmt_path(.path))]
    Io {
        path: Option<PathBuf>,
        #[source]
        source: std::io::Error,
    },

    /// Ambiguous or unmatched playlist selector (exit code 2).
    #[error("playlist selection error: {0}")]
    PlaylistSelection(String),

    /// Invalid command-line arguments detected after parsing (exit code 2).
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    /// The selected playlists contain nothing to synchronize (exit code 4).
    #[error("no synchronizable items")]
    NoSynchronizableItems,
}

fn fmt_message(message: &Option<String>) -> String {
    match message {
        Some(m) if !m.is_empty() => format!(" ({m})"),
        _ => String::new(),
    }
}

fn fmt_path(path: &Option<PathBuf>) -> String {
    match path {
        Some(p) => format!(" at {}", p.display()),
        None => String::new(),
    }
}

impl Error {
    /// Build an I/O error tagged with the path being accessed.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: Some(path.into()),
            source,
        }
    }

    /// Process exit code as defined in the specification (section 18).
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Authentication(_) => 3,
            Error::PlaylistSelection(_) | Error::InvalidArguments(_) => 2,
            Error::NoSynchronizableItems => 4,
            _ => 1,
        }
    }

    /// Whether the error is a transient transport failure worth retrying.
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::Network(_) | Error::RateLimited { .. })
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io { path: None, source }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Manifest(e.to_string())
    }
}

/// Exit code constants.
pub mod exit {
    pub const SUCCESS: i32 = 0;
    pub const GENERAL: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const AUTH: i32 = 3;
    pub const NO_ITEMS: i32 = 4;
}
