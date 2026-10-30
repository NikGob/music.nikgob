//! The Soulseek source (docs/stage4.5.md, poll answer 3), run from the
//! `[soulseek]` part of settings.toml: either the embedded client, on a
//! thread of its own at the lowest priority with a one-thread Tokio
//! runtime, sharing the library folder read-only; or a remote `nikgob-slsk`
//! the player only talks to. It starts when the settings turn it on, and
//! stops, or restarts with new credentials, when they change; share
//! settings apply live.
//!
//! Searching and downloading go through [`SoulseekService::source`] (and
//! [`SoulseekService::spawn`] for the async calls): the «Источники» source
//! wires them in.

use std::path::PathBuf;
use std::time::Duration;

use nikgob_core::settings::{SoulseekMode, SoulseekSettings};
use nikgob_soulseek::{Client, Config, RemoteClient, ShareSettings, Soulseek};

use crate::Worker;

struct Running {
    rt: tokio::runtime::Handle,
    source: Soulseek,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub(crate) struct SoulseekService {
    running: Option<Running>,
    applied: Option<SoulseekSettings>,
    library_dir: PathBuf,
    /// The share index cache; None = not kept.
    state_dir: Option<PathBuf>,
}

fn share_of(s: &SoulseekSettings) -> ShareSettings {
    ShareSettings { enabled: s.share, upload_limit_kbps: s.upload_limit_kbps, upload_slots: s.upload_slots.max(1) }
}

/// Settings that need a new client (the rest apply live).
fn same_session(a: &SoulseekSettings, b: &SoulseekSettings) -> bool {
    a.enabled == b.enabled
        && a.mode == b.mode
        && a.username == b.username
        && a.password == b.password
        && a.server == b.server
        && a.listen_port == b.listen_port
        && a.remote_url == b.remote_url
        && a.remote_token == b.remote_token
}

impl SoulseekService {
    pub(crate) fn new(library_dir: PathBuf, state_dir: Option<PathBuf>) -> Self {
        Self { running: None, applied: None, library_dir, state_dir }
    }

    /// The running source, if any.
    #[allow(dead_code)]
    pub(crate) fn source(&self) -> Option<Soulseek> {
        self.running.as_ref().map(|r| r.source.clone())
    }

    /// Runs `f` on the source's runtime.
    #[allow(dead_code)]
    pub(crate) fn spawn<F>(&self, f: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        match &self.running {
            Some(r) => {
                r.rt.spawn(f);
                true
            }
            None => false,
        }
    }

    /// Starts, restarts, changes or stops the source to match `s`.
    pub(crate) fn apply(&mut self, s: &SoulseekSettings) {
        if self.applied.as_ref() == Some(s) {
            return;
        }
        if let (Some(old), Some(r)) = (&self.applied, &self.running)
            && same_session(old, s)
        {
            if let Soulseek::Embedded(c) = &r.source {
                c.set_share(share_of(s));
            }
            self.applied = Some(s.clone());
            return;
        }
        self.shutdown();
        self.applied = Some(s.clone());
        if !s.enabled {
            return;
        }
        match self.start(s) {
            Ok(r) => self.running = Some(r),
            Err(e) => log::warn!("soulseek: not started: {e}"),
        }
    }

