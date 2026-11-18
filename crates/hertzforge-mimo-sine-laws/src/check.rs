//! The MIMO Sine law check: the cases a law is run through before it may
//! drive hardware, what each case asks of it, the runner that plays a case
//! through the engine on a simulated station, and the judge that reads the
//! run and answers pass or fail with the number that decided it.
//!
//! The requirements were written before any law was run through a case, for
//! a good law, not for the laws that exist. A built-in law may fail a case;
//! that is recorded as a failure, never hidden by easing the requirement.
//! Where a requirement is carried from the single-shaker Sine check, its
//! number is that check's.
//!
//! # Every run
//!
//! Every case is a whole run on the engine ([`hertzforge_mimo_sine::service`])
//! at [`CHECK_FS_HZ`] with a [`CHECK_TICK_S`] update: every control at the
//! structure's full level (1 g peak) on a flat profile with an alarm band of
//! [`CHECK_ALARM_DB`] and an abort band of [`CHECK_ABORT_DB`] either way;
//! every drive with a ceiling of [`CHECK_CEILING_V`] and an abort of
//! [`CHECK_DRIVE_ABORT_V`]; identification at [`CHECK_IDENT_DB`] with no
//! review; the guards at their defaults; measurement noise of
//! [`CHECK_NOISE_G`] g rms on every input unless the case says otherwise. A
//! law whose manifest accepts an amplitude-only reference runs one; any other
//! law gets every control in phase. A law is run only on the cases of the
//! test types its manifest accepts ([`cases_for`]).
//!
//! A judged update is one outside a settle window, with the law's answer
//! used, at the case's last level.
//!
//! # Every case asks, in this order
//!
//! 1. **`ran`**: the case could be set up, identified (the coherence gate
//!    passed) and engaged.
//! 2. **`no_law_fault`**: every answer of the law is used (no miss) and the
//!    run does not end on a law fault.
//! 3. **`within_ceiling`**: no sample of any drive over its ceiling, and the
//!    engine never had to scale an answer down to the ceilings (except where
//!    the case only asks the run to survive).
//! 4. **`no_abort`**: the run does not end on a control, limit, drive or
//!    kinematic abort or on the divergence stop: no control outside its abort
//!    band for longer than the engine allows.
//! 5. **`completes`**: the run ends completed.
//! 6. The case's own requirement, below.
//!
//! # The cases
//!
//! | Case | Type | Structure and run | Requirement |
//! |---|---|---|---|
//! | `dwell_at_resonance` | dwell | `sine-linear-4x4`, 6 s at its 96 Hz mode | every update from 2 s after the engage within 1.0 dB |
//! | `dwell_away_from_resonance` | dwell | `sine-linear-4x4`, 6 s at 130 Hz, between modes | every update from 2 s after the engage within 1.0 dB |
//! | `measurement_noise` | dwell | `sine-linear-4x4`, 6 s at 96 Hz, noise 30 dB under full level (0.0316 g rms) | every judged update within 1.5 dB |
//! | `transport_delay` | dwell | the plate's mass line alone (its modes taken out), 50 ms on every drive, quiet (0.0001 g rms), engaged at -6 dB, 6 s at 100 Hz | every update from 2 s after the engage within 0.5 dB |
//! | `level_ladder` | dwell | `sine-softening-4x4-mild` at its 96 Hz mode, rungs -20, -12, -6, 0 dB of 2 s each | every judged update of every rung within 1.0 dB of that rung |
//! | `stepped` | stepped | `sine-linear-4x4`, 60 to 120 Hz, 6 steps an octave, 2 s each | every judged update within 1.5 dB |
//! | `slow_sweep` | sweep | `sine-linear-4x4`, 80 to 120 Hz up at 1 octave a minute | every judged update within 1.5 dB, median within 0.5 dB |
//! | `fast_sweep` | sweep | `sine-linear-4x4`, 80 to 160 Hz up at 4 octaves a minute | within 5.0 dB within 12 Hz of the 96 Hz mode, within 2.0 dB further away |
//! | `close_modes` | sweep | the linear plate with its 165 Hz mode moved to 97.92 Hz, 2 percent above the 96 Hz mode; 80 to 120 Hz at 2 octaves a minute | every judged update within 2.0 dB |
//! | `near_singular` | sweep | `sine-near-singular-4x4`, 350 to 420 Hz up at 1 octave a minute, through 381.2 Hz | survive: completes, or ends on the engine's own control abort or divergence stop; no sample over a ceiling |
//! | `limit_channel` | sweep | `sine-base-article-6x4`, the two article responses limited at 2 g, 150 to 350 Hz at 2 octaves a minute | the backstop notch engages; the limit channels at most 0.5 dB over their bound; no control more than 1.0 dB over its reference in the 1 s after each release |
//! | `more_controls_than_drives` | sweep | `sine-tall-6x3`, 80 to 120 Hz at 1 octave a minute | judged against the response the drives can reach (the least-squares fit through the identification): within 3.0 dB, median within 0.5 dB |
//! | `more_drives_than_controls` | sweep | `sine-wide-3x6`, 80 to 120 Hz at 1 octave a minute | within 1.5 dB, median within 0.5 dB; the opposition ratio at most 6 dB |
//! | `law_misbehaves` | dwell | `sine-linear-4x4`, 96 Hz; from the 150th update the law's answer has a non-finite drive | the run ends on a law fault after three refused answers in a row, every drive walked to zero, no sample over a ceiling |
//! | `moved_resonance_mild` | sweep | `sine-softening-4x4-mild` as `slow_sweep` | reported, not a pass condition |
//! | `moved_resonance_moderate` | sweep | `sine-softening-4x4-moderate` as `slow_sweep` | reported, not a pass condition |
//! | `moved_resonance_hardening` | sweep | `sine-hardening-4x4`, 140 to 190 Hz at 1 octave a minute through its 165 Hz mode | reported, not a pass condition |
//!
//! A law that declares itself for weakly coupled pairs ([`PAIRED_SETUP_LAWS`]:
//! `paired`, each control with its own drive and no model of the coupling)
//! runs the two dwells, the measurement noise, the level ladder and the
//! stepped sine on `sine-weak-4x4` ([`PAIRED_STRUCTURE`]), the plate's modes
//! with every cross path at least 20 dB under the direct one, at the same
//! frequencies; its transport-delay and misbehaving cases are the other laws'.
//!
//! The moved-resonance cases are judged against the slow sweep's band and
//! printed, so a reader sees where `matrix`'s stated limit begins; they never
//! fail a law.
//!
//! Bands and dwells are shorter than a real test's so that the whole check
//! of one law runs in seconds: each sweep covers the one mode it is about.
//!
//! The station a case runs on is built by the caller from
//! [`MimoSineCheckCase::station`] alone (with the `simulator` feature,
//! [`sim_station`] builds it); [`run_check_case`] sets up everything else.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::Backend;
use hertzforge_mimo_sine::law::{LawDescriptor, LawManifest, LawSetup, LawValues};
use hertzforge_mimo_sine::records::{Event, EventKind, Outcome, TickRecord};
use hertzforge_mimo_sine::service::{EngineOptions, MimoSineService, ServiceState};
use hertzforge_mimo_sine::{
    ControlChannel, DriveChannel, IdentificationSettings, LawChoice, LawFault, LawIdentification,
    LevelMode, LimitChannel, MimoSineLaw, MimoSineLawFactory, MimoSineSetup, PhasePoint, Schedule,
    StepPlan, TestPlan, TestType, TickInputs, TickOutputs,
};
use hertzforge_module_abi::StationClass;
use hertzforge_random_laws::{CheckCase, CheckRecord};
use hertzforge_sdm::C64;
use hertzforge_sine::profile::{ProfileRow, SineProfile};
use hertzforge_sine::scheduler::{
    BidirSweepConfig, DwellFor, DwellPlan, LadderConfig, RungFor, StepSineConfig, StepSpacing,
    SweepDirection, SweepTarget,
};
use hertzforge_sine::SweepRate;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

/// The version of the MIMO Sine check written into its records. A change to
/// a case, a structure or a rule changes it.
pub const MIMO_SINE_CHECK_VERSION: &str = "mimo-sine-check 1";

/// The rate every case runs at, Hz (the sine structures' calibration rate).
pub const CHECK_FS_HZ: f64 = 16_384.0;

/// The update, s.
pub const CHECK_TICK_S: f64 = 0.01;

/// The alarm band of every control, dB either way.
pub const CHECK_ALARM_DB: f64 = 1.5;

/// The abort band of every control, dB either way.
pub const CHECK_ABORT_DB: f64 = 6.0;

/// Every drive's ceiling, volts peak.
pub const CHECK_CEILING_V: f64 = 2.0;

/// Every drive's abort, volts peak.
pub const CHECK_DRIVE_ABORT_V: f64 = 4.0;

/// The identification's level, dB under the reference.
pub const CHECK_IDENT_DB: f64 = -20.0;

/// Measurement noise on every input, g rms per sample (about 40 dB under the
/// structures' full level).
pub const CHECK_NOISE_G: f64 = 0.01;

/// The update from which the misbehaving law's answers are refused.
pub const CHECK_MISBEHAVE_FROM: u64 = 150;

/// The bound on each article response in the limit case, g.
pub const CHECK_LIMIT_BOUND_G: f64 = 2.0;

/// How long after a notch releases the controls are watched for overshoot, s.
pub const CHECK_RELEASE_WATCH_S: f64 = 1.0;

