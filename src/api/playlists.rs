//! Personal-playlist discovery (**Profile / Playlist**).
//!
//! Endpoints were established on 2026-09-17 by probing the live API with a
//! test account (spec §2.2): the server answers `ReturnCode 10 "Command is
//! not match"` for unknown paths, which made a read-only search possible.
//!
//! ```text
//! POST /Playlist/GetMyList         {UserId, Token, StartNum, EndNum}
//!      -> {ReturnCode, PlayList: [{PlayId, Name, Describe, CoverFileId,
//!                                  Count, HideFlag, AddFlag}]}
//! POST /Playlist/GetSomeoneList    {UserId, Token, SomeOneUserId, StartNum, EndNum}
//!      -> same shape; public view of another user's playlists
//! POST /Playlist/GetMyImageList    {UserId, Token, PlayId, Version: 12, FileSize: 0, StartNum, EndNum}
//!      -> {ReturnCode, CurListNum, FileList: [gallery records with
//!          GalleryId, FileId, FileName, UserId, UserName, ...]}
//! POST /Playlist/GetSomeoneImageList {UserId, Token, PlayId, SomeOneUserId, StartNum, EndNum}
//! ```
//!
//! The historical APIxoo `/Discover/GetAlbumList` endpoints expose Divoom's
//! editorial albums, not personal playlists; they are kept only as the
//! "public/Discover" reference in `playlist probe`. Paths can still be
//! overridden through environment variables.

use serde::Serialize;
use tracing::{debug, warn};

use super::models::*;
use super::DivoomClient;
use crate::auth::Session;
use crate::error::{Error, Result};
use crate::playlist::{Playlist, PlaylistEntry, Visibility};

/// Owner playlist listing endpoint (verified 2026-09-17).
pub const OWNER_LIST: &str = "/Playlist/GetMyList";
/// Public view of a user's playlists (verified 2026-09-17; needs `SomeOneUserId`).
pub const PUBLIC_LIST: &str = "/Playlist/GetSomeoneList";
/// Playlist item listing endpoint (verified 2026-09-17; needs `PlayId`).
pub const ITEMS: &str = "/Playlist/GetMyImageList";
/// Public item listing of another user's playlist.
pub const PUBLIC_ITEMS: &str = "/Playlist/GetSomeoneImageList";
/// Historical APIxoo endpoint: Divoom editorial albums from the Discover tab.
pub const DISCOVER_ALBUM_LIST: &str = "/Discover/GetAlbumList";

/// Environment variables overriding the candidate endpoints.
pub const ENV_OWNER_LIST: &str = "DIVOOM_ENDPOINT_OWNER_PLAYLISTS";
pub const ENV_PUBLIC_LIST: &str = "DIVOOM_ENDPOINT_PUBLIC_PLAYLISTS";
pub const ENV_ITEMS: &str = "DIVOOM_ENDPOINT_PLAYLIST_ITEMS";

/// `FileSize` value returning items of every dimension. Without it the
/// server silently filters the playlist down to 16×16 artwork, which would
/// hide positions from the exported order.
pub const FILE_SIZE_ALL: i64 = 0;

/// Protocol `Version` sent with item listings. Observed 2026-09-17: once a
/// playlist grew past a handful of items the server returned an empty
/// `FileList` unless `Version` was present (any value from 1 to 999 worked;
/// 12 matches `/GetCategoryFileListV2`).
pub const ITEMS_VERSION: i64 = super::CATEGORY_LIST_VERSION;

/// Page size for item pagination (inclusive `StartNum..=EndNum` windows).
pub const ITEMS_PAGE_SIZE: u64 = 50;
/// Hard upper bound on items read from a single playlist.
pub const MAX_ITEMS_PER_PLAYLIST: usize = 10_000;
/// Hard upper bound on playlists read from a listing.
pub const MAX_PLAYLISTS: usize = 1_000;

