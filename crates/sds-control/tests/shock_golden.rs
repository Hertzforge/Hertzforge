//! The classical shock goldens in `schema/golden/`, built by this crate's own builders:
//! a configuration parses and checks to the golden derived values and state, the
//! binary frames encode to the golden bytes exactly, the text frames are the objects
//! the service sends, and the law catalog's classical entry and check report are the
//! golden ones.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_random_laws::CheckCase;
use hertzforge_shock_common::ShockRunMode;
use hertzforge_shock_laws::{ClassicalCaseEnd, ClassicalCaseMeasure, ClassicalCheckCase};
use sds_control::codec::{self, Arrays, Frame};
use sds_control::protocol;
use sds_control::shock::config::{self, CheckContext, Grid, LawFacts, QuietWindow};
use sds_control::shock::law_check;
use sds_control::shock::preview::{self, ReferenceFacts};
use sds_control::shock::subscribe::{self, DataContext};
use sds_control::shock::view::{
    self, Band, Counts, IdentFacts, IdentFigures, LawCall, LawReading, LawRef, Prediction,
    PulseFigures, Row, ShockView, SrsFigures, State, StationRef, TimeFigures,
};
use sds_control::table;
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

/// The built-in classical law's entry as the catalog lists it, from a catalog on
/// an empty data folder.
fn builtin_classical_entry() -> Value {
    let dir = std::env::temp_dir().join(format!(
        "sds-control-golden-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        // the clock alone repeats: macOS reads it to the microsecond, and two
        // tests running at once could share a folder and delete it under each other
        {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            NEXT.fetch_add(1, Ordering::Relaxed)
        }
    ));
    let cat = hertzforge_random_laws::LawCatalog::open(&dir).unwrap();
    let entry = sds_control::laws::entry_json(&cat.entry("fd_correct").unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    entry
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
        let at = got.iter().zip(want).position(|(a, b)| a != b);
        panic!(
            "{name}: {} bytes encoded vs {} golden; first difference at {at:?}\n got: {}\nwant: {}",
            got.len(),
            want.len(),
            String::from_utf8_lossy(&got[..got.len().min(400)]),
            String::from_utf8_lossy(&want[..want.len().min(400)]),
        );
    }
}

/// A control row (1), a current monitor (2) and a drive (3), as the golden
/// configuration names them.
fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c1 = table::new_row(1);
    c1.role = ChannelRole::Control;
    let mut c2 = table::new_row(2);
    c2.role = ChannelRole::Monitor;
    c2.units = Units::Custom("A".into());
    let mut c3 = table::new_row(3);
    c3.role = ChannelRole::Drive;
    c3.units = Units::VoltageV;
    t.channels = vec![c1, c2, c3];
    t
}

fn law_lookup(id: &str) -> Option<LawFacts> {
    (id == "fd_correct").then(sds_control::laws::builtin_classical_facts)
}

fn check(config: &Value) -> (sds_control::table::Report, Option<config::Derived>) {
    let w = config::parse(config).unwrap();
    let t = table();
    let ctx = CheckContext {
        table: &t,
        law: &law_lookup,
        timeout_s: 30.0,
    };
    w.check(&ctx)
}

fn sim_station() -> StationRef {
    StationRef {
        kind: "sim",
        class: "sim",
        devices: Vec::new(),
    }
}

fn law_ref() -> LawRef {
    LawRef {
        id: "fd_correct".into(),
        name: "frequency-domain correction".into(),
        version: "0.1.0".into(),
        sha256: String::new(),
    }
}

// --- the golden run's figures (schema/make_golden.py, "classical shock") -------------

const FS: f64 = 1000.0;
const START: i64 = -2;
const REF: [f64; 8] = [0.0, -0.5, 0.0, 7.5, 10.0, 7.5, 0.0, -0.5];
const BAND_START: usize = 1;
const BAND_HI: [f64; 6] = [1.5, 1.5, 9.0, 11.5, 9.0, 1.5];
const BAND_LO: [f64; 6] = [-1.5, -1.5, 6.0, 8.5, 6.0, -1.5];
const MEASURED: [f64; 8] = [0.0, -0.25, 0.25, 7.25, 9.75, 7.75, 0.25, -0.5];
const DRIVE: [f64; 8] = [0.0, -0.0125, 0.0, 0.1875, 0.25, 0.1875, 0.0, -0.0125];

