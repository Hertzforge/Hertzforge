//! The check a Sine law file passes on the simulator before it drives
//! hardware: the plant set, what each case asks, what a run is measured by,
//! and the rules that make a measure a passed or failed case.
//!
//! Every case is a whole run under the core's guards on a simulated station:
//! one drive, one control channel at 1 V per g, a flat target of
//! [`SINE_CHECK_TARGET_G`] g pk, identification at -20 dB, no review hold.
//! Every case asks the same of the law first: every call answered, in time,
//! with zero misses and zero refused outputs. Then each case asks for its
//! [`SineRequirement`]. The worst call time and the count of capped ticks are
//! reported with every case.
//!
//! The station a case runs on is built by the controller from these methods
//! alone ([`SineCheckCase::plant`], [`SineCheckCase::noise_below_db`],
//! [`SineCheckCase::limit`], at [`SINE_CHECK_FS_HZ`]); the runner's own run
//! configuration is the authority on everything else, and the controller's
//! station must follow it.

use hertzforge_hal::Backend;
use hertzforge_random_laws::CheckCase;
use hertzforge_sine::SineLawFactory;
use std::fmt;
use std::sync::Arc;

/// The version of the Sine check written into its records. A change to the
/// plant set or to the rules changes it.
pub const SINE_CHECK_VERSION: &str = "sine-check 1";

/// The rate every case runs at, Hz.
pub const SINE_CHECK_FS_HZ: f64 = 16_384.0;

/// The flat target every case controls to, g pk.
pub const SINE_CHECK_TARGET_G: f64 = 1.0;

/// The drive ceiling every case runs under, volts peak.
pub const SINE_CHECK_DRIVE_CEILING_V: f64 = 10.0;

/// The drive abort every case runs under, volts peak.
pub const SINE_CHECK_DRIVE_ABORT_V: f64 = 20.0;

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

impl std::error::Error for CheckRunError {}

/// The simulated test article a case runs on. The controller builds a
/// one-drive simulated station from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SineCheckPlant {
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
    /// A flat plant with an anti-resonance: a notch at least `depth_db` deep,
    /// with the coherence there under the identification's 0.5 trust floor.
    AntiResonance {
        /// The notch's frequency, Hz.
        f_hz: f64,
        /// Its depth, dB.
        depth_db: f64,
    },
}

impl fmt::Display for SineCheckPlant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SineCheckPlant::OneMode { f_hz, q } => write!(f, "one mode, {f_hz} Hz, Q {q}"),
            SineCheckPlant::TwoModes { f_hz, q } => {
                write!(f, "two modes, {} and {} Hz, Q {q}", f_hz[0], f_hz[1])
            }
            SineCheckPlant::AntiResonance { f_hz, depth_db } => {
                write!(f, "a {depth_db} dB anti-resonance at {f_hz} Hz")
            }
        }
    }
}

/// What a case's run does with the frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SineCheckMode {
    /// A logarithmic sweep up the band.
    Sweep {
        /// The band, Hz.
        band_hz: (f64, f64),
        /// The rate, octaves per minute.
        oct_per_min: f64,
        /// One-way sweeps at full level.
        sweeps: u32,
    },
    /// A dwell.
    Dwell {
        /// The frequency, Hz.
        freq_hz: f64,
        /// Seconds at each level.
        seconds: f64,
    },
}

/// A limit channel a case watches: its response is the control channel's
/// times `1 + (peak_ratio - 1) / (1 + ((f - f0) / width_hz)^2)` around the
/// plant's mode `f0`, under a flat bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SineCheckLimit {
    /// The limit channel's response over the control channel's at the mode.
    pub peak_ratio: f64,
    /// The half width of that rise, Hz.
    pub width_hz: f64,
    /// The flat bound, g pk.
    pub bound_g: f64,
}

