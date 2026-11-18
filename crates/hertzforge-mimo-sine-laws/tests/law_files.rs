//! The five built-in MIMO Sine laws as law files: each states what its
//! declaration states, is refused a run it does not accept, lists in the
//! catalog with its test types, and gives the drives of its native form on
//! the same inputs over whole recorded sequences (sweeps, dwells and stepped
//! sines on the law mathematics' exact linear structure, square, tall and
//! wide).

use hertzforge_law_host::{HostConfig, LawHost, Param, ParamValue};
use hertzforge_law_sdk::sine_mimo::builtin::{self, LawDecl};
use hertzforge_law_sdk::Params;
use hertzforge_mimo_sine::{LawFault, MimoSineLawFactory, TestType};
use hertzforge_mimo_sine_law_math::reference_structure::{ModalStructure, Schedule};
use hertzforge_mimo_sine_law_math::Setup;
use hertzforge_mimo_sine_laws::parity::bench::{record, Recording};
use hertzforge_mimo_sine_laws::parity::{relative_difference, run_sequence};
use hertzforge_mimo_sine_laws::{
    compare_laws, entries, load_law_file, MathLawFactory, WasmSineMimoLawFactory,
};
use hertzforge_random_laws::LawCatalog;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn law_file(name: &str) -> PathBuf {
    let file = format!("{name}.wasm");
    let exe = std::env::current_exe().expect("the test binary knows its own path");
    for dir in exe.ancestors().skip(1).take(4) {
        let p = dir.join("wasm32-wasip2").join("release").join(&file);
        if p.is_file() {
            return p;
        }
    }
    panic!("the law file {file} is not built. Run `bash scripts/build_laws.sh`.");
}

fn file_of(d: &LawDecl) -> PathBuf {
    law_file(&format!("hertzforge_law_mimo_sine_{}", d.id))
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "hertzforge-mimo-sine-laws-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const BUDGET: Duration = Duration::from_secs(5);

/// A coupled structure of three modes, `controls` by `drives`.
fn structure(controls: usize, drives: usize) -> ModalStructure {
    let modes = 3;
    let shape = |k: usize, n: usize| -> Vec<f64> {
        (0..k * n)
            .map(|i| {
                let (r, c) = ((i / n) as f64, (i % n) as f64);
                (1.3 * r + 0.7 * c + 0.4).sin() + if r as usize == c as usize { 1.5 } else { 0.0 }
            })
            .collect()
    };
    ModalStructure {
        f_hz: vec![42.0, 63.0, 91.0],
        zeta: vec![0.03, 0.02, 0.04],
        drive_shape: shape(modes, drives),
        output_shape: shape(modes, controls),
        drives,
        controls,
    }
}

fn setup_of(st: &ModalStructure) -> Setup {
    st.setup((30.0, 120.0), false)
}

const DWELL: Schedule = Schedule::Dwell { f: 63.0, secs: 2.0 };
const STEPS: Schedule = Schedule::Steps {
    f0: 40.0,
    f1: 90.0,
    per_octave: 6.0,
    secs: 0.4,
};
const SWEEP: Schedule = Schedule::Sweep {
    f0: 40.0,
    f1: 85.0,
    oct_min: 4.0,
};

fn host_params(p: &[(&str, ParamValue)]) -> Vec<Param> {
    p.iter()
        .map(|(k, v)| Param {
            key: (*k).into(),
            value: v.clone(),
        })
        .collect()
}

fn sdk_params(d: &LawDecl, p: &[(&str, ParamValue)]) -> Params {
    let mut all = builtin::defaults(d);
    for (k, v) in p {
        let v = hertzforge_mimo_sine_laws::choice::sdk_value(v);
        all.0.iter_mut().find(|q| q.key == *k).unwrap().value = v;
    }
    all
}

/// Rounding only: the law file and the native law run the same code, and
/// differ only where the platforms' elementary functions round differently.
const ROUNDING: f64 = 1e-12;

/// `matrix` with its default modal model. The modal fit is an iterative
/// search; elementary functions that round differently in the last place on
/// the two platforms move its result, and so the drive, by up to 2.4e-7 of the
/// update's largest drive and a reported value (the condition number, the
/// rotation) by up to 1.0e-5 (measured on this suite's sequences on a Mac; the
/// worst was the wide sweep). Other platforms round differently again, so the
/// bounds keep a margin: 1e-5 of the drive (100 dB under it) and 1e-3 of a
/// value. The same law on a model without a fit is held to [`ROUNDING`].
const MODAL_FIT: f64 = 1e-5;

/// Record `d` on the structure, then run its native form and its law file
/// over the recorded inputs: the native law gives the mathematics' own drives,
/// and the law file the native law's, on every update, within `tol` of the
/// update's largest drive.
fn parity(
    d: &'static LawDecl,
    p: &[(&str, ParamValue)],
    st: &ModalStructure,
    setup: &Setup,
    schedule: Schedule,
    test_type: TestType,
) -> usize {
    parity_within(d, p, st, setup, schedule, test_type, ROUNDING)
}

fn parity_within(
    d: &'static LawDecl,
    p: &[(&str, ParamValue)],
    st: &ModalStructure,
    setup: &Setup,
    schedule: Schedule,
    test_type: TestType,
    tol: f64,
) -> usize {
    let params = sdk_params(d, p);
    let Recording {
        setup: law_setup,
        inputs,
        asked,
    } = record(d, &params, st, setup, schedule, test_type, 20_000).unwrap();
    assert!(inputs.len() > 20, "{}: a short sequence", d.id);
    let native = MathLawFactory::new(d, params).unwrap();
    let host = LawHost::new(HostConfig::default()).unwrap();
    let file = WasmSineMimoLawFactory::new(
        load_law_file(&host, &file_of(d)).unwrap(),
        host_params(p),
        BUDGET,
    );
    // The native law gives the mathematics' drives.
    let mut law = native.build(&law_setup).unwrap();
    let answers = run_sequence(
        law.as_mut(),
        &law_setup,
        native.manifest().values.len(),
        &inputs,
    )
    .unwrap();
    for (k, (a, m)) in answers.iter().zip(&asked).enumerate() {
        let e = relative_difference(&a.drive, m);
        assert!(
            e <= 1e-12,
            "{}: native against the math at {k}: {e:e}",
            d.id
        );
    }
    // The law file gives the native law's drives and values.
    let r = compare_laws(&native, &file, &law_setup, &inputs).unwrap();
    assert_eq!(r.updates, inputs.len());
    eprintln!(
        "{} {test_type:?}: {} updates, file against native {:e}, values {:e}",
        d.id, r.updates, r.worst_relative, r.worst_value
    );
    assert!(
        r.worst_relative <= tol,
        "{}: file against native, {:e} at update {}",
        d.id,
        r.worst_relative,
        r.worst_update
    );
    assert!(
        r.worst_value <= if tol > ROUNDING { 1e-3 } else { 1e-9 },
        "{}: values {:e}",
        d.id,
        r.worst_value
    );
    assert_eq!(r.mismatched_updates, 0, "{}", d.id);
    inputs.len()
}

#[test]
fn mimo_sine_each_law_file_states_its_declaration() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    for d in &builtin::ALL {
        let law = load_law_file(&host, &file_of(d)).unwrap();
        let m = law.manifest();
        assert_eq!(m.law.name, d.id);
        assert_eq!(m.shapes.len(), d.shapes.len(), "{}", d.id);
        assert_eq!(m.amplitude_only, d.amplitude_only, "{}", d.id);
        let keys: Vec<&str> = m.law.params.iter().map(|p| p.key.as_str()).collect();
        let want_keys: Vec<&str> = d.params.iter().map(|p| p.key).collect();
        assert_eq!(keys, want_keys, "{}", d.id);
        let values: Vec<&str> = m.law.diagnostics.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(values, d.value_keys(), "{}", d.id);
        let f = WasmSineMimoLawFactory::new(law.clone(), Vec::new(), BUDGET);
        let native = MathLawFactory::new(d, builtin::defaults(d)).unwrap();
        assert_eq!(f.manifest(), native.manifest(), "{}", d.id);
    }
}

