//! `wire-probe` - the split deployment's cross-host acceptance probe.
//!
//! Runs on the FRONTEND host (the Mac) against a daemon on the backend host,
//! over the real LAN: hello as Controller -> list the backends the daemon
//! scanned -> configure + start the Data Recorder -> subscribe with a channel
//! selector -> count real binary frames -> stop -> report PASS/FAIL rows.
//!
//! This is deliberately the same shape as `tests/wire.rs`, but pointed at a
//! REMOTE url with a REAL token - the loopback tests prove the gates; this
//! probe proves the LAN. Exit 0 = every row passed.
//!
//! ```text
//! cargo run -p eigen-backendd --example wire_probe -- \
//!     --url ws://192.168.4.x:8765 --token <token> --seconds 8
//! ```

use eigen_protocol::{
    ChannelSel, Command, Doc, Envelope, Hello, PeerRole, RecorderCommand, RunRole, StreamKind,
    StreamSub, PROTOCOL_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

struct Args {
    url: String,
    token: String,
    seconds: u64,
}

fn parse() -> Args {
    let mut a = Args {
        url: "ws://127.0.0.1:8765".into(),
        token: String::new(),
        seconds: 8,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--url" => {
                i += 1;
                a.url = argv[i].clone();
            }
            "--token" => {
                i += 1;
                a.token = argv[i].clone();
            }
            "--seconds" => {
                i += 1;
                a.seconds = argv[i].parse().expect("--seconds");
            }
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(3);
            }
        }
        i += 1;
    }
    a
}

struct Rows {
    failed: u32,
}

