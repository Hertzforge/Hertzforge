//! The eight TWR check cases on the simulator: the built-in passes every one,
//! a misbehaving law file fails every one, and the deep-queue station aligns
//! on the lag the identification measured, not the one its lookahead implies.

mod common;

use common::{law_file, station_for};
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_twr_laws::{
    default_twr_choice, judge_twr, resolve_twr, run_twr_check_case, twr_check_record, TwrCaseEnd,
    TwrCheckCase, WasmTwrLawFactory,
};
use std::sync::Arc;

fn catalog() -> (hertzforge_random_laws::LawCatalog, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "hertzforge-twr-laws-check-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    (
        hertzforge_random_laws::LawCatalog::open(&root).unwrap(),
        root,
    )
}

#[test]
fn the_built_in_passes_every_case() {
    let (cat, root) = catalog();
    let mut report = Vec::new();
    let mut cases = Vec::new();
    for case in TwrCheckCase::ALL {
        let factory = resolve_twr(&cat, &default_twr_choice()).unwrap();
        let m = run_twr_check_case(case, factory, station_for(case)).unwrap();
        let judged = judge_twr(case, &m);
        report.push(format!(
            "{:22} {:5} {}",
            case.name(),
            if judged.passed { "pass" } else { "FAIL" },
            judged.detail
        ));
        cases.push(judged);
    }
    for line in &report {
        println!("{line}");
    }
    assert!(cases.iter().all(|c| c.passed), "{}", report.join("\n"));
    let record = twr_check_record("0".repeat(64), "2026-11-08T20:00:00Z", cases);
    assert!(record.passed);
    assert_eq!(record.tool_version, "twr-check 1");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_deep_queue_station_aligns_on_the_measured_lag() {
    let (cat, root) = catalog();
    let case = TwrCheckCase::DeepQueue;
    let m = run_twr_check_case(
        case,
        resolve_twr(&cat, &default_twr_choice()).unwrap(),
        station_for(case),
    )
    .unwrap();
    assert_eq!(m.end, TwrCaseEnd::Completed);
    // The station states a 512-frame lookahead, so the derived lag is 512;
    // the record plays on a cold task with its head committed, so the
    // response trails the drive by the wire's own 41 frames.
    assert!(m.lag_measured);
    assert_eq!(m.lag_frames, Some(41));
    let _ = std::fs::remove_dir_all(root);
}

/// A law that returns a NaN on every correction fails every case that calls
/// it after its seed, and so fails the check. On the cases whose every rung
/// gates on the seed the law is not called again, and its seed is the
/// built-in's, so those pass; the identification error is the case built so
/// that a law must correct.
#[test]
fn a_misbehaving_law_file_fails_the_check() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_twr(&std::fs::read(law_file("law_fixture_twr_misbehave")).unwrap())
        .unwrap();
    let params = law
        .resolve_params(&[Param::new("mode", ParamValue::Choice("nan".into()))])
        .unwrap();
    let mut cases = Vec::new();
    let mut called = Vec::new();
    for case in TwrCheckCase::ALL {
        let factory = Arc::new(WasmTwrLawFactory::new(law.clone(), params.clone()));
        let m = run_twr_check_case(case, factory, station_for(case)).unwrap();
        let judged = judge_twr(case, &m);
        if m.calls > 1 {
            called.push(case);
            assert!(!judged.passed, "{}: {}", case.name(), judged.detail);
            assert!(
                matches!(m.end, TwrCaseEnd::LawFault(ref r) if r == "non_finite"),
                "{:?}",
                m.end
            );
        }
        cases.push(judged);
    }
    // The identification error is the case a law must correct to pass.
    assert!(
        called.contains(&TwrCheckCase::IdentificationError),
        "{called:?}"
    );
    assert!(!twr_check_record("0".repeat(64), "2026-11-08T20:00:00Z", cases).passed);
}

/// The SDK's template at its defaults, the law the bench imports, passes
/// every case, the deep queue included.
#[test]
fn the_template_passes_every_case() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let law = host
        .load_twr(&std::fs::read(law_file("law_fixture_twr_template")).unwrap())
        .unwrap();
    let params = law.resolve_params(&[]).unwrap();
    let mut report = Vec::new();
    let mut all = true;
    for case in TwrCheckCase::ALL {
        let factory = Arc::new(WasmTwrLawFactory::new(law.clone(), params.clone()));
        let judged = judge_twr(
            case,
            &run_twr_check_case(case, factory, station_for(case)).unwrap(),
        );
        all &= judged.passed;
        report.push(format!(
            "{}: {} {}",
            case.name(),
            judged.passed,
            judged.detail
        ));
    }
    for line in &report {
        println!("{line}");
    }
    assert!(all, "{}", report.join("\n"));
}
