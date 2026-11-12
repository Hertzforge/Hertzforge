//! MIMO Shock end to end on the simulator's shock plants, through the wire, on the
//! MIMO Shock engine: identification, the review and the unreachable hold, the
//! law and its guards, the ladder, every way a run ends, and the station released
//! before the end is published.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::{Service, ServiceOptions};
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(300);

async fn ms_state(c: &mut Client, state: &str) -> Value {
    let i = c
        .wait_for(&format!("mimo_shock.state {state}"), LONG, |e| {
            e.topic() == "mimo_shock.state" && e.data()["state"] == state
        })
        .await;
    c.events[i].data().clone()
}

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn opts(timeout: Duration) -> ServiceOptions {
    options(no_driver(), 0.0, timeout, Duration::from_secs(5))
}

fn row(index: u32, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": role,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 1000,
        "range_max": if role == "drive" { 10 } else { 1000 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// Claim control, select the simulator and set `nc` control rows then `nd` drive
/// rows.
async fn controller(c: &mut Client, nc: u32, nd: u32) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let rows: Vec<Value> = (1..=nc)
        .map(|i| row(i, "control"))
        .chain((nc + 1..=nc + nd).map(|i| row(i, "drive")))
        .collect();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
}

fn channels(nc: u32, nd: u32) -> Value {
    json!({
        "controls": (1..=nc).map(|i| json!({"channel": i})).collect::<Vec<_>>(),
        "drives": (nc + 1..=nc + nd)
            .map(|i| json!({"channel": i, "ceiling_vpk": 6, "abort_vpk": 9}))
            .collect::<Vec<_>>(),
        "limits": [],
    })
}

fn half_sine(g: f64, polarity: &str) -> Value {
    json!({"shape": "half_sine", "amplitude_g": g, "duration_ms": 11, "polarity": polarity})
}

/// Four classical pulses on the linear 4 x 4, `inverse`, unpaced.
fn classical(plant: &str, mode: &str) -> Value {
    json!({
        "title": "Four-Drive Half-Sine",
        "channels": channels(4, 4),
        "target": {"kind": "waveform", "pulses": [
            half_sine(20.0, "positive"), half_sine(18.0, "positive"),
            half_sine(20.0, "positive"), half_sine(18.0, "positive")]},
        "acquisition": {"fs_hz": 12800, "record_len": 8192, "band_hz": [5, 2500]},
        "schedule": {"mode": mode, "levels_db": [-12, -9, -6, -3, 0]},
        "spacing": {"cooling_s": 0.2, "settling_s": 0, "compute_s": 0},
        "identification": {"drive_vrms": 1, "frames": 40, "review": false},
        "control": {"law": {"id": "inverse"}, "law_budget": 1},
        "simulator": {"plant": plant},
    })
}

/// The SRS of each control: +6 dB an octave from 50 to 300 Hz, then flat.
fn spectrum(flat: &[f64], polarity: &[&str], plant: &str) -> Value {
    let nc = flat.len() as u32;
    let nd = if nc == 6 { 3 } else { 4 };
    json!({
        "title": "Spectrum",
        "channels": channels(nc, nd),
        "target": {"kind": "srs",
                   "bands": {"f_lo_hz": 50, "f_hi_hz": 2000, "bands_per_octave": 3, "q": 10},
                   "tolerance_db": [-3, 3],
                   "controls": flat.iter().zip(polarity).map(|(a, p)| json!({
                       "target": {"kind": "breakpoints",
                                  "points": [[50, a / 6.0], [300, a], [2500, a]]},
                       "polarity": p,
                   })).collect::<Vec<_>>()},
        "acquisition": {"fs_hz": 12800, "record_len": 16384, "band_hz": [5, 2500]},
        "schedule": {"mode": "auto", "levels_db": [-12, -9, -6, -3, 0]},
        "spacing": {"cooling_s": 0.2, "settling_s": 0, "compute_s": 0},
        "identification": {"drive_vrms": 1, "frames": 40, "review": false},
        "control": {"law": {"id": "shape"}, "law_budget": 1},
        "simulator": {"plant": plant},
    })
}

async fn configure(c: &mut Client, cfg: Value) -> Value {
    c.ok("mimo_shock.configure", json!({"config": cfg})).await["state"]["run_id"].clone()
}

async fn wait_end(c: &mut Client, id: &Value) -> Value {
    let i = c
        .wait_for("the run's end", LONG, |e| {
            e.topic() == "mimo_shock.state"
                && &e.data()["run_id"] == id
                && e.data()["state"] == "finished"
        })
        .await;
    c.events[i].data().clone()
}

async fn rows(c: &mut Client) -> Vec<Value> {
    c.ok("mimo_shock.pulses", json!({})).await["pulses"]
        .as_array()
        .unwrap()
        .clone()
}

fn golden_row() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden/event_mimo_shock_state.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    v["data"]["last_pulse"].clone()
}

