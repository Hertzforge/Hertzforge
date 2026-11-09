//! `laws.check` for a TWR law (schema/PROTOCOL.md, "Control Laws", "A TWR law"):
//! the simulated test articles the check runs on, built through the controller's
//! station factory, each case run under the core's guards by the controller's own
//! check runner, and the result on the wire.
//!
//! What a case asks, the plant it names and how a run is judged are the
//! controller's (`hertzforge_twr_laws::TwrCheckCase`, `run_twr_check_case`,
//! `judge_twr`); this module builds each case's simulated station through the
//! station factory and says the result in the protocol's words.

use crate::protocol::{num, opt_num, ErrorCode, MethodError};
use hertzforge_random_laws::{CheckCase, CheckRecord};
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_station::{BackendKind, SimPlant, SimPlantSpec, StationSpec};
use hertzforge_twr::TwrLawFactory;
use hertzforge_twr_laws::{
    twr_check_station, TwrCaseEnd, TwrCaseMeasure, TwrCheckCase, TwrCheckStation, TWR_CHECK_FS_HZ,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn r(x: f64, d: i32) -> f64 {
    crate::shock::config::round(x, d)
}

/// The test article in words: the case's own.
pub fn plant_words(case: TwrCheckCase) -> String {
    case.plant().to_string()
}

/// How a case's run ended, as a run outcome.
pub fn end_name(e: &TwrCaseEnd) -> &'static str {
    match e {
        TwrCaseEnd::Completed => "completed",
        TwrCaseEnd::NotConverged => "not_converged",
        TwrCaseEnd::Divergence | TwrCaseEnd::Aborted(_) => "aborted",
        TwrCaseEnd::LawFault(_) => "law_fault",
        TwrCaseEnd::Held => "held",
        TwrCaseEnd::Fault(_) => "fault",
    }
}

/// What tripped, in words, when anything did: the trip's kind or the law
/// fault's reason.
pub fn trip_name(e: &TwrCaseEnd) -> Option<String> {
    match e {
        TwrCaseEnd::Completed | TwrCaseEnd::Held => None,
        TwrCaseEnd::NotConverged => Some("not_converged".into()),
        TwrCaseEnd::Divergence => Some("divergence".into()),
        TwrCaseEnd::Aborted(k) | TwrCaseEnd::LawFault(k) | TwrCaseEnd::Fault(k) => Some(k.clone()),
    }
}

/// One case's result on the wire.
pub fn case_json(case: TwrCheckCase, m: &TwrCaseMeasure, v: &CheckCase) -> Value {
    json!({
        "name": case.name(),
        "plant": plant_words(case),
        "requirement": case.requirement().as_str(),
        "passed": v.passed,
        "detail": v.detail,
        "end": end_name(&m.end),
        "trip": trip_name(&m.end),
        "plays": m.plays,
        "rungs": m.rungs.iter().map(|(l, p)| json!({
            "level_db": num(*l),
            "plays_to_gate": p,
        })).collect::<Vec<_>>(),
        "full_level_error_pct": opt_num(m.full_level_error_pct.map(|x| r(x, 3))),
        "lag_frames": m.lag_frames,
        "lag_measured": m.lag_measured,
        "calls": m.calls,
        "misses": m.misses,
        "refused": m.refused,
        "clamped_bins": m.clamped_bins,
        "worst_peak_rise_db": opt_num(m.worst_peak_rise_db.map(|x| r(x, 3))),
        "worst_bin_rise_db": opt_num(m.worst_bin_rise_db.map(|x| r(x, 3))),
        "worst_call_ms": opt_num(m.worst_call_ms.map(|x| r(x, 3))),
        "budget_ms": num(r(m.budget_ms, 3)),
        "max_drive_vpk": num(r(m.max_drive_vpk, 4)),
    })
}

/// The `laws.check` report of a TWR law.
pub fn report_json(cases: &[Value], passed: bool, recorded: bool) -> Value {
    json!({
        "passed": passed,
        "recorded": recorded,
        "tool_version": hertzforge_twr_laws::TWR_CHECK_VERSION,
        "cases": cases,
    })
}

/// One `laws.check` progress event: a case starting (`result` null) or done.
pub fn progress_json(
    law_id: &str,
    index: usize,
    case: TwrCheckCase,
    result: Option<Value>,
) -> Value {
    json!({
        "law_id": law_id,
        "index": index,
        "of": TwrCheckCase::ALL.len(),
        "case": case.name(),
        "state": if result.is_some() { "done" } else { "running" },
        "result": result.unwrap_or(Value::Null),
    })
}

