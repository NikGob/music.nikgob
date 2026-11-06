//! The Soloist child process: started with the flags of Spotify's command
//! line reference (`--device-name`, `--api-key`, `--ws 127.0.0.1:0`,
//! `--data-dir`, `--cache-dir`, `--cache-size`, `--initial-volume`, and on
//! Linux `--pipewire-device` when a route is given), its API port read from
//! `ws.port` in the data dir, its output kept in `soloist-output.log` with
//! the key blanked out (Soloist takes the key only on its command line, so
//! the command line is never logged). One run lasts until it is stopped,
//! the process exits, or its API connection drops twice within 30 s.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use crate::client::{self, Shared};
use crate::protocol::Command;

/// `CREATE_NO_WINDOW`: no console window flashes up for the child.
#[cfg(windows)]
pub const NO_WINDOW: u32 = 0x0800_0000;
/// How long Soloist may take to open its API.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// A second dropped connection this soon after the first ends the run.
const RECONNECT_WINDOW: Duration = Duration::from_secs(30);
/// Soloist's exit code for an expired build.
pub const EXIT_EXPIRED: i32 = 10;
/// Soloist's own output kept per run, at most.
const LOG_LIMIT: usize = 1 << 20;

pub struct Launch {
    pub exe: PathBuf,
    /// The builds' folder: a stale process is stopped only if it runs one
    /// of ours.
    pub builds: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub device_name: String,
    pub key: String,
    /// 0–100.
    pub initial_volume: u8,
    pub cache_mb: u32,
    /// Linux: the PipeWire node Soloist plays into.
    pub pipewire_device: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Asked to stop.
    Stopped,
    /// The build has expired (exit code 10).
    Expired,
    /// The key was refused (seen in Soloist's own output).
    BadKey,
    /// Anything else, with the reason for the log.
    Failed(String),
}

/// What one run tells its owner as it goes.
pub struct Hooks {
    /// The child is up (its process id) or gone (`None`).
    pub process: Box<dyn Fn(Option<u32>) + Send + Sync>,
    /// The commands channel of the current API connection.
    pub commands: Box<dyn Fn(Option<mpsc::UnboundedSender<Command>>) + Send + Sync>,
    /// Something changed (state or connection).
    pub changed: Arc<dyn Fn() + Send + Sync>,
}

