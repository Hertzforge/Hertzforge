//! The Sine goldens in `schema/golden/`, built by this crate's own builders: the
//! configuration parses and checks to the golden derived values, a configuration
//! with errors is refused as the golden error, the import reads to the golden
//! profile, the catalog's Sine entry is the golden one, the binary frames decode and
//! re-encode byte for byte, and the text frames are compact text frames.

use hertzforge_channel_table::{ChannelRole, ChannelTable, Units};
use hertzforge_random_laws::CheckCase;
use hertzforge_sine::{RunMode, SweepDirection};
use hertzforge_sine_laws::{SineCaseEnd, SineCaseMeasure, SineCheckCase};
use sds_control::codec::{self, Arrays, Frame};
use sds_control::sine::config::{self, CheckContext, Grid, LawRef};
use sds_control::sine::profile::{self as sprofile, Crossover, Kinematics, Peaks, ProfileFigures};
use sds_control::sine::subscribe;
use sds_control::sine::view::{
    self, Alarm, Figures, IdentFacts, IdentFigures, InputCurves, LiveChannel, Rate, SineView,
    State, StationRef,
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

/// A control row (1), a response (2), the fixture's accelerometer (3, the limit
/// channel) and a drive (4), as the golden configuration names them.
fn table() -> ChannelTable {
    let mut t = table::Table::new().table;
    let mut c1 = table::new_row(1);
    c1.role = ChannelRole::Control;
    c1.description = "Control".into();
    let mut c2 = table::new_row(2);
    c2.role = ChannelRole::Monitor;
    c2.description = "Response".into();
    let mut c3 = table::new_row(3);
    c3.role = ChannelRole::Monitor;
    c3.description = "Fixture".into();
    let mut c4 = table::new_row(4);
    c4.role = ChannelRole::Drive;
    c4.units = Units::VoltageV;
    t.channels = vec![c1, c2, c3, c4];
    t
}

fn check(config: &Value) -> (table::Report, Option<config::Derived>) {
    let w = config::parse(config).unwrap();
    let t = table();
    let law = |id: &str| sds_control::laws::sine_lookup(None, id);
    let ctx = CheckContext {
        table: &t,
        law: &law,
        timeout_s: 30.0,
        station_outputs: 1,
    };
    w.check(&ctx)
}

fn golden_config() -> Value {
    let request = read_json("request_sine_configure.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "sine.configure");
    parsed.params["config"].clone()
}

#[test]
fn the_configuration_checks_to_the_golden_derived_values() {
    let (report, derived) = check(&golden_config());
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let want = read_json("response_sine_configure.json");
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
    config["limits"]["drive_vpk_abort"] = json!(0.4);
    config["limits"]["channels"][0]["channel"] = json!(1);
    config["acquisition"]["tick_ms"] = json!(1);
    let (report, derived) = check(&config);
    assert!(derived.is_none());
    let e = config::config_invalid(&report);
    let want = read_json("response_error_sine_config_invalid.json");
    same(&protocol::error_response(79, &e), &want, "error");
}

#[test]
fn the_import_reads_the_golden_rows_to_the_golden_profile() {
    let text = "# the golden profile\nfreq_hz,displacement (in pk-pk),accel (g),alarm,abort\n\
                20,0.0097,,,\n45,,1,1,2\n2000,,1,,\n";
    let want = read_json("response_sine_profile_import.json");
    let got = sds_control::sine::profile::import_text(text, "text");
    same(&got, &want["result"], "import");
    // The file at a path reads the same, and a relative path is refused.
    let dir = std::env::temp_dir().join(format!("sds-sine-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("profile.csv");
    std::fs::write(&file, text).unwrap();
    let params = json!({"path": file.to_str().unwrap()});
    let got = sds_control::sine::profile::import(params.as_object().unwrap()).unwrap();
    same(&got, &want["result"], "import by path");
    let _ = std::fs::remove_dir_all(dir);
    let relative = json!({"path": "profile.csv"});
    let e = sds_control::sine::profile::import(relative.as_object().unwrap()).unwrap_err();
    assert_eq!(e.reason, Some("bad_params"));
}

#[test]
fn the_sine_law_entry_is_the_golden_one() {
    let dir = std::env::temp_dir().join(format!(
        "sds-control-sine-golden-{}-{}-{}",
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
    let list = sds_control::laws::list_json(&cat, Some(hertzforge_random_laws::LawWorld::Sine));
    let _ = std::fs::remove_dir_all(&dir);
    let want = read_json("response_laws_list_sine.json");
    same(&list, &want["result"], "laws.list sine");
    same(
        &want["result"]["laws"][0],
        &read_json("response_laws_check_sine.json")["result"]["law"],
        "the checked law's entry",
    );
}

const BINARY: [&str; 3] = [
    "event_sine_live",
    "event_sine_data",
    "response_sine_profile_validate",
];

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
fn every_sine_text_golden_is_a_compact_text_frame() {
    for name in [
        "request_sine_configure",
        "response_sine_configure",
        "event_sine_state",
        "request_sine_subscribe",
        "response_sine_subscribe",
        "request_sine_level",
        "request_sine_adjust",
        "response_sine_profile_import",
        "response_laws_list_sine",
        "response_laws_check_sine",
        "response_error_sine_config_invalid",
    ] {
        let text = std::fs::read_to_string(golden(&format!("{name}.json"))).unwrap();
        let message = codec::decode_text(&text).unwrap();
        match codec::encode_frame(&message, &Arrays::new()).unwrap() {
            Frame::Text(t) => same(&serde_json::from_str(&t).unwrap(), &message, name),
            Frame::Binary(_) => panic!("{name}: a message without arrays must be a text frame"),
        }
    }
}

fn bytes_equal(name: &str, got: &[u8], want: &[u8]) {
    if got != want {
        let at = got.iter().zip(want).position(|(a, b)| a != b);
        panic!(
            "{name}: {} bytes encoded vs {} golden; first difference at {at:?}\n got: {}\nwant: {}",
            got.len(),
            want.len(),
            String::from_utf8_lossy(&got[..got.len().min(600)]),
            String::from_utf8_lossy(&want[..want.len().min(600)]),
        );
    }
}

fn sim_station() -> StationRef {
    StationRef {
        kind: "sim",
        class: "sim",
        devices: Vec::new(),
    }
}

fn servo_blend() -> LawRef {
    LawRef {
        id: "servo_blend".into(),
        name: "Servo blend".into(),
        version: "0.1.0".into(),
        sha256: String::new(),
        live: vec!["blend_w".into()],
    }
}

fn law_labels() -> Vec<(String, String, String)> {
    sds_control::laws::sine_builtin_manifest()
        .diagnostics
        .iter()
        .map(|d| (d.key.clone(), d.label.clone(), d.unit.clone()))
        .collect()
}

#[test]
fn the_configured_state_is_the_golden_one() {
    let config = golden_config();
    let (_, derived) = check(&config);
    let derived = derived.unwrap();
    let w = config::parse(&config).unwrap();
    let law = view::law_state(servo_blend(), vec![("blend_w".into(), 0.5)], law_labels());
    let v = view::configured_view("sine-0001", &w, derived.grid, sim_station(), Some(law));
    let want = read_json("response_sine_configure.json");
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

// --- the golden run's figures (schema/make_golden.py, "Sine") ------------------------

const F: [f64; 4] = [500.0, 750.0, 1000.0, 1250.0];
const NAN: f64 = f64::NAN;

/// The golden run: a down sweep at -6 dB, three enabled inputs.
fn running() -> SineView {
    let config = golden_config();
    let w = config::parse(&config).unwrap();
    let mut v = view::configured_view(
        "sine-0003",
        &w,
        Grid {
            fs_hz: 16384.0,
            tick_frames: 164,
            actual: true,
        },
        sim_station(),
        None,
    );
    v.state = State::Running;
    v.phase = "engaged";
    v.clock = Some("shared");
    v.level_db = Some(-6.0);
    v.rung = Some(1);
    v.frequency_hz = Some(970.25);
    v.direction = Some(SweepDirection::Down);
    v.sweeps = Some((0, Some(1)));
    v.elapsed_s = Some(230.5);
    v.at_level_s = Some(31.25);
    v.remaining_s = Some(566.75);
    v.identification = Some(IdentFacts {
        level_db: -20.0,
        band_coherence: 0.98,
        gate_coherence: 0.5,
        lag_frames: 0,
        trusted_frac: 0.97,
        passed: true,
    });
    let mut law = view::law_state(servo_blend(), vec![("blend_w".into(), 0.5)], law_labels());
    law.built = true;
    law.capped_ticks = 2;
    law.values = vec![
        ("servo_step_db".into(), 0.015625),
        ("prediction_step_db".into(), 0.0078125),
        ("total_step_db".into(), 0.0234375),
        ("servo_level_db".into(), -12.5),
    ];
    v.law = Some(law);
    v.feasibility = Some(("pass", "The profile is within the shaker's ratings.".into()));
    v.t_s = Some(236.75);
    v.rate = Rate::OctPerMin(-2.0);
    v.achieved_db = Some(-6.0);
    v.target = Some(0.5);
    v.measured = Some(0.5);
    v.drive_vpk = Some(0.125);
    v.streaming = true;
    let ch = |index, description: &str, role, amplitude, phase_deg, rms| LiveChannel {
        index,
        description: description.into(),
        role,
        amplitude: Some(amplitude),
        phase_deg: Some(phase_deg),
        rms: Some(rms),
        unit: "g".into(),
        overload: false,
    };
    v.channels = vec![
        ch(1, "Control", "control", 0.5, -12.5, 0.375),
        ch(2, "Response", "monitor", 0.75, -30.25, 0.5625),
        ch(3, "Fixture", "monitor", 1.25, -95.5, 0.875),
    ];
    v.alarms = Vec::<Alarm>::new();
    v.figures = Some(Arc::new(figures()));
    v
}

fn figures() -> Figures {
    let visited = vec![false, false, true, true];
    let on = |a: f64, b: f64| vec![NAN, NAN, a, b];
    Figures {
        against_time: false,
        x: F.to_vec(),
        visited,
        reference: vec![0.5; 4],
        alarm_lo: vec![0.375; 4],
        alarm_hi: vec![0.625; 4],
        abort_lo: vec![0.25; 4],
        abort_hi: vec![0.75; 4],
        measured: on(0.5, 0.515625),
        drive: on(0.125, 0.140625),
        cola: None,
        thd: on(0.005, 0.00625),
        inputs: vec![
            InputCurves {
                index: 1,
                tracked: on(0.5, 0.515625),
                phase_deg: on(-12.5, -14.0),
                limit: None,
            },
            InputCurves {
                index: 2,
                tracked: on(0.75, 0.875),
                phase_deg: on(-30.25, -31.0),
                limit: None,
            },
            InputCurves {
                index: 3,
                tracked: on(1.25, 1.5),
                phase_deg: on(-95.5, -101.25),
                limit: Some(vec![1.5; 4]),
            },
        ],
        time: vec![(1, vec![0.0, 0.25, 0.5, 0.25])],
        time_fs_hz: 16384.0,
        identification: Some(IdentFigures {
            freqs_hz: vec![100.0, 200.0, 300.0],
            gain: vec![0.5, 1.0, 0.75],
            coherence: vec![0.99, 0.25, 0.98],
            trusted: vec![true, false, true],
            floor: 0.05,
            trust_coherence: 0.5,
            unit: "g".into(),
        }),
    }
}

#[test]
fn the_running_state_is_the_golden_state_event() {
    let want = read_json("event_sine_state.json");
    assert_eq!(want["topic"], "sine.state");
    same(&view::state_json(Some(&running())), &want["data"], "state");
}

fn event_sine_live() -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let data = view::live_json(&running(), &mut arrays);
    (protocol::event("sine.live", 171, data), arrays)
}

fn event_sine_data() -> (Value, Arrays) {
    let request = read_json("request_sine_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    let mut arrays = Arrays::new();
    let data = subscribe::data_json(&running(), &sub, &mut arrays);
    (protocol::event("sine.data", 172, data), arrays)
}

fn response_sine_profile_validate() -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let fig = ProfileFigures {
        freqs_hz: vec![50.0, 100.0, 200.0],
        target: vec![1.0; 3],
        alarm_hi: vec![1.1875; 3],
        alarm_lo: vec![0.84375; 3],
        abort_hi: vec![1.4140625; 3],
        abort_lo: vec![0.70703125; 3],
        velocity: vec![1.25, 0.625, 0.3125],
        displacement: vec![0.0078125, 0.001953125, 0.00048828125],
    };
    let k = Kinematics {
        peaks: Peaks {
            accel_g: 1.0,
            velocity_ips: 1.25,
            disp_pp_in: 0.0078,
        },
        verdict: "pass",
        message: "The profile is within the shaker's ratings.".into(),
    };
    let none: [Crossover; 0] = [];
    let result = sprofile::validate_frame(
        &table::Report::default(),
        &none,
        Some(&k),
        Some(&fig),
        &mut arrays,
    );
    (protocol::response(78, result), arrays)
}

fn build(name: &str) -> (Value, Arrays) {
    match name {
        "event_sine_live" => event_sine_live(),
        "event_sine_data" => event_sine_data(),
        "response_sine_profile_validate" => response_sine_profile_validate(),
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
fn the_subscription_parses_to_the_golden_result_and_the_verbs_parse() {
    let request = read_json("request_sine_subscribe.json");
    let parsed = protocol::parse_request(&request).unwrap();
    assert_eq!(parsed.method, "sine.subscribe");
    let sub = subscribe::parse(&parsed.params).unwrap().unwrap();
    same(
        &subscribe::result_json(Some(&sub)),
        &read_json("response_sine_subscribe.json")["result"],
        "subscribe result",
    );
    let level = protocol::parse_request(&read_json("request_sine_level.json")).unwrap();
    assert_eq!(level.method, "sine.level");
    assert_eq!(level.params["delta_db"], json!(-1));
    let adjust = protocol::parse_request(&read_json("request_sine_adjust.json")).unwrap();
    assert_eq!(adjust.method, "sine.adjust");
    assert_eq!(adjust.params["key"], "blend_w");
    let _ = RunMode::Auto;
}

/// A golden case, its detail in the words the golden gives it.
#[allow(clippy::too_many_arguments)]
fn measured_case(
    case: SineCheckCase,
    ticks: u64,
    judged: u64,
    worst: f64,
    worst_hz: f64,
    away: Option<f64>,
    rungs: Vec<(f64, Option<f64>)>,
    limit_over: Option<f64>,
    overshoot: Option<f64>,
    capped: u64,
    vpk: f64,
) -> Value {
    let m = SineCaseMeasure {
        end: SineCaseEnd::Completed,
        ticks,
        judged_ticks: judged,
        worst_error_db: Some(worst),
        worst_error_hz: Some(worst_hz),
        worst_away_error_db: away,
        rungs,
        worst_limit_over_db: limit_over,
        worst_release_overshoot_db: overshoot,
        calls: ticks,
        misses: 0,
        refused: 0,
        capped_ticks: capped,
        worst_call_ms: Some(0.01),
        budget_ms: 2.5,
        max_drive_vpk: vpk,
        drive_ceiling_vpk: 10.0,
        worst_rise_db: 0.5,
    };
    let v = CheckCase {
        name: case.name().into(),
        passed: true,
        detail: format!(
            "completed; {judged} of {ticks} ticks judged, worst {worst} dB; {ticks} calls, \
             slowest 0.01 ms of 2.5 ms"
        ),
    };
    sds_control::sine::law_check::case_json(case, &m, &v)
}

#[test]
fn the_sine_check_report_is_the_golden_one() {
    use SineCheckCase::*;
    let cases = vec![
        measured_case(
            Slow,
            18000,
            17400,
            0.42,
            251.25,
            None,
            vec![],
            None,
            None,
            0,
            0.41,
        ),
        measured_case(
            Fast,
            3000,
            2900,
            3.5,
            249.5,
            Some(0.75),
            vec![],
            None,
            None,
            0,
            0.43,
        ),
        measured_case(
            CloseModes,
            9000,
            8700,
            1.25,
            260.5,
            None,
            vec![],
            None,
            None,
            0,
            0.39,
        ),
        measured_case(
            AntiResonance,
            9000,
            8700,
            2.5,
            300.25,
            None,
            vec![],
            None,
            None,
            12,
            4.5,
        ),
        measured_case(
            Noise,
            3000,
            2900,
            0.5,
            250.0,
            None,
            vec![],
            None,
            None,
            0,
            0.02,
        ),
        measured_case(
            Ladder,
            4000,
            3600,
            0.25,
            250.0,
            None,
            vec![
                (-20.0, Some(0.25)),
                (-12.0, Some(0.125)),
                (-6.0, Some(0.125)),
                (0.0, Some(0.25)),
            ],
            None,
            None,
            0,
            0.02,
        ),
        measured_case(
            Notch,
            9000,
            8700,
            0.75,
            300.5,
            None,
            vec![],
            Some(0.25),
            Some(0.5),
            4,
            0.39,
        ),
        measured_case(
            Delay,
            600,
            400,
            0.125,
            100.0,
            None,
            vec![(-6.0, Some(0.125))],
            None,
            None,
            0,
            0.53,
        ),
    ];
    let want = read_json("response_laws_check_sine.json");
    same(
        &sds_control::sine::law_check::report_json(&cases, true, false),
        &want["result"]["report"],
        "report",
    );
}
