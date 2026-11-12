//! The law file, run through the law host, gives the drives of the shared
//! `inverse` mathematics called natively, within 1e-12 of each record's peak,
//! over a run that holds and steps the level, for both target kinds and every
//! option that changes the mathematics.

use hertzforge_law_host::{Param, ParamValue};
use hertzforge_law_sdk::shock_mimo::builtin::{InverseConfig, ModelChoice};
use hertzforge_law_sdk::shock_mimo::math::inverse::InverseParams;
use hertzforge_mimo_shock::TargetKind;
use hertzforge_mimo_shock_laws::parity::bench::{law_file_against, Bench};
use hertzforge_mimo_shock_laws::MathLaw;

const FILE: &str = "hertzforge_law_mimo_shock_inverse";

fn sets() -> Vec<(&'static str, Vec<Param>, InverseParams)> {
    let p = |key: &str, value| Param {
        key: key.into(),
        value,
    };
    let d = InverseParams::default;
    vec![
        ("defaults", Vec::new(), d()),
        (
            "refresh",
            vec![
                p("refresh", ParamValue::Toggle(true)),
                p("refresh_mu", ParamValue::Number(0.75)),
            ],
            InverseParams {
                refresh: true,
                refresh_mu: 0.75,
                ..d()
            },
        ),
        (
            "align",
            vec![
                p("align", ParamValue::Toggle(true)),
                p("align_max_ms", ParamValue::Number(1.0)),
            ],
            InverseParams {
                align: true,
                align_max_s: 1.0e-3,
                ..d()
            },
        ),
        (
            "no watchdog, half gain, more regularization",
            vec![
                p("watchdog", ParamValue::Toggle(false)),
                p("gain", ParamValue::Number(0.5)),
                p("lambda_rel", ParamValue::Number(1.0e-2)),
            ],
            InverseParams {
                watchdog: false,
                gain: 0.5,
                lambda_rel: 1.0e-2,
                ..d()
            },
        ),
    ]
}

#[test]
fn mimo_shock_inverse_law_file_gives_the_native_drives() {
    for kind in [TargetKind::Waveform, TargetKind::Srs] {
        for (what, file_params, native_params) in sets() {
            let bench = Bench::four_by_four(kind);
            let mut native = MathLaw::inverse(&bench.setup, native_params).unwrap();
            let r = law_file_against(FILE, file_params, &mut native, &bench).unwrap();
            println!("inverse {kind:?} {what}: {r:?}");
            assert_eq!(r.calls, 6);
            assert!(r.same_keys, "{r:?}");
            assert!(r.worst_rel <= 1e-12, "{kind:?} {what}: {r:?}");
            assert!(r.worst_value_rel <= 1e-12, "{kind:?} {what}: {r:?}");
        }
    }
}

#[test]
fn mimo_shock_inverse_parity_sees_each_option_the_law_file_ignores() {
    let bench = Bench::four_by_four(TargetKind::Waveform);
    for (what, _, native_params) in sets().into_iter().skip(1) {
        let mut native = MathLaw::inverse(&bench.setup, native_params).unwrap();
        let r = law_file_against(FILE, Vec::new(), &mut native, &bench).unwrap();
        assert!(r.worst_rel > 1e-6, "{what}: {r:?}");
    }
}

fn model_sets() -> Vec<(&'static str, Vec<Param>, InverseConfig)> {
    let p = |key: &str, value| Param {
        key: key.into(),
        value,
    };
    let with = |m: ModelChoice, step: bool| {
        let mut c = InverseConfig::default();
        c.model.model = m;
        c.predicted_step = step;
        c
    };
    let mut v = Vec::new();
    for m in [
        ModelChoice::Secant,
        ModelChoice::Track,
        ModelChoice::TrackSecant,
    ] {
        v.push((
            m.name(),
            vec![p("model", ParamValue::Choice(m.name().into()))],
            with(m, false),
        ));
    }
    v.push((
        "track_secant with the predicted step",
        vec![
            p("model", ParamValue::Choice("track_secant".into())),
            p("predicted_step", ParamValue::Toggle(true)),
        ],
        with(ModelChoice::TrackSecant, true),
    ));
    let mut c = with(ModelChoice::Track, true);
    c.model.from_identification = true;
    c.model.modes = 4;
    v.push((
        "track, 4 modes, from the identification, predicted step",
        vec![
            p("model", ParamValue::Choice("track".into())),
            p("modes", ParamValue::Integer(4)),
            p("track_from_identification", ParamValue::Toggle(true)),
            p("predicted_step", ParamValue::Toggle(true)),
        ],
        c,
    ));
    v
}

#[test]
fn mimo_shock_inverse_model_options_law_file_gives_the_native_drives() {
    for kind in [TargetKind::Waveform, TargetKind::Srs] {
        for (what, file_params, config) in model_sets() {
            let bench = Bench::four_by_four(kind);
            let mut native = MathLaw::inverse_with(&bench.setup, &config).unwrap();
            let r = law_file_against(FILE, file_params, &mut native, &bench).unwrap();
            println!("inverse {what} ({}): {r:?}", kind.as_str());
            assert_eq!(r.calls, 6);
            assert!(r.same_keys, "{r:?}");
            assert!(r.worst_rel <= 1e-12, "{what}: {r:?}");
            assert!(r.worst_value_rel <= 1e-12, "{what}: {r:?}");
            let mut native = MathLaw::inverse_with(&bench.setup, &config).unwrap();
            let r = law_file_against(FILE, Vec::new(), &mut native, &bench).unwrap();
            assert!(
                r.worst_rel > 1e-6 || r.worst_value_rel > 1e-6,
                "{what}: {r:?}"
            );
        }
    }
}
