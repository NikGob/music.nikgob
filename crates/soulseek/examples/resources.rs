//! Measures what the embedded client costs: memory and CPU while idle,
//! while sharing a library-sized share under a stream of incoming searches,
//! and while uploading. Everything runs against the in-process mock server
//! of the tests, on synthetic files in a temporary folder.
//!
//! `cargo run --release -p nikgob-soulseek --example resources`
//! (`NIKGOB_SLSK_RES_FILES` sets the number of files, 20 000 by default).

#[path = "../tests/common/mod.rs"]
mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{MockServer, config};
use nikgob_soulseek::codec::{read_frame, split_u8, split_u32, write_all};
use nikgob_soulseek::msg::{ConnType, DistribMsg, FromServer, PeerInit, PeerMsg};
use nikgob_soulseek::share::{ScanOptions, ShareIndex};
use nikgob_soulseek::{Client, ShareSettings};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

const WORDS: &[&str] = &[
    "dream", "night", "blue", "light", "rain", "ocean", "summer", "fire", "heart", "star", "moon", "city", "road", "time",
    "love", "shadow", "silver", "echo", "river", "storm", "glass", "winter", "gold", "sky", "wind", "stone", "garden",
];

/// A tiny valid WAV file (so the headers really get read).
fn wav(samples: usize) -> Vec<u8> {
    let data = samples * 4;
    let mut v = Vec::with_capacity(44 + data);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&44100u32.to_le_bytes());
    v.extend_from_slice(&(44100u32 * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data as u32).to_le_bytes());
    v.resize(44 + data, 0);
    v
}

fn make_library(root: &Path, files: usize) -> usize {
    let content = wav(512);
    let mut n = 0;
    let mut seed = 12345u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut album = 0;
    while n < files {
        let artist = format!("Artist {} {}", album % 700, WORDS[album % WORDS.len()]);
        let dir = root.join(&artist).join(format!("Album {album} {}", WORDS[(album / 3) % WORDS.len()]));
        std::fs::create_dir_all(&dir).unwrap();
        for t in 1..=10 {
            if n >= files {
                break;
            }
            let w1 = WORDS[(rnd() % WORDS.len() as u64) as usize];
            let w2 = WORDS[(rnd() % WORDS.len() as u64) as usize];
            std::fs::write(dir.join(format!("{t:02} {w1} {w2} {}.flac", rnd() % 100_000)), &content).unwrap();
            n += 1;
        }
        album += 1;
    }
    n
}

struct Meter {
    sys: System,
    pid: Pid,
    cpu_ms: u64,
    at: Instant,
}

impl Meter {
    fn new() -> Self {
        let mut m = Self { sys: System::new(), pid: sysinfo::get_current_pid().unwrap(), cpu_ms: 0, at: Instant::now() };
        m.sample();
        m
    }

    /// (RSS MiB, CPU % of one core since the last sample).
    fn sample(&mut self) -> (f64, f64) {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let p = self.sys.process(self.pid).unwrap();
        let cpu = p.accumulated_cpu_time();
        let wall = self.at.elapsed().as_secs_f64() * 1000.0;
        let pct = if wall > 0.0 { (cpu - self.cpu_ms) as f64 / wall * 100.0 } else { 0.0 };
        self.cpu_ms = cpu;
        self.at = Instant::now();
        (p.memory() as f64 / 1_048_576.0, pct)
    }
}

