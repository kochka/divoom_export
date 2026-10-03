//! Shared test scaffolding: an in-process fake Divoom API and CDN built on
//! `wiremock`, plus synthetic artwork fixtures.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use divoom_export::api::playlists::PlaylistEndpoints;
use divoom_export::api::{ClientConfig, DivoomClient};
use divoom_export::auth::{Credentials, Session};
use divoom_export::decode::{format17, format9, FRAME_BYTES};

pub const EMAIL: &str = "tester@example.com";
pub const PASSWORD_MD5: &str = "5f4dcc3b5aa765d61d8327deb882cf99";
pub const TOKEN: &str = "tok-secret-0123456789";
/// Numeric token variant (what the live server actually returns).
pub const TOKEN_NUMBER: i64 = 151_176_847;
pub const USER_ID: u64 = 4242;

/// Real endpoint paths (verified 2026-09-17), served by the fake.
pub const OWNER_PATH: &str = "/Playlist/GetMyList";
pub const PUBLIC_PATH: &str = "/Playlist/GetSomeoneList";
pub const ITEMS_PATH: &str = "/Playlist/GetMyImageList";
pub const DISCOVER_PATH: &str = "/Discover/GetAlbumList";

#[derive(Debug, Clone)]
pub struct FakePlaylist {
    pub id: u64,
    pub name: String,
    pub private: bool,
    pub items: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct FakeGallery {
    pub file_id: String,
    pub file_name: String,
    pub user_id: u64,
    pub user_name: String,
}

#[derive(Debug, Default)]
pub struct FakeState {
    pub playlists: Vec<FakePlaylist>,
    pub gallery: BTreeMap<u64, FakeGallery>,
    pub files: HashMap<String, Vec<u8>>,
    /// GalleryInfo returns ReturnCode 1 for these.
    pub fail_gallery: HashSet<u64>,
    /// CDN returns HTTP 500 for these file ids.
    pub fail_download: HashSet<String>,
    /// CDN returns HTTP 500 this many times before succeeding.
    pub flaky_download: HashMap<String, u32>,
    /// Catalogue entries served by `/GetCategoryFileListV2`.
    pub catalog: Vec<u64>,
    /// Serve the same catalogue page regardless of StartNum (repeat detection).
    pub catalog_repeat_pages: bool,
    /// Serve HTTP 500 this many times for API POSTs to the given path.
    pub flaky_api: HashMap<String, u32>,
    /// Serve HTTP 429 (with Retry-After: 0) this many times for the path.
    pub rate_limit_api: HashMap<String, u32>,
    /// Return the token as a JSON integer instead of a string.
    pub numeric_token: bool,
    /// Extra items to add to the announced `Count` of every playlist
    /// (live servers were observed announcing more items than they list).
    pub count_inflation: usize,
}

impl FakeState {
    pub fn add_asset(&mut self, gallery_id: u64, name: &str, bytes: Vec<u8>) -> String {
        let file_id = format!("group1/M00/{gallery_id}/{}.dat", slug(name));
        self.gallery.insert(
            gallery_id,
            FakeGallery {
                file_id: file_id.clone(),
                file_name: name.to_string(),
                user_id: 77,
                user_name: "artist".to_string(),
            },
        );
        self.files.insert(file_id.clone(), bytes);
        file_id
    }

    /// Replace the binary of `gallery_id` with new content and a new FileId.
    pub fn replace_asset(&mut self, gallery_id: u64, bytes: Vec<u8>) -> String {
        let g = self.gallery.get_mut(&gallery_id).expect("gallery");
        let file_id = format!("group1/M00/{gallery_id}/v2-{}.dat", bytes.len());
        g.file_id = file_id.clone();
        self.files.insert(file_id.clone(), bytes);
        file_id
    }

