//! The check of a classical shock law before it drives hardware: the fixed set
//! of simulated test articles, what a run on each is measured by, and the rules
//! that turn a measurement into a passed or failed case of a [`CheckRecord`].
//!
//! Every case is a whole run under the core's guards on a simulated station:
//! AUTO `[-12, -9, -6, -3, 0]` dB with `[1, 1, 1, 1, 3]` pulses, advancing on a
//! passing pulse, at most 5 pulses a rung, MIL-STD-810 Method 516, a half-sine
//! of 11 ms unless the case says otherwise. Every case asks the same of the law
//! first: every call answered, in time, with zero misses and zero refused
//! outputs. Then each case asks for its [`ClassicalRequirement`].
//!
//! The controller builds the simulated station each case names
//! ([`ClassicalCheckCase::plant`]) and hands it to [`run_classical_check_case`];
//! [`judge_classical`] holds everything that decides the verdict, so the rules
//! are the same whoever runs the plants.
//!
//! [`CheckRecord`]: hertzforge_random_laws::CheckRecord

use hertzforge_hal::Backend;
use hertzforge_random_laws::CheckCase;
use hertzforge_shock_host::{ClassicalLawFactory, ClassicalShape};
use std::fmt;
use std::sync::Arc;

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
    /// The simulated station's plant (200 and 550 Hz, Q 2.5), a terminal-peak
    /// sawtooth of 6 ms.
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
            ClassicalCheckCase::StationPlant => (ClassicalShape::TerminalPeakSawtooth, 6.0),
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

    /// Measurement noise, dB under the response at full level (`None`: the
    /// simulator's floor only).
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
                by_pulse: 5,
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
    /// The check runner is not built yet.
    NotBuilt,
    /// The case's run could not be set up on the backend it was handed.
    Setup(String),
}

impl fmt::Display for CheckRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckRunError::NotBuilt => {
                f.write_str("the classical shock law check is not built yet")
            }
            CheckRunError::Setup(s) => write!(f, "the check case could not be set up: {s}"),
        }
    }
}

impl std::error::Error for CheckRunError {}

/// Run `case` with the law `factory` builds, under the core's guards, on
/// `backend` (the simulated station the controller built for
/// [`ClassicalCheckCase::plant`]), and measure it.
pub fn run_classical_check_case(
    case: ClassicalCheckCase,
    factory: Arc<dyn ClassicalLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<ClassicalCaseMeasure, CheckRunError> {
    let _ = (case, factory, backend);
    Err(CheckRunError::NotBuilt)
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
pub fn judge_classical(case: ClassicalCheckCase, measure: &ClassicalCaseMeasure) -> CheckCase {
    let _ = measure;
    CheckCase {
        name: case.name().to_string(),
        passed: false,
        detail: "the classical shock law check is not built yet".into(),
    }
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
        assert_eq!(
            C::StationPlant.pulse(),
            (ClassicalShape::TerminalPeakSawtooth, 6.0)
        );
        assert_eq!(C::Flat.pulse(), (ClassicalShape::HalfSine, 11.0));
        assert!(matches!(
            C::Delay.plant(),
            ClassicalCheckPlant::DelayedMode { delay_s, .. } if delay_s == 0.002
        ));
    }

    #[test]
    fn nothing_passes_until_the_check_is_built() {
        let m = ClassicalCaseMeasure {
            end: ClassicalCaseEnd::Completed,
            pulses: 7,
            first_pass: Some((1, 0)),
            pulses_passed: 7,
            full_level_pulses: 3,
            full_level_passed: 3,
            calls: 8,
            misses: 0,
            refused: 0,
            capped_bins: 0,
            capped_peaks: 0,
            worst_call_ms: Some(1.0),
            budget_ms: 250.0,
            max_drive_vpk: 0.4,
            drive_ceiling_vpk: 0.5,
        };
        assert!(!judge_classical(ClassicalCheckCase::Flat, &m).passed);
    }
}
