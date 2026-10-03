//! Incremental, resumable synchronization of remote playlists into a local
//! library of lossless WebP assets.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::Serialize;
use tracing::{debug, info, warn};

use crate::api::models::GalleryInfo;
use crate::api::playlists::PlaylistEndpoints;
use crate::api::DivoomClient;
use crate::auth::Session;
use crate::decode::{self, DecodedArtwork};
use crate::error::{Error, Result};
use crate::export::{inspect_webp, write_webp_atomic};
use crate::manifest::{
    write_atomic, AssetState, Library, PlaylistManifest, PlaylistManifestItem, PlaylistState,
    StateManifest, ASSETS_DIR, METADATA_FILE, PLAYLISTS_DIR, RAW_FILE, SCHEMA_VERSION, WEBP_FILE,
};
use crate::naming::{asset_dir_name, playlist_file_name};
use crate::playlist::{select_playlist, Playlist, PlaylistEntry};
use crate::sha256_hex;

/// Options of `playlist sync`.
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    pub dry_run: bool,
    pub refresh: bool,
    pub prune: bool,
    pub no_raw: bool,
    pub fail_fast: bool,
}

/// Which playlists to synchronize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    All,
    One(String),
}

/// Everything the engine needs to talk to the API and the library.
pub struct SyncContext<'a> {
    pub client: &'a DivoomClient,
    pub session: &'a Session,
    pub endpoints: &'a PlaylistEndpoints,
    pub library: Library,
}

/// Outcome of processing one unique asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetAction {
    Unchanged,
    Downloaded,
    Rebuilt,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetReport {
    pub gallery_id: u64,
    pub action: AssetAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaylistReport {
    pub playlist_id: u64,
    pub name: String,
    pub item_count: usize,
    pub unique_assets: usize,
    pub failed_items: usize,
    pub manifest_path: String,
    /// `Count` reported by the playlist listing, when available. A value
    /// above `item_count` means the API did not list every item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub announced_count: Option<u64>,
}

/// Machine-readable summary of one run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncReport {
    pub dry_run: bool,
    pub unchanged: usize,
    pub downloaded: usize,
    pub rebuilt: usize,
    pub pruned: usize,
    pub failed: usize,
    /// Symlinks created in `webp/` (0 in dry-run).
    pub linked: usize,
    pub playlists: Vec<PlaylistReport>,
    pub assets: Vec<AssetReport>,
    pub removed_playlists: Vec<u64>,
}

impl SyncReport {
    /// `3 unchanged, 2 downloaded, 1 rebuilt, 0 pruned, 1 failed`
    pub fn summary_line(&self) -> String {
        format!(
            "{} unchanged, {} downloaded, {} rebuilt, {} pruned, {} failed{}",
            self.unchanged,
            self.downloaded,
            self.rebuilt,
            self.pruned,
            self.failed,
            if self.dry_run { " (dry run)" } else { "" }
        )
    }

    pub fn is_success(&self) -> bool {
        self.failed == 0
    }

    fn record(&mut self, gallery_id: u64, action: AssetAction, error: Option<String>) {
        match action {
            AssetAction::Unchanged => self.unchanged += 1,
            AssetAction::Downloaded => self.downloaded += 1,
            AssetAction::Rebuilt => self.rebuilt += 1,
            AssetAction::Failed => self.failed += 1,
        }
        self.assets.push(AssetReport {
            gallery_id,
            action,
            error,
        });
    }
}

/// Download one CDN binary by `FileId` and decode it. No account is needed:
/// the CDN is unauthenticated. Used by `fetch` for artworks that the
/// playlist listing hides while their files remain downloadable.
pub async fn fetch_artwork(
    client: &DivoomClient,
    file_id: &str,
) -> Result<(Vec<u8>, DecodedArtwork)> {
    let bytes = client.download_file(file_id).await?;
    let art = decode::decode(&bytes)?;
    Ok((bytes, art))
}

