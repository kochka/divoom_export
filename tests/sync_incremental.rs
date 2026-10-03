mod common;

use std::path::Path;

use common::*;
use divoom_export::api::DivoomClient;
use divoom_export::auth::Session;
use divoom_export::manifest::{Library, StateManifest};
use divoom_export::sync::{run_sync, AssetAction, Selector, SyncContext, SyncOptions, SyncReport};
use divoom_export::Error;

struct Harness {
    fake: FakeDivoom,
    client: DivoomClient,
    session: Session,
    dir: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Self {
        let fake = FakeDivoom::start().await;
        let client = fake.client();
        let session = fake.session(&client).await;
        Self {
            fake,
            client,
            session,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn library(&self) -> Library {
        Library::new(self.root())
    }

    fn state(&self) -> StateManifest {
        self.library().load_state().unwrap()
    }

    async fn sync(&self, selector: Selector, options: &SyncOptions) -> Result<SyncReport, Error> {
        let endpoints = self.fake.endpoints();
        let ctx = SyncContext {
            client: &self.client,
            session: &self.session,
            endpoints: &endpoints,
            library: self.library(),
        };
        run_sync(&ctx, selector, options).await
    }

    async fn sync_all(&self) -> SyncReport {
        self.fake.reset_requests().await;
        self.sync(Selector::All, &SyncOptions::default())
            .await
            .unwrap()
    }

    fn webp_of(&self, gid: u64) -> Vec<u8> {
        let st = self.state();
        let a = st.asset(gid).expect("asset in state");
        std::fs::read(self.root().join(&a.webp_path)).expect("webp file")
    }
}

fn seed_basic(fake: &FakeDivoom) {
    fake.with_state(|s| {
        s.add_asset(1, "Space Cat", format9_fixture(3, 100, 1));
        s.add_asset(2, "Ghost", format17_fixture(2));
        s.add_asset(3, "Dog", format9_fixture(1, 500, 3));
        s.playlists.push(FakePlaylist {
            id: 123,
            name: "Favourites".into(),
            private: true,
            items: vec![1, 2, 1],
        });
        s.playlists.push(FakePlaylist {
            id: 456,
            name: "Seasonal".into(),
            private: false,
            items: vec![2, 3],
        });
    });
}

#[tokio::test]
async fn first_sync_downloads_every_unique_asset_once() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    let report = h.sync_all().await;
    assert_eq!(
        report.summary_line(),
        "0 unchanged, 3 downloaded, 0 rebuilt, 0 pruned, 0 failed"
    );
    assert_eq!(h.fake.cdn_downloads().await, 3);
    assert_eq!(h.fake.api_calls("/Cloud/GalleryInfo").await, 3);

    let root = h.root();
    assert!(root.join("state.json").is_file());
    assert!(root.join("assets/1-space-cat/artwork.webp").is_file());
    assert!(root.join("assets/1-space-cat/source.dat").is_file());
    assert!(root.join("assets/1-space-cat/metadata.json").is_file());
    assert!(root.join("assets/2-ghost/artwork.webp").is_file());
    assert!(root.join("playlists/123-favourites.json").is_file());
    assert!(root.join("playlists/456-seasonal.json").is_file());

    let st = h.state();
    assert_eq!(st.playlist(123).unwrap().ordered_gallery_ids, vec![1, 2, 1]);
    let a = st.asset(1).unwrap();
    assert_eq!(a.format, 9);
    assert_eq!(a.frame_count, 3);
    assert_eq!(a.frame_duration_ms, 100);
    assert_eq!(a.author_name.as_deref(), Some("artist"));

    let m = h
        .library()
        .load_playlist_manifest("playlists/123-favourites.json")
        .unwrap();
    assert_eq!(m.name, "Favourites");
    assert_eq!(m.visibility, divoom_export::playlist::Visibility::Private);
    let ids: Vec<u64> = m.items.iter().map(|i| i.gallery_id).collect();
    assert_eq!(ids, vec![1, 2, 1]);
    assert_eq!(
        m.items[0].asset_path.as_deref(),
        Some("../assets/1-space-cat/artwork.webp")
    );
    assert_eq!(
        m.items[2].asset_path.as_deref(),
        Some("../assets/1-space-cat/artwork.webp")
    );

    let text = std::fs::read_to_string(root.join("state.json")).unwrap();
    assert!(!text.contains(TOKEN) && !text.contains(PASSWORD_MD5) && !text.contains(EMAIL));
}

#[tokio::test]
async fn unchanged_second_sync_makes_no_cdn_request() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    let report = h.sync_all().await;
    assert_eq!(report.unchanged, 3);
    assert_eq!(report.downloaded, 0);
    assert_eq!(h.fake.cdn_downloads().await, 0);
}

