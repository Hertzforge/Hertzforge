//! The MIMO Mixed Mode run service: [`MimoMixedService`].
//!
//! Its lifecycle is single-output Mixed Mode's: build it with
//! [`MimoMixedService::new`] (the configuration validated at the station's
//! rate, every control and limit an enabled input, the tone law's manifest
//! asked about every tone), [`MimoMixedService::arm`] the station,
//! [`MimoMixedService::identify`] (MIMO Random's identification played
//! through the station's engine), [`MimoMixedService::coherence_gate`],
//! [`MimoMixedService::proceed`] (the review hold ends: the budget forecast,
//! the floor law built and seeded, one tone law per tone, the station
//! restarted), then [`MimoMixedService::run_chunk`] about a quarter second of
//! station time at a time until it returns `false`. The floor plays alone at
//! the soft-start level until it is equalized, then the parts ramp in and the
//! level schedule runs. The operator's verbs are methods between chunks.
//!
//! The run's real-time start is MIMO Sine's: on a station that runs in real
//! time a lead-in of zeros (the lookahead plus one block) is written before
//! every start and the engine writes none of its own; each start's frames
//! ahead of its first drive frame are counted, and an engaged start whose
//! count differs from the identification's is refused. Every end ramps the
//! drive to zero, lets the station play out, and frees the hardware before
//! the run reports itself ended.

mod floor;
mod ident;
pub mod records;
pub mod step;
#[cfg(test)]
mod tests;

pub use step::{
    MimoMixedStep, StepTrip, ABORT_RAMP_S, RANDOM_PEAK_RELEASE_S, TONE_ABORT_DEBOUNCE_S,
};

use crate::band::{BandResolution, BandSetup, MovingFrameEstimate};
use crate::config::{MimoMixedConfig, MimoMixedConfigError, MimoPartBody};
use crate::estimate::{FrameSpectra, MatrixFloorEstimator};
use crate::fill::FillRule;
use crate::tone::{tone_judging, Ramp, ToneBank, ToneLock};
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_engine::{Engine, EngineConfig, StopReason, VecCaptureSink};
use hertzforge_hal::{AcquisitionConfig, Backend, ConfiguredActual};
use hertzforge_mimo_random::estimate::rows;
use hertzforge_mimo_sine::law::{
    check_law_accepts, LawIdentification, LawSetup, LawTolerance, MimoSineLawFactory,
};
use hertzforge_mimo_sine::setup::{GuardSettings, TestType, Tracking};
use hertzforge_mixed::{PartClock, PartSchedule};
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::engine::{DriveSynth, LoopConfig};
use hertzforge_random_laws::{LawChoice, MimoLawFactory};
use hertzforge_sdm::law::{MimoControlLaw, MimoInput, MimoShape};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use records::LawRecord;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use step::{StepShape, ToneCtl};

/// Engine ticks per bounded batch.
pub const TICKS_PER_BATCH: u64 = 4;
/// Station time per chunk, seconds.
pub const CHUNK_S: f64 = 0.25;
/// Batches the station plays out after the drive reaches zero.
pub const DRAIN_BATCHES: u32 = 8;
/// The floor is equalized (the parts may come in) when every control's
/// in-band level is within this of its reference, dB (single-output Mixed
/// Mode's).
pub const FLOOR_EQUALIZED_DB: f64 = 1.0;
/// A frame whose random gain varies more than this share across it is not
/// averaged (it played during a ramp).
pub const GAIN_STEADY: f64 = 0.01;
/// A floor line is judged against its envelope at this many degrees of
/// freedom (MIMO Random's).
pub const MIN_ENVELOPE_DOF: f64 = 16.0;
/// The random part ramps in and out over this, seconds.
pub const RANDOM_RAMP_S: f64 = 0.25;
/// The setup forecast's step, seconds.
pub const FORECAST_STEP_S: f64 = 0.5;

/// Why a service operation was refused or the run could not go on.
#[derive(Debug, Clone, PartialEq)]
pub enum MimoMixedError {
    /// The configuration was refused (by name).
    Config(MimoMixedConfigError),
    /// Not allowed in the present state.
    BadState(String),
    /// The station refused an operation, or a start could not be counted.
    Plant(String),
    /// A law could not be built or seeded.
    Law(String),
    /// A refusal with its own wire name, about a part when it names one.
    Refused {
        /// The wire name.
        name: &'static str,
        /// The part's id, or empty.
        part: String,
        /// Why.
        why: String,
    },
}

impl MimoMixedError {
    /// The stable wire name.
    pub fn name(&self) -> &'static str {
        match self {
            MimoMixedError::Config(e) => e.name(),
            MimoMixedError::BadState(_) => "bad_state",
            MimoMixedError::Plant(_) => "station_error",
            MimoMixedError::Law(_) => "law_error",
            MimoMixedError::Refused { name, .. } => name,
        }
    }
}

impl std::fmt::Display for MimoMixedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MimoMixedError::Config(e) => write!(f, "{e}"),
            MimoMixedError::BadState(s) => write!(f, "not now: {s}"),
            MimoMixedError::Plant(s) => write!(f, "station: {s}"),
            MimoMixedError::Law(s) => write!(f, "control law: {s}"),
            MimoMixedError::Refused { part, why, .. } if part.is_empty() => f.write_str(why),
            MimoMixedError::Refused { part, why, .. } => write!(f, "{part}: {why}"),
        }
    }
}

impl std::error::Error for MimoMixedError {}

impl From<MimoMixedConfigError> for MimoMixedError {
    fn from(e: MimoMixedConfigError) -> Self {
        MimoMixedError::Config(e)
    }
}

impl From<hertzforge_mixed::ConfigError> for MimoMixedError {
    fn from(e: hertzforge_mixed::ConfigError) -> Self {
        MimoMixedError::Config(e.into())
    }
}

/// Which of the run's two laws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LawRole {
    /// The floor law (a MIMO Random law).
    Floor,
    /// The tone law (a MIMO Sine law; one instance per tone).
    Tones,
}

/// The factories the run builds its laws with.
#[derive(Clone)]
pub struct MimoMixedLaws {
    /// Builds the floor law.
    pub floor: Arc<dyn MimoLawFactory>,
    /// Builds each tone's law (one instance per tone).
    pub tones: Arc<dyn MimoSineLawFactory>,
}

impl std::fmt::Debug for MimoMixedLaws {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MimoMixedLaws")
            .field("floor", &self.floor.identity().name)
            .field("tones", &self.tones.describe().name)
            .finish()
    }
}

impl MimoMixedLaws {
    /// The built-in laws the configuration names, with their parameters
    /// (the floor's resolved by MIMO Random's built-in factory; the tone law
    /// at its defaults: a tone law's parameters resolve through the law
    /// catalog, `hertzforge-mimo-mixed-laws`).
    pub fn builtin(cfg: &MimoMixedConfig) -> Result<Self, MimoMixedError> {
        let floor_pick = &cfg.law.floor;
        let choice = if floor_pick.params.is_empty() {
            LawChoice::new(floor_pick.id.clone(), Vec::new())
        } else {
            return Err(MimoMixedError::Law(format!(
                "floor law {}: parameters resolve through the law catalog",
                floor_pick.id
            )));
        };
        let floor = hertzforge_mimo_random::law::builtin_factory(&choice, cfg.n())
            .map_err(|e| MimoMixedError::Law(format!("floor law {}: {e}", floor_pick.id)))?;
        if !cfg.law.tones.params.is_empty() {
            return Err(MimoMixedError::Law(format!(
                "tone law {}: parameters resolve through the law catalog",
                cfg.law.tones.id
            )));
        }
        let tones = hertzforge_mimo_sine::laws::builtin(&cfg.law.tones.id).ok_or_else(|| {
            MimoMixedError::Law(format!(
                "tone law {} is not a built-in MIMO Sine law",
                cfg.law.tones.id
            ))
        })?;
        Ok(MimoMixedLaws { floor, tones })
    }
}

/// The run's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoMixedState {
    /// Built; nothing opened.
    Idle,
    /// The station is open and armed.
    Armed,
    /// Identified; the station is parked.
    Identified,
    /// The gate passed (or was overridden); held for review.
    Review,
    /// Running.
    Running,
    /// Ended (the hardware is free).
    Ended,
}

/// Where a running test is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MimoMixedPhase {
    /// Before the run.
    Idle,
    /// The floor alone at the soft-start level until it is equalized.
    FloorAlone,
    /// The parts ramping in and the level going to the start level.
    PartsIn,
    /// The level schedule.
    Schedule,
    /// At full level.
    FullLevel,
    /// The drive on its way to zero.
    Stopping,
}

/// How the run ended: the wire name, the part and the control (or drive or
/// channel) it names, in words.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RunEnd {
    /// The wire name (`completed`, `stopped`, `drive_vpk`, `tone_abort`, ...).
    pub name: &'static str,
    /// Whether it was an abort.
    pub aborted: bool,
    /// The part's id, when the end is about one.
    pub part: Option<String>,
    /// The control, when it names one.
    pub control: Option<usize>,
    /// The drive, when it names one.
    pub drive: Option<usize>,
    /// In words.
    pub words: String,
}

/// One start of the station (MIMO Sine's account).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct StationStart {
    /// Which phase: `identification` or `engaged`.
    pub phase: &'static str,
    /// Zeros the service wrote ahead of the start (a real-time station).
    pub lead_in_frames: usize,
    /// Frames the engine wrote ahead of its first drive frame.
    pub prefill_frames: usize,
    /// The engine's output queue at the start.
    pub queue_frames: usize,
}

impl StationStart {
    /// Frames ahead of the first drive frame.
    pub fn frames_ahead(&self) -> usize {
        self.lead_in_frames + self.prefill_frames + self.queue_frames
    }
}

/// The identification.
#[derive(Debug, Clone)]
pub struct MimoMixedIdent {
    /// `H` on the grid, controls then limits by drives.
    pub h: MatrixStack,
    /// Each control's multiple coherence, line-major (`l * n + i`).
    pub coherence: Vec<f64>,
    /// Each limit's multiple coherence, line-major.
    pub limit_coherence: Vec<f64>,
    /// The controls' spectral matrix, at the identification's level.
    pub g_cc: HermitianStack,
    /// The drives' spectral matrix, at the identification's level.
    pub g_dd: HermitianStack,
    /// Welch frames behind it.
    pub frames: usize,
    /// The controls' level against the reference, dB.
    pub level_db: f64,
    /// Each drive's rms, volts.
    pub drive_vrms: Vec<f64>,
    /// Level-search steps taken.
    pub steps: usize,
    /// The response's lag behind the drive, frames.
    pub lag_frames: usize,
    /// Frames ahead of the first drive frame at its start.
    pub frames_ahead: usize,
    /// Each control's mean coherence over the band.
    pub band_coherence: Vec<f64>,
}

/// One tone's line in the status.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToneStatus {
    /// The part's id.
    pub id: String,
    /// Its frequency now, Hz.
    pub frequency_hz: f64,
    /// Its part level (0 off, 1 on).
    pub part_level: f64,
    /// Whether its law is engaged.
    pub engaged: bool,
    /// Whether it was usable (in, settled, readable) at the last tick.
    pub usable: bool,
    /// Whether the shared ceiling holds it.
    pub limiting: bool,
    /// The ceiling rule's scale on its ask.
    pub scale: f64,
    /// Per control: the judged error, dB.
    pub error_db: Vec<f64>,
    /// Per control: in alarm.
    pub alarm: Vec<bool>,
    /// Per control: the tracked amplitude, peak EU.
    pub measured: Vec<f64>,
    /// Per control: the target at the level, peak EU.
    pub target: Vec<f64>,
}

