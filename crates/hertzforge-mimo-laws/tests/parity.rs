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
