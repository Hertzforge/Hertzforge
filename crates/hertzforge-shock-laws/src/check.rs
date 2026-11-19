//! The check of a classical shock law before it drives hardware: the fixed set
//! of simulated test articles, what a run on each is measured by, and the rules
//! that turn a measurement into a passed or failed case of a [`CheckRecord`].
//!
//! Every case is a whole run under the core's guards on a simulated station:
//! AUTO `[-12, -9, -6, -3, 0]` dB with `[1, 1, 1, 1, 3]` pulses, advancing on a
//! passing pulse, at most 5 pulses a rung (8 at the identification-error
//! case's first rung), MIL-STD-810 Method 516, a 1 g half-sine of 11 ms with
//! rectangular shoulders 4 times its duration unless the case says otherwise,
//! the drive ceiling [`CHECK_DRIVE_CEILING_V`] and abort
//! [`CHECK_DRIVE_ABORT_V`], identification without the review hold. The
//! spacing is the rate model's, on a virtual clock: a simulated station needs
//! no cooling. Every case asks the same of the law first: every call
//! answered, in time, with zero misses and zero refused outputs. Then each
//! case asks for its [`ClassicalRequirement`].
//!
//! The controller builds the simulated station each case names
//! ([`ClassicalCheckCase::plant`]) and hands it to [`run_classical_check_case`];
//! [`judge_classical`] holds everything that decides the verdict, so the rules
//! are the same whoever runs the plants.
//!
//! [`CheckRecord`]: hertzforge_random_laws::CheckRecord

use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::{CompensationConfig, CompensationShape};
use hertzforge_hal::Backend;
use hertzforge_random_laws::{CheckCase, CheckRecord};
use hertzforge_shock_common::{LadderAdvance, ShockLevelSchedule};
use hertzforge_shock_host::service::pacer::VirtualPacer;
use hertzforge_shock_host::{
    BackendPulseFirer, ClassicalControl, ClassicalLawFactory, ClassicalPulseSpec, ClassicalShape,
    ClassicalShockService, ClassicalTestConfig, RunState, TripKind,
};
use std::fmt;
use std::sync::Arc;

/// The check pulse's peak, g.
pub const CHECK_PULSE_G: f64 = 1.0;
/// The check runs' drive ceiling, volts peak.
pub const CHECK_DRIVE_CEILING_V: f64 = 10.0;
/// The check runs' drive abort, volts peak.
pub const CHECK_DRIVE_ABORT_V: f64 = 20.0;
/// The check runs' identification drive, volts rms: an eighth of the pulse's
/// peak, the bench's ratio (0.05 V rms for a 0.4 V pulse).
pub const CHECK_SYSID_DRIVE_VRMS: f64 = 0.125;

/// The version of the classical check written into its records. A change to
/// the plant set or to the rules changes it.
pub const CLASSICAL_CHECK_VERSION: &str = "shock-classical-check 1";

/// What a case asks of the law beyond answering every call, in time, with a
/// usable drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicalRequirement {
    /// Every pulse passes, the first included.
    EveryPulse,
    /// A passing pulse by pulse `by_pulse` (counted from 1) at the first rung;
    /// with `every_full_level`, every full-level pulse passes too.
    PassBy {
        /// The pulse by which one must have passed.
        by_pulse: u32,
        /// Every full-level pulse must pass as well.
        every_full_level: bool,
    },
    /// The run completes, or is ended by the core's own judgement
    /// (`not_converged`, `divergence`). Never over the drive ceiling, never an
    /// overshoot abort.
    Survive,
}

