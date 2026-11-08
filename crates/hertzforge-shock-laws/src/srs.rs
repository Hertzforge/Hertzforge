//! The SRS shock control laws an operator can choose, and the check a law file
//! passes before it drives hardware.
//!
//! - [`resolve_srs`] turns the operator's [`LawChoice`] into the
//!   [`SrsLawFactory`] the SRS service builds its law with
//!   (`SrsShockService::with_law_factory`): the built-in `srs_ratio`
//!   ([`DEFAULT_SRS_LAW_ID`]) or a `shock-srs` law file from the catalog. A law
//!   of another world is refused ([`ChoiceError::WrongTestType`]).
//! - [`SrsCheckCase`] is the check: the fixed set of simulated test articles,
//!   what a run on each is measured by ([`SrsCaseMeasure`]) and the rules that
//!   make the measure a passed or failed case ([`judge_srs`]).
//!   [`run_srs_check_case`] runs one case under the core's guards on the
//!   backend it is handed.
//!
//! Every case is a whole run under the core's guards on a simulated station: a
//! flat target on a 1/3-octave grid from 100 to 2000 Hz, Q 10, synthesized from
//! odd-half-cycle wavelets. Every case asks the same of the law first: every
//! call answered, in time, with zero misses and zero refused outputs. Then each
//! case asks for its [`SrsRequirement`].
//!
//! The choice of the built-in law, the catalog's SRS world and the check runner
//! are not built yet: [`resolve_srs`] refuses every law, the runner refuses
//! every case, and [`judge_srs`] passes none.
//!
//! [`LawChoice`]: hertzforge_random_laws::LawChoice
//! [`ChoiceError::WrongTestType`]: hertzforge_random_laws::ChoiceError::WrongTestType

use crate::check::CheckRunError;
use hertzforge_hal::Backend;
use hertzforge_random_laws::{CheckCase, ChoiceError, LawCatalog, LawChoice, LawWorld};
use hertzforge_srs::SrsLawFactory;
use std::sync::Arc;

/// The catalog id of the built-in SRS law, `srs_ratio`: the default.
pub const DEFAULT_SRS_LAW_ID: &str = hertzforge_shock_law_math::srs::ID;

/// The version of the SRS check written into its records. A change to the
/// plant set or to the rules changes it.
pub const SRS_CHECK_VERSION: &str = "shock-srs-check 1";

/// The default law with its default parameters.
pub fn default_srs_choice() -> LawChoice {
    LawChoice::new(DEFAULT_SRS_LAW_ID, Vec::new())
}

/// Resolve `choice` against `catalog`: check its parameter values against the
/// law's manifest and return the factory an SRS run builds its law with.
///
/// Refusals: [`ChoiceError::UnknownLaw`] for an id the catalog does not hold,
/// [`ChoiceError::WrongTestType`] for a law of another world,
/// [`ChoiceError::BadParam`] for a value its declaration refuses. The built-in
/// law is not built yet and is refused ([`ChoiceError::BadConfig`]).
pub fn resolve_srs(
    catalog: &LawCatalog,
    choice: &LawChoice,
) -> Result<Arc<dyn SrsLawFactory>, ChoiceError> {
    if choice.id == DEFAULT_SRS_LAW_ID {
        return Err(ChoiceError::BadConfig(
            "the built-in SRS law is not built yet".into(),
        ));
    }
    match catalog.entry(&choice.id) {
        None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        Some(entry) => match entry.world {
            LawWorld::ShockSrs => Err(ChoiceError::BadConfig(
                "SRS shock law files are not built yet".into(),
            )),
            LawWorld::Single | LawWorld::Mimo | LawWorld::ShockClassical => {
                Err(ChoiceError::WrongTestType {
                    id: choice.id.clone(),
                    law_is_for: entry.world.test_type(),
                })
            }
        },
    }
}

/// What a case asks of the law beyond answering every call, in time, with
/// usable gains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SrsRequirement {
    /// Every band within `db` of the target by full-level pulse `by_pulse`
    /// (counted from 1), and on every full-level pulse after it.
    WithinBy {
        /// The band, dB either way.
        db: f64,
        /// The full-level pulse by which every band must be within it.
        by_pulse: u32,
    },
    /// At every level of the schedule, every band within `db` of the target
    /// at that level after `after_pulses` pulses there.
    EachLevelWithin {
        /// The band, dB either way.
        db: f64,
        /// Pulses at a level before it is judged.
        after_pulses: u32,
    },
    /// The predicted limit-channel SRS at most `over_bound_db` over its bound
    /// on every pulse, and the control SRS within `db` of the target outside
    /// the notched bands.
    LimitHeld {
        /// How far over the bound a prediction may be, dB.
        over_bound_db: f64,
        /// The control band outside the notched bands, dB either way.
        db: f64,
    },
}

