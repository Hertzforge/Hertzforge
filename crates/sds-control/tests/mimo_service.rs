//! MIMO Random through the protocol on the controller's own run service (the wire
//! adapter, `src/mimo/adapter.rs`), on the simulator's cross-coupled 4 by 4 modal
//! plant, over a real loopback WebSocket: a whole run from setup to full level and a
//! ramped stop, an abort on the ladder, a lost controller ending in the safe stop, a
//! law fault ending in a controlled stop, and setups refused before anything runs.
//! Each test asserts the states and events the shell sees.

mod support;

use hertzforge_hal_sim::plants::StandardPlant;
use hertzforge_law_host::{LawIdentity, Param};
use hertzforge_random_laws::{ChoiceError, MimoLawFactory};
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::{HermitianStack, C64};
use hertzforge_station::StationBuildError;
use sds_control::mimo::adapter::{self, AdapterOptions};
use sds_control::server::{Service, ServiceOptions};
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(600);
const FS: f64 = 4096.0;
const NPTS: usize = 2048;
const BAND: (f64, f64) = (160.0, 1500.0);

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

/// The service with the real MIMO driver, the simulator at `speed` simulated
/// seconds per wall second (0: unpaced).
fn real_options(speed: f64, timeout: Duration, heartbeat: Duration) -> ServiceOptions {
    let mut o = options(no_driver(), speed, timeout, heartbeat);
    o.mimo_driver = None;
    o
}

fn row(index: u32, role: &str, description: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": description,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 5000 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// Controls 1 to 4, drives 5 to 8: the size of the simulator's 4 by 4 plant.
fn four_by_four() -> Value {
    let mut rows: Vec<Value> = (1..=4)
        .map(|i| row(i, "control", &format!("Control {i}")))
        .collect();
    rows.extend((5..=8).map(|k| row(k, "drive", &format!("Shaker {}", k - 4))));
    Value::from(rows)
}

/// A deterministic generator for the drive's mixing matrix.
struct Rng(u64);

impl Rng {
    fn sym(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        2.0 * ((self.0 >> 11) as f64 / (1u64 << 53) as f64) - 1.0
    }
}

/// A reference the plant can reach: `H X H^H` with the plant's exact FRF and a
/// partially coherent drive of `vrms` per drive, packed on the grid's band lines.
fn reachable_reference(vrms: f64) -> Value {
    let plant = StandardPlant::Modal4x4.build();
    let df = FS / NPTS as f64;
    let lines = NPTS / 2 + 1;
    let h = plant.model.analytic_frf(lines, df, FS);
    let (lo, hi) = (
        (BAND.0 / df).round() as usize,
        (BAND.1 / df).round() as usize + 1,
    );
    let bw = (hi - lo) as f64 * df;
    let n = 4;
    let mut rng = Rng(0xD21E ^ 0x9E37_79B9_7F4A_7C15);
    let a: Vec<C64> = (0..n * n).map(|_| C64::new(rng.sym(), rng.sym())).collect();
    let psd = vrms * vrms / bw;
    let x = |i: usize, j: usize| {
        let mut s = C64::new(0.0, 0.0);
        for k in 0..n {
            s += a[i * n + k] * a[j * n + k].conj();
        }
        let mut v = s * (0.4 / n as f64);
        if i == j {
            v += C64::new(0.6, 0.0);
        }
        v * psd
    };
    let (mut freqs, mut re, mut im) = (Vec::new(), Vec::new(), Vec::new());
    for line in lo..hi {
        freqs.push(line as f64 * df);
        for r in 0..n {
            for c in r..n {
                let mut acc = C64::new(0.0, 0.0);
                for p in 0..n {
                    for q in 0..n {
                        acc += h.get(line, r, p) * x(p, q) * h.get(line, c, q).conj();
                    }
                }
                re.push(acc.re);
                im.push(if r == c { 0.0 } else { acc.im });
            }
        }
    }
    assert_eq!(re.len(), freqs.len() * HermitianStack::packed_len(n));
    json!({"kind": "inline", "freqs_hz": freqs, "n": n, "re": re, "im": im})
}

fn setup(levels: Value, full_level_s: f64, review: bool) -> Value {
    let mut s = json!({
        "title": "Four-Axis Random",
        "channels": {
            "controls": (1..=4).map(|c| json!({"channel": c, "alarm_db": [3, 3], "abort_db": [6, 6]})).collect::<Vec<_>>(),
            "drives": (5..=8).map(|d| json!({"channel": d, "ceiling_vrms": 5, "abort_vrms": null})).collect::<Vec<_>>(),
        },
        "space": {"kind": "physical"},
        "reference": reachable_reference(0.5),
        "law": {"id": "pinv_closed", "params": {}},
        "estimation": {"fs_hz": FS, "n_pts": NPTS, "frames_per_loop": 16, "window": "hann",
                        "overlap": 0.5, "memory_loops": 4},
        "identification": {"level_db": -20, "frames": 64, "noise_frames": 16,
                           "coherence_floor": 0.5, "condition_limit": 1000, "review": review},
        "control": {"frf": "hold", "level_ramp_s": 0, "shutdown_ramp_s": 2, "max_loops_out": 5},
        "schedule": {"mode": "auto", "levels": levels, "full_level_s": full_level_s},
        "backend": {"kind": "sim"},
    });
    // A debug build of the linear algebra is many times slower than release; the
    // law's time budget is the design's in release and the whole loop in a debug
    // test run, as the service's own convergence tests have it (a 12 by 6 seed on
    // a loaded CI runner overran a quarter of the loop). A test of the budget sets
    // its own after this.
    if cfg!(debug_assertions) {
        s["control"]["law_budget"] = json!(1.0);
    }
    s
}

async fn controller(c: &mut Client) {
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": four_by_four()}),
    )
    .await;
}