/// The simulated test article a case runs on. The controller builds a
/// one-drive, one-control simulated station from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClassicalCheckPlant {
    /// A wire: feedthrough 1, no modes.
    Wire,
    /// One mode.
    OneMode {
        /// Natural frequency, Hz.
        f_hz: f64,
        /// Quality factor.
        q: f64,
    },
    /// The simulated station's own plant: two modes.
    Station {
        /// The modes' natural frequencies, Hz.
        f_hz: [f64; 2],
        /// Their quality factor.
        q: f64,
    },
    /// One mode behind a transport delay.
    DelayedMode {
        /// Natural frequency, Hz.
        f_hz: f64,
        /// Quality factor.
        q: f64,
        /// The delay, seconds.
        delay_s: f64,
    },
    /// A deep notch, its coherence under the identification floor.
    Notch {
        /// The notch's frequency, Hz.
        f_hz: f64,
        /// Its depth, dB.
        depth_db: f64,
    },
}

/// One simulated test article or condition of the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClassicalCheckCase {
    /// A wire; the first pulse and every pulse pass.
    Flat,
    /// One mode at 400 Hz, Q 10.
    OneMode,
    /// The simulated station's plant (200 and 550 Hz, Q 2.5), a haversine of
    /// 6 ms.
    StationPlant,
    /// One mode plus a 2 ms transport delay.
    Delay,
    /// A wire, with the core's identified gain scaled +3 dB before inversion:
    /// a law that does not correct sits 29% low and cannot pass.
    IdentificationError,
    /// One mode with measurement noise 30 dB under the response.
    Noise,
    /// A 30 dB notch at 300 Hz with coherence under the floor.
    AntiResonance,
}

impl ClassicalCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [ClassicalCheckCase; 7] = [
        ClassicalCheckCase::Flat,
        ClassicalCheckCase::OneMode,
        ClassicalCheckCase::StationPlant,
        ClassicalCheckCase::Delay,
        ClassicalCheckCase::IdentificationError,
        ClassicalCheckCase::Noise,
        ClassicalCheckCase::AntiResonance,
    ];

    /// The case's name in a record and on screen.
    pub fn name(self) -> &'static str {
        match self {
            ClassicalCheckCase::Flat => "flat",
            ClassicalCheckCase::OneMode => "one mode",
            ClassicalCheckCase::StationPlant => "station plant",
            ClassicalCheckCase::Delay => "transport delay",
            ClassicalCheckCase::IdentificationError => "identification error",
            ClassicalCheckCase::Noise => "measurement noise",
            ClassicalCheckCase::AntiResonance => "anti-resonance",
        }
    }

    /// The simulated test article.
    pub fn plant(self) -> ClassicalCheckPlant {
        match self {
            ClassicalCheckCase::Flat | ClassicalCheckCase::IdentificationError => {
                ClassicalCheckPlant::Wire
            }
            ClassicalCheckCase::OneMode | ClassicalCheckCase::Noise => {
                ClassicalCheckPlant::OneMode {
                    f_hz: 400.0,
                    q: 10.0,
                }
            }
            ClassicalCheckCase::StationPlant => ClassicalCheckPlant::Station {
                f_hz: [200.0, 550.0],
                q: 2.5,
            },
            ClassicalCheckCase::Delay => ClassicalCheckPlant::DelayedMode {
                f_hz: 400.0,
                q: 10.0,
                delay_s: 0.002,
            },
            ClassicalCheckCase::AntiResonance => ClassicalCheckPlant::Notch {
                f_hz: 300.0,
                depth_db: 30.0,
            },
        }
    }

    /// The pulse shape and its duration, ms.
    pub fn pulse(self) -> (ClassicalShape, f64) {
        match self {
            ClassicalCheckCase::StationPlant => (ClassicalShape::Haversine, 6.0),
            _ => (ClassicalShape::HalfSine, 11.0),
        }
    }

    /// The factor, dB, the core scales its identified gain by before inversion
    /// (`None`: the identification as measured).
    pub fn identification_scale_db(self) -> Option<f64> {
        match self {
            ClassicalCheckCase::IdentificationError => Some(3.0),
            _ => None,
        }
    }

    /// The most pulses at one rung without a pass: 8 for the identification
    /// error (at beta 0.5 a 3 dB error leaves 0.65 of itself after each pulse,
    /// and a pulse passes once the error is under about 4%), 5 otherwise.
    pub fn max_pulses_per_level(self) -> u32 {
        match self {
            ClassicalCheckCase::IdentificationError => 8,
            _ => 5,
        }
    }

    /// Measurement noise, dB under the response (`None`: the simulator's floor
    /// only): Gaussian noise whose standard deviation is this many dB under the
    /// main pulse's RMS at the first rung on the station's control channel
    /// (`CHECK_PULSE_G / sqrt 2` at -12 dB for the half-sine on a wire). Taken at
    /// the first rung because that is where the case asks for its pass: the
    /// standard's envelope is 15% of the peak at every sample, and noise set
    /// against the full-level pulse would sit only 18 dB under a -12 dB pulse.
    pub fn noise_below_db(self) -> Option<f64> {
        match self {
            ClassicalCheckCase::Noise => Some(30.0),
            _ => None,
        }
    }

    /// What the case asks of the law.
    pub fn requirement(self) -> ClassicalRequirement {
        match self {
            ClassicalCheckCase::Flat => ClassicalRequirement::EveryPulse,
            ClassicalCheckCase::OneMode
            | ClassicalCheckCase::StationPlant
            | ClassicalCheckCase::Delay => ClassicalRequirement::PassBy {
                by_pulse: 3,
                every_full_level: true,
            },
            ClassicalCheckCase::IdentificationError => ClassicalRequirement::PassBy {
                by_pulse: 8,
                every_full_level: false,
            },
            ClassicalCheckCase::Noise => ClassicalRequirement::PassBy {
                by_pulse: 4,
                every_full_level: true,
            },
            ClassicalCheckCase::AntiResonance => ClassicalRequirement::Survive,
        }
    }
}

