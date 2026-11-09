//! The Explorer configuration on the wire against the core's own (schema/PROTOCOL.md,
//! "Explorer Configuration"): every wire field reaches `ExplorerHostConfig`, so moving
//! one moves exactly its field there; every refusal is by name with its `param` or
//! `field`; a key the wire does not have is refused by name; and the derived values are
//! the station's own arithmetic at the rates the bench runs.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_explorer_host::ExplorerHostConfig;
use sds_control::explorer::config::{self, CheckContext, StationFigures, WireExplorer};
use sds_control::table;
use serde_json::{json, Value};

/// A control row (1) in g, a drive row (2) on a 10 V range, a monitor row (3).
fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c = table::new_row(1);
    c.role = ChannelRole::Control;
    let mut d = table::new_row(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    d.range_max = 10.0;
    let mut m = table::new_row(3);
    m.role = ChannelRole::Monitor;
    t.channels = vec![c, d, m];
    t
}

fn full() -> Value {
    json!({
        "title": "Survey, fixture A",
        "acquisition": {"fs_hz": 12800},
        "band_hz": [20, 2000], "start_hz": 250, "max_drive_vpk": 0.5,
        "slew": {"freq_oct_per_s": 1, "drive_db_per_s": 6},
        "display": {"nfft": 4096},
        "capture": {"run_name": "explorer"},
        "metadata": [{"key": "run_number", "label": "Run Number", "value": "3"}]
    })
}

fn check_on(
    config: &Value,
    t: &ChannelTable,
    station: StationFigures,
) -> (table::Report, Option<config::Derived>) {
    let w = config::parse(config).expect("parses");
    w.check(&CheckContext {
        table: t,
        timeout_s: sds_control::session::TIMEOUT.as_secs_f64(),
        station,
    })
}

fn check(config: &Value) -> (table::Report, Option<config::Derived>) {
    check_on(config, &table(), StationFigures::Ni)
}

fn built(config: &Value) -> ExplorerHostConfig {
    let w = config::parse(config).unwrap();
    let (r, d) = check(config);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(d.is_some());
    w.build()
}

/// The fields of the core's configuration as one comparable tuple.
fn fields(c: &ExplorerHostConfig) -> Vec<(&'static str, f64)> {
    vec![
        ("fs_hz", c.fs_hz),
        ("band_lo", c.band_hz.0),
        ("band_hi", c.band_hz.1),
        ("start_hz", c.start_hz),
        ("max_drive_vpk", c.max_drive_vpk),
        ("freq_oct_per_s", c.slew.freq_oct_per_s),
        ("drive_db_per_s", c.slew.drive_db_per_s),
        ("ramp_span_db", c.ramp.ramp_span_db),
        ("display_nfft", c.display_nfft as f64),
        ("ring_blocks", c.ring_blocks as f64),
    ]
}

#[test]
fn explorer_every_wire_field_reaches_the_core_config() {
    let base = built(&full());
    let core = ExplorerHostConfig::default();
    assert_eq!(base.fs_hz, 12_800.0);
    assert_eq!(base.band_hz, (20.0, 2_000.0));
    assert_eq!(base.start_hz, 250.0);
    assert_eq!(base.max_drive_vpk, 0.5);
    assert_eq!(base.slew.freq_oct_per_s, 1.0);
    assert_eq!(base.slew.drive_db_per_s, 6.0);
    assert_eq!(base.display_nfft, 4096);
    assert_eq!(base.ramp, core.ramp, "the 40 dB span is the core's");
    assert_eq!(base.ring_blocks, core.ring_blocks);

    // Moving one wire field moves exactly its field of the core's configuration.
    let moves: [(&str, Value, &str); 8] = [
        ("/acquisition/fs_hz", json!(25600), "fs_hz"),
        ("/band_hz", json!([20, 1500]), "band_hi"),
        ("/band_hz", json!([30, 2000]), "band_lo"),
        ("/start_hz", json!(400), "start_hz"),
        ("/max_drive_vpk", json!(0.25), "max_drive_vpk"),
        ("/slew/freq_oct_per_s", json!(2), "freq_oct_per_s"),
        ("/slew/drive_db_per_s", json!(30), "drive_db_per_s"),
        ("/display/nfft", json!(8192), "display_nfft"),
    ];
    let before = fields(&base);
    for (path, value, field) in moves {
        let mut c = full();
        *c.pointer_mut(path).unwrap() = value;
        let after = fields(&built(&c));
        let moved: Vec<&str> = before
            .iter()
            .zip(&after)
            .filter(|(a, b)| a.1 != b.1)
            .map(|(a, _)| a.0)
            .collect();
        assert_eq!(moved, [field], "{path}");
    }

    // Every field but the ceiling has the core's default.
    let minimal = built(&json!({"max_drive_vpk": 0.5}));
    assert_eq!(
        fields(&minimal),
        fields(&ExplorerHostConfig {
            max_drive_vpk: 0.5,
            ..core
        })
    );
    let w = config::parse(&json!({"max_drive_vpk": 0.5})).unwrap();
    assert_eq!(w.run_name, "explorer");
    assert_eq!(w.metadata.len(), 4, "the default fields");
    assert_eq!(w.title, "");
    let w = config::parse(&full()).unwrap();
    assert_eq!(w.title, "Survey, fixture A");
    assert_eq!(w.metadata[0].label, "Run Number");
    assert_eq!(w.raw, full());
}

