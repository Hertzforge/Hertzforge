//! Narrowbands: what a band states, whether it is added random, its
//! increment as an `n` by `n` matrix with edge weights, folding a dwelling
//! band into the floor's reference, the band's estimate in its own moving
//! frame, and the band laws behind one trait (design D10; theory 5.1 to 5.4;
//! Candidate Laws section 2).
//!
//! A band's reference at the controls is `G_F + w_b Delta_b` on each line
//! (5.1), `w_b` the share of the line's bin the band covers
//! ([`hertzforge_mixed::band::band_weights`]). `raise` states a dB per
//! control (U1): `Delta_ij = (sqrt(L_i L_j) - 1) G_F,ij`, which is added
//! random only when every control is raised by the same ratio or the floor is
//! incoherent; `added` states a density per control and one coherence (U2):
//! `Delta_ii = a_i`, `Delta_ij = gamma sqrt(a_i a_j)`, always added random.
//! A band that is not added random is refused `band_not_additive`.

use crate::config::{MimoMixedConfigError, MimoNarrowbandPart, MimoPart, PerControl};
use crate::mat::{CMat, Z_REGULARIZATION};
use hertzforge_mixed::band::{
    band_weights, BandServo, DEFAULT_BAND_SERVO_CAP_DB, DEFAULT_BAND_SERVO_GAIN,
};
use hertzforge_mixed::{BandWidth, LawPick, PartSchedule, PartWindow};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a band adds to the floor, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BandStatement {
    /// The floor raised by `db` across the band (U1): coherence and phase are
    /// the floor's. One dB for every control or one per control.
    Raise {
        /// dB over the floor, zero or more (at least one control raised).
        db: PerControl<f64>,
    },
    /// A density added per control with one coherence between every pair
    /// (U2), phase zero.
    Added {
        /// Added density per control at full level, EU^2/Hz.
        level_g2_hz: PerControl<f64>,
        /// Coherence between every pair of controls, 0 to 1. Default 0.
        #[serde(default)]
        coherence: f64,
    },
    /// A full matrix (U3). Reserved: refused `band_matrix_not_built`.
    Matrix {
        /// The matrix as written.
        #[serde(default)]
        values: Value,
    },
}

/// The band's increment `Delta` over a floor line `g_f` (`n` by `n`), at
/// full level. A `matrix` statement gives zeros (it is refused at
/// validation).
pub fn increment(statement: &BandStatement, g_f: &CMat) -> CMat {
    let n = g_f.rows();
    let mut d = CMat::zeros(n, n);
    match statement {
        BandStatement::Raise { db } => {
            let l: Vec<f64> = (0..n)
                .map(|i| 10f64.powf(db.get(i).copied().unwrap_or(0.0) / 10.0))
                .collect();
            for i in 0..n {
                for j in 0..n {
                    d.set(i, j, g_f.get(i, j) * ((l[i] * l[j]).sqrt() - 1.0));
                }
            }
        }
        BandStatement::Added {
            level_g2_hz,
            coherence,
        } => {
            let a: Vec<f64> = (0..n)
                .map(|i| level_g2_hz.get(i).copied().unwrap_or(0.0).max(0.0))
                .collect();
            for i in 0..n {
                for j in 0..n {
                    let v = if i == j {
                        a[i]
                    } else {
                        coherence * (a[i] * a[j]).sqrt()
                    };
                    d.set(i, j, C64::new(v, 0.0));
                }
            }
        }
        BandStatement::Matrix { .. } => {}
    }
    d
}

/// The band's increment on every line of the floor's reference stack.
pub fn increment_stack(statement: &BandStatement, floor: &HermitianStack) -> HermitianStack {
    let mut out = HermitianStack::zeros(floor.lines(), floor.n());
    for l in 0..floor.lines() {
        increment(statement, &CMat::from_hermitian(floor, l)).store_hermitian(&mut out, l);
    }
    out
}

