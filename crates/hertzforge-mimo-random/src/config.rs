//! The test configuration of a multiple-output Random run.
//!
//! Everything the service needs is here and nothing is read from a file: the
//! reference arrives in memory as the spectral matrix, tolerances, band and line
//! spacing of [`hertzforge_sdm::law::MimoSetup`]. [`MimoConfig::validate`] returns
//! the first problem, in words that say what to change.

use crate::pool;
use hertzforge_random::service::config::{LawGuards, LevelSchedule};
use hertzforge_random_laws::LawChoice;
use hertzforge_sdm::law::Tolerances;
use hertzforge_sdm::{HermitianStack, MatrixStack};
use hertzforge_shaker_db::{MovingMass, ShakerRecord};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Default per-loop rise cap on each drive's PSD, dB (design 8.3).
pub const DEFAULT_CAP_UP_DB: f64 = 3.5;
/// Default per-loop fall cap on each drive's PSD, dB.
pub const DEFAULT_CAP_DOWN_DB: f64 = 6.0;
/// Default condition-number threshold per line (design 8.4).
pub const DEFAULT_CONDITION_THRESHOLD: f64 = 1_000.0;
/// Default multiple-coherence threshold per control (mean over the band).
pub const DEFAULT_COHERENCE_THRESHOLD: f64 = 0.5;
/// Default Welch frames per control loop.
pub const DEFAULT_FRAMES_PER_LOOP: usize = 64;
/// The identification level relative to the reference, dB (Q9 and Q34).
pub const DEFAULT_IDENT_BELOW_REFERENCE_DB: f64 = 20.0;
/// Default full spectral-matrix snapshot cadence, seconds (Q31).
pub const DEFAULT_SNAPSHOT_EVERY_S: f64 = 30.0;
/// Default lines outside the abort lines, on one control, that abort the run.
pub const DEFAULT_ENVELOPE_ABORT_LINES: usize = 10;
/// Largest per-loop rise cap the operator may set, dB.
pub const MAX_CAP_UP_DB: f64 = 6.0;

/// A configuration problem, with what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError(msg.into()))
}

/// One output: the channel it drives, its voltage limits, and the shaker behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveSpec {
    /// The drive row's channel index in the channel table.
    pub channel_index: u32,
    /// The output's voltage ceiling, V rms over the whole grid. The core scales
    /// this drive's PSD down (a diagonal congruence) so the drive never asks for
    /// more.
    pub ceiling_vrms: f64,
    /// The output's peak voltage: the synthesized stream is clipped here.
    pub peak_v: f64,
    /// A drive whose realized rms reaches this aborts the run. Defaults to the
    /// ceiling plus 1 dB when absent.
    pub abort_vrms: Option<f64>,
    /// The highest rms the identification may use on this output.
    pub pretest_max_vrms: f64,
    /// The shaker on this output, for feasibility. Absent: not evaluated.
    pub shaker: Option<ShakerRecord>,
    /// This shaker's own moving mass (armature, adapter, fixturing). The test
    /// article's mass is split across the shakers ([`MimoConfig::article_mass_lb`]).
    pub moving_mass: MovingMass,
}

impl DriveSpec {
    /// A drive on `channel_index` with a ceiling of `ceiling_vrms`, a peak of three
    /// times that, and identification allowed up to a tenth of the ceiling.
    pub fn new(channel_index: u32, ceiling_vrms: f64) -> Self {
        DriveSpec {
            channel_index,
            ceiling_vrms,
            peak_v: 3.0 * ceiling_vrms,
            abort_vrms: None,
            pretest_max_vrms: 0.1 * ceiling_vrms,
            shaker: None,
            moving_mass: MovingMass::default(),
        }
    }

    /// The rms at which this drive aborts the run.
    pub fn abort_level_vrms(&self) -> f64 {
        self.abort_vrms
            .unwrap_or(self.ceiling_vrms * 10f64.powf(1.0 / 20.0))
    }
}

