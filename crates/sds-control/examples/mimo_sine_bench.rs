//! The MIMO Sine bench driver: the rows of the NI loopback bench (four outputs jumpered to
//! four inputs), each a run through the control service's own protocol, each judged and
//! printed as a row with its result.
//!
//! ```text
//! # rehearsal: an in-process control service on the simulator's loopback 4 by 4
//! cargo run -p sds-control --example mimo_sine_bench -- --rehearse [--python <python>] [--out <dir>]
//! # the bench: a control service already running on the NI station, its devices named
//! cargo run -p sds-control --example mimo_sine_bench -- --url ws://127.0.0.1:8765/ --ni <dev>[,<dev>...] [--python <python>]
//! ```
//!
//! The table is four control rows (inputs 1 to 4) and four drive rows (outputs 5 to 8), each
//! drive at a 0.8 V ceiling or under and a 0.95 V abort, every reference 0.5 in its unit: no
//! commanded drive passes 0.8 V peak, under the bench contract's 1.0 V, failure rows included.
//! Before anything is configured on NI the driver checks every row's configuration and refuses
//! to start if any drive's ceiling is over 0.8 V or its abort over 1.0 V (or either is missing);
//! every row's largest drive is read from its report and a row over 0.8 V fails. On NI the drive
//! rows are the first four outputs and the control rows the first four inputs of the devices
//! named, in order; IEPE is off on every row.
//!
//! The rows: B1 tone amplitude and phase between channels (a dwell with each control's own
//! phase); B2 sweep timing; B3 dwell timing; B4 step timing; B5 the drive ceiling; B6 the change
//! cap; B7 a control abort; B8 the controlled stop; B9 the abort; B10 a law file arming and
//! running; B11 each of the five built-in laws on every test type it accepts (the types read
//! from `laws.list`), completed with its worst control error within 1.0 dB, the tolerance for an
//! uncoupled loopback; B12 the level ladder (-12, -6 and 0 dB); B13 hold, resume and a manual
//! frequency; B14 the finished run opening in the analysis side (converted by
//! `hertzforge convert-run`).
//!
//! The rehearsal runs on the simulator's `sine-loopback-4x4`, the bench's twin: four outputs
//! wired straight to four inputs (unity gain, a 0.8 ms delay, no coupling). On the simulator the
//! rows check the driver and the service; the physics of a loopback (the tone's coherence and
//! phase through real converters) is the bench's alone. Nothing here is evidence about hardware
//! until it has run on the bench.

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
/// The bench contract's limit, V peak: no row may be able to command a drive past it.
const CONTRACT_V: f64 = 1.0;
/// The worst control error a law may leave on an uncoupled loopback, dB (B11, B12).
const LOOPBACK_TOLERANCE_DB: f64 = 1.0;
/// The simulator's twin of the bench.
const TWIN: &str = "sine-loopback-4x4";