/// The same keys, in the same order, at every level of objects (lists by their
/// first element).
fn same_keys(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            assert_eq!(kx, ky, "keys differ at {path}");
            for (k, v) in x {
                if !v.is_null() && !y[k].is_null() {
                    same_keys(v, &y[k], &format!("{path}.{k}"));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if let (Some(u), Some(v)) = (x.first(), y.first()) {
                same_keys(u, v, &format!("{path}[0]"));
            }
        }
        _ => {}
    }
}

fn print_rows(rows: &[Value]) {
    for r in rows {
        println!(
            "pulse {} at {} dB: passed {}, over-test {} dB, global {}",
            r["index"], r["level_db"], r["passed"], r["over_test_db"], r["global_srs_error_db"]
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_classical_on_the_linear_plant_completes_with_three_full_level_passes() {
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    let id = configure(&mut c, classical("shock-linear-4x4", "auto")).await;
    c.ok("mimo_shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    let rows = rows(&mut c).await;
    print_rows(&rows);
    assert_eq!(end["end"]["outcome"], "completed", "{end}");
    let last3 = &rows[rows.len() - 3..];
    assert!(last3
        .iter()
        .all(|r| r["level_db"] == 0 && r["passed"] == true));
    // Every row has the golden row's shape.
    for r in &rows {
        same_keys(r, &golden_row(), "row");
    }
    // The run at the rate it asked of a station stated at 51 200 Hz.
    assert_eq!(end["grid"]["fs_hz"], 12800, "{end}");
    assert_eq!(end["grid"]["actual"], true);
    assert_eq!(end["station"]["plant"], "shock-linear-4x4");
    // Each control's reachable floor was there before the first pulse.
    assert!(end["review"]["controls"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["reachable_ok"] == true && c["floor_db"].is_number()));
    // One pulse event per row, carrying the row.
    c.pump(Duration::from_millis(300)).await;
    let events = c.of_topic("mimo_shock.pulse");
    assert_eq!(events.len(), rows.len());
    for (e, r) in events.iter().zip(&rows) {
        assert_eq!(&e.data()["pulse"], r);
        let titles: Vec<&str> = e.data()["views"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Pulse 1", "Pulse 2", "Pulse 3", "Pulse 4", "Drives"]
        );
    }
    let passed = rows.iter().filter(|r| r["passed"] == true).count();
    assert_eq!(end["pulses"]["passed"], passed);
}

/// A control the drives reach in full reports its floor at the precision the
/// floor is computed to, not at the guard that keeps the logarithm finite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_a_fully_reachable_control_reports_a_floor_of_minus_120_db() {
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    let mut cfg = classical("shock-linear-4x4", "auto");
    cfg["identification"]["review"] = json!(true);
    configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    let s = ms_state(&mut c, "review").await;
    let controls = s["review"]["controls"].as_array().unwrap().clone();
    assert_eq!(controls.len(), 4);
    for k in &controls {
        assert_eq!(k["reachable"], 1.0, "{k}");
        assert_eq!(k["floor_db"], -120.0, "{k}");
    }
    c.ok("mimo_shock.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_srs_on_the_linear_plant_reaches_full_level_passes() {
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    let cfg = spectrum(
        &[20.0, 16.0, 20.0, 16.0],
        &["positive"; 4],
        "shock-linear-4x4",
    );
    let id = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    let rows = rows(&mut c).await;
    print_rows(&rows);
    println!("{}", end["end"]);
    let full: Vec<&Value> = rows.iter().filter(|r| r["level_db"] == 0).collect();
    let full_passes = full.iter().filter(|r| r["passed"] == true).count();
    println!("full-level pulses {}, passed {full_passes}", full.len());
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    assert!(full_passes >= 3);
    let srs = &rows[0]["controls"][0]["srs"];
    assert!(srs["worst_err_db"].is_number() && srs["bands"].as_u64().unwrap() > 10);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_the_severe_jointed_plant_ends_in_a_judged_or_controlled_stop() {
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    // Alternate polarities drive the plate's twist, the mode the joints soften.
    let cfg = spectrum(
        &[50.0; 4],
        &["positive", "negative", "positive", "negative"],
        "shock-jointed-4x4-severe",
    );
    let id = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    let rows = rows(&mut c).await;
    print_rows(&rows);
    println!("{}", end["end"]);
    let e = &end["end"];
    assert!(
        matches!(
            e["outcome"].as_str().unwrap(),
            "completed" | "not_converged" | "safe_stop"
        ),
        "{e}"
    );
    assert!(
        !e["reason"].as_str().unwrap().is_empty() && !e["message"].as_str().unwrap().is_empty()
    );
    assert_eq!(end["phase"], "stopped");
}

/// Stop, then abort, each after two pulses of an auto ladder: nothing fires after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_stop_and_abort_mid_ladder_fire_nothing_after() {
    for (verb, outcome) in [
        ("mimo_shock.stop", "operator_stop"),
        ("mimo_shock.abort", "operator_abort"),
    ] {
        let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
        let mut c = Client::connect(addr).await;
        controller(&mut c, 4, 4).await;
        let mut cfg = classical("shock-linear-4x4", "auto");
        cfg["spacing"]["cooling_s"] = json!(0.6);
        let id = configure(&mut c, cfg).await;
        c.ok("mimo_shock.start", json!({})).await;
        c.wait_for("two pulses", LONG, |e| {
            e.topic() == "mimo_shock.pulse" && e.data()["pulse"]["index"] == 1
        })
        .await;
        c.ok(verb, json!({})).await;
        let end = wait_end(&mut c, &id).await;
        assert_eq!(end["end"]["outcome"], outcome, "{end}");
        let n = rows(&mut c).await.len();
        assert!(n <= 3, "{n} pulses");
        c.pump(Duration::from_millis(1500)).await;
        assert_eq!(rows(&mut c).await.len(), n, "a pulse fired after {verb}");
    }
}

/// The controller drops without a goodbye; nobody claims control within the
/// timeout; the run ends in its controlled stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_a_dropped_controller_ends_in_the_safe_stop() {
    let addr = serve(Service::with_options(opts(Duration::from_secs(2)))).await;
    let mut watcher = Client::connect(addr).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    let mut cfg = classical("shock-linear-4x4", "auto");
    cfg["spacing"]["cooling_s"] = json!(0.6);
    let id = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    c.wait_for("a pulse", LONG, |e| e.topic() == "mimo_shock.pulse")
        .await;
    drop(c);
    let end = wait_end(&mut watcher, &id).await;
    assert_eq!(end["end"]["outcome"], "safe_stop", "{end}");
    assert_eq!(end["end"]["reason"], "orphan", "{end}");
}

/// The law file `<name>.wasm` built by scripts/build_laws.sh.
fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|d| d.join("wasm32-wasip2").join("release").join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{file} is not built; run scripts/build_laws.sh"))
}

async fn misbehaving(mode: &str) -> Value {
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    let id = c
        .ok(
            "laws.import",
            json!({"path": law_file("law_fixture_shock_mimo_misbehave").display().to_string()}),
        )
        .await["law"]["id"]
        .clone();
    let mut cfg = classical("shock-linear-4x4", "auto");
    cfg["control"]["law"] = json!({"id": id, "params": {"mode": mode}});
    // Room for three misses in a row at the first level.
    cfg["schedule"]["max_pulses_per_level"] = json!(10);
    let run = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    let end = wait_end(&mut c, &run).await;
    println!("{mode}: {}", end["end"]);
    end
}

/// A law file that panics ends the run on its first iteration, by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_a_law_that_panics_ends_the_run_by_name() {
    let end = misbehaving("trap").await;
    assert_eq!(end["end"]["outcome"], "law_fault");
    assert_eq!(end["end"]["reason"], "panicked");
    assert_eq!(end["phase"], "aborted");
}

/// A law file's refused output the core folds into `wrong_length` is named by
/// the host's own rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_a_law_files_refusal_is_named_by_the_hosts_rule() {
    let end = misbehaving("wrong_drive_count").await;
    assert_eq!(end["end"]["outcome"], "law_fault");
    assert_eq!(end["end"]["reason"], "wrong_drive_count");
    let end = misbehaving("bad_delays").await;
    assert_eq!(end["end"]["reason"], "delays_shape");
}

