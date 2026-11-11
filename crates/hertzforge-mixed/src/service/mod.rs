//! The Mixed Mode run service: [`MixedService`].
//!
//! Its lifecycle and method set are Sine's (`SineService`), so a control
//! service's run thread drives it the same way: it owns the service outright,
//! builds it with [`MixedService::new`], gives it its laws, arms it, runs the
//! identification and the coherence gate, holds for review, proceeds, then
//! calls [`MixedService::run_chunk`] (about a quarter second of station time
//! each) until it returns `false`, posting the operator's verbs between chunks
//! and reading [`MixedService::live_status`] and [`MixedService::display`].
//!
//! The core always holds at review (when `identification.review_hold` is on);
//! the run thread proceeds. The core keeps its own cancel flag
//! ([`MixedService::sysid_cancel_flag`]) and abort latch, which the run thread
//! bridges from its own.
//!
//! This is the contract the run thread and the core are built against. What
//! the run needs before it can drive is refused [`MixedError::NotBuilt`] until
//! the core lands; signatures do not change when it does.

pub mod display;
pub mod law_records;
pub mod status;

use crate::parts::{ConfigError, MixedConfig, MixedLawGuards};
use crate::schedule::PartClock;
use display::MixedDisplay;
use hertzforge_capture::Mark;
use hertzforge_channel_table::ChannelTable;
use hertzforge_hal::Backend;
use hertzforge_random::{HinvConfig, RunMode};
use status::{
    MixedLawStatus, MixedLog, MixedOutcome, MixedState, MixedStatus, MixedTrip, RoleTally,
};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Why a service operation was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum MixedError {
    /// The configuration was refused (by name: [`ConfigError::name`]).
    Config(ConfigError),
    /// The operation is not available in the core yet. Names the operation.
    NotBuilt(&'static str),
    /// The operation is not allowed in the present state.
    BadState(String),
    /// The station refused an operation.
    Plant(String),
    /// A law could not be built or seeded.
    Law(String),
    /// A capture or snapshot could not be written.
    Io(String),
}

impl MixedError {
    /// The stable wire name.
    pub fn name(&self) -> &'static str {
        match self {
            MixedError::Config(e) => e.name(),
            MixedError::NotBuilt(_) => "not_built",
            MixedError::BadState(_) => "bad_state",
            MixedError::Plant(_) => "station_error",
            MixedError::Law(_) => "law_error",
            MixedError::Io(_) => "io_error",
        }
    }
}

impl std::fmt::Display for MixedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MixedError::Config(e) => write!(f, "{e}"),
            MixedError::NotBuilt(what) => write!(f, "Mixed Mode cannot {what} yet"),
            MixedError::BadState(s) => write!(f, "not now: {s}"),
            MixedError::Plant(s) => write!(f, "station: {s}"),
            MixedError::Law(s) => write!(f, "control law: {s}"),
            MixedError::Io(s) => write!(f, "could not write: {s}"),
        }
    }
}

impl std::error::Error for MixedError {}

impl From<ConfigError> for MixedError {
    fn from(e: ConfigError) -> Self {
        MixedError::Config(e)
    }
}

/// Which of the run's two laws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawRole {
    /// The floor law (a single-output Random law).
    Floor,
    /// The tone law (a Sine law; one instance per tone).
    Tones,
}

/// The factories the run builds its laws with: one floor law, and one tone law
/// instance per tone from the same factory.
#[derive(Clone)]
pub struct MixedLawFactories {
    /// Builds the floor law.
    pub floor: Arc<dyn hertzforge_random::LawFactory>,
    /// The built-in floor law's settings (Random's `hinv`), as the floor law's
    /// choice set them; the core reads some of them too (the divergence count,
    /// the change cap's floor), so they are the law's.
    pub floor_hinv: HinvConfig,
    /// Builds each tone's law.
    pub tones: Arc<dyn hertzforge_sine::SineLawFactory>,
}