/// Why a case could not be run at all (as distinct from a run the law
/// failed).
#[derive(Debug, Clone, PartialEq)]
pub enum CheckRunError {
    /// The case's run could not be set up, identified or engaged.
    Setup(String),
}

impl fmt::Display for CheckRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckRunError::Setup(s) => write!(f, "the check case could not be run: {s}"),
        }
    }
}

impl std::error::Error for CheckRunError {}

/// What the first version's case-free entry answers: a check needs a law.
/// [`check_law`] runs the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckNotBuilt;

impl fmt::Display for CheckNotBuilt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the MIMO Sine law check is not built in this version; no case was run")
    }
}

impl std::error::Error for CheckNotBuilt {}

/// The first version's case-free entry, kept so its callers still build: it
/// runs nothing. [`check_law`] runs the check of a law.
pub fn run_check() -> Result<(), CheckNotBuilt> {
    Err(CheckNotBuilt)
}

/// A standard sine structure the check uses: its name and the facts a
/// case's setup needs (checked against the simulator in the tests).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckStructure {
    /// The simulator's name for it.
    pub name: &'static str,
    /// Every response it gives: its controls, then any article responses.
    pub outputs: usize,
    /// How many of the outputs are controls (the first ones).
    pub controls: usize,
    /// Its drives.
    pub drives: usize,
    /// Its full level at every control, g peak.
    pub full_level_g: f64,
}

/// The standard sine structures, by name, with their sizes.
pub const CHECK_STRUCTURES: [CheckStructure; 10] = [
    plate("sine-linear-4x4"),
    plate("sine-softening-4x4-mild"),
    plate("sine-softening-4x4-moderate"),
    plate("sine-hardening-4x4"),
    CheckStructure {
        name: "sine-tall-6x3",
        outputs: 6,
        controls: 6,
        drives: 3,
        full_level_g: 1.0,
    },
    CheckStructure {
        name: "sine-wide-3x6",
        outputs: 3,
        controls: 3,
        drives: 6,
        full_level_g: 1.0,
    },
    CheckStructure {
        name: "sine-base-article-6x4",
        outputs: 6,
        controls: 4,
        drives: 4,
        full_level_g: 1.0,
    },
    plate("sine-near-singular-4x4"),
    plate("sine-weak-4x4"),
    plate("sine-loopback-4x4"),
];

const fn plate(name: &'static str) -> CheckStructure {
    CheckStructure {
        name,
        outputs: 4,
        controls: 4,
        drives: 4,
        full_level_g: 1.0,
    }
}

/// The standard structure named `name`.
pub fn check_structure(name: &str) -> Option<CheckStructure> {
    CHECK_STRUCTURES.into_iter().find(|s| s.name == name)
}

/// What a case changes in its standard structure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StructureEdit {
    /// Nothing.
    None,
    /// The structure's mass line alone: every elastic mode taken out, so the
    /// response is coupled but has no resonance.
    MassLineOnly,
    /// The mode nearest `from_hz` moved to `to_hz`, its shapes and damping
    /// kept.
    MoveMode {
        /// Which mode, by its frequency, Hz.
        from_hz: f64,
        /// Where it goes, Hz.
        to_hz: f64,
    },
}

/// The simulated station a case runs on: what the caller builds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StationSpec {
    /// The standard structure, by name.
    pub structure: &'static str,
    /// What is changed in it.
    pub edit: StructureEdit,
    /// A delay on every drive, s.
    pub transport_delay_s: f64,
    /// Noise on every input, g rms per sample.
    pub noise_g: f64,
    /// The seed of the station's noise.
    pub seed: u64,
    /// The rate, Hz.
    pub fs_hz: f64,
}

/// What a case asks of the run beyond the requirements every case asks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Requirement {
    /// Every update from `after_s` after the engage on (settle window or
    /// not) within `db` of the reference.
    Settles {
        /// The band, dB either way.
        db: f64,
        /// How long after the engage the band applies, s.
        after_s: f64,
    },
    /// Every judged update within `db`.
    Within {
        /// The band, dB either way.
        db: f64,
    },
    /// Every judged update within `db`, and the median error over every
    /// control and judged update within `median_db`.
    SweepWithin {
        /// The band, dB either way.
        db: f64,
        /// The median's band, dB.
        median_db: f64,
    },
    /// Within `through_db` within `away_hz` of the resonance at
    /// `resonance_hz`, and within `away_db` further from it.
    WithinAway {
        /// The band through the resonance, dB either way.
        through_db: f64,
        /// The band away from it, dB either way.
        away_db: f64,
        /// The resonance, Hz.
        resonance_hz: f64,
        /// How far from the resonance the tighter band applies, Hz.
        away_hz: f64,
    },
    /// As [`Requirement::SweepWithin`], each control's error taken against
    /// the response the drives can reach: the least-squares fit through the
    /// identification.
    Reachable {
        /// The band, dB either way.
        db: f64,
        /// The median's band, dB.
        median_db: f64,
    },
    /// As [`Requirement::SweepWithin`], and the opposition ratio (the drive's
    /// norm over the least norm that gives the same response) at most
    /// `opposition_db` on every judged update.
    WithinEffort {
        /// The band, dB either way.
        db: f64,
        /// The median's band, dB.
        median_db: f64,
        /// The largest opposition ratio, dB.
        opposition_db: f64,
    },
    /// At every rung of the ladder, every judged update within `db` of the
    /// reference at that rung.
    EachRungWithin {
        /// The band, dB either way.
        db: f64,
    },
    /// The backstop notch engages; the limit channels never over their bound
    /// by more than `over_bound_db`; no control over its reference by more
    /// than `overshoot_db` in the [`CHECK_RELEASE_WATCH_S`] after a release.
    LimitHeld {
        /// How far over the bound a limit may read, dB.
        over_bound_db: f64,
        /// How far over its reference a control may read after a release,
        /// dB.
        overshoot_db: f64,
    },
    /// The run completes or ends on the engine's own control abort or
    /// divergence stop, with no sample over a ceiling.
    Survive,
    /// The run ends on a law fault after the miss rule's count of refused
    /// answers in a row, every drive walked to zero.
    StopsOnFault,
}

impl Requirement {
    /// The requirement's name in a verdict.
    pub fn as_str(self) -> &'static str {
        match self {
            Requirement::Settles { .. } => "settles",
            Requirement::Within { .. } => "within",
            Requirement::SweepWithin { .. } => "sweep_within",
            Requirement::WithinAway { .. } => "within_away",
            Requirement::Reachable { .. } => "reachable",
            Requirement::WithinEffort { .. } => "within_effort",
            Requirement::EachRungWithin { .. } => "each_rung_within",
            Requirement::LimitHeld { .. } => "limit_held",
            Requirement::Survive => "survive",
            Requirement::StopsOnFault => "stops_on_fault",
        }
    }
}

/// The cases of the check, one simulated run each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimoSineCheckCase {
    /// A dwell at a resonance of the linear four-by-four structure.
    DwellAtResonance,
    /// A dwell between its resonances.
    DwellAwayFromResonance,
    /// A slow sweep on the linear four-by-four structure.
    SlowSweep,
    /// A fast sweep on the same structure.
    FastSweep,
    /// Two resonances 2 percent apart.
    CloseModes,
    /// A frequency where the response is nearly singular.
    NearSingular,
    /// Noise on the measurement.
    MeasurementNoise,
    /// A level ladder up to full level on a softening structure.
    LevelLadder,
    /// Limit channels that bind near a resonance of the article.
    LimitChannel,
    /// A delay between the drive and its measurement.
    TransportDelay,
    /// More controls than drives.
    MoreControlsThanDrives,
    /// More drives than controls.
    MoreDrivesThanControls,
    /// A stepped sine.
    Stepped,
    /// The law's answers become unusable part way through.
    LawMisbehaves,
    /// A sweep where the resonance moves 0.15 of its bandwidth at full
    /// level; reported only.
    MovedResonanceMild,
    /// The same, moved 0.75 of its bandwidth; reported only.
    MovedResonanceModerate,
    /// A hardening structure, moved 0.75 of a bandwidth up; reported only.
    MovedResonanceHardening,
}

