//! Each built-in law resolved natively (the engine's law, by name, through the
//! SDK's declarations) and as its law file gives the same drives, within 1e-12
//! of each record's peak, over a scripted run that holds and steps the level, for
//! every target kind the law accepts.

#![cfg(feature = "reference-plant")]

use hertzforge_mimo_shock::TargetKind;
use hertzforge_mimo_shock_laws::parity::bench::{law_file_against, Bench};
use hertzforge_mimo_shock_laws::{resolve_builtin, BuiltinMimoShock};

#[test]
fn mimo_shock_each_builtin_natively_and_as_its_law_file_gives_the_same_drives() {
    for b in BuiltinMimoShock::ALL {
        let file = format!("hertzforge_law_mimo_shock_{}", b.id());
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
