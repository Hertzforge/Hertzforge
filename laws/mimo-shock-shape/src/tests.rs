//! The law file, run through the law host, gives the drives of the shared
//! `shape` mathematics called natively, within 1e-12 of each record's peak,
//! over a run that holds and steps the level, for every option that changes
//! the mathematics.

use hertzforge_law_host::{Param, ParamValue};
use hertzforge_law_sdk::shock_mimo::math::delay::LeastEffort;
use hertzforge_law_sdk::shock_mimo::math::shape::{Anchor, SeedTiming, ShapeParams};
use hertzforge_mimo_shock::TargetKind;
use hertzforge_mimo_shock_laws::parity::bench::{law_file_against, Bench};
use hertzforge_mimo_shock_laws::MathLaw;

const FILE: &str = "hertzforge_law_mimo_shock_shape";

fn sets() -> Vec<(&'static str, Vec<Param>, ShapeParams)> {
    let p = |key: &str, value| Param {
        key: key.into(),
        value,
    };
    let d = ShapeParams::default;
    vec![
        ("defaults", Vec::new(), d()),
        (
            "refresh",
            vec![p("refresh", ParamValue::Toggle(true))],
            ShapeParams {
                refresh: true,
                ..d()
            },
        ),
        (
            "measured anchor",
            vec![
                p("anchor", ParamValue::Choice("measured".into())),
                p("refresh", ParamValue::Toggle(true)),
                p("refresh_mu", ParamValue::Number(0.5)),
            ],
            ShapeParams {
                anchor: Anchor::Measured,
                refresh: true,
                refresh_mu: 0.5,
                ..d()
            },
        ),
        (
            "least-effort seed timing",
            vec![p("seed_timing", ParamValue::Choice("least_effort".into()))],
            ShapeParams {
                seed_timing: SeedTiming::LeastEffort(LeastEffort::default()),
                ..d()
            },
        ),
        (
            "step, delays and regularization",
            vec![
                p("max_step_db", ParamValue::Number(3.0)),
                p("delay_relax", ParamValue::Number(0.0)),
                p("delay_bound_ms", ParamValue::Number(1.0)),
                p("lambda_rel", ParamValue::Number(1.0e-2)),
            ],
            ShapeParams {
                max_step_db: 3.0,
                delay_relax: 0.0,
                delay_bound_s: 1.0e-3,
                lambda_rel: 1.0e-2,
                ..d()
            },
        ),
    ]
}

#[test]
fn mimo_shock_shape_law_file_gives_the_native_drives() {
    for (what, file_params, native_params) in sets() {
        let bench = Bench::four_by_four(TargetKind::Srs);
        let mut native = MathLaw::shape(&bench.setup, native_params).unwrap();
        let r = law_file_against(FILE, file_params, &mut native, &bench).unwrap();
        println!("shape {what}: {r:?}");
        assert_eq!(r.calls, 6);
        assert!(r.same_keys, "{r:?}");
        assert!(r.worst_rel <= 1e-12, "{what}: {r:?}");
        assert!(r.worst_value_rel <= 1e-12, "{what}: {r:?}");
    }
}

#[test]
fn mimo_shock_shape_parity_sees_each_option_the_law_file_ignores() {
    let bench = Bench::four_by_four(TargetKind::Srs);
    for (what, _, native_params) in sets().into_iter().skip(1) {
        let mut native = MathLaw::shape(&bench.setup, native_params).unwrap();
        let r = law_file_against(FILE, Vec::new(), &mut native, &bench).unwrap();
        assert!(
            r.worst_rel > 1e-6 || r.worst_value_rel > 1e-6,
            "{what}: {r:?}"
        );
    }
}
