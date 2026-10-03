//! HTTP client for the undocumented Divoom account API and file CDN.
//!
//! All protocol constants (hosts, endpoint paths, user agent) are centralized
//! here. Requests are sequential; transient failures are retried with bounded
//! exponential backoff.

pub mod models;
pub mod playlists;

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::{debug, warn};

use crate::auth::{Credentials, Session};
use crate::error::{Error, Result};
use crate::naming::validate_file_id;
use models::*;

/// Default JSON API host.
pub const DEFAULT_API_BASE: &str = "https://app.divoom-gz.com";
/// Default file CDN host.
pub const DEFAULT_CDN_BASE: &str = "https://f.divoom-gz.com";
/// User agent expected by the account API.
pub const USER_AGENT: &str = "Aurabox/3.1.10 (iPad; iOS 14.8; Scale/2.00)";
/// Maximum accepted CDN download size.
pub const MAX_DOWNLOAD_BYTES: u64 = 16 * 1024 * 1024;

/// Endpoint paths.
pub mod endpoints {
    pub const USER_LOGIN: &str = "/UserLogin";
    pub const GALLERY_INFO: &str = "/Cloud/GalleryInfo";
    pub const CATEGORY_FILE_LIST: &str = "/GetCategoryFileListV2";
}

/// Protocol version sent in catalogue requests.
pub const CATEGORY_LIST_VERSION: i64 = 12;
/// `FileSize` value for 16×16 artwork.
pub const FILE_SIZE_16: i64 = 1;
/// `FileType` value meaning "all artwork types".
pub const FILE_TYPE_ALL: i64 = 5;

/// Client configuration.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub api_base: String,
    pub cdn_base: String,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// Maximum number of attempts for transient failures (>= 1).
    pub max_attempts: u32,
    /// Base delay of the exponential backoff (1 s in production).
    pub retry_base_delay: Duration,
    pub max_download_bytes: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            api_base: DEFAULT_API_BASE.into(),
            cdn_base: DEFAULT_CDN_BASE.into(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            max_attempts: 3,
            retry_base_delay: Duration::from_secs(1),
            max_download_bytes: MAX_DOWNLOAD_BYTES,
        }
    }
}

impl ClientConfig {
    /// Configuration pointing at custom hosts (tests, proxies).
    pub fn with_hosts(api_base: &str, cdn_base: &str) -> Self {
        Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            cdn_base: cdn_base.trim_end_matches('/').to_string(),
            ..Default::default()
        }
    }
}

/// Divoom API client.
#[derive(Debug, Clone)]
pub struct DivoomClient {
    http: reqwest::Client,
    config: ClientConfig,
}