impl MixedLawFactories {
    /// The built-in laws with their defaults: `hinv` for the floor,
    /// `servo_blend` for the tones.
    pub fn builtin() -> Self {
        MixedLawFactories {
            floor: Arc::new(hertzforge_random::HinvLawFactory),
            floor_hinv: HinvConfig::default(),
            tones: Arc::new(hertzforge_sine::law::servo_blend::ServoBlendFactory {
                params: hertzforge_sine::law::servo_blend::ServoBlendParams::default(),
            }),
        }
    }
}

impl std::fmt::Debug for MixedLawFactories {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MixedLawFactories")
            .field("tones", &self.tones.describe())
            .finish_non_exhaustive()
    }
}

/// Why a live change of a law parameter was refused (Sine's reasons and wire
/// names).
#[derive(Debug, Clone, PartialEq)]
pub enum MixedAdjustRefusal {
    /// The law is not built yet (the loop has not engaged).
    NotEngaged,
    /// The law has no parameter by that key.
    UnknownKey,
    /// The parameter cannot change while the test runs.
    NotLive,
    /// The value is outside the parameter's range.
    OutOfRange {
        /// Lowest.
        min: f64,
        /// Highest.
        max: f64,
    },
    /// The law refused it; its reason.
    LawRefused(String),
}

impl MixedAdjustRefusal {
    /// The stable wire name (`adjust_refused`'s `why`).
    pub fn why(&self) -> &'static str {
        match self {
            MixedAdjustRefusal::NotEngaged => "not_engaged",
            MixedAdjustRefusal::UnknownKey => "unknown_key",
            MixedAdjustRefusal::NotLive => "not_live",
            MixedAdjustRefusal::OutOfRange { .. } => "out_of_range",
            MixedAdjustRefusal::LawRefused(_) => "law_refused",
        }
    }
}

impl std::fmt::Display for MixedAdjustRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MixedAdjustRefusal::NotEngaged => f.write_str("the control law is not built yet"),
            MixedAdjustRefusal::UnknownKey => f.write_str("the control law has no such parameter"),
            MixedAdjustRefusal::NotLive => {
                f.write_str("that parameter cannot be changed while the test runs")
            }
            MixedAdjustRefusal::OutOfRange { min, max } => {
                write!(
                    f,
                    "the value is outside the parameter's range, {min} to {max}"
                )
            }
            MixedAdjustRefusal::LawRefused(why) => write!(f, "the control law refused it: {why}"),
        }
    }
}

/// The record of a run: what it was asked, how it went, how it ended.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MixedReport {
    /// The configuration as run.
    pub config: MixedConfig,
    /// The rate the run adopted, Hz.
    pub sample_rate_hz: f64,
    /// The final state.
    pub state: MixedState,
    /// How it ended.
    pub outcome: Option<MixedOutcome>,
    /// The trip, when it tripped.
    pub trip: Option<MixedTrip>,
    /// The status at the end.
    pub status: MixedStatus,
    /// The event log.
    pub events: Vec<status::MixedEvent>,
}

/// The Mixed Mode run service. One per run; the run thread owns it and drops it
/// (and with it the station) when the run ends.
pub struct MixedService {
    cfg: MixedConfig,
    table: ChannelTable,
    /// The station, until `arm` opens it into the run's engine.
    #[allow(dead_code)]
    backend: Option<Box<dyn Backend>>,
    station: hertzforge_module_abi::StationClass,
    sample_rate_hz: f64,
    sysid_seed: u64,
    data_root: Option<PathBuf>,
    state: MixedState,
    run_mode: RunMode,
    log: MixedLog,
    laws: MixedLawFactories,
    floor_clock: Arc<dyn hertzforge_random::LawClock>,
    tone_clock: Arc<dyn hertzforge_sine::law::LawClock>,
    block_frames: Option<usize>,
    sysid_cancel: Arc<AtomicBool>,
    clock: PartClock,
    trip: Option<MixedTrip>,
}