    pub fn playlist_mut(&mut self, id: u64) -> &mut FakePlaylist {
        self.playlists
            .iter_mut()
            .find(|p| p.id == id)
            .expect("playlist")
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

struct ApiResponder(Arc<Mutex<FakeState>>);

fn page_window(body: &Value) -> (usize, usize) {
    let start = body["StartNum"].as_u64().unwrap_or(1).max(1) as usize;
    let end = body["EndNum"].as_u64().unwrap_or(start as u64) as usize;
    (start - 1, end)
}

fn authed(body: &Value) -> bool {
    body["UserId"].as_u64() == Some(USER_ID)
        && (body["Token"].as_str() == Some(TOKEN) || body["Token"].as_i64() == Some(TOKEN_NUMBER))
}

fn album_json(p: &FakePlaylist, inflation: usize) -> Value {
    json!({
        "PlayId": p.id,
        "Name": p.name,
        "Describe": "",
        "CoverFileId": "group1/cover.png",
        "Count": p.items.len() + inflation,
        "HideFlag": if p.private { 1 } else { 0 },
        "AddFlag": 0,
    })
}

impl Respond for ApiResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let path = req.url.path().to_string();
        if let Some(n) = st.flaky_api.get_mut(&path) {
            if *n > 0 {
                *n -= 1;
                return ResponseTemplate::new(500);
            }
        }
        if let Some(n) = st.rate_limit_api.get_mut(&path) {
            if *n > 0 {
                *n -= 1;
                return ResponseTemplate::new(429).insert_header("Retry-After", "0");
            }
        }
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        if path == "/UserLogin" {
            if body["Email"].as_str() == Some(EMAIL)
                && body["Password"].as_str() == Some(PASSWORD_MD5)
            {
                let token = if st.numeric_token {
                    json!(TOKEN_NUMBER)
                } else {
                    json!(TOKEN)
                };
                return ResponseTemplate::new(200).set_body_json(json!({
                    "ReturnCode": 0, "UserId": USER_ID, "Token": token, "UserToken": "other"
                }));
            }
            return ResponseTemplate::new(200).set_body_json(json!({
                "ReturnCode": 1, "ReturnMessage": "bad credentials"
            }));
        }
        if !authed(&body) {
            return ResponseTemplate::new(200)
                .set_body_json(json!({"ReturnCode": 100, "ReturnMessage": "not logged in"}));
        }
        match path.as_str() {
            "/Cloud/GalleryInfo" => {
                let gid = body["GalleryId"].as_u64().unwrap_or(0);
                if st.fail_gallery.contains(&gid) {
                    return ResponseTemplate::new(200)
                        .set_body_json(json!({"ReturnCode": 1, "ReturnMessage": "unavailable"}));
                }
                match st.gallery.get(&gid) {
                    Some(g) => ResponseTemplate::new(200).set_body_json(json!({
                        "ReturnCode": 0,
                        "GalleryId": gid,
                        "FileId": g.file_id,
                        "FileName": g.file_name,
                        "FileType": 1,
                        "UserId": g.user_id,
                        "UserName": g.user_name,
                        "LikeCnt": 3,
                        "WatchCnt": 10,
                        "CommentCnt": 0,
                        "PrivateFlag": 0,
                        "SomeFutureField": "x"
                    })),
                    None => ResponseTemplate::new(200).set_body_json(
                        json!({"ReturnCode": 2, "ReturnMessage": "no such gallery"}),
                    ),
                }
            }
            OWNER_PATH | PUBLIC_PATH => {
                if path == PUBLIC_PATH && body["SomeOneUserId"].as_u64() != Some(USER_ID) {
                    return ResponseTemplate::new(200)
                        .set_body_json(json!({"ReturnCode": 0, "PlayList": []}));
                }
                let (from, to) = page_window(&body);
                let inflation = st.count_inflation;
                let list: Vec<Value> = st
                    .playlists
                    .iter()
                    .filter(|p| path == OWNER_PATH || !p.private)
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .map(|p| album_json(p, inflation))
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"ReturnCode": 0, "PlayList": list}))
            }
            DISCOVER_PATH => {
                // Editorial albums: string ids, unrelated to personal playlists.
                ResponseTemplate::new(200).set_body_json(json!({"ReturnCode": 0, "AlbumList": [
                    {"AlbumId": "112", "AlbumName": "Underwater Stories",
                     "AlbumImageId": "group1/a.jpg", "AlbumBigImageId": "group1/b.jpg"}
                ]}))
            }
            ITEMS_PATH => {
                // Live behaviour: no `Version` => empty list.
                if body.get("Version").is_none() {
                    return ResponseTemplate::new(200)
                        .set_body_json(json!({"ReturnCode": 0, "CurListNum": 0, "FileList": []}));
                }
                let album = body["PlayId"].as_u64().unwrap_or(0);
                let (from, to) = page_window(&body);
                let Some(p) = st.playlists.iter().find(|p| p.id == album) else {
                    return ResponseTemplate::new(200)
                        .set_body_json(json!({"ReturnCode": 3, "ReturnMessage": "no album"}));
                };
                let list: Vec<Value> = p
                    .items
                    .iter()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .map(|gid| {
                        let g = st.gallery.get(gid);
                        json!({
                            "GalleryId": gid,
                            "FileId": g.map(|g| g.file_id.clone()),
                            "FileName": g.map(|g| g.file_name.clone()),
                            "UserName": g.map(|g| g.user_name.clone()),
                            "UserId": g.map(|g| g.user_id),
                        })
                    })
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"ReturnCode": 0, "FileList": list}))
            }
            "/GetCategoryFileListV2" => {
                let (from, to) = page_window(&body);
                let from = if st.catalog_repeat_pages { 0 } else { from };
                let to = if st.catalog_repeat_pages {
                    to - page_window(&body).0
                } else {
                    to
                };
                let list: Vec<Value> = st
                    .catalog
                    .iter()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .map(|gid| json!({"GalleryId": gid, "FileName": format!("cat-{gid}"), "LikeCnt": 1}))
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"ReturnCode": 0, "FileList": list}))
            }
            _ => ResponseTemplate::new(404),
        }
    }
}

