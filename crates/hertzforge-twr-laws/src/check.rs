//! The check a TWR law file passes on the simulator before it drives
//! hardware: the plant set, what each case asks, what a run is measured by,
//! and the rules that make a measure a passed or failed case.
//!
//! Every case is a whole run under the core's guards on a simulated station at
//! [`TWR_CHECK_FS_HZ`]: a 2.0 s field-like reference (three decaying tones over
//! a multi-sine floor) peaking at [`TWR_CHECK_PEAK_G`] g, quiet pads of 512 and
//! 1024 samples (a record of [`TWR_CHECK_RECORD_LEN`] samples), one drive and
//! one control channel at 1 V per g unless the case says otherwise, the drive
//! ceiling [`TWR_CHECK_DRIVE_CEILING_V`], identification with band-limited
//! random at [`TWR_CHECK_SYSID_DRIVE_VRMS`] V rms for 4 s over 20 to 2000 Hz
//! without the review hold, AUTO through [`TWR_CHECK_LEVELS_DB`] with a 10%
//! gate and at most 8 iterations a rung, a gate failure ending the run, and
//! playback once with adaptation off. Every case asks the same of the law
//! first: every call answered, in time, with zero misses and zero refused
//! outputs. Then each case asks for its [`TwrRequirement`]. The worst call, the
//! clamped bins and the worst peak and bin rise per call are reported with
//! every case.
//!
//! The station a case runs on is built by the controller from these methods
//! alone ([`TwrCheckCase::plant`], [`TwrCheckCase::noise_below_db`], at
//! [`TWR_CHECK_FS_HZ`]); the runner's own run configuration is the authority on
//! everything else, and the controller's station must follow it.

use hertzforge_hal::Backend;
use hertzforge_random_laws::CheckCase;
use hertzforge_twr::TwrLawFactory;
use std::fmt;
use std::sync::Arc;

/// The version of the TWR check written into its records. A change to the
/// plant set or to the rules changes it.
pub const TWR_CHECK_VERSION: &str = "twr-check 1";

/// The rate every case runs at, Hz.
pub const TWR_CHECK_FS_HZ: f64 = 12_800.0;

/// The reference's peak, g.
pub const TWR_CHECK_PEAK_G: f64 = 1.0;

/// Samples in every record of a case: the 2.0 s section between pads of 512
/// and 1024 samples, padded to a power of two.
pub const TWR_CHECK_RECORD_LEN: usize = 32_768;

/// The drive ceiling every case runs under, volts peak.
pub const TWR_CHECK_DRIVE_CEILING_V: f64 = 10.0;

/// The identification's drive, volts rms.
pub const TWR_CHECK_SYSID_DRIVE_VRMS: f64 = 0.125;

/// The ladder every case climbs, dB; the last is full level.
pub const TWR_CHECK_LEVELS_DB: [f64; 5] = [-12.0, -9.0, -6.0, -3.0, 0.0];

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
/// one-drive, one-control simulated station from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TwrCheckPlant {
    /// A wire: feedthrough 1 (1 V per g), no modes.
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
    /// A wire behind the converters' delay, on a station that keeps its output
    /// queue `lookahead_frames` ahead of the input in blocks of `block_frames`,
    /// as a hardware chassis is driven.
    DeepQueue {
        /// The converters' delay, samples.
        delay_samples: usize,
        /// The output lookahead the station states, frames.
        lookahead_frames: u32,
        /// The station's block, frames.
        block_frames: usize,
    },
    /// A deep notch, its coherence under the identification's trust floor.
    Notch {
        /// The notch's frequency, Hz.
        f_hz: f64,
        /// Its depth, dB.
        depth_db: f64,
    },
}

impl fmt::Display for TwrCheckPlant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TwrCheckPlant::Wire => f.write_str("a wire"),
            TwrCheckPlant::OneMode { f_hz, q } => write!(f, "one mode, {f_hz} Hz, Q {q}"),
            TwrCheckPlant::Station { f_hz, q } => {
                write!(f, "two modes, {} and {} Hz, Q {q}", f_hz[0], f_hz[1])
            }
            TwrCheckPlant::DelayedMode { f_hz, q, delay_s } => {
                write!(f, "one mode, {f_hz} Hz, Q {q}, behind {} ms", delay_s * 1e3)
            }
            TwrCheckPlant::DeepQueue {
                delay_samples,
                lookahead_frames,
                block_frames,
            } => write!(
                f,
                "a wire behind {delay_samples} samples, the output {lookahead_frames} frames \
                 ahead in {block_frames}-frame blocks"
            ),
            TwrCheckPlant::Notch { f_hz, depth_db } => {
                write!(f, "a {depth_db} dB notch at {f_hz} Hz")
            }
        }
    }
}

