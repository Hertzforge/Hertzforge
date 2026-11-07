//! `laws.check` for a classical shock law (schema/PROTOCOL.md, "Control Laws", "A
//! classical shock law"): the simulated test articles the check runs on, built
//! through the controller's station factory, each case run under the core's
//! guards by the controller's own check runner, and the result on the wire.
//!
//! What a case asks and how a run is judged are the controller's
//! (`hertzforge_shock_laws::check`); this module builds each case's simulated
//! station and says the result in the protocol's words.

use crate::protocol::{num, opt_num, ErrorCode, MethodError};
use hertzforge_random_laws::{CheckCase, CheckRecord, CHECK_FORMAT};
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_shock_host::ClassicalLawFactory;
use hertzforge_shock_laws::{
    ClassicalCaseEnd, ClassicalCaseMeasure, ClassicalCheckCase, ClassicalCheckPlant,
    ClassicalRequirement, CLASSICAL_CHECK_VERSION,
};
use hertzforge_station::{BackendKind, SimPlant, SimPlantSpec, StationSpec};
use serde_json::{json, Value};
use std::sync::Arc;

/// The rate every check case runs at, Hz: the bench's.
pub const CHECK_FS_HZ: f64 = 51_200.0;
/// The FRF line spacing of the anti-resonance plant, Hz.
const NOTCH_DF_HZ: f64 = 5.0;
/// The anti-resonance's pole damping; its zeros are damped less by the notch depth.
const NOTCH_POLE_DAMPING: f64 = 0.1;
/// The simulator's floor on every case: noise well under any pulse.
const FLOOR_NOISE_V: f64 = 2e-5;
/// The response the noise case's noise is set under, volts at the input: the
/// full-level pulse's peak on a one-to-one wire at the check's amplitude.
pub const CHECK_RESPONSE_PEAK_V: f64 = 1.0;

fn r(x: f64, d: i32) -> f64 {
    super::config::round(x, d)
}

/// The test article in words.
pub fn plant_words(case: ClassicalCheckCase) -> String {
    let base = match case.plant() {
        ClassicalCheckPlant::Wire => "a wire".to_owned(),
        ClassicalCheckPlant::OneMode { f_hz, q } => {
            format!("one mode, {} Hz, Q {}", num(f_hz), num(q))
        }
        ClassicalCheckPlant::Station { f_hz, q } => format!(
            "two modes, {} and {} Hz, Q {}",
            num(f_hz[0]),
            num(f_hz[1]),
            num(q)
        ),
        ClassicalCheckPlant::DelayedMode { f_hz, q, delay_s } => format!(
            "one mode, {} Hz, Q {}, behind {} ms",
            num(f_hz),
            num(q),
            num(r(delay_s * 1000.0, 3))
        ),
        ClassicalCheckPlant::Notch { f_hz, depth_db } => {
            format!("a {} dB notch at {} Hz", num(depth_db), num(f_hz))
        }
    };
    let mut words = base;
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
    words
}

pub fn requirement_name(r: ClassicalRequirement) -> &'static str {
    match r {
        ClassicalRequirement::EveryPulse => "every_pulse",
        ClassicalRequirement::PassBy { .. } => "pass_by",
        ClassicalRequirement::Survive => "survive",
    }
}

/// How a case's run ended, as a run outcome.
pub fn end_name(e: &ClassicalCaseEnd) -> &'static str {
    match e {
        ClassicalCaseEnd::Completed => "completed",
        ClassicalCaseEnd::NotConverged => "not_converged",
        ClassicalCaseEnd::Divergence | ClassicalCaseEnd::Aborted(_) => "aborted",
        ClassicalCaseEnd::LawFault(_) => "law_fault",
        ClassicalCaseEnd::Held => "held",
        ClassicalCaseEnd::Fault(_) => "fault",
    }
}