impl MimoSineCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [MimoSineCheckCase; 17] = [
        MimoSineCheckCase::DwellAtResonance,
        MimoSineCheckCase::DwellAwayFromResonance,
        MimoSineCheckCase::MeasurementNoise,
        MimoSineCheckCase::TransportDelay,
        MimoSineCheckCase::LevelLadder,
        MimoSineCheckCase::Stepped,
        MimoSineCheckCase::SlowSweep,
        MimoSineCheckCase::FastSweep,
        MimoSineCheckCase::CloseModes,
        MimoSineCheckCase::NearSingular,
        MimoSineCheckCase::LimitChannel,
        MimoSineCheckCase::MoreControlsThanDrives,
        MimoSineCheckCase::MoreDrivesThanControls,
        MimoSineCheckCase::LawMisbehaves,
        MimoSineCheckCase::MovedResonanceMild,
        MimoSineCheckCase::MovedResonanceModerate,
        MimoSineCheckCase::MovedResonanceHardening,
    ];

    /// The stable name.
    pub fn name(self) -> &'static str {
        match self {
            MimoSineCheckCase::DwellAtResonance => "dwell_at_resonance",
            MimoSineCheckCase::DwellAwayFromResonance => "dwell_away_from_resonance",
            MimoSineCheckCase::SlowSweep => "slow_sweep",
            MimoSineCheckCase::FastSweep => "fast_sweep",
            MimoSineCheckCase::CloseModes => "close_modes",
            MimoSineCheckCase::NearSingular => "near_singular",
            MimoSineCheckCase::MeasurementNoise => "measurement_noise",
            MimoSineCheckCase::LevelLadder => "level_ladder",
            MimoSineCheckCase::LimitChannel => "limit_channel",
            MimoSineCheckCase::TransportDelay => "transport_delay",
            MimoSineCheckCase::MoreControlsThanDrives => "more_controls_than_drives",
            MimoSineCheckCase::MoreDrivesThanControls => "more_drives_than_controls",
            MimoSineCheckCase::Stepped => "stepped",
            MimoSineCheckCase::LawMisbehaves => "law_misbehaves",
            MimoSineCheckCase::MovedResonanceMild => "moved_resonance_mild",
            MimoSineCheckCase::MovedResonanceModerate => "moved_resonance_moderate",
            MimoSineCheckCase::MovedResonanceHardening => "moved_resonance_hardening",
        }
    }

    /// The case named `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What the case is, in words, for the check report.
    pub fn describe(self) -> &'static str {
        match self {
            MimoSineCheckCase::DwellAtResonance => {
                "a 6 s dwell at the 96 Hz mode of the linear four-by-four structure"
            }
            MimoSineCheckCase::DwellAwayFromResonance => {
                "a 6 s dwell at 130 Hz, between the linear structure's modes"
            }
            MimoSineCheckCase::MeasurementNoise => {
                "a dwell at 96 Hz with noise 30 dB under full level on every input"
            }
            MimoSineCheckCase::TransportDelay => {
                "a 50 ms delay on every drive, engaged at -6 dB, a 6 s dwell at 100 Hz"
            }
            MimoSineCheckCase::LevelLadder => {
                "rungs at -20, -12, -6 and 0 dB at a resonance of a mildly softening structure"
            }
            MimoSineCheckCase::Stepped => "a stepped sine from 60 to 120 Hz, six steps an octave",
            MimoSineCheckCase::SlowSweep => "a sweep through the 96 Hz mode at one octave a minute",
            MimoSineCheckCase::FastSweep => {
                "a sweep through the 96 Hz mode at four octaves a minute"
            }
            MimoSineCheckCase::CloseModes => {
                "two modes 2 percent apart, swept at two octaves a minute"
            }
            MimoSineCheckCase::NearSingular => {
                "a sweep through a frequency where the response is nearly singular"
            }
            MimoSineCheckCase::LimitChannel => {
                "a base and article with the article's responses limited at 2 g"
            }
            MimoSineCheckCase::MoreControlsThanDrives => "six controls on three drives",
            MimoSineCheckCase::MoreDrivesThanControls => "three controls on six drives",
            MimoSineCheckCase::LawMisbehaves => {
                "the law's answer turns non-finite part way through a dwell"
            }
            MimoSineCheckCase::MovedResonanceMild => {
                "a sweep where the resonance moves 0.15 of its bandwidth at full level"
            }
            MimoSineCheckCase::MovedResonanceModerate => {
                "a sweep where the resonance moves 0.75 of its bandwidth at full level"
            }
            MimoSineCheckCase::MovedResonanceHardening => {
                "a sweep where the resonance rises 0.75 of its bandwidth at full level"
            }
        }
    }

    /// The test type the case runs.
    pub fn test_type(self) -> TestType {
        self.plan().test_type()
    }

    /// Whether the case decides the check; the moved-resonance cases are
    /// reported only.
    pub fn scored(self) -> bool {
        !matches!(
            self,
            MimoSineCheckCase::MovedResonanceMild
                | MimoSineCheckCase::MovedResonanceModerate
                | MimoSineCheckCase::MovedResonanceHardening
        )
    }

    /// The standard structure the case runs on.
    pub fn structure(self) -> CheckStructure {
        let name = match self {
            MimoSineCheckCase::LevelLadder | MimoSineCheckCase::MovedResonanceMild => {
                "sine-softening-4x4-mild"
            }
            MimoSineCheckCase::MovedResonanceModerate => "sine-softening-4x4-moderate",
            MimoSineCheckCase::MovedResonanceHardening => "sine-hardening-4x4",
            MimoSineCheckCase::NearSingular => "sine-near-singular-4x4",
            MimoSineCheckCase::LimitChannel => "sine-base-article-6x4",
            MimoSineCheckCase::MoreControlsThanDrives => "sine-tall-6x3",
            MimoSineCheckCase::MoreDrivesThanControls => "sine-wide-3x6",
            _ => "sine-linear-4x4",
        };
        check_structure(name).expect("a standard structure")
    }

    /// The station the case runs on.
    pub fn station(self) -> StationSpec {
        let (edit, delay, noise) = match self {
            MimoSineCheckCase::TransportDelay => {
                (StructureEdit::MassLineOnly, 0.05, CHECK_NOISE_G / 100.0)
            }
            MimoSineCheckCase::CloseModes => (
                StructureEdit::MoveMode {
                    from_hz: 165.0,
                    to_hz: 96.0 * 1.02,
                },
                0.0,
                CHECK_NOISE_G,
            ),
            MimoSineCheckCase::MeasurementNoise => {
                (StructureEdit::None, 0.0, 10f64.powf(-30.0 / 20.0))
            }
            _ => (StructureEdit::None, 0.0, CHECK_NOISE_G),
        };
        StationSpec {
            structure: self.structure().name,
            edit,
            transport_delay_s: delay,
            noise_g: noise,
            seed: 0x5eed_5100 + self as u64,
            fs_hz: CHECK_FS_HZ,
        }
    }

    /// The test the case runs.
    pub fn plan(self) -> TestPlan {
        let dwell = |f: f64, s: f64| {
            TestPlan::Dwell(DwellPlan {
                freq_hz: f,
                for_: DwellFor::Seconds(s),
            })
        };
        let sweep = |lo: f64, hi: f64, rate: f64| {
            TestPlan::Sweep(BidirSweepConfig {
                band: (lo, hi),
                rate: SweepRate::LogOctPerMin(rate),
                start_hz: lo,
                initial_direction: SweepDirection::Up,
                target: SweepTarget::Sweeps(1),
            })
        };
        match self {
            MimoSineCheckCase::DwellAtResonance
            | MimoSineCheckCase::MeasurementNoise
            | MimoSineCheckCase::LawMisbehaves => dwell(96.0, 6.0),
            MimoSineCheckCase::DwellAwayFromResonance => dwell(130.0, 6.0),
            MimoSineCheckCase::TransportDelay => dwell(100.0, 6.0),
            MimoSineCheckCase::LevelLadder => dwell(96.0, 8.0),
            MimoSineCheckCase::Stepped => TestPlan::Stepped(StepPlan::Grid(StepSineConfig {
                band: (60.0, 120.0),
                steps_per_octave: 6.0,
                spacing: StepSpacing::Log,
                dwell: Some(DwellFor::Seconds(2.0)),
            })),
            MimoSineCheckCase::SlowSweep
            | MimoSineCheckCase::MoreControlsThanDrives
            | MimoSineCheckCase::MoreDrivesThanControls
            | MimoSineCheckCase::MovedResonanceMild
            | MimoSineCheckCase::MovedResonanceModerate => sweep(80.0, 120.0, 1.0),
            MimoSineCheckCase::FastSweep => sweep(80.0, 160.0, 4.0),
            MimoSineCheckCase::CloseModes => sweep(80.0, 120.0, 2.0),
            MimoSineCheckCase::NearSingular => sweep(350.0, 420.0, 1.0),
            MimoSineCheckCase::LimitChannel => sweep(150.0, 350.0, 2.0),
            MimoSineCheckCase::MovedResonanceHardening => sweep(140.0, 190.0, 1.0),
        }
    }

    /// The level ladder, dB: the levels the run steps through, the last the
    /// judged one.
    pub fn ladder_db(self) -> Vec<f64> {
        match self {
            MimoSineCheckCase::TransportDelay => vec![-6.0],
            MimoSineCheckCase::LevelLadder => vec![-20.0, -12.0, -6.0, 0.0],
            _ => vec![0.0],
        }
    }

    /// The longest the run may take, test time, s.
    pub fn max_s(self) -> f64 {
        match self.plan() {
            TestPlan::Dwell(d) => match d.for_ {
                DwellFor::Seconds(s) => s + 10.0,
                _ => 30.0,
            },
            TestPlan::Sweep(c) => {
                let rate = match c.rate {
                    SweepRate::LogOctPerMin(r) => r,
                    _ => 1.0,
                };
                60.0 * (c.band.1 / c.band.0).log2() / rate + 10.0
            }
            TestPlan::Stepped(_) => 40.0,
        }
    }

    /// What the case asks beyond what every case asks.
    pub fn requirement(self) -> Requirement {
        match self {
            MimoSineCheckCase::DwellAtResonance | MimoSineCheckCase::DwellAwayFromResonance => {
                Requirement::Settles {
                    db: 1.0,
                    after_s: 2.0,
                }
            }
            MimoSineCheckCase::MeasurementNoise => Requirement::Within { db: 1.5 },
            MimoSineCheckCase::TransportDelay => Requirement::Settles {
                db: 0.5,
                after_s: 2.0,
            },
            MimoSineCheckCase::LevelLadder => Requirement::EachRungWithin { db: 1.0 },
            MimoSineCheckCase::Stepped => Requirement::Within { db: 1.5 },
            MimoSineCheckCase::SlowSweep
            | MimoSineCheckCase::MovedResonanceMild
            | MimoSineCheckCase::MovedResonanceModerate
            | MimoSineCheckCase::MovedResonanceHardening => Requirement::SweepWithin {
                db: 1.5,
                median_db: 0.5,
            },
            MimoSineCheckCase::FastSweep => Requirement::WithinAway {
                through_db: 5.0,
                away_db: 2.0,
                resonance_hz: 96.0,
                away_hz: 12.0,
            },
            MimoSineCheckCase::CloseModes => Requirement::Within { db: 2.0 },
            MimoSineCheckCase::NearSingular => Requirement::Survive,
            MimoSineCheckCase::LimitChannel => Requirement::LimitHeld {
                over_bound_db: 0.5,
                overshoot_db: 1.0,
            },
            MimoSineCheckCase::MoreControlsThanDrives => Requirement::Reachable {
                db: 3.0,
                median_db: 0.5,
            },
            MimoSineCheckCase::MoreDrivesThanControls => Requirement::WithinEffort {
                db: 1.5,
                median_db: 0.5,
                opposition_db: 6.0,
            },
            MimoSineCheckCase::LawMisbehaves => Requirement::StopsOnFault,
        }
    }
}

