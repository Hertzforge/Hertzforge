//! `sds-control [--host 127.0.0.1] [--port 8792]`: serve the control protocol.

use sds_control::{protocol, server, DEFAULT_HOST, DEFAULT_PORT};

const USAGE: &str = "usage: sds-control [--host <addr>] [--port <port>]\n\
\n\
Serves the sds/1 control protocol over WebSocket (default ws://127.0.0.1:8792/), on\n\
loopback only. Runs closed-loop Random on the simulator or on NI-DAQmx hardware.";

fn parse_args() -> Result<(String, u16), String> {
    let mut host = DEFAULT_HOST.to_owned();
    let mut port = DEFAULT_PORT;
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
            "--host" => host = value("--host")?,
            "--port" => {
                let v = value("--port")?;
                port = v
                    .parse()
                    .map_err(|_| format!("--port {v} is not a port number\n\n{USAGE}"))?;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }
    Ok((host, port))
}

#[tokio::main]
async fn main() {
    let (host, port) = match parse_args() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = server::check_loopback(&host) {
        eprintln!("sds-control: {e}");
        std::process::exit(2);
    }
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
    eprintln!(
        "sds-control {} listening on ws://{local}/ (protocol {})",
        protocol::VERSION,
        protocol::PROTOCOL
    );
    let options = server::ServiceOptions {
        bind: format!("ws://{local}/"),
        ..server::ServiceOptions::default()
    };
    if let Err(e) = server::serve(listener, server::Service::with_options(options)).await {
        eprintln!("sds-control: listener failed: {e}");
        std::process::exit(1);
    }
}
