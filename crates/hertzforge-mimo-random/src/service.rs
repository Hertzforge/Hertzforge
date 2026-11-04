//! [`MimoRandomService`]: the multiple-output Random run.
//!
//! One run, in order (design 2): feasibility, the noise floor (drives off),
//! system identification (uncorrelated random on every drive, H1, multiple
//! coherence, conditioning), the gates, the prediction (the law's seed against
//! the identification set), then the closed loop under the level schedule, and
//! a ramped stop or an immediate abort. Every transition is an event.
//!
//! Each loop (design 7.1): play the drive in force, estimate the response
//! spectral matrix and the response-to-drive cross spectrum, transform to the
//! control space and refer to full level, judge the envelope and the core's
//! divergence, call the law under its guards, check its drive, expand it through
//! the drive transformation, cap each drive's change, hold each drive's ceiling,
//! limit the limit channels, and keep the result as the drive in force. The law
//! works at full level; the core owns the level (D9).
//!
//! Reused from the single-output module unchanged: the plant loop and its arm
//! gate (`PlantLoop::open_outputs`), the level ramp (`DriveRamp`), the level
//! schedule (`LevelSchedule`), the kinematic monitor, the law clock, and the
//! generalized drive synthesizer.

use crate::config::{ConfigError, ControlSpace, FrfMode, MimoConfig};
use crate::estimate::{Estimate, SdmEstimator};
use crate::guards::{self, DriveFactors};
use crate::law::{guarded_call, Miss};
use crate::pool;
use crate::report::*;
use hertzforge_channel_table::{ChannelRole, ChannelTable};
use hertzforge_hal::Backend;
use hertzforge_module_abi::StationClass;
use hertzforge_random::service::config::RunMode;
use hertzforge_random::service::engine::{
    DriveFactor, DriveSynth, KinematicMonitor, LoopConfig, PlantLoop, SegmentResult,
};
use hertzforge_random::service::law::{LawClock, MonotonicClock};
use hertzforge_random::service::ramp::DriveRamp;
use hertzforge_random_laws::MimoLawFactory;
use hertzforge_sdm::law::{
    DiagnosticValue, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup,
    MimoShape,
};
use hertzforge_sdm::linalg::Regularization;
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// The depth of a controlled stop's ramp-down, dB (the single-output value).
pub const STOP_RAMP_DEPTH_DB: f64 = 40.0;

/// No envelope judgement on an estimate with fewer degrees of freedom.
pub const MIN_ENVELOPE_DOF: f64 = 16.0;

/// The most frames the lag search looks back.
const MAX_LAG_FRAMES: usize = 4096;

/// Why a service call could not do what was asked.
#[derive(Debug, Clone, PartialEq)]
pub enum MimoError {
    /// The configuration is not usable.
    Config(ConfigError),
    /// The call does not apply in the run's state.
    BadState(String),
    /// The plant loop could not open or faulted.
    Plant(String),
    /// The law could not be built or seeded.
    Law(String),
    /// The identification could not reach its level within the drive limits.
    IdentUnreachable(String),
    /// A gate, a feasibility verdict or a prediction holds the run until an
    /// override is acknowledged.
    Blocked(String),
    /// Records could not be written or read.
    Io(String),
}

impl fmt::Display for MimoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MimoError::Config(e) => write!(f, "configuration: {e}"),
            MimoError::BadState(s) => write!(f, "{s}"),
            MimoError::Plant(s) => write!(f, "plant loop: {s}"),
            MimoError::Law(s) => write!(f, "control law: {s}"),
            MimoError::IdentUnreachable(s) => write!(f, "identification: {s}"),
            MimoError::Blocked(s) => write!(f, "{s}"),
            MimoError::Io(s) => write!(f, "records: {s}"),
        }
    }
}

impl std::error::Error for MimoError {}

impl From<ConfigError> for MimoError {
    fn from(e: ConfigError) -> Self {
        MimoError::Config(e)
    }
}

fn la<T>(r: Result<T, hertzforge_sdm::linalg::LinalgError>) -> Result<T, MimoError> {
    r.map_err(|e| MimoError::Law(e.to_string()))
}

/// Everything system identification measured, kept for the run and loadable into
/// a later run on the same setup.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentificationSet {
    /// The setup hash: the channel roles, sensitivities, transformations and grid.
    pub setup_hash: String,
    /// Physical FRF, responses (controls then limit channels) by drives.
    pub frf: MatrixStack,
    /// Control response SDM during identification (physical controls).
    pub g_cc: HermitianStack,
    /// Response-to-drive cross spectrum during identification.
    pub g_rd: MatrixStack,
    /// Drive SDM during identification.
    pub g_dd: HermitianStack,
    /// Control response SDM with the drives off (physical controls).
    pub noise: HermitianStack,
    /// Multiple coherence of each control-space DOF, line-major.
    pub coherence: Vec<f64>,
    /// Condition number of the control-space FRF per line (NaN out of band).
    pub condition: Vec<f64>,
    /// The rms each drive played, volts.
    pub drive_vrms: Vec<f64>,
    /// The controls' level relative to the reference, dB.
    pub level_db: f64,
    pub frames: usize,
    pub steps: usize,
    /// Response-to-drive lag the estimates were aligned by, frames.
    pub lag_frames: usize,
}

/// The run's drive and estimate state between loops.
struct LoopState {
    /// The accumulated estimate, at the level the run is at.
    est: Option<Estimate>,
    est_loops: u32,
    /// The drive in force, physical, at full level.
    drive_full: HermitianStack,
    /// The drive the law last proposed, in its own space, at full level.
    law_drive: HermitianStack,
    /// The control-space response at full level from the last judged loop.
    response_full: Option<HermitianStack>,
    /// Per control and line, the dB error at full level, line-major.
    error_lines: Vec<f64>,
    /// Per control, the in-band rms error, dB.
    error_db: Vec<f64>,
    /// Per control, the worst in-band line error, dB.
    worst_db: Vec<f64>,
    /// Divergence bookkeeping per control: loops without improvement and the best
    /// error seen.
    div_streak: Vec<u32>,
    div_best: Vec<f64>,
    /// Per drive and line (line-major): which of the last loops the change cap
    /// held it back in, one bit per loop, newest lowest.
    cap_hist: Vec<u32>,
    settle: u32,
    caps: Option<DriveFactors>,
    diagnostics: Vec<DiagnosticValue>,
    drive_rms: Vec<f64>,
}

/// The multiple-output Random run service.
pub struct MimoRandomService {
    cfg: MimoConfig,
    loop_cfg: LoopConfig,
    backend: Option<Box<dyn Backend>>,
    station: StationClass,
    table: ChannelTable,
    state: MimoState,
    log: MimoEventLog,
    data_root: PathBuf,

    lines: usize,
    df_hz: f64,
    band: (usize, usize),
    n_c: usize,
    n_d: usize,
    n_l: usize,
    n_r: usize,
    n_u: usize,
    /// Interleave slot of each control, then each limit channel.
    response_slots: Vec<usize>,
    /// The reference in the control space, full level.
    reference: HermitianStack,
    /// The same reference and its tolerances, shared with display snapshots.
    reference_shared: Arc<HermitianStack>,
    tolerances_shared: Arc<hertzforge_sdm::law::Tolerances>,
    /// The reference's diagonal per line (line-major, `lines * n_r`).
    ref_diag: Vec<f64>,
    /// What the plant can reach of the reference (`P G_ref P^H`), for the core's
    /// divergence judgement; the reference itself on a square plant.
    achievable_diag: Vec<f64>,
    t_resp: Option<MatrixStack>,
    t_drive: Option<MatrixStack>,
    t_drive_pinv: Option<MatrixStack>,

    law_factory: Option<Arc<dyn MimoLawFactory>>,
    law_clock: Arc<dyn LawClock>,
    law: Option<Box<dyn MimoControlLaw + Send>>,
    law_needs: Vec<MimoInput>,
    law_line: Option<LawLine>,
    law_stats: LawStats,
    law_misses: u32,
    law_stop_due: Option<MimoFaultReason>,
    law_retired: bool,
    last_miss: Option<Miss>,

    ident: Option<IdentificationSet>,
    ident_loaded: bool,
    noise: Option<HermitianStack>,
    /// Physical FRF of the controls and of the limit channels.
    frf_c: Option<MatrixStack>,
    frf_l: Option<MatrixStack>,
    /// The FRF the law sees (control space by law drives), held or updated.
    frf_law: Option<MatrixStack>,
    frf_law_ident: Option<MatrixStack>,
    coherence: Vec<f64>,
    condition: Vec<f64>,
    gate: Option<GateReport>,
    prediction: Option<Prediction>,
    feasibility: Vec<DriveFeasibilityRow>,
    acknowledged: Vec<String>,

    ls: LoopState,
    schedule_level_db: f64,
    level_target_db: f64,
    level_index: usize,
    ramp: Arc<DriveRamp>,
    elapsed_at_level_s: f64,
    loop_count: u64,
    full_level_loops: u64,

    plant_loop: Option<PlantLoop>,
    synth: Option<DriveSynth>,
    estimator: Option<SdmEstimator>,
    /// The drive stream's last frames per output, for aligning the drive with the
    /// response.
    drive_tail: Vec<Vec<f64>>,
    lag_frames: usize,
    /// Measure the lag on the next segment played.
    need_lag: bool,
    kinematic: Option<KinematicMonitor>,
    overloads: Vec<(u32, f64, bool)>,
    trip: Option<MimoTripRecord>,
    timing: LoopTiming,
    guards_last: GuardSummary,
    faults: Vec<String>,
    pub(crate) records: Option<crate::records::RunRecorder>,
}

/// The `n` by `n` identity on one line.
fn identity1(n: usize) -> MatrixStack {
    MatrixStack::identity(1, n)
}

