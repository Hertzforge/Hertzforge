//! The MIMO Mixed check's records and the hardware rule of a role.
//!
//! A law file passes the MIMO Mixed check in a role (floor or tones) with the
//! built-in in the other. Its result is kept beside the file as a further
//! check record of the catalog, one per role
//! (`<sha256>.mimo-mixed-floor.check.json` and
//! `<sha256>.mimo-mixed-tones.check.json`), named by the file's SHA-256 so it
//! can never vouch for other bytes. A record clears the law in its role only
//! at the check in force: the version this code writes and the case list it
//! runs.

use crate::check::MimoMixedCheckCase;
use hertzforge_mimo_mixed::service::LawRole;
use hertzforge_mimo_sine_laws::MIMO_SINE_CHECK_VERSION;
use hertzforge_random_laws::{
    may_arm_on_hardware, ArmRefusal, CatalogEntry, CheckCase, CheckRecord, CheckStatus, LawCatalog,
    LawWorld, MIMO_CHECK_VERSION,
};

/// The version of the MIMO Mixed check written into its records. A change to
/// the cases, their stations or their requirements changes it, and a law
/// checked before must be checked again.
pub const MIMO_MIXED_CHECK_VERSION: &str = "mimo-mixed-check 2";

/// The catalog's name for the record of the MIMO Mixed check in `role`.
pub fn mimo_mixed_check_name(role: LawRole) -> &'static str {
    match role {
        LawRole::Floor => "mimo-mixed-floor",
        LawRole::Tones => "mimo-mixed-tones",
    }
}

/// The world a law of `role` is from: the floor's is `mimo`, the tones'
/// `sine_mimo`.
pub fn role_world(role: LawRole) -> LawWorld {
    match role {
        LawRole::Floor => LawWorld::Mimo,
        LawRole::Tones => LawWorld::SineMimo,
    }
}

/// The version a law of `role` must carry on its own world's check.
pub fn own_check_version(role: LawRole) -> &'static str {
    match role {
        LawRole::Floor => MIMO_CHECK_VERSION,
        LawRole::Tones => MIMO_SINE_CHECK_VERSION,
    }
}

/// Why a law may not drive hardware in a MIMO Mixed Mode role.
#[derive(Debug, Clone, PartialEq)]
pub enum RoleRefusal {
    /// A law of another world offered for the role.
    WrongWorld {
        /// The world the role takes.
        wants: LawWorld,
        /// The law's world.
        got: LawWorld,
    },
    /// Its own world's check.
    OwnCheck(ArmRefusal),
    /// The MIMO Mixed check in this role.
    MixedCheck(ArmRefusal),
}

impl RoleRefusal {
    /// The refusal of the check that stands in the way (none for a law of the
    /// wrong world).
    pub fn refusal(&self) -> Option<&ArmRefusal> {
        match self {
            RoleRefusal::OwnCheck(r) | RoleRefusal::MixedCheck(r) => Some(r),
            RoleRefusal::WrongWorld { .. } => None,
        }
    }
}

impl std::fmt::Display for RoleRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let which = match self {
            RoleRefusal::WrongWorld { wants, got } => {
                return write!(
                    f,
                    "the role takes a {} law, not a {} law",
                    wants.as_str(),
                    got.as_str()
                )
            }
            RoleRefusal::OwnCheck(_) => "its own check",
            RoleRefusal::MixedCheck(_) => "the MIMO Mixed check in this role",
        };
        match self.refusal().expect("a check refusal") {
            ArmRefusal::Unchecked => write!(f, "{which} has not been run"),
            ArmRefusal::CheckFailed { date, failed_cases } => write!(
                f,
                "{which} did not pass on {date} ({})",
                if failed_cases.is_empty() {
                    "no cases ran".to_owned()
                } else {
                    failed_cases.join(", ")
                }
            ),
            ArmRefusal::CheckOutdated {
                tool_version,
                current,
            } => write!(
                f,
                "{which} passed {tool_version}, which {current} has replaced"
            ),
            ArmRefusal::CheckUnreadable(why) => {
                write!(f, "{which} has a record that cannot be used ({why})")
            }
        }
    }
}

impl std::error::Error for RoleRefusal {}

/// The record of a law file's MIMO Mixed check: `cases` in the order the
/// check ran them, each named by its case's record name. The required cases
/// make the verdict; the informational ones
/// ([`MimoMixedCheckCase::INFORMATIONAL`]) are recorded beside it. A case
/// of no known name counts as required, so it can never clear a law.
pub fn mimo_mixed_check_record(law_sha256: &str, date: &str, cases: Vec<CheckCase>) -> CheckRecord {
    let (informational, required): (Vec<_>, Vec<_>) = cases
        .into_iter()
        .partition(|c| MimoMixedCheckCase::from_name(&c.name).is_some_and(|k| !k.required()));
    CheckRecord::new(law_sha256, MIMO_MIXED_CHECK_VERSION, date, required)
        .with_informational(informational)
}

