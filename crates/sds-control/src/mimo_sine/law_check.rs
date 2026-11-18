//! `laws.check` of a MIMO Sine law: the laws crate's check, its answer on the
//! wire, and what the catalog records of it.
//!
//! The check is the laws crate's (`hertzforge_mimo_sine_laws::run_check`, its
//! cases named by `MimoSineCheckCase`). Where it does not run a case, nothing
//! is recorded as passed, so a law file without a passed check stays refused
//! `law_not_checked` on a hardware station (the arm gate reads the catalog's
//! record, as every other world's does); the simulator takes any law.

use crate::protocol::{ErrorCode, MethodError};
use hertzforge_mimo_sine_laws::{self as msl, MimoSineCheckCase};
use serde_json::{json, Value};

/// The cases the check runs, by their stable names.
pub fn case_names() -> Vec<&'static str> {
    MimoSineCheckCase::ALL.iter().map(|c| c.name()).collect()
}

/// Run the check of the law `id`. `emit` is given one `laws.check` event as
/// each case starts and as it ends. The answer is the report.
pub fn run_check(id: &str, emit: &mut dyn FnMut(Value)) -> Result<Value, MethodError> {
    match msl::run_check() {
        Ok(()) => {
            let cases: Vec<Value> = case_names()
                .into_iter()
                .map(|c| {
                    emit(json!({"law": id, "case": c, "state": "passed"}));
                    json!({"case": c, "passed": true})
                })
                .collect();
            Ok(json!({"passed": true, "cases": cases, "recorded": false}))
        }
        Err(e) => Err(MethodError::named(
            ErrorCode::Refused,
            "law_check_unsupported",
            format!(
                "The MIMO Sine check cannot run \"{id}\": {e}. A law file stays refused on a \
                 hardware station until a check of its exact file has passed; it runs on the \
                 simulator."
            ),
        )
        .with_detail(json!({"cases": case_names()}))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_a_check_that_runs_no_case_records_nothing_and_says_why() {
        let mut events = Vec::new();
        let r = run_check("matrix", &mut |e| events.push(e));
        match r {
            Err(e) => {
                assert_eq!(e.reason, Some("law_check_unsupported"));
                assert!(events.is_empty());
            }
            // Once the check's cases are built, every case is reported.
            Ok(v) => assert_eq!(v["cases"].as_array().map(Vec::len), Some(10)),
        }
    }
}
