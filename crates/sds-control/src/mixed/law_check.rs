//! The mixed check behind `laws.check` with `mixed` (schema/PROTOCOL.md, "Mixed
//! Mode Laws"): every case of the core's mixed check, each a whole Mixed Mode run
//! on a simulated station built here from the case's own facts (the core names no
//! simulator), with the law under check in its role and the built-in in the other.

use crate::protocol::{ErrorCode, MethodError};
use hertzforge_mixed::{LawRole, MixedLawFactories};
use hertzforge_mixed_laws::{mixed_check_station, run_mixed_check_case, MixedCheckCase};
use hertzforge_random_laws::CheckCase;
use hertzforge_station::SimPlantSpec;
use hertzforge_station::{BackendKind, StationSpec};
use serde_json::{json, Value};

/// The role's wire name.
pub fn role_name(role: LawRole) -> &'static str {
    match role {
        LawRole::Floor => "floor",
        LawRole::Tones => "tones",
    }
}

/// The simulated station `case` runs on, from the case's own facts.
pub fn case_station(case: MixedCheckCase) -> StationSpec {
    let s = mixed_check_station(case);
    StationSpec {
        sim: SimPlantSpec {
            sample_rate_hz: s.fs_hz,
            resonances: s.modes,
            feedthrough: s.feedthrough,
            noise_level: s.noise_rms_g,
            transport_delay_samples: s.transport_delay_samples,
            shared_clock: true,
            seed: s.seed,
            ..SimPlantSpec::default()
        },
        ..StationSpec::default()
    }
}

/// One case's result on the wire.
pub fn case_json(case: MixedCheckCase, role: LawRole, v: &CheckCase) -> Value {
    json!({
        "name": case.name().replace('_', " "),
        "case": case.name(),
        "role": role_name(role),
        "plant": case.plant(),
        "requirement": case.requirement(),
        "passed": v.passed,
        "detail": v.detail,
    })
}

/// One progress event: a case starting (`result` null) or done.
pub fn progress_json(
    law_id: &str,
    index: usize,
    case: MixedCheckCase,
    result: Option<Value>,
) -> Value {
    json!({
        "law_id": law_id,
        "index": index,
        "of": MixedCheckCase::ALL.len(),
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
    laws: &MixedLawFactories,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<(Value, CheckCase)>, MethodError> {
    let mut out = Vec::with_capacity(MixedCheckCase::ALL.len());
    for (i, case) in MixedCheckCase::ALL.iter().copied().enumerate() {
        progress(progress_json(law_id, i, case, None));
        let station =
            hertzforge_station::try_build_station_backend(BackendKind::Sim, &case_station(case))
                .map_err(crate::station::build_refusal)?;
        let v = run_mixed_check_case(case, laws, station.backend).map_err(|e| {
            MethodError::named(
                ErrorCode::Refused,
                "law_check_unsupported",
                format!(
                    "The mixed check could not run case \"{}\": {e}.",
                    case.name()
                ),
            )
        })?;
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
    fn every_case_has_its_own_station() {
        for case in MixedCheckCase::ALL {
            let s = case_station(case).sim;
            let want = mixed_check_station(case);
            assert_eq!(s.sample_rate_hz, want.fs_hz, "{}", case.name());
            assert_eq!(s.transport_delay_samples, want.transport_delay_samples);
            assert_eq!(s.resonances, want.modes);
        }
        let d = case_station(MixedCheckCase::TransportDelay).sim;
        assert_eq!(
            d.transport_delay_samples,
            hertzforge_mixed_laws::MIXED_CHECK_DELAY_FRAMES
        );
    }
}
