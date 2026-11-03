//! The built-in laws through the same door as an imported law: each built-in
//! law, run natively, against its own law file run through the law host and
//! [`hertzforge_random_laws::WasmControlLaw`], over a whole Random run on the
//! Sim plant (identification, the soft start, every rung of the ladder with
//! its ramps, full level, and the ramped stop), at the default parameter values
//! and at others (which must change the run, so they are known to reach both
//! builds).
//!
//! The two are one implementation built for two targets, so they agree to
//! within floating-point rounding, not bit for bit: a law file's `powf`,
//! `log10` and the like are the Rust standard library's WebAssembly versions,
//! the native law's are the platform's. The Sim plant's noise is seeded, so
//! the only source of difference is that rounding, carried around the loop.
//!
//! **Tolerance.** Each drive the core played, each drive and level command the
//! law proposed: agree within a relative `1e-9` per line (and `1e-9` dB per
//! level command). A last-bit difference in one transcendental call is about
//! `1e-16`; the loop is stable, so differences decay rather than grow, and the
//! largest seen over a whole run is printed by each test (measured well under
//! `1e-12`). `1e-9` leaves three orders of headroom for another platform's
//! math library while still catching any real difference in the law: one
//! line moved by a thousandth of a dB is `2.3e-4`. Everything discrete must
//! agree exactly: the run's states and levels tick by tick, which calls were
//! made and which failed, the trip, and the core's guard counts.

mod common;

use common::*;
use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_random::service::config::RandomTestConfig;
use hertzforge_random::{LawFactory, LawGuardStats, RunState, TripRecord};
use hertzforge_random_laws::{BuiltinLaw, LawCatalog, LawChoice};
use std::sync::Arc;

/// Relative agreement required of every drive line and level command.
const TOL: f64 = 1e-9;

struct Ran {
    /// The name the run's law gave itself.
    law: String,
    ticks: Vec<Tick>,
    calls: Vec<Call>,
    state: RunState,
    trip: Option<TripRecord>,
    stats: LawGuardStats,
}

fn ladder(factory: Arc<dyn LawFactory>, cfg: RandomTestConfig, tag: &str) -> Ran {
    let scratch = Scratch::new(tag);
    let (f, log) = recording(factory);
    let mut svc = service(cfg, f, scratch.path(), true);
    engage(&mut svc);
    let ticks = run(&mut svc, 400);
    let calls = log.lock().unwrap().clone();
    Ran {
        law: svc.law_name().unwrap_or_default().to_string(),
        ticks,
        calls,
        state: svc.state(),
        trip: svc.trip().cloned(),
        stats: svc.law_guard_stats(),
    }
}

/// The largest relative difference between two spectra, over lines where
/// either is above zero.
fn rel_diff(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len(), "spectra of different lengths");
    a.iter()
        .zip(b)
        .filter(|(x, y)| x.max(**y) > 0.0)
        .map(|(x, y)| (x - y).abs() / x.max(*y))
        .fold(0.0, f64::max)
}

/// Native and law-file runs agree; returns the largest drive and level
/// differences seen.
fn assert_agree(native: &Ran, wasm: &Ran) -> (f64, f64) {
    assert_eq!(native.state, wasm.state, "run outcome");
    assert_eq!(native.trip, wasm.trip, "trip");
    assert_eq!(native.stats.calls, wasm.stats.calls, "law calls");
    assert_eq!(native.stats.misses, wasm.stats.misses, "misses");
    assert_eq!(
        native.stats.capped_lines, wasm.stats.capped_lines,
        "capped lines"
    );
    assert_eq!(native.ticks.len(), wasm.ticks.len(), "ticks");
    let mut worst_drive = 0.0f64;
    for (i, (n, w)) in native.ticks.iter().zip(&wasm.ticks).enumerate() {
        assert_eq!(n.state, w.state, "tick {}: state", i + 1);
        assert_eq!(n.level_db, w.level_db, "tick {}: scheduled level", i + 1);
        let d = rel_diff(&n.drive, &w.drive);
        assert!(d <= TOL, "tick {}: played drives differ by {d:e}", i + 1);
        worst_drive = worst_drive.max(d);
    }
    assert_eq!(native.calls.len(), wasm.calls.len(), "law calls recorded");
    let mut worst_level = 0.0f64;
    for (i, (n, w)) in native.calls.iter().zip(&wasm.calls).enumerate() {
        assert_eq!(n.kind, w.kind, "call {i}: kind");
        match (&n.out, &w.out) {
            (Ok((nd, nl)), Ok((wd, wl))) => {
                let d = rel_diff(nd, wd);
                assert!(
                    d <= TOL,
                    "call {i} ({}): proposed drives differ by {d:e}",
                    n.kind
                );
                worst_drive = worst_drive.max(d);
                let l = (nl - wl).abs();
                assert!(
                    l <= TOL,
                    "call {i} ({}): level commands {nl} and {wl}",
                    n.kind
                );
                worst_level = worst_level.max(l);
            }
            (Err(_), Err(_)) => {}
            (n, w) => panic!("call {i}: native {n:?}, law file {w:?}"),
        }
    }
    (worst_drive, worst_level)
}

