//! `laws.check` for a Sine law (schema/PROTOCOL.md, "Control Laws", "A Sine law"):
//! the simulated test articles the check runs on, built through the controller's
//! station factory, each case run under the core's guards by the controller's own
//! check runner, and the result on the wire.
//!
//! What a case asks, the station it runs on and how a run is judged are the
//! controller's (`hertzforge_sine_laws::SineCheckCase`, `sine_check_station`,
//! `judge_sine`); this module builds each case's simulated station through the
//! station factory and says the result in the protocol's words.

use crate::protocol::{num, opt_num, ErrorCode, MethodError};
use hertzforge_random_laws::{CheckCase, CheckRecord, CHECK_FORMAT};
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_sine::SineLawFactory;
use hertzforge_sine_laws::{
    sine_check_station, SineCaseEnd, SineCaseMeasure, SineCheckCase, SineCheckStation,
    SINE_CHECK_FS_HZ, SINE_CHECK_VERSION,
};
use hertzforge_station::{BackendKind, SimPlant, SimPlantSpec, StationSpec};
use serde_json::{json, Value};
use std::sync::Arc;

fn r(x: f64, d: i32) -> f64 {
    super::config::round(x, d)
}

/// The test article in words: the case's own.
pub fn plant_words(case: SineCheckCase) -> String {
    case.plant().to_string()
}

/// How a case's run ended, as a run outcome.
pub fn end_name(e: &SineCaseEnd) -> &'static str {
    match e {
        SineCaseEnd::Completed => "completed",
        SineCaseEnd::Aborted(_) => "aborted",
        SineCaseEnd::LawFault(_) => "law_fault",
        SineCaseEnd::Held => "held",
        SineCaseEnd::Fault(_) => "fault",
    }
}

/// What tripped, in words, when anything did.
pub fn trip_name(e: &SineCaseEnd) -> Option<String> {
    match e {
        SineCaseEnd::Completed | SineCaseEnd::Held => None,
        SineCaseEnd::Aborted(k) | SineCaseEnd::LawFault(k) | SineCaseEnd::Fault(k) => {
            Some(k.clone())
        }
    }
}

/// One case's result on the wire.
pub fn case_json(case: SineCheckCase, m: &SineCaseMeasure, v: &CheckCase) -> Value {
    let rungs = if m.rungs.is_empty() {
        Value::Null
    } else {
        Value::Array(
            m.rungs
                .iter()
                .map(|(l, e)| json!([num(*l), opt_num(e.map(|x| r(x, 3)))]))
                .collect(),
        )
    };
    json!({
        "name": case.name(),
        "plant": plant_words(case),
        "requirement": case.requirement().as_str(),
        "passed": v.passed,
        "detail": v.detail,
        "end": end_name(&m.end),
        "trip": trip_name(&m.end),
        "ticks": m.ticks,
        "judged_ticks": m.judged_ticks,
        "worst_error_db": opt_num(m.worst_error_db.map(|x| r(x, 3))),
        "worst_error_hz": opt_num(m.worst_error_hz.map(|x| r(x, 2))),
        "worst_away_error_db": opt_num(m.worst_away_error_db.map(|x| r(x, 3))),
        "rungs": rungs,
        "worst_limit_over_db": opt_num(m.worst_limit_over_db.map(|x| r(x, 3))),
        "worst_release_overshoot_db": opt_num(m.worst_release_overshoot_db.map(|x| r(x, 3))),
        "calls": m.calls,
        "misses": m.misses,
        "refused": m.refused,
        "capped_ticks": m.capped_ticks,
        "worst_call_ms": opt_num(m.worst_call_ms.map(|x| r(x, 3))),
        "budget_ms": num(r(m.budget_ms, 3)),
        "max_drive_vpk": num(r(m.max_drive_vpk, 4)),
    })
}

/// The `laws.check` report of a Sine law.
pub fn report_json(cases: &[Value], passed: bool, recorded: bool) -> Value {
    json!({
        "passed": passed,
        "recorded": recorded,
        "tool_version": SINE_CHECK_VERSION,
        "cases": cases,
    })
}

/// One `laws.check` progress event: a case starting (`result` null) or done.
pub fn progress_json(
    law_id: &str,
    index: usize,
    case: SineCheckCase,
    result: Option<Value>,
) -> Value {
    json!({
        "law_id": law_id,
        "index": index,
        "of": SineCheckCase::ALL.len(),
        "case": case.name(),
        "state": if result.is_some() { "done" } else { "running" },
        "result": result.unwrap_or(Value::Null),
    })
}

