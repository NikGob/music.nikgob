//! The client against the mock server and against itself or hand-driven
//! peers: login, searches both ways, direct and indirect connections,
//! downloads with resume, the upload limit, sharing off, browse, the
//! distributed parent and the headless service's HTTP API.

mod common;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use common::*;
use nikgob_soulseek::codec::{read_frame, split_u8, split_u32, write_all};
use nikgob_soulseek::msg::{ConnType, DistribMsg, FromServer, PeerInit, PeerMsg, ToServer};
use nikgob_soulseek::{Client, Connection, DownloadState, Error, RemoteClient, SearchQuery, ShareSettings, Soulseek};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

const SEARCH_WAIT: Duration = Duration::from_millis(1500);

#[tokio::test]
async fn login_and_refused_password() {
    let server = MockServer::start().await;
    let a = client(config(&server, "alice", None)).await;
    assert_eq!(a.status().connection, Connection::LoggedIn);
    server.wait_for("alice", |m| matches!(m, ToServer::AcceptChildren(_)), Duration::from_secs(2)).await.unwrap();
    let sent = server.received("alice");
    assert!(sent.iter().any(|m| matches!(m, ToServer::SetWaitPort(p) if *p == u32::from(a.status().listen_port))));
    assert!(sent.iter().any(|m| matches!(m, ToServer::AcceptChildren(false))));
    // Not sharing: no distributed parent asked for.
    assert!(!sent.iter().any(|m| matches!(m, ToServer::HaveNoParent(true))));
    a.shutdown();

    server.set_password("bob", "right");
    let mut cfg = config(&server, "bob", None);
    cfg.password = "wrong".into();
    let b = Client::start(cfg).await.unwrap();
    match b.wait_logged_in(Duration::from_secs(5)).await {
        Err(Error::Login(r)) => assert_eq!(r, "INVALIDPASS"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(b.status().connection, Connection::Failed { .. }));
}

#[tokio::test]
async fn empty_credentials_do_not_start() {
    let server = MockServer::start().await;
    let mut cfg = config(&server, "x", None);
    cfg.username.clear();
    assert!(matches!(Client::start(cfg).await, Err(Error::Config(_))));
}

#[tokio::test]
async fn search_and_download_between_two_clients() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path(), 300_000);
    let before = snapshot(&lib);
    let a = client(config(&server, "alice", Some(&lib))).await;
    assert_eq!(a.status().shared_files, 4);
    let b = client(config(&server, "bob", None)).await;
    let res = b.search(&SearchQuery::track("aimer insane"), SEARCH_WAIT).await.unwrap();
    assert_eq!(res.len(), 1, "{res:?}");
    let r = &res[0];
    assert_eq!(r.user, "alice");
    assert_eq!(r.path, "Music\\Aimer\\Daydream\\01 Insane Dream.flac");
    assert_eq!(r.format, "FLAC");
    assert!(r.lossless);
    assert_eq!(r.size, 300_000);
    assert!(r.free_slot);
    // An album query gets the folder.
    let res = b.search(&SearchQuery::album("aimer daydream"), SEARCH_WAIT).await.unwrap();
    let albums = nikgob_soulseek::group_albums(&res);
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].files.len(), 3);
    // Japanese names.
    let res = b.search(&SearchQuery::track("勘冴えて悔しいわ"), SEARCH_WAIT).await.unwrap();
    assert_eq!(res.len(), 1);

    let out = tempfile::tempdir().unwrap();
    let r = b.search(&SearchQuery::track("insane dream"), SEARCH_WAIT).await.unwrap().remove(0);
    let mut d = b.download(&r, out.path());
    let path = tokio::time::timeout(Duration::from_secs(20), d.wait()).await.unwrap().unwrap();
    assert_eq!(path, out.path().join("Daydream").join("01 Insane Dream.flac"));
    assert_eq!(std::fs::read(&path).unwrap(), pattern(300_000));
    assert!(!out.path().join("Daydream").join("01 Insane Dream.flac.part").exists());
    assert!(wait(|| a.status().uploaded_bytes == 300_000, Duration::from_secs(5)).await);
    // Asking again for a file already there finishes at once.
    let mut again = b.download(&r, out.path());
    assert_eq!(again.wait().await.unwrap(), path);
    // The whole folder.
    let files = b.folder("alice", "Music\\Aimer\\Daydream").await.unwrap();
    assert_eq!(files.len(), 3);
    // The library is as it was.
    assert_eq!(snapshot(&lib), before);
    a.shutdown();
    b.shutdown();
}