/// A limit channel: a response the core keeps under a ceiling by predicted
/// limiting (uniform scaling of the line's drive, design 8.2) and by a measured
/// abort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitSpec {
    pub channel_index: u32,
    /// The ceiling on this channel's PSD at full level, one value per line of the
    /// grid, or one value for every line. Units squared per hertz.
    pub ceiling_psd: Vec<f64>,
    /// A measured PSD this many dB above the ceiling on any in-band line aborts the
    /// run.
    pub abort_margin_db: f64,
}

impl LimitSpec {
    /// The ceiling on line `k`.
    pub fn ceiling(&self, k: usize) -> f64 {
        if self.ceiling_psd.len() == 1 {
            self.ceiling_psd[0]
        } else {
            self.ceiling_psd.get(k).copied().unwrap_or(f64::INFINITY)
        }
    }
}

/// Which space the reference is written in (design 5.2).
#[derive(Debug, Clone, PartialEq)]
pub enum ControlSpace {
    /// One row and column per control channel.
    Physical,
    /// `n_v` virtual degrees of freedom: `G_vv = T_r G_cc T_r^H`. The
    /// transformation is `n_v` by `n_c` on one line (applied to every line) or one
    /// per line, and must have full row rank.
    Virtual { response: MatrixStack },
}

/// The FRF the law sees during the run (design 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum FrfMode {
    /// Keep the identification FRF (the default).
    Hold,
    /// Re-estimate it every loop from the played drive and the measured
    /// response, blended with the identification estimate: `H = (1 - blend)
    /// H_id + blend H_loop`.
    Update { blend: f64 },
}

/// The per-loop estimate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Estimation {
    /// Welch frames (Hann, 50% overlap) per loop. The loop's length follows.
    pub frames_per_loop: usize,
    /// Exponential memory across loops, as an equivalent count of loops: the
    /// estimate is a plain mean until it holds this many, then an exponential
    /// average with `alpha = 2 / (memory_loops + 1)`. 1 means no memory.
    pub memory_loops: f64,
}

impl Default for Estimation {
    fn default() -> Self {
        Estimation {
            frames_per_loop: DEFAULT_FRAMES_PER_LOOP,
            memory_loops: 4.0,
        }
    }
}

/// System identification (design 2 step 4, Q9 and Q34).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Identification {
    /// The rms each drive starts the level search at, volts.
    pub start_vrms: f64,
    /// The search steps the drive until the control response sits this many dB
    /// below the reference (rms over the band, averaged over the controls).
    pub below_reference_db: f64,
    /// How close to that level is close enough, dB.
    pub tolerance_db: f64,
    /// The level search gives up after this many steps.
    pub max_steps: usize,
    /// Welch frames in the identification record. At least twice the drive count
    /// (a K-frame drive SDM has rank at most K); ten times is better.
    pub frames: usize,
    /// Welch frames in the noise-floor record (drives off).
    pub noise_frames: usize,
}

impl Identification {
    /// Defaults for `drives` outputs.
    pub fn for_drives(drives: usize) -> Self {
        Identification {
            start_vrms: 0.01,
            below_reference_db: DEFAULT_IDENT_BELOW_REFERENCE_DB,
            tolerance_db: 1.0,
            max_steps: 8,
            frames: (10 * drives).max(64),
            noise_frames: 32,
        }
    }
}

/// The identification gates (design 2 step 5, 8.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gates {
    /// Each control's multiple coherence, averaged over the band, must reach this.
    pub coherence_threshold: f64,
    /// Each line's FRF condition number must stay under this.
    pub condition_threshold: f64,
}

impl Default for Gates {
    fn default() -> Self {
        Gates {
            coherence_threshold: DEFAULT_COHERENCE_THRESHOLD,
            condition_threshold: DEFAULT_CONDITION_THRESHOLD,
        }
    }
}

/// The core's per-loop change cap on each drive's PSD (design 8.1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Caps {
    pub up_db: f64,
    pub down_db: f64,
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            up_db: DEFAULT_CAP_UP_DB,
            down_db: DEFAULT_CAP_DOWN_DB,
        }
    }
}

