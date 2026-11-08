//! The TWR goldens in `schema/golden/`, built by this crate's own builders: the
//! configuration parses and checks to the golden derived values, a configuration
//! with errors is refused as the golden error, the import reads to the golden
//! facts, the state, the plays and the check report are the golden ones, the
//! binary frames decode and re-encode byte for byte and encoding reproduces them,
//! and the text frames are compact text frames.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_random_laws::CheckCase;
use hertzforge_twr_laws::{TwrCaseEnd, TwrCaseMeasure, TwrCheckCase};
use sds_control::codec::{self, Arrays, Frame};
use sds_control::twr::config::{self, CheckContext, Grid};
use sds_control::twr::reference::{
    self, ImportOptions, ImportRequest, PreviewFigures, Source, Stored,
};
use sds_control::twr::subscribe::{self, DataContext};
use sds_control::twr::view::{
    self, Counts, IdentFacts, IdentFigures, InFlight, LawCall, LawValueSpec, MaskFigures,
    PlayRecords, PlayRow, Prediction, Scope, Spectra, State, StationRef, TwrView,
};
use sds_control::{protocol, table};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

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
        if let (Ok(a), Ok(b)) = (codec::decode(got), codec::decode(want)) {
            let (a, b) = (a.expanded().to_string(), b.expanded().to_string());
            let at = a
                .bytes()
                .zip(b.bytes())
                .position(|(x, y)| x != y)
                .unwrap_or(0);
            let from = at.saturating_sub(200);
            panic!(
                "{name}: decoded frames differ at {at}\n got: {}\nwant: {}",
                &a[from..(at + 200).min(a.len())],
                &b[from..(at + 200).min(b.len())]
            );
        }
        let at = got.iter().zip(want).position(|(a, b)| a != b);
        panic!(
            "{name}: {} bytes encoded vs {} golden; first difference at {at:?}\n got: {}\nwant: {}",
            got.len(),
            want.len(),
            String::from_utf8_lossy(&got[..got.len().min(900)]),
            String::from_utf8_lossy(&want[..want.len().min(900)]),
        );
    }
}

