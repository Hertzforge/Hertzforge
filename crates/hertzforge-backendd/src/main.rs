//! `hertzforge-backendd` - the CLI.
//!
//! ```text
//! hertzforge-backendd --config <path>              serve until stopped
//! hertzforge-backendd --config <path> --stop       stop the daemon holding that data root
//! hertzforge-backendd --config <path> --soak 1800  run the soak harness for N seconds
//! hertzforge-backendd --config <path> --check      validate config + preflight, then exit
//! ```
//!
//! # Why start/stop is a lockfile and a signal rather than a service manager
//!
//! The daemon has to be startable and stoppable by one person at a bench, on three
//! platforms, without root. A pidfile the process already writes for MUTUAL EXCLUSION
//! ([`hertzforge_backendd::lock`]) answers "who is running" for free, so `--stop` is a
//! read of that file plus a `SIGTERM`. Under systemd on the box this is still correct:
//! systemd stops it the same way.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use hertzforge_backendd::{build, config::DaemonConfig, lock, preflight};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("HERTZFORGE_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("hertzforge=debug,info")),
        )
        .init();

    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let cfg = match DaemonConfig::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hertzforge-backendd: {e}");
            return ExitCode::FAILURE;
        }
    };

    if args.stop {
        return stop(&cfg);
    }

    // THE PREFLIGHT, before the socket exists: a failure belongs in this log, at
    // start, not at the remote operator's first configure.
    let pf = preflight::run(cfg.backend_kind(), &cfg.ni_devices);
    tracing::info!(target: "hertzforge::backendd", "{}", pf.report());
    if !pf.is_usable() && cfg.preflight_required {
        eprintln!("hertzforge-backendd: {}", pf.report());
        return ExitCode::FAILURE;
    }

    // THE ADVISORY LOCK, before anything opens a device.
    let data_root = cfg.resolved_data_root();
    let _guard = match lock::acquire(&data_root, &cfg.bind, &cfg.station) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("hertzforge-backendd: {e}");
            return ExitCode::FAILURE;
        }
    };

    if args.check {
        println!("hertzforge-backendd: config OK, {}", pf.report());
        return ExitCode::SUCCESS;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("hertzforge-backendd: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move {
        let server = build(cfg);
        match args.soak_s {
            Some(secs) => match hertzforge_backendd::soak::run(
                Arc::clone(&server),
                Duration::from_secs_f64(secs),
            )
            .await
            {
                Ok(report) => {
                    println!("{}", report.render());
                    if report.passed() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(e) => {
                    eprintln!("hertzforge-backendd: the soak could not run: {e}");
                    ExitCode::FAILURE
                }
            },
            None => {
                let (listener, addr) = match server.bind().await {
                    Ok(x) => x,
                    Err(e) => {
                        eprintln!(
                            "hertzforge-backendd: cannot bind {}: {e}",
                            server.cfg.bind
                        );
                        return ExitCode::FAILURE;
                    }
                };
                tracing::info!(
                    target: "hertzforge::backendd",
                    "serving hertzforge-protocol v{} on ws://{addr}/ (station {}, \
                     controller timeout {:.0} s)",
                    hertzforge_protocol::PROTOCOL_VERSION,
                    server.cfg.station,
                    server.cfg.controller_timeout_s
                );
                let serving = Arc::clone(&server);
                tokio::spawn(async move { serving.serve(listener).await });
                // SIGTERM (what `--stop` sends) and Ctrl-C both end the process, and
                // the lock guard's Drop releases the advisory lock on the way out.
                // Windows has no SIGTERM (and `--stop` is unix-only), so Ctrl-C is
                // the stop there.
                #[cfg(unix)]
                {
                    let mut term = match tokio::signal::unix::signal(
                        tokio::signal::unix::SignalKind::terminate(),
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("hertzforge-backendd: cannot install the signal handler: {e}");
                            return ExitCode::FAILURE;
                        }
                    };
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = term.recv() => {}
                    }
                }
                #[cfg(not(unix))]
                {
                    if let Err(e) = tokio::signal::ctrl_c().await {
                        eprintln!("hertzforge-backendd: cannot install the signal handler: {e}");
                        return ExitCode::FAILURE;
                    }
                }
                tracing::info!(target: "hertzforge::backendd", "stopping; the run is brought down and the lock released");
                server.core.recorder.safe_stop("the daemon is stopping");
                ExitCode::SUCCESS
            }
        }
    })
}

/// `--stop`: read the lockfile and signal its holder.
fn stop(cfg: &DaemonConfig) -> ExitCode {
    let root = cfg.resolved_data_root();
    let Some(holder) = lock::read_live(&root) else {
        println!("hertzforge-backendd: no daemon holds {}", root.display());
        return ExitCode::SUCCESS;
    };
    #[cfg(unix)]
    {
        // `kill(2)` through the shell rather than a libc binding: this crate is
        // `#![forbid(unsafe_code)]` and a raw `libc::kill` would need an unsafe
        // block, which is not worth relaxing a crate-wide invariant for a one-line
        // signal on an operator command.
        let out = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(holder.pid.to_string())
            .status();
        match out {
            Ok(s) if s.success() => {
                println!("hertzforge-backendd: sent SIGTERM to pid {}", holder.pid);
                ExitCode::SUCCESS
            }
            _ => {
                eprintln!(
                    "hertzforge-backendd: could not signal pid {}. {}",
                    holder.pid,
                    holder.advice()
                );
                ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Windows has no SIGTERM to send, and `taskkill /F` would skip the safe stop
        // (a killed process can leave an NI output holding its last sample), so the
        // honest answer is the console's own Ctrl-C.
        eprintln!(
            "hertzforge-backendd: --stop works only on Linux and macOS. On Windows, stop the \
             daemon (pid {}) with Ctrl-C in its own console, which runs the safe stop; \
             `taskkill /F` skips it.",
            holder.pid
        );
        ExitCode::FAILURE
    }
}

const USAGE: &str = "\
usage: hertzforge-backendd --config <path> [--stop | --soak <seconds> | --check]

  --config <path>   the daemon TOML (required; it must name an explicit bind
                    interface and a non-empty shared token)
  --stop            signal the daemon holding this config's data root, then exit
  --soak <seconds>  run the soak harness for N seconds and print the report
  --check           validate the config, run the preflight, and exit";

struct Args {
    config: PathBuf,
    stop: bool,
    check: bool,
    soak_s: Option<f64>,
}

impl Args {
    fn parse(it: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut config = None;
        let mut stop = false;
        let mut check = false;
        let mut soak_s = None;
        let mut it = it.peekable();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--config" => {
                    config = Some(PathBuf::from(it.next().ok_or("--config needs a path")?))
                }
                "--stop" => stop = true,
                "--check" => check = true,
                "--soak" => {
                    let v = it.next().ok_or("--soak needs a number of seconds")?;
                    soak_s = Some(
                        v.parse::<f64>()
                            .map_err(|_| format!("--soak needs a number, got {v:?}"))?,
                    )
                }
                "-h" | "--help" => return Err("".into()),
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(Args {
            config: config.ok_or("--config is required")?,
            stop,
            check,
            soak_s,
        })
    }
}
