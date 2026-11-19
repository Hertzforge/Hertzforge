//! The MIMO Shock law check: the cases a law is run on before it may drive
//! hardware, what each case asks of a law, and the judge that reads a run's
//! per-pulse reports and answers pass or fail with the number that decided it.
//!
//! The requirements are written for a good law, not for the laws that exist.
//! A built-in law may fail a case; that is recorded as a failure and said in
//! the law's facts ([`crate::facts`]), never hidden by easing the requirement.
//!
//! The runner that plays a case through the engine on the simulator is the
//! caller's: [`run_check`] hands it each [`CaseSpec`] and judges what comes
//! back.
//!
//! # Every case asks
//!
//! 1. **`no_law_fault`**: every law answer is used: no pulse reports a law
//!    miss, and the run does not end on a law fault.
//! 2. **`no_abort`**: the run does not end in an abort, a safe stop or a fault
//!    (the core's divergence watch ends a run in a safe stop).
//! 3. **`reaches_full_level`**: a pulse is fired at full level (0 dB), except
//!    on the limit-channel case, where the limit may hold the run below it.
//! 4. **`converges`**: on the last level fired, the last pulse passes jointly
//!    (every control inside its tolerance). The number is the worst control's
//!    error on that pulse.
//! 5. **`holds`**: at the last level fired, the worst control's error on the
//!    last pulse is no more than [`HOLD_DB`] above its best at that level: a
//!    law that found the target and wandered off fails.
//! 6. **`drive_bounded`**: no drive's peak on any pulse at the last level is
//!    more than [`DRIVE_GROWTH_DB`] above its peak on the first pulse at that
//!    level.

use hertzforge_mimo_shock::{EndOutcome, PulseReport, TargetKind};
use hertzforge_random_laws::{CheckCase, CheckRecord};
use serde::{Deserialize, Serialize};

/// The version of the MIMO Shock check, recorded in every check record.
pub const MIMO_SHOCK_CHECK_VERSION: &str = "mimo-shock-check 1";

/// How far the worst control's error may rise, at the last level, above its
/// best at that level, dB.
pub const HOLD_DB: f64 = 1.0;

/// How far a drive's peak may grow over a level's pulses, dB.
pub const DRIVE_GROWTH_DB: f64 = 6.0;

/// The level ladder every case runs, dB.
pub const CHECK_LADDER_DB: [f64; 5] = [-12.0, -9.0, -6.0, -3.0, 0.0];

/// Pulses a level may take before the run ends not converged.
pub const CHECK_PULSES_PER_LEVEL: u32 = 3;

/// The SRS tolerance every case judges an SRS target by, dB below and above.
pub const CHECK_SRS_TOLERANCE_DB: (f64, f64) = (-1.5, 3.0);

/// One case of the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoShockCheckCase {
    /// Four drives, four controls, a linear coupled structure.
    Linear4x4,
    /// The 4 x 4 structure with moderately level-dependent joints.
    Jointed4x4Moderate,
    /// The 4 x 4 structure with strongly level-dependent joints.
    Jointed4x4Severe,
    /// Six controls on three drives with jointed behaviour: more controls than
    /// drives.
    Jointed6x3,
    /// The linear 4 x 4 with the law's model wrong by a delay.
    DelayError,
    /// The moderate jointed 4 x 4 with the model identified at low level and
    /// its gain wrong.
    IdentificationError,
    /// The moderate jointed 4 x 4 with noise on every measured response.
    MeasurementNoise,
    /// The linear 4 x 4 with a limit channel that binds before full level.
    LimitChannel,
}

/// What a case does to the plant or the measurement beyond the plant itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Perturbation {
    /// Nothing: the law is given the plant as identified.
    None,
    /// The model the law is given is the identified plant delayed by this
    /// much on every path (`H exp(-j 2 pi f delay)`), s.
    ModelDelay {
        /// The delay error, s.
        delay_s: f64,
    },
    /// The plant is identified at `identify_db` below the reference level, and
    /// the model the law is given is scaled by `gain_db` on every path.
    Identification {
        /// The identification level, dB under the reference.
        identify_db: f64,
        /// The gain error, dB.
        gain_db: f64,
    },
    /// Independent Gaussian noise on every measured response, its rms this
    /// fraction of the control's full-level target rms (the same at every
    /// level).
    ResponseNoise {
        /// The noise rms over the target rms.
        rms_rel: f64,
    },
    /// One monitored output (the plant's last output, measured but not
    /// controlled) with an abort peak at this fraction of the peak it reaches
    /// at full level under the least-energy drive, so it binds from the -3 dB
    /// level up.
    LimitChannel {
        /// The abort peak over the full-level peak.
        abort_rel: f64,
    },
}