/// Endpoint paths used for playlist operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEndpoints {
    pub owner_list: String,
    pub public_list: String,
    pub items: String,
    /// True when at least one path came from an environment override.
    pub overridden: bool,
}

impl Default for PlaylistEndpoints {
    fn default() -> Self {
        Self {
            owner_list: OWNER_LIST.into(),
            public_list: PUBLIC_LIST.into(),
            items: ITEMS.into(),
            overridden: false,
        }
    }
}

impl PlaylistEndpoints {
    /// Verified defaults with environment overrides applied.
    pub fn from_env() -> Result<Self> {
        let mut e = Self::default();
        for (var, target) in [
            (ENV_OWNER_LIST, &mut e.owner_list),
            (ENV_PUBLIC_LIST, &mut e.public_list),
            (ENV_ITEMS, &mut e.items),
        ] {
            if let Ok(v) = std::env::var(var) {
                let v = v.trim().to_string();
                if !v.starts_with('/') || v.contains("://") || v.contains("..") {
                    return Err(Error::Configuration(format!(
                        "{var} must be an absolute API path such as /Playlist/GetMyList"
                    )));
                }
                *target = v;
                e.overridden = true;
            }
        }
        Ok(e)
    }
}

fn visibility_of(private_flag: Option<i64>, hide_flag: Option<i64>) -> Visibility {
    match (private_flag, hide_flag) {
        (Some(p), _) if p != 0 => Visibility::Private,
        (Some(_), _) => Visibility::Public,
        (None, Some(h)) if h != 0 => Visibility::Private,
        (None, Some(_)) => Visibility::Public,
        (None, None) => Visibility::Unknown,
    }
}

impl From<AlbumRecord> for Playlist {
    fn from(a: AlbumRecord) -> Self {
        Playlist {
            id: a.album_id.unwrap_or(0),
            name: a.album_name.unwrap_or_default(),
            visibility: visibility_of(a.private_flag, a.hide_flag),
            item_count: a.file_cnt,
            cover_file_id: a
                .cover_file_id
                .or(a.album_big_image_id)
                .or(a.album_image_id),
            updated_at: a.date.map(|d| d.to_string()),
        }
    }
}

/// Result of listing albums at one endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct AlbumListing {
    pub endpoint: String,
    pub playlists: Vec<Playlist>,
}

impl DivoomClient {
    /// Fetch a paginated album list from `path`.
    async fn fetch_album_list(
        &self,
        session: &Session,
        path: &str,
        page_size: u64,
        someone_user_id: Option<u64>,
    ) -> Result<Vec<Playlist>> {
        let mut out: Vec<Playlist> = Vec::new();
        let mut start = 1u64;
        let mut previous_page: Option<Vec<u64>> = None;
        loop {
            let body = AlbumRequest {
                album_id: None,
                someone_user_id,
                file_size: None,
                version: None,
                start_num: start,
                end_num: start + page_size - 1,
                auth: session.auth(),
            };
            let page: AlbumListResponse = self.post_checked(path, &body).await?;
            if page.album_list.is_empty() {
                break;
            }
            let ids: Vec<u64> = page
                .album_list
                .iter()
                .map(|a| a.album_id.unwrap_or(0))
                .collect();
            if previous_page.as_ref() == Some(&ids) {
                break;
            }
            let mut added = 0usize;
            for record in page.album_list {
                let pl: Playlist = record.into();
                if pl.id == 0 {
                    warn!("skipping album record without AlbumId");
                    continue;
                }
                if out.iter().any(|p| p.id == pl.id) {
                    continue;
                }
                out.push(pl);
                added += 1;
                if out.len() >= MAX_PLAYLISTS {
                    return Ok(out);
                }
            }
            if added == 0 || (ids.len() as u64) < page_size {
                break;
            }
            previous_page = Some(ids);
            start += page_size;
        }
        Ok(out)
    }