/// One band's line in the status.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BandStatus {
    /// The part's id.
    pub id: String,
    /// Folded into the floor's reference (no law of its own).
    pub folded: bool,
    /// Its center now, Hz.
    pub center_hz: f64,
    /// Its ramp (0 off, 1 on).
    pub part_level: f64,
    /// Per control: its mean added level over the floor against the stated,
    /// dB (servoed bands; `NaN` before a reading).
    pub error_db: Vec<f64>,
    /// Per control: outside its alarm band.
    pub alarm: Vec<bool>,
    /// A drive's rms ceiling or the change cap holds its drive (its servo's
    /// gain does not rise meanwhile).
    pub limiting: bool,
}

/// One drive's line in the status.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct DriveStatus {
    /// The random part's rms in the last chunk, volts.
    pub random_rms_v: f64,
    /// The largest magnitude played in the last chunk, volts.
    pub peak_v: f64,
    /// The tones' summed amplitude in force, volts at the level.
    pub tone_sum_v: f64,
    /// The peak abort the run's step holds this drive to, volts (above the
    /// clamp at the drive's `peak_v`); the configured one before the step
    /// is built.
    pub abort_vpk: f64,
}

/// The run's status.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MimoMixedStatus {
    /// The state.
    pub state: MimoMixedState,
    /// The phase.
    pub phase: MimoMixedPhase,
    /// Station time since engage, seconds.
    pub t_s: f64,
    /// The level in force, dB.
    pub level_db: f64,
    /// Each control's floor error on clean lines, dB (rms level against the
    /// reference).
    pub floor_error_db: Vec<f64>,
    /// Whether the floor is equalized.
    pub floor_equalized: bool,
    /// Floor loops run.
    pub loops: u64,
    /// The floor law's misses in a row.
    pub floor_misses: u32,
    /// The tones.
    pub tones: Vec<ToneStatus>,
    /// The bands.
    pub bands: Vec<BandStatus>,
    /// The drives.
    pub drives: Vec<DriveStatus>,
    /// Frames the ring could not supply (played as zero).
    pub underruns: u64,
    /// Frames the step could not hand over.
    pub dropped: u64,
    /// Frames clamped at a peak ceiling.
    pub clamped: u64,
    /// How it ended.
    pub end: Option<RunEnd>,
    /// Each limit channel, in the setup's order.
    pub limits: Vec<LimitStatus>,
}

/// One limit channel's reading.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LimitStatus {
    /// The channel's index in the channel table.
    pub channel_index: u32,
    /// What the channel carries on its worst line (the random part and
    /// every tone on it) over its ceiling, dB; negative infinity before
    /// anything is read. It aborts `limit_abort` past its abort margin.
    pub over_db: f64,
    /// That line's frequency, Hz.
    pub at_hz: f64,
}

/// One band's control state.
pub(crate) struct BandCtl {
    pub(crate) part: usize,
    pub(crate) id: String,
    pub(crate) width: hertzforge_mixed::BandWidth,
    pub(crate) statement: crate::band::BandStatement,
    /// `None` when folded.
    pub(crate) law: Option<Box<dyn crate::band::BandLaw>>,
    pub(crate) setup: Option<BandSetup>,
    pub(crate) mfe: Option<MovingFrameEstimate>,
    pub(crate) drive: Option<HermitianStack>,
    pub(crate) ramp: f64,
    pub(crate) center_hz: f64,
    pub(crate) error_db: Vec<f64>,
    pub(crate) limiting: bool,
    /// The change cap cut the band law's last rise.
    pub(crate) cap_bound: bool,
    /// Its law was held at its last loop (it was ramping in).
    pub(crate) ramp_held: bool,
    pub(crate) alarm_db: Vec<(f64, f64)>,
    pub(crate) abort_db: Vec<(f64, f64)>,
    /// Its center's rate, Hz per second.
    pub(crate) rate_hz_s: f64,
    /// How far ahead the ring was synthesized at the last top-up, seconds.
    pub(crate) ahead_s: f64,
    /// The center its drive was last placed at, Hz.
    pub(crate) drawn_hz: f64,
    /// Loops in a row outside its abort band, per control.
    pub(crate) over_loops: Vec<u32>,
}

/// The MIMO Mixed Mode run service.
pub struct MimoMixedService {
    pub(crate) cfg: MimoMixedConfig,
    pub(crate) table: ChannelTable,
    pub(crate) backend: Box<dyn Backend>,
    pub(crate) station: StationClass,
    pub(crate) laws: MimoMixedLaws,
    pub(crate) floor_clock: Arc<dyn hertzforge_random::service::law::LawClock>,
    pub(crate) tone_clock: Arc<dyn hertzforge_sine::law::LawClock>,
    pub(crate) real_time: bool,
    pub(crate) block_frames: Option<usize>,
    pub(crate) fill_override: Option<Box<dyn FillRule>>,
    pub(crate) guards: GuardSettings,
    pub(crate) state: MimoMixedState,
    pub(crate) phase: MimoMixedPhase,
    pub(crate) end: Option<RunEnd>,
    pub(crate) ending: Option<RunEnd>,
    pub(crate) drain_left: u32,
    pub(crate) parked: bool,
    // The station.
    pub(crate) actual: Option<ConfiguredActual>,
    pub(crate) fs: f64,
    pub(crate) nfft: usize,
    pub(crate) lines: usize,
    pub(crate) df: f64,
    pub(crate) band: (usize, usize),
    pub(crate) n_in: usize,
    pub(crate) slots: Vec<usize>,
    /// The station's output frame: one column per enabled drive row of the
    /// table, in table order; drive `j` of the test plays in column
    /// `drive_slots[j]` and every other column plays zero (MIMO Sine's map).
    pub(crate) n_out: usize,
    pub(crate) drive_slots: Vec<usize>,
    pub(crate) engine: Option<Engine>,
    pub(crate) step: Option<MimoMixedStep>,
    pub(crate) uncounted: Option<StationStart>,
    pub(crate) starts: Vec<StationStart>,
    pub(crate) loop_cfg: Option<LoopConfig>,
    pub(crate) synth: Option<DriveSynth>,
    pub(crate) ident: Option<MimoMixedIdent>,
    pub(crate) gate_passed: Option<bool>,
    // The floor loop.
    pub(crate) spectra: Option<FrameSpectra>,
    pub(crate) est: Option<MatrixFloorEstimator>,
    pub(crate) bufs: Vec<VecDeque<f64>>,
    pub(crate) gains: VecDeque<f64>,
    pub(crate) resp_at: u64,
    pub(crate) gain_at: u64,
    pub(crate) snapshots: VecDeque<(u64, Vec<f64>, Vec<f64>)>,
    pub(crate) frames_in_loop: usize,
    pub(crate) loops: u64,
    pub(crate) floor_law: Option<Box<dyn MimoControlLaw + Send>>,
    pub(crate) floor_needs: Vec<MimoInput>,
    pub(crate) drive_full: Option<HermitianStack>,
    pub(crate) floor_misses: u32,
    pub(crate) fill: Option<Box<dyn FillRule>>,
    pub(crate) floor_ref: Option<HermitianStack>,
    pub(crate) bands: Vec<BandCtl>,
    pub(crate) floor_err_db: Vec<f64>,
    /// Each limit's worst line over its ceiling, dB, and the line.
    pub(crate) limit_over_db: Vec<(f64, usize)>,
    pub(crate) equalized: bool,
    pub(crate) clean_lines: usize,
    pub(crate) fill_report: Option<crate::fill::FillReport>,
    pub(crate) model: Option<MatrixStack>,
    pub(crate) h_l: Option<MatrixStack>,
    pub(crate) factor: Option<hertzforge_random::service::engine::DriveFactor>,
    pub(crate) refactor: bool,
    pub(crate) clip_v: Vec<f64>,
    pub(crate) unsteady: u64,
    pub(crate) ident_rec: Option<ident::IdentRec>,
    pub(crate) forecast: Option<crate::budget::BudgetForecast>,
    // The level.
    pub(crate) level_db: f64,
    pub(crate) rung: usize,
    pub(crate) phase_at_s: f64,
    pub(crate) t_s: f64,
    pub(crate) chunk_rms: Vec<f64>,
    pub(crate) chunk_peak: Vec<f64>,
    // Records.
    pub(crate) run_dir: Option<PathBuf>,
    pub(crate) law_writer: Option<records::RecordWriter>,
    pub(crate) law_kept: Vec<LawRecord>,
    pub(crate) keep_law_records: bool,
    pub(crate) events: Vec<String>,
}

impl std::fmt::Debug for MimoMixedService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MimoMixedService")
            .field("state", &self.state)
            .field("phase", &self.phase)
            .field("fs", &self.fs)
            .finish_non_exhaustive()
    }
}

/// The words for the engine's stop `reason` (MIMO Sine's wording: an
/// output that takes no drive is said to have run dry).
pub fn station_stop_words(reason: &StopReason) -> String {
    match reason {
        StopReason::BackendError(m) if m.starts_with("drive output stalled") => format!(
            "the drive output ran dry: the station played every frame it had been given              before the next was written, and an output that has run dry takes no more              drive, so the run was stopped ({m})"
        ),
        other => format!("{other:?}"),
    }
}

fn plant(e: hertzforge_hal::HalError) -> MimoMixedError {
    MimoMixedError::Plant(e.to_string())
}

/// The test type a tone asks of the tone law.
pub fn tone_test_type(s: &PartSchedule) -> TestType {
    match s {
        PartSchedule::Fixed { .. } => TestType::Dwell,
        _ => TestType::Sweep,
    }
}