/// What a case asks of the law beyond answering every call, in time, with a
/// usable drive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TwrRequirement {
    /// Every rung gates on its first play, and the full-level error is at most
    /// `full_level_pct`. With `lag_frames`, the run's lag is that many frames,
    /// measured.
    FirstPlay {
        /// The full-level rms error bound, percent.
        full_level_pct: f64,
        /// The lag the run must have aligned with, frames.
        lag_frames: Option<usize>,
    },
    /// Every rung gates within its iteration budget, and the full-level error
    /// is at most `full_level_pct`.
    EveryRung {
        /// The full-level rms error bound, percent.
        full_level_pct: f64,
    },
    /// The first rung gates by play `by_play` (counted from 1), and every later
    /// rung on its first play.
    FirstRungBy {
        /// The play by which the first rung must have gated.
        by_play: u32,
    },
    /// The full-level error is at most `pct`.
    FullLevel {
        /// The full-level rms error bound, percent.
        pct: f64,
    },
    /// The run completes, or is ended by the core's own judgement
    /// (`not_converged`, `divergence`, the trust mask's coverage floor). Never
    /// over the drive ceiling, never an envelope abort.
    Survive,
}

impl TwrRequirement {
    /// The stable record and wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            TwrRequirement::FirstPlay { .. } => "first_play",
            TwrRequirement::EveryRung { .. } => "every_rung",
            TwrRequirement::FirstRungBy { .. } => "first_rung_by",
            TwrRequirement::FullLevel { .. } => "full_level",
            TwrRequirement::Survive => "survive",
        }
    }
}

/// One simulated test article or condition of the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TwrCheckCase {
    /// A wire; every rung gates on its first play.
    Flat,
    /// One mode at 400 Hz, Q 10.
    OneMode,
    /// The simulated station's plant (200 and 550 Hz, Q 2.5).
    StationPlant,
    /// One mode plus a 2 ms transport delay.
    Delay,
    /// A wire behind 41 samples on a station stating a 512-frame output
    /// lookahead at 256-frame blocks (the NI chassis at 12 800 Hz).
    DeepQueue,
    /// A wire, with the core's identified gain scaled +3 dB before inversion:
    /// a law that does not correct sits 29% low and cannot pass.
    IdentificationError,
    /// One mode with measurement noise 30 dB under the response.
    Noise,
    /// A 30 dB notch at 300 Hz with its coherence under the floor.
    AntiResonance,
}