#[tokio::test]
async fn indirect_connections_when_the_sharer_is_not_reachable() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path(), 50_000);
    // Bob is behind a router nobody gets through: alice answers his
    // search, and later sends him the file, through connections he opens
    // to her when the server asks him to.
    server.hide_port("bob");
    let a = client(config(&server, "alice", Some(&lib))).await;
    let b = client(config(&server, "bob", None)).await;
    let res = b.search(&SearchQuery::track("falling alone"), SEARCH_WAIT).await.unwrap();
    assert_eq!(res.len(), 1);
    let out = tempfile::tempdir().unwrap();
    let mut d = b.download(&res[0], out.path());
    let path = tokio::time::timeout(Duration::from_secs(20), d.wait()).await.unwrap().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), pattern(25_000));
    let asked = server.received("alice");
    for kind in [ConnType::P, ConnType::F] {
        assert!(asked.iter().any(|m| matches!(m, ToServer::ConnectToPeer { user, kind: k, .. } if user == "bob" && *k == kind)));
    }
    a.shutdown();
    b.shutdown();
}

#[tokio::test]
async fn nobody_reachable_fails_cleanly() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path(), 10_000);
    let a = client(config(&server, "alice", Some(&lib))).await;
    let b = client(config(&server, "bob", None)).await;
    let res = b.search(&SearchQuery::track("falling alone"), SEARCH_WAIT).await.unwrap();
    a.shutdown();
    assert!(wait(|| !server.is_online("alice"), Duration::from_secs(5)).await);
    let out = tempfile::tempdir().unwrap();
    let mut d = b.download(&res[0], out.path());
    let err = tokio::time::timeout(Duration::from_secs(10), d.wait()).await.unwrap().unwrap_err();
    assert!(matches!(err, Error::Denied(ref r) if r.contains("offline")), "{err:?}");
}

#[tokio::test]
async fn upload_speed_limit_and_slots() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path(), 300_000);
    let mut cfg = config(&server, "alice", Some(&lib));
    cfg.share.upload_limit_kbps = 100;
    let a = client(cfg).await;
    let b = client(config(&server, "bob", None)).await;
    let r = b.search(&SearchQuery::track("insane dream"), SEARCH_WAIT).await.unwrap().remove(0);
    let out = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let mut d = b.download(&r, out.path());
    let mut seen_progress = false;
    let mut rx = d.subscribe();
    let watch = tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            if matches!(*rx.borrow(), DownloadState::Transferring { received, .. } if received > 0) {
                return true;
            }
        }
        false
    });
    tokio::time::timeout(Duration::from_secs(20), d.wait()).await.unwrap().unwrap();
    let secs = started.elapsed().as_secs_f64();
    // 300 000 bytes at 100 KiB/s: about 2.9 s.
    assert!(secs > 2.3, "took {secs:.2} s");
    assert!(secs < 8.0, "took {secs:.2} s");
    seen_progress |= watch.await.unwrap();
    assert!(seen_progress);

    // The limit changes live; turning sharing off denies new requests.
    a.set_share(ShareSettings { enabled: false, upload_limit_kbps: 0, upload_slots: 2 });
    assert_eq!(a.status().shared_files, 0);
    let r2 = nikgob_soulseek::SearchResult { path: "Music\\Aimer\\Daydream\\02 Falling Alone.flac".into(), ..r.clone() };
    let mut d2 = b.download(&r2, out.path());
    let err = tokio::time::timeout(Duration::from_secs(10), d2.wait()).await.unwrap().unwrap_err();
    assert!(matches!(err, Error::Denied(ref m) if m == "File not shared."), "{err:?}");
    // And searches go unanswered.
    let res = b.search(&SearchQuery::track("insane dream"), SEARCH_WAIT).await.unwrap();
    assert!(res.is_empty());
    a.shutdown();
    b.shutdown();
}