    /// List playlists owned by the authenticated account (public and private).
    pub async fn list_owned_playlists(
        &self,
        session: &Session,
        endpoints: &PlaylistEndpoints,
    ) -> Result<Vec<Playlist>> {
        if endpoints.overridden {
            warn!(endpoint = %endpoints.owner_list, "using overridden owner playlist endpoint");
        }
        self.fetch_album_list(session, &endpoints.owner_list, ITEMS_PAGE_SIZE, None)
            .await
    }

    /// List the playlists of `someone_user_id` as visible to other users.
    pub async fn list_public_albums(
        &self,
        session: &Session,
        endpoints: &PlaylistEndpoints,
        someone_user_id: u64,
    ) -> Result<Vec<Playlist>> {
        self.fetch_album_list(
            session,
            &endpoints.public_list,
            ITEMS_PAGE_SIZE,
            Some(someone_user_id),
        )
        .await
    }

    /// Fetch the ordered entries of one playlist with inclusive pagination.
    /// Duplicate entries are preserved; a repeated page stops iteration.
    pub async fn list_playlist_items(
        &self,
        session: &Session,
        endpoints: &PlaylistEndpoints,
        playlist_id: u64,
    ) -> Result<Vec<PlaylistEntry>> {
        let mut out: Vec<PlaylistEntry> = Vec::new();
        let mut start = 1u64;
        let mut previous_page: Option<Vec<u64>> = None;
        loop {
            let body = AlbumRequest {
                album_id: Some(playlist_id),
                someone_user_id: None,
                file_size: Some(FILE_SIZE_ALL),
                version: Some(ITEMS_VERSION),
                start_num: start,
                end_num: start + ITEMS_PAGE_SIZE - 1,
                auth: session.auth(),
            };
            let page: FileListResponse = self.post_checked(&endpoints.items, &body).await?;
            if page.file_list.is_empty() {
                break;
            }
            let ids: Vec<u64> = page
                .file_list
                .iter()
                .map(|r| r.gallery_id.unwrap_or(0))
                .collect();
            if previous_page.as_ref() == Some(&ids) {
                debug!(playlist_id, start, "page repeated; stopping pagination");
                break;
            }
            let page_len = page.file_list.len() as u64;
            for record in page.file_list {
                let Some(gallery_id) = record.gallery_id.filter(|g| *g != 0) else {
                    warn!(playlist_id, "skipping playlist record without GalleryId");
                    continue;
                };
                if let Some(fid) = &record.file_id {
                    if crate::naming::validate_file_id(fid).is_err() {
                        warn!(gallery_id, "ignoring unsafe FileId in playlist record");
                    }
                }
                out.push(PlaylistEntry {
                    position: out.len(),
                    gallery_id,
                    title: record.file_name,
                    author_name: record.user_name,
                    author_user_id: record.user_id,
                    file_id: record
                        .file_id
                        .filter(|f| crate::naming::validate_file_id(f).is_ok()),
                });
                if out.len() >= MAX_ITEMS_PER_PLAYLIST {
                    warn!(
                        playlist_id,
                        "playlist truncated at {MAX_ITEMS_PER_PLAYLIST} items"
                    );
                    return Ok(out);
                }
            }
            if page_len < ITEMS_PAGE_SIZE {
                break;
            }
            previous_page = Some(ids);
            start += ITEMS_PAGE_SIZE;
        }
        Ok(out)
    }
}

/// Sanitized report of one probed endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub role: String,
    pub endpoint: String,
    pub request_schema: Vec<String>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub playlist_count: usize,
    pub playlists: Vec<ProbePlaylist>,
    pub private_playlist_visible: bool,
}

/// Sanitized playlist summary for probe output (no CDN identifiers).
#[derive(Debug, Clone, Serialize)]
pub struct ProbePlaylist {
    pub id: u64,
    pub name: String,
    pub visibility: Visibility,
    pub item_count: Option<u64>,
    pub has_cover: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_items_gallery_ids: Option<Vec<u64>>,
}

