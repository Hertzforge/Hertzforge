//! A MIMO Sine run's record, field by field, on the simulator (run data rules,
//! the whole record): the table as used, the station and the simulator's
//! structure and seed, the setup as accepted and resolved, the limits and
//! aborts, the law, its parameters and the identification, the shaker
//! ratings, the timeline and the outcome, every input and drive and the
//! results, the run's identity and times, and a unit on every array; and the
//! folder valid while the run drives.

mod support;

use sds_control::server::Service;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use support::golden_runs::read_json;
use support::msine::{self, controller};
use support::{serve, Client};

fn shaker(drive: u32) -> Value {
    json!({"drive": drive,
           "shaker": {"name": "Bench shaker", "manufacturer": "", "model": "", "notes": "",
                      "force_random": null, "force_sine": null, "force_shock": null,
                      "max_velocity_ips": {"value": 70, "basis": "Peak"},
                      "max_displacement_in_pp": {"value": 1, "basis": "Peak"},
                      "max_accel_bare_g": null, "armature_mass_lb": null,
                      "overturning_moment_limit_lbf_in": null, "custom": []},
           "moving_mass_lb": {"armature": 10, "slip_table": 0, "drive_adapter": 0,
                              "fixturing": 0, "test_article": 1}})
}

/// The base and article: a sweep through the article's resonance, its two
/// responses as limit channels, shakers on every drive.
fn base_article_config() -> Value {
    let mut cfg = msine::config(
        "sine-base-article-6x4",
        4,
        2,
        4,
        msine::sweep(200.0, 240.0, 4.0),
        "matrix",
    );
    for k in 0..4 {
        cfg["channels"]["controls"][k]["profile"] = msine::flat(1.0, 1.5, 12.0);
    }
    cfg["limits"] = json!({"channels": [
        {"channel": 5, "profile": msine::flat(2.0, 1.5, 3.0)},
        {"channel": 6, "profile": msine::flat(2.0, 1.5, 3.0)},
    ]});
    cfg["feasibility"] = json!({"shakers": [shaker(7), shaker(8), shaker(9), shaker(10)]});
    cfg["specification"] = json!({"document": "Procedure", "clause": "4.2"});
    cfg
}