/// A peer that takes every connection and reads it to the end, counting
/// search answers.
async fn drain(listener: TcpListener, answers: Arc<AtomicUsize>) {
    while let Ok((mut s, _)) = listener.accept().await {
        let answers = answers.clone();
        tokio::spawn(async move {
            if read_frame(&mut s, 4096).await.is_err() {
                return;
            }
            while let Ok(f) = read_frame(&mut s, 1 << 24).await {
                if split_u32(&f).is_ok_and(|(c, _)| c == 9) {
                    answers.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }
}

/// Downloads `vpath` from `sharer` by hand and returns the bytes received.
async fn leech(sharer: SocketAddr, listener: TcpListener, vpath: String) -> u64 {
    let mut p = TcpStream::connect(sharer).await.unwrap();
    write_all(&mut p, &PeerInit::Init { user: "leech".into(), kind: ConnType::P, token: 0 }.encode()).await.unwrap();
    write_all(&mut p, &PeerMsg::QueueUpload(vpath).encode()).await.unwrap();
    let size = loop {
        let f = read_frame(&mut p, 1 << 20).await.unwrap();
        let (code, body) = split_u32(&f).unwrap();
        if let Ok(PeerMsg::TransferRequest { direction: 1, token, size, .. }) = PeerMsg::decode(code, body) {
            write_all(&mut p, &PeerMsg::TransferResponse { token, allowed: true, size: None, reason: None }.encode())
                .await
                .unwrap();
            break size.unwrap();
        }
    };
    let (mut f, _) = listener.accept().await.unwrap();
    let init = read_frame(&mut f, 4096).await.unwrap();
    let (code, body) = split_u8(&init).unwrap();
    assert!(matches!(PeerInit::decode(code, body), Ok(PeerInit::Init { kind: ConnType::F, .. })));
    let _token = f.read_u32_le().await.unwrap();
    f.write_u64_le(0).await.unwrap();
    let mut buf = vec![0u8; 256 * 1024];
    let mut got = 0u64;
    while got < size {
        let n = f.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        got += n as u64;
    }
    got
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let files: usize = std::env::var("NIKGOB_SLSK_RES_FILES").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000);
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("Music");
    let state = tmp.path().join("state");
    let made = Instant::now();
    let n = make_library(&lib, files);
    let big = lib.join("Big").join("Long Mix");
    std::fs::create_dir_all(&big).unwrap();
    std::fs::write(big.join("01 long mix.flac"), vec![7u8; 64 << 20]).unwrap();
    std::fs::write(big.join("02 short mix.flac"), vec![7u8; 8 << 20]).unwrap();
    println!("synthetic library: {n} small files + a 64 and an 8 MiB file, made in {:.1} s", made.elapsed().as_secs_f64());

    let mut m = Meter::new();
    let (base_rss, _) = m.sample();
    let cache = state.join("share.cache");
    let t = Instant::now();
    let idx = ShareIndex::scan(std::slice::from_ref(&lib), Some(&cache), ScanOptions::default());
    let cold = t.elapsed();
    let t = Instant::now();
    let idx2 = ShareIndex::scan(std::slice::from_ref(&lib), Some(&cache), ScanOptions::default());
    let warm = t.elapsed();
    let (rss_two_idx, _) = m.sample();
    assert_eq!(idx.file_count(), idx2.file_count());
    let t = Instant::now();
    let mut hits = 0;
    for i in 0..2000 {
        hits += idx.search(&format!("{} {}", WORDS[i % WORDS.len()], WORDS[(i * 7) % WORDS.len()]), &[], 100).len();
    }
    let per_search = t.elapsed() / 2000;
    drop(idx);
    drop(idx2);
    println!(
        "listing: first {:.2} s (headers read), again from the cache {:.2} s; two indexes held: +{:.1} MiB",
        cold.as_secs_f64(),
        warm.as_secs_f64(),
        rss_two_idx - base_rss
    );
    println!("answering a two-word search from the index: {:.1} µs ({hits} hits in 2000 searches)", per_search.as_secs_f64() * 1e6);

    let server = MockServer::start().await;
    server.set_embed(false);
    let (rss0, _) = m.sample();
    let mut cfg = config(&server, "alice", Some(&lib));
    cfg.state_dir = Some(state.clone());
    let client = Client::start(cfg).await.unwrap();
    client.wait_logged_in(Duration::from_secs(5)).await.unwrap();
    let t = Instant::now();
    while client.status().shared_files < n as u32 + 2 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let started = t.elapsed();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (rss1, _) = m.sample();
    println!(
        "client logged in and sharing {} files after {:.2} s; memory +{:.1} MiB over the process before it",
        client.status().shared_files,
        started.as_secs_f64(),
        rss1 - rss0
    );

    let idle_secs = 30;
    m.sample();
    tokio::time::sleep(Duration::from_secs(idle_secs)).await;
    let (rss_idle, cpu_idle) = m.sample();
    println!("idle {idle_secs} s: CPU {cpu_idle:.2}% of one core, memory {rss_idle:.1} MiB (whole process)");

    // Searches arrive as from a distributed parent: 50 a second, one in
    // five matching something.
    let searcher = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let SocketAddr::V4(sa) = searcher.local_addr().unwrap() else { unreachable!() };
    server.add_static_peer("searcher", sa);
    let answers = Arc::new(AtomicUsize::new(0));
    tokio::spawn(drain(searcher, answers.clone()));
    let load_secs = 20u32;
    let rate = 50u32;
    m.sample();
    let mut tick = tokio::time::interval(Duration::from_millis(u64::from(1000 / rate)));
    for i in 0..load_secs * rate {
        tick.tick().await;
        let q = if i % 5 == 0 {
            format!("{} {}", WORDS[i as usize % WORDS.len()], WORDS[(i as usize * 3) % WORDS.len()])
        } else {
            format!("nothing{i} here")
        };
        let body = DistribMsg::search_body("searcher", 10_000 + i, &q);
        server.send_to("alice", FromServer::EmbeddedMessage { code: 3, body });
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (rss_load, cpu_load) = m.sample();
    println!(
        "{rate} searches/s for {load_secs} s: CPU {cpu_load:.2}% of one core, memory {rss_load:.1} MiB, {} answers sent",
        answers.load(Ordering::Relaxed)
    );

    // One upload, no speed limit, then at 1 MiB/s.
    for limit in [0u32, 1024] {
        client.set_share(ShareSettings { enabled: true, upload_limit_kbps: limit, upload_slots: 2 });
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let SocketAddr::V4(la) = l.local_addr().unwrap() else { unreachable!() };
        server.add_static_peer("leech", la);
        let sharer = SocketAddr::from((Ipv4Addr::LOCALHOST, client.status().listen_port));
        let vpath = if limit == 0 { "Music\\Big\\Long Mix\\01 long mix.flac" } else { "Music\\Big\\Long Mix\\02 short mix.flac" };
        m.sample();
        let t = Instant::now();
        let shown = tokio::time::timeout(Duration::from_secs(120), leech(sharer, l, vpath.to_owned())).await.unwrap();
        let secs = t.elapsed().as_secs_f64();
        let (rss_up, cpu_up) = m.sample();
        println!(
            "upload {} MiB {}: {:.1} MiB/s, CPU {cpu_up:.1}% of one core (sender and receiver together), memory {rss_up:.1} MiB",
            shown >> 20,
            if limit == 0 { "unlimited".to_owned() } else { format!("limited to {limit} KiB/s") },
            (shown as f64 / 1_048_576.0) / secs,
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    client.shutdown();
}
