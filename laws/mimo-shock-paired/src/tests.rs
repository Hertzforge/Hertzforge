//! The law file, run through the law host, gives the drives of the shared
//! `paired` mathematics called natively, within 1e-12 of each record's peak,
//! over a run that holds and steps the level, for both target kinds and two
//! parameter sets.

use crate::Paired;
use hertzforge_law_host::{Param, ParamValue};
use hertzforge_law_sdk::shock_mimo::builtin;
use hertzforge_law_sdk::shock_mimo::math::paired::PairedParams;
use hertzforge_law_sdk::ShockMimoLaw;
use hertzforge_mimo_shock::TargetKind;
use hertzforge_mimo_shock_laws::parity::bench::{law_file_against, Bench};
use hertzforge_mimo_shock_laws::MathLaw;

const FILE: &str = "hertzforge_law_mimo_shock_paired";

fn sets() -> Vec<(Vec<Param>, PairedParams)> {
    let p = |key: &str, value| Param {
        key: key.into(),
        value,
    };
    vec![
        (Vec::new(), PairedParams::diagonal(4, 4)),
        (
            vec![
                p("weight", ParamValue::Number(0.5)),
                p("smooth_lines", ParamValue::Integer(5)),
                p("max_step_db", ParamValue::Number(3.0)),
            ],
            PairedParams {
                weight: 0.5,
                smooth_lines: 5,
                max_step_db: 3.0,
                ..PairedParams::diagonal(4, 4)
            },
        ),
    ]
}

#[test]
fn mimo_shock_paired_law_file_gives_the_native_drives() {
    for kind in [TargetKind::Waveform, TargetKind::Srs] {
        for (k, (file_params, native_params)) in sets().into_iter().enumerate() {
            let bench = Bench::four_by_four(kind);
            let mut native = MathLaw::paired(&bench.setup, native_params).unwrap();
            let r = law_file_against(FILE, file_params, &mut native, &bench).unwrap();
            println!("paired {kind:?} set {k}: {r:?}");
            assert_eq!(r.calls, 6);
            assert!(r.same_keys, "{r:?}");
            assert!(r.worst_rel <= 1e-12, "{kind:?} set {k}: {r:?}");
            assert!(r.worst_value_rel <= 1e-12, "{kind:?} set {k}: {r:?}");
        }
    }
}

#[test]
fn mimo_shock_paired_parity_sees_a_parameter_the_law_file_ignores() {
    // The law file at its defaults against the native law at the second set:
    // the run must differ, or the parity test could not see a law file that
    // dropped its parameters.
    let bench = Bench::four_by_four(TargetKind::Waveform);
    let (_, other) = sets().remove(1);
    let mut native = MathLaw::paired(&bench.setup, other).unwrap();
    let r = law_file_against(FILE, Vec::new(), &mut native, &bench).unwrap();
    assert!(r.worst_rel > 1e-3, "{r:?}");
}

#[test]
fn mimo_shock_paired_declares_what_the_builtin_declaration_says() {
    assert_eq!(Paired::describe(), builtin::PAIRED.manifest());
}