/// The cases a law with `manifest` is run on: those of the test types it
/// accepts, scored and reported, in the check's order.
pub fn cases_for(manifest: &LawManifest) -> Vec<MimoSineCheckCase> {
    MimoSineCheckCase::ALL
        .into_iter()
        .filter(|c| manifest.test_types.contains(&c.test_type()))
        .collect()
}

/// A flat acceleration profile at `g` over 5 to 2000 Hz with the given
/// alarm and abort bands.
fn flat(g: f64, alarm_db: f64, abort_db: f64) -> SineProfile {
    let mut p = SineProfile::new("flat");
    p.global_alarm_db = (alarm_db, alarm_db);
    p.global_abort_db = (abort_db, abort_db);
    p.push(ProfileRow::accel(5.0, g));
    p.push(ProfileRow::accel(2000.0, g));
    p
}

/// The channel table of `s`: its controls, its other responses (limit
/// channels), then its drives; channel `k` has index `k + 1`.
pub fn check_table(s: CheckStructure) -> ChannelTable {
    let mut t = ChannelTable::new();
    for k in 0..s.outputs + s.drives {
        let mut c = Channel::new(k as u32 + 1);
        c.role = if k < s.controls {
            ChannelRole::Control
        } else if k < s.outputs {
            ChannelRole::Monitor
        } else {
            ChannelRole::Drive
        };
        c.range_max = 1e9;
        t.channels.push(c);
    }
    t
}

/// One run on the engine, as a check case or a study run plays it: the
/// structure and its station, the test, the level ladder, the limits and
/// how the run is measured.
#[derive(Debug, Clone, PartialEq)]
pub struct RunPlan {
    /// The structure.
    pub structure: CheckStructure,
    /// The station the caller builds.
    pub station: StationSpec,
    /// The test.
    pub test: TestPlan,
    /// The levels, dB; the last is the judged one. Each rung but the last is
    /// held [`RUNG_S`].
    pub ladder_db: Vec<f64>,
    /// The bound on every response that is not a control, g; `None` for no
    /// limit channels.
    pub limit_bound_g: Option<f64>,
    /// Every control's abort band, dB either way.
    pub abort_db: f64,
    /// The longest the run may take, test time, s.
    pub max_s: f64,
    /// Judge each control against the response the drives can reach.
    pub reachable: bool,
    /// Measure the worst error from this long after the engage, s.
    pub settle_after_s: Option<f64>,
    /// Measure near (`.1` Hz either side of) and away from this resonance,
    /// Hz.
    pub resonance: Option<(f64, f64)>,
    /// Make the law's answers unusable from this update on.
    pub misbehave_from: Option<u64>,
}

/// How long each rung of a ladder below the last is held, s.
pub const RUNG_S: f64 = 2.0;

/// The laws that declare themselves for weakly coupled pairs (each control
/// with its own drive, no model of the coupling between pairs), by name:
/// their dwells, the measurement noise, the stepped sine and the level ladder
/// run on [`PAIRED_STRUCTURE`] instead of the coupled plate. Their safety
/// cases stay where every law runs them.
pub const PAIRED_SETUP_LAWS: [&str; 1] = ["paired"];

/// The structure the cases of a law in [`PAIRED_SETUP_LAWS`] run on where the
/// coupled plate would otherwise be used.
pub const PAIRED_STRUCTURE: &str = "sine-weak-4x4";

impl MimoSineCheckCase {
    /// Whether a law that declares itself for weakly coupled pairs runs this
    /// case on [`PAIRED_STRUCTURE`].
    pub fn runs_on_pairs(self) -> bool {
        matches!(
            self,
            MimoSineCheckCase::DwellAtResonance
                | MimoSineCheckCase::DwellAwayFromResonance
                | MimoSineCheckCase::MeasurementNoise
                | MimoSineCheckCase::LevelLadder
                | MimoSineCheckCase::Stepped
        )
    }

    /// The case's run for the law named `law`: [`Self::run_plan`], on
    /// [`PAIRED_STRUCTURE`] where the law is one of [`PAIRED_SETUP_LAWS`] and
    /// the case [`Self::runs_on_pairs`].
    pub fn run_plan_for(self, law: &str) -> RunPlan {
        let mut p = self.run_plan();
        if PAIRED_SETUP_LAWS.contains(&law) && self.runs_on_pairs() {
            p.structure = check_structure(PAIRED_STRUCTURE).expect("a standard structure");
            p.station.structure = PAIRED_STRUCTURE;
        }
        p
    }

    /// The case's run.
    pub fn run_plan(self) -> RunPlan {
        let req = self.requirement();
        RunPlan {
            structure: self.structure(),
            station: self.station(),
            test: self.plan(),
            ladder_db: self.ladder_db(),
            limit_bound_g: (self == MimoSineCheckCase::LimitChannel).then_some(CHECK_LIMIT_BOUND_G),
            // A delay's first updates read a response that has not arrived
            // yet: a control briefly far from its reference is the case, not
            // a fault.
            abort_db: if self == MimoSineCheckCase::TransportDelay {
                20.0
            } else {
                CHECK_ABORT_DB
            },
            max_s: self.max_s(),
            reachable: matches!(req, Requirement::Reachable { .. }),
            settle_after_s: match req {
                Requirement::Settles { after_s, .. } => Some(after_s),
                _ => None,
            },
            resonance: match req {
                Requirement::WithinAway {
                    resonance_hz,
                    away_hz,
                    ..
                } => Some((resonance_hz, away_hz)),
                _ => None,
            },
            misbehave_from: (self == MimoSineCheckCase::LawMisbehaves)
                .then_some(CHECK_MISBEHAVE_FROM),
        }
    }
}

/// The setup `case` runs with a law of `id` and `manifest`.
pub fn check_setup(case: MimoSineCheckCase, id: &str, manifest: &LawManifest) -> MimoSineSetup {
    plan_setup(&case.run_plan(), id, manifest)
}

/// The setup of the run `p` with a law of `id` and `manifest`: every control
/// at full level on a flat profile, in phase unless the law accepts an
/// amplitude-only reference.
pub fn plan_setup(p: &RunPlan, id: &str, manifest: &LawManifest) -> MimoSineSetup {
    let s = p.structure;
    let g = s.full_level_g;
    let phased = !manifest.amplitude_only;
    let ladder = p.ladder_db.clone();
    let start = ladder[0];
    let limits = match p.limit_bound_g {
        Some(bound) => (s.controls..s.outputs)
            .map(|j| LimitChannel {
                channel: 1 + j,
                profile: flat(bound, CHECK_ALARM_DB, 3.0),
            })
            .collect(),
        None => Vec::new(),
    };
    let rung = if ladder.len() > 1 { RUNG_S } else { 1e6 };
    let abort_db = p.abort_db;
    MimoSineSetup {
        controls: (0..s.controls)
            .map(|i| ControlChannel {
                channel: 1 + i,
                profile: flat(g, CHECK_ALARM_DB, abort_db),
                phase: if phased {
                    vec![PhasePoint {
                        freq_hz: 5.0,
                        phase_deg: 0.0,
                    }]
                } else {
                    vec![]
                },
            })
            .collect(),
        drives: (0..s.drives)
            .map(|i| DriveChannel {
                channel: 1 + s.outputs + i,
                ceiling_vpk: CHECK_CEILING_V,
                abort_vpk: CHECK_DRIVE_ABORT_V,
            })
            .collect(),
        limits,
        phase_specified: phased,
        test: p.test.clone(),
        schedule: Schedule {
            mode: LevelMode::Auto,
            ladder: LadderConfig {
                sysid_level_db: CHECK_IDENT_DB,
                start_db: start,
                table_db: ladder,
                per_rung: RungFor::Seconds(rung),
                full_for: RungFor::Seconds(1e6),
            },
        },
        fs_hz: CHECK_FS_HZ,
        tick_s: CHECK_TICK_S,
        tracking: Default::default(),
        identification: IdentificationSettings {
            level_db: CHECK_IDENT_DB,
            resolution_hz: 0.25,
            frames: Some(6.max(s.drives as u32 + 2)),
            trust_coherence: 0.5,
            gate_coherence: 0.5,
            review: false,
        },
        law: LawChoice {
            id: id.into(),
            params: Default::default(),
        },
        requests: Default::default(),
        guards: Default::default(),
    }
}