impl MimoRandomService {
    /// A run of `cfg` on `table`, through `backend` (the station factory builds
    /// it), with the loop settings `loop_cfg` (its line grid must carry the
    /// reference). Records go under `data_root`. The configuration and the table
    /// are checked here; nothing is opened until [`Self::start`].
    pub fn new(
        cfg: MimoConfig,
        table: ChannelTable,
        mut loop_cfg: LoopConfig,
        backend: Box<dyn Backend>,
        station: StationClass,
        data_root: impl Into<PathBuf>,
    ) -> Result<Self, MimoError> {
        // The loop measures `frames_per_loop` Welch frames at 50% overlap.
        let k = cfg.estimation.frames_per_loop;
        loop_cfg.overlap = 0.5;
        loop_cfg.blocks_per_window = k.div_ceil(2) + 1;
        let lines = loop_cfg.nbins();
        let df = loop_cfg.df();
        cfg.validate(lines, df)?;
        let (n_c, n_d, n_l) = (cfg.n_controls(), cfg.n_drives(), cfg.limits.len());
        let (n_r, n_u) = (cfg.n_reference(), cfg.n_law_drives());
        // The table: every control and limit an enabled input, the drives the
        // enabled Drive rows in the order the config lists them.
        let mut slot = 0usize;
        let mut slot_of = std::collections::BTreeMap::new();
        let mut drive_rows = Vec::new();
        for ch in &table.channels {
            if !ch.enabled {
                continue;
            }
            if ch.role == ChannelRole::Drive {
                drive_rows.push(ch.index);
            } else {
                slot_of.insert(ch.index, slot);
                slot += 1;
            }
        }
        let want: Vec<u32> = cfg.drives.iter().map(|d| d.channel_index).collect();
        if drive_rows != want {
            return Err(MimoError::Config(ConfigError(format!(
                "the channel table's enabled drive rows are {drive_rows:?} and the test lists \
                 drives {want:?}; list the drives in the order of the table's drive rows"
            ))));
        }
        let mut response_slots = Vec::with_capacity(n_c + n_l);
        for idx in cfg
            .controls
            .iter()
            .copied()
            .chain(cfg.limits.iter().map(|l| l.channel_index))
        {
            match slot_of.get(&idx) {
                Some(&s) => response_slots.push(s),
                None => {
                    return Err(MimoError::Config(ConfigError(format!(
                        "channel {idx} is not an enabled input in the channel table; enable it or \
                         remove it from the test"
                    ))))
                }
            }
        }
        let band = cfg.reference.band;
        let mut reference = cfg.reference.sdm.clone();
        // The reference must be realizable: repair rounding-level negatives the
        // same way a law's drive is repaired, refuse anything worse.
        let rep = pool::psd_repair(
            &mut reference,
            hertzforge_sdm::linalg::PSD_CLAMP_REL,
            hertzforge_sdm::linalg::PSD_REJECT_REL,
            band,
        )
        .map_err(|e| {
            MimoError::Config(ConfigError(format!(
                "the reference is not positive semi-definite: {e}; check its coherence and \
                 phase entries"
            )))
        })?;
        let ref_diag = hertzforge_sdm::linalg::diagonals(&reference);
        let t_resp = match &cfg.control_space {
            ControlSpace::Physical => None,
            ControlSpace::Virtual { response } => Some(response.clone()),
        };
        let t_drive = cfg.drive_transform.clone();
        let t_drive_pinv = match &t_drive {
            Some(t) if t.lines() == 1 => {
                Some(la(hertzforge_sdm::linalg::pinv(t, Regularization::None))?.0)
            }
            Some(t) => Some(la(pool::pinv(t, Regularization::None, (0, t.lines())))?),
            None => None,
        };
        let mut log = MimoEventLog::default();
        if rep.total_clamped() > 0 {
            log.push(MimoEventKind::Note {
                text: format!(
                    "The reference was repaired to be positive semi-definite on {} lines \
                     ({} eigenvalues clamped, the autospectra kept).",
                    rep.lines_repaired(),
                    rep.total_clamped()
                ),
            });
        }
        let overloads = table
            .channels
            .iter()
            .filter(|c| c.enabled && c.role != ChannelRole::Drive)
            .map(|c| (c.index, c.range_max, false))
            .collect();
        let ramp = Arc::new(DriveRamp::new(STOP_RAMP_DEPTH_DB, cfg.shutdown_ramp_s));
        let law_factory = crate::law::builtin_factory(&cfg.law, n_r).ok();
        let ls = LoopState {
            est: None,
            est_loops: 0,
            drive_full: HermitianStack::zeros(lines, n_d),
            law_drive: HermitianStack::zeros(lines, n_u),
            response_full: None,
            error_lines: Vec::new(),
            error_db: Vec::new(),
            worst_db: Vec::new(),
            div_streak: vec![0; n_r],
            div_best: vec![f64::INFINITY; n_r],
            cap_hist: vec![0; lines * n_d],
            settle: 0,
            caps: None,
            diagnostics: Vec::new(),
            drive_rms: vec![0.0; n_d],
        };
        let tolerances_shared = Arc::new(cfg.reference.tolerances.clone());
        Ok(MimoRandomService {
            achievable_diag: ref_diag.clone(),
            cfg,
            loop_cfg,
            backend: Some(backend),
            station,
            table,
            state: MimoState::Idle,
            log,
            data_root: data_root.into(),
            lines,
            df_hz: df,
            band,
            n_c,
            n_d,
            n_l,
            n_r,
            n_u,
            response_slots,
            reference_shared: Arc::new(reference.clone()),
            tolerances_shared,
            reference,
            ref_diag,
            t_resp,
            t_drive,
            t_drive_pinv,
            law_factory,
            law_clock: Arc::new(MonotonicClock::new()),
            law: None,
            law_needs: Vec::new(),
            law_line: None,
            law_stats: LawStats::default(),
            law_misses: 0,
            law_stop_due: None,
            law_retired: false,
            last_miss: None,
            ident: None,
            ident_loaded: false,
            noise: None,
            frf_c: None,
            frf_l: None,
            frf_law: None,
            frf_law_ident: None,
            coherence: Vec::new(),
            condition: Vec::new(),
            gate: None,
            prediction: None,
            feasibility: Vec::new(),
            acknowledged: Vec::new(),
            ls,
            schedule_level_db: -f64::INFINITY,
            level_target_db: -f64::INFINITY,
            level_index: 0,
            ramp,
            elapsed_at_level_s: 0.0,
            loop_count: 0,
            full_level_loops: 0,
            plant_loop: None,
            synth: None,
            estimator: None,
            drive_tail: vec![Vec::new(); n_d],
            lag_frames: 0,
            need_lag: false,
            kinematic: None,
            overloads,
            trip: None,
            timing: LoopTiming::default(),
            guards_last: GuardSummary::default(),
            faults: Vec::new(),
            records: None,
        })
    }

    /// Run with a law from the catalog (an imported law file, or a built-in law
    /// resolved there). Without this the configured built-in law is used.
    pub fn with_law_factory(mut self, factory: Arc<dyn MimoLawFactory>) -> Self {
        self.law_factory = Some(factory);
        self
    }

    /// Time law calls on `clock` (tests make a law slow deterministically).
    pub fn with_law_clock(mut self, clock: Arc<dyn LawClock>) -> Self {
        self.law_clock = clock;
        self
    }

    // --- read-only surface --------------------------------------------------------

    pub fn state(&self) -> MimoState {
        self.state
    }
    pub fn config(&self) -> &MimoConfig {
        &self.cfg
    }
    pub fn loop_config(&self) -> &LoopConfig {
        &self.loop_cfg
    }
    pub fn log(&self) -> &MimoEventLog {
        &self.log
    }
    pub fn trip(&self) -> Option<&MimoTripRecord> {
        self.trip.as_ref()
    }
    pub fn gate(&self) -> Option<&GateReport> {
        self.gate.as_ref()
    }
    pub fn prediction(&self) -> Option<&Prediction> {
        self.prediction.as_ref()
    }
    pub fn identification(&self) -> Option<&IdentificationSet> {
        self.ident.as_ref()
    }
    pub fn level_db(&self) -> f64 {
        self.schedule_level_db
    }
    pub fn loop_count(&self) -> u64 {
        self.loop_count
    }
    pub fn loops_at_full_level(&self) -> u64 {
        self.full_level_loops
    }
    pub fn lines(&self) -> usize {
        self.lines
    }
    pub fn band(&self) -> (usize, usize) {
        self.band
    }
    pub fn df_hz(&self) -> f64 {
        self.df_hz
    }
    /// The drive ramp, so a front end can start the controlled stop from another
    /// thread (`drive_ramp().request_stop()`), as single-output Random allows.
    pub fn drive_ramp(&self) -> Arc<DriveRamp> {
        self.ramp.clone()
    }
    /// The drive in force, physical, at full level.
    pub fn drive_full_level(&self) -> &HermitianStack {
        &self.ls.drive_full
    }
    /// Each drive's realized rms on the last loop, volts.
    pub fn drive_rms(&self) -> &[f64] {
        &self.ls.drive_rms
    }
    /// The in-band rms error per control (control space), dB, last judged loop.
    pub fn error_db(&self) -> &[f64] {
        &self.ls.error_db
    }
    /// The worst in-band line error per control, dB, last judged loop.
    pub fn worst_error_db(&self) -> &[f64] {
        &self.ls.worst_db
    }
    /// The control-space response at full level, last judged loop.
    pub fn response_full_level(&self) -> Option<&HermitianStack> {
        self.ls.response_full.as_ref()
    }
    /// The reference in the control space.
    pub fn reference(&self) -> &HermitianStack {
        &self.reference
    }
    /// What the law and guards did, last loop.
    pub fn guard_summary(&self) -> &GuardSummary {
        &self.guards_last
    }
    pub fn law_stats(&self) -> LawStats {
        self.law_stats
    }
    pub fn timing(&self) -> LoopTiming {
        self.timing
    }
    /// The drive's peak clip and rms ceiling hold the drive; this is what played.
    pub fn data_root(&self) -> &std::path::Path {
        &self.data_root
    }

    pub(crate) fn reference_shared(&self) -> Arc<HermitianStack> {
        self.reference_shared.clone()
    }

    pub(crate) fn tolerances_shared(&self) -> Arc<hertzforge_sdm::law::Tolerances> {
        self.tolerances_shared.clone()
    }

    /// Per drive, the smallest power factor the change cap and the ceiling applied
    /// on any band line in the last accepted loop (1 when nothing held it down);
    /// empty before the first.
    pub fn drive_caps(&self) -> Vec<f64> {
        let Some(c) = self.ls.caps.as_ref() else {
            return Vec::new();
        };
        (0..c.drives)
            .map(|k| {
                (self.band.0..self.band.1)
                    .filter_map(|l| c.factors.get(l * c.drives + k).copied())
                    .filter(|f| f.is_finite())
                    .fold(1.0f64, f64::min)
                    .clamp(0.0, 1.0)
            })
            .collect()
    }

    /// Seconds at the present level (the loop's acquisition time summed).
    pub fn elapsed_at_level_s(&self) -> f64 {
        self.elapsed_at_level_s
    }

    /// Seconds left at full level before the run stops by itself, at full level
    /// with a duration set.
    pub fn remaining_at_full_level_s(&self) -> Option<f64> {
        if self.state != MimoState::FullLevel {
            return None;
        }
        self.cfg
            .full_level_duration_s
            .map(|d| (d - self.elapsed_at_level_s).max(0.0))
    }

    /// A stop has been asked for and the drive is ramping down to it.
    pub fn stopping(&self) -> bool {
        self.ramp.stop_requested().is_some()
            && matches!(self.state, MimoState::Engaged | MimoState::FullLevel)
    }

    /// The law the run uses, by the catalog's id, with its parameter values.
    pub fn law_choice(&self) -> &hertzforge_random_laws::LawChoice {
        &self.cfg.law
    }

    /// Choose another law (or other parameters) during the prediction step: the law
    /// is built and seeded again against the identification set and the prediction
    /// starts over. `factory` builds an imported law file (resolved through the law
    /// catalog); `None` builds the built-in law `choice` names.
    pub fn change_law(
        &mut self,
        choice: hertzforge_random_laws::LawChoice,
        factory: Option<Arc<dyn MimoLawFactory>>,
    ) -> Result<(), MimoError> {
        if self.state != MimoState::Predict {
            return Err(MimoError::BadState(format!(
                "the law is changed in the prediction step; the run is {:?}",
                self.state
            )));
        }
        let factory = match factory {
            Some(f) => f,
            None => crate::law::builtin_factory(&choice, self.n_r)
                .map_err(|e| MimoError::Law(e.to_string()))?,
        };
        let before = (self.law_factory.clone(), self.cfg.law.clone());
        self.law_factory = Some(factory);
        self.cfg.law = choice;
        self.law_misses = 0;
        self.law_stop_due = None;
        if let Err(e) = self.predict() {
            // Keep the law that was there; the prediction it made stands.
            self.law_factory = before.0;
            self.cfg.law = before.1;
            let _ = self.predict();
            return Err(e);
        }
        self.note(format!(
            "Law changed to `{}` in the prediction step.",
            self.cfg.law.id
        ));
        Ok(())
    }

