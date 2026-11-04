//! The check of a multiple-output law before hardware, run for real: every
//! built-in law passes on every simulated test article; the zero law and the
//! misbehaving fixture fail with their reasons named; a pass recorded against a
//! law file's hash clears exactly that file for hardware, and a failure keeps it
//! on the simulator.
//!
//! These runs are long in a debug build (the simulator, the estimator and the
//! linear algebra are unoptimized); `cargo test --release -p sds-control --test
//! law_check` is the way to run them. In a debug build the law's time budget is
//! relaxed to the whole loop, as the service's own convergence tests do.

mod support;

use hertzforge_law_host::{Param, ParamValue};
use hertzforge_mimo_random::law::builtin_factory;
use hertzforge_random_laws::{
    may_arm_on_hardware, mimo_check_record, ArmRefusal, BuiltinMimo, CheckStatus, LawCatalog,
    LawChoice, MimoCheckCase,
};
use sds_control::mimo::law_check::{run_check, CaseResult, CheckOptions};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use support::schema::Schema;
use support::{options, serve, Client};

fn opts() -> CheckOptions {
    let mut o = CheckOptions::default();
    if cfg!(debug_assertions) {
        o.budget_fraction = 1.0;
    }
    o
}

fn print(law: &str, rs: &[CaseResult]) {
    for r in rs {
        eprintln!(
            "CHECK {law} | {} | {} | conv {:?} | settle {:.2} | final {:.3} dB | worst line {:.3} dB | trip {:?} \
             | misses {} rejected {} | repaired {} capped {} | calls {} | slowest {:.1} ms of \
             {:.0} ms | gate {} | {}",
            r.case.name(),
            if r.verdict.passed { "PASS" } else { "FAIL" },
            r.measure.loops_to_converge,
            r.measure.settle_db,
            r.measure.final_error_db,
            r.measure.worst_line_db,
            r.measure.trip,
            r.measure.misses.len(),
            r.measure.rejected,
            r.measure.repaired_lines,
            r.measure.capped_lines,
            r.measure.calls,
            r.measure.worst_call_ms,
            r.measure.budget_ms,
            r.measure.gate_passed,
            r.verdict.detail
        );
    }
}

fn check_builtin(b: BuiltinMimo) -> Vec<CaseResult> {
    let law = move |controls: usize| {
        builtin_factory(&LawChoice::new(b.id(), Vec::new()), controls).map_err(|e| e.to_string())
    };
    run_check(&law, &opts(), &mut |_, _, _| {})
}

/// The law file `<name>.wasm` built by scripts/build_laws.sh.
fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|d| d.join("wasm32-wasip2").join("release").join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{file} is not built; run `bash scripts/build_laws.sh`"))
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sds-law-check-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Import `file` into a fresh catalog, check it with `params`, record the result,
/// and hand back the catalog and the results.
fn check_file(file: &str, params: Vec<Param>, tag: &str) -> (LawCatalog, String, Vec<CaseResult>) {
    let root = scratch(tag);
    let mut cat = LawCatalog::open(&root).unwrap();
    let entry = cat.import(&law_file(file)).unwrap();
    let id = entry.id.clone();
    let rs = {
        let cat = &cat;
        let law = |controls: usize| {
            cat.resolve_mimo(&LawChoice::new(id.clone(), params.clone()), controls)
                .map_err(|e| e.to_string())
        };
        run_check(&law, &opts(), &mut |_, _, _| {})
    };
    let record = mimo_check_record(
        id.clone(),
        "2026-11-03T12:00:00Z",
        rs.iter().map(|r| r.verdict.clone()).collect(),
    );
    cat.record_check(&record).unwrap();
    (cat, id, rs)
}

