//! The MIMO Shock run engine: its configuration, run states, the run itself and
//! its reports.
//!
//! A run is driven by the service one step at a time:
//!
//! 1. [`MimoShockEngine::arm`]: open the station at the rate asked, adopt the
//!    rate it programmed, re-derive the pulse grid and the targets at it, pass
//!    the arm gate. Nothing is driven.
//! 2. [`MimoShockEngine::identify`]: one cold shot of independent band-limited
//!    random on every drive, the plant on the pulse grid, its trust mask, and
//!    the holds (low coherence, a target out of the trusted band).
//! 3. [`MimoShockEngine::proceed`]: build the law for the run, report each
//!    control's reachable fraction, seed the law and guard the seed.
//! 4. [`MimoShockEngine::fire_next`], once per pulse, in this order: build and
//!    check the drives (ceilings, abort, the predicted response), fire them in
//!    one shot, measure every input, judge each control, check the measured
//!    response, tick the ladder (which fixes the next level), watch for
//!    divergence, call the law with the next level, guard its answer.
//! 5. The run ends by itself ([`EndOutcome`] and a reason) or on
//!    [`MimoShockEngine::stop`] or [`MimoShockEngine::abort`]. Every end stops
//!    and releases the station before the engine reports it ended.

use crate::guards::{bands, growth, predict, DivergenceWatch};
use crate::ident::{band_lines, estimate, excitation, rows_of};
use crate::judge::{global_srs_error_db, judge_control, peak, ControlJudgement};
use crate::ladder::{Ladder, StepNote, Tick};
use crate::law::{check_law_accepts, LawManifest, MimoShockLaw, MimoShockLawFactory};
use crate::limits::{MAX_CONTROLS, MAX_DRIVES, MAX_RECORD_LEN, MIN_RECORD_LEN};
use crate::pulse::{ControlVerdict, PulseInputs, PulseOutputs};
use crate::records::{LawCallEntry, PulseEntry, RunRecord};
use crate::setup::{PlantStack, Setup, SetupError, SetupRule, Target, Timing};
use crate::targets::{BuiltTarget, Judging, TargetSpec, DEFAULT_ENVELOPE_TOL};
use hertzforge_channel_table::ChannelTable;
use hertzforge_dsp::shock::pulse_kinematics;
use hertzforge_hal::Backend;
use hertzforge_mimo_shock_law_math::check::reachable_fraction;
use hertzforge_mimo_shock_law_math::spectrum::{rfft, rows_irfft, rows_rfft};
use hertzforge_module_abi::StationClass;
use hertzforge_sdm::linalg::{pinv, Regularization};
use hertzforge_sdm::MatrixStack;
use hertzforge_shock_common::law_guard::{
    LawCall, LawClock, LawDescriptor, LawFault, LawGuards, LawValue, MissCounter, MonotonicClock,
    OutputRule as GuardRule,
};
use hertzforge_shock_common::shot::{Prepared, ShotHooks};
use hertzforge_shock_host::service::firer::{BackendPulseFirer, FireControl, PulseRows};
use hertzforge_shock_host::{PulseFirer, ServiceError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One drive: a table Drive row with its own ceiling and abort.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DriveChannel {
    /// The table `index` of an enabled Drive row.
    pub channel: u32,
    /// A drive over this peak, volts, is scaled down to it.
    pub ceiling_v: f64,
    /// A drive over this peak, volts, is refused and the run aborts.
    pub abort_v: f64,
}

/// One limit channel: a monitored input held under an abort peak.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimitChannel {
    /// The table `index` of an enabled Monitor row.
    pub channel: u32,
    /// A measured or predicted peak over this, in the channel's units, aborts.
    pub abort_peak: f64,
}

/// The level ladder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderPlan {
    /// The levels, dB, ascending, ending at 0 dB (full level).
    pub levels_db: Vec<f64>,
    /// Pulses at one level without a joint pass before the run ends
    /// `not_converged`.
    pub max_pulses_per_level: u32,
    /// Passing pulses at full level that complete the run (one passing pulse
    /// completes every lower level).
    #[serde(default = "default_full_level_pulses")]
    pub full_level_pulses: u32,
}

fn default_full_level_pulses() -> u32 {
    3
}

impl Default for LadderPlan {
    /// -12, -9, -6, -3, -1.5 and 0 dB (a half step before full level), three
    /// pulses a level, three passing pulses at full level.
    fn default() -> Self {
        LadderPlan {
            levels_db: vec![-12.0, -9.0, -6.0, -3.0, -1.5, 0.0],
            max_pulses_per_level: 3,
            full_level_pulses: default_full_level_pulses(),
        }
    }
}

/// The identification shot: independent band-limited random on every drive,
/// estimated on the pulse grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentificationPlan {
    /// The most each drive plays, volts rms (and never over its ceiling at
    /// its peak). The level itself is found relative to the reference: a short
    /// probe shot at a tenth of this, then the shot scaled so the control
    /// closest to it reaches its target's rms less [`IDENTIFICATION_BELOW_DB`].
    pub drive_vrms: f64,
    /// Records of `record_len` averaged; at least 10 times the drive count.
    pub frames: u32,
    /// Lines with a control's multiple coherence under this are not trusted.
    pub coherence_floor: f64,
    /// Hold for the operator after identification.
    pub review: bool,
}

/// The core's guards on every pulse, whatever the law.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuardPlan {
    /// The most any drive's peak may grow from one pulse to the next beyond the
    /// level step, dB.
    pub max_growth_db: f64,
    /// The most any third-octave band of a drive may grow from one pulse to
    /// the next beyond the level step, dB. Above the built-in laws' own
    /// per-pulse clamp (6 dB), as the single-output bin cap is (10 dB).
    #[serde(default = "default_max_band_growth_db")]
    pub max_band_growth_db: f64,
    /// Pulses in a row at one level with the waveform error rising that end
    /// the run (`divergence`).
    pub divergence_pulses: u32,
    /// The law call budget, misses and the run's pulse cap.
    pub law: LawGuards,
    /// A drive whose rms at one level grows past this many times its rms on
    /// that level's first pulse ends the run (`divergence`).
    #[serde(default = "default_divergence_drive_ratio")]
    pub divergence_drive_ratio: f64,
    /// A control's abort line: its response's peak over its target's peak at
    /// the level, dB, predicted before a fire and measured after it. `None` is
    /// the default for the target kind: twice the classical standard's
    /// amplitude tolerance for a classical pulse (the single-output overshoot
    /// abort), 6 dB for an SRS target (the single-output SRS abort, applied to
    /// the synthesized waveform's peak).
    #[serde(default)]
    pub control_abort_db: Option<f64>,
    /// The seed's predicted response at every control may be at most this many
    /// times its target's peak at the first level.
    #[serde(default = "default_first_drive_ratio")]
    pub first_drive_ratio: f64,
    /// A control's peak velocity, in/s, predicted or measured, over this ends
    /// the run.
    #[serde(default)]
    pub max_velocity_ips: Option<f64>,
    /// A control's peak-to-peak displacement, in, predicted or measured, over
    /// this ends the run.
    #[serde(default)]
    pub max_displacement_in: Option<f64>,
}

fn default_max_band_growth_db() -> f64 {
    10.0
}

fn default_divergence_drive_ratio() -> f64 {
    1.3
}

fn default_first_drive_ratio() -> f64 {
    2.0
}

/// A drive band below this fraction of the drive's largest band does not count
/// toward the growth cap.
pub const GROWTH_BAND_FLOOR: f64 = 1e-2;

impl Default for GuardPlan {
    fn default() -> Self {
        GuardPlan {
            max_growth_db: 6.0,
            max_band_growth_db: default_max_band_growth_db(),
            divergence_pulses: 3,
            law: LawGuards::default(),
            divergence_drive_ratio: default_divergence_drive_ratio(),
            control_abort_db: None,
            first_drive_ratio: default_first_drive_ratio(),
            max_velocity_ips: None,
            max_displacement_in: None,
        }
    }
}

/// A MIMO Shock run's configuration, as the run service hands it to the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MimoShockConfig {
    /// The rate requested of the station, Hz. The run adopts the rate the
    /// station runs at and re-derives the grid from it.
    pub fs_hz: f64,
    /// Samples in every record: a power of two.
    pub record_len: usize,
    /// The band the law corrects, Hz.
    pub band_lo_hz: f64,
    pub band_hi_hz: f64,
    /// The control channels, by table `index` (enabled Control rows), in the
    /// order the target lists them.
    pub controls: Vec<u32>,
    /// The drives, in the order a law's drive records are in.
    pub drives: Vec<DriveChannel>,
    /// The limit channels.
    pub limits: Vec<LimitChannel>,
    /// The full-level target, already synthesized.
    pub target: Target,
    pub timing: Timing,
    pub ladder: LadderPlan,
    pub identification: IdentificationPlan,
    pub guards: GuardPlan,
    /// The least time between pulse starts, s.
    pub min_spacing_s: f64,
}

