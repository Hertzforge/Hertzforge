//! The hardware rule of a MIMO Mixed Mode role, on a catalog of its own.

use hertzforge_law_host::LawIdentity;
use hertzforge_mimo_mixed_laws::*;
use hertzforge_random_laws::{
    classical_builtin_manifest, ArmRefusal, CatalogEntry, CheckCase, CheckRecord, CheckStatus,
    LawCatalog, LawWorld,
};

const SHA: &str = "cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34";
const DATE: &str = "2026-11-21T10:00:00Z";

fn entry(world: LawWorld, builtin: bool, check: CheckStatus) -> CatalogEntry {
    CatalogEntry {
        id: "a-law".into(),
        identity: LawIdentity {
            name: "a-law".into(),
            version: "1".into(),
            author: String::new(),
            sha256: SHA.into(),
        },
        manifest: classical_builtin_manifest(),
        builtin,
        check,
        world,
        mimo: None,
        live: Vec::new(),
    }
}

fn own(version: &str) -> CheckStatus {
    let case = CheckCase {
        name: "a".into(),
        passed: true,
        detail: String::new(),
    };
    CheckStatus::Checked(CheckRecord::new(SHA, version, DATE, vec![case]))
}

fn passing() -> CheckRecord {
    let cases = MimoMixedCheckCase::ALL
        .iter()
        .map(|c| CheckCase {
            name: c.name().into(),
            passed: true,
            detail: String::new(),
        })
        .collect();
    mimo_mixed_check_record(SHA, DATE, cases)
}

/// Requirement: a role takes laws of its own world only; a built-in of that
/// world always arms; an imported law needs its own world's check at the
/// version in force and the MIMO Mixed check in that role, and a record for
/// the other role clears nothing.
#[test]
fn mimo_mixed_a_law_arms_in_a_role_only_on_both_checks() {
    let dir = std::env::temp_dir().join(format!("mimo-mixed-arm-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("laws")).unwrap();
    let mut cat = LawCatalog::open(&dir).unwrap();
    let floor = LawRole::Floor;
    let tones = LawRole::Tones;
    // Worlds.
    let b = entry(LawWorld::Mimo, true, CheckStatus::BuiltIn);
    assert_eq!(may_arm_in_role(&cat, &b, floor), Ok(()));
    assert!(matches!(
        may_arm_in_role(&cat, &b, tones),
        Err(RoleRefusal::WrongWorld { .. })
    ));
    let single = entry(LawWorld::Single, true, CheckStatus::BuiltIn);
    assert!(matches!(
        may_arm_in_role(&cat, &single, floor),
        Err(RoleRefusal::WrongWorld { .. })
    ));
    // Its own check.
    let unchecked = entry(LawWorld::Mimo, false, CheckStatus::Unchecked);
    assert_eq!(
        may_arm_in_role(&cat, &unchecked, floor),
        Err(RoleRefusal::OwnCheck(ArmRefusal::Unchecked))
    );
    let old = entry(LawWorld::Mimo, false, own("mimo-check 0"));
    assert!(matches!(
        may_arm_in_role(&cat, &old, floor),
        Err(RoleRefusal::OwnCheck(ArmRefusal::CheckOutdated { .. }))
    ));
    // Its own check passed; the MIMO Mixed check not yet run.
    let f = entry(LawWorld::Mimo, false, own(own_check_version(floor)));
    assert_eq!(
        may_arm_in_role(&cat, &f, floor),
        Err(RoleRefusal::MixedCheck(ArmRefusal::Unchecked))
    );
    // Passed in the floor role: the floor arms, the tones role does not.
    record_mimo_mixed_check(&mut cat, floor, &passing()).unwrap();
    assert_eq!(may_arm_in_role(&cat, &f, floor), Ok(()));
    let t = entry(LawWorld::SineMimo, false, own(own_check_version(tones)));
    assert_eq!(
        may_arm_in_role(&cat, &t, tones),
        Err(RoleRefusal::MixedCheck(ArmRefusal::Unchecked))
    );
    record_mimo_mixed_check(&mut cat, tones, &passing()).unwrap();
    assert_eq!(may_arm_in_role(&cat, &t, tones), Ok(()));
    std::fs::remove_dir_all(&dir).unwrap();
}
