//! Deterministic, filesystem-safe naming for managed paths.

use std::path::Path;

use crate::error::{Error, Result};

/// Maximum slug length appended to identifiers.
pub const MAX_SLUG_LEN: usize = 40;

/// Reduce an arbitrary name to a bounded lowercase ASCII slug (`a-z0-9-`).
/// Returns an empty string when nothing usable remains.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(MAX_SLUG_LEN);
    let mut last_dash = true;
    for c in name.chars() {
        let mapped = if c.is_ascii_alphanumeric() {
            Some(c.to_ascii_lowercase())
        } else if c.is_ascii_whitespace() || matches!(c, '-' | '_' | '.' | '/' | '\\' | ':') {
            None
        } else {
            continue;
        };
        match mapped {
            Some(ch) => {
                out.push(ch);
                last_dash = false;
            }
            None if !last_dash => {
                out.push('-');
                last_dash = true;
            }
            None => {}
        }
        if out.len() >= MAX_SLUG_LEN {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Directory name for an asset: `<gallery_id>[-<slug>]`.
pub fn asset_dir_name(gallery_id: u64, file_name: &str) -> String {
    let slug = slugify(file_name);
    if slug.is_empty() {
        gallery_id.to_string()
    } else {
        format!("{gallery_id}-{slug}")
    }
}

/// File name for a playlist manifest: `<playlist_id>[-<slug>].json`.
pub fn playlist_file_name(playlist_id: u64, name: &str) -> String {
    let slug = slugify(name);
    if slug.is_empty() {
        format!("{playlist_id}.json")
    } else {
        format!("{playlist_id}-{slug}.json")
    }
}

/// Validate a relative managed path stored in the state manifest: no absolute
/// path, no `..`, no empty component, forward slashes only.
pub fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty() {
        return Err(Error::Manifest("empty managed path".into()));
    }
    if path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return Err(Error::Manifest(format!("unsafe managed path: {path}")));
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(Error::Manifest(format!("unsafe managed path: {path}")));
        }
    }
    if Path::new(path).is_absolute() {
        return Err(Error::Manifest(format!("unsafe managed path: {path}")));
    }
    Ok(())
}

/// Validate a CDN `FileId` before building a download URL: non-empty, relative,
/// no scheme or host, no `..`, no backslash, no control characters.
pub fn validate_file_id(file_id: &str) -> Result<()> {
    let bad = |why: &str| Err(Error::InvalidMetadata(format!("rejected FileId ({why})")));
    if file_id.trim().is_empty() {
        return bad("empty");
    }
    if file_id.contains("://") || file_id.starts_with("//") {
        return bad("absolute URL");
    }
    if file_id.starts_with('/') {
        return bad("absolute path");
    }
    if file_id.contains('\\') {
        return bad("backslash");
    }
    if file_id
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '?' || c == '#')
    {
        return bad("illegal character");
    }
    if file_id
        .split('/')
        .any(|seg| seg == ".." || seg == "." || seg.is_empty())
    {
        return bad("path traversal");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_bounded_ascii() {
        assert_eq!(slugify("Space Cat"), "space-cat");
        assert_eq!(slugify("  Émoji ✨ Party!!  "), "moji-party");
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
        assert!(slugify(&"x".repeat(200)).len() <= MAX_SLUG_LEN);
        assert_eq!(slugify("✨✨"), "");
    }

    #[test]
    fn names_start_with_ids() {
        assert_eq!(asset_dir_name(4152005, "Space Cat"), "4152005-space-cat");
        assert_eq!(asset_dir_name(7, ""), "7");
        assert_eq!(playlist_file_name(123, "Favourites"), "123-favourites.json");
    }

    #[test]
    fn rejects_dangerous_file_ids() {
        assert!(validate_file_id("").is_err());
        assert!(validate_file_id("http://evil/x").is_err());
        assert!(validate_file_id("//evil/x").is_err());
        assert!(validate_file_id("/group1/x").is_err());
        assert!(validate_file_id("group1/../x").is_err());
        assert!(validate_file_id("group1\\x").is_err());
        assert!(validate_file_id("group1/x y").is_err());
        assert!(validate_file_id("group1/M00/00/01/abc.dat").is_ok());
    }

    #[test]
    fn rejects_dangerous_managed_paths() {
        assert!(validate_relative_path("assets/1/a.webp").is_ok());
        assert!(validate_relative_path("/assets/1/a.webp").is_err());
        assert!(validate_relative_path("assets/../a").is_err());
        assert!(validate_relative_path("").is_err());
    }
}
