//! Native against law file: each law runs both ways over the same synthetic
//! run and the two are held to the same drives and diagnostics (the pilot's
//! acceptance A3: within 1e-12 relative, and a 1 percent parameter change is
//! detected).
//!
//! Needs the law files built by `scripts/build_laws.sh`.

mod common;

use common::*;
use hertzforge_law_host::{DiagnosticKind, Param, ParamValue};
use hertzforge_mimo_laws::{BuiltinMimoLaw, DiagKind, ParamKindDecl, ParamVal, ZeroLaw};

const ZERO: &str = "hertzforge_law_mimo_zero";

/// The bar: the pilot's single-output laws agreed to 1e-15; the MIMO bar in
/// the design is 1e-12 relative.
const PARITY: f64 = 1e-12;

#[test]
fn the_zero_law_file_declares_what_the_native_law_declares() {
    let spec = ZeroLaw::spec();
    let law = load(ZERO);
    let m = law.manifest();
    assert_eq!(m.law.name, spec.name);
    assert_eq!(m.law.version, spec.version);
    assert_eq!(m.law.author, spec.author);
    assert_eq!(m.law.summary, spec.summary);
    assert_eq!(m.shapes, spec.shapes);
    assert_eq!(m.needs, spec.needs);
    assert_eq!(m.law.params.len(), spec.params.len());
    for (p, d) in m.law.params.iter().zip(&spec.params) {
        assert_eq!(p.key, d.key);
        match (&p.kind, &d.kind) {
            (
                hertzforge_law_host::ParamKind::Number(s),
                ParamKindDecl::Number {
                    default, min, max, ..
                },
            ) => assert_eq!((s.default, s.min, s.max), (*default, *min, *max)),
            other => panic!("{other:?}"),
        }
    }
    let kinds: Vec<(String, DiagnosticKind)> = m
        .diagnostic_layouts
        .iter()
        .map(|l| (l.key.clone(), l.kind))
        .collect();
    let want: Vec<(String, DiagnosticKind)> = spec
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.key.to_string(),
                match d.kind {
                    DiagKind::Scalar => DiagnosticKind::Scalar,
                    DiagKind::PerLine => DiagnosticKind::PerLine,
                    DiagKind::PerControlLine => DiagnosticKind::PerControlLine,
                    DiagKind::PerDriveLine => DiagnosticKind::PerDriveLine,
                },
            )
        })
        .collect();
    assert_eq!(kinds, want);
}

#[test]
fn the_zero_law_agrees_native_and_as_a_law_file_at_both_sizes() {
    for (nc, nd, seed) in [(3, 2, 11u64), (6, 4, 12)] {
        let s = Synthetic::random(seed, nc, nd, 64);
        let mut native = ZeroLaw::with_params(&s.setup, &[]).expect("builds");
        let mut file = wasm_law(ZERO, &s.setup, &[]);
        let p = parity(&mut native, &mut file, &s, 10);
        println!("zero law {nc} x {nd}: {p:?}");
        assert_eq!(p.loops, 11);
        assert_eq!(p.drive, 0.0);
        assert_eq!(p.drive_scale, 0.0, "the zero law drives nothing");
        assert!(p.diagnostics <= PARITY, "{nc} x {nd}: {p:?}");
    }
}

#[test]
fn a_one_percent_parameter_change_is_detected() {
    let s = Synthetic::random(13, 3, 2, 64);
    let mut native =
        ZeroLaw::with_params(&s.setup, &[("gain", ParamVal::Number(1.0))]).expect("builds");
    let mut file = wasm_law(
        ZERO,
        &s.setup,
        &[Param::new("gain", ParamValue::Number(1.01))],
    );
    let p = parity(&mut native, &mut file, &s, 3);
    println!("zero law, gain 1.00 against 1.01: {p:?}");
    assert!(
        p.diagnostics > 1e3 * PARITY,
        "a 1% change was not seen: {p:?}"
    );
}

// -------------------------------------------------------------------------
// The built-in laws
// -------------------------------------------------------------------------

use hertzforge_mimo_laws::BuiltinMimo;

/// The law file of each built-in law.
fn file_of(law: BuiltinMimo) -> &'static str {
    match law {
        BuiltinMimo::PseudoInverse => "hertzforge_law_mimo_pinv",
        BuiltinMimo::OpenLoop => "hertzforge_law_mimo_open_loop",
        BuiltinMimo::ShapeConstrained => "hertzforge_law_mimo_shape",
        BuiltinMimo::BuzzMatched => "hertzforge_law_mimo_buzz",
        BuiltinMimo::MinimumDrive => "hertzforge_law_mimo_min_drive",
        BuiltinMimo::BandAverage => "hertzforge_law_mimo_band_average",
    }
}