/// Run a synchronization.
pub async fn run_sync(
    ctx: &SyncContext<'_>,
    selector: Selector,
    options: &SyncOptions,
) -> Result<SyncReport> {
    let mut state = ctx.library.load_state()?;
    let mut report = SyncReport {
        dry_run: options.dry_run,
        ..Default::default()
    };

    let remote = ctx
        .client
        .list_owned_playlists(ctx.session, ctx.endpoints)
        .await?;
    let selected: Vec<Playlist> = match &selector {
        Selector::All => remote.clone(),
        Selector::One(sel) => vec![select_playlist(&remote, sel)?.clone()],
    };

    // Resolve every selected playlist's ordered contents first, so that an
    // empty selection is detected before any asset work.
    let mut contents: Vec<(Playlist, Vec<PlaylistEntry>)> = Vec::new();
    for playlist in selected {
        let entries = ctx
            .client
            .list_playlist_items(ctx.session, ctx.endpoints, playlist.id)
            .await?;
        info!(playlist_id = playlist.id, name = %playlist.name, items = entries.len(), "resolved playlist");
        if let Some(missing) = crate::playlist::missing_items(&playlist, entries.len()) {
            warn!(
                playlist_id = playlist.id,
                announced = playlist.item_count.unwrap_or(0),
                returned = entries.len(),
                "server returned fewer items than the playlist count; {missing} item(s) not listed by the API"
            );
        }
        contents.push((playlist, entries));
    }
    if contents.iter().all(|(_, e)| e.is_empty()) {
        return Err(Error::NoSynchronizableItems);
    }

    let now = crate::now_rfc3339();
    let mut processed: BTreeSet<u64> = BTreeSet::new();
    let mut stop = false;

    for (playlist, entries) in &contents {
        let unique: Vec<u64> = {
            let mut seen = BTreeSet::new();
            entries
                .iter()
                .map(|e| e.gallery_id)
                .filter(|g| seen.insert(*g))
                .collect()
        };
        for gallery_id in &unique {
            if processed.contains(gallery_id) {
                continue;
            }
            match sync_asset(ctx, &mut state, *gallery_id, options, &now).await {
                Ok(action) => {
                    processed.insert(*gallery_id);
                    report.record(*gallery_id, action, None);
                }
                Err(e) => {
                    warn!(gallery_id, error = %e, "asset failed");
                    report.record(*gallery_id, AssetAction::Failed, Some(e.to_string()));
                    processed.insert(*gallery_id);
                    if options.fail_fast {
                        stop = true;
                        break;
                    }
                }
            }
        }
        // Every entry whose asset failed (now or under an earlier playlist).
        let failed_ids: BTreeSet<u64> = report
            .assets
            .iter()
            .filter(|a| a.action == AssetAction::Failed)
            .map(|a| a.gallery_id)
            .collect();
        let failed_items = entries
            .iter()
            .filter(|e| failed_ids.contains(&e.gallery_id))
            .count();

        let manifest_rel = format!(
            "{PLAYLISTS_DIR}/{}",
            playlist_file_name(playlist.id, &playlist.name)
        );
        if !options.dry_run && !stop {
            write_playlist(ctx, &mut state, playlist, entries, &manifest_rel, &now)?;
            ctx.library.save_state(&mut state)?;
        }
        report.playlists.push(PlaylistReport {
            playlist_id: playlist.id,
            name: playlist.name.clone(),
            item_count: entries.len(),
            unique_assets: unique.len(),
            failed_items,
            manifest_path: manifest_rel,
            announced_count: playlist.item_count,
        });
        if stop {
            break;
        }
    }

    if stop {
        if !options.dry_run {
            ctx.library.save_state(&mut state)?;
        }
        return Ok(report);
    }

    if selector == Selector::All {
        let remote_ids: BTreeSet<u64> = remote.iter().map(|p| p.id).collect();
        let stale: Vec<PlaylistState> = state
            .playlists
            .values()
            .filter(|p| !remote_ids.contains(&p.playlist_id))
            .cloned()
            .collect();
        for p in stale {
            info!(
                playlist_id = p.playlist_id,
                "remote playlist deleted; removing local snapshot"
            );
            if !options.dry_run {
                if let Ok(path) = ctx.library.resolve(&p.manifest_path) {
                    remove_if_exists(&path)?;
                }
                state.playlists.remove(&p.playlist_id.to_string());
            }
            report.removed_playlists.push(p.playlist_id);
        }
    }

    if options.prune {
        if report.is_success() {
            let referenced = state.referenced_gallery_ids();
            let unreferenced: Vec<AssetState> = state
                .assets
                .values()
                .filter(|a| !referenced.contains(&a.gallery_id))
                .cloned()
                .collect();
            for asset in unreferenced {
                info!(gallery_id = asset.gallery_id, "pruning unreferenced asset");
                if !options.dry_run {
                    prune_asset(&ctx.library, &asset)?;
                    state.assets.remove(&asset.gallery_id.to_string());
                }
                report.pruned += 1;
            }
        } else {
            warn!("skipping --prune because the sync had failures");
        }
    }

    if !options.dry_run {
        ctx.library.save_state(&mut state)?;
        report.linked = crate::links::rebuild_links(&ctx.library, &state)?;
    }
    Ok(report)
}