/// Six controls on three drives: the run holds before the first pulse, names its
/// floors, refuses to proceed without words and goes on with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_the_unreachable_hold_and_its_acknowledgement() {
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 6, 3).await;
    let cfg = spectrum(
        &[20.0, 16.0, 20.0, 16.0, 18.0, 18.0],
        &["positive"; 6],
        "shock-jointed-6x3",
    );
    let id = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    let s = ms_state(&mut c, "review").await;
    println!("{}", s["review"]);
    assert_eq!(s["review"]["hold"]["reason"], "unreachable_target");
    let e = c.refused("mimo_shock.proceed", json!({})).await;
    assert_eq!(e["reason"], "unreachable_target");
    assert_eq!(e["detail"]["controls"].as_array().unwrap().len(), 6);
    assert!(c.refused("mimo_shock.fire", json!({})).await["reason"] == "wrong_mode");
    c.ok(
        "mimo_shock.proceed",
        json!({"acknowledge": "more controls than drives"}),
    )
    .await;
    let s = c.ok("mimo_shock.state", json!({})).await;
    assert_eq!(s["review"]["acknowledged"], "more controls than drives");
    c.ok("mimo_shock.stop", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    assert_eq!(end["end"]["outcome"], "operator_stop");
    let log = c.ok("mimo_shock.log", json!({})).await;
    assert!(log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "override"));
}