#[tokio::test]
async fn changed_file_id_triggers_replacement() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    let before = h.webp_of(1);
    h.fake.with_state(|s| {
        s.replace_asset(1, format9_fixture(2, 200, 50));
    });
    let report = h.sync_all().await;
    assert_eq!((report.downloaded, report.unchanged), (1, 2));
    assert_eq!(h.fake.cdn_downloads().await, 1);
    let after = h.webp_of(1);
    assert_ne!(before, after);
    let st = h.state();
    assert_eq!(st.asset(1).unwrap().frame_count, 2);
    assert!(st.asset(1).unwrap().file_id.contains("v2-"));
}

#[tokio::test]
async fn missing_webp_with_valid_raw_rebuilds_locally() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    let original = h.webp_of(2);
    std::fs::remove_file(h.root().join("assets/2-ghost/artwork.webp")).unwrap();
    let report = h.sync_all().await;
    assert_eq!((report.rebuilt, report.unchanged), (1, 2));
    assert_eq!(h.fake.cdn_downloads().await, 0);
    assert_eq!(h.webp_of(2), original);

    // Corrupt WebP with valid raw: also rebuilt without downloads.
    std::fs::write(h.root().join("assets/2-ghost/artwork.webp"), b"junk").unwrap();
    let report = h.sync_all().await;
    assert_eq!(report.rebuilt, 1);
    assert_eq!(h.fake.cdn_downloads().await, 0);

    // Missing WebP and corrupt raw: redownloaded.
    std::fs::remove_file(h.root().join("assets/2-ghost/artwork.webp")).unwrap();
    std::fs::write(h.root().join("assets/2-ghost/source.dat"), b"junk").unwrap();
    let report = h.sync_all().await;
    assert_eq!(report.downloaded, 1);
    assert_eq!(h.fake.cdn_downloads().await, 1);
    assert_eq!(h.webp_of(2), original);
}

#[tokio::test]
async fn failed_download_preserves_previous_output() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    let before = h.webp_of(1);
    let before_state = h.state().asset(1).cloned().unwrap();
    h.fake.with_state(|s| {
        let fid = s.replace_asset(1, format9_fixture(2, 200, 50));
        s.fail_download.insert(fid);
    });
    let report = h.sync_all().await;
    assert_eq!(report.failed, 1);
    assert!(!report.is_success());
    assert_eq!(h.webp_of(1), before);
    assert_eq!(h.state().asset(1).unwrap(), &before_state);

    // Corrupt replacement content: decode fails, old output preserved.
    h.fake.with_state(|s| {
        s.replace_asset(1, vec![9, 3, 0, 100, 1, 2, 3]);
    });
    let report = h.sync_all().await;
    assert_eq!(report.failed, 1);
    assert_eq!(h.webp_of(1), before);

    // Metadata unavailable: output preserved, failure recorded.
    h.fake.with_state(|s| {
        s.fail_gallery.insert(1);
    });
    let report = h.sync_all().await;
    assert_eq!(report.failed, 1);
    assert_eq!(h.webp_of(1), before);
    let a = &report.assets.iter().find(|a| a.gallery_id == 1).unwrap();
    assert_eq!(a.action, AssetAction::Failed);
    assert!(a.error.as_ref().unwrap().contains("ReturnCode 1"));
}