/// One control's errors over a case's judged updates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlErrors {
    /// The worst |error|, dB.
    pub worst_db: f64,
    /// The median |error|, dB.
    pub median_db: f64,
    /// The worst |phase error|, degrees; NaN where the reference is
    /// amplitude only.
    pub worst_deg: f64,
    /// The median |phase error|, degrees; NaN where amplitude only.
    pub median_deg: f64,
}

/// What a case's run measured.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseMeasure {
    /// The case; `None` for a run that is not a check case.
    pub case: Option<MimoSineCheckCase>,
    /// How the run ended; `None` when it had not ended in the case's time.
    pub outcome: Option<Outcome>,
    /// Where the run's stop began, Hz, when it stopped early.
    pub stop_hz: Option<f64>,
    /// Updates whose answer was refused.
    pub misses: usize,
    /// Updates where the engine scaled an answer down to the ceilings.
    pub over_ceiling: u64,
    /// The largest drive sample played, V.
    pub largest_sample_v: f64,
    /// The largest drive played against its ceiling, dB (0 at the ceiling).
    pub largest_drive_db: f64,
    /// Whether every drive was at zero when the run ended.
    pub drive_is_zero: bool,
    /// Judged updates.
    pub judged: usize,
    /// The worst |error| over every control and judged update, dB (against
    /// the reachable response where the case judges that).
    pub worst_db: f64,
    /// The median |error| over every control and judged update, dB.
    pub median_db: f64,
    /// Per control.
    pub per_control: Vec<ControlErrors>,
    /// The worst |error| from the settle time on, dB (`Settles` cases).
    pub settled_db: f64,
    /// The updates `settled_db` is taken over.
    pub settled_updates: usize,
    /// The worst |error| near the resonance, dB (`WithinAway`).
    pub through_db: f64,
    /// The worst |error| away from it, dB (`WithinAway`).
    pub away_db: f64,
    /// Per rung: its level and its worst judged |error|, dB (NaN when the
    /// rung had no judged update).
    pub rungs: Vec<(f64, f64)>,
    /// The worst limit reading over its bound, dB (negative under it; minus
    /// infinity with no limit).
    pub limit_over_db: f64,
    /// Whether the backstop notch engaged.
    pub notch_engaged: bool,
    /// The deepest notch, dB (0 for none).
    pub deepest_notch_db: f64,
    /// The worst control over its reference in the watch after a release,
    /// dB (minus infinity with no release).
    pub release_overshoot_db: f64,
    /// The same per control, dB (minus infinity with no release).
    pub release_overshoot_by_control_db: Vec<f64>,
    /// How many times the backstop notch went in (a notch that toggles goes
    /// in many times).
    pub notch_entries: usize,
    /// The largest opposition ratio over the judged updates, dB.
    pub opposition_db: f64,
    /// Wall time of the identification, s.
    pub ident_s: f64,
    /// Wall time of the engage (the law's build), s.
    pub build_s: f64,
    /// Wall time of the run after the engage, s.
    pub run_s: f64,
}

/// A law's answers, refused from an update on: one drive non-finite.
struct Misbehaving(Arc<dyn MimoSineLawFactory>, u64);

struct MisbehavingLaw(Box<dyn MimoSineLaw + Send>, u64);

impl MimoSineLaw for MisbehavingLaw {
    fn name(&self) -> &str {
        "misbehaving"
    }
    fn engage(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.engage(x, o, v)
    }
    fn tick(
        &mut self,
        x: &TickInputs<'_>,
        o: &mut TickOutputs,
        v: &mut LawValues,
    ) -> Result<(), LawFault> {
        self.0.tick(x, o, v)?;
        if x.index >= self.1 {
            o.drive[0] = C64::new(f64::NAN, 0.0);
        }
        Ok(())
    }
}

impl MimoSineLawFactory for Misbehaving {
    fn build(&self, s: &LawSetup) -> Result<Box<dyn MimoSineLaw + Send>, LawFault> {
        Ok(Box::new(MisbehavingLaw(self.0.build(s)?, self.1)))
    }
    fn describe(&self) -> LawDescriptor {
        self.0.describe()
    }
    fn manifest(&self) -> LawManifest {
        self.0.manifest()
    }
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-300).log10()
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn largest(v: impl IntoIterator<Item = f64>) -> f64 {
    v.into_iter().fold(f64::NAN, f64::max)
}

/// The response the drives can reach at the update `t`, by the
/// identification: `H Z |R|` with `Z` the regularized pseudo-inverse.
fn reachable(id: &LawIdentification, n: usize, m: usize, t: &TickRecord) -> Vec<C64> {
    use hertzforge_mimo_sine_law_math::{linalg, setup};
    let mut h = vec![C64::new(0.0, 0.0); n * m];
    setup::interp_stack(
        &id.frequency_hz,
        id.response.data(),
        n * m,
        t.frequency_hz,
        &mut h,
    );
    let Ok((z, _)) = linalg::reg_pinv(&h, n, m, 1e-3) else {
        return t.judged_reference[..n].to_vec();
    };
    let r: Vec<C64> = t.judged_reference[..n]
        .iter()
        .map(|v| C64::new(v.norm(), 0.0))
        .collect();
    let mut d = vec![C64::new(0.0, 0.0); m];
    linalg::mat_vec(&z, m, n, &r, &mut d);
    let mut c = vec![C64::new(0.0, 0.0); n];
    linalg::mat_vec(&h, n, m, &d, &mut c);
    c
}