fn scratch() -> PathBuf {
    std::env::temp_dir().join(format!(
        "sds-twr-golden-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// A control row (1) in g and a drive row (2).
fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c = table::new_row(1);
    c.role = ChannelRole::Control;
    let mut d = table::new_row(2);
    d.role = ChannelRole::Drive;
    d.units = Units::VoltageV;
    t.channels = vec![c, d];
    t
}

/// The golden configuration's reference: a 200 Hz triangle starting at rest.
fn r1() -> Stored {
    let samples: Vec<f64> = (0..2048)
        .map(|i| {
            let k = ((i + 16) % 64) as f64;
            0.25 * (1.0 - (k - 32.0).abs() / 16.0)
        })
        .collect();
    let text: String = samples.iter().map(|v| format!("{v}\n")).collect();
    let dir = scratch();
    let s = reference::import(
        &dir,
        &ImportRequest {
            source: Source::Text(text),
            options: ImportOptions {
                fs_hz: Some(12_800.0),
                column: 0,
                units: None,
            },
            name: Some("r1".into()),
        },
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(dir);
    s
}

fn golden_config() -> Value {
    let request = read_json("request_twr_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "twr.configure");
    parsed.params["config"].clone()
}

fn check(config: &Value) -> (table::Report, Option<config::Derived>) {
    let w = config::parse(config).unwrap();
    let t = table();
    let stored = Arc::new(r1());
    let refs = move |sha: &str| (sha == stored.sha256).then(|| stored.clone());
    let law = |id: &str| sds_control::laws::twr_lookup(None, id);
    let file = |_: &Stored, _: &str| Ok(());
    let ctx = CheckContext {
        table: &t,
        law: &law,
        reference: &refs,
        file_check: &file,
    };
    let (r, d, _) = w.check(&ctx);
    (r, d)
}

#[test]
fn the_reference_hash_is_the_goldens() {
    assert_eq!(
        golden_config()["reference"]["sha256"],
        json!(r1().sha256),
        "the golden's SHA-256 is of the record the service keeps"
    );
}

#[test]
fn the_configuration_checks_to_the_golden_derived_values() {
    let (report, derived) = check(&golden_config());
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let want = read_json("response_twr_configure.json");
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
fn a_configuration_with_errors_is_refused_as_the_golden_error() {
    let mut config = golden_config();
    config["limits"]["drive_vpk_ceiling"] = json!(12);
    config["limits"]["envelope_alarm_frac"] = json!(0.6);
    config["correction"]["band_hz"] = json!([10, 3000]);
    let (report, derived) = check(&config);
    assert!(derived.is_none());
    let e = config::config_invalid(&report);
    let want = read_json("response_error_twr_config_invalid.json");
    same(&protocol::error_response(89, &e), &want, "error");
}

fn sim_station() -> StationRef {
    StationRef {
        kind: "sim",
        class: "sim",
        devices: Vec::new(),
    }
}

#[test]
fn the_configured_state_is_the_golden_one() {
    let config = golden_config();
    let (_, derived) = check(&config);
    let derived = derived.unwrap();
    let w = config::parse(&config).unwrap();
    let v = view::configured_view("twr-0001", &w, &derived, sim_station(), "g");
    let want = read_json("response_twr_configure.json");
    same(
        &view::state_json(Some(&v)),
        &want["result"]["state"],
        "state",
    );
    same(
        &view::state_json(None)["state"],
        &json!("unconfigured"),
        "unconfigured",
    );
}

// --- the golden run's figures (schema/make_golden.py, "Time waveform replication") ----

fn law_call(
    used: bool,
    miss: Option<&'static str>,
    ms: f64,
    rises: Option<(f64, f64, f64)>,
    values: &[(&str, f64)],
) -> LawCall {
    LawCall {
        used,
        miss,
        call_ms: ms,
        peak_rise_db: rises.map(|r| r.0),
        bin_rise_db: rises.map(|r| r.1),
        predicted_peak_ratio: rises.map(|r| r.2),
        values: values.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn row(
    index: u64,
    playback: bool,
    level_db: f64,
    rung: usize,
    iteration: Option<u32>,
    rms: f64,
    peak: f64,
    spectral: f64,
    gated: bool,
    clamped: u64,
    drive: f64,
    response: f64,
    start: u64,
    at_s: f64,
    law: Option<LawCall>,
) -> PlayRow {
    PlayRow {
        index,
        playback,
        level_db,
        rung: Some(rung),
        iteration,
        rms_error_pct: rms,
        peak_error_pct: peak,
        spectral_error_db: spectral,
        gated,
        clipped_samples: 0,
        clamped_bins: clamped,
        drive_vpk: drive,
        response_peak: response,
        lag_frames: None,
        drift_frames: None,
        start_frame: start,
        frames: 5120,
        at_s,
        law,
    }
}

fn rows() -> Vec<PlayRow> {
    vec![
        row(
            0,
            false,
            -12.0,
            0,
            Some(1),
            25.0,
            50.0,
            1.25,
            false,
            3,
            0.0625,
            0.046875,
            52224,
            4.08,
            Some(law_call(
                true,
                None,
                3.25,
                Some((0.5, 1.25, 1.0)),
                &[
                    ("corrected_bins", 812.0),
                    ("clamped_bins", 3.0),
                    ("drive_change_db", 0.5),
                ],
            )),
        ),
        row(
            1,
            false,
            -12.0,
            0,
            Some(2),
            12.5,
            25.0,
            0.625,
            false,
            0,
            0.0625,
            0.0546875,
            57344,
            4.48,
            Some(law_call(
                true,
                None,
                3.5,
                Some((0.25, 0.75, 1.0)),
                &[
                    ("corrected_bins", 812.0),
                    ("clamped_bins", 0.0),
                    ("drive_change_db", 0.25),
                ],
            )),
        ),
        row(
            2,
            false,
            -12.0,
            0,
            Some(3),
            6.25,
            12.5,
            0.375,
            true,
            0,
            0.0625,
            0.059,
            62464,
            4.88,
            Some(law_call(false, Some("over_budget"), 1500.0, None, &[])),
        ),
    ]
}

/// The golden run: a playback play in flight at full level.
fn running() -> TwrView {
    let config = golden_config();
    let (_, derived) = check(&config);
    let derived = derived.unwrap();
    let w = config::parse(&config).unwrap();
    let mut v = view::configured_view("twr-0003", &w, &derived, sim_station(), "g");
    v.state = State::Running;
    v.phase = "playing";
    v.grid = Grid {
        actual: true,
        ..v.grid
    };
    v.clock = Some("shared");
    v.level_db = Some(0.0);
    v.rung = Some(4);
    v.plays = Counts {
        done: 10,
        at_level: 1,
        of_level: 8,
        playback: 2,
        of_playback: 375,
    };
    v.t_s = Some(4.25);
    v.identification = Some(IdentFacts {
        excitation: "random",
        drive_vrms: 0.05,
        drive_vpk: 0.2297,
        coherence_mean: 0.9999971,
        min_mean_coherence: 0.8,
        coverage: Some(0.999267),
        lag_frames: Some(41.0),
        lag_source: "measured",
        passed: true,
    });
    v.prediction = Some(Prediction {
        drive_vpk: 0.25,
        drive_vpk_ceiling: 0.5,
        level_db: 0.0,
    });
    v.last_play = Some(row(
        9, true, 0.0, 4, None, 1.75, 4.5, 0.125, true, 0, 0.25, 0.2539, 46080, 3.6, None,
    ));
    v.feasibility = Some((
        "pass",
        "The section at full level is within its kinematic ceilings.".into(),
    ));
    v.streaming = true;
    v.play = Some(InFlight {
        index: 10,
        playback: true,
        iteration: None,
        frames_done: 2560,
        frames: 5120,
    });
    v
}

#[test]
fn the_running_state_is_the_golden_state_event() {
    let want = read_json("event_twr_state.json");
    assert_eq!(want["topic"], "twr.state");
    same(&view::state_json(Some(&running())), &want["data"], "state");
}

#[test]
fn the_plays_are_the_golden_plays() {
    let want = read_json("response_twr_plays.json");
    let got = json!({"plays": rows().iter().map(PlayRow::to_json).collect::<Vec<_>>(), "next": 3});
    same(&got, &want["result"], "plays");
}

const FS: f64 = 12_800.0;

fn scope() -> Scope {
    Scope {
        t0_s: 4.0,
        fs_hz: FS,
        samples: vec![0.0, 0.125, 0.25, 0.125],
    }
}

fn records() -> PlayRecords {
    PlayRecords {
        fs_hz: FS,
        target: vec![0.0, 0.25, 0.5, 0.25],
        response: vec![0.0, 0.25, 0.53125, 0.25],
        drive: vec![0.0, 0.125, 0.25, 0.125],
        alarm_frac: 0.25,
        abort_frac: 0.5,
        spectra: Some(Spectra {
            freqs_hz: vec![100.0, 200.0, 300.0],
            target: vec![0.5, 1.0, 0.75],
            response: vec![0.5, 1.0, 0.8125],
            drive: vec![0.25, 0.5, 0.375],
        }),
    }
}

fn ident() -> IdentFigures {
    IdentFigures {
        freqs_hz: vec![100.0, 200.0, 300.0],
        gain: vec![1.0, 0.5, 0.25],
        phase_deg: vec![-10.0, -20.0, -30.0],
        coherence: vec![0.99, 0.5, 0.98],
        coherence_floor: 0.9,
        min_mean_coherence: 0.8,
        unit: "g/V".into(),
    }
}

fn mask() -> MaskFigures {
    MaskFigures {
        freqs_hz: vec![100.0, 200.0, 300.0],
        trusted: vec![1.0, 0.0, 1.0],
        reference: vec![0.5, 1.0, 0.75],
    }
}

fn specs() -> Vec<LawValueSpec> {
    [
        ("corrected_bins", "Corrected bins", "bins"),
        ("clamped_bins", "Bins at the rise limit", "bins"),
        ("drive_change_db", "Drive change", "dB"),
    ]
    .iter()
    .map(|(k, l, u)| LawValueSpec {
        key: (*k).into(),
        label: (*l).into(),
        unit: (*u).into(),
    })
    .collect()
}

fn event_twr_live() -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let data = view::live_json(&running(), &scope(), &mut arrays);
    (protocol::event("twr.live", 181, data), arrays)
}

fn event_twr_data() -> (Value, Arrays) {
    let request = read_json("request_twr_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    let v = running();
    let rec = records();
    let id = ident();
    let m = mask();
    let rows = rows();
    let specs = specs();
    let s = scope();
    let ctx = DataContext {
        view: &v,
        last: Some(&rec),
        target: Some(&rec),
        identification: Some(&id),
        mask: Some(&m),
        rows: &rows,
        law_specs: &specs,
        scope: &s,
    };
    let mut arrays = Arrays::new();
    let data = subscribe::data_json(&ctx, &sub, &mut arrays);
    (protocol::event("twr.data", 182, data), arrays)
}

fn response_twr_reference_import() -> (Value, Arrays) {
    let tiny =
        "0\n0.25\n0.5\n0.25\n0\n-0.25\n-0.5\n-0.25\n0\n0.25\n0.5\n0.25\n0\n-0.25\n-0.5\n-0.25\n";
    let dir = scratch();
    let s = reference::import(
        &dir,
        &ImportRequest {
            source: Source::Text(tiny.into()),
            options: ImportOptions {
                fs_hz: Some(FS),
                column: 0,
                units: None,
            },
            name: None,
        },
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let mut arrays = Arrays::new();
    let result = reference::import_json(&s, &mut arrays);
    (protocol::response(82, result), arrays)
}

fn response_twr_reference_preview() -> (Value, Arrays) {
    let (_, derived) = check(&golden_config());
    let f = PreviewFigures {
        fs_hz: FS,
        units: "g".into(),
        target: vec![0.0, 0.25, 0.5, 0.25],
        freqs_hz: vec![0.0, 3200.0, 6400.0],
        spectrum: vec![0.0, 1.0, 0.5],
        band_hz: (20.0, 2000.0),
        floor: 0.003125,
        velocity: vec![0.0, 0.5, 1.5, 2.0],
        displacement: vec![0.0, 0.0625, 0.25, 0.5],
    };
    let mut arrays = Arrays::new();
    let views = reference::preview_views(&f, Some(subscribe::DEFAULT_POINTS), &mut arrays);
    let result = json!({"derived": derived.unwrap().preview.to_json(), "views": views});
    (protocol::response(83, result), arrays)
}

const BINARY: [&str; 4] = [
    "event_twr_live",
    "event_twr_data",
    "response_twr_reference_import",
    "response_twr_reference_preview",
];

fn build(name: &str) -> (Value, Arrays) {
    match name {
        "event_twr_live" => event_twr_live(),
        "event_twr_data" => event_twr_data(),
        "response_twr_reference_import" => response_twr_reference_import(),
        "response_twr_reference_preview" => response_twr_reference_preview(),
        other => panic!("no builder for {other}"),
    }
}

#[test]
fn the_binary_goldens_decode_to_their_expected_json_and_re_encode_byte_for_byte() {
    for name in BINARY {
        let bytes = read_bin(&format!("{name}.bin"));
        let decoded = codec::decode(&bytes).unwrap();
        same(
            &decoded.expanded(),
            &read_json(&format!("{name}.expected.json")),
            name,
        );
        let order = codec::header_array_order(&bytes).unwrap();
        let order: Vec<&str> = order.iter().map(String::as_str).collect();
        assert_eq!(codec::reencode(&decoded, &order).unwrap(), bytes, "{name}");
    }
}

#[test]
fn encoding_reproduces_the_golden_bytes() {
    for name in BINARY {
        let (message, arrays) = build(name);
        let got = codec::encode(&message, &arrays).unwrap();
        bytes_equal(name, &got, &read_bin(&format!("{name}.bin")));
    }
}

#[test]
fn every_twr_text_golden_is_a_compact_text_frame() {
    for name in [
        "request_twr_configure",
        "response_twr_configure",
        "event_twr_state",
        "request_twr_subscribe",
        "response_twr_subscribe",
        "response_twr_plays",
        "response_laws_list_twr",
        "response_laws_check_twr",
        "response_error_twr_config_invalid",
    ] {
        let text = std::fs::read_to_string(golden(&format!("{name}.json"))).unwrap();
        let message = codec::decode_text(&text).unwrap();
        match codec::encode_frame(&message, &Arrays::new()).unwrap() {
            Frame::Text(t) => same(&serde_json::from_str(&t).unwrap(), &message, name),
            Frame::Binary(_) => panic!("{name}: a message without arrays must be a text frame"),
        }
    }
}

#[test]
fn the_subscription_parses_to_the_golden_result() {
    let request = read_json("request_twr_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "twr.subscribe");
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    same(
        &subscribe::result_json(Some(&sub)),
        &read_json("response_twr_subscribe.json")["result"],
        "subscribe result",
    );
}

#[test]
fn the_twr_law_entry_is_the_golden_one() {
    let dir = scratch();
    let cat = hertzforge_random_laws::LawCatalog::open(&dir).unwrap();
    let list = sds_control::laws::list_json(&cat, Some(hertzforge_random_laws::LawWorld::Twr));
    let _ = std::fs::remove_dir_all(&dir);
    let want = read_json("response_laws_list_twr.json");
    same(&list, &want["result"], "laws.list twr");
    same(
        &want["result"]["laws"][0],
        &read_json("response_laws_check_twr.json")["result"]["law"],
        "the checked law's entry",
    );
}

/// A golden case, its detail in the words the golden gives it.
#[allow(clippy::too_many_arguments)]
fn measured_case(
    case: TwrCheckCase,
    plays: u32,
    rungs: &[(f64, u32)],
    full: f64,
    lag: usize,
    calls: u32,
    ms: f64,
    vpk: f64,
    clamped: u64,
    rises: Option<(f64, f64)>,
) -> Value {
    let m = TwrCaseMeasure {
        end: TwrCaseEnd::Completed,
        plays,
        rungs: rungs.iter().map(|(l, p)| (*l, Some(*p))).collect(),
        full_level_error_pct: Some(full),
        lag_frames: Some(lag),
        lag_measured: true,
        calls,
        misses: 0,
        refused: 0,
        clamped_bins: clamped,
        worst_peak_rise_db: rises.map(|r| r.0),
        worst_bin_rise_db: rises.map(|r| r.1),
        worst_call_ms: Some(ms),
        budget_ms: 1000.0,
        max_drive_vpk: vpk,
        drive_ceiling_vpk: 10.0,
    };
    let v = CheckCase {
        name: case.name().into(),
        passed: true,
        detail: format!("completed; {plays} plays; {calls} calls, slowest {ms} ms of 1000 ms"),
    };
    sds_control::twr::law_check::case_json(case, &m, &v)
}

#[test]
fn the_twr_check_report_is_the_golden_one() {
    use TwrCheckCase::*;
    let first = [(-12.0, 1), (-9.0, 1), (-6.0, 1), (-3.0, 1), (0.0, 1)];
    let three = [(-12.0, 3), (-9.0, 1), (-6.0, 1), (-3.0, 1), (0.0, 1)];
    let cases = vec![
        measured_case(Flat, 6, &first, 1.5, 4, 1, 2.5, 1.0, 0, None),
        measured_case(
            OneMode,
            9,
            &three,
            4.5,
            4,
            3,
            2.75,
            1.25,
            12,
            Some((3.5, 6.0)),
        ),
        measured_case(
            StationPlant,
            8,
            &[(-12.0, 2), (-9.0, 1), (-6.0, 1), (-3.0, 1), (0.0, 1)],
            5.25,
            4,
            2,
            2.5,
            2.5,
            0,
            Some((1.5, 4.0)),
        ),
        measured_case(
            Delay,
            9,
            &three,
            6.5,
            30,
            3,
            2.75,
            1.25,
            0,
            Some((3.5, 6.0)),
        ),
        measured_case(DeepQueue, 6, &first, 1.75, 41, 1, 2.5, 1.0, 0, None),
        measured_case(
            IdentificationError,
            9,
            &[(-12.0, 4), (-9.0, 1), (-6.0, 1), (-3.0, 1), (0.0, 1)],
            3.25,
            4,
            4,
            2.5,
            1.0,
            0,
            Some((2.5, 2.5)),
        ),
        measured_case(
            Noise,
            10,
            &[(-12.0, 3), (-9.0, 2), (-6.0, 1), (-3.0, 1), (0.0, 1)],
            7.5,
            4,
            4,
            2.75,
            1.25,
            0,
            Some((3.5, 6.0)),
        ),
        measured_case(AntiResonance, 6, &first, 8.25, 4, 1, 2.5, 1.5, 0, None),
    ];
    let want = read_json("response_laws_check_twr.json");
    same(
        &sds_control::twr::law_check::report_json(&cases, true, false),
        &want["result"]["report"],
        "report",
    );
}
