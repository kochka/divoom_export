//! Wire models for the undocumented Divoom account API.
//!
//! Field names follow the protocol (`PascalCase`). Every response model
//! tolerates missing and additional fields.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Login request body. Never logged.
#[derive(Debug, Serialize)]
pub struct LoginRequest<'a> {
    #[serde(rename = "Email")]
    pub email: &'a str,
    #[serde(rename = "Password")]
    pub password_md5: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginResponse {
    #[serde(rename = "ReturnCode", default)]
    pub return_code: i64,
    #[serde(rename = "ReturnMessage", default)]
    pub return_message: Option<String>,
    #[serde(rename = "UserId", default)]
    pub user_id: Option<u64>,
    #[serde(rename = "Token", default)]
    pub token: Option<crate::auth::Token>,
}

/// Generic envelope used to read `ReturnCode` before decoding a payload.
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    #[serde(rename = "ReturnCode", default)]
    pub return_code: i64,
    #[serde(rename = "ReturnMessage", default)]
    pub return_message: Option<String>,
}

/// Authentication fields attached to every authenticated JSON request.
#[derive(Debug, Serialize)]
pub struct Auth<'a> {
    #[serde(rename = "UserId")]
    pub user_id: u64,
    #[serde(rename = "Token")]
    pub token: &'a crate::auth::Token,
}

#[derive(Debug, Serialize)]
pub struct GalleryInfoRequest<'a> {
    #[serde(flatten)]
    pub auth: Auth<'a>,
    #[serde(rename = "GalleryId")]
    pub gallery_id: u64,
}

/// `/Cloud/GalleryInfo` response. Unknown fields are preserved in `extra`
/// so they can be written to `metadata.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GalleryInfo {
    #[serde(rename = "ReturnCode", default)]
    pub return_code: i64,
    #[serde(
        rename = "ReturnMessage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub return_message: Option<String>,
    #[serde(rename = "GalleryId", default)]
    pub gallery_id: Option<u64>,
    #[serde(rename = "FileId", default)]
    pub file_id: Option<String>,
    #[serde(rename = "FileName", default)]
    pub file_name: Option<String>,
    #[serde(rename = "FileType", default)]
    pub file_type: Option<i64>,
    #[serde(rename = "FileURL", default)]
    pub file_url: Option<String>,
    #[serde(rename = "LayerFileId", default)]
    pub layer_file_id: Option<String>,
    #[serde(rename = "Classify", default)]
    pub classify: Option<i64>,
    #[serde(rename = "Date", default)]
    pub date: Option<i64>,
    #[serde(rename = "LikeCnt", default)]
    pub like_cnt: Option<i64>,
    #[serde(rename = "WatchCnt", default)]
    pub watch_cnt: Option<i64>,
    #[serde(rename = "CommentCnt", default)]
    pub comment_cnt: Option<i64>,
    #[serde(rename = "UserId", default)]
    pub user_id: Option<u64>,
    #[serde(rename = "UserName", default)]
    pub user_name: Option<String>,
    #[serde(rename = "CopyrightFlag", default)]
    pub copyright_flag: Option<i64>,
    #[serde(rename = "HideFlag", default)]
    pub hide_flag: Option<i64>,
    #[serde(rename = "PrivateFlag", default)]
    pub private_flag: Option<i64>,
    #[serde(rename = "OriginalGalleryId", default)]
    pub original_gallery_id: Option<u64>,
    #[serde(rename = "FileTagArray", default)]
    pub file_tag_array: Option<serde_json::Value>,
    #[serde(rename = "Content", default)]
    pub content: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl GalleryInfo {
    /// Current `FileId`, validated for safe CDN use.
    pub fn required_file_id(&self) -> Result<&str> {
        let id = self
            .file_id
            .as_deref()
            .ok_or_else(|| Error::InvalidMetadata("GalleryInfo has no FileId".into()))?;
        crate::naming::validate_file_id(id)?;
        Ok(id)
    }

    pub fn display_name(&self) -> String {
        self.file_name.clone().unwrap_or_default()
    }
}

/// Catalogue category (`Classify`) values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    New,
    Character,
    Emoji,
    Daily,
    Nature,
    Symbol,
    Pattern,
    Creative,
    Top,
    Festival,
    Recommend,
    Plant,
    Animal,
    Person,
    Food,
}

