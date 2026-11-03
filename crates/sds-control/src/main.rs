//! `sds-control [--host 127.0.0.1] [--port 8792] [--watch-stdin]`: serve the control
//! protocol.
//!
//! The process does not end with a run live. Told to stop (Ctrl-C, SIGTERM or SIGHUP
//! on Unix; Ctrl-C, Ctrl-Break or a closed console on Windows; and with
//! `--watch-stdin` the end of its standard input) it shuts the service down
//! (`Service::shutdown`): a live run ends with its controlled stop, and only then does
//! the process exit. A killed process can leave an NI output holding its last sample,
//! so a parent that starts this service (the desktop app) stops it by closing the pipe
//! on its standard input, never by killing it, and the operating system closes that
//! pipe for the parent if the parent dies.

use sds_control::{protocol, server, DEFAULT_HOST, DEFAULT_PORT};
use std::io::{Read, Write};
use std::time::Duration;
use tokio::sync::oneshot;

const USAGE: &str = "usage: sds-control [--host <addr>] [--port <port>] [--watch-stdin]\n\
\n\
Serves the sds/1 control protocol over WebSocket (default ws://127.0.0.1:8792/), on\n\
loopback only. Runs closed-loop Random on the simulator or on NI-DAQmx hardware.\n\
\n\
Interrupted or terminated, it stops any live run with its controlled stop and exits\n\
once the run has ended. --watch-stdin does the same when standard input closes, for\n\
a parent process that starts the service: it closes the pipe to stop it, and the\n\
pipe closes by itself if the parent dies.";

/// How long the last events have to reach the screens before the process ends.
const FLUSH: Duration = Duration::from_millis(300);

/// One line to standard error that cannot fail the process: once a parent has gone,
/// its end of a pipe may be closed, and `eprintln!` would panic.
fn say(line: &str) {
    let _ = writeln!(std::io::stderr(), "{line}");
}

struct Args {
    host: String,
    port: u16,
    watch_stdin: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut out = Args {
        host: DEFAULT_HOST.to_owned(),
        port: DEFAULT_PORT,
        watch_stdin: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_owned(), Some(v.to_owned())),
            None => (arg.clone(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{name} needs a value\n\n{USAGE}"))
        };
        match flag.as_str() {
            "--host" => out.host = value("--host")?,
            "--port" => {
                let v = value("--port")?;
                out.port = v
                    .parse()
                    .map_err(|_| format!("--port {v} is not a port number\n\n{USAGE}"))?;
            }
            "--watch-stdin" if inline.is_none() => out.watch_stdin = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }
    Ok(out)
}

/// The end of standard input, read on a thread of its own: a blocking read behaves
/// the same on every platform, and a broken pipe counts as the end.
fn watch_stdin() -> std::io::Result<oneshot::Receiver<()>> {
    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("stdin-watch".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = [0u8; 256];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = tx.send(());
        })?;
    Ok(rx)
}

/// The signals that ask the process to stop. Registered before the service serves,
/// so none of them can end it the default way.
#[cfg(unix)]
struct Signals {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> std::io::Result<Signals> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Signals {
            term: signal(SignalKind::terminate())?,
            int: signal(SignalKind::interrupt())?,
            hup: signal(SignalKind::hangup())?,
        })
    }

    async fn next(&mut self) -> &'static str {
        tokio::select! {
            _ = self.term.recv() => "terminated (SIGTERM)",
            _ = self.int.recv() => "interrupted (SIGINT)",
            _ = self.hup.recv() => "hung up (SIGHUP)",
        }
    }
}

#[cfg(windows)]
struct Signals {
    c: tokio::signal::windows::CtrlC,
    brk: tokio::signal::windows::CtrlBreak,
    close: tokio::signal::windows::CtrlClose,
}

#[cfg(windows)]
impl Signals {
    fn new() -> std::io::Result<Signals> {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};
        Ok(Signals {
            c: ctrl_c()?,
            brk: ctrl_break()?,
            close: ctrl_close()?,
        })
    }

    async fn next(&mut self) -> &'static str {
        tokio::select! {
            _ = self.c.recv() => "interrupted (Ctrl-C)",
            _ = self.brk.recv() => "interrupted (Ctrl-Break)",
            _ = self.close.recv() => "its console closed",
        }
    }
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let (host, port) = (args.host, args.port);
    if let Err(e) = server::check_loopback(&host) {
        eprintln!("sds-control: {e}");
        std::process::exit(2);
    }
    let mut signals = match Signals::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sds-control: cannot handle the stop signals: {e}");
            std::process::exit(1);
        }
    };
    let stdin_closed = if args.watch_stdin {
        match watch_stdin() {
            Ok(rx) => Some(rx),
            Err(e) => {
                eprintln!("sds-control: cannot watch standard input: {e}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    let listener = match tokio::net::TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("sds-control: cannot listen on {host}:{port}: {e}");
            std::process::exit(1);
        }
    };
    // Bound: check what the name resolved to as well as the name.
    let local = match listener.local_addr() {
        Ok(a) if a.ip().is_loopback() => a,
        Ok(a) => {
            eprintln!(
                "sds-control: {host} resolved to {a}, which is not a loopback address; refusing \
                 to serve"
            );
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("sds-control: cannot read the bound address: {e}");
            std::process::exit(1);
        }
    };
    say(&format!(
        "sds-control {} listening on ws://{local}/ (protocol {})",
        protocol::VERSION,
        protocol::PROTOCOL
    ));
    let options = server::ServiceOptions {
        bind: format!("ws://{local}/"),
        ..server::ServiceOptions::default()
    };
    let service = server::Service::with_options(options);
    // The service goes on serving through the shutdown below, so every screen sees
    // the run end and keeps Stop and Abort.
    let mut serving = tokio::spawn(server::serve(listener, service.clone()));
    let stdin_end = async {
        match stdin_closed {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    let why = tokio::select! {
        r = &mut serving => {
            match r {
                Ok(Err(e)) => say(&format!("sds-control: listener failed: {e}")),
                _ => say("sds-control: the listener ended"),
            }
            std::process::exit(1);
        }
        _ = stdin_end => "standard input closed",
        why = signals.next() => why,
    };
    say(&format!("sds-control: {why}; shutting down"));
    // Asked again, nothing changes: the run's controlled stop is already under way.
    tokio::spawn(async move {
        loop {
            let again = signals.next().await;
            say(&format!(
                "sds-control: {again} while shutting down; waiting for the run's controlled stop"
            ));
        }
    });
    match service.shutdown().await {
        Some(what) => say(&format!("sds-control: {what}")),
        None => say("sds-control: no run was live"),
    }
    tokio::time::sleep(FLUSH).await;
    say("sds-control: stopped");
}