fn arrays_have_units(folder: &Path, name: &str) {
    let side = read_json(&folder.join(format!("results/{name}.json")));
    for a in side["arrays"].as_array().unwrap() {
        assert!(a.get("unit").is_some(), "{name}.{} has no unit", a["name"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_the_record_holds_every_item_of_the_run() {
    let svc = Service::with_options(msine::opts(Duration::from_secs(600)));
    let mut c = Client::connect(serve(svc).await).await;
    controller(&mut c, 4, 2, 4).await;
    let end = msine::run_to_end(&mut c, base_article_config()).await;
    assert_eq!(end["end"]["outcome"], "completed", "{}", end["end"]);
    let folder = msine::folder(&end);
    let run = msine::complete(&folder).await;
    let name = folder.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.ends_with("_mimo_sine_msine-0001"), "{name}");

    // The table as used, every column.
    let table = read_json(&folder.join("table.json"));
    assert_eq!(table["rows"].as_array().unwrap().len(), 10);
    assert_eq!(table["rows"][4]["role"], "monitor");
    assert!(table["rows"][0].get("sensitivity_mv_per_eu").is_some());
    assert_eq!(run["channel_table"]["revision"], table["revision"]);

    // The station, the simulator's structure and seed, the rates.
    assert_eq!(run["source"], "SIM");
    assert_eq!(run["sim"]["structure"], "sine-base-article-6x4");
    assert!(run["sim"]["seed"].is_u64());
    assert_eq!(run["station"]["kind"], "sim");
    assert_eq!(run["grid"]["fs_hz"], 16384);

    // The setup as accepted and resolved.
    let setup = read_json(&folder.join("setup.json"));
    assert_eq!(setup["config"]["title"], "Four-Shaker Test");
    let r = &setup["resolved"];
    assert_eq!(r["test"]["kind"], "sweep");
    assert_eq!(r["controls"].as_array().unwrap().len(), 4);
    assert_eq!(r["controls"][0]["unit"], "g");
    assert_eq!(r["drives"][0]["ceiling_vpk"], 2);
    assert_eq!(r["schedule"]["levels_db"], json!([0]));
    assert_eq!(r["simulator"]["structure"], "sine-base-article-6x4");
    assert_eq!(setup["derived"]["grid"]["actual"], true);

    // The limits and aborts in force.
    assert_eq!(r["limits"].as_array().unwrap().len(), 2);
    assert_eq!(r["limits"][0]["channel"], 5);
    assert_eq!(r["aborts"]["abort_ticks"], 3);
    assert!(folder.join("results/limit_ch5.json").is_file());
    assert!(folder.join("results/limit_ch6.json").is_file());

    // The law, its parameters and file, and the identification.
    let law = read_json(&folder.join("law/law.json"));
    assert_eq!(law["id"], "matrix");
    assert_eq!(law["builtin"], true);
    assert!(law["params"]["carry"].is_number(), "{law}");
    assert_eq!(
        law["accepts"]["test_types"],
        json!(["sweep", "dwell", "stepped"])
    );
    let id = &run["identification"];
    assert_eq!(id["gate_passed"], true);
    assert!(id["tracker_delay_factor"].is_number());
    assert!(id["transport_delay_s"].is_number());
    assert!(folder.join("results/identification_frf.json").is_file());
    let frf = read_json(&folder.join("results/identification_frf.json"));
    assert_eq!(frf["channels"]["responses"], json!([1, 2, 3, 4, 5, 6]));
    assert_eq!(frf["channels"]["references"], json!([7, 8, 9, 10]));

    // The shaker and feasibility inputs.
    assert_eq!(
        setup["feasibility"]["inputs"]["shakers"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(r["aborts"]["shaker_ratings"].as_array().unwrap().len(), 4);
    assert!(r["aborts"]["shaker_ratings"][0]["max_velocity_m_s_pk"].is_number());

    // The timeline and the outcome.
    let log = std::fs::read_to_string(folder.join("log.jsonl")).unwrap();
    let kinds: Vec<String> = log
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    for k in ["state", "station", "gate", "level", "notch"] {
        assert!(kinds.iter().any(|x| x == k), "no {k} entry: {kinds:?}");
    }
    let e = read_json(&folder.join("end.json"));
    assert_eq!(e["end"]["outcome"], "completed");
    assert!(e["report"]["limits"][0]["notched_s"].as_f64().unwrap() > 0.0);

    // Every input and drive, and the results.
    let inputs = read_json(&folder.join("streams/inputs/stream.json"));
    assert_eq!(inputs["channels"].as_array().unwrap().len(), 6);
    let drives = read_json(&folder.join("streams/drives/stream.json"));
    assert_eq!(drives["channels"].as_array().unwrap().len(), 4);
    let levels = read_json(&folder.join("results/channel_levels.json"));
    assert_eq!(
        levels["channels"]["responses"],
        json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
    );
    let lines = std::fs::read_to_string(folder.join("results/channel_levels.jsonl")).unwrap();
    assert!(lines.lines().count() > 1);

    // The run's identity and times.
    assert_eq!(run["operator"], "A. Operator");
    assert_eq!(run["article"], "Plate");
    assert_eq!(run["specification"]["clause"], "4.2");
    assert!(run["build"]["version"].is_string());
    assert!(run["armed"]["unix"].is_number());
    assert!(run["armed"].get("utc_offset_s").is_some());
    assert!(e["at_unix"].is_number());

    // A unit on every array.
    for n in [
        "reference",
        "limit_ch5",
        "identification_frf",
        "identification_coherence",
        "identification_trust",
        "channel_levels",
        "control_trace",
    ] {
        arrays_have_units(&folder, n);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_the_folder_is_valid_while_the_run_drives() {
    let svc = Service::with_options(msine::paced(5.0, Duration::from_secs(600)));
    let mut c = Client::connect(serve(svc).await).await;
    controller(&mut c, 4, 0, 4).await;
    let cfg = msine::config(
        "sine-linear-4x4",
        4,
        0,
        4,
        msine::dwell(80.0, 60.0),
        "matrix",
    );
    c.ok("mimo_sine.configure", json!({"config": cfg})).await;
    let armed = c.ok("mimo_sine.arm", json!({})).await;
    let folder = msine::folder(&armed);
    // Before the first sample: the manifest, the table and the setup.
    for f in ["run.json", "table.json", "setup.json", "law/law.json"] {
        assert!(folder.join(f).is_file(), "{f} is not there at arm");
    }
    assert_eq!(read_json(&folder.join("run.json"))["complete"], false);
    c.ok("mimo_sine.start", json!({})).await;
    msine::phase(&mut c, "engaged").await;
    c.pump(Duration::from_millis(1500)).await;
    // Driving: incomplete, readable, the identification on disk.
    assert_eq!(read_json(&folder.join("run.json"))["complete"], false);
    assert!(folder.join("results/identification_frf.json").is_file());
    assert!(!folder.join("end.json").is_file());
    c.ok("mimo_sine.abort", json!({})).await;
    let end = msine::state(&mut c, "finished").await;
    assert_eq!(end["end"]["outcome"], "operator_abort");
    let run = msine::complete(&folder).await;
    assert_eq!(run["complete"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mimo_sine_a_record_that_cannot_be_written_refuses_the_arm() {
    let mut o = msine::opts(Duration::from_secs(600));
    let blocked = o.data_root.join("not-a-folder");
    std::fs::create_dir_all(&o.data_root).unwrap();
    std::fs::write(&blocked, b"a file where the folder would go").unwrap();
    o.capture_root = Some(blocked.clone());
    let mut c = Client::connect(serve(Service::with_options(o)).await).await;
    controller(&mut c, 4, 0, 4).await;
    let cfg = msine::config(
        "sine-linear-4x4",
        4,
        0,
        4,
        msine::dwell(80.0, 1.0),
        "matrix",
    );
    c.ok("mimo_sine.configure", json!({"config": cfg})).await;
    let e = c.refused("mimo_sine.arm", json!({})).await;
    assert_eq!(e["reason"], "record_unwritable", "{e}");
    let s = c.ok("mimo_sine.state", json!({})).await;
    assert_eq!(s["state"], "configured");
}