impl Category {
    pub const ALL: &'static [Category] = &[
        Category::New,
        Category::Character,
        Category::Emoji,
        Category::Daily,
        Category::Nature,
        Category::Symbol,
        Category::Pattern,
        Category::Creative,
        Category::Top,
        Category::Festival,
        Category::Recommend,
        Category::Plant,
        Category::Animal,
        Category::Person,
        Category::Food,
    ];

    pub fn classify(self) -> i64 {
        match self {
            Category::New => 0,
            Category::Character => 3,
            Category::Emoji => 4,
            Category::Daily => 5,
            Category::Nature => 6,
            Category::Symbol => 7,
            Category::Pattern => 8,
            Category::Creative => 9,
            Category::Top => 14,
            Category::Festival => 17,
            Category::Recommend => 18,
            Category::Plant => 31,
            Category::Animal => 32,
            Category::Person => 33,
            Category::Food => 35,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::New => "new",
            Category::Character => "character",
            Category::Emoji => "emoji",
            Category::Daily => "daily",
            Category::Nature => "nature",
            Category::Symbol => "symbol",
            Category::Pattern => "pattern",
            Category::Creative => "creative",
            Category::Top => "top",
            Category::Festival => "festival",
            Category::Recommend => "recommend",
            Category::Plant => "plant",
            Category::Animal => "animal",
            Category::Person => "person",
            Category::Food => "food",
        }
    }
}

impl std::str::FromStr for Category {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Category::ALL
            .iter()
            .copied()
            .find(|c| c.name().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| {
                Error::InvalidArguments(format!(
                    "unknown category {s:?}; known: {}",
                    Category::ALL
                        .iter()
                        .map(|c| c.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }
}

/// Catalogue sort order (`FileSort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    New,
    Liked,
}

impl SortOrder {
    pub fn file_sort(self) -> i64 {
        match self {
            SortOrder::New => 0,
            SortOrder::Liked => 1,
        }
    }
}

impl std::str::FromStr for SortOrder {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "new" => Ok(SortOrder::New),
            "liked" => Ok(SortOrder::Liked),
            _ => Err(Error::InvalidArguments(format!(
                "unknown sort {s:?}; known: new, liked"
            ))),
        }
    }
}

/// `/GetCategoryFileListV2` request.
#[derive(Debug, Serialize)]
pub struct CategoryListRequest<'a> {
    #[serde(rename = "StartNum")]
    pub start_num: u64,
    #[serde(rename = "EndNum")]
    pub end_num: u64,
    #[serde(rename = "Classify")]
    pub classify: i64,
    #[serde(rename = "Version")]
    pub version: i64,
    #[serde(rename = "FileSize")]
    pub file_size: i64,
    #[serde(rename = "FileType")]
    pub file_type: i64,
    #[serde(rename = "FileSort")]
    pub file_sort: i64,
    #[serde(rename = "RefreshIndex")]
    pub refresh_index: i64,
    #[serde(flatten)]
    pub auth: Auth<'a>,
}

/// One artwork record as returned by list endpoints (catalogue and playlists).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FileRecord {
    #[serde(rename = "GalleryId", default)]
    pub gallery_id: Option<u64>,
    #[serde(rename = "FileId", default)]
    pub file_id: Option<String>,
    #[serde(rename = "FileName", default)]
    pub file_name: Option<String>,
    #[serde(rename = "FileType", default)]
    pub file_type: Option<i64>,
    #[serde(rename = "UserId", default)]
    pub user_id: Option<u64>,
    #[serde(rename = "UserName", default)]
    pub user_name: Option<String>,
    #[serde(rename = "LikeCnt", default)]
    pub like_cnt: Option<i64>,
    #[serde(rename = "WatchCnt", default)]
    pub watch_cnt: Option<i64>,
    #[serde(rename = "Classify", default)]
    pub classify: Option<i64>,
    #[serde(rename = "Date", default)]
    pub date: Option<i64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Response of list endpoints. Several list field names are accepted.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FileListResponse {
    #[serde(rename = "ReturnCode", default)]
    pub return_code: i64,
    #[serde(rename = "ReturnMessage", default)]
    pub return_message: Option<String>,
    #[serde(
        rename = "FileList",
        alias = "ImageList",
        alias = "AlbumImageList",
        alias = "GalleryList",
        alias = "List",
        default
    )]
    pub file_list: Vec<FileRecord>,
    #[serde(
        rename = "FileListNum",
        alias = "TotalNum",
        alias = "CurListNum",
        alias = "TotalCnt",
        default
    )]
    pub total_num: Option<u64>,
}