/// Why `cfg` could command a drive past what the bench allows, if it could: a drive with no
/// ceiling or abort, a ceiling over [`CEILING_V`], or an abort over [`CONTRACT_V`].
fn could_pass(cfg: &Value) -> Option<String> {
    let drives = cfg["channels"]["drives"].as_array()?;
    if drives.is_empty() {
        return Some("no drive rows".into());
    }
    for d in drives {
        let ch = &d["channel"];
        match (d["ceiling_vpk"].as_f64(), d["abort_vpk"].as_f64()) {
            (Some(c), Some(a)) if c <= CEILING_V && a <= CONTRACT_V && c <= a => {}
            (c, a) => {
                return Some(format!(
                    "drive {ch}: ceiling {c:?} V, abort {a:?} V (at most {CEILING_V} V and {CONTRACT_V} V)"
                ))
            }
        }
    }
    None
}

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
        "simulator": {"structure": if rehearse { json!(TWIN) } else { Value::Null }},
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

    /// Configure `cfg`, after the bench's drive check (on NI a configuration that could pass
    /// the bench's limit ends the driver before it is sent).
    async fn configure(&mut self, cfg: Value) -> Value {
        if let Some(why) = could_pass(&cfg) {
            if !self.rehearse {
                eprintln!("refused: a row's configuration could pass {CONTRACT_V} V: {why}");
                std::process::exit(2);
            }
            return json!({"error": {"reason": "bench_drive_check", "message": why}});
        }
        self.c
            .call("mimo_sine.configure", json!({"config": cfg}))
            .await
    }

    /// The largest drive the last run played, V peak, from its report.
    async fn drive_peak(&mut self) -> f64 {
        let rep = self.c.ok("mimo_sine.report", json!({})).await["report"].clone();
        rep["drives"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|d| d["vpk_max"].as_f64())
                    .fold(0.0, f64::max)
            })
            .unwrap_or(0.0)
    }

    /// The worst control error of the last run's report, dB (outside settle windows).
    async fn worst_db(&mut self) -> f64 {
        let rep = self.c.ok("mimo_sine.report", json!({})).await["report"].clone();
        rep["controls"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|c| c["worst_error_db"].as_f64())
                    .map(f64::abs)
                    .fold(0.0, f64::max)
            })
            .unwrap_or(f64::NAN)
    }

    /// A row, failed as well when its run played a drive over [`CEILING_V`].
    async fn judged_row(&mut self, id: &str, ok: bool, detail: String) {
        let peak = self.drive_peak().await;
        let under = peak <= CEILING_V + 1e-9;
        self.row(
            id,
            ok && under,
            format!("{detail}; largest drive {peak:.4} V"),
        );
    }

    /// Configure and start `cfg`; its finished state.
    async fn run(&mut self, cfg: Value) -> Value {
        let r = self.configure(cfg).await;
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

/// Each built-in law and the test types its manifest accepts, as `laws.list` gives them.
async fn law_types(c: &mut Client) -> Vec<(String, Vec<String>)> {
    let listed = c.ok("laws.list", json!({"world": "sine_mimo"})).await;
    ["matrix", "paired", "steepest_descent", "secant", "ratio"]
        .iter()
        .map(|id| {
            let e = listed["laws"]
                .as_array()
                .and_then(|v| v.iter().find(|l| l["id"] == *id))
                .unwrap_or_else(|| panic!("{id} is not listed"));
            let types = e["test_types"]
                .as_array()
                .map(|v| {
                    v.iter()
                        .filter_map(|t| t.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            ((*id).to_owned(), types)
        })
        .collect()
}

/// Every row's configuration by row id.
fn row_configs(rehearse: bool, laws: &[(String, Vec<String>)]) -> Vec<(String, Value)> {
    let base = |test: Value, law: &str| config(test, law, [0.0; 4], 6.0, rehearse);
    let mut out = vec![
        (
            "B1".to_owned(),
            config(
                dwell(100.0, 4.0),
                "matrix",
                [0.0, 30.0, 60.0, 90.0],
                6.0,
                rehearse,
            ),
        ),
        ("B2".to_owned(), base(sweep(40.0, 80.0, 4.0), "matrix")),
        ("B3".to_owned(), base(dwell(100.0, 5.0), "matrix")),
        (
            "B4".to_owned(),
            base(stepped(&[60.0, 90.0, 120.0], 2.0), "matrix"),
        ),
    ];
    let mut b5 = base(dwell(100.0, 3.0), "matrix");
    for k in 0..4 {
        b5["channels"]["controls"][k]["profile"] = flat(REFERENCE * 2.0, 20.0);
        b5["channels"]["drives"][k]["ceiling_vpk"] = json!(0.2);
        b5["channels"]["drives"][k]["abort_vpk"] = json!(0.3);
    }
    b5["identification"]["level_db"] = json!(-30);
    out.push(("B5".into(), b5));
    // B6: the reference rises 12 dB (0.025 to 0.1) from 59.9 to 60 Hz. At four octaves a
    // minute the sweep crosses that tenth of a hertz in under four updates, so following it
    // asks for more than the change cap's 1 dB an update and the cap binds. The largest
    // reference here is 0.1, so every drive stays far under its ceiling.
    let mut b6 = base(sweep(40.0, 80.0, 4.0), "matrix");
    for k in 0..4 {
        b6["channels"]["controls"][k]["profile"] = json!({
            "rows": [{"freq_hz": 5, "quantity": "accel_g", "value": 0.025},
                     {"freq_hz": 59.9, "quantity": "accel_g", "value": 0.025},
                     {"freq_hz": 60, "quantity": "accel_g", "value": 0.1},
                     {"freq_hz": 2000, "quantity": "accel_g", "value": 0.1}],
            "alarm_db": [20, 20], "abort_db": [20, 20]});
    }
    out.push(("B6".into(), b6));
    // B7: the ceilings (0.3 V) hold every control 4.4 dB under its 0.5 reference; the abort
    // band is 3 dB.
    let mut b7 = config(dwell(100.0, 5.0), "matrix", [0.0; 4], 3.0, rehearse);
    for k in 0..4 {
        b7["channels"]["drives"][k]["ceiling_vpk"] = json!(0.3);
        b7["channels"]["drives"][k]["abort_vpk"] = json!(0.4);
    }
    out.push(("B7".into(), b7));
    out.push(("B8".into(), base(dwell(100.0, 600.0), "matrix")));
    // B10's law is the imported file's id, set when it is imported.
    out.push(("B10".into(), base(dwell(100.0, 3.0), "matrix")));
    for (law, types) in laws {
        for t in types {
            let test = match t.as_str() {
                "sweep" => sweep(40.0, 80.0, 4.0),
                "dwell" => dwell(100.0, 4.0),
                _ => stepped(&[60.0, 90.0, 120.0], 2.0),
            };
            out.push((format!("B11 {law} {t}"), base(test, law)));
        }
    }
    let mut b12 = base(dwell(100.0, 6.0), "matrix");
    b12["schedule"] = json!({"mode": "auto", "levels_db": [-12, -6, 0], "start_db": -12,
                             "per_level": {"seconds": 2}, "full_level": {"seconds": 2}});
    out.push(("B12".into(), b12));
    out.push(("B13".into(), base(sweep(40.0, 80.0, 0.5), "matrix")));
    out
}

/// B13: on a sweep, hold (the frequency stays), go to 60 Hz while held, resume (the sweep
/// moves on from there), then the controlled stop.
async fn hold_resume_goto(b: &mut Bench, cfg: Value) -> (bool, String) {
    let r = b.configure(cfg).await;
    if let Some(e) = r.get("error") {
        return (false, format!("refused {}", e["reason"]));
    }
    b.c.ok("mimo_sine.start", json!({})).await;
    b.c.wait_phase("engaged", Duration::from_secs(300)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let held = b.c.ok("mimo_sine.hold", json!({})).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let f0 = b.c.ok("mimo_sine.state", json!({})).await["frequency_hz"]
        .as_f64()
        .unwrap_or(f64::NAN);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let f1 = b.c.ok("mimo_sine.state", json!({})).await["frequency_hz"]
        .as_f64()
        .unwrap_or(f64::NAN);
    let went = b.c.call("mimo_sine.goto", json!({"hz": 60.0})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let f2 = b.c.ok("mimo_sine.state", json!({})).await["frequency_hz"]
        .as_f64()
        .unwrap_or(f64::NAN);
    b.c.ok("mimo_sine.resume", json!({})).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let s3 = b.c.ok("mimo_sine.state", json!({})).await;
    let f3 = s3["frequency_hz"].as_f64().unwrap_or(f64::NAN);
    b.c.ok("mimo_sine.stop", json!({})).await;
    let end = b.c.finished(Duration::from_secs(60)).await;
    let ok = held["held"] == true
        && (f1 - f0).abs() < 1e-6
        && went.get("error").is_none()
        && (f2 - 60.0).abs() < 0.5
        && s3["held"] == false
        && f3 > f2
        && matches!(outcome(&end).as_str(), "operator_stop" | "completed");
    (
        ok,
        format!(
            "{}; held at {f0:.3} Hz and still {f1:.3} Hz 0.5 s later, went to {f2:.3} Hz, resumed to {f3:.3} Hz",
            outcome(&end)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_bench_every_row_stays_under_the_bench_limit() {
        let laws: Vec<(String, Vec<String>)> = vec![
            (
                "matrix".into(),
                vec!["sweep".into(), "dwell".into(), "stepped".into()],
            ),
            ("paired".into(), vec!["dwell".into(), "stepped".into()]),
        ];
        for rehearse in [true, false] {
            for (id, cfg) in row_configs(rehearse, &laws) {
                assert_eq!(could_pass(&cfg), None, "{id}");
            }
        }
    }

    #[test]
    fn mimo_sine_bench_a_row_that_could_pass_the_limit_is_refused() {
        let ok = config(dwell(100.0, 1.0), "matrix", [0.0; 4], 6.0, false);
        assert_eq!(could_pass(&ok), None);
        for (key, v) in [
            ("ceiling_vpk", json!(0.9)),
            ("abort_vpk", json!(1.2)),
            ("abort_vpk", Value::Null),
        ] {
            let mut bad = ok.clone();
            bad["channels"]["drives"][2][key] = v.clone();
            assert!(could_pass(&bad).is_some(), "{key} {v} was taken");
        }
    }
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
    // Every row's configuration, checked before anything is configured: on NI the driver
    // refuses to start if any could command a drive past the bench's limit.
    let laws = law_types(&mut c).await;
    let plan = row_configs(rehearse, &laws);
    let bad: Vec<String> = plan
        .iter()
        .filter_map(|(id, cfg)| could_pass(cfg).map(|w| format!("{id}: {w}")))
        .collect();
    if !bad.is_empty() {
        eprintln!(
            "refused to start: a row's configuration could pass {CONTRACT_V} V:\n  {}",
            bad.join("\n  ")
        );
        if !rehearse {
            std::process::exit(2);
        }
    }
    let cfg_of = |id: &str| {
        plan.iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("no configuration for {id}"))
    };
    let mut b = Bench {
        c,
        rehearse,
        rows: Vec::new(),
        last_folder: None,
    };
    println!(
        "MIMO Sine bench rows, {}",
        if rehearse {
            "rehearsed on the simulator's sine-loopback-4x4"
        } else {
            "on the NI station"
        }
    );
    println!(
        "{} row configurations checked: every drive ceiling at most {CEILING_V} V, every abort at most {CONTRACT_V} V",
        plan.len()
    );

    // B1: amplitude and phase between channels on a dwell.
    let end = b.run(cfg_of("B1")).await;
    let worst_db = b.worst_db().await;
    let rep = b.c.ok("mimo_sine.report", json!({})).await["report"].clone();
    let worst_ph = rep["controls"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|c| c["worst_phase_error_deg"].as_f64())
                .fold(0.0, f64::max)
        })
        .unwrap_or(f64::NAN);
    b.judged_row(
        "B1 tone amplitude and phase between channels",
        outcome(&end) == "completed" && worst_db < 1.0 && worst_ph < 5.0,
        format!(
            "{}; worst error {worst_db:.2} dB, worst phase error {worst_ph:.2} deg (phases 0, 30, 60, 90)",
            outcome(&end)
        ),
    )
    .await;

    // B2: sweep timing. 40 to 80 Hz at 4 oct/min is 15 s.
    let end = b.run(cfg_of("B2")).await;
    let at = end["end"]["at_s"].as_f64().unwrap_or(f64::NAN);
    b.judged_row(
        "B2 sweep timing",
        outcome(&end) == "completed" && (at - 15.0).abs() < 0.5,
        format!("{}; the sweep ended at {at:.3} s of 15 s", outcome(&end)),
    )
    .await;

    // B3: dwell timing: 5 s at 100 Hz after the settle window.
    let end = b.run(cfg_of("B3")).await;
    let at = end["end"]["at_s"].as_f64().unwrap_or(f64::NAN);
    b.judged_row(
        "B3 dwell timing",
        outcome(&end) == "completed" && (5.0..6.0).contains(&at),
        format!(
            "{}; the dwell ended at {at:.3} s (5 s and the settle window)",
            outcome(&end)
        ),
    )
    .await;

    // B4: step timing: three steps of 2 s, each after its settle window.
    let end = b.run(cfg_of("B4")).await;
    let at = end["end"]["at_s"].as_f64().unwrap_or(f64::NAN);
    b.judged_row(
        "B4 step timing",
        outcome(&end) == "completed" && end["steps"]["done"] == 3 && (6.0..9.0).contains(&at),
        format!(
            "{}; {} steps, ended at {at:.3} s (6 s and three settle windows)",
            outcome(&end),
            end["steps"]["done"]
        ),
    )
    .await;

    // B5: the drive ceiling: a reference the ceiling cannot reach.
    let end = b.run(cfg_of("B5")).await;
    let rep = b.c.ok("mimo_sine.report", json!({})).await["report"].clone();
    let peak = b.drive_peak().await;
    let over: u64 = rep["drives"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|d| d["over_ceiling_ticks"].as_u64())
                .sum()
        })
        .unwrap_or(0);
    b.judged_row(
        "B5 the drive ceiling",
        peak <= 0.2 + 1e-6 && over > 0,
        format!(
            "{} (trip {}); largest drive {peak:.4} V against a 0.2 V ceiling, {over} ticks over it",
            outcome(&end),
            end["end"]["trip"]["kind"]
        ),
    )
    .await;

    // B6: the change cap: a reference that jumps 12 dB within four updates.
    let end = b.run(cfg_of("B6")).await;
    let capped = end["law"]["capped_ticks"].as_u64().unwrap_or(0);
    let worst = b.worst_db().await;
    b.judged_row(
        "B6 the change cap",
        outcome(&end) == "completed" && capped > 0,
        format!(
            "{}; a cap bound on {capped} ticks, worst error {worst:.2} dB",
            outcome(&end)
        ),
    )
    .await;

    // B7: a control abort: ceilings that hold every control under its abort band.
    let end = b.run(cfg_of("B7")).await;
    b.judged_row(
        "B7 a control abort",
        outcome(&end) == "aborted" && end["end"]["trip"]["kind"] == "abort_band",
        format!("{}; trip {}", outcome(&end), end["end"]["trip"]["kind"]),
    )
    .await;

    // B8 and B9: the controlled stop and the abort.
    for (id, verb, want) in [
        ("B8 the controlled stop", "mimo_sine.stop", "operator_stop"),
        ("B9 the abort", "mimo_sine.abort", "operator_abort"),
    ] {
        let r = b.configure(cfg_of("B8")).await;
        assert!(r.get("error").is_none(), "{id}: {r}");
        b.c.ok("mimo_sine.start", json!({})).await;
        b.c.wait_phase("engaged", Duration::from_secs(300)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        b.c.ok(verb, json!({})).await;
        let end = b.c.finished(Duration::from_secs(60)).await;
        let ramp = end["end"]["ramp_s"].clone();
        b.judged_row(
            id,
            outcome(&end) == want,
            format!("{}; ramp {ramp}", outcome(&end)),
        )
        .await;
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
            let mut cfg = cfg_of("B10");
            cfg["control"]["law"]["id"] = json!(id);
            let end = b.run(cfg).await;
            b.judged_row(
                "B10 a law file arms and runs",
                outcome(&end) == "completed",
                format!(
                    "{} (on NI it arms only once its check has passed)",
                    outcome(&end)
                ),
            )
            .await;
        }
    }

    // B11: each built-in law on every test type it accepts, within the loopback's tolerance.
    for (law, types) in &laws {
        for t in types {
            let id = format!("B11 {law} {t}");
            let end = b.run(cfg_of(&id)).await;
            let worst = b.worst_db().await;
            b.judged_row(
                &format!("B11 {law} on a {t}"),
                outcome(&end) == "completed" && worst <= LOOPBACK_TOLERANCE_DB,
                format!(
                    "{}{}; worst error {worst:.2} dB (tolerance {LOOPBACK_TOLERANCE_DB} dB)",
                    outcome(&end),
                    end["end"]["trip"]["kind"]
                        .as_str()
                        .map(|k| format!(", trip {k}"))
                        .unwrap_or_default()
                ),
            )
            .await;
        }
    }

    // B12: the level ladder: -12, -6 and 0 dB, each rung held, the last at full level.
    b.c.events.clear();
    let end = b.run(cfg_of("B12")).await;
    let worst = b.worst_db().await;
    let rungs: Vec<f64> =
        b.c.events
            .iter()
            .filter(|e| e["topic"] == "mimo_sine.state")
            .filter_map(|e| e["data"]["level_db"].as_f64())
            .fold(Vec::new(), |mut v, l| {
                if v.last() != Some(&l) {
                    v.push(l);
                }
                v
            });
    let reached = |l: f64| rungs.iter().any(|x| (x - l).abs() < 1e-6);
    b.judged_row(
        "B12 the level ladder",
        outcome(&end) == "completed"
            && end["level_db"].as_f64() == Some(0.0)
            && worst <= LOOPBACK_TOLERANCE_DB,
        format!(
            "{}; ended at {} dB, levels seen {rungs:?} (all three: {}), worst error {worst:.2} dB",
            outcome(&end),
            end["level_db"],
            reached(-12.0) && reached(-6.0) && reached(0.0)
        ),
    )
    .await;

    // B13: hold, resume and a manual frequency on a sweep.
    let row = hold_resume_goto(&mut b, cfg_of("B13")).await;
    b.judged_row("B13 hold, resume and a manual frequency", row.0, row.1)
        .await;

    // B14: the finished run opens in the analysis side.
    match b.last_folder.clone() {
        None => b.row(
            "B14 the finished run opens in the analysis side",
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
                "B14 the finished run opens in the analysis side",
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