/// Run-level protections other than the drive and limit settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Protections {
    /// Lines outside a control's abort lines (in band, one control) that abort
    /// the run.
    pub envelope_abort_lines: usize,
    /// Loops in a row with a growing in-band error on one control, at or above
    /// `divergence_floor_db`, that abort the run.
    pub divergence_loops: u32,
    /// The in-band rms error, dB, below which a growing error is not divergence.
    pub divergence_floor_db: f64,
    /// An input sample at or above this fraction of its range latches the
    /// overload indicator.
    pub overload_fraction: f64,
    /// Loops after a restart (engage, a resume) that are not judged and do not
    /// steer the law.
    pub settle_loops: u32,
}

impl Default for Protections {
    fn default() -> Self {
        Protections {
            envelope_abort_lines: DEFAULT_ENVELOPE_ABORT_LINES,
            divergence_loops: 5,
            divergence_floor_db: 3.0,
            overload_fraction: 0.9,
            settle_loops: 1,
        }
    }
}

/// What the run records (design 11, Q38).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Records {
    /// Record the run: the raw stream, the identification set, per-loop diagonals
    /// and summaries, spectral-matrix snapshots and the law record. On by default.
    pub enabled: bool,
    /// Full spectral-matrix snapshot cadence, seconds, plus the first and last loop
    /// at full level.
    pub snapshot_every_s: f64,
}

impl Default for Records {
    fn default() -> Self {
        Records {
            enabled: true,
            snapshot_every_s: DEFAULT_SNAPSHOT_EVERY_S,
        }
    }
}

/// The reference: the spectral matrix at full level on the loop's line grid, in
/// the control space, with its tolerances and band (the material of
/// [`hertzforge_sdm::law::MimoSetup`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceSpec {
    pub sdm: HermitianStack,
    pub tolerances: Tolerances,
    /// The half-open controlled band `[lo, hi)`, in lines.
    pub band: (usize, usize),
    /// Line spacing, Hz; must equal the loop's.
    pub df_hz: f64,
}

/// Everything a multiple-output Random run needs.
#[derive(Debug, Clone)]
pub struct MimoConfig {
    /// Control channels in reference order (the rows of a physical reference).
    pub controls: Vec<u32>,
    /// One per output, in the order of the drive rows the plant loop plays.
    pub drives: Vec<DriveSpec>,
    pub limits: Vec<LimitSpec>,
    pub control_space: ControlSpace,
    /// `T_d`, `n_d` by `n_u`, one line or one per line, full column rank: the law
    /// commands `n_u` virtual drives and the core expands `G_dd = T_d G_uu T_d^H`.
    pub drive_transform: Option<MatrixStack>,
    pub reference: ReferenceSpec,
    /// The law and its parameter values (checked against the law's manifest).
    pub law: LawChoice,
    pub frf_mode: FrfMode,
    pub estimation: Estimation,
    pub identification: Identification,
    pub gates: Gates,
    pub caps: Caps,
    /// The law call's time budget (a fraction of the loop) and the misses in a
    /// row that stop the run. `max_change_db` is not used here: [`Caps`] is.
    pub law_guards: LawGuards,
    pub schedule: LevelSchedule,
    /// Seconds a commanded level change takes (0: a step between loops).
    pub level_ramp_s: f64,
    /// Seconds a controlled stop takes to ramp the drive down.
    pub shutdown_ramp_s: f64,
    /// Seconds at full level before the run stops by itself (`None`: until stopped).
    pub full_level_duration_s: Option<f64>,
    pub protections: Protections,
    /// The test article's mass, split equally across the shakers for feasibility.
    pub article_mass_lb: f64,
    /// Crest factor feasibility and the peak budget assume.
    pub crest_factor: f64,
    pub records: Records,
    /// The run's random seed (the drive's phases); recorded with the run.
    pub seed: u64,
}