/// Run `case` with the law `factory` builds on `backend` (the station built
/// from [`MimoSineCheckCase::station`]) and measure it.
pub fn run_check_case(
    case: MimoSineCheckCase,
    factory: Arc<dyn MimoSineLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<CaseMeasure, CheckRunError> {
    let mut m = run_plan(&case.run_plan(), factory, backend)?;
    m.case = Some(case);
    Ok(m)
}

/// Run `plan` with the law `factory` builds on `backend` (the station built
/// from `plan.station`) and measure it.
pub fn run_plan(
    plan: &RunPlan,
    factory: Arc<dyn MimoSineLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<CaseMeasure, CheckRunError> {
    let manifest = factory.manifest();
    let id = factory.describe().id;
    let setup = plan_setup(plan, &id, &manifest);
    let phased = setup.phase_specified;
    let s = plan.structure;
    let factory: Arc<dyn MimoSineLawFactory> = match plan.misbehave_from {
        Some(from) => Arc::new(Misbehaving(factory, from)),
        None => factory,
    };
    let err = |e: hertzforge_mimo_sine::service::ServiceError| CheckRunError::Setup(e.to_string());
    let mut svc = MimoSineService::new(
        setup,
        check_table(s),
        backend,
        factory,
        EngineOptions::new(StationClass::HertzforgeSim),
    )
    .map_err(err)?;
    svc.arm().map_err(err)?;
    let t0 = Instant::now();
    let gate = svc.identify().map_err(err)?.gate_passed;
    let ident_s = t0.elapsed().as_secs_f64();
    if !gate {
        return Err(CheckRunError::Setup(
            "the identification's coherence gate held the run".into(),
        ));
    }
    let t1 = Instant::now();
    svc.engage().map_err(err)?;
    let build_s = t1.elapsed().as_secs_f64();
    let id = svc
        .identification()
        .expect("identified")
        .identification
        .clone();
    let (rt, re) = svc
        .take_records()
        .ok_or_else(|| CheckRunError::Setup("the records were already taken".into()))?;
    let t2 = Instant::now();
    let mut ticks: Vec<TickRecord> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    let rounds = (plan.max_s * CHECK_FS_HZ / 512.0 / 16.0) as u64 + 64;
    for _ in 0..rounds {
        let alive = svc.run_batches(16).map_err(err)?;
        ticks.extend(rt.try_iter());
        events.extend(re.try_iter());
        if !alive {
            break;
        }
    }
    let run_s = t2.elapsed().as_secs_f64();
    let outcome = match svc.state() {
        ServiceState::Ended(o) => Some(o),
        _ => {
            svc.request_stop();
            let _ = svc.run_batches(4096);
            None
        }
    };
    let core = svc.core().expect("engaged");
    let station = Station {
        over_ceiling: core.over_ceiling_count(),
        largest_sample_v: core.largest_sample_v(),
        drive_is_zero: core.drive_is_zero(),
    };
    Ok(measure(
        plan,
        &ticks,
        &events,
        outcome,
        station,
        &id,
        phased,
        (ident_s, build_s, run_s),
    ))
}

struct Station {
    over_ceiling: u64,
    largest_sample_v: f64,
    drive_is_zero: bool,
}

#[allow(clippy::too_many_arguments)]
fn measure(
    plan: &RunPlan,
    ticks: &[TickRecord],
    events: &[Event],
    outcome: Option<Outcome>,
    st: Station,
    id: &LawIdentification,
    phased: bool,
    (ident_s, build_s, run_s): (f64, f64, f64),
) -> CaseMeasure {
    let s = plan.structure;
    let (n, m) = (s.controls, s.drives);
    let ladder = &plan.ladder_db;
    let top = *ladder.last().unwrap_or(&0.0);
    let at = |t: &TickRecord, l: f64| (t.level_db - l).abs() < 1e-6;
    let judged: Vec<&TickRecord> = ticks
        .iter()
        .filter(|t| !t.settling && t.miss.is_none() && at(t, top))
        .collect();
    let reach = plan.reachable;
    let errs = |t: &TickRecord| -> Vec<f64> {
        if reach {
            let c = reachable(id, n, m, t);
            (0..n)
                .map(|k| db(t.control[k].norm() / c[k].norm().max(1e-300)).abs())
                .collect()
        } else {
            t.errors_db().iter().map(|e| e.abs()).collect()
        }
    };
    let mut by_control: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut by_control_deg: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut all = Vec::new();
    let mut opposition = f64::NAN;
    let (mut through, mut away) = (f64::NAN, f64::NAN);
    for t in &judged {
        let e = errs(t);
        for (k, &x) in e.iter().enumerate() {
            by_control[k].push(x);
            all.push(x);
            let r = t.judged_reference[k];
            if phased && t.control[k].norm() > 0.0 && r.norm() > 0.0 {
                by_control_deg[k].push((t.control[k] / r).arg().to_degrees().abs());
            }
        }
        let w = largest(e.iter().copied());
        if let Some((resonance_hz, away_hz)) = plan.resonance {
            if (t.frequency_hz - resonance_hz).abs() <= away_hz {
                through = through.max(w);
            } else {
                away = away.max(w);
            }
        }
        if t.opposition_ratio.is_finite() && t.opposition_ratio > 0.0 {
            opposition = opposition.max(db(t.opposition_ratio));
        }
    }
    let per_control = by_control
        .iter_mut()
        .zip(by_control_deg.iter_mut())
        .map(|(e, d)| ControlErrors {
            worst_db: largest(e.iter().copied()),
            median_db: median(e),
            worst_deg: largest(d.iter().copied()),
            median_deg: median(d),
        })
        .collect();
    let worst = largest(all.iter().copied());
    let med = median(&mut all);
    let after: Vec<&TickRecord> = match plan.settle_after_s {
        Some(after_s) => ticks
            .iter()
            .filter(|t| t.time_s >= after_s - 1e-9 && t.miss.is_none() && at(t, top))
            .collect(),
        None => Vec::new(),
    };
    let settled = largest(
        after
            .iter()
            .flat_map(|t| t.errors_db().iter().map(|e| e.abs()).collect::<Vec<_>>()),
    );
    let rungs = ladder
        .iter()
        .map(|&l| {
            let w = largest(
                ticks
                    .iter()
                    .filter(|t| !t.settling && t.miss.is_none() && at(t, l))
                    .flat_map(|t| t.errors_db().iter().map(|e| e.abs()).collect::<Vec<_>>()),
            );
            (l, w)
        })
        .collect();
    let nl = s.outputs - s.controls;
    let mut limit_over = f64::NEG_INFINITY;
    if plan.limit_bound_g.is_some() {
        for t in ticks {
            for j in 0..nl {
                if t.limit_bound[j] > 0.0 {
                    limit_over = limit_over.max(db(t.limit[j].norm() / t.limit_bound[j]));
                }
            }
        }
    }
    let notch_engaged = events
        .iter()
        .any(|e| matches!(e.kind, EventKind::NotchIn { .. }));
    let deepest = ticks.iter().map(|t| t.notch_depth).fold(1.0f64, f64::min);
    let mut release = f64::NEG_INFINITY;
    let mut release_by = vec![f64::NEG_INFINITY; n];
    for e in events.iter().filter(|e| e.kind == EventKind::NotchOut) {
        for t in ticks.iter().filter(|t| {
            t.time_s >= e.time_s
                && t.time_s <= e.time_s + CHECK_RELEASE_WATCH_S
                && !t.settling
                && t.miss.is_none()
        }) {
            for (k, &x) in t.errors_db().iter().enumerate() {
                release = release.max(x);
                release_by[k] = release_by[k].max(x);
            }
        }
    }
    let largest_drive = ticks
        .iter()
        .flat_map(|t| t.drives().iter().map(|d| d.norm()).collect::<Vec<_>>())
        .fold(0.0f64, f64::max);
    let stop_hz = events
        .iter()
        .find(|e| matches!(e.kind, EventKind::Stopping { .. }))
        .map(|e| e.frequency_hz);
    CaseMeasure {
        case: None,
        outcome,
        stop_hz,
        misses: ticks.iter().filter(|t| t.miss.is_some()).count(),
        over_ceiling: st.over_ceiling,
        largest_sample_v: st.largest_sample_v,
        largest_drive_db: db(largest_drive / CHECK_CEILING_V),
        drive_is_zero: st.drive_is_zero,
        judged: judged.len(),
        worst_db: worst,
        median_db: med,
        per_control,
        settled_db: settled,
        settled_updates: after.len(),
        through_db: through,
        away_db: away,
        rungs,
        limit_over_db: limit_over,
        notch_engaged,
        deepest_notch_db: db(deepest),
        release_overshoot_db: release,
        release_overshoot_by_control_db: release_by,
        notch_entries: events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::NotchIn { .. }))
            .count(),
        opposition_db: opposition,
        ident_s,
        build_s,
        run_s,
    }
}

/// The judgement of one case.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseVerdict {
    /// The case.
    pub case: MimoSineCheckCase,
    /// Whether the case decides the check.
    pub scored: bool,
    /// Whether every requirement held.
    pub passed: bool,
    /// The first requirement that failed, by name, when one did.
    pub failed: Option<String>,
    /// The number that decided it: for a pass, the case requirement's own
    /// measure; for a failure, the failing requirement's.
    pub number: f64,
    /// What was measured, in words.
    pub detail: String,
}

fn outcome_str(o: Option<Outcome>) -> String {
    match o {
        Some(o) => o.as_str().to_string(),
        None => "not ended".into(),
    }
}

/// Judge a case's measure against the check's requirements.
pub fn judge(m: &CaseMeasure) -> CaseVerdict {
    let Some(case) = m.case else {
        return CaseVerdict {
            case: MimoSineCheckCase::DwellAtResonance,
            scored: false,
            passed: false,
            failed: Some("ran".into()),
            number: f64::NAN,
            detail: "the run was not a check case".into(),
        };
    };
    let req = case.requirement();
    let v = |failed: Option<&str>, number: f64, detail: String| CaseVerdict {
        case,
        scored: case.scored(),
        passed: failed.is_none(),
        failed: failed.map(str::to_string),
        number,
        detail,
    };
    let ceiling = CHECK_CEILING_V + 1e-6;
    let at = m
        .stop_hz
        .map(|f| format!(" at {f:.1} Hz"))
        .unwrap_or_default();
    if req == Requirement::StopsOnFault {
        if m.largest_sample_v > ceiling {
            return v(
                Some("within_ceiling"),
                m.largest_sample_v,
                format!("a drive sample reached {:.3} V", m.largest_sample_v),
            );
        }
        let misses = hertzforge_mimo_sine::limits::DEFAULT_LAW_MISSES as usize;
        let held = m.outcome == Some(Outcome::LawFault) && m.misses == misses && m.drive_is_zero;
        let detail = format!(
            "ended {} after {} refused answers; every drive at zero: {}",
            outcome_str(m.outcome),
            m.misses,
            m.drive_is_zero
        );
        return v((!held).then_some("stops_on_fault"), m.misses as f64, detail);
    }
    if m.misses > 0 || m.outcome == Some(Outcome::LawFault) {
        return v(
            Some("no_law_fault"),
            m.misses as f64,
            format!(
                "{} refused answers; the run ended {}{at}",
                m.misses,
                outcome_str(m.outcome)
            ),
        );
    }
    let survive = req == Requirement::Survive;
    if m.largest_sample_v > ceiling || (!survive && m.over_ceiling > 0) {
        return v(
            Some("within_ceiling"),
            m.over_ceiling as f64,
            format!(
                "the engine scaled {} answers to the ceilings; largest sample {:.3} V",
                m.over_ceiling, m.largest_sample_v
            ),
        );
    }
    if survive {
        let ok = matches!(
            m.outcome,
            Some(Outcome::Completed | Outcome::ControlAbort { .. } | Outcome::Divergence)
        );
        return v(
            (!ok).then_some("survive"),
            m.largest_sample_v,
            format!(
                "ended {}{at}; largest sample {:.3} V",
                outcome_str(m.outcome),
                m.largest_sample_v
            ),
        );
    }
    match m.outcome {
        Some(Outcome::Completed) => {}
        Some(
            o @ (Outcome::ControlAbort { .. }
            | Outcome::LimitAbort { .. }
            | Outcome::DriveAbort { .. }
            | Outcome::Kinematic { .. }
            | Outcome::Divergence),
        ) => {
            return v(
                Some("no_abort"),
                m.stop_hz.unwrap_or(f64::NAN),
                format!("the run ended {}{at}", o.as_str()),
            );
        }
        o => {
            return v(
                Some("completes"),
                f64::NAN,
                format!("the run ended {}{at}", outcome_str(o)),
            )
        }
    }
    let name = req.as_str();
    let inside = |x: f64, lim: f64| x.is_finite() && x <= lim;
    match req {
        Requirement::Settles { db, after_s } => {
            let x = m.settled_db;
            v(
                (!inside(x, db)).then_some(name),
                x,
                format!(
                    "worst error from {after_s} s after the engage {x:.2} dB over {} updates \
                     (asked {db})",
                    m.settled_updates
                ),
            )
        }
        Requirement::Within { db } => {
            let x = m.worst_db;
            v(
                (!inside(x, db)).then_some(name),
                x,
                format!(
                    "worst judged error {x:.2} dB over {} updates (asked {db})",
                    m.judged
                ),
            )
        }
        Requirement::SweepWithin { db, median_db }
        | Requirement::Reachable { db, median_db }
        | Requirement::WithinEffort { db, median_db, .. } => {
            let (x, md) = (m.worst_db, m.median_db);
            let against = if matches!(req, Requirement::Reachable { .. }) {
                " against the reachable response"
            } else {
                ""
            };
            let detail = format!(
                "worst judged error{against} {x:.2} dB, median {md:.2} dB \
                 (asked {db} and {median_db})"
            );
            if !inside(x, db) {
                return v(Some(name), x, detail);
            }
            if !inside(md, median_db) {
                return v(Some(name), md, detail);
            }
            if let Requirement::WithinEffort { opposition_db, .. } = req {
                let o = m.opposition_db;
                let detail =
                    format!("{detail}; opposition ratio {o:.2} dB (asked {opposition_db})");
                if !inside(o, opposition_db) {
                    return v(Some(name), o, detail);
                }
                return v(None, x, detail);
            }
            v(None, x, detail)
        }
        Requirement::WithinAway {
            through_db,
            away_db,
            ..
        } => {
            let (t, a) = (m.through_db, m.away_db);
            let detail = format!(
                "worst {t:.2} dB through the resonance (asked {through_db}), \
                 {a:.2} dB away from it (asked {away_db})"
            );
            if !inside(t, through_db) {
                return v(Some(name), t, detail);
            }
            if !inside(a, away_db) {
                return v(Some(name), a, detail);
            }
            v(None, t, detail)
        }
        Requirement::EachRungWithin { db } => {
            let worst = largest(m.rungs.iter().map(|r| r.1));
            let per = m
                .rungs
                .iter()
                .map(|(l, w)| format!("{l:.0} dB: {w:.2}"))
                .collect::<Vec<_>>()
                .join(", ");
            let bad = m.rungs.iter().any(|r| !inside(r.1, db));
            v(
                bad.then_some(name),
                worst,
                format!("worst judged error by rung {per} (asked {db})"),
            )
        }
        Requirement::LimitHeld {
            over_bound_db,
            overshoot_db,
        } => {
            let (l, r) = (m.limit_over_db, m.release_overshoot_db);
            let detail = format!(
                "limit {l:.2} dB over its bound (asked {over_bound_db}), deepest notch \
                 {:.1} dB, in {} times, control over its reference after release {r:.2} dB \
                 (asked {overshoot_db})",
                m.deepest_notch_db, m.notch_entries
            );
            if !m.notch_engaged {
                return v(Some(name), l, format!("the notch never engaged; {detail}"));
            }
            if l > over_bound_db {
                return v(Some(name), l, detail);
            }
            if r > overshoot_db {
                return v(Some(name), r, detail);
            }
            v(None, l, detail)
        }
        Requirement::Survive | Requirement::StopsOnFault => unreachable!("judged above"),
    }
}