impl MimoShockConfig {
    /// Check sizes, channels, drive limits, the ladder, the target's shape and
    /// the guards. Rules as [`crate::Setup::validate`] names them; the plant is
    /// not known until identification.
    pub fn validate(&self) -> Result<(), SetupError> {
        use SetupRule as R;
        let e = SetupError::new;
        let (nc, nd, len) = (self.controls.len(), self.drives.len(), self.record_len);
        if nc == 0 || nc > MAX_CONTROLS {
            return Err(e(
                R::ControlsOutOfRange,
                format!("{nc} control channels; a run controls 1 to {MAX_CONTROLS}"),
            ));
        }
        if nd == 0 || nd > MAX_DRIVES {
            return Err(e(
                R::DrivesOutOfRange,
                format!("{nd} drives; a run commands 1 to {MAX_DRIVES}"),
            ));
        }
        if !len.is_power_of_two() || !(MIN_RECORD_LEN..=MAX_RECORD_LEN).contains(&len) {
            return Err(e(
                R::RecordLenInvalid,
                format!(
                    "a record of {len} samples; a record is a power of two from \
                     {MIN_RECORD_LEN} to {MAX_RECORD_LEN}"
                ),
            ));
        }
        if !(self.fs_hz.is_finite() && self.fs_hz > 0.0) {
            return Err(e(R::RateInvalid, format!("a rate of {} Hz", self.fs_hz)));
        }
        let mut seen: Vec<u32> = self.controls.clone();
        seen.extend(self.drives.iter().map(|d| d.channel));
        seen.extend(self.limits.iter().map(|l| l.channel));
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        if sorted.windows(2).any(|w| w[0] == w[1]) {
            return Err(e(
                R::DuplicateChannel,
                "a channel is named twice among the controls, drives and limits".to_string(),
            ));
        }
        for d in &self.drives {
            if !(d.ceiling_v.is_finite()
                && d.ceiling_v > 0.0
                && d.abort_v.is_finite()
                && d.abort_v > d.ceiling_v)
            {
                return Err(e(
                    R::DriveCeiling,
                    format!(
                        "drive {} has a ceiling of {} V and an abort of {} V; the ceiling is \
                         above 0 and the abort above the ceiling",
                        d.channel, d.ceiling_v, d.abort_v
                    ),
                ));
            }
        }
        let l = &self.ladder.levels_db;
        if l.is_empty()
            || !l.iter().all(|x| x.is_finite())
            || l.windows(2).any(|w| w[1] <= w[0])
            || l.last() != Some(&0.0)
            || self.ladder.max_pulses_per_level == 0
        {
            return Err(e(
                R::Ladder,
                format!("a ladder of {l:?} dB; it ascends and ends at 0 dB"),
            ));
        }
        if self.target.waveforms().len() != nc * len {
            return Err(e(
                R::TargetShape,
                format!(
                    "{} target waveform values; {nc} controls of {len} samples need {}",
                    self.target.waveforms().len(),
                    nc * len
                ),
            ));
        }
        if let Timing::Hold(h) = &self.timing {
            if h.delays_s.len() != nc {
                return Err(e(
                    R::Timing,
                    format!("{} hold delays for {nc} controls", h.delays_s.len()),
                ));
            }
        }
        self.guards
            .law
            .validate()
            .map_err(|m| e(R::Ladder, m))
            .and_then(|_| {
                if self.guards.max_growth_db.is_finite() && self.guards.max_growth_db > 0.0 {
                    Ok(())
                } else {
                    Err(e(
                        R::DriveCeiling,
                        format!(
                            "a drive growth cap of {} dB; above 0",
                            self.guards.max_growth_db
                        ),
                    ))
                }
            })
    }
}

/// Where a run is. Wire names as Classical Shock's states and phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Configured; nothing open.
    Idle,
    /// The station is open and gated; nothing driven.
    Armed,
    /// The identification shot is playing or being estimated.
    Identifying,
    /// Identification done; nothing fires until the run proceeds.
    AwaitingReview,
    /// Between pulses: waiting out the time to the next pulse.
    Waiting,
    /// A pulse being brought up and playing.
    Firing,
    /// The pulse in flight finishes; nothing more fires.
    Stopping,
    /// Ended; [`EndOutcome`] says how.
    Finished,
}

impl RunState {
    /// The wire `phase`: `idle`, `armed`, `sysid`, `awaiting_review`,
    /// `waiting`, `firing`, `stopping` or `finished`.
    pub fn phase(self) -> &'static str {
        match self {
            RunState::Idle => "idle",
            RunState::Armed => "armed",
            RunState::Identifying => "sysid",
            RunState::AwaitingReview => "awaiting_review",
            RunState::Waiting => "waiting",
            RunState::Firing => "firing",
            RunState::Stopping => "stopping",
            RunState::Finished => "finished",
        }
    }
}

/// How a run ended. Wire names as Classical Shock's `end.outcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndOutcome {
    Completed,
    OperatorStop,
    OperatorAbort,
    SafeStop,
    Aborted,
    NotConverged,
    Held,
    LawFault,
    Fault,
}

/// One control's line in a pulse's report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ControlReport {
    /// The control's table `index`.
    pub channel: u32,
    /// The core's judgement, as the law was given it.
    pub verdict: ControlVerdict,
    /// The measured peak, EU.
    pub peak: f64,
    /// The waveform error after the delay is removed, dB (for an SRS target,
    /// against the synthesized waveform).
    pub waveform_error_db: f64,
    /// Peak velocity, in/s, and peak-to-peak displacement, in.
    pub velocity_ips: f64,
    pub displacement_pk_pk_in: f64,
    /// The control's delay was inside its timing window; a control whose
    /// delay was not is judged failing for it.
    pub timing_ok: bool,
    /// The worst signed error, dB, positive an over-test (the SRS deviation
    /// above the target, or the peak over the target's peak).
    pub over_test_db: f64,
    /// The pulse was the right way up; an upside-down pulse fails.
    pub polarity_ok: bool,
    /// The effective duration over the target's (SRS) and whether it is in
    /// its window; an SRS control out of it fails.
    pub duration_ratio: Option<f64>,
    pub duration_ok: bool,
    /// Reported, not judged: the energy over the target's at the level, dB,
    /// and the rms duration over the target's.
    pub energy_db: f64,
    pub rms_duration_ratio: f64,
}

/// One fired pulse's report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseReport {
    /// Every fired pulse, from 0.
    pub index: u64,
    /// The level the ladder asked for, dB.
    pub scheduled_db: f64,
    /// The level fired, dB.
    pub level_db: f64,
    /// Each drive's scale from its ceiling or a limit (1 when none).
    pub limit_scale: Vec<f64>,
    /// Each drive's peak as played, volts.
    pub drive_vpk: Vec<f64>,
    /// Each control's judgement, in control order.
    pub controls: Vec<ControlReport>,
    /// Every control passed.
    pub passed: bool,
    /// For an SRS target, the energy-weighted SRS error over every control,
    /// dB: shown, never judged.
    pub global_srs_error_db: Option<f64>,
    /// The pulse's start, s from arm.
    pub at_s: f64,
    /// The drive this pulse played came from a law call that was used.
    pub law_used: bool,
    /// Why the law's last answer was not used, when it was not.
    pub law_miss: Option<String>,
    /// The values the law reported with the drive this pulse played.
    pub law_values: Vec<LawValue>,
    /// Each drive's peak predicted response of the controls over the abort
    /// line's reference (the target's peak at the level), before the fire.
    pub predicted_peak_ratio: Vec<f64>,
    /// How the run ended, when this pulse ended it.
    pub end: Option<RunEnd>,
    /// The worst signed error over every control, dB, positive an over-test.
    pub over_test_db: f64,
    /// The spread of the controls' delays, s (latest less earliest).
    pub arrival_spread_s: f64,
    /// Things reported, not judged: an arrival spread over the timing window
    /// (`free` timing), an energy or rms duration outside its stated tolerance.
    pub warnings: Vec<String>,
    /// When the ladder stepped up after this pulse with a halved step, why.
    pub step_note: Option<StepNote>,
}

/// How a run ended and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEnd {
    pub outcome: EndOutcome,
    /// A stable reason: `completed`, `not_converged`, `divergence`,
    /// `drive_abort`, `predicted_abort`, `control_abort`, `limit_abort`,
    /// `kinematics`, `max_pulses`, `low_coherence`, `reference_out_of_band`,
    /// a law fault's reason (`panicked`, `over_budget`, `wrong_length`, ...),
    /// `operator`, `cancelled` or `fault`.
    pub reason: String,
    /// The facts, in words.
    pub detail: String,
}

/// Why an engine call was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// The configuration breaks a rule.
    Config(SetupError),
    /// The call is not allowed in this state.
    WrongState(RunState),
    /// This version does not run this step; the string names it.
    NotBuilt(&'static str),
    /// The station could not be armed or a shot failed; the run ended.
    Station(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Config(e) => write!(f, "the configuration is refused: {e}"),
            EngineError::WrongState(s) => write!(f, "not allowed while {}", s.phase()),
            EngineError::NotBuilt(step) => {
                write!(f, "MIMO Shock cannot run its {step} step in this version")
            }
            EngineError::Station(m) => write!(f, "the station failed: {m}"),
        }
    }
}