impl Rows {
    fn row(&mut self, ok: bool, what: &str, detail: String) {
        println!(
            "  {}    {what} ({detail})",
            if ok { "PASS" } else { "FAIL" }
        );
        if !ok {
            self.failed += 1;
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = parse();
    let mut rows = Rows { failed: 0 };

    println!(
        "=== wire-probe -> {} (protocol v{PROTOCOL_VERSION}) ===",
        args.url
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&args.url)
        .await
        .expect("connect");

    // Hello as Controller.
    let hello = Hello::new(PeerRole::Ui, "wire-probe(mac)")
        .asking(RunRole::Controller)
        .with_token(&args.token);
    ws.send(Message::text(serde_json::to_string(&hello).unwrap()))
        .await
        .unwrap();
    let ack = next_text(&mut ws).await.expect("hello ack");
    let controller = ack.contains("controller");
    rows.row(
        controller,
        "hello accepted as CONTROLLER",
        ack[..ack.len().min(120)].to_string(),
    );

    // The daemon's scanned backends over the wire.
    let resp = cmd(&mut ws, 1, Command::ListBackends).await;
    let ok = resp.contains("backends") || resp.contains("ni");
    rows.row(ok, "ListBackends answers over the LAN", short(&resp));

    // Configure the recorder (defaults; the daemon holds the station).
    let cfg = Doc::new(
        eigen_protocol::SchemaKind::RecorderConfig,
        r#"{"acq":{"native_rate_hz":51200.0,"analysis_decimation":1,"block_size":2048,"window":"Hann","overlap_pct":50.0,"averaging":"Linear","average_count":4},"trigger_mode":"FreeRun","auto_rearm":0,"pretrigger_s":null,"timed_record_s":null,"watchdog_enabled":false,"watchdog_explicit":false,"overload_fraction":0.9,"frf_pair":null,"metadata":[],"data_root":null}"#,
    );
    let resp = cmd(
        &mut ws,
        2,
        Command::Recorder(RecorderCommand::Configure { config: cfg }),
    )
    .await;
    let configured = resp.contains("ok") || resp.contains("Ack") || !resp.contains("error");
    rows.row(configured, "Recorder Configure accepted", short(&resp));

    // Start monitoring.
    let mut provenance_raw: Option<String> = None;
    let resp = cmd_watch(
        &mut ws,
        3,
        Command::Recorder(RecorderCommand::StartMonitor),
        &mut provenance_raw,
    )
    .await;
    let monitoring = !resp.to_lowercase().contains("error");
    rows.row(monitoring, "Recorder StartMonitor accepted", short(&resp));

    // Subscribe: a channel SELECTOR (the v3 scale item) - first 4 channels only.
    let sub = StreamSub {
        // Spectrum, not Scope: the daemon's display stream carries the
        // recorder's SPECTRUM batches, and the forwarding branch matches on the
        // subscribed kind - a Scope subscription is silently no-frames (the
        // exact 0-frame symptom this probe chased through three real backend
        // defects before the subscription itself turned out wrong, 2026-07-30).
        kind: StreamKind::Spectrum,
        channels: ChannelSel::Ids(vec![0, 1, 2, 3]),
        max_rate_hz: Some(10.0),
        bin_decimation: None,
    };
    let resp = cmd_watch(
        &mut ws,
        4,
        Command::Subscribe { subs: vec![sub] },
        &mut provenance_raw,
    )
    .await;
    rows.row(
        !resp.to_lowercase().contains("error"),
        "Subscribe with ChannelSel::Ids accepted",
        short(&resp),
    );

    // Count BINARY frames + provenance events for the window.
    let mut frames = 0u64;
    let mut frame_bytes = 0u64;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(args.seconds);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => {
                frames += 1;
                frame_bytes += b.len() as u64;
            }
            Ok(Some(Ok(Message::Text(t)))) => {
                if t.contains("run_provenance") && provenance_raw.is_none() {
                    provenance_raw = Some(t.to_string());
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => break,
        }
    }
    rows.row(
        frames > 0,
        "binary display frames crossed the LAN",
        format!("{frames} frames, {frame_bytes} bytes in {} s", args.seconds),
    );
    rows.row(
        provenance_raw.is_some(),
        "run provenance event crossed the LAN",
        String::from("run_provenance event observed"),
    );
    // CONTENT verify, not just presence: the whole point of the provenance block
    // is what it says. runId must be a JSON STRING (a u64 loses precision crossing
    // into a JS number - the daemon rule made observable on the wire), and the
    // station/build fields must be present and non-empty. The detail line prints
    // the VALUES so the acceptance log doubles as run evidence.
    let (prov_ok, prov_detail) = match &provenance_raw {
        None => (false, String::from("no event captured")),
        Some(raw) => verify_provenance_content(raw),
    };
    rows.row(
        prov_ok,
        "provenance content verified (runId-as-string + fields)",
        prov_detail,
    );

    // Clean stop.
    let resp = cmd(&mut ws, 5, Command::Recorder(RecorderCommand::StopMonitor)).await;
    rows.row(
        !resp.to_lowercase().contains("error"),
        "StopMonitor accepted (clean remote stop)",
        short(&resp),
    );

    if rows.failed > 0 {
        println!("RESULT: FAIL ({} row(s))", rows.failed);
        std::process::exit(1);
    }
    println!("RESULT: PASS");
}

fn short(s: &str) -> String {
    s.chars().take(420).collect()
}

/// Parse the captured run_provenance event text and check its CONTENT.
/// Wire shape: an EventEnvelope whose event is externally tagged
/// `{"run_provenance":{"provenance":{"schema_version":N,"json":"<verbatim>"}}}`;
/// the inner `json` string is the daemon's `RunProvenance` in camelCase.
fn verify_provenance_content(raw: &str) -> (bool, String) {
    fn find_key<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => m
                .get(key)
                .or_else(|| m.values().find_map(|x| find_key(x, key))),
            serde_json::Value::Array(a) => a.iter().find_map(|x| find_key(x, key)),
            _ => None,
        }
    }
    let env: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return (false, format!("event text is not JSON: {e}")),
    };
    let doc_json = match find_key(&env, "run_provenance")
        .and_then(|rp| find_key(rp, "json"))
        .and_then(|j| j.as_str())
    {
        Some(j) => j,
        None => {
            return (
                false,
                String::from("no run_provenance.provenance.json in event"),
            )
        }
    };
    let p: serde_json::Value = match serde_json::from_str(doc_json) {
        Ok(v) => v,
        Err(e) => return (false, format!("provenance doc is not JSON: {e}")),
    };
    let run_id = p.get("runId");
    if run_id.is_none() {
        return (false, String::from("runId MISSING from provenance"));
    }
    if !run_id.unwrap().is_string() {
        // The precision rule violated on the wire - this is the one failure mode
        // this row exists to catch.
        return (
            false,
            format!("runId is NOT a string on the wire: {}", run_id.unwrap()),
        );
    }
    let station = p.get("stationLabel").and_then(|v| v.as_str()).unwrap_or("");
    let core = p.get("coreBuild").and_then(|v| v.as_str()).unwrap_or("");
    let badge = p.get("simulatedBadge").and_then(serde_json::Value::as_bool);
    if station.is_empty() || core.is_empty() || badge.is_none() {
        return (
            false,
            format!(
                "field(s) missing/empty: stationLabel={station:?} coreBuild={core:?} simulatedBadge={badge:?}"
            ),
        );
    }
    (
        true,
        format!(
            "runId={} (string) | station={} | simulatedBadge={} | core={}",
            p["runId"],
            station,
            badge.unwrap(),
            core
        ),
    )
}

async fn next_text(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Option<String> {
    while let Some(m) = ws.next().await {
        match m.ok()? {
            Message::Text(t) => return Some(t.to_string()),
            Message::Binary(_) => continue,
            _ => continue,
        }
    }
    None
}

async fn cmd(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
    c: Command,
) -> String {
    cmd_watch(ws, id, c, &mut None).await
}

/// `cmd`, but events drained while waiting are CHECKED for the provenance
/// marker instead of discarded blind - the RunProvenance event fires at
/// StartMonitor, i.e. DURING a command round-trip, not after Subscribe.
async fn cmd_watch(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
    c: Command,
    provenance_raw: &mut Option<String>,
) -> String {
    let env = Envelope::new(id, c);
    ws.send(Message::text(serde_json::to_string(&env).unwrap()))
        .await
        .unwrap();
    // The next TEXT frame that is a response to us (skip events/binary).
    loop {
        match next_text(ws).await {
            Some(t) => {
                if t.contains("run_provenance") && provenance_raw.is_none() {
                    *provenance_raw = Some(t.clone());
                }
                // Match THIS command's id, not any envelope with an id - the
                // event envelopes carry ids too, and a coinciding seq made the
                // provenance event masquerade as the Subscribe response in the
                // first passing acceptance log (2026-07-30; cosmetic, but an
                // evidence log should say what it means).
                if t.contains(&format!("\"id\":{id},")) || t.contains("error") {
                    return t;
                }
                // an event - keep draining
            }
            None => return String::from("<connection closed>"),
        }
    }
}
