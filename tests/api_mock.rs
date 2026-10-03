mod common;

use common::*;
use divoom_export::api::models::{Category, SortOrder};
use divoom_export::api::{ClientConfig, DivoomClient};
use divoom_export::auth::{Credentials, Token};
use divoom_export::Error;
use serde_json::Value;

#[tokio::test]
async fn login_serializes_exact_payload_and_returns_session() {
    let fake = FakeDivoom::start().await;
    let client = fake.client();
    let session = fake.session(&client).await;
    assert_eq!(session.user_id, USER_ID);
    assert_eq!(session.token, Token::Text(TOKEN.into()));

    let requests = fake.api.received_requests().await.unwrap();
    let login = requests
        .iter()
        .find(|r| r.url.path() == "/UserLogin")
        .unwrap();
    let body: Value = serde_json::from_slice(&login.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"Email": EMAIL, "Password": PASSWORD_MD5})
    );
    let ua = login.headers.get("user-agent").unwrap().to_str().unwrap();
    assert_eq!(ua, divoom_export::api::USER_AGENT);
    assert_eq!(
        login.headers.get("content-type").unwrap().to_str().unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn login_failure_is_authentication_error_and_not_retried() {
    let fake = FakeDivoom::start().await;
    let client = fake.client();
    let creds = Credentials {
        email: EMAIL.into(),
        password_md5: "0".repeat(32),
    };
    let err = client.login(&creds).await.unwrap_err();
    assert!(matches!(err, Error::Authentication(_)));
    assert_eq!(err.exit_code(), 3);
    assert_eq!(fake.api_calls("/UserLogin").await, 1);
    assert!(!err.to_string().contains(&"0".repeat(32)));
}

#[tokio::test]
async fn authenticated_payload_includes_user_id_and_token() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| {
        s.add_asset(10, "Cat", format9_fixture(1, 100, 0));
    });
    let client = fake.client();
    let session = fake.session(&client).await;
    let info = client.gallery_info(&session, 10).await.unwrap();
    assert_eq!(info.file_name.as_deref(), Some("Cat"));
    assert_eq!(info.extra.get("SomeFutureField"), Some(&Value::from("x")));

    let requests = fake.api.received_requests().await.unwrap();
    let req = requests
        .iter()
        .find(|r| r.url.path() == "/Cloud/GalleryInfo")
        .unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["UserId"], USER_ID);
    assert_eq!(body["Token"], TOKEN);
    assert_eq!(body["GalleryId"], 10);
}

#[tokio::test]
async fn non_zero_return_code_is_api_error_and_not_retried() {
    let fake = FakeDivoom::start().await;
    let client = fake.client();
    let session = fake.session(&client).await;
    let err = client.gallery_info(&session, 999).await.unwrap_err();
    match err {
        Error::Api {
            return_code,
            ref endpoint,
            ..
        } => {
            assert_eq!(return_code, 2);
            assert_eq!(endpoint, "/Cloud/GalleryInfo");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(fake.api_calls("/Cloud/GalleryInfo").await, 1);
}

#[tokio::test]
async fn transient_http_errors_are_retried_up_to_three_times() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| {
        s.add_asset(10, "Cat", format9_fixture(1, 100, 0));
        s.flaky_api.insert("/Cloud/GalleryInfo".into(), 2);
    });
    let client = fake.client();
    let session = fake.session(&client).await;
    assert!(client.gallery_info(&session, 10).await.is_ok());
    assert_eq!(fake.api_calls("/Cloud/GalleryInfo").await, 3);

    fake.with_state(|s| {
        s.flaky_api.insert("/Cloud/GalleryInfo".into(), 3);
    });
    fake.reset_requests().await;
    let err = client.gallery_info(&session, 10).await.unwrap_err();
    assert!(matches!(err, Error::Network(_)));
    assert_eq!(fake.api_calls("/Cloud/GalleryInfo").await, 3);
}

#[tokio::test]
async fn rate_limit_honours_retry_after_then_fails() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| {
        s.add_asset(10, "Cat", format9_fixture(1, 100, 0));
        s.rate_limit_api.insert("/Cloud/GalleryInfo".into(), 5);
    });
    let client = fake.client();
    let session = fake.session(&client).await;
    let err = client.gallery_info(&session, 10).await.unwrap_err();
    assert!(matches!(err, Error::RateLimited { attempts: 3 }));
}