fn time_figures(measured: bool) -> TimeFigures {
    TimeFigures {
        start: START,
        fs_hz: FS,
        reference: REF.to_vec(),
        band: Some(Band {
            start: BAND_START,
            hi: BAND_HI.to_vec(),
            lo: BAND_LO.to_vec(),
        }),
        measured: measured.then(|| MEASURED.to_vec()),
    }
}

fn pulse_figures() -> PulseFigures {
    PulseFigures {
        time: time_figures(true),
        drive: DRIVE.to_vec(),
        velocity: None,
        displacement: None,
        srs: Some(SrsFigures {
            freqs_hz: vec![100.0, 200.0, 400.0],
            reference: vec![12.5, 15.0, 11.0],
            band_hi: vec![17.5, 21.0, 15.5],
            band_lo: vec![9.0, 10.5, 7.75],
            measured: Some(vec![12.25, 15.5, 10.5]),
        }),
        spectrum: None,
    }
}

fn row() -> Row {
    Row {
        index: 2,
        origin: "ladder",
        scheduled_db: -6.0,
        commanded_db: -6.0,
        limit_scale: 1.0,
        drive_vpk: 0.2,
        peak_g: Some(9.98),
        peak_ratio: Some(0.998),
        error_pct: Some(1.9),
        velocity_change_ips: Some(27.02),
        velocity_change_ratio: Some(1.0014),
        verdicts: Some((true, true, true)),
        passed: Some(true),
        srs: Some((21, 21)),
        at_s: 9.75,
        law: Some(LawCall {
            used: true,
            miss: None,
            capped_bins: 0,
            peak_capped: false,
            call_ms: 1.4,
        }),
    }
}

/// The golden run between pulses: -6 dB in AUTO, three pulses passed.
fn running() -> ShockView {
    ShockView {
        run_id: "s-0003".into(),
        title: "Half-Sine 20 g 11 ms".into(),
        state: State::Running,
        phase: "waiting",
        station: sim_station(),
        grid: Grid {
            fs_hz: 51_200.0,
            record_len: 8192,
            actual: true,
        },
        clock: Some("shared"),
        mode: ShockRunMode::Auto,
        levels_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
        level_db: Some(-6.0),
        rung: Some(2),
        pulses: Counts {
            fired: 3,
            passed: 3,
            at_level: 1,
            of_level: 1,
        },
        next_pulse_s: Some(0.6),
        elapsed_s: Some(12.5),
        alarm: false,
        identification: Some(IdentFacts {
            drive_vrms: 0.05,
            drive_vpk: 0.21,
            coherence_mean: 0.998,
            min_mean_coherence: 0.8,
            passed: true,
        }),
        prediction: Some(Prediction {
            drive_vpk: 0.4,
            drive_vpk_ceiling: 0.5,
            drive_vpk_abort: 0.8,
            response_peak_g: 20.1,
            velocity_pk_ips: 27.3,
            displacement_pk_pk_in: 0.69,
        }),
        last_pulse: Some(row()),
        law: Some(law_ref()),
        misses: 0,
        feasibility: Some(("pass", "The pulse is within the shaker's ratings.".into())),
        end: None,
        fire: None,
        streaming: true,
        record: None,
    }
}

fn identification() -> IdentFigures {
    IdentFigures {
        freqs_hz: vec![100.0, 200.0, 300.0],
        gain: vec![0.5, 1.0, 0.75],
        phase_deg: vec![-10.0, -20.0, -30.0],
        coherence: vec![0.99, 0.5, 0.98],
        trusted: vec![true, false, true],
        floor: 0.9,
        unit: "g".into(),
    }
}

fn law_readings() -> Vec<LawReading> {
    vec![
        LawReading {
            label: "Corrected bins".into(),
            value: 412.0,
            unit: "bins".into(),
        },
        LawReading {
            label: "Bins at the rise limit".into(),
            value: 3.0,
            unit: "bins".into(),
        },
        LawReading {
            label: "Drive change".into(),
            value: 0.42,
            unit: "dB".into(),
        },
    ]
}

