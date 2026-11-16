//! `nikgobd`: the player without a window (a Raspberry Pi, a server), for
//! `systemd --user`. The same runtime and engine as the app, driven over
//! the API.
//!
//! `nikgobd [--config <file>]` reads a TOML file (default
//! `$XDG_CONFIG_HOME/nikgob/nikgobd.toml`, see [`config::Config`]);
//! `NIKGOBD_*` variables override it. `--print-config` prints an example.
//! It logs to stderr (the journal under systemd) and appends panics to
//! `crash.log` in its data dir; SIGTERM or Ctrl+C stops it as the app
//! quits, its session and prefs saved. `--console` takes commands typed on
//! stdin (the duck, the output, play and pause) for trying it by hand.

mod config;
mod console;
mod host;
mod lock;
mod wake;

use std::io::Write;
use std::path::PathBuf;

fn main() {
    let mut asked: Option<PathBuf> = None;
    let mut console = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => match args.next() {
                Some(p) => asked = Some(PathBuf::from(p)),
                None => usage("--config needs a file"),
            },
            "--console" => console = true,
            "--print-config" => {
                print!("{}", config::EXAMPLE);
                return;
            }
            "--version" => {
                println!("nikgobd {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => usage(&format!("unknown argument {other}")),
        }
    }
    let (cfg, from) = match config::load(asked.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("nikgobd: {e}");
            std::process::exit(2);
        }
    };
    init_log(&cfg.log);
    let paths = config::Paths::of(&cfg);
    log_panics(paths.crash_log.clone());
    log::info!("nikgobd {} starting", env!("CARGO_PKG_VERSION"));
    match &from {
        Some(p) => log::info!("config {}", p.display()),
        None => log::info!("no config file: the defaults"),
    }
    log::info!("data in {}", paths.data.display());

    let host = match host::Host::start(&cfg, paths) {
        Ok(h) => h,
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("nikgobd").enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("tokio: {e}");
            host.shutdown();
            std::process::exit(1);
        }
    };
    rt.block_on(async {
        // A server next to the player starts here, on this runtime, with
        // `host.runtime`, `host.library`, `host.settings` and
        // `host.wake.add(...)` for the state's changes.
        if console {
            tokio::select! {
                () = stopped() => {}
                () = console::run(&host.runtime) => {}
            }
        } else {
            stopped().await;
        }
    });
    log::info!("stopping");
    host.shutdown();
    drop(rt);
    log::info!("stopped");
}

fn usage(problem: &str) -> ! {
    eprintln!("nikgobd: {problem}\nusage: nikgobd [--config <file>] [--console] | --print-config | --version");
    std::process::exit(2);
}

/// Logs to stderr at the config's level; under systemd, whose journal
/// stamps each line itself, without a time of its own.
fn init_log(filters: &str) {
    let mut b = env_logger::Builder::new();
    b.parse_filters(filters);
    if std::env::var_os("JOURNAL_STREAM").is_some() {
        b.format_timestamp(None);
    }
    b.init();
}

/// Panics (thread, message, place, backtrace) are appended to `crash.log`
/// in the data dir, then logged and handled as usual.
fn log_panics(file: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let thread = std::thread::current();
        let report = format!(
            "--- panic at unix {at} in thread '{}'\n{info}\n{}\n",
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture(),
        );
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
            let _ = f.write_all(report.as_bytes());
        }
        log::error!("{info}");
        default(info);
    }));
}

/// Until SIGTERM (systemd's stop) or Ctrl+C.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => log::info!("interrupted"),
                    _ = term.recv() => log::info!("asked to stop (SIGTERM)"),
                }
            }
            Err(e) => {
                log::warn!("cannot hear SIGTERM: {e}");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        log::info!("interrupted");
    }
}