    fn start(&self, s: &SoulseekSettings) -> Result<Running, String> {
        let remote = match s.mode {
            SoulseekMode::Remote => Some(RemoteClient::new(&s.remote_url, &s.remote_token).map_err(|e| e.to_string())?),
            SoulseekMode::Embedded => None,
        };
        let cfg = Config {
            server: s.server.clone(),
            username: s.username.clone(),
            password: s.password.clone(),
            listen_port: s.listen_port,
            share_folders: if self.library_dir.as_os_str().is_empty() { Vec::new() } else { vec![self.library_dir.clone()] },
            share: share_of(s),
            state_dir: self.state_dir.clone(),
            ..Config::default()
        };
        let (tx, rx) = std::sync::mpsc::channel::<Result<(tokio::runtime::Handle, Soulseek), String>>();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("nikgob-slsk".into())
            .spawn(move || {
                let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                rt.block_on(async move {
                    let source = match remote {
                        Some(r) => Soulseek::Remote(r),
                        None => match Client::start(cfg).await {
                            Ok(c) => Soulseek::Embedded(c),
                            Err(e) => {
                                let _ = tx.send(Err(e.to_string()));
                                return;
                            }
                        },
                    };
                    let _ = tx.send(Ok((tokio::runtime::Handle::current(), source.clone())));
                    let _ = stop_rx.await;
                    if let Soulseek::Embedded(c) = source {
                        c.shutdown();
                        // Lets the tasks see the stop and close their sockets.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        let (rt, source) = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => return Err("the Soulseek thread did not start".into()),
        };
        log::info!("soulseek: started ({})", if matches!(source, Soulseek::Remote(_)) { "remote" } else { "embedded" });
        Ok(Running { rt, source, stop: Some(stop_tx), thread: Some(thread) })
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(mut r) = self.running.take() {
            if let Some(s) = r.stop.take() {
                let _ = s.send(());
            }
            if let Some(t) = r.thread.take() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for SoulseekService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Worker {
    /// Starts or stops Soulseek to match settings.toml (at start-up, and
    /// whenever its settings are saved).
    pub(crate) fn soulseek_apply(&mut self) {
        let s = self
            .settings_path
            .as_deref()
            .and_then(|p| nikgob_core::settings::Settings::load(p).ok())
            .map(|s| s.soulseek)
            .unwrap_or_default();
        self.soulseek.apply(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_soulseek::codec::{read_frame, split_u32, write_all};
    use nikgob_soulseek::msg::{FromServer, ToServer};

    /// A server that logs everyone in and keeps what they send.
    async fn server() -> (std::net::SocketAddr, std::sync::Arc<parking_lot::Mutex<Vec<ToServer>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let got = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let seen = got.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let _ = read_frame(&mut s, 1 << 16).await;
                    let ok = FromServer::LoginOk { greet: String::new(), ip: "127.0.0.1".parse().unwrap(), supporter: false };
                    write_all(&mut s, &ok.encode()).await.unwrap();
                    while let Ok(f) = read_frame(&mut s, 1 << 20).await {
                        let (code, body) = split_u32(&f).unwrap();
                        if let Ok(m) = ToServer::decode(code, body) {
                            seen.lock().push(m);
                        }
                    }
                });
            }
        });
        (addr, got)
    }

    async fn until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..250 {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn starts_applies_share_live_and_stops() {
        let (addr, got) = server().await;
        let lib = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lib.path().join("A")).unwrap();
        std::fs::write(lib.path().join("A").join("x.flac"), b"x").unwrap();
        let state = tempfile::tempdir().unwrap();
        let mut svc = SoulseekService::new(lib.path().to_path_buf(), Some(state.path().to_path_buf()));
        let mut s = SoulseekSettings { server: addr.to_string(), listen_port: 0, ..SoulseekSettings::default() };
        // Off by default.
        svc.apply(&s);
        assert!(svc.source().is_none());
        // On without an account: refused, nothing runs.
        s.enabled = true;
        svc.apply(&s);
        assert!(svc.source().is_none());
        s.username = "me".into();
        s.password = "pw".into();
        svc.apply(&s);
        let Some(Soulseek::Embedded(c)) = svc.source() else { panic!("not running") };
        assert!(until(|| c.status().shared_files == 1).await);
        assert!(until(|| got.lock().contains(&ToServer::SharedFoldersFiles { dirs: 1, files: 1 })).await);
        // The limit changes without a new session.
        s.upload_limit_kbps = 50;
        svc.apply(&s);
        assert_eq!(c.share_settings().upload_limit_kbps, 50);
        s.share = false;
        svc.apply(&s);
        assert_eq!(c.status().shared_files, 0);
        s.enabled = false;
        svc.apply(&s);
        assert!(svc.source().is_none());
        // The library folder was only read.
        assert_eq!(std::fs::read_dir(lib.path()).unwrap().count(), 1);
    }
}
