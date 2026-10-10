//! Offline integration tests: build a torrent from local files, then let the
//! client adopt the existing data (no network: DHT, listeners, LSD all off).

use std::path::Path;
use std::time::{Duration, Instant};

use librqbit::spawn_utils::BlockingSpawner;
use librqbit::{CreateTorrentOptions, create_torrent};
use nikgob_torrent::{
    NetOptions, TorrentClient, TorrentConfig, TorrentFile, TorrentPayload, TorrentState,
    TorrentStatus,
};

const OFFLINE: NetOptions = NetOptions {
    dht: false,
    listen: false,
    upnp: false,
    local_discovery: false,
};

fn config(root: &Path) -> TorrentConfig {
    TorrentConfig {
        downloads_dir: root.join("downloads"),
        state_dir: root.join("state"),
        no_seed: false,
        seed_ratio: 0.0,
        seed_days: 0,
        seed_limit_gb: 0,
        download_limit_kbps: 0,
        upload_limit_kbps: 0,
    }
}

fn payload_bytes(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

async fn make_torrent(path: &Path) -> Vec<u8> {
    let opts = CreateTorrentOptions {
        piece_length: Some(16384 * 2),
        ..Default::default()
    };
    let t = create_torrent(path, opts, &BlockingSpawner::new(1))
        .await
        .unwrap();
    t.as_bytes().unwrap().to_vec()
}

async fn wait_for(
    client: &TorrentClient,
    what: &str,
    pred: impl Fn(&TorrentStatus) -> bool,
) -> TorrentStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(s) = client.list().into_iter().find(|s| pred(s)) {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            client.list()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn single_file_existing_data_seeds_and_policy_removes_it() {
    let root = tempfile::tempdir().unwrap();
    let cfg = config(root.path());
    std::fs::create_dir_all(&cfg.downloads_dir).unwrap();
    let file = cfg.downloads_dir.join("album.flac");
    std::fs::write(&file, payload_bytes(200_000, 1)).unwrap();
    let torrent = make_torrent(&file).await;

    let client = TorrentClient::start_with(cfg.clone(), OFFLINE)
        .await
        .unwrap();
    let meta = client
        .inspect(TorrentPayload::TorrentFile(torrent.clone()))
        .await
        .unwrap();
    assert_eq!(meta.name, "album.flac");
    assert_eq!(meta.info_hash.len(), 40);
    assert_eq!(
        meta.files,
        vec![TorrentFile {
            index: 0,
            path: "album.flac".into(),
            size: 200_000
        }]
    );
    assert!(client.list().is_empty(), "inspect must not add the torrent");

    let id = client
        .add(TorrentPayload::TorrentFile(torrent.clone()), None)
        .await
        .unwrap();
    // Adding the same hash again returns the same id.
    assert_eq!(
        client
            .add(TorrentPayload::TorrentFile(torrent), None)
            .await
            .unwrap(),
        id
    );

    let st = wait_for(&client, "Seeding", |s| s.state == TorrentState::Seeding).await;
    assert_eq!(st.id, id);
    assert_eq!(st.info_hash, meta.info_hash);
    assert_eq!(st.total, 200_000);
    assert_eq!(st.downloaded, 200_000);
    assert!((st.progress - 1.0).abs() < f32::EPSILON);
    assert!(st.completed_at.is_some());
    assert_eq!(st.selected_files, vec![file.clone()]);

    // pause / resume round trip
    client.pause(id).await.unwrap();
    wait_for(&client, "Paused", |s| s.state == TorrentState::Paused).await;
    client.resume(id).await.unwrap();
    wait_for(&client, "Seeding again", |s| {
        s.state == TorrentState::Seeding
    })
    .await;

    // Nothing to stop with the default policy.
    assert!(client.enforce_seed_policy().await.is_empty());

    // "no_seed": removed from the session, files kept.
    client.set_config(TorrentConfig {
        no_seed: true,
        ..cfg
    });
    let stopped = client.enforce_seed_policy().await;
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].id, id);
    assert_eq!(stopped[0].selected_files, vec![file.clone()]);
    assert!(client.list().is_empty());
    assert!(file.exists());

    client.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_file_partial_selection_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let cfg = config(root.path());
    let album = cfg.downloads_dir.join("Album");
    std::fs::create_dir_all(&album).unwrap();
    std::fs::write(album.join("01 One.flac"), payload_bytes(100_000, 2)).unwrap();
    std::fs::write(album.join("02 Two.flac"), payload_bytes(150_000, 3)).unwrap();
    let torrent = make_torrent(&album).await;

    let client = TorrentClient::start_with(cfg.clone(), OFFLINE)
        .await
        .unwrap();
    let meta = client
        .inspect(TorrentPayload::TorrentFile(torrent.clone()))
        .await
        .unwrap();
    assert_eq!(meta.name, "Album");
    let two = meta
        .files
        .iter()
        .find(|f| f.path == "02 Two.flac")
        .expect("file listed");
    assert_eq!(two.size, 150_000);

    let id = client
        .add(
            TorrentPayload::TorrentFile(torrent.clone()),
            Some(vec![two.index]),
        )
        .await
        .unwrap();
    let st = wait_for(&client, "Seeding", |s| s.state == TorrentState::Seeding).await;
    assert_eq!(st.total, 150_000, "total counts selected bytes only");
    assert_eq!(st.selected_files, vec![album.join("02 Two.flac")]);
    let completed_at = st.completed_at.unwrap();

    // Widening the selection keeps the id and grows the total.
    assert_eq!(
        client
            .add(TorrentPayload::TorrentFile(torrent), None)
            .await
            .unwrap(),
        id
    );
    let st = wait_for(&client, "full selection", |s| s.total == 250_000).await;
    assert_eq!(st.selected_files.len(), 2);

    client.shutdown().await;
    drop(client);

    // Restart: the torrent and its id come back from persistence.
    let client = TorrentClient::start_with(cfg, OFFLINE).await.unwrap();
    let st = wait_for(&client, "restored Seeding", |s| {
        s.state == TorrentState::Seeding
    })
    .await;
    assert_eq!(st.id, id);
    assert_eq!(st.total, 250_000);
    assert!(st.completed_at.is_some());
    assert!(st.completed_at.unwrap() >= completed_at);

    client.remove(id, false).await.unwrap();
    assert!(client.list().is_empty());
    assert!(album.join("01 One.flac").exists());
    client.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_id_is_not_found() {
    let root = tempfile::tempdir().unwrap();
    let client = TorrentClient::start_with(config(root.path()), OFFLINE)
        .await
        .unwrap();
    assert!(matches!(
        client.pause(99).await,
        Err(nikgob_torrent::TorrentError::NotFound(99))
    ));
    assert!(
        client
            .add(TorrentPayload::Magnet(String::new()), Some(vec![]))
            .await
            .is_err()
    );
    client.shutdown().await;
}