impl std::error::Error for EngineError {}

// ---------------------------------------------------------------------------
// The shot path
// ---------------------------------------------------------------------------

/// Why a shot did not deliver its rows.
#[derive(Debug, Clone, PartialEq)]
pub enum ShotError {
    /// The operator's cancel flag stopped the play.
    Cancelled,
    /// The station failed.
    Failed(String),
}

/// What the engine needs of a station: arm once, fire whole records on every
/// drive at once, stop. The host firer over any backend is one
/// ([`BackendPulseFirer`]); tests may give their own.
pub trait ShotPath: Send {
    /// Open and configure the station at `fs_hz`, park it, and return the rate
    /// it programmed.
    fn arm(&mut self, fs_hz: f64) -> Result<f64, String>;
    /// Quiet frames played before and after every record.
    fn set_quiet_frames(&mut self, frames: usize);
    /// Fire one record, `n_drive` samples a frame, and return every input row.
    fn fire<'a>(
        &mut self,
        drive: &[f64],
        n_drive: usize,
        cancel: Option<&'a AtomicBool>,
        hooks: Option<&'a mut dyn ShotHooks>,
    ) -> Result<PulseRows, ShotError>;
    /// Park the station and release it.
    fn stop(&mut self) -> Result<(), String>;
    /// Take every drive down at once.
    fn emergency_stop(&mut self) -> Result<(), String>;
}

/// The host firer, armed at the rate the run asks for.
struct FirerShot<B: Backend + Send> {
    firer: Option<BackendPulseFirer<B>>,
}

impl<B: Backend + Send> ShotPath for FirerShot<B> {
    fn arm(&mut self, fs_hz: f64) -> Result<f64, String> {
        let f = self.firer.take().ok_or("the station is already armed")?;
        let mut f = f.with_requested_rate(fs_hz);
        f.arm().map_err(|e| e.to_string())?;
        let fs = f
            .actual_fs_hz()
            .ok_or("the station did not say what rate it runs at")?;
        self.firer = Some(f);
        Ok(fs)
    }

    fn set_quiet_frames(&mut self, frames: usize) {
        if let Some(f) = self.firer.as_mut() {
            f.set_quiet_frames(frames);
        }
    }

    fn fire<'a>(
        &mut self,
        drive: &[f64],
        n_drive: usize,
        cancel: Option<&'a AtomicBool>,
        hooks: Option<&'a mut dyn ShotHooks>,
    ) -> Result<PulseRows, ShotError> {
        let f = self
            .firer
            .as_mut()
            .ok_or(ShotError::Failed("the station is not armed".into()))?;
        let mut ctl = FireControl {
            cancel,
            observer: None,
            hooks,
        };
        f.fire_rows(drive, n_drive, &mut ctl).map_err(|e| match e {
            ServiceError::Cancelled { .. } => ShotError::Cancelled,
            other => ShotError::Failed(other.to_string()),
        })
    }

    fn stop(&mut self) -> Result<(), String> {
        let r = match self.firer.as_mut() {
            Some(f) => f.stop().map_err(|e| e.to_string()),
            None => Ok(()),
        };
        // Released: the backend goes with the firer.
        self.firer = None;
        r
    }

    fn emergency_stop(&mut self) -> Result<(), String> {
        let r = match self.firer.as_mut() {
            Some(f) => f.emergency_stop().map_err(|e| e.to_string()),
            None => Ok(()),
        };
        self.firer = None;
        r
    }
}

/// The time between pulse starts: the shot waits in its `ready` hook, so only
/// the start lands on the floor.
struct Pace<'a> {
    last_start: &'a mut Option<Instant>,
    spacing_s: f64,
}

impl ShotHooks for Pace<'_> {
    fn ready(&mut self, _prepared: Prepared) -> bool {
        if let Some(t) = *self.last_start {
            let due = t + Duration::from_secs_f64(self.spacing_s.max(0.0));
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        true
    }

    fn started(&mut self, _start_s: f64) {
        *self.last_start = Some(Instant::now());
    }
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// What identification found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentificationReport {
    /// The rate the station runs at, Hz, and the grid's line spacing.
    pub fs_hz: f64,
    pub df_hz: f64,
    /// Lines on the grid.
    pub lines: usize,
    /// Realizations measured.
    pub realizations: usize,
    /// Each drive's rms and peak in the shot, volts.
    pub drive_vrms: Vec<f64>,
    pub drive_vpk: Vec<f64>,
    /// Each control's band-mean multiple coherence.
    pub mean_coherence: Vec<f64>,
    /// Each control's target energy on the trusted lines, 0 to 1.
    pub coverage: Vec<f64>,
    /// Trusted lines.
    pub trusted_lines: usize,
    /// The largest condition number of the plant on a trusted line.
    pub max_condition: f64,
    /// Each control's peak response in the shot, EU.
    pub response_pk: Vec<f64>,
}

/// What the core found before the first pulse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeedReport {
    /// Each control's reachable fraction: the share of its target energy the
    /// drives can reach at all (below one only with more controls than drives
    /// or a rank-deficient plant).
    pub reachable_fraction: Vec<f64>,
    /// Each control's floor: for a waveform target, the waveform error of the
    /// best reachable response, dB; for an SRS target, the largest SRS
    /// deviation of the best reachable response from the target, dB.
    pub floor_db: Vec<f64>,
    /// The seed's predicted peak at each control over its target's peak at
    /// the first level.
    pub predicted_peak_ratio: Vec<f64>,
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// The minimum mean coherence of a control over the band before the run holds
/// (the single-output classical identification's floor).
pub const MIN_MEAN_COHERENCE: f64 = 0.8;

/// The share of a control's target energy that must lie on trusted lines
/// before the run holds (the single-output classical rule).
pub const MIN_BAND_ENERGY: f64 = 0.9;

/// Identification plays each control's response this far under its target's
/// rms over the record, dB (the single-output identification level rule).
pub const IDENTIFICATION_BELOW_DB: f64 = 20.0;

/// Realizations in the probe shot that sets the identification level.
const PROBE_REALIZATIONS: usize = 2;

/// The value a law may report with its answer: the worst over-test it
/// predicts at any control on the pulse it built, dB over the target's peak at
/// the next level. The core holds it to the controls' abort line.
pub const LAW_PREDICTED_OVER_TEST: &str = "predicted_over_test_db";

/// Predicted peaks before a fire: each control's over its target's peak at
/// the level, each limit's in its units, and a kinematic breach in words.
type Predicted = (Vec<f64>, Vec<f64>, Option<String>);

/// A law's answer, timed and caught.
enum Answer {
    Used(PulseOutputs),
    Missed(LawFault, Option<PulseOutputs>),
    Panicked(String),
}

/// The run engine. Built from a checked configuration and the law's factory.
pub struct MimoShockEngine {
    config: MimoShockConfig,
    law: Arc<dyn MimoShockLawFactory>,
    state: RunState,
    pulses: Vec<PulseReport>,
    target_spec: Option<TargetSpec>,
    seed: u64,
    clock: Arc<dyn LawClock>,
    pacing: bool,
    shot: Option<Box<dyn ShotPath>>,
    fs_hz: f64,
    quiet: usize,
    built: Option<BuiltTarget>,
    /// H of every control and limit row, controls first.
    plant_all: Option<MatrixStack>,
    ident: Option<IdentificationReport>,
    seed_report: Option<SeedReport>,
    setup: Option<Setup>,
    manifest: Option<LawManifest>,
    running: Option<Box<dyn MimoShockLaw + Send>>,
    ladder: Option<Ladder>,
    misses: MissCounter,
    watch: DivergenceWatch,
    bands: Vec<(usize, usize)>,
    /// The drive the next pulse plays, channel-major, at `next_level`.
    next_drive: Vec<f64>,
    next_level: f64,
    next_used: bool,
    next_miss: Option<String>,
    next_values: Vec<LawValue>,
    /// The drives the last pulse played and its level.
    last_played: Option<(Vec<f64>, f64)>,
    last_start: Option<Instant>,
    records: RunRecord,
    end: Option<RunEnd>,
}

impl MimoShockEngine {
    /// An engine for `config`, refused when the configuration breaks a rule.
    pub fn new(
        config: MimoShockConfig,
        law: Arc<dyn MimoShockLawFactory>,
    ) -> Result<Self, EngineError> {
        config.validate().map_err(EngineError::Config)?;
        let misses = MissCounter::new(config.guards.law.max_misses);
        let watch = DivergenceWatch::new(
            config.guards.divergence_pulses,
            config.guards.divergence_drive_ratio,
        );
        let fs_hz = config.fs_hz;
        Ok(MimoShockEngine {
            config,
            law,
            state: RunState::Idle,
            pulses: Vec::new(),
            target_spec: None,
            seed: 0x5eed_5eed,
            clock: Arc::new(MonotonicClock::new()),
            pacing: true,
            shot: None,
            fs_hz,
            quiet: 0,
            built: None,
            plant_all: None,
            ident: None,
            seed_report: None,
            setup: None,
            manifest: None,
            running: None,
            ladder: None,
            misses,
            watch,
            bands: Vec::new(),
            next_drive: Vec::new(),
            next_level: 0.0,
            next_used: false,
            next_miss: None,
            next_values: Vec::new(),
            last_played: None,
            last_start: None,
            records: RunRecord::default(),
            end: None,
        })
    }

