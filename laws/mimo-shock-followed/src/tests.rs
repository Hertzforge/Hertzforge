//! The law file, run through the law host, gives the drives of the shared
//! `followed` mathematics called natively, within 1e-12 of each record's peak,
//! over a run that holds and steps the level, at its defaults and with the
//! ceiling moved; and the law file at its defaults is not the law with the
//! ceiling moved.

use hertzforge_law_host::{Param, ParamValue};
use hertzforge_law_sdk::shock_mimo::builtin::FollowedConfig;
use hertzforge_law_sdk::shock_mimo::math::followed::{FollowedLaw, FollowedOptions};
use hertzforge_mimo_shock::TargetKind;
use hertzforge_mimo_shock_laws::parity::bench::{law_file_against, Bench};
use hertzforge_mimo_shock_laws::parity::math_setup;
use hertzforge_mimo_shock_laws::MathLaw;

const FILE: &str = "hertzforge_law_mimo_shock_followed";

fn sets() -> Vec<(&'static str, Vec<Param>, FollowedConfig)> {
    let mut wide = FollowedConfig::followed();
    wide.options.ceiling = 0.02;
    vec![
        ("defaults", Vec::new(), FollowedConfig::followed()),
        (
            "ceiling 2 %",
            vec![Param {
                key: "ceiling_pct".into(),
                value: ParamValue::Number(2.0),
            }],
            wide,
        ),
    ]
}

#[test]
fn mimo_shock_followed_defaults_are_the_law_math_followed() {
    assert_eq!(
        FollowedConfig::followed().options,
        FollowedOptions::followed()
    );
}

#[test]
fn mimo_shock_followed_law_file_gives_the_native_drives() {
    for (what, file_params, config) in sets() {
        let bench = Bench::four_by_four(TargetKind::Srs);
        let mut native = MathLaw::followed_with(&bench.setup, &config).unwrap();
        let r = law_file_against(FILE, file_params, &mut native, &bench).unwrap();
        println!("followed {what}: {r:?}");
        assert_eq!(r.calls, 6);
        assert!(r.same_keys, "{r:?}");
        assert!(r.worst_rel <= 1e-12, "{what}: {r:?}");
        assert!(r.worst_value_rel <= 1e-12, "{what}: {r:?}");
    }
}

#[test]
fn mimo_shock_followed_law_file_is_the_law_math_constructor() {
    let bench = Bench::four_by_four(TargetKind::Srs);
    let law = FollowedLaw::followed(math_setup(&bench.setup)).unwrap();
    let mut native = MathLaw::new("followed", &bench.setup, Box::new(law));
    let r = law_file_against(FILE, Vec::new(), &mut native, &bench).unwrap();
    println!("followed against FollowedLaw::followed: {r:?}");
    assert_eq!(r.calls, 6);
    assert!(r.worst_rel <= 1e-12, "{r:?}");
}

#[test]
fn mimo_shock_followed_parity_sees_the_ceiling_the_law_file_ignores() {
    let bench = Bench::four_by_four(TargetKind::Srs);
    for (what, _, config) in sets().into_iter().skip(1) {
        let mut native = MathLaw::followed_with(&bench.setup, &config).unwrap();
        let r = law_file_against(FILE, Vec::new(), &mut native, &bench).unwrap();
        assert!(
            r.worst_rel > 1e-6 || r.worst_value_rel > 1e-6,
            "{what}: {r:?}"
        );
    }
}