#[tokio::test]
async fn one_failure_does_not_stop_later_items_unless_fail_fast() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.fake.with_state(|s| {
        s.fail_gallery.insert(1);
    });
    let report = h.sync_all().await;
    assert_eq!((report.failed, report.downloaded), (1, 2));
    assert_eq!(report.playlists[0].failed_items, 2);
    let m = h
        .library()
        .load_playlist_manifest("playlists/123-favourites.json")
        .unwrap();
    assert!(m.items[0].asset_path.is_none());
    assert!(m.items[1].asset_path.is_some());
    assert!(h.state().asset(1).is_none());

    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.fake.with_state(|s| {
        s.fail_gallery.insert(1);
    });
    let report = h
        .sync(
            Selector::All,
            &SyncOptions {
                fail_fast: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.downloaded, 0);
    assert!(!h.root().join("playlists/123-favourites.json").exists());
    assert!(h.state().playlists.is_empty());
}

#[tokio::test]
async fn dry_run_performs_no_writes_or_downloads() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    let report = h
        .sync(
            Selector::All,
            &SyncOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(report.dry_run);
    assert_eq!(report.downloaded, 3);
    assert_eq!(h.fake.cdn_downloads().await, 0);
    assert!(std::fs::read_dir(h.root()).unwrap().next().is_none());
}

#[tokio::test]
async fn item_addition_downloads_only_the_added_asset() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    h.fake.with_state(|s| {
        s.add_asset(4, "New", format9_fixture(1, 100, 4));
        s.playlist_mut(123).items.push(4);
    });
    let report = h.sync_all().await;
    assert_eq!((report.downloaded, report.unchanged), (1, 3));
    assert_eq!(
        h.fake.cdn_paths().await,
        vec!["group1/M00/4/new.dat".to_string()]
    );
    assert_eq!(
        h.state().playlist(123).unwrap().ordered_gallery_ids,
        vec![1, 2, 1, 4]
    );
}

#[tokio::test]
async fn item_removal_deletes_nothing_without_prune() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    h.fake.with_state(|s| {
        s.playlist_mut(456).items = vec![2];
    });
    let report = h.sync_all().await;
    assert_eq!(report.pruned, 0);
    assert!(h.root().join("assets/3-dog/artwork.webp").is_file());
    assert!(h.state().asset(3).is_some());
    assert_eq!(
        h.state().playlist(456).unwrap().ordered_gallery_ids,
        vec![2]
    );
}

#[tokio::test]
async fn reordering_downloads_nothing_and_preserves_new_order() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    h.fake.with_state(|s| {
        s.playlist_mut(123).items = vec![2, 1, 1];
    });
    let report = h.sync_all().await;
    assert_eq!(report.downloaded, 0);
    assert_eq!(h.fake.cdn_downloads().await, 0);
    assert_eq!(
        h.state().playlist(123).unwrap().ordered_gallery_ids,
        vec![2, 1, 1]
    );
    let m = h
        .library()
        .load_playlist_manifest("playlists/123-favourites.json")
        .unwrap();
    assert_eq!(
        m.items.iter().map(|i| i.gallery_id).collect::<Vec<_>>(),
        vec![2, 1, 1]
    );
    assert_eq!(
        m.items.iter().map(|i| i.position).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[tokio::test]
async fn shared_and_duplicate_entries_store_one_asset() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    let report = h.sync_all().await;
    // Gallery 2 appears in both playlists, gallery 1 twice in one playlist.
    assert_eq!(report.downloaded, 3);
    let paths = h.fake.cdn_paths().await;
    assert_eq!(paths.iter().filter(|p| p.contains("/2/")).count(), 1);
    assert_eq!(paths.iter().filter(|p| p.contains("/1/")).count(), 1);
    assert_eq!(h.state().assets.len(), 3);
    let dirs: Vec<_> = std::fs::read_dir(h.root().join("assets"))
        .unwrap()
        .collect();
    assert_eq!(dirs.len(), 3);
}