impl DivoomClient {
    pub fn new(config: ClientConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| Error::Network(format!("cannot build HTTP client: {e}")))?;
        Ok(Self { http, config })
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.config.api_base.trim_end_matches('/'), path)
    }

    /// Authenticate and return an in-memory session. Never retried on a
    /// functional failure; the login payload is never logged.
    pub async fn login(&self, credentials: &Credentials) -> Result<Session> {
        let body = LoginRequest {
            email: &credentials.email,
            password_md5: &credentials.password_md5,
        };
        let response: LoginResponse = self.post_json(endpoints::USER_LOGIN, &body).await?;
        if response.return_code != 0 {
            return Err(Error::Authentication(format!(
                "login rejected with ReturnCode {}{}",
                response.return_code,
                response
                    .return_message
                    .filter(|m| !m.is_empty())
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            )));
        }
        match (response.user_id, response.token) {
            (Some(user_id), Some(token)) if !token.is_empty() => {
                debug!(user_id, "login succeeded");
                Ok(Session { user_id, token })
            }
            _ => Err(Error::Authentication(
                "login response lacks UserId or Token".into(),
            )),
        }
    }

    /// POST a JSON body and decode the JSON response. The request body is
    /// never logged. Transient HTTP failures are retried; functional
    /// `ReturnCode` values are not.
    pub async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = self.api_url(path);
        let payload = serde_json::to_vec(body)
            .map_err(|e| Error::InvalidMetadata(format!("cannot serialize request: {e}")))?;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            debug!(path, attempt, "POST");
            let result = self
                .http
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(payload.clone())
                .send()
                .await;
            match self.classify(result, attempt).await? {
                Outcome::Done(bytes) => {
                    let value: T = serde_json::from_slice(&bytes).map_err(|e| {
                        Error::InvalidMetadata(format!("invalid JSON from {path}: {e}"))
                    })?;
                    return Ok(value);
                }
                Outcome::Retry(delay) => tokio::time::sleep(delay).await,
            }
        }
    }

    /// POST and check the envelope `ReturnCode` before returning the typed
    /// payload. Non-zero codes become [`Error::Api`].
    pub async fn post_checked<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let raw: serde_json::Value = self.post_json(path, body).await?;
        let env: Envelope = serde_json::from_value(raw.clone())
            .map_err(|e| Error::InvalidMetadata(format!("invalid envelope from {path}: {e}")))?;
        if env.return_code != 0 {
            return Err(Error::Api {
                endpoint: path.to_string(),
                return_code: env.return_code,
                message: env.return_message,
            });
        }
        serde_json::from_value(raw)
            .map_err(|e| Error::InvalidMetadata(format!("invalid payload from {path}: {e}")))
    }

    /// Resolve one artwork's current metadata.
    pub async fn gallery_info(&self, session: &Session, gallery_id: u64) -> Result<GalleryInfo> {
        let body = GalleryInfoRequest {
            auth: session.auth(),
            gallery_id,
        };
        let info: GalleryInfo = self.post_checked(endpoints::GALLERY_INFO, &body).await?;
        Ok(info)
    }

    /// List a catalogue category with a finite `limit` (inclusive pagination
    /// windows of `page_size`). Stops on an empty page, a repeated page, a
    /// non-zero `ReturnCode`, or the limit.
    pub async fn list_category(
        &self,
        session: &Session,
        category: Category,
        sort: SortOrder,
        limit: usize,
        page_size: u64,
    ) -> Result<Vec<FileRecord>> {
        let mut out: Vec<FileRecord> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut start = 1u64;
        let page_size = page_size.max(1);
        while out.len() < limit {
            let body = CategoryListRequest {
                start_num: start,
                end_num: start + page_size - 1,
                classify: category.classify(),
                version: CATEGORY_LIST_VERSION,
                file_size: FILE_SIZE_16,
                file_type: FILE_TYPE_ALL,
                file_sort: sort.file_sort(),
                refresh_index: 0,
                auth: session.auth(),
            };
            let page: FileListResponse = self
                .post_checked(endpoints::CATEGORY_FILE_LIST, &body)
                .await?;
            if page.file_list.is_empty() {
                break;
            }
            let page_len = page.file_list.len() as u64;
            let mut added = 0usize;
            for record in page.file_list {
                if out.len() >= limit {
                    break;
                }
                let key = record.gallery_id;
                if let Some(id) = key {
                    if !seen.insert(id) {
                        continue;
                    }
                }
                out.push(record);
                added += 1;
            }
            if added == 0 || page_len < page_size {
                break;
            }
            start += page_size;
        }
        Ok(out)
    }

    /// Download one CDN binary by `FileId`, streaming with a size limit.
    pub async fn download_file(&self, file_id: &str) -> Result<Vec<u8>> {
        validate_file_id(file_id)?;
        let url = format!("{}/{}", self.config.cdn_base.trim_end_matches('/'), file_id);
        let limit = self.config.max_download_bytes;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            debug!(attempt, "GET CDN binary");
            let result = self.http.get(&url).send().await;
            let response = match result {
                Ok(r) => r,
                Err(e) => {
                    if attempt < self.config.max_attempts {
                        tokio::time::sleep(self.backoff(attempt, None)).await;
                        continue;
                    }
                    return Err(Error::Network(format!("download failed: {e}")));
                }
            };
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                if attempt < self.config.max_attempts {
                    let ra = retry_after(response.headers());
                    tokio::time::sleep(self.backoff(attempt, ra)).await;
                    continue;
                }
                return Err(if status == StatusCode::TOO_MANY_REQUESTS {
                    Error::RateLimited { attempts: attempt }
                } else {
                    Error::Network(format!("download failed with HTTP {status}"))
                });
            }
            if !status.is_success() {
                return Err(Error::Network(format!(
                    "download failed with HTTP {status}"
                )));
            }
            if let Some(len) = response.content_length() {
                if len > limit {
                    return Err(Error::DownloadTooLarge { limit });
                }
            }
            let mut stream = response.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            let mut stream_error = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(c) => {
                        if buf.len() as u64 + c.len() as u64 > limit {
                            return Err(Error::DownloadTooLarge { limit });
                        }
                        buf.extend_from_slice(&c);
                    }
                    Err(e) => {
                        stream_error = Some(e);
                        break;
                    }
                }
            }
            if let Some(e) = stream_error {
                if attempt < self.config.max_attempts {
                    tokio::time::sleep(self.backoff(attempt, None)).await;
                    continue;
                }
                return Err(Error::Network(format!("download interrupted: {e}")));
            }
            if buf.is_empty() {
                return Err(Error::InvalidContainer("empty download".into()));
            }
            return Ok(buf);
        }
    }

    async fn classify(
        &self,
        result: std::result::Result<reqwest::Response, reqwest::Error>,
        attempt: u32,
    ) -> Result<Outcome> {
        let can_retry = attempt < self.config.max_attempts;
        let response = match result {
            Ok(r) => r,
            Err(e) => {
                let msg = redact_reqwest_error(&e);
                if can_retry {
                    warn!(attempt, "transient network error: {msg}");
                    return Ok(Outcome::Retry(self.backoff(attempt, None)));
                }
                return Err(Error::Network(msg));
            }
        };
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            let ra = retry_after(response.headers());
            if can_retry {
                warn!(attempt, %status, "retrying after HTTP error");
                return Ok(Outcome::Retry(self.backoff(attempt, ra)));
            }
            return Err(if status == StatusCode::TOO_MANY_REQUESTS {
                Error::RateLimited { attempts: attempt }
            } else {
                Error::Network(format!("HTTP {status}"))
            });
        }
        if !status.is_success() {
            return Err(Error::Network(format!("HTTP {status}")));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| Error::Network(redact_reqwest_error(&e)))?;
        Ok(Outcome::Done(bytes.to_vec()))
    }

    /// Exponential backoff (base, 2×base, 4×base) with jitter, or the server's
    /// `Retry-After` when provided and reasonable.
    fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(ra) = retry_after {
            return ra.min(Duration::from_secs(120));
        }
        let base = self.config.retry_base_delay;
        let factor = 1u32 << (attempt.saturating_sub(1)).min(4);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let jitter = base.mul_f64((nanos % 500) as f64 / 1000.0);
        base * factor + jitter
    }
}

enum Outcome {
    Done(Vec<u8>),
    Retry(Duration),
}

impl Session {
    /// Authentication fields for request bodies.
    pub fn auth(&self) -> Auth<'_> {
        Auth {
            user_id: self.user_id,
            token: &self.token,
        }
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Format a transport error without the request URL (which could embed a
/// `FileId`) and without any body content.
fn redact_reqwest_error(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connection error"
    } else if e.is_body() || e.is_decode() {
        "body error"
    } else if e.is_request() {
        "request error"
    } else {
        "transport error"
    };
    let mut msg = kind.to_string();
    if let Some(status) = e.status() {
        msg.push_str(&format!(" (HTTP {status})"));
    }
    msg
}