/// Full probe report.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub user_id: u64,
    pub endpoints_overridden: bool,
    pub results: Vec<ProbeResult>,
    pub items_endpoint: String,
    pub items_request_schema: Vec<String>,
    pub notes: Vec<String>,
}

fn sanitize(p: &Playlist, first_items: Option<Vec<u64>>) -> ProbePlaylist {
    ProbePlaylist {
        id: p.id,
        name: p.name.clone(),
        visibility: p.visibility,
        item_count: p.item_count,
        has_cover: p.cover_file_id.is_some(),
        first_items_gallery_ids: first_items,
    }
}

impl DivoomClient {
    /// Read-only probe of the candidate playlist endpoints.
    pub async fn probe_playlists(
        &self,
        session: &Session,
        endpoints: &PlaylistEndpoints,
    ) -> ProbeReport {
        let mut results = Vec::new();
        let roles = [
            ("owner", endpoints.owner_list.as_str(), None),
            (
                "public",
                endpoints.public_list.as_str(),
                Some(session.user_id),
            ),
            ("discover", DISCOVER_ALBUM_LIST, None),
        ];
        for (role, path, someone) in roles {
            let mut schema = vec![
                "StartNum".to_string(),
                "EndNum".to_string(),
                "UserId".to_string(),
                "Token".to_string(),
            ];
            if someone.is_some() {
                schema.insert(0, "SomeOneUserId".to_string());
            }
            let listing = self
                .fetch_album_list(session, path, ITEMS_PAGE_SIZE, someone)
                .await;
            let result = match listing {
                Ok(playlists) => {
                    let mut sanitized = Vec::new();
                    for p in playlists.iter().take(20) {
                        let items = if role == "discover" {
                            None
                        } else {
                            self.list_playlist_items(session, endpoints, p.id)
                                .await
                                .ok()
                        };
                        let items = items.map(|items| {
                            items
                                .iter()
                                .take(5)
                                .map(|i| i.gallery_id)
                                .collect::<Vec<_>>()
                        });
                        sanitized.push(sanitize(p, items));
                    }
                    ProbeResult {
                        role: role.into(),
                        endpoint: path.into(),
                        request_schema: schema.clone(),
                        ok: true,
                        error: None,
                        playlist_count: playlists.len(),
                        private_playlist_visible: playlists
                            .iter()
                            .any(|p| p.visibility == Visibility::Private),
                        playlists: sanitized,
                    }
                }
                Err(e) => ProbeResult {
                    role: role.into(),
                    endpoint: path.into(),
                    request_schema: schema.clone(),
                    ok: false,
                    error: Some(e.to_string()),
                    playlist_count: 0,
                    playlists: Vec::new(),
                    private_playlist_visible: false,
                },
            };
            results.push(result);
        }
        ProbeReport {
            user_id: session.user_id,
            endpoints_overridden: endpoints.overridden,
            results,
            items_endpoint: endpoints.items.clone(),
            items_request_schema: vec![
                "PlayId".into(),
                "Version".into(),
                "FileSize".into(),
                "StartNum".into(),
                "EndNum".into(),
                "UserId".into(),
                "Token".into(),
            ],
            notes: vec![
                "Owner and public endpoints were verified against the live API on 2026-09-17.".into(),
                "The Discover listing shows Divoom editorial albums, not personal playlists.".into(),
                "A hidden (HideFlag=1) playlist must appear in the owner listing and not in the public one.".into(),
                format!(
                    "Override candidates with {ENV_OWNER_LIST}, {ENV_PUBLIC_LIST}, {ENV_ITEMS}."
                ),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_flags() {
        assert_eq!(visibility_of(Some(1), None), Visibility::Private);
        assert_eq!(visibility_of(Some(0), Some(1)), Visibility::Public);
        assert_eq!(visibility_of(None, Some(1)), Visibility::Private);
        assert_eq!(visibility_of(None, None), Visibility::Unknown);
    }
}