/// The global verdict on the 6 x 3: the floors are shown and nothing holds; a
/// pulse's joint verdict is its global SRS error against the tolerance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_the_global_verdict_does_not_hold_and_judges_the_global_error() {
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 6, 3).await;
    let mut cfg = spectrum(
        &[20.0, 16.0, 20.0, 16.0, 18.0, 18.0],
        &["positive"; 6],
        "shock-jointed-6x3",
    );
    cfg["verdict"] = json!("global");
    let id = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    let end = wait_end(&mut c, &id).await;
    let rows = rows(&mut c).await;
    print_rows(&rows);
    println!("{}", end["end"]);
    assert!(end["review"]["hold"].is_null());
    assert!(end["review"]["controls"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["reachable_ok"] == false));
    assert!(!rows.is_empty());
    for r in &rows {
        let g = r["global_srs_error_db"].as_f64().unwrap();
        let over = r["controls"].as_array().unwrap().iter().any(|c| {
            c["peak_ratio"].as_f64().unwrap() * 10f64.powf(r["level_db"].as_f64().unwrap() / 20.0)
                > 10f64.powf(0.3)
        });
        assert_eq!(r["passed"], g <= 3.0 + 0.005 && !over, "{r}");
    }
}

/// Capture on: the run's records are written to the capture root.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_every_pulse_is_written_to_the_capture_root() {
    let o = opts(Duration::from_secs(30));
    let root = o.capture_root.clone().unwrap();
    let addr = serve(Service::with_options(o)).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    let mut cfg = classical("shock-linear-4x4", "auto");
    cfg["capture"] = json!({"auto_start": true, "run_name": "capture_test"});
    cfg["schedule"]["levels_db"] = json!([-6, 0]);
    let id = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    wait_end(&mut c, &id).await;
    let n = rows(&mut c).await.len();
    let dir = root.join(format!("capture_test_{}", id.as_str().unwrap()));
    let back = hertzforge_mimo_shock::RunRecord::read_dir(&dir).expect("records written");
    assert_eq!(back.pulses.len(), n);
    assert!(back.setup.is_some());
}

/// A contributor's law file, imported under a name of its own, is listed with
/// the target kinds it accepts, chosen by its id and run on the simulator; the
/// built-ins say which target kind each is the default for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mimo_shock_an_imported_law_file_is_listed_chosen_and_run() {
    let dir = std::env::temp_dir().join(format!("mimo_shock_import_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("my_pairing_idea.wasm");
    std::fs::copy(law_file("hertzforge_law_mimo_shock_paired"), &copy).unwrap();
    let addr = serve(Service::with_options(opts(Duration::from_secs(30)))).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c, 4, 4).await;
    let entry = c
        .ok("laws.import", json!({"path": copy.display().to_string()}))
        .await["law"]
        .clone();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(entry["world"], "shock_mimo", "{entry}");
    let id = entry["id"].clone();
    let list = c.ok("laws.list", json!({"world": "shock_mimo"})).await;
    let laws = list["laws"].as_array().unwrap();
    let mine = laws.iter().find(|l| l["id"] == id).expect("listed");
    assert_eq!(mine["builtin"], false);
    assert_eq!(mine["targets"], json!(["waveform", "srs"]));
    assert_eq!(mine["default_for"], json!([]));
    let default_of = |kind: &str| {
        laws.iter()
            .filter(|l| {
                l["default_for"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|k| k == kind)
            })
            .map(|l| l["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(default_of("waveform"), vec!["inverse".to_owned()]);
    assert_eq!(default_of("srs"), vec!["shape".to_owned()]);
    let mut cfg = spectrum(
        &[20.0, 16.0, 20.0, 16.0],
        &["positive"; 4],
        "shock-linear-4x4",
    );
    cfg["control"]["law"] = json!({"id": id});
    let run = configure(&mut c, cfg).await;
    c.ok("mimo_shock.start", json!({})).await;
    let end = wait_end(&mut c, &run).await;
    println!("{}", end["end"]);
    assert_eq!(end["law"]["id"], id, "{end}");
    assert!(
        !rows(&mut c).await.is_empty(),
        "the imported law fired no pulse: {end}"
    );
}