/// A sharer the test drives by hand: it sends the file in pieces and drops
/// the connection in between, as an interrupted transfer would.
async fn hand_driven_uploader(
    listener: TcpListener,
    downloader: SocketAddr,
    data: Vec<u8>,
    path: String,
    cut_at: Vec<usize>,
    offsets: tokio::sync::mpsc::UnboundedSender<u64>,
) {
    let (mut p, _) = listener.accept().await.unwrap();
    let frame = read_frame(&mut p, 4096).await.unwrap();
    let (code, body) = split_u8(&frame).unwrap();
    let PeerInit::Init { user, kind: ConnType::P, .. } = PeerInit::decode(code, body).unwrap() else { panic!() };
    assert_eq!(user, "bob");
    let mut cuts = cut_at.into_iter();
    let mut token = 500;
    loop {
        let Ok(frame) = read_frame(&mut p, 1 << 20).await else { return };
        let (code, body) = split_u32(&frame).unwrap();
        match PeerMsg::decode(code, body).unwrap() {
            PeerMsg::QueueUpload(f) => {
                assert_eq!(f, path);
                token += 1;
                let req = PeerMsg::TransferRequest { direction: 1, token, filename: f, size: Some(data.len() as u64) };
                write_all(&mut p, &req.encode()).await.unwrap();
            }
            PeerMsg::TransferResponse { token: t, allowed, .. } => {
                assert!(allowed);
                assert_eq!(t, token);
                let mut f = TcpStream::connect(downloader).await.unwrap();
                write_all(&mut f, &PeerInit::Init { user: "carol".into(), kind: ConnType::F, token: 0 }.encode()).await.unwrap();
                f.write_u32_le(t).await.unwrap();
                let offset = f.read_u64_le().await.unwrap();
                offsets.send(offset).unwrap();
                let end = cuts.next().unwrap_or(data.len());
                f.write_all(&data[offset as usize..end]).await.unwrap();
                f.flush().await.unwrap();
                if end < data.len() {
                    drop(f);
                    write_all(&mut p, &PeerMsg::UploadFailed(path.clone()).encode()).await.unwrap();
                } else {
                    // The downloader closes once it has it all.
                    let mut one = [0u8; 1];
                    let _ = tokio::time::timeout(Duration::from_secs(5), f.read(&mut one)).await;
                }
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn download_resumes_after_an_interruption_and_from_a_partial_file() {
    let server = MockServer::start().await;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let SocketAddr::V4(addr) = listener.local_addr().unwrap() else { unreachable!() };
    server.add_static_peer("carol", addr);
    let b = client(config(&server, "bob", None)).await;
    let downloader = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, b.status().listen_port));
    let data = pattern(200_000);
    let path = "Shared\\Album\\track.flac".to_owned();
    let out = tempfile::tempdir().unwrap();
    // 30 000 bytes from an earlier run are there already.
    std::fs::create_dir_all(out.path().join("Album")).unwrap();
    std::fs::write(out.path().join("Album").join("track.flac.part"), &data[..30_000]).unwrap();
    let (tx, mut offsets) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(hand_driven_uploader(listener, downloader, data.clone(), path.clone(), vec![90_000], tx));
    let r = nikgob_soulseek::SearchResult { user: "carol".into(), path, size: 200_000, ..Default::default() };
    let mut d = b.download(&r, out.path());
    let got = tokio::time::timeout(Duration::from_secs(20), d.wait()).await.unwrap().unwrap();
    assert_eq!(std::fs::read(&got).unwrap(), data);
    assert_eq!(offsets.recv().await, Some(30_000));
    assert_eq!(offsets.recv().await, Some(90_000));
    b.shutdown();
}

#[tokio::test]
async fn searches_reach_a_sharer_through_its_distributed_parent() {
    let server = MockServer::start().await;
    server.set_embed(false);
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path(), 1000);
    let a = client(config(&server, "alice", Some(&lib))).await;
    assert!(server.wait_for("alice", |m| matches!(m, ToServer::HaveNoParent(true)), Duration::from_secs(5)).await.is_some());
    let parent = MockParent::start("rootuser").await;
    assert!(server.send_to("alice", FromServer::PossibleParents(vec![("parentuser".into(), *parent.addr.ip(), u32::from(parent.addr.port()))])));
    assert!(wait(|| parent.connected.lock().is_some(), Duration::from_secs(5)).await);
    assert_eq!(parent.connected.lock().as_deref(), Some("alice"));

    let b = client(config(&server, "bob", None)).await;
    let h = b.start_search(&SearchQuery::track("zutomayo")).unwrap();
    parent.searches.send(DistribMsg::Search { user: "bob".into(), token: h.token(), query: "zutomayo".into() }).unwrap();
    assert!(wait(|| !h.results().is_empty(), Duration::from_secs(5)).await);
    assert_eq!(h.results()[0].format, "MP3");
    // Alice told the server where she is in the tree.
    let br = server.wait_for("alice", |m| matches!(m, ToServer::BranchRoot(_)), Duration::from_secs(2)).await;
    assert_eq!(br, Some(ToServer::BranchRoot("rootuser".into())));
    assert!(server.received("alice").contains(&ToServer::BranchLevel(1)));
    assert!(server.received("alice").contains(&ToServer::HaveNoParent(false)));
    assert!(a.status().distributed);
    a.shutdown();
    b.shutdown();
}

#[tokio::test]
async fn browse_and_user_info_from_a_raw_peer() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path(), 1000);
    let a = client(config(&server, "alice", Some(&lib))).await;
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, a.status().listen_port)).await.unwrap();
    write_all(&mut s, &PeerInit::Init { user: "dave".into(), kind: ConnType::P, token: 0 }.encode()).await.unwrap();
    write_all(&mut s, &PeerMsg::SharedFileListRequest.encode()).await.unwrap();
    let frame = read_frame(&mut s, 1 << 24).await.unwrap();
    let (code, body) = split_u32(&frame).unwrap();
    let PeerMsg::SharedFileListResponse(dirs) = PeerMsg::decode(code, body).unwrap() else { panic!() };
    assert_eq!(dirs.len(), 2);
    assert_eq!(dirs.iter().map(|d| d.files.len()).sum::<usize>(), 4);
    write_all(&mut s, &PeerMsg::UserInfoRequest.encode()).await.unwrap();
    let frame = read_frame(&mut s, 1 << 20).await.unwrap();
    let (code, body) = split_u32(&frame).unwrap();
    assert!(matches!(PeerMsg::decode(code, body).unwrap(), PeerMsg::UserInfoResponse { slots_free: true, .. }));
    // A path outside the share is refused.
    write_all(&mut s, &PeerMsg::QueueUpload("Music\\..\\..\\secret.txt".into()).encode()).await.unwrap();
    let frame = read_frame(&mut s, 1 << 20).await.unwrap();
    let (code, body) = split_u32(&frame).unwrap();
    assert_eq!(
        PeerMsg::decode(code, body).unwrap(),
        PeerMsg::UploadDenied { filename: "Music\\..\\..\\secret.txt".into(), reason: "File not shared.".into() }
    );
    a.shutdown();
}

