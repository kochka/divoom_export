//! Playlist manifests, the global state manifest, and atomic file writes.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::naming::validate_relative_path;
use crate::playlist::Visibility;

/// Schema version written by this build.
pub const SCHEMA_VERSION: u32 = 1;
/// File name of the global state manifest inside the library directory.
pub const STATE_FILE: &str = "state.json";
/// Sub-directory of managed assets.
pub const ASSETS_DIR: &str = "assets";
/// Sub-directory of playlist manifests.
pub const PLAYLISTS_DIR: &str = "playlists";
/// Managed file names inside an asset directory.
pub const WEBP_FILE: &str = "artwork.webp";
pub const RAW_FILE: &str = "source.dat";
pub const METADATA_FILE: &str = "metadata.json";

/// One entry of a playlist manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistManifestItem {
    pub position: usize,
    pub gallery_id: u64,
    /// Relative path from the playlist manifest to the WebP file, or `null`
    /// when the asset could not be produced.
    pub asset_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Readable ordered snapshot of one remote playlist.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistManifest {
    pub schema_version: u32,
    pub playlist_id: u64,
    pub name: String,
    pub visibility: Visibility,
    pub synced_at: String,
    pub items: Vec<PlaylistManifestItem>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Per-playlist entry of the global state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistState {
    pub playlist_id: u64,
    pub name: String,
    pub visibility: Visibility,
    pub manifest_path: String,
    pub ordered_gallery_ids: Vec<u64>,
    pub synced_at: String,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Per-asset entry of the global state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetState {
    pub gallery_id: u64,
    pub file_id: String,
    pub file_name: String,
    #[serde(default)]
    pub author_user_id: Option<u64>,
    #[serde(default)]
    pub author_name: Option<String>,
    pub format: u8,
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub frame_duration_ms: u32,
    /// Frames stored in the WebP; lower than `frame_count` when libwebp
    /// merged identical consecutive frames.
    #[serde(default)]
    pub webp_frame_count: Option<u32>,
    /// Relative raw path, `None` when discarded with `--no-raw`.
    #[serde(default)]
    pub raw_path: Option<String>,
    #[serde(default)]
    pub raw_sha256: Option<String>,
    pub webp_path: String,
    pub webp_sha256: String,
    pub synced_at: String,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl AssetState {
    /// Directory of this asset relative to the library root.
    pub fn dir(&self) -> String {
        match self.webp_path.rsplit_once('/') {
            Some((dir, _)) => dir.to_string(),
            None => String::new(),
        }
    }
}

/// Global state manifest (`state.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateManifest {
    pub schema_version: u32,
    pub updated_at: String,
    #[serde(default)]
    pub playlists: BTreeMap<String, PlaylistState>,
    #[serde(default)]
    pub assets: BTreeMap<String, AssetState>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for StateManifest {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            updated_at: crate::now_rfc3339(),
            playlists: BTreeMap::new(),
            assets: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl StateManifest {
    pub fn asset(&self, gallery_id: u64) -> Option<&AssetState> {
        self.assets.get(&gallery_id.to_string())
    }

    pub fn playlist(&self, playlist_id: u64) -> Option<&PlaylistState> {
        self.playlists.get(&playlist_id.to_string())
    }

    pub fn insert_asset(&mut self, asset: AssetState) {
        self.assets.insert(asset.gallery_id.to_string(), asset);
    }

    pub fn insert_playlist(&mut self, playlist: PlaylistState) {
        self.playlists
            .insert(playlist.playlist_id.to_string(), playlist);
    }

    /// Gallery IDs referenced by any recorded playlist.
    pub fn referenced_gallery_ids(&self) -> std::collections::BTreeSet<u64> {
        self.playlists
            .values()
            .flat_map(|p| p.ordered_gallery_ids.iter().copied())
            .collect()
    }

    /// Validate schema version and stored paths.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(Error::Manifest(format!(
                "unsupported state schema version {} (expected {SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        for p in self.playlists.values() {
            validate_relative_path(&p.manifest_path)?;
        }
        for a in self.assets.values() {
            validate_relative_path(&a.webp_path)?;
            if let Some(raw) = &a.raw_path {
                validate_relative_path(raw)?;
            }
        }
        Ok(())
    }
}

/// Library directory layout helper.
#[derive(Debug, Clone)]
pub struct Library {
    pub root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join(ASSETS_DIR)
    }

    pub fn playlists_dir(&self) -> PathBuf {
        self.root.join(PLAYLISTS_DIR)
    }

    /// Resolve a validated relative managed path against the root.
    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        validate_relative_path(relative)?;
        Ok(self.root.join(relative))
    }

    /// Load `state.json`, returning a fresh state when the file is absent.
    pub fn load_state(&self) -> Result<StateManifest> {
        let path = self.state_path();
        match fs::read(&path) {
            Ok(bytes) => {
                let state: StateManifest = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Manifest(format!("{}: {e}", path.display())))?;
                state.validate()?;
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StateManifest::default()),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    /// Atomically write `state.json`, refreshing `updated_at`.
    pub fn save_state(&self, state: &mut StateManifest) -> Result<()> {
        state.updated_at = crate::now_rfc3339();
        state.validate()?;
        let json = serde_json::to_vec_pretty(state)?;
        write_atomic(&self.state_path(), &json)
    }

    /// Atomically write a playlist manifest at a validated relative path.
    pub fn save_playlist_manifest(
        &self,
        relative: &str,
        manifest: &PlaylistManifest,
    ) -> Result<()> {
        let path = self.resolve(relative)?;
        let json = serde_json::to_vec_pretty(manifest)?;
        write_atomic(&path, &json)
    }

    pub fn load_playlist_manifest(&self, relative: &str) -> Result<PlaylistManifest> {
        let path = self.resolve(relative)?;
        let bytes = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let manifest: PlaylistManifest = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Manifest(format!("{}: {e}", path.display())))?;
        if manifest.schema_version != SCHEMA_VERSION {
            return Err(Error::Manifest(format!(
                "unsupported playlist schema version {}",
                manifest.schema_version
            )));
        }
        Ok(manifest)
    }
}

/// Write `bytes` to `path` through a temporary sibling file followed by an
/// atomic rename. Parent directories are created as needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Manifest(format!("{} has no parent", path.display())))?;
    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::Manifest(format!("{} has no file name", path.display())))?;
    let tmp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut f = fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        f.write_all(bytes).map_err(|e| Error::io(&tmp, e))?;
        f.sync_all().map_err(|e| Error::io(&tmp, e))?;
        fs::rename(&tmp, path).map_err(|e| Error::io(path, e))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip_and_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path());
        assert!(lib.load_state().unwrap().assets.is_empty());
        let mut state = StateManifest::default();
        state.extra.insert("future".into(), serde_json::json!(1));
        lib.save_state(&mut state).unwrap();
        let loaded = lib.load_state().unwrap();
        assert_eq!(loaded.extra.get("future"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn rejects_unsupported_schema() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path());
        fs::write(
            lib.state_path(),
            r#"{"schema_version": 99, "updated_at": "x"}"#,
        )
        .unwrap();
        assert!(matches!(lib.load_state(), Err(Error::Manifest(_))));
    }

    #[test]
    fn atomic_write_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("a.txt");
        write_atomic(&p, b"hello").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"hello");
        let entries: Vec<_> = fs::read_dir(dir.path().join("sub")).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }
}