/// The drive-side congruence of a `raise` (theory 5.1, U1):
/// `M_d = Z Lambda^1/2 H`, `Lambda = diag(10^(db_i / 10))`, `Z` the
/// regularized inverse of the model `h` (`n` by `m`). The total drive in the
/// band is `M_d S_F M_d^H`: a congruence of the floor law's own drive, so it
/// is positive semi-definite and carries what the floor law has learned
/// there. With an exact model and `n <= m` its response is
/// `Lambda^1/2 G_F Lambda^1/2`, the total (5.2).
pub fn raise_drive_transform(
    h: &CMat,
    db: &[f64],
) -> Result<CMat, hertzforge_sdm::linalg::LinalgError> {
    let z = h.pinv(Z_REGULARIZATION)?;
    let l: Vec<f64> = db.iter().map(|d| 10f64.powf(d / 20.0)).collect();
    Ok(z.mul(&CMat::diag(&l)).mul(h))
}

/// Relative tolerance below zero on an eigenvalue still read as zero.
pub const ADDITIVE_TOL: f64 = 1e-9;

/// How a band's statement classifies over a range of lines (MV9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandClass {
    /// Whether the increment is positive semi-definite on every line.
    pub additive: bool,
    /// Whether every control is raised by the same ratio (a `raise` with one
    /// dB), for which the increment is `(L - 1) G_F`.
    pub equal_ratio: bool,
    /// The line with the smallest relative eigenvalue.
    pub worst_line: usize,
    /// Its smallest eigenvalue over its largest eigenvalue modulus (0 for a
    /// zero increment).
    pub min_eigenvalue_rel: f64,
}

/// Classify a statement over the floor's lines `[lines.0, lines.1)`: added
/// random when the increment is positive semi-definite on every line.
pub fn classify(
    statement: &BandStatement,
    floor: &HermitianStack,
    lines: (usize, usize),
) -> BandClass {
    let equal_ratio = match statement {
        BandStatement::Raise { db } => db.values().windows(2).all(|w| w[0] == w[1]),
        _ => false,
    };
    let mut out = BandClass {
        additive: true,
        equal_ratio,
        worst_line: lines.0,
        min_eigenvalue_rel: 0.0,
    };
    let mut worst = f64::INFINITY;
    for l in lines.0..lines.1.min(floor.lines()) {
        let d = increment(statement, &CMat::from_hermitian(floor, l));
        let Ok((vals, _)) = d.eigh() else {
            continue;
        };
        let big = vals.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        let rel = if big > 0.0 { vals[0] / big } else { 0.0 };
        if rel < worst {
            worst = rel;
            out.worst_line = l;
            out.min_eigenvalue_rel = rel;
        }
    }
    out.additive = out.min_eigenvalue_rel >= -ADDITIVE_TOL;
    out
}

/// The edge weights of a band at `center_hz` on a grid of `lines` at `df`
/// (line `k` at `k df`): the share of each line's bin the band covers.
pub fn weights_at(width: &BandWidth, center_hz: f64, df: f64, out: &mut [f64]) {
    let (lo, hi) = width.edges_at(center_hz);
    band_weights(0.0, df, lo, hi, out);
}

/// Fold a band at `center_hz` into a reference: `G_ref += w_b Delta_b` on
/// every line the band covers (theory 5.3 (iii)). The increment is formed
/// from the reference as given (the floor).
pub fn fold_into_reference(
    reference: &mut HermitianStack,
    statement: &BandStatement,
    width: &BandWidth,
    center_hz: f64,
    df: f64,
) {
    let mut w = vec![0.0; reference.lines()];
    weights_at(width, center_hz, df, &mut w);
    let floor = reference.clone();
    for (l, &wl) in w.iter().enumerate() {
        if wl <= 0.0 {
            continue;
        }
        let g = CMat::from_hermitian(&floor, l);
        let total = g.add(&increment(statement, &g).scaled(wl));
        total.store_hermitian(reference, l);
    }
}

/// Whether a band may be folded into the floor's reference under `auto`:
/// it never moves (a `fixed` schedule with no end), has the default window
/// and is enabled. Whether it is switched live during the run is the
/// engine's to know; a band that may be is not to be folded.
pub fn foldable(part: &MimoPart, band: &MimoNarrowbandPart) -> bool {
    matches!(
        band.schedule,
        PartSchedule::Fixed {
            for_s: None,
            for_cycles: None,
            ..
        }
    ) && part.enabled
        && part.window == PartWindow::default()
}

// =============================================================================
// The moving-frame estimate
// =============================================================================