/// Write `record` as the law's MIMO Mixed check in `role`.
pub fn record_mimo_mixed_check(
    catalog: &mut LawCatalog,
    role: LawRole,
    record: &CheckRecord,
) -> Result<(), String> {
    catalog
        .record_named_check(mimo_mixed_check_name(role), record)
        .map_err(|e| e.to_string())
}

/// Where the law in `entry` stands on the MIMO Mixed check in `role`.
pub fn mimo_mixed_check_status(
    catalog: &LawCatalog,
    entry: &CatalogEntry,
    role: LawRole,
) -> CheckStatus {
    if entry.builtin {
        return CheckStatus::BuiltIn;
    }
    catalog.named_check(&entry.identity.sha256, mimo_mixed_check_name(role))
}

/// Whether the law in `entry` may drive hardware in `role` of a MIMO Mixed
/// Mode run. A law of the other world never may. A built-in law of the
/// role's world always may. An imported law needs:
///
/// - its own world's check passed for its exact file, at the check in force
///   ([`own_check_version`]: a `mimo` law's record of [`MIMO_CHECK_VERSION`],
///   a `sine_mimo` law's of [`MIMO_SINE_CHECK_VERSION`]);
/// - the MIMO Mixed check in this role passed for its exact file, at
///   [`MIMO_MIXED_CHECK_VERSION`] and over the cases the check requires
///   now ([`MimoMixedCheckCase::REQUIRED`], by name and order); the
///   informational cases take no part.
///
/// A record for the other role clears nothing here. The simulator runs any
/// law; this rule is for hardware only.
pub fn may_arm_in_role(
    catalog: &LawCatalog,
    entry: &CatalogEntry,
    role: LawRole,
) -> Result<(), RoleRefusal> {
    let wants = role_world(role);
    if entry.world != wants {
        return Err(RoleRefusal::WrongWorld {
            wants,
            got: entry.world,
        });
    }
    if entry.builtin {
        return Ok(());
    }
    may_arm_on_hardware(entry).map_err(RoleRefusal::OwnCheck)?;
    let current = own_check_version(role);
    if let CheckStatus::Checked(r) = &entry.check {
        if r.tool_version != current {
            return Err(RoleRefusal::OwnCheck(ArmRefusal::CheckOutdated {
                tool_version: r.tool_version.clone(),
                current: current.into(),
            }));
        }
    }
    role_record_clears(
        &mimo_mixed_check_status(catalog, entry, role),
        &entry.identity.sha256,
    )
    .map_err(RoleRefusal::MixedCheck)
}