/// Runs Soloist once.
pub async fn run(launch: &Launch, live: &Shared, hooks: &Hooks, mut stop: watch::Receiver<bool>) -> Outcome {
    if let Err(e) = std::fs::create_dir_all(&launch.data_dir).and_then(|()| std::fs::create_dir_all(&launch.cache_dir)) {
        return Outcome::Failed(format!("data dir: {e}"));
    }
    for f in ["ws.port", "ws.addr"] {
        let _ = std::fs::remove_file(launch.data_dir.join(f));
    }
    let builds = launch.builds.clone();
    let data = launch.data_dir.clone();
    let _ = tokio::task::spawn_blocking(move || stop_stale(&builds, &data)).await;

    let mut cmd = tokio::process::Command::new(&launch.exe);
    cmd.arg("--device-name")
        .arg(&launch.device_name)
        .arg("--api-key")
        .arg(&launch.key)
        .args(["--ws", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(&launch.data_dir)
        .arg("--cache-dir")
        .arg(&launch.cache_dir)
        .arg("--cache-size")
        .arg(launch.cache_mb.max(100).to_string())
        .arg("--initial-volume")
        .arg(launch.initial_volume.min(100).to_string());
    if cfg!(target_os = "linux")
        && let Some(d) = &launch.pipewire_device
    {
        cmd.arg("--pipewire-device").arg(d);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(NO_WINDOW);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(format!("spawn: {e}")),
    };
    let bad_key = Arc::new(std::sync::atomic::AtomicBool::new(false));
    keep_output(&launch.data_dir.join("soloist-output.log"), &launch.key, &mut child, bad_key.clone()).await;
    (hooks.process)(child.id());
    log::info!("soloist: started (pid {:?})", child.id());

    let outcome = async {
        // Wait for the API port.
        let started = Instant::now();
        let port = loop {
            if let Ok(Some(status)) = child.try_wait() {
                return exited(status.code(), &bad_key);
            }
            if let Ok(text) = std::fs::read_to_string(launch.data_dir.join("ws.port"))
                && let Ok(port) = text.trim().parse::<u16>()
                && port != 0
            {
                let addr = std::fs::read_to_string(launch.data_dir.join("ws.addr")).unwrap_or_default();
                if !addr.trim().is_empty() && addr.trim() != "127.0.0.1" {
                    return Outcome::Failed(format!("API bound to {}", addr.trim()));
                }
                break port;
            }
            if started.elapsed() > START_TIMEOUT {
                return Outcome::Failed("API did not open".into());
            }
            tokio::select! {
                _ = stop.changed() => return Outcome::Stopped,
                () = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        };
        let mut last_drop: Option<Instant> = None;
        loop {
            let (tx, rx) = mpsc::unbounded_channel();
            (hooks.commands)(Some(tx));
            let changed = hooks.changed.clone();
            let served = tokio::select! {
                _ = stop.changed() => return Outcome::Stopped,
                status = child.wait() => {
                    return exited(status.ok().and_then(|s| s.code()), &bad_key);
                }
                r = client::serve(port, live.clone(), rx, &*changed) => r,
            };
            (hooks.commands)(None);
            if bad_key.load(std::sync::atomic::Ordering::Acquire) {
                return Outcome::BadKey;
            }
            let why = served.err().unwrap_or_else(|| "connection ended".into());
            if last_drop.is_some_and(|t| t.elapsed() < RECONNECT_WINDOW) {
                return Outcome::Failed(format!("API keeps dropping: {why}"));
            }
            log::info!("soloist: API connection dropped ({why}), connecting again");
            last_drop = Some(Instant::now());
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    .await;
    (hooks.commands)(None);
    let _ = child.kill().await;
    let _ = child.wait().await;
    // Killed, it leaves its discovery files behind.
    for f in ["ws.port", "ws.addr", "soloist.pid"] {
        let _ = std::fs::remove_file(launch.data_dir.join(f));
    }
    (hooks.process)(None);
    {
        let mut l = client::lock(live);
        *l = client::Live::default();
    }
    (hooks.changed)();
    log::info!("soloist: stopped ({outcome:?})");
    outcome
}

fn exited(code: Option<i32>, bad_key: &std::sync::atomic::AtomicBool) -> Outcome {
    match code {
        Some(EXIT_EXPIRED) => Outcome::Expired,
        _ if bad_key.load(std::sync::atomic::Ordering::Acquire) => Outcome::BadKey,
        c => Outcome::Failed(format!("exited with {c:?}")),
    }
}

/// Whether a line of Soloist's output says the key was refused. Spotify does
/// not document these lines; this looks for an error line about the key.
pub fn says_bad_key(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    let error = l.contains(" e [") || l.contains("error");
    let key = l.contains("api key") || l.contains("api_key") || l.contains("apikey") || l.contains("developer key");
    let refused = ["invalid", "revoked", "expired", "rejected", "unauthori", "denied", "forbidden"]
        .iter()
        .any(|w| l.contains(w));
    error && key && refused
}

/// Drains the child's output into `path` (a full pipe would stall it), with
/// the key replaced, at most [`LOG_LIMIT`] per run.
async fn keep_output(path: &Path, key: &str, child: &mut tokio::process::Child, bad_key: Arc<std::sync::atomic::AtomicBool>) {
    let Ok(file) = tokio::fs::File::create(path).await else {
        return;
    };
    let file = Arc::new(tokio::sync::Mutex::new((file, 0usize)));
    let streams: [Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>; 2] = [
        child.stdout.take().map(|s| Box::new(s) as _),
        child.stderr.take().map(|s| Box::new(s) as _),
    ];
    for s in streams.into_iter().flatten() {
        let (file, key, bad_key) = (file.clone(), key.to_owned(), bad_key.clone());
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(s).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if says_bad_key(&line) {
                    bad_key.store(true, std::sync::atomic::Ordering::Release);
                }
                let line = format!("{}\n", if key.is_empty() { line } else { line.replace(&key, "<key>") });
                let mut f = file.lock().await;
                if f.1 + line.len() <= LOG_LIMIT {
                    f.1 += line.len();
                    let _ = f.0.write_all(line.as_bytes()).await;
                    let _ = f.0.flush().await;
                }
            }
        });
    }
}

/// Stops a Soloist an earlier run of the player left behind: it still holds
/// the data dir (a second one would exit at once). Only a process started
/// from one of our builds is stopped, as the recorded id may be reused.
pub fn stop_stale(builds: &Path, data: &Path) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let Some(pid) = std::fs::read_to_string(data.join("soloist.pid"))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .map(Pid::from_u32)
    else {
        return;
    };
    let Ok(builds) = builds.canonicalize() else { return };
    let mut sys = System::new();
    let refresh = |sys: &mut System| {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
    };
    refresh(&mut sys);
    let Some(p) = sys.process(pid) else { return };
    let ours = p
        .exe()
        .and_then(|e| e.canonicalize().ok())
        .is_some_and(|e| e.starts_with(&builds));
    if !ours || !p.kill() {
        return;
    }
    log::info!("soloist: stopped the one an earlier run left");
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
        refresh(&mut sys);
        if sys.process(pid).is_none() {
            break;
        }
    }
    let _ = std::fs::remove_file(data.join("soloist.pid"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_error_lines_about_the_key_count() {
        assert!(says_bad_key("03:27:33.741 E [/f:47 ] api key rejected: invalid"));
        assert!(says_bad_key("error: Developer key revoked"));
        assert!(!says_bad_key("03:27:33.762 I [/f:462 ] ready, device_name=x"));
        assert!(!says_bad_key("03:27:33.741 E [/f:47 ] Unable to open settings file for reading"));
    }
}