/// The simulated test article a case runs on. The controller builds a
/// one-drive, one-control simulated station from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SrsCheckPlant {
    /// A wire: feedthrough 1, no modes.
    Wire,
    /// One mode.
    OneMode {
        /// Natural frequency, Hz.
        f_hz: f64,
        /// Quality factor.
        q: f64,
    },
    /// Two close modes.
    TwoModes {
        /// The modes' natural frequencies, Hz.
        f_hz: [f64; 2],
        /// Their quality factor.
        q: f64,
    },
}

/// One simulated test article or condition of the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SrsCheckCase {
    /// One mode at 400 Hz, Q 10.
    OneMode,
    /// Two close modes at 240 and 260 Hz, Q 30.
    CloseModes,
    /// One mode with measurement noise 30 dB under the response.
    Noise,
    /// One mode on a ladder of -12, -6 and 0 dB, with manual moves.
    Ladder,
    /// One mode, with the identified gain scaled +3 dB before the seed.
    IdentificationError,
    /// A wire for control, and one limit channel with a user-supplied FRF that
    /// resonates at 500 Hz under a flat bound.
    Limit,
}

impl SrsCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [SrsCheckCase; 6] = [
        SrsCheckCase::OneMode,
        SrsCheckCase::CloseModes,
        SrsCheckCase::Noise,
        SrsCheckCase::Ladder,
        SrsCheckCase::IdentificationError,
        SrsCheckCase::Limit,
    ];

    /// The case's name in a record and on screen.
    pub fn name(self) -> &'static str {
        match self {
            SrsCheckCase::OneMode => "one mode",
            SrsCheckCase::CloseModes => "close modes",
            SrsCheckCase::Noise => "measurement noise",
            SrsCheckCase::Ladder => "level ladder",
            SrsCheckCase::IdentificationError => "identification error",
            SrsCheckCase::Limit => "limit channel",
        }
    }

    /// The simulated test article.
    pub fn plant(self) -> SrsCheckPlant {
        match self {
            SrsCheckCase::CloseModes => SrsCheckPlant::TwoModes {
                f_hz: [240.0, 260.0],
                q: 30.0,
            },
            SrsCheckCase::Limit => SrsCheckPlant::Wire,
            _ => SrsCheckPlant::OneMode {
                f_hz: 400.0,
                q: 10.0,
            },
        }
    }

    /// The factor, dB, the core scales its identified gain by before the seed
    /// (`None`: the identification as measured).
    pub fn identification_scale_db(self) -> Option<f64> {
        match self {
            SrsCheckCase::IdentificationError => Some(3.0),
            _ => None,
        }
    }

    /// Measurement noise, dB under the response (`None`: the simulator's
    /// floor only).
    pub fn noise_below_db(self) -> Option<f64> {
        match self {
            SrsCheckCase::Noise => Some(30.0),
            _ => None,
        }
    }

    /// The resonance of the limit channel's user-supplied FRF, Hz (`None`: no
    /// limit channel).
    pub fn limit_resonance_hz(self) -> Option<f64> {
        match self {
            SrsCheckCase::Limit => Some(500.0),
            _ => None,
        }
    }

    /// What the case asks of the law.
    pub fn requirement(self) -> SrsRequirement {
        match self {
            SrsCheckCase::Ladder => SrsRequirement::EachLevelWithin {
                db: 3.0,
                after_pulses: 3,
            },
            SrsCheckCase::IdentificationError => SrsRequirement::WithinBy {
                db: 3.0,
                by_pulse: 8,
            },
            SrsCheckCase::Limit => SrsRequirement::LimitHeld {
                over_bound_db: 0.5,
                db: 3.0,
            },
            SrsCheckCase::OneMode | SrsCheckCase::CloseModes | SrsCheckCase::Noise => {
                SrsRequirement::WithinBy {
                    db: 3.0,
                    by_pulse: 6,
                }
            }
        }
    }
}

/// How a case's run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum SrsCaseEnd {
    /// The schedule completed.
    Completed,
    /// The core's iteration limit ended it.
    NotConverged,
    /// A protection aborted it; the trip kind's wire name (`envelope`,
    /// `drive_vpk`, `kinematic`, ...).
    Aborted(String),
    /// The law's misses ended it; the reason's wire name.
    LawFault(String),
    /// Identification held the run (coherence).
    Held,
    /// The run failed; the controller's words.
    Fault(String),
}

