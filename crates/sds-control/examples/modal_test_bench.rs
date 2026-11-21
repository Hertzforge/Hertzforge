//! The Modal Test bench driver: the rows of the NI loopback bench (four outputs jumpered to
//! four inputs), each a run through the control service's own protocol, each judged from its
//! run folder and printed as a row with its numbers and its result.
//!
//! ```text
//! # rehearsal: an in-process control service on the Modal Test's simulated structure
//! cargo run -p sds-control --example modal_test_bench -- --rehearse [--out <dir>] [--row M1,M4]
//! # the bench: a control service already running on the NI station, its modules named
//! cargo run -p sds-control --example modal_test_bench -- --url ws://127.0.0.1:8765/ --ni <input module>,<output module> [--out <dir>] [--row M1]
//! ```
//!
//! The table is the loopback's: inputs 1 to 4 read volts (1 unit per volt, IEPE off), drives
//! 5 to 8 are outputs 0 to 3, output k wired to input k. In a shaker row the reference rows are
//! the inputs looped from the active drives, and the responses are inputs whose drive is idle.
//! Before anything is configured on NI the driver checks every row: it refuses to start if any
//! ceiling is over 0.8 V, any level over 0.2 V RMS, or any row has IEPE on. On an abnormal end
//! (a run that ends on an abort it did not ask for, or a call that fails) it aborts, confirms the
//! drive is zero by reading the looped inputs in impact mode, and stops at that row.
//!
//! The rehearsal runs on the Modal Test's own simulated structure: each shaker's reference row
//! reads its own drive times `SHAKER_LBF_PER_V` (100 per volt), so the reference criteria are
//! meaningful there (scaled by that gain); the response rows read the structure, not an idle
//! input, so the criteria that only a loopback can show are computed and printed but marked
//! "not meaningful on the simulated structure". Nothing here is evidence about hardware until
//! it has run on the bench.
//!
//! `--out <dir>` keeps `modal_test_bench_summary.json`, every row's numbers and verdicts (and,
//! in rehearsal, is the data root the run folders are written under).

use futures_util::{SinkExt, StreamExt};
use sds_control::server::{self, Service, ServiceOptions};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

/// The bench contract's limits the driver enforces before anything is configured on NI.
const CEILING_MAX_V: f64 = 0.8;
const LEVEL_MAX_VRMS: f64 = 0.2;
/// The ceiling every shaker row runs at, and the levels (a random's bounded crest is 4, so
/// 0.15 V RMS is 0.6 V peak; a chirp's is the square root of 2).
const CEILING_V: f64 = 0.8;
const RANDOM_LEVEL: f64 = 0.15;
const CHIRP_LEVEL: f64 = 0.2;
const RANDOM_CREST: f64 = 4.0;
/// The bench's rate and block: 2048 Hz (a rate the bench's input module programs exactly),
/// 1 Hz lines, the band 10 to 400 Hz.
const FS: f64 = 2048.0;
const BLOCK: usize = 2048;
const BAND: (f64, f64) = (10.0, 400.0);
/// The shared stop ramp: 40 dB over 3 s, then zero.
const STOP_S: f64 = 3.0;
const STOP_DB: f64 = 40.0;
/// The simulator's force rows read their own drive times this (lbf per volt).
const SIM_GAIN: f64 = 100.0;
/// An input at the floor reads under this RMS, volts (times the gain in rehearsal).
const FLOOR_V: f64 = 2e-3;
const NOT_SIM: &str = "not meaningful on the simulated structure";

struct Client {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next: i64,
}

impl Client {
    async fn connect(url: &str) -> Client {
        let (mut ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("connects");
        let _hello = ws.next().await;
        Client { ws, next: 1 }
    }

    /// A request's whole response; events met on the way are dropped.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let req = json!({"type": "request", "id": id, "method": method, "params": params});
        if self.ws.send(Message::Text(req.to_string())).await.is_err() {
            return json!({"error": {"reason": "socket", "message": "the socket closed"}});
        }
        loop {
            match tokio::time::timeout(Duration::from_secs(600), self.ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => {
                    let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                    if v["type"] == "response" && v["id"] == id {
                        return v;
                    }
                }
                Ok(Some(Ok(Message::Binary(b)))) => {
                    if let Ok(d) = sds_control::codec::decode(&b) {
                        if d.message["type"] == "response" && d.message["id"] == id {
                            return d.message;
                        }
                    }
                }
                Ok(Some(Ok(Message::Ping(p)))) => {
                    let _ = self.ws.send(Message::Pong(p)).await;
                }
                Ok(Some(Ok(_))) => {}
                _ => return json!({"error": {"reason": "socket", "message": "no answer"}}),
            }
        }
    }

    async fn ok(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let r = self.call(method, params).await;
        match r.get("error") {
            Some(e) => Err(format!("{method}: {e}")),
            None => Ok(r["result"].clone()),
        }
    }

    async fn state(&mut self) -> Result<Value, String> {
        self.ok("modal_test.state", json!({})).await
    }

