//! The MIMO Sine bench driver: the rows of the NI loopback bench (four outputs jumpered to
//! four inputs), each a run through the control service's own protocol, each judged and
//! printed as a row with its result.
//!
//! ```text
//! # rehearsal: an in-process control service on the simulator's linear 4 by 4
//! cargo run -p sds-control --example mimo_sine_bench -- --rehearse [--python <python>] [--out <dir>]
//! # the bench: a control service already running on the NI station, its devices named
//! cargo run -p sds-control --example mimo_sine_bench -- --url ws://127.0.0.1:8765/ --ni <dev>[,<dev>...] [--python <python>]
//! ```
//!
//! The table is four control rows (inputs 1 to 4) and four drive rows (outputs 5 to 8), each
//! drive at a 0.8 V ceiling and a 0.95 V abort, every reference 0.5 in its unit: no commanded
//! drive passes 1.0 V peak, the bench contract's limit, failure cases included. On NI the drive
//! rows are the first four outputs and the control rows the first four inputs of the devices
//! named, in order; IEPE is off on every row.
//!
//! The rows: B1 tone amplitude and phase between channels (a dwell with each control's own
//! phase); B2 sweep timing; B3 dwell timing; B4 step timing; B5 the drive ceiling; B6 the change
//! cap; B7 a control abort; B8 the controlled stop; B9 the abort; B10 a law file arming and
//! running; B11 each of the five built-in laws on every test type it accepts; B12 the finished
//! run opening in the analysis side (converted by `hertzforge convert-run`).
//!
//! On the simulator the rows check the driver and the service; the physics of a loopback (the
//! tone's coherence and phase through real converters) is the bench's alone. Nothing here is
//! evidence about hardware until it has run on the bench.

use futures_util::{SinkExt, StreamExt};
use sds_control::server::{self, Service, ServiceOptions};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

/// Every commanded drive stays under this, V peak (the bench contract's 1.0 V, with margin).
const CEILING_V: f64 = 0.8;
const ABORT_V: f64 = 0.95;
const REFERENCE: f64 = 0.5;

struct Client {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next: i64,
    events: Vec<Value>,
}