/// A band's `n` by `n` estimate in its own moving frame (theory 5.2): per
/// analysis frame, each offset `o` (line `round(center / df) + o - half`)
/// takes that line's periodogram matrix; averaged frame to frame with a
/// forgetting depth. Preallocated; [`Self::add_frame`] does not allocate.
#[derive(Debug, Clone)]
pub struct MovingFrameEstimate {
    n: usize,
    half: usize,
    depth: f64,
    sum: Vec<C64>,
    counts: Vec<f64>,
}

impl MovingFrameEstimate {
    /// An empty estimate of `n` channels over `2 half + 1` offsets that
    /// forgets past `depth` frames per offset (0: never).
    pub fn new(n: usize, half: usize, depth: usize) -> Self {
        let offsets = 2 * half + 1;
        MovingFrameEstimate {
            n,
            half,
            depth: depth as f64,
            sum: vec![C64::new(0.0, 0.0); offsets * HermitianStack::packed_len(n)],
            counts: vec![0.0; offsets],
        }
    }

    /// Offsets.
    pub fn offsets(&self) -> usize {
        2 * self.half + 1
    }

    /// Offsets either side of the center.
    pub fn half(&self) -> usize {
        self.half
    }

    /// Forget everything.
    pub fn reset(&mut self) {
        self.sum.fill(C64::new(0.0, 0.0));
        self.counts.fill(0.0);
    }

    /// The grid line of offset `o` for a band at `center_hz`, if on the grid.
    pub fn line_of(&self, o: usize, center_hz: f64, df: f64, lines: usize) -> Option<usize> {
        let c = (center_hz / df).round() as i64;
        let l = c + o as i64 - self.half as i64;
        (l >= 0 && (l as usize) < lines).then_some(l as usize)
    }

    /// Add one frame: `spectra` the frame's scaled spectra, line-major (`n`
    /// per line, as [`crate::estimate::FrameSpectra::spectra`] gives them),
    /// for a band at `center_hz` at the frame's middle. Lines marked in
    /// `excluded` (a tone inside the band, theory 5.4) are skipped.
    pub fn add_frame(&mut self, spectra: &[C64], center_hz: f64, df: f64, excluded: &[bool]) {
        let n = self.n;
        let lines = spectra.len() / n.max(1);
        let pl = HermitianStack::packed_len(n);
        for o in 0..self.offsets() {
            let Some(l) = self.line_of(o, center_hz, df, lines) else {
                continue;
            };
            if excluded.get(l).copied().unwrap_or(false) {
                continue;
            }
            let x = &spectra[l * n..(l + 1) * n];
            let s = &mut self.sum[o * pl..(o + 1) * pl];
            if self.depth > 0.0 && self.counts[o] >= self.depth {
                let keep = (self.depth - 1.0) / self.depth;
                for v in s.iter_mut() {
                    *v *= keep;
                }
                self.counts[o] *= keep;
            }
            let mut k = 0;
            for r in 0..n {
                for c in r..n {
                    s[k] += x[r] * x[c].conj();
                    k += 1;
                }
            }
            self.counts[o] += 1.0;
        }
    }

    /// Frames in the average at offset `o`.
    pub fn frames(&self, o: usize) -> f64 {
        self.counts[o]
    }

    /// Degrees of freedom at offset `o`: two per frame, less the half-overlap
    /// correlation of Hann frames ([`hertzforge_mixed::floor::dof_per_frame`]).
    pub fn dof(&self, o: usize) -> f64 {
        self.counts[o] * hertzforge_mixed::floor::dof_per_frame()
    }

    /// The smallest degrees of freedom over offsets `[a, b)`.
    pub fn min_dof(&self, inner: (usize, usize)) -> f64 {
        (inner.0..inner.1.min(self.offsets()))
            .map(|o| self.dof(o))
            .fold(f64::INFINITY, f64::min)
    }

    /// The mean per offset (zeros where no frame has arrived).
    pub fn estimate(&self) -> HermitianStack {
        let pl = HermitianStack::packed_len(self.n);
        let mut data = vec![C64::new(0.0, 0.0); self.sum.len()];
        for o in 0..self.offsets() {
            if self.counts[o] > 0.0 {
                for k in 0..pl {
                    data[o * pl + k] = self.sum[o * pl + k] / self.counts[o];
                }
            }
        }
        HermitianStack::from_packed(self.offsets(), self.n, data).expect("packed shape")
    }