#[test]
fn explorer_a_shape_error_or_a_key_the_wire_lacks_is_bad_params_naming_it() {
    let refused = |c: Value, param: &str| {
        let e = config::parse(&c).expect_err(param);
        assert_eq!(e.reason, Some("bad_params"), "{param}: {e:?}");
        assert_eq!(e.detail.as_ref().unwrap()["param"], json!(param), "{e:?}");
    };
    let with = |path: &str, v: Value| {
        let mut c = full();
        *c.pointer_mut(path).unwrap() = v;
        c
    };
    let mut c = full();
    c.as_object_mut().unwrap().remove("max_drive_vpk");
    refused(c, "config.max_drive_vpk");
    let mut c = full();
    c["level_g"] = json!(1);
    refused(c, "config.level_g");
    let mut c = full();
    c["slew"]["drive_g_per_s"] = json!(1);
    refused(c, "config.slew.drive_g_per_s");
    let mut c = full();
    c["acquisition"]["tick_ms"] = json!(10);
    refused(c, "config.acquisition.tick_ms");
    let mut c = full();
    c["metadata"][0]["units"] = json!("g");
    refused(c, "config.metadata[0].units");
    refused(with("/band_hz", json!([20])), "config.band_hz");
    refused(with("/start_hz", json!("250")), "config.start_hz");
    refused(with("/display/nfft", json!(4096.5)), "config.display.nfft");
    refused(
        with("/capture/run_name", json!(3)),
        "config.capture.run_name",
    );
    refused(with("/slew", json!(6)), "config.slew");
    refused(json!([1]), "config");
}

#[test]
fn explorer_each_content_error_is_an_item_by_name_at_its_field() {
    let item = |path: &str, v: Value, reason: &str, field: &str| {
        let mut c = full();
        *c.pointer_mut(path).unwrap() = v;
        let (r, d) = check(&c);
        assert!(d.is_none(), "{path}: no derived values with errors");
        let got: Vec<(String, Value)> = r
            .errors
            .iter()
            .map(|i| (i.reason.to_owned(), i.to_json()["field"].clone()))
            .collect();
        assert_eq!(got, [(reason.to_owned(), json!(field))], "{path}");
        // The error response carries the same items.
        let e = config::config_invalid(&r);
        assert_eq!(e.reason, Some("config_invalid"));
        assert_eq!(e.detail.unwrap()["errors"][0]["reason"], json!(reason));
    };
    item(
        "/acquisition/fs_hz",
        json!(500),
        "out_of_range",
        "acquisition.fs_hz",
    );
    item(
        "/acquisition/fs_hz",
        json!(409_600),
        "out_of_range",
        "acquisition.fs_hz",
    );
    item("/band_hz", json!([20, 6000]), "band_above_grid", "band_hz");
    item("/band_hz", json!([0, 2000]), "out_of_range", "band_hz");
    item("/band_hz", json!([2000, 20]), "out_of_range", "band_hz");
    item("/start_hz", json!(10), "out_of_range", "start_hz");
    item("/max_drive_vpk", json!(0), "out_of_range", "max_drive_vpk");
    item("/max_drive_vpk", json!(12), "out_of_range", "max_drive_vpk");
    item(
        "/slew/freq_oct_per_s",
        json!(0),
        "out_of_range",
        "slew.freq_oct_per_s",
    );
    item(
        "/slew/freq_oct_per_s",
        json!(11),
        "out_of_range",
        "slew.freq_oct_per_s",
    );
    item(
        "/slew/drive_db_per_s",
        json!(-6),
        "out_of_range",
        "slew.drive_db_per_s",
    );
    item(
        "/slew/drive_db_per_s",
        json!(1001),
        "out_of_range",
        "slew.drive_db_per_s",
    );
    item("/display/nfft", json!(3000), "out_of_range", "display.nfft");
    item("/display/nfft", json!(512), "out_of_range", "display.nfft");
    item(
        "/display/nfft",
        json!(32768),
        "out_of_range",
        "display.nfft",
    );
    item(
        "/capture/run_name",
        json!("a/b"),
        "out_of_range",
        "capture.run_name",
    );
    item(
        "/capture/run_name",
        json!(".."),
        "out_of_range",
        "capture.run_name",
    );

    // The band's edge against the rate asked: 0.45 x 12 800 = 5760 Hz is the last band
    // edge accepted.
    let mut c = full();
    c["band_hz"] = json!([20, 5760]);
    assert!(check(&c).0.errors.is_empty());
    c["band_hz"] = json!([20, 5760.5]);
    assert_eq!(check(&c).0.errors[0].reason, "band_above_grid");

    // The ceiling against the drive row's range.
    let mut t = table();
    t.channels[1].range_max = 1.0;
    let mut c = full();
    c["max_drive_vpk"] = json!(1.5);
    let (r, _) = check_on(&c, &t, StationFigures::Ni);
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].reason, "ceiling_over_range");
    assert_eq!(r.errors[0].to_json()["field"], json!("max_drive_vpk"));
    c["max_drive_vpk"] = json!(1.0);
    assert!(check_on(&c, &t, StationFigures::Ni).0.errors.is_empty());
}

