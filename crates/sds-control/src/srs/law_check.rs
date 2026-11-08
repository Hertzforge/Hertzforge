//! `laws.check` for an SRS shock law (schema/PROTOCOL.md, "Control Laws", "An SRS
//! shock law"): the simulated test articles the check runs on, built through the
//! controller's station factory, each case run under the core's guards by the
//! controller's own check runner, and the result on the wire.
//!
//! What a case asks and how a run is judged are the controller's
//! (`hertzforge_shock_laws::SrsCheckCase`, `judge_srs`); this module builds each
//! case's simulated station and says the result in the protocol's words.

use crate::protocol::{num, opt_num, ErrorCode, MethodError};
use hertzforge_random_laws::{CheckCase, CheckRecord, CHECK_FORMAT};
use hertzforge_shock_laws::{
    SrsCaseEnd, SrsCaseMeasure, SrsCheckCase, SrsCheckPlant, SrsRequirement, SRS_CHECK_VERSION,
};
use hertzforge_srs::SrsLawFactory;
use hertzforge_station::{BackendKind, SimPlant, SimPlantSpec, StationSpec};
use serde_json::{json, Value};
use std::sync::Arc;

/// The rate every check case runs at, Hz: the bench's.
pub const CHECK_FS_HZ: f64 = 51_200.0;
/// The simulator's floor on every case: noise well under any pulse.
const FLOOR_NOISE_V: f64 = 2e-5;
/// The response the noise case's noise is referred to, g rms: the check target's
/// full-level pulse on its one-mode plant, at the first level of its schedule.
pub const NOISE_REFERENCE_RMS_G: f64 = 1.0;

fn r(x: f64, d: i32) -> f64 {
    super::config::round(x, d)
}

/// The test article in words.
pub fn plant_words(case: SrsCheckCase) -> String {
    let mut words = match case.plant() {
        SrsCheckPlant::Wire => "a wire".to_owned(),
        SrsCheckPlant::OneMode { f_hz, q } => format!("one mode, {} Hz, Q {}", num(f_hz), num(q)),
        SrsCheckPlant::TwoModes { f_hz, q } => format!(
            "two modes, {} and {} Hz, Q {}",
            num(f_hz[0]),
            num(f_hz[1]),
            num(q)
        ),
    };
    if let Some(db) = case.identification_scale_db() {
        words.push_str(&format!(
            ", identified gain {}{} dB",
            if db >= 0.0 { "+" } else { "" },
            num(db)
        ));
    }
    if let Some(db) = case.noise_below_db() {
        words.push_str(&format!(", noise {} dB under", num(db)));
    }
    if let Some(f) = case.limit_resonance_hz() {
        words.push_str(&format!(", a limit channel resonating at {} Hz", num(f)));
    }
    words
}

pub fn requirement_name(r: SrsRequirement) -> &'static str {
    match r {
        SrsRequirement::WithinBy { .. } => "within_by",
        SrsRequirement::EachLevelWithin { .. } => "each_level_within",
        SrsRequirement::LimitHeld { .. } => "limit_held",
    }
}

/// How a case's run ended, as a run outcome.
pub fn end_name(e: &SrsCaseEnd) -> &'static str {
    match e {
        SrsCaseEnd::Completed => "completed",
        SrsCaseEnd::NotConverged => "not_converged",
        SrsCaseEnd::Aborted(_) => "aborted",
        SrsCaseEnd::LawFault(_) => "law_fault",
        SrsCaseEnd::Held => "held",
        SrsCaseEnd::Fault(_) => "fault",
    }
}

/// What tripped, by its wire name, when anything did.
pub fn trip_name(e: &SrsCaseEnd) -> Option<String> {
    match e {
        SrsCaseEnd::Completed | SrsCaseEnd::Held => None,
        SrsCaseEnd::NotConverged => Some("not_converged".into()),
        SrsCaseEnd::Aborted(k) | SrsCaseEnd::LawFault(k) | SrsCaseEnd::Fault(k) => Some(k.clone()),
    }
}

/// One case's result on the wire.
pub fn case_json(case: SrsCheckCase, m: &SrsCaseMeasure, v: &CheckCase) -> Value {
    json!({
        "name": case.name(),
        "plant": plant_words(case),
        "requirement": requirement_name(case.requirement()),
        "passed": v.passed,
        "detail": v.detail,
        "end": end_name(&m.end),
        "trip": trip_name(&m.end),
        "pulses": m.pulses,
        "full_level_pulses": m.full_level_pulses,
        "first_within_pulse": m.first_within_pulse,
        "final_worst_band_db": opt_num(m.final_worst_band_db.map(|x| r(x, 2))),
        "worst_limit_over_db": opt_num(m.worst_limit_over_db.map(|x| r(x, 2))),
        "misses": m.misses,
        "refused": m.refused,
        "limited_components": m.limited_components,
        "calls": m.calls,
        "worst_call_ms": opt_num(m.worst_call_ms.map(|x| r(x, 3))),
        "budget_ms": num(r(m.budget_ms, 3)),
        "max_drive_vpk": num(r(m.max_drive_vpk, 4)),
    })
}