#[tokio::test]
async fn catalogue_pagination_is_inclusive_without_duplicates_and_bounded() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| s.catalog = (1..=120).collect());
    let client = fake.client();
    let session = fake.session(&client).await;

    let items = client
        .list_category(&session, Category::Top, SortOrder::New, 75, 50)
        .await
        .unwrap();
    assert_eq!(items.len(), 75);
    let ids: Vec<u64> = items.iter().map(|i| i.gallery_id.unwrap()).collect();
    assert_eq!(ids, (1..=75).collect::<Vec<u64>>());

    let requests = fake.api.received_requests().await.unwrap();
    let pages: Vec<(u64, u64)> = requests
        .iter()
        .filter(|r| r.url.path() == "/GetCategoryFileListV2")
        .map(|r| {
            let b: Value = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(b["Classify"], 14);
            assert_eq!(b["FileSize"], 1);
            assert_eq!(b["FileType"], 5);
            assert_eq!(b["FileSort"], 0);
            assert_eq!(b["Version"], 12);
            (
                b["StartNum"].as_u64().unwrap(),
                b["EndNum"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(pages, vec![(1, 50), (51, 100)]);

    // All 120 items with a generous limit: exactly three pages.
    fake.reset_requests().await;
    let items = client
        .list_category(&session, Category::Animal, SortOrder::Liked, 500, 50)
        .await
        .unwrap();
    assert_eq!(items.len(), 120);
    assert_eq!(fake.api_calls("/GetCategoryFileListV2").await, 3);
}

#[tokio::test]
async fn catalogue_stops_on_repeated_page() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| {
        s.catalog = (1..=50).collect();
        s.catalog_repeat_pages = true;
    });
    let client = fake.client();
    let session = fake.session(&client).await;
    let items = client
        .list_category(&session, Category::Top, SortOrder::New, 500, 50)
        .await
        .unwrap();
    assert_eq!(items.len(), 50);
    assert_eq!(fake.api_calls("/GetCategoryFileListV2").await, 2);
}

#[tokio::test]
async fn dangerous_file_ids_are_rejected_before_any_request() {
    let fake = FakeDivoom::start().await;
    let client = fake.client();
    for bad in [
        "",
        "http://example.com/x.dat",
        "//example.com/x.dat",
        "/etc/passwd",
        "group1/../../secret",
        "group1\\x.dat",
        "group1/x y.dat",
    ] {
        let err = client.download_file(bad).await.unwrap_err();
        assert!(matches!(err, Error::InvalidMetadata(_)), "{bad:?}: {err}");
    }
    assert_eq!(fake.cdn_downloads().await, 0);
}

#[tokio::test]
async fn download_enforces_size_limit_and_retries() {
    let fake = FakeDivoom::start().await;
    let big = vec![9u8; 1024];
    fake.with_state(|s| {
        s.files.insert("group1/big.dat".into(), big.clone());
        s.files.insert("group1/ok.dat".into(), vec![1, 2, 3]);
        s.flaky_download.insert("group1/ok.dat".into(), 1);
        s.fail_download.insert("group1/dead.dat".into());
    });
    let client = DivoomClient::new(ClientConfig {
        max_download_bytes: 512,
        ..fake.config()
    })
    .unwrap();
    let err = client.download_file("group1/big.dat").await.unwrap_err();
    assert!(matches!(err, Error::DownloadTooLarge { limit: 512 }));

    assert_eq!(
        client.download_file("group1/ok.dat").await.unwrap(),
        vec![1, 2, 3]
    );
    let err = client.download_file("group1/dead.dat").await.unwrap_err();
    assert!(matches!(err, Error::Network(_)));
    assert_eq!(
        fake.cdn_paths()
            .await
            .iter()
            .filter(|p| p.ends_with("dead.dat"))
            .count(),
        3
    );
}

#[tokio::test]
async fn errors_and_debug_output_never_contain_secrets() {
    let fake = FakeDivoom::start().await;
    let client = fake.client();
    let session = fake.session(&client).await;
    let err = client.gallery_info(&session, 999).await.unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains(TOKEN));
    assert!(!text.contains(PASSWORD_MD5));
    assert!(!format!("{session:?}").contains(TOKEN));
    assert!(!format!("{:?}", fake.credentials()).contains(PASSWORD_MD5));
}

#[tokio::test]
async fn numeric_token_is_accepted_and_echoed_as_integer() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| {
        s.numeric_token = true;
        s.add_asset(10, "Cat", format9_fixture(1, 100, 0));
    });
    let client = fake.client();
    let session = fake.session(&client).await;
    assert_eq!(session.token, Token::Number(TOKEN_NUMBER));
    client.gallery_info(&session, 10).await.unwrap();
    let requests = fake.api.received_requests().await.unwrap();
    let req = requests
        .iter()
        .find(|r| r.url.path() == "/Cloud/GalleryInfo")
        .unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["Token"], Value::from(TOKEN_NUMBER));
}

#[tokio::test]
async fn fetch_artwork_needs_no_login_and_decodes() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| {
        s.files
            .insert("group1/M00/AA/cover.dat".into(), format9_fixture(3, 90, 2));
        s.files
            .insert("group1/M00/AA/bad.dat".into(), vec![42, 0, 0, 0]);
    });
    let client = fake.client();
    let (bytes, art) = divoom_export::sync::fetch_artwork(&client, "group1/M00/AA/cover.dat")
        .await
        .unwrap();
    assert_eq!(bytes[0], 9);
    assert_eq!(art.frames.len(), 3);
    assert_eq!(fake.api_calls("/UserLogin").await, 0);
    let err = divoom_export::sync::fetch_artwork(&client, "group1/M00/AA/bad.dat")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::UnsupportedFormat(42)));
    let err = divoom_export::sync::fetch_artwork(&client, "../x")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidMetadata(_)));
}