    /// Each control's mean autospectrum over offsets `[a, b)`.
    pub fn control_levels(&self, inner: (usize, usize)) -> Vec<f64> {
        let e = self.estimate();
        mean_diagonal(&e, inner)
    }
}

/// Each channel's mean autospectrum over lines `[a, b)` of a stack.
pub fn mean_diagonal(g: &HermitianStack, lines: (usize, usize)) -> Vec<f64> {
    let n = g.n();
    let (a, b) = (lines.0, lines.1.min(g.lines()));
    let k = b.saturating_sub(a).max(1) as f64;
    (0..n)
        .map(|i| (a..b).map(|l| g.get(l, i, i).re).sum::<f64>() / k)
        .collect()
}

// =============================================================================
// The band law trait
// =============================================================================

/// What a band law's drive is: added to the floor's drive, or the total in
/// the band (Candidate Laws section 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveDeclared {
    /// `S_b`, added to the floor law's drive.
    Increment,
    /// The total drive in the band.
    Total,
}

/// What a band law is told once, at setup.
#[derive(Debug, Clone, PartialEq)]
pub struct BandSetup {
    /// Control channels.
    pub n: usize,
    /// Drives.
    pub m: usize,
    /// The band's statement.
    pub statement: BandStatement,
    /// Whether the statement is added random over the floor.
    pub additive: bool,
    /// Offsets in the band's frame (`2 half + 1`), and their spacing, Hz.
    pub offsets: usize,
    /// Offset spacing, Hz (the grid's).
    pub df_hz: f64,
    /// The band's inner offsets `[a, b)`, where its level is read.
    pub inner: (usize, usize),
    /// Alarm and abort per control, dB below and above.
    pub alarm_db: Vec<(f64, f64)>,
    /// Abort per control.
    pub abort_db: Vec<(f64, f64)>,
}

/// What a band law is given each loop, in the band's own frame: every stack
/// has one line per offset.
#[derive(Debug, Clone, Copy)]
pub struct BandLoopInputs<'a> {
    /// The floor's reference under the band, `n` by `n`, at full level.
    pub floor_reference: &'a HermitianStack,
    /// The band's increment `Delta` per offset, at full level.
    pub increment: &'a HermitianStack,
    /// The moving-frame estimate (floor plus band, measured), at full level.
    pub estimate: &'a HermitianStack,
    /// The estimate's degrees of freedom (the smallest inner offset's).
    pub dof: f64,
    /// The model at the band's next position, `n` by `m`.
    pub model_next: &'a MatrixStack,
    /// The floor law's drive on those lines, `m` by `m`.
    pub floor_drive: &'a HermitianStack,
    /// The band's drive in force, `m` by `m`.
    pub band_drive: &'a HermitianStack,
    /// The band's sweep rate, Hz per second (0 when dwelling).
    pub sweep_rate_hz_s: f64,
    /// Whether the band's drive was held by a ceiling last loop.
    pub limiting: bool,
}

/// A band law's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct BandLoopOutput {
    /// The drive per offset, `m` by `m`, positive semi-definite, before the
    /// core's edge weights.
    pub drive: HermitianStack,
    /// Increment or total.
    pub declared: DriveDeclared,
    /// Whether the law's own per-loop cap bound.
    pub capped: bool,
}

/// Why a band law could not answer.
#[derive(Debug, Clone, PartialEq)]
pub struct BandLawError(pub String);

impl std::fmt::Display for BandLawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BandLawError {}