#[test]
fn mimo_sine_a_law_file_is_refused_a_run_it_does_not_accept() {
    let host = LawHost::new(HostConfig::default()).unwrap();
    let st = structure(3, 3);
    let mut setup = setup_of(&st);
    setup.phase_specified = false;
    let law_setup = hertzforge_mimo_sine_laws::parity::bench::law_setup(&setup, TestType::Dwell);
    for (id, accepts) in [("matrix", true), ("ratio", true), ("secant", false)] {
        let d = builtin::decl(id).unwrap();
        let f = WasmSineMimoLawFactory::new(
            load_law_file(&host, &file_of(d)).unwrap(),
            Vec::new(),
            BUDGET,
        );
        match f.build(&law_setup) {
            Ok(_) => assert!(accepts, "{id} took an amplitude-only reference"),
            Err(LawFault::Failed(m)) => {
                assert!(!accepts, "{id}: {m}");
                assert!(m.contains("amplitudes only"), "{m}");
            }
            Err(e) => panic!("{id}: {e}"),
        }
    }
    let sweep =
        hertzforge_mimo_sine_laws::parity::bench::law_setup(&setup_of(&st), TestType::Sweep);
    let d = builtin::decl("steepest_descent").unwrap();
    let f = WasmSineMimoLawFactory::new(
        load_law_file(&host, &file_of(d)).unwrap(),
        Vec::new(),
        BUDGET,
    );
    match f.build(&sweep) {
        Err(LawFault::Failed(m)) => assert!(m.contains("accepts dwell, not sweep"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    // `drive` pairing needs a square setup: the law's own refusal.
    let tall = structure(3, 2);
    let tall_setup =
        hertzforge_mimo_sine_laws::parity::bench::law_setup(&setup_of(&tall), TestType::Dwell);
    let d = builtin::decl("ratio").unwrap();
    let f = WasmSineMimoLawFactory::new(
        load_law_file(&host, &file_of(d)).unwrap(),
        host_params(&[("pairing", ParamValue::Choice("drive".into()))]),
        BUDGET,
    );
    match f.build(&tall_setup) {
        Err(LawFault::Failed(m)) => assert!(m.contains("square"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn mimo_sine_an_imported_law_file_lists_with_its_test_types() {
    let scratch = Scratch::new("entries");
    let mut catalog =
        LawCatalog::with_host(LawHost::new(HostConfig::default()).unwrap(), scratch.path())
            .unwrap();
    let before = entries(&catalog);
    let ids: Vec<&str> = before.iter().map(|e| e.entry.id.as_str()).collect();
    assert_eq!(
        ids,
        ["matrix", "paired", "steepest_descent", "secant", "ratio"]
    );
    assert_eq!(before[0].test_type_names(), ["sweep", "dwell", "stepped"]);
    assert_eq!(before[3].test_type_names(), ["dwell"]);
    assert!(before[0].amplitude_only && !before[3].amplitude_only);
    let imported = catalog
        .import(&law_file("law_fixture_sine_mimo_template"))
        .unwrap();
    let after = entries(&catalog);
    assert_eq!(after.len(), 6);
    let file = after.last().unwrap();
    assert_eq!(file.entry.id, imported.id);
    assert!(!file.entry.builtin);
    assert_eq!(file.test_type_names(), ["dwell", "stepped"]);
    assert!(file.amplitude_only);
    assert_eq!(file.shape_names().len(), 3);
    // It resolves to its file; a law of another world is named as such.
    let f = hertzforge_mimo_sine_laws::resolve(&catalog, &imported.id, Vec::new(), BUDGET).unwrap();
    assert_eq!(f.describe().sha256.as_deref(), Some(imported.id.as_str()));
    let other = catalog
        .import(&law_file("hertzforge_law_mimo_shock_paired"))
        .unwrap();
    let e = hertzforge_mimo_sine_laws::resolve(&catalog, &other.id, Vec::new(), BUDGET)
        .err()
        .unwrap();
    assert!(e.to_string().contains("not a MIMO Sine law"), "{e}");
}

#[test]
fn mimo_sine_parity_matrix_sweeps_square_tall_and_wide() {
    let d = &builtin::MATRIX;
    let mut n = 0;
    let measured = [("model", ParamValue::Choice("identification".into()))];
    for (c, m) in [(3, 3), (3, 2), (2, 3)] {
        let st = structure(c, m);
        let s = setup_of(&st);
        n += parity_within(d, &[], &st, &s, SWEEP, TestType::Sweep, MODAL_FIT);
        n += parity(d, &measured, &st, &s, SWEEP, TestType::Sweep);
    }
    let st = structure(3, 3);
    n += parity(
        d,
        &[
            ("carry", ParamValue::Number(0.0)),
            ("gain_rule", ParamValue::Choice("fixed".into())),
            ("fixed_gain", ParamValue::Number(0.2)),
            ("model", ParamValue::Choice("smoothed".into())),
        ],
        &st,
        &setup_of(&st),
        SWEEP,
        TestType::Sweep,
    );
    assert!(n > 3000, "{n} updates");
}

#[test]
fn mimo_sine_parity_matrix_dwell_steps_and_free_phases() {
    let d = &builtin::MATRIX;
    let st = structure(3, 3);
    let s = setup_of(&st);
    parity_within(d, &[], &st, &s, DWELL, TestType::Dwell, MODAL_FIT);
    parity(
        d,
        &[("model", ParamValue::Choice("smoothed".into()))],
        &st,
        &s,
        STEPS,
        TestType::Stepped,
    );
    let mut free = s.clone();
    free.phase_specified = false;
    for fp in ["in_phase", "measured"] {
        parity(
            d,
            &[
                ("free_phase", ParamValue::Choice(fp.into())),
                ("model", ParamValue::Choice("identification".into())),
            ],
            &st,
            &free,
            DWELL,
            TestType::Dwell,
        );
    }
}

#[test]
fn mimo_sine_parity_paired_steepest_secant_and_ratio() {
    let st = structure(3, 3);
    let s = setup_of(&st);
    parity(&builtin::PAIRED, &[], &st, &s, DWELL, TestType::Dwell);
    parity(&builtin::PAIRED, &[], &st, &s, STEPS, TestType::Stepped);
    parity(
        &builtin::STEEPEST_DESCENT,
        &[],
        &st,
        &s,
        DWELL,
        TestType::Dwell,
    );
    for carry in ["factor", "plain"] {
        parity(
            &builtin::SECANT,
            &[("carry", ParamValue::Choice(carry.into()))],
            &st,
            &s,
            DWELL,
            TestType::Dwell,
        );
    }
    parity(&builtin::RATIO, &[], &st, &s, DWELL, TestType::Dwell);
    parity(&builtin::RATIO, &[], &st, &s, STEPS, TestType::Stepped);
    parity(
        &builtin::RATIO,
        &[("pairing", ParamValue::Choice("drive".into()))],
        &st,
        &s,
        DWELL,
        TestType::Dwell,
    );
    let tall = structure(3, 2);
    parity(
        &builtin::PAIRED,
        &[],
        &tall,
        &setup_of(&tall),
        DWELL,
        TestType::Dwell,
    );
}
