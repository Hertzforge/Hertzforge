//! Every Recorder configuration field reaches the core's `RecorderConfig`, and every
//! refusal is named (schema/PROTOCOL.md, "Recorder Configuration"): a shape problem is
//! `bad_params` with its `param`, a content problem a `config_invalid` item with its
//! `field`.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_recorder::{AveragingKind, Slope, TriggerMode, WindowKind};
use sds_control::protocol::ErrorCode;
use sds_control::recorder::config::{parse, CheckContext, RecorderStation, WireRecorder};
use serde_json::{json, Value};

fn row(index: u32, role: ChannelRole, range: f64) -> Channel {
    let mut c = Channel::new(index);
    c.role = role;
    c.enabled = true;
    c.units = Units::AccelG;
    c.range_max = range;
    c.sensitivity_mv_per_eu = 1000.0;
    c
}

/// Four inputs at range 5 and a drive row, as the bench's table.
fn table() -> ChannelTable {
    let mut t = ChannelTable::new();
    t.channels.push(row(1, ChannelRole::Control, 5.0));
    for i in 2..=4 {
        t.channels.push(row(i, ChannelRole::Monitor, 5.0));
    }
    t.channels.push(row(5, ChannelRole::Drive, 10.0));
    t
}

/// PROTOCOL's example, every field given.
fn example() -> Value {
    json!({
        "title": "Fixture A, run 4",
        "acquisition": {"fs_hz": 12800},
        "trigger": {"mode": "level", "row": 1, "level": 0.25, "slope": "rising",
                    "hysteresis": null, "pre_s": 0.5, "record_s": 1.0, "count": 5,
                    "holdoff_s": 0},
        "analysis": {"block": 2048, "window": "hann", "overlap_pct": null,
                     "averaging": "linear", "averages": 8, "decimation": 1,
                     "frf": {"reference": 2, "response": 4}},
        "capture": {"run_name": "recorder", "keep_free_gb": 1},
        "metadata": [{"key": "run_number", "label": "Run Number", "value": "4"}]
    })
}

fn with(mut v: Value, path: &str, value: Value) -> Value {
    let keys: Vec<&str> = path.split('.').collect();
    let mut node = &mut v;
    for k in &keys[..keys.len() - 1] {
        node = node.get_mut(*k).unwrap();
    }
    node[keys[keys.len() - 1]] = value;
    v
}

fn without(mut v: Value, path: &str) -> Value {
    let keys: Vec<&str> = path.split('.').collect();
    let mut node = &mut v;
    for k in &keys[..keys.len() - 1] {
        node = node.get_mut(*k).unwrap();
    }
    node.as_object_mut().unwrap().remove(keys[keys.len() - 1]);
    v
}

fn ctx(t: &ChannelTable) -> CheckContext<'_> {
    CheckContext {
        table: t,
        station: RecorderStation::Ni,
        free_bytes: Some(107_374_182_400),
    }
}

