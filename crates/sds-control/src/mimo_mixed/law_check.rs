//! The MIMO Mixed check behind `laws.check` with `mimo_mixed` (schema/PROTOCOL.md,
//! "MIMO Mixed Mode Laws"): every case of the laws crate's check, each a whole MIMO
//! Mixed Mode run on a simulated sine structure built here from the case's own
//! facts (the laws crate names no simulator), with the law under check in its role
//! and the built-in in the other.

use crate::protocol::{ErrorCode, MethodError};
use hertzforge_hal::Backend;
use hertzforge_mimo_mixed::service::{LawRole, MimoMixedLaws};
use hertzforge_mimo_mixed_laws::{
    mimo_mixed_check_station, run_mimo_mixed_check_case, MimoMixedCheckCase,
};
use hertzforge_random_laws::CheckCase;
use serde_json::{json, Value};

pub use super::config::role_name;

/// The simulated station `case` runs on: its sine structure, its delay, noise
/// and seed.
pub fn case_backend(case: MimoMixedCheckCase) -> Result<Box<dyn Backend>, String> {
    use hertzforge_hal_sim::plants::SineStructure;
    use hertzforge_hal_sim::PlantModel;
    let s = mimo_mixed_check_station(case);
    let structure = SineStructure::from_name(s.structure)
        .ok_or_else(|| format!("no standard sine structure {}", s.structure))?;
    let mut model = structure.build().model;
    if s.transport_delay_samples > 0 {
        let PlantModel::Jointed(p) = &mut model else {
            return Err(format!("{} is not a modal structure", s.structure));
        };
        p.transport_delay_samples = s.transport_delay_samples;
    }
    Ok(Box::new(
        hertzforge_hal_sim::SimConfig::new()
            .sample_rate_hz(s.fs_hz)
            .plant(model)
            .noise_level(s.noise_rms_g)
            .seed(s.seed)
            .shared_clock(true)
            .build(),
    ))
}

/// One case's result on the wire.
pub fn case_json(case: MimoMixedCheckCase, role: LawRole, v: &CheckCase) -> Value {
    json!({
        "name": case.name().replace('_', " "),
        "case": case.name(),
        "role": role_name(role),
        "plant": case.plant(),
        "requirement": case.requirement(),
        "required": case.required(),
        "passed": v.passed,
        "detail": v.detail,
    })
}

/// One progress event: a case starting (`result` null) or done.
pub fn progress_json(
    law_id: &str,
    index: usize,
    case: MimoMixedCheckCase,
    result: Option<Value>,
) -> Value {
    json!({
        "law_id": law_id,
        "index": index,
        "of": MimoMixedCheckCase::ALL.len(),
        "case": case.name(),
        "state": if result.is_some() { "done" } else { "running" },
        "result": result.unwrap_or(Value::Null),
    })
}

/// Run every case with `laws` (the law under check in `role`, the built-in in
/// the other): one event as each starts and as it ends through `progress`.
pub fn run_check(
    law_id: &str,
    role: LawRole,
    laws: &MimoMixedLaws,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<(Value, CheckCase)>, MethodError> {
    let mut out = Vec::with_capacity(MimoMixedCheckCase::ALL.len());
    for (i, case) in MimoMixedCheckCase::ALL.iter().copied().enumerate() {
        progress(progress_json(law_id, i, case, None));
        let unsupported = |words: String| {
            MethodError::named(
                ErrorCode::Refused,
                "law_check_unsupported",
                format!(
                    "The MIMO Mixed check could not run case \"{}\": {words}.",
                    case.name()
                ),
            )
        };
        let backend = case_backend(case).map_err(unsupported)?;
        let v = run_mimo_mixed_check_case(case, laws, backend)
            .map_err(|e| unsupported(e.to_string()))?;
        let j = case_json(case, role, &v);
        progress(progress_json(law_id, i, case, Some(j.clone())));
        out.push((j, v));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_builds_its_own_station() {
        for case in MimoMixedCheckCase::ALL {
            let b = case_backend(case).expect("a station");
            let want = mimo_mixed_check_station(case);
            assert_eq!(b.caps().sample_rate_hz, want.fs_hz, "{}", case.name());
        }
    }
}