async fn wait_mimo(c: &mut Client, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let i = c
        .wait_for(what, LONG, |e| {
            e.topic() == "mimo_random.status" && pred(e.data())
        })
        .await;
    c.events[i].data().clone()
}

fn phases(c: &Client) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in c.of_topic("mimo_random.status") {
        let p = e.data()["phase"].as_str().unwrap_or("").to_owned();
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

fn assert_every_status_valid(schema: &Schema, c: &Client) {
    for e in c.of_topic("mimo_random.status") {
        schema.assert_valid(&e.msg, None);
        schema.assert_valid(e.data(), Some("mimoStatus"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_run_goes_from_setup_to_full_level_and_a_ramped_stop() {
    let schema = Schema::load();
    // Paced, so the two-second stop ramp is seen on the wire as it happens.
    let svc = Service::with_options(real_options(
        8.0,
        Duration::from_secs(600),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    let mut obs = Client::connect(addr).await;
    controller(&mut c).await;
    let set = c
        .ok(
            "mimo_random.setup",
            json!({"setup": setup(json!([{"db": -6, "dwell_s": 0}, {"db": 0}]), 600.0, true)}),
        )
        .await;
    schema.assert_valid(&set, Some("mimoSetupResult"));
    assert_eq!(set["validation"]["ok"], true);
    let started = c.ok("mimo_random.start", json!({})).await;
    schema.assert_valid(&started, Some("mimoStatus"));
    assert_eq!(started["state"], "running");

    // Identification, the gates and the prediction, then the review hold.
    let review = wait_mimo(&mut obs, "the review hold", |d| {
        d["phase"] == "awaiting_review"
    })
    .await;
    assert_eq!(review["state"], "review");
    assert_eq!(review["gates"]["passed"], true, "{review}");
    assert!(review["gates"]["coherence_min"].as_f64().unwrap() > 0.9);
    let pred = &review["prediction"];
    assert_eq!(pred["iteration"], 1);
    assert_eq!(pred["drives_pass"], true);
    assert_eq!(pred["drive_rms_v"].as_array().unwrap().len(), 4);
    // The prediction iterates on the controller's law, and the parameters can change.
    let p = c.ok("mimo_random.iterate_prediction", json!({})).await;
    assert_eq!(p["prediction"]["iteration"], 2);
    let p = c
        .ok(
            "mimo_random.iterate_prediction",
            json!({"law": {"id": "pinv_closed", "params": {"convergence": 0.5}}}),
        )
        .await;
    assert_eq!(p["prediction"]["iteration"], 1, "a changed law starts over");

    // A display subscription is answered from the run's own measurements.
    let sub = obs
        .ok(
            "mimo_random.subscribe",
            json!({"rate_hz": 5, "plots": [
                {"id": "p0", "quantity": "psd", "index": 0, "points": 256},
                {"id": "ce", "quantity": "coh_error", "rows": "all", "points": 256},
                {"id": "fr", "quantity": "frf", "pair": [1, 2], "points": 256}
            ]}),
        )
        .await;
    assert_eq!(sub["plots"].as_array().unwrap().len(), 3);

    let engaged = c.ok("mimo_random.proceed", json!({})).await;
    assert_eq!(engaged["state"], "running");
    let full = wait_mimo(&mut obs, "full level", |d| d["phase"] == "full_level").await;
    assert_eq!(full["control_error_db"].as_array().unwrap().len(), 4);
    assert_eq!(full["drive_rms_v"].as_array().unwrap().len(), 4);
    assert_eq!(full["caps"].as_array().unwrap().len(), 4);
    // A loop or two at full level, then the observer stops it.
    let at = full["loop"].as_u64().unwrap();
    wait_mimo(&mut obs, "a loop at full level", |d| {
        d["loop"].as_u64().unwrap_or(0) >= at + 2
    })
    .await;
    let i = obs
        .wait_for("a data frame", LONG, |e| {
            e.topic() == "mimo_random.data"
                && e.data()["plots"]
                    .as_array()
                    .is_some_and(|p| p.iter().all(|p| !p["views"].as_array().unwrap().is_empty()))
        })
        .await;
    schema.assert_valid(&obs.events[i].msg, None);
    let stopping = obs.ok("mimo_random.stop", json!({})).await;
    assert!(
        matches!(stopping["state"].as_str(), Some("stopping" | "finished")),
        "{stopping}"
    );
    let done = wait_mimo(&mut obs, "the end", |d| d["state"] == "finished").await;
    assert_eq!(done["end"]["outcome"], "operator_stop", "{done}");
    assert_eq!(done["phase"], "stopped");
    let i = obs
        .wait_for("the report", LONG, |e| e.topic() == "mimo_random.report")
        .await;
    let report = obs.events[i].data()["report"].clone();
    schema.assert_valid(&report, Some("mimoReport"));
    assert_eq!(report["controls"].as_array().unwrap().len(), 4);
    for ctl in report["controls"].as_array().unwrap() {
        let e = ctl["error_db"].as_f64().unwrap();
        assert!(e.abs() < 3.0, "final error {e} dB: {report}");
    }
    let seen = phases(&obs);
    for want in [
        "sysid_noise",
        "awaiting_review",
        "engaged",
        "full_level",
        "ramping_down",
        "stopped",
    ] {
        assert!(seen.iter().any(|p| p == want), "{want} not in {seen:?}");
    }
    assert_every_status_valid(&schema, &obs);
    let e = c.refused("mimo_random.start", json!({})).await;
    assert_eq!(e["reason"], "run_finished");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_abort_on_the_ladder_takes_the_drive_down_at_once() {
    let schema = Schema::load();
    let svc = Service::with_options(real_options(
        0.0,
        Duration::from_secs(600),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let levels = json!([{"db": -12, "dwell_s": 600}, {"db": -6, "dwell_s": 600}, {"db": 0}]);
    c.ok(
        "mimo_random.setup",
        json!({"setup": setup(levels, 600.0, false)}),
    )
    .await;
    c.ok("mimo_random.start", json!({})).await;
    let on = wait_mimo(&mut c, "the first rung", |d| {
        d["phase"] == "engaged" && d["loop"].as_u64().unwrap_or(0) >= 1
    })
    .await;
    assert!(on["level_db"].as_f64().unwrap() < -11.0, "{on}");
    let a = c.ok("mimo_random.abort", json!({})).await;
    schema.assert_valid(&a, Some("mimoStatus"));
    let done = wait_mimo(&mut c, "the abort", |d| d["state"] == "finished").await;
    assert_eq!(done["end"]["outcome"], "operator_abort", "{done}");
    assert_eq!(done["phase"], "aborted");
    assert!(done["end"]["level_db"].as_f64().unwrap() < -11.0, "{done}");
    let seen = phases(&c);
    assert!(!seen.iter().any(|p| p == "full_level"), "{seen:?}");
    assert!(
        !seen.iter().any(|p| p == "awaiting_review"),
        "no hold: {seen:?}"
    );
    assert_every_status_valid(&schema, &c);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_controller_during_a_run_ends_in_the_safe_stop() {
    let svc = Service::with_options(real_options(
        8.0,
        Duration::from_millis(1500),
        Duration::from_millis(300),
    ));
    let addr = serve(svc).await;
    {
        let mut c = Client::connect(addr).await;
        controller(&mut c).await;
        c.ok(
            "mimo_random.setup",
            json!({"setup": setup(json!([{"db": -6, "dwell_s": 600}, {"db": 0}]), 600.0, false)}),
        )
        .await;
        c.ok("mimo_random.start", json!({})).await;
        wait_mimo(&mut c, "the loop", |d| d["phase"] == "engaged").await;
        // Gone without a goodbye while the loop drives.
        drop(c);
    }
    // An observer watches (connected now: an idle connection would itself time out).
    let mut obs = Client::connect(addr).await;
    let done = wait_mimo(&mut obs, "the safe stop", |d| d["state"] == "finished").await;
    assert_eq!(done["end"]["outcome"], "safe_stop", "{done}");
    assert_eq!(done["phase"], "stopped");
    let seen = phases(&obs);
    assert!(seen.iter().any(|p| p == "ramping_down"), "{seen:?}");
}

/// The default law for the seed, then an error on every iteration.
struct Failing(Box<dyn MimoControlLaw + Send>);

impl MimoControlLaw for Failing {
    fn needs(&self) -> &[MimoInput] {
        self.0.needs()
    }
    fn shapes(&self) -> &[MimoShape] {
        self.0.shapes()
    }
    fn seed(&mut self, i: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.0.seed(i)
    }
    fn iterate(&mut self, _: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        Err(LawError("no drive this loop".into()))
    }
}

struct FailingFactory(Arc<dyn MimoLawFactory>);

impl MimoLawFactory for FailingFactory {
    fn identity(&self) -> LawIdentity {
        self.0.identity()
    }
    fn params(&self) -> &[Param] {
        self.0.params()
    }
    fn build(
        &self,
        s: &MimoSetup,
        b: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        Ok(Box::new(Failing(self.0.build(s, b)?)))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_law_fault_ends_the_run_in_a_controlled_stop() {
    let schema = Schema::load();
    let mut o = options(
        no_driver(),
        0.0,
        Duration::from_secs(600),
        Duration::from_secs(5),
    );
    let inner = hertzforge_mimo_random::law::builtin_factory(
        &hertzforge_random_laws::default_mimo_choice(),
        4,
    )
    .unwrap();
    let mut a = AdapterOptions::local(&o.data_root, o.scanner.clone());
    a.capture_root = o.capture_root.clone();
    a.law_factory = Some(Arc::new(FailingFactory(inner)));
    o.mimo_driver = Some(adapter::factory(a));
    let svc = Service::with_options(o);
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    c.ok(
        "mimo_random.setup",
        json!({"setup": setup(json!([{"db": -6, "dwell_s": 600}, {"db": 0}]), 600.0, false)}),
    )
    .await;
    c.ok("mimo_random.start", json!({})).await;
    let done = wait_mimo(&mut c, "the law fault", |d| d["state"] == "finished").await;
    assert_eq!(done["end"]["outcome"], "law_fault", "{done}");
    assert_eq!(done["phase"], "stopped");
    assert_eq!(done["end"]["trip"]["kind"], "law_fault");
    assert_eq!(done["end"]["trip"]["reason"], "error");
    assert_eq!(done["end"]["trip"]["consecutive"], 3);
    let seen = phases(&c);
    assert!(seen.iter().any(|p| p == "engaged"), "{seen:?}");
    assert_every_status_valid(&schema, &c);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_setup_with_a_changed_reference_file_or_a_rank_deficient_transform_is_refused() {
    let schema = Schema::load();
    let svc = Service::with_options(real_options(
        0.0,
        Duration::from_secs(600),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let reasons = |e: &Value| -> Vec<(String, String)> {
        e["detail"]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["reason"].as_str().unwrap().to_owned(),
                    i["message"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    // A reference file whose bytes are not the ones the setup names.
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/mimo_reference/three_channel");
    let mut s = setup(json!([{"db": 0}]), 10.0, true);
    s["reference"] = json!({"kind": "file", "path": dir.display().to_string(),
                            "sha256": "0".repeat(64)});
    let e = c.refused("mimo_random.setup", json!({"setup": s})).await;
    assert_eq!(e["reason"], "setup_invalid");
    schema.assert_valid(&e["detail"], Some("mimoValidation"));
    let r = reasons(&e);
    assert!(
        r.iter().any(|(k, m)| k == "reference_file_unread"
            && m.contains("SHA-256")
            && m.contains("choose it again")),
        "{r:?}"
    );
    // The right bytes, but a reference for three controls on another grid.
    let bytes = std::fs::read(dir.join("reference.bin")).unwrap();
    let hash = hertzforge_reference::native::content_hash(&bytes);
    s["reference"]["sha256"] = json!(hash.trim_start_matches("sha256:"));
    let e = c.refused("mimo_random.setup", json!({"setup": s})).await;
    let r = reasons(&e);
    assert!(
        r.iter()
            .any(|(k, m)| k == "reference_file_unread" && m.contains("grid")),
        "{r:?}"
    );
    // A virtual control space whose response transformation has rank 1 of 2.
    let mut s = setup(json!([{"db": 0}]), 10.0, true);
    s["space"] = json!({"kind": "virtual", "dofs": ["X", "Y"],
        "response_transform": {"rows": 2, "cols": 4, "values": [1, 1, 0, 0, 2, 2, 0, 0]}});
    let f: Vec<f64> = (80..=88).map(|k| k as f64 * 2.0).collect();
    let per = 3;
    let re: Vec<f64> = [1e-3, 0.0, 1e-3].repeat(f.len());
    s["reference"] = json!({"kind": "inline", "freqs_hz": f, "n": 2,
        "re": re, "im": vec![0.0; f.len() * per]});
    let e = c.refused("mimo_random.setup", json!({"setup": s})).await;
    assert_eq!(e["reason"], "setup_invalid");
    let r = reasons(&e);
    assert!(
        r.iter().any(|(k, _)| k == "transform_rank_deficient"),
        "{r:?}"
    );
    // Nothing was replaced, and nothing runs.
    assert_eq!(
        c.ok("mimo_random.status", json!({})).await["state"],
        "unconfigured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identification_set_is_saved_loaded_into_a_later_run_and_refused_on_another_setup() {
    let schema = Schema::load();
    let o = real_options(0.0, Duration::from_secs(600), Duration::from_secs(5));
    let keep = o.data_root.join("identification").join("four_axis");
    let svc = Service::with_options(o);
    let addr = serve(svc).await;
    let mut c = Client::connect(addr).await;
    controller(&mut c).await;
    let s = setup(json!([{"db": -6, "dwell_s": 0}, {"db": 0}]), 600.0, true);
    c.ok("mimo_random.setup", json!({"setup": s})).await;
    // Nothing measured yet: nothing to save.
    let path = keep.display().to_string();
    let e = c
        .refused("mimo_random.identification.save", json!({"path": path}))
        .await;
    assert_eq!(e["reason"], "wrong_state");
    c.ok("mimo_random.start", json!({})).await;
    wait_mimo(&mut c, "the review hold", |d| {
        d["phase"] == "awaiting_review"
    })
    .await;
    let saved = c
        .ok("mimo_random.identification.save", json!({"path": path}))
        .await;
    schema.assert_valid(&saved, Some("mimoIdentificationResult"));
    assert!(saved["identification"]["frames"].as_u64().unwrap() >= 64);
    c.ok("mimo_random.stop", json!({})).await;
    wait_mimo(&mut c, "the end", |d| d["state"] == "finished").await;

    // The same setup again: the set loads, and the start goes straight to the gates.
    c.ok("mimo_random.setup", json!({"setup": s})).await;
    let loaded = c
        .ok("mimo_random.identification.load", json!({"path": path}))
        .await;
    schema.assert_valid(&loaded, Some("mimoIdentificationResult"));
    assert_eq!(
        loaded["identification"]["setup_hash"],
        saved["identification"]["setup_hash"]
    );
    let n_before = c.of_topic("mimo_random.status").len();
    let started = c.ok("mimo_random.start", json!({})).await;
    assert_eq!(started["phase"], "awaiting_review", "{started}");
    assert_eq!(started["gates"]["passed"], true);
    let later: Vec<String> = c.of_topic("mimo_random.status")[n_before..]
        .iter()
        .map(|e| e.data()["phase"].as_str().unwrap_or("").to_owned())
        .collect();
    assert!(
        !later.iter().any(|p| p == "sysid" || p == "sysid_noise"),
        "identification ran again: {later:?}"
    );
    c.ok("mimo_random.abort", json!({})).await;
    wait_mimo(&mut c, "the abort", |d| {
        d["state"] == "finished" && d["end"]["outcome"] == "operator_abort"
    })
    .await;

    // Another setup: control 1's sensitivity changed in the table.
    let mut rows = four_by_four();
    rows[0]["sensitivity_mv_per_eu"] = json!(98.5);
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
    c.ok("mimo_random.setup", json!({"setup": s})).await;
    let e = c
        .refused("mimo_random.identification.load", json!({"path": path}))
        .await;
    assert_eq!(e["reason"], "identification_mismatch", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("Run the identification again"),
        "{e}"
    );
    schema.assert_valid(&json!({"type": "response", "id": 1, "error": e}), None);
    // A folder with no set is a bad path, said so.
    let e = c
        .refused(
            "mimo_random.identification.load",
            json!({"path": keep.join("missing").display().to_string()}),
        )
        .await;
    assert_eq!(e["reason"], "bad_params");
}
