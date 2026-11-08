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
use hertzforge_random_laws::{CheckCase, CheckRecord, CHECK_FORMAT};
use hertzforge_sdm::{MatrixStack, C64};
use hertzforge_station::{BackendKind, SimPlant, SimPlantSpec, StationSpec};
use hertzforge_twr::TwrLawFactory;
use hertzforge_twr_laws::{
    TwrCaseEnd, TwrCaseMeasure, TwrCheckCase, TwrCheckPlant, TWR_CHECK_FS_HZ, TWR_CHECK_VERSION,
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
        "tool_version": TWR_CHECK_VERSION,
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

/// The lines of a notch plant's FRF, one drive to one control input, at `df_hz`
/// to half the check's rate: `(s^2 + w s / qz + w^2) / (s^2 + w s / qp + w^2)`,
/// whose gain at the notch is `qp / qz`.
fn notch_lines(f_hz: f64, depth_db: f64, df_hz: f64) -> Vec<(f64, f64)> {
    let qp = 2.0;
    let qz = qp * 10f64.powf(depth_db / 20.0);
    let w0 = 2.0 * std::f64::consts::PI * f_hz;
    let n = (TWR_CHECK_FS_HZ / 2.0 / df_hz).floor() as usize + 1;
    (0..n)
        .map(|k| {
            let w = 2.0 * std::f64::consts::PI * k as f64 * df_hz;
            let re = w0 * w0 - w * w;
            // Zeros over poles at s = jw.
            let (zr, zi) = (re, w * w0 / qz);
            let (pr, pi) = (re, w * w0 / qp);
            let d = pr * pr + pi * pi;
            if d == 0.0 {
                (1.0, 0.0)
            } else {
                ((zr * pr + zi * pi) / d, (zi * pr - zr * pi) / d)
            }
        })
        .collect()
}

/// The simulated station a case runs on: one drive and one control input at the
/// check's rate, from the case's own plant, noise and lookahead.
pub fn case_station(case: TwrCheckCase) -> Result<StationSpec, String> {
    let fs = TWR_CHECK_FS_HZ;
    let base = SimPlantSpec {
        sample_rate_hz: fs,
        resonances: Vec::new(),
        noise_level: 0.0,
        feedthrough: 1.0,
        transport_delay_samples: 0,
        seed: 0x7A11_5EED,
        max_input_channels: 1,
        max_output_channels: 1,
        shared_clock: true,
        plant: SimPlant::Resonators,
        output_lookahead_frames: None,
    };
    let sim = match case.plant() {
        TwrCheckPlant::Wire => base,
        TwrCheckPlant::OneMode { f_hz, q } => SimPlantSpec {
            resonances: vec![(f_hz, q)],
            feedthrough: 0.0,
            ..base
        },
        TwrCheckPlant::Station { f_hz, q } => SimPlantSpec {
            resonances: vec![(f_hz[0], q), (f_hz[1], q)],
            feedthrough: 0.0,
            ..base
        },
        TwrCheckPlant::DelayedMode { f_hz, q, delay_s } => SimPlantSpec {
            resonances: vec![(f_hz, q)],
            feedthrough: 0.0,
            transport_delay_samples: (delay_s * fs).round() as usize,
            ..base
        },
        TwrCheckPlant::DeepQueue {
            delay_samples,
            lookahead_frames,
            ..
        } => SimPlantSpec {
            transport_delay_samples: delay_samples,
            output_lookahead_frames: Some(lookahead_frames),
            ..base
        },
        TwrCheckPlant::Notch { f_hz, depth_db } => {
            let df_hz = 1.0;
            let lines = notch_lines(f_hz, depth_db, df_hz);
            let mut stack = MatrixStack::zeros(lines.len(), 1, 1);
            for (k, &(re, im)) in lines.iter().enumerate() {
                stack.set(k, 0, 0, C64::new(re, im));
            }
            SimPlantSpec {
                plant: SimPlant::Frf {
                    stack,
                    df_hz,
                    block_frames: Some(256),
                },
                ..base
            }
        }
    };
    let noise = case.noise_below_db().map_or(sim.noise_level, |db| {
        // The response's rms on the check's reference is about a third of its
        // 1 g peak; the noise sits `db` under it.
        0.3 * 10f64.powf(-db / 20.0)
    });
    Ok(StationSpec {
        sim: SimPlantSpec {
            noise_level: noise,
            ..sim
        },
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

/// The check record of a TWR law file's result.
pub fn check_record(sha256: String, date: String, cases: Vec<CheckCase>) -> CheckRecord {
    CheckRecord {
        format: CHECK_FORMAT,
        law_sha256: sha256,
        tool_version: TWR_CHECK_VERSION.to_owned(),
        date,
        passed: !cases.is_empty() && cases.iter().all(|c| c.passed),
        cases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_has_a_station_with_its_own_plant() {
        for case in TwrCheckCase::ALL {
            let s = case_station(case).unwrap().sim;
            assert_eq!(s.sample_rate_hz, TWR_CHECK_FS_HZ, "{}", case.name());
            assert_eq!(s.max_output_channels, 1);
        }
        let q = case_station(TwrCheckCase::DeepQueue).unwrap().sim;
        assert_eq!(q.output_lookahead_frames, Some(512));
        assert_eq!(q.transport_delay_samples, 41);
        let d = case_station(TwrCheckCase::Delay).unwrap().sim;
        assert_eq!(d.transport_delay_samples, 26);
        let n = case_station(TwrCheckCase::AntiResonance).unwrap().sim;
        assert!(matches!(n.plant, SimPlant::Frf { .. }));
        // The notch is 30 dB deep at 300 Hz and flat far from it.
        let lines = notch_lines(300.0, 30.0, 1.0);
        let mag = |c: (f64, f64)| (c.0 * c.0 + c.1 * c.1).sqrt();
        assert!((20.0 * mag(lines[300]).log10() + 30.0).abs() < 0.01);
        assert!((mag(lines[3000]) - 1.0).abs() < 0.01);
        assert!(case_station(TwrCheckCase::Noise).unwrap().sim.noise_level > 0.0);
        assert_eq!(
            case_station(TwrCheckCase::Flat).unwrap().sim.noise_level,
            0.0
        );
    }
}
