//! The mixed check's records and the hardware rule of a Mixed Mode role.
//!
//! A law file passes the mixed check in a role (floor or tones) with the
//! built-in in the other. Its result is kept beside the file as a further
//! check record of the catalog, one per role (`<sha256>.mixed-floor.check.json`
//! and `<sha256>.mixed-tones.check.json`), named by the file's SHA-256 so it can
//! never vouch for other bytes. A record clears the law in its role only at the
//! check in force: the version this code writes and the case list it runs.

use crate::check::MixedCheckCase;
use hertzforge_mixed::LawRole;
use hertzforge_random_laws::{
    may_arm_on_hardware, ArmRefusal, CatalogEntry, CheckCase, CheckRecord, CheckStatus, LawCatalog,
    LawWorld,
};
use hertzforge_sine_laws::SINE_CHECK_VERSION;

/// The version of the mixed check written into its records. A change to the
/// cases, their stations or their requirements changes it, and a law checked
/// before must be checked again.
pub const MIXED_CHECK_VERSION: &str = "mixed-check 1";

/// The catalog's name for the record of the mixed check in `role`.
pub fn mixed_check_name(role: LawRole) -> &'static str {
    match role {
        LawRole::Floor => "mixed-floor",
        LawRole::Tones => "mixed-tones",
    }
}

/// The world a law of `role` is from.
pub fn role_world(role: LawRole) -> LawWorld {
    match role {
        LawRole::Floor => LawWorld::Single,
        LawRole::Tones => LawWorld::Sine,
    }
}

/// Why a law may not drive hardware in a Mixed Mode role: which of its two
/// checks stands in the way, and how.
#[derive(Debug, Clone, PartialEq)]
pub enum RoleRefusal {
    /// Its own world's check.
    OwnCheck(ArmRefusal),
    /// The mixed check in this role.
    MixedCheck(ArmRefusal),
}

impl RoleRefusal {
    /// The refusal of the check that stands in the way.
    pub fn refusal(&self) -> &ArmRefusal {
        match self {
            RoleRefusal::OwnCheck(r) | RoleRefusal::MixedCheck(r) => r,
        }
    }
}