    /// The test as specified per control, synthesized at the rate the station
    /// adopts (in place of the configuration's `target`, which is then
    /// replaced). Without it the configuration's target is used as given and
    /// judged sample by sample, and a station that programs another rate than
    /// the one asked is refused.
    pub fn with_target_spec(mut self, spec: TargetSpec) -> Self {
        self.target_spec = Some(spec);
        self
    }

    /// The identification shot's random seed.
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// The clock law calls are timed on (the default is the monotonic clock).
    pub fn with_clock(mut self, clock: Arc<dyn LawClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Whether pulses are paced to `min_spacing_s` (the default). A simulator
    /// run may turn it off.
    pub fn with_pacing(mut self, pacing: bool) -> Self {
        self.pacing = pacing;
        self
    }

    pub fn config(&self) -> &MimoShockConfig {
        &self.config
    }

    pub fn state(&self) -> RunState {
        self.state
    }

    /// The run's law as its records name it.
    pub fn law(&self) -> LawDescriptor {
        self.law.describe()
    }

    /// Every pulse fired so far.
    pub fn pulses(&self) -> &[PulseReport] {
        &self.pulses
    }

    /// The rate the run runs at, Hz (the station's, once armed).
    pub fn fs_hz(&self) -> f64 {
        self.fs_hz
    }

    /// The pulse grid's line spacing, Hz.
    pub fn df_hz(&self) -> f64 {
        self.fs_hz / self.config.record_len as f64
    }

    /// The target as built at the adopted rate.
    pub fn target(&self) -> Option<&Target> {
        self.built.as_ref().map(|b| &b.target)
    }

    /// What identification found.
    pub fn identification(&self) -> Option<&IdentificationReport> {
        self.ident.as_ref()
    }

    /// The law's setup, after identification.
    pub fn setup(&self) -> Option<&Setup> {
        self.setup.as_ref()
    }

    /// What the core found before the first pulse.
    pub fn seed_report(&self) -> Option<&SeedReport> {
        self.seed_report.as_ref()
    }

    /// How the run ended, once it has.
    pub fn end(&self) -> Option<&RunEnd> {
        self.end.as_ref()
    }

    /// The run's records.
    pub fn records(&self) -> &RunRecord {
        &self.records
    }

    /// Open the station `backend` with `table` at the configured rate, adopt
    /// the rate it programmed, re-derive the grid and the targets at it and
    /// pass the arm gate for `station`. Nothing is driven.
    pub fn arm(
        &mut self,
        backend: Box<dyn Backend + Send>,
        table: ChannelTable,
        station: StationClass,
    ) -> Result<(), EngineError> {
        if self.state != RunState::Idle {
            return Err(EngineError::WrongState(self.state));
        }
        self.check_table(&table)?;
        let firer = BackendPulseFirer::new(backend, table, station)
            .map_err(|e| EngineError::Station(e.to_string()))?;
        self.arm_with(Box::new(FirerShot { firer: Some(firer) }))
    }

    /// Arm on a shot path of the caller's (see [`Self::arm`]).
    pub fn arm_with(&mut self, mut shot: Box<dyn ShotPath>) -> Result<(), EngineError> {
        if self.state != RunState::Idle {
            return Err(EngineError::WrongState(self.state));
        }
        let fs = shot.arm(self.config.fs_hz).map_err(EngineError::Station)?;
        if !(fs.is_finite() && fs > 0.0) {
            let _ = shot.stop();
            return Err(EngineError::Station(format!("the station runs at {fs} Hz")));
        }
        let n = self.config.record_len;
        let built = match &self.target_spec {
            Some(spec) => spec.build(fs, n),
            None if (fs - self.config.fs_hz).abs() <= 1e-9 * fs => Ok(BuiltTarget {
                target: self.config.target.clone(),
                judging: Judging::Envelope {
                    tol_frac_of_peak: DEFAULT_ENVELOPE_TOL,
                },
            }),
            None => Err(SetupError::new(
                SetupRule::RateInvalid,
                format!(
                    "the station runs at {fs} Hz, not the {} Hz asked, and a target given \
                     sample by sample cannot be re-derived at it",
                    self.config.fs_hz
                ),
            )),
        };
        let built = match built {
            Ok(b) => b,
            Err(e) => {
                let _ = shot.stop();
                return Err(EngineError::Config(e));
            }
        };
        if built.target.waveforms().len() != self.config.controls.len() * n {
            let _ = shot.stop();
            return Err(EngineError::Config(SetupError::new(
                SetupRule::TargetShape,
                "the target spec does not describe one waveform per control",
            )));
        }
        self.fs_hz = fs;
        self.quiet = (n / 16).max(64);
        shot.set_quiet_frames(self.quiet);
        self.bands = bands(fs, n, self.config.band_lo_hz, self.config.band_hi_hz);
        self.built = Some(built);
        self.shot = Some(shot);
        self.state = RunState::Armed;
        Ok(())
    }

    fn check_table(&self, table: &ChannelTable) -> Result<(), EngineError> {
        use hertzforge_channel_table::ChannelRole as Role;
        let role_of = |ch: u32| {
            table
                .channels
                .iter()
                .find(|c| c.index == ch && c.enabled)
                .map(|c| c.role)
        };
        let err = |m: String| EngineError::Config(SetupError::new(SetupRule::DuplicateChannel, m));
        for &c in &self.config.controls {
            if role_of(c) != Some(Role::Control) {
                return Err(err(format!("control {c} is not an enabled Control row")));
            }
        }
        for d in &self.config.drives {
            if role_of(d.channel) != Some(Role::Drive) {
                return Err(err(format!(
                    "drive {} is not an enabled Drive row",
                    d.channel
                )));
            }
        }
        let drive_rows = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == Role::Drive)
            .map(|c| c.index)
            .collect::<Vec<_>>();
        let ours: Vec<u32> = self.config.drives.iter().map(|d| d.channel).collect();
        if drive_rows != ours {
            return Err(err(format!(
                "the table's drive rows {drive_rows:?} are not the run's drives {ours:?} in order"
            )));
        }
        for l in &self.config.limits {
            if !matches!(
                role_of(l.channel),
                Some(Role::Monitor) | Some(Role::Control)
            ) {
                return Err(err(format!(
                    "limit {} is not an enabled input row",
                    l.channel
                )));
            }
        }
        Ok(())
    }

    /// End the run: stop and release the station, then record the end.
    fn finish(&mut self, outcome: EndOutcome, reason: &str, detail: String) -> RunEnd {
        if let Some(mut shot) = self.shot.take() {
            let _ = shot.stop();
        }
        self.running = None;
        let end = RunEnd {
            outcome,
            reason: reason.into(),
            detail,
        };
        self.state = RunState::Finished;
        self.end = Some(end.clone());
        end
    }

    fn rows_of_channels(
        &self,
        rows: &PulseRows,
        channels: &[u32],
    ) -> Result<Vec<Vec<f64>>, String> {
        channels
            .iter()
            .map(|&ch| {
                rows.row_of_channel(ch)
                    .map(|r| r[rows.quiet_frames.min(r.len())..].to_vec())
                    .ok_or_else(|| format!("channel {ch} was not measured"))
            })
            .collect()
    }

    fn input_channels(&self) -> Vec<u32> {
        let mut v = self.config.controls.clone();
        v.extend(self.config.limits.iter().map(|l| l.channel));
        v
    }

