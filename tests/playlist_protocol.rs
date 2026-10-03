mod common;

use common::*;
use divoom_export::api::playlists::PlaylistEndpoints;
use divoom_export::playlist::Visibility;
use serde_json::Value;

fn seed(fake: &FakeDivoom) {
    fake.with_state(|s| {
        for gid in 1..=3 {
            s.add_asset(
                gid,
                &format!("art {gid}"),
                format9_fixture(1, 100, gid as u8),
            );
        }
        s.playlists.push(FakePlaylist {
            id: 100,
            name: "Public test".into(),
            private: false,
            items: vec![3, 1, 2, 1],
        });
        s.playlists.push(FakePlaylist {
            id: 200,
            name: "Private test".into(),
            private: true,
            items: vec![2],
        });
    });
}

#[tokio::test]
async fn owner_listing_includes_private_and_public_playlists() {
    let fake = FakeDivoom::start().await;
    seed(&fake);
    let client = fake.client();
    let session = fake.session(&client).await;
    let owned = client
        .list_owned_playlists(&session, &fake.endpoints())
        .await
        .unwrap();
    let ids: Vec<u64> = owned.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![100, 200]);
    assert_eq!(owned[0].visibility, Visibility::Public);
    assert_eq!(owned[1].visibility, Visibility::Private);
    assert_eq!(owned[0].item_count, Some(4));
    assert_eq!(owned[0].name, "Public test");
    assert!(owned[0].cover_file_id.is_some());
}

#[tokio::test]
async fn public_listing_does_not_expose_private_playlist() {
    let fake = FakeDivoom::start().await;
    seed(&fake);
    let client = fake.client();
    let session = fake.session(&client).await;
    let public = client
        .list_public_albums(&session, &fake.endpoints(), USER_ID)
        .await
        .unwrap();
    assert_eq!(public.iter().map(|p| p.id).collect::<Vec<_>>(), vec![100]);
}

#[tokio::test]
async fn probe_distinguishes_owner_public_and_discover_listings() {
    let fake = FakeDivoom::start().await;
    seed(&fake);
    let client = fake.client();
    let session = fake.session(&client).await;
    let defaults = PlaylistEndpoints::default();
    assert!(!defaults.overridden);
    assert_eq!(defaults, fake.endpoints_unflagged());

    let report = client.probe_playlists(&session, &defaults).await;
    let discover = report
        .results
        .iter()
        .find(|r| r.role == "discover")
        .unwrap();
    assert!(discover.ok);
    assert_eq!(discover.playlist_count, 1);
    assert_eq!(discover.playlists[0].id, 112);
    assert!(!discover.private_playlist_visible);

    let verified = fake.endpoints();
    let report = client.probe_playlists(&session, &verified).await;
    let owner = report.results.iter().find(|r| r.role == "owner").unwrap();
    let public = report.results.iter().find(|r| r.role == "public").unwrap();
    assert!(owner.private_playlist_visible);
    assert!(!public.private_playlist_visible);
    assert_eq!(owner.playlist_count, 2);
    assert_eq!(public.playlist_count, 1);
    assert_eq!(
        owner.playlists[0].first_items_gallery_ids.as_deref(),
        Some(&[3u64, 1, 2, 1][..])
    );
    assert_eq!(
        report.items_request_schema,
        vec!["PlayId", "Version", "FileSize", "StartNum", "EndNum", "UserId", "Token"]
    );
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains(TOKEN));
    assert!(!json.contains("group1/"));
}

#[tokio::test]
async fn playlist_items_preserve_remote_order_and_duplicates() {
    let fake = FakeDivoom::start().await;
    seed(&fake);
    let client = fake.client();
    let session = fake.session(&client).await;
    let items = client
        .list_playlist_items(&session, &fake.endpoints(), 100)
        .await
        .unwrap();
    let ids: Vec<u64> = items.iter().map(|i| i.gallery_id).collect();
    assert_eq!(ids, vec![3, 1, 2, 1]);
    let positions: Vec<usize> = items.iter().map(|i| i.position).collect();
    assert_eq!(positions, vec![0, 1, 2, 3]);
    assert_eq!(items[0].title.as_deref(), Some("art 3"));
    assert_eq!(items[0].author_name.as_deref(), Some("artist"));
    assert!(items[0].file_id.as_deref().unwrap().starts_with("group1/"));

    let requests = fake.api.received_requests().await.unwrap();
    let req = requests
        .iter()
        .find(|r| r.url.path() == ITEMS_PATH)
        .unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["PlayId"], 100);
    assert_eq!(body["FileSize"], 0);
    assert_eq!(body["Version"], 12);
    assert!(body.get("SomeOneUserId").is_none());
    assert_eq!(body["StartNum"], 1);
    assert_eq!(body["EndNum"], 50);
    assert_eq!(body["UserId"], USER_ID);
    assert_eq!(body["Token"], TOKEN);
}

#[tokio::test]
async fn playlist_items_paginate_inclusively_without_duplicating_pages() {
    let fake = FakeDivoom::start().await;
    fake.with_state(|s| {
        for gid in 1..=130 {
            s.add_asset(gid, "x", vec![]);
        }
        s.playlists.push(FakePlaylist {
            id: 5,
            name: "Big".into(),
            private: false,
            items: (1..=130).collect(),
        });
    });
    let client = fake.client();
    let session = fake.session(&client).await;
    let items = client
        .list_playlist_items(&session, &fake.endpoints(), 5)
        .await
        .unwrap();
    assert_eq!(
        items.iter().map(|i| i.gallery_id).collect::<Vec<_>>(),
        (1..=130).collect::<Vec<u64>>()
    );
    assert_eq!(fake.api_calls(ITEMS_PATH).await, 3);
}

#[test]
fn endpoint_overrides_are_validated() {
    // Environment-driven configuration is process-global; keep it in a single
    // test and restore afterwards.
    std::env::set_var("DIVOOM_ENDPOINT_OWNER_PLAYLISTS", "/Owner/GetMyAlbumList");
    let e = PlaylistEndpoints::from_env().unwrap();
    assert!(e.overridden);
    assert_eq!(e.owner_list, "/Owner/GetMyAlbumList");
    assert_eq!(e.items, "/Playlist/GetMyImageList");
    std::env::set_var("DIVOOM_ENDPOINT_OWNER_PLAYLISTS", "https://evil/x");
    assert!(PlaylistEndpoints::from_env().is_err());
    std::env::remove_var("DIVOOM_ENDPOINT_OWNER_PLAYLISTS");
    assert!(!PlaylistEndpoints::from_env().unwrap().overridden);
}