#[tokio::test]
async fn headless_service_over_http() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = library(dir.path(), 120_000);
    let a = client(config(&server, "alice", Some(&lib))).await;
    let b = client(config(&server, "bob", None)).await;
    let svc_dl = tempfile::tempdir().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let addr = nikgob_soulseek::http::serve(b.clone(), svc_dl.path().to_path_buf(), "127.0.0.1:0", "s3cret".into(), async {
        let _ = stop_rx.await;
    })
    .await
    .unwrap();
    let base = format!("http://{addr}");
    let wrong = RemoteClient::new(&base, "nope").unwrap();
    assert!(wrong.status().await.is_err());
    let remote = Soulseek::Remote(RemoteClient::new(&base, "s3cret").unwrap());
    assert_eq!(remote.status().await.unwrap().connection, Connection::LoggedIn);
    let res = remote.search(&SearchQuery::track("insane dream"), Duration::from_secs(1)).await.unwrap();
    assert_eq!(res.len(), 1);
    let local = tempfile::tempdir().unwrap();
    let mut d = remote.download(&res[0], local.path()).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(30), d.wait()).await.unwrap().unwrap();
    assert_eq!(got, local.path().join("Daydream").join("01 Insane Dream.flac"));
    assert_eq!(std::fs::read(&got).unwrap(), pattern(120_000));
    // Fetched and removed on the service's side.
    let Soulseek::Remote(rc) = &remote else { unreachable!() };
    assert!(rc.files().await.unwrap().is_empty());
    // Share settings through the API.
    remote.set_share(ShareSettings { enabled: true, upload_limit_kbps: 64, upload_slots: 1 }).await.unwrap();
    assert_eq!(b.share_settings().upload_limit_kbps, 64);
    // Without a token it will not listen beyond loopback.
    let open = nikgob_soulseek::http::serve(b.clone(), svc_dl.path().to_path_buf(), "0.0.0.0:0", String::new(), async {}).await;
    assert!(matches!(open, Err(Error::Config(_))));
    let _ = stop_tx.send(());
    a.shutdown();
    b.shutdown();
}