    fn transition(&mut self, to: MimoState) {
        if self.state != to {
            self.log.push(MimoEventKind::StateChange {
                from: self.state,
                to,
            });
            self.state = to;
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.log.push(MimoEventKind::Note { text: text.into() });
    }

    // --- control-space transforms -------------------------------------------------

    /// `T_r G T_r^H` (or `G`) on the band.
    fn to_control(&self, g_cc: &HermitianStack) -> Result<HermitianStack, MimoError> {
        match &self.t_resp {
            Some(t) => la(pool::congruence(t, g_cc, self.band)),
            None => Ok(g_cc.clone()),
        }
    }

    /// `T_r G_cd` (or `G_cd`), control space by physical drives.
    fn csd_to_control(&self, g_cd: &MatrixStack) -> Result<MatrixStack, MimoError> {
        match &self.t_resp {
            Some(t) => la(pool::frf_transform(t, g_cd, None, self.band)),
            None => Ok(g_cd.clone()),
        }
    }

    /// The FRF the law sees: `T_r H T_d`.
    fn frf_to_law(&self, h_c: &MatrixStack) -> Result<MatrixStack, MimoError> {
        let tr = self.t_resp.clone().unwrap_or_else(|| identity1(self.n_c));
        la(pool::frf_transform(
            &tr,
            h_c,
            self.t_drive.as_ref(),
            self.band,
        ))
    }

    /// The physical drive SDM from the law's: `T_d G_uu T_d^H` (or `G_uu`).
    fn expand_drive(&self, g_uu: &HermitianStack) -> Result<HermitianStack, MimoError> {
        match &self.t_drive {
            Some(t) => la(pool::congruence(t, g_uu, self.band)),
            None => Ok(g_uu.clone()),
        }
    }

    /// The law's drive space from the physical: `T_d^+ G_dd T_d^+^H` (or `G_dd`).
    fn reduce_drive(&self, g_dd: &HermitianStack) -> Result<HermitianStack, MimoError> {
        match &self.t_drive_pinv {
            Some(t) => la(pool::congruence(t, g_dd, self.band)),
            None => Ok(g_dd.clone()),
        }
    }

    /// The control-space cross spectrum against the law's drives:
    /// `T_r G_cd T_d^+^H`.
    fn csd_to_law(&self, g_cd: &MatrixStack) -> Result<MatrixStack, MimoError> {
        let v = self.csd_to_control(g_cd)?;
        match &self.t_drive_pinv {
            Some(tp) => {
                // (T_d^+ G_dc)^H = G_cd T_d^+^H.
                let tph = tp.conj_transposed();
                let id = identity1(v.rows());
                la(pool::frf_transform(&id, &v, Some(&tph), self.band))
            }
            None => Ok(v),
        }
    }

    // --- feasibility, arm -----------------------------------------------------------

    /// Feasibility, then arm the plant loop: `Idle -> NoiseFloor`, or `Idle ->
    /// Feasibility` (held) when a drive's shaker would be exceeded; then
    /// [`Self::override_with`] releases it.
    pub fn start(&mut self) -> Result<(), MimoError> {
        if self.state != MimoState::Idle {
            return Err(MimoError::BadState(format!(
                "start applies to a run that has not started; this one is {:?}",
                self.state
            )));
        }
        self.assess_feasibility();
        if self.feasibility.iter().any(|r| r.verdict == "Exceeded") {
            self.transition(MimoState::Feasibility);
            return Err(MimoError::Blocked(
                "a drive's shaker would be exceeded at the reference; change the test or \
                 acknowledge an override to go on"
                    .into(),
            ));
        }
        self.arm()
    }

    fn assess_feasibility(&mut self) {
        use hertzforge_breakpoints::LineSpectrum;
        use hertzforge_shaker_db::{FeasibilityCalculator, Verdict};
        // Before identification the acceleration at each shaker is taken as the
        // controls' mean reference PSD (the rigid-body estimate).
        let (lo, hi) = self.band;
        let mut psd = vec![0.0; self.lines];
        for (line, p) in psd.iter_mut().enumerate().take(hi).skip(lo) {
            let d = &self.ref_diag[line * self.n_r..(line + 1) * self.n_r];
            *p = d.iter().sum::<f64>() / self.n_r as f64;
        }
        let profile = LineSpectrum {
            f0_hz: 0.0,
            df_hz: self.df_hz,
            psd,
        };
        let shakers: Vec<(usize, &crate::config::DriveSpec)> = self
            .cfg
            .drives
            .iter()
            .enumerate()
            .filter(|(_, d)| d.shaker.is_some())
            .collect();
        let pairs: Vec<_> = shakers
            .iter()
            .map(|(_, d)| (d.shaker.clone().unwrap_or_default(), d.moving_mass))
            .collect();
        let shared = FeasibilityCalculator::assess_shared(
            &pairs,
            self.cfg.article_mass_lb,
            &vec![profile; pairs.len()],
            self.cfg.crest_factor,
        );
        let mut rows = Vec::new();
        for (k, d) in self.cfg.drives.iter().enumerate() {
            let rep = shakers
                .iter()
                .position(|(i, _)| *i == k)
                .map(|p| &shared.drives[p]);
            rows.push(DriveFeasibilityRow {
                drive: k,
                channel_index: d.channel_index,
                verdict: rep
                    .map(|r| r.overall.to_string())
                    .unwrap_or_else(|| Verdict::NotEvaluated.to_string()),
                detail: rep
                    .map(|r| {
                        r.items
                            .iter()
                            .filter(|i| i.verdict != Verdict::Pass)
                            .map(|i| format!("{}: {}", i.label, i.verdict))
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_else(|| "no shaker record".into()),
            });
        }
        self.feasibility = rows;
    }

    /// Acknowledge what holds the run (a feasibility verdict, a failed gate, a
    /// prediction over a ceiling) with the operator's words, and go on: from
    /// `Feasibility` the run arms; at `Gate` and `Predict` the next `proceed`
    /// is allowed.
    pub fn override_with(&mut self, ack: &str) -> Result<(), MimoError> {
        if ack.trim().is_empty() {
            return Err(MimoError::Blocked(
                "an override needs the operator's acknowledgement in words".into(),
            ));
        }
        let what = match self.state {
            MimoState::Feasibility => "feasibility",
            MimoState::Gate => "gate",
            MimoState::Predict => "prediction",
            s => {
                return Err(MimoError::BadState(format!(
                    "there is nothing to override in {s:?}"
                )))
            }
        };
        self.log.push(MimoEventKind::Override {
            what: what.into(),
            ack: ack.into(),
        });
        self.acknowledged.push(what.into());
        if self.state == MimoState::Feasibility {
            self.arm()?;
        }
        Ok(())
    }

    fn arm(&mut self) -> Result<(), MimoError> {
        let backend = self.backend.take().ok_or_else(|| {
            MimoError::Plant("the station backend was already used; rebuild the service".into())
        })?;
        let before = (self.loop_cfg.fs_hz, self.loop_cfg.df());
        let plant = PlantLoop::open_outputs(
            backend,
            self.station,
            &mut self.loop_cfg,
            self.table.clone(),
        )
        .map_err(MimoError::Plant)?;
        if (self.loop_cfg.df() - before.1).abs() > 1e-9 * before.1 {
            return Err(MimoError::Plant(format!(
                "the station runs at {} Hz, not the {} Hz the reference was built for; rebuild \
                 the reference on the station's grid",
                self.loop_cfg.fs_hz, before.0
            )));
        }
        if plant.n_outputs() != self.n_d {
            return Err(MimoError::Plant(format!(
                "the plant loop drives {} outputs and the test has {} drives",
                plant.n_outputs(),
                self.n_d
            )));
        }
        self.plant_loop = Some(plant);
        let mut lc = self.loop_cfg.clone();
        lc.cola_seed = self.cfg.seed;
        self.synth = Some(DriveSynth::with_outputs(&lc, self.n_d));
        self.estimator = Some(SdmEstimator::new(
            self.loop_cfg.nfft,
            self.loop_cfg.fs_hz,
            0.5,
            self.band,
        ));
        self.kinematic = self.build_kinematic();
        self.note(format!(
            "Armed: {} drives, {} controls ({} in the control space), {} limit channels, {} \
             lines of {:.3} Hz, band lines {} to {}, seed {:#x}.",
            self.n_d,
            self.n_c,
            self.n_r,
            self.n_l,
            self.lines,
            self.df_hz,
            self.band.0,
            self.band.1 - 1,
            self.cfg.seed
        ));
        if self.cfg.records.enabled {
            if let Err(e) = self.start_records() {
                self.note(format!(
                    "Records could not start ({e}); the run continues without them."
                ));
            }
        }
        self.transition(MimoState::NoiseFloor);
        Ok(())
    }

    fn build_kinematic(&self) -> Option<KinematicMonitor> {
        let k = KinematicMonitor::new(&self.table, self.loop_cfg.fs_hz, 5.0, 1, 5.0);
        k.is_active().then_some(k)
    }

    // --- playing a drive ---------------------------------------------------------------

    /// The factor of a physical drive SDM at `level_db`, for the synthesizer.
    fn factor_of(
        &self,
        g_dd_full: &HermitianStack,
        level_db: f64,
    ) -> Result<DriveFactor, MimoError> {
        let mut g = g_dd_full.clone();
        let p = 10f64.powf(level_db / 10.0);
        let _ = g.scale_lines(&vec![p; self.lines]);
        let l = la(pool::sqrt_factor(&g, self.band))?;
        let n = self.n_d;
        let mut f = DriveFactor::zeros(self.lines, n, n);
        for line in self.band.0..self.band.1 {
            let src = l.line(line);
            let dst = &mut f.data[line * n * n..(line + 1) * n * n];
            for (d, s) in dst.iter_mut().zip(src) {
                *d = (s.re, s.im);
            }
        }
        Ok(f)
    }

    /// Synthesize, clip, play one segment. Returns the segment and each output's
    /// drive record aligned to the response.
    fn play(
        &mut self,
        factor: &DriveFactor,
        ramp_base_db: Option<f64>,
    ) -> Result<(SegmentResult, Vec<Vec<f64>>), MimoError> {
        let n = self.n_d;
        let t0 = Instant::now();
        let plant = self
            .plant_loop
            .as_mut()
            .ok_or_else(|| MimoError::BadState("the run is not armed".into()))?;
        let lead = plant.cold_leadin_frames();
        let synth = self
            .synth
            .as_mut()
            .ok_or_else(|| MimoError::BadState("the run is not armed".into()))?;
        let mut leadin = if lead > 0 {
            synth
                .frames_factored(lead, factor)
                .map_err(MimoError::Plant)?
        } else {
            Vec::new()
        };
        let mut record = synth.segment_factored(factor).map_err(MimoError::Plant)?;
        // Peak clip per output.
        for (i, v) in leadin.iter_mut().chain(record.iter_mut()).enumerate() {
            let pk = self.cfg.drives[i % n].peak_v;
            *v = v.clamp(-pk, pk);
        }
        self.timing.synth_ms = t0.elapsed().as_secs_f64() * 1e3;
        let frames = record.len() / n;
        // The stream as played, per output: the tail of what came before, the
        // lead-in, the record.
        let mut streams: Vec<Vec<f64>> = Vec::with_capacity(n);
        for k in 0..n {
            let mut s = std::mem::take(&mut self.drive_tail[k]);
            s.extend(leadin.iter().skip(k).step_by(n));
            s.extend(record.iter().skip(k).step_by(n));
            streams.push(s);
        }
        let t1 = Instant::now();
        plant.set_drive_ramp(ramp_base_db.map(|b| (self.ramp.clone(), b)));
        let seg = plant.run_segment(leadin, record);
        self.timing.plant_ms = t1.elapsed().as_secs_f64() * 1e3;
        if self.need_lag {
            self.need_lag = false;
            self.lag_frames = self.measure_lag(&seg, &streams[0]);
        }
        // Aligned drive: response frame t pairs with stream frame t - lag.
        let lag = self.lag_frames;
        let keep = MAX_LAG_FRAMES.max(lag) + 1;
        let mut aligned = Vec::with_capacity(n);
        for s in &mut streams {
            let end = s.len();
            let start = end - frames;
            let a: Vec<f64> = (0..frames)
                .map(|t| {
                    let i = (start + t) as isize - lag as isize;
                    if i >= 0 {
                        s[i as usize]
                    } else {
                        0.0
                    }
                })
                .collect();
            aligned.push(a);
            let cut = end.saturating_sub(keep);
            s.drain(..cut);
        }
        self.drive_tail = streams;
        Ok((seg, aligned))
    }

    /// Mark the drive stream as interrupted (a suspend): what came before is
    /// silence.
    fn drive_discontinuity(&mut self) {
        for t in &mut self.drive_tail {
            t.clear();
        }
        if let Some(p) = self.plant_loop.as_mut() {
            p.suspend();
        }
    }

    /// The response records of a segment: controls, then limit channels.
    fn responses(&self, seg: &SegmentResult) -> Vec<Vec<f64>> {
        self.response_slots
            .iter()
            .map(|&s| seg.channel(s))
            .collect()
    }

    fn estimate(&self, seg: &SegmentResult, drives: &[Vec<f64>]) -> Estimate {
        let resp = self.responses(seg);
        let r: Vec<&[f64]> = resp.iter().map(|v| v.as_slice()).collect();
        let d: Vec<&[f64]> = drives.iter().map(|v| v.as_slice()).collect();
        self.estimator
            .as_ref()
            .map(|e| e.estimate(&r, self.n_c, &d))
            .unwrap_or_else(|| Estimate::zeros(self.lines, self.n_c, self.n_c + self.n_l, self.n_d))
    }

    /// The protections every segment is judged by, identification included: a
    /// faulted engine, the engine's watchdog, the kinematic monitor, each drive's
    /// rms abort, the overload latches. Returns `false` when the run aborted.
    fn segment_protections(&mut self, seg: &SegmentResult, drive_rms: &[f64]) -> bool {
        if let Some(msg) = seg.faulted.clone() {
            self.abort(MimoTripKind::Plant { message: msg });
            return false;
        }
        if let Some(t) = seg.watchdog_trip {
            self.abort(MimoTripKind::Kinematic {
                channel_index: t.channel_index,
                axis: format!("{:?}", t.axis),
                value: t.value,
                limit: t.limit,
            });
            return false;
        }
        if let Some(k) = self.kinematic.as_mut() {
            if let Some(t) = k.feed(&seg.interleaved) {
                self.abort(MimoTripKind::Kinematic {
                    channel_index: t.channel_index,
                    axis: format!("{:?}", t.axis),
                    value: t.value,
                    limit: t.limit,
                });
                return false;
            }
        }
        for (k, &rms) in drive_rms.iter().enumerate() {
            let d = &self.cfg.drives[k];
            if rms >= d.abort_level_vrms() {
                let kind = MimoTripKind::DriveVrms {
                    drive: k,
                    channel_index: d.channel_index,
                    realized_vrms: rms,
                    limit_vrms: d.abort_level_vrms(),
                };
                self.abort(kind);
                return false;
            }
        }
        self.check_overloads(seg);
        if let Some(r) = self.records.as_mut() {
            r.feed_raw(seg);
        }
        true
    }

    fn check_overloads(&mut self, seg: &SegmentResult) {
        let station = self
            .plant_loop
            .as_mut()
            .map(|p| p.input_overloads())
            .unwrap_or_default();
        let frac = self.cfg.protections.overload_fraction;
        let n_ch = seg.n_ch.max(1);
        let frames = seg.interleaved.len() / n_ch;
        let mut latched = Vec::new();
        for (slot, (idx, range, l)) in self.overloads.iter_mut().enumerate() {
            if *l {
                continue;
            }
            let hit = station.get(slot).copied().unwrap_or(false)
                || (slot < n_ch
                    && (0..frames)
                        .any(|f| seg.interleaved[f * n_ch + slot].abs() >= frac * *range));
            if hit {
                *l = true;
                latched.push(*idx);
            }
        }
        for idx in latched {
            self.log
                .push(MimoEventKind::OverloadLatch { channel_index: idx });
        }
    }

    /// The realized rms of each output's record, volts.
    fn record_rms(drives: &[Vec<f64>]) -> Vec<f64> {
        drives
            .iter()
            .map(|d| {
                if d.is_empty() {
                    0.0
                } else {
                    (d.iter().map(|x| x * x).sum::<f64>() / d.len() as f64).sqrt()
                }
            })
            .collect()
    }

    /// Limit channels measured against their ceilings plus margin (`level_db`
    /// referred). Returns `false` when the run aborted.
    fn limit_aborts(&mut self, est: &Estimate, level_db: f64) -> bool {
        if self.n_l == 0 {
            return true;
        }
        let nr = self.n_c + self.n_l;
        let p = 10f64.powf(level_db / 10.0);
        for (i, l) in self.cfg.limits.clone().iter().enumerate() {
            let margin = 10f64.powf(l.abort_margin_db / 10.0);
            for line in self.band.0..self.band.1 {
                let m = est.auto[line * nr + self.n_c + i];
                let c = l.ceiling(line) * p * margin;
                if m > c {
                    let over_db = 10.0 * (m / (l.ceiling(line) * p)).log10();
                    self.abort(MimoTripKind::Limit {
                        channel_index: l.channel_index,
                        line,
                        over_db,
                    });
                    return false;
                }
            }
        }
        true
    }

    // --- noise floor and identification ------------------------------------------------

    /// Measure the controls with the drives off: `NoiseFloor -> SysId`.
    pub fn run_noise_floor(&mut self) -> Result<(), MimoError> {
        if self.state != MimoState::NoiseFloor {
            return Err(MimoError::BadState(format!(
                "the noise floor is measured right after arming; the run is {:?}",
                self.state
            )));
        }
        let zero = DriveFactor::zeros(self.lines, self.n_d, self.n_d);
        let mut acc: Option<Estimate> = None;
        let want = self.cfg.identification.noise_frames;
        while acc.as_ref().map(|e| e.frames).unwrap_or(0) < want {
            let (seg, drives) = self.play(&zero, None)?;
            if !self.segment_protections(&seg, &vec![0.0; self.n_d]) {
                return Ok(());
            }
            let e = self.estimate(&seg, &drives);
            acc = Some(merge(acc, e));
        }
        let e = acc.expect("at least one segment");
        self.noise = Some(e.g_cc.clone());
        self.note(format!("Noise floor measured over {} frames.", e.frames));
        self.transition(MimoState::SysId);
        Ok(())
    }

    /// The flat, uncorrelated identification drive: each output `vrms` over the
    /// band, independent phases per output.
    fn ident_factor(&self, vrms: &[f64]) -> DriveFactor {
        let n = self.n_d;
        let bw = (self.band.1 - self.band.0) as f64 * self.df_hz;
        let mut f = DriveFactor::zeros(self.lines, n, n);
        for line in self.band.0..self.band.1 {
            for k in 0..n {
                f.data[(line * n + k) * n + k] = ((vrms[k] * vrms[k] / bw).sqrt(), 0.0);
            }
        }
        f
    }

    /// The controls' mean in-band power relative to the reference, dB.
    fn level_vs_reference(&self, g_cc: &HermitianStack) -> Result<f64, MimoError> {
        let v = self.to_control(g_cc)?;
        let (mut m, mut r) = (0.0, 0.0);
        for line in self.band.0..self.band.1 {
            for i in 0..self.n_r {
                m += v.get(line, i, i).re.max(0.0);
                r += self.ref_diag[line * self.n_r + i].max(0.0);
            }
        }
        Ok(if r > 0.0 && m > 0.0 {
            10.0 * (m / r).log10()
        } else {
            f64::NEG_INFINITY
        })
    }

    /// The lag between the drive as synthesized and the response, frames: the
    /// peak of the cross-correlation of drive 0 with the first controls over a
    /// window of the record. A lag common to every drive is a pure delay the FRF
    /// absorbs; aligning by it keeps the coherence (an unaligned lag near the
    /// frame length destroys it).
    fn measure_lag(&self, seg: &SegmentResult, stream0: &[f64]) -> usize {
        let resp = self.responses(seg);
        let len = resp.first().map(|r| r.len()).unwrap_or(0);
        if len < 256 || stream0.len() < len {
            return 0;
        }
        let offset = stream0.len() - len;
        let max_lag = MAX_LAG_FRAMES.min(offset).min(len / 2);
        let w = (len / 2).min(8192);
        let start = len - w;
        let mut best = (0usize, 0.0f64);
        for lag in 0..=max_lag {
            let mut acc = 0.0;
            for r in resp.iter().take(self.n_c.min(4)) {
                let mut s = 0.0;
                for t in start..start + w {
                    s += r[t] * stream0[offset + t - lag];
                }
                acc += s.abs();
            }
            if acc > best.1 {
                best = (lag, acc);
            }
        }
        best.0
    }

    /// System identification (Q34): uncorrelated random on every drive, stepped
    /// from the start level until the controls sit `below_reference_db` under the
    /// reference (never above any drive's identification limit), then recorded for
    /// `frames` Welch frames. Every absolute protection is armed. `SysId -> Gate`,
    /// or `Aborted` with the trip recorded (and `Ok`).
    pub fn run_identification(&mut self) -> Result<(), MimoError> {
        if self.state != MimoState::SysId {
            return Err(MimoError::BadState(format!(
                "identification follows the noise floor; the run is {:?}",
                self.state
            )));
        }
        let id = self.cfg.identification;
        let mut vrms = vec![id.start_vrms; self.n_d];
        let target = -id.below_reference_db;
        let mut steps = 0usize;
        let mut acc: Option<Estimate>;
        // The level search.
        loop {
            let factor = self.ident_factor(&vrms);
            if steps == 0 {
                self.need_lag = true;
            }
            let (seg, drives) = self.play(&factor, None)?;
            let rms = Self::record_rms(&drives);
            if !self.segment_protections(&seg, &rms) {
                return Ok(());
            }
            let e = self.estimate(&seg, &drives);
            if !self.limit_aborts(&e, 0.0) {
                return Ok(());
            }
            let level = self.level_vs_reference(&e.g_cc)?;
            steps += 1;
            if (level - target).abs() <= id.tolerance_db {
                acc = Some(e);
                break;
            }
            let at_limit = vrms
                .iter()
                .zip(&self.cfg.drives)
                .all(|(v, d)| *v >= d.pretest_max_vrms * (1.0 - 1e-9));
            if (level < target && at_limit) || steps >= id.max_steps || !level.is_finite() {
                let msg = format!(
                    "the controls reached {level:.1} dB against the reference after {steps} \
                     steps, short of {target:.1} dB within {:.1} dB, with each drive at most at \
                     its identification limit; raise the identification limits or check the \
                     wiring and sensitivities",
                    id.tolerance_db
                );
                self.note(msg.clone());
                return Err(MimoError::IdentUnreachable(msg));
            }
            let gain = 10f64.powf(((target - level) / 20.0).clamp(-1.0, 1.0));
            for (v, d) in vrms.iter_mut().zip(&self.cfg.drives) {
                *v = (*v * gain).min(d.pretest_max_vrms);
            }
        }
        // The record.
        while acc.as_ref().map(|e| e.frames).unwrap_or(0) < id.frames {
            let factor = self.ident_factor(&vrms);
            let (seg, drives) = self.play(&factor, None)?;
            let rms = Self::record_rms(&drives);
            if !self.segment_protections(&seg, &rms) {
                return Ok(());
            }
            let e = self.estimate(&seg, &drives);
            if !self.limit_aborts(&e, 0.0) {
                return Ok(());
            }
            acc = Some(merge(acc, e));
        }
        let e = acc.expect("identification record");
        let level = self.level_vs_reference(&e.g_cc)?;
        self.finish_identification(e, vrms, level, steps)?;
        Ok(())
    }

    fn finish_identification(
        &mut self,
        e: Estimate,
        vrms: Vec<f64>,
        level_db: f64,
        steps: usize,
    ) -> Result<(), MimoError> {
        let frf = la(pool::h1_frf(
            &e.g_rd,
            &e.g_dd,
            Regularization::None,
            self.band,
        ))?;
        let g_cd = e.g_cd(self.n_c);
        let g_vv = self.to_control(&e.g_cc)?;
        let g_vd = self.csd_to_control(&g_cd)?;
        let coherence = la(pool::multiple_coherence(
            &g_vv,
            &g_vd,
            &e.g_dd,
            Regularization::None,
            self.band,
        ))?;
        let h_c = crate::estimate::rows(&frf, 0..self.n_c);
        let frf_law = self.frf_to_law(&h_c)?;
        let condition = la(pool::condition_numbers(&frf_law, self.band))?;
        let set = IdentificationSet {
            setup_hash: self.setup_hash(),
            frf,
            g_cc: e.g_cc,
            g_rd: e.g_rd,
            g_dd: e.g_dd,
            noise: self
                .noise
                .clone()
                .unwrap_or_else(|| HermitianStack::zeros(self.lines, self.n_c)),
            coherence,
            condition,
            drive_vrms: vrms,
            level_db,
            frames: e.frames,
            steps,
            lag_frames: self.lag_frames,
        };
        self.adopt_identification(set, false)
    }

    fn adopt_identification(
        &mut self,
        set: IdentificationSet,
        loaded: bool,
    ) -> Result<(), MimoError> {
        let h_c = crate::estimate::rows(&set.frf, 0..self.n_c);
        let h_l = crate::estimate::rows(&set.frf, self.n_c..self.n_c + self.n_l);
        let frf_law = self.frf_to_law(&h_c)?;
        if self.n_r > self.n_u {
            let ach = la(pool::projection_achievable(
                &frf_law,
                &self.reference,
                Regularization::None,
                self.band,
            ))?;
            self.achievable_diag = hertzforge_sdm::linalg::diagonals(&ach);
        }
        self.coherence = set.coherence.clone();
        self.condition = set.condition.clone();
        self.frf_c = Some(h_c);
        self.frf_l = Some(h_l);
        self.frf_law_ident = Some(frf_law.clone());
        self.frf_law = Some(frf_law);
        self.noise = Some(set.noise.clone());
        self.lag_frames = set.lag_frames;
        self.ident_loaded = loaded;
        self.note(format!(
            "Identification {}: {} frames, controls at {:.1} dB against the reference, lag {} \
             frames.",
            if loaded { "loaded" } else { "measured" },
            set.frames,
            set.level_db,
            set.lag_frames
        ));
        if let Some(r) = self.records.as_mut() {
            match r.write_identification(&set) {
                Ok(p) => self.log.push(MimoEventKind::Record { path: p }),
                Err(e) => self.faults.push(format!("identification record: {e}")),
            }
        }
        self.ident = Some(set);
        self.gate = Some(self.gate_report());
        self.transition(MimoState::Gate);
        Ok(())
    }

    /// Load an identification set from an earlier run on the same setup, skipping
    /// the noise floor and the identification: `NoiseFloor -> Gate`. Refused when
    /// the setup hash differs.
    pub fn load_identification(&mut self, set: IdentificationSet) -> Result<(), MimoError> {
        if self.state != MimoState::NoiseFloor {
            return Err(MimoError::BadState(format!(
                "an identification set is loaded right after arming; the run is {:?}",
                self.state
            )));
        }
        let want = self.setup_hash();
        if set.setup_hash != want {
            return Err(MimoError::Blocked(format!(
                "the identification set was measured on another setup (hash {}, this setup {}); \
                 run the identification again",
                set.setup_hash, want
            )));
        }
        if set.frf.dims() != (self.lines, self.n_c + self.n_l, self.n_d) {
            return Err(MimoError::Blocked(
                "the identification set does not fit this test's grid or channels; run the \
                 identification again"
                    .into(),
            ));
        }
        self.adopt_identification(set, true)
    }

    /// The setup hash: FNV-1a over the channel roles and sensitivities, the
    /// transformations, the drives and the grid. A loaded identification set must
    /// carry the same.
    pub fn setup_hash(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "lines={};df={:.12e};band={:?};",
            self.lines, self.df_hz, self.band
        ));
        let find = |idx: u32| self.table.channels.iter().find(|c| c.index == idx);
        for (role, idx) in self
            .cfg
            .controls
            .iter()
            .map(|&c| ("c", c))
            .chain(self.cfg.limits.iter().map(|l| ("l", l.channel_index)))
            .chain(self.cfg.drives.iter().map(|d| ("d", d.channel_index)))
        {
            let sens = find(idx)
                .map(|c| c.sensitivity_mv_per_eu)
                .unwrap_or(f64::NAN);
            s.push_str(&format!("{role}{idx}:{sens:.12e};"));
        }
        for t in [&self.t_resp, &self.t_drive].into_iter().flatten() {
            s.push_str(&format!("t{:?}:", t.dims()));
            for v in t.data() {
                s.push_str(&format!("{:.12e},{:.12e};", v.re, v.im));
            }
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in s.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    fn gate_report(&self) -> GateReport {
        let (lo, hi) = self.band;
        let n = self.n_r;
        let coherence: Vec<f64> = (0..n)
            .map(|i| {
                let s: f64 = (lo..hi).map(|l| self.coherence[l * n + i]).sum();
                s / (hi - lo) as f64
            })
            .collect();
        let g = self.cfg.gates;
        let low: Vec<usize> = (0..n)
            .filter(|&i| coherence[i] < g.coherence_threshold)
            .collect();
        let ill: Vec<usize> = (lo..hi)
            .filter(|&l| self.condition[l].is_nan() || self.condition[l] > g.condition_threshold)
            .collect();
        let worst = (lo..hi).map(|l| self.condition[l]).fold(0.0f64, |a, b| {
            if b.is_nan() {
                f64::INFINITY
            } else {
                a.max(b)
            }
        });
        GateReport {
            passed: low.is_empty() && ill.is_empty(),
            coherence,
            coherence_threshold: g.coherence_threshold,
            low_coherence: low,
            ill_conditioned_lines: ill,
            condition_threshold: g.condition_threshold,
            worst_condition: worst,
        }
    }

    // --- gate, prediction, engage ----------------------------------------------------

    /// Go on: from `Gate` to `Predict` (a failed gate needs an override first);
    /// from `Predict` to `Engaged` (a prediction over a ceiling needs an override
    /// first).
    pub fn proceed(&mut self) -> Result<(), MimoError> {
        match self.state {
            MimoState::Gate => {
                let passed = self.gate.as_ref().map(|g| g.passed).unwrap_or(false);
                if !passed && !self.acknowledged.iter().any(|a| a == "gate") {
                    return Err(MimoError::Blocked(
                        "the identification gate did not pass (low coherence or ill-conditioned \
                         lines); acknowledge an override or change the setup"
                            .into(),
                    ));
                }
                self.predict()
            }
            MimoState::Predict => {
                let breach = self
                    .prediction
                    .as_ref()
                    .map(|p| !p.breaches.is_empty())
                    .unwrap_or(true);
                if breach && !self.acknowledged.iter().any(|a| a == "prediction") {
                    return Err(MimoError::Blocked(
                        "the predicted drive is over a ceiling on at least one output; change \
                         the law, its parameters or the setup, or acknowledge an override (the \
                         ceilings still hold the drive)"
                            .into(),
                    ));
                }
                self.engage();
                Ok(())
            }
            s => Err(MimoError::BadState(format!(
                "proceed applies at the gate and the prediction; the run is {s:?}"
            ))),
        }
    }

    fn law_budget_s(&self) -> f64 {
        self.cfg.law_guards.budget_fraction * self.loop_cfg.segment_seconds()
    }

    fn setup(&self) -> MimoSetup {
        MimoSetup {
            controls: self.n_r,
            drives: self.n_u,
            lines: self.lines,
            df_hz: self.df_hz,
            band: self.band,
            reference: self.reference.clone(),
            tolerances: self.cfg.reference.tolerances.clone(),
        }
    }

    /// Build the law and seed it against the identification set; the prediction
    /// follows. `Gate -> Predict`.
    fn predict(&mut self) -> Result<(), MimoError> {
        let factory = self.law_factory.clone().ok_or_else(|| {
            MimoError::Law(format!(
                "no factory for law `{}`; choose a built-in law or resolve the law file through \
                 the catalog",
                self.cfg.law.id
            ))
        })?;
        let setup = self.setup();
        let budget = std::time::Duration::from_secs_f64(self.law_budget_s().max(1e-3));
        let law = factory
            .build(&setup, budget)
            .map_err(|e| MimoError::Law(e.to_string()))?;
        let shape = MimoShape::of(self.n_r, self.n_u);
        if !law.shapes().contains(&shape) {
            return Err(MimoError::Law(format!(
                "the law does not support a test of {} controls and {} drives ({shape:?}); \
                 choose another law",
                self.n_r, self.n_u
            )));
        }
        self.law_needs = law.needs().to_vec();
        let id = factory.identity();
        self.law_line = Some(LawLine {
            id: self.cfg.law.id.clone(),
            name: id.name,
            version: id.version,
            sha256: id.sha256,
            parameters: factory
                .params()
                .iter()
                .map(|p| format!("{}={:?}", p.key, p.value))
                .collect(),
        });
        self.law = Some(law);
        // The seed: the identification response referred to the reference's level.
        let set = self.ident.clone().ok_or_else(|| {
            MimoError::BadState("there is no identification set to seed the law with".into())
        })?;
        let mut resp = self.to_control(&set.g_cc)?;
        let k = 10f64.powf(-set.level_db / 10.0);
        if k.is_finite() {
            let _ = resp.scale_lines(&vec![k; self.lines]);
        }
        let out = self.call_law(0, &resp, f64::from(2 * set.frames as u32), None);
        let out = match out {
            Ok(o) => o,
            Err(m) => {
                return Err(MimoError::Law(format!(
                    "the law's seed was not usable: {}; change the law or its parameters",
                    m.words()
                )))
            }
        };
        self.prediction_from(out, 0)?;
        self.transition(MimoState::Predict);
        Ok(())
    }

    /// Feed the predicted response back to the law as if measured, to see where a
    /// closed-loop law settles before anything drives at test level.
    pub fn iterate_prediction(&mut self) -> Result<(), MimoError> {
        if self.state != MimoState::Predict {
            return Err(MimoError::BadState(format!(
                "the prediction is iterated in the prediction step; the run is {:?}",
                self.state
            )));
        }
        let frf = self
            .frf_law
            .clone()
            .ok_or_else(|| MimoError::BadState("no FRF".into()))?;
        let pred = la(pool::response_from_drive(
            &frf,
            &self.ls.law_drive,
            self.band,
        ))?;
        let it = self.prediction.as_ref().map(|p| p.iterations).unwrap_or(0) + 1;
        let dof = 2.0 * self.cfg.estimation.frames_per_loop as f64;
        let out = self
            .call_law(u64::from(it), &pred, dof, None)
            .map_err(|m| {
                MimoError::Law(format!("the iterated prediction failed: {}", m.words()))
            })?;
        self.prediction_from(out, it)
    }

    /// Guard a proposed drive and make the prediction from it.
    fn prediction_from(&mut self, out: MimoLoopOutputs, iterations: u32) -> Result<(), MimoError> {
        let mut drive_u = out.drive;
        let rep = guards::check_output(&mut drive_u, self.lines, self.n_u, self.band)
            .map_err(|(f, d)| MimoError::Law(format!("{} ({d})", f.words())))?;
        let requested = self.expand_drive(&drive_u)?;
        let req_rms = guards::drive_rms(&requested, self.band, self.df_hz);
        let ceilings: Vec<f64> = self.cfg.drives.iter().map(|d| d.ceiling_vrms).collect();
        let breaches: Vec<usize> = (0..self.n_d)
            .filter(|&k| req_rms[k] > ceilings[k])
            .collect();
        let mut played = requested;
        let cf = guards::apply_ceilings(&mut played, self.band, self.df_hz, &ceilings);
        let (_, limited) = self.apply_limits(&mut played);
        let frf = self.frf_law.clone().expect("identified");
        let played_u = self.reduce_drive(&played)?;
        let resp = la(pool::response_from_drive(&frf, &played_u, self.band))?;
        let (err, _, _) = self.errors_of(&resp, false);
        let unreachable_db: Vec<f64> = (0..self.n_r)
            .map(|i| {
                let (mut a, mut r) = (0.0, 0.0);
                for l in self.band.0..self.band.1 {
                    a += self.achievable_diag[l * self.n_r + i];
                    r += self.ref_diag[l * self.n_r + i];
                }
                if a > 0.0 && r > 0.0 {
                    10.0 * (a / r).log10()
                } else {
                    0.0
                }
            })
            .collect();
        self.guards_last = GuardSummary {
            repair: rep,
            capped: 0,
            ceiling_bound: cf.bound,
            limited_lines: limited,
            peak_change_db: 0.0,
        };
        self.ls.law_drive = drive_u;
        self.ls.drive_full = played;
        self.ls.diagnostics = out.diagnostics;
        self.prediction = Some(Prediction {
            drive_rms: req_rms,
            ceilings,
            breaches,
            error_db: err,
            iterations,
            has_achievable: self.n_r > self.n_u,
            unreachable_db,
        });
        Ok(())
    }

    /// Engage at the schedule's start level: `Predict -> Engaged`.
    fn engage(&mut self) {
        self.drive_discontinuity();
        self.ls.settle = self.cfg.protections.settle_loops;
        self.ls.est = None;
        self.ls.est_loops = 0;
        self.kinematic = self.build_kinematic();
        self.level_index = 0;
        let start = self.cfg.schedule.start_db;
        self.set_level_now(start);
        self.level_target_db = start;
        self.ramp.set(start);
        self.transition(MimoState::Engaged);
        if self.cfg.schedule.mode == RunMode::Auto {
            // Start on the first rung at or above the start level.
            self.level_index = self
                .cfg
                .schedule
                .table_db
                .iter()
                .position(|&l| l >= start - 1e-9)
                .unwrap_or(0);
        }
    }

    fn set_level_now(&mut self, level_db: f64) {
        let from = self.schedule_level_db;
        if from.is_finite() && (level_db - from).abs() > 1e-12 {
            let p = 10f64.powf((level_db - from) / 10.0);
            if let Some(e) = self.ls.est.as_mut() {
                e.scale(p);
            }
        }
        self.schedule_level_db = level_db;
    }

    /// Command a level (the ladder, or an operator step): ramped over
    /// `level_ramp_s` while the loop drives, instant otherwise.
    pub fn set_level(&mut self, level_db: f64, auto: bool) {
        let level_db = level_db.min(0.0);
        let from = self.schedule_level_db;
        self.level_target_db = level_db;
        let driving = matches!(self.state, MimoState::Engaged | MimoState::FullLevel);
        if driving && self.cfg.level_ramp_s > f64::EPSILON && (level_db - from).abs() > 1e-9 {
            let rate = ((level_db - from).abs() / self.cfg.level_ramp_s).max(0.1);
            self.ramp.ramp_to(level_db, rate);
        } else {
            self.set_level_now(level_db);
            self.ramp.set(level_db);
        }
        self.elapsed_at_level_s = 0.0;
        self.log.push(MimoEventKind::LevelChange {
            from_db: from,
            to_db: level_db,
            auto,
        });
    }

    fn maybe_advance_auto(&mut self) {
        if self.state != MimoState::Engaged || self.cfg.schedule.mode != RunMode::Auto {
            return;
        }
        if self.elapsed_at_level_s < self.cfg.schedule.dwell_for(self.level_index) {
            return;
        }
        if self.level_index + 1 < self.cfg.schedule.table_db.len() {
            self.level_index += 1;
            let next = self.cfg.schedule.table_db[self.level_index];
            self.set_level(next, true);
        } else if self.schedule_level_db >= -1e-9 {
            self.transition(MimoState::FullLevel);
            self.elapsed_at_level_s = 0.0;
        }
    }

    /// A controlled stop: the drive ramps down over `shutdown_ramp_s`, then the run
    /// stops. Immediate when nothing drives or no ramp is set.
    pub fn request_stop(&mut self) -> Result<(), MimoError> {
        if matches!(self.state, MimoState::Engaged | MimoState::FullLevel)
            && self.ramp.request_stop().is_some()
        {
            self.level_target_db = self.ramp.target_db();
            self.note(format!(
                "Controlled stop: ramping down over {:.1} s, then stopping.",
                self.cfg.shutdown_ramp_s
            ));
            return Ok(());
        }
        self.stop();
        Ok(())
    }

    /// Stop now (clean): `Stopped`, unless already aborted.
    pub fn stop(&mut self) {
        if let Some(p) = self.plant_loop.as_mut() {
            p.suspend();
        }
        if self.state != MimoState::Aborted {
            self.transition(MimoState::Stopped);
        }
        // After the transition, so the report records the state the run ended in.
        self.finish_records();
    }

    /// The operator's abort: the drive to zero at once.
    pub fn abort_now(&mut self, why: &str) {
        self.note(format!("Operator abort: {why}"));
        if let Some(p) = self.plant_loop.as_mut() {
            p.suspend();
        }
        self.transition(MimoState::Aborted);
        self.finish_records();
    }

    fn abort(&mut self, kind: MimoTripKind) {
        let record = MimoTripRecord {
            kind,
            at_level_db: self.schedule_level_db,
            at_loop: self.loop_count,
        };
        self.log.push(MimoEventKind::Trip {
            record: record.clone(),
        });
        self.trip = Some(record);
        if let Some(p) = self.plant_loop.as_mut() {
            p.suspend();
        }
        self.transition(MimoState::Aborted);
        self.finish_records();
    }

    fn stop_for_law(&mut self, reason: MimoFaultReason) {
        if self.law_retired {
            return;
        }
        self.law_retired = true;
        let record = MimoTripRecord {
            kind: MimoTripKind::LawFault {
                reason,
                consecutive: self.law_misses,
            },
            at_level_db: self.schedule_level_db,
            at_loop: self.loop_count,
        };
        self.log.push(MimoEventKind::Trip {
            record: record.clone(),
        });
        self.trip = Some(record);
        let _ = self.request_stop();
    }

    // --- the law call -----------------------------------------------------------------

    /// One guarded law call with the declared inputs. `response` is at full level
    /// in the control space.
    fn call_law(
        &mut self,
        loop_index: u64,
        response: &HermitianStack,
        dof: f64,
        csd: Option<&MatrixStack>,
    ) -> Result<MimoLoopOutputs, Miss> {
        let frf = match self.frf_law.clone() {
            Some(f) => f,
            None => {
                return Err(Miss {
                    reason: MimoFaultReason::Error,
                    detail: "no FRF".into(),
                })
            }
        };
        let needs = self.law_needs.clone();
        let want = |n: MimoInput| needs.contains(&n);
        let last_drive = if want(MimoInput::LastDrive) {
            Some(
                self.reduce_drive(&self.ls.drive_full)
                    .unwrap_or_else(|_| self.ls.law_drive.clone()),
            )
        } else {
            None
        };
        let ident = self.ident.clone();
        let sysid_response = match (&ident, want(MimoInput::SysidResponse)) {
            (Some(s), true) => self.to_control(&s.g_cc).ok(),
            _ => None,
        };
        let sysid_drive = match (&ident, want(MimoInput::SysidDrive)) {
            (Some(s), true) => self.reduce_drive(&s.g_dd).ok(),
            _ => None,
        };
        let sysid_noise = match (&self.noise, want(MimoInput::SysidNoise)) {
            (Some(n), true) => self.to_control(n).ok(),
            _ => None,
        };
        let csd_owned = if want(MimoInput::ResponseDriveCsd) {
            match csd {
                Some(c) => Some(c.clone()),
                None => ident.as_ref().and_then(|s| {
                    self.csd_to_law(&crate::estimate::rows(&s.g_rd, 0..self.n_c))
                        .ok()
                }),
            }
        } else {
            None
        };
        let tolerances = self.cfg.reference.tolerances.clone();
        let reference = self.reference.clone();
        let coherence = self.coherence.clone();
        let inputs = MimoLoopInputs {
            reference: &reference,
            tolerances: &tolerances,
            response,
            response_dof: dof,
            frf: &frf,
            multiple_coherence: &coherence,
            df_hz: self.df_hz,
            band: self.band,
            loop_index,
            last_drive: last_drive.as_ref(),
            response_drive_csd: csd_owned.as_ref(),
            sysid_response: sysid_response.as_ref(),
            sysid_drive: sysid_drive.as_ref(),
            sysid_noise: sysid_noise.as_ref(),
        };
        let clock = self.law_clock.clone();
        let budget = self.law_budget_s();
        let law = match self.law.as_mut() {
            Some(l) => l,
            None => {
                return Err(Miss {
                    reason: MimoFaultReason::Error,
                    detail: "no law".into(),
                })
            }
        };
        let t0 = Instant::now();
        let r = guarded_call(law.as_mut(), clock.as_ref(), budget, |l| {
            if loop_index == 0 {
                l.seed(&inputs)
            } else {
                l.iterate(&inputs)
            }
        });
        self.timing.law_ms = t0.elapsed().as_secs_f64() * 1e3;
        r
    }

    fn law_missed(&mut self, miss: Miss) {
        self.law_stats.calls += 1;
        self.law_stats.misses += 1;
        self.law_misses += 1;
        self.log.push(MimoEventKind::LawMiss {
            reason: miss.reason,
            detail: miss.detail.clone(),
        });
        if self.law_misses >= self.cfg.law_guards.max_misses && self.law_stop_due.is_none() {
            self.law_stop_due = Some(miss.reason);
        }
        self.last_miss = Some(miss);
    }

    /// Limit the limit channels by uniform scaling per line (design 8.2).
    fn apply_limits(&self, g: &mut HermitianStack) -> (Vec<f64>, usize) {
        match &self.frf_l {
            Some(h_l) if self.n_l > 0 => {
                let limits = self.cfg.limits.clone();
                guards::limit_responses(g, h_l, &|i, line| limits[i].ceiling(line), self.band)
            }
            _ => (vec![1.0; self.lines], 0),
        }
    }

    /// A usable law drive becomes the drive in force: checked, expanded, capped,
    /// held under each ceiling, limited.
    fn accept(&mut self, out: MimoLoopOutputs) -> Result<(), Miss> {
        let t0 = Instant::now();
        let mut drive_u = out.drive;
        let rep = match guards::check_output(&mut drive_u, self.lines, self.n_u, self.band) {
            Ok(r) => r,
            Err((fault, detail)) => {
                return Err(Miss {
                    reason: MimoFaultReason::BadOutput(fault),
                    detail,
                })
            }
        };
        let mut g = self.expand_drive(&drive_u).map_err(|e| Miss {
            reason: MimoFaultReason::Error,
            detail: e.to_string(),
        })?;
        let caps = guards::cap_change(
            &mut g,
            &self.ls.drive_full,
            self.band,
            self.cfg.caps.up_db,
            self.cfg.caps.down_db,
        );
        let ceilings: Vec<f64> = self.cfg.drives.iter().map(|d| d.ceiling_vrms).collect();
        let cf = guards::apply_ceilings(&mut g, self.band, self.df_hz, &ceilings);
        let (_, limited) = self.apply_limits(&mut g);
        self.law_stats.calls += 1;
        self.law_stats.capped_drive_lines += caps.bound as u64;
        self.law_stats.repaired_lines += rep.lines_repaired as u64;
        self.law_misses = 0;
        self.guards_last = GuardSummary {
            repair: rep,
            capped: caps.bound,
            ceiling_bound: cf.bound,
            limited_lines: limited,
            peak_change_db: caps.peak_change_db,
        };
        let window = cap_window(self.cfg.protections.divergence_loops);
        for (h, f) in self.ls.cap_hist.iter_mut().zip(&caps.factors) {
            *h = ((*h << 1) | u32::from(*f != 1.0)) & window;
        }
        self.ls.caps = Some(caps);
        self.ls.law_drive = drive_u;
        self.ls.drive_full = g;
        self.ls.diagnostics = out.diagnostics;
        self.timing.guards_ms = t0.elapsed().as_secs_f64() * 1e3;
        Ok(())
    }

    // --- errors and judgement -----------------------------------------------------------

    /// Per-control errors of a control-space response at full level: in-band rms
    /// dB, worst line dB, and the per-line errors. `achievable` judges against what
    /// the plant can reach rather than the reference.
    fn errors_of(&self, resp: &HermitianStack, achievable: bool) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let n = self.n_r;
        let target = if achievable {
            &self.achievable_diag
        } else {
            &self.ref_diag
        };
        let mut lines = vec![f64::NAN; self.lines * n];
        let mut rms = vec![0.0; n];
        let mut worst = vec![0.0f64; n];
        let mut count = vec![0usize; n];
        for line in self.band.0..self.band.1 {
            for i in 0..n {
                let r = target[line * n + i];
                let m = resp.get(line, i, i).re;
                if r > 0.0 && m > 0.0 {
                    let e = 10.0 * (m / r).log10();
                    lines[line * n + i] = e;
                    rms[i] += e * e;
                    count[i] += 1;
                    if e.abs() > worst[i].abs() {
                        worst[i] = e;
                    }
                }
            }
        }
        for i in 0..n {
            rms[i] = if count[i] > 0 {
                (rms[i] / count[i] as f64).sqrt()
            } else {
                f64::NAN
            };
        }
        (rms, worst, lines)
    }

    /// The envelope on each control's PSD (D11). Returns `false` when it aborted.
    fn check_envelope(&mut self, resp: &HermitianStack, dof: f64) -> bool {
        if dof < MIN_ENVELOPE_DOF {
            return true;
        }
        let n = self.n_r;
        let tol = self.cfg.reference.tolerances.clone();
        for i in 0..n {
            let (mut abort, mut alarm) = (0usize, 0usize);
            let mut worst = 0.0f64;
            for line in self.band.0..self.band.1 {
                let k = line * n + i;
                let m = resp.get(line, i, i).re;
                let out = |hi: f64, lo: f64| (!hi.is_nan() && m > hi) || (!lo.is_nan() && m < lo);
                if out(tol.abort_upper[k], tol.abort_lower[k]) {
                    abort += 1;
                }
                if out(tol.alarm_upper[k], tol.alarm_lower[k]) {
                    alarm += 1;
                    let r = self.ref_diag[k];
                    if r > 0.0 && m > 0.0 {
                        let e = 10.0 * (m / r).log10();
                        if e.abs() > worst.abs() {
                            worst = e;
                        }
                    }
                }
            }
            if abort >= self.cfg.protections.envelope_abort_lines {
                self.abort(MimoTripKind::Envelope {
                    control: i,
                    abort_lines: abort,
                    worst_db: worst,
                });
                return false;
            }
            if alarm > 0 {
                self.log.push(MimoEventKind::EnvelopeAlarm {
                    control: i,
                    lines: alarm,
                    worst_db: worst,
                });
            }
        }
        true
    }

    /// The core's own divergence judgement (design 8.4): a control whose error
    /// against what the plant can reach stays at or above the floor without
    /// improving for `divergence_loops` judged loops. Returns `false` when it
    /// aborted.
    fn check_divergence(&mut self, err_ach: &[f64]) -> bool {
        let p = self.cfg.protections;
        for (i, &e) in err_ach.iter().enumerate() {
            if !e.is_finite() || e < p.divergence_floor_db {
                self.ls.div_streak[i] = 0;
                self.ls.div_best[i] = self.ls.div_best[i].min(e.max(0.0));
                continue;
            }
            if e < self.ls.div_best[i] - 0.1 {
                self.ls.div_best[i] = e;
                self.ls.div_streak[i] = 0;
            } else {
                self.ls.div_streak[i] += 1;
            }
            if self.ls.div_streak[i] >= p.divergence_loops {
                self.abort(MimoTripKind::Divergence {
                    control: i,
                    consecutive_loops: self.ls.div_streak[i],
                    error_db: e,
                });
                return false;
            }
        }
        true
    }

    /// The core's other divergence count (the single-output rule, per drive): a
    /// drive line the change cap has held back in `divergence_loops` of the last
    /// `2 * divergence_loops` loops is divergent whatever the law says;
    /// `envelope_abort_lines` of them on one drive abort the run. A law that swings
    /// the drive faster than the caps allow is caught here even when the averaged
    /// estimate looks right. Returns `false` when it aborted.
    fn check_cap_divergence(&mut self) -> bool {
        let p = self.cfg.protections;
        let n = self.n_d;
        for k in 0..n {
            let lines = (self.band.0..self.band.1)
                .filter(|&l| self.ls.cap_hist[l * n + k].count_ones() >= p.divergence_loops)
                .count();
            if lines >= p.envelope_abort_lines {
                self.abort(MimoTripKind::CapDivergence {
                    drive: k,
                    lines,
                    consecutive_loops: p.divergence_loops,
                });
                return false;
            }
        }
        true
    }

    // --- the loop -----------------------------------------------------------------------

    /// One closed-loop step. Returns `true` while the run goes on.
    pub fn loop_tick(&mut self) -> Result<bool, MimoError> {
        if !matches!(self.state, MimoState::Engaged | MimoState::FullLevel) {
            return Err(MimoError::BadState(format!(
                "the loop runs while engaged; the run is {:?}",
                self.state
            )));
        }
        if let Some(reason) = self.law_stop_due.take() {
            self.stop_for_law(reason);
            if !matches!(self.state, MimoState::Engaged | MimoState::FullLevel) {
                return Ok(false);
            }
        }
        // 1. Play the drive in force at the scheduled level; the ramp walks it.
        let base_db = self.schedule_level_db;
        let factor = self.factor_of(&self.ls.drive_full.clone(), base_db)?;
        let peak_db = self
            .ramp
            .projected(self.loop_cfg.segment_seconds())
            .max(base_db);
        let lift = 10f64.powf((peak_db - base_db) / 20.0);
        let (seg, drives) = self.play(&factor, Some(base_db))?;
        let rms: Vec<f64> = Self::record_rms(&drives).iter().map(|r| r * lift).collect();
        self.ls.drive_rms = rms.clone();
        self.loop_count += 1;
        self.elapsed_at_level_s += self.loop_cfg.segment_seconds();
        if self.state == MimoState::FullLevel {
            self.full_level_loops += 1;
        }
        // 2. Protections on what was played.
        if !self.segment_protections(&seg, &rms) {
            return Ok(false);
        }
        let end_db = self.ramp.level_db();
        let moved = (end_db - base_db).abs() > 1e-9;
        if moved {
            if let Some(s) = self.synth.as_mut() {
                s.scale_carry(10f64.powf((end_db - base_db) / 20.0));
            }
            self.set_level_now(end_db);
        }
        if self.ramp.stop_requested().is_some() && !self.ramp.moving() {
            self.stop();
            return Ok(false);
        }
        // 3. Estimate; fold into the accumulated estimate at the level reached.
        let t0 = Instant::now();
        let mut raw = self.estimate(&seg, &drives);
        if moved {
            let k = 10f64.powf((end_db - base_db) / 10.0) / seg.drive_power_gain.max(1e-12);
            raw.scale(k);
        }
        if !self.limit_aborts(&raw, self.schedule_level_db) {
            return Ok(false);
        }
        if self.ls.settle > 0 {
            self.ls.settle -= 1;
            self.ls.est = None;
            self.ls.est_loops = 0;
            self.timing.estimate_ms = t0.elapsed().as_secs_f64() * 1e3;
            self.after_loop();
            return Ok(self.state.is_running());
        }
        let mem = self.cfg.estimation.memory_loops;
        let alpha = (2.0 / (mem + 1.0)).max(1.0 / f64::from(self.ls.est_loops + 1));
        let est = match self.ls.est.take() {
            Some(mut e) => {
                e.blend(&raw, alpha);
                e
            }
            None => raw.clone(),
        };
        self.ls.est_loops += 1;
        let dof_frames = (est.frames as f64).min(2.0 / alpha * raw.frames as f64);
        let dof = 2.0 * dof_frames;
        // 4. The control space at full level.
        let mut resp = self
            .to_control(&est.g_cc)
            .map_err(|e| MimoError::Law(e.to_string()))?;
        let to_full = 10f64.powf(-self.schedule_level_db / 10.0);
        let _ = resp.scale_lines(&vec![to_full; self.lines]);
        self.ls.est = Some(est.clone());
        self.timing.estimate_ms = t0.elapsed().as_secs_f64() * 1e3;
        let (err, worst, lines) = self.errors_of(&resp, false);
        self.ls.error_db = err;
        self.ls.worst_db = worst;
        self.ls.error_lines = lines;
        self.ls.response_full = Some(resp.clone());
        if moved {
            // Not judged and not steering the law while the level moves.
            self.after_loop();
            return Ok(self.state.is_running());
        }
        // 5. Run-level judgement.
        if !self.check_envelope(&resp, dof) {
            return Ok(false);
        }
        // The core's divergence judges this loop's own estimate, not the averaged
        // one: an average across loops hides a drive that swings loop to loop.
        let mut raw_resp = self
            .to_control(&raw.g_cc)
            .map_err(|e| MimoError::Law(e.to_string()))?;
        let _ = raw_resp.scale_lines(&vec![to_full; self.lines]);
        let (err_ach, _, _) = self.errors_of(&raw_resp, self.n_r > self.n_u);
        if !self.check_divergence(&err_ach) {
            return Ok(false);
        }
        // 6. The FRF the law sees.
        let g_cd_full = {
            let mut c = est.g_cd(self.n_c);
            for v in c.data_mut() {
                *v *= to_full;
            }
            c
        };
        if let FrfMode::Update { blend } = self.cfg.frf_mode {
            let mut g_dd = est.g_dd.clone();
            let _ = g_dd.scale_lines(&vec![to_full; self.lines]);
            if let Ok(h) = pool::h1_frf(
                &g_cd_full,
                &g_dd,
                Regularization::Truncated { rcond: 1e-3 },
                self.band,
            ) {
                if let (Ok(h_law), Some(base)) = (self.frf_to_law(&h), self.frf_law_ident.clone()) {
                    let mut m = base;
                    for (a, b) in m.data_mut().iter_mut().zip(h_law.data()) {
                        *a = *a * (1.0 - blend) + *b * blend;
                    }
                    self.frf_law = Some(m);
                }
            }
        }
        // 7. The law.
        if !self.law_retired {
            let csd = if self.law_needs.contains(&MimoInput::ResponseDriveCsd) {
                self.csd_to_law(&g_cd_full).ok()
            } else {
                None
            };
            let idx = self.loop_count;
            match self.call_law(idx, &resp, dof, csd.as_ref()) {
                Ok(out) => match self.accept(out) {
                    Ok(()) => {
                        if !self.check_cap_divergence() {
                            return Ok(false);
                        }
                    }
                    Err(m) => self.law_missed(m),
                },
                Err(m) => self.law_missed(m),
            }
        }
        self.after_loop();
        Ok(self.state.is_running())
    }

    fn after_loop(&mut self) {
        if let Some(r) = self.records.as_mut() {
            let summary = crate::records::LoopSummary {
                loop_index: self.loop_count,
                level_db: self.schedule_level_db,
                state: self.state,
                error_db: self.ls.error_db.clone(),
                drive_rms: self.ls.drive_rms.clone(),
                guards: self.guards_last.clone(),
                timing: self.timing,
                law_misses: self.law_misses,
            };
            if let Err(e) = r.write_loop(
                &summary,
                self.ls.response_full.as_ref(),
                &self.ls.drive_full,
                &self.ls.error_lines,
                self.state == MimoState::FullLevel,
                self.loop_cfg.segment_seconds(),
            ) {
                self.faults.push(format!("loop record: {e}"));
            }
        }
        self.maybe_advance_auto();
        if self.state == MimoState::FullLevel {
            if let Some(d) = self.cfg.full_level_duration_s {
                if self.elapsed_at_level_s >= d && self.ramp.stop_requested().is_none() {
                    let _ = self.request_stop();
                }
            }
        }
    }

    /// The whole run without an operator: start, noise floor, identification, the
    /// gate, the prediction, then loops until the run ends or `max_loops` pass.
    /// Gates and predictions that need an override hold the run in their state and
    /// return.
    pub fn run_auto(&mut self, max_loops: usize) -> Result<MimoState, MimoError> {
        if self.state == MimoState::Idle {
            self.start()?;
        }
        if self.state == MimoState::NoiseFloor {
            self.run_noise_floor()?;
        }
        if self.state == MimoState::SysId {
            self.run_identification()?;
        }
        if self.state == MimoState::Gate {
            self.proceed()?;
        }
        if self.state == MimoState::Predict {
            self.proceed()?;
        }
        let mut n = 0;
        while matches!(self.state, MimoState::Engaged | MimoState::FullLevel) && n < max_loops {
            self.loop_tick()?;
            n += 1;
        }
        Ok(self.state)
    }

    // --- status and report ---------------------------------------------------------------

    /// The live status.
    pub fn status(&self) -> MimoStatus {
        MimoStatus {
            state: self.state,
            level_db: self.schedule_level_db,
            target_level_db: self.level_target_db,
            loop_index: self.loop_count,
            error_db: self.ls.error_db.clone(),
            gate: self.gate.clone(),
            prediction: self.prediction.clone(),
            guards: self.guards_last.clone(),
            law: self.law_line.clone(),
            law_stats: self.law_stats,
            faults: self.faults.clone(),
            trip: self.trip.clone(),
            timing: self.timing,
            drive_rms: self.ls.drive_rms.clone(),
            overloads: self.overloads.iter().filter(|o| o.2).map(|o| o.0).collect(),
        }
    }

    /// The report data (design 11): the law line, the identification summary, the
    /// final error per control, the gates and the trips.
    pub fn report(&self) -> MimoReport {
        MimoReport {
            law: self.law_line.clone(),
            identification: self.ident.as_ref().map(|s| IdentSummary {
                drive_vrms: s.drive_vrms.clone(),
                level_db: s.level_db,
                frames: s.frames,
                steps: s.steps,
                setup_hash: s.setup_hash.clone(),
                loaded: self.ident_loaded,
            }),
            gate: self.gate.clone(),
            prediction: self.prediction.clone(),
            feasibility: self.feasibility.clone(),
            final_error_db: self.ls.error_db.clone(),
            final_worst_db: self.ls.worst_db.clone(),
            loops: self.loop_count,
            loops_at_full_level: self.full_level_loops,
            trip: self.trip.clone(),
            law_stats: self.law_stats,
            final_state: self.state,
            seed: self.cfg.seed,
            events: self.log.events().to_vec(),
        }
    }

    // --- access for the quantities and records modules -----------------------------------

    pub(crate) fn quantity_state(&self) -> crate::quantities::View<'_> {
        crate::quantities::View {
            lines: self.lines,
            df_hz: self.df_hz,
            band: self.band,
            n_r: self.n_r,
            n_d: self.n_d,
            level_db: self.schedule_level_db,
            reference: &self.reference,
            tolerances: &self.cfg.reference.tolerances,
            response_full: self.ls.response_full.as_ref(),
            error_lines: &self.ls.error_lines,
            drive_full: &self.ls.drive_full,
            frf: self.frf_law.as_ref(),
            coherence: &self.coherence,
            condition: &self.condition,
            caps: self.ls.caps.as_ref(),
            prediction: self.prediction.as_ref(),
            diagnostics: &self.ls.diagnostics,
        }
    }

    fn start_records(&mut self) -> Result<(), MimoError> {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let name = format!("mimo_random_{unix}_{:08x}", self.cfg.seed as u32);
        let n_ch = self
            .plant_loop
            .as_ref()
            .map(|p| p.n_input_channels())
            .unwrap_or(1);
        let rec = crate::records::RunRecorder::start(
            &self.data_root,
            &name,
            n_ch,
            self.loop_cfg.fs_hz,
            self.table.clone(),
            &self.settings_json(),
        )
        .map_err(MimoError::Io)?;
        let mut rec = rec;
        rec.set_snapshot_every_s(self.cfg.records.snapshot_every_s);
        self.log.push(MimoEventKind::Record {
            path: rec.run_dir().display().to_string(),
        });
        self.records = Some(rec);
        Ok(())
    }

    fn settings_json(&self) -> String {
        serde_json::json!({
            "test": "mimo_random",
            "controls": self.cfg.controls,
            "drives": self.cfg.drives.iter().map(|d| d.channel_index).collect::<Vec<_>>(),
            "limits": self.cfg.limits.iter().map(|l| l.channel_index).collect::<Vec<_>>(),
            "control_space_dofs": self.n_r,
            "law_drives": self.n_u,
            "lines": self.lines,
            "df_hz": self.df_hz,
            "band": [self.band.0, self.band.1],
            "law": self.cfg.law.id,
            "seed": self.cfg.seed,
            "setup_hash": self.setup_hash(),
        })
        .to_string()
    }

    fn finish_records(&mut self) {
        if let Some(mut r) = self.records.take() {
            let law = self.law_line.clone();
            let report = self.report();
            if let Err(e) = r.finish(law.as_ref(), &report) {
                self.faults.push(format!("records: {e}"));
            }
        }
    }

    /// The run's record folder, while records are being written.
    pub fn records_dir(&self) -> Option<PathBuf> {
        self.records.as_ref().map(|r| r.run_dir().to_path_buf())
    }
}

/// Fold `e` into an average weighted by frames.
fn merge(acc: Option<Estimate>, e: Estimate) -> Estimate {
    match acc {
        None => e,
        Some(mut a) => {
            let w = e.frames as f64 / (a.frames + e.frames).max(1) as f64;
            a.blend(&e, w);
            a
        }
    }
}

#[allow(dead_code)]
fn _c64_used(_: C64) {}

/// The bits of the change-cap history the divergence count looks at: the last
/// `2 * loops` loops (at most 32).
fn cap_window(loops: u32) -> u32 {
    let bits = (2 * loops).min(32);
    if bits >= 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    }
}
