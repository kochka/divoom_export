//! Flat directory of normalized symlinks to exported WebP files.
//!
//! `library/webp/` is rebuilt after every successful sync: one relative
//! symlink per asset referenced by a synchronized playlist, named after the
//! artwork title (`cute_tiger_cat.webp`). Duplicate names receive `_1`, `_2`…
//! in ascending `GalleryId` order so the result is deterministic. Only
//! symlinks are ever removed from the directory; regular files are left alone.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use tracing::debug;

use crate::error::{Error, Result};
use crate::manifest::{Library, StateManifest};

/// Sub-directory of normalized WebP symlinks.
pub const LINKS_DIR: &str = "webp";
/// Maximum length of a normalized stem.
pub const MAX_STEM_LEN: usize = 60;

/// Normalize a title into a lowercase `[a-z0-9_]` file stem.
pub fn link_stem(name: &str) -> String {
    let mut out = String::with_capacity(MAX_STEM_LEN);
    let mut pending_sep = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('_');
            }
            pending_sep = false;
            out.push(c.to_ascii_lowercase());
            if out.len() >= MAX_STEM_LEN {
                break;
            }
        } else {
            pending_sep = true;
        }
    }
    out.truncate(MAX_STEM_LEN);
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Compute `stem -> gallery_id` link names for the referenced assets.
pub fn plan_links(state: &StateManifest) -> BTreeMap<String, u64> {
    let referenced = state.referenced_gallery_ids();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut plan = BTreeMap::new();
    // BTreeMap iteration over string keys is not numeric; sort explicitly.
    let mut assets: Vec<_> = state
        .assets
        .values()
        .filter(|a| referenced.contains(&a.gallery_id))
        .collect();
    assets.sort_by_key(|a| a.gallery_id);
    for asset in assets {
        let mut stem = link_stem(&asset.file_name);
        if stem.is_empty() {
            stem = format!("gallery_{}", asset.gallery_id);
        }
        let n = counts.entry(stem.clone()).or_insert(0);
        let name = if *n == 0 {
            format!("{stem}.webp")
        } else {
            format!("{stem}_{n}.webp")
        };
        *n += 1;
        plan.insert(name, asset.gallery_id);
    }
    plan
}

/// Rebuild `library/webp/`. Returns the number of links created.
pub fn rebuild_links(library: &Library, state: &StateManifest) -> Result<usize> {
    let dir = library.root.join(LINKS_DIR);
    fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    // Remove every existing symlink (stale or not); keep regular files.
    for entry in fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))? {
        let entry = entry.map_err(|e| Error::io(&dir, e))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?;
        if meta.file_type().is_symlink() {
            fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
        }
    }
    let plan = plan_links(state);
    let mut created = 0usize;
    for (name, gallery_id) in &plan {
        let Some(asset) = state.asset(*gallery_id) else {
            continue;
        };
        let link = dir.join(name);
        if fs::symlink_metadata(&link).is_ok() {
            debug!(name, "skipping link: a regular file with that name exists");
            continue;
        }
        let target = Path::new("..").join(&asset.webp_path);
        make_symlink(&target, &link)?;
        created += 1;
    }
    Ok(created)
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| Error::io(link, e))
}

#[cfg(not(unix))]
fn make_symlink(_target: &Path, _link: &Path) -> Result<()> {
    Err(Error::Configuration(
        "symlink directory is only supported on Unix".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_are_normalized() {
        assert_eq!(link_stem("Cute Tiger Cat"), "cute_tiger_cat");
        assert_eq!(link_stem("  Sea2026+Xplode!! "), "sea2026_xplode");
        assert_eq!(link_stem("Émoji ✨ Party"), "moji_party");
        assert_eq!(link_stem("../../x"), "x");
        assert_eq!(link_stem("✨"), "");
        assert!(link_stem(&"a ".repeat(200)).len() <= MAX_STEM_LEN);
    }
}
