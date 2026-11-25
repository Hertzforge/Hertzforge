//! Each built-in law resolved natively (the engine's law, by name, through the
//! SDK's declarations) and as its law file gives the same drives, within 1e-12
//! of each record's peak, over a scripted run that holds and steps the level, for
//! every target kind the law accepts.

#![cfg(feature = "reference-plant")]

use hertzforge_law_sdk::shock_mimo::math::followed::{FollowedLaw, FollowedOptions};
use hertzforge_mimo_shock::TargetKind;
use hertzforge_mimo_shock_laws::parity::bench::{law_file_against, Bench, PARITY_LEVELS};
use hertzforge_mimo_shock_laws::parity::{compare_laws, math_setup};
use hertzforge_mimo_shock_laws::{resolve_builtin, BuiltinMimoShock, MathLaw};

#[test]
fn mimo_shock_each_builtin_natively_and_as_its_law_file_gives_the_same_drives() {
    for b in BuiltinMimoShock::ALL {
        let file = format!("hertzforge_law_mimo_shock_{}", b.id().replace('-', "_"));
        for &kind in b.targets() {
            let bench = Bench::four_by_four(kind);
            let mut native = resolve_builtin(b.id())
                .unwrap()
                .build(&bench.setup)
                .unwrap();
            let r = law_file_against(&file, Vec::new(), native.as_mut(), &bench)
                .unwrap_or_else(|e| panic!("{} ({}): {e}", b.id(), kind.as_str()));
            println!(
                "{} ({}): {} calls, worst {:.2e} of the peak",
                b.id(),
                kind.as_str(),
                r.calls,
                r.worst_rel
            );
            assert!(r.calls >= 6);
            assert!(
                r.worst_rel <= 1e-12,
                "{} ({}): {r:?}",
                b.id(),
                kind.as_str()
            );
            let _ = TargetKind::ALL;
        }
    }
}

/// `followed` and `followed-hold` chosen by name, as a run chooses them, give
/// the drives of the law mathematics' own constructors, within 1e-12 of each
/// record's peak: `followed` exactly as `FollowedLaw::followed`, and
/// `followed-hold` as `FollowedLaw::followed_hold` with its probe check on
/// the run's first two levels.
#[test]
fn mimo_shock_the_followed_laws_by_name_are_the_law_math() {
    let bench = Bench::four_by_four(TargetKind::Srs);
    let mut hold = FollowedOptions::followed_hold();
    hold.probe_check.as_mut().unwrap().probes_db = bench.setup.ladder_db[..2].to_vec();
    for (id, options) in [
        ("followed", FollowedOptions::followed()),
        ("followed-hold", hold),
    ] {
        let mut product = resolve_builtin(id).unwrap().build(&bench.setup).unwrap();
        let law = FollowedLaw::new(math_setup(&bench.setup), options).unwrap();
        let mut math = MathLaw::new(id, &bench.setup, Box::new(law));
        let r = compare_laws(
            product.as_mut(),
            &mut math,
            &bench.setup,
            &PARITY_LEVELS,
            &mut |d, l| bench.respond(d, l),
        )
        .unwrap();
        println!(
            "{id} by name against the law math: {} calls, worst {:.2e} of the peak on call {}",
            r.calls, r.worst_rel, r.worst_call
        );
        assert_eq!(r.calls, 6);
        assert!(r.worst_rel <= 1e-12, "{id}: {r:?}");
    }
}