/// One case of a law's check: its verdict and, when it ran, its measure.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseResult {
    /// The verdict.
    pub verdict: CaseVerdict,
    /// What the run measured; `None` when the case could not be run.
    pub measure: Option<CaseMeasure>,
}

/// Run every case `factory`'s manifest accepts, each on the station
/// `station` builds for it, and judge each. A case that could not be run is
/// a failure named `ran`.
pub fn check_law(
    factory: Arc<dyn MimoSineLawFactory>,
    mut station: impl FnMut(&StationSpec) -> Result<Box<dyn Backend>, String>,
) -> Vec<CaseResult> {
    cases_for(&factory.manifest())
        .into_iter()
        .map(|case| check_case(case, factory.clone(), &mut station))
        .collect()
}

/// What builds a case's station: the simulator's, or another a caller holds.
pub type StationBuilder<'a> = dyn FnMut(&StationSpec) -> Result<Box<dyn Backend>, String> + 'a;

/// Run one `case` of `factory`'s law as [`check_law`] runs it (its run for the
/// law's name, [`MimoSineCheckCase::run_plan_for`]), on the station `station`
/// builds for it, and judge it. A caller that reports each case as it goes
/// runs the cases of [`cases_for`] through this.
pub fn check_case(
    case: MimoSineCheckCase,
    factory: Arc<dyn MimoSineLawFactory>,
    station: &mut StationBuilder<'_>,
) -> CaseResult {
    let plan = case.run_plan_for(&factory.describe().name);
    let ran = station(&plan.station)
        .map_err(CheckRunError::Setup)
        .and_then(|b| {
            let mut m = run_plan(&plan, factory.clone(), b)?;
            m.case = Some(case);
            Ok(m)
        });
    match ran {
        Ok(m) => CaseResult {
            verdict: judge(&m),
            measure: Some(m),
        },
        Err(e) => CaseResult {
            verdict: CaseVerdict {
                case,
                scored: case.scored(),
                passed: false,
                failed: Some("ran".into()),
                number: f64::NAN,
                detail: e.to_string(),
            },
            measure: None,
        },
    }
}

/// Whether every scored verdict passed.
pub fn check_passed(verdicts: &[CaseVerdict]) -> bool {
    verdicts.iter().filter(|v| v.scored).all(|v| v.passed)
}

/// The check record of the scored `verdicts` for the law file `law_sha256`,
/// dated `date` (RFC 3339). The reported cases are not in it.
pub fn check_record(law_sha256: &str, date: &str, verdicts: &[CaseVerdict]) -> CheckRecord {
    CheckRecord::new(
        law_sha256,
        MIMO_SINE_CHECK_VERSION,
        date,
        verdicts
            .iter()
            .filter(|v| v.scored)
            .map(|v| CheckCase {
                name: v.case.name().to_string(),
                passed: v.passed,
                detail: match &v.failed {
                    None => v.detail.clone(),
                    Some(rule) => format!("{rule}: {}", v.detail),
                },
            })
            .collect(),
    )
}

/// The simulated station of `spec`.
#[cfg(feature = "simulator")]
pub fn sim_station(spec: &StationSpec) -> Result<Box<dyn Backend>, String> {
    use hertzforge_hal_sim::{SimBackend, SimConfig};
    let b: SimBackend = SimConfig::new()
        .sample_rate_hz(spec.fs_hz)
        .plant(sim_structure(spec)?)
        .noise_level(spec.noise_g)
        .seed(spec.seed)
        .shared_clock(true)
        .build();
    Ok(Box::new(b))
}

