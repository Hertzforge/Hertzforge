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
//! reported with every case. One case's station plays the drive late, by a
//! hardware station's measured lag ([`SINE_CHECK_TRANSPORT_DELAY_S`]); the
//! others have none.
//!
//! The station a case runs on is built by the controller from these methods
//! alone ([`SineCheckCase::plant`], [`SineCheckCase::noise_below_db`],
//! [`SineCheckCase::limit`], at [`SINE_CHECK_FS_HZ`]); the runner's own run
//! configuration is the authority on everything else, and the controller's
//! station must follow it.

use hertzforge_channel_table::{Channel, ChannelRole, ChannelTable, Units};
use hertzforge_hal::Backend;
use hertzforge_random_laws::{CheckCase, CheckRecord};
use hertzforge_sine::law::servo_blend::ServoBlendParams;
use hertzforge_sine::law::LawGuards;
use hertzforge_sine::service::config::{
    ControlChannel, ControlSet, LimitProfile, SineMode, SineTestConfig,
};
use hertzforge_sine::service::engine::TickObservation;
use hertzforge_sine::{
    BandwidthMode, BidirSweepConfig, CompositeMode, CompressionPreset, DwellFor, DwellPlan,
    FeedforwardConfig, LadderConfig, NotchGovernorConfig, ProfileRow, RunMode, RunState, RungFor,
    SineLawFactory, SineProfile, SineService, SweepDirection, SweepRate, SweepTarget, TripKind,
};
use std::fmt;
use std::sync::{Arc, Mutex};