/// What tripped, by its wire name, when anything did.
pub fn trip_name(e: &ClassicalCaseEnd) -> Option<String> {
    match e {
        ClassicalCaseEnd::Completed | ClassicalCaseEnd::Held => None,
        ClassicalCaseEnd::NotConverged => Some("not_converged".into()),
        ClassicalCaseEnd::Divergence => Some("divergence".into()),
        ClassicalCaseEnd::Aborted(k) => Some(k.clone()),
        ClassicalCaseEnd::LawFault(k) => Some(k.clone()),
        ClassicalCaseEnd::Fault(words) => Some(words.clone()),
    }
}

/// One case's result on the wire.
pub fn case_json(case: ClassicalCheckCase, m: &ClassicalCaseMeasure, v: &CheckCase) -> Value {
    json!({
        "name": case.name(),
        "plant": plant_words(case),
        "requirement": requirement_name(case.requirement()),
        "passed": v.passed,
        "detail": v.detail,
        "end": end_name(&m.end),
        "trip": trip_name(&m.end),
        "pulses": m.pulses,
        "first_pass_pulse": m.first_pass.map(|(p, _)| p),
        "full_level_pulses": m.full_level_pulses,
        "full_level_passed": m.full_level_passed,
        "misses": m.misses,
        "refused": m.refused,
        "capped_bins": m.capped_bins,
        "calls": m.calls,
        "worst_call_ms": opt_num(m.worst_call_ms.map(|x| r(x, 3))),
        "budget_ms": num(r(m.budget_ms, 3)),
        "max_drive_vpk": num(r(m.max_drive_vpk, 4)),
    })
}

/// The `laws.check` report of a classical law: every case, and whether the result
/// was recorded against the law file.
pub fn report_json(cases: &[Value], passed: bool, recorded: bool) -> Value {
    json!({
        "passed": passed,
        "recorded": recorded,
        "tool_version": CLASSICAL_CHECK_VERSION,
        "cases": cases,
    })
}

/// One `laws.check` progress event: a case starting (`result` null) or done.
pub fn progress_json(
    law_id: &str,
    index: usize,
    case: ClassicalCheckCase,
    result: Option<Value>,
) -> Value {
    json!({
        "law_id": law_id,
        "index": index,
        "of": ClassicalCheckCase::ALL.len(),
        "case": case.name(),
        "state": if result.is_some() { "done" } else { "running" },
        "result": result.unwrap_or(Value::Null),
    })
}

/// The simulated station a case runs on: one drive, one control input.
pub fn case_station(case: ClassicalCheckCase) -> StationSpec {
    let noise = match case.noise_below_db() {
        Some(db) => CHECK_RESPONSE_PEAK_V * 10f64.powf(-db / 20.0),
        None => FLOOR_NOISE_V,
    };
    let base = SimPlantSpec {
        sample_rate_hz: CHECK_FS_HZ,
        resonances: Vec::new(),
        noise_level: noise,
        feedthrough: 1.0,
        transport_delay_samples: 0,
        seed: 0x5A5A_7001 + case as u64,
        max_input_channels: 1,
        max_output_channels: 1,
        shared_clock: true,
        plant: SimPlant::Resonators,
    };
    let sim = match case.plant() {
        ClassicalCheckPlant::Wire => base,
        ClassicalCheckPlant::OneMode { f_hz, q } => SimPlantSpec {
            resonances: vec![(f_hz, q)],
            ..base
        },
        ClassicalCheckPlant::Station { f_hz, q } => SimPlantSpec {
            resonances: vec![(f_hz[0], q), (f_hz[1], q)],
            feedthrough: 0.35,
            ..base
        },
        ClassicalCheckPlant::DelayedMode { f_hz, q, delay_s } => SimPlantSpec {
            resonances: vec![(f_hz, q)],
            transport_delay_samples: (delay_s * CHECK_FS_HZ).round() as usize,
            ..base
        },
        ClassicalCheckPlant::Notch { f_hz, depth_db } => SimPlantSpec {
            plant: SimPlant::Frf {
                stack: notch_stack(f_hz, depth_db),
                df_hz: NOTCH_DF_HZ,
            },
            ..base
        },
    };
    StationSpec {
        sim,
        ..StationSpec::default()
    }
}