impl MimoConfig {
    /// A configuration with every default, for `controls` and `drives` and a
    /// reference. The law is the default law with its default parameters.
    pub fn new(controls: Vec<u32>, drives: Vec<DriveSpec>, reference: ReferenceSpec) -> Self {
        let n_d = drives.len();
        MimoConfig {
            controls,
            drives,
            limits: Vec::new(),
            control_space: ControlSpace::Physical,
            drive_transform: None,
            reference,
            law: hertzforge_random_laws::default_mimo_choice(),
            frf_mode: FrfMode::Hold,
            estimation: Estimation::default(),
            identification: Identification::for_drives(n_d),
            gates: Gates::default(),
            caps: Caps::default(),
            law_guards: LawGuards::default(),
            schedule: LevelSchedule::auto_default(),
            level_ramp_s: 0.0,
            shutdown_ramp_s: 2.0,
            full_level_duration_s: None,
            protections: Protections::default(),
            article_mass_lb: 0.0,
            crest_factor: hertzforge_shaker_db::DEFAULT_CREST_FACTOR,
            records: Records::default(),
            seed: 0x4D49_4D4F_5241_4E44,
        }
    }

    /// Control channels.
    pub fn n_controls(&self) -> usize {
        self.controls.len()
    }

    /// Physical outputs.
    pub fn n_drives(&self) -> usize {
        self.drives.len()
    }

    /// The control space's dimension: controls, or virtual degrees of freedom.
    pub fn n_reference(&self) -> usize {
        match &self.control_space {
            ControlSpace::Physical => self.controls.len(),
            ControlSpace::Virtual { response } => response.rows(),
        }
    }

    /// The drives the law commands: physical outputs, or virtual drives.
    pub fn n_law_drives(&self) -> usize {
        self.drive_transform
            .as_ref()
            .map(|t| t.cols())
            .unwrap_or(self.drives.len())
    }