struct CdnResponder(Arc<Mutex<FakeState>>);

impl Respond for CdnResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        let file_id = req.url.path().trim_start_matches('/').to_string();
        if st.fail_download.contains(&file_id) {
            return ResponseTemplate::new(500);
        }
        if let Some(n) = st.flaky_download.get_mut(&file_id) {
            if *n > 0 {
                *n -= 1;
                return ResponseTemplate::new(500);
            }
        }
        match st.files.get(&file_id) {
            Some(bytes) => {
                ResponseTemplate::new(200).set_body_raw(bytes.clone(), "application/octet-stream")
            }
            None => ResponseTemplate::new(404),
        }
    }
}

/// Fake Divoom backend (API + CDN) for integration tests.
pub struct FakeDivoom {
    pub api: MockServer,
    pub cdn: MockServer,
    pub state: Arc<Mutex<FakeState>>,
}

impl FakeDivoom {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let api = MockServer::start().await;
        let cdn = MockServer::start().await;
        let fake = Self { api, cdn, state };
        fake.mount().await;
        fake
    }

    async fn mount(&self) {
        Mock::given(wiremock::matchers::method("POST"))
            .respond_with(ApiResponder(self.state.clone()))
            .mount(&self.api)
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .respond_with(CdnResponder(self.state.clone()))
            .mount(&self.cdn)
            .await;
    }

    /// Clear recorded requests (mocks are re-mounted).
    pub async fn reset_requests(&self) {
        self.api.reset().await;
        self.cdn.reset().await;
        self.mount().await;
    }

    pub fn with_state<R>(&self, f: impl FnOnce(&mut FakeState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    pub fn config(&self) -> ClientConfig {
        ClientConfig {
            max_attempts: 3,
            retry_base_delay: Duration::from_millis(5),
            ..ClientConfig::with_hosts(&self.api.uri(), &self.cdn.uri())
        }
    }

    pub fn client(&self) -> DivoomClient {
        DivoomClient::new(self.config()).unwrap()
    }

    pub fn credentials(&self) -> Credentials {
        Credentials {
            email: EMAIL.into(),
            password_md5: PASSWORD_MD5.into(),
        }
    }

    pub async fn session(&self, client: &DivoomClient) -> Session {
        client.login(&self.credentials()).await.expect("login")
    }

    pub fn endpoints(&self) -> PlaylistEndpoints {
        PlaylistEndpoints {
            owner_list: OWNER_PATH.into(),
            public_list: PUBLIC_PATH.into(),
            items: ITEMS_PATH.into(),
            overridden: true,
        }
    }

    /// Same paths as the defaults, without the `overridden` flag.
    pub fn endpoints_unflagged(&self) -> PlaylistEndpoints {
        PlaylistEndpoints {
            overridden: false,
            ..self.endpoints()
        }
    }

    pub async fn api_calls(&self, path: &str) -> usize {
        self.api
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path() == path)
            .count()
    }

    pub async fn cdn_downloads(&self) -> usize {
        self.cdn.received_requests().await.unwrap_or_default().len()
    }

    pub async fn cdn_paths(&self) -> Vec<String> {
        self.cdn
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .map(|r| r.url.path().trim_start_matches('/').to_string())
            .collect()
    }
}

/// A 768-byte RGB frame filled with a deterministic pattern seeded by `seed`.
pub fn frame(seed: u8) -> Vec<u8> {
    (0..FRAME_BYTES)
        .map(|i| (i as u32 * 7 + seed as u32 * 13) as u8)
        .collect()
}

pub fn format9_fixture(frames: usize, duration_ms: u16, seed: u8) -> Vec<u8> {
    let frames: Vec<Vec<u8>> = (0..frames)
        .map(|i| frame(seed.wrapping_add(i as u8)))
        .collect();
    format9::encode(&frames, duration_ms).unwrap()
}

pub fn format17_fixture(seed: u8) -> Vec<u8> {
    format17::encode(&frame(seed), 1, 1).unwrap()
}