/// A band law (design D10). The trait takes and returns what a future band
/// world would (Candidate Laws section 2), so it can be frozen into one.
pub trait BandLaw: Send {
    /// The law's id.
    fn id(&self) -> &'static str;
    /// Told once, before the first loop.
    fn setup(&mut self, setup: &BandSetup) -> Result<(), BandLawError>;
    /// One loop: the band's drive at the position it will have when the next
    /// segment plays.
    fn iterate(&mut self, inputs: &BandLoopInputs<'_>) -> Result<BandLoopOutput, BandLawError>;
    /// The law's gains now, dB (one for a scalar servo, one per control for a
    /// per-control servo), for the record.
    fn gains_db(&self) -> Vec<f64>;
}

/// The parameters the built-in band laws read from `{id, params}`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLawParams {
    /// Proportional gain on the dB error (`gain`, default 0.5).
    pub gain: f64,
    /// Largest move per loop, dB (`cap_db`, default 1).
    pub cap_db: f64,
    /// `per_control` with more controls than drives: the largest gain either
    /// way, dB (`max_gain_db`, default 6), the anti-windup cap.
    pub max_gain_db: f64,
}

/// Default anti-windup cap of `per_control` when `n > m`, dB.
pub const DEFAULT_PER_CONTROL_MAX_GAIN_DB: f64 = 6.0;

impl Default for BandLawParams {
    fn default() -> Self {
        BandLawParams {
            gain: DEFAULT_BAND_SERVO_GAIN,
            cap_db: DEFAULT_BAND_SERVO_CAP_DB,
            max_gain_db: DEFAULT_PER_CONTROL_MAX_GAIN_DB,
        }
    }
}

/// Read the built-in band laws' parameters; an unknown or unusable
/// parameter is refused with plain words.
pub fn band_law_params(pick: &LawPick) -> Result<BandLawParams, String> {
    let mut p = BandLawParams::default();
    for (k, v) in &pick.params {
        let x = v
            .as_f64()
            .ok_or_else(|| format!("band law parameter {k} is a number"))?;
        match k.as_str() {
            "gain" if x.is_finite() && x > 0.0 && x <= 1.0 => p.gain = x,
            "cap_db" if x.is_finite() && x > 0.0 => p.cap_db = x,
            "max_gain_db" if x.is_finite() && x > 0.0 => p.max_gain_db = x,
            "gain" | "cap_db" | "max_gain_db" => {
                return Err(format!("band law parameter {k} out of range"))
            }
            _ => return Err(format!("unknown band law parameter {k}")),
        }
    }
    Ok(p)
}

/// Build a band law by id (`scalar_servo`, `per_control`). `auto` is not a
/// law: see [`resolve_band_law`].
pub fn band_law(pick: &LawPick) -> Result<Box<dyn BandLaw>, MimoMixedConfigError> {
    let params = band_law_params(pick).map_err(|why| {
        MimoMixedConfigError::Parts(hertzforge_mixed::ConfigError::BadConfig {
            field: "law.bands",
            why,
        })
    })?;
    match pick.id.as_str() {
        ScalarServo::ID => Ok(Box::new(ScalarServo::new(params))),
        PerControlServo::ID => Ok(Box::new(PerControlServo::new(params))),
        _ => Err(MimoMixedConfigError::UnknownBandLaw {
            id: pick.id.clone(),
        }),
    }
}

/// What a band gets under the configured band law.
pub enum BandResolution {
    /// Folded into the floor's reference; no band law, no exclusion.
    Fold,
    /// Servoed by this law; the floor's lines under it pause.
    Servo(Box<dyn BandLaw>),
}

impl std::fmt::Debug for BandResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BandResolution::Fold => f.write_str("Fold"),
            BandResolution::Servo(l) => write!(f, "Servo({})", l.id()),
        }
    }
}

/// Resolve the band law for one band: under `auto`, a band that is
/// [`foldable`] and additive is folded, any other gets `scalar_servo`; a
/// named law is that law for every band.
pub fn resolve_band_law(
    pick: &LawPick,
    part: &MimoPart,
    band: &MimoNarrowbandPart,
) -> Result<BandResolution, MimoMixedConfigError> {
    if pick.id == "auto" {
        if foldable(part, band) {
            return Ok(BandResolution::Fold);
        }
        let servo = LawPick {
            id: ScalarServo::ID.into(),
            params: pick.params.clone(),
        };
        return band_law(&servo).map(BandResolution::Servo);
    }
    band_law(pick).map(BandResolution::Servo)
}

/// The regularized inverse of the model on one offset.
fn z_of(model: &MatrixStack, o: usize) -> Result<CMat, BandLawError> {
    CMat::from_stack(model, o)
        .pinv(Z_REGULARIZATION)
        .map_err(|e| BandLawError(format!("model inverse on offset {o}: {e}")))
}