#[tokio::test]
async fn prune_removes_only_unreferenced_managed_assets() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    std::fs::write(h.root().join("assets/3-dog/notes.txt"), b"keep me").unwrap();
    h.fake.with_state(|s| {
        s.playlist_mut(456).items = vec![2];
    });
    h.fake.reset_requests().await;
    let report = h
        .sync(
            Selector::All,
            &SyncOptions {
                prune: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.pruned, 1);
    assert!(!h.root().join("assets/3-dog/artwork.webp").exists());
    assert!(!h.root().join("assets/3-dog/source.dat").exists());
    assert!(h.root().join("assets/3-dog/notes.txt").is_file());
    assert!(h.state().asset(3).is_none());
    // Gallery 2 is still referenced by playlist 123: retained.
    assert!(h.root().join("assets/2-ghost/artwork.webp").is_file());
    assert!(h.state().asset(2).is_some());
}

#[tokio::test]
async fn prune_is_skipped_when_sync_has_failures() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    h.fake.with_state(|s| {
        s.playlist_mut(456).items = vec![2];
        s.fail_gallery.insert(1);
    });
    let report = h
        .sync(
            Selector::All,
            &SyncOptions {
                prune: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.pruned, 0);
    assert!(h.root().join("assets/3-dog/artwork.webp").is_file());
}

#[tokio::test]
async fn single_playlist_sync_keeps_other_playlists_state() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    h.fake.with_state(|s| {
        s.playlists.retain(|p| p.id != 456);
    });
    h.fake.reset_requests().await;
    let report = h
        .sync(Selector::One("Favourites".into()), &SyncOptions::default())
        .await
        .unwrap();
    assert_eq!(report.playlists.len(), 1);
    assert!(report.removed_playlists.is_empty());
    assert!(h.state().playlist(456).is_some());
    assert!(h.root().join("playlists/456-seasonal.json").is_file());
    assert_eq!(h.fake.api_calls("/Cloud/GalleryInfo").await, 2);

    let by_id = h
        .sync(Selector::One("123".into()), &SyncOptions::default())
        .await
        .unwrap();
    assert_eq!(by_id.playlists[0].playlist_id, 123);
    let err = h
        .sync(Selector::One("Nope".into()), &SyncOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PlaylistSelection(_)));
    assert_eq!(err.exit_code(), 2);
}

#[tokio::test]
async fn sync_all_reconciles_deleted_remote_playlists_and_renames() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    h.fake.with_state(|s| {
        s.playlists.retain(|p| p.id != 456);
        let p = s.playlist_mut(123);
        p.name = "Best Of".into();
        p.private = false;
    });
    let report = h.sync_all().await;
    assert_eq!(report.removed_playlists, vec![456]);
    assert_eq!(report.downloaded, 0);
    assert!(h.state().playlist(456).is_none());
    assert!(!h.root().join("playlists/456-seasonal.json").exists());
    assert!(!h.root().join("playlists/123-favourites.json").exists());
    let m = h
        .library()
        .load_playlist_manifest("playlists/123-best-of.json")
        .unwrap();
    assert_eq!(m.name, "Best Of");
    assert_eq!(m.visibility, divoom_export::playlist::Visibility::Public);
    assert_eq!(
        h.state().playlist(123).unwrap().manifest_path,
        "playlists/123-best-of.json"
    );
    // Asset 3 is unreferenced now but retained without --prune.
    assert!(h.root().join("assets/3-dog/artwork.webp").is_file());
}

#[tokio::test]
async fn no_raw_discards_source_after_validation() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    let opts = SyncOptions {
        no_raw: true,
        ..Default::default()
    };
    h.sync(Selector::All, &opts).await.unwrap();
    assert!(!h.root().join("assets/1-space-cat/source.dat").exists());
    assert!(h.root().join("assets/1-space-cat/artwork.webp").is_file());
    assert!(h.state().asset(1).unwrap().raw_path.is_none());
    // Without raw, a missing WebP must be redownloaded.
    std::fs::remove_file(h.root().join("assets/1-space-cat/artwork.webp")).unwrap();
    h.fake.reset_requests().await;
    let report = h.sync(Selector::All, &opts).await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(h.fake.cdn_downloads().await, 1);
}

#[tokio::test]
async fn refresh_redownloads_everything() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.sync_all().await;
    h.fake.reset_requests().await;
    let report = h
        .sync(
            Selector::All,
            &SyncOptions {
                refresh: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.downloaded, 3);
    assert_eq!(h.fake.cdn_downloads().await, 3);
}