/// How a case's run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassicalCaseEnd {
    /// The schedule completed.
    Completed,
    /// The core's iteration limit ended it.
    NotConverged,
    /// The core's divergence judgement ended it.
    Divergence,
    /// A protection aborted it; the trip kind's wire name (`overshoot`,
    /// `drive_vpk`, `kinematic`, `tolerance`, ...).
    Aborted(String),
    /// The law's misses, or a refused first drive, ended it; the reason's wire
    /// name.
    LawFault(String),
    /// Identification held the run (coherence).
    Held,
    /// The run failed; the controller's words.
    Fault(String),
}

/// What one case's run is judged by.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassicalCaseMeasure {
    /// How the run ended.
    pub end: ClassicalCaseEnd,
    /// Pulses fired.
    pub pulses: u32,
    /// The first passing pulse, counted from 1, and the rung it was at
    /// (counted from 0); `None` when none passed.
    pub first_pass: Option<(u32, usize)>,
    /// Pulses that passed.
    pub pulses_passed: u32,
    /// Pulses fired at full level.
    pub full_level_pulses: u32,
    /// Full-level pulses that passed.
    pub full_level_passed: u32,
    /// Law calls made, the seed included.
    pub calls: u32,
    /// Calls whose output was not used.
    pub misses: u32,
    /// Calls whose output the core refused (a subset of `misses`).
    pub refused: u32,
    /// Drive bins the core's per-pulse cap held back, over the run.
    pub capped_bins: u64,
    /// Pulses whose drive peak the core's per-pulse cap held back.
    pub capped_peaks: u32,
    /// The slowest call, ms; `None` when no call was made.
    pub worst_call_ms: Option<f64>,
    /// One call's budget, ms.
    pub budget_ms: f64,
    /// The largest drive commanded, volts peak.
    pub max_drive_vpk: f64,
    /// The drive ceiling, volts peak.
    pub drive_ceiling_vpk: f64,
}

/// Why a case could not be run at all (as distinct from a run the law failed).
#[derive(Debug, Clone, PartialEq)]
pub enum CheckRunError {
    /// The case's run could not be set up on the backend it was handed.
    Setup(String),
}