impl TwrCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [TwrCheckCase; 8] = [
        TwrCheckCase::Flat,
        TwrCheckCase::OneMode,
        TwrCheckCase::StationPlant,
        TwrCheckCase::Delay,
        TwrCheckCase::DeepQueue,
        TwrCheckCase::IdentificationError,
        TwrCheckCase::Noise,
        TwrCheckCase::AntiResonance,
    ];

    /// The case's name in a record and on screen.
    pub fn name(self) -> &'static str {
        match self {
            TwrCheckCase::Flat => "flat",
            TwrCheckCase::OneMode => "one mode",
            TwrCheckCase::StationPlant => "station plant",
            TwrCheckCase::Delay => "transport delay",
            TwrCheckCase::DeepQueue => "deep queue",
            TwrCheckCase::IdentificationError => "identification error",
            TwrCheckCase::Noise => "measurement noise",
            TwrCheckCase::AntiResonance => "anti-resonance",
        }
    }

    /// The simulated test article.
    pub fn plant(self) -> TwrCheckPlant {
        match self {
            TwrCheckCase::Flat | TwrCheckCase::IdentificationError => TwrCheckPlant::Wire,
            TwrCheckCase::OneMode | TwrCheckCase::Noise => TwrCheckPlant::OneMode {
                f_hz: 400.0,
                q: 10.0,
            },
            TwrCheckCase::StationPlant => TwrCheckPlant::Station {
                f_hz: [200.0, 550.0],
                q: 2.5,
            },
            TwrCheckCase::Delay => TwrCheckPlant::DelayedMode {
                f_hz: 400.0,
                q: 10.0,
                delay_s: 0.002,
            },
            TwrCheckCase::DeepQueue => TwrCheckPlant::DeepQueue {
                delay_samples: 41,
                lookahead_frames: 512,
                block_frames: 256,
            },
            TwrCheckCase::AntiResonance => TwrCheckPlant::Notch {
                f_hz: 300.0,
                depth_db: 30.0,
            },
        }
    }

    /// The factor, dB, the core scales its identified gain by before inversion
    /// (`None`: the identification as measured).
    pub fn identification_scale_db(self) -> Option<f64> {
        match self {
            TwrCheckCase::IdentificationError => Some(3.0),
            _ => None,
        }
    }

    /// Measurement noise, dB under the response (`None`: the simulator's floor
    /// only).
    pub fn noise_below_db(self) -> Option<f64> {
        match self {
            TwrCheckCase::Noise => Some(30.0),
            _ => None,
        }
    }

    /// What the case asks of the law.
    pub fn requirement(self) -> TwrRequirement {
        match self {
            TwrCheckCase::Flat => TwrRequirement::FirstPlay {
                full_level_pct: 5.0,
                lag_frames: None,
            },
            TwrCheckCase::OneMode | TwrCheckCase::StationPlant | TwrCheckCase::Delay => {
                TwrRequirement::EveryRung {
                    full_level_pct: 10.0,
                }
            }
            TwrCheckCase::DeepQueue => TwrRequirement::FirstPlay {
                full_level_pct: 5.0,
                lag_frames: Some(41),
            },
            TwrCheckCase::IdentificationError => TwrRequirement::FirstRungBy { by_play: 4 },
            TwrCheckCase::Noise => TwrRequirement::FullLevel { pct: 10.0 },
            TwrCheckCase::AntiResonance => TwrRequirement::Survive,
        }
    }
}

/// How a case's run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TwrCaseEnd {
    /// The schedule completed.
    Completed,
    /// A rung exhausted its iterations without gating.
    NotConverged,
    /// The core's divergence judgement ended it.
    Divergence,
    /// A protection aborted it; the trip kind's wire name (`envelope`,
    /// `drive_vpk`, `alignment`, `coverage`, ...).
    Aborted(String),
    /// The law's misses, or a refused first drive, ended it; the reason's wire
    /// name.
    LawFault(String),
    /// The coherence hold held the run after identification (its coherence,
    /// or a correlation with no peak). The trust mask's coverage floor ends a
    /// run [`TwrCaseEnd::Aborted`] with the trip `coverage`.
    Held,
    /// The run failed; the controller's words.
    Fault(String),
}

