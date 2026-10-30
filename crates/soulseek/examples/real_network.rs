//! Logs in to the real Soulseek network with the account in the
//! environment, searches once and prints the best results. Shares nothing.
//!
//! `SLSK_USERNAME=… SLSK_PASSWORD=… SLSK_QUERY="aimer daydream"
//! cargo run -p nikgob-soulseek --example real_network`
//! (`SLSK_DOWNLOAD_TO=<folder>` also downloads the first lossless result).

use std::time::Duration;

use nikgob_soulseek::{Client, Config, SearchQuery, ShareSettings};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(username), Some(password)) = (env("SLSK_USERNAME"), env("SLSK_PASSWORD")) else {
        eprintln!("set SLSK_USERNAME and SLSK_PASSWORD (and SLSK_QUERY) to try the real network");
        return;
    };
    let query = env("SLSK_QUERY").unwrap_or_else(|| "aimer daydream".into());
    let cfg = Config {
        username,
        password,
        share: ShareSettings { enabled: false, ..ShareSettings::default() },
        ..Config::default()
    };
    let client = Client::start(cfg).await.expect("start");
    if let Err(e) = client.wait_logged_in(Duration::from_secs(20)).await {
        eprintln!("login: {e}");
        return;
    }
    println!("logged in; listening on {}", client.status().listen_port);
    let mut res = client.search(&SearchQuery::track(&query), Duration::from_secs(10)).await.expect("search");
    res.sort_by_key(|r| (!r.lossless, !r.free_slot, r.queue_length));
    println!("{} results for {query:?}", res.len());
    for r in res.iter().take(20) {
        println!(
            "{:>4} {} {:>9} B slot:{} queue:{} {} {}",
            r.format,
            if r.lossless { "lossless" } else { "lossy   " },
            r.size,
            r.free_slot,
            r.queue_length,
            r.user,
            r.path
        );
    }
    if let (Some(dir), Some(r)) = (env("SLSK_DOWNLOAD_TO"), res.iter().find(|r| r.lossless && r.free_slot)) {
        let mut d = client.download(r, std::path::Path::new(&dir));
        match tokio::time::timeout(Duration::from_secs(600), d.wait()).await {
            Ok(Ok(p)) => println!("downloaded to {}", p.display()),
            Ok(Err(e)) => println!("download failed: {e}"),
            Err(_) => println!("still {:?} after ten minutes", d.state()),
        }
    }
    client.shutdown();
}