impl std::fmt::Display for RoleRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let which = match self {
            RoleRefusal::OwnCheck(_) => "its own check",
            RoleRefusal::MixedCheck(_) => "the mixed check in this role",
        };
        match self.refusal() {
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

/// The record of a law file's mixed check: `cases` in the order the check ran
/// them, each named by its case's record name.
pub fn mixed_check_record(law_sha256: &str, date: &str, cases: Vec<CheckCase>) -> CheckRecord {
    CheckRecord::new(law_sha256, MIXED_CHECK_VERSION, date, cases)
}

/// Write `record` as the law's mixed check in `role`.
pub fn record_mixed_check(
    catalog: &mut LawCatalog,
    role: LawRole,
    record: &CheckRecord,
) -> Result<(), String> {
    catalog
        .record_named_check(mixed_check_name(role), record)
        .map_err(|e| e.to_string())
}

/// Where the law in `entry` stands on the mixed check in `role`.
pub fn mixed_check_status(
    catalog: &LawCatalog,
    entry: &CatalogEntry,
    role: LawRole,
) -> CheckStatus {
    if entry.builtin {
        return CheckStatus::BuiltIn;
    }
    catalog.named_check(&entry.identity.sha256, mixed_check_name(role))
}

/// Whether the law in `entry` may drive hardware in `role` of a Mixed Mode run
/// (design D15). A built-in law always may. An imported law needs:
///
/// - its own world's check passed for its exact file, at the check in force (a
///   `single` law has no check of its own; a `sine` law's record must be of
///   [`SINE_CHECK_VERSION`]);
/// - the mixed check in this role passed for its exact file, at
///   [`MIXED_CHECK_VERSION`] and over the cases the check runs now
///   ([`MixedCheckCase::ALL`], by name and order).
///
/// A record for the other role clears nothing here. The simulator runs any
/// law; this rule is for hardware only.
pub fn may_arm_in_role(
    catalog: &LawCatalog,
    entry: &CatalogEntry,
    role: LawRole,
) -> Result<(), RoleRefusal> {
    if entry.builtin {
        return Ok(());
    }
    if entry.world == LawWorld::Sine {
        may_arm_on_hardware(entry).map_err(RoleRefusal::OwnCheck)?;
        if let CheckStatus::Checked(r) = &entry.check {
            if r.tool_version != SINE_CHECK_VERSION {
                return Err(RoleRefusal::OwnCheck(ArmRefusal::CheckOutdated {
                    tool_version: r.tool_version.clone(),
                    current: SINE_CHECK_VERSION.into(),
                }));
            }
        }
    }
    role_record_clears(
        &mixed_check_status(catalog, entry, role),
        &entry.identity.sha256,
    )
    .map_err(RoleRefusal::MixedCheck)
}

/// Whether a mixed-check `status` clears the file `law_sha256`.
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
    let names_now = MixedCheckCase::ALL.iter().map(|c| c.name());
    let same_cases = record.cases.len() == MixedCheckCase::ALL.len()
        && record.cases.iter().map(|c| c.name.as_str()).eq(names_now);
    if record.tool_version != MIXED_CHECK_VERSION || !same_cases {
        return Err(ArmRefusal::CheckOutdated {
            tool_version: record.tool_version.clone(),
            current: MIXED_CHECK_VERSION.into(),
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
        MixedCheckCase::ALL
            .iter()
            .map(|c| CheckCase {
                name: c.name().into(),
                passed: pass || *c != MixedCheckCase::TransportDelay,
                detail: String::new(),
            })
            .collect()
    }

    fn checked(r: CheckRecord) -> CheckStatus {
        CheckStatus::Checked(r)
    }

    #[test]
    fn a_mixed_record_clears_only_at_the_check_in_force() {
        let good = mixed_check_record(SHA, "2026-11-11T10:00:00Z", cases(true));
        assert_eq!(role_record_clears(&checked(good.clone()), SHA), Ok(()));
        assert_eq!(
            role_record_clears(&CheckStatus::Unchecked, SHA),
            Err(ArmRefusal::Unchecked)
        );
        // A failed case.
        let failed = mixed_check_record(SHA, "2026-11-11T10:00:00Z", cases(false));
        assert!(matches!(
            role_record_clears(&checked(failed), SHA),
            Err(ArmRefusal::CheckFailed { failed_cases, .. }) if failed_cases == ["transport_delay"]
        ));
        // Another version of the check.
        let mut old = good.clone();
        old.tool_version = "mixed-check 0".into();
        assert!(matches!(
            role_record_clears(&checked(old), SHA),
            Err(ArmRefusal::CheckOutdated { .. })
        ));
        // The same version over a shorter case list (a record made before a case
        // was added) is outdated too.
        let mut fewer = good.clone();
        fewer.cases.pop();
        assert!(matches!(
            role_record_clears(&checked(fewer), SHA),
            Err(ArmRefusal::CheckOutdated { .. })
        ));
        // Cases in another order are another check.
        let mut moved = good.clone();
        moved.cases.swap(0, 1);
        assert!(matches!(
            role_record_clears(&checked(moved), SHA),
            Err(ArmRefusal::CheckOutdated { .. })
        ));
        // Another file's record.
        assert!(matches!(
            role_record_clears(&checked(good), &"0".repeat(64)),
            Err(ArmRefusal::CheckUnreadable(_))
        ));
    }

    #[test]
    fn a_role_refusal_says_which_check_stands_in_the_way() {
        let own = RoleRefusal::OwnCheck(ArmRefusal::Unchecked).to_string();
        assert_eq!(own, "its own check has not been run");
        let mixed = RoleRefusal::MixedCheck(ArmRefusal::CheckOutdated {
            tool_version: "mixed-check 0".into(),
            current: MIXED_CHECK_VERSION.into(),
        })
        .to_string();
        assert_eq!(
            mixed,
            "the mixed check in this role passed mixed-check 0, which mixed-check 1 has replaced"
        );
    }

    #[test]
    fn each_role_has_its_own_record_name_and_world() {
        assert_eq!(mixed_check_name(LawRole::Floor), "mixed-floor");
        assert_eq!(mixed_check_name(LawRole::Tones), "mixed-tones");
        assert_eq!(role_world(LawRole::Floor), LawWorld::Single);
        assert_eq!(role_world(LawRole::Tones), LawWorld::Sine);
    }
}
