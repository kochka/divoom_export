//! `divoom-export` library: Divoom account API client, personal-playlist
//! synchronization, 16×16 artwork decoding, and lossless WebP export.
//!
//! The binary in `src/main.rs` is a thin CLI over this crate. API, decoding,
//! export, and synchronization modules do not depend on CLI parsing.

pub mod api;
pub mod auth;
pub mod decode;
pub mod error;
pub mod export;
pub mod links;
pub mod manifest;
pub mod naming;
pub mod playlist;
pub mod sync;

pub use error::{Error, Result};

/// Current UTC timestamp formatted as RFC 3339 with second precision (`Z` suffix).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Lowercase hexadecimal SHA-256 digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}