impl fmt::Display for CheckRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckRunError::Setup(s) => write!(f, "the check case could not be set up: {s}"),
        }
    }
}

/// The run a case is: its pulse, schedule and closed-loop settings at the
/// station's rate `fs_hz`.
pub fn classical_check_config(case: ClassicalCheckCase, fs_hz: f64) -> ClassicalTestConfig {
    let (shape, ms) = case.pulse();
    let mut spec = ClassicalPulseSpec::new(shape, CHECK_PULSE_G, ms);
    spec.compensation = CompensationConfig {
        shape: CompensationShape::Rectangular,
        pre_frac: 4.0,
        post_frac: 4.0,
        symmetric: true,
    };
    let mut cfg = ClassicalTestConfig::new(spec);
    cfg.fs_hz = fs_hz;
    cfg.schedule = ShockLevelSchedule {
        pulses_per_level: vec![1, 1, 1, 1, 3],
        advance: LadderAdvance::OnPass,
        max_pulses_per_level: case.max_pulses_per_level(),
        ..ShockLevelSchedule::auto_default()
    };
    let mut control = ClassicalControl::new(fs_hz, CHECK_DRIVE_CEILING_V, CHECK_DRIVE_ABORT_V);
    control.review_hold = false;
    control.sysid.drive_rms = CHECK_SYSID_DRIVE_VRMS;
    cfg.control = Some(control);
    cfg
}

impl std::error::Error for CheckRunError {}