    /// The first problem with this configuration on a grid of `lines` lines at
    /// `df_hz` per line, or `Ok`.
    pub fn validate(&self, lines: usize, df_hz: f64) -> Result<(), ConfigError> {
        let (n_c, n_d) = (self.controls.len(), self.drives.len());
        if n_c == 0 {
            return err("no control channels: list at least one control channel");
        }
        if n_d == 0 {
            return err("no drives: list at least one drive output");
        }
        if n_c > 1024 || n_d > 1024 {
            return err(format!(
                "{n_c} controls and {n_d} drives: at most 1,024 of each is supported; split the test"
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (what, idx) in self
            .controls
            .iter()
            .map(|&c| ("control", c))
            .chain(self.drives.iter().map(|d| ("drive", d.channel_index)))
            .chain(self.limits.iter().map(|l| ("limit", l.channel_index)))
        {
            if !seen.insert(idx) {
                return err(format!(
                    "channel {idx} is listed twice (once as a {what}); give each channel one role"
                ));
            }
        }
        for (k, d) in self.drives.iter().enumerate() {
            let ok = |v: f64| v.is_finite() && v > 0.0;
            if !ok(d.ceiling_vrms) || !ok(d.peak_v) || !ok(d.pretest_max_vrms) {
                return err(format!(
                    "drive {k} (channel {}): the ceiling, peak and identification limit must be \
                     positive voltages; set all three",
                    d.channel_index
                ));
            }
            if d.peak_v < d.ceiling_vrms {
                return err(format!(
                    "drive {k} (channel {}): the peak {} V is below the rms ceiling {} V; set the \
                     peak at or above the ceiling (three times it is usual)",
                    d.channel_index, d.peak_v, d.ceiling_vrms
                ));
            }
            if d.pretest_max_vrms > d.ceiling_vrms {
                return err(format!(
                    "drive {k} (channel {}): the identification limit {} V rms is above the \
                     ceiling {} V rms; lower it to the ceiling or below",
                    d.channel_index, d.pretest_max_vrms, d.ceiling_vrms
                ));
            }
            if let Some(a) = d.abort_vrms {
                if !(a.is_finite() && a > d.ceiling_vrms) {
                    return err(format!(
                        "drive {k} (channel {}): the abort level {a} V rms must be above the \
                         ceiling {} V rms; raise it or leave it unset",
                        d.channel_index, d.ceiling_vrms
                    ));
                }
            }
        }
        for l in &self.limits {
            let n = l.ceiling_psd.len();
            if !(n == 1 || n == lines) {
                return err(format!(
                    "limit channel {}: {n} ceiling values; give one, or one per line ({lines})",
                    l.channel_index
                ));
            }
            if l.ceiling_psd.iter().any(|v| !(v.is_finite() && *v > 0.0)) {
                return err(format!(
                    "limit channel {}: every ceiling value must be a positive number",
                    l.channel_index
                ));
            }
            if !(l.abort_margin_db.is_finite() && l.abort_margin_db >= 0.0) {
                return err(format!(
                    "limit channel {}: the abort margin must be 0 dB or more",
                    l.channel_index
                ));
            }
        }
        // The reference.
        let n_r = self.n_reference();
        let r = &self.reference;
        if r.sdm.lines() != lines || r.sdm.n() != n_r {
            return err(format!(
                "the reference is {} lines by {} channels; this test needs {lines} lines (the \
                 loop's grid) by {n_r}; rebuild the reference on this grid",
                r.sdm.lines(),
                r.sdm.n()
            ));
        }
        if !(r.df_hz.is_finite() && (r.df_hz - df_hz).abs() <= 1e-9 * df_hz.abs().max(1.0)) {
            return err(format!(
                "the reference's line spacing is {} Hz and the loop's is {df_hz} Hz; rebuild the \
                 reference on the loop's grid",
                r.df_hz
            ));
        }
        let (lo, hi) = r.band;
        if lo == 0 || lo >= hi || hi >= lines {
            return err(format!(
                "the band [{lo}, {hi}) must lie strictly inside the grid (lines 1 to {}); move \
                 its edges in",
                lines - 2
            ));
        }
        if !r.sdm.is_finite() {
            return err("the reference holds a value that is not a number; fix the reference");
        }
        let probe = hertzforge_sdm::law::MimoSetup {
            controls: n_r,
            drives: self.n_law_drives(),
            lines,
            df_hz,
            band: r.band,
            reference: r.sdm.clone(),
            tolerances: r.tolerances.clone(),
        };
        if let Err(e) = probe.check() {
            return err(format!("the reference does not fit the test: {e}"));
        }
        for line in lo..hi {
            if r.sdm.diagonal(line).iter().any(|&p| p < 0.0) {
                return err(format!(
                    "the reference has a negative PSD on line {line}; fix the reference"
                ));
            }
        }
        // Transformations.
        if let ControlSpace::Virtual { response } = &self.control_space {
            let (tl, rows, cols) = response.dims();
            if cols != n_c || !(tl == 1 || tl == lines) || rows == 0 {
                return err(format!(
                    "the response transformation is {rows} x {cols} on {tl} lines; it must be \
                     (virtual DOFs) x {n_c} (the controls) on one line or {lines}"
                ));
            }
            if rows > n_c {
                return err(format!(
                    "the response transformation has {rows} virtual DOFs from {n_c} controls; it \
                     needs full row rank, so at most {n_c} DOFs"
                ));
            }
            let rank = pool::min_rank(response).map_err(|e| ConfigError(e.to_string()))?;
            if rank < rows {
                return err(format!(
                    "the response transformation has rank {rank}, it needs full row rank {rows}; \
                     check its rows for duplicated or dependent DOFs"
                ));
            }
        }
        if let Some(td) = &self.drive_transform {
            let (tl, rows, cols) = td.dims();
            if rows != n_d || !(tl == 1 || tl == lines) || cols == 0 {
                return err(format!(
                    "the drive transformation is {rows} x {cols} on {tl} lines; it must be {n_d} \
                     (the drives) x (virtual drives) on one line or {lines}"
                ));
            }
            if cols > n_d {
                return err(format!(
                    "the drive transformation has {cols} virtual drives for {n_d} outputs; it \
                     needs full column rank, so at most {n_d}"
                ));
            }
            let rank = pool::min_rank(td).map_err(|e| ConfigError(e.to_string()))?;
            if rank < cols {
                return err(format!(
                    "the drive transformation has rank {rank}, it needs full column rank {cols}; \
                     check its columns for dependent virtual drives"
                ));
            }
        }
        // Settings.
        if let FrfMode::Update { blend } = self.frf_mode {
            if !(blend > 0.0 && blend <= 1.0) {
                return err(format!(
                    "the FRF update blend {blend} must be above 0 and at most 1"
                ));
            }
        }
        if self.estimation.frames_per_loop < 2 {
            return err("frames per loop must be at least 2");
        }
        if !(self.estimation.memory_loops.is_finite() && self.estimation.memory_loops >= 1.0) {
            return err("the estimate's memory must be at least 1 loop");
        }
        let id = &self.identification;
        if id.frames < 2 * n_d {
            return err(format!(
                "the identification record of {} frames is too short for {n_d} drives; use at \
                 least {} (ten times the drives is better)",
                id.frames,
                2 * n_d
            ));
        }
        if id.noise_frames < 2 {
            return err("the noise-floor record needs at least 2 frames");
        }
        let positive = id.start_vrms.is_finite()
            && id.start_vrms > 0.0
            && id.below_reference_db.is_finite()
            && id.below_reference_db >= 0.0
            && id.tolerance_db.is_finite()
            && id.tolerance_db > 0.0
            && id.max_steps > 0;
        if !positive {
            return err(
                "the identification start level, offset below the reference, tolerance and step \
                 count must all be positive",
            );
        }
        for d in &self.drives {
            if id.start_vrms > d.pretest_max_vrms {
                return err(format!(
                    "the identification starts at {} V rms, above drive channel {}'s \
                     identification limit {} V rms; lower the start",
                    id.start_vrms, d.channel_index, d.pretest_max_vrms
                ));
            }
        }
        let g = &self.gates;
        if !(g.coherence_threshold > 0.0 && g.coherence_threshold < 1.0) {
            return err("the coherence threshold must be between 0 and 1");
        }
        if !(g.condition_threshold.is_finite() && g.condition_threshold >= 1.0) {
            return err("the condition-number threshold must be 1 or more");
        }
        let c = &self.caps;
        if !(c.up_db > 0.0 && c.up_db <= MAX_CAP_UP_DB && c.down_db > 0.0 && c.down_db <= 20.0) {
            return err(format!(
                "the per-loop caps must be above 0 dB, at most {MAX_CAP_UP_DB} dB up and 20 dB \
                 down"
            ));
        }
        let lg = &self.law_guards;
        if !(lg.budget_fraction > 0.0 && lg.budget_fraction <= 1.0) || lg.max_misses == 0 {
            return err(
                "the law budget must be a fraction of the loop above 0 and at most 1, and the \
                 misses before a stop at least 1",
            );
        }
        let s = &self.schedule;
        if s.table_db.is_empty() || s.table_db.windows(2).any(|w| w[1] < w[0]) {
            return err("the level table must be non-empty and non-decreasing");
        }
        if s.table_db.iter().any(|&l| !(l.is_finite() && l <= 0.0)) || s.start_db > 0.0 {
            return err("every level must be at or below full level (0 dB)");
        }
        if (s.table_db[s.table_db.len() - 1]).abs() > 1e-9 {
            return err("the level table must end at full level (0 dB)");
        }
        if !(self.level_ramp_s.is_finite()
            && self.level_ramp_s >= 0.0
            && self.shutdown_ramp_s.is_finite()
            && self.shutdown_ramp_s >= 0.0)
        {
            return err("ramp times must be 0 s or more");
        }
        if let Some(d) = self.full_level_duration_s {
            if !(d.is_finite() && d >= 0.0) {
                return err("the full-level duration must be 0 s or more");
            }
        }
        let p = &self.protections;
        if p.envelope_abort_lines == 0 || p.divergence_loops == 0 {
            return err("the envelope abort lines and divergence loops must be at least 1");
        }
        if !(p.overload_fraction > 0.0 && p.overload_fraction <= 1.0) {
            return err("the overload fraction must be above 0 and at most 1");
        }
        if !(self.crest_factor.is_finite() && self.crest_factor >= 1.0) {
            return err("the crest factor must be 1 or more");
        }
        if self.records.enabled
            && !(self.records.snapshot_every_s.is_finite() && self.records.snapshot_every_s > 0.0)
        {
            return err("the snapshot cadence must be a positive number of seconds");
        }
        Ok(())
    }
}
