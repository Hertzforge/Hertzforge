//! `srs.target.import` and `srs.table.import` on the wire: each reads a file by full
//! path into what a configuration takes, names a bad row, refuses a relative path,
//! needs no control, and what it returns configures.

mod support;

use hertzforge_station::StationBuildError;
use sds_control::server::Service;
use sds_control::station::FixedScanner;
use serde_json::{json, Value};
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

fn service() -> std::sync::Arc<Service> {
    Service::with_options(options(
        FixedScanner(Err(StationBuildError::NiDriverUnavailable(
            "not installed on this test host".into(),
        ))),
        0.0,
        Duration::from_secs(30),
        Duration::from_secs(5),
    ))
}

fn scratch(name: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("sds-srs-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p.display().to_string()
}

fn row(index: u32, role: &str) -> Value {
    let units = if role == "drive" { "V" } else { "g" };
    json!({
        "index": index, "enabled": true, "description": role,
        "role": role, "physical": null, "interface": "single_ended", "iepe_bias_v": null,
        "units": units, "sensitivity_mv_per_eu": 1000,
        "range_max": 10, "polarity": "positive", "control_group": null,
        "abort": {"accel_g": null, "velocity_ips": null, "displacement_mil": null}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_imports_read_name_a_bad_row_refuse_a_relative_path_and_configure() {
    let schema = Schema::load();
    let addr = serve(service()).await;
    // An observer: the imports need no control.
    let mut obs = Client::connect(addr).await;

    let target = obs
        .ok(
            "srs.target.import",
            json!({"path": scratch("target.csv",
                "# from the test specification\nfrequency_hz,srs_g\n100,40\n1000,100\n2500,100\n")}),
        )
        .await;
    schema.assert_valid(&target, Some("srsTargetImportResult"));
    assert_eq!(
        target["target"],
        json!({"kind": "breakpoints", "points": [[100, 40], [1000, 100], [2500, 100]]})
    );
    let bad = obs
        .ok(
            "srs.target.import",
            json!({"path": scratch("bad_target.tsv", "100\t40\n1000\t100\n900\t100\n")}),
        )
        .await;
    schema.assert_valid(&bad, Some("srsTargetImportResult"));
    assert_eq!(bad["target"], Value::Null);
    assert_eq!(bad["errors"][0]["reason"], "target_file");
    assert_eq!(bad["errors"][0]["row"], 2);
    let missing = obs
        .ok(
            "srs.target.import",
            json!({"path": scratch("x", "") + ".none"}),
        )
        .await;
    assert_eq!(missing["errors"][0]["field"], "path");

    let table = obs
        .ok(
            "srs.table.import",
            json!({"path": scratch("table.tsv",
                "freq\tamp\thalfcycles\tdelay\n100\t1.5\t5\t0.04\n200\t1.25\t7\t0.02\n400\t1\t9\t0\n"),
                   "engine": "wavsyn"}),
        )
        .await;
    schema.assert_valid(&table, Some("srsTableImportResult"));
    assert_eq!(table["components"]["engine"], "wavsyn");
    assert_eq!(table["components"]["rows"].as_array().unwrap().len(), 3);
    let bad = obs
        .ok(
            "srs.table.import",
            json!({"path": scratch("bad_table.csv",
                "freq,amp,halfcycles,delay\n100,1,5,0\n200,1,4,0\n"), "engine": "wavsyn"}),
        )
        .await;
    assert_eq!(bad["components"], Value::Null);
    assert_eq!(bad["errors"][0]["reason"], "table_file");
    assert_eq!(bad["errors"][0]["row"], 1);
    let seds = obs
        .ok(
            "srs.table.import",
            json!({"path": scratch("seds.csv",
                "freq,amp,zeta,delay,polarity\n150,1,0.05,0,negative\n"), "engine": "seds"}),
        )
        .await;
    assert_eq!(seds["components"]["rows"][0]["polarity"], "negative");

    for (method, params) in [
        ("srs.target.import", json!({"path": "target.csv"})),
        ("srs.target.import", json!({})),
        (
            "srs.table.import",
            json!({"path": "table.tsv", "engine": "wavsyn"}),
        ),
        ("srs.table.import", json!({"path": "/data/table.tsv"})),
        (
            "srs.table.import",
            json!({"path": "/data/table.tsv", "engine": "fourier"}),
        ),
    ] {
        let e = obs.refused(method, params.clone()).await;
        assert_eq!(e["reason"], "bad_params", "{method} {params}: {e}");
    }

    // What the imports return is what a configuration takes.
    let mut c = Client::connect(addr).await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    c.ok("station.select", json!({"kind": "sim"})).await;
    let rev = c.ok("channels.get", json!({})).await["revision"].clone();
    c.ok(
        "channels.set",
        json!({"revision": rev, "channels": [row(1, "control"), row(2, "drive")]}),
    )
    .await;
    let cfg = json!({
        "target": target["target"],
        "bands": {"f_lo_hz": 100, "f_hi_hz": 2000},
        "synthesis": {"engine": "wavsyn", "components": table["components"]},
        "acquisition": {"fs_hz": 20480},
        "schedule": {"mode": "manual", "levels_db": [0], "pulses": [1]},
        "limits": {"drive_vpk_ceiling": 0.5, "drive_vpk_abort": 0.8}
    });
    let set = c.ok("srs.configure", json!({"config": cfg})).await;
    schema.assert_valid(&set, Some("srsConfigureResult"));
    assert_eq!(set["derived"]["synthesis"]["components"], 3);
}