#[tokio::test]
async fn empty_selection_is_no_synchronizable_items() {
    let h = Harness::new().await;
    h.fake.with_state(|s| {
        s.playlists.push(FakePlaylist {
            id: 1,
            name: "Empty".into(),
            private: false,
            items: vec![],
        });
    });
    let err = h
        .sync(Selector::All, &SyncOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NoSynchronizableItems));
    assert_eq!(err.exit_code(), 4);
}

#[tokio::test]
async fn manifest_stays_valid_after_interruption_and_partial_state() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    // First playlist succeeds, then the second one's new asset fails on CDN.
    h.fake.with_state(|s| {
        let fid = s.gallery[&3].file_id.clone();
        s.fail_download.insert(fid);
    });
    let report = h.sync_all().await;
    assert_eq!((report.downloaded, report.failed), (2, 1));
    let st = h.state();
    st.validate().unwrap();
    assert!(st.playlist(123).is_some());
    assert!(st.playlist(456).is_some());
    assert!(st.asset(3).is_none());
    // No temporary files are left behind anywhere.
    fn walk(p: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            if e.path().is_dir() {
                walk(&e.path(), out);
            } else {
                out.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    let mut names = Vec::new();
    walk(h.root(), &mut names);
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
    // Resuming completes the job without touching the finished assets.
    h.fake.with_state(|s| s.fail_download.clear());
    let report = h.sync_all().await;
    assert_eq!((report.downloaded, report.unchanged), (1, 2));
    assert_eq!(h.fake.cdn_downloads().await, 1);
}

#[tokio::test]
async fn webp_directory_holds_normalized_relative_symlinks() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.fake.with_state(|s| {
        // Same title as gallery 1 and an untitled artwork.
        s.add_asset(4, "Space  Cat!", format9_fixture(1, 100, 4));
        s.add_asset(5, "", format9_fixture(1, 100, 5));
        s.playlist_mut(456).items.extend([4, 5]);
    });
    let report = h.sync_all().await;
    assert_eq!(report.failed, 0);
    assert_eq!(report.linked, 5);
    let dir = h.root().join("webp");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "dog.webp",
            "gallery_5.webp",
            "ghost.webp",
            "space_cat.webp",
            "space_cat_1.webp"
        ]
    );
    // Links are relative, resolve to the asset, and the lower GalleryId wins the bare name.
    let target = std::fs::read_link(dir.join("space_cat.webp")).unwrap();
    assert_eq!(target, Path::new("../assets/1-space-cat/artwork.webp"));
    assert_eq!(
        std::fs::read(dir.join("space_cat_1.webp")).unwrap(),
        std::fs::read(h.root().join("assets/4-space-cat/artwork.webp")).unwrap()
    );
    assert_eq!(std::fs::read(dir.join("dog.webp")).unwrap(), h.webp_of(3));

    // Removing an item drops its link; regular files in the directory survive.
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    h.fake.with_state(|s| {
        s.playlist_mut(456).items = vec![2];
    });
    let report = h.sync_all().await;
    assert_eq!(report.linked, 2);
    assert!(!dir.join("dog.webp").exists());
    assert!(dir.join("ghost.webp").exists());
    assert!(dir.join("notes.txt").is_file());

    // Dry run touches nothing.
    std::fs::remove_file(dir.join("ghost.webp")).unwrap();
    let report = h
        .sync(
            Selector::All,
            &SyncOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.linked, 0);
    assert!(!dir.join("ghost.webp").exists());
}

#[tokio::test]
async fn announced_count_above_listed_items_is_reported_not_fatal() {
    let h = Harness::new().await;
    seed_basic(&h.fake);
    h.fake.with_state(|s| s.count_inflation = 3);
    let report = h.sync_all().await;
    assert_eq!(report.failed, 0);
    let p = report
        .playlists
        .iter()
        .find(|p| p.playlist_id == 123)
        .unwrap();
    assert_eq!(p.item_count, 3);
    assert_eq!(p.announced_count, Some(6));
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["playlists"][0]["announced_count"], 6);
}