/// The simulator's model of `spec`'s structure, edited as it says.
#[cfg(feature = "simulator")]
pub fn sim_structure(spec: &StationSpec) -> Result<hertzforge_hal_sim::PlantModel, String> {
    use hertzforge_hal_sim::plants::SineStructure;
    use hertzforge_hal_sim::PlantModel;
    let s = SineStructure::from_name(spec.structure)
        .ok_or_else(|| format!("no standard sine structure {}", spec.structure))?;
    let mut model = s.build().model;
    if spec.edit != StructureEdit::None || spec.transport_delay_s > 0.0 {
        let PlantModel::Jointed(p) = &mut model else {
            return Err(format!("{} is not a modal structure", spec.structure));
        };
        match spec.edit {
            StructureEdit::None => {}
            StructureEdit::MassLineOnly => p.modes.clear(),
            StructureEdit::MoveMode { from_hz, to_hz } => {
                let k = (0..p.modes.len())
                    .min_by(|&a, &b| {
                        (p.modes[a].f_hz - from_hz)
                            .abs()
                            .total_cmp(&(p.modes[b].f_hz - from_hz).abs())
                    })
                    .ok_or("the structure has no mode")?;
                p.modes[k].f_hz = to_hz;
            }
        }
        p.transport_delay_samples = (spec.transport_delay_s * spec.fs_hz).round() as usize;
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_the_check_cases_are_named_once_and_described() {
        let names: Vec<&str> = MimoSineCheckCase::ALL.iter().map(|c| c.name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        for c in MimoSineCheckCase::ALL {
            assert_eq!(MimoSineCheckCase::from_name(c.name()), Some(c));
            assert!(!c.describe().is_empty());
            assert!(c.max_s() > 0.0);
            assert_ne!(c.station().seed, 0);
        }
        let reported: Vec<&str> = MimoSineCheckCase::ALL
            .iter()
            .filter(|c| !c.scored())
            .map(|c| c.name())
            .collect();
        assert_eq!(
            reported,
            [
                "moved_resonance_mild",
                "moved_resonance_moderate",
                "moved_resonance_hardening"
            ]
        );
        // The first version's entry still builds and runs nothing.
        assert_eq!(run_check(), Err(CheckNotBuilt));
    }

    /// A run that meets every requirement of every case.
    fn clean(case: MimoSineCheckCase) -> CaseMeasure {
        CaseMeasure {
            case: Some(case),
            outcome: Some(Outcome::Completed),
            stop_hz: None,
            misses: 0,
            over_ceiling: 0,
            largest_sample_v: 0.2,
            largest_drive_db: -20.0,
            drive_is_zero: true,
            judged: 500,
            worst_db: 0.2,
            median_db: 0.05,
            per_control: Vec::new(),
            settled_db: 0.1,
            settled_updates: 400,
            through_db: 0.3,
            away_db: 0.2,
            rungs: vec![(0.0, 0.2)],
            limit_over_db: 0.1,
            notch_engaged: true,
            deepest_notch_db: -6.0,
            release_overshoot_db: 0.2,
            release_overshoot_by_control_db: vec![0.2],
            notch_entries: 1,
            opposition_db: 0.1,
            ident_s: 0.0,
            build_s: 0.0,
            run_s: 0.0,
        }
    }

    fn failed(m: &CaseMeasure) -> Option<String> {
        judge(m).failed
    }

    #[test]
    fn mimo_sine_check_a_clean_run_passes_every_case() {
        for c in MimoSineCheckCase::ALL {
            let mut m = clean(c);
            if c == MimoSineCheckCase::LawMisbehaves {
                m.outcome = Some(Outcome::LawFault);
                m.misses = 3;
            }
            let v = judge(&m);
            assert!(v.passed, "{}: {v:?}", c.name());
            assert_eq!(v.scored, c.scored());
        }
    }

    #[test]
    fn mimo_sine_check_each_requirement_fails_by_name() {
        use MimoSineCheckCase as C;
        let with = |c: C, f: &dyn Fn(&mut CaseMeasure)| {
            let mut m = clean(c);
            f(&mut m);
            failed(&m)
        };
        let s = |x: &str| Some(x.to_string());
        // Every case.
        assert_eq!(with(C::SlowSweep, &|m| m.misses = 1), s("no_law_fault"));
        assert_eq!(
            with(C::SlowSweep, &|m| m.outcome = Some(Outcome::LawFault)),
            s("no_law_fault")
        );
        assert_eq!(
            with(C::SlowSweep, &|m| m.over_ceiling = 1),
            s("within_ceiling")
        );
        assert_eq!(
            with(C::SlowSweep, &|m| m.largest_sample_v = 2.1),
            s("within_ceiling")
        );
        for o in [
            Outcome::ControlAbort { control: 1 },
            Outcome::LimitAbort { limit: 0 },
            Outcome::DriveAbort { drive: 2 },
            Outcome::Kinematic { control: 0 },
            Outcome::Divergence,
        ] {
            assert_eq!(with(C::SlowSweep, &|m| m.outcome = Some(o)), s("no_abort"));
        }
        assert_eq!(with(C::SlowSweep, &|m| m.outcome = None), s("completes"));
        assert_eq!(
            with(C::SlowSweep, &|m| m.outcome = Some(Outcome::Stopped)),
            s("completes")
        );
        // Each case's own.
        assert_eq!(
            with(C::DwellAtResonance, &|m| m.settled_db = 1.01),
            s("settles")
        );
        assert_eq!(
            with(C::TransportDelay, &|m| m.settled_db = 0.51),
            s("settles")
        );
        assert_eq!(
            with(C::MeasurementNoise, &|m| m.worst_db = 1.51),
            s("within")
        );
        assert_eq!(with(C::Stepped, &|m| m.worst_db = f64::NAN), s("within"));
        assert_eq!(
            with(C::SlowSweep, &|m| m.worst_db = 1.51),
            s("sweep_within")
        );
        assert_eq!(
            with(C::SlowSweep, &|m| m.median_db = 0.51),
            s("sweep_within")
        );
        assert_eq!(
            with(C::FastSweep, &|m| m.through_db = 5.01),
            s("within_away")
        );
        assert_eq!(with(C::FastSweep, &|m| m.away_db = 2.01), s("within_away"));
        assert_eq!(with(C::CloseModes, &|m| m.worst_db = 2.01), s("within"));
        assert_eq!(
            with(C::MoreControlsThanDrives, &|m| m.worst_db = 3.01),
            s("reachable")
        );
        assert_eq!(
            with(C::MoreDrivesThanControls, &|m| m.opposition_db = 6.01),
            s("within_effort")
        );
        assert_eq!(
            with(C::LevelLadder, &|m| m.rungs =
                vec![(-20.0, 1.01), (0.0, 0.1)]),
            s("each_rung_within")
        );
        assert_eq!(
            with(C::LimitChannel, &|m| m.limit_over_db = 0.51),
            s("limit_held")
        );
        assert_eq!(
            with(C::LimitChannel, &|m| m.release_overshoot_db = 1.01),
            s("limit_held")
        );
        assert_eq!(
            with(C::LimitChannel, &|m| m.notch_engaged = false),
            s("limit_held")
        );
        // Surviving: an engine stop and the ceiling's scaling are allowed; a
        // sample over the ceiling, a law fault or another end is not.
        assert_eq!(
            with(C::NearSingular, &|m| {
                m.outcome = Some(Outcome::ControlAbort { control: 0 });
                m.over_ceiling = 40;
            }),
            None
        );
        assert_eq!(
            with(C::NearSingular, &|m| m.outcome = Some(Outcome::Divergence)),
            None
        );
        assert_eq!(
            with(C::NearSingular, &|m| m.largest_sample_v = 2.1),
            s("within_ceiling")
        );
        assert_eq!(
            with(C::NearSingular, &|m| m.outcome =
                Some(Outcome::DriveAbort { drive: 0 })),
            s("survive")
        );
        // The misbehaving law: the run must end on the fault, at once.
        let bad = |f: &dyn Fn(&mut CaseMeasure)| {
            with(C::LawMisbehaves, &|m| {
                m.outcome = Some(Outcome::LawFault);
                m.misses = 3;
                f(m);
            })
        };
        assert_eq!(bad(&|_| {}), None);
        assert_eq!(bad(&|m| m.misses = 4), s("stops_on_fault"));
        assert_eq!(
            bad(&|m| m.outcome = Some(Outcome::Completed)),
            s("stops_on_fault")
        );
        assert_eq!(bad(&|m| m.drive_is_zero = false), s("stops_on_fault"));
        assert_eq!(bad(&|m| m.largest_sample_v = 2.1), s("within_ceiling"));
    }

    #[test]
    fn mimo_sine_check_the_record_holds_the_scored_cases_only() {
        let v: Vec<CaseVerdict> = MimoSineCheckCase::ALL
            .iter()
            .map(|&c| {
                let mut m = clean(c);
                if c == MimoSineCheckCase::LawMisbehaves {
                    m.outcome = Some(Outcome::LawFault);
                    m.misses = 3;
                }
                if !c.scored() {
                    m.worst_db = 9.0;
                }
                judge(&m)
            })
            .collect();
        assert!(check_passed(&v));
        let rec = check_record(&"a".repeat(64), "2026-11-18T00:00:00Z", &v);
        assert!(rec.passed);
        assert_eq!(rec.tool_version, MIMO_SINE_CHECK_VERSION);
        assert_eq!(rec.cases.len(), 14);
        assert!(rec.cases.iter().all(|c| !c.name.starts_with("moved")));
        rec.validate().unwrap();
    }

    #[cfg(feature = "simulator")]
    #[test]
    fn mimo_sine_check_structures_are_the_simulators_and_the_stations_are_edited() {
        use hertzforge_hal_sim::plants::SineStructure;
        use hertzforge_hal_sim::PlantModel;
        assert_eq!(CHECK_STRUCTURES.len(), SineStructure::ALL.len());
        for s in SineStructure::ALL {
            let c = check_structure(s.name()).unwrap();
            assert_eq!((c.outputs, c.drives), s.dims(), "{}", s.name());
            assert_eq!(c.controls, s.n_controls(), "{}", s.name());
            assert_eq!(c.full_level_g, s.full_level_g(), "{}", s.name());
        }
        // The edits reach the structure the station is built on.
        let modes = |c: MimoSineCheckCase| -> (Vec<f64>, usize) {
            match sim_structure(&c.station()).unwrap() {
                PlantModel::Jointed(p) => (
                    p.modes.iter().map(|m| m.f_hz).collect(),
                    p.transport_delay_samples,
                ),
                _ => panic!("a modal structure"),
            }
        };
        let (plain, d) = modes(MimoSineCheckCase::SlowSweep);
        assert_eq!(plain.len(), 8);
        assert_eq!(d, 0);
        let (close, _) = modes(MimoSineCheckCase::CloseModes);
        assert!(close.iter().any(|f| (f - 97.92).abs() < 1e-9));
        assert!(close.iter().any(|f| (f - 96.0).abs() < 1e-9));
        assert!(!close.iter().any(|f| (f - 165.0).abs() < 1e-9));
        let (none, d) = modes(MimoSineCheckCase::TransportDelay);
        assert!(none.is_empty());
        assert_eq!(d, (0.05 * CHECK_FS_HZ).round() as usize);
        assert!(sim_station(&StationSpec {
            structure: "none",
            ..MimoSineCheckCase::SlowSweep.station()
        })
        .is_err());
    }

    #[test]
    fn mimo_sine_check_a_law_runs_only_the_cases_of_its_test_types() {
        let types = |id: &str| {
            let m = hertzforge_mimo_sine::laws::builtin(id).unwrap().manifest();
            cases_for(&m).len()
        };
        assert_eq!(types("matrix"), 17);
        assert_eq!(types("paired"), 7);
        assert_eq!(types("ratio"), 7);
        assert_eq!(types("steepest_descent"), 6);
        assert_eq!(types("secant"), 6);
    }
}