/// The `laws.check` report of an SRS law: every case, and whether the result was
/// recorded against the law file.
pub fn report_json(cases: &[Value], passed: bool, recorded: bool) -> Value {
    json!({
        "passed": passed,
        "recorded": recorded,
        "tool_version": SRS_CHECK_VERSION,
        "cases": cases,
    })
}

/// One `laws.check` progress event: a case starting (`result` null) or done.
pub fn progress_json(
    law_id: &str,
    index: usize,
    case: SrsCheckCase,
    result: Option<Value>,
) -> Value {
    json!({
        "law_id": law_id,
        "index": index,
        "of": SrsCheckCase::ALL.len(),
        "case": case.name(),
        "state": if result.is_some() { "done" } else { "running" },
        "result": result.unwrap_or(Value::Null),
    })
}

/// The simulated station a case runs on: one drive, one control input.
pub fn case_station(case: SrsCheckCase) -> StationSpec {
    let noise = match case.noise_below_db() {
        Some(db) => NOISE_REFERENCE_RMS_G * 10f64.powf(-db / 20.0),
        None => FLOOR_NOISE_V,
    };
    let base = SimPlantSpec {
        sample_rate_hz: CHECK_FS_HZ,
        resonances: Vec::new(),
        noise_level: noise,
        feedthrough: 1.0,
        transport_delay_samples: 0,
        seed: 0x5A5A_8001 + case as u64,
        max_input_channels: 1,
        max_output_channels: 1,
        shared_clock: true,
        plant: SimPlant::Resonators,
    };
    let sim = match case.plant() {
        SrsCheckPlant::Wire => base,
        SrsCheckPlant::OneMode { f_hz, q } => SimPlantSpec {
            resonances: vec![(f_hz, q)],
            ..base
        },
        SrsCheckPlant::TwoModes { f_hz, q } => SimPlantSpec {
            resonances: vec![(f_hz[0], q), (f_hz[1], q)],
            ..base
        },
    };
    StationSpec {
        sim,
        ..StationSpec::default()
    }
}

/// Run every case with the law `factory` builds: one event as each starts and as it
/// ends through `progress`. Returns each case's wire result and verdict.
pub fn run_check(
    law_id: &str,
    factory: Arc<dyn SrsLawFactory>,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<(Value, CheckCase)>, MethodError> {
    let mut out = Vec::with_capacity(SrsCheckCase::ALL.len());
    for (i, case) in SrsCheckCase::ALL.iter().copied().enumerate() {
        progress(progress_json(law_id, i, case, None));
        let station =
            hertzforge_station::try_build_station_backend(BackendKind::Sim, &case_station(case))
                .map_err(crate::station::build_refusal)?;
        let measure =
            hertzforge_shock_laws::run_srs_check_case(case, factory.clone(), station.backend)
                .map_err(|e| {
                    MethodError::named(
                        ErrorCode::Refused,
                        "law_check_unsupported",
                        format!(
                            "The SRS shock check could not run case \"{}\": {e}.",
                            case.name()
                        ),
                    )
                })?;
        let verdict = hertzforge_shock_laws::judge_srs(case, &measure);
        let json = case_json(case, &measure, &verdict);
        progress(progress_json(law_id, i, case, Some(json.clone())));
        out.push((json, verdict));
    }
    Ok(out)
}

/// The check record of an SRS law file's result.
pub fn check_record(sha256: String, date: String, cases: Vec<CheckCase>) -> CheckRecord {
    CheckRecord {
        format: CHECK_FORMAT,
        law_sha256: sha256,
        tool_version: SRS_CHECK_VERSION.to_owned(),
        date,
        passed: !cases.is_empty() && cases.iter().all(|c| c.passed),
        cases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_is_said_in_words_and_its_station_is_one_drive_one_input() {
        let words: Vec<String> = SrsCheckCase::ALL.iter().map(|c| plant_words(*c)).collect();
        assert_eq!(
            words,
            [
                "one mode, 400 Hz, Q 10",
                "two modes, 240 and 260 Hz, Q 30",
                "one mode, 400 Hz, Q 10, noise 30 dB under",
                "one mode, 400 Hz, Q 10",
                "one mode, 400 Hz, Q 10, identified gain +3 dB",
                "a wire, a limit channel resonating at 500 Hz",
            ]
        );
        for c in SrsCheckCase::ALL {
            let s = case_station(c);
            assert_eq!(
                (s.sim.max_input_channels, s.sim.max_output_channels),
                (1, 1)
            );
            assert!(s.sim.shared_clock);
        }
        assert_eq!(
            case_station(SrsCheckCase::CloseModes).sim.resonances.len(),
            2
        );
    }
}
