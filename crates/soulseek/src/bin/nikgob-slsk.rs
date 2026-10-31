//! `nikgob-slsk`: the Soulseek client as an always-on service (a server, a
//! Raspberry Pi) that the player talks to over HTTP.
//!
//! `nikgob-slsk [--config path]` reads a TOML file (default
//! `nikgob-slsk.toml` next to where it runs; see
//! [`nikgob_soulseek::http::ServiceConfig`]). `NIKGOB_SLSK_USERNAME`,
//! `NIKGOB_SLSK_PASSWORD` and `NIKGOB_SLSK_TOKEN` override the file, so the
//! secrets can stay out of it. `--print-config` prints an example file.

use std::path::PathBuf;

use nikgob_soulseek::http::{self, ServiceConfig};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let mut path = PathBuf::from("nikgob-slsk.toml");
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => path = args.next().map(PathBuf::from).unwrap_or(path),
            "--print-config" => {
                let example = ServiceConfig {
                    share_folders: vec![PathBuf::from("/srv/music")],
                    downloads_dir: PathBuf::from("/srv/slsk/downloads"),
                    state_dir: Some(PathBuf::from("/srv/slsk/state")),
                    ..ServiceConfig::default()
                };
                print!("{}", toml::to_string_pretty(&example).expect("serializable"));
                return;
            }
            other => {
                eprintln!("unknown argument {other}; use --config <file> or --print-config");
                std::process::exit(2);
            }
        }
    }
    let mut cfg: ServiceConfig = match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                std::process::exit(2);
            }
        },
        Err(e) => {
            eprintln!("{}: {e} (write one with --print-config)", path.display());
            std::process::exit(2);
        }
    };
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(v) = env("NIKGOB_SLSK_USERNAME") {
        cfg.username = v;
    }
    if let Some(v) = env("NIKGOB_SLSK_PASSWORD") {
        cfg.password = v;
    }
    if let Some(v) = env("NIKGOB_SLSK_TOKEN") {
        cfg.http.token = v;
    }
    // One thread is plenty for a client that mostly waits on sockets.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    let code = rt.block_on(async move {
        let client = match nikgob_soulseek::Client::start(cfg.client_config()).await {
            Ok(c) => c,
            Err(e) => {
                log::error!("{e}");
                return 1;
            }
        };
        let stop = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        match http::serve(client.clone(), cfg.downloads_dir.clone(), &cfg.http.listen, cfg.http.token.clone(), stop).await {
            Ok(addr) => log::info!("HTTP API on {addr}"),
            Err(e) => {
                log::error!("{e}");
                return 1;
            }
        }
        let _ = tokio::signal::ctrl_c().await;
        client.shutdown();
        0
    });
    std::process::exit(code);
}