/// What a case asks of the law beyond answering every call, in time, with a
/// usable drive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SineRequirement {
    /// Every judged tick at full level (after the settle window) within `db`
    /// of the target.
    Within {
        /// The band, dB either way.
        db: f64,
    },
    /// Within `through_db` through the resonance, and within `away_db` on
    /// every judged tick more than `away_hz` from it.
    WithinAway {
        /// The band through the resonance, dB either way.
        through_db: f64,
        /// The band away from it, dB either way.
        away_db: f64,
        /// How far from the resonance the tighter band applies, Hz.
        away_hz: f64,
    },
    /// The run ends completed, or by the core's own divergence trip; the drive
    /// never goes over the ceiling.
    Survive,
    /// At every level of the ladder, every judged tick after the settle window
    /// within `db` of the target at that level.
    EachRungWithin {
        /// The band, dB either way.
        db: f64,
    },
    /// The limit channel never over its bound by more than `over_bound_db`,
    /// and no control overshoot above `overshoot_db` when the notch releases.
    LimitHeld {
        /// How far over the bound the limit channel may read, dB.
        over_bound_db: f64,
        /// How far over the target the control channel may read on release,
        /// dB.
        overshoot_db: f64,
    },
}

impl SineRequirement {
    /// The stable record and wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            SineRequirement::Within { .. } => "within",
            SineRequirement::WithinAway { .. } => "within_away",
            SineRequirement::Survive => "survive",
            SineRequirement::EachRungWithin { .. } => "each_rung_within",
            SineRequirement::LimitHeld { .. } => "limit_held",
        }
    }
}

/// One simulated test article or condition of the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SineCheckCase {
    /// One mode at 250 Hz, Q 50; a log sweep 100 to 800 Hz at 1 oct/min.
    Slow,
    /// The same plant at 6 oct/min.
    Fast,
    /// Two close modes at 240 and 260 Hz, Q 30; 2 oct/min.
    CloseModes,
    /// A 30 dB anti-resonance with its coherence under the trust floor.
    AntiResonance,
    /// Measurement noise 30 dB under the response; a 30 s dwell at 250 Hz.
    Noise,
    /// A ladder of -20, -12, -6 and 0 dB with manual moves and one live
    /// parameter change.
    Ladder,
    /// A limit channel that binds at the resonance.
    Notch,
}

impl SineCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [SineCheckCase; 7] = [
        SineCheckCase::Slow,
        SineCheckCase::Fast,
        SineCheckCase::CloseModes,
        SineCheckCase::AntiResonance,
        SineCheckCase::Noise,
        SineCheckCase::Ladder,
        SineCheckCase::Notch,
    ];

    /// The case's name in a record and on screen.
    pub fn name(self) -> &'static str {
        match self {
            SineCheckCase::Slow => "slow sweep",
            SineCheckCase::Fast => "fast sweep",
            SineCheckCase::CloseModes => "close modes",
            SineCheckCase::AntiResonance => "anti-resonance",
            SineCheckCase::Noise => "measurement noise",
            SineCheckCase::Ladder => "level ladder",
            SineCheckCase::Notch => "limit channel",
        }
    }

    /// The simulated test article.
    pub fn plant(self) -> SineCheckPlant {
        match self {
            SineCheckCase::CloseModes => SineCheckPlant::TwoModes {
                f_hz: [240.0, 260.0],
                q: 30.0,
            },
            SineCheckCase::AntiResonance => SineCheckPlant::AntiResonance {
                f_hz: 300.0,
                depth_db: 30.0,
            },
            _ => SineCheckPlant::OneMode {
                f_hz: 250.0,
                q: 50.0,
            },
        }
    }

    /// What the run does with the frequency.
    pub fn mode(self) -> SineCheckMode {
        let sweep = |oct_per_min| SineCheckMode::Sweep {
            band_hz: (100.0, 800.0),
            oct_per_min,
            sweeps: 1,
        };
        match self {
            SineCheckCase::Slow => sweep(1.0),
            SineCheckCase::Fast => sweep(6.0),
            SineCheckCase::CloseModes | SineCheckCase::AntiResonance | SineCheckCase::Notch => {
                sweep(2.0)
            }
            SineCheckCase::Noise => SineCheckMode::Dwell {
                freq_hz: 250.0,
                seconds: 30.0,
            },
            SineCheckCase::Ladder => SineCheckMode::Dwell {
                freq_hz: 250.0,
                seconds: 10.0,
            },
        }
    }

    /// The levels the run goes through, dB; the last is full level.
    pub fn levels_db(self) -> &'static [f64] {
        match self {
            SineCheckCase::Ladder => &[-20.0, -12.0, -6.0, 0.0],
            _ => &[0.0],
        }
    }

    /// Whether the levels are moved by hand (the ladder is manual).
    pub fn manual(self) -> bool {
        matches!(self, SineCheckCase::Ladder)
    }

    /// Whether the run changes one live parameter partway (to the middle of
    /// its declared range; nothing for a law that declares none).
    pub fn live_change(self) -> bool {
        matches!(self, SineCheckCase::Ladder)
    }

    /// Measurement noise, dB under the response (`None`: the simulator's
    /// floor only).
    pub fn noise_below_db(self) -> Option<f64> {
        match self {
            SineCheckCase::Noise => Some(30.0),
            _ => None,
        }
    }

    /// The limit channel the case watches (`None`: none).
    pub fn limit(self) -> Option<SineCheckLimit> {
        match self {
            SineCheckCase::Notch => Some(SineCheckLimit {
                peak_ratio: 4.0,
                width_hz: 25.0,
                bound_g: 2.0 * SINE_CHECK_TARGET_G,
            }),
            _ => None,
        }
    }

    /// What the case asks of the law.
    pub fn requirement(self) -> SineRequirement {
        match self {
            SineCheckCase::Slow => SineRequirement::Within { db: 1.0 },
            SineCheckCase::Fast => SineRequirement::WithinAway {
                through_db: 5.0,
                away_db: 2.0,
                away_hz: 30.0,
            },
            SineCheckCase::CloseModes => SineRequirement::Within { db: 2.0 },
            SineCheckCase::AntiResonance => SineRequirement::Survive,
            SineCheckCase::Noise => SineRequirement::Within { db: 1.5 },
            SineCheckCase::Ladder => SineRequirement::EachRungWithin { db: 1.0 },
            SineCheckCase::Notch => SineRequirement::LimitHeld {
                over_bound_db: 0.5,
                overshoot_db: 1.0,
            },
        }
    }
}