/// The simulated station a case runs on, built from the check's own facts for
/// it (`twr_check_station`) alone: one drive and one control input at the
/// check's rate, the case's plant (a resonator bank, or the anti-resonance as an
/// FRF plant played in its partitions), its noise, its delay and the lookahead it
/// states. The player's block is the runner's (`run_twr_check_case` takes it from
/// the same facts).
pub fn case_station(case: TwrCheckCase) -> Result<StationSpec, String> {
    let spec = twr_check_station(case);
    let base = SimPlantSpec {
        sample_rate_hz: TWR_CHECK_FS_HZ,
        resonances: Vec::new(),
        noise_level: spec.noise_rms_g,
        feedthrough: 1.0,
        transport_delay_samples: spec.transport_delay_samples,
        seed: 0x7357_5EED,
        max_input_channels: 1,
        max_output_channels: 1,
        shared_clock: true,
        plant: SimPlant::Resonators,
        output_lookahead_frames: spec.output_lookahead_frames,
    };
    let sim = match spec.station {
        TwrCheckStation::Resonators { modes, feedthrough } => SimPlantSpec {
            resonances: modes,
            feedthrough,
            ..base
        },
        TwrCheckStation::Frf {
            df_hz,
            lines,
            block_frames,
        } => {
            if lines.is_empty() || df_hz.is_nan() || df_hz <= 0.0 {
                return Err("its FRF plant has no lines".into());
            }
            let mut stack = MatrixStack::zeros(lines.len(), 1, 1);
            for (k, &(re, im)) in lines.iter().enumerate() {
                stack.set(k, 0, 0, C64::new(re, im));
            }
            SimPlantSpec {
                plant: SimPlant::Frf {
                    stack,
                    df_hz,
                    block_frames: Some(block_frames),
                },
                ..base
            }
        }
    };
    Ok(StationSpec {
        sim,
        ..StationSpec::default()
    })
}

/// Run every case with the law `factory` builds: one event as each starts and as it
/// ends through `progress`. Returns each case's wire result and verdict.
pub fn run_check(
    law_id: &str,
    factory: Arc<dyn TwrLawFactory>,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<(Value, CheckCase)>, MethodError> {
    let unsupported = |case: TwrCheckCase, why: String| {
        MethodError::named(
            ErrorCode::Refused,
            "law_check_unsupported",
            format!(
                "The TWR check could not run case \"{}\": {why}.",
                case.name()
            ),
        )
    };
    let mut out = Vec::with_capacity(TwrCheckCase::ALL.len());
    for (i, case) in TwrCheckCase::ALL.iter().copied().enumerate() {
        progress(progress_json(law_id, i, case, None));
        let spec = case_station(case).map_err(|why| unsupported(case, why))?;
        let station = hertzforge_station::try_build_station_backend(BackendKind::Sim, &spec)
            .map_err(crate::station::build_refusal)?;
        let measure =
            hertzforge_twr_laws::run_twr_check_case(case, factory.clone(), station.backend)
                .map_err(|e| unsupported(case, e.to_string()))?;
        let verdict = hertzforge_twr_laws::judge_twr(case, &measure);
        let json = case_json(case, &measure, &verdict);
        progress(progress_json(law_id, i, case, Some(json.clone())));
        out.push((json, verdict));
    }
    Ok(out)
}

/// The check record of a TWR law file's result, at the check in force.
pub fn check_record(sha256: String, date: String, cases: Vec<CheckCase>) -> CheckRecord {
    hertzforge_twr_laws::twr_check_record(sha256, date, cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_has_a_station_from_the_checks_own_facts() {
        for case in TwrCheckCase::ALL {
            let s = case_station(case).unwrap().sim;
            let f = twr_check_station(case);
            assert_eq!(s.sample_rate_hz, TWR_CHECK_FS_HZ, "{}", case.name());
            assert_eq!(s.max_output_channels, 1);
            assert_eq!(s.noise_level, f.noise_rms_g, "{}", case.name());
            assert_eq!(s.transport_delay_samples, f.transport_delay_samples);
            assert_eq!(s.output_lookahead_frames, f.output_lookahead_frames);
            assert!(s.shared_clock);
        }
        let q = case_station(TwrCheckCase::DeepQueue).unwrap().sim;
        assert_eq!(q.output_lookahead_frames, Some(512));
        assert_eq!(q.transport_delay_samples, 41);
        let m = case_station(TwrCheckCase::OneMode).unwrap().sim;
        assert_eq!(m.resonances.len(), 1);
        assert_eq!(m.feedthrough, hertzforge_twr_laws::TWR_CHECK_FEEDTHROUGH);
        let n = case_station(TwrCheckCase::AntiResonance).unwrap().sim;
        match n.plant {
            SimPlant::Frf { block_frames, .. } => assert_eq!(block_frames, Some(256)),
            other => panic!("the notch is an FRF plant, not {other:?}"),
        }
    }
}