/// Album / playlist record. `/Playlist/GetMyList` uses `PlayId`, `Name`,
/// `Describe`, `CoverFileId`, `Count`, `HideFlag`, `AddFlag`; the Discover
/// album endpoints use `AlbumId` (as a string), `AlbumName`, `AlbumImageId`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AlbumRecord {
    #[serde(
        rename = "PlayId",
        alias = "AlbumId",
        alias = "PlaylistId",
        default,
        deserialize_with = "lenient_u64"
    )]
    pub album_id: Option<u64>,
    #[serde(rename = "Name", alias = "AlbumName", alias = "PlaylistName", default)]
    pub album_name: Option<String>,
    #[serde(rename = "Describe", default)]
    pub describe: Option<String>,
    #[serde(rename = "CoverFileId", default)]
    pub cover_file_id: Option<String>,
    #[serde(rename = "AddFlag", default)]
    pub add_flag: Option<i64>,
    #[serde(rename = "AlbumImageId", default)]
    pub album_image_id: Option<String>,
    #[serde(rename = "AlbumBigImageId", default)]
    pub album_big_image_id: Option<String>,
    #[serde(rename = "PrivateFlag", default)]
    pub private_flag: Option<i64>,
    #[serde(rename = "HideFlag", default)]
    pub hide_flag: Option<i64>,
    #[serde(
        rename = "Count",
        alias = "FileCnt",
        alias = "ImageCnt",
        alias = "GalleryCnt",
        default,
        deserialize_with = "lenient_u64"
    )]
    pub file_cnt: Option<u64>,
    #[serde(rename = "UserId", default)]
    pub user_id: Option<u64>,
    #[serde(rename = "Date", alias = "UpdateTime", default)]
    pub date: Option<i64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Response of the candidate album-list endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AlbumListResponse {
    #[serde(rename = "ReturnCode", default)]
    pub return_code: i64,
    #[serde(rename = "ReturnMessage", default)]
    pub return_message: Option<String>,
    #[serde(
        rename = "PlayList",
        alias = "AlbumList",
        alias = "PlaylistList",
        default
    )]
    pub album_list: Vec<AlbumRecord>,
}

/// Accept `123`, `"123"`, or `null` for identifiers the API sometimes
/// serializes as strings.
pub fn lenient_u64<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<u64>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => Ok(n.as_u64()),
        serde_json::Value::String(s) => Ok(s.trim().parse().ok()),
        _ => Ok(None),
    }
}

/// Paginated authenticated request with an optional playlist identifier
/// (`PlayId`) and optional target user (`SomeOneUserId`).
#[derive(Debug, Serialize)]
pub struct AlbumRequest<'a> {
    #[serde(rename = "PlayId", skip_serializing_if = "Option::is_none")]
    pub album_id: Option<u64>,
    #[serde(rename = "SomeOneUserId", skip_serializing_if = "Option::is_none")]
    pub someone_user_id: Option<u64>,
    /// `FileSize` filter for item listings: `0` returns every dimension
    /// (the server defaults to 16×16 only when the field is absent).
    #[serde(rename = "FileSize", skip_serializing_if = "Option::is_none")]
    pub file_size: Option<i64>,
    /// Protocol `Version`. Item listings return nothing when it is absent.
    #[serde(rename = "Version", skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(rename = "StartNum")]
    pub start_num: u64,
    #[serde(rename = "EndNum")]
    pub end_num: u64,
    #[serde(flatten)]
    pub auth: Auth<'a>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_record_accepts_playlist_and_discover_shapes() {
        let live: AlbumRecord = serde_json::from_str(
            r#"{"PlayId":212135,"Name":"seb","Describe":"","CoverFileId":"group1/x","Count":3,"HideFlag":0,"AddFlag":0}"#,
        )
        .unwrap();
        assert_eq!(live.album_id, Some(212135));
        assert_eq!(live.album_name.as_deref(), Some("seb"));
        assert_eq!(live.file_cnt, Some(3));
        assert_eq!(live.hide_flag, Some(0));
        assert_eq!(live.cover_file_id.as_deref(), Some("group1/x"));

        let discover: AlbumRecord = serde_json::from_str(
            r#"{"AlbumId":"112","AlbumName":"Underwater Stories","AlbumImageId":"group1/a.jpg"}"#,
        )
        .unwrap();
        assert_eq!(discover.album_id, Some(112));
        assert_eq!(discover.album_name.as_deref(), Some("Underwater Stories"));
        assert_eq!(discover.file_cnt, None);
    }
}