/// The simulated station a case runs on, from the check runner's own facts
/// (`sine_check_station`): the resonator bank on its stated feedthrough, or the FRF
/// plant (an anti-resonance, a limit channel with its own gain), with the case's
/// measurement noise, one drive and the case's inputs, at the check's rate.
pub fn case_station(case: SineCheckCase) -> Result<StationSpec, String> {
    let spec = sine_check_station(case, SINE_CHECK_FS_HZ);
    let base = SimPlantSpec {
        sample_rate_hz: SINE_CHECK_FS_HZ,
        resonances: Vec::new(),
        noise_level: spec.noise_rms_g,
        feedthrough: 0.0,
        transport_delay_samples: spec.transport_delay_samples,
        seed: 0x51E5_5EED,
        // The simulator's default, as the runner's own reference station has it.
        max_input_channels: 8,
        max_output_channels: 1,
        shared_clock: true,
        plant: SimPlant::Resonators,
    };
    let sim = match spec.station {
        SineCheckStation::Resonators { modes, feedthrough } => SimPlantSpec {
            resonances: modes,
            feedthrough,
            ..base
        },
        SineCheckStation::Frf {
            df_hz,
            lines,
            block_frames,
        } => {
            let n_lines = lines.first().map_or(0, Vec::len);
            if n_lines == 0 {
                return Err(format!("case \"{}\" states no plant lines", case.name()));
            }
            let mut stack = MatrixStack::zeros(n_lines, lines.len(), 1);
            for (i, ch) in lines.iter().enumerate() {
                for (k, &(re, im)) in ch.iter().enumerate() {
                    stack.set(k, i, 0, C64::new(re, im));
                }
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
    factory: Arc<dyn SineLawFactory>,
    progress: &mut dyn FnMut(Value),
) -> Result<Vec<(Value, CheckCase)>, MethodError> {
    let unsupported = |case: SineCheckCase, why: String| {
        MethodError::named(
            ErrorCode::Refused,
            "law_check_unsupported",
            format!(
                "The Sine check could not run case \"{}\": {why}.",
                case.name()
            ),
        )
    };
    let mut out = Vec::with_capacity(SineCheckCase::ALL.len());
    for (i, case) in SineCheckCase::ALL.iter().copied().enumerate() {
        progress(progress_json(law_id, i, case, None));
        let spec = case_station(case).map_err(|why| unsupported(case, why))?;
        let station = hertzforge_station::try_build_station_backend(BackendKind::Sim, &spec)
            .map_err(crate::station::build_refusal)?;
        let measure = hertzforge_sine_laws::run_check_case(case, factory.clone(), station.backend)
            .map_err(|e| unsupported(case, e.to_string()))?;
        let verdict = hertzforge_sine_laws::judge_sine(case, &measure);
        let json = case_json(case, &measure, &verdict);
        progress(progress_json(law_id, i, case, Some(json.clone())));
        out.push((json, verdict));
    }
    Ok(out)
}

/// The check record of a Sine law file's result.
pub fn check_record(sha256: String, date: String, cases: Vec<CheckCase>) -> CheckRecord {
    CheckRecord {
        format: CHECK_FORMAT,
        law_sha256: sha256,
        tool_version: SINE_CHECK_VERSION.to_owned(),
        date,
        passed: !cases.is_empty() && cases.iter().all(|c| c.passed),
        cases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_is_said_in_the_runners_words() {
        let words: Vec<String> = SineCheckCase::ALL.iter().map(|c| plant_words(*c)).collect();
        assert_eq!(
            words,
            [
                "one mode, 250 Hz, Q 50",
                "one mode, 250 Hz, Q 50",
                "two modes, 240 and 260 Hz, Q 30",
                "a 30 dB anti-resonance at 300 Hz",
                "one mode, 250 Hz, Q 50",
                "one mode, 250 Hz, Q 50",
                "one mode, 250 Hz, Q 50",
                "a wire behind 50 ms",
            ]
        );
        let s = case_station(SineCheckCase::CloseModes).unwrap().sim;
        assert_eq!(s.resonances, vec![(240.0, 30.0), (260.0, 30.0)]);
        assert_eq!(s.feedthrough, hertzforge_sine_laws::SINE_CHECK_FEEDTHROUGH);
        assert_eq!(s.sample_rate_hz, SINE_CHECK_FS_HZ);
        // The test articles the resonator bank cannot play are the runner's FRF plants.
        for case in [SineCheckCase::AntiResonance, SineCheckCase::Notch] {
            let s = case_station(case).unwrap().sim;
            assert!(matches!(s.plant, SimPlant::Frf { .. }), "{}", case.name());
        }
        // Every case has a station, and the noise case its stated noise.
        for case in SineCheckCase::ALL {
            assert!(case_station(case).is_ok(), "{}", case.name());
        }
        let noise = case_station(SineCheckCase::Noise).unwrap().sim.noise_level;
        assert_eq!(Some(noise), SineCheckCase::Noise.noise_rms_g());
        // The transport delay case's wire plays the drive 50 ms late; no other
        // case's station has a lag.
        for case in SineCheckCase::ALL {
            let s = case_station(case).unwrap().sim;
            let want = if case == SineCheckCase::Delay { 819 } else { 0 };
            assert_eq!(s.transport_delay_samples, want, "{}", case.name());
        }
        let s = case_station(SineCheckCase::Delay).unwrap().sim;
        assert!(s.resonances.is_empty() && s.feedthrough == 1.0);
    }
}
