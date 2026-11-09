//! The Recorder goldens in `schema/golden/`, against this crate's own code: the
//! configuration checks to the golden derived values on the NI station at 12 800 Hz, a
//! configuration with errors is refused as the golden error, the subscription parses to
//! the golden result, the states are the view builders' own output, the live and data
//! frames are the golden bytes, and the core's writer writes the golden recording's
//! samples under the golden record's names.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_recorder::store::{RecordStore, RecordingSpec, WriterStore};
use hertzforge_recorder::{AcquisitionConfig, StationFacts, TriggerEvent};
use sds_control::codec::{self, Arrays};
use sds_control::explorer::view::StationRef;
use sds_control::recorder::config::{self, CheckContext, RecorderStation};
use sds_control::recorder::subscribe::{self, DataContext, FrfData, RowSpectrum};
use sds_control::recorder::view::{self, Disk, Health, RecorderView, Recording, State, Trigger};
use sds_control::{protocol, table};
use serde_json::{json, Value};
use std::path::PathBuf;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(name)
}

fn read_json(name: &str) -> Value {
    let text = std::fs::read_to_string(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn read_bin(name: &str) -> Vec<u8> {
    std::fs::read(golden(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

/// Structural equality with numbers compared by value and keys in order.
fn same(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            assert_eq!(x.as_f64(), y.as_f64(), "number differs at {path}")
        }
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            assert_eq!(kx, ky, "keys differ at {path}");
            for (k, v) in x {
                same(v, &y[k], &format!("{path}.{k}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "lengths differ at {path}");
            for (i, (u, v)) in x.iter().zip(y).enumerate() {
                same(u, v, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "values differ at {path}"),
    }
}

fn bytes_equal(name: &str, got: &[u8], want: &[u8]) {
    if got != want {
        let (a, b) = (
            codec::decode(got).unwrap().expanded().to_string(),
            codec::decode(want).unwrap().expanded().to_string(),
        );
        let at = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        let from = at.saturating_sub(150);
        panic!(
            "{name}: decoded frames differ at {at}\n got: {}\nwant: {}",
            &a[from..(at + 150).min(a.len())],
            &b[from..(at + 150).min(b.len())]
        );
    }
}

/// Four recorded rows at range 5 g (Table, Fixture, Bracket, Cover) and a drive row 5.
fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let names = ["Table", "Fixture", "Bracket", "Cover"];
    t.channels = (1..=4u32)
        .map(|i| {
            let mut c = table::new_row(i);
            c.role = if i == 1 {
                ChannelRole::Control
            } else {
                ChannelRole::Monitor
            };
            c.description = names[i as usize - 1].into();
            c.units = Units::AccelG;
            c.range_max = 5.0;
            c
        })
        .collect();
    let mut d = table::new_row(5);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    d.range_max = 10.0;
    t.channels.push(d);
    t
}

fn golden_config() -> Value {
    let request = read_json("request_recorder_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "recorder.configure");
    parsed.params["config"].clone()
}

fn check(config: &Value) -> (table::Report, Option<config::Derived>) {
    config::parse(config).unwrap().check(&CheckContext {
        table: &table(),
        station: RecorderStation::Ni,
        free_bytes: Some(107_374_182_400),
    })
}

#[test]
fn recorder_the_configuration_checks_to_the_golden_derived_values() {
    let (report, derived) = check(&golden_config());
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let want = read_json("response_recorder_configure.json");
    same(
        &derived.unwrap().to_json(),
        &want["result"]["derived"],
        "derived",
    );
    same(
        &Value::Array(report.warnings.iter().map(|i| i.to_json()).collect()),
        &want["result"]["warnings"],
        "warnings",
    );
}

#[test]
fn recorder_a_configuration_with_errors_is_refused_as_the_golden_error() {
    let mut config = golden_config();
    config["trigger"]["row"] = json!(5);
    config["trigger"]["pre_s"] = json!(12);
    config["analysis"]["block"] = json!(3000);
    let (report, derived) = check(&config);
    assert!(derived.is_none());
    let e = config::config_invalid(&report);
    same(
        &protocol::error_response(109, &e),
        &read_json("response_error_recorder_config_invalid.json"),
        "error",
    );
}

#[test]
fn recorder_the_subscription_parses_to_the_golden_result() {
    let request = read_json("request_recorder_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    let sub = subscribe::parse(&parsed.params).unwrap();
    same(
        &subscribe::result_json(sub.as_ref()),
        &read_json("response_recorder_subscribe.json")["result"],
        "subscribe",
    );
}

fn ni_station() -> StationRef {
    StationRef {
        kind: "ni",
        class: "ni_real",
        devices: vec![("cDAQ2Mod1".into(), "01A2B3C4".into())],
    }
}

fn configured() -> RecorderView {
    let config = golden_config();
    let w = config::parse(&config).unwrap();
    let d = check(&config).1.unwrap();
    let mut v = view::configured_view("recorder-0001", &w, &d, ni_station(), &table());
    v.source = Some("HARDWARE");
    v
}

fn health(batches: u64, last: f64, worst: f64, gap: Option<f64>) -> Health {
    Health {
        batches,
        blocks_dropped: 0,
        driver_warnings: 0,
        station_errors: 0,
        pass_last_ms: Some(last),
        pass_worst_ms: Some(worst),
        batch_gap_ms: gap,
    }
}

fn running() -> RecorderView {
    let mut v = configured();
    v.run_id = "recorder-0002".into();
    v.state = State::Running;
    v.phase = "recording";
    v.grid.actual = true;
    v.clock = Some("shared");
    v.t_s = Some(11.36);
    v.station_frames = Some(145_408);
    v.trigger = Some(Trigger {
        armed: false,
        pre_frames: 6400,
        held_frames: 0,
        fires: 2,
        last_station_frame: Some(134_422),
        saved: 1,
        left: Some(3),
        holdoff_left_s: 0.0,
    });
    v.recording = Some(Recording {
        name: "rec_0002".into(),
        trigger: "level".into(),
        first_station_frame: 128_022,
        trigger_station_frame: 134_422,
        pre_frames: 6400,
        pre_frames_asked: 6400,
        frames_fed: 17_386,
        frames_on_disk: 16_384,
        target_frames: Some(19_200),
        seconds: 10_986.0 / 12_800.0,
        length_s: Some(1.0),
        backlog_s: 1002.0 / 12_800.0,
    });
    v.recordings = 1;
    v.disk = Some(Disk {
        free_bytes: Some(107_373_363_200),
        floor_bytes: 1_073_741_824,
        seconds_above_floor: Some(519_041),
        fault: None,
    });
    v.health = Some(health(284, 1.2, 2.8, Some(3.9)));
    let rows = [(0.0221, 0.5), (0.1414, 0.2), (0.0707, 0.1), (0.1061, 0.15)];
    for (r, (rms, peak)) in v.live_rows.iter_mut().zip(rows) {
        r.rms = Some(rms);
        r.peak = Some(peak);
    }
    v
}

#[test]
fn recorder_the_configured_started_and_running_states_are_the_golden_ones() {
    same(
        &view::state_json(Some(&configured())),
        &read_json("response_recorder_configure.json")["result"]["state"],
        "configured",
    );
    let mut started = configured();
    started.state = State::Running;
    started.phase = "monitoring";
    started.grid.actual = true;
    started.clock = Some("shared");
    started.t_s = Some(0.04);
    started.station_frames = Some(512);
    started.health = Some(health(1, 1.2, 1.2, None));
    same(
        &view::state_json(Some(&started)),
        &read_json("response_recorder_start.json")["result"],
        "started",
    );
    let event = read_json("event_recorder_state.json");
    assert_eq!(event["topic"], "recorder.state");
    same(
        &view::state_json(Some(&running())),
        &event["data"],
        "running",
    );
    assert_eq!(view::state_json(None)["state"], json!("unconfigured"));
}

#[test]
fn recorder_the_live_and_data_frames_are_the_golden_bytes() {
    let mut arrays = Arrays::new();
    let data = view::live_json(&running(), &mut arrays);
    let got = codec::encode(&protocol::event("recorder.live", 211, data), &arrays).unwrap();
    bytes_equal(
        "event_recorder_live",
        &got,
        &read_bin("event_recorder_live.bin"),
    );

    let request = read_json("request_recorder_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    // The last 512 frames of rows 1 and 2 at 64 points: 8 frames a point, each
    // interval's largest and smallest the golden's.
    let first = 145_408 - 512;
    let mut cols: Vec<Vec<f32>> = vec![Vec::new(); 4];
    for i in 0..64usize {
        let hi1 = 0.125 + (i % 4) as f32 / 64.0;
        let hi2 = 0.1875 - (i % 8) as f32 / 64.0;
        for k in 0..8 {
            let s = if k == 0 {
                1.0
            } else if k == 1 {
                -1.0
            } else {
                0.0
            };
            cols[0].push(hi1 * s);
            cols[1].push(hi2 * s);
            cols[2].push(0.0);
            cols[3].push(0.0);
        }
    }
    let spectra = vec![RowSpectrum {
        index: 1,
        description: "Table".into(),
        unit: "g".into(),
        df_hz: 6.25,
        psd: vec![0.0009765625, 0.001953125, 0.00048828125, 0.000244140625],
    }];
    let frf = FrfData {
        unit: "g/g".into(),
        df_hz: 6.25,
        mag: vec![0.5; 4],
        phase_deg: vec![0.0, -0.17578125, -0.3515625, -0.52734375],
        coherence: vec![0.0, 0.9990234375, 0.998046875, 0.9970703125],
        valid: true,
    };
    let v = running();
    let ctx = DataContext {
        view: &v,
        time: Some((first, &cols)),
        spectra: &spectra,
        frf: Some(&frf),
    };
    let mut arrays = Arrays::new();
    let data = subscribe::data_json(&ctx, &sub, &mut arrays);
    let got = codec::encode(&protocol::event("recorder.data", 212, data), &arrays).unwrap();
    bytes_equal(
        "event_recorder_data",
        &got,
        &read_bin("event_recorder_data.bin"),
    );
}

#[test]
fn recorder_the_cores_writer_writes_the_golden_recording() {
    // The golden run folder's one recording: one second of two rows at 1024 Hz, its
    // first station frame 2048, a 256-frame pre-window, every value a function of its
    // station frame.
    let dir = golden("recorder_run/recordings/rec_0001");
    let want_record: Value = read_json("recorder_run/recordings/rec_0001/recorder.json");
    let want_bytes = std::fs::read(dir.join("telemetry.f32")).unwrap();
    let row = |n: u64| -> [f32; 2] {
        [
            1.0 - ((n % 64) as f32 - 32.0).abs() / 16.0,
            (n % 1024) as f32 / 1024.0 - 0.5,
        ]
    };
    let mut t = table::Table::new().table;
    t.channels = (1..=2u32)
        .map(|i| {
            let mut c = table::new_row(i);
            c.role = if i == 1 {
                ChannelRole::Control
            } else {
                ChannelRole::Monitor
            };
            c.units = Units::AccelG;
            c.range_max = 5.0;
            c
        })
        .collect();
    let pre: Vec<f32> = (2048..2304u64).flat_map(row).collect();
    let root = std::env::temp_dir().join(format!("recorder_golden_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut store = WriterStore::default();
    store
        .start(RecordingSpec {
            data_root: root.clone(),
            name: "rec_0001".into(),
            table: t,
            acq: AcquisitionConfig {
                native_rate_hz: 1024.0,
                block_size: 256,
                ..Default::default()
            },
            requested_fs_hz: 1024.0,
            floor_bytes: 0,
            pretrigger: Some(TriggerEvent {
                trigger_frame_global: 2304,
                pretrigger: pre,
                pretrigger_frames: 256,
            }),
            station: StationFacts {
                name: "rec_0001".into(),
                trigger: "manual".into(),
                group_delay_frames: 0.0,
                first_station_frame: 2048,
                pre_frames_asked: 256,
            },
        })
        .unwrap();
    let live: Vec<f32> = (2304..3072u64).flat_map(row).collect();
    store.push(&live, 768);
    store.finish().unwrap();
    let got_bytes = std::fs::read(root.join("rec_0001/telemetry.f32")).unwrap();
    let got_record: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("rec_0001/recorder.json")).unwrap(),
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(got_bytes, want_bytes, "the samples byte for byte");
    // Every field of the golden record, in the golden's order; the writer's own extra
    // fields follow them (a reader ignores fields it does not know).
    let got_keys: Vec<&String> = got_record.as_object().unwrap().keys().collect();
    let want_keys: Vec<&String> = want_record.as_object().unwrap().keys().collect();
    assert_eq!(&got_keys[..want_keys.len()], &want_keys[..]);
    for k in want_keys {
        same(&got_record[k], &want_record[k], k);
    }
}