/// Whether a MIMO Mixed check `status` clears the file `law_sha256`.
pub fn role_record_clears(status: &CheckStatus, law_sha256: &str) -> Result<(), ArmRefusal> {
    let record = match status {
        CheckStatus::BuiltIn => return Ok(()),
        CheckStatus::Unchecked => return Err(ArmRefusal::Unchecked),
        CheckStatus::Unreadable(why) => return Err(ArmRefusal::CheckUnreadable(why.clone())),
        CheckStatus::Checked(r) => r,
    };
    record.validate().map_err(ArmRefusal::CheckUnreadable)?;
    if record.law_sha256 != law_sha256 {
        return Err(ArmRefusal::CheckUnreadable(format!(
            "the record is for the law file {}",
            record.law_sha256
        )));
    }
    let names_now = MimoMixedCheckCase::REQUIRED.iter().map(|c| c.name());
    let same_cases = record.cases.len() == MimoMixedCheckCase::REQUIRED.len()
        && record.cases.iter().map(|c| c.name.as_str()).eq(names_now);
    if record.tool_version != MIMO_MIXED_CHECK_VERSION || !same_cases {
        return Err(ArmRefusal::CheckOutdated {
            tool_version: record.tool_version.clone(),
            current: MIMO_MIXED_CHECK_VERSION.into(),
        });
    }
    if record.passed {
        Ok(())
    } else {
        Err(ArmRefusal::CheckFailed {
            date: record.date.clone(),
            failed_cases: record
                .cases
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.name.clone())
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    fn cases(pass: bool) -> Vec<CheckCase> {
        MimoMixedCheckCase::REQUIRED
            .iter()
            .map(|c| CheckCase {
                name: c.name().into(),
                passed: pass || *c != MimoMixedCheckCase::TransportDelay,
                detail: String::new(),
            })
            .collect()
    }

    fn checked(r: CheckRecord) -> CheckStatus {
        CheckStatus::Checked(r)
    }

    #[test]
    fn mimo_mixed_a_record_clears_only_at_the_check_in_force() {
        let good = mimo_mixed_check_record(SHA, "2026-11-21T10:00:00Z", cases(true));
        assert_eq!(role_record_clears(&checked(good.clone()), SHA), Ok(()));
        assert_eq!(
            role_record_clears(&CheckStatus::Unchecked, SHA),
            Err(ArmRefusal::Unchecked)
        );
        let failed = mimo_mixed_check_record(SHA, "2026-11-21T10:00:00Z", cases(false));
        assert!(matches!(
            role_record_clears(&checked(failed), SHA),
            Err(ArmRefusal::CheckFailed { failed_cases, .. }) if failed_cases == ["transport_delay"]
        ));
        let mut old = good.clone();
        old.tool_version = "mimo-mixed-check 0".into();
        assert!(matches!(
            role_record_clears(&checked(old), SHA),
            Err(ArmRefusal::CheckOutdated { .. })
        ));
        let mut fewer = good.clone();
        fewer.cases.pop();
        assert!(matches!(
            role_record_clears(&checked(fewer), SHA),
            Err(ArmRefusal::CheckOutdated { .. })
        ));
        let mut moved = good.clone();
        moved.cases.swap(0, 1);
        assert!(matches!(
            role_record_clears(&checked(moved), SHA),
            Err(ArmRefusal::CheckOutdated { .. })
        ));
        assert!(matches!(
            role_record_clears(&checked(good), &"0".repeat(64)),
            Err(ArmRefusal::CheckUnreadable(_))
        ));
    }

    /// Requirement: the swept-tone case is run and recorded but is not
    /// required: a law that fails only it clears, and the record keeps the
    /// result; a record that lists it among the required cases (the check
    /// before this version) does not clear.
    #[test]
    fn mimo_mixed_the_swept_tone_case_is_recorded_but_not_required() {
        let all: Vec<CheckCase> = MimoMixedCheckCase::ALL
            .iter()
            .map(|c| CheckCase {
                name: c.name().into(),
                passed: *c != MimoMixedCheckCase::SweptToneThroughMode,
                detail: String::new(),
            })
            .collect();
        let rec = mimo_mixed_check_record(SHA, "2026-11-21T10:00:00Z", all.clone());
        assert!(rec.passed);
        assert_eq!(rec.cases.len(), 9);
        assert_eq!(rec.informational.len(), 1);
        assert_eq!(rec.informational[0].name, "swept_tone_through_mode");
        assert!(!rec.informational[0].passed);
        assert_eq!(role_record_clears(&checked(rec), SHA), Ok(()));
        let mut old = CheckRecord::new(SHA, MIMO_MIXED_CHECK_VERSION, "2026-11-21T10:00:00Z", all);
        old.passed = old.cases.iter().all(|c| c.passed);
        assert!(matches!(
            role_record_clears(&checked(old), SHA),
            Err(ArmRefusal::CheckOutdated { .. })
        ));
        // A required case failed still fails, whatever the informational say.
        let mut some: Vec<CheckCase> = cases(false);
        some.push(CheckCase {
            name: "swept_tone_through_mode".into(),
            passed: true,
            detail: String::new(),
        });
        let rec = mimo_mixed_check_record(SHA, "2026-11-21T10:00:00Z", some);
        assert!(!rec.passed);
        assert!(matches!(
            role_record_clears(&checked(rec), SHA),
            Err(ArmRefusal::CheckFailed { .. })
        ));
    }

    #[test]
    fn mimo_mixed_a_role_refusal_says_what_stands_in_the_way() {
        assert_eq!(
            RoleRefusal::OwnCheck(ArmRefusal::Unchecked).to_string(),
            "its own check has not been run"
        );
        assert_eq!(
            RoleRefusal::MixedCheck(ArmRefusal::CheckOutdated {
                tool_version: "mimo-mixed-check 0".into(),
                current: MIMO_MIXED_CHECK_VERSION.into(),
            })
            .to_string(),
            "the MIMO Mixed check in this role passed mimo-mixed-check 0, which \
             mimo-mixed-check 2 has replaced"
        );
        assert_eq!(
            RoleRefusal::WrongWorld {
                wants: LawWorld::Mimo,
                got: LawWorld::Single,
            }
            .to_string(),
            format!(
                "the role takes a {} law, not a {} law",
                LawWorld::Mimo.as_str(),
                LawWorld::Single.as_str()
            )
        );
    }

    #[test]
    fn mimo_mixed_each_role_has_its_own_record_name_world_and_check() {
        assert_eq!(mimo_mixed_check_name(LawRole::Floor), "mimo-mixed-floor");
        assert_eq!(mimo_mixed_check_name(LawRole::Tones), "mimo-mixed-tones");
        assert_eq!(role_world(LawRole::Floor), LawWorld::Mimo);
        assert_eq!(role_world(LawRole::Tones), LawWorld::SineMimo);
        assert_eq!(own_check_version(LawRole::Floor), "mimo-check 1");
        assert_eq!(own_check_version(LawRole::Tones), "mimo-sine-check 3");
    }
}