fn check_inputs(s: &BandSetup, i: &BandLoopInputs<'_>) -> Result<(), BandLawError> {
    let k = s.offsets;
    let ok = i.floor_reference.lines() == k
        && i.increment.lines() == k
        && i.estimate.lines() == k
        && i.model_next.lines() == k
        && i.floor_reference.n() == s.n
        && i.increment.n() == s.n
        && i.estimate.n() == s.n
        && i.model_next.rows() == s.n
        && i.model_next.cols() == s.m;
    if ok {
        Ok(())
    } else {
        Err(BandLawError(format!(
            "band inputs are not {k} offsets of {} controls by {} drives",
            s.n, s.m
        )))
    }
}

/// The measured band level over the floor per control on the inner offsets:
/// `ahat_i = mean(Ghat_ii - G_F,ii)` and the stated `a_i = mean(Delta_ii)`.
fn added_levels(s: &BandSetup, i: &BandLoopInputs<'_>) -> (Vec<f64>, Vec<f64>) {
    let est = mean_diagonal(i.estimate, s.inner);
    let floor = mean_diagonal(i.floor_reference, s.inner);
    let stated = mean_diagonal(i.increment, s.inner);
    let measured = est.iter().zip(&floor).map(|(e, f)| e - f).collect();
    (measured, stated)
}

/// `Z Gamma^1/2 Delta Gamma^1/2 Z^H` on every offset (`gains` per control,
/// power).
fn feed_forward(
    s: &BandSetup,
    i: &BandLoopInputs<'_>,
    gains: &[f64],
) -> Result<HermitianStack, BandLawError> {
    let mut out = HermitianStack::zeros(s.offsets, s.m);
    let half: Vec<f64> = gains.iter().map(|g| g.max(0.0).sqrt()).collect();
    let g = CMat::diag(&half);
    for o in 0..s.offsets {
        let z = z_of(i.model_next, o)?;
        let d = CMat::from_hermitian(i.increment, o).congruence(&g);
        d.congruence(&z).store_hermitian(&mut out, o);
    }
    Ok(out)
}

/// `scalar_servo` (theory 5.3 (i), the single-output method lifted):
/// `S_b = g_b Z Delta_b Z^H`, `ln g_b` servoed on the mean of the controls'
/// band-over-floor ratios read in the moving frame
/// (`level_b = sum_i v_i ahat_i / sum_i v_i a_i`, `v_i = 1 / a_i`), by
/// single-output Mixed Mode's [`BandServo`]. While the band is limiting its
/// gain does not rise.
#[derive(Debug, Clone)]
pub struct ScalarServo {
    servo: BandServo,
    setup: Option<BandSetup>,
}

impl ScalarServo {
    /// The law's id.
    pub const ID: &'static str = "scalar_servo";

    /// A servo at unity gain.
    pub fn new(p: BandLawParams) -> Self {
        ScalarServo {
            servo: BandServo {
                gain_db: 0.0,
                k: p.gain,
                cap_db: p.cap_db,
            },
            setup: None,
        }
    }

    /// The servo's state.
    pub fn servo(&self) -> &BandServo {
        &self.servo
    }
}

impl BandLaw for ScalarServo {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn setup(&mut self, setup: &BandSetup) -> Result<(), BandLawError> {
        self.setup = Some(setup.clone());
        Ok(())
    }

    fn iterate(&mut self, i: &BandLoopInputs<'_>) -> Result<BandLoopOutput, BandLawError> {
        let s = self
            .setup
            .clone()
            .ok_or_else(|| BandLawError("iterate before setup".into()))?;
        check_inputs(&s, i)?;
        let mut capped = false;
        if i.dof > 0.0 {
            let (measured, stated) = added_levels(&s, i);
            let (mut num, mut den) = (0.0, 0.0);
            for (m, a) in measured.iter().zip(&stated) {
                if *a > 0.0 {
                    num += m / a;
                    den += 1.0;
                }
            }
            if den > 0.0 {
                let before = self.servo;
                capped = self.servo.update(num / den, 1.0);
                if i.limiting && self.servo.gain_db > before.gain_db {
                    self.servo = before;
                }
            }
        }
        let g = vec![self.servo.gain(); s.n];
        Ok(BandLoopOutput {
            drive: feed_forward(&s, i, &g)?,
            declared: DriveDeclared::Increment,
            capped,
        })
    }

