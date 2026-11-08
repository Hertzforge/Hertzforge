//! MIMO Random's display on the wire: only what a screen subscribed to is sent,
//! decimated to its plots' widths. The measurement: a 72-control, 24-drive
//! simulated run, one connection subscribed to 8 coherence pairs and the error
//! heatmap, the bytes counted on the socket against what streaming the whole
//! spectral matrix would cost.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, Client};

const LONG: Duration = Duration::from_secs(60);
const CONTROLS: usize = 72;
const DRIVES: usize = 24;
const WIDTH: usize = 512;

fn no_driver() -> FixedScanner {
    FixedScanner(Err(StationBuildError::NiDriverUnavailable(
        "not installed on this test host".into(),
    )))
}

fn row(index: usize, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": format!("{role} {index}"),
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": if units == "V" { 1000 } else { 100 },
        "range_max": if units == "V" { 10 } else { 100 }, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

/// 72 controls (rows 1 to 72) and 24 drives (rows 101 to 124), a reference by
/// file, 1,600 lines.
fn big_setup() -> (Value, Value) {
    let mut rows: Vec<Value> = (1..=CONTROLS).map(|i| row(i, "control")).collect();
    rows.extend((101..101 + DRIVES).map(|i| row(i, "drive")));
    let setup = json!({
        "title": "Full-Scale MIMO",
        "channels": {
            "controls": (1..=CONTROLS).map(|i| json!({"channel": i})).collect::<Vec<_>>(),
            "drives": (101..101 + DRIVES).map(|i| json!({"channel": i, "ceiling_vrms": 5})).collect::<Vec<_>>(),
        },
        "reference": {"kind": "file", "path": "/data/references/full_scale.hforge", "sha256": "ab".repeat(32)},
        "estimation": {"fs_hz": 8192, "n_pts": 4096},
        "identification": {"review": false},
        "schedule": {"mode": "auto", "levels": [{"db": 0}], "full_level_s": 3600},
    });
    (Value::from(rows), setup)
}

async fn running(c: &mut Client) {
    let (rows, setup) = big_setup();
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok("channels.set", json!({"revision": rev, "channels": rows}))
        .await;
    let set = c.ok("mimo_random.setup", json!({"setup": setup})).await;
    assert_eq!(set["state"]["grid"]["lines"], 1600);
    assert_eq!(set["state"]["shape"]["shape"], "more_controls_than_drives");
    c.ok("mimo_random.start", json!({})).await;
    c.wait_for("full level", LONG, |e| {
        e.topic() == "mimo_random.status" && e.data()["phase"] == "full_level"
    })
    .await;
}

fn arrays_of(e: &support::Event) -> Vec<(String, Vec<usize>, usize)> {
    let d = e.decoded.as_ref().expect("a binary frame");
    d.arrays
        .iter()
        .map(|(id, a)| (id.clone(), a.shape.clone(), a.data.len()))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_subscribed_pairs_and_heatmap_cross_the_socket() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        40.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut ctl = Client::connect(addr).await;
    let mut screen = Client::connect(addr).await;
    running(&mut ctl).await;

    let pairs: Vec<(usize, usize)> = (0..8).map(|k| (2 * k, 2 * k + 1)).collect();
    let mut plots: Vec<Value> = pairs
        .iter()
        .map(|(i, j)| json!({"id": format!("coh{i}_{j}"), "quantity": "coh", "pair": [i, j], "points": WIDTH}))
        .collect();
    plots.push(json!({"id": "heat", "quantity": "error_db", "rows": "all", "points": WIDTH}));
    let params = json!({"rate_hz": 10, "plots": plots});
    schema.assert_valid(&params, Some("mimoSubscribeParams"));
    let result = screen.ok("mimo_random.subscribe", params).await;
    schema.assert_valid(&result, Some("mimoSubscribeResult"));
    assert_eq!(result["plots"].as_array().unwrap().len(), 9);

    // Count every frame on the subscribed socket for two seconds.
    let start = Instant::now();
    screen.events.clear();
    while start.elapsed() < Duration::from_secs(2) {
        screen.read(Duration::from_millis(500)).await;
    }
    let data: Vec<&support::Event> = screen
        .events
        .iter()
        .filter(|e| e.topic() == "mimo_random.data")
        .collect();
    assert!(data.len() >= 5, "{} frames in 2 s at 10 Hz", data.len());
    let sent: usize = data.iter().map(|e| e.bytes).sum();
    let mean = sent / data.len();
    // The whole response SDM, packed (2,628 complex values per line), at f64.
    let packed = CONTROLS * (CONTROLS + 1) / 2;
    let whole = packed * 1600 * 16;
    eprintln!(
        "mimo display bytes: {} frames, {} bytes in all, {} bytes per frame on average; \
         the whole packed SDM would be {} bytes per frame ({:.3} % of it sent)",
        data.len(),
        sent,
        mean,
        whole,
        100.0 * mean as f64 / whole as f64
    );
    assert!(
        mean * 100 < whole,
        "a frame is under 1 % of the whole matrix"
    );
    for e in &data {
        schema.assert_valid(e.data(), Some("mimoData"));
        let plots = e.data()["plots"].as_array().unwrap();
        let ids: Vec<&str> = plots.iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 9);
        for (p, (i, j)) in plots.iter().zip(&pairs) {
            assert_eq!(
                p["selector"]["pair"],
                json!([i, j]),
                "only the subscribed pair"
            );
            let series = p["views"][0]["series"].as_array().unwrap();
            assert_eq!(series.len(), 2, "measured and reference");
        }
        assert_eq!(plots[8]["views"][0]["view"], "heatmap");
        // No array is larger than one heatmap row set at the plot's width, and none
        // has the shape of a matrix line.
        for (id, shape, len) in arrays_of(e) {
            assert!(len <= CONTROLS * WIDTH, "{id} holds {len} values");
            assert!(
                !shape.contains(&packed),
                "{id} has a packed-matrix dimension"
            );
            if shape.len() == 2 {
                assert_eq!(shape[0], CONTROLS, "{id}: the heatmap's rows");
                assert!(shape[1] <= WIDTH, "{id}: decimated to the width");
            } else {
                assert!(shape[0] <= WIDTH, "{id}: {} points at most", WIDTH);
            }
        }
    }

    // The controlling connection never subscribed: it received no data at all, only
    // status.
    ctl.pump(Duration::from_millis(300)).await;
    assert!(
        ctl.events.iter().all(|e| e.topic() != "mimo_random.data"),
        "nothing is sent to a connection that did not subscribe"
    );
    // Back to no subscription: the data stops.
    screen
        .ok("mimo_random.subscribe", json!({"default": true}))
        .await;
    screen.events.clear();
    screen.pump(Duration::from_millis(500)).await;
    assert!(screen
        .events
        .iter()
        .all(|e| e.topic() != "mimo_random.data"));
    ctl.ok("mimo_random.abort", json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_quantity_routes_to_its_own_views() {
    let schema = Schema::load();
    let svc = Service::with_options(options(
        no_driver(),
        40.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ));
    let addr = serve(svc).await;
    let mut ctl = Client::connect(addr).await;
    running(&mut ctl).await;
    let plots = json!([
        {"id": "psd", "quantity": "psd", "index": 3, "points": 256},
        {"id": "diag", "quantity": "drive_psd", "diagonal": true, "points": 128},
        {"id": "ph", "quantity": "phase", "pair": [0, 1]},
        {"id": "csd", "quantity": "csd_mag", "pair": [4, 9], "points": 300},
        {"id": "dc", "quantity": "drive_coh", "pair": [0, 1]},
        {"id": "frf", "quantity": "frf", "pair": [5, 2], "points": 200},
        {"id": "sv", "quantity": "sv", "points": 100},
        {"id": "cond", "quantity": "cond"},
        {"id": "mcoh", "quantity": "mcoh", "index": 0},
        {"id": "cap", "quantity": "cap"},
        {"id": "trace", "quantity": "trace"},
        {"id": "pred", "quantity": "pred_drive_rms", "index": 23},
        {"id": "sdm", "quantity": "sdm", "block": [0, 1, 2, 3, 4, 5, 6, 7], "points": 64},
        {"id": "rd", "quantity": "readings"},
        {"id": "gone", "quantity": "psd", "index": 500},
        {"id": "rows", "quantity": "error_db", "rows": [70, 71, 99]},
    ]);
    ctl.ok(
        "mimo_random.subscribe",
        json!({"rate_hz": 2, "plots": plots}),
    )
    .await;
    let i = ctl
        .wait_for("data", LONG, |e| e.topic() == "mimo_random.data")
        .await;
    let e = &ctl.events[i];
    schema.assert_valid(&e.msg, None);
    schema.assert_valid(e.data(), Some("mimoData"));
    let by = |id: &str| {
        e.data()["plots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()
            .clone()
    };
    let d = e.decoded.as_ref().unwrap();
    let len = |r: &Value| d.array(r).unwrap().data.len();
    let psd = by("psd");
    assert_eq!(psd["labels"], json!(["control 4"]));
    let s = &psd["views"][0]["series"];
    assert_eq!(
        s.as_array().unwrap().len(),
        6,
        "control, reference and four bands"
    );
    assert_eq!(s[0]["shape"], "steps", "a density is drawn in steps");
    assert!(s[0].get("y_min").is_some(), "decimated: an envelope");
    assert!(len(&s[0]["y"]) <= 256);
    assert_eq!(len(&s[0]["edges"]), len(&s[0]["y"]) + 1);
    let diag = by("diag");
    assert_eq!(diag["views"][0]["series"].as_array().unwrap().len(), DRIVES);
    assert_eq!(
        by("frf")["views"].as_array().unwrap().len(),
        2,
        "magnitude and phase"
    );
    assert_eq!(
        by("frf")["labels"],
        json!(["control 6", "drive 103"]),
        "control 5 from drive 2"
    );
    assert_eq!(
        by("sv")["views"][0]["series"].as_array().unwrap().len(),
        DRIVES
    );
    assert_eq!(by("cond")["views"][0]["series"][1]["role"], "limit");
    assert_eq!(
        by("cap")["views"][0]["rows"].as_array().unwrap().len(),
        DRIVES
    );
    assert_eq!(by("pred")["views"][0]["rows"][0]["unit"], "V");
    let sdm = &by("sdm")["views"][0];
    assert_eq!(sdm["view"], "sdm");
    let re = d.array(&sdm["measured"]["re"]).unwrap();
    assert_eq!(re.shape[1], 36, "8 by 8, packed: 36 values per line");
    assert!(re.shape[0] <= 64);
    assert_eq!(
        d.array(&sdm["reference"]["im"]).unwrap().shape,
        re.shape,
        "the reference's block on the same lines"
    );
    assert_eq!(by("rd")["views"][0]["view"], "reading");
    assert_eq!(
        by("gone")["views"],
        json!([]),
        "a position the run does not have shows nothing"
    );
    let rows = by("rows");
    assert_eq!(rows["labels"], json!(["control 71", "control 72"]));
    let heat = &rows["views"][0];
    assert_eq!(d.array(&heat["values"]).unwrap().shape[0], 2);
    assert_eq!(heat["row_bands"].as_array().unwrap().len(), 2);
    ctl.ok("mimo_random.abort", json!({})).await;
}
