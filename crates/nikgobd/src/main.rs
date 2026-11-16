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
//! `nikgobd token <name> [control|full|read] [owner|assistant|agent|guest]`
//! makes a token for the API while the player is stopped and prints it once.
//! `nikgobd pair <ticket>` joins the owner's devices with the ticket of
//! «Добавить устройство» (through the running player, or by itself when it
//! is stopped); `nikgobd pair-code` prints the code the running player
//! shows for «Подключить» on another device.

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
    let mut token: Option<Vec<String>> = None;
    let mut pair: Option<Vec<String>> = None;
    let mut pair_code = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if let Some(words) = pair.as_mut() {
            if a == "--config" {
                match args.next() {
                    Some(p) => asked = Some(PathBuf::from(p)),
                    None => usage("--config needs a file"),
                }
            } else {
                words.push(a);
            }
            continue;
        }
        if let Some(words) = token.as_mut() {
            if a == "--config" {
                match args.next() {
                    Some(p) => asked = Some(PathBuf::from(p)),
                    None => usage("--config needs a file"),
                }
            } else {
                words.push(a);
            }
            continue;
        }
        match a.as_str() {
            "token" => token = Some(Vec::new()),
            "pair" => pair = Some(Vec::new()),
            "pair-code" => pair_code = true,
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
    if let Some(words) = token {
        make_token(&config::Paths::of(&cfg), &words);
        return;
    }
    if let Some(words) = pair {
        pair_with(&cfg, &words);
        return;
    }
    if pair_code {
        let paths = config::Paths::of(&cfg);
        match nikgob_mesh::read_pair_code(&paths.data) {
            Some(code) => println!("{code}"),
            None => {
                eprintln!("nikgobd: no code: the player is paired already, not running, or its mesh is off");
                std::process::exit(1);
            }
        }
        return;
    }
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
    let api_wake = nikgob_api::Wake::new();
    {
        let w = api_wake.clone();
        host.wake.add(move || w.wake());
    }
    let api = match nikgob_api::start(nikgob_api::ApiContext {
        runtime: host.runtime.handle(),
        library: host.library.clone(),
        wake: api_wake.clone(),
        media_roots: Vec::new(),
    }) {
        Ok(a) => Some(a),
        Err(e) => {
            log::error!("the API did not start: {e}");
            None
        }
    };
    let mesh = if cfg.mesh {
        let ctx = nikgob_mesh::MeshContext {
            data_dir: host.paths.data.clone(),
            device_name: cfg.name.clone().unwrap_or_else(nikgob_mesh::default_name),
            kind: cfg.mesh_kind(),
            runtime: host.runtime.handle(),
            api: api.as_ref().map(nikgob_api::ApiHandle::local),
            wake: api_wake.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            headless: true,
        };
        match nikgob_mesh::start(ctx) {
            Ok(m) => Some(m),
            Err(e) => {
                log::error!("the mesh did not start: {e}");
                None
            }
        }
    } else {
        log::info!("mesh off (mesh = true in the config turns it on)");
        None
    };
    rt.block_on(async {
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
    if let Some(m) = mesh {
        m.stop();
    }
    if let Some(a) = api {
        a.stop();
    }
    host.shutdown();
    // Nothing on it is waited for (a blocked read of stdin, a server's
    // last connections): the player has stopped and saved already.
    rt.shutdown_background();
    log::info!("stopped");
}

/// `nikgobd token <name> [preset] [actor]`: a new API token in the data
/// dir's settings.toml (refused while the player runs), printed once.
fn make_token(paths: &config::Paths, words: &[String]) {
    use nikgob_core::settings::{ApiActor, ApiPreset};
    let Some(name) = words.first() else { usage("token needs a name") };
    let preset = match words.get(1).map(String::as_str) {
        None | Some("control") => ApiPreset::Control,
        Some("full") => ApiPreset::Full,
        Some("read") => ApiPreset::Read,
        Some(other) => usage(&format!("unknown preset {other}")),
    };
    let actor = match words.get(2).map(String::as_str) {
        None => match preset {
            ApiPreset::Control => ApiActor::Assistant,
            ApiPreset::Full => ApiActor::Agent,
            ApiPreset::Read => ApiActor::Owner,
        },
        Some("owner") => ApiActor::Owner,
        Some("assistant") => ApiActor::Assistant,
        Some("agent") => ApiActor::Agent,
        Some("guest") => ApiActor::Guest,
        Some(other) => usage(&format!("unknown actor {other}")),
    };
    if words.len() > 3 {
        usage("token takes a name, a preset and an actor");
    }
    if let Err(e) = std::fs::create_dir_all(&paths.data) {
        eprintln!("nikgobd: {}: {e}", paths.data.display());
        std::process::exit(1);
    }
    match nikgob_api::create_token(&paths.settings, &paths.lock, name, preset, actor) {
        Ok(secret) => println!("{secret}"),
        Err(e) => {
            eprintln!("nikgobd: {e}");
            std::process::exit(1);
        }
    }
}

/// `nikgobd pair <ticket>`: through the running player (it holds the data
/// dir's lock), or by itself while it is stopped.
fn pair_with(cfg: &config::Config, words: &[String]) {
    let ticket = match words {
        [t] => t.as_str(),
        [] => usage("pair needs the ticket of «Добавить устройство»"),
        _ => usage("pair takes one ticket"),
    };
    let paths = config::Paths::of(cfg);
    if let Err(e) = std::fs::create_dir_all(&paths.data) {
        eprintln!("nikgobd: {}: {e}", paths.data.display());
        std::process::exit(1);
    }
    let out = match lock::DataLock::acquire(&paths.lock) {
        // Not running: paired here, taken at the next start.
        Ok(_lock) => {
            let name = cfg.name.clone().unwrap_or_else(nikgob_mesh::default_name);
            nikgob_mesh::pair_offline(&paths.data, &name, cfg.mesh_kind(), ticket)
        }
        Err(_) => nikgob_mesh::pair_through_running(&paths.data, ticket, std::time::Duration::from_secs(45)),
    };
    match out {
        Ok(holder) => println!("paired with {holder}"),
        Err(why) => {
            eprintln!("nikgobd: {why}");
            std::process::exit(1);
        }
    }
}

fn usage(problem: &str) -> ! {
    eprintln!("nikgobd: {problem}\nusage: nikgobd [--config <file>] [--console] | token <name> [control|full|read] [owner|assistant|agent|guest] [--config <file>] | pair <ticket> [--config <file>] | pair-code [--config <file>] | --print-config | --version");
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