fn event_shock_live() -> (Value, Arrays) {
    let mut v = running();
    v.phase = "firing";
    v.next_pulse_s = None;
    v.fire = Some((2048, 8192));
    (
        protocol::event("shock.live", 139, view::live_json(&v)),
        Arrays::new(),
    )
}

fn event_shock_pulse() -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let views = view::default_pulse_views(&pulse_figures(), Some(1024), &mut arrays);
    let data = json!({"run_id": "s-0003", "pulse": row().to_json(), "views": views});
    (protocol::event("shock.pulse", 141, data), arrays)
}

fn event_shock_data() -> (Value, Arrays) {
    let request = read_json("request_shock_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    let v = running();
    let figures = pulse_figures();
    let ident = identification();
    let law = law_readings();
    let rows = vec![row()];
    let ctx = DataContext {
        view: &v,
        last: Some(&figures),
        identification: Some(&ident),
        law: &law,
        rows: &rows,
    };
    let mut arrays = Arrays::new();
    let data = subscribe::data_json(&ctx, &sub, &mut arrays);
    (protocol::event("shock.data", 142, data), arrays)
}

fn response_shock_preview() -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let facts = ReferenceFacts {
        peak_g: 10.0,
        velocity_change_ips: 9.6522,
        displacement_pk_pk_in: 0.0123,
        main_start: 2,
        main_len: 3,
    };
    let quiet = QuietWindow {
        worst_frac: 0.05,
        limit_frac: 0.15,
    };
    let result = preview::preview_frame(
        &facts,
        &quiet,
        &time_figures(false),
        &[0.0, -0.096522, -0.193044, 1.25, 4.6, 7.9, 9.65, 9.6522],
        &[0.0, 0.0, -0.0001, -0.0002, 0.0026, 0.0089, 0.0123, 0.0122],
        Some(1024),
        &mut arrays,
    );
    (protocol::response(53, result), arrays)
}

const BINARY: [&str; 4] = [
    "event_shock_live",
    "event_shock_pulse",
    "event_shock_data",
    "response_shock_preview",
];