    fn fire_shot(
        &mut self,
        drive_rows: &[Vec<f64>],
        cancel: Option<&AtomicBool>,
    ) -> Result<PulseRows, ShotError> {
        let n_d = drive_rows.len();
        let len = drive_rows.first().map_or(0, |r| r.len());
        let mut inter = vec![0.0; n_d * len];
        for (d, row) in drive_rows.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                inter[j * n_d + d] = *v;
            }
        }
        let spacing = self.config.min_spacing_s;
        let pacing = self.pacing;
        let shot = self
            .shot
            .as_mut()
            .ok_or(ShotError::Failed("the station is not armed".into()))?;
        let mut pace = Pace {
            last_start: &mut self.last_start,
            spacing_s: spacing,
        };
        let hooks: Option<&mut dyn ShotHooks> = if pacing { Some(&mut pace) } else { None };
        shot.fire(&inter, n_d, cancel, hooks)
    }

    /// Play the identification shot and form the plant on the pulse grid. A
    /// hold (low coherence, a target outside the trusted band) ends the run
    /// `held` with its reason.
    pub fn identify(&mut self) -> Result<(), EngineError> {
        if self.state != RunState::Armed {
            return Err(EngineError::WrongState(self.state));
        }
        self.state = RunState::Identifying;
        let n = self.config.record_len;
        let (nc, nd) = (self.config.controls.len(), self.config.drives.len());
        let fs = self.fs_hz;
        let plan = self.config.identification.clone();
        let band = band_lines(fs, n, self.config.band_lo_hz, self.config.band_hi_hz);
        let realizations = (plan.frames as usize).max(10 * nd);
        let caps: Vec<f64> = self.config.drives.iter().map(|d| d.ceiling_v).collect();
        let target_rms: Vec<f64> = {
            let w = self.built.as_ref().expect("armed").target.waveforms();
            (0..nc)
                .map(|c| {
                    let r = &w[c * n..(c + 1) * n];
                    (r.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt()
                })
                .collect()
        };
        // The level: a probe at a tenth of the most, then scaled so the control
        // closest to its mark reaches it.
        let probe_vrms = 0.1 * plan.drive_vrms;
        let (probe, probe_out) = match self.id_shot(n, &band, probe_vrms, &caps, PROBE_REALIZATIONS)
        {
            Ok(x) => x,
            Err(e) => return e,
        };
        let _ = probe;
        let used = 2 * PROBE_REALIZATIONS * n;
        let want = 10f64.powf(-IDENTIFICATION_BELOW_DB / 20.0);
        let scale = (0..nc)
            .map(|c| {
                let r = &probe_out[c][n..used.min(probe_out[c].len())];
                let y = (r.iter().map(|v| v * v).sum::<f64>() / r.len().max(1) as f64).sqrt();
                if y > 0.0 {
                    want * target_rms[c] / y
                } else {
                    f64::INFINITY
                }
            })
            .fold(f64::INFINITY, f64::min);
        let vrms = (probe_vrms * scale).min(plan.drive_vrms);
        let (drives, outputs) = match self.id_shot(n, &band, vrms, &caps, realizations) {
            Ok(x) => x,
            Err(e) => return e,
        };
        let est = match estimate(&drives, &outputs, nc, n, realizations) {
            Ok(e) => e,
            Err(e) => {
                self.finish(EndOutcome::Fault, "fault", e.clone());
                return Err(EngineError::Station(e));
            }
        };
        let lines = n / 2 + 1;
        // H is the plant only where the shot excited it: elsewhere the
        // estimate is noise over a drive spectrum of nothing. Zero there, so a
        // prediction sees no plant outside the band (as MIMO Random's does).
        let mut est = est;
        for (f, on) in band.iter().enumerate() {
            if !on {
                est.frf.line_mut(f).fill(hertzforge_sdm::C64::new(0.0, 0.0));
            }
        }
        let floor = plan.coherence_floor;
        let trusted: Vec<bool> = (0..lines)
            .map(|f| band[f] && (0..nc).all(|c| est.coherence[c * lines + f] >= floor))
            .collect();
        let band_n = band.iter().filter(|b| **b).count().max(1) as f64;
        let mean_coherence: Vec<f64> = (0..nc)
            .map(|c| {
                (0..lines)
                    .filter(|&f| band[f])
                    .map(|f| est.coherence[c * lines + f])
                    .sum::<f64>()
                    / band_n
            })
            .collect();
        let target = self.built.as_ref().expect("armed").target.clone();
        let coverage: Vec<f64> = (0..nc)
            .map(|c| {
                let r = rfft(&target.waveforms()[c * n..(c + 1) * n]).unwrap_or_default();
                let (mut on, mut all) = (0.0, 0.0);
                for (f, v) in r.iter().enumerate() {
                    all += v.norm_sqr();
                    if trusted[f] {
                        on += v.norm_sqr();
                    }
                }
                if all > 0.0 {
                    on / all
                } else {
                    0.0
                }
            })
            .collect();
        let max_condition = est
            .condition
            .iter()
            .zip(&trusted)
            .filter(|(_, t)| **t)
            .fold(0.0f64, |a, (k, _)| a.max(*k));
        let row_stats = |r: &Vec<f64>| -> (f64, f64) {
            let used = &r[..(2 * realizations * n).min(r.len())];
            let rms = (used.iter().map(|v| v * v).sum::<f64>() / used.len().max(1) as f64).sqrt();
            (rms, peak(used))
        };
        let report = IdentificationReport {
            fs_hz: fs,
            df_hz: fs / n as f64,
            lines,
            realizations,
            drive_vrms: drives.iter().map(|r| row_stats(r).0).collect(),
            drive_vpk: drives.iter().map(|r| row_stats(r).1).collect(),
            mean_coherence: mean_coherence.clone(),
            coverage: coverage.clone(),
            trusted_lines: trusted.iter().filter(|t| **t).count(),
            max_condition,
            response_pk: outputs[..nc].iter().map(|r| peak(r)).collect(),
        };
        let plant = PlantStack {
            frf: rows_of(&est.frf, 0, nc),
            coherence: est.coherence[..nc * lines].to_vec(),
            trusted,
        };
        self.plant_all = Some(est.frf);
        self.ident = Some(report);
        let setup = Setup {
            fs_hz: fs,
            record_len: n,
            n_drives: nd,
            n_controls: nc,
            band_lo_hz: self.config.band_lo_hz,
            band_hi_hz: self.config.band_hi_hz,
            target,
            timing: self.config.timing.clone(),
            plant,
            drive_ceiling_v: self.config.drives.iter().map(|d| d.ceiling_v).collect(),
            ladder_db: self.config.ladder.levels_db.clone(),
        };
        self.setup = Some(setup);
        if let Some((c, m)) = mean_coherence
            .iter()
            .enumerate()
            .find(|(_, m)| **m < MIN_MEAN_COHERENCE)
        {
            self.finish(
                EndOutcome::Held,
                "low_coherence",
                format!(
                    "control {} has a band-mean coherence of {m:.3}, under {MIN_MEAN_COHERENCE}",
                    self.config.controls[c]
                ),
            );
            return Ok(());
        }
        if let Some((c, v)) = coverage
            .iter()
            .enumerate()
            .find(|(_, v)| **v < MIN_BAND_ENERGY)
        {
            self.finish(
                EndOutcome::Held,
                "reference_out_of_band",
                format!(
                    "{:.1} % of control {}'s target lies on trusted lines, under {:.0} %",
                    100.0 * v,
                    self.config.controls[c],
                    100.0 * MIN_BAND_ENERGY
                ),
            );
            return Ok(());
        }
        self.state = RunState::AwaitingReview;
        Ok(())
    }

    /// One identification shot at `vrms` on every drive: the drives and every
    /// control and limit row. A limit over its abort ends the run.
    #[allow(clippy::type_complexity)]
    fn id_shot(
        &mut self,
        n: usize,
        band: &[bool],
        vrms: f64,
        caps: &[f64],
        realizations: usize,
    ) -> Result<(Vec<Vec<f64>>, Vec<Vec<f64>>), Result<(), EngineError>> {
        let nd = caps.len();
        let nc = self.config.controls.len();
        let drives = match excitation(n, band, &vec![vrms; nd], caps, realizations, self.seed) {
            Ok(d) => d,
            Err(e) => {
                self.finish(EndOutcome::Fault, "fault", e.clone());
                return Err(Err(EngineError::Station(e)));
            }
        };
        let rows = match self.fire_shot(&drives, None) {
            Ok(r) => r,
            Err(e) => {
                let detail = format!("{e:?}");
                self.finish(EndOutcome::Fault, "fault", detail.clone());
                return Err(Err(EngineError::Station(detail)));
            }
        };
        let outputs = match self.rows_of_channels(&rows, &self.input_channels()) {
            Ok(o) => o,
            Err(e) => {
                self.finish(EndOutcome::Fault, "fault", e.clone());
                return Err(Err(EngineError::Station(e)));
            }
        };
        let breach = self
            .config
            .limits
            .iter()
            .zip(&outputs[nc..])
            .find(|(l, r)| peak(r) > l.abort_peak)
            .map(|(l, r)| (l.channel, peak(r), l.abort_peak));
        if let Some((ch, pk, ab)) = breach {
            self.finish(
                EndOutcome::Aborted,
                "limit_abort",
                format!("limit {ch} measured {pk:.3} in identification, over its abort of {ab}"),
            );
            return Err(Ok(()));
        }
        Ok((drives, outputs))
    }

    /// The level scale of `db`.
    fn scale(db: f64) -> f64 {
        10f64.powf(db / 20.0)
    }

    /// Each control's abort line as a ratio over its target's peak at the
    /// level.
    fn control_abort_ratio(&self) -> f64 {
        let db = self.config.guards.control_abort_db.unwrap_or_else(|| {
            match &self.built.as_ref().expect("armed").judging {
                Judging::Classical { standard, .. } => {
                    20.0 * (1.0 + 2.0 * standard.params().amp_tol_frac_of_peak).log10()
                }
                Judging::Envelope { tol_frac_of_peak } => {
                    20.0 * (1.0 + 2.0 * tol_frac_of_peak).log10()
                }
                Judging::Srs => 6.0,
            }
        });
        Self::scale(db)
    }

    /// The upper tolerance the first pulse of a level is held to, dB: the SRS
    /// tolerance above the target, or the classical amplitude tolerance on the
    /// peak.
    fn upper_tolerance_db(&self) -> f64 {
        let built = self.built.as_ref().expect("armed");
        match (&built.judging, &built.target) {
            (Judging::Srs, Target::Srs(t)) => t.tol_hi_db,
            (Judging::Classical { standard, .. }, _) => {
                20.0 * (1.0 + standard.params().amp_tol_frac_of_peak).log10()
            }
            (Judging::Envelope { tol_frac_of_peak }, _) => 20.0 * (1.0 + tol_frac_of_peak).log10(),
            (Judging::Srs, Target::Waveform(_)) => 0.0,
        }
    }

    /// The target's peak at each control, full level.
    fn target_peaks(&self) -> Vec<f64> {
        let n = self.config.record_len;
        let w = self.built.as_ref().expect("armed").target.waveforms();
        (0..self.config.controls.len())
            .map(|c| peak(&w[c * n..(c + 1) * n]))
            .collect()
    }

    /// Call the law under the core's containment: caught, timed, its output
    /// checked against the setup and the manifest.
    fn call_law(&mut self, inputs: Option<&PulseInputs>, seed_level: f64) -> (Answer, u64) {
        let setup = self.setup.clone().expect("proceeded");
        let declared = self
            .manifest
            .as_ref()
            .map(|m| m.values.clone())
            .unwrap_or_default();
        let budget = self
            .config
            .guards
            .law
            .call_budget_s(self.config.min_spacing_s);
        let law = self.running.as_mut().expect("built");
        let t0 = self.clock.now_s();
        let r = catch_unwind(AssertUnwindSafe(|| match inputs {
            None => law.seed(seed_level),
            Some(i) => law.iterate(i),
        }));
        let took = (self.clock.now_s() - t0).max(0.0);
        let us = (took * 1e6) as u64;
        let ans = match r {
            Err(p) => {
                let m = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic".into());
                Answer::Panicked(m)
            }
            Ok(Err(f)) => Answer::Missed(f, None),
            Ok(Ok(out)) => {
                if took > budget {
                    Answer::Missed(
                        LawFault::OverBudget {
                            took_s: took,
                            budget_s: budget,
                        },
                        Some(out),
                    )
                } else if let Err(e) = out.validate(&setup, &declared) {
                    let rule = match e.rule {
                        crate::pulse::OutputRule::WrongLength => GuardRule::WrongLength,
                        crate::pulse::OutputRule::NonFinite => GuardRule::NonFinite,
                        crate::pulse::OutputRule::DelaysShape => GuardRule::WrongLength,
                        crate::pulse::OutputRule::UndeclaredValue => GuardRule::UndeclaredValue,
                        crate::pulse::OutputRule::TooManyValues => GuardRule::TooManyValues,
                    };
                    Answer::Missed(LawFault::BadOutput(rule), Some(out))
                } else {
                    Answer::Used(out)
                }
            }
        };
        (ans, us)
    }

    /// The predicted peak of every control (over its target's peak at
    /// `level_db`) and every limit (in its units) for `drives`, and whether any
    /// control's predicted kinematics are over their limits.
    fn predicted(&self, drives: &[f64], level_db: f64) -> Result<Predicted, String> {
        let h = self.plant_all.as_ref().ok_or("no plant")?;
        let y = predict(h, drives, self.config.record_len)?;
        let nc = self.config.controls.len();
        let s = Self::scale(level_db);
        let pk = self.target_peaks();
        let ratios: Vec<f64> = (0..nc)
            .map(|c| {
                let r = s * pk[c];
                if r > 0.0 {
                    peak(&y[c]) / r
                } else {
                    f64::INFINITY
                }
            })
            .collect();
        let limits: Vec<f64> = y[nc..].iter().map(|r| peak(r)).collect();
        let kin = self.kinematics_breach(&y[..nc]);
        Ok((ratios, limits, kin))
    }

    fn kinematics_breach(&self, controls: &[Vec<f64>]) -> Option<String> {
        let g = &self.config.guards;
        for (c, y) in controls.iter().enumerate() {
            let k = pulse_kinematics(y, self.fs_hz);
            if let Some(v) = g.max_velocity_ips {
                if k.peak_velocity_ips > v {
                    return Some(format!(
                        "control {} reaches {:.3} in/s, over {v} in/s",
                        self.config.controls[c], k.peak_velocity_ips
                    ));
                }
            }
            if let Some(d) = g.max_displacement_in {
                if k.pk_pk_disp_in > d {
                    return Some(format!(
                        "control {} moves {:.4} in peak to peak, over {d} in",
                        self.config.controls[c], k.pk_pk_disp_in
                    ));
                }
            }
        }
        None
    }

    /// Why `drives` may not be fired at `level_db` by the predicted response,
    /// if they may not.
    fn predicted_breach(
        &self,
        drives: &[f64],
        level_db: f64,
    ) -> Result<(Vec<f64>, Option<String>), String> {
        let (ratios, limits, kin) = self.predicted(drives, level_db)?;
        let line = self.control_abort_ratio();
        if let Some((c, r)) = ratios.iter().enumerate().find(|(_, r)| **r > line) {
            return Ok((
                ratios.clone(),
                Some(format!(
                    "control {} is predicted at {:.2} dB over its target's peak; its abort line is \
                     {:.2} dB",
                    self.config.controls[c],
                    20.0 * r.log10(),
                    20.0 * line.log10()
                )),
            ));
        }
        for (l, (lim, pk)) in self.config.limits.iter().zip(&limits).enumerate() {
            let _ = l;
            if *pk > lim.abort_peak {
                return Ok((
                    ratios,
                    Some(format!(
                        "limit {} is predicted at {pk:.3}, over its abort of {}",
                        lim.channel, lim.abort_peak
                    )),
                ));
            }
        }
        Ok((ratios, kin))
    }

    /// Build and seed the law after identification. Each control's reachable
    /// fraction is computed and reported first.
    pub fn proceed(&mut self) -> Result<(), EngineError> {
        if self.state != RunState::AwaitingReview {
            return Err(EngineError::WrongState(self.state));
        }
        let setup = self.setup.clone().expect("identified");
        if let Err(e) = setup.validate() {
            self.finish(EndOutcome::Fault, "fault", e.to_string());
            return Err(EngineError::Config(e));
        }
        let manifest = self.law.manifest();
        if let Err(e) = check_law_accepts(&manifest, &setup) {
            self.finish(EndOutcome::LawFault, "target_not_accepted", e.to_string());
            return Err(EngineError::Config(e));
        }
        self.seed_report = Some(self.floors(&setup));
        self.records.setup = Some(setup.clone());
        self.records.law = Some(self.law.describe());
        let built = catch_unwind(AssertUnwindSafe(|| self.law.build(&setup)));
        let law = match built {
            Ok(Ok(l)) => l,
            Ok(Err(f)) => {
                self.finish(EndOutcome::LawFault, f.reason(), f.to_string());
                return Ok(());
            }
            Err(_) => {
                self.finish(
                    EndOutcome::LawFault,
                    "panicked",
                    "the law panicked while built".into(),
                );
                return Ok(());
            }
        };
        self.running = Some(law);
        self.manifest = Some(manifest);
        let lp = &self.config.ladder;
        let ladder = Ladder::new(
            lp.levels_db.clone(),
            lp.full_level_pulses,
            lp.max_pulses_per_level,
            self.upper_tolerance_db(),
        );
        let level = ladder.level_db();
        self.ladder = Some(ladder);
        let (ans, us) = self.call_law(None, level);
        let (miss, outputs) = match ans {
            Answer::Used(o) => {
                // The first drive's bound: its predicted response at every
                // control within `first_drive_ratio` of the target's peak.
                match self.predicted(&o.drives, level) {
                    Ok((ratios, _, _)) => {
                        if let Some(sr) = self.seed_report.as_mut() {
                            sr.predicted_peak_ratio = ratios.clone();
                        }
                        if ratios
                            .iter()
                            .any(|r| *r > self.config.guards.first_drive_ratio)
                        {
                            (Some(LawFault::BadOutput(GuardRule::FirstDrive)), Some(o))
                        } else {
                            (None, Some(o))
                        }
                    }
                    Err(e) => (Some(LawFault::Failed(e)), Some(o)),
                }
            }
            Answer::Missed(f, o) => (Some(f), o),
            Answer::Panicked(m) => (Some(LawFault::Crashed(m)), None),
        };
        self.records.law_calls.push(LawCallEntry {
            call: LawCall::Seed,
            seed_level_db: Some(level),
            inputs: None,
            outputs: outputs.clone(),
            miss: miss.clone(),
            call_us: us,
        });
        if let Some(f) = miss {
            // A seed that is not usable fires nothing.
            self.finish(EndOutcome::LawFault, f.reason(), f.to_string());
            return Ok(());
        }
        let o = outputs.expect("used");
        self.next_drive = o.drives;
        self.next_values = o.values;
        self.next_level = level;
        self.next_used = true;
        self.next_miss = None;
        self.state = RunState::Waiting;
        Ok(())
    }

    /// The reachable fraction and the floor of each control.
    fn floors(&self, setup: &Setup) -> SeedReport {
        let n = setup.record_len;
        let nc = setup.n_controls;
        let mask: Vec<bool> = {
            let ms = crate::builtin::math_setup(setup);
            hertzforge_mimo_shock_law_math::drive_mask(&ms)
        };
        let reference: Vec<Vec<f64>> = setup
            .target
            .waveforms()
            .chunks(n)
            .map(|c| c.to_vec())
            .collect();
        let spectra = rows_rfft(&reference, 1.0).unwrap_or_default();
        let fraction = reachable_fraction(&setup.plant.frf, &mask, &spectra)
            .unwrap_or_else(|_| vec![f64::NAN; nc]);
        // The best reachable response: H H^+ R on the driven lines.
        let best: Vec<Vec<f64>> = match pinv(&setup.plant.frf, Regularization::None) {
            Ok((hp, _)) => {
                let lines = n / 2 + 1;
                let nd = setup.n_drives;
                let mut out = vec![vec![hertzforge_sdm::C64::new(0.0, 0.0); lines]; nc];
                for f in (0..lines).filter(|&f| mask[f]) {
                    let g = hp.line(f);
                    let h = setup.plant.frf.line(f);
                    let d: Vec<hertzforge_sdm::C64> = (0..nd)
                        .map(|i| (0..nc).map(|c| g[i * nc + c] * spectra[c][f]).sum())
                        .collect();
                    for (c, o) in out.iter_mut().enumerate() {
                        o[f] = (0..nd).map(|i| h[c * nd + i] * d[i]).sum();
                    }
                }
                rows_irfft(&out, n, 1.0).unwrap_or_default()
            }
            Err(_) => Vec::new(),
        };
        let floor_db: Vec<f64> = (0..nc)
            .map(|c| match (&setup.target, best.get(c)) {
                (Target::Srs(t), Some(b)) => {
                    let nf = t.natural_hz.len();
                    let m = hertzforge_dsp::srs::srs_analyze(b, setup.fs_hz, &t.natural_hz, t.q);
                    m.iter()
                        .zip(&t.points[c * nf..(c + 1) * nf])
                        .map(|(mv, tv)| (20.0 * (mv.max(1e-300) / tv).log10()).abs())
                        .fold(0.0f64, f64::max)
                }
                (Target::Waveform(_), _) | (Target::Srs(_), None) => {
                    10.0 * (1.0 - fraction[c]).max(1e-30).log10()
                }
            })
            .collect();
        SeedReport {
            reachable_fraction: fraction,
            floor_db,
            predicted_peak_ratio: Vec::new(),
        }
    }

    /// Fire the next pulse, in the order the module documentation gives, and
    /// report it. When the pulse ends the run, its report carries the end.
    /// `cancel` stops the play at the next block.
    pub fn fire_next(&mut self, cancel: Option<&AtomicBool>) -> Result<PulseReport, EngineError> {
        if self.state != RunState::Waiting {
            return Err(EngineError::WrongState(self.state));
        }
        let n = self.config.record_len;
        let nd = self.config.drives.len();
        let nc = self.config.controls.len();
        let index = self.pulses.len() as u64;
        let level = self.next_level;
        let mut report = PulseReport {
            index,
            scheduled_db: level,
            level_db: level,
            limit_scale: vec![1.0; nd],
            drive_vpk: vec![0.0; nd],
            controls: Vec::new(),
            passed: false,
            global_srs_error_db: None,
            at_s: 0.0,
            law_used: self.next_used,
            law_miss: self.next_miss.take(),
            law_values: std::mem::take(&mut self.next_values),
            predicted_peak_ratio: Vec::new(),
            end: None,
            over_test_db: f64::NEG_INFINITY,
            arrival_spread_s: 0.0,
            warnings: Vec::new(),
            step_note: None,
        };
        if index >= self.config.guards.law.max_pulses {
            let end = self.finish(
                EndOutcome::SafeStop,
                "max_pulses",
                format!("the run has fired its {index} pulses"),
            );
            report.end = Some(end);
            return Ok(report);
        }
        // Build and check: the drive ceiling and abort per drive.
        let mut drives = std::mem::take(&mut self.next_drive);
        for (d, ch) in self.config.drives.iter().enumerate() {
            let row = &mut drives[d * n..(d + 1) * n];
            let pk = peak(row);
            if pk > ch.abort_v {
                let end = self.finish(
                    EndOutcome::Aborted,
                    "drive_abort",
                    format!(
                        "drive {} would play {pk:.3} V, over its abort of {} V",
                        ch.channel, ch.abort_v
                    ),
                );
                report.end = Some(end);
                return Ok(report);
            }
            if pk > ch.ceiling_v {
                let k = ch.ceiling_v / pk;
                row.iter_mut().for_each(|v| *v *= k);
                report.limit_scale[d] = k;
            }
            report.drive_vpk[d] = peak(row);
        }
        // The predicted response against the abort lines.
        match self.predicted_breach(&drives, level) {
            Ok((ratios, breach)) => {
                report.predicted_peak_ratio = ratios;
                if let Some(why) = breach {
                    let end = self.finish(EndOutcome::SafeStop, "predicted_abort", why);
                    report.end = Some(end);
                    return Ok(report);
                }
            }
            Err(e) => {
                let end = self.finish(EndOutcome::Fault, "fault", e);
                report.end = Some(end);
                return Ok(report);
            }
        }
        // Fire.
        self.state = RunState::Firing;
        let rows: Vec<Vec<f64>> = drives.chunks(n).map(|c| c.to_vec()).collect();
        let t_fire = Instant::now();
        let shot = self.fire_shot(&rows, cancel);
        report.at_s = t_fire.elapsed().as_secs_f64();
        let shot = match shot {
            Ok(s) => s,
            Err(ShotError::Cancelled) => {
                let end = self.finish(
                    EndOutcome::OperatorStop,
                    "cancelled",
                    "the play was cancelled".into(),
                );
                report.end = Some(end);
                return Ok(report);
            }
            Err(ShotError::Failed(m)) => {
                let end = self.finish(EndOutcome::Fault, "fault", m);
                report.end = Some(end);
                return Ok(report);
            }
        };
        // Measure.
        let measured = match self.rows_of_channels(&shot, &self.input_channels()) {
            Ok(m) => m,
            Err(e) => {
                let end = self.finish(EndOutcome::Fault, "fault", e);
                report.end = Some(end);
                return Ok(report);
            }
        };
        let responses: Vec<Vec<f64>> = measured[..nc]
            .iter()
            .map(|r| r[..n.min(r.len())].to_vec())
            .collect();
        let limits: Vec<Vec<f64>> = measured[nc..].to_vec();
        self.records.pulses.push(PulseEntry {
            index,
            level_db: level,
            drives: drives.clone(),
            quiet_frames: shot.quiet_frames,
            channels: shot.channels.clone(),
            rows: shot.rows.clone(),
        });
        // Judge each control.
        let built = self.built.clone().expect("armed");
        let judgements: Vec<ControlJudgement> = responses
            .iter()
            .enumerate()
            .map(|(c, y)| {
                judge_control(
                    c,
                    y,
                    level,
                    self.fs_hz,
                    &built.target,
                    &built.judging,
                    &self.config.timing,
                )
            })
            .collect();
        report.controls = judgements
            .iter()
            .zip(&self.config.controls)
            .map(|(j, &ch)| ControlReport {
                channel: ch,
                verdict: j.verdict,
                peak: j.peak,
                waveform_error_db: j.waveform_error_db,
                velocity_ips: j.velocity_ips,
                displacement_pk_pk_in: j.displacement_pk_pk_in,
                timing_ok: j.timing_ok,
                over_test_db: j.over_test_db,
                polarity_ok: j.polarity_ok,
                duration_ratio: j.duration_ratio,
                duration_ok: j.duration_ok,
                energy_db: j.energy_db,
                rms_duration_ratio: j.rms_duration_ratio,
            })
            .collect();
        report.passed = judgements.iter().all(|j| j.verdict.passed);
        report.over_test_db = judgements
            .iter()
            .fold(f64::NEG_INFINITY, |a, j| a.max(j.over_test_db));
        let delays: Vec<f64> = judgements.iter().map(|j| j.verdict.delay_s).collect();
        let (spread, warning) = crate::judge::arrival_spread(&delays, &self.config.timing);
        report.arrival_spread_s = spread;
        report.warnings.extend(warning);
        for (j, ch) in judgements.iter().zip(&self.config.controls) {
            if j.energy_db.abs() > crate::judge::ENERGY_TOL_DB {
                report.warnings.push(format!(
                    "control {ch} has {:+.2} dB of its target's energy",
                    j.energy_db
                ));
            }
            if (j.rms_duration_ratio - 1.0).abs() > crate::judge::RMS_DURATION_TOL {
                report.warnings.push(format!(
                    "control {ch}'s rms duration is {:.2} times its target's",
                    j.rms_duration_ratio
                ));
            }
        }
        let srs_errors: Vec<Vec<f64>> = judgements
            .iter()
            .filter_map(|j| j.srs_error_db.clone())
            .collect();
        if srs_errors.len() == nc {
            report.global_srs_error_db = global_srs_error_db(&built.target, &srs_errors);
        }
        self.last_played = Some((drives.clone(), level));
        // The measured response against the abort lines.
        let line = self.control_abort_ratio();
        let measured_breach = if let Some((c, j)) = judgements
            .iter()
            .enumerate()
            .find(|(_, j)| j.verdict.peak_ratio > line)
        {
            Some((
                EndOutcome::Aborted,
                "control_abort",
                format!(
                    "control {} measured {:.2} dB over its target's peak; its abort line is {:.2} dB",
                    self.config.controls[c],
                    20.0 * j.verdict.peak_ratio.log10(),
                    20.0 * line.log10()
                ),
            ))
        } else if let Some((lim, pk)) = self
            .config
            .limits
            .iter()
            .zip(limits.iter().map(|r| peak(r)))
            .find(|(l, pk)| *pk > l.abort_peak)
        {
            Some((
                EndOutcome::Aborted,
                "limit_abort",
                format!(
                    "limit {} measured {pk:.3}, over its abort of {}",
                    lim.channel, lim.abort_peak
                ),
            ))
        } else {
            self.kinematics_breach(&responses)
                .map(|m| (EndOutcome::Aborted, "kinematics", m))
        };
        let verdicts: Vec<ControlVerdict> = judgements.iter().map(|j| j.verdict).collect();
        self.pulses.push(report.clone());
        if let Some((o, r, m)) = measured_breach {
            let end = self.finish(o, r, m);
            return Ok(self.close_report(end));
        }
        // The ladder: the joint verdict fixes the next level.
        let tick = self
            .ladder
            .as_mut()
            .expect("proceeded")
            .on_judged(report.passed, report.over_test_db);
        match tick {
            Tick::Completed => {
                let end = self.finish(
                    EndOutcome::Completed,
                    "completed",
                    "every level passed".into(),
                );
                return Ok(self.close_report(end));
            }
            Tick::NotConverged { pulses } => {
                let end = self.finish(
                    EndOutcome::NotConverged,
                    "not_converged",
                    format!("{pulses} pulses at {level} dB without every control passing"),
                );
                return Ok(self.close_report(end));
            }
            Tick::Advanced { note: Some(n), .. } => {
                self.records.step_notes.push(n.clone());
                if let Some(r) = self.pulses.last_mut() {
                    r.step_note = Some(n);
                }
            }
            Tick::Advanced { note: None, .. } | Tick::Held => {}
        }
        let next_level = self.ladder.as_ref().expect("proceeded").level_db();
        // The law, with the next level.
        let srs = match &built.target {
            Target::Srs(_) => Some(
                judgements
                    .iter()
                    .flat_map(|j| j.srs.clone().unwrap_or_default())
                    .collect::<Vec<f64>>(),
            ),
            Target::Waveform(_) => None,
        };
        let inputs = PulseInputs {
            pulse_index: index + 1,
            level_db: level,
            next_level_db: next_level,
            drives: drives.clone(),
            responses: responses.concat(),
            srs,
            verdicts,
            limited: report.limit_scale.iter().any(|k| *k < 1.0),
        };
        let (ans, us) = self.call_law(Some(&inputs), next_level);
        let step = Self::scale(next_level) / Self::scale(level);
        let (miss, outputs, panicked) = match ans {
            Answer::Used(o) => match self.guard_answer(&o, &drives, step, next_level) {
                None => (None, Some(o), false),
                Some(f) => (Some(f), Some(o), false),
            },
            Answer::Missed(f, o) => (Some(f), o, false),
            Answer::Panicked(m) => (Some(LawFault::Crashed(m)), None, true),
        };
        self.records.law_calls.push(LawCallEntry {
            call: LawCall::Iterate,
            seed_level_db: None,
            inputs: Some(inputs),
            outputs: outputs.clone(),
            miss: miss.clone(),
            call_us: us,
        });
        // The divergence watch, on the pulse just played.
        let waveform_error = match &built.target {
            Target::Waveform(_) => Some(
                judgements
                    .iter()
                    .fold(f64::NEG_INFINITY, |a, j| a.max(j.waveform_error_db)),
            ),
            Target::Srs(_) => outputs.as_ref().and_then(|o| {
                o.values
                    .iter()
                    .find(|v| v.key == "waveform_error")
                    .map(|v| v.value)
            }),
        };
        let drive_rms: Vec<f64> = drives
            .chunks(n)
            .map(|d| (d.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt())
            .collect();
        if let Some(d) = self.watch.observe(level, waveform_error, &drive_rms) {
            let end = self.finish(
                EndOutcome::SafeStop,
                "divergence",
                format!("{} ({})", d.detail, d.rule),
            );
            return Ok(self.close_report(end));
        }
        match miss {
            None => {
                self.misses.used();
                let o = outputs.expect("used");
                self.next_drive = o.drives;
                self.next_values = o.values;
                self.next_used = true;
                self.next_miss = None;
            }
            Some(f) => {
                // A panicked law's state is unknown: it is not called again.
                let tripped = self.misses.miss() || panicked;
                if tripped {
                    let detail = if panicked {
                        f.to_string()
                    } else {
                        format!("{} misses in a row; the last: {f}", self.misses.in_a_row())
                    };
                    let end = self.finish(EndOutcome::LawFault, f.reason(), detail);
                    return Ok(self.close_report(end));
                }
                // The last drive played, at the next level.
                self.next_drive = drives.iter().map(|v| v * step).collect();
                self.next_values = Vec::new();
                self.next_used = false;
                self.next_miss = Some(format!("{} ({})", f, f.reason()));
            }
        }
        self.next_level = next_level;
        self.state = RunState::Waiting;
        Ok(self.pulses.last().cloned().expect("pushed"))
    }

    /// The core's checks of a law's answer beyond its shape: the growth cap
    /// over the drive just played (beyond the level step), and the predicted
    /// response at the next level.
    fn guard_answer(
        &self,
        o: &PulseOutputs,
        played: &[f64],
        step: f64,
        next_level: f64,
    ) -> Option<LawFault> {
        let n = self.config.record_len;
        match growth(&o.drives, played, n, step, &self.bands, GROWTH_BAND_FLOOR) {
            Ok(g) => {
                let gp = &self.config.guards;
                if g.iter().any(|g| g.peak_db > gp.max_growth_db) {
                    return Some(LawFault::BadOutput(GuardRule::PeakRise));
                }
                if g.iter().any(|g| g.band_db > gp.max_band_growth_db) {
                    return Some(LawFault::BadOutput(GuardRule::BinRise));
                }
            }
            Err(e) => return Some(LawFault::Failed(e)),
        }
        // A law's own prediction of its next pulse's worst over-test, dB, when
        // it reports one, against the controls' abort line.
        if let Some(v) = o.values.iter().find(|v| v.key == LAW_PREDICTED_OVER_TEST) {
            if v.value > 20.0 * self.control_abort_ratio().log10() {
                return Some(LawFault::BadOutput(GuardRule::PredictedPeak));
            }
        }
        match self.predicted_breach(&o.drives, next_level) {
            Ok((_, None)) => None,
            Ok((_, Some(_))) => Some(LawFault::BadOutput(GuardRule::PredictedPeak)),
            Err(e) => Some(LawFault::Failed(e)),
        }
    }

    /// The last pulse's report with the run's end on it.
    fn close_report(&mut self, end: RunEnd) -> PulseReport {
        let r = self.pulses.last_mut().expect("pushed");
        r.end = Some(end);
        r.clone()
    }

    /// Run every remaining pulse until the run ends; the end.
    pub fn run_to_end(&mut self, cancel: Option<&AtomicBool>) -> Result<RunEnd, EngineError> {
        while self.state == RunState::Waiting {
            self.fire_next(cancel)?;
        }
        self.end.clone().ok_or(EngineError::WrongState(self.state))
    }

    /// End the run at the next boundary: nothing more fires, the station is
    /// stopped and released.
    pub fn stop(&mut self) -> Result<(), EngineError> {
        match self.state {
            RunState::Idle | RunState::Finished => Err(EngineError::WrongState(self.state)),
            _ => {
                self.finish(
                    EndOutcome::OperatorStop,
                    "operator",
                    "stopped by the operator".into(),
                );
                Ok(())
            }
        }
    }

    /// Take every drive down at once and end the run.
    pub fn abort(&mut self) -> Result<(), EngineError> {
        match self.state {
            RunState::Idle | RunState::Finished => Err(EngineError::WrongState(self.state)),
            _ => {
                if let Some(mut shot) = self.shot.take() {
                    let _ = shot.emergency_stop();
                }
                self.finish(
                    EndOutcome::OperatorAbort,
                    "operator",
                    "aborted by the operator".into(),
                );
                Ok(())
            }
        }
    }
}