impl Client {
    async fn connect(url: &str) -> Client {
        let (mut ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("connects");
        let _hello = ws.next().await;
        Client {
            ws,
            next: 1,
            events: Vec::new(),
        }
    }

    /// A request's response; events met on the way are kept.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let req = json!({"type": "request", "id": id, "method": method, "params": params});
        self.ws.send(Message::Text(req.to_string())).await.unwrap();
        loop {
            match tokio::time::timeout(Duration::from_secs(1800), self.ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["type"] == "response" && v["id"] == id {
                        return v;
                    }
                    if v["type"] == "event" {
                        self.events.push(v);
                    }
                }
                Ok(Some(Ok(Message::Binary(b)))) => {
                    if let Ok(d) = sds_control::codec::decode(&b) {
                        if d.message["type"] == "response" && d.message["id"] == id {
                            return d.message;
                        }
                        self.events.push(d.message);
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => panic!("no answer to {method}"),
            }
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params).await;
        if let Some(e) = r.get("error") {
            panic!("{method} failed: {e}");
        }
        r["result"].clone()
    }

    /// Wait until the run has finished; its state.
    async fn finished(&mut self, limit: Duration) -> Value {
        let t0 = Instant::now();
        loop {
            let s = self.ok("mimo_sine.state", json!({})).await;
            if s["state"] == "finished" || t0.elapsed() > limit {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn wait_phase(&mut self, phase: &str, limit: Duration) -> Value {
        let t0 = Instant::now();
        loop {
            let s = self.ok("mimo_sine.state", json!({})).await;
            if s["phase"] == phase || s["state"] == "finished" || t0.elapsed() > limit {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn flat(v: f64, abort_db: f64) -> Value {
    json!({"rows": [{"freq_hz": 5, "quantity": "accel_g", "value": v},
                    {"freq_hz": 2000, "quantity": "accel_g", "value": v}],
           "alarm_db": [abort_db.min(1.5), abort_db.min(1.5)], "abort_db": [abort_db, abort_db]})
}

/// A configuration of `test` with `law` on the bench's table.
fn config(test: Value, law: &str, phases: [f64; 4], abort_db: f64, rehearse: bool) -> Value {
    json!({
        "title": "Bench Row",
        "channels": {
            "controls": (0..4).map(|k| json!({"channel": k + 1, "profile": flat(REFERENCE, abort_db),
                                              "phase": [{"freq_hz": 10, "phase_deg": phases[k]}]}))
                .collect::<Vec<_>>(),
            "drives": (5..=8).map(|c| json!({"channel": c, "ceiling_vpk": CEILING_V, "abort_vpk": ABORT_V}))
                .collect::<Vec<_>>(),
        },
        "test": test,
        "identification": {"level_db": -20, "resolution_hz": 1, "frames": 8, "review": false},
        "control": {"law": {"id": law}},
        "simulator": {"structure": if rehearse { json!("sine-linear-4x4") } else { Value::Null }},
        "operator": "Bench", "article": "Loopback", "notes": "",
    })
}

fn sweep(lo: f64, hi: f64, oct: f64) -> Value {
    json!({"kind": "sweep", "band_hz": [lo, hi], "rate": {"law": "log", "oct_per_min": oct},
           "end": {"sweeps": 1}})
}

fn dwell(f: f64, s: f64) -> Value {
    json!({"kind": "dwell", "freq_hz": f, "end": {"seconds": s}})
}

fn stepped(fs: &[f64], s: f64) -> Value {
    json!({"kind": "stepped", "frequencies_hz": fs, "dwell": {"seconds": s}})
}

struct Bench {
    c: Client,
    rehearse: bool,
    rows: Vec<(String, bool, String)>,
    last_folder: Option<PathBuf>,
}

impl Bench {
    fn row(&mut self, id: &str, ok: bool, detail: String) {
        println!("{id:<44} {:<4} {detail}", if ok { "PASS" } else { "FAIL" });
        self.rows.push((id.into(), ok, detail));
    }

    /// Configure and start `cfg`; its finished state.
    async fn run(&mut self, cfg: Value) -> Value {
        let r = self
            .c
            .call("mimo_sine.configure", json!({"config": cfg}))
            .await;
        if let Some(e) = r.get("error") {
            return json!({"refused": e});
        }
        let r = self.c.call("mimo_sine.start", json!({})).await;
        if let Some(e) = r.get("error") {
            return json!({"refused": e});
        }
        let end = self.c.finished(Duration::from_secs(900)).await;
        if let Some(f) = end["record"]["folder"].as_str() {
            self.last_folder = Some(PathBuf::from(f));
        }
        end
    }

    fn cfg(&self, test: Value, law: &str) -> Value {
        config(test, law, [0.0; 4], 6.0, self.rehearse)
    }
}

fn outcome(end: &Value) -> String {
    if let Some(e) = end.get("refused") {
        return format!("refused {}", e["reason"]);
    }
    end["end"]["outcome"]
        .as_str()
        .unwrap_or("not ended")
        .to_owned()
}

fn law_file(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .map(|d| {
            d.join("wasm32-wasip2")
                .join("release")
                .join(format!("{name}.wasm"))
        })
        .find(|p| p.is_file())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let rehearse = args.iter().any(|a| a == "--rehearse");
    let python = flag("--python").unwrap_or_else(|| "python3".into());
    let url = if rehearse {
        let out = flag("--out").map(PathBuf::from).unwrap_or_else(|| {
            std::env::temp_dir().join(format!("msine-bench-{}", std::process::id()))
        });
        let opts = ServiceOptions {
            data_root: out.clone(),
            capture_root: Some(out),
            sim_speed: 0.0,
            ..ServiceOptions::default()
        };
        let svc = Service::with_options(opts);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(server::serve(listener, svc));
        format!("ws://{addr}/")
    } else {
        flag("--url").expect("--url ws://... of the control service on the NI station")
    };
    let mut c = Client::connect(&url).await;
    c.ok("session.claim", json!({"name": "MIMO Sine bench"}))
        .await;
    if rehearse {
        c.ok("station.select", json!({"kind": "sim"})).await;
    } else {
        let devices: Vec<String> = flag("--ni")
            .expect("--ni <device>[,<device>...]")
            .split(',')
            .map(str::to_owned)
            .collect();
        c.ok("station.select", json!({"kind": "ni", "devices": devices}))
            .await;
    }
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let row = |i: u32, role: &str| {
        json!({"index": i, "enabled": true, "description": format!("{role} {i}"), "role": role,
               "physical": null, "interface": "single_ended", "iepe_bias_v": null,
               "units": if role == "drive" { "V" } else { "g" }, "sensitivity_mv_per_eu": 1000,
               "range_max": if role == "drive" { 1 } else { 5 }, "polarity": "positive",
               "control_group": null,
               "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}})
    };
    let rows: Vec<Value> = (1..=4)
        .map(|i| row(i, "control"))
        .chain((5..=8).map(|i| row(i, "drive")))
        .collect();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
    let mut b = Bench {
        c,
        rehearse,
        rows: Vec::new(),
        last_folder: None,
    };
    println!(
        "MIMO Sine bench rows, {}",
        if rehearse {
            "rehearsed on the simulator"
        } else {
            "on the NI station"
        }
    );

    // B1: amplitude and phase between channels on a dwell.
    let phases = [0.0, 30.0, 60.0, 90.0];
    let end = b
        .run(config(dwell(100.0, 4.0), "matrix", phases, 6.0, rehearse))
        .await;
    let rep = b.c.ok("mimo_sine.report", json!({})).await["report"].clone();
    let worst_db = rep["controls"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|c| c["worst_error_db"].as_f64())
                .map(f64::abs)
                .fold(0.0, f64::max)
        })
        .unwrap_or(f64::NAN);
    let worst_ph = rep["controls"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|c| c["worst_phase_error_deg"].as_f64())
                .fold(0.0, f64::max)
        })
        .unwrap_or(f64::NAN);
    b.row(
        "B1 tone amplitude and phase between channels",
        outcome(&end) == "completed" && worst_db < 1.0 && worst_ph < 5.0,
        format!(
            "{}; worst error {worst_db:.2} dB, worst phase error {worst_ph:.2} deg (phases 0, 30, 60, 90)",
            outcome(&end)
        ),
    );

    // B2: sweep timing. 40 to 80 Hz at 4 oct/min is 15 s.
    let end = b.run(b.cfg(sweep(40.0, 80.0, 4.0), "matrix")).await;
    let at = end["end"]["at_s"].as_f64().unwrap_or(f64::NAN);
    b.row(
        "B2 sweep timing",
        outcome(&end) == "completed" && (at - 15.0).abs() < 0.5,
        format!("{}; the sweep ended at {at:.3} s of 15 s", outcome(&end)),
    );

    // B3: dwell timing: 5 s at 100 Hz after the settle window.
    let end = b.run(b.cfg(dwell(100.0, 5.0), "matrix")).await;
    let at = end["end"]["at_s"].as_f64().unwrap_or(f64::NAN);
    b.row(
        "B3 dwell timing",
        outcome(&end) == "completed" && (5.0..6.0).contains(&at),
        format!(
            "{}; the dwell ended at {at:.3} s (5 s and the settle window)",
            outcome(&end)
        ),
    );

    // B4: step timing: three steps of 2 s, each after its settle window.
    let end = b
        .run(b.cfg(stepped(&[60.0, 90.0, 120.0], 2.0), "matrix"))
        .await;
    let at = end["end"]["at_s"].as_f64().unwrap_or(f64::NAN);
    b.row(
        "B4 step timing",
        outcome(&end) == "completed" && end["steps"]["done"] == 3 && (6.0..9.0).contains(&at),
        format!(
            "{}; {} steps, ended at {at:.3} s (6 s and three settle windows)",
            outcome(&end),
            end["steps"]["done"]
        ),
    );

    // B5: the drive ceiling: a reference the ceiling cannot reach.
    let mut cfg = b.cfg(dwell(100.0, 3.0), "matrix");
    for k in 0..4 {
        cfg["channels"]["controls"][k]["profile"] = flat(REFERENCE * 4.0, 20.0);
        cfg["channels"]["drives"][k]["ceiling_vpk"] = json!(0.2);
        cfg["channels"]["drives"][k]["abort_vpk"] = json!(0.3);
    }
    cfg["identification"]["level_db"] = json!(-30);
    let end = b.run(cfg).await;
    let rep = b.c.ok("mimo_sine.report", json!({})).await["report"].clone();
    let peak = rep["drives"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|d| d["vpk_max"].as_f64())
                .fold(0.0, f64::max)
        })
        .unwrap_or(f64::NAN);
    let over: u64 = rep["drives"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|d| d["over_ceiling_ticks"].as_u64())
                .sum()
        })
        .unwrap_or(0);
    b.row(
        "B5 the drive ceiling",
        peak <= 0.2 + 1e-6 && over > 0,
        format!(
            "{}; largest drive {peak:.4} V against a 0.2 V ceiling, {over} ticks over it",
            outcome(&end)
        ),
    );

    // B6: the change cap: a level step of 12 dB, capped at 1 dB a tick.
    let mut cfg = b.cfg(dwell(100.0, 4.0), "matrix");
    cfg["schedule"] = json!({"levels_db": [-12, 0], "start_db": -12,
                             "per_level": {"seconds": 1}, "full_level": {"seconds": 2}});
    let end = b.run(cfg).await;
    let capped = end["law"]["capped_ticks"].as_u64().unwrap_or(0);
    b.row(
        "B6 the change cap",
        outcome(&end) == "completed" && capped > 0,
        format!("{}; a cap bound on {capped} ticks", outcome(&end)),
    );

    // B7: a control abort: an abort band no tracked tone holds.
    let end = b
        .run(config(
            dwell(100.0, 5.0),
            "matrix",
            [0.0; 4],
            0.01,
            rehearse,
        ))
        .await;
    b.row(
        "B7 a control abort",
        outcome(&end) == "aborted" && end["end"]["trip"]["kind"] == "abort_band",
        format!("{}; trip {}", outcome(&end), end["end"]["trip"]["kind"]),
    );

    // B8 and B9: the controlled stop and the abort.
    for (id, verb, want) in [
        ("B8 the controlled stop", "mimo_sine.stop", "operator_stop"),
        ("B9 the abort", "mimo_sine.abort", "operator_abort"),
    ] {
        let cfg = b.cfg(dwell(100.0, 600.0), "matrix");
        b.c.ok("mimo_sine.configure", json!({"config": cfg})).await;
        b.c.ok("mimo_sine.start", json!({})).await;
        b.c.wait_phase("engaged", Duration::from_secs(300)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        b.c.ok(verb, json!({})).await;
        let end = b.c.finished(Duration::from_secs(60)).await;
        let ramp = end["end"]["ramp_s"].clone();
        b.row(
            id,
            outcome(&end) == want,
            format!("{}; ramp {ramp}", outcome(&end)),
        );
    }

    // B10: a law file arms and runs.
    match law_file("hertzforge_law_mimo_sine_ratio") {
        None => b.row(
            "B10 a law file arms and runs",
            false,
            "the law file is not built; run scripts/build_laws.sh".into(),
        ),
        Some(p) => {
            let id =
                b.c.ok("laws.import", json!({"path": p.display().to_string()}))
                    .await["law"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
            let end = b.run(b.cfg(dwell(100.0, 3.0), &id)).await;
            b.row(
                "B10 a law file arms and runs",
                outcome(&end) == "completed",
                format!(
                    "{} (on NI it arms only once its check has passed)",
                    outcome(&end)
                ),
            );
        }
    }

    // B11: each built-in law on every test type it accepts.
    for (law, tests) in [
        ("matrix", vec!["sweep", "dwell", "stepped"]),
        ("paired", vec!["dwell", "stepped"]),
        ("steepest_descent", vec!["dwell"]),
        ("secant", vec!["dwell"]),
        ("ratio", vec!["dwell", "stepped"]),
    ] {
        for t in tests {
            let test = match t {
                "sweep" => sweep(40.0, 80.0, 4.0),
                "dwell" => dwell(100.0, 4.0),
                _ => stepped(&[60.0, 90.0, 120.0], 2.0),
            };
            let end = b.run(b.cfg(test, law)).await;
            // On a loopback every law can hold; on the simulator's coupled plate a law the
            // research record says fails there is recorded as it ran.
            b.row(
                &format!("B11 {law} on a {t}"),
                !outcome(&end).starts_with("refused") && end["end"].is_object(),
                format!(
                    "{}{}",
                    outcome(&end),
                    end["end"]["trip"]["kind"]
                        .as_str()
                        .map(|k| format!(", trip {k}"))
                        .unwrap_or_default()
                ),
            );
        }
    }

    // B12: the finished run opens in the analysis side.
    match b.last_folder.clone() {
        None => b.row(
            "B12 the finished run opens in the analysis side",
            false,
            "no run folder".into(),
        ),
        Some(f) => {
            let out = std::process::Command::new(&python)
                .args(["-m", "hertzforge", "convert-run"])
                .arg(&f)
                .output();
            let (ok, words) = match out {
                Ok(o) => (
                    o.status.success(),
                    String::from_utf8_lossy(if o.status.success() {
                        &o.stdout
                    } else {
                        &o.stderr
                    })
                    .lines()
                    .last()
                    .unwrap_or("")
                    .to_owned(),
                ),
                Err(e) => (false, e.to_string()),
            };
            let hforge = std::fs::read_dir(&f)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .any(|e| e.path().extension().is_some_and(|x| x == "hforge"))
                })
                .unwrap_or(false);
            b.row(
                "B12 the finished run opens in the analysis side",
                ok && hforge,
                format!("{}: {words}", f.display()),
            );
        }
    }
    let passed = b.rows.iter().filter(|r| r.1).count();
    println!("{passed} of {} rows passed", b.rows.len());
    // Every drive at zero at the end: no run is left live.
    let s = b.c.ok("mimo_sine.state", json!({})).await;
    assert_ne!(s["state"], "running");
}