#[test]
fn explorer_the_table_carries_one_drive_row_and_an_input() {
    let reasons = |t: &ChannelTable| -> Vec<&'static str> {
        check_on(&full(), t, StationFigures::Ni)
            .0
            .errors
            .iter()
            .map(|i| i.reason)
            .collect()
    };
    let mut none = table();
    none.channels[1].enabled = false;
    assert_eq!(reasons(&none), ["no_drive_channel"]);
    let mut two = table();
    two.channels[2].role = ChannelRole::Drive;
    assert_eq!(reasons(&two), ["extra_drive_channels"]);
    let mut deaf = table();
    deaf.channels[0].enabled = false;
    deaf.channels[2].enabled = false;
    assert_eq!(reasons(&deaf), ["no_input_channel"]);
    let mut empty = table::Report::default();
    WireExplorer::check_table(&table(), &mut empty);
    assert!(empty.errors.is_empty());
    // The control row: the first enabled control row, else the first enabled input.
    assert_eq!(WireExplorer::control_row(&table()).unwrap().index, 1);
    let mut t = table();
    t.channels[0].role = ChannelRole::Monitor;
    assert_eq!(WireExplorer::control_row(&t).unwrap().index, 1);
    t.channels[0].enabled = false;
    assert_eq!(WireExplorer::control_row(&t).unwrap().index, 3);
}

#[test]
fn explorer_derived_values_are_the_stations_arithmetic() {
    let at = |fs: f64, station: StationFigures| {
        let mut c = full();
        c["acquisition"]["fs_hz"] = json!(fs);
        check_on(&c, &table(), station).1.unwrap()
    };
    // NI at 12 800 Hz: block 256 (20 ms), two blocks of lookahead, the input path's 39
    // frames of group delay (38.44 samples, rounded up): 2 x 512 + 39.
    let d = at(12_800.0, StationFigures::Ni);
    assert_eq!(
        (d.block_frames, d.lookahead_frames, d.pipeline_frames),
        (256, 512, 1063)
    );
    assert!((d.pipeline_s() - 0.083_047).abs() < 1e-6);
    let j = d.to_json();
    assert_eq!(j["pipeline_s"], json!(0.08305));
    assert_eq!(j["display"], json!({"df_hz": 3.125, "window_s": 0.32}));
    assert_eq!(j["survey"]["tolerance_hz"], json!(1.5625));
    assert_eq!(j["survey"]["first_point_s"], json!([0.383, 0.125]));
    assert_eq!(j["sweep_max_oct_per_min"], json!(60));
    assert_eq!(j["stop_wait_s"], json!(30));
    assert_eq!(
        j["grid"],
        json!({"fs_hz": 12800, "block_frames": 256, "lookahead_frames": 512, "actual": false})
    );
    // At 8192 Hz asked: block 128, 551 frames (the programmed 8533.33 Hz gives the same).
    let d = at(8_192.0, StationFigures::Ni);
    assert_eq!(
        (d.block_frames, d.lookahead_frames, d.pipeline_frames),
        (128, 256, 551)
    );
    // At 51 200 Hz: block 1024, lookahead 2048, 2 x 2048 + 39.
    let d = at(51_200.0, StationFigures::Ni);
    assert_eq!(
        (d.block_frames, d.lookahead_frames, d.pipeline_frames),
        (1024, 2048, 4135)
    );
    // The control service's simulator: its 4-frame lookahead and no group delay.
    let d = at(12_800.0, StationFigures::sim());
    assert_eq!((d.lookahead_frames, d.pipeline_frames), (4, 8));
    // A simulated station stating the NI lookahead: 1024, no group delay.
    let d = at(
        12_800.0,
        StationFigures::Sim {
            lookahead_frames: 512,
        },
    );
    assert_eq!(d.pipeline_frames, 1024);

    // The first survey point after a click, at 12 800 Hz on NI, per frequency: the
    // pipeline, the guard (512 frames or three cycles), three cycles.
    let ms: Vec<u64> = [20.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 1500.0, 2000.0]
        .iter()
        .map(|&hz| {
            (config::first_point_frames(1063, 256, 12_800.0, hz) as f64 / 12.8).round() as u64
        })
        .collect();
    assert_eq!(ms, [383, 203, 153, 135, 129, 126, 125, 125]);

    // A rise and the stop: 40 dB at the drive slew.
    for (slew, s) in [(6.0, 6.667), (30.0, 1.333), (240.0, 0.167)] {
        let mut c = full();
        c["slew"]["drive_db_per_s"] = json!(slew);
        let j = check(&c).1.unwrap().to_json();
        assert_eq!(
            (j["rise_s"].clone(), j["stop_s"].clone()),
            (json!(s), json!(s))
        );
    }
}