fn build(name: &str) -> (Value, Arrays) {
    match name {
        "event_shock_live" => event_shock_live(),
        "event_shock_pulse" => event_shock_pulse(),
        "event_shock_data" => event_shock_data(),
        "response_shock_preview" => response_shock_preview(),
        other => panic!("no builder for {other}"),
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
fn decoding_matches_the_expected_json_and_re_encodes_byte_for_byte() {
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
fn the_configuration_checks_to_the_golden_derived_values_and_state() {
    let request = read_json("request_shock_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "shock.configure");
    let config = &parsed.params["config"];
    let (report, derived) = check(config);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let derived = derived.unwrap();
    let want = read_json("response_shock_configure.json");
    same(&derived.to_json(), &want["result"]["derived"], "derived");
    same(
        &Value::Array(report.warnings.iter().map(|i| i.to_json()).collect()),
        &want["result"]["warnings"],
        "warnings",
    );
    let w = config::parse(config).unwrap();
    let v = view::configured_view("s-0001", &w, &derived, sim_station(), law_ref());
    same(
        &view::state_json(Some(&v)),
        &want["result"]["state"],
        "state",
    );
}

#[test]
fn the_running_state_is_the_golden_state_event() {
    let want = read_json("event_shock_state.json");
    assert_eq!(want["topic"], "shock.state");
    same(&view::state_json(Some(&running())), &want["data"], "state");
}

#[test]
fn the_subscription_parses_to_the_golden_result() {
    let request = read_json("request_shock_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "shock.subscribe");
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    same(
        &subscribe::result_json(Some(&sub)),
        &read_json("response_shock_subscribe.json")["result"],
        "subscribe result",
    );
}

#[test]
fn the_fire_and_level_requests_parse() {
    let fire = protocol::parse_request(&read_json("request_shock_fire.json")).unwrap();
    assert_eq!((fire.method.as_str(), fire.params.len()), ("shock.fire", 0));
    let level = protocol::parse_request(&read_json("request_shock_level.json")).unwrap();
    assert_eq!(level.method, "shock.level");
    assert_eq!(level.params["rung"], json!(4));
}

#[test]
fn the_classical_law_entry_is_the_golden_one() {
    let want = read_json("response_laws_list_shock_classical.json");
    same(
        &builtin_classical_entry(),
        &want["result"]["laws"][0],
        "entry",
    );
    assert_eq!(want["result"]["problems"], json!([]));
}

/// A golden case, its detail in the words the golden gives it.
#[allow(clippy::too_many_arguments)]
fn measured_case(
    case: ClassicalCheckCase,
    pulses: u32,
    first: u32,
    full: u32,
    calls: u32,
    ms: f64,
    vpk: f64,
    capped: u64,
) -> Value {
    let m = ClassicalCaseMeasure {
        end: ClassicalCaseEnd::Completed,
        pulses,
        first_pass: Some((first, 0)),
        pulses_passed: pulses,
        full_level_pulses: full,
        full_level_passed: full,
        calls,
        misses: 0,
        refused: 0,
        capped_bins: capped,
        capped_peaks: 0,
        worst_call_ms: Some(ms),
        budget_ms: 250.0,
        max_drive_vpk: vpk,
        drive_ceiling_vpk: 0.5,
    };
    let v = CheckCase {
        name: case.name().into(),
        passed: true,
        detail: format!(
            "completed; {pulses} pulses, the first pass at pulse {first}; {full} of {full} \
             full-level pulses passed; {calls} calls, slowest {ms} ms of 250 ms"
        ),
    };
    law_check::case_json(case, &m, &v)
}

#[test]
fn the_classical_check_report_is_the_golden_one() {
    use ClassicalCheckCase::*;
    let cases = vec![
        measured_case(Flat, 7, 1, 3, 8, 1.1, 0.4, 0),
        measured_case(OneMode, 8, 2, 3, 9, 1.2, 0.41, 0),
        measured_case(StationPlant, 8, 2, 3, 9, 1.2, 0.47, 0),
        measured_case(Delay, 9, 3, 3, 10, 1.3, 0.42, 0),
        measured_case(IdentificationError, 8, 2, 3, 9, 1.1, 0.4, 0),
        measured_case(Noise, 9, 3, 3, 10, 1.2, 0.41, 0),
        measured_case(AntiResonance, 7, 1, 3, 8, 1.2, 0.44, 12),
    ];
    let want = read_json("response_laws_check_shock_classical.json");
    same(
        &law_check::report_json(&cases, true, false),
        &want["result"]["report"],
        "report",
    );
    same(&builtin_classical_entry(), &want["result"]["law"], "law");
}

#[test]
fn a_configuration_with_errors_is_refused_as_the_golden_error() {
    let mut config = read_json("request_shock_configure.json")["params"]["config"].clone();
    config["limits"]["drive_vpk_abort"] = json!(0.4);
    config["train"]["spacing_s"] = json!(0.5);
    config["pulse"]["compensation"] =
        json!({"shape": "half_sine", "pre_frac": 1.5, "post_frac": 1.5, "symmetric": true});
    let (report, derived) = check(&config);
    assert!(derived.is_none());
    let e = config::config_invalid(&report);
    let want = read_json("response_error_shock_config_invalid.json");
    same(&protocol::error_response(58, &e), &want, "error");
}

#[test]
fn every_shock_text_golden_is_a_compact_text_frame() {
    for name in [
        "request_shock_configure",
        "response_shock_configure",
        "event_shock_state",
        "request_shock_subscribe",
        "response_shock_subscribe",
        "request_shock_fire",
        "request_shock_level",
        "response_laws_list_shock_classical",
        "response_laws_check_shock_classical",
        "response_error_shock_config_invalid",
    ] {
        let text = std::fs::read_to_string(golden(&format!("{name}.json"))).unwrap();
        let message = codec::decode_text(&text).unwrap();
        match codec::encode_frame(&message, &Arrays::new()).unwrap() {
            Frame::Text(t) => same(&serde_json::from_str(&t).unwrap(), &message, name),
            Frame::Binary(_) => panic!("{name}: a message without arrays must be a text frame"),
        }
    }
}
