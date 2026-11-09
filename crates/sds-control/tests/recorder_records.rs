//! The Recorder's run folder (schema/PROTOCOL.md, "Recorder Records"): made in the
//! local data root under the run's name (the next free `<name>-<n>` when it is taken),
//! holding run.json, log.jsonl, marks.jsonl, end.json and recordings/rec_NNNN, each
//! file as the schema states it.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use support::schema::Schema;
use support::{options, serve, Client};

fn read(p: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recorder_run_folder_holds_its_records() {
    let schema = Schema::load();
    let opts = options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable("none".into()))),
        1.0,
        Duration::from_secs(30),
        Duration::from_millis(300),
    );
    let root = opts.capture_root.clone().unwrap();
    let svc = Service::with_options(opts);
    svc.start_background();
    let addr = serve(svc.clone()).await;
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    let row = |i: u32, limit: Option<f64>| {
        json!({
        "index": i, "enabled": true, "description": "", "role": "monitor", "physical": null,
        "interface": "single_ended", "iepe_bias_v": null, "units": "g",
        "sensitivity_mv_per_eu": 1000, "range_max": 0.5, "polarity": "positive",
        "control_group": null,
        "abort": {"accel_g": limit, "velocity_ips": null, "displacement_mil": null}})
    };
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, Some(0.4)), row(2, None)]}),
    )
    .await;
    let cfg =
        json!({"trigger": {"mode": "start", "record_s": 1}, "capture": {"run_name": "folder"}});
    for n in 0..2 {
        c.ok("recorder.configure", json!({"config": cfg})).await;
        c.ok("recorder.start", json!({})).await;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let s = c.ok("recorder.state", json!({})).await;
            if s["recordings"] == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "{s}");
            c.pump(Duration::from_millis(100)).await;
        }
        c.ok("recorder.stop", json!({})).await;
        let deadline = Instant::now() + Duration::from_secs(20);
        while c.ok("recorder.state", json!({})).await["state"] != "finished" {
            assert!(Instant::now() < deadline);
            c.pump(Duration::from_millis(50)).await;
        }
        let folder: PathBuf = root.join(if n == 0 {
            "folder".to_owned()
        } else {
            "folder-2".into()
        });
        assert!(
            folder.is_dir(),
            "{folder:?}: a taken name gets the next free one"
        );
        schema.assert_valid(&read(&folder.join("run.json")), Some("recorderRunRecord"));
        let end = read(&folder.join("end.json"));
        schema.assert_valid(&end, Some("recorderEndRecord"));
        assert_eq!(end["end"]["outcome"], "operator_stop");
        for line in std::fs::read_to_string(folder.join("log.jsonl"))
            .unwrap()
            .lines()
        {
            schema.assert_valid(&serde_json::from_str(line).unwrap(), Some("logEntry"));
        }
        let marks: Vec<Value> = std::fs::read_to_string(folder.join("marks.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(!marks.is_empty(), "the marker passes row 1's 0.4 g limit");
        for m in &marks {
            schema.assert_valid(m, Some("recorderMark"));
        }
        let rec = folder.join("recordings/rec_0001");
        let side = read(&rec.join("recorder.json"));
        schema.assert_valid(&side, Some("recorderSidecar"));
        assert_eq!(side["complete"], true);
        let bytes = std::fs::metadata(rec.join("telemetry.f32")).unwrap().len();
        assert_eq!(bytes, 12_800 * 2 * 4, "one second of two rows");
        assert_eq!(side["frames"], 12_800);
    }
}