#[test]
fn every_built_in_law_passes_the_check() {
    let mut failed = Vec::new();
    for b in BuiltinMimo::ALL {
        let rs = check_builtin(b);
        print(b.id(), &rs);
        assert_eq!(rs.len(), MimoCheckCase::ALL.len());
        failed.extend(
            rs.iter()
                .filter(|r| !r.verdict.passed)
                .map(|r| format!("{}: {}: {}", b.id(), r.case.name(), r.verdict.detail)),
        );
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn a_passing_law_file_is_recorded_by_its_hash_and_cleared_for_hardware() {
    let (cat, id, rs) = check_file("hertzforge_law_mimo_open_loop", Vec::new(), "open");
    print("pinv_open (law file)", &rs);
    assert!(rs.iter().all(|r| r.verdict.passed));
    let entry = cat.entry(&id).unwrap();
    assert!(matches!(&entry.check, CheckStatus::Checked(r) if r.passed));
    may_arm_on_hardware(&entry).expect("a passed law file arms on hardware");
}

#[test]
fn the_zero_law_fails_every_case_with_its_reasons_and_stays_off_hardware() {
    let (cat, id, rs) = check_file("hertzforge_law_mimo_zero", Vec::new(), "zero");
    print("mimo-zero", &rs);
    for r in &rs {
        assert!(!r.verdict.passed, "{}: {}", r.case.name(), r.verdict.detail);
        assert!(
            r.verdict.detail.contains("ended early")
                || r.verdict.detail.contains("not every control"),
            "{}: {}",
            r.case.name(),
            r.verdict.detail
        );
    }
    match may_arm_on_hardware(&cat.entry(&id).unwrap()) {
        Err(ArmRefusal::CheckFailed { failed_cases, .. }) => {
            assert_eq!(failed_cases.len(), MimoCheckCase::ALL.len());
            assert_eq!(failed_cases[0], "modal 4 x 4");
        }
        other => panic!("expected a failed check, got {other:?}"),
    }
}

#[test]
fn the_misbehaving_fixture_fails_with_the_outputs_that_were_refused() {
    let nan = vec![Param {
        key: "mode".into(),
        value: ParamValue::Choice("nan".into()),
    }];
    let (cat, id, rs) = check_file("law_fixture_mimo_misbehave", nan, "misbehave");
    print("misbehave nan", &rs);
    // The fixture's seed is a flat uncorrelated drive, so on most plants the core's
    // envelope ends the run before the first loop call; where a loop call is made,
    // its NaN drive is refused at the law host's boundary three times and the run
    // takes its law-fault stop. Every case fails, each with its reason.
    let mut refused = 0;
    for r in &rs {
        assert!(!r.verdict.passed, "{}", r.case.name());
        if r.measure.calls > 0 {
            assert!(
                r.measure.rejected >= 3,
                "{}: {:?}",
                r.case.name(),
                r.measure
            );
            assert!(
                r.verdict.detail.contains("refused at a check")
                    && r.verdict.detail.contains("not a finite number"),
                "{}: {}",
                r.case.name(),
                r.verdict.detail
            );
            refused += 1;
        } else {
            assert!(
                r.verdict.detail.contains("abort lines"),
                "{}",
                r.verdict.detail
            );
        }
    }
    assert!(refused >= 1, "no case reached a loop call");
    assert!(matches!(
        may_arm_on_hardware(&cat.entry(&id).unwrap()),
        Err(ArmRefusal::CheckFailed { .. })
    ));
}

fn no_driver() -> sds_control::station::FixedScanner {
    sds_control::station::FixedScanner(Err(
        hertzforge_station::StationBuildError::NiDriverUnavailable("not on this test host".into()),
    ))
}

async fn wire() -> (std::net::SocketAddr, Client) {
    let svc = sds_control::server::Service::with_options(options(
        no_driver(),
        5.0,
        Duration::from_secs(60),
        Duration::from_secs(10),
    ));
    let addr = serve(svc).await;
    let c = Client::connect(addr).await;
    (addr, c)
}

/// The refusals, all before anything runs: no law named, both named, an unknown id, a
/// single-output law, parameters the law does not accept, and an observer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn laws_check_refuses_what_it_cannot_check() {
    let schema = Schema::load();
    let (addr, mut c) = wire().await;
    let mut obs = Client::connect(addr).await;
    let e = obs.refused("laws.check", json!({"id": "pinv_open"})).await;
    assert_eq!(e["reason"], "not_controller");
    c.ok("session.claim", json!({"name": "Shell"})).await;
    for (params, reason) in [
        (json!({}), "bad_params"),
        (
            json!({"id": "pinv_open", "path": "/x/y.wasm"}),
            "bad_params",
        ),
        (json!({"id": "pinv_open", "params": [1]}), "bad_params"),
        (json!({"id": "f".repeat(64)}), "law_unknown"),
        (json!({"id": "hinv"}), "law_check_unsupported"),
        (
            json!({"id": "pinv_open", "params": {"regularization": "sideways"}}),
            "law_params_invalid",
        ),
        (json!({"path": "law.wasm"}), "bad_params"),
    ] {
        let r = c.call("laws.check", params.clone()).await;
        schema.assert_valid(&r, None);
        assert_eq!(r["error"]["reason"], reason, "{params}: {r}");
    }
    assert!(c.of_topic("laws.check").is_empty(), "nothing ran");
}

/// A whole check through the protocol, by path: one event as each case starts and one
/// as it ends, the report, and the law's entry now cleared for hardware. Release only
/// (the 60 s answer deadline of the test client is not enough for a debug build).
#[cfg_attr(debug_assertions, ignore = "run with --release")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn laws_check_runs_a_law_file_and_clears_it_for_hardware() {
    let schema = Schema::load();
    let (_, mut c) = wire().await;
    c.ok("session.claim", json!({"name": "Shell"})).await;
    let path = law_file("hertzforge_law_mimo_open_loop");
    let req = json!({"path": path.to_str().unwrap(), "params": {"regularization_amount": 0.001}});
    let r = c.ok("laws.check", req).await;
    schema.assert_valid(&r, Some("lawsCheckResult"));
    assert_eq!(r["report"]["passed"], true, "{}", r["report"]);
    assert_eq!(r["report"]["recorded"], true);
    assert_eq!(r["report"]["cases"].as_array().unwrap().len(), 6);
    assert_eq!(r["law"]["check"]["status"], "passed");
    assert_eq!(r["law"]["check"]["hardware"], true);
    assert_eq!(r["law"]["check"]["tool_version"], "mimo-check 1");
    let events: Vec<Value> = c
        .of_topic("laws.check")
        .iter()
        .map(|e| e.data().clone())
        .collect();
    assert_eq!(events.len(), 12, "{events:#?}");
    for (k, e) in events.iter().enumerate() {
        schema.assert_valid(e, Some("lawsCheckEvent"));
        assert_eq!(e["index"], k / 2);
        assert_eq!(e["state"], if k % 2 == 0 { "running" } else { "done" });
        assert_eq!(e["law_id"], r["law"]["id"]);
    }
    // The catalog lists it cleared too.
    let list = c.ok("laws.list", json!({"world": "mimo"})).await;
    let entry = list["laws"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == r["law"]["id"])
        .unwrap()
        .clone();
    assert_eq!(entry["check"]["hardware"], true);
}

