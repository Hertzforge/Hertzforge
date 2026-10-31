//! `sds-control [--host 127.0.0.1] [--port 8792]`: serve the control protocol.

use sds_control::{protocol, server, DEFAULT_HOST, DEFAULT_PORT};

const USAGE: &str = "usage: sds-control [--host <addr>] [--port <port>]\n\
\n\
Serves the sds/1 control protocol over WebSocket (default ws://127.0.0.1:8792/).\n\
Runs the random control engine against its Sim backend only; every run is SIM.";

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
    let listener = match tokio::net::TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("sds-control: cannot listen on {host}:{port}: {e}");
            std::process::exit(1);
        }
    };
    let addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| format!("{host}:{port}"));
    eprintln!(
        "sds-control {} listening on ws://{addr}/ (protocol {}, Sim backend only)",
        protocol::VERSION,
        protocol::PROTOCOL
    );
    if let Err(e) = server::serve(listener, server::Service::new()).await {
        eprintln!("sds-control: listener failed: {e}");
        std::process::exit(1);
    }
}