    fn gains_db(&self) -> Vec<f64> {
        vec![self.servo.gain_db]
    }
}

/// `per_control` (theory 5.3 (ii)): `S_b = Z Gamma^1/2 Delta Gamma^1/2 Z^H`
/// with one power gain per control, `gamma_i <- gamma_i (a_i / ahat_i)^c`,
/// capped per loop; with more controls than drives each gain is also held
/// within `max_gain_db` either way (anti-windup: the reachable increment has
/// rank `m`, so `n` gains chase levels that cannot all be met).
#[derive(Debug, Clone)]
pub struct PerControlServo {
    params: BandLawParams,
    servos: Vec<BandServo>,
    setup: Option<BandSetup>,
}

impl PerControlServo {
    /// The law's id.
    pub const ID: &'static str = "per_control";

    /// Servos at unity gain (sized at setup).
    pub fn new(p: BandLawParams) -> Self {
        PerControlServo {
            params: p,
            servos: Vec::new(),
            setup: None,
        }
    }
}

impl BandLaw for PerControlServo {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn setup(&mut self, setup: &BandSetup) -> Result<(), BandLawError> {
        self.servos = vec![
            BandServo {
                gain_db: 0.0,
                k: self.params.gain,
                cap_db: self.params.cap_db,
            };
            setup.n
        ];
        self.setup = Some(setup.clone());
        Ok(())
    }

    fn iterate(&mut self, i: &BandLoopInputs<'_>) -> Result<BandLoopOutput, BandLawError> {
        let s = self
            .setup
            .clone()
            .ok_or_else(|| BandLawError("iterate before setup".into()))?;
        check_inputs(&s, i)?;
        let mut capped = false;
        if i.dof > 0.0 {
            let (measured, stated) = added_levels(&s, i);
            for (k, sv) in self.servos.iter_mut().enumerate() {
                if stated[k] <= 0.0 {
                    continue;
                }
                let before = *sv;
                capped |= sv.update(measured[k], stated[k]);
                if i.limiting && sv.gain_db > before.gain_db {
                    *sv = before;
                }
                if s.n > s.m {
                    let cap = self.params.max_gain_db;
                    if sv.gain_db.abs() > cap {
                        sv.gain_db = sv.gain_db.clamp(-cap, cap);
                        capped = true;
                    }
                }
            }
        }
        let g: Vec<f64> = self.servos.iter().map(|s| s.gain()).collect();
        Ok(BandLoopOutput {
            drive: feed_forward(&s, i, &g)?,
            declared: DriveDeclared::Increment,
            capped,
        })
    }

    fn gains_db(&self) -> Vec<f64> {
        self.servos.iter().map(|s| s.gain_db).collect()
    }
}

/// The core's side of a band law's answer: weight each offset by the band's
/// edge weight there and check the result is positive semi-definite (an
/// eigenvalue below `-ADDITIVE_TOL` of the line's largest refuses it).
pub fn place_band_drive(
    out: &BandLoopOutput,
    weights: &[f64],
) -> Result<HermitianStack, BandLawError> {
    let d = &out.drive;
    if weights.len() != d.lines() {
        return Err(BandLawError(format!(
            "{} weights for {} offsets",
            weights.len(),
            d.lines()
        )));
    }
    let mut placed = HermitianStack::zeros(d.lines(), d.n());
    for (o, &w) in weights.iter().enumerate() {
        let m = CMat::from_hermitian(d, o);
        if !m.is_finite() {
            return Err(BandLawError(format!("offset {o}: a non-finite drive")));
        }
        let (lo, hi) = m
            .eig_range()
            .map_err(|e| BandLawError(format!("offset {o}: {e}")))?;
        let big = lo.abs().max(hi.abs());
        if big > 0.0 && lo < -ADDITIVE_TOL * big {
            return Err(BandLawError(format!(
                "offset {o}: the band drive is not positive semi-definite ({lo:e})"
            )));
        }
        m.scaled(w.clamp(0.0, 1.0)).store_hermitian(&mut placed, o);
    }
    Ok(placed)
}
