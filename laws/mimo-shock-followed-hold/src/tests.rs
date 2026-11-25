//! The law file, run through the law host, gives the drives of the shared
//! `followed-hold` mathematics called natively, within 1e-12 of each record's
//! peak, over a run that holds and steps the level, at its defaults and with
//! each protection off; and the law file at its defaults is not the law with
//! a protection off.

use hertzforge_law_host::{Param, ParamValue};
use hertzforge_law_sdk::shock_mimo::builtin::FollowedConfig;
use hertzforge_law_sdk::shock_mimo::math::followed::FollowedOptions;
use hertzforge_mimo_shock::TargetKind;
use hertzforge_mimo_shock_laws::parity::bench::{law_file_against, Bench};
use hertzforge_mimo_shock_laws::MathLaw;

const FILE: &str = "hertzforge_law_mimo_shock_followed_hold";

fn off(key: &str) -> Vec<Param> {
    vec![Param {
        key: key.into(),
        value: ParamValue::Toggle(false),
    }]
}

fn sets() -> Vec<(&'static str, Vec<Param>, FollowedConfig)> {
    let with = |f: fn(&mut FollowedOptions)| {
        let mut c = FollowedConfig::followed_hold();
        f(&mut c.options);
        c
    };
    vec![
        ("defaults", Vec::new(), FollowedConfig::followed_hold()),
        (
            "no probe check",
            off("probe_check"),
            with(|o| o.probe_check = None),
        ),
        (
            "probe limit 1 dB",
            vec![Param {
                key: "probe_limit_db".into(),
                value: ParamValue::Number(1.0),
            }],
            with(|o| o.probe_check.as_mut().unwrap().limit_db = 1.0),
        ),
        (
            "no power bound",
            off("power_bound"),
            with(|o| o.resolved_power_gate = None),
        ),
        ("no hold", off("hold"), with(|o| o.hold_db = None)),
    ]
}

#[test]
fn mimo_shock_followed_hold_defaults_are_the_law_math_followed_hold_on_the_runs_probes() {
    let c = FollowedConfig::followed_hold();
    let ladder = [-40.0, -34.0, -12.0, -6.0, 0.0];
    // The math's own probes are the study's ladder's first two levels; on
    // that ladder the built-in is the math's law exactly.
    assert_eq!(c.options_for(&ladder), FollowedOptions::followed_hold());
    // On any other ladder the check moves to the run's first two levels.
    let o = c.options_for(&[-12.0, -9.0, -6.0, 0.0]);
    assert_eq!(o.probe_check.unwrap().probes_db, vec![-12.0, -9.0]);
}

#[test]
fn mimo_shock_followed_hold_law_file_gives_the_native_drives() {
    for (what, file_params, config) in sets() {
        let bench = Bench::four_by_four(TargetKind::Srs);
        let mut native = MathLaw::followed_with(&bench.setup, &config).unwrap();
        let r = law_file_against(FILE, file_params, &mut native, &bench).unwrap();
        println!("followed-hold {what}: {r:?}");
        assert_eq!(r.calls, 6);
        assert!(r.same_keys, "{r:?}");
        assert!(r.worst_rel <= 1e-12, "{what}: {r:?}");
        assert!(r.worst_value_rel <= 1e-12, "{what}: {r:?}");
    }
}

#[test]
fn mimo_shock_followed_hold_parity_sees_each_option_the_law_file_ignores() {
    let bench = Bench::four_by_four(TargetKind::Srs);
    for (what, _, config) in sets().into_iter().skip(1) {
        let mut native = MathLaw::followed_with(&bench.setup, &config).unwrap();
        let r = law_file_against(FILE, Vec::new(), &mut native, &bench).unwrap();
        println!("followed-hold defaults against {what}: {r:?}");
        assert!(
            r.worst_rel > 1e-6 || r.worst_value_rel > 1e-6,
            "{what}: {r:?}"
        );
    }
}