/// What one case's run is judged by.
#[derive(Debug, Clone, PartialEq)]
pub struct TwrCaseMeasure {
    /// How the run ended.
    pub end: TwrCaseEnd,
    /// Plays after the identification, iterations and playback.
    pub plays: u32,
    /// Per rung of the ladder, in order: its level, dB, and the plays it took
    /// to gate, counted from 1 (`None` when it did not gate or was not
    /// reached).
    pub rungs: Vec<(f64, Option<u32>)>,
    /// The last full-level play's rms error, percent; `None` when full level
    /// was not played.
    pub full_level_error_pct: Option<f64>,
    /// The lag the run aligned with, frames; `None` before identification.
    pub lag_frames: Option<usize>,
    /// Whether that lag was measured on the identification (not derived).
    pub lag_measured: bool,
    /// Law calls made, the seed included.
    pub calls: u32,
    /// Calls whose output was not used.
    pub misses: u32,
    /// Calls whose output the core refused (a subset of `misses`).
    pub refused: u32,
    /// Drive bins the built-in law's own clamp, or the core's cap, held back
    /// over the run.
    pub clamped_bins: u64,
    /// The largest rise of the drive's peak from one call to the next, dB;
    /// `None` before a second drive.
    pub worst_peak_rise_db: Option<f64>,
    /// The largest rise of any drive bin from one call to the next, dB; `None`
    /// before a second drive.
    pub worst_bin_rise_db: Option<f64>,
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
pub fn run_twr_check_case(
    case: TwrCheckCase,
    factory: Arc<dyn TwrLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<TwrCaseMeasure, CheckRunError> {
    let _ = (factory, backend);
    Err(CheckRunError::Setup(format!(
        "the TWR law check is not built yet (case \"{}\")",
        case.name()
    )))
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
pub fn judge_twr(case: TwrCheckCase, measure: &TwrCaseMeasure) -> CheckCase {
    let _ = measure;
    CheckCase {
        name: case.name().to_string(),
        passed: false,
        detail: "the TWR law check is not built yet".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_channel_table::ChannelTable;
    use hertzforge_hal::{
        AcquisitionConfig, BackendStatus, Caps, ClockDomain, ConfiguredActual, HalError, InputRead,
        LoopOwner,
    };
    use hertzforge_shock_common::law_guard::{LawDescriptor, LawFault};
    use hertzforge_twr::{TwrLaw, TwrLawSetup};

    /// A backend nothing may touch: every call but `caps` fails.
    struct Untouched;

    impl Backend for Untouched {
        fn caps(&self) -> Caps {
            Caps {
                loop_owner: LoopOwner::HostClosesLoop,
                native_modes: vec![],
                clock: ClockDomain::Shared,
                max_input_channels: 1,
                max_output_channels: 1,
                abort_latency_bound: None,
                output_buffer: None,
                sample_rate_hz: TWR_CHECK_FS_HZ,
                delegated_response_observable: false,
                clock_groups: vec![],
            }
        }
        fn open(&mut self) -> Result<(), HalError> {
            Err(HalError::NotConnected)
        }
        fn close(&mut self) -> Result<(), HalError> {
            Err(HalError::NotConnected)
        }
        fn configure(
            &mut self,
            _: &ChannelTable,
            _: &AcquisitionConfig,
        ) -> Result<ConfiguredActual, HalError> {
            Err(HalError::NotConnected)
        }
        fn arm(&mut self) -> Result<(), HalError> {
            Err(HalError::NotConnected)
        }
        fn start(&mut self) -> Result<(), HalError> {
            Err(HalError::NotConnected)
        }
        fn stop(&mut self) -> Result<(), HalError> {
            Err(HalError::NotConnected)
        }
        fn read_input(&mut self, _: &mut [f32]) -> Result<InputRead, HalError> {
            Err(HalError::NotConnected)
        }
        fn write_drive(&mut self, _: &[f32]) -> Result<usize, HalError> {
            Err(HalError::NotConnected)
        }
        fn poll_status(&mut self) -> BackendStatus {
            BackendStatus::default()
        }
        fn emergency_stop(&mut self) -> Result<(), HalError> {
            Err(HalError::NotConnected)
        }
    }

    /// A factory nothing may build from.
    struct Never;

    impl TwrLawFactory for Never {
        fn build(&self, _: &TwrLawSetup<'_>) -> Result<Box<dyn TwrLaw + Send>, LawFault> {
            Err(LawFault::Failed("never built".into()))
        }
        fn describe(&self) -> LawDescriptor {
            LawDescriptor {
                id: "never".into(),
                name: "never".into(),
                version: "0".into(),
                sha256: None,
                params: Default::default(),
            }
        }
    }

    #[test]
    fn every_case_is_listed_once_with_a_unique_name() {
        let mut names: Vec<&str> = TwrCheckCase::ALL.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TwrCheckCase::ALL.len());
        assert_eq!(TWR_CHECK_VERSION, "twr-check 1");
    }

    #[test]
    fn the_record_is_the_section_between_its_pads_padded_to_a_power_of_two() {
        let section = (2.0 * TWR_CHECK_FS_HZ) as usize;
        assert_eq!(
            (section + 512 + 1024).next_power_of_two(),
            TWR_CHECK_RECORD_LEN
        );
        assert_eq!(TWR_CHECK_LEVELS_DB.last(), Some(&0.0));
    }

    #[test]
    fn the_cases_carry_the_plan_plants_and_requirements() {
        use TwrCheckCase as C;
        assert_eq!(C::Flat.plant(), TwrCheckPlant::Wire);
        assert_eq!(
            C::Flat.requirement(),
            TwrRequirement::FirstPlay {
                full_level_pct: 5.0,
                lag_frames: None
            }
        );
        assert_eq!(
            C::OneMode.plant(),
            TwrCheckPlant::OneMode {
                f_hz: 400.0,
                q: 10.0
            }
        );
        assert_eq!(
            C::ALL.map(|c| c.plant().to_string()),
            [
                "a wire",
                "one mode, 400 Hz, Q 10",
                "two modes, 200 and 550 Hz, Q 2.5",
                "one mode, 400 Hz, Q 10, behind 2 ms",
                "a wire behind 41 samples, the output 512 frames ahead in 256-frame blocks",
                "a wire",
                "one mode, 400 Hz, Q 10",
                "a 30 dB notch at 300 Hz",
            ]
        );
        assert!(matches!(
            C::StationPlant.plant(),
            TwrCheckPlant::Station { f_hz, q } if f_hz == [200.0, 550.0] && q == 2.5
        ));
        assert!(matches!(
            C::Delay.plant(),
            TwrCheckPlant::DelayedMode { delay_s, .. } if delay_s == 0.002
        ));
        for c in [C::OneMode, C::StationPlant, C::Delay] {
            assert_eq!(
                c.requirement(),
                TwrRequirement::EveryRung {
                    full_level_pct: 10.0
                }
            );
        }
        assert_eq!(
            C::DeepQueue.plant(),
            TwrCheckPlant::DeepQueue {
                delay_samples: 41,
                lookahead_frames: 512,
                block_frames: 256
            }
        );
        assert_eq!(
            C::DeepQueue.requirement(),
            TwrRequirement::FirstPlay {
                full_level_pct: 5.0,
                lag_frames: Some(41)
            }
        );
        assert_eq!(C::IdentificationError.plant(), TwrCheckPlant::Wire);
        assert_eq!(C::IdentificationError.identification_scale_db(), Some(3.0));
        assert_eq!(
            C::IdentificationError.requirement(),
            TwrRequirement::FirstRungBy { by_play: 4 }
        );
        assert_eq!(C::Noise.noise_below_db(), Some(30.0));
        assert_eq!(
            C::Noise.requirement(),
            TwrRequirement::FullLevel { pct: 10.0 }
        );
        assert!(matches!(
            C::AntiResonance.plant(),
            TwrCheckPlant::Notch { f_hz, depth_db } if f_hz == 300.0 && depth_db == 30.0
        ));
        assert_eq!(C::AntiResonance.requirement(), TwrRequirement::Survive);
        assert!(C::ALL
            .iter()
            .filter(|c| **c != C::IdentificationError)
            .all(|c| c.identification_scale_db().is_none()));
        assert!(C::ALL
            .iter()
            .filter(|c| **c != C::Noise)
            .all(|c| c.noise_below_db().is_none()));
        assert_eq!(
            C::ALL.map(|c| c.requirement().as_str()),
            [
                "first_play",
                "every_rung",
                "every_rung",
                "every_rung",
                "first_play",
                "first_rung_by",
                "full_level",
                "survive"
            ]
        );
    }

    #[test]
    fn no_case_runs_or_passes_until_the_check_is_built() {
        for case in TwrCheckCase::ALL {
            match run_twr_check_case(case, Arc::new(Never), Box::new(Untouched)) {
                Err(CheckRunError::Setup(m)) => {
                    assert!(m.contains("not built yet"), "{m}");
                    assert!(m.contains(case.name()), "{m}");
                }
                Ok(_) => panic!("{}: a case ran before the check was built", case.name()),
            }
        }
        let m = TwrCaseMeasure {
            end: TwrCaseEnd::Completed,
            plays: 6,
            rungs: TWR_CHECK_LEVELS_DB.iter().map(|&l| (l, Some(1))).collect(),
            full_level_error_pct: Some(1.0),
            lag_frames: Some(0),
            lag_measured: true,
            calls: 1,
            misses: 0,
            refused: 0,
            clamped_bins: 0,
            worst_peak_rise_db: None,
            worst_bin_rise_db: None,
            worst_call_ms: Some(1.0),
            budget_ms: 1000.0,
            max_drive_vpk: 1.0,
            drive_ceiling_vpk: TWR_CHECK_DRIVE_CEILING_V,
        };
        for case in TwrCheckCase::ALL {
            let judged = judge_twr(case, &m);
            assert!(
                !judged.passed,
                "{} passed before the check was built",
                case.name()
            );
            assert_eq!(judged.name, case.name());
        }
    }
}