/// The version of the Sine check written into its records. A change to the
/// plant set or to the rules changes it.
pub const SINE_CHECK_VERSION: &str = "sine-check 2";

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
    /// A flat wire (1 g per volt) that plays the drive `delay_s` late, as a
    /// hardware station's converters and filters do.
    DelayedWire {
        /// The drive-to-response lag, seconds.
        delay_s: f64,
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
            SineCheckPlant::DelayedWire { delay_s } => {
                write!(f, "a wire behind {} ms", delay_s * 1e3)
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
    /// Every judged tick at full level within `hz` of the resonance (after the
    /// settle window) within `db` of the target.
    Through {
        /// The band, dB either way.
        db: f64,
        /// How far either side of the resonance a tick is judged, Hz.
        hz: f64,
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
    /// The run completes, and every tick from `after_s` after the engage on
    /// (settle window or not) is within `db` of the target.
    Settles {
        /// The band, dB either way.
        db: f64,
        /// How long after the engage the band applies, seconds.
        after_s: f64,
    },
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
            SineRequirement::Through { .. } => "through",
            SineRequirement::WithinAway { .. } => "within_away",
            SineRequirement::Survive => "survive",
            SineRequirement::Settles { .. } => "settles",
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
    /// A flat wire behind a hardware station's lag; an engage at -6 dB and a
    /// 6 s dwell at 100 Hz.
    Delay,
}

impl SineCheckCase {
    /// Every case, in the order the check runs them.
    pub const ALL: [SineCheckCase; 8] = [
        SineCheckCase::Slow,
        SineCheckCase::Fast,
        SineCheckCase::CloseModes,
        SineCheckCase::AntiResonance,
        SineCheckCase::Noise,
        SineCheckCase::Ladder,
        SineCheckCase::Notch,
        SineCheckCase::Delay,
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
            SineCheckCase::Delay => "transport delay",
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
            SineCheckCase::Delay => SineCheckPlant::DelayedWire {
                delay_s: SINE_CHECK_TRANSPORT_DELAY_S,
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
            SineCheckCase::Delay => SineCheckMode::Dwell {
                freq_hz: 100.0,
                seconds: 6.0,
            },
        }
    }

    /// The levels the run goes through, dB; the last is full level.
    pub fn levels_db(self) -> &'static [f64] {
        match self {
            SineCheckCase::Ladder => &[-20.0, -12.0, -6.0, 0.0],
            SineCheckCase::Delay => &[-6.0],
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
            // Within 1 dB through the resonance (within 30 Hz of it): the
            // built-in at its defaults measures -0.48 dB at 253.6 Hz at the
            // check's 10 ms tick. Judged near the resonance only: where the
            // identification is not trusted the seed starts low, and the
            // sweep's first ticks after the settle window are still climbing.
            SineCheckCase::Slow => SineRequirement::Through { db: 1.0, hz: 30.0 },
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
            // The response of a new drive arrives five ticks late: a law that
            // reads the error before then sees a target it has not yet met and
            // climbs. Judged from 2 s after the engage, inside the settle
            // window, which at the slow compression is longer.
            SineCheckCase::Delay => SineRequirement::Settles {
                db: 0.5,
                after_s: 2.0,
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
    /// after the settle window; for [`SineRequirement::Through`], only those
    /// near the resonance; for [`SineRequirement::Settles`], every tick from
    /// its time after the engage on).
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
    /// The largest rise the core took from the law in one tick, dB (for the
    /// rise cap's sizing: it should sit well under the cap).
    pub worst_rise_db: f64,
}

/// The control tick every case runs at, seconds (164 frames at
/// [`SINE_CHECK_FS_HZ`]).
pub const SINE_CHECK_TICK_S: f64 = 0.010;

/// The simulated plant's broadband drive-to-response gain under its modes.
pub const SINE_CHECK_FEEDTHROUGH: f64 = 0.35;

/// The simulated station's noise floor on every input, g rms (a case with
/// measurement noise adds its own, [`SineCheckCase::noise_rms_g`]).
pub const SINE_CHECK_NOISE_FLOOR_G: f64 = 1e-4;

/// The abort band every case runs with, dB either way of the target: wide,
/// so a case measures the law and not the band. The alarm band is half.
pub const SINE_CHECK_ABORT_DB: f64 = 12.0;

/// The identification level every case runs at, dB.
pub const SINE_CHECK_SYSID_DB: f64 = -20.0;

/// The seed of the identification's noise burst (the same run every time).
pub const SINE_CHECK_SEED: u64 = 0x51E5_C4EC;

/// The noise the anti-resonance case's station adds, g rms: enough that the
/// identification's coherence at the anti-resonance is under the trust floor
/// and over it everywhere else.
pub const SINE_CHECK_ANTI_RESONANCE_NOISE_G: f64 = 1e-3;

/// How long after the notch releases the control channel is watched for
/// overshoot, seconds.
pub const SINE_CHECK_RELEASE_WATCH_S: f64 = 1.0;

/// The station a case runs on, as data: the controller builds a simulated
/// station from it, and the check's own tests build theirs the same way.
#[derive(Debug, Clone, PartialEq)]
pub enum SineCheckStation {
    /// The simulator's resonator bank: one drive, every input the same
    /// response, `feedthrough` plus each `(f_n Hz, Q)` mode's constant-peak
    /// bandpass (its gain `Q` at `f_n`).
    Resonators {
        /// The modes.
        modes: Vec<(f64, f64)>,
        /// The broadband gain under them.
        feedthrough: f64,
    },
    /// The simulator's FRF plant: one drive; for each input, one complex gain
    /// `(re, im)` per line, line `k` at `k * df_hz` from 0 Hz to half the
    /// rate, played through partitions of `block_frames`.
    Frf {
        /// Line spacing, Hz.
        df_hz: f64,
        /// Per input (control first, then the limit channel), per line.
        lines: Vec<Vec<(f64, f64)>>,
        /// The FRF plant's partition length.
        block_frames: usize,
    },
}

/// Everything about a case's station: its plant, its noise, its inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct SineCheckStationSpec {
    /// The plant.
    pub station: SineCheckStation,
    /// The noise on every input, g rms.
    pub noise_rms_g: f64,
    /// Inputs: 1 (the control channel), or 2 (the control channel, then the
    /// limit channel).
    pub inputs: usize,
    /// The drive-to-response lag the station plays, whole samples at the
    /// case's rate, on every path.
    pub transport_delay_samples: usize,
}

/// One mode's response at `f`: the constant-peak bandpass `j r / (1 - r^2 + j
/// r / Q)`, `r = f / f_n`, as `(re, im)`.
fn mode_response(f: f64, f_n: f64, q: f64) -> (f64, f64) {
    let r = f / f_n;
    let (dr, di) = (1.0 - r * r, r / q);
    let d = dr * dr + di * di;
    // (0 + j r) / (dr + j di) = (r di + j r dr) / d
    (r * di / d, r * dr / d)
}

impl SineCheckCase {
    /// The measurement noise the case's station adds, g rms at the full-level
    /// response (`None`: the station's floor only).
    pub fn noise_rms_g(self) -> Option<f64> {
        self.noise_below_db()
            .map(|db| SINE_CHECK_TARGET_G / std::f64::consts::SQRT_2 * 10f64.powf(-db / 20.0))
    }
}

/// The transport delay case's drive-to-response lag, seconds: a hardware
/// station's measured lag (43.2 ms from drive out to response in, on an NI
/// chassis looped back at 12 800 Hz), rounded up; 819 samples at
/// [`SINE_CHECK_FS_HZ`]. The other cases' stations have none.
pub const SINE_CHECK_TRANSPORT_DELAY_S: f64 = 0.05;

/// The station `case` runs on at `fs_hz`.
pub fn sine_check_station(case: SineCheckCase, fs_hz: f64) -> SineCheckStationSpec {
    let noise = case
        .noise_rms_g()
        .unwrap_or(SINE_CHECK_NOISE_FLOOR_G)
        .max(SINE_CHECK_NOISE_FLOOR_G);
    let df = 1.0;
    let n_lines = (fs_hz / 2.0 / df).floor() as usize + 1;
    match (case.plant(), case.limit()) {
        (SineCheckPlant::OneMode { f_hz, q }, Some(lim)) => {
            let mut control = Vec::with_capacity(n_lines);
            let mut limit = Vec::with_capacity(n_lines);
            for k in 0..n_lines {
                let f = k as f64 * df;
                let (re, im) = mode_response(f, f_hz, q);
                let c = (SINE_CHECK_FEEDTHROUGH + re, im);
                let bump =
                    1.0 + (lim.peak_ratio - 1.0) / (1.0 + ((f - f_hz) / lim.width_hz).powi(2));
                control.push(c);
                limit.push((c.0 * bump, c.1 * bump));
            }
            SineCheckStationSpec {
                station: SineCheckStation::Frf {
                    df_hz: df,
                    lines: vec![control, limit],
                    block_frames: 16,
                },
                noise_rms_g: noise,
                inputs: 2,
                transport_delay_samples: 0,
            }
        }
        (SineCheckPlant::OneMode { f_hz, q }, None) => SineCheckStationSpec {
            station: SineCheckStation::Resonators {
                modes: vec![(f_hz, q)],
                feedthrough: SINE_CHECK_FEEDTHROUGH,
            },
            noise_rms_g: noise,
            inputs: 1,
            transport_delay_samples: 0,
        },
        (SineCheckPlant::TwoModes { f_hz, q }, _) => SineCheckStationSpec {
            station: SineCheckStation::Resonators {
                modes: vec![(f_hz[0], q), (f_hz[1], q)],
                feedthrough: SINE_CHECK_FEEDTHROUGH,
            },
            noise_rms_g: noise,
            inputs: 1,
            transport_delay_samples: 0,
        },
        (SineCheckPlant::AntiResonance { f_hz, depth_db }, _) => {
            // A flat wire cut `depth_db` at `f_hz` by a peaking biquad (Q 5).
            let a = 10f64.powf(-depth_db / 40.0);
            let w0 = std::f64::consts::TAU * f_hz / fs_hz;
            let alpha = w0.sin() / (2.0 * 5.0);
            let (b0, b1, b2) = (1.0 + alpha * a, -2.0 * w0.cos(), 1.0 - alpha * a);
            let (a0, a1, a2) = (1.0 + alpha / a, -2.0 * w0.cos(), 1.0 - alpha / a);
            let lines = (0..n_lines)
                .map(|k| {
                    let w = std::f64::consts::TAU * k as f64 * df / fs_hz;
                    let (c1, s1) = (w.cos(), -w.sin());
                    let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
                    let (nr, ni) = (b0 + b1 * c1 + b2 * c2, b1 * s1 + b2 * s2);
                    let (dr, di) = (a0 + a1 * c1 + a2 * c2, a1 * s1 + a2 * s2);
                    let d = dr * dr + di * di;
                    ((nr * dr + ni * di) / d, (ni * dr - nr * di) / d)
                })
                .collect();
            SineCheckStationSpec {
                station: SineCheckStation::Frf {
                    df_hz: df,
                    lines: vec![lines],
                    block_frames: 16,
                },
                noise_rms_g: noise.max(SINE_CHECK_ANTI_RESONANCE_NOISE_G),
                inputs: 1,
                transport_delay_samples: 0,
            }
        }
        (SineCheckPlant::DelayedWire { delay_s }, _) => SineCheckStationSpec {
            station: SineCheckStation::Resonators {
                modes: Vec::new(),
                feedthrough: 1.0,
            },
            noise_rms_g: noise,
            inputs: 1,
            transport_delay_samples: (delay_s * fs_hz).round() as usize,
        },
    }
}

fn flat_profile(band: (f64, f64), g: f64, alarm_db: f64, abort_db: f64) -> SineProfile {
    let mut p = SineProfile::new("check");
    p.push(ProfileRow::accel(band.0, g));
    p.push(ProfileRow::accel(band.1, g));
    p.global_alarm_db = (alarm_db, alarm_db);
    p.global_abort_db = (abort_db, abort_db);
    p
}

/// The band a case's profile covers: the sweep's, or a band around the
/// dwell.
fn case_band(case: SineCheckCase) -> (f64, f64) {
    match case.mode() {
        SineCheckMode::Sweep { band_hz, .. } => band_hz,
        SineCheckMode::Dwell { .. } => (100.0, 800.0),
    }
}

/// The channel table every case runs with: the drive (index 1), the control
/// channel (2) and, with a limit channel, the limit channel (3).
pub fn sine_check_table(case: SineCheckCase) -> ChannelTable {
    let mut t = ChannelTable::new();
    let mut drive = Channel::new(1);
    drive.role = ChannelRole::Drive;
    drive.description = "Drive".into();
    drive.units = Units::VoltageV;
    t.channels.push(drive);
    let mut control = Channel::new(2);
    control.role = ChannelRole::Control;
    control.description = "Control".into();
    control.units = Units::AccelG;
    t.channels.push(control);
    if case.limit().is_some() {
        let mut lim = Channel::new(3);
        lim.role = ChannelRole::Monitor;
        lim.description = "Limit".into();
        lim.units = Units::AccelG;
        t.channels.push(lim);
    }
    t
}

/// The run a case is, at the station's rate `fs_hz`: the profile, the
/// schedule, the tick, the limits, the core's guards at their defaults.
pub fn sine_check_config(case: SineCheckCase, fs_hz: f64) -> SineTestConfig {
    let band = case_band(case);
    let levels = case.levels_db().to_vec();
    let (mode, per_rung, full_for) = match case.mode() {
        SineCheckMode::Sweep {
            band_hz,
            oct_per_min,
            sweeps,
        } => (
            SineMode::Sweep(BidirSweepConfig {
                band: band_hz,
                rate: SweepRate::LogOctPerMin(oct_per_min),
                start_hz: band_hz.0,
                initial_direction: SweepDirection::Up,
                target: SweepTarget::Sweeps(sweeps),
            }),
            RungFor::Sweeps(1),
            RungFor::Sweeps(sweeps),
        ),
        SineCheckMode::Dwell { freq_hz, seconds } => (
            SineMode::Dwell(DwellPlan {
                freq_hz,
                for_: DwellFor::Seconds(seconds * levels.len() as f64),
            }),
            RungFor::Seconds(seconds),
            RungFor::Seconds(seconds),
        ),
    };
    let limit_profiles = case
        .limit()
        .map(|l| {
            vec![LimitProfile {
                channel_index: 3,
                profile: flat_profile(band, l.bound_g, 1.5, 3.0),
                shared_name: None,
            }]
        })
        .unwrap_or_default();
    SineTestConfig {
        profile: flat_profile(
            band,
            SINE_CHECK_TARGET_G,
            SINE_CHECK_ABORT_DB / 2.0,
            SINE_CHECK_ABORT_DB,
        ),
        mode,
        control: ControlSet {
            channels: vec![ControlChannel {
                index: 2,
                weight: 1.0,
            }],
            mode: CompositeMode::WeightedAvg,
        },
        limit_profiles,
        blend_w: 0.5,
        compression: CompressionPreset::Slow,
        tracking_bw: BandwidthMode::Proportional {
            pct: 10.0,
            max_hz: 100.0,
        },
        feedforward: FeedforwardConfig::default(),
        notch: NotchGovernorConfig::default(),
        ladder: LadderConfig {
            sysid_level_db: SINE_CHECK_SYSID_DB,
            start_db: levels[0],
            table_db: levels,
            per_rung,
            full_for,
        },
        sysid_level_db: SINE_CHECK_SYSID_DB,
        // The check measures the law, not the gate: no case is held at it.
        coherence_floor: 0.0,
        drive_vpk_ceiling: SINE_CHECK_DRIVE_CEILING_V,
        drive_vpk_abort: SINE_CHECK_DRIVE_ABORT_V,
        fault_debounce_ticks: 50,
        kinematic_monitor: false,
        capture: Default::default(),
        cola: Default::default(),
        shaker: None,
        moving_mass: Default::default(),
        review_hold: false,
        control_tick_frames: ((SINE_CHECK_TICK_S * fs_hz).round() as usize).max(1),
        fs_hz: None,
        metadata: Vec::new(),
        law_guards: Some(LawGuards::default()),
        control_tick_s: Some(SINE_CHECK_TICK_S),
    }
}

/// What the observer keeps while a case runs (no allocation per tick).
#[derive(Debug, Clone, Default)]
struct Watch {
    ticks: u64,
    judged: u64,
    worst: Option<(f64, f64)>,
    worst_away: Option<f64>,
    rungs: Vec<(f64, Option<f64>)>,
    worst_limit_over: Option<f64>,
    release_overshoot: Option<f64>,
    watching_release_until: Option<f64>,
    was_notched: bool,
    max_drive: f64,
    t: f64,
}

fn worse(a: Option<f64>, e: f64) -> Option<f64> {
    Some(match a {
        Some(w) if w.abs() >= e.abs() => w,
        _ => e,
    })
}

/// Run `case` with the law `factory` builds, under the core's guards, on
/// `backend` (the simulated station the controller built for the case from
/// [`sine_check_station`]), and measure it. The run is
/// [`sine_check_config`] on [`sine_check_table`].
pub fn run_check_case(
    case: SineCheckCase,
    factory: Arc<dyn SineLawFactory>,
    backend: Box<dyn Backend>,
) -> Result<SineCaseMeasure, CheckRunError> {
    let fs_hz = backend.caps().sample_rate_hz;
    let mut cfg = sine_check_config(case, fs_hz);
    // The built-in runs under its own blend and compression, as the control
    // service writes them into a run's configuration (the settle window reads
    // the compression); a law file runs under the configuration's.
    let described = factory.describe();
    if described.sha256.is_none() && described.id == hertzforge_sine::law::SERVO_BLEND_ID {
        if let Ok(p) = ServoBlendParams::from_settings(&described.params) {
            cfg.blend_w = p.blend_w;
            cfg.compression = p.compression;
        }
    }
    let levels = case.levels_db().to_vec();
    let full_db = *levels.last().unwrap_or(&0.0);
    let tick_s = cfg.control_tick_frames as f64 / fs_hz;
    let watch = Arc::new(Mutex::new(Watch {
        rungs: levels.iter().map(|&l| (l, None)).collect(),
        ..Watch::default()
    }));
    let sink = watch.clone();
    let mode_hz = match case.plant() {
        SineCheckPlant::OneMode { f_hz, .. } | SineCheckPlant::AntiResonance { f_hz, .. } => f_hz,
        SineCheckPlant::TwoModes { f_hz, .. } => 0.5 * (f_hz[0] + f_hz[1]),
        // No mode: only `through` and `within_away` read it.
        SineCheckPlant::DelayedWire { .. } => 0.0,
    };
    let away_hz = match case.requirement() {
        SineRequirement::WithinAway { away_hz, .. } => away_hz,
        _ => f64::INFINITY,
    };
    let through_hz = match case.requirement() {
        SineRequirement::Through { hz, .. } => hz,
        _ => f64::INFINITY,
    };
    // `settles` judges on the time since the engage, not on the settle window.
    let settles_after_s = match case.requirement() {
        SineRequirement::Settles { after_s, .. } => Some(after_s),
        _ => None,
    };
    let observer: hertzforge_sine::service::engine::TickObserver =
        Box::new(move |o: &TickObservation| {
            let Ok(mut w) = sink.lock() else {
                return;
            };
            w.ticks += 1;
            w.t += tick_s;
            w.max_drive = w.max_drive.max(o.drive);
            if let Some(l) = o.limit_over_db {
                w.worst_limit_over = Some(w.worst_limit_over.map_or(l, |x| x.max(l)));
            }
            if let Some(after_s) = settles_after_s {
                if w.t > after_s + 1e-9 && (o.level_db - full_db).abs() < 1e-9 {
                    let e = if o.measured > 0.0 && o.target > 0.0 {
                        20.0 * (o.measured / o.target).log10()
                    } else {
                        f64::NEG_INFINITY
                    };
                    w.judged += 1;
                    if w.worst.is_none_or(|(we, _)| e.abs() > we.abs()) {
                        w.worst = Some((e, o.freq_hz));
                    }
                }
            }
            if o.settling || !(o.measured > 0.0 && o.target > 0.0) {
                w.was_notched = o.notch_binding;
                return;
            }
            let e = 20.0 * (o.measured / o.target).log10();
            if let Some(r) = w
                .rungs
                .iter_mut()
                .find(|(l, _)| (l - o.level_db).abs() < 1e-9)
            {
                r.1 = worse(r.1, e);
            }
            if w.was_notched && !o.notch_binding {
                w.watching_release_until = Some(w.t + SINE_CHECK_RELEASE_WATCH_S);
            }
            if w.watching_release_until.is_some_and(|until| w.t <= until) && !o.notch_binding {
                w.release_overshoot = Some(w.release_overshoot.map_or(e, |x| x.max(e)));
            }
            w.was_notched = o.notch_binding;
            if settles_after_s.is_none()
                && (o.level_db - full_db).abs() < 1e-9
                && !o.notch_binding
                && (o.freq_hz - mode_hz).abs() < through_hz
            {
                w.judged += 1;
                if w.worst.is_none_or(|(we, _)| e.abs() > we.abs()) {
                    w.worst = Some((e, o.freq_hz));
                }
                if (o.freq_hz - mode_hz).abs() > away_hz {
                    w.worst_away = worse(w.worst_away, e);
                }
            }
        });
    let root = std::env::temp_dir().join("hertzforge-sine-check");
    let svc = SineService::new(
        cfg,
        sine_check_table(case),
        backend,
        hertzforge_module_abi::StationClass::HertzforgeSim,
        SINE_CHECK_SEED,
        Some(root),
    )
    .map_err(|e| CheckRunError::Setup(e.to_string()))?
    .with_law_factory(factory.clone())
    .with_tick_observer(observer);
    let mut svc = svc;
    let fault = drive(case, &mut svc, &factory).err();
    if matches!(svc.state(), RunState::Engaged | RunState::FullLevel) {
        let _ = svc.stop();
    }
    let w = watch.lock().map(|w| w.clone()).unwrap_or_default();
    Ok(measure(case, &svc, &w, fault, tick_s))
}

/// The case's run: identification, the gate, the engage, then the schedule;
/// the ladder case's levels moved by hand at each rung's end, with one live
/// change partway through the third rung.
fn drive(
    case: SineCheckCase,
    s: &mut SineService,
    factory: &Arc<dyn SineLawFactory>,
) -> Result<(), String> {
    s.assess_feasibility().map_err(|e| e.to_string())?;
    s.arm().map_err(|e| e.to_string())?;
    s.run_sysid(0).map_err(|e| e.to_string())?;
    s.coherence_gate().map_err(|e| e.to_string())?;
    if case.manual() {
        s.set_run_mode(RunMode::Manual).map_err(|e| e.to_string())?;
    }
    let (rung_s, rungs) = match case.mode() {
        SineCheckMode::Dwell { seconds, .. } => (seconds, case.levels_db().len()),
        SineCheckMode::Sweep { .. } => (f64::INFINITY, 1),
    };
    // A bound on chunks: the longest case is a 3-octave sweep at 1 oct/min.
    let max_chunks = 4 * 60 * 4 + 400;
    let mut chunk = 0u64;
    let chunk_s = 0.25;
    let live = factory.describe().live;
    while matches!(s.state(), RunState::Engaged | RunState::FullLevel) && chunk < max_chunks {
        let t = chunk as f64 * chunk_s;
        if case.manual() {
            let rung = ((t / rung_s).floor() as usize).min(rungs - 1);
            if (t - rung as f64 * rung_s).abs() < 1e-9 && rung > 0 {
                s.jump_to_level(rung).map_err(|e| e.to_string())?;
            }
            if case.live_change() && (t - 2.5 * rung_s).abs() < 1e-9 {
                if let Some(l) = live.first() {
                    let _ = s.adjust(&l.key, 0.5 * (l.min + l.max));
                }
            }
        }
        if !s.run_chunk().map_err(|e| e.to_string())? {
            break;
        }
        chunk += 1;
    }
    Ok(())
}

fn measure(
    case: SineCheckCase,
    s: &SineService,
    w: &Watch,
    fault: Option<String>,
    tick_s: f64,
) -> SineCaseMeasure {
    let end = match (fault, s.state(), s.trip().map(|t| &t.kind)) {
        (Some(f), ..) => SineCaseEnd::Fault(f),
        (None, RunState::CoherenceHold, _) => SineCaseEnd::Held,
        (None, _, Some(TripKind::LawFault { fault, .. })) => {
            SineCaseEnd::LawFault(fault.reason().into())
        }
        (None, RunState::Aborted, Some(k)) => SineCaseEnd::Aborted(trip_name(k).into()),
        (None, RunState::Aborted, None) => {
            SineCaseEnd::Fault(s.end_reason().unwrap_or("aborted").to_string())
        }
        (None, _, _) => SineCaseEnd::Completed,
    };
    let tally = s.law_tally();
    let budget_ms = 1e3
        * s.config()
            .law_guards
            .as_ref()
            .map_or(f64::INFINITY, |g| g.call_budget_s(tick_s));
    SineCaseMeasure {
        end,
        ticks: w.ticks,
        judged_ticks: w.judged,
        worst_error_db: w.worst.map(|(e, _)| e),
        worst_error_hz: w.worst.map(|(_, f)| f),
        worst_away_error_db: w.worst_away,
        rungs: w.rungs.clone(),
        worst_limit_over_db: w.worst_limit_over,
        worst_release_overshoot_db: if case.limit().is_some() {
            Some(w.release_overshoot.unwrap_or(0.0))
        } else {
            None
        },
        calls: tally.calls,
        misses: tally.misses,
        refused: tally.refused,
        capped_ticks: tally.capped,
        worst_call_ms: (tally.calls > 0).then_some(tally.worst_call_us / 1e3),
        budget_ms,
        max_drive_vpk: w.max_drive,
        drive_ceiling_vpk: s.config().drive_vpk_ceiling,
        worst_rise_db: tally.worst_rise_db,
    }
}

/// A trip kind's wire name.
pub fn trip_name(k: &TripKind) -> &'static str {
    match k {
        TripKind::ScalarAbort { .. } => "abort_band",
        TripKind::Kinematic { .. } => "kinematic",
        TripKind::LimitChannel { .. } => "limit_channel",
        TripKind::DriveVpk { .. } => "drive_vpk",
        TripKind::ServoFault { .. } => "servo_fault",
        TripKind::WatchdogAccel { .. } => "watchdog",
        TripKind::OscillatorRange { .. } => "oscillator_range",
        TripKind::LawFault { .. } => "law_fault",
        TripKind::Divergence { .. } => "divergence",
    }
}

fn end_words(e: &SineCaseEnd) -> String {
    match e {
        SineCaseEnd::Completed => "completed".into(),
        SineCaseEnd::Aborted(k) => format!("aborted ({k})"),
        SineCaseEnd::LawFault(r) => format!("ended on a law fault ({r})"),
        SineCaseEnd::Held => "held at identification".into(),
        SineCaseEnd::Fault(f) => format!("failed: {f}"),
    }
}

/// `scope` says which ticks were judged ("at full level", "at -6 dB").
fn within(why: &mut Vec<String>, scope: &str, err: Option<f64>, db: f64) {
    match err {
        Some(e) if e.abs() <= db => {}
        Some(e) => why.push(format!(
            "{scope} the worst tick was {e:+.2} dB from the target (at most {db} dB)"
        )),
        None => why.push(format!("no tick was judged {scope}")),
    }
}

/// Judge `measure` against `case`: a passed or failed case, with what was
/// measured in words.
pub fn judge_sine(case: SineCheckCase, measure: &SineCaseMeasure) -> CheckCase {
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
                "the slowest call took {w:.2} ms, over its {:.2} ms budget",
                m.budget_ms
            ));
        }
    }
    let survived = matches!(m.end, SineCaseEnd::Completed)
        || matches!(&m.end, SineCaseEnd::Aborted(k) if k == "divergence");
    match case.requirement() {
        SineRequirement::Survive => {
            if !survived {
                why.push(format!("the run {}", end_words(&m.end)));
            }
        }
        req => {
            if m.end != SineCaseEnd::Completed {
                why.push(format!("the run {}", end_words(&m.end)));
            }
            match req {
                SineRequirement::Within { db } => {
                    within(&mut why, "at full level", m.worst_error_db, db)
                }
                SineRequirement::Through { db, hz } => within(
                    &mut why,
                    &format!("within {hz} Hz of the resonance"),
                    m.worst_error_db,
                    db,
                ),
                SineRequirement::Settles { db, after_s } => within(
                    &mut why,
                    &format!("from {after_s} s after the engage"),
                    m.worst_error_db,
                    db,
                ),
                SineRequirement::WithinAway {
                    through_db,
                    away_db,
                    away_hz,
                } => {
                    within(&mut why, "at full level", m.worst_error_db, through_db);
                    within(
                        &mut why,
                        &format!("more than {away_hz} Hz from the resonance"),
                        m.worst_away_error_db,
                        away_db,
                    );
                }
                SineRequirement::EachRungWithin { db } => {
                    for (level, err) in &m.rungs {
                        within(&mut why, &format!("at {level:+.0} dB"), *err, db);
                    }
                }
                SineRequirement::LimitHeld {
                    over_bound_db,
                    overshoot_db,
                } => {
                    match m.worst_limit_over_db {
                        Some(o) if o <= over_bound_db => {}
                        Some(o) => why.push(format!(
                            "the limit channel read {o:+.2} dB over its limit (at most \
                             {over_bound_db} dB)"
                        )),
                        None => why.push("the limit channel was not read".into()),
                    }
                    match m.worst_release_overshoot_db {
                        Some(o) if o <= overshoot_db => {}
                        Some(o) => why.push(format!(
                            "the control channel overshot {o:+.2} dB after the notch released \
                             (at most {overshoot_db} dB)"
                        )),
                        None => why.push("the notch release was not watched".into()),
                    }
                }
                SineRequirement::Survive => {}
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
        .map(|w| format!("{w:.3} ms"))
        .unwrap_or_else(|| "no call".into());
    let summary = format!(
        "{}; {} ticks, {} judged; worst {}{}; slowest call {worst} of {:.2} ms; {} capped ticks",
        end_words(&m.end),
        m.ticks,
        m.judged_ticks,
        m.worst_error_db
            .map(|e| format!("{e:+.2} dB"))
            .unwrap_or_else(|| "not measured".into()),
        m.worst_error_hz
            .map(|f| format!(" at {f:.1} Hz"))
            .unwrap_or_default(),
        m.budget_ms,
        m.capped_ticks
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
pub fn sine_check_record(
    law_sha256: impl Into<String>,
    date: impl Into<String>,
    cases: Vec<CheckCase>,
) -> CheckRecord {
    CheckRecord::new(law_sha256, SINE_CHECK_VERSION, date, cases)
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
        assert_eq!(
            C::Slow.requirement(),
            SineRequirement::Through { db: 1.0, hz: 30.0 }
        );
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
            C::Delay.plant(),
            SineCheckPlant::DelayedWire {
                delay_s: SINE_CHECK_TRANSPORT_DELAY_S
            }
        );
        assert_eq!(C::Delay.plant().to_string(), "a wire behind 50 ms");
        assert_eq!(
            C::Delay.mode(),
            SineCheckMode::Dwell {
                freq_hz: 100.0,
                seconds: 6.0
            }
        );
        assert_eq!(C::Delay.levels_db(), &[-6.0]);
        assert_eq!(
            C::Delay.requirement(),
            SineRequirement::Settles {
                db: 0.5,
                after_s: 2.0
            }
        );
        assert_eq!(
            C::ALL.map(|c| c.requirement().as_str()),
            [
                "through",
                "within_away",
                "within",
                "survive",
                "within",
                "each_rung_within",
                "limit_held",
                "settles"
            ]
        );
    }

    fn good() -> SineCaseMeasure {
        SineCaseMeasure {
            end: SineCaseEnd::Completed,
            ticks: 18_000,
            judged_ticks: 17_000,
            worst_error_db: Some(0.4),
            worst_error_hz: Some(251.0),
            worst_away_error_db: Some(0.3),
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
            worst_rise_db: 0.5,
        }
    }

    #[test]
    fn the_rules_pass_a_good_run_and_name_each_failure() {
        use SineCheckCase as C;
        let ok = |c, m: &SineCaseMeasure| judge_sine(c, m).passed;
        assert!(ok(C::Slow, &good()));
        let fails = |c, m: SineCaseMeasure, words: &str| {
            let j = judge_sine(c, &m);
            assert!(!j.passed, "{c:?} passed: {}", j.detail);
            assert!(j.detail.contains(words), "{c:?}: {}", j.detail);
        };
        fails(
            C::Slow,
            SineCaseMeasure {
                worst_error_db: Some(-1.1),
                ..good()
            },
            "within 30 Hz of the resonance the worst tick was -1.10 dB",
        );
        assert!(ok(
            C::Slow,
            &SineCaseMeasure {
                worst_error_db: Some(0.9),
                ..good()
            }
        ));
        fails(
            C::Slow,
            SineCaseMeasure {
                misses: 2,
                refused: 1,
                ..good()
            },
            "2 call(s) not used",
        );
        fails(
            C::Slow,
            SineCaseMeasure {
                worst_call_ms: Some(3.0),
                ..good()
            },
            "slowest call",
        );
        fails(
            C::Slow,
            SineCaseMeasure {
                end: SineCaseEnd::Aborted("abort_band".into()),
                ..good()
            },
            "aborted (abort_band)",
        );
        fails(
            C::Slow,
            SineCaseMeasure {
                max_drive_vpk: 11.0,
                ..good()
            },
            "over the",
        );
        // Fast: 5 dB through, 2 dB away.
        assert!(ok(
            C::Fast,
            &SineCaseMeasure {
                worst_error_db: Some(4.0),
                ..good()
            }
        ));
        fails(
            C::Fast,
            SineCaseMeasure {
                worst_away_error_db: Some(2.5),
                ..good()
            },
            "more than 30 Hz",
        );
        // Survive: completed or the core's own divergence.
        assert!(ok(
            C::AntiResonance,
            &SineCaseMeasure {
                end: SineCaseEnd::Aborted("divergence".into()),
                worst_error_db: Some(-9.0),
                ..good()
            }
        ));
        fails(
            C::AntiResonance,
            SineCaseMeasure {
                end: SineCaseEnd::Aborted("drive_vpk".into()),
                ..good()
            },
            "aborted (drive_vpk)",
        );
        // The ladder: every rung judged and within.
        let rungs = |r: Vec<(f64, Option<f64>)>| SineCaseMeasure { rungs: r, ..good() };
        assert!(ok(
            C::Ladder,
            &rungs(vec![
                (-20.0, Some(0.5)),
                (-12.0, Some(0.2)),
                (-6.0, Some(-0.9)),
                (0.0, Some(0.3))
            ])
        ));
        fails(
            C::Ladder,
            rungs(vec![
                (-20.0, Some(0.5)),
                (-12.0, None),
                (-6.0, Some(0.1)),
                (0.0, Some(0.3)),
            ]),
            "at -12 dB",
        );
        // The notch: the limit channel and the release.
        let notch = |over: Option<f64>, release: Option<f64>| SineCaseMeasure {
            worst_limit_over_db: over,
            worst_release_overshoot_db: release,
            ..good()
        };
        assert!(ok(C::Notch, &notch(Some(0.3), Some(0.6))));
        fails(C::Notch, notch(Some(0.8), Some(0.6)), "over its limit");
        fails(C::Notch, notch(Some(0.3), Some(1.4)), "overshot");
        // The transport delay: completed, and within 0.5 dB from 2 s on.
        assert!(ok(C::Delay, &good()));
        fails(
            C::Delay,
            SineCaseMeasure {
                worst_error_db: Some(-0.6),
                ..good()
            },
            "from 2 s after the engage the worst tick was -0.60 dB",
        );
        fails(
            C::Delay,
            SineCaseMeasure {
                end: SineCaseEnd::Aborted("watchdog".into()),
                ..good()
            },
            "aborted (watchdog)",
        );
        fails(
            C::Delay,
            SineCaseMeasure {
                worst_error_db: None,
                ..good()
            },
            "no tick was judged from 2 s after the engage",
        );
    }

    #[test]
    fn every_station_is_one_drive_with_the_inputs_its_case_names() {
        for c in SineCheckCase::ALL {
            let st = sine_check_station(c, SINE_CHECK_FS_HZ);
            assert_eq!(
                st.inputs,
                if c == SineCheckCase::Notch { 2 } else { 1 },
                "{c:?}"
            );
            assert!(st.noise_rms_g >= SINE_CHECK_NOISE_FLOOR_G);
            if let SineCheckStation::Frf { lines, df_hz, .. } = &st.station {
                assert_eq!(lines.len(), st.inputs);
                assert_eq!(
                    lines[0].len(),
                    (SINE_CHECK_FS_HZ / 2.0 / df_hz) as usize + 1
                );
            }
        }
        // The limit channel reads four times the control at the mode.
        let st = sine_check_station(SineCheckCase::Notch, SINE_CHECK_FS_HZ);
        let SineCheckStation::Frf { lines, .. } = st.station else {
            panic!("the notch case is an FRF plant");
        };
        let mag = |c: (f64, f64)| (c.0 * c.0 + c.1 * c.1).sqrt();
        let r = mag(lines[1][250]) / mag(lines[0][250]);
        assert!((r - 4.0).abs() < 1e-9, "{r}");
        // The anti-resonance is 30 dB deep.
        let st = sine_check_station(SineCheckCase::AntiResonance, SINE_CHECK_FS_HZ);
        let SineCheckStation::Frf { lines, .. } = st.station else {
            panic!("the anti-resonance case is an FRF plant");
        };
        let depth = 20.0 * (mag(lines[0][300]) / mag(lines[0][1])).log10();
        assert!((depth + 30.0).abs() < 0.5, "{depth}");
        // The noise case's noise is 30 dB under the full-level response.
        let n = SineCheckCase::Noise.noise_rms_g().unwrap();
        assert!((20.0 * (n / (SINE_CHECK_TARGET_G / 2f64.sqrt())).log10() + 30.0).abs() < 1e-9);
        // Only the transport delay case's station plays the drive late: a flat
        // wire, 50 ms behind, 819 samples at the check's rate.
        for c in SineCheckCase::ALL {
            let st = sine_check_station(c, SINE_CHECK_FS_HZ);
            if c == SineCheckCase::Delay {
                assert_eq!(st.transport_delay_samples, 819);
                assert_eq!(
                    st.station,
                    SineCheckStation::Resonators {
                        modes: Vec::new(),
                        feedthrough: 1.0
                    }
                );
            } else {
                assert_eq!(st.transport_delay_samples, 0, "{c:?}");
            }
        }
    }
}