/// A one-by-one FRF with a notch of `depth_db` at `f_hz`: a second-order section
/// whose zeros are damped less than its poles by the depth.
fn notch_stack(f_hz: f64, depth_db: f64) -> MatrixStack {
    let lines = (CHECK_FS_HZ / 2.0 / NOTCH_DF_HZ) as usize + 1;
    let zeta_p = NOTCH_POLE_DAMPING;
    let zeta_z = zeta_p * 10f64.powf(-depth_db / 20.0);
    let w0 = 2.0 * std::f64::consts::PI * f_hz;
    let data = (0..lines)
        .map(|k| {
            let w = 2.0 * std::f64::consts::PI * k as f64 * NOTCH_DF_HZ;
            let s = C64::new(0.0, w);
            let num = s * s + s * (2.0 * zeta_z * w0) + w0 * w0;
            let den = s * s + s * (2.0 * zeta_p * w0) + w0 * w0;
            num / den
        })
        .collect();
    MatrixStack::from_vec(lines, 1, 1, data).unwrap_or_else(|_| MatrixStack::identity(lines, 1))
}

/// Run every case with the law `factory` builds: one event as each starts and as it
/// ends through `progress`. Returns each case's wire result and verdict.
pub fn run_check(
    law_id: &str,
    factory: Arc<dyn ClassicalLawFactory>,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<(Value, CheckCase)>, MethodError> {
    let mut out = Vec::with_capacity(ClassicalCheckCase::ALL.len());
    for (i, case) in ClassicalCheckCase::ALL.iter().copied().enumerate() {
        progress(progress_json(law_id, i, case, None));
        let station =
            hertzforge_station::try_build_station_backend(BackendKind::Sim, &case_station(case))
                .map_err(crate::station::build_refusal)?;
        let measure =
            hertzforge_shock_laws::run_classical_check_case(case, factory.clone(), station.backend)
                .map_err(|e| {
                    MethodError::named(
                        ErrorCode::Refused,
                        "law_check_unsupported",
                        format!(
                            "The classical shock check could not run case \"{}\": {e}.",
                            case.name()
                        ),
                    )
                })?;
        let verdict = hertzforge_shock_laws::judge_classical(case, &measure);
        let json = case_json(case, &measure, &verdict);
        progress(progress_json(law_id, i, case, Some(json.clone())));
        out.push((json, verdict));
    }
    Ok(out)
}

/// The check record of a classical law file's result.
pub fn check_record(sha256: String, date: String, cases: Vec<CheckCase>) -> CheckRecord {
    CheckRecord {
        format: CHECK_FORMAT,
        law_sha256: sha256,
        tool_version: CLASSICAL_CHECK_VERSION.to_owned(),
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
        let words: Vec<String> = ClassicalCheckCase::ALL
            .iter()
            .map(|c| plant_words(*c))
            .collect();
        assert_eq!(
            words,
            [
                "a wire",
                "one mode, 400 Hz, Q 10",
                "two modes, 200 and 550 Hz, Q 2.5",
                "one mode, 400 Hz, Q 10, behind 2 ms",
                "a wire, identified gain +3 dB",
                "one mode, 400 Hz, Q 10, noise 30 dB under",
                "a 30 dB notch at 300 Hz",
            ]
        );
        for c in ClassicalCheckCase::ALL {
            let s = case_station(c);
            assert_eq!(
                (s.sim.max_input_channels, s.sim.max_output_channels),
                (1, 1)
            );
            assert!(s.sim.shared_clock);
        }
        let d = case_station(ClassicalCheckCase::Delay);
        assert_eq!(d.sim.transport_delay_samples, 102);
    }

    #[test]
    fn the_notch_is_as_deep_as_the_case_says_and_flat_far_from_it() {
        let s = notch_stack(300.0, 30.0);
        let at = |f: f64| s.data()[(f / NOTCH_DF_HZ) as usize].norm();
        assert!(
            (20.0 * at(300.0).log10() + 30.0).abs() < 0.01,
            "{}",
            at(300.0)
        );
        assert!((at(5000.0) - 1.0).abs() < 0.01);
    }
}