/// One case of the check for one target kind: what the runner plays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseSpec {
    /// The case.
    pub case: MimoShockCheckCase,
    /// The target kind it is run with.
    pub kind: TargetKind,
    /// The simulator's standard shock plant, by name.
    pub plant: &'static str,
    /// What is changed beyond the plant.
    pub perturbation: Perturbation,
    /// The level ladder, dB.
    pub ladder_db: Vec<f64>,
    /// Pulses per level before the run ends not converged.
    pub pulses_per_level: u32,
    /// The SRS tolerance for an SRS target, dB below and above.
    pub srs_tolerance_db: (f64, f64),
    /// The seed of every random draw in the run.
    pub seed: u64,
}

impl MimoShockCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [MimoShockCheckCase; 8] = [
        MimoShockCheckCase::Linear4x4,
        MimoShockCheckCase::Jointed4x4Moderate,
        MimoShockCheckCase::Jointed4x4Severe,
        MimoShockCheckCase::Jointed6x3,
        MimoShockCheckCase::DelayError,
        MimoShockCheckCase::IdentificationError,
        MimoShockCheckCase::MeasurementNoise,
        MimoShockCheckCase::LimitChannel,
    ];

    /// The stable name.
    pub fn name(self) -> &'static str {
        match self {
            MimoShockCheckCase::Linear4x4 => "linear_4x4",
            MimoShockCheckCase::Jointed4x4Moderate => "jointed_4x4_moderate",
            MimoShockCheckCase::Jointed4x4Severe => "jointed_4x4_severe",
            MimoShockCheckCase::Jointed6x3 => "jointed_6x3",
            MimoShockCheckCase::DelayError => "delay_error",
            MimoShockCheckCase::IdentificationError => "identification_error",
            MimoShockCheckCase::MeasurementNoise => "measurement_noise",
            MimoShockCheckCase::LimitChannel => "limit_channel",
        }
    }

    /// The case with `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What the case is, in words, for the check report.
    pub fn describe(self) -> &'static str {
        match self {
            MimoShockCheckCase::Linear4x4 => {
                "four drives and four controls on a linear coupled structure"
            }
            MimoShockCheckCase::Jointed4x4Moderate => {
                "the 4 x 4 structure whose joints soften and damp moderately with level"
            }
            MimoShockCheckCase::Jointed4x4Severe => {
                "the 4 x 4 structure whose joints soften and damp strongly with level"
            }
            MimoShockCheckCase::Jointed6x3 => {
                "six controls on three drives, jointed: more controls than drives"
            }
            MimoShockCheckCase::DelayError => "the law's model is wrong by a 0.5 ms delay",
            MimoShockCheckCase::IdentificationError => {
                "the plant identified 20 dB under the reference and the model 2 dB high"
            }
            MimoShockCheckCase::MeasurementNoise => {
                "noise at 3 % of the target rms on every measured response"
            }
            MimoShockCheckCase::LimitChannel => {
                "a limit channel that binds from the -3 dB level, so full level is not reached"
            }
        }
    }

    /// The simulator's standard shock plant the case runs on.
    pub fn plant(self) -> &'static str {
        match self {
            MimoShockCheckCase::Linear4x4
            | MimoShockCheckCase::DelayError
            | MimoShockCheckCase::LimitChannel => "shock-linear-4x4",
            MimoShockCheckCase::Jointed4x4Moderate
            | MimoShockCheckCase::IdentificationError
            | MimoShockCheckCase::MeasurementNoise => "shock-jointed-4x4-moderate",
            MimoShockCheckCase::Jointed4x4Severe => "shock-jointed-4x4-severe",
            MimoShockCheckCase::Jointed6x3 => "shock-jointed-6x3",
        }
    }

    /// What the case changes beyond the plant.
    pub fn perturbation(self) -> Perturbation {
        match self {
            MimoShockCheckCase::DelayError => Perturbation::ModelDelay { delay_s: 0.5e-3 },
            MimoShockCheckCase::IdentificationError => Perturbation::Identification {
                identify_db: -20.0,
                gain_db: 2.0,
            },
            MimoShockCheckCase::MeasurementNoise => Perturbation::ResponseNoise { rms_rel: 0.03 },
            MimoShockCheckCase::LimitChannel => Perturbation::LimitChannel { abort_rel: 0.6 },
            _ => Perturbation::None,
        }
    }

    /// Whether the case requires a pulse at full level.
    pub fn needs_full_level(self) -> bool {
        self != MimoShockCheckCase::LimitChannel
    }

    /// The case for a target kind.
    pub fn spec(self, kind: TargetKind) -> CaseSpec {
        CaseSpec {
            case: self,
            kind,
            plant: self.plant(),
            perturbation: self.perturbation(),
            ladder_db: CHECK_LADDER_DB.to_vec(),
            pulses_per_level: CHECK_PULSES_PER_LEVEL,
            srs_tolerance_db: CHECK_SRS_TOLERANCE_DB,
            seed: 0x5eed_0000 + self as u64,
        }
    }
}