/// How a case's run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum SineCaseEnd {
    /// The schedule completed.
    Completed,
    /// A protection aborted it; the trip kind's wire name (`abort_band`,
    /// `drive_vpk`, `divergence`, ...).
    Aborted(String),
    /// The law's misses ended it; the reason's wire name.
    LawFault(String),
    /// The coherence gate held the run.
    Held,
    /// The run failed; the controller's words.
    Fault(String),
}

/// What one case's run is judged by.
#[derive(Debug, Clone, PartialEq)]
pub struct SineCaseMeasure {
    /// How the run ended.
    pub end: SineCaseEnd,
    /// Control ticks run.
    pub ticks: u64,
    /// Ticks judged against the requirement (at full level, or at a rung,
    /// after the settle window).
    pub judged_ticks: u64,
    /// The judged tick furthest from the target, dB, signed; `None` when none
    /// was judged.
    pub worst_error_db: Option<f64>,
    /// Its frequency, Hz.
    pub worst_error_hz: Option<f64>,
    /// For [`SineRequirement::WithinAway`]: the worst judged error away from
    /// the resonance, dB, signed.
    pub worst_away_error_db: Option<f64>,
    /// Per level of the ladder, in order: the level, dB, and the worst judged
    /// error there, dB (`None` when the level was not reached).
    pub rungs: Vec<(f64, Option<f64>)>,
    /// The limit channel's largest reading over its bound, dB; `None` without a
    /// limit channel.
    pub worst_limit_over_db: Option<f64>,
    /// The control channel's largest overshoot over the target after the notch
    /// released, dB; `None` without a limit channel.
    pub worst_release_overshoot_db: Option<f64>,
    /// Law calls made, `engage` included.
    pub calls: u64,
    /// Calls whose output was not used.
    pub misses: u64,
    /// Calls whose output the core refused (a subset of `misses`).
    pub refused: u64,
    /// Ticks on which the core's change or rise cap bound.
    pub capped_ticks: u64,
    /// The slowest call, ms; `None` when no call was made.
    pub worst_call_ms: Option<f64>,
    /// One call's budget, ms.
    pub budget_ms: f64,
    /// The largest drive played, volts peak.
    pub max_drive_vpk: f64,
    /// The drive ceiling, volts peak.
    pub drive_ceiling_vpk: f64,
}

