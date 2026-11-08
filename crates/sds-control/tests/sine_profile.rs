//! `sine.profile.import` and `sine.profile.validate` through the protocol: rows
//! from pasted text and from a file at a path, a bad row named, a relative path
//! refused, and a profile's checks, crossovers, kinematics and views.

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

const ROWS: &str = "# a displacement leg into a 1 g plateau\n\
                    freq_hz\tdisplacement\taccel\talarm\n\
                    20\t0.0097\t\t\n\
                    45\t\t1\t1\n\
                    2000\t\t1\t\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_import_reads_text_and_a_file_and_names_what_it_cannot_read() {
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let r = c.ok("sine.profile.import", json!({"text": ROWS})).await;
    schema.assert_valid(&r, Some("sineProfileImportResult"));
    assert_eq!(r["profile"]["rows"].as_array().unwrap().len(), 3);
    assert_eq!(r["profile"]["rows"][0]["quantity"], "disp_pp_in");
    assert_eq!(r["profile"]["rows"][1]["alarm_db"], json!([1, 1]));
    // The same rows from a file, by its full path.
    let dir = std::env::temp_dir().join(format!("sds-sine-profile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("profile.tsv");
    std::fs::write(&file, ROWS).unwrap();
    let by_path = c
        .ok(
            "sine.profile.import",
            json!({"path": file.to_str().unwrap()}),
        )
        .await;
    assert_eq!(by_path, r);
    // A row that does not read is named by its data row.
    let bad = c
        .ok(
            "sine.profile.import",
            json!({"text": "freq_hz,accel\n20,1\n40,x\n2000,1\n"}),
        )
        .await;
    assert_eq!(bad["profile"], Value::Null);
    assert_eq!(bad["errors"][0]["reason"], "profile_file");
    assert_eq!(bad["errors"][0]["row"], 1);
    schema.assert_valid(&bad, Some("sineProfileImportResult"));
    // Rows out of order are the profile's own item.
    let order = c
        .ok(
            "sine.profile.import",
            json!({"text": "freq_hz,accel\n20,1\n10,1\n"}),
        )
        .await;
    assert_eq!(order["errors"][0]["reason"], "frequency_not_ascending");
    // A file that is not there is an item at the path; a relative path is refused.
    let missing = c
        .ok(
            "sine.profile.import",
            json!({"path": dir.join("nope.csv").to_str().unwrap()}),
        )
        .await;
    assert_eq!(missing["errors"][0]["field"], "path");
    for p in [
        json!({"path": "profile.csv"}),
        json!({"text": ROWS, "path": file.to_str().unwrap()}),
        json!({}),
    ] {
        assert_eq!(
            c.refused("sine.profile.import", p).await["reason"],
            "bad_params"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_validated_profile_carries_its_crossovers_kinematics_and_three_views() {
    let schema = Schema::load();
    let addr = serve(service()).await;
    let mut c = Client::connect(addr).await;
    let profile = c.ok("sine.profile.import", json!({"text": ROWS})).await["profile"].clone();
    let shaker = json!({
        "name": "Bench shaker", "manufacturer": "", "model": "", "notes": "",
        "force_random": null, "force_sine": {"value": 2000, "basis": "Peak"},
        "force_shock": {"value": 4000, "basis": "Peak"},
        "max_velocity_ips": {"value": 70, "basis": "Peak"},
        "max_displacement_in_pp": {"value": 1, "basis": "Peak"},
        "max_accel_bare_g": {"value": 100, "basis": "Peak"},
        "armature_mass_lb": 45, "overturning_moment_limit_lbf_in": null, "custom": []});
    let r = c
        .call(
            "sine.profile.validate",
            json!({"profile": profile, "band_hz": [20, 2000],
                   "feasibility": {"shaker": shaker, "moving_mass_lb": {
                       "armature": 45, "slip_table": 0, "drive_adapter": 3, "fixturing": 10,
                       "test_article": 12}}}),
        )
        .await;
    schema.assert_valid(&r, None);
    let result = c.decoded_response(&r["id"]).unwrap().expanded()["result"].clone();
    assert_eq!(result["crossovers"][0]["from"], "disp_pp_in");
    assert_eq!(result["kinematics"]["verdict"], "pass");
    assert_eq!(result["kinematics"]["peak_disp_pp_in"], 0.0097);
    let titles: Vec<&str> = result["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Profile", "Velocity", "Displacement"]);
    let x = &result["views"][0]["series"][0]["x"]["values"];
    assert!(
        x.as_array().unwrap().iter().any(|f| f == 45.0),
        "the row frequencies are points"
    );
    // Without a shaker the kinematics are not evaluated; a band the profile does not
    // cover is an item and draws nothing.
    let r = c
        .call("sine.profile.validate", json!({"profile": profile}))
        .await;
    assert_eq!(r["result"]["kinematics"]["verdict"], "not_evaluated");
    let r = c
        .ok(
            "sine.profile.validate",
            json!({"profile": profile, "band_hz": [10, 2000]}),
        )
        .await;
    assert_eq!(r["errors"][0]["reason"], "profile_not_covering");
    assert_eq!(r["errors"][0]["field"], "band_hz");
    assert_eq!(r["views"], json!([]));
    assert_eq!(r["kinematics"], Value::Null);
}
