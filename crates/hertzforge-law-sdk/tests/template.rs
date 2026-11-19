//! The multiple-output law template (`template/mimo-open-loop`), its law compiled
//! natively and run through the SDK's trait, so the code a law author copies builds
//! and does what its documentation says. (Its law file is built and checked on the
//! simulator by the controller's tests and the template's README.)

#[path = "../template/mimo-open-loop/src/law.rs"]
mod law;

use hertzforge_law_sdk::mimo::*;
use hertzforge_law_sdk::{Param, ParamKind, ParamValue, Params, RandomMimoLaw};
use law::OpenLoop;

fn inputs<'a>(
    reference: &'a HermitianStack,
    tolerances: &'a Tolerances,
    frf: &'a MatrixStack,
    coherence: &'a [f64],
    band: (usize, usize),
) -> MimoLoopInputs<'a> {
    MimoLoopInputs {
        reference,
        tolerances,
        response: reference,
        response_dof: 64.0,
        frf,
        multiple_coherence: coherence,
        df_hz: 1.0,
        band,
        loop_index: 0,
        last_drive: None,
        response_drive_csd: None,
        sysid_response: None,
        sysid_drive: None,
        sysid_noise: None,
    }
}

#[test]
fn the_template_declares_its_parameters_shapes_and_diagnostics() {
    let m = OpenLoop::describe();
    let keys: Vec<&str> = m.law.params.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(keys, ["regularization", "amount"]);
    assert!(matches!(m.law.params[1].kind, ParamKind::Number(_)));
    assert_eq!(m.shapes.len(), 3);
    assert!(m.needs.is_empty());
    let diag: Vec<&str> = m.law.diagnostics.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(diag, ["condition", "kept"]);
}

#[test]
fn the_template_drive_is_the_regularized_pseudo_inverse_drive() {
    let (lines, band) = (8, (2, 6));
    let mut reference = HermitianStack::zeros(lines, 3);
    let mut frf = MatrixStack::zeros(lines, 3, 2);
    for line in 0..lines {
        for i in 0..3 {
            reference.set(line, i, i, C64::new(1e-2, 0.0));
        }
        reference.set(line, 0, 1, C64::new(2e-3, 1e-3));
        frf.set(line, 0, 0, C64::new(2.0, 0.0));
        frf.set(line, 1, 1, C64::new(1.0, 0.0));
        frf.set(line, 2, 0, C64::new(0.5, 0.5));
    }
    let setup = MimoSetup {
        controls: 3,
        drives: 2,
        lines,
        df_hz: 1.0,
        band,
        reference: reference.clone(),
        tolerances: Tolerances::none(lines, 3),
    };
    let coherence = vec![1.0; lines * 3];
    for (reg, amount, want_reg) in [
        (
            "truncated",
            1e-3,
            linalg::Regularization::Truncated { rcond: 1e-3 },
        ),
        (
            "tikhonov",
            0.05,
            linalg::Regularization::Tikhonov { lambda_rel: 0.05 },
        ),
    ] {
        let params = Params(vec![
            Param {
                key: "regularization".into(),
                value: ParamValue::Choice(reg.into()),
            },
            Param {
                key: "amount".into(),
                value: ParamValue::Number(amount),
            },
        ]);
        let mut law = OpenLoop::new(setup.clone(), params).expect("builds");
        let i = inputs(&reference, &setup.tolerances, &frf, &coherence, band);
        let seed = law.seed(&i).expect("seeds");
        let again = law.iterate(&i).expect("iterates");
        assert_eq!(
            seed.drive, again.drive,
            "open loop: the same drive every loop"
        );
        let (hp, _) = linalg::pinv(&frf, want_reg).unwrap();
        let want = linalg::congruence(&hp, &reference).unwrap();
        for line in 0..lines {
            if (band.0..band.1).contains(&line) {
                assert_eq!(seed.drive.line(line), want.line(line), "{reg} line {line}");
            } else {
                assert_eq!(seed.drive.trace(line), 0.0, "zero outside the band");
            }
        }
        assert_eq!(seed.diagnostics.len(), 2);
    }
    let bad = Params(vec![
        Param {
            key: "regularization".into(),
            value: ParamValue::Choice("sideways".into()),
        },
        Param {
            key: "amount".into(),
            value: ParamValue::Number(1e-3),
        },
    ]);
    assert!(OpenLoop::new(setup, bad).is_err());
}