/// Run `case` with the law `factory` builds, under the core's guards, on
/// `backend` (the simulated station the controller built for the case), and
/// measure it.
pub fn run_check_case(
    case: SineCheckCase,
    factory: Arc<dyn SineLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<SineCaseMeasure, CheckRunError> {
    let _ = (factory, backend);
    Err(CheckRunError::Setup(format!(
        "the Sine law check is not built yet (case \"{}\")",
        case.name()
    )))
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
pub fn judge_sine(case: SineCheckCase, measure: &SineCaseMeasure) -> CheckCase {
    let _ = measure;
    CheckCase {
        name: case.name().to_string(),
        passed: false,
        detail: "the Sine law check is not built yet".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_case_is_listed_once_with_a_unique_name() {
        let mut names: Vec<&str> = SineCheckCase::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SineCheckCase::ALL.len());
    }

    #[test]
    fn the_cases_carry_the_plan_plants_modes_and_requirements() {
        use SineCheckCase as C;
        assert_eq!(
            C::Slow.plant(),
            SineCheckPlant::OneMode {
                f_hz: 250.0,
                q: 50.0
            }
        );
        assert_eq!(C::Slow.plant().to_string(), "one mode, 250 Hz, Q 50");
        assert_eq!(
            C::Slow.mode(),
            SineCheckMode::Sweep {
                band_hz: (100.0, 800.0),
                oct_per_min: 1.0,
                sweeps: 1
            }
        );
        assert_eq!(C::Slow.requirement(), SineRequirement::Within { db: 1.0 });
        assert!(matches!(
            C::Fast.mode(),
            SineCheckMode::Sweep { oct_per_min, .. } if oct_per_min == 6.0
        ));
        assert!(matches!(
            C::Fast.requirement(),
            SineRequirement::WithinAway { through_db, away_db, .. }
                if through_db == 5.0 && away_db == 2.0
        ));
        assert!(matches!(
            C::CloseModes.plant(),
            SineCheckPlant::TwoModes { f_hz, q } if f_hz == [240.0, 260.0] && q == 30.0
        ));
        assert!(matches!(
            C::AntiResonance.plant(),
            SineCheckPlant::AntiResonance { depth_db, .. } if depth_db >= 30.0
        ));
        assert_eq!(C::AntiResonance.requirement(), SineRequirement::Survive);
        assert_eq!(C::Noise.noise_below_db(), Some(30.0));
        assert!(matches!(
            C::Noise.mode(),
            SineCheckMode::Dwell { freq_hz, seconds } if freq_hz == 250.0 && seconds == 30.0
        ));
        assert_eq!(C::Ladder.levels_db(), &[-20.0, -12.0, -6.0, 0.0]);
        assert!(C::Ladder.manual() && C::Ladder.live_change());
        assert!(C::Notch.limit().is_some());
        assert!(C::ALL
            .iter()
            .filter(|c| **c != C::Notch)
            .all(|c| c.limit().is_none()));
        assert_eq!(
            C::ALL.map(|c| c.requirement().as_str()),
            [
                "within",
                "within_away",
                "within",
                "survive",
                "within",
                "each_rung_within",
                "limit_held"
            ]
        );
    }

    #[test]
    fn nothing_runs_or_passes_until_the_check_is_built() {
        let m = SineCaseMeasure {
            end: SineCaseEnd::Completed,
            ticks: 18_000,
            judged_ticks: 17_000,
            worst_error_db: Some(0.4),
            worst_error_hz: Some(251.0),
            worst_away_error_db: None,
            rungs: vec![(0.0, Some(0.4))],
            worst_limit_over_db: None,
            worst_release_overshoot_db: None,
            calls: 18_000,
            misses: 0,
            refused: 0,
            capped_ticks: 0,
            worst_call_ms: Some(0.01),
            budget_ms: 2.5,
            max_drive_vpk: 0.4,
            drive_ceiling_vpk: SINE_CHECK_DRIVE_CEILING_V,
        };
        assert!(!judge_sine(SineCheckCase::Slow, &m).passed);
    }
}