/// Run `case` with the law `factory` builds, under the core's guards, on
/// `backend` (the simulated station the controller built for
/// [`ClassicalCheckCase::plant`], with any [`ClassicalCheckCase::noise_below_db`]),
/// and measure it. The station is configured with one Control channel.
pub fn run_classical_check_case(
    case: ClassicalCheckCase,
    factory: Arc<dyn ClassicalLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<ClassicalCaseMeasure, CheckRunError> {
    let setup = |e: &dyn fmt::Display| CheckRunError::Setup(e.to_string());
    let fs_hz = backend.caps().sample_rate_hz;
    let table = ChannelTable::default_with_channels(1);
    let firer = BackendPulseFirer::new(
        backend,
        table.clone(),
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .map_err(|e| setup(&e))?;
    let cfg = classical_check_config(case, fs_hz);
    let max_pulses = cfg
        .control
        .as_ref()
        .map(|c| c.guards.max_pulses)
        .unwrap_or(100);
    let root = std::env::temp_dir().join("hertzforge-classical-check");
    let mut s = ClassicalShockService::new(cfg, table, firer, Some(root))
        .map_err(|e| setup(&e))?
        .with_law_factory(factory)
        .with_identification_scale_db(case.identification_scale_db().unwrap_or(0.0));
    s.set_pacer(Box::new(VirtualPacer::new(0.0)));
    s.assess_feasibility().map_err(|e| setup(&e))?;
    s.arm().map_err(|e| setup(&e))?;
    let ran = s.identify().and_then(|st| {
        if st == RunState::Firing {
            s.run_auto(max_pulses)
        } else {
            Ok(st)
        }
    });
    let fault = ran.err().map(|e| e.to_string());
    let _ = s.stop();
    Ok(measure(&s, fault))
}

fn measure<F: hertzforge_shock_host::PulseFirer>(
    s: &ClassicalShockService<F>,
    fault: Option<String>,
) -> ClassicalCaseMeasure {
    let report = s.report_data();
    let rows = &report.pulses;
    let table = &s.config().schedule.table_db;
    let full_db = s.config().schedule.full_db();
    let rung = |db: f64| {
        table
            .iter()
            .position(|&t| (t - db).abs() < 1e-9)
            .unwrap_or(table.len())
    };
    let passed = |r: &hertzforge_shock_host::PulseRow| r.tolerance_overall == Some(true);
    let full: Vec<_> = rows
        .iter()
        .filter(|r| r.scheduled_level_db >= full_db - 1e-9)
        .collect();
    let end = match (fault, s.state(), s.trip().map(|t| &t.kind)) {
        (Some(f), ..) => ClassicalCaseEnd::Fault(f),
        (None, RunState::Held, _) => ClassicalCaseEnd::Held,
        (None, RunState::NotConverged, _) => ClassicalCaseEnd::NotConverged,
        (None, _, Some(TripKind::Divergence { .. })) => ClassicalCaseEnd::Divergence,
        (None, _, Some(TripKind::LawFault { fault, .. })) => {
            ClassicalCaseEnd::LawFault(fault.reason().into())
        }
        (None, RunState::Aborted, Some(k)) => ClassicalCaseEnd::Aborted(k.name().into()),
        (None, _, _) => ClassicalCaseEnd::Completed,
    };
    let law = report.law.as_ref();
    let control = s.config().control.as_ref();
    ClassicalCaseMeasure {
        end,
        pulses: rows.len() as u32,
        first_pass: rows
            .iter()
            .position(passed)
            .map(|i| (i as u32 + 1, rung(rows[i].scheduled_level_db))),
        pulses_passed: rows.iter().filter(|r| passed(r)).count() as u32,
        full_level_pulses: full.len() as u32,
        full_level_passed: full.iter().filter(|r| passed(r)).count() as u32,
        calls: law.map(|l| l.calls as u32).unwrap_or(0),
        misses: law.map(|l| l.misses as u32).unwrap_or(0),
        refused: law.map(|l| l.refused as u32).unwrap_or(0),
        capped_bins: law.map(|l| l.capped_bins).unwrap_or(0),
        capped_peaks: law.map(|l| l.capped_peaks as u32).unwrap_or(0),
        worst_call_ms: law.filter(|l| l.calls > 0).map(|l| l.worst_call_ms),
        budget_ms: law.map(|l| l.budget_ms).unwrap_or(0.0),
        max_drive_vpk: rows
            .iter()
            .filter_map(|r| r.drive_vpk)
            .fold(0.0f64, f64::max),
        drive_ceiling_vpk: control.map(|c| c.drive_vpk_ceiling).unwrap_or(0.0),
    }
}

fn end_words(e: &ClassicalCaseEnd) -> String {
    match e {
        ClassicalCaseEnd::Completed => "completed".into(),
        ClassicalCaseEnd::NotConverged => "ended not converged".into(),
        ClassicalCaseEnd::Divergence => "ended on divergence".into(),
        ClassicalCaseEnd::Aborted(k) => format!("aborted ({k})"),
        ClassicalCaseEnd::LawFault(r) => format!("ended on a law fault ({r})"),
        ClassicalCaseEnd::Held => "held at identification".into(),
        ClassicalCaseEnd::Fault(f) => format!("failed: {f}"),
    }
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
pub fn judge_classical(case: ClassicalCheckCase, measure: &ClassicalCaseMeasure) -> CheckCase {
    let m = measure;
    let mut why: Vec<String> = Vec::new();
    if m.calls == 0 {
        why.push("the law was never called".into());
    }
    if m.misses > 0 {
        why.push(format!(
            "{} call(s) not used, {} of them refused",
            m.misses, m.refused
        ));
    }
    if let Some(w) = m.worst_call_ms {
        if w > m.budget_ms {
            why.push(format!(
                "the slowest call took {w:.1} ms, over its {:.1} ms budget",
                m.budget_ms
            ));
        }
    }
    match case.requirement() {
        ClassicalRequirement::EveryPulse => {
            if m.end != ClassicalCaseEnd::Completed {
                why.push(format!("the run {}", end_words(&m.end)));
            }
            if m.first_pass != Some((1, 0)) {
                why.push("the first pulse did not pass".into());
            }
            if m.pulses_passed != m.pulses {
                why.push(format!("{} of {} pulses passed", m.pulses_passed, m.pulses));
            }
        }
        ClassicalRequirement::PassBy {
            by_pulse,
            every_full_level,
        } => {
            if m.end != ClassicalCaseEnd::Completed {
                why.push(format!("the run {}", end_words(&m.end)));
            }
            match m.first_pass {
                Some((n, 0)) if n <= by_pulse => {}
                Some((n, r)) => why.push(format!(
                    "the first pass was pulse {n} at rung {r}, not by pulse {by_pulse} at the \
                     first rung"
                )),
                None => why.push("no pulse passed".into()),
            }
            if every_full_level
                && (m.full_level_pulses == 0 || m.full_level_passed != m.full_level_pulses)
            {
                why.push(format!(
                    "{} of {} full-level pulses passed",
                    m.full_level_passed, m.full_level_pulses
                ));
            }
        }
        ClassicalRequirement::Survive => {
            if !matches!(
                m.end,
                ClassicalCaseEnd::Completed
                    | ClassicalCaseEnd::NotConverged
                    | ClassicalCaseEnd::Divergence
            ) {
                why.push(format!("the run {}", end_words(&m.end)));
            }
        }
    }
    if m.max_drive_vpk > m.drive_ceiling_vpk + 1e-12 {
        why.push(format!(
            "the drive reached {:.3} V pk, over the {:.3} V pk ceiling",
            m.max_drive_vpk, m.drive_ceiling_vpk
        ));
    }
    let worst = m
        .worst_call_ms
        .map(|w| format!("{w:.2} ms"))
        .unwrap_or_else(|| "no call".into());
    let summary = format!(
        "{}; {} pulses, {} passed{}; slowest call {worst} of {:.0} ms; {} capped bins",
        end_words(&m.end),
        m.pulses,
        m.pulses_passed,
        m.first_pass
            .map(|(n, r)| format!(", the first at pulse {n} (rung {r})"))
            .unwrap_or_default(),
        m.budget_ms,
        m.capped_bins
    );
    CheckCase {
        name: case.name().to_string(),
        passed: why.is_empty(),
        detail: if why.is_empty() {
            summary
        } else {
            format!("{}. {summary}", why.join("; "))
        },
    }
}

/// The check record for the law file `law_sha256` from its judged cases.
pub fn classical_check_record(
    law_sha256: impl Into<String>,
    date: impl Into<String>,
    cases: Vec<CheckCase>,
) -> CheckRecord {
    CheckRecord::new(law_sha256, CLASSICAL_CHECK_VERSION, date, cases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_is_listed_once_with_a_unique_name() {
        let mut names: Vec<&str> = ClassicalCheckCase::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ClassicalCheckCase::ALL.len());
    }

    #[test]
    fn the_cases_carry_the_plan_plants_and_requirements() {
        use ClassicalCheckCase as C;
        assert_eq!(C::Flat.requirement(), ClassicalRequirement::EveryPulse);
        assert_eq!(
            C::AntiResonance.requirement(),
            ClassicalRequirement::Survive
        );
        assert_eq!(C::IdentificationError.identification_scale_db(), Some(3.0));
        assert_eq!(C::Noise.noise_below_db(), Some(30.0));
        assert_eq!(C::StationPlant.pulse(), (ClassicalShape::Haversine, 6.0));
        assert_eq!(C::IdentificationError.max_pulses_per_level(), 8);
        assert_eq!(C::OneMode.max_pulses_per_level(), 5);
        assert_eq!(C::Flat.pulse(), (ClassicalShape::HalfSine, 11.0));
        assert!(matches!(
            C::Delay.plant(),
            ClassicalCheckPlant::DelayedMode { delay_s, .. } if delay_s == 0.002
        ));
    }

    fn good() -> ClassicalCaseMeasure {
        ClassicalCaseMeasure {
            end: ClassicalCaseEnd::Completed,
            pulses: 7,
            first_pass: Some((1, 0)),
            pulses_passed: 7,
            full_level_pulses: 3,
            full_level_passed: 3,
            calls: 7,
            misses: 0,
            refused: 0,
            capped_bins: 0,
            capped_peaks: 0,
            worst_call_ms: Some(1.0),
            budget_ms: 250.0,
            max_drive_vpk: 0.4,
            drive_ceiling_vpk: 0.5,
        }
    }

    #[test]
    fn a_clean_run_passes_every_case_it_meets() {
        for case in ClassicalCheckCase::ALL {
            let j = judge_classical(case, &good());
            assert!(j.passed, "{}: {}", case.name(), j.detail);
        }
    }

    #[test]
    fn each_rule_fails_a_case_by_name() {
        use ClassicalCheckCase as C;
        let cases: Vec<(C, ClassicalCaseMeasure, &str)> = vec![
            (
                C::Flat,
                ClassicalCaseMeasure {
                    misses: 1,
                    refused: 1,
                    ..good()
                },
                "not used",
            ),
            (
                C::Flat,
                ClassicalCaseMeasure {
                    worst_call_ms: Some(300.0),
                    ..good()
                },
                "budget",
            ),
            (
                C::Flat,
                ClassicalCaseMeasure {
                    first_pass: Some((2, 0)),
                    pulses_passed: 6,
                    ..good()
                },
                "first pulse",
            ),
            (
                C::OneMode,
                ClassicalCaseMeasure {
                    first_pass: Some((4, 0)),
                    ..good()
                },
                "by pulse 3",
            ),
            (
                C::OneMode,
                ClassicalCaseMeasure {
                    first_pass: Some((2, 1)),
                    ..good()
                },
                "rung 1",
            ),
            (
                C::OneMode,
                ClassicalCaseMeasure {
                    full_level_passed: 2,
                    ..good()
                },
                "full-level",
            ),
            (
                C::OneMode,
                ClassicalCaseMeasure {
                    end: ClassicalCaseEnd::NotConverged,
                    ..good()
                },
                "not converged",
            ),
            (
                C::IdentificationError,
                ClassicalCaseMeasure {
                    first_pass: None,
                    pulses_passed: 0,
                    ..good()
                },
                "no pulse passed",
            ),
            (
                C::AntiResonance,
                ClassicalCaseMeasure {
                    end: ClassicalCaseEnd::Aborted("overshoot".into()),
                    ..good()
                },
                "overshoot",
            ),
            (
                C::AntiResonance,
                ClassicalCaseMeasure {
                    max_drive_vpk: 0.6,
                    ..good()
                },
                "ceiling",
            ),
            (
                C::Delay,
                ClassicalCaseMeasure {
                    calls: 0,
                    worst_call_ms: None,
                    ..good()
                },
                "never called",
            ),
        ];
        for (case, m, words) in cases {
            let j = judge_classical(case, &m);
            assert!(!j.passed, "{}: {words}", case.name());
            assert!(j.detail.contains(words), "{}: {}", case.name(), j.detail);
        }
        // The anti-resonance case survives on the core's own judgement.
        for end in [ClassicalCaseEnd::NotConverged, ClassicalCaseEnd::Divergence] {
            let m = ClassicalCaseMeasure {
                end,
                first_pass: None,
                ..good()
            };
            assert!(judge_classical(C::AntiResonance, &m).passed);
        }
        // The identification-error case does not ask every full-level pulse.
        let m = ClassicalCaseMeasure {
            first_pass: Some((6, 0)),
            full_level_passed: 1,
            ..good()
        };
        assert!(judge_classical(C::IdentificationError, &m).passed);
    }

    #[test]
    fn the_record_carries_the_check_version_and_the_verdict() {
        let cases: Vec<CheckCase> = ClassicalCheckCase::ALL
            .iter()
            .map(|&c| judge_classical(c, &good()))
            .collect();
        let r = classical_check_record("ab".repeat(32), "2026-11-07T00:00:00Z", cases);
        assert!(r.passed);
        assert_eq!(r.tool_version, CLASSICAL_CHECK_VERSION);
    }
}