#[tokio::test]
async fn reconnects_after_the_server_drops_but_not_after_relogged() {
    let server = MockServer::start().await;
    let a = client(config(&server, "alice", None)).await;
    let logins = || server.received("alice").iter().filter(|m| matches!(m, ToServer::SetWaitPort(_))).count();
    assert!(wait(|| logins() == 1, Duration::from_secs(2)).await);
    server.kick("alice");
    assert!(wait(|| logins() == 2, Duration::from_secs(5)).await);
    assert!(wait(|| a.status().connection == Connection::LoggedIn, Duration::from_secs(5)).await);
    // Logged in somewhere else: the session ends for good.
    assert!(server.send_to("alice", FromServer::Relogged));
    assert!(wait(|| matches!(a.status().connection, Connection::Failed { .. }), Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(logins(), 2);
}

/// Every path under `dir` with its size and modification time.
fn snapshot(dir: &std::path::Path) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let md = e.metadata().unwrap();
            if md.is_dir() {
                // NTFS updates folder times lazily; the files are what counts.
                stack.push(e.path());
                out.push((e.path(), 0, std::time::UNIX_EPOCH));
            } else {
                out.push((e.path(), md.len(), md.modified().unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn a_queued_download_can_be_cancelled() {
    let server = MockServer::start().await;
    // A sharer that takes the request and never gets to it.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let SocketAddr::V4(addr) = listener.local_addr().unwrap() else { unreachable!() };
    server.add_static_peer("slow", addr);
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut sink = Vec::new();
        let _ = s.read_to_end(&mut sink).await;
    });
    let b = client(config(&server, "bob", None)).await;
    let out = tempfile::tempdir().unwrap();
    let r = nikgob_soulseek::SearchResult { user: "slow".into(), path: r"X\Y\z.flac".into(), size: 10, ..Default::default() };
    let mut d = b.download(&r, out.path());
    assert!(matches!(d.state(), DownloadState::Queued { .. }));
    assert_eq!(b.downloads().len(), 1);
    d.cancel();
    assert!(matches!(tokio::time::timeout(Duration::from_secs(5), d.wait()).await.unwrap(), Err(Error::Cancelled)));
    assert!(b.downloads()[0].state.is_finished());
    b.shutdown();
}
