//! Check records: the result of running a law file against the simulated test
//! articles, kept beside the file, and the rule the arming gate applies.
//!
//! A record is `<data_root>/laws/<sha256>.check.json`, for the law file
//! `<sha256>.wasm`: it names the exact file it is for by that hash, so a record
//! can never vouch for different bytes. The tool that runs the checks and
//! writes records is separate; this module reads and writes them and decides,
//! from a catalog entry, whether a law may drive hardware.

use crate::catalog::{CatalogEntry, CheckStatus, LawWorld};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// The record format this code reads and writes.
pub const CHECK_FORMAT: u32 = 1;

/// One case of a check: one simulated test article or condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckCase {
    /// What the case is, such as "two close modes".
    pub name: String,
    /// Whether the law passed it.
    pub passed: bool,
    /// What was measured, in words.
    pub detail: String,
}

/// The result of checking one law file.
///
/// `passed` must agree with the cases: true exactly when there is at least one
/// case and every case passed. A record that says otherwise is refused on read
/// and on write, so a hand-edited verdict cannot clear a law.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckRecord {
    /// The record format ([`CHECK_FORMAT`]).
    pub format: u32,
    /// SHA-256 of the law file checked, 64 lower-case hex digits.
    pub law_sha256: String,
    /// The version of the tool that ran the check.
    pub tool_version: String,
    /// When the check ran, RFC 3339 (for example `2026-11-02T17:00:00Z`).
    pub date: String,
    /// Whether the law passed every case.
    pub passed: bool,
    /// Each case's result.
    pub cases: Vec<CheckCase>,
}

impl CheckRecord {
    /// A record for `law_sha256` from `cases`, its verdict derived from them.
    pub fn new(
        law_sha256: impl Into<String>,
        tool_version: impl Into<String>,
        date: impl Into<String>,
        cases: Vec<CheckCase>,
    ) -> Self {
        let passed = !cases.is_empty() && cases.iter().all(|c| c.passed);
        CheckRecord {
            format: CHECK_FORMAT,
            law_sha256: law_sha256.into(),
            tool_version: tool_version.into(),
            date: date.into(),
            passed,
            cases,
        }
    }

    /// Where the record for `law_sha256` lives in `laws_dir`.
    pub fn path(laws_dir: &Path, law_sha256: &str) -> PathBuf {
        laws_dir.join(format!("{law_sha256}.check.json"))
    }

    /// Whether the record is well formed: the known format, a SHA-256 for a
    /// hash, and a verdict that agrees with its cases.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != CHECK_FORMAT {
            return Err(format!(
                "record format {} is not the {CHECK_FORMAT} this program reads",
                self.format
            ));
        }
        if !is_sha256(&self.law_sha256) {
            return Err(format!(
                "`{}` is not a SHA-256 of a law file",
                self.law_sha256
            ));
        }
        let derived = !self.cases.is_empty() && self.cases.iter().all(|c| c.passed);
        if self.passed != derived {
            return Err(format!(
                "the record says {} but its cases say {}",
                verdict(self.passed),
                verdict(derived)
            ));
        }
        Ok(())
    }

    /// Read the record for `law_sha256` from `laws_dir`: `Ok(None)` when there
    /// is none, an error when there is one that cannot be read, is malformed,
    /// or is for another file.
    pub fn read(laws_dir: &Path, law_sha256: &str) -> Result<Option<CheckRecord>, String> {
        let path = Self::path(laws_dir, law_sha256);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let record: CheckRecord =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        record
            .validate()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if record.law_sha256 != law_sha256 {
            return Err(format!(
                "{}: the record is for the law file {}",
                path.display(),
                record.law_sha256
            ));
        }
        Ok(Some(record))
    }

    /// Write this record into `laws_dir` (created if missing), replacing any
    /// earlier record for the same file. Written whole or not at all.
    pub fn write(&self, laws_dir: &Path) -> Result<PathBuf, String> {
        self.validate()?;
        let path = Self::path(laws_dir, &self.law_sha256);
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::catalog::write_whole(laws_dir, &path, text.as_bytes())?;
        Ok(path)
    }
}

fn verdict(passed: bool) -> &'static str {
    if passed {
        "passed"
    } else {
        "not passed"
    }
}

/// Whether `s` is 64 lower-case hex digits.
pub(crate) fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Why a law may not drive hardware.
#[derive(Debug, Clone, PartialEq)]
pub enum ArmRefusal {
    /// The law file has never been checked.
    Unchecked,
    /// The law file was checked and did not pass.
    CheckFailed {
        /// When the check ran.
        date: String,
        /// The cases it did not pass.
        failed_cases: Vec<String>,
    },
    /// The law file's check record could not be read, or is not for this file.
    CheckUnreadable(String),
    /// The law file passed a check the current check has replaced.
    CheckOutdated {
        /// The check that cleared it.
        tool_version: String,
        /// The check in force.
        current: String,
    },
}

/// The version of the TWR law check in force. A TWR law file is cleared for
/// hardware only by a record of this check: a change to its plant set or its
/// rules changes this, and a file checked before must be checked again.
pub const TWR_CHECK_VERSION: &str = "twr-check 1";

impl fmt::Display for ArmRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArmRefusal::Unchecked => write!(
                f,
                "this control law has not been checked on the simulator; check it before it \
                 drives hardware"
            ),
            ArmRefusal::CheckFailed { date, failed_cases } => {
                let which = if failed_cases.is_empty() {
                    "no cases ran".to_string()
                } else {
                    format!("failed: {}", failed_cases.join(", "))
                };
                write!(
                    f,
                    "this control law did not pass its check on {date} ({which}); it may run on \
                     the simulator only"
                )
            }
            ArmRefusal::CheckOutdated {
                tool_version,
                current,
            } => write!(
                f,
                "this control law passed {tool_version}, which {current} has replaced; check it \
                 again before it drives hardware"
            ),
            ArmRefusal::CheckUnreadable(why) => write!(
                f,
                "this control law's check record cannot be used ({why}); check it again before \
                 it drives hardware"
            ),
        }
    }
}

impl std::error::Error for ArmRefusal {}

/// Whether the law in `entry` may drive hardware. A built-in law always may.
/// An imported law may only with a passing check record for its exact file.
/// The simulator runs any law; this gate is for hardware only.
pub fn may_arm_on_hardware(entry: &CatalogEntry) -> Result<(), ArmRefusal> {
    if entry.builtin {
        return Ok(());
    }
    match &entry.check {
        CheckStatus::BuiltIn => Err(ArmRefusal::CheckUnreadable(
            "an imported law cannot be cleared as built in".into(),
        )),
        CheckStatus::Unchecked => Err(ArmRefusal::Unchecked),
        CheckStatus::Unreadable(why) => Err(ArmRefusal::CheckUnreadable(why.clone())),
        CheckStatus::Checked(record) => {
            record.validate().map_err(ArmRefusal::CheckUnreadable)?;
            if record.law_sha256 != entry.identity.sha256 {
                return Err(ArmRefusal::CheckUnreadable(format!(
                    "the record is for the law file {}",
                    record.law_sha256
                )));
            }
            // A TWR record clears a file only at the check in force. The
            // other worlds' records are not held to their version yet.
            if entry.world == LawWorld::Twr && record.tool_version != TWR_CHECK_VERSION {
                return Err(ArmRefusal::CheckOutdated {
                    tool_version: record.tool_version.clone(),
                    current: TWR_CHECK_VERSION.into(),
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
    }
}
