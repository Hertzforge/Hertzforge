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

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable};
use hertzforge_hal::Backend;
use hertzforge_random_laws::{CheckCase, CheckRecord};
use hertzforge_twr::player::LagSource;
use hertzforge_twr::schedule::RunSchedule;
use hertzforge_twr::service::config::TwrConfig;
use hertzforge_twr::{
    BackendReferencePlayer, ReferenceWaveform, TripKind, TwrControl, TwrLawFactory, TwrService,
    TwrState,
};
use std::fmt;
use std::sync::Arc;

/// The version of the TWR check written into its records, the arming rule's
/// own ([`hertzforge_random_laws::TWR_CHECK_VERSION`]). A change to the plant
/// set or to the rules changes it.
pub use hertzforge_random_laws::TWR_CHECK_VERSION;

/// The noise every case's station adds to its input, g rms, unless the case
/// asks for more.
pub const TWR_CHECK_NOISE_FLOOR_G: f64 = 1e-4;

/// The broadband gain under the modal cases' modes (the controller's simulated
/// station's own), g per volt.
pub const TWR_CHECK_FEEDTHROUGH: f64 = 0.35;

/// The identification's Tikhonov floor in every case: a fraction of the
/// in-band peak gain. A plant thirty times hotter at its mode than between
/// modes needs a floor under its smallest gain, or the inverse is regularized
/// there and a law corrects for many plays.
pub const TWR_CHECK_TIKHONOV_FLOOR: f64 = 0.01;

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
    /// One mode with measurement noise 30 dB under the first rung's response.
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

    /// Measurement noise, dB under the response at the ladder's first rung
    /// (`None`: the simulator's floor only). The first rung, because the noise
    /// does not fall with the level and every rung must be able to gate.
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

/// The simulated station's plant for a case.
#[derive(Debug, Clone, PartialEq)]
pub enum TwrCheckStation {
    /// The simulator's resonator bank: one drive, the control channel's
    /// response `feedthrough` plus each `(f_n Hz, Q)` mode's constant-peak
    /// bandpass (its gain `Q` at `f_n`).
    Resonators {
        /// The modes.
        modes: Vec<(f64, f64)>,
        /// The broadband gain under them.
        feedthrough: f64,
    },
    /// The simulator's FRF plant: one complex gain `(re, im)` per line, line
    /// `k` at `k * df_hz` from 0 Hz to half the rate, played through
    /// partitions of `block_frames` (which is also the latency it adds).
    Frf {
        /// Line spacing, Hz.
        df_hz: f64,
        /// The control channel's response per drive volt, per line.
        lines: Vec<(f64, f64)>,
        /// The FRF plant's partition length.
        block_frames: usize,
    },
}

/// Everything the station a case runs on is built from, at
/// [`TWR_CHECK_FS_HZ`]: the controller and this crate's tests build it from
/// these facts alone.
#[derive(Debug, Clone, PartialEq)]
pub struct TwrCheckStationSpec {
    /// The plant.
    pub station: TwrCheckStation,
    /// The noise on the control channel, g rms.
    pub noise_rms_g: f64,
    /// The drive-to-response delay the station plays, whole samples.
    pub transport_delay_samples: usize,
    /// The output lookahead the station states in its caps, frames (`None`:
    /// the simulator's own).
    pub output_lookahead_frames: Option<u32>,
    /// The block the case's player requests, frames (`None`: the default for
    /// the rate).
    pub block_frames: Option<usize>,
}

/// The cases' FRF plants' line spacing, Hz.
const TWR_CHECK_FRF_DF_HZ: f64 = 1.0;
/// The anti-resonance's pole Q; its zero's is this times the depth.
const NOTCH_POLE_Q: f64 = 2.0;