fn errors(w: &WireRecorder, t: &ChannelTable) -> Vec<(String, String)> {
    let (r, _) = w.check(&ctx(t));
    r.errors
        .iter()
        .map(|i| {
            let j = i.to_json();
            (
                j["reason"].as_str().unwrap().to_owned(),
                j.get("field")
                    .or_else(|| j.get("channel"))
                    .map(|v| v.to_string().trim_matches('"').to_owned())
                    .unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn recorder_every_wire_field_reaches_the_cores_config() {
    let t = table();
    let w = parse(&example()).unwrap();
    let core = w.build(&t);
    assert_eq!(core.acq.native_rate_hz, 12_800.0);
    assert_eq!(core.acq.block_size, 2048);
    assert_eq!(core.acq.window, WindowKind::Hann);
    assert_eq!(core.acq.overlap_pct, 50.0, "null takes the window's own");
    assert_eq!(core.acq.averaging, AveragingKind::Linear);
    assert_eq!(core.acq.average_count, 8);
    assert_eq!(core.acq.analysis_decimation, 1);
    assert_eq!(core.pretrigger_s, Some(0.5));
    assert_eq!(core.timed_record_s, Some(1.0));
    assert_eq!(
        core.auto_rearm, 4,
        "count 5 is the first recording and four re-arms"
    );
    assert_eq!(
        core.frf_pair,
        Some((1, 3)),
        "rows 2 and 4 are columns 1 and 3"
    );
    assert_eq!(core.data_root, None, "the wire offers no data root");
    match core.trigger_mode {
        TriggerMode::Level(l) => {
            assert_eq!(l.channel_index, 0);
            assert_eq!(l.threshold_eu, 0.25);
            assert_eq!(l.slope, Slope::Rising);
            assert!((l.hysteresis_eu - 0.05).abs() < 1e-12, "1% of range 5");
            assert_eq!(l.holdoff_s, 0.0);
        }
        other => panic!("level mode became {other:?}"),
    }
    assert_eq!(core.metadata.len(), 1);
    assert_eq!(core.metadata[0].value, "4");

    // Each field changed reaches the core changed.
    let changed: [(&str, Value); 14] = [
        ("acquisition.fs_hz", json!(51200)),
        ("trigger.row", json!(3)),
        ("trigger.level", json!(-0.5)),
        ("trigger.slope", json!("falling")),
        ("trigger.hysteresis", json!(0.2)),
        ("trigger.pre_s", json!(2.0)),
        ("trigger.record_s", json!(3.0)),
        ("trigger.count", json!(2)),
        ("trigger.holdoff_s", json!(1.5)),
        ("analysis.block", json!(4096)),
        ("analysis.window", json!("flattop")),
        ("analysis.averaging", json!("peak_hold")),
        ("analysis.averages", json!(16)),
        ("analysis.decimation", json!(4)),
    ];
    for (path, value) in changed {
        let other = parse(&with(example(), path, value.clone()))
            .unwrap()
            .build(&t);
        assert_ne!(other, core, "{path} = {value} did not reach the core");
    }
    let other = parse(&with(example(), "analysis.overlap_pct", json!(25)))
        .unwrap()
        .build(&t);
    assert_eq!(other.acq.overlap_pct, 25.0);
    let other = parse(&with(example(), "trigger.count", Value::Null))
        .unwrap()
        .build(&t);
    assert_eq!(other.auto_rearm, u32::MAX, "null is until the run ends");
    let other = parse(&with(example(), "analysis.frf", Value::Null))
        .unwrap()
        .build(&t);
    assert_eq!(other.frf_pair, None);
    // The run name is the run's (the run folder); the floor reaches the core too, which
    // keeps it while the run records: the running core kept its 1 GiB least whatever was
    // asked, so a run asked to keep more never ended at its floor.
    let w2 = parse(&with(example(), "capture.keep_free_gb", json!(3))).unwrap();
    let (_, d) = w2.check(&ctx(&t));
    assert_eq!(d.unwrap().floor_bytes, 3 * 1_073_741_824);
    assert_eq!(w2.build(&t).keep_free_bytes, Some(3 * 1_073_741_824));
    let frac = parse(&with(example(), "capture.keep_free_gb", json!(667.25))).unwrap();
    assert_eq!(
        frac.build(&t).keep_free_bytes,
        Some((667.25 * 1_073_741_824.0f64).round() as u64)
    );
    assert_eq!(
        parse(&with(example(), "capture.run_name", json!("bench")))
            .unwrap()
            .run_name,
        "bench"
    );
    assert_eq!(parse(&example()).unwrap().title, "Fixture A, run 4");
}

#[test]
fn recorder_start_and_manual_modes_reach_the_core_without_a_level() {
    let t = table();
    let start = parse(&json!({"trigger": {"mode": "start", "record_s": 10}}))
        .unwrap()
        .build(&t);
    assert_eq!(start.trigger_mode, TriggerMode::Manual);
    assert_eq!(
        start.pretrigger_s,
        Some(0.0),
        "start mode has no pre-window"
    );
    assert_eq!(start.timed_record_s, Some(10.0));
    let manual = parse(&json!({})).unwrap();
    assert_eq!(manual.fs_hz, 12_800.0, "the rate asked when none is named");
    let core = manual.build(&t);
    assert_eq!(core.trigger_mode, TriggerMode::Manual);
    assert_eq!(core.pretrigger_s, Some(1.0));
    assert_eq!(core.timed_record_s, None);
    assert_eq!(core.auto_rearm, 0);
}

#[test]
fn recorder_shape_problems_are_bad_params_naming_the_field() {
    let cases: [(Value, &str); 9] = [
        (without(example(), "trigger.row"), "config.trigger.row"),
        (without(example(), "trigger.level"), "config.trigger.level"),
        (
            without(example(), "trigger.record_s"),
            "config.trigger.record_s",
        ),
        (
            with(example(), "trigger.mode", json!("free_run")),
            "config.trigger.mode",
        ),
        (
            with(example(), "trigger.level", json!("high")),
            "config.trigger.level",
        ),
        (
            with(example(), "analysis.window", json!("kaiser")),
            "config.analysis.window",
        ),
        (
            with(example(), "analysis.block", json!(2048.5)),
            "config.analysis.block",
        ),
        (
            with(example(), "capture.data_root", json!("/tmp")),
            "config.capture.data_root",
        ),
        (with(example(), "drive", json!({})), "config.drive"),
    ];
    for (config, param) in cases {
        let e = parse(&config).expect_err(param);
        assert_eq!(e.code, ErrorCode::BadRequest, "{param}: {e:?}");
        assert_eq!(e.reason, Some("bad_params"), "{param}");
        assert_eq!(e.detail.as_ref().unwrap()["param"], param, "{e:?}");
    }
}

#[test]
fn recorder_content_problems_are_items_located_by_field() {
    let t = table();
    let cases: [(&str, Value, &str, &str); 13] = [
        ("trigger.row", json!(5), "row_not_recorded", "trigger.row"),
        ("trigger.row", json!(9), "row_not_recorded", "trigger.row"),
        ("trigger.level", json!(6), "out_of_range", "trigger.level"),
        (
            "trigger.hysteresis",
            json!(0),
            "out_of_range",
            "trigger.hysteresis",
        ),
        ("trigger.pre_s", json!(12), "out_of_range", "trigger.pre_s"),
        (
            "trigger.record_s",
            json!(0),
            "out_of_range",
            "trigger.record_s",
        ),
        ("trigger.count", json!(0), "out_of_range", "trigger.count"),
        (
            "trigger.holdoff_s",
            json!(-1),
            "out_of_range",
            "trigger.holdoff_s",
        ),
        (
            "analysis.block",
            json!(3000),
            "not_power_of_two",
            "analysis.block",
        ),
        (
            "analysis.block",
            json!(128),
            "out_of_range",
            "analysis.block",
        ),
        (
            "analysis.decimation",
            json!(3),
            "not_power_of_two",
            "analysis.decimation",
        ),
        (
            "acquisition.fs_hz",
            json!(500),
            "out_of_range",
            "acquisition.fs_hz",
        ),
        (
            "capture.keep_free_gb",
            json!(0.5),
            "out_of_range",
            "capture.keep_free_gb",
        ),
    ];
    for (path, value, reason, field) in cases {
        let w = parse(&with(example(), path, value.clone())).unwrap();
        let errs = errors(&w, &t);
        assert_eq!(
            errs,
            vec![(reason.to_owned(), field.to_owned())],
            "{path} = {value}"
        );
        assert!(
            w.check(&ctx(&t)).1.is_none(),
            "no derived values with errors"
        );
    }
    let same = parse(&with(
        example(),
        "analysis.frf",
        json!({"reference": 2, "response": 2}),
    ))
    .unwrap();
    assert_eq!(
        errors(&same, &t),
        vec![("frf_same_row".to_owned(), "analysis.frf".to_owned())]
    );
    let gone = parse(&with(
        example(),
        "analysis.frf",
        json!({"reference": 2, "response": 7}),
    ))
    .unwrap();
    assert_eq!(
        errors(&gone, &t),
        vec![("row_not_recorded".to_owned(), "analysis.frf".to_owned())]
    );
    let path = parse(&with(example(), "capture.run_name", json!("../up"))).unwrap();
    assert_eq!(
        errors(&path, &t),
        vec![("out_of_range".to_owned(), "capture.run_name".to_owned())]
    );
}

#[test]
fn recorder_a_table_with_no_input_is_refused_and_drive_rows_are_named() {
    let mut drive_only = ChannelTable::new();
    drive_only.channels.push(row(1, ChannelRole::Drive, 10.0));
    let w = parse(&json!({})).unwrap();
    let (r, d) = w.check(&ctx(&drive_only));
    assert!(d.is_none());
    assert_eq!(r.errors[0].reason, "no_input_channel");
    let t = table();
    let (r, d) = w.check(&ctx(&t));
    assert!(r.errors.is_empty());
    assert_eq!(r.warnings.len(), 1);
    assert_eq!(r.warnings[0].reason, "drive_channel_present");
    assert_eq!(r.warnings[0].to_json()["channel"], 5);
    assert_eq!(
        d.unwrap().rows,
        vec![1, 2, 3, 4],
        "the drive row is not recorded"
    );
}

#[test]
fn recorder_derived_values_are_protocols_example_on_ni_at_12800_hz() {
    let t = table();
    let (r, d) = parse(&example()).unwrap().check(&ctx(&t));
    assert!(r.errors.is_empty(), "{r:?}");
    let d = d.unwrap().to_json();
    assert_eq!(
        d,
        json!({
            "grid": {"fs_hz": 12800, "block_frames": 256, "batch_frames": 512,
                     "group_delay_frames": 38.44, "actual": false},
            "outputs": "none", "rows": [1, 2, 3, 4],
            "analysis": {"fs_hz": 12800, "df_hz": 6.25, "lines": 800, "span_hz": 5000,
                         "block_s": 0.16, "overlap_pct": 50, "averages_s": 0.72},
            "trigger": {"pre_frames": 6400, "record_frames": 12800, "holdoff_frames": 0},
            "disk": {"bytes_per_s": 204800, "floor_bytes": 1073741824,
                     "free_bytes": 107374182400u64, "seconds_above_floor": 519045}
        })
    );
    // The bench's other rates and decimation 4.
    let at = |fs: f64, station| {
        let w = parse(&with(example(), "acquisition.fs_hz", json!(fs))).unwrap();
        w.derive(&t, station, fs, None)
    };
    let d51 = at(51_200.0, RecorderStation::Ni);
    assert_eq!(d51.block_frames, 1024);
    assert_eq!(d51.group_delay_frames, 38.56);
    assert_eq!(d51.bytes_per_s, 819_200);
    let fs = 13_107_200.0 / 256.0 / 6.0;
    let d85 = at(fs, RecorderStation::Ni);
    assert_eq!(d85.block_frames, 128);
    assert_eq!(sds_control::recorder::config::frames_for(10.0, fs), 85_333);
    assert_eq!(at(12_800.0, RecorderStation::Sim).group_delay_frames, 0.0);
    let dec4 = parse(&with(example(), "analysis.decimation", json!(4)))
        .unwrap()
        .derive(&t, RecorderStation::Ni, 12_800.0, None);
    assert_eq!(
        (dec4.analysis_fs_hz, dec4.span_hz, dec4.df_hz),
        (3200.0, 1250.0, 1.5625)
    );
    let avg16 = parse(&with(example(), "analysis.averages", json!(16)))
        .unwrap()
        .derive(&t, RecorderStation::Ni, 12_800.0, None);
    assert_eq!(avg16.averages_s, 1.36);
}
