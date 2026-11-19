//! The built-in law and its law file declare the same thing, and the check
//! fails a law file that misbehaves while passing the same file when it does
//! not.

mod common;

use common::{law_file, station_for};
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_random_laws::{sine_builtin_live, sine_builtin_manifest};
use hertzforge_sine_laws::{
    judge_sine, run_check_case, SineCaseEnd, SineCheckCase, WasmSineLawFactory,
};
use std::sync::Arc;

#[test]
fn the_built_in_declares_what_its_law_file_declares() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let bytes = std::fs::read(law_file("hertzforge_law_sine_servo")).unwrap();
    let law = host.load_sine(&bytes).unwrap();
    assert_eq!(law.manifest(), &sine_builtin_manifest());
    assert_eq!(law.live(), sine_builtin_live().as_slice());
}

fn fixture(host: &LawHost, mode: &str, after: i64) -> WasmSineLawFactory {
    let bytes = std::fs::read(law_file("law_fixture_sine_misbehave")).unwrap();
    let law = host.load_sine(&bytes).unwrap();
    let params = law
        .resolve_params(
            &[
                Param::new("mode", ParamValue::Choice(mode.into())),
                Param::new("after", ParamValue::Integer(after)),
            ],
            1,
        )
        .unwrap();
    WasmSineLawFactory::new(law, params)
}

#[test]
fn a_sine_law_file_that_misbehaves_fails_the_check_and_the_same_file_behaving_passes() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let case = SineCheckCase::Noise;
    let m = run_check_case(case, Arc::new(fixture(&host, "ok", 0)), station_for(case)).unwrap();
    let j = judge_sine(case, &m);
    assert!(j.passed, "the fixture behaving: {}", j.detail);
    for (mode, after, reason) in [
        ("nan", 0, "non_finite"),
        ("negative", 0, "negative"),
        ("sleep_after", 100, "over_budget"),
    ] {
        let m = run_check_case(
            case,
            Arc::new(fixture(&host, mode, after)),
            station_for(case),
        )
        .unwrap();
        let j = judge_sine(case, &m);
        assert!(!j.passed, "{mode} passed: {}", j.detail);
        assert_eq!(m.end, SineCaseEnd::LawFault(reason.into()), "{mode}");
        assert!(m.misses >= 3, "{mode}: {} misses", m.misses);
        assert!(j.detail.contains("not used"), "{mode}: {}", j.detail);
        println!("{mode}: {}", j.detail);
    }
}