    /// Poll the state until `f` holds, at most `limit`.
    async fn until(
        &mut self,
        limit: Duration,
        what: &str,
        f: impl Fn(&Value) -> bool,
    ) -> Result<Value, String> {
        let t0 = Instant::now();
        loop {
            let s = self.state().await?;
            if f(&s) {
                return Ok(s);
            }
            if s["state"] == "finished" && !f(&s) {
                return Err(format!(
                    "the run finished while waiting for {what}: {}",
                    s["end"]
                ));
            }
            if t0.elapsed() > limit {
                return Err(format!("timed out waiting for {what}"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// One judged criterion: its value, its limit in words, and whether it passed (`None`: not
/// judged, with the reason in `limit`).
struct Crit {
    name: String,
    value: String,
    limit: String,
    ok: Option<bool>,
}

fn crit(name: &str, value: String, limit: &str, ok: Option<bool>) -> Crit {
    Crit {
        name: name.into(),
        value,
        limit: limit.into(),
        ok,
    }
}

/// `ok`, or `None` with the reason when the criterion is a loopback's alone and this is the
/// rehearsal.
fn loopback(rehearse: bool, ok: bool) -> Option<bool> {
    (!rehearse).then_some(ok)
}

struct RowOut {
    id: &'static str,
    what: &'static str,
    crits: Vec<Crit>,
    folder: Option<PathBuf>,
    not_run: Option<String>,
}

impl RowOut {
    fn verdict(&self) -> &'static str {
        if self.not_run.is_some() {
            "NOT RUN"
        } else if self.crits.iter().any(|c| c.ok == Some(false)) {
            "FAIL"
        } else {
            "PASS"
        }
    }

    fn json(&self) -> Value {
        json!({
            "row": self.id, "what": self.what, "verdict": self.verdict(),
            "not_run": self.not_run,
            "folder": self.folder.as_ref().map(|f| f.display().to_string()),
            "criteria": self.crits.iter().map(|c| json!({
                "name": c.name, "value": c.value, "limit": c.limit,
                "judged": c.ok.is_some(), "pass": c.ok,
            })).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------- the table and configs

/// The loopback table. `refs` are the reference inputs (each looped from its drive), every
/// other input a response; `drive_range` the drive rows' `range_max`.
fn table(modules: &Option<(String, String)>, refs: &[u32], drive_range: f64) -> Vec<Value> {
    let physical = |i: u32| match modules {
        None => Value::Null,
        Some((_, ao)) if i > 4 => json!({"device": ao, "channel": format!("ao{}", i - 5)}),
        Some((ai, _)) => json!({"device": ai, "channel": format!("ai{}", i - 1)}),
    };
    let mut rows: Vec<Value> = (1..=4)
        .map(|i| {
            let part = if refs.contains(&i) {
                "reference"
            } else {
                "response"
            };
            // On the simulator a force row reads its drive times 100 and a response the
            // structure's answer to it: a range to hold either.
            let range = if modules.is_none() { 200.0 } else { 5.0 };
            json!({"index": i, "enabled": true, "description": format!("input {i}"),
                   "role": "monitor", "physical": physical(i), "interface": "single_ended",
                   "iepe_bias_v": null, "units": "V", "sensitivity_mv_per_eu": 1000,
                   "range_max": range, "polarity": "positive", "control_group": null,
                   "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null},
                   "node": i, "direction": "Z+", "analysis_role": part})
        })
        .collect();
    rows.extend((5..=8).map(|i| {
        json!({"index": i, "enabled": true, "description": format!("output {}", i - 5),
               "role": "drive", "physical": physical(i), "interface": "single_ended",
               "iepe_bias_v": null, "units": "V", "sensitivity_mv_per_eu": 1000,
               "range_max": drive_range, "polarity": "positive", "control_group": null,
               "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}})
    }));
    rows
}

fn shaker(name: &str, source: &str, drives: &[u32], level: f64, averages: u32) -> Value {
    json!({
        "title": format!("Bench {name}"), "operator": "Bench", "article": "Loopback",
        "mode": "shaker", "fs_hz": FS, "block": BLOCK,
        "band": {"lo_hz": BAND.0, "hi_hz": BAND.1}, "averages": averages,
        "capture": {"run_name": format!("modal_bench_{}", name.to_lowercase())},
        "shaker": {"drives": drives.iter().map(|d| json!({"row": d, "level_vrms": level}))
                       .collect::<Vec<_>>(),
                   "source": source, "burst_pct": 50, "ramp_s": 1.0, "ceiling_v": CEILING_V,
                   "delay_frames": 2, "seed": 11},
    })
}

fn impact(name: &str) -> Value {
    json!({
        "title": format!("Bench {name}"), "operator": "Bench", "article": "Loopback",
        "mode": "impact", "fs_hz": FS, "block": BLOCK,
        "band": {"lo_hz": BAND.0, "hi_hz": BAND.1}, "averages": 1,
        "capture": {"run_name": format!("modal_bench_{}", name.to_lowercase())},
        "impact": {"holdoff_blocks": 0},
    })
}

/// Why `cfg` or `rows` break the bench contract, if they do.
fn contract(cfg: &Value, rows: &[Value]) -> Option<String> {
    if let Some(c) = cfg["shaker"]["ceiling_v"].as_f64() {
        if c > CEILING_MAX_V {
            return Some(format!("ceiling {c} V over {CEILING_MAX_V} V"));
        }
    }
    for d in cfg["shaker"]["drives"].as_array().into_iter().flatten() {
        let l = d["level_vrms"].as_f64().unwrap_or(f64::INFINITY);
        if l > LEVEL_MAX_VRMS {
            return Some(format!(
                "drive {}: level {l} V RMS over {LEVEL_MAX_VRMS}",
                d["row"]
            ));
        }
    }
    for r in rows {
        if !r["iepe_bias_v"].is_null() || r["interface"] == "iepe" {
            return Some(format!("row {}: IEPE on", r["index"]));
        }
    }
    None
}

// ---------------------------------------------------------------- the run folder

struct Stream {
    fs: f64,
    first: u64,
    index: Vec<u32>,
    data: Vec<f32>,
}

impl Stream {
    fn read(folder: &Path, name: &str) -> Option<Stream> {
        let dir = folder.join("streams").join(name);
        let meta: Value =
            serde_json::from_slice(&std::fs::read(dir.join("stream.json")).ok()?).ok()?;
        let bytes = std::fs::read(dir.join("telemetry.f32")).ok()?;
        Some(Stream {
            fs: meta["fs_hz"].as_f64()?,
            first: meta["first_station_frame"].as_u64().unwrap_or(0),
            index: meta["channels"]
                .as_array()?
                .iter()
                .filter_map(|c| c["index"].as_u64().map(|v| v as u32))
                .collect(),
            data: bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
        })
    }

    /// The channel of table row `row`.
    fn ch(&self, row: u32) -> Vec<f64> {
        let w = self.index.len();
        let k = self.index.iter().position(|i| *i == row).unwrap_or(0);
        self.data
            .iter()
            .skip(k)
            .step_by(w.max(1))
            .map(|v| f64::from(*v))
            .collect()
    }
}

fn rms(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
}

fn json_at(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

fn jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn f64s(path: &Path) -> Vec<f64> {
    std::fs::read(path)
        .unwrap_or_default()
        .as_chunks::<8>()
        .0
        .iter()
        .map(|b| f64::from_le_bytes(*b))
        .collect()
}

/// The station frames of the frames taken and accepted.
fn taken(folder: &Path) -> Vec<u64> {
    jsonl(&folder.join("results/modal_frames.jsonl"))
        .iter()
        .filter(|l| l["event"] == "taken" && l["status"] == "accepted")
        .filter_map(|l| l["start_frame"].as_u64())
        .collect()
}

/// The DFT of `x` at whole lines `lines` of `x.len()`, (re, im).
fn dft(x: &[f64], lines: std::ops::RangeInclusive<usize>, hann: bool) -> Vec<(f64, f64)> {
    let n = x.len();
    let w: Vec<f64> = (0..n)
        .map(|i| {
            if hann {
                0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()
            } else {
                1.0
            }
        })
        .collect();
    lines
        .map(|l| {
            let (mut re, mut im) = (0.0, 0.0);
            let step = 2.0 * std::f64::consts::PI * l as f64 / n as f64;
            for (i, v) in x.iter().enumerate() {
                let a = step * i as f64;
                re += v * w[i] * a.cos();
                im -= v * w[i] * a.sin();
            }
            (re, im)
        })
        .collect()
}

/// The first and last nonzero samples of `x`.
fn extent(x: &[f64]) -> Option<(usize, usize)> {
    let a = x.iter().position(|v| *v != 0.0)?;
    let b = x.iter().rposition(|v| *v != 0.0)?;
    Some((a, b))
}

/// The frames of `drive` on the drive's own grid from its ramp's start `n0`.
fn frame(drive: &[f64], n0: usize, f: usize) -> &[f64] {
    let a = (n0 + f * BLOCK).min(drive.len());
    let b = (a + BLOCK).min(drive.len());
    &drive[a..b]
}

fn same(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && !a.is_empty() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-7)
}

// ---------------------------------------------------------------- the bench

struct Bench {
    c: Client,
    rehearse: bool,
    modules: Option<(String, String)>,
    /// The reference rows' units per volt of drive: 1 on the loopback, 100 on the simulator.
    gain: f64,
}

impl Bench {
    async fn set_table(&mut self, rows: &[Value]) -> Result<(), String> {
        let rev = self.c.ok("channels.get", json!({})).await?["revision"].clone();
        self.c
            .ok("channels.set", json!({"revision": rev, "channels": rows}))
            .await
            .map(|_| ())
    }

    /// Configure `cfg` on `rows`, after the bench's contract check.
    async fn configure(&mut self, cfg: &Value, rows: &[Value]) -> Result<Value, String> {
        if let Some(why) = contract(cfg, rows) {
            return Err(format!("the bench contract refuses this row: {why}"));
        }
        self.set_table(rows).await?;
        Ok(self
            .c
            .call("modal_test.configure", json!({"config": cfg}))
            .await)
    }

    /// The finished run's folder once its record is complete.
    async fn finish(&mut self, s: &Value) -> Result<PathBuf, String> {
        let folder = PathBuf::from(s["record"]["folder"].as_str().ok_or("no record folder")?);
        let t0 = Instant::now();
        while !(folder.join("end.json").is_file()
            && json_at(&folder.join("run.json"))["complete"] == true)
        {
            if t0.elapsed() > Duration::from_secs(60) {
                return Err(format!(
                    "the record of {} did not complete",
                    folder.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(folder)
    }

    /// End the run with `modal_test.end` and wait for its record.
    async fn end(&mut self, want: &str) -> Result<(Value, PathBuf), String> {
        self.c.ok("modal_test.end", json!({})).await?;
        let s = self
            .c
            .until(Duration::from_secs(120), "the end", |s| {
                s["state"] == "finished"
            })
            .await?;
        if s["end"]["outcome"] != want {
            return Err(format!(
                "the run ended {} not {want}: {}",
                s["end"]["outcome"], s["end"]["message"]
            ));
        }
        let f = self.finish(&s).await?;
        Ok((s, f))
    }

    /// A shaker row: configure, start, drive until the point is measured, end.
    async fn shaker_run(&mut self, cfg: &Value, refs: &[u32]) -> Result<(Value, PathBuf), String> {
        let rows = table(&self.modules, refs, 1.0);
        let r = self.configure(cfg, &rows).await?;
        if let Some(e) = r.get("error") {
            return Err(format!("configure refused: {e}"));
        }
        self.c.ok("modal_test.start", json!({})).await?;
        self.c.ok("modal_test.drive_start", json!({})).await?;
        self.c
            .until(Duration::from_secs(1800), "the point", |s| {
                s["counts"]["totals"]["points_measured"] == 1
                    || s["counts"]["point"]["rejected"].as_u64().unwrap_or(0) > 10
            })
            .await?;
        let s = self.c.state().await?;
        if s["counts"]["totals"]["points_measured"] != 1 {
            return Err(format!("frames rejected: {}", s["latest"]["reason"]));
        }
        self.c
            .until(Duration::from_secs(60), "the drive idle", |s| {
                s["drive_phase"] == "idle"
            })
            .await?;
        self.end("ended").await
    }

    /// Read the looped inputs in impact mode for `secs` (nothing driven, nothing armed): the
    /// RMS of each, so a drive left on shows on its input.
    async fn confirm_zero(&mut self, secs: f64) -> Result<Vec<f64>, String> {
        let rows = table(&self.modules, &[1], 1.0);
        let r = self.configure(&impact("zero_check"), &rows).await?;
        if let Some(e) = r.get("error") {
            return Err(format!("the zero check's configure was refused: {e}"));
        }
        self.c.ok("modal_test.start", json!({})).await?;
        tokio::time::sleep(Duration::from_secs_f64(secs / self.speed())).await;
        let (_, folder) = self.end("ended").await?;
        let s = Stream::read(&folder, "inputs").ok_or("no inputs stream")?;
        Ok((1..=4).map(|i| rms(&s.ch(i))).collect())
    }

    fn speed(&self) -> f64 {
        if self.rehearse {
            SPEED
        } else {
            1.0
        }
    }
}

const SPEED: f64 = 16.0;

// ---------------------------------------------------------------- the rows

async fn m1(b: &mut Bench) -> Result<RowOut, String> {
    let cfg = shaker("M1", "random", &[5], RANDOM_LEVEL, 30);
    let (_, f) = b.shaker_run(&cfg, &[1]).await?;
    let inp = Stream::read(&f, "inputs").ok_or("no inputs stream")?;
    let drv = Stream::read(&f, "drives").ok_or("no drives stream")?;
    let t = taken(&f);
    let x = inp.ch(1);
    let span = |s: u64| ((s - inp.first) as usize, (s - inp.first) as usize + BLOCK);
    let mut seg = Vec::new();
    for s in &t {
        let (a, z) = span(*s);
        seg.extend_from_slice(&x[a.min(x.len())..z.min(x.len())]);
    }
    let level = rms(&seg) / b.gain;
    let err = (level / RANDOM_LEVEL - 1.0) * 100.0;
    // The band's edges on the reference: the Hann-windowed autospectrum over the frames taken.
    let (lo, hi) = (
        (BAND.0 * BLOCK as f64 / FS).round() as usize,
        (BAND.1 * BLOCK as f64 / FS).round() as usize,
    );
    let mut g = vec![0.0; hi + 6 - (lo - 5) + 1];
    for s in &t {
        let (a, z) = span(*s);
        if z > x.len() {
            continue;
        }
        for (k, (re, im)) in dft(&x[a..z], lo - 5..=hi + 6, true).iter().enumerate() {
            g[k] += re * re + im * im;
        }
    }
    let mut inband: Vec<f64> = g[7..g.len() - 8].to_vec();
    inband.sort_by(|p, q| p.total_cmp(q));
    let med = inband[inband.len() / 2];
    let above: Vec<usize> = (0..g.len())
        .filter(|k| g[*k] > med / 2.0)
        .map(|k| k + lo - 5)
        .collect();
    let (elo, ehi) = (above[0], *above.last().unwrap());
    let edges_ok = elo.abs_diff(lo) <= 1 && ehi.abs_diff(hi) <= 1;
    let peak = drv.ch(5).iter().fold(0.0f64, |m, v| m.max(v.abs()));
    // The FRF to the idle input and its coherence.
    let coh = f64s(&f.join("results/modal_coherence.bin"));
    let mean_coh = coh.iter().sum::<f64>() / coh.len().max(1) as f64;
    let rows = b.rehearse;
    Ok(RowOut {
        id: "M1",
        what: "Random, one drive",
        folder: Some(f),
        not_run: None,
        crits: vec![
            crit(
                "reference RMS against the level",
                format!("{level:.5} V of {RANDOM_LEVEL} ({err:+.2} %)"),
                "within 2 %",
                Some(err.abs() <= 2.0),
            ),
            crit(
                "band edges",
                format!("lines {elo} to {ehi} of {lo} to {hi}"),
                "within one line",
                Some(edges_ok),
            ),
            crit(
                "drive peak",
                format!("{peak:.4} V"),
                "at most the ceiling 0.8 V",
                Some(peak <= CEILING_V),
            ),
            crit(
                "coherence to the idle input",
                format!("{mean_coh:.3} mean"),
                if rows { NOT_SIM } else { "below 0.1" },
                loopback(rows, mean_coh < 0.1),
            ),
        ],
    })
}

async fn m2(b: &mut Bench) -> Result<RowOut, String> {
    let cfg = shaker("M2", "burst_random", &[5], RANDOM_LEVEL, 10);
    let (_, f) = b.shaker_run(&cfg, &[1]).await?;
    let inp = Stream::read(&f, "inputs").ok_or("no inputs stream")?;
    let drv = Stream::read(&f, "drives").ok_or("no drives stream")?.ch(5);
    let n0 = extent(&drv).ok_or("nothing driven")?.0.saturating_sub(1);
    let burst = BLOCK / 2;
    // Every frame at full gain, on the drive's own grid: its burst from the frame's start to
    // the burst's end, then zero.
    // Every frame taken, on the drive's own grid (the input delay is under a frame).
    let (mut worst_start, mut worst_end, mut frames) = (0usize, 0usize, 0);
    for s in taken(&f) {
        let k = ((s - inp.first) as usize).saturating_sub(n0) / BLOCK;
        let fr = frame(&drv, n0, k);
        let Some((a, z)) = extent(fr) else { continue };
        worst_start = worst_start.max(a);
        worst_end = worst_end.max(z.abs_diff(burst - 1));
        frames += 1;
    }
    // The quiet part on the looped reference against the floor before the drive.
    let x = inp.ch(1);
    let d0 = (drv.len() - x.len().min(drv.len())) + n0;
    let floor = rms(&x[..d0.min(x.len()).max(1)]);
    let mut quiet = Vec::new();
    for s in taken(&f) {
        let a = (s - inp.first) as usize;
        if a + BLOCK <= x.len() {
            quiet.extend_from_slice(&x[a + burst + 8..a + BLOCK]);
        }
    }
    let q = rms(&quiet);
    let qlim = 2.0 * floor + FLOOR_V * 0.05 * b.gain;
    Ok(RowOut {
        id: "M2",
        what: "Burst random, one drive",
        folder: Some(f),
        not_run: None,
        crits: vec![
            crit(
                "burst start in the frame",
                format!("worst {worst_start} samples over {frames} frames"),
                "within one sample of the frame's start",
                Some(frames > 0 && worst_start <= 1),
            ),
            crit(
                "burst end in the frame",
                format!("worst {worst_end} samples from sample {}", burst - 1),
                "within one sample",
                Some(frames > 0 && worst_end <= 1),
            ),
            crit(
                "quiet part on the reference",
                format!("{q:.2e} RMS, floor {floor:.2e}"),
                "at most twice the floor (and 0.1 mV)",
                Some(q <= qlim),
            ),
        ],
    })
}

async fn m3(b: &mut Bench) -> Result<RowOut, String> {
    let mut crits = Vec::new();
    let mut last = None;
    for (src, level, repeats) in [
        ("pseudo_random", RANDOM_LEVEL, "repeats"),
        ("periodic_random", RANDOM_LEVEL, "groups"),
        ("periodic_chirp", CHIRP_LEVEL, "repeats"),
        ("burst_chirp", CHIRP_LEVEL, "repeats"),
    ] {
        let averages = 6;
        let mut cfg = shaker("M3", src, &[5], level, averages);
        cfg["capture"]["run_name"] = json!(format!("modal_bench_m3_{src}"));
        let (_, f) = b.shaker_run(&cfg, &[1]).await?;
        let inp = Stream::read(&f, "inputs").ok_or("no inputs stream")?;
        let drv = Stream::read(&f, "drives").ok_or("no drives stream")?.ch(5);
        let n0 = extent(&drv).ok_or("nothing driven")?.0.saturating_sub(1);
        let t = taken(&f);
        // A taken frame's index on the drive's grid (the input delay is under a frame).
        let idx: Vec<usize> = t
            .iter()
            .map(|s| ((s - inp.first) as usize).saturating_sub(n0) / BLOCK)
            .collect();
        let count_ok = t.len() == averages as usize;
        // The ramp is one frame (1 s at 2048 Hz): frame 1 is the first at full level.
        let full = (FS / BLOCK as f64).ceil() as usize;
        let (ok, words) = if repeats == "groups" {
            // Each taken frame is the last of its group of three: the two before it are the
            // same frame played at full level (its delay frames), and the next one is new.
            let last = idx.len().saturating_sub(1);
            let all = idx.windows(2).all(|w| w[1] - w[0] == 3)
                && idx.iter().enumerate().all(|(q, k)| {
                    *k >= full + 2
                        && same(frame(&drv, n0, *k), frame(&drv, n0, k - 1))
                        && same(frame(&drv, n0, *k), frame(&drv, n0, k - 2))
                        && (q == last || !same(frame(&drv, n0, *k), frame(&drv, n0, k + 1)))
                });
            (
                all,
                format!("taken frames {idx:?}, full level from frame {full}"),
            )
        } else {
            // One frame repeated at full level; the first taken comes after the delay frames
            // (burst chirp has none).
            let delay = if src == "burst_chirp" { 0 } else { 2 };
            let rep = idx
                .iter()
                .all(|k| *k >= full + delay && same(frame(&drv, n0, *k), frame(&drv, n0, full)));
            (
                rep,
                format!("full level from frame {full}, {delay} delay frames, taken {idx:?}"),
            )
        };
        crits.push(crit(
            &format!("{src}: frames {repeats} as the source says, delay frames not averaged"),
            words,
            "as the source's definition",
            Some(ok),
        ));
        crits.push(crit(
            &format!("{src}: frames taken"),
            format!("{} of {averages}", t.len()),
            "equal to the averages asked",
            Some(count_ok),
        ));
        last = Some(f);
    }
    Ok(RowOut {
        id: "M3",
        what: "Pseudo random, periodic random and both chirps, one drive",
        folder: last,
        not_run: None,
        crits,
    })
}

/// The coherence between the looped references 1 and 2 over the frames taken, the band's mean.
fn ref_coherence(f: &Path) -> Option<f64> {
    let inp = Stream::read(f, "inputs")?;
    let (x, y) = (inp.ch(1), inp.ch(2));
    let (lo, hi) = (
        (BAND.0 * BLOCK as f64 / FS).round() as usize,
        (BAND.1 * BLOCK as f64 / FS).round() as usize,
    );
    let n = hi - lo + 1;
    let (mut gxx, mut gyy, mut gxy) = (vec![0.0; n], vec![0.0; n], vec![(0.0, 0.0); n]);
    for s in taken(f) {
        let a = (s - inp.first) as usize;
        if a + BLOCK > x.len() {
            continue;
        }
        let fx = dft(&x[a..a + BLOCK], lo..=hi, false);
        let fy = dft(&y[a..a + BLOCK], lo..=hi, false);
        for k in 0..n {
            gxx[k] += fx[k].0 * fx[k].0 + fx[k].1 * fx[k].1;
            gyy[k] += fy[k].0 * fy[k].0 + fy[k].1 * fy[k].1;
            gxy[k].0 += fx[k].0 * fy[k].0 + fx[k].1 * fy[k].1;
            gxy[k].1 += fx[k].1 * fy[k].0 - fx[k].0 * fy[k].1;
        }
    }
    let c: f64 = (0..n)
        .map(|k| (gxy[k].0 * gxy[k].0 + gxy[k].1 * gxy[k].1) / (gxx[k] * gyy[k]).max(1e-300))
        .sum();
    Some(c / n as f64)
}

async fn m4(b: &mut Bench) -> Result<RowOut, String> {
    // Too few averages for two references: refused by name.
    let rows = table(&b.modules, &[1, 2], 1.0);
    let few = shaker("M4", "burst_random", &[5, 6], RANDOM_LEVEL, 3);
    let r = b.configure(&few, &rows).await?;
    let refused = r["error"]["detail"]["errors"]
        .as_array()
        .is_some_and(|e| e.iter().any(|x| x["reason"] == "too_few_averages"));
    let cfg = shaker("M4", "burst_random", &[5, 6], RANDOM_LEVEL, 30);
    let (_, f) = b.shaker_run(&cfg, &[1, 2]).await?;
    let c = ref_coherence(&f).unwrap_or(f64::NAN);
    let side = json_at(&f.join("results/modal_reference_spectra.json"));
    let shape = &side["arrays"][1]["shape"];
    let v = f64s(&f.join("results/modal_reference_spectra.bin"));
    let nl = shape[0].as_u64().unwrap_or(0) as usize;
    let cond: Vec<f64> = v.iter().rev().take(nl).copied().collect();
    let worst = cond.iter().fold(0.0f64, |m, v| m.max(*v));
    let finite = nl > 0 && cond.iter().all(|v| v.is_finite());
    Ok(RowOut {
        id: "M4",
        what: "Two drives, burst random",
        folder: Some(f),
        not_run: None,
        crits: vec![
            crit(
                "coherence between the references",
                format!("{c:.4} over the band"),
                "below 0.1",
                Some(c < 0.1),
            ),
            crit(
                "condition number per line",
                format!("{nl} lines, largest {worst:.3}"),
                "finite and reported",
                Some(finite),
            ),
            crit(
                "3 averages for 2 references",
                format!("refused too_few_averages: {refused}"),
                "refused by name",
                Some(refused),
            ),
        ],
    })
}

async fn m6(b: &mut Bench) -> Result<RowOut, String> {
    let before = b.c.state().await?;
    let mut crits = Vec::new();
    let cases: [(&str, Value, f64, &str); 4] = [
        (
            "a level over the ceiling",
            shaker("M6", "random", &[5], 0.2, 30),
            1.0,
            "level_over_ceiling",
        ),
        (
            "a ceiling over the row's range",
            shaker("M6", "random", &[5], 0.05, 30),
            0.5,
            "ceiling_over_range",
        ),
        (
            "pseudo random on two drives",
            shaker("M6", "pseudo_random", &[5, 6], 0.05, 30),
            1.0,
            "source_refused",
        ),
        (
            "chirp on two drives",
            shaker("M6", "periodic_chirp", &[5, 6], 0.05, 30),
            1.0,
            "source_refused",
        ),
    ];
    for (what, cfg, drive_range, reason) in cases {
        let refs: Vec<u32> = if cfg["shaker"]["drives"].as_array().map_or(0, Vec::len) == 2 {
            vec![1, 2]
        } else {
            vec![1]
        };
        let rows = table(&b.modules, &refs, drive_range);
        let r = b.configure(&cfg, &rows).await?;
        let names: Vec<String> = r["error"]["detail"]["errors"]
            .as_array()
            .map(|e| {
                e.iter()
                    .filter_map(|x| x["reason"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        crits.push(crit(
            what,
            format!("refused {names:?}"),
            reason,
            Some(names.iter().any(|n| n == reason)),
        ));
    }
    // Nothing armed: the state is as before (no run started, so no sample written).
    let after = b.c.state().await?;
    let same_run = after["run_id"] == before["run_id"]
        && after["state"] != "idle"
        && after["state"] != "measuring";
    crits.push(crit(
        "no run started",
        format!("state {} (run {})", after["state"], after["run_id"]),
        "no run armed by a refused configure",
        Some(same_run),
    ));
    crits.push(crit(
        "inputs at the floor",
        "no run folder: nothing was armed".into(),
        "not computable from a run folder (no run starts)",
        None,
    ));
    Ok(RowOut {
        id: "M6",
        what: "Refusals",
        folder: None,
        not_run: None,
        crits,
    })
}

async fn m7(b: &mut Bench) -> Result<RowOut, String> {
    let mut cfg = shaker("M7", "random", &[5], RANDOM_LEVEL, 10000);
    cfg["shaker"]["ramp_s"] = json!(2.0);
    let rows = table(&b.modules, &[1], 1.0);
    let r = b.configure(&cfg, &rows).await?;
    if let Some(e) = r.get("error") {
        return Err(format!("configure refused: {e}"));
    }
    b.c.ok("modal_test.start", json!({})).await?;
    b.c.ok("modal_test.drive_start", json!({})).await?;
    b.c.until(Duration::from_secs(60), "driving", |s| {
        s["drive_phase"] == "driving"
    })
    .await?;
    tokio::time::sleep(Duration::from_secs_f64(3.0 / b.speed())).await;
    b.c.ok("modal_test.drive_stop", json!({})).await?;
    b.c.until(Duration::from_secs(60), "the drive idle", |s| {
        s["drive_phase"] == "idle"
    })
    .await?;
    let (_, f) = b.end("ended").await?;
    let drv = Stream::read(&f, "drives").ok_or("no drives stream")?;
    let x = drv.ch(5);
    let fs = drv.fs;
    let (first, last) = extent(&x).ok_or("nothing driven")?;
    let n0 = first.saturating_sub(1);
    let ramp = (2.0 * fs).round() as usize;
    let stop = (STOP_S * fs).round() as usize;
    let ns = (last + 1).saturating_sub(stop);
    let top = RANDOM_CREST * RANDOM_LEVEL * 1.0001;
    // Inside the ramps' envelopes, sample by sample: no step larger than the ramp allows.
    let mut over = 0;
    for k in 0..ramp.min(x.len() - n0) {
        if x[n0 + k].abs() > top * k as f64 / ramp as f64 + 1e-6 {
            over += 1;
        }
    }
    for k in 0..stop {
        let g = 10f64.powf(-STOP_DB / 20.0 * k as f64 / stop as f64);
        if x[ns + k].abs() > top * g + 1e-6 {
            over += 1;
        }
    }
    let w = (0.25 * fs) as usize;
    let before = rms(&x[ns.saturating_sub(w)..ns]) / RANDOM_LEVEL;
    let mid = 20.0 * (rms(&x[ns + stop / 2 - w / 2..ns + stop / 2 + w / 2]) / RANDOM_LEVEL).log10();
    let after = x[last + 1..].iter().all(|v| *v == 0.0);
    let rise = rms(&x[n0 + ramp - w..n0 + ramp]) / RANDOM_LEVEL;
    Ok(RowOut {
        id: "M7",
        what: "Start ramp and controlled stop",
        folder: Some(f),
        not_run: None,
        crits: vec![
            crit(
                "samples outside the ramps' envelopes",
                format!("{over}"),
                "none",
                Some(over == 0),
            ),
            crit(
                "level at the ramp's end",
                format!("{rise:.3} of the level (last 0.25 s)"),
                "0.8 to 1.05",
                Some((0.8..=1.05).contains(&rise)),
            ),
            crit(
                "level as the stop begins",
                format!("{before:.3} of the level"),
                "within 10 %",
                Some((before - 1.0).abs() <= 0.1),
            ),
            crit(
                "half way down the stop",
                format!("{mid:.2} dB"),
                "-20 dB within 2 dB",
                Some((mid + 20.0).abs() <= 2.0),
            ),
            crit(
                "zero after the stop",
                format!("{after}"),
                "every later sample zero",
                Some(after),
            ),
        ],
    })
}

async fn m8(b: &mut Bench) -> Result<RowOut, String> {
    let cfg = shaker("M8", "random", &[5], RANDOM_LEVEL, 10000);
    let rows = table(&b.modules, &[1], 1.0);
    let r = b.configure(&cfg, &rows).await?;
    if let Some(e) = r.get("error") {
        return Err(format!("configure refused: {e}"));
    }
    b.c.ok("modal_test.start", json!({})).await?;
    b.c.ok("modal_test.drive_start", json!({})).await?;
    b.c.until(Duration::from_secs(60), "driving", |s| {
        s["drive_phase"] == "driving"
    })
    .await?;
    tokio::time::sleep(Duration::from_secs_f64(2.0 / b.speed())).await;
    b.c.ok("modal_test.abort", json!({})).await?;
    let s =
        b.c.until(Duration::from_secs(60), "the end", |s| {
            s["state"] == "finished"
        })
        .await?;
    let f = b.finish(&s).await?;
    let zero = b.confirm_zero(2.0).await?;
    let worst = zero[0] / b.gain;
    Ok(RowOut {
        id: "M8",
        what: "Abort while driving",
        folder: Some(f),
        not_run: None,
        crits: vec![
            crit(
                "outcome",
                format!(
                    "{} with emergency stop {}, phase {}",
                    s["end"]["outcome"], s["end"]["emergency_stop"], s["drive_phase"]
                ),
                "aborted, emergency stop, zeroed",
                Some(
                    s["end"]["outcome"] == "aborted"
                        && s["end"]["emergency_stop"] == true
                        && s["drive_phase"] == "zeroed",
                ),
            ),
            crit(
                "drive at zero within the abort bound",
                "the recorded streams end with the loop at the abort".into(),
                "not computable from the run folder",
                None,
            ),
            crit(
                "looped input after the abort",
                format!("{worst:.2e} V RMS on input 1"),
                "at the floor (under 2 mV)",
                Some(worst < FLOOR_V),
            ),
        ],
    })
}

async fn m9(b: &mut Bench) -> Result<RowOut, String> {
    let rows = table(&b.modules, &[1], 1.0);
    let r = b.configure(&impact("M9"), &rows).await?;
    if let Some(e) = r.get("error") {
        return Err(format!("configure refused: {e}"));
    }
    b.c.ok("modal_test.start", json!({})).await?;
    b.c.ok("modal_test.arm", json!({})).await?;
    tokio::time::sleep(Duration::from_secs_f64(30.0 / b.speed())).await;
    let s = b.c.state().await?;
    let hits = s["counts"]["totals"]["taken"].as_u64().unwrap_or(0);
    b.c.ok("modal_test.disarm", json!({})).await?;
    let (_, f) = b.end("ended").await?;
    let inp = Stream::read(&f, "inputs").ok_or("no inputs stream")?;
    let worst = (1..=4).map(|i| rms(&inp.ch(i))).fold(0.0f64, f64::max);
    let no_drives = !f.join("streams/drives").exists()
        && json_at(&f.join("setup.json"))["resolved"]["drives"] == json!([]);
    let sim = b.rehearse;
    Ok(RowOut {
        id: "M9",
        what: "Impact mode writes no drive",
        folder: Some(f),
        not_run: None,
        crits: vec![
            crit(
                "no drive row opened, no drives stream",
                format!("{no_drives}"),
                "none",
                Some(no_drives),
            ),
            crit(
                "inputs at the floor over 30 s",
                format!("{worst:.2e} V RMS worst"),
                if sim {
                    "not meaningful on the simulated structure (it plays a hit every 1.5 s while armed)"
                } else {
                    "under 2 mV"
                },
                loopback(sim, worst < FLOOR_V),
            ),
            crit(
                "no trigger on noise at the default level",
                format!("{hits} taken"),
                if sim {
                    "not meaningful on the simulated structure (it plays a hit every 1.5 s while armed)"
                } else {
                    "none"
                },
                loopback(sim, hits == 0),
            ),
        ],
    })
}

/// M11's control side: a shaker run's folder holds what the analysis side converts.
fn m11(f: &Path) -> RowOut {
    let run = json_at(&f.join("run.json"));
    let frf = json_at(&f.join("results/modal_frf.json"));
    let recs = jsonl(&f.join("results/modal_frf.jsonl"));
    let table = json_at(&f.join("table.json"));
    let shape: Vec<u64> = frf["arrays"][0]["shape"]
        .as_array()
        .map(|v| v.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    let n = shape.iter().product::<u64>() as usize * 2 * recs.len();
    let bin = f64s(&f.join("results/modal_frf.bin")).len();
    let files = [
        "run.json",
        "table.json",
        "setup.json",
        "end.json",
        "streams/inputs/stream.json",
        "streams/drives/stream.json",
        "results/modal_coherence.json",
        "results/modal_reference_spectra.json",
        "results/modal_outcome.json",
    ]
    .iter()
    .filter(|p| !f.join(p).is_file())
    .map(|p| p.to_string())
    .collect::<Vec<_>>();
    RowOut {
        id: "M11",
        what: "The bench run opens in the analysis side (control side: the folder)",
        folder: Some(f.to_path_buf()),
        not_run: None,
        crits: vec![
            crit(
                "run complete, test modal_test",
                format!("complete {}, test {}", run["complete"], run["test"]),
                "true, modal_test",
                Some(run["complete"] == true && run["test"] == "modal_test"),
            ),
            crit(
                "files present",
                if files.is_empty() {
                    "all".into()
                } else {
                    format!("missing {files:?}")
                },
                "every file the converter reads",
                Some(files.is_empty()),
            ),
            crit(
                "FRF records and data",
                format!(
                    "{} records, shape {shape:?}, {bin} values of {n}",
                    recs.len()
                ),
                "one record per point, the data its shape",
                Some(!recs.is_empty() && bin == n),
            ),
            crit(
                "channel table",
                format!("{} rows", table["rows"].as_array().map_or(0, Vec::len)),
                "the bench's 8 rows",
                Some(table["rows"].as_array().map_or(0, Vec::len) == 8),
            ),
            crit(
                "converts and opens; the fit's refusal check",
                "the analysis side's half".into(),
                "run on the Mac with the converter",
                None,
            ),
        ],
    }
}

fn print_row(r: &RowOut) {
    println!("{:<4} {:<60} {}", r.id, r.what, r.verdict());
    if let Some(w) = &r.not_run {
        println!("       {w}");
    }
    for c in &r.crits {
        let v = match c.ok {
            Some(true) => "pass",
            Some(false) => "FAIL",
            None => "n/j",
        };
        println!("       {v:<4} {}: {} ({})", c.name, c.value, c.limit);
    }
    if let Some(f) = &r.folder {
        println!("       folder {}", f.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_test_bench_refuses_a_row_past_the_contract() {
        let rows = table(&None, &[1], 1.0);
        let ok = shaker("T", "random", &[5], RANDOM_LEVEL, 30);
        assert_eq!(contract(&ok, &rows), None);
        let mut hi = ok.clone();
        hi["shaker"]["ceiling_v"] = json!(0.81);
        assert!(contract(&hi, &rows).is_some());
        let mut lv = ok.clone();
        lv["shaker"]["drives"][0]["level_vrms"] = json!(0.21);
        assert!(contract(&lv, &rows).is_some());
        let mut iepe = rows.clone();
        iepe[2]["iepe_bias_v"] = json!(24);
        assert!(contract(&ok, &iepe).is_some());
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
    let only: Option<Vec<String>> = flag("--row").map(|s| {
        s.split(',')
            .map(|r| r.trim().to_uppercase())
            .filter(|r| !r.is_empty())
            .collect()
    });
    let wanted = |id: &str| only.as_ref().is_none_or(|v| v.iter().any(|r| r == id));
    let out = flag("--out").map(PathBuf::from);
    let modules = if rehearse {
        None
    } else {
        let d: Vec<String> = flag("--ni")
            .expect("--ni <input module>,<output module>")
            .split(',')
            .map(str::to_owned)
            .collect();
        assert_eq!(
            d.len(),
            2,
            "--ni names the input module then the output module"
        );
        Some((d[0].clone(), d[1].clone()))
    };
    // Every row's configuration against the contract before anything is configured.
    let plan: Vec<(Value, Vec<u32>)> = vec![
        (shaker("M1", "random", &[5], RANDOM_LEVEL, 30), vec![1]),
        (
            shaker("M2", "burst_random", &[5], RANDOM_LEVEL, 10),
            vec![1],
        ),
        (
            shaker("M3", "pseudo_random", &[5], RANDOM_LEVEL, 6),
            vec![1],
        ),
        (
            shaker("M3", "periodic_chirp", &[5], CHIRP_LEVEL, 6),
            vec![1],
        ),
        (
            shaker("M4", "burst_random", &[5, 6], RANDOM_LEVEL, 30),
            vec![1, 2],
        ),
        (shaker("M6", "random", &[5], 0.2, 30), vec![1]),
        (shaker("M7", "random", &[5], RANDOM_LEVEL, 10000), vec![1]),
        (impact("M9"), vec![1]),
    ];
    for (cfg, refs) in &plan {
        if let Some(why) = contract(cfg, &table(&modules, refs, 1.0)) {
            eprintln!("refused to start: {}: {why}", cfg["title"]);
            std::process::exit(2);
        }
    }
    let url = if rehearse {
        let root = out.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("modal-bench-{}", std::process::id()))
        });
        let svc = Service::with_options(ServiceOptions {
            data_root: root.clone(),
            capture_root: Some(root),
            sim_speed: SPEED,
            ..ServiceOptions::default()
        });
        svc.start_background();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(server::serve(listener, svc));
        format!("ws://{addr}/")
    } else {
        flag("--url").expect("--url ws://... of the control service on the NI station")
    };
    let mut c = Client::connect(&url).await;
    c.ok("session.claim", json!({"name": "Modal Test bench"}))
        .await
        .expect("claims control");
    let station = match &modules {
        None => c.ok("station.select", json!({"kind": "sim"})).await,
        Some((ai, ao)) => {
            c.ok("station.select", json!({"kind": "ni", "devices": [ai, ao]}))
                .await
        }
    };
    println!("station: {}", station.expect("selects the station"));
    let mut b = Bench {
        c,
        rehearse,
        modules,
        gain: if rehearse { SIM_GAIN } else { 1.0 },
    };
    println!(
        "Modal Test bench rows, {}; every ceiling {CEILING_V} V, levels at most {LEVEL_MAX_VRMS} V RMS, IEPE off",
        if rehearse { "rehearsed on the Modal Test's simulated structure" } else { "on the NI station" }
    );
    let mut done: Vec<RowOut> = Vec::new();
    let mut shaker_folder: Option<PathBuf> = None;
    for id in ["M1", "M2", "M3", "M4", "M5", "M6", "M7", "M8", "M9", "M11"] {
        if !wanted(id) {
            continue;
        }
        let res = match id {
            "M1" => m1(&mut b).await,
            "M2" => m2(&mut b).await,
            "M3" => m3(&mut b).await,
            "M4" => m4(&mut b).await,
            "M5" => Ok(RowOut {
                id: "M5",
                what: "Four drives, periodic random",
                crits: Vec::new(),
                folder: None,
                not_run: Some("four references take all four inputs and leave no response row, which the setup refuses (no_response): the row needs a fifth input".into()),
            }),
            "M6" => m6(&mut b).await,
            "M7" => m7(&mut b).await,
            "M8" => m8(&mut b).await,
            "M9" => m9(&mut b).await,
            _ => match shaker_folder.clone() {
                Some(f) => Ok(m11(&f)),
                None => m1(&mut b).await.map(|r| m11(r.folder.as_ref().unwrap())),
            },
        };
        match res {
            Ok(r) => {
                if matches!(id, "M1" | "M4") {
                    shaker_folder = r.folder.clone();
                }
                print_row(&r);
                let failed = r.verdict() == "FAIL";
                done.push(r);
                if failed && !rehearse {
                    eprintln!("stopping at the first fault ({id})");
                    break;
                }
            }
            Err(why) => {
                // An abnormal end: abort, confirm zero on the looped inputs, stop on NI.
                println!("{id:<4} FAIL {why}");
                let _ = b.c.call("modal_test.abort", json!({})).await;
                let _ =
                    b.c.until(Duration::from_secs(30), "the end", |s| {
                        s["state"] == "finished"
                            || s["state"] == "none"
                            || s["state"] == "configured"
                    })
                    .await;
                match b.confirm_zero(2.0).await {
                    Ok(z) => println!(
                        "       aborted; looped inputs after: {:?} RMS",
                        z.iter()
                            .map(|v| format!("{:.2e}", v / b.gain))
                            .collect::<Vec<_>>()
                    ),
                    Err(e) => println!("       aborted; the zero check failed: {e}"),
                }
                done.push(RowOut {
                    id: "M?",
                    what: "abnormal end",
                    crits: vec![crit(id, why, "the row ran", Some(false))],
                    folder: None,
                    not_run: None,
                });
                if !rehearse {
                    std::process::exit(1);
                }
            }
        }
    }
    let passed = done.iter().filter(|r| r.verdict() == "PASS").count();
    println!("{passed} of {} rows passed", done.len());
    if let Some(dir) = &out {
        let _ = std::fs::create_dir_all(dir);
        let summary = json!({
            "bench": if rehearse { "rehearsal on the Modal Test's simulated structure" } else { "NI loopback" },
            "rows": done.iter().map(RowOut::json).collect::<Vec<_>>(),
        });
        let p = dir.join("modal_test_bench_summary.json");
        std::fs::write(&p, serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
        println!("summary {}", p.display());
    }
}