fn write_playlist(
    ctx: &SyncContext<'_>,
    state: &mut StateManifest,
    playlist: &Playlist,
    entries: &[PlaylistEntry],
    manifest_rel: &str,
    now: &str,
) -> Result<()> {
    let items: Vec<PlaylistManifestItem> = entries
        .iter()
        .map(|e| PlaylistManifestItem {
            position: e.position,
            gallery_id: e.gallery_id,
            asset_path: state
                .asset(e.gallery_id)
                .map(|a| format!("../{}", a.webp_path)),
            title: e.title.clone(),
        })
        .collect();
    let manifest = PlaylistManifest {
        schema_version: SCHEMA_VERSION,
        playlist_id: playlist.id,
        name: playlist.name.clone(),
        visibility: playlist.visibility,
        synced_at: now.to_string(),
        items,
        extra: Default::default(),
    };
    ctx.library
        .save_playlist_manifest(manifest_rel, &manifest)?;
    if let Some(prev) = state.playlist(playlist.id) {
        if prev.manifest_path != manifest_rel {
            if let Ok(old) = ctx.library.resolve(&prev.manifest_path) {
                remove_if_exists(&old)?;
            }
        }
    }
    state.insert_playlist(PlaylistState {
        playlist_id: playlist.id,
        name: playlist.name.clone(),
        visibility: playlist.visibility,
        manifest_path: manifest_rel.to_string(),
        ordered_gallery_ids: entries.iter().map(|e| e.gallery_id).collect(),
        synced_at: now.to_string(),
        extra: Default::default(),
    });
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(path, e)),
    }
}

fn prune_asset(library: &Library, asset: &AssetState) -> Result<()> {
    let webp = library.resolve(&asset.webp_path)?;
    remove_if_exists(&webp)?;
    if let Some(raw) = &asset.raw_path {
        remove_if_exists(&library.resolve(raw)?)?;
    }
    let dir = library.resolve(&asset.dir())?;
    remove_if_exists(&dir.join(METADATA_FILE))?;
    if dir.starts_with(library.assets_dir()) && dir != library.assets_dir() {
        // Remove the directory only when nothing unmanaged remains.
        if fs::read_dir(&dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            fs::remove_dir(&dir).map_err(|e| Error::io(&dir, e))?;
        }
    }
    Ok(())
}

fn file_matches(path: &Path, expected_sha: &str) -> bool {
    match fs::read(path) {
        Ok(bytes) => sha256_hex(&bytes) == expected_sha,
        Err(_) => false,
    }
}