// ---- the goldens (schema/golden/*laws_check*) ------------------------------------------

/// The cases of the golden report: the open-loop law file's check, its figures
/// rounded, with the ill-conditioned case as a law ended by the core's judgement.
fn golden_cases() -> Vec<CaseResult> {
    use hertzforge_random_laws::{judge, MimoCaseMeasure, TripClass};
    let m =
        |conv: Option<u64>, at_full: u64, fin: f64, line: f64, settle: f64, calls: u64, ms: f64| {
            MimoCaseMeasure {
                reached_full_level: true,
                loops_to_converge: conv,
                loops_at_full: at_full,
                final_error_db: fin,
                worst_line_db: line,
                settle_db: settle,
                calls,
                worst_call_ms: ms,
                budget_ms: 2125.0,
                gate_passed: true,
                ..MimoCaseMeasure::default()
            }
        };
    let mut ill = m(None, 2, 6.32, 10.55, f64::NAN, 5, 32.3);
    ill.trip = Some(
        "The change cap held drive 4 back on 721 lines for 5 loops in a row: the control law \
         asked for more change than the core allows."
            .into(),
    );
    ill.trip_class = TripClass::CoreJudgement;
    ill.capped_lines = 7210;
    ill.gate_passed = false;
    let measures = [
        (
            MimoCheckCase::Modal4x4,
            m(Some(1), 1, 0.46, 4.26, f64::NAN, 3, 7.4),
        ),
        (
            MimoCheckCase::Modal12x6,
            m(None, 20, 1.98, 11.06, 0.01, 22, 32.9),
        ),
        (MimoCheckCase::Modal12x6Ill, ill),
        (
            MimoCheckCase::RigidPlate12x6,
            m(Some(1), 1, 0.62, 2.79, f64::NAN, 3, 5.7),
        ),
        (
            MimoCheckCase::Noisy4x4,
            m(Some(1), 1, 0.77, 5.29, f64::NAN, 3, 7.4),
        ),
        (
            MimoCheckCase::Ladder12x6,
            m(Some(1), 1, 0.36, 4.02, f64::NAN, 6, 31.7),
        ),
    ];
    measures
        .into_iter()
        .map(|(case, measure)| CaseResult {
            case,
            verdict: judge(case, &measure),
            measure,
        })
        .collect()
}

fn golden(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/golden")
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap()
}

#[test]
fn the_check_report_and_its_events_on_the_wire_are_the_golden_ones() {
    use sds_control::mimo::law_check::{progress_json, report_json};
    let cases = golden_cases();
    let report = report_json(&cases, true);
    let event = progress_json(
        "9f2c4e6a8b0d1f3e5a7c9b1d3f5e7a9c0b2d4f6e8a0c2e4f6a8b0d2f4e6a8c0b",
        2,
        MimoCheckCase::Modal12x6Ill,
        Some(&cases[2]),
    );
    if std::env::var_os("PRINT_LAW_CHECK_GOLDEN").is_some() {
        println!("REPORT {report}");
        println!("EVENT {event}");
    }
    let schema = Schema::load();
    let resp = golden("response_laws_check");
    schema.assert_valid(&resp, None);
    schema.assert_valid(&resp["result"], Some("lawsCheckResult"));
    assert_eq!(resp["result"]["report"], report);
    // The entry's check holds the record's cases: each case's name, verdict and words.
    let entry_cases: Vec<Value> = cases
        .iter()
        .map(|c| json!({"name": c.verdict.name, "passed": c.verdict.passed, "detail": c.verdict.detail}))
        .collect();
    assert_eq!(
        resp["result"]["law"]["check"]["cases"],
        Value::from(entry_cases)
    );
    let ev = golden("event_laws_check");
    schema.assert_valid(&ev, None);
    schema.assert_valid(&ev["data"], Some("lawsCheckEvent"));
    assert_eq!(ev["data"], event);
    let req = golden("request_laws_check");
    schema.assert_valid(&req["params"], Some("lawsCheckParams"));
    let r = sds_control::protocol::parse_request(&req).unwrap();
    assert!(r.params["path"].as_str().unwrap().ends_with(".wasm"));
}