impl std::fmt::Debug for MixedService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MixedService")
            .field("state", &self.state)
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("parts", &self.cfg.parts.len())
            .finish_non_exhaustive()
    }
}

impl MixedService {
    /// Build a service for `cfg` against `table` on `backend`, the station
    /// bound to `station`. Validates the configuration at the backend's rate
    /// (every part inside the floor's band and under half the rate, tones
    /// spaced, follows resolved) and that every control and limit channel is an
    /// enabled row of `table`, then stores everything; nothing is driven.
    /// `sysid_seed` seeds the identification's noise; `data_root` is the local
    /// capture root (`None`: the default).
    ///
    /// The built-in laws are in force until [`Self::with_laws`].
    pub fn new(
        cfg: MixedConfig,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        station: hertzforge_module_abi::StationClass,
        sysid_seed: u64,
        data_root: Option<PathBuf>,
    ) -> Result<Self, MixedError> {
        let sample_rate_hz = backend.caps().sample_rate_hz;
        cfg.validate(Some(sample_rate_hz))?;
        let enabled = |idx: u32| table.channels.iter().any(|c| c.index == idx && c.enabled);
        for c in &cfg.control.channels {
            if !enabled(c.channel_index) {
                return Err(ConfigError::BadConfig {
                    field: "control",
                    why: format!("channel {} is not an enabled channel", c.channel_index),
                }
                .into());
            }
        }
        for l in &cfg.limits.limit_profiles {
            if !enabled(l.channel_index) {
                return Err(ConfigError::BadConfig {
                    field: "limit_profiles",
                    why: format!("channel {} is not an enabled channel", l.channel_index),
                }
                .into());
            }
        }
        let clock = PartClock::new(&cfg)?;
        Ok(MixedService {
            run_mode: cfg.schedule.mode,
            cfg,
            table,
            backend: Some(backend),
            station,
            sample_rate_hz,
            sysid_seed,
            data_root,
            state: MixedState::Idle,
            log: MixedLog::default(),
            laws: MixedLawFactories::builtin(),
            floor_clock: Arc::new(hertzforge_random::MonotonicClock::new()),
            tone_clock: Arc::new(hertzforge_sine::law::MonotonicClock::new()),
            block_frames: None,
            sysid_cancel: Arc::new(AtomicBool::new(false)),
            clock,
            trip: None,
        })
    }

    // ---- construction ----------------------------------------------------

    /// The laws the run builds: the floor law's factory and settings, and the
    /// tone law's factory (built once per tone at engage).
    pub fn with_laws(mut self, laws: MixedLawFactories) -> Self {
        self.laws = laws;
        self
    }

    /// The clocks law calls are timed on (Random's for the floor law, Sine's
    /// for the tones'); a test injects a fake to make a call late.
    pub fn with_law_clocks(
        mut self,
        floor: Arc<dyn hertzforge_random::LawClock>,
        tones: Arc<dyn hertzforge_sine::law::LawClock>,
    ) -> Self {
        self.floor_clock = floor;
        self.tone_clock = tones;
        self
    }