/// Synchronize one unique asset following the decision table of spec §14.
async fn sync_asset(
    ctx: &SyncContext<'_>,
    state: &mut StateManifest,
    gallery_id: u64,
    options: &SyncOptions,
    now: &str,
) -> Result<AssetAction> {
    let previous = state.asset(gallery_id).cloned();
    let info = ctx.client.gallery_info(ctx.session, gallery_id).await?;
    let file_id = info.required_file_id()?.to_string();
    let file_name = info.display_name();

    let dir_rel = match &previous {
        Some(p) => p.dir(),
        None => format!("{ASSETS_DIR}/{}", asset_dir_name(gallery_id, &file_name)),
    };
    let webp_rel = format!("{dir_rel}/{WEBP_FILE}");
    let raw_rel = format!("{dir_rel}/{RAW_FILE}");
    let webp_path = ctx.library.resolve(&webp_rel)?;
    let raw_path = ctx.library.resolve(&raw_rel)?;

    if let Some(prev) = &previous {
        if !options.refresh && prev.file_id == file_id {
            if file_matches(&webp_path, &prev.webp_sha256) {
                debug!(gallery_id, "unchanged");
                if !options.dry_run {
                    let mut updated = prev.clone();
                    refresh_metadata(&mut updated, &info);
                    if options.no_raw && updated.raw_path.is_some() {
                        remove_if_exists(&raw_path)?;
                        updated.raw_path = None;
                        updated.raw_sha256 = None;
                    }
                    state.insert_asset(updated);
                }
                return Ok(AssetAction::Unchanged);
            }
            let raw_ok = prev
                .raw_sha256
                .as_deref()
                .map(|sha| file_matches(&raw_path, sha))
                .unwrap_or(false);
            if raw_ok {
                info!(gallery_id, "rebuilding WebP from local raw data");
                if options.dry_run {
                    return Ok(AssetAction::Rebuilt);
                }
                let bytes = fs::read(&raw_path).map_err(|e| Error::io(&raw_path, e))?;
                let art = decode::decode(&bytes)?;
                let webp = write_webp_atomic(&webp_path, &art)?;
                let mut updated = prev.clone();
                refresh_metadata(&mut updated, &info);
                fill_decoded(&mut updated, &art, bytes[0]);
                updated.webp_frame_count = inspect_webp(&webp).ok().map(|i| i.frame_count as u32);
                updated.webp_sha256 = sha256_hex(&webp);
                updated.synced_at = now.to_string();
                if options.no_raw {
                    remove_if_exists(&raw_path)?;
                    updated.raw_path = None;
                    updated.raw_sha256 = None;
                }
                state.insert_asset(updated);
                return Ok(AssetAction::Rebuilt);
            }
        }
    }

    info!(gallery_id, "downloading");
    if options.dry_run {
        return Ok(AssetAction::Downloaded);
    }
    let bytes = ctx.client.download_file(&file_id).await?;
    let raw_sha = sha256_hex(&bytes);
    let art: DecodedArtwork = decode::decode(&bytes)?;
    // Encode and validate before touching any previously valid output.
    let webp = write_webp_atomic(&webp_path, &art)?;
    let (raw_rel_out, raw_sha_out) = if options.no_raw {
        remove_if_exists(&raw_path)?;
        (None, None)
    } else {
        write_atomic(&raw_path, &bytes)?;
        (Some(raw_rel), Some(raw_sha))
    };
    let metadata_path = ctx.library.resolve(&format!("{dir_rel}/{METADATA_FILE}"))?;
    let metadata = serde_json::to_vec_pretty(&info)?;
    write_atomic(&metadata_path, &metadata)?;

    let mut asset = AssetState {
        gallery_id,
        file_id,
        file_name,
        author_user_id: info.user_id,
        author_name: info.user_name.clone(),
        format: bytes[0],
        width: art.width,
        height: art.height,
        frame_count: art.frames.len() as u32,
        frame_duration_ms: art.frame_duration_ms,
        webp_frame_count: inspect_webp(&webp).ok().map(|i| i.frame_count as u32),
        raw_path: raw_rel_out,
        raw_sha256: raw_sha_out,
        webp_path: webp_rel,
        webp_sha256: sha256_hex(&webp),
        synced_at: now.to_string(),
        extra: Default::default(),
    };
    fill_decoded(&mut asset, &art, bytes[0]);
    state.insert_asset(asset);
    Ok(AssetAction::Downloaded)
}

fn refresh_metadata(asset: &mut AssetState, info: &GalleryInfo) {
    if let Some(name) = &info.file_name {
        asset.file_name = name.clone();
    }
    if info.user_id.is_some() {
        asset.author_user_id = info.user_id;
    }
    if info.user_name.is_some() {
        asset.author_name = info.user_name.clone();
    }
}

fn fill_decoded(asset: &mut AssetState, art: &DecodedArtwork, format: u8) {
    asset.format = format;
    asset.width = art.width;
    asset.height = art.height;
    asset.frame_count = art.frames.len() as u32;
    asset.frame_duration_ms = art.frame_duration_ms;
}