/// The station `case` runs on.
pub fn twr_check_station(case: TwrCheckCase) -> TwrCheckStationSpec {
    let resonators = |modes: Vec<(f64, f64)>, feedthrough: f64| TwrCheckStation::Resonators {
        modes,
        feedthrough,
    };
    let mut spec = TwrCheckStationSpec {
        station: resonators(Vec::new(), 1.0),
        noise_rms_g: TWR_CHECK_NOISE_FLOOR_G,
        transport_delay_samples: 0,
        output_lookahead_frames: None,
        block_frames: None,
    };
    match case.plant() {
        TwrCheckPlant::Wire => {}
        TwrCheckPlant::OneMode { f_hz, q } => {
            spec.station = resonators(vec![(f_hz, q)], TWR_CHECK_FEEDTHROUGH);
        }
        TwrCheckPlant::Station { f_hz, q } => {
            spec.station = resonators(vec![(f_hz[0], q), (f_hz[1], q)], TWR_CHECK_FEEDTHROUGH);
        }
        TwrCheckPlant::DelayedMode { f_hz, q, delay_s } => {
            spec.station = resonators(vec![(f_hz, q)], TWR_CHECK_FEEDTHROUGH);
            spec.transport_delay_samples = (delay_s * TWR_CHECK_FS_HZ).round() as usize;
        }
        TwrCheckPlant::DeepQueue {
            delay_samples,
            lookahead_frames,
            block_frames,
        } => {
            spec.transport_delay_samples = delay_samples;
            spec.output_lookahead_frames = Some(lookahead_frames);
            spec.block_frames = Some(block_frames);
        }
        TwrCheckPlant::Notch { f_hz, depth_db } => {
            // A biquad notch: unity away from it, `pole Q / zero Q` at it.
            let qz = NOTCH_POLE_Q * 10f64.powf(depth_db / 20.0);
            let n_lines = (TWR_CHECK_FS_HZ / 2.0 / TWR_CHECK_FRF_DF_HZ).floor() as usize + 1;
            let lines = (0..n_lines)
                .map(|k| {
                    let f = k as f64 * TWR_CHECK_FRF_DF_HZ;
                    let re = f_hz * f_hz - f * f;
                    let (nr, ni) = (re, f * f_hz / qz);
                    let (dr, di) = (re, f * f_hz / NOTCH_POLE_Q);
                    let d = dr * dr + di * di;
                    if d == 0.0 {
                        (1.0, 0.0)
                    } else {
                        ((nr * dr + ni * di) / d, (ni * dr - nr * di) / d)
                    }
                })
                .collect();
            spec.station = TwrCheckStation::Frf {
                df_hz: TWR_CHECK_FRF_DF_HZ,
                lines,
                block_frames: 256,
            };
            // The notch's own response sits under the noise there, so the
            // identification's coherence falls under its floor at the notch.
            spec.noise_rms_g = TWR_CHECK_SYSID_DRIVE_VRMS * 10f64.powf(-depth_db / 20.0);
        }
    }
    if let Some(db) = case.noise_below_db() {
        // Under the response at the ladder's first rung: a rung must be able to
        // gate, and the noise does not fall with the level.
        let first_rung = TWR_CHECK_LEVELS_DB[0];
        spec.noise_rms_g =
            section_rms(&twr_check_reference().samples) * 10f64.powf((first_rung - db) / 20.0);
    }
    spec
}