    /// The block the station is pumped in, frames (the simulator takes the
    /// block a hardware station would at the rate).
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.block_frames = Some(frames.max(1));
        self
    }

    // ---- facts -------------------------------------------------------------

    /// The station class the run is bound to.
    pub fn station(&self) -> hertzforge_module_abi::StationClass {
        self.station
    }

    /// The configuration.
    pub fn config(&self) -> &MixedConfig {
        &self.cfg
    }

    /// The channel table.
    pub fn table(&self) -> &ChannelTable {
        &self.table
    }

    /// The laws in force.
    pub fn laws(&self) -> &MixedLawFactories {
        &self.laws
    }

    /// The core's guards on the laws (the configured ones, else the defaults).
    pub fn law_guards(&self) -> MixedLawGuards {
        self.cfg.law_guards.clone().unwrap_or_default()
    }

    /// The tones' control tick, seconds.
    pub fn control_tick_s(&self) -> f64 {
        self.cfg
            .control_tick_s
            .unwrap_or(hertzforge_sine::law::DEFAULT_TICK_S)
    }

    /// The rate the run runs at, Hz: the backend's own answer at construction,
    /// replaced by the station's adopted rate at arm.
    pub fn sample_rate_hz(&self) -> f64 {
        self.sample_rate_hz
    }

    /// Whether arming moved the rate from the one asked for.
    pub fn rate_was_coerced(&self) -> bool {
        false
    }

    /// The station's configured facts after arm; `None` before.
    pub fn configured(&self) -> Option<hertzforge_hal::ConfiguredActual> {
        None
    }

    /// The engine's health counters; `None` before the loop engages.
    pub fn engine_health(&self) -> Option<hertzforge_engine::EngineHealth> {
        None
    }

    /// Input blocks the station dropped.
    pub fn blocks_dropped(&self) -> u64 {
        0
    }

    /// The identification's seed.
    pub fn sysid_seed(&self) -> u64 {
        self.sysid_seed
    }

    /// The capture root, if one was given.
    pub fn data_root(&self) -> Option<&PathBuf> {
        self.data_root.as_ref()
    }

    /// The lifecycle state.
    pub fn state(&self) -> MixedState {
        self.state
    }

    /// The event log.
    pub fn log(&self) -> &MixedLog {
        &self.log
    }

    /// The trip, when the run tripped.
    pub fn trip(&self) -> Option<&MixedTrip> {
        self.trip.as_ref()
    }

    /// The run-end reason in plain words (the trip's, naming its part, or the
    /// law fault's); `None` while running or after a clean end.
    pub fn end_reason(&self) -> Option<String> {
        self.trip.as_ref().map(MixedTrip::words)
    }

    /// Whether test progress is held.
    pub fn is_held(&self) -> bool {
        self.clock.held()
    }

    /// Whether the drive is at zero (before engage, after a stop or abort).
    pub fn drive_is_zero(&self) -> bool {
        !self.state.is_running()
    }

    /// How the level schedule advances.
    pub fn run_mode(&self) -> RunMode {
        self.run_mode
    }

    /// The identification's band coherence; 0 before it runs.
    pub fn band_coherence(&self) -> f64 {
        0.0
    }

    /// The identification's measured pipeline lag, frames; 0 before it runs.
    pub fn sysid_lag_frames(&self) -> usize {
        0
    }

    /// The flag that cancels a running identification (the run thread bridges
    /// its own stop to it).
    pub fn sysid_cancel_flag(&self) -> Arc<AtomicBool> {
        self.sysid_cancel.clone()
    }

    /// Test progress, 0 to 1, when the run's end is known (every part's
    /// schedule bounded, or a full-level time); `None` otherwise.
    pub fn progress(&self) -> Option<f64> {
        None
    }

    // ---- arm, identification, gate, review -----------------------------

    /// Arm: adopt the station's rate (re-validating every part under its
    /// half), open the station through the shared arm gate, build the line
    /// grid, the floor target and envelopes, the band references and the tone
    /// targets. State stays [`MixedState::Idle`] until identification.
    pub fn arm(&mut self) -> Result<(), MixedError> {
        Err(MixedError::NotBuilt("arm"))
    }

    /// Run Random's broadband pretest at the identification level: `|H|^2`,
    /// coherence and the pipeline lag on the line grid. `pretest_frames` 0
    /// takes the length the grid and the target degrees of freedom need. State
    /// [`MixedState::SysId`] while it runs; cancelled by
    /// [`Self::sysid_cancel_flag`].
    pub fn run_sysid(&mut self, pretest_frames: usize) -> Result<(), MixedError> {
        let _ = pretest_frames;
        Err(MixedError::NotBuilt("identify the plant"))
    }

    /// Judge the identification's band coherence against
    /// `identification.coherence_floor`. `Ok(true)`: passed; the run holds at
    /// [`MixedState::AwaitingReview`] when `review_hold` is on, else is ready
    /// to proceed. `Ok(false)`: parked at [`MixedState::CoherenceHold`].
    pub fn coherence_gate(&mut self) -> Result<bool, MixedError> {
        Err(MixedError::NotBuilt("judge the identification"))
    }

    /// Leave the review hold and engage: build the floor law and one tone law
    /// per tone, seed them, start the station's persistent run, and play the
    /// floor alone at the schedule's start level ([`status::MixedPhase::FloorOnly`]).
    pub fn proceed_after_review(&mut self) -> Result<(), MixedError> {
        Err(MixedError::NotBuilt("engage"))
    }

    // ---- running ---------------------------------------------------------

    /// Run about a quarter second of station time: per frame, every part's
    /// clock, oscillator and tracker, and the summed drive's peak check; per
    /// control tick, each settled tone's law; between chunks, the floor's
    /// estimate and law, the bands' servo and the next synthesis segment.
    /// Returns `true` while the run goes on, `false` once it has ended
    /// (stopped, completed or aborted).
    pub fn run_chunk(&mut self) -> Result<bool, MixedError> {
        Err(MixedError::NotBuilt("run"))
    }

    /// Run `batches` station blocks (a test's finer step than a chunk). Same
    /// return as [`Self::run_chunk`].
    pub fn run_batches(&mut self, batches: u32) -> Result<bool, MixedError> {
        let _ = batches;
        Err(MixedError::NotBuilt("run"))
    }

    /// Hand the last chunk's raw input blocks to `on_block` (`block`, `stride`
    /// = input channels per frame) and release them: the run thread's tap for
    /// time plots.
    pub fn service_analysis_tapped<F>(&mut self, on_block: F)
    where
        F: FnMut(&[f32], usize),
    {
        let _ = on_block;
    }

    // ---- verbs (posted to the step; take effect at its next tick) ----------

    /// Step the level by `delta_db` (every part moves by the same dB, as a
    /// ramp).
    pub fn step_level(&self, delta_db: f64) -> Result<(), MixedError> {
        let _ = delta_db;
        Err(MixedError::NotBuilt("change the level"))
    }

    /// Jump to rung `i` of the level table.
    pub fn jump_to_level(&self, i: usize) -> Result<(), MixedError> {
        let _ = i;
        Err(MixedError::NotBuilt("change the level"))
    }

    /// Go to `db` against full (clamped to the table's range); returns the
    /// level taken.
    pub fn set_level_db(&self, db: f64) -> Result<f64, MixedError> {
        let _ = db;
        Err(MixedError::NotBuilt("change the level"))
    }

    /// Set how the schedule advances (automatic or manual). Allowed before
    /// and during the run.
    pub fn set_run_mode(&mut self, mode: RunMode) -> Result<(), MixedError> {
        if self.state.is_terminal() {
            return Err(MixedError::BadState("the run has ended".into()));
        }
        self.run_mode = mode;
        Ok(())
    }

    /// Hold test progress: every part's clock stops (sweeps, dwell timers,
    /// windows); every part keeps playing at its frequency and level, under
    /// control.
    pub fn hold(&self) -> Result<(), MixedError> {
        Err(MixedError::NotBuilt("hold"))
    }

    /// Resume test progress; every clock restarts together.
    pub fn resume(&self) -> Result<(), MixedError> {
        Err(MixedError::NotBuilt("resume"))
    }

    /// Switch the part `part` on or off during the run, as a ramp at the stop
    /// ramp's rate. The floor cannot be switched off.
    pub fn set_part_enabled(&self, part: &str, on: bool) -> Result<(), MixedError> {
        let _ = (part, on);
        Err(MixedError::NotBuilt("switch a part"))
    }

    /// Change a live parameter `key` of one law to `value`; returns the value
    /// taken. For [`LawRole::Tones`] it changes every tone's instance.
    pub fn adjust(
        &mut self,
        role: LawRole,
        key: &str,
        value: f64,
    ) -> Result<f64, MixedAdjustRefusal> {
        let _ = (role, key, value);
        Err(MixedAdjustRefusal::NotEngaged)
    }

    /// Ask for the controlled stop: every part ramps out over
    /// `shutdown_ramp_s`, then the run ends [`MixedOutcome::Stopped`]. The run
    /// thread keeps calling [`Self::run_chunk`] until it returns `false`.
    pub fn request_stop(&self) -> Result<(), MixedError> {
        Err(MixedError::NotBuilt("stop"))
    }

    /// Stop now, ramping every part out within this call, and end the run.
    pub fn stop(&mut self) -> Result<(), MixedError> {
        if matches!(self.state, MixedState::Idle) || self.state.is_terminal() {
            return Ok(());
        }
        Err(MixedError::NotBuilt("stop"))
    }

    /// Abort: the drive goes to zero at the next block, the run ends
    /// [`MixedOutcome::Aborted`] with `reason` logged. Always succeeds before
    /// engage (there is nothing to take down) and after the end.
    pub fn abort(&mut self, reason: &str) -> Result<(), MixedError> {
        if matches!(self.state, MixedState::Idle) || self.state.is_terminal() {
            self.log.push(
                0.0,
                status::MixedEventKind::Note {
                    text: format!("abort: {reason}"),
                },
            );
            return Ok(());
        }
        Err(MixedError::NotBuilt("abort"))
    }

    // ---- capture -------------------------------------------------------------

    /// Start a capture named `run_name` under the capture root; returns its
    /// folder. The law records are written beside it while it runs.
    pub fn start_capture(&mut self, run_name: &str) -> Result<PathBuf, MixedError> {
        let _ = run_name;
        Err(MixedError::NotBuilt("capture"))
    }

    /// Stop the capture.
    pub fn stop_capture(&mut self) -> Result<(), MixedError> {
        Ok(())
    }

    /// Save a snapshot of the spectra and status with `marks`; returns its
    /// path.
    pub fn save_snapshot(&mut self, marks: Vec<Mark>) -> Result<PathBuf, MixedError> {
        let _ = marks;
        Err(MixedError::NotBuilt("save a snapshot"))
    }

    // ---- reading -------------------------------------------------------------

    /// The full status (every part's row, the floor, the totals, the laws).
    pub fn status(&self) -> MixedStatus {
        self.live_status()
    }

    /// The status as of the last chunk, cheap to call each chunk.
    pub fn live_status(&self) -> MixedStatus {
        let mut s = MixedStatus::idle(&self.cfg);
        s.state = self.state;
        s.trip = self.trip.clone();
        s.run_mode = self.run_mode;
        s.held = self.clock.held();
        s
    }

    /// The display: spectra on the line grid and each tone's and band's
    /// history. Empty before the loop engages.
    pub fn display(&self) -> MixedDisplay {
        MixedDisplay::default()
    }

    /// The identification on the line grid; `None` before it has run.
    pub fn frf(&self) -> Option<display::MixedFrf> {
        None
    }

    /// The laws' counters; `None` before the laws are built.
    pub fn law_status(&self) -> Option<MixedLawStatus> {
        None
    }

    /// The floor law's counters (zero before engage).
    pub fn floor_law_tally(&self) -> RoleTally {
        RoleTally::default()
    }

    /// The run's record.
    pub fn report_data(&self) -> MixedReport {
        MixedReport {
            config: self.cfg.clone(),
            sample_rate_hz: self.sample_rate_hz,
            state: self.state,
            outcome: None,
            trip: self.trip.clone(),
            status: self.status(),
            events: self.log.events().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests;