/// What one case's run is judged by.
#[derive(Debug, Clone, PartialEq)]
pub struct SrsCaseMeasure {
    /// How the run ended.
    pub end: SrsCaseEnd,
    /// Pulses fired.
    pub pulses: u32,
    /// Pulses fired at full level.
    pub full_level_pulses: u32,
    /// The first full-level pulse, counted from 1, with every band within the
    /// case's band; `None` when none was.
    pub first_within_pulse: Option<u32>,
    /// The worst band's error on the last pulse, dB, signed.
    pub final_worst_band_db: Option<f64>,
    /// Per level of the schedule, in order: the level, dB, and the worst
    /// band's error after the case's pulses there, dB (`None` when the level
    /// was not reached).
    pub levels: Vec<(f64, Option<f64>)>,
    /// The largest predicted limit-channel SRS over its bound on any pulse,
    /// dB; `None` without a limit channel.
    pub worst_limit_over_db: Option<f64>,
    /// Law calls made, the seed included.
    pub calls: u32,
    /// Calls whose output was not used.
    pub misses: u32,
    /// Calls whose output the core refused (a subset of `misses`).
    pub refused: u32,
    /// Component gains the core's limit cap held back, over the run.
    pub limited_components: u64,
    /// The slowest call, ms; `None` when no call was made.
    pub worst_call_ms: Option<f64>,
    /// One call's budget, ms.
    pub budget_ms: f64,
    /// The largest drive commanded, volts peak.
    pub max_drive_vpk: f64,
    /// The drive ceiling, volts peak.
    pub drive_ceiling_vpk: f64,
}

/// Run `case` with the law `factory` builds, under the core's guards, on
/// `backend` (the simulated station the controller built for
/// [`SrsCheckCase::plant`]), and measure it.
pub fn run_srs_check_case(
    case: SrsCheckCase,
    factory: Arc<dyn SrsLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<SrsCaseMeasure, CheckRunError> {
    let _ = (factory, backend);
    Err(CheckRunError::Setup(format!(
        "the SRS shock law check is not built yet (case \"{}\")",
        case.name()
    )))
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
pub fn judge_srs(case: SrsCheckCase, measure: &SrsCaseMeasure) -> CheckCase {
    let _ = measure;
    CheckCase {
        name: case.name().to_string(),
        passed: false,
        detail: "the SRS shock law check is not built yet".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_is_listed_once_with_a_unique_name() {
        let mut names: Vec<&str> = SrsCheckCase::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SrsCheckCase::ALL.len());
    }

    #[test]
    fn the_cases_carry_the_plan_plants_and_requirements() {
        use SrsCheckCase as C;
        assert_eq!(
            C::OneMode.requirement(),
            SrsRequirement::WithinBy {
                db: 3.0,
                by_pulse: 6
            }
        );
        assert!(matches!(
            C::IdentificationError.requirement(),
            SrsRequirement::WithinBy { by_pulse: 8, .. }
        ));
        assert_eq!(C::IdentificationError.identification_scale_db(), Some(3.0));
        assert_eq!(C::Noise.noise_below_db(), Some(30.0));
        assert_eq!(C::Limit.limit_resonance_hz(), Some(500.0));
        assert!(matches!(
            C::CloseModes.plant(),
            SrsCheckPlant::TwoModes { f_hz, q } if f_hz == [240.0, 260.0] && q == 30.0
        ));
    }

    #[test]
    fn nothing_resolves_runs_or_passes_until_the_check_is_built() {
        let root =
            std::env::temp_dir().join(format!("hertzforge-shock-laws-srs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cat = LawCatalog::open(&root).expect("an empty catalog opens");
        assert!(matches!(
            resolve_srs(&cat, &default_srs_choice()),
            Err(ChoiceError::BadConfig(_))
        ));
        assert!(matches!(
            resolve_srs(&cat, &LawChoice::new("fd_correct", Vec::new())),
            Err(ChoiceError::WrongTestType {
                law_is_for: "classical shock",
                ..
            })
        ));
        assert!(matches!(
            resolve_srs(&cat, &LawChoice::new("no_such_law", Vec::new())),
            Err(ChoiceError::UnknownLaw(_))
        ));
        let _ = std::fs::remove_dir_all(root);

        let m = SrsCaseMeasure {
            end: SrsCaseEnd::Completed,
            pulses: 9,
            full_level_pulses: 3,
            first_within_pulse: Some(1),
            final_worst_band_db: Some(0.4),
            levels: vec![(-12.0, Some(0.5)), (-6.0, Some(0.4)), (0.0, Some(0.4))],
            worst_limit_over_db: None,
            calls: 10,
            misses: 0,
            refused: 0,
            limited_components: 0,
            worst_call_ms: Some(0.1),
            budget_ms: 250.0,
            max_drive_vpk: 0.4,
            drive_ceiling_vpk: 0.5,
        };
        assert!(!judge_srs(SrsCheckCase::OneMode, &m).passed);
    }
}