/// What a case's run produced: every fired pulse's report and how the run
/// ended.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseRun {
    /// Every fired pulse, in order.
    pub pulses: Vec<PulseReport>,
    /// How the run ended.
    pub end: EndOutcome,
}

/// The judgement of one case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseVerdict {
    /// The case.
    pub case: MimoShockCheckCase,
    /// The target kind.
    pub kind: TargetKind,
    /// Whether every requirement held.
    pub passed: bool,
    /// The first requirement that failed, by name, when one did.
    pub failed: Option<String>,
    /// The number that decided it: for a pass, the worst control's error on
    /// the last pulse, dB; for a failure, the failing requirement's own
    /// number.
    pub number: f64,
    /// What was measured, in words.
    pub detail: String,
}

impl CaseVerdict {
    /// The record's name for the case: `<case> (<kind>)`.
    pub fn record_name(&self) -> String {
        format!("{} ({})", self.case.name(), self.kind.as_str())
    }
}

fn worst_db(p: &PulseReport) -> f64 {
    p.controls
        .iter()
        .map(|c| c.verdict.error_db)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Judge a case's run against the requirements of the module.
pub fn judge(spec: &CaseSpec, run: &CaseRun) -> CaseVerdict {
    let verdict = |failed: Option<&str>, number: f64, detail: String| CaseVerdict {
        case: spec.case,
        kind: spec.kind,
        passed: failed.is_none(),
        failed: failed.map(str::to_string),
        number,
        detail,
    };
    if let Some((i, p)) = run
        .pulses
        .iter()
        .enumerate()
        .find(|(_, p)| p.law_miss.is_some())
    {
        return verdict(
            Some("no_law_fault"),
            i as f64,
            format!(
                "pulse {i}: the law's answer was not used ({})",
                p.law_miss.as_deref().unwrap_or("")
            ),
        );
    }
    match run.end {
        EndOutcome::LawFault => {
            return verdict(
                Some("no_law_fault"),
                run.pulses.len() as f64,
                "the run ended on a law fault".into(),
            )
        }
        EndOutcome::Aborted | EndOutcome::SafeStop | EndOutcome::Fault => {
            return verdict(
                Some("no_abort"),
                run.pulses.len() as f64,
                format!(
                    "the run ended {:?} after {} pulses",
                    run.end,
                    run.pulses.len()
                ),
            )
        }
        _ => {}
    }
    let Some(last) = run.pulses.last() else {
        return verdict(Some("reaches_full_level"), 0.0, "no pulse was fired".into());
    };
    let top = last.level_db;
    if spec.case.needs_full_level() && !run.pulses.iter().any(|p| p.level_db >= -1e-9) {
        return verdict(
            Some("reaches_full_level"),
            top,
            format!("the highest level fired was {top:.1} dB"),
        );
    }
    let at_top: Vec<&PulseReport> = run
        .pulses
        .iter()
        .filter(|p| (p.level_db - top).abs() < 1e-9)
        .collect();
    let last_db = worst_db(last);
    if !last.passed {
        return verdict(
            Some("converges"),
            last_db,
            format!(
                "the last pulse at {top:.1} dB did not pass: worst control error {last_db:.2} dB"
            ),
        );
    }
    let best = at_top
        .iter()
        .map(|p| worst_db(p))
        .fold(f64::INFINITY, f64::min);
    if last_db > best + HOLD_DB {
        return verdict(
            Some("holds"),
            last_db - best,
            format!(
                "the worst control error rose {:.2} dB above its best at {top:.1} dB",
                last_db - best
            ),
        );
    }
    let first = at_top[0];
    let mut growth = f64::NEG_INFINITY;
    for p in &at_top {
        for (d, (&v, &v0)) in p.drive_vpk.iter().zip(&first.drive_vpk).enumerate() {
            let g = 20.0 * (v.max(1e-300) / v0.max(1e-300)).log10();
            if g > growth {
                growth = g;
            }
            if g > DRIVE_GROWTH_DB {
                return verdict(
                    Some("drive_bounded"),
                    g,
                    format!("drive {d} grew {g:.2} dB over the pulses at {top:.1} dB"),
                );
            }
        }
    }
    verdict(
        None,
        last_db,
        format!(
            "passed at {top:.1} dB after {} pulses; worst control error {last_db:.2} dB, \
             largest drive growth {growth:.2} dB",
            run.pulses.len()
        ),
    )
}

/// Run every case for every kind in `kinds` with `run_case`, which plays one
/// case through the engine on the simulator and returns its pulses, and judge
/// each. A case the runner could not run is a failure named `ran`.
pub fn run_check(
    kinds: &[TargetKind],
    mut run_case: impl FnMut(&CaseSpec) -> Result<CaseRun, String>,
) -> Vec<CaseVerdict> {
    let mut out = Vec::new();
    for case in MimoShockCheckCase::ALL {
        for &kind in kinds {
            let spec = case.spec(kind);
            out.push(match run_case(&spec) {
                Ok(run) => judge(&spec, &run),
                Err(why) => CaseVerdict {
                    case,
                    kind,
                    passed: false,
                    failed: Some("ran".into()),
                    number: f64::NAN,
                    detail: format!("the case could not be run: {why}"),
                },
            });
        }
    }
    out
}

/// The check record of `verdicts` for the law file `law_sha256`, dated
/// `date` (RFC 3339).
pub fn check_record(law_sha256: &str, date: &str, verdicts: &[CaseVerdict]) -> CheckRecord {
    CheckRecord::new(
        law_sha256,
        MIMO_SHOCK_CHECK_VERSION,
        date,
        verdicts
            .iter()
            .map(|v| CheckCase {
                name: v.record_name(),
                passed: v.passed,
                detail: match &v.failed {
                    None => v.detail.clone(),
                    Some(rule) => format!("{rule}: {} ({})", v.detail, v.number),
                },
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_mimo_shock::{ControlReport, ControlVerdict};

    fn pulse(index: u64, level_db: f64, errs: &[f64], passed: bool, vpk: f64) -> PulseReport {
        PulseReport {
            index,
            scheduled_db: level_db,
            level_db,
            limit_scale: vec![1.0; 2],
            drive_vpk: vec![vpk, vpk],
            controls: errs
                .iter()
                .enumerate()
                .map(|(c, &e)| ControlReport {
                    channel: c as u32,
                    verdict: ControlVerdict {
                        passed,
                        error_db: e,
                        delay_s: 0.0,
                        peak_ratio: 1.0,
                    },
                    peak: 1.0,
                    waveform_error_db: e,
                    velocity_ips: 0.0,
                    displacement_pk_pk_in: 0.0,
                    timing_ok: true,
                    shape_ok: passed,
                    over_test_db: 0.0,
                    polarity_ok: true,
                    duration_ratio: None,
                    duration_ok: true,
                    energy_db: 0.0,
                    rms_duration_ratio: 1.0,
                    scaled_margin_db: -6.0,
                    absolute_margin_db: -6.0,
                })
                .collect(),
            passed,
            global_srs_error_db: None,
            at_s: index as f64,
            law_used: true,
            law_miss: None,
            law_values: Vec::new(),
            predicted_peak_ratio: Vec::new(),
            end: None,
            over_test_db: 0.0,
            arrival_spread_s: 0.0,
            warnings: Vec::new(),
            step_note: None,
        }
    }

    fn good() -> CaseRun {
        let mut pulses = Vec::new();
        for (k, l) in CHECK_LADDER_DB.iter().enumerate() {
            pulses.push(pulse(
                k as u64,
                *l,
                &[2.0, 1.5],
                *l == 0.0,
                1.0 + k as f64 * 0.1,
            ));
        }
        CaseRun {
            pulses,
            end: EndOutcome::Completed,
        }
    }

    fn failed(spec: &CaseSpec, run: &CaseRun) -> Option<String> {
        judge(spec, run).failed
    }

    #[test]
    fn mimo_shock_check_cases_are_named_and_cover_the_design() {
        let names: Vec<&str> = MimoShockCheckCase::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            [
                "linear_4x4",
                "jointed_4x4_moderate",
                "jointed_4x4_severe",
                "jointed_6x3",
                "delay_error",
                "identification_error",
                "measurement_noise",
                "limit_channel"
            ]
        );
        for c in MimoShockCheckCase::ALL {
            assert_eq!(MimoShockCheckCase::from_name(c.name()), Some(c));
            assert!(c.plant().starts_with("shock-"));
            assert_ne!(c.spec(TargetKind::Srs).seed, 0);
        }
    }

    #[test]
    fn mimo_shock_check_a_clean_run_passes_with_its_worst_error() {
        let spec = MimoShockCheckCase::Linear4x4.spec(TargetKind::Srs);
        let v = judge(&spec, &good());
        assert!(v.passed, "{v:?}");
        assert_eq!(v.number, 2.0);
    }

    #[test]
    fn mimo_shock_check_each_requirement_fails_by_name() {
        let spec = MimoShockCheckCase::Linear4x4.spec(TargetKind::Waveform);
        let mut r = good();
        r.pulses[2].law_miss = Some("over_budget".into());
        assert_eq!(failed(&spec, &r).as_deref(), Some("no_law_fault"));
        let mut r = good();
        r.end = EndOutcome::LawFault;
        assert_eq!(failed(&spec, &r).as_deref(), Some("no_law_fault"));
        for end in [EndOutcome::Aborted, EndOutcome::SafeStop, EndOutcome::Fault] {
            let mut r = good();
            r.end = end;
            assert_eq!(failed(&spec, &r).as_deref(), Some("no_abort"), "{end:?}");
        }
        let mut r = good();
        r.pulses.pop();
        r.end = EndOutcome::NotConverged;
        assert_eq!(failed(&spec, &r).as_deref(), Some("reaches_full_level"));
        let r = CaseRun {
            pulses: Vec::new(),
            end: EndOutcome::NotConverged,
        };
        assert_eq!(failed(&spec, &r).as_deref(), Some("reaches_full_level"));
        let mut r = good();
        let last = r.pulses.len() - 1;
        r.pulses[last].passed = false;
        let v = judge(&spec, &r);
        assert_eq!(v.failed.as_deref(), Some("converges"));
        assert_eq!(v.number, 2.0);
        // Found the target at full level, then wandered off by more than 1 dB.
        let mut r = good();
        r.pulses.push(pulse(5, 0.0, &[0.5, 0.2], true, 1.4));
        r.pulses.push(pulse(6, 0.0, &[1.8, 0.2], true, 1.4));
        let v = judge(&spec, &r);
        assert_eq!(v.failed.as_deref(), Some("holds"));
        assert!((v.number - 1.3).abs() < 1e-12);
        // Drive growth at the last level past 6 dB.
        let mut r = good();
        r.pulses.push(pulse(5, 0.0, &[2.0, 1.5], true, 1.4 * 2.1));
        let v = judge(&spec, &r);
        assert_eq!(v.failed.as_deref(), Some("drive_bounded"));
        assert!(v.number > DRIVE_GROWTH_DB);
    }

    #[test]
    fn mimo_shock_check_the_limit_case_is_judged_at_the_level_reached() {
        let spec = MimoShockCheckCase::LimitChannel.spec(TargetKind::Srs);
        let mut r = good();
        r.pulses.truncate(4);
        let last = r.pulses.len() - 1;
        r.pulses[last].passed = true;
        r.end = EndOutcome::NotConverged;
        assert!(judge(&spec, &r).passed);
        r.pulses[last].passed = false;
        assert_eq!(failed(&spec, &r).as_deref(), Some("converges"));
    }

    #[test]
    fn mimo_shock_check_runs_every_case_for_every_kind_and_records_it() {
        let mut seen = Vec::new();
        let v = run_check(&[TargetKind::Waveform, TargetKind::Srs], |s| {
            seen.push((s.case, s.kind));
            if s.case == MimoShockCheckCase::Jointed4x4Severe {
                Err("no plant".into())
            } else {
                Ok(good())
            }
        });
        assert_eq!(v.len(), 16);
        assert_eq!(seen.len(), 16);
        let severe: Vec<_> = v
            .iter()
            .filter(|v| v.case == MimoShockCheckCase::Jointed4x4Severe)
            .collect();
        assert!(severe.iter().all(|v| v.failed.as_deref() == Some("ran")));
        let rec = check_record(&"a".repeat(64), "2026-11-11T00:00:00Z", &v);
        assert!(!rec.passed);
        assert_eq!(rec.tool_version, MIMO_SHOCK_CHECK_VERSION);
        assert_eq!(rec.cases[0].name, "linear_4x4 (waveform)");
        rec.validate().unwrap();
    }
}