/// The run is not vacuous: it identified, climbed every rung, held full level
/// and stopped under control, with the law steering every loop.
fn assert_full_ladder(r: &Ran, cfg: &RandomTestConfig) {
    assert_eq!(
        r.state,
        RunState::Stopped,
        "the ladder completes: {:?}",
        r.trip
    );
    assert!(r.trip.is_none(), "no trip: {:?}", r.trip);
    assert_eq!(r.stats.misses, 0);
    for rung in &cfg.schedule.table_db {
        assert!(
            r.ticks.iter().any(|t| (t.level_db - rung).abs() < 1e-9),
            "the run never reached the {rung} dB rung"
        );
    }
    let iterations = r.calls.iter().filter(|c| c.kind == "iterate").count();
    println!(
        "{} ticks, {iterations} loops steered by the law, rungs {:?}",
        r.ticks.len(),
        cfg.schedule.table_db
    );
    let retargets = r.calls.iter().filter(|c| c.kind == "retarget").count();
    assert!(
        iterations >= 2 * cfg.schedule.table_db.len(),
        "only {iterations} loops ran"
    );
    assert!(
        retargets >= cfg.schedule.table_db.len(),
        "only {retargets} level changes"
    );
}

fn catalog(scratch: &Scratch) -> LawCatalog {
    let host = LawHost::new(HostConfig::default()).expect("the host starts");
    LawCatalog::with_host(host, scratch.path()).expect("the catalog opens")
}

fn compare(builtin: BuiltinLaw, file: &str, params: Vec<Param>, tag: &str) -> Ran {
    let scratch = Scratch::new(tag);
    let mut cat = catalog(&scratch);
    let imported = cat.import(&law_file(file)).expect("the law file imports");
    assert!(!imported.builtin);

    let mut native_cfg = ladder_cfg();
    let native = cat
        .resolve(
            &LawChoice::new(builtin.id(), params.clone()),
            &mut native_cfg,
        )
        .expect("the built-in resolves");
    let mut wasm_cfg = ladder_cfg();
    let wasm = cat
        .resolve(&LawChoice::new(imported.id.clone(), params), &mut wasm_cfg)
        .expect("the law file resolves");
    // The core reads some of the `hinv` settings itself (the divergence count,
    // the notch's gain floor, the change cap's floor). Choosing the built-in
    // writes the law's values there; give the law-file run the same, so the
    // two runs differ only in which build of the law steers them.
    wasm_cfg.hinv = native_cfg.hinv.clone();
    assert_eq!(native_cfg, wasm_cfg, "both runs are configured alike");

    let n = ladder(native, native_cfg.clone(), &format!("{tag}-native"));
    let w = ladder(wasm, wasm_cfg, &format!("{tag}-wasm"));
    // Not the same law twice: the native law names itself by its id, the law
    // file by its manifest.
    assert_eq!(n.law, builtin.id());
    assert_eq!(w.law, imported.manifest.name);
    assert_ne!(n.law, w.law);
    assert_full_ladder(&n, &native_cfg);
    let (drive, level) = assert_agree(&n, &w);
    println!(
        "{tag}: {} ticks, {} law calls; largest relative drive difference {drive:e}, largest \
         level command difference {level:e} dB (tolerance {TOL:e})",
        n.ticks.len(),
        n.calls.len()
    );
    n
}

/// The parameter values changed the run: they were not dropped on the way to
/// both builds.
fn assert_moved(with: &Ran, without: &Ran) {
    let moved = with
        .ticks
        .iter()
        .zip(&without.ticks)
        .map(|(a, b)| rel_diff(&a.drive, &b.drive))
        .fold(0.0, f64::max);
    println!("the parameter values moved the played drive by up to {moved:e}");
    assert!(
        moved > 1e-3,
        "the parameter values changed nothing: {moved:e}"
    );
}

#[test]
fn native_hinv_and_the_hinv_law_file_run_a_ladder_alike() {
    compare(BuiltinLaw::Hinv, HINV, Vec::new(), "hinv");
}

#[test]
fn native_ratio_and_the_ratio_law_file_run_a_ladder_alike() {
    compare(BuiltinLaw::Ratio, RATIO, Vec::new(), "ratio");
}

fn num(key: &str, v: f64) -> Param {
    Param::new(key, ParamValue::Number(v))
}

#[test]
fn parameter_values_reach_both_builds_of_hinv_alike() {
    let without = compare(BuiltinLaw::Hinv, HINV, Vec::new(), "hinv-defaults");
    let with = compare(
        BuiltinLaw::Hinv,
        HINV,
        vec![
            num("damping", 2.0),
            num("max_correction_db_per_iter", 3.0),
            num("trust_coherence_floor", 0.6),
            Param::new("divergence_n", ParamValue::Integer(8)),
        ],
        "hinv-params",
    );
    assert_moved(&with, &without);
}

#[test]
fn parameter_values_reach_both_builds_of_ratio_alike() {
    let without = compare(BuiltinLaw::Ratio, RATIO, Vec::new(), "ratio-defaults");
    let with = compare(
        BuiltinLaw::Ratio,
        RATIO,
        vec![num("gain", 0.3), num("max_step_db", 3.0)],
        "ratio-params",
    );
    assert_moved(&with, &without);
}

#[test]
fn each_built_in_manifest_is_its_law_files_manifest() {
    let host = LawHost::new(HostConfig::default()).expect("the host starts");
    for (b, file) in [(BuiltinLaw::Hinv, HINV), (BuiltinLaw::Ratio, RATIO)] {
        let bytes = std::fs::read(law_file(file)).expect("the law file reads");
        let loaded = host.load(&bytes).expect("the law file loads");
        assert_eq!(loaded.manifest(), &b.manifest(), "{b:?}");
    }
}