impl MimoMixedService {
    /// Build a service for `cfg` against `table` on `backend` bound to
    /// `station`, with `laws`. Validates the configuration at the backend's
    /// rate, that every control and limit is an enabled input and every drive
    /// an enabled drive of `table`, and that the tone law's manifest accepts
    /// every tone (`law_refuses_tone`). Nothing is opened.
    pub fn new(
        cfg: MimoMixedConfig,
        table: ChannelTable,
        backend: Box<dyn Backend>,
        station: StationClass,
        laws: MimoMixedLaws,
    ) -> Result<Self, MimoMixedError> {
        let fs = backend.caps().sample_rate_hz;
        cfg.validate(Some(fs))?;
        let input = |idx: u32| {
            table
                .channels
                .iter()
                .any(|c| c.index == idx && c.enabled && c.role != ChannelRole::Drive)
        };
        for &c in cfg
            .controls
            .iter()
            .chain(cfg.limits.iter().map(|l| &l.channel_index))
        {
            if !input(c) {
                return Err(MimoMixedError::BadState(format!(
                    "channel {c} is not an enabled input of the table"
                )));
            }
        }
        // The station's output columns are the table's enabled drive rows in
        // table order; each drive of the test is mapped to its own row's
        // column, so a test may list its drives in any order or use some of
        // the rows (the others play zero).
        let drive_rows: Vec<u32> = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role == ChannelRole::Drive)
            .map(|c| c.index)
            .collect();
        let mut drive_slots = Vec::with_capacity(cfg.m());
        for d in &cfg.drives {
            let Some(slot) = drive_rows.iter().position(|&r| r == d.channel_index) else {
                return Err(MimoMixedError::BadState(format!(
                    "drive {} is not an enabled drive of the table",
                    d.channel_index
                )));
            };
            if drive_slots.contains(&slot) {
                return Err(MimoMixedError::Refused {
                    name: "duplicate_drive",
                    part: String::new(),
                    why: format!(
                        "drive {} is listed more than once; each shaker is one drive of the test",
                        d.channel_index
                    ),
                });
            }
            drive_slots.push(slot);
        }
        let manifest = laws.tones.manifest();
        let shape = MimoShape::of(cfg.n(), cfg.m());
        for (_, p, t) in cfg.tones() {
            let tt = tone_test_type(&t.schedule);
            if check_law_accepts(&manifest, shape, tt, t.phases_stated).is_err() {
                return Err(MimoMixedConfigError::LawRefusesTone {
                    part: p.id.clone(),
                    law: cfg.law.tones.id.clone(),
                    test_type: tt.as_str().to_string(),
                }
                .into());
            }
        }
        let n = cfg.n();
        let real_time = station.is_hardware();
        Ok(MimoMixedService {
            floor_err_db: vec![f64::NAN; n],
            limit_over_db: vec![(f64::NEG_INFINITY, 0); cfg.p()],
            cfg,
            table,
            backend,
            station,
            laws,
            floor_clock: Arc::new(hertzforge_random::service::law::MonotonicClock::new()),
            tone_clock: Arc::new(hertzforge_sine::law::MonotonicClock::new()),
            real_time,
            block_frames: None,
            fill_override: None,
            guards: GuardSettings::default(),
            state: MimoMixedState::Idle,
            phase: MimoMixedPhase::Idle,
            end: None,
            ending: None,
            drain_left: DRAIN_BATCHES,
            parked: true,
            actual: None,
            fs,
            nfft: 0,
            lines: 0,
            df: 0.0,
            band: (0, 0),
            n_in: 0,
            slots: Vec::new(),
            n_out: drive_rows.len(),
            drive_slots,
            engine: None,
            step: None,
            uncounted: None,
            starts: Vec::new(),
            loop_cfg: None,
            synth: None,
            ident: None,
            gate_passed: None,
            spectra: None,
            est: None,
            bufs: Vec::new(),
            gains: VecDeque::new(),
            resp_at: 0,
            gain_at: 0,
            snapshots: VecDeque::new(),
            frames_in_loop: 0,
            loops: 0,
            floor_law: None,
            floor_needs: Vec::new(),
            drive_full: None,
            floor_misses: 0,
            fill: None,
            floor_ref: None,
            bands: Vec::new(),
            equalized: false,
            clean_lines: 0,
            fill_report: None,
            model: None,
            h_l: None,
            factor: None,
            refactor: true,
            clip_v: Vec::new(),
            unsteady: 0,
            ident_rec: None,
            forecast: None,
            level_db: f64::NEG_INFINITY,
            rung: 0,
            phase_at_s: 0.0,
            t_s: 0.0,
            chunk_rms: Vec::new(),
            chunk_peak: Vec::new(),
            run_dir: None,
            law_writer: None,
            law_kept: Vec::new(),
            keep_law_records: false,
            events: Vec::new(),
        })
    }

    // ---- construction -------------------------------------------------------

    /// The clocks the laws' calls are timed with (a test's late law).
    pub fn with_law_clocks(
        mut self,
        floor: Arc<dyn hertzforge_random::service::law::LawClock>,
        tones: Arc<dyn hertzforge_sine::law::LawClock>,
    ) -> Self {
        self.floor_clock = floor;
        self.tone_clock = tones;
        self
    }

    /// Treat the station as one that runs in real time (the lead-in and its
    /// count), whatever its class says.
    pub fn with_real_time(mut self, on: bool) -> Self {
        self.real_time = on;
        self
    }

    /// The station's block, frames.
    pub fn with_block_frames(mut self, frames: usize) -> Self {
        self.block_frames = Some(frames);
        self
    }

    /// The tone laws' guards (MIMO Sine's settings).
    pub fn with_guards(mut self, g: GuardSettings) -> Self {
        self.guards = g;
        self
    }

    /// A fill rule of the caller's in place of the configured one (a test
    /// seam: the configured rules are chosen by id).
    pub fn with_fill_rule(mut self, rule: Box<dyn FillRule>) -> Self {
        self.fill_override = Some(rule);
        self
    }

    /// Write the law-call record into `dir`, on a writer thread.
    pub fn with_run_dir(mut self, dir: PathBuf) -> Self {
        let json = records::law_json(
            &self.cfg.law.floor.id,
            &self.cfg.law.tones.id,
            self.cfg.tones().count(),
        );
        self.law_writer = Some(records::RecordWriter::start(dir.clone(), json));
        self.run_dir = Some(dir);
        self
    }

    /// Let the law-call record's writer finish what it holds and stop (the
    /// files are whole when this returns). Dropping the service does the
    /// same.
    pub fn close_records(&mut self) {
        if let Some(w) = self.law_writer.as_mut() {
            w.close();
        }
    }

    /// Keep the law-call records for the caller ([`Self::take_law_records`]).
    pub fn keep_law_records(&mut self, keep: bool) {
        self.keep_law_records = keep;
    }

    /// The law-call records kept since the last take.
    pub fn take_law_records(&mut self) -> Vec<LawRecord> {
        std::mem::take(&mut self.law_kept)
    }

    // ---- reading ------------------------------------------------------------

    /// The configuration.
    pub fn config(&self) -> &MimoMixedConfig {
        &self.cfg
    }

    /// The state.
    pub fn state(&self) -> MimoMixedState {
        self.state
    }

    /// The phase.
    pub fn phase(&self) -> MimoMixedPhase {
        self.phase
    }

    /// The rate in force, Hz.
    pub fn sample_rate_hz(&self) -> f64 {
        self.fs
    }

    /// How the run ended.
    pub fn end(&self) -> Option<&RunEnd> {
        self.end.as_ref()
    }

    /// The floor's present estimate on the controls, full level (`None`
    /// before the run), and which lines are clean.
    pub fn floor_measured(&self) -> Option<(HermitianStack, Vec<bool>)> {
        let est = self.est.as_ref()?;
        let n = self.cfg.n();
        let mut g = HermitianStack::zeros(self.lines, n);
        for l in 0..self.lines {
            for r in 0..n {
                for c in r..n {
                    g.set(l, r, c, est.get(l, r, c));
                }
            }
        }
        Some((g, est.clean_mask()))
    }

    /// The floor law's reference: the floor plus every folded band (after
    /// proceed), full level.
    pub fn floor_reference(&self) -> Option<&HermitianStack> {
        self.floor_ref.as_ref()
    }

    /// The setup forecast's worst moment (after proceed).
    pub fn budget_forecast(&self) -> Option<crate::budget::BudgetForecast> {
        self.forecast
    }

    /// The floor's band, lines `[lo, hi)`.
    pub fn band_lines(&self) -> (usize, usize) {
        self.band
    }

    /// Frames the floor loop skipped because the random gain moved across
    /// them.
    pub fn unsteady_frames(&self) -> u64 {
        self.unsteady
    }

    /// The station's starts.
    pub fn starts(&self) -> &[StationStart] {
        &self.starts
    }

    /// The identification.
    pub fn identification(&self) -> Option<&MimoMixedIdent> {
        self.ident.as_ref()
    }

    /// The event log, in words.
    pub fn events(&self) -> &[String] {
        &self.events
    }

    /// Whether the drive is at zero (or the run never drove).
    pub fn drive_is_zero(&self) -> bool {
        self.step.as_ref().is_none_or(|s| s.stopped())
    }

    /// The floor law's drive in force, at full level.
    pub fn floor_drive(&self) -> Option<&HermitianStack> {
        self.drive_full.as_ref()
    }

    /// The fill rule's last report.
    pub fn fill_report(&self) -> Option<crate::fill::FillReport> {
        self.fill_report.clone()
    }

    /// The step (the cost measure and the tests read it).
    pub fn step(&self) -> Option<&MimoMixedStep> {
        self.step.as_ref()
    }

    pub(crate) fn note(&mut self, s: impl Into<String>) {
        self.events.push(format!("{:.2} s: {}", self.t_s, s.into()));
    }

    // ---- the station --------------------------------------------------------

    /// Open, configure, gate and arm the station; build the grid. Nothing
    /// plays.
    pub fn arm(&mut self) -> Result<(), MimoMixedError> {
        if self.state != MimoMixedState::Idle {
            return Err(MimoMixedError::BadState("arm once, before the run".into()));
        }
        self.backend.open().map_err(plant)?;
        let block = self.block_frames.unwrap_or_else(|| {
            if self.station.is_hardware() {
                hertzforge_engine::default_block_frames(self.fs)
            } else {
                hertzforge_engine::SIM_NATURAL_BLOCK_FRAMES
            }
        });
        let acq = AcquisitionConfig::new(self.fs, block);
        let actual = self.backend.configure(&self.table, &acq).map_err(plant)?;
        let fs = actual.actual_rate_hz;
        self.cfg.validate(Some(fs))?;
        hertzforge_module_abi::arm_gate(
            &self.backend.caps(),
            self.station,
            hertzforge_module_abi::Resolution::host_closed_loop(),
            None,
            fs,
        )
        .map_err(|r| MimoMixedError::BadState(format!("arm refused: {r}")))?;
        // The grid is the reference's: the frame length that puts its lines
        // at the station's rate.
        let df = self.cfg.df_hz();
        let lines = self.cfg.lines();
        let nfft = 2 * (lines - 1);
        if ((fs / nfft as f64) - df).abs() > 1e-9 * df || !nfft.is_power_of_two() {
            return Err(MimoMixedError::Plant(format!(
                "the station runs at {fs} Hz; the reference's {lines} lines of {df} Hz need \
                 {} Hz with a power-of-two frame; rebuild the reference on the station's grid",
                df * nfft as f64
            )));
        }
        if self.cfg.tones().next().is_some() && df > self.cfg.exclusion_hz / 2.0 {
            return Err(MimoMixedError::Config(MimoMixedConfigError::Parts(
                hertzforge_mixed::ConfigError::BadConfig {
                    field: "exclusion_hz",
                    why: format!(
                        "the exclusion covers at least two lines: lines of {df:.3} Hz need \
                         exclusion_hz of at least {:.3} Hz",
                        2.0 * df
                    ),
                },
            )));
        }
        // Input frame layout: enabled non-drive rows in table order.
        let mut slot_of = std::collections::HashMap::new();
        let mut n_in = 0usize;
        for ch in &self.table.channels {
            if ch.enabled && ch.role != ChannelRole::Drive {
                slot_of.insert(ch.index, n_in);
                n_in += 1;
            }
        }
        self.slots = self
            .cfg
            .controls
            .iter()
            .chain(self.cfg.limits.iter().map(|l| &l.channel_index))
            .map(|c| slot_of[c])
            .collect();
        self.n_in = n_in.max(1);
        self.backend.arm().map_err(plant)?;
        let band = self.cfg.reference.band;
        let mut lc = LoopConfig::demo_default();
        lc.fs_hz = fs;
        lc.nfft = nfft;
        lc.overlap = 0.5;
        lc.band_lo_hz = band.0 as f64 * df;
        lc.band_hi_hz = band.1.saturating_sub(1) as f64 * df;
        lc.cola_seed = self.cfg.seed;
        lc.block_frames = actual.block_frames;
        self.fs = fs;
        self.nfft = nfft;
        self.lines = lines;
        self.df = df;
        self.band = (band.0.max(1), band.1.min(lines));
        self.loop_cfg = Some(lc);
        self.actual = Some(actual);
        self.state = MimoMixedState::Armed;
        self.note(format!(
            "Armed: {} controls, {} drives, {} limits, {lines} lines of {df} Hz at {fs} Hz.",
            self.cfg.n(),
            self.cfg.m(),
            self.cfg.p()
        ));
        Ok(())
    }

    /// A fresh engine for the phase.
    pub(crate) fn fresh_engine(&mut self) -> Result<(), MimoMixedError> {
        let actual = self
            .actual
            .ok_or_else(|| MimoMixedError::BadState("the station is not armed".into()))?;
        let mut ecfg = EngineConfig::with_block(self.n_in, self.n_out, &actual);
        if !self.station.is_hardware() {
            ecfg.jitter_frac = hertzforge_engine::TEST_DISABLE_JITTER_GUARD;
            ecfg.compute_budget = Some(hertzforge_engine::TEST_RELAX_COMPUTE_BUDGET);
        }
        self.engine = Some(Engine::new(ecfg, &self.table, self.backend.as_ref()));
        Ok(())
    }

    /// Bring a parked station back (MIMO Sine's re-engage: close, open,
    /// configure at the same rate, arm).
    pub(crate) fn unpark(&mut self) -> Result<(), MimoMixedError> {
        if !self.parked || self.starts.is_empty() && self.uncounted.is_none() {
            return Ok(());
        }
        let actual = self
            .actual
            .ok_or_else(|| MimoMixedError::BadState("the station is not armed".into()))?;
        let _ = self.backend.close();
        self.backend.open().map_err(plant)?;
        let acq = AcquisitionConfig::new(actual.actual_rate_hz, actual.block_frames);
        let again = self.backend.configure(&self.table, &acq).map_err(plant)?;
        if again.actual_rate_hz != actual.actual_rate_hz {
            return Err(MimoMixedError::Plant(format!(
                "the station came back at {} Hz, not {} Hz",
                again.actual_rate_hz, actual.actual_rate_hz
            )));
        }
        self.backend.arm().map_err(plant)?;
        Ok(())
    }

    /// Start the station for `phase` (MIMO Sine's start): on a real-time
    /// station the lead-in of zeros, its count checked, the engine told it
    /// was prefilled; otherwise the start is counted after the first batch.
    pub(crate) fn start_station(&mut self, phase: &'static str) -> Result<(), MimoMixedError> {
        // The lead-in is written in the station's own output frame.
        let m = self.n_out;
        let block = self.actual.map_or(1, |a| a.block_frames.max(1));
        let engine = self
            .engine
            .as_mut()
            .ok_or_else(|| MimoMixedError::BadState("no engine for the phase".into()))?;
        let mut start = StationStart {
            phase,
            lead_in_frames: 0,
            prefill_frames: 0,
            queue_frames: engine.health().output_queued,
        };
        if self.real_time {
            let lookahead = self
                .backend
                .caps()
                .output_buffer
                .map_or(0, |o| o.min_lookahead_frames as usize);
            let room = self.backend.output_space();
            let want = match room {
                Some(r) if r > 0 => (lookahead + block).min(r),
                _ => lookahead + block,
            };
            let zeros = vec![0.0f32; want * m.max(1)];
            let mut written = 0usize;
            while written < want {
                match self.backend.write_drive(&zeros[written * m..]) {
                    Ok(0) => break,
                    Ok(k) => written += k.min(want - written),
                    Err(e) => return Err(plant(e)),
                }
            }
            if written < want || written < lookahead {
                return Err(MimoMixedError::Plant(format!(
                    "the station took {written} of the {want} frames written ahead of the start \
                     of the {phase} phase, under the {lookahead} it must hold to start; the \
                     frames ahead of the first drive frame would not be the ones counted, so it \
                     was not started"
                )));
            }
            if let Some(r) = room.filter(|r| *r > 0) {
                let after = self.backend.output_space();
                if after != Some(r - written) {
                    return Err(MimoMixedError::Plant(format!(
                        "the station reported {after:?} frames of room after taking {written} of \
                         {r}; it holds frames the service did not write, so the {phase} phase \
                         was not started"
                    )));
                }
            }
            engine.mark_output_prefilled();
            start.lead_in_frames = written;
            self.check_start(&start)?;
            self.starts.push(start);
        } else {
            self.uncounted = Some(start);
        }
        self.backend.start().map_err(plant)?;
        self.parked = false;
        Ok(())
    }

    /// A start the station refused: the step and the synthesis dropped (no
    /// drive was taken by a started station), the station parked, the run
    /// ended `start_refused`.
    pub(crate) fn refuse_start(&mut self, e: MimoMixedError) -> MimoMixedError {
        self.step = None;
        self.synth = None;
        self.ident_rec = None;
        self.park();
        self.state = MimoMixedState::Ended;
        self.end = Some(RunEnd {
            name: "start_refused",
            aborted: true,
            part: None,
            control: None,
            drive: None,
            words: e.to_string(),
        });
        e
    }

    /// An engaged start must hold the frames ahead the identification
    /// measured its lag with.
    pub(crate) fn check_start(&self, start: &StationStart) -> Result<(), MimoMixedError> {
        if start.phase != "engaged" {
            return Ok(());
        }
        let Some(id) = self.ident.as_ref() else {
            return Ok(());
        };
        if start.frames_ahead() != id.frames_ahead {
            return Err(MimoMixedError::Plant(format!(
                "the engaged run would start with {} frames ahead of its first drive frame and \
                 the identification measured its lag with {}; every phase the run reads would \
                 be off by the difference, so it was not run",
                start.frames_ahead(),
                id.frames_ahead
            )));
        }
        Ok(())
    }

    fn count_start(&mut self) -> Result<(), MimoMixedError> {
        let Some(mut start) = self.uncounted.take() else {
            return Ok(());
        };
        start.prefill_frames = self
            .engine
            .as_ref()
            .map_or(0, |e| e.output_prefill_frames());
        self.starts.push(start);
        self.check_start(&start)
    }

    /// Park: the station's tasks stop with nothing playing.
    pub(crate) fn park(&mut self) {
        let _ = self.backend.stop();
        self.engine = None;
        self.parked = true;
    }

    /// One bounded batch of engine ticks under `catch_unwind`. `Ok(false)`
    /// when the station stopped the run (the run has ended).
    pub(crate) fn run_batch(&mut self) -> Result<bool, MimoMixedError> {
        let (Some(engine), Some(step)) = (self.engine.as_mut(), self.step.as_mut()) else {
            return Err(MimoMixedError::BadState(
                "the station is not running".into(),
            ));
        };
        let backend = &mut self.backend;
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.run_ticks(
                backend.as_mut(),
                step,
                &mut VecCaptureSink::new(),
                TICKS_PER_BATCH,
            )
        }));
        let report = match caught {
            Ok(r) => r,
            Err(_) => {
                let _ = self.backend.emergency_stop();
                self.engine = None;
                self.finish(RunEnd {
                    name: "run_thread_failure",
                    aborted: true,
                    part: None,
                    control: None,
                    drive: None,
                    words: "the run's step panicked; the station was stopped at once".into(),
                });
                return Ok(false);
            }
        };
        self.count_start()?;
        if report.reason != StopReason::InputEnded {
            let words = match &report.reason {
                StopReason::AbortTripped(t) => format!(
                    "the station's watchdog tripped on channel {} ({} against {})",
                    t.channel_index, t.value, t.limit
                ),
                other => station_stop_words(other),
            };
            let _ = self.backend.emergency_stop();
            self.engine = None;
            self.finish(RunEnd {
                name: "station_stop",
                aborted: true,
                part: None,
                control: None,
                drive: None,
                words,
            });
            return Ok(false);
        }
        Ok(true)
    }

    /// End the run: the hardware is freed first, then the run reports
    /// itself ended.
    pub(crate) fn finish(&mut self, end: RunEnd) {
        let _ = self.backend.stop();
        self.engine = None;
        self.parked = true;
        self.note(format!("Ended: {} ({})", end.name, end.words));
        self.flush_law_records();
        self.end = Some(end);
        self.state = MimoMixedState::Ended;
        self.phase = MimoMixedPhase::Stopping;
    }

    /// Ramp the drive to zero on the way to `end`. An abort replaces an
    /// ending that is not one (a stop ramping down) and shortens its ramp
    /// to the abort ramp; anything else keeps the ending already set.
    pub(crate) fn begin_end(&mut self, end: RunEnd, ramp_s: f64) {
        if self.state == MimoMixedState::Ended {
            return;
        }
        if let Some(e) = self.ending.as_ref() {
            if e.aborted || !end.aborted {
                return;
            }
        }
        if let Some(s) = self.step.as_mut() {
            s.ramp_down(ramp_s.max(ABORT_RAMP_S));
        }
        self.note(format!("Ending: {} ({})", end.name, end.words));
        self.ending = Some(end);
        self.phase = MimoMixedPhase::Stopping;
    }

    /// An abort: the abort ramp, the end named.
    pub(crate) fn abort_with(&mut self, end: RunEnd) {
        self.begin_end(end, ABORT_RAMP_S);
    }

    // ---- the gate and the review ----------------------------------------------

    /// The coherence gate: every control's mean multiple coherence over the
    /// band at or above the gate's threshold.
    pub fn coherence_gate(&mut self) -> Result<bool, MimoMixedError> {
        let id = self
            .ident
            .as_ref()
            .ok_or_else(|| MimoMixedError::BadState("identify first".into()))?;
        let thr = self.cfg.gates.coherence_threshold;
        let ok = id.band_coherence.iter().all(|c| *c >= thr);
        let words = format!(
            "Coherence gate {}: per control {:?} against {thr}.",
            if ok { "passed" } else { "failed" },
            id.band_coherence
        );
        self.note(words);
        self.gate_passed = Some(ok);
        if ok {
            self.state = MimoMixedState::Review;
        }
        Ok(ok)
    }

    /// Proceed after the review: the setup forecast, the laws built and
    /// seeded, the step built, the station restarted; the floor plays alone
    /// at the soft-start level.
    pub fn proceed(&mut self) -> Result<(), MimoMixedError> {
        if self.state != MimoMixedState::Review {
            return Err(MimoMixedError::BadState(
                "proceed after the identification and a passed gate".into(),
            ));
        }
        self.engage()
    }

    // ---- the verbs ------------------------------------------------------------

    fn running(&self) -> Result<(), MimoMixedError> {
        if self.state == MimoMixedState::Running && self.ending.is_none() {
            Ok(())
        } else {
            Err(MimoMixedError::BadState("the test is not running".into()))
        }
    }

    /// Set the level, dB (manual mode, or a step from the schedule).
    pub fn set_level_db(&mut self, db: f64) -> Result<f64, MimoMixedError> {
        self.running()?;
        if !db.is_finite() {
            return Err(MimoMixedError::BadState("a level in dB".into()));
        }
        let db = db.min(crate::budget::max_level(&self.cfg).log10() * 20.0);
        self.go_level(db);
        Ok(db)
    }

    /// Step the level by `delta_db`.
    pub fn step_level(&mut self, delta_db: f64) -> Result<f64, MimoMixedError> {
        let now = self.level_db;
        self.set_level_db(now + delta_db)
    }

    /// Hold: the parts' clock stops (the drive keeps playing).
    pub fn hold(&mut self) -> Result<(), MimoMixedError> {
        self.running()?;
        if let Some(s) = self.step.as_mut() {
            s.clock.hold();
        }
        self.note("Held.");
        Ok(())
    }

    /// Resume the parts' clock.
    pub fn resume(&mut self) -> Result<(), MimoMixedError> {
        self.running()?;
        if let Some(s) = self.step.as_mut() {
            s.clock.resume();
        }
        self.note("Resumed.");
        Ok(())
    }

    /// Switch a part on or off live, as a ramp. A folded band refuses
    /// (`band_folded`): its level is in the floor's reference.
    pub fn set_part_enabled(&mut self, part: &str, on: bool) -> Result<(), MimoMixedError> {
        self.running()?;
        let Some(i) = self.cfg.parts.iter().position(|p| p.id == part) else {
            return Err(MimoMixedError::Refused {
                name: "unknown_part",
                part: part.into(),
                why: "no part by that id".into(),
            });
        };
        if let Some(b) = self.bands.iter().find(|b| b.part == i) {
            if b.law.is_none() {
                return Err(MimoMixedError::Refused {
                    name: "band_folded",
                    part: part.into(),
                    why: "the band is folded into the floor's reference and cannot be switched \
                          while the test runs; name a band law to switch it live"
                        .into(),
                });
            }
        }
        if matches!(self.cfg.parts[i].body, MimoPartBody::Floor(_)) {
            if let Some(s) = self.step.as_mut() {
                let frames = (self.cfg.part_ramp_s * self.fs).max(1.0);
                s.go_random(if on { 1.0 } else { 0.0 }, frames);
            }
        } else if let Some(s) = self.step.as_mut() {
            s.clock.set_enabled(i, on);
        }
        self.note(format!(
            "Part {part} switched {}.",
            if on { "on" } else { "off" }
        ));
        Ok(())
    }

    /// The operator's stop: the drive ramps down over the shutdown ramp.
    pub fn stop(&mut self) -> Result<(), MimoMixedError> {
        self.running()?;
        let ramp = self.cfg.shutdown_ramp_s.max(ABORT_RAMP_S);
        self.begin_end(
            RunEnd {
                name: "stopped",
                aborted: false,
                part: None,
                control: None,
                drive: None,
                words: "the operator stopped the test".into(),
            },
            ramp,
        );
        Ok(())
    }

    /// The operator's abort: the abort ramp. Taken while a stop ramps down
    /// too: the abort replaces the stop and shortens its ramp.
    pub fn abort(&mut self, reason: &str) -> Result<(), MimoMixedError> {
        self.running_or_ending()?;
        self.abort_with(RunEnd {
            name: "operator_abort",
            aborted: true,
            part: None,
            control: None,
            drive: None,
            words: reason.to_string(),
        });
        Ok(())
    }

    // ---- the run ----------------------------------------------------------------

    /// Run about [`CHUNK_S`] of station time. `Ok(false)` once the run has
    /// ended (the hardware is free).
    pub fn run_chunk(&mut self) -> Result<bool, MimoMixedError> {
        if self.state == MimoMixedState::Ended {
            return Ok(false);
        }
        self.running_or_ending()?;
        let block = self.actual.map_or(256, |a| a.block_frames.max(1));
        let batch = TICKS_PER_BATCH as usize * block;
        let batches = ((CHUNK_S * self.fs / batch as f64).ceil() as u32).max(1);
        for _ in 0..batches {
            match self.run_one() {
                Ok(true) => {}
                Ok(false) => return Ok(false),
                Err(e) => return Err(self.fail_run(e)),
            }
        }
        Ok(self.state != MimoMixedState::Ended)
    }

    /// Run for `seconds` of station time; `Ok(false)` once ended.
    pub fn run_for(&mut self, seconds: f64) -> Result<bool, MimoMixedError> {
        let chunks = (seconds / CHUNK_S).ceil().max(1.0) as usize;
        for _ in 0..chunks {
            if !self.run_chunk()? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn running_or_ending(&self) -> Result<(), MimoMixedError> {
        if self.state == MimoMixedState::Running {
            Ok(())
        } else {
            Err(MimoMixedError::BadState("the test is not running".into()))
        }
    }

    /// One batch: top up, play, take what the step handed over, judge,
    /// advance.
    fn run_one(&mut self) -> Result<bool, MimoMixedError> {
        self.top_up()?;
        if !self.run_batch()? {
            return Ok(false);
        }
        let frames = self.take_records();
        self.t_s += frames as f64 / self.fs;
        if let Some(t) = self.step.as_ref().and_then(|s| s.trip.clone()) {
            let end = self.step_end(&t);
            if self.ending.as_ref().is_none_or(|e| !e.aborted) {
                self.note(format!("Trip: {} ({})", end.name, end.words));
                self.ending = Some(end);
                self.phase = MimoMixedPhase::Stopping;
            }
        }
        if let Some(t) = self.judge_drive_rms() {
            self.abort_with(t);
        }
        self.judge_limits();
        if self.ending.is_none() {
            self.advance();
        }
        if self.ending.is_some() && self.step.as_ref().is_some_and(|s| s.stopped()) {
            if self.drain_left == 0 {
                let end = self.ending.take().expect("ending");
                self.finish(end);
                return Ok(false);
            }
            self.drain_left -= 1;
        }
        Ok(true)
    }

    /// The run end a step trip names.
    fn step_end(&self, t: &StepTrip) -> RunEnd {
        let tone_id = |k: usize| {
            self.step
                .as_ref()
                .and_then(|s| s.tones.get(k))
                .map(|tc| self.cfg.parts[tc.part].id.clone())
        };
        match t {
            StepTrip::DriveVpk { drive, volts } => RunEnd {
                name: "drive_vpk",
                aborted: true,
                part: None,
                control: None,
                drive: Some(*drive),
                words: format!(
                    "drive {drive}'s demanded sum reached {volts:.3} V against its {:.3} V peak",
                    self.cfg.drives[*drive].peak_v
                ),
            },
            StepTrip::DriveNotFinite { drive } => RunEnd {
                name: "drive_not_finite",
                aborted: true,
                part: None,
                control: None,
                drive: Some(*drive),
                words: format!("drive {drive}'s demanded sum was not a number; it played 0"),
            },
            StepTrip::RandomUnderrun { frames } => RunEnd {
                name: "random_underrun",
                aborted: true,
                part: None,
                control: None,
                drive: None,
                words: format!("the random drive ran dry for {frames} frames in all"),
            },
            StepTrip::Overload {
                channel_index,
                value,
            } => RunEnd {
                name: "input_overload",
                aborted: true,
                part: None,
                control: self.cfg.controls.iter().position(|c| c == channel_index),
                drive: None,
                words: format!("channel {channel_index} read {value:.3}, at its overload level"),
            },
            StepTrip::Kinematic {
                channel_index,
                value,
                limit,
            } => RunEnd {
                name: "kinematic",
                aborted: true,
                part: None,
                control: None,
                drive: None,
                words: format!("channel {channel_index}: {value:.3} against {limit:.3}"),
            },
            StepTrip::ToneAbort {
                tone,
                control,
                error_db,
                limiting,
            } => {
                let id = tone_id(*tone);
                RunEnd {
                    name: if *limiting {
                        "tone_abort_at_ceiling"
                    } else {
                        "tone_abort"
                    },
                    aborted: true,
                    control: Some(*control),
                    drive: None,
                    words: if *limiting {
                        format!(
                            "tone {} read {error_db:.2} dB on control {control} while the \
                             shared drive ceiling held it",
                            id.clone().unwrap_or_default()
                        )
                    } else {
                        format!(
                            "tone {} read {error_db:.2} dB on control {control}, outside its \
                             abort band",
                            id.clone().unwrap_or_default()
                        )
                    },
                    part: id,
                }
            }
            StepTrip::ToneLaw { tone, fault } => {
                let id = tone_id(*tone);
                RunEnd {
                    name: "law_fault",
                    aborted: true,
                    control: None,
                    drive: None,
                    words: format!(
                        "the tone law of {} faulted: {fault}; the controlled stop",
                        id.clone().unwrap_or_default()
                    ),
                    part: id,
                }
            }
            StepTrip::ToneDivergence { tone } => {
                let id = tone_id(*tone);
                RunEnd {
                    name: "divergence",
                    aborted: true,
                    control: None,
                    drive: None,
                    words: format!(
                        "the tone law of {} kept asking past the change caps",
                        id.clone().unwrap_or_default()
                    ),
                    part: id,
                }
            }
            StepTrip::ToneUnreadable { tone, seconds } => {
                let id = tone_id(*tone);
                RunEnd {
                    name: "tone_unreadable",
                    aborted: true,
                    control: None,
                    drive: None,
                    words: format!(
                        "tone {} had no usable reading for {seconds:.1} s with its ramps done: \
                         its drive was played open loop",
                        id.clone().unwrap_or_default()
                    ),
                    part: id,
                }
            }
        }
    }

    /// The level schedule and the phases.
    fn advance(&mut self) {
        let since = self.t_s - self.phase_at_s;
        match self.phase {
            MimoMixedPhase::FloorAlone => {
                if self.equalized {
                    self.phase = MimoMixedPhase::PartsIn;
                    self.phase_at_s = self.t_s;
                    if let Some(s) = self.step.as_mut() {
                        s.parts_live = true;
                    }
                    let start = self.cfg.schedule.start_db;
                    self.go_level(start);
                    self.note("The floor is equalized; the parts come in.");
                } else if since > self.cfg.floor_settle_s {
                    self.abort_with(RunEnd {
                        name: "floor_not_equalized",
                        aborted: true,
                        part: self.floor_id(),
                        control: None,
                        drive: None,
                        words: format!(
                            "the floor did not equalize within {:.1} s (errors {:?} dB)",
                            self.cfg.floor_settle_s, self.floor_err_db
                        ),
                    });
                }
            }
            MimoMixedPhase::PartsIn => {
                if since >= self.cfg.part_ramp_s + self.cfg.level_ramp_s {
                    self.phase = MimoMixedPhase::Schedule;
                    self.phase_at_s = self.t_s;
                    self.rung = 0;
                }
            }
            MimoMixedPhase::Schedule => {
                if self.cfg.schedule.mode != hertzforge_random::RunMode::Auto {
                    return;
                }
                let rungs: Vec<f64> = self
                    .cfg
                    .schedule
                    .table_db
                    .iter()
                    .copied()
                    .filter(|&d| d > self.cfg.schedule.start_db && d < 0.0)
                    .collect();
                let dwell = self.cfg.schedule.dwell_for(self.rung);
                if since >= dwell + self.cfg.level_ramp_s {
                    if self.rung < rungs.len() {
                        let d = rungs[self.rung];
                        self.rung += 1;
                        self.go_level(d);
                    } else {
                        self.go_level(0.0);
                        self.phase = MimoMixedPhase::FullLevel;
                    }
                    self.phase_at_s = self.t_s;
                }
            }
            MimoMixedPhase::FullLevel => {
                if let Some(d) = self.cfg.full_level_duration_s {
                    if since >= d + self.cfg.level_ramp_s {
                        let ramp = self.cfg.shutdown_ramp_s.max(ABORT_RAMP_S);
                        self.begin_end(
                            RunEnd {
                                name: "completed",
                                aborted: false,
                                part: None,
                                control: None,
                                drive: None,
                                words: "the schedule ran to its end".into(),
                            },
                            ramp,
                        );
                    }
                }
            }
            MimoMixedPhase::Idle | MimoMixedPhase::Stopping => {}
        }
    }

    pub(crate) fn floor_id(&self) -> Option<String> {
        self.cfg
            .parts
            .iter()
            .find(|p| matches!(p.body, MimoPartBody::Floor(_)))
            .map(|p| p.id.clone())
    }

    /// Ramp the level to `db` over the level ramp.
    pub(crate) fn go_level(&mut self, db: f64) {
        self.level_db = db;
        let frames = (self.cfg.level_ramp_s * self.fs).max(1.0);
        if let Some(s) = self.step.as_mut() {
            s.go_level(10f64.powf(db / 20.0), frames);
        }
        self.note(format!("Level {db:.2} dB."));
    }

    /// The status.
    pub fn status(&self) -> MimoMixedStatus {
        let n = self.cfg.n();
        let mut tones = Vec::new();
        if let Some(s) = self.step.as_ref() {
            let lam = s.level.value();
            for (k, tc) in s.tones.iter().enumerate() {
                let f = s.bank.frequency(k);
                let line = ((f / self.df.max(1e-12)).round() as usize).min(self.lines.max(1) - 1);
                tones.push(ToneStatus {
                    id: self.cfg.parts[tc.part].id.clone(),
                    frequency_hz: f,
                    part_level: s.bank.part_level(k),
                    engaged: tc.engaged,
                    usable: tc.usable,
                    limiting: tc.limiting,
                    scale: tc.scale,
                    error_db: tc.err_db.clone(),
                    alarm: tc.alarm.clone(),
                    measured: (0..n).map(|i| s.bank.tracked(k, i).norm()).collect(),
                    target: (0..n)
                        .map(|i| tc.reference[i * self.lines + line].norm() * lam)
                        .collect(),
                });
            }
        }
        let drives = (0..self.cfg.m())
            .map(|j| {
                let tone_sum_v = self.step.as_ref().map_or(0.0, |s| {
                    let mut d = vec![C64::new(0.0, 0.0); self.cfg.m()];
                    (0..s.tones.len())
                        .map(|k| {
                            s.bank.drive_in_force(k, &mut d);
                            d[j].norm() * s.bank.part_level(k) * s.level.value()
                        })
                        .sum()
                });
                DriveStatus {
                    random_rms_v: self.chunk_rms.get(j).copied().unwrap_or(0.0),
                    peak_v: self.chunk_peak.get(j).copied().unwrap_or(0.0),
                    tone_sum_v,
                    abort_vpk: self
                        .step
                        .as_ref()
                        .and_then(|s| s.abort_v.get(j).copied())
                        .unwrap_or_else(|| self.cfg.drives[j].abort_level_vpk()),
                }
            })
            .collect();
        MimoMixedStatus {
            state: self.state,
            phase: self.phase,
            t_s: self.t_s,
            level_db: self.level_db,
            floor_error_db: self.floor_err_db.clone(),
            floor_equalized: self.equalized,
            loops: self.loops,
            floor_misses: self.floor_misses,
            tones,
            bands: self
                .bands
                .iter()
                .map(|b| BandStatus {
                    id: b.id.clone(),
                    folded: b.law.is_none(),
                    center_hz: b.center_hz,
                    part_level: b.ramp,
                    error_db: b.error_db.clone(),
                    alarm: b
                        .error_db
                        .iter()
                        .zip(&b.alarm_db)
                        .map(|(e, (lo, hi))| e.is_finite() && (*e < -lo || *e > *hi))
                        .collect(),
                    limiting: b.limiting,
                })
                .collect(),
            drives,
            underruns: self.step.as_ref().map_or(0, |s| s.underruns),
            dropped: self.step.as_ref().map_or(0, |s| s.records.dropped),
            clamped: self.step.as_ref().map_or(0, |s| s.clamped),
            end: self.end.clone(),
            limits: self
                .cfg
                .limits
                .iter()
                .zip(&self.limit_over_db)
                .map(|(l, &(db, line))| LimitStatus {
                    channel_index: l.channel_index,
                    over_db: db,
                    at_hz: line as f64 * self.df,
                })
                .collect(),
        }
    }

    /// The law-call records: into the caller's keep and the run's folder.
    pub(crate) fn flush_law_records(&mut self) {
        let Some(s) = self.step.as_mut() else { return };
        if s.records.law.is_empty() {
            return;
        }
        let recs: Vec<LawRecord> = s.records.law.clone();
        s.records.law.clear();
        self.push_law_records(&recs);
    }

    pub(crate) fn push_law_records(&mut self, recs: &[LawRecord]) {
        let failed = match self.law_writer.as_ref() {
            Some(w) => {
                let mut bytes = Vec::with_capacity(recs.len() * LawRecord::BYTES);
                for r in recs {
                    r.write_to(&mut bytes);
                }
                w.send(bytes);
                w.failures()
            }
            None => Vec::new(),
        };
        for f in failed {
            self.note(f);
        }
        if self.keep_law_records {
            self.law_kept.extend_from_slice(recs);
        }
    }

    // ---- engage -----------------------------------------------------------------

    fn engage(&mut self) -> Result<(), MimoMixedError> {
        let id = self
            .ident
            .clone()
            .ok_or_else(|| MimoMixedError::BadState("identify first".into()))?;
        let (n, m, p) = (self.cfg.n(), self.cfg.m(), self.cfg.p());
        let q = n + p;
        let lines = self.lines;
        let band = self.band;
        let model = rows(&id.h, 0..n);
        // The setup forecast.
        let pred = crate::predict::predict(&self.cfg, &model)?;
        let horizon = self.schedule_horizon_s();
        let fc = crate::budget::forecast(
            &self.cfg,
            &model,
            &pred.random_peaks_v(),
            FORECAST_STEP_S,
            horizon,
        )?;
        self.forecast = Some(fc);
        // The floor's reference with the folded bands; the bands.
        self.build_bands()?;
        let floor_ref = self.loop_reference();
        self.floor_ref = Some(floor_ref.clone());
        self.model = Some(model.clone());
        self.h_l = Some(rows(&id.h, n..q));
        // The floor law, seeded.
        self.engage_floor(&id, &model, &floor_ref)?;
        // One tone law per tone.
        let tones = self.build_tones(&id, &model)?;
        let f0: Vec<f64> = {
            let proj = self.cfg.projection(0);
            let mut clock = PartClock::new(&proj)?;
            let now = clock.tick(0.0).to_vec();
            tones
                .iter()
                .map(|t| now[t.part].freq_hz.unwrap_or(0.0))
                .collect()
        };
        let locks = vec![ToneLock::Free; tones.len()];
        let bank = ToneBank::new(
            self.fs,
            &f0,
            &locks,
            m,
            q,
            self.cfg.tracking_hz,
            id.lag_frames,
        )
        .map_err(|e| MimoMixedError::BadState(e.to_string()))?;
        let mut step = self.build_step(bank, tones)?;
        step.tones_run = true;
        let l0 = self.cfg.schedule.start_db - self.cfg.soft_start_db;
        step.level = Ramp::at(10f64.powf(l0 / 20.0));
        step.rand_ramp = Ramp::at(0.0);
        step.rand_ramp.go(1.0, RANDOM_RAMP_S * self.fs);
        self.level_db = l0;
        self.step = Some(step);
        self.reset_loop()?;
        let lc = self.loop_cfg.clone().expect("armed");
        self.synth = Some(DriveSynth::with_outputs(&lc, m));
        self.top_up()?;
        // The station back, started as MIMO Sine starts it.
        self.unpark()?;
        self.fresh_engine()?;
        if let Err(e) = self.start_station("engaged") {
            return Err(self.refuse_start(e));
        }
        self.state = MimoMixedState::Running;
        self.phase = MimoMixedPhase::FloorAlone;
        self.phase_at_s = 0.0;
        self.t_s = 0.0;
        self.note(format!("Engaged at {l0:.1} dB, the floor alone."));
        let _ = band;
        let _ = lines;
        Ok(())
    }

    /// The schedule's length, seconds (the forecast's horizon).
    fn schedule_horizon_s(&self) -> f64 {
        let s = &self.cfg.schedule;
        let rungs = s.table_db.len();
        let walk: f64 = (0..rungs).map(|i| s.dwell_for(i)).sum();
        self.cfg.part_ramp_s + walk + self.cfg.full_level_duration_s.unwrap_or(60.0)
    }

    /// Build the step: the slots, the guards, the ring and the buffers.
    pub(crate) fn build_step(
        &mut self,
        bank: ToneBank,
        tones: Vec<ToneCtl>,
    ) -> Result<MimoMixedStep, MimoMixedError> {
        let (n, m, p) = (self.cfg.n(), self.cfg.m(), self.cfg.p());
        let block = self.actual.map_or(256, |a| a.block_frames.max(1));
        let chunk = (CHUNK_S * self.fs) as usize;
        let mut floor_diag = vec![0.0; self.lines * n];
        for l in 0..self.lines {
            for i in 0..n {
                floor_diag[l * n + i] = self.cfg.reference.sdm.get(l, i, i).re.max(0.0);
            }
        }
        let rule = crate::budget::ceiling_rule(&self.cfg.law.ceiling_rule)?;
        let clock = PartClock::new(&self.cfg.projection(0))?;
        let shape = StepShape {
            fs: self.fs,
            n,
            m,
            p,
            n_in: self.n_in,
            slots: self.slots.clone(),
            n_out: self.n_out,
            out_slots: self.drive_slots.clone(),
            peak_v: self.cfg.drives.iter().map(|d| d.peak_v).collect(),
            abort_v: self
                .cfg
                .drives
                .iter()
                .map(|d| d.abort_level_vpk())
                .collect(),
            tick_s: self.cfg.control_tick_s(),
            ring_frames: 4 * chunk + 4 * self.nfft,
            record_frames: 2 * TICKS_PER_BATCH as usize * block + 4096,
            max_block: TICKS_PER_BATCH as usize * block * 2,
            floor_diag,
            df: self.df,
            lines: self.lines,
            part_ramp_s: self.cfg.part_ramp_s,
        };
        let mut step = MimoMixedStep::new(
            shape,
            bank,
            tones,
            clock,
            rule,
            self.guards.clone(),
            self.tone_clock.clone(),
        );
        let at = self.cfg.protections.overload_fraction;
        for (k, &c) in self
            .cfg
            .controls
            .iter()
            .chain(self.cfg.limits.iter().map(|l| &l.channel_index))
            .enumerate()
        {
            let range = self
                .table
                .channels
                .iter()
                .find(|ch| ch.index == c)
                .map_or(0.0, |ch| ch.range_max);
            if range > 0.0 {
                step.overload.push((self.slots[k], at * range, c));
            }
        }
        if self.guards.kinematic_monitor {
            let kin = hertzforge_random::service::engine::KinematicMonitor::new(
                &self.table,
                self.fs,
                5.0,
                1,
                5.0,
            );
            step.kin = kin.is_active().then_some(kin);
        }
        Ok(step)
    }

    /// Each tone's law, built from the identification (MIMO Sine's setup).
    fn build_tones(
        &mut self,
        id: &MimoMixedIdent,
        model: &MatrixStack,
    ) -> Result<Vec<ToneCtl>, MimoMixedError> {
        let (n, m, p) = (self.cfg.n(), self.cfg.m(), self.cfg.p());
        let lines = self.lines;
        let (lo, hi) = self.band;
        // The identification on the floor's band, as MIMO Sine's grid.
        let grid: Vec<f64> = (lo..hi).map(|l| l as f64 * self.df).collect();
        let gl = grid.len();
        let mut resp = MatrixStack::zeros(gl, n, m);
        let mut lresp = MatrixStack::zeros(gl, p, m);
        let mut coh = vec![0.0; n * gl];
        let mut lcoh = vec![0.0; p * gl];
        for (k, l) in (lo..hi).enumerate() {
            for r in 0..n {
                for c in 0..m {
                    resp.set(k, r, c, id.h.get(l, r, c));
                }
                coh[r * gl + k] = id.coherence[l * n + r].clamp(0.0, 1.0);
            }
            for r in 0..p {
                for c in 0..m {
                    lresp.set(k, r, c, id.h.get(l, n + r, c));
                }
                lcoh[r * gl + k] = id
                    .limit_coherence
                    .get(l * p + r)
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
            }
        }
        let trusted: Vec<bool> = (0..gl)
            .map(|k| (0..n).all(|r| coh[r * gl + k] >= self.cfg.gates.coherence_threshold))
            .collect();
        let identification = LawIdentification {
            frequency_hz: grid.clone(),
            response: resp,
            limit_response: lresp,
            coherence: coh,
            limit_coherence: lcoh,
            trusted,
        };
        let band_hz = (grid[0].max(self.df), *grid.last().unwrap_or(&self.df));
        let mut out = Vec::new();
        let tone_parts: Vec<(usize, crate::config::MimoTonePart)> =
            self.cfg.tones().map(|(i, _, t)| (i, t.clone())).collect();
        for (part, t) in tone_parts {
            let mut reference_grid = vec![C64::new(0.0, 0.0); n * gl];
            let mut reference = vec![C64::new(0.0, 0.0); n * lines];
            let mut ff = vec![C64::new(0.0, 0.0); lines * m];
            for l in 0..lines {
                let f = l as f64 * self.df;
                let r = t
                    .reference_at(n, f)
                    .unwrap_or_else(|| vec![C64::new(0.0, 0.0); n]);
                for i in 0..n {
                    reference[i * lines + l] = r[i];
                    if (lo..hi).contains(&l) {
                        reference_grid[i * gl + (l - lo)] = r[i];
                    }
                }
                if (lo..hi).contains(&l) {
                    if let Some(d) = crate::budget::model_drive(model, self.df, f, &r) {
                        ff[l * m..(l + 1) * m].copy_from_slice(&d);
                    }
                }
            }
            let db = |v: &crate::config::PerControl<(f64, f64)>, i: usize| {
                v.get(i).copied().unwrap_or((1.0, 1.0))
            };
            let alarm: Vec<(f64, f64)> = (0..n).map(|i| db(&t.alarm_db, i)).collect();
            let abort: Vec<(f64, f64)> = (0..n).map(|i| db(&t.abort_db, i)).collect();
            let mut tol = LawTolerance {
                alarm_low_db: vec![0.0; n * gl],
                alarm_high_db: vec![0.0; n * gl],
                abort_low_db: vec![0.0; n * gl],
                abort_high_db: vec![0.0; n * gl],
            };
            for i in 0..n {
                for k in 0..gl {
                    tol.alarm_low_db[i * gl + k] = alarm[i].0;
                    tol.alarm_high_db[i * gl + k] = alarm[i].1;
                    tol.abort_low_db[i * gl + k] = abort[i].0;
                    tol.abort_high_db[i * gl + k] = abort[i].1;
                }
            }
            let setup = LawSetup {
                controls: n,
                drives: m,
                limits: p,
                fs_hz: self.fs,
                tick_s: (self.cfg.control_tick_s() * self.fs).round().max(1.0) / self.fs,
                band_lo_hz: band_hz.0,
                band_hi_hz: band_hz.1,
                test_type: tone_test_type(&t.schedule),
                shape: MimoShape::of(n, m),
                drive_ceiling_v: self.cfg.drives.iter().map(|d| d.peak_v).collect(),
                identification: identification.clone(),
                identification_level_db: Some(id.level_db),
                reference: reference_grid,
                phase_specified: t.phases_stated,
                limit_bounds: vec![None; p * gl],
                tolerance: tol,
                // The bank's trackers: a fixed bandwidth, so the law knows the
                // reading's delay.
                tracking: Tracking::Fixed {
                    hz: self.cfg.tracking_hz,
                },
                tracker_delay_factor: hertzforge_dsp::tone::TRACKER_DELAY_FACTOR,
                transport_delay_s: id.lag_frames as f64 / self.fs,
            };
            let pid = self.cfg.parts[part].id.clone();
            setup
                .validate()
                .map_err(|e| MimoMixedError::Law(format!("tone {pid}: {}", e.message)))?;
            let law = self
                .laws
                .tones
                .build(&setup)
                .map_err(|e| MimoMixedError::Law(format!("tone {pid}: {}", e.reason())))?;
            let desc = self.laws.tones.describe();
            // D17: how each control's reading is judged, at full level.
            let f0 = {
                let proj = self.cfg.projection(0);
                let mut clock = PartClock::new(&proj)?;
                clock.tick(0.0)[part].freq_hz.unwrap_or(0.0)
            };
            let l0 = ((f0 / self.df).round() as usize).min(lines - 1);
            let judging = (0..n)
                .map(|i| {
                    tone_judging(
                        reference[i * lines + l0].norm(),
                        self.cfg.reference.sdm.get(l0, i, i).re.max(0.0),
                        alarm[i],
                        abort[i],
                        self.cfg.tracking_hz,
                    )
                })
                .collect();
            out.push(ToneCtl {
                part,
                reference,
                ff,
                law,
                values: hertzforge_mimo_sine::law::LawValues::new(desc.diagnostics.len()),
                out: hertzforge_mimo_sine::tick::TickOutputs::new(m, n),
                engaged: false,
                calls: 0,
                misses: 0,
                watch: hertzforge_mimo_sine::guards::DivergenceWatch::new(self.guards.divergence_s),
                limiting: false,
                limited_s: 0.0,
                judging,
                tol_db: alarm.iter().copied().zip(abort.iter().copied()).collect(),
                judged_line: l0,
                ms: vec![0.0; n],
                err_db: vec![f64::NAN; n],
                alarm: vec![false; n],
                abort_for_s: vec![0.0; n],
                avg_s: vec![0.0; n],
                ask: vec![C64::new(0.0, 0.0); m],
                target: 0.0,
                f_prev: f0,
                usable: false,
                scale: 1.0,
            });
        }
        Ok(out)
    }

    /// Resolve every band: folded into the floor's reference, or servoed.
    fn build_bands(&mut self) -> Result<(), MimoMixedError> {
        let n = self.cfg.n();
        let m = self.cfg.m();
        let df = self.df;
        let mut out = Vec::new();
        let list: Vec<(
            usize,
            crate::config::MimoPart,
            crate::config::MimoNarrowbandPart,
        )> = self
            .cfg
            .bands()
            .map(|(i, p, b)| (i, p.clone(), b.clone()))
            .collect();
        let proj = self.cfg.projection(0);
        let mut clock = PartClock::new(&proj)?;
        let now = clock.tick(0.0).to_vec();
        for (i, part, b) in list {
            let res = crate::band::resolve_band_law(&self.cfg.law.bands, &part, &b)?;
            let center = now[i].freq_hz.unwrap_or(0.0);
            let alarm: Vec<(f64, f64)> = (0..n)
                .map(|c| b.alarm_db.get(c).copied().unwrap_or((3.0, 3.0)))
                .collect();
            let abort: Vec<(f64, f64)> = (0..n)
                .map(|c| b.abort_db.get(c).copied().unwrap_or((6.0, 6.0)))
                .collect();
            let (law, setup, mfe) = match res {
                BandResolution::Fold => (None, None, None),
                BandResolution::Servo(mut law) => {
                    let (lo, hi) = b.width.edges_at(center);
                    let half = (((hi - lo) / 2.0 / df).ceil() as usize) + 2;
                    let offsets = 2 * half + 1;
                    let w_lo = (half as f64 - (center - lo) / df).ceil().max(0.0) as usize;
                    let w_hi = (half as f64 + (hi - center) / df).floor() as usize + 1;
                    let setup = BandSetup {
                        n,
                        m,
                        statement: b.statement.clone(),
                        additive: true,
                        offsets,
                        df_hz: df,
                        inner: (w_lo.min(offsets), w_hi.min(offsets)),
                        alarm_db: alarm.clone(),
                        abort_db: abort.clone(),
                    };
                    law.setup(&setup)
                        .map_err(|e| MimoMixedError::Law(format!("band {}: {}", part.id, e.0)))?;
                    let depth = self.cfg.estimation.frames_per_loop.max(1)
                        * self.cfg.estimation.memory_loops.max(1.0) as usize;
                    (
                        Some(law),
                        Some(setup),
                        Some(MovingFrameEstimate::new(n, half, depth)),
                    )
                }
            };
            out.push(BandCtl {
                part: i,
                id: part.id.clone(),
                width: b.width,
                statement: b.statement.clone(),
                law,
                setup,
                mfe,
                drive: None,
                ramp: 0.0,
                center_hz: center,
                error_db: vec![f64::NAN; n],
                limiting: false,
                cap_bound: false,
                ramp_held: false,
                alarm_db: alarm,
                abort_db: abort,
                rate_hz_s: 0.0,
                ahead_s: 0.0,
                drawn_hz: center,
                over_loops: vec![0; n],
            });
        }
        let _ = m;
        self.bands = out;
        Ok(())
    }

    /// The floor's alarm and abort lines about the floor law's reference
    /// (the floor plus every folded band): a folded band's lines are judged
    /// against floor plus band, not the floor alone.
    pub(crate) fn floor_tolerances(&self) -> hertzforge_sdm::law::Tolerances {
        match &self.floor_ref {
            Some(r) => self.cfg.floor_tolerances_about(r),
            None => self.cfg.floor_tolerances_about(&self.loop_reference()),
        }
    }

    /// The floor law's reference: the floor plus every folded band.
    pub(crate) fn loop_reference(&self) -> HermitianStack {
        let mut g = self.cfg.reference.sdm.clone();
        for b in self.bands.iter().filter(|b| b.law.is_none()) {
            crate::band::fold_into_reference(&mut g, &b.statement, &b.width, b.center_hz, self.df);
        }
        g
    }

    /// Build the floor law and seed it from the identification, as MIMO
    /// Random seeds its law; the seed's drive is the drive in force.
    fn engage_floor(
        &mut self,
        id: &MimoMixedIdent,
        model: &MatrixStack,
        floor_ref: &HermitianStack,
    ) -> Result<(), MimoMixedError> {
        let (n, m) = (self.cfg.n(), self.cfg.m());
        let setup = hertzforge_sdm::law::MimoSetup {
            controls: n,
            drives: m,
            lines: self.lines,
            df_hz: self.df,
            band: self.band,
            reference: floor_ref.clone(),
            tolerances: self.cfg.floor_tolerances_about(floor_ref),
        };
        let budget_s = self.floor_budget_s();
        let law = self
            .laws
            .floor
            .build(
                &setup,
                std::time::Duration::from_secs_f64(budget_s.max(1e-3)),
            )
            .map_err(|e| MimoMixedError::Law(format!("floor law: {e}")))?;
        let shape = MimoShape::of(n, m);
        if !law.shapes().contains(&shape) {
            return Err(MimoMixedError::Law(format!(
                "the floor law does not support {n} controls and {m} drives ({shape:?})"
            )));
        }
        self.floor_needs = law.needs().to_vec();
        self.floor_law = Some(law);
        let p = 10f64.powf(-id.level_db / 10.0);
        let mut resp = id.g_cc.clone();
        let _ = resp.scale_lines(&vec![p; self.lines]);
        let mut gdd = id.g_dd.clone();
        let _ = gdd.scale_lines(&vec![p; self.lines]);
        self.drive_full = Some(gdd);
        let dof = 2.0 * id.frames as f64;
        match self.call_floor(0, &resp, dof, model, id) {
            Ok(out) => self.accept_floor(out).map_err(|m| {
                MimoMixedError::Law(format!("the floor law's seed was not usable: {m}"))
            }),
            Err(miss) => Err(MimoMixedError::Law(format!(
                "the floor law's seed was not usable: {}",
                miss.words()
            ))),
        }
    }

    /// The floor law's time budget per call, seconds (MIMO Random's: a share
    /// of the loop's time).
    pub(crate) fn floor_budget_s(&self) -> f64 {
        let hop = self.nfft as f64 / 2.0;
        let loop_s = self.cfg.estimation.frames_per_loop.max(1) as f64 * hop / self.fs;
        hertzforge_random::service::config::LawGuards::default().budget_fraction * loop_s
    }
}

/// Everything the cost measure needs to build a step with no station.
pub struct StepCostProbe {
    /// The step.
    pub step: MimoMixedStep,
    /// Input channels per frame.
    pub n_in: usize,
}

impl StepCostProbe {
    /// A step of `tones` tones on `n` controls, `p` limits and `m` drives at
    /// `fs`, every tone in, settled and running its `law` (the step's cost is
    /// the measure, not the control).
    pub fn new(
        fs: f64,
        n: usize,
        m: usize,
        p: usize,
        tones: usize,
        law: Arc<dyn MimoSineLawFactory>,
    ) -> Result<Self, String> {
        let q = n + p;
        let lines = 1025usize;
        let df = fs / 2048.0;
        let grid: Vec<f64> = (1..lines - 1).map(|l| l as f64 * df).collect();
        let gl = grid.len();
        let mut resp = MatrixStack::zeros(gl, n, m);
        for k in 0..gl {
            for r in 0..n {
                for c in 0..m {
                    resp.set(k, r, c, C64::new(if r % m == c { 1.0 } else { 0.1 }, 0.0));
                }
            }
        }
        let setup = LawSetup {
            controls: n,
            drives: m,
            limits: p,
            fs_hz: fs,
            tick_s: 0.01,
            band_lo_hz: grid[0],
            band_hi_hz: grid[gl - 1],
            test_type: TestType::Dwell,
            shape: MimoShape::of(n, m),
            drive_ceiling_v: vec![10.0; m],
            identification: LawIdentification {
                frequency_hz: grid.clone(),
                response: resp,
                limit_response: MatrixStack::zeros(gl, p, m),
                coherence: vec![1.0; n * gl],
                limit_coherence: vec![1.0; p * gl],
                trusted: vec![true; gl],
            },
            identification_level_db: Some(-20.0),
            reference: vec![C64::new(0.1, 0.0); n * gl],
            phase_specified: false,
            limit_bounds: vec![None; p * gl],
            tolerance: LawTolerance {
                alarm_low_db: vec![3.0; n * gl],
                alarm_high_db: vec![3.0; n * gl],
                abort_low_db: vec![60.0; n * gl],
                abort_high_db: vec![60.0; n * gl],
            },
            tracking: Tracking::Fixed { hz: 0.5 },
            tracker_delay_factor: hertzforge_dsp::tone::TRACKER_DELAY_FACTOR,
            transport_delay_s: 0.0,
        };
        setup.validate().map_err(|e| e.message)?;
        let desc = law.describe();
        let mut ctls = Vec::new();
        let mut f0 = Vec::new();
        for k in 0..tones {
            let f = 100.0 + 97.0 * k as f64;
            f0.push(f);
            ctls.push(ToneCtl {
                part: 0,
                reference: vec![C64::new(0.1, 0.0); n * lines],
                ff: vec![C64::new(0.05, 0.0); lines * m],
                law: law.build(&setup).map_err(|e| e.reason())?,
                values: hertzforge_mimo_sine::law::LawValues::new(desc.diagnostics.len()),
                out: hertzforge_mimo_sine::tick::TickOutputs::new(m, n),
                engaged: false,
                calls: 0,
                misses: 0,
                watch: hertzforge_mimo_sine::guards::DivergenceWatch::new(1e9),
                limiting: false,
                limited_s: 0.0,
                judging: vec![tone_judging(0.1, 1e-6, (3.0, 3.0), (60.0, 60.0), 0.5); n],
                tol_db: vec![((3.0, 3.0), (60.0, 60.0)); n],
                judged_line: ((f / df).round() as usize).min(lines - 1),
                ms: vec![0.0; n],
                err_db: vec![0.0; n],
                alarm: vec![false; n],
                abort_for_s: vec![0.0; n],
                avg_s: vec![0.0; n],
                ask: vec![C64::new(0.0, 0.0); m],
                target: 0.0,
                f_prev: f,
                usable: false,
                scale: 1.0,
            });
        }
        let bank =
            ToneBank::new(fs, &f0, &vec![ToneLock::Free; tones], m, q, 0.5, 16).map_err(|e| e.0)?;
        let cfg = probe_config(n, tones, fs)?;
        let clock = PartClock::new(&cfg.projection(0)).map_err(|e| e.to_string())?;
        let shape = StepShape {
            fs,
            n,
            m,
            p,
            n_in: q,
            slots: (0..q).collect(),
            n_out: m,
            out_slots: (0..m).collect(),
            peak_v: vec![1e9; m],
            abort_v: vec![1e9; m],
            tick_s: 0.01,
            ring_frames: 1 << 16,
            record_frames: 1 << 16,
            max_block: 4096,
            floor_diag: vec![1e-9; lines * n],
            df,
            lines,
            part_ramp_s: 0.0,
        };
        let mut step = MimoMixedStep::new(
            shape,
            bank,
            ctls,
            clock,
            Box::new(crate::budget::CommonScale),
            GuardSettings {
                law_misses: u32::MAX,
                ..GuardSettings::default()
            },
            Arc::new(hertzforge_sine::law::MonotonicClock::new()),
        );
        step.law_budget_s = f64::INFINITY;
        step.level = Ramp::at(1.0);
        step.rand_ramp = Ramp::at(1.0);
        step.parts_live = true;
        step.tones_run = true;
        step.part_ramp_frames = 1.0;
        Ok(StepCostProbe { step, n_in: q })
    }

    /// Feed the step's ring and its records' room (between blocks).
    pub fn feed(&mut self, frames: usize) {
        let m = self.step.m;
        let v: Vec<f64> = (0..frames * m)
            .map(|i| 0.01 * ((i as f64) * 0.618).sin())
            .collect();
        self.step.ring.push(&v);
        self.step.records.clear();
        self.step.records.law.clear();
    }
}

/// A configuration for the cost probe: `tones` fixed tones on a flat floor.
fn probe_config(n: usize, tones: usize, fs: f64) -> Result<MimoMixedConfig, String> {
    use crate::config::*;
    let lines = 1025usize;
    let df = fs / 2048.0;
    let mut sdm = HermitianStack::zeros(lines, n);
    for l in 2..lines - 2 {
        for i in 0..n {
            sdm.set(l, i, i, C64::new(1e-4, 0.0));
        }
    }
    let mut parts = vec![MimoPart {
        id: "floor".into(),
        name: String::new(),
        enabled: true,
        window: Default::default(),
        body: MimoPartBody::Floor(MimoFloorPart::default()),
    }];
    for k in 0..tones {
        parts.push(MimoPart {
            id: format!("t{k}"),
            name: String::new(),
            enabled: true,
            window: Default::default(),
            body: MimoPartBody::Tone(MimoTonePart {
                schedule: PartSchedule::Fixed {
                    hz: 100.0 + 97.0 * k as f64,
                    for_s: None,
                    for_cycles: None,
                },
                amplitude: PerControl::All(hertzforge_mixed::ToneAmplitude::Constant { g_pk: 0.1 }),
                phases_stated: false,
                phase_deg: Vec::new(),
                alarm_db: PerControl::All(DEFAULT_TONE_ALARM_DB),
                abort_db: PerControl::All(DEFAULT_TONE_ABORT_DB),
            }),
        });
    }
    let reference = hertzforge_reference::Reference {
        sdm,
        df_hz: df,
        band: (2, lines - 2),
        labels: (0..n).map(|i| format!("c{i}")).collect(),
        unit: "g".into(),
        tolerances: hertzforge_sdm::law::Tolerances::none(lines, n),
        response_transformation: None,
        drive_transformation: None,
        provenance: hertzforge_reference::Provenance::new(
            hertzforge_reference::Method::Breakpoints,
            "cost probe",
        ),
    };
    let mut cfg: MimoMixedConfig = serde_json::from_value(serde_json::json!({}))
        .unwrap_or_else(|_| fallback_config(reference.clone()));
    cfg.reference = reference;
    cfg.parts = parts;
    cfg.controls = (0..n as u32).collect();
    Ok(cfg)
}

fn fallback_config(reference: hertzforge_reference::Reference) -> MimoMixedConfig {
    use crate::config::*;
    MimoMixedConfig {
        estimation: Default::default(),
        controls: Vec::new(),
        drives: vec![MimoDrive::new(100, 2.0)],
        limits: Vec::new(),
        reference,
        parts: Vec::new(),
        exclusion_hz: DEFAULT_EXCLUSION_HZ,
        tracking_hz: DEFAULT_TRACKING_HZ,
        schedule: hertzforge_random::service::config::LevelSchedule::auto_default(),
        full_level_duration_s: None,
        level_ramp_s: 0.0,
        shutdown_ramp_s: 0.0,
        part_ramp_s: 0.0,
        floor_settle_s: 30.0,
        soft_start_db: 0.0,
        soft_start_relax_db: 0.0,
        identification: None,
        gates: Default::default(),
        caps: Default::default(),
        protections: Default::default(),
        frf_mode: hertzforge_mimo_random::config::FrfMode::Hold,
        crest_factor: 3.0,
        drive_clip_sigma: None,
        article_mass_lb: 0.0,
        capture: Default::default(),
        law: MimoMixedLawChoice::default(),
        control_tick_s: None,
        metadata: Vec::new(),
        seed: 1,
    }
}