fn section_rms(x: &[f64]) -> f64 {
    (x.iter().map(|v| v * v).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// The check's reference: a 2.0 s field-like transient at [`TWR_CHECK_FS_HZ`]
/// (three decaying tones over a scattered multi-sine floor), peaking at
/// [`TWR_CHECK_PEAK_G`] g.
pub fn twr_check_reference() -> ReferenceWaveform {
    let fs = TWR_CHECK_FS_HZ;
    let n = (2.0 * fs) as usize;
    let mut s = vec![0.0f64; n];
    for (f, a, tau, ph) in [
        (160.0, 1.00, 0.18, 0.3),
        (400.0, 0.65, 0.10, 1.7),
        (750.0, 0.35, 0.06, 4.1),
    ] {
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f64 / fs;
            *v += a * (-t / tau).exp() * (std::f64::consts::TAU * f * t + ph).sin();
        }
    }
    for k in 0..15 {
        let f = 60.0 + 70.0 * k as f64;
        let ph = (k as f64) * 2.399963;
        for (i, v) in s.iter_mut().enumerate() {
            let t = i as f64 / fs;
            *v += 0.04 * (std::f64::consts::TAU * f * t + ph).sin();
        }
    }
    let peak = s.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    for v in s.iter_mut() {
        *v *= TWR_CHECK_PEAK_G / peak;
    }
    ReferenceWaveform::from_samples("check transient", fs, s).expect("a finite record")
}

/// The run every case runs, the authority on everything the case's constants
/// and station do not say: prep (a 20 Hz high-pass, a 5% taper), pads 512 and
/// 1024, identification with band-limited random at
/// [`TWR_CHECK_SYSID_DRIVE_VRMS`] V rms for 4 s over 20 to 2000 Hz (nfft
/// 2048), the inverse's Tikhonov floor [`TWR_CHECK_TIKHONOV_FLOOR`] and
/// coherence floor 0.9, the correction band 20 to 2000 Hz with a 50 dB
/// reference floor and a 0.9 coverage floor, AUTO through
/// [`TWR_CHECK_LEVELS_DB`] with a 10% gate and at most 8 iterations a rung, a
/// gate failure ending the run, playback once with adaptation off, the drive
/// ceiling [`TWR_CHECK_DRIVE_CEILING_V`], the core's guards at their defaults.
pub fn twr_check_config() -> TwrConfig {
    let mut c = TwrConfig::for_reference(twr_check_reference());
    c.prep.highpass_hz = Some(20.0);
    c.prep.taper_frac = Some(0.05);
    c.pad_pre = 512;
    c.pad_post = 1024;
    c.sysid.drive_rms = TWR_CHECK_SYSID_DRIVE_VRMS;
    c.sysid.duration_s = 4.0;
    c.sysid.nfft = 2048;
    c.sysid.f_lo_hz = 20.0;
    c.sysid.f_hi_hz = 2000.0;
    c.inverse.tikhonov_beta_floor = TWR_CHECK_TIKHONOV_FLOOR;
    c.inverse.coherence_floor = 0.9;
    c.law.f_lo_hz = 20.0;
    c.law.f_hi_hz = 2000.0;
    c.law.ref_floor_db = 50.0;
    c.law.min_masked_energy_frac = 0.9;
    c.schedule.table_db = TWR_CHECK_LEVELS_DB.to_vec();
    c.schedule.gate_rms_error_pct = 10.0;
    c.schedule.max_iterations_per_level = 8;
    c.run = RunSchedule::Once;
    c.adapt_during_playback = false;
    c.limits.max_drive_vpk = Some(TWR_CHECK_DRIVE_CEILING_V);
    c.control = Some(TwrControl::default());
    c
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
/// `backend` (the simulated station the controller built for the case from
/// [`twr_check_station`]), and measure it.
pub fn run_twr_check_case(
    case: TwrCheckCase,
    factory: Arc<dyn TwrLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<TwrCaseMeasure, CheckRunError> {
    let setup = |e: hertzforge_twr::TwrError| CheckRunError::Setup(e.to_string());
    let spec = twr_check_station(case);
    let mut table = ChannelTable::new();
    let mut control = Channel::new(1);
    control.role = ChannelRole::Control;
    table.channels.push(control);
    let mut drive = Channel::new(2);
    drive.role = ChannelRole::Drive;
    table.channels.push(drive);
    let mut player = BackendReferencePlayer::new(
        backend,
        table,
        hertzforge_module_abi::StationClass::HertzforgeSim,
    )
    .map_err(setup)?;
    if let Some(b) = spec.block_frames {
        player = player.with_block_frames(b);
    }
    let cfg = twr_check_config();
    let ceiling = cfg
        .limits
        .max_drive_vpk
        .unwrap_or(TWR_CHECK_DRIVE_CEILING_V);
    let record_s = {
        let n = (2.0 * TWR_CHECK_FS_HZ) as usize + cfg.pad_pre + cfg.pad_post;
        n.next_power_of_two() as f64 / TWR_CHECK_FS_HZ
    };
    let budget_ms = TwrControl::default().call_budget_s(record_s) * 1e3;
    let mut svc = TwrService::new(player, cfg)
        .map_err(setup)?
        .with_law_factory(factory);
    if let Some(db) = case.identification_scale_db() {
        svc = svc.with_identification_scale_db(db);
    }
    let mut fault = None;
    for _ in 0..400 {
        match svc.state() {
            TwrState::Review => {
                if let Err(e) = svc.proceed() {
                    fault = Some(e.to_string());
                    break;
                }
            }
            TwrState::Complete
            | TwrState::Aborted
            | TwrState::Stopped
            | TwrState::Holding
            | TwrState::CoherenceHold => break,
            _ => {
                if let Err(e) = svc.advance() {
                    if svc.state() != TwrState::CoherenceHold {
                        fault = Some(e.to_string());
                    }
                    break;
                }
            }
        }
    }
    Ok(measure(&svc, fault, budget_ms, ceiling))
}

fn measure<P: hertzforge_twr::ReferencePlayer>(
    svc: &TwrService<P>,
    fault: Option<String>,
    budget_ms: f64,
    ceiling: f64,
) -> TwrCaseMeasure {
    let end = match (fault, svc.state(), svc.trip().map(|t| &t.kind)) {
        (Some(f), _, _) => TwrCaseEnd::Fault(f),
        (None, TwrState::Complete, _) => TwrCaseEnd::Completed,
        (None, TwrState::CoherenceHold, _) => TwrCaseEnd::Held,
        (None, TwrState::Holding, _) => TwrCaseEnd::NotConverged,
        (None, TwrState::Aborted, Some(TripKind::NotConverged { .. })) => TwrCaseEnd::NotConverged,
        (None, TwrState::Aborted, Some(TripKind::Divergence { .. })) => TwrCaseEnd::Divergence,
        (None, TwrState::Aborted, Some(TripKind::LawFault { reason, .. })) => {
            TwrCaseEnd::LawFault(reason.reason().to_string())
        }
        (None, TwrState::Aborted, Some(TripKind::FirstDrive { .. })) => {
            TwrCaseEnd::LawFault("first_drive".into())
        }
        (None, TwrState::Aborted, Some(k)) => TwrCaseEnd::Aborted(k.as_str().to_string()),
        (None, s, _) => TwrCaseEnd::Fault(format!("the run stopped in state {s:?}")),
    };
    let rows = svc.plays();
    let table = &svc.config().schedule.table_db;
    let rungs = (0..table.len())
        .map(|i| {
            let at: Vec<_> = rows
                .iter()
                .filter(|r| r.iteration.is_some() && r.rung == Some(i))
                .collect();
            let gated = at.iter().find(|r| r.gated).and_then(|r| r.iteration);
            (table[i], gated)
        })
        .collect();
    let full_level_error_pct = rows
        .iter()
        .rev()
        .find(|r| r.level_db == table.last().copied().unwrap_or(0.0))
        .map(|r| r.metrics.rms_error_pct);
    let law = svc.report().law;
    let worst = |f: fn(&hertzforge_twr::PlayLawCall) -> Option<f64>| {
        rows.iter()
            .filter_map(|r| r.law.as_ref().filter(|c| c.used).and_then(f))
            .filter(|v| v.is_finite())
            .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
    };
    TwrCaseMeasure {
        end,
        plays: rows.len() as u32,
        rungs,
        full_level_error_pct,
        lag_frames: svc.lag_frames(),
        lag_measured: svc.lag_provenance() == Some(LagSource::Measured),
        calls: law.as_ref().map_or(0, |l| l.calls as u32),
        misses: law.as_ref().map_or(0, |l| l.misses as u32),
        refused: law.as_ref().map_or(0, |l| l.refused as u32),
        clamped_bins: rows.iter().map(|r| r.clamped_bins as u64).sum(),
        worst_peak_rise_db: worst(|c| c.peak_rise_db),
        worst_bin_rise_db: worst(|c| c.bin_rise_db),
        worst_call_ms: law
            .as_ref()
            .filter(|l| l.calls > 0)
            .map(|l| l.worst_call_ms),
        budget_ms,
        max_drive_vpk: rows.iter().fold(0.0f64, |m, r| m.max(r.drive_vpk)),
        drive_ceiling_vpk: ceiling,
    }
}

fn end_words(end: &TwrCaseEnd) -> String {
    match end {
        TwrCaseEnd::Completed => "completed".into(),
        TwrCaseEnd::NotConverged => "not converged".into(),
        TwrCaseEnd::Divergence => "ended on divergence".into(),
        TwrCaseEnd::Aborted(k) => format!("aborted ({k})"),
        TwrCaseEnd::LawFault(r) => format!("ended on a law fault ({r})"),
        TwrCaseEnd::Held => "held at the coherence hold".into(),
        TwrCaseEnd::Fault(f) => format!("failed: {f}"),
    }
}

fn rung_words(m: &TwrCaseMeasure) -> String {
    let each: Vec<String> = m
        .rungs
        .iter()
        .map(|(db, p)| match p {
            Some(p) => format!("{db} dB on play {p}"),
            None => format!("{db} dB not gated"),
        })
        .collect();
    each.join(", ")
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
///
/// Every case asks first that every call was answered in time with a usable
/// drive: no misses, no refused outputs, no law fault. Then the case's
/// [`TwrRequirement`]; [`TwrRequirement::Survive`] also asks that the drive
/// never passed the ceiling and the envelope never aborted.
pub fn judge_twr(case: TwrCheckCase, measure: &TwrCaseMeasure) -> CheckCase {
    let m = measure;
    let mut why: Vec<String> = Vec::new();
    if m.misses > 0 || m.refused > 0 {
        why.push(format!(
            "{} of {} calls not used ({} refused)",
            m.misses, m.calls, m.refused
        ));
    }
    let completed = m.end == TwrCaseEnd::Completed;
    let full = m.full_level_error_pct;
    let full_within = |pct: f64| full.is_some_and(|e| e <= pct);
    match case.requirement() {
        TwrRequirement::FirstPlay {
            full_level_pct,
            lag_frames,
        } => {
            if !completed {
                why.push(end_words(&m.end));
            }
            if !m.rungs.iter().all(|(_, p)| *p == Some(1)) {
                why.push(format!(
                    "not every rung on its first play ({})",
                    rung_words(m)
                ));
            }
            if !full_within(full_level_pct) {
                why.push(format!("full level over {full_level_pct}%"));
            }
            if let Some(lag) = lag_frames {
                if m.lag_frames != Some(lag) || !m.lag_measured {
                    why.push(format!(
                        "aligned at {:?} frames ({}), not the measured {lag}",
                        m.lag_frames,
                        if m.lag_measured {
                            "measured"
                        } else {
                            "derived"
                        }
                    ));
                }
            }
        }
        TwrRequirement::EveryRung { full_level_pct } => {
            if !completed {
                why.push(end_words(&m.end));
            }
            if !m.rungs.iter().all(|(_, p)| p.is_some()) {
                why.push(format!("a rung did not gate ({})", rung_words(m)));
            }
            if !full_within(full_level_pct) {
                why.push(format!("full level over {full_level_pct}%"));
            }
        }
        TwrRequirement::FirstRungBy { by_play } => {
            if !completed {
                why.push(end_words(&m.end));
            }
            let first = m.rungs.first().and_then(|(_, p)| *p);
            if first.is_none_or(|p| p > by_play) {
                why.push(format!("the first rung did not gate by play {by_play}"));
            }
            if !m.rungs.iter().skip(1).all(|(_, p)| *p == Some(1)) {
                why.push(format!(
                    "a later rung not on its first play ({})",
                    rung_words(m)
                ));
            }
        }
        TwrRequirement::FullLevel { pct } => {
            if !completed {
                why.push(end_words(&m.end));
            }
            if !full_within(pct) {
                why.push(format!("full level over {pct}%"));
            }
        }
        TwrRequirement::Survive => {
            let own = matches!(
                &m.end,
                TwrCaseEnd::Completed | TwrCaseEnd::NotConverged | TwrCaseEnd::Divergence
            ) || m.end == TwrCaseEnd::Aborted("coverage".into());
            if !own {
                why.push(end_words(&m.end));
            }
            if m.max_drive_vpk > m.drive_ceiling_vpk {
                why.push("the drive passed the ceiling".into());
            }
        }
    }
    let rises = match (m.worst_peak_rise_db, m.worst_bin_rise_db) {
        (Some(p), Some(b)) => format!("worst rise {p:.2} dB peak, {b:.2} dB bin"),
        _ => "no correction".into(),
    };
    let measured = format!(
        "{}; {}; full level {}; lag {} frames ({}); {} calls, {} not used; worst call {}; {}; \
         {} clamped bins; largest drive {:.3} V of {} V",
        end_words(&m.end),
        rung_words(m),
        full.map_or("not played".into(), |e| format!("{e:.2}%")),
        m.lag_frames.map_or("none".into(), |l| l.to_string()),
        if m.lag_measured {
            "measured"
        } else {
            "derived"
        },
        m.calls,
        m.misses,
        m.worst_call_ms.map_or("none".into(), |c| format!(
            "{c:.1} ms of {:.0} ms",
            m.budget_ms
        )),
        rises,
        m.clamped_bins,
        m.max_drive_vpk,
        m.drive_ceiling_vpk,
    );
    let passed = why.is_empty();
    CheckCase {
        name: case.name().to_string(),
        passed,
        detail: if passed {
            measured
        } else {
            format!("{}. Measured: {measured}", why.join("; "))
        },
    }
}

/// The check record for the law file `law_sha256` from its judged cases, at
/// the check in force ([`TWR_CHECK_VERSION`]).
pub fn twr_check_record(
    law_sha256: impl Into<String>,
    date: impl Into<String>,
    cases: Vec<CheckCase>,
) -> CheckRecord {
    CheckRecord::new(law_sha256, TWR_CHECK_VERSION, date, cases)
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

    fn good() -> TwrCaseMeasure {
        TwrCaseMeasure {
            end: TwrCaseEnd::Completed,
            plays: 6,
            rungs: TWR_CHECK_LEVELS_DB.iter().map(|&l| (l, Some(1))).collect(),
            full_level_error_pct: Some(1.0),
            lag_frames: Some(41),
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
        }
    }

    #[test]
    fn the_rules_pass_a_good_measure_and_fail_each_broken_rule() {
        use TwrCheckCase as C;
        let ok = |c: C, m: &TwrCaseMeasure| judge_twr(c, m).passed;
        for c in C::ALL {
            assert!(
                ok(c, &good()),
                "{}: {}",
                c.name(),
                judge_twr(c, &good()).detail
            );
        }
        // Every case: a miss or a refusal fails it.
        let mut m = good();
        m.misses = 1;
        m.refused = 1;
        assert!(C::ALL.iter().all(|&c| !ok(c, &m)));
        // First play: a rung on its second play fails flat, not one mode.
        let mut m = good();
        m.rungs[2].1 = Some(2);
        assert!(!ok(C::Flat, &m) && ok(C::OneMode, &m));
        // The deep queue: a derived lag, or another number, fails it.
        let mut m = good();
        m.lag_measured = false;
        assert!(!ok(C::DeepQueue, &m) && ok(C::Flat, &m));
        let mut m = good();
        m.lag_frames = Some(553);
        assert!(!ok(C::DeepQueue, &m));
        // Full level over its bound.
        let mut m = good();
        m.full_level_error_pct = Some(6.0);
        assert!(!ok(C::Flat, &m) && ok(C::OneMode, &m));
        m.full_level_error_pct = Some(11.0);
        assert!(!ok(C::Noise, &m) && !ok(C::OneMode, &m));
        // Every rung within its eight; a rung that never gates fails.
        let mut m = good();
        m.rungs[1].1 = None;
        m.end = TwrCaseEnd::NotConverged;
        assert!(!ok(C::OneMode, &m));
        assert!(
            ok(C::AntiResonance, &m),
            "not converged is the core's own end"
        );
        // The first rung by play 4, every later on its first.
        let mut m = good();
        m.rungs[0].1 = Some(4);
        assert!(ok(C::IdentificationError, &m));
        m.rungs[0].1 = Some(5);
        assert!(!ok(C::IdentificationError, &m));
        let mut m = good();
        m.rungs[3].1 = Some(2);
        assert!(!ok(C::IdentificationError, &m));
        // Survive: an envelope abort, a law fault or a drive over the ceiling
        // fail it; the coverage floor and divergence do not.
        for (end, pass) in [
            (TwrCaseEnd::Aborted("coverage".into()), true),
            (TwrCaseEnd::Divergence, true),
            (TwrCaseEnd::Aborted("envelope".into()), false),
            (TwrCaseEnd::LawFault("over_budget".into()), false),
            (TwrCaseEnd::Held, false),
        ] {
            let mut m = good();
            m.end = end.clone();
            assert_eq!(ok(C::AntiResonance, &m), pass, "{end:?}");
        }
        let mut m = good();
        m.max_drive_vpk = 10.5;
        assert!(!ok(C::AntiResonance, &m));
        let d = judge_twr(C::Flat, &good()).detail;
        assert!(
            d.contains("completed") && d.contains("41 frames (measured)"),
            "{d}"
        );
    }

    #[test]
    fn a_case_on_a_dead_station_is_measured_as_a_fault_and_fails() {
        let m = run_twr_check_case(TwrCheckCase::Flat, Arc::new(Never), Box::new(Untouched))
            .expect("the run is set up; the station refuses it");
        assert!(matches!(m.end, TwrCaseEnd::Fault(_)), "{:?}", m.end);
        assert_eq!(m.plays, 0);
        assert!(!judge_twr(TwrCheckCase::Flat, &m).passed);
    }

    #[test]
    fn the_station_facts_follow_the_plants() {
        use TwrCheckCase as C;
        let wire = twr_check_station(C::Flat);
        assert_eq!(
            wire.station,
            TwrCheckStation::Resonators {
                modes: vec![],
                feedthrough: 1.0
            }
        );
        assert_eq!(wire.noise_rms_g, TWR_CHECK_NOISE_FLOOR_G);
        let dq = twr_check_station(C::DeepQueue);
        assert_eq!(
            (
                dq.transport_delay_samples,
                dq.output_lookahead_frames,
                dq.block_frames
            ),
            (41, Some(512), Some(256))
        );
        assert_eq!(twr_check_station(C::Delay).transport_delay_samples, 26);
        let noisy = twr_check_station(C::Noise).noise_rms_g;
        let rms = section_rms(&twr_check_reference().samples);
        // 30 dB under the -12 dB rung's response: 42 dB under full level.
        assert!((20.0 * (rms / noisy).log10() - 42.0).abs() < 1e-9);
        let TwrCheckStation::Frf { df_hz, lines, .. } = twr_check_station(C::AntiResonance).station
        else {
            panic!("the notch is an FRF plant")
        };
        let at = |f: f64| {
            let (re, im) = lines[(f / df_hz) as usize];
            20.0 * (re * re + im * im).sqrt().log10()
        };
        assert!((at(300.0) + 30.0).abs() < 1e-9, "{}", at(300.0));
        assert!(at(100.0).abs() < 0.5 && at(1000.0).abs() < 0.5);
        let r = twr_check_reference();
        assert_eq!(r.samples.len(), 25_600);
        let pk = r.samples.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!((pk - TWR_CHECK_PEAK_G).abs() < 1e-12);
    }
}
