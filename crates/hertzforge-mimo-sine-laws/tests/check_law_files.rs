//! The law check run on the five built-in laws as law files: each gives the
//! verdicts its native form gives, case by case.
//!
//! A law file of `matrix` fits its model in its constructor, and in
//! WebAssembly that takes several times as long as natively (seconds per
//! case here, up to about a minute on a wide band). The check times the
//! build apart from the run ([`CaseMeasure::build_s`]), so the build counts
//! once per case and never against the law's per-update budget; this test
//! prints both.
//!
//! Both forms run with the law's time budget relaxed by the engine's named
//! test-only switch, `TEST_RELAX_COMPUTE_BUDGET`, in the engine and as the law
//! host's deadline on each call, so no verdict depends on how busy the
//! machine is. The constructor keeps its own budget.

use hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET;
use hertzforge_law_host::{HostConfig, LawHost};
use hertzforge_mimo_sine::laws::builtin;
use hertzforge_mimo_sine::MimoSineLawFactory;
use hertzforge_mimo_sine_laws::check::{check_law_with, sim_station, CaseResult};
use hertzforge_mimo_sine_laws::{load_law_file, WasmSineMimoLawFactory};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn law_file(id: &str) -> PathBuf {
    let file = format!("hertzforge_law_mimo_sine_{id}.wasm");
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1) {
        let p = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return p;
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

fn file_law(id: &str) -> Arc<dyn MimoSineLawFactory> {
    let host = LawHost::new(HostConfig::default()).unwrap();
    Arc::new(WasmSineMimoLawFactory::new(
        load_law_file(&host, &law_file(id)).unwrap(),
        Vec::new(),
        TEST_RELAX_COMPUTE_BUDGET,
    ))
}

fn verdicts(r: &[CaseResult]) -> Vec<(&'static str, bool, Option<String>)> {
    r.iter()
        .map(|c| {
            (
                c.verdict.case.name(),
                c.verdict.passed,
                c.verdict.failed.clone(),
            )
        })
        .collect()
}

fn same_as_native(id: &str) {
    let relaxed = Some(TEST_RELAX_COMPUTE_BUDGET);
    let native = check_law_with(builtin(id).unwrap(), sim_station, relaxed);
    let t = Instant::now();
    let file = check_law_with(file_law(id), sim_station, relaxed);
    let wall = t.elapsed().as_secs_f64();
    let build: f64 = file
        .iter()
        .filter_map(|r| r.measure.as_ref())
        .map(|m| m.build_s)
        .sum();
    for (n, f) in native.iter().zip(&file) {
        let (mn, mf) = (n.measure.as_ref(), f.measure.as_ref());
        println!(
            "{id} {:27} native {:5} file {:5} | native {:.3} file {:.3} | file build {:.2} s, run {:.2} s",
            f.verdict.case.name(),
            n.verdict.passed,
            f.verdict.passed,
            n.verdict.number,
            f.verdict.number,
            mf.map_or(f64::NAN, |m| m.build_s),
            mf.map_or(f64::NAN, |m| m.run_s),
        );
        if let (Some(a), Some(b), true) = (mn, mf, f.verdict.scored) {
            // The same law on the same station: the errors agree to rounding
            // (the file's modal fit rounds differently in its last places,
            // which moves a reported case's resonance by a tenth of a dB).
            let d = (a.worst_db - b.worst_db).abs();
            assert!(
                d.is_nan() || d <= 0.05,
                "{id} {}: worst {} against {}",
                f.verdict.case.name(),
                a.worst_db,
                b.worst_db
            );
        }
    }
    println!(
        "{id} as a law file: the check took {wall:.1} s, {build:.1} s of it building the law, \
         {:.1} s without",
        wall - build
    );
    assert_eq!(verdicts(&file), verdicts(&native), "{id}");
}

#[test]
fn mimo_sine_check_matrix_as_a_law_file() {
    same_as_native("matrix");
}

#[test]
fn mimo_sine_check_paired_as_a_law_file() {
    same_as_native("paired");
}

#[test]
fn mimo_sine_check_ratio_as_a_law_file() {
    same_as_native("ratio");
}

#[test]
fn mimo_sine_check_steepest_descent_as_a_law_file() {
    same_as_native("steepest_descent");
}

#[test]
fn mimo_sine_check_secant_as_a_law_file() {
    same_as_native("secant");
}

#[test]
fn mimo_sine_check_followed_poles_as_a_law_file() {
    same_as_native("followed_poles");
}

/// The check tells a law its station's delay through `with_number`: each
/// form of `followed_poles` takes it inside the parameter's range, with
/// every other value kept, and refuses it outside; a key the law does not
/// declare, or a law without the parameter, is not told.
#[test]
fn mimo_sine_followed_poles_is_told_its_station_delay_only_in_range() {
    use hertzforge_law_sdk::sine_mimo::builtin::STATION_DELAY_KEY as KEY;
    use hertzforge_mimo_sine::law::ParamSetting;
    let setting = |f: &dyn MimoSineLawFactory, key: &str| {
        f.describe()
            .params
            .into_iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    };
    for f in [
        builtin("followed_poles").unwrap(),
        file_law("followed_poles"),
    ] {
        assert_eq!(setting(&*f, KEY), Some(ParamSetting::Number(0.0)));
        let told = f.with_number(KEY, 0.0162).expect("in range");
        assert_eq!(setting(&*told, KEY), Some(ParamSetting::Number(0.0162)));
        assert_eq!(setting(&*told, "window"), setting(&*f, "window"));
        assert_eq!(setting(&*told, "gate"), setting(&*f, "gate"));
        for v in [-1e-3, 1.5] {
            assert!(f.with_number(KEY, v).is_none(), "{v} s");
        }
        assert!(f.with_number("no_such_key", 0.0162).is_none());
    }
    for id in ["matrix", "paired"] {
        assert!(builtin(id).unwrap().with_number(KEY, 0.0162).is_none());
        assert!(file_law(id).with_number(KEY, 0.0162).is_none());
    }
}
