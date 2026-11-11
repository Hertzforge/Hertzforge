//! The MIMO Shock run engine's public surface: its configuration, run states,
//! per-pulse report and per-control report.
//!
//! The configuration and its validation are complete. The run itself
//! (identification on the pulse grid, the plant stack, drive building and
//! judging per control, timing, the ladder, the guards, records and replay) is
//! not built in this version: every run function answers
//! [`EngineError::NotBuilt`] naming the step, and changes nothing.
//!
//! The order of one pulse is fixed here so every implementation keeps it: build
//! and check the drives, fire them in one shot, measure every input, judge each
//! control, tick the ladder (which fixes the next level), call the law with the
//! next level, guard its answer.

use crate::law::MimoShockLawFactory;
use crate::limits::{MAX_CONTROLS, MAX_DRIVES, MAX_RECORD_LEN, MIN_RECORD_LEN};
use crate::pulse::ControlVerdict;
use crate::setup::{SetupError, SetupRule, Target, Timing};
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::Backend;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawGuards};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
}

impl Default for LadderPlan {
    /// -12, -9, -6, -3 and 0 dB, three pulses a level.
    fn default() -> Self {
        LadderPlan {
            levels_db: vec![-12.0, -9.0, -6.0, -3.0, 0.0],
            max_pulses_per_level: 3,
        }
    }
}

/// The identification shot: independent band-limited random on every drive,
/// estimated on the pulse grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentificationPlan {
    /// Each drive's level, volts rms, under its ceiling.
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
    /// Pulses in a row at one level with the error rising that end the run
    /// (`divergence`).
    pub divergence_pulses: u32,
    /// The law call budget, misses and the run's pulse cap.
    pub law: LawGuards,
}

impl Default for GuardPlan {
    fn default() -> Self {
        GuardPlan {
            max_growth_db: 6.0,
            divergence_pulses: 2,
            law: LawGuards::default(),
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
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Config(e) => write!(f, "the configuration is refused: {e}"),
            EngineError::WrongState(s) => write!(f, "not allowed while {}", s.phase()),
            EngineError::NotBuilt(step) => {
                write!(f, "MIMO Shock cannot run its {step} step in this version")
            }
        }
    }
}

impl std::error::Error for EngineError {}

/// The run engine. Built from a checked configuration and the law's factory.
pub struct MimoShockEngine {
    config: MimoShockConfig,
    law: Arc<dyn MimoShockLawFactory>,
    state: RunState,
    pulses: Vec<PulseReport>,
}

impl MimoShockEngine {
    /// An engine for `config`, refused when the configuration breaks a rule.
    pub fn new(
        config: MimoShockConfig,
        law: Arc<dyn MimoShockLawFactory>,
    ) -> Result<Self, EngineError> {
        config.validate().map_err(EngineError::Config)?;
        Ok(MimoShockEngine {
            config,
            law,
            state: RunState::Idle,
            pulses: Vec::new(),
        })
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

    /// Open the station `backend` with `table`, adopt its rate, re-derive the
    /// grid at it and pass the arm gate. Nothing is driven.
    pub fn arm(
        &mut self,
        _backend: Box<dyn Backend + Send>,
        _table: ChannelTable,
    ) -> Result<(), EngineError> {
        Err(EngineError::NotBuilt("arm"))
    }

    /// Play the identification shot and form the plant on the pulse grid.
    pub fn identify(&mut self) -> Result<(), EngineError> {
        Err(EngineError::NotBuilt("identification"))
    }

    /// Build and seed the law after identification.
    pub fn proceed(&mut self) -> Result<(), EngineError> {
        Err(EngineError::NotBuilt("law seed"))
    }

    /// Fire the next pulse, in the order the module documentation gives.
    /// `cancel` stops the play at the next block.
    pub fn fire_next(&mut self, _cancel: Option<&AtomicBool>) -> Result<PulseReport, EngineError> {
        Err(EngineError::NotBuilt("pulse"))
    }

    /// End the run at the next boundary.
    pub fn stop(&mut self) -> Result<(), EngineError> {
        Err(EngineError::NotBuilt("stop"))
    }

    /// Take every drive down at once.
    pub fn abort(&mut self) -> Result<(), EngineError> {
        Err(EngineError::NotBuilt("abort"))
    }
}
