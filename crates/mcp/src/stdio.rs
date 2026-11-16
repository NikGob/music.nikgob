//! `nikgob mcp`: the MCP server on stdin/stdout for a client on this
//! computer (Claude Code, Claude Desktop). It talks to the running player
//! over its API with the token of `NIKGOB_TOKEN`; the API is at
//! `NIKGOB_API_URL`, else where settings.toml says it listens (127.0.0.1
//! and port 47210 by default). stdout carries only the protocol (one JSON
//! message a line); logs go to stderr.

use std::sync::Arc;

use nikgob_core::settings::{API_PORT, ApiSettings, Settings};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::http::HttpBackend;
use crate::protocol::McpServer;

/// Where the server finds the player and with which token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub url: String,
    pub token: String,
}

/// The API's address on this computer as `[api]` of settings.toml has it:
/// the explicit address, or 127.0.0.1 for one that listens everywhere.
pub fn local_url(api: &ApiSettings) -> String {
    let ip = api.bind_ip();
    let host = match ip.as_str() {
        "0.0.0.0" | "" => "127.0.0.1".to_owned(),
        "::" => "[::1]".to_owned(),
        ip if ip.contains(':') && !ip.starts_with('[') => format!("[{ip}]"),
        ip => ip.to_owned(),
    };
    let port = if api.port == 0 { API_PORT } else { api.port };
    format!("http://{host}:{port}")
}

/// The config from the environment's `NIKGOB_TOKEN` and `NIKGOB_API_URL`
/// and settings.toml's `[api]` (`None`: not readable, the defaults).
pub fn config(token: Option<String>, url: Option<String>, api: Option<&ApiSettings>) -> Result<Config, String> {
    let token = token.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).ok_or_else(|| {
        "NIKGOB_TOKEN не задан: создай токен в плеере (Настройки → Межсеть → API и MCP → «Новый токен») и передай его в окружении".to_owned()
    })?;
    let url = match url.map(|u| u.trim().trim_end_matches('/').to_owned()).filter(|u| !u.is_empty()) {
        Some(u) if u.starts_with("http://") || u.starts_with("https://") => u,
        Some(u) => return Err(format!("NIKGOB_API_URL должен начинаться с http://: {u}")),
        None => api.map_or_else(|| format!("http://127.0.0.1:{API_PORT}"), local_url),
    };
    Ok(Config { url, token })
}

/// The config of this process: the environment and the player's
/// settings.toml.
pub fn config_from_env() -> Result<Config, String> {
    let settings = Settings::default_path().ok().and_then(|p| match Settings::load(&p) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("mcp: settings.toml not read ({e}), the API is looked for on its default port");
            None
        }
    });
    config(std::env::var("NIKGOB_TOKEN").ok(), std::env::var("NIKGOB_API_URL").ok(), settings.as_ref().map(|s| &s.api))
}

/// Serves MCP on stdin/stdout until stdin closes. Returns the process's
/// exit code.
pub fn main() -> i32 {
    let config = match config_from_env() {
        Ok(c) => c,
        Err(e) => {
            log::error!("mcp: {e}");
            eprintln!("{e}");
            return 2;
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("mcp: no runtime: {e}");
            return 1;
        }
    };
    match rt.block_on(serve(config)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mcp: {e}");
            1
        }
    }
}

/// One request at most: the longest tool makes a few calls to the API,
/// each bounded by the client's own timeout.
const REQUEST_WAIT: std::time::Duration = std::time::Duration::from_secs(200);
/// The answers left are written within this once every request is done.
const FLUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Reads messages a line each and answers each as it is done (requests run
/// side by side; their answers carry their ids). When stdin closes, the
/// requests still running are waited for (each at most [`REQUEST_WAIT`])
/// and their answers written before the end.
pub async fn serve(config: Config) -> Result<(), String> {
    let backend = HttpBackend::new(&config.url, &config.token)?;
    log::info!("mcp: on stdio, the player's API at {}", config.url);
    let server = Arc::new(McpServer::new(backend));
    // Requests at once at most: a client sends a few, never thousands.
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(line) = rx.recv().await {
            if out.write_all(line.as_bytes()).await.is_err() || out.write_all(b"\n").await.is_err() || out.flush().await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut running = JoinSet::new();
    loop {
        // The finished ones are let go as it reads.
        while running.try_join_next().is_some() {}
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                log::warn!("mcp: stdin: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let server = server.clone();
        let tx = tx.clone();
        running.spawn(async move {
            match tokio::time::timeout(REQUEST_WAIT, server.handle_text(&line)).await {
                Ok(Some(answer)) => {
                    let _ = tx.send(answer).await;
                }
                Ok(None) => {}
                Err(_) => log::warn!("mcp: a request took longer than {} s, dropped", REQUEST_WAIT.as_secs()),
            }
        });
    }
    // The requests still running finish (each bounded), then their answers
    // are written.
    while running.join_next().await.is_some() {}
    drop(tx);
    let _ = tokio::time::timeout(FLUSH_WAIT, writer).await;
    log::info!("mcp: stdin closed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::settings::ApiAccess;

    fn api(access: ApiAccess, address: &str, port: u16) -> ApiSettings {
        ApiSettings { access, address: address.into(), port, ..ApiSettings::default() }
    }

    #[test]
    fn the_token_is_required() {
        assert!(config(None, None, None).unwrap_err().contains("NIKGOB_TOKEN"));
        assert!(config(Some("  ".into()), None, None).is_err());
        let c = config(Some(" nk_x ".into()), None, None).unwrap();
        assert_eq!(c, Config { url: "http://127.0.0.1:47210".into(), token: "nk_x".into() });
    }

    #[test]
    fn the_address_comes_from_the_environment_or_the_settings() {
        let c = config(Some("nk_x".into()), Some("http://192.168.0.157:47211/".into()), None).unwrap();
        assert_eq!(c.url, "http://192.168.0.157:47211");
        assert!(config(Some("nk_x".into()), Some("192.168.0.157:1".into()), None).is_err());
        let local = api(ApiAccess::Local, "", 47215);
        assert_eq!(config(Some("t".into()), None, Some(&local)).unwrap().url, "http://127.0.0.1:47215");
        assert_eq!(local_url(&api(ApiAccess::Network, "", 47210)), "http://127.0.0.1:47210");
        assert_eq!(local_url(&api(ApiAccess::Network, "192.168.0.157", 47210)), "http://192.168.0.157:47210");
        assert_eq!(local_url(&api(ApiAccess::Network, "::", 1)), "http://[::1]:1");
        assert_eq!(local_url(&api(ApiAccess::Network, "fe80::1", 2)), "http://[fe80::1]:2");
    }
}