/// Each law's parameters away from their defaults, and a 1 percent change of
/// one of them that moves the drive.
/// Parameter values by key.
type Given = Vec<(&'static str, ParamVal)>;
/// A parameter and two values 1 percent apart.
type OnePercent = (&'static str, f64, f64);

fn non_default(law: BuiltinMimo) -> (Given, OnePercent) {
    let num = ParamVal::Number;
    let choice = |s: &str| ParamVal::Choice(s.into());
    match law {
        BuiltinMimo::PseudoInverse => (
            vec![
                ("regularization", choice("tikhonov")),
                ("regularization_amount", num(1e-2)),
                ("convergence", num(0.8)),
                ("cap_up_db", num(2.0)),
                ("cap_down_db", num(4.0)),
                ("rank_tolerance", num(1e-4)),
                ("weighting", choice("reference")),
            ],
            ("convergence", 0.5, 0.505),
        ),
        BuiltinMimo::OpenLoop => (
            vec![
                ("regularization", choice("tikhonov")),
                ("regularization_amount", num(1e-2)),
                ("weighting", choice("reference")),
            ],
            ("regularization_amount", 0.1, 0.101),
        ),
        BuiltinMimo::ShapeConstrained => (
            vec![
                ("selection", choice("count")),
                ("shapes", ParamVal::Integer(1)),
                ("convergence", num(0.7)),
            ],
            ("convergence", 0.5, 0.505),
        ),
        BuiltinMimo::BuzzMatched => (
            vec![("coherence_floor", num(0.3)), ("convergence", num(0.9))],
            ("convergence", 0.5, 0.505),
        ),
        BuiltinMimo::MinimumDrive => (
            vec![
                ("tradeoff", choice("fixed")),
                ("tradeoff_value", num(0.1)),
                ("shortfall_db", num(1.0)),
                ("residual", choice("absolute")),
                ("convergence", num(0.7)),
            ],
            ("convergence", 0.5, 0.505),
        ),
        BuiltinMimo::BandAverage => (
            vec![
                ("oct_order", ParamVal::Integer(3)),
                ("kappa_rule", choice("knee")),
                ("drive_reduction", num(0.3)),
                ("use_tf_update", ParamVal::Toggle(true)),
                ("cross_terms", choice("reference")),
            ],
            ("gain_correction_rate", 0.25, 0.2525),
        ),
    }
}

fn host_param(key: &str, v: &ParamVal) -> Param {
    Param::new(
        key,
        match v {
            ParamVal::Number(x) => ParamValue::Number(*x),
            ParamVal::Integer(x) => ParamValue::Integer(*x),
            ParamVal::Toggle(x) => ParamValue::Toggle(*x),
            ParamVal::Choice(x) => ParamValue::Choice(x.clone()),
        },
    )
}

/// A synthetic run where the closed-loop laws have work to do: the 64-line
/// random plant, the estimate 5 percent off it.
fn run_for_parity(seed: u64, nc: usize, nd: usize) -> Synthetic {
    let mut s = Synthetic::random(seed, nc, nd, 64);
    s.frf = perturbed(&s.plant, seed, 0.05);
    s
}

#[test]
fn every_built_in_law_agrees_native_and_as_a_law_file() {
    let mut worst = 0.0f64;
    for law in BuiltinMimo::ALL {
        for (nc, nd, seed) in [(3, 2, 21u64), (6, 4, 22)] {
            let s = run_for_parity(seed, nc, nd);
            for (label, given) in [
                ("defaults", Vec::new()),
                ("non-default", non_default(law).0),
            ] {
                let resolved = law.spec().resolve(&given).expect("valid parameters");
                let mut native = law.build(&s.setup, &resolved).expect("builds natively");
                let params: Vec<Param> = given.iter().map(|(k, v)| host_param(k, v)).collect();
                let mut file = wasm_law(file_of(law), &s.setup, &params);
                let p = parity(native.as_mut(), &mut file, &s, 10);
                println!("{law:?} {nc} x {nd} {label}: {p:?}");
                assert!(p.drive_scale > 0.0, "{law:?}: nothing was driven");
                assert!(
                    p.drive <= PARITY,
                    "{law:?} {nc} x {nd} {label}: drive {p:?}"
                );
                assert!(
                    p.diagnostics <= PARITY,
                    "{law:?} {nc} x {nd} {label}: {p:?}"
                );
                worst = worst.max(p.drive).max(p.diagnostics);
            }
        }
    }
    println!("worst relative difference, native against law file, every law: {worst:.3e}");
}

#[test]
fn a_one_percent_change_of_each_laws_parameter_is_detected() {
    for law in BuiltinMimo::ALL {
        let (base, (key, a, b)) = non_default(law);
        let s = run_for_parity(31, 6, 4);
        let with = |v: f64| -> Vec<(&str, ParamVal)> {
            let mut p: Vec<(&str, ParamVal)> =
                base.iter().filter(|(k, _)| *k != key).cloned().collect();
            p.push((key, ParamVal::Number(v)));
            p
        };
        let resolved = law.spec().resolve(&with(a)).expect("valid");
        let mut native = law.build(&s.setup, &resolved).expect("builds");
        let params: Vec<Param> = with(b).iter().map(|(k, v)| host_param(k, v)).collect();
        let mut file = wasm_law(file_of(law), &s.setup, &params);
        let p = parity(native.as_mut(), &mut file, &s, 5);
        println!("{law:?}, {key} {a} against {b}: {p:?}");
        assert!(
            p.drive > 1e3 * PARITY,
            "{law:?}: a 1% change of {key} was not seen: {p:?}"
        );
    }
}
