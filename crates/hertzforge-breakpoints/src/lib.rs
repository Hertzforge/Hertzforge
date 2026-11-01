//! `hertzforge-breakpoints` - Random reference profiles + alarm/abort envelopes.
//!
//! A `BreakpointTable` is the operator-authored definition of a random-vibration
//! reference PSD: a small set of frequency breakpoints, each either a PSD anchor
//! (`g^2/Hz`) or a slope (`dB/oct`) that derives its level from the prior anchor.
//! The table is interpolated onto the control line grid with log-log straight
//! segments (constant dB/oct within a segment), which matches established
//! controller behavior (the controller's host software
//! interpolates the breakpoint PSD to the line grid itself before download). The
//! crate also computes gRMS (two independent ways that must agree), RMS kinematics
//! (velocity / displacement, for the feasibility calculator), and the NEW
//! frequency-domain alarm/abort envelope, which is distinct from and
//! in addition to the per-channel tri-field kinematic abort.
//!
//! Import hardening (header aliasing, RFC-4180 CSV quoting, delimiter auto-detect,
//! comment tolerance, lossless JSON round-trip) mirrors the bar set by
//! `hertzforge-channel-table`.
//!
//! Pure library: no I/O beyond what serde does; no `unwrap`/`expect`/`panic!` in
//! library paths (validation returns errors).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::fmt;

/// Standard gravity in SI, `m/s^2`. Used to convert acceleration PSD (`g^2/Hz`)
/// into velocity / displacement RMS in SI, which the caller may then present in
/// in/s and mil.
pub const G0_M_PER_S2: f64 = 9.80665;

/// Inches per metre, for converting SI kinematic integrals into the in/s and mil
/// units the `AbortLimits` and shaker-DB conventions use.
pub const IN_PER_M: f64 = 39.37007874015748; // 1 / 0.0254

/// dB per octave that corresponds to the power-law exponent `n = -1` (the
/// analytic-integral singularity where the per-segment RMS integral becomes a
/// natural log). `10 * log10(2) ~= 3.0103`, so `-3.0103 dB/oct` is the exact
/// singular slope; a nominal `-3 dB/oct` is very close but not exactly singular.
pub const DB_PER_OCT_PER_DECADE: f64 = 3.010_299_956_639_812; // 10*log10(2)

/// One breakpoint level: either an explicit PSD anchor or a slope that derives the
/// level from the previous resolved level across the frequency span.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LevelSpec {
    /// Explicit PSD anchor in `g^2/Hz`.
    Psd(f64),
    /// Slope in `dB/oct` from the previous resolved level. The level at this row's
    /// frequency is `prev_level * 2^(slope/DB_PER_OCT_PER_DECADE * log2(f/f_prev))`,
    /// i.e. a straight line in log-log space with the given dB/oct.
    SlopeDbPerOct(f64),
}

/// A single breakpoint row.
///
/// `alarm_db` / `abort_db` are optional per-row `(-, +)` dB offsets applied to the
/// resolved reference level to form the frequency-domain envelope. The
/// first tuple element is the negative (below-reference) offset magnitude, the
/// second is the positive (above-reference) offset; both are stored as positive
/// dB magnitudes (e.g. `(3.0, 6.0)` = 3 dB below / 6 dB above). A single symmetric
/// value in an import maps to `(v, v)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BreakpointRow {
    pub freq_hz: f64,
    pub level: LevelSpec,
    /// `(minus_db, plus_db)` alarm band, positive magnitudes. `None` -> use the
    /// envelope's global default.
    pub alarm_db: Option<(f64, f64)>,
    /// `(minus_db, plus_db)` abort band, positive magnitudes. `None` -> use the
    /// envelope's global default.
    pub abort_db: Option<(f64, f64)>,
}

impl BreakpointRow {
    /// A PSD-anchor row with no per-row alarm/abort override.
    pub fn psd(freq_hz: f64, g2_per_hz: f64) -> Self {
        BreakpointRow {
            freq_hz,
            level: LevelSpec::Psd(g2_per_hz),
            alarm_db: None,
            abort_db: None,
        }
    }

    /// A slope row (`dB/oct` from the prior anchor) with no per-row override.
    pub fn slope(freq_hz: f64, db_per_oct: f64) -> Self {
        BreakpointRow {
            freq_hz,
            level: LevelSpec::SlopeDbPerOct(db_per_oct),
            alarm_db: None,
            abort_db: None,
        }
    }
}

/// The breakpoint table: an ordered list of rows plus an optional label. Frequency
/// must be strictly increasing and the first row must be a PSD anchor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BreakpointTable {
    pub label: String,
    pub rows: Vec<BreakpointRow>,
}

/// Hard errors that make a table unusable.
#[derive(Debug, Clone, PartialEq)]
pub enum BreakpointError {
    /// The table has no rows.
    Empty,
    /// The first row is a slope, not a PSD anchor (grammar rule: first row must be
    /// a PSD anchor so slopes have something to derive from).
    FirstRowNotAnchor,
    /// Frequencies are not strictly increasing. Carries the row index (0-based) of
    /// the offending row and its frequency.
    NonMonotonicFreq { row: usize, freq_hz: f64 },
    /// A frequency is not strictly positive (log-log interpolation needs `f > 0`).
    NonPositiveFreq { row: usize, freq_hz: f64 },
    /// A PSD anchor is negative (a density cannot be negative).
    NegativePsd { row: usize, value: f64 },
    /// Parse failure from an import path.
    Parse(String),
}

impl fmt::Display for BreakpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BreakpointError::Empty => write!(f, "breakpoint table is empty"),
            BreakpointError::FirstRowNotAnchor => {
                write!(f, "first breakpoint row must be a PSD anchor, not a slope")
            }
            BreakpointError::NonMonotonicFreq { row, freq_hz } => write!(
                f,
                "row {row}: frequency {freq_hz} Hz is not strictly greater than the previous row"
            ),
            BreakpointError::NonPositiveFreq { row, freq_hz } => {
                write!(
                    f,
                    "row {row}: frequency {freq_hz} Hz must be strictly positive"
                )
            }
            BreakpointError::NegativePsd { row, value } => {
                write!(
                    f,
                    "row {row}: PSD anchor {value} g^2/Hz must not be negative"
                )
            }
            BreakpointError::Parse(s) => write!(f, "parse error: {s}"),
        }
    }
}

impl std::error::Error for BreakpointError {}

/// Non-fatal findings from [`BreakpointTable::validate_report`]. These do not block
/// use but flag likely operator mistakes.
#[derive(Debug, Clone, PartialEq)]
pub enum BreakpointWarning {
    /// An imported row carried BOTH a PSD anchor and a slope, and the slope-implied
    /// level disagreed with the PSD by more than 0.1 dB. PSD wins (grammar rule);
    /// this records the discrepancy. Carries the row index, the PSD level used, and
    /// the level the slope would have implied.
    PsdSlopeDisagreement {
        row: usize,
        psd_level: f64,
        slope_implied_level: f64,
        delta_db: f64,
    },
    /// A per-row alarm band is wider than the abort band on the same row (alarm
    /// should trip before abort). Carries the row index.
    AlarmWiderThanAbort(usize),
}

impl fmt::Display for BreakpointWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BreakpointWarning::PsdSlopeDisagreement {
                row,
                psd_level,
                slope_implied_level,
                delta_db,
            } => write!(
                f,
                "row {row}: imported PSD {psd_level:.6} g^2/Hz and slope-implied \
                 {slope_implied_level:.6} g^2/Hz disagree by {delta_db:.2} dB (PSD wins)"
            ),
            BreakpointWarning::AlarmWiderThanAbort(row) => write!(
                f,
                "row {row}: alarm band is wider than the abort band (alarm should trip first)"
            ),
        }
    }
}

/// Full validation result: hard errors + non-fatal warnings, mirroring
/// `hertzforge-channel-table::ValidationReport`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationReport {
    pub errors: Vec<BreakpointError>,
    pub warnings: Vec<BreakpointWarning>,
}

impl ValidationReport {
    /// True when there are no hard errors (warnings allowed).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// A resolved anchor: every row's frequency paired with its resolved PSD level in
/// `g^2/Hz` (slopes turned into concrete levels). The output of
/// [`BreakpointTable::resolve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedAnchor {
    pub freq_hz: f64,
    pub level_g2_hz: f64,
}

/// A uniform line-grid spectrum: PSD sampled at `f0_hz + k*df_hz`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineSpectrum {
    pub f0_hz: f64,
    pub df_hz: f64,
    /// PSD per line in `g^2/Hz`. Out-of-band bins are `0.0`.
    pub psd: Vec<f64>,
}

impl LineSpectrum {
    /// Frequency of line `k`.
    #[inline]
    pub fn freq(&self, k: usize) -> f64 {
        self.f0_hz + k as f64 * self.df_hz
    }

    /// Number of lines.
    pub fn len(&self) -> usize {
        self.psd.len()
    }

    pub fn is_empty(&self) -> bool {
        self.psd.is_empty()
    }
}

// --- log-log segment interpolation core --------------------------------------

/// Interpolate the PSD level at frequency `f` on the log-log straight segment
/// joining `(f1, w1)` and `(f2, w2)`. `w1`/`w2` are `g^2/Hz`; a segment is a
/// constant dB/oct line. Requires `f1 > 0`, `f2 > f1`, both levels `>= 0`.
///
/// A zero endpoint (a level of exactly 0 `g^2/Hz`) cannot be represented as a
/// finite dB/oct slope; in that case the level is treated as the geometric floor 0
/// across the segment except at the nonzero endpoint (this never occurs for real
/// profiles, which are strictly positive in-band). The normal path is
/// `w(f) = w1 * (f/f1)^n` with `n = log(w2/w1) / log(f2/f1)`.
fn loglog_level_at(f: f64, f1: f64, w1: f64, f2: f64, w2: f64) -> f64 {
    if w1 <= 0.0 || w2 <= 0.0 {
        // Degenerate: a zero-level endpoint. Linear-in-log fallback is undefined;
        // return the nearer endpoint level (0 dominates). Real profiles avoid this.
        if f <= f1 {
            return w1;
        }
        if f >= f2 {
            return w2;
        }
        return 0.0;
    }
    let n = (w2 / w1).ln() / (f2 / f1).ln();
    w1 * (f / f1).powf(n)
}

/// Analytic RMS-squared contribution (integral of PSD over `df`) of a single
/// log-log segment from `(f1, w1)` to `(f2, w2)`.
///
/// The PSD on a log-log-straight segment is the power law `W(f) = w1 (f/f1)^n`
/// with `n = ln(w2/w1)/ln(f2/f1)`. The standard closed form is
///
/// ```text
///   integral_{f1}^{f2} w1 (f/f1)^n df
///     = w1/f1^n * (f2^{n+1} - f1^{n+1}) / (n+1)     (n != -1)
///     = w1 * f1 * ln(f2/f1)                          (n == -1, the -3.0103 dB/oct case)
/// ```
///
/// The `n = -1` branch is the `m = -3.0103 dB/oct` singularity (a flat velocity
/// PSD); it is handled explicitly. See the demo profile (20/0.01, 80/0.04,
/// 350/0.04, 2000/0.007 = 6.058 gRMS), whose final segment is exactly `-3.0103 dB/oct`.
fn segment_integral(f1: f64, w1: f64, f2: f64, w2: f64) -> f64 {
    if f2 <= f1 || f1 <= 0.0 {
        return 0.0;
    }
    // Flat segment (equal levels): the power law degenerates to n = 0.
    if (w1 - w2).abs() <= f64::EPSILON * w1.max(w2).max(1.0) {
        return w1 * (f2 - f1);
    }
    if w1 <= 0.0 || w2 <= 0.0 {
        // Degenerate zero endpoint; approximate with a trapezoid in linear f.
        return 0.5 * (w1 + w2) * (f2 - f1);
    }
    let n = (w2 / w1).ln() / (f2 / f1).ln();
    if (n + 1.0).abs() < 1e-9 {
        // singular n = -1  ->  integral = w1 * f1 * ln(f2/f1)
        w1 * f1 * (f2 / f1).ln()
    } else {
        // w1/f1^n * (f2^{n+1} - f1^{n+1})/(n+1)
        (w1 / f1.powf(n)) * (f2.powf(n + 1.0) - f1.powf(n + 1.0)) / (n + 1.0)
    }
}

/// Analytic integral of `PSD(f) * f^p` over a log-log segment - the general moment
/// used for kinematics. With `W(f) = w1 (f/f1)^n`, the integrand exponent is
/// `n + p`, so the closed form is the same as [`segment_integral`] with `n -> n+p`
/// and the `n + p == -1` singular branch.
///
/// `p = 0` gives the RMS-squared (acceleration) integral; `p = -2` gives the
/// velocity moment `integral PSD/f^2 df`; `p = -4` gives the displacement moment.
fn segment_moment(f1: f64, w1: f64, f2: f64, w2: f64, p: f64) -> f64 {
    if f2 <= f1 || f1 <= 0.0 || w1 <= 0.0 || w2 <= 0.0 {
        // Degenerate: fall back to a log-space trapezoid on W(f)*f^p.
        if f2 <= f1 || f1 <= 0.0 {
            return 0.0;
        }
        let a = w1 * f1.powf(p);
        let b = w2 * f2.powf(p);
        return 0.5 * (a + b) * (f2 - f1);
    }
    let n = if (w1 - w2).abs() <= f64::EPSILON * w1.max(w2).max(1.0) {
        0.0
    } else {
        (w2 / w1).ln() / (f2 / f1).ln()
    };
    // integrand = w1 * (f/f1)^n * f^p = (w1 * f1^{-n}) * f^{n+p}
    let e = n + p;
    let pref = w1 * f1.powf(-n);
    if (e + 1.0).abs() < 1e-9 {
        // integral pref * f^{-1} df = pref * ln(f2/f1)
        pref * (f2 / f1).ln()
    } else {
        pref * (f2.powf(e + 1.0) - f1.powf(e + 1.0)) / (e + 1.0)
    }
}

impl BreakpointTable {
    /// A table with a label and no rows.
    pub fn new(label: impl Into<String>) -> Self {
        BreakpointTable {
            label: label.into(),
            rows: Vec::new(),
        }
    }

    /// Push a row (builder-style; does not validate).
    pub fn push(&mut self, row: BreakpointRow) -> &mut Self {
        self.rows.push(row);
        self
    }

    /// Hard-error gate: returns the first violating error. Use
    /// [`BreakpointTable::validate_report`] to collect all errors + warnings.
    ///
    /// Hard errors, in check order:
    /// 1. Table is non-empty.
    /// 2. First row is a PSD anchor.
    /// 3. Every frequency is strictly positive.
    /// 4. Frequencies are strictly increasing.
    /// 5. No PSD anchor is negative.
    pub fn validate(&self) -> Result<(), BreakpointError> {
        match self.validate_report().errors.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Full validation: all hard errors + all non-fatal warnings in one pass.
    pub fn validate_report(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        if self.rows.is_empty() {
            report.errors.push(BreakpointError::Empty);
            return report;
        }
        if !matches!(self.rows[0].level, LevelSpec::Psd(_)) {
            report.errors.push(BreakpointError::FirstRowNotAnchor);
        }
        let mut prev_f: Option<f64> = None;
        for (i, row) in self.rows.iter().enumerate() {
            // `<= 0.0 || is_nan()` (not `!(f > 0.0)`) so a NaN frequency is caught
            // without a negated partial-order comparison (clippy neg_cmp_op).
            if row.freq_hz <= 0.0 || row.freq_hz.is_nan() {
                report.errors.push(BreakpointError::NonPositiveFreq {
                    row: i,
                    freq_hz: row.freq_hz,
                });
            }
            if let Some(pf) = prev_f {
                if row.freq_hz <= pf || row.freq_hz.is_nan() {
                    report.errors.push(BreakpointError::NonMonotonicFreq {
                        row: i,
                        freq_hz: row.freq_hz,
                    });
                }
            }
            if let LevelSpec::Psd(v) = row.level {
                if v < 0.0 {
                    report
                        .errors
                        .push(BreakpointError::NegativePsd { row: i, value: v });
                }
            }
            // alarm-wider-than-abort warning (per row, when both present).
            if let (Some((am, ap)), Some((bm, bp))) = (row.alarm_db, row.abort_db) {
                if am > bm || ap > bp {
                    report
                        .warnings
                        .push(BreakpointWarning::AlarmWiderThanAbort(i));
                }
            }
            prev_f = Some(row.freq_hz);
        }
        report
    }

    /// Resolve every row to a concrete `(freq, level)` anchor, turning slopes into
    /// PSD levels by walking the rows in order. The first row must be a PSD anchor
    /// (validated). Returns an error if the table fails hard validation.
    pub fn resolve(&self) -> Result<Vec<ResolvedAnchor>, BreakpointError> {
        self.validate()?;
        let mut out: Vec<ResolvedAnchor> = Vec::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            let level = match row.level {
                LevelSpec::Psd(v) => v,
                LevelSpec::SlopeDbPerOct(slope) => {
                    // derive from the previous resolved anchor across the span.
                    let prev = out[i - 1]; // i>=1 guaranteed: first row is a PSD anchor
                    let octaves = (row.freq_hz / prev.freq_hz).log2();
                    prev.level_g2_hz * 10f64.powf(slope * octaves / 10.0)
                }
            };
            out.push(ResolvedAnchor {
                freq_hz: row.freq_hz,
                level_g2_hz: level,
            });
        }
        Ok(out)
    }

    /// Interpolate the resolved profile onto a uniform line grid with spacing
    /// `df_hz`, spanning `0 .. f_max` inclusive of the line at or below `f_max`.
    /// Log-log straight segments between resolved anchors (constant dB/oct within a
    /// segment). Bins below the first anchor frequency or above the last are `0.0`
    /// (out-of-band). This matches established controller behavior (the host
    /// interpolates the breakpoint PSD to the line grid before download).
    ///
    /// `df_hz` must be strictly positive. The returned spectrum starts at `f0 = 0`.
    pub fn interpolate(&self, df_hz: f64, f_max: f64) -> Result<LineSpectrum, BreakpointError> {
        // `<= 0.0 || is_nan()` catches a NaN df without a negated partial-order op.
        if df_hz <= 0.0 || df_hz.is_nan() {
            return Err(BreakpointError::Parse(format!(
                "interpolate: df_hz must be positive (got {df_hz})"
            )));
        }
        let anchors = self.resolve()?;
        let n_lines = if f_max < 0.0 {
            0
        } else {
            (f_max / df_hz).floor() as usize + 1
        };
        let f_lo = anchors.first().map(|a| a.freq_hz).unwrap_or(0.0);
        let f_hi = anchors.last().map(|a| a.freq_hz).unwrap_or(0.0);
        let mut psd = vec![0.0f64; n_lines];
        for (k, p) in psd.iter_mut().enumerate() {
            let f = k as f64 * df_hz;
            if f < f_lo || f > f_hi {
                continue; // out-of-band
            }
            // find the segment [a, b] containing f.
            let mut val = 0.0;
            for w in anchors.windows(2) {
                let (a, b) = (w[0], w[1]);
                if f >= a.freq_hz && f <= b.freq_hz {
                    val = loglog_level_at(f, a.freq_hz, a.level_g2_hz, b.freq_hz, b.level_g2_hz);
                    break;
                }
            }
            // single-anchor table: the one point only.
            if anchors.len() == 1 && (f - anchors[0].freq_hz).abs() < df_hz * 0.5 {
                val = anchors[0].level_g2_hz;
            }
            *p = val;
        }
        Ok(LineSpectrum {
            f0_hz: 0.0,
            df_hz,
            psd,
        })
    }

    /// Analytic gRMS from the resolved anchors, integrating each log-log segment in
    /// closed form (see [`segment_integral`]). Independent of any line grid; this is
    /// the reference value the discrete [`grms`] must agree with. Returns an error
    /// if the table fails hard validation.
    pub fn grms_of_table(&self) -> Result<f64, BreakpointError> {
        let anchors = self.resolve()?;
        let mut acc = 0.0;
        for w in anchors.windows(2) {
            acc += segment_integral(
                w[0].freq_hz,
                w[0].level_g2_hz,
                w[1].freq_hz,
                w[1].level_g2_hz,
            );
        }
        Ok(acc.max(0.0).sqrt())
    }

    /// Analytic RMS velocity in **in/s** from the resolved anchors.
    ///
    /// `v_rms = sqrt( integral PSD(f) * (g0 / (2 pi f))^2 df )`, where `PSD` is the
    /// acceleration density in `g^2/Hz` and `g0` is [`G0_M_PER_S2`]. The `(g0)^2`
    /// factor converts `g^2` to `(m/s^2)^2`, and dividing by `(2 pi f)^2` integrates
    /// acceleration to velocity in the frequency domain (each `1/(j 2 pi f)` is one
    /// integration). The SI result (`m/s`) is scaled by [`IN_PER_M`] to in/s.
    pub fn velocity_rms_ips(&self) -> Result<f64, BreakpointError> {
        Ok(self.velocity_rms_si()? * IN_PER_M)
    }

    /// Analytic RMS velocity in SI (`m/s`).
    pub fn velocity_rms_si(&self) -> Result<f64, BreakpointError> {
        let anchors = self.resolve()?;
        // integral PSD * (g0/(2 pi f))^2 df = (g0/(2 pi))^2 * integral PSD * f^{-2} df
        let k = (G0_M_PER_S2 / std::f64::consts::TAU).powi(2);
        let mut acc = 0.0;
        for w in anchors.windows(2) {
            acc += segment_moment(
                w[0].freq_hz,
                w[0].level_g2_hz,
                w[1].freq_hz,
                w[1].level_g2_hz,
                -2.0,
            );
        }
        Ok((k * acc).max(0.0).sqrt())
    }

    /// Analytic RMS displacement in **mil** (thousandths of an inch) from the
    /// resolved anchors.
    ///
    /// `d_rms = sqrt( integral PSD(f) * (g0 / (2 pi f)^2)^2 df )` = double
    /// integration of acceleration to displacement (`f^{-4}` moment). The SI result
    /// (`m`) is scaled to inches and then to mil (`* 1000`).
    pub fn displacement_rms_mil(&self) -> Result<f64, BreakpointError> {
        Ok(self.displacement_rms_si()? * IN_PER_M * 1000.0)
    }

    /// Analytic RMS displacement in SI (`m`).
    pub fn displacement_rms_si(&self) -> Result<f64, BreakpointError> {
        let anchors = self.resolve()?;
        // integral PSD * (g0/(2 pi f)^2)^2 df = (g0/(2 pi)^2)^2 * integral PSD f^{-4} df
        let k = (G0_M_PER_S2 / (std::f64::consts::TAU * std::f64::consts::TAU)).powi(2);
        let mut acc = 0.0;
        for w in anchors.windows(2) {
            acc += segment_moment(
                w[0].freq_hz,
                w[0].level_g2_hz,
                w[1].freq_hz,
                w[1].level_g2_hz,
                -4.0,
            );
        }
        Ok((k * acc).max(0.0).sqrt())
    }

    // --- imports (mirror hertzforge-channel-table hardening) ---------------------

    /// Parse a tab-separated breakpoint table (header aliasing + comment
    /// tolerance). See [`BreakpointTable::from_delimited`].
    pub fn from_tsv(s: &str) -> Result<Self, BreakpointError> {
        Self::from_delimited(s, Some('\t'))
    }

    /// Parse a comma-separated breakpoint table with RFC-4180 quoting.
    pub fn from_csv(s: &str) -> Result<Self, BreakpointError> {
        Self::from_delimited(s, Some(','))
    }

    /// Clipboard paste with automatic delimiter detection (tab, comma, semicolon,
    /// or whitespace). Header aliasing, comment lines (`#`, `//`), and the
    /// two-column unitless `(freq, psd)` shorthand are all tolerated.
    pub fn paste_from_clipboard(s: &str) -> Result<Self, BreakpointError> {
        Self::from_delimited(s, None)
    }

    /// The shared import path. `delim = None` auto-detects the delimiter from the
    /// first data line. Recognized columns (via header aliasing):
    ///
    /// - frequency: `freq`, `frequency`, `hz`, `f`
    /// - level (PSD): `psd`, `level`, `g2`, `g^2`, `g2/hz`, `asd`, `accel`
    /// - slope: `slope`, `db/oct`, `dboct`, `db`, `oct`
    /// - alarm: `alarm` (symmetric) or `alarm-` / `alarm+` (pair)
    /// - abort: `abort` (symmetric) or `abort-` / `abort+` (pair)
    ///
    /// A row may carry BOTH a PSD and a slope; PSD wins (grammar rule) and a
    /// disagreement above 0.1 dB is recorded as a warning by [`import_with_report`].
    /// With no header, a two-column layout is read as `(freq, psd)` and a
    /// three-column layout as `(freq, psd, slope)`.
    ///
    /// Comment lines beginning with `#` or `//` (after optional leading whitespace)
    /// are skipped; blank lines are skipped. RFC-4180 quoting applies for the comma
    /// delimiter.
    pub fn from_delimited(s: &str, delim: Option<char>) -> Result<Self, BreakpointError> {
        // gather non-comment, non-blank logical lines.
        let mut lines: Vec<&str> = Vec::new();
        for raw in s.lines() {
            let line = raw.trim_end_matches('\r');
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
                continue;
            }
            lines.push(line);
        }
        if lines.is_empty() {
            return Err(BreakpointError::Parse("no data rows".into()));
        }

        let d = match delim {
            Some(c) => c,
            None => detect_delimiter(lines[0]),
        };

        // Detect a header row: the first line has at least one non-numeric cell in
        // a recognizable header position.
        let first_cells = split_cells(lines[0], d);
        let has_header = first_cells
            .iter()
            .any(|c| Column::from_header(c).is_some() && parse_num(c).is_none());

        // Build the column map.
        let (col_map, data_start) = if has_header {
            let mut map: Vec<Option<Column>> = Vec::with_capacity(first_cells.len());
            for cell in &first_cells {
                map.push(Column::from_header(cell));
            }
            (map, 1usize)
        } else {
            // positional: (freq, psd[, slope]) - but if a column is textual slope
            // marker we still only assume the numeric positional layout.
            let ncol = first_cells.len();
            let mut map: Vec<Option<Column>> = vec![None; ncol];
            if ncol >= 1 {
                map[0] = Some(Column::Freq);
            }
            if ncol >= 2 {
                map[1] = Some(Column::Psd);
            }
            if ncol >= 3 {
                map[2] = Some(Column::Slope);
            }
            (map, 0usize)
        };

        let mut table = BreakpointTable::new(String::new());
        for line in &lines[data_start..] {
            let cells = split_cells(line, d);
            if cells.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            let mut freq: Option<f64> = None;
            let mut psd: Option<f64> = None;
            let mut slope: Option<f64> = None;
            let mut alarm_m: Option<f64> = None;
            let mut alarm_p: Option<f64> = None;
            let mut alarm_sym: Option<f64> = None;
            let mut abort_m: Option<f64> = None;
            let mut abort_p: Option<f64> = None;
            let mut abort_sym: Option<f64> = None;
            for (i, col) in col_map.iter().enumerate() {
                let Some(col) = col else { continue };
                let raw = cells.get(i).map(|s| s.trim()).unwrap_or("");
                if raw.is_empty() {
                    continue;
                }
                match col {
                    Column::Freq => freq = parse_num(raw),
                    Column::Psd => psd = parse_num(raw),
                    Column::Slope => slope = parse_num(raw),
                    Column::Alarm => alarm_sym = parse_num(raw),
                    Column::AlarmMinus => alarm_m = parse_num(raw),
                    Column::AlarmPlus => alarm_p = parse_num(raw),
                    Column::Abort => abort_sym = parse_num(raw),
                    Column::AbortMinus => abort_m = parse_num(raw),
                    Column::AbortPlus => abort_p = parse_num(raw),
                }
            }
            let freq = freq.ok_or_else(|| {
                BreakpointError::Parse(format!("row missing a frequency: {line:?}"))
            })?;
            // level: PSD wins if present; else slope.
            let level = match (psd, slope) {
                (Some(p), _) => LevelSpec::Psd(p),
                (None, Some(s)) => LevelSpec::SlopeDbPerOct(s),
                (None, None) => {
                    return Err(BreakpointError::Parse(format!(
                        "row {line:?} has neither a PSD level nor a slope"
                    )))
                }
            };
            let alarm_db = combine_band(alarm_sym, alarm_m, alarm_p);
            let abort_db = combine_band(abort_sym, abort_m, abort_p);
            // `level` already applied the grammar rule: when a row carries BOTH a
            // PSD and a slope, the PSD wins. The PSD-vs-slope disagreement WARNING
            // (>0.1 dB) requires knowing the discarded slope, which `BreakpointRow`
            // does not store; that warning is produced by `import_with_report`, which
            // re-scans the source for both-column rows. Plain `from_*` importers apply
            // the rule silently.
            table.rows.push(BreakpointRow {
                freq_hz: freq,
                level,
                alarm_db,
                abort_db,
            });
        }
        Ok(table)
    }

    /// After an import that carried both PSD and slope on some rows, compute the
    /// disagreement warnings (PSD wins; a >0.1 dB gap is flagged). Callers that want
    /// these warnings should run [`BreakpointTable::validate_report`], which does not
    /// have the imported slope; this method is the import-time equivalent that takes
    /// the (row, slope) notes recorded during parsing.
    fn import_disagreements(&self, notes: &[(usize, f64)]) -> Vec<BreakpointWarning> {
        let mut out = Vec::new();
        // resolve without the both-column rows' slopes.
        let anchors = match self.resolve() {
            Ok(a) => a,
            Err(_) => return out,
        };
        for &(row_i, slope) in notes {
            if row_i == 0 || row_i >= self.rows.len() {
                continue;
            }
            let prev = anchors[row_i - 1];
            let cur = anchors[row_i];
            let octaves = (cur.freq_hz / prev.freq_hz).log2();
            let slope_level = prev.level_g2_hz * 10f64.powf(slope * octaves / 10.0);
            let psd_level = cur.level_g2_hz;
            if psd_level > 0.0 && slope_level > 0.0 {
                let delta_db = 10.0 * (psd_level / slope_level).log10();
                if delta_db.abs() > 0.1 {
                    out.push(BreakpointWarning::PsdSlopeDisagreement {
                        row: row_i,
                        psd_level,
                        slope_implied_level: slope_level,
                        delta_db,
                    });
                }
            }
        }
        out
    }

    // --- JSON round-trip (serde) --------------------------------------------

    /// Serialize to pretty JSON (lossless).
    pub fn to_json(&self) -> Result<String, BreakpointError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| BreakpointError::Parse(format!("json serialize: {e}")))
    }

    /// Parse from JSON produced by [`BreakpointTable::to_json`]. Does NOT validate.
    pub fn from_json(s: &str) -> Result<Self, BreakpointError> {
        serde_json::from_str(s).map_err(|e| BreakpointError::Parse(format!("json parse: {e}")))
    }
}

// --- import parsing helpers (module-private) ----------------------------------

/// Import a delimited table AND collect the full validation report, including the
/// PSD-vs-slope disagreement warnings that require knowing the
/// imported slope on both-column rows. This is the entry point a table editor
/// should use so it can surface those warnings.
///
/// Returns the parsed table plus its report. The report merges structural
/// validation with import-time disagreement warnings.
pub fn import_with_report(
    s: &str,
    delim: Option<char>,
) -> Result<(BreakpointTable, ValidationReport), BreakpointError> {
    // First pass: parse to a table (PSD wins on both-column rows).
    let table = BreakpointTable::from_delimited(s, delim)?;
    // Second pass: re-scan the source to collect (row, slope) notes for rows that
    // carried both a PSD and a slope, so we can compute disagreement warnings.
    let notes = collect_both_column_notes(s, delim);
    let mut report = table.validate_report();
    let mut dis = table.import_disagreements(&notes);
    report.warnings.append(&mut dis);
    Ok((table, report))
}

/// Re-scan the source and record `(data_row_index, slope)` for every row that
/// carried BOTH a numeric PSD and a numeric slope. The row index matches the index
/// in the parsed table's `rows`.
fn collect_both_column_notes(s: &str, delim: Option<char>) -> Vec<(usize, f64)> {
    let mut lines: Vec<&str> = Vec::new();
    for raw in s.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return Vec::new();
    }
    let d = delim.unwrap_or_else(|| detect_delimiter(lines[0]));
    let first_cells = split_cells(lines[0], d);
    let has_header = first_cells
        .iter()
        .any(|c| Column::from_header(c).is_some() && parse_num(c).is_none());
    let (col_map, data_start) = if has_header {
        (
            first_cells
                .iter()
                .map(|c| Column::from_header(c))
                .collect::<Vec<_>>(),
            1usize,
        )
    } else {
        let ncol = first_cells.len();
        let mut map: Vec<Option<Column>> = vec![None; ncol];
        if ncol >= 1 {
            map[0] = Some(Column::Freq);
        }
        if ncol >= 2 {
            map[1] = Some(Column::Psd);
        }
        if ncol >= 3 {
            map[2] = Some(Column::Slope);
        }
        (map, 0usize)
    };
    let mut notes = Vec::new();
    let mut row_i = 0usize;
    for line in &lines[data_start..] {
        let cells = split_cells(line, d);
        if cells.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let mut psd = None;
        let mut slope = None;
        for (i, col) in col_map.iter().enumerate() {
            let Some(col) = col else { continue };
            let raw = cells.get(i).map(|s| s.trim()).unwrap_or("");
            match col {
                Column::Psd => psd = parse_num(raw),
                Column::Slope => slope = parse_num(raw),
                _ => {}
            }
        }
        if let (Some(_), Some(sl)) = (psd, slope) {
            notes.push((row_i, sl));
        }
        row_i += 1;
    }
    notes
}

/// Turn a symmetric-or-pair alarm/abort spec into the stored `(minus, plus)`
/// tuple. A symmetric value maps to `(v, v)`; an explicit pair takes precedence.
fn combine_band(sym: Option<f64>, minus: Option<f64>, plus: Option<f64>) -> Option<(f64, f64)> {
    match (minus, plus, sym) {
        (Some(m), Some(p), _) => Some((m.abs(), p.abs())),
        (Some(m), None, Some(s)) => Some((m.abs(), s.abs())),
        (None, Some(p), Some(s)) => Some((s.abs(), p.abs())),
        (Some(m), None, None) => Some((m.abs(), m.abs())),
        (None, Some(p), None) => Some((p.abs(), p.abs())),
        (None, None, Some(s)) => Some((s.abs(), s.abs())),
        (None, None, None) => None,
    }
}

/// Detect the delimiter of a data line: prefer tab, then semicolon, then comma,
/// else fall back to whitespace (rendered as a single space split downstream).
fn detect_delimiter(line: &str) -> char {
    if line.contains('\t') {
        '\t'
    } else if line.contains(';') {
        ';'
    } else if line.contains(',') {
        ','
    } else {
        ' '
    }
}

/// Split one line into trimmed owned cells. For the comma delimiter, honor RFC-4180
/// quoting; for a space delimiter, collapse runs of whitespace. Other delimiters
/// split literally.
fn split_cells(line: &str, delim: char) -> Vec<String> {
    match delim {
        ',' => parse_csv_line(line),
        ' ' => line.split_whitespace().map(|s| s.to_string()).collect(),
        d => line.split(d).map(|s| s.trim().to_string()).collect(),
    }
}

/// Minimal RFC-4180 field splitter for a single comma-delimited line: honors
/// double-quoted fields, `""` escapes, and commas inside quotes. Mirrors the
/// behavior the `hertzforge-channel-table` CSV module relies on for paste survival.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == ',' {
            out.push(field.trim().to_string());
            field = String::new();
        } else {
            field.push(c);
        }
    }
    out.push(field.trim().to_string());
    out
}

/// Parse a numeric cell, tolerating a leading `+`, surrounding whitespace, and a
/// trailing unit token separated by a space (e.g. `0.04 g2/Hz`). Returns `None` if
/// the leading token is not a finite number.
fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Take the leading numeric token (up to the first space) so `0.04 g2/Hz` works.
    let tok = t.split_whitespace().next().unwrap_or(t);
    let tok = tok.trim_start_matches('+');
    match tok.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => None,
    }
}

/// The recognizable columns for import header aliasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Freq,
    Psd,
    Slope,
    Alarm,
    AlarmMinus,
    AlarmPlus,
    Abort,
    AbortMinus,
    AbortPlus,
}

impl Column {
    /// Resolve a header cell (case/space/underscore-insensitive) to a column. The
    /// `-`/`+` suffixes for alarm/abort are preserved before normalization so
    /// `alarm-` and `alarm+` map to the minus/plus variants.
    fn from_header(raw: &str) -> Option<Column> {
        let t = raw.trim();
        // Preserve a trailing +/- sign for the band columns.
        let (base, sign) = if let Some(stripped) = t.strip_suffix('-') {
            (stripped, Some('-'))
        } else if let Some(stripped) = t.strip_suffix('+') {
            (stripped, Some('+'))
        } else {
            (t, None)
        };
        let n = normalize(base);
        match (n.as_str(), sign) {
            ("freq", _) | ("frequency", _) | ("hz", _) | ("f", _) => Some(Column::Freq),
            ("psd", _)
            | ("level", _)
            | ("g2", _)
            | ("g2hz", _)
            | ("asd", _)
            | ("accel", _)
            | ("acceleration", _)
            | ("ghz", _) => Some(Column::Psd),
            ("slope", _) | ("dboct", _) | ("db", _) | ("oct", _) | ("dbperoct", _) => {
                Some(Column::Slope)
            }
            ("alarm", Some('-')) | ("alarmminus", _) | ("alarmlo", _) | ("alarmlow", _) => {
                Some(Column::AlarmMinus)
            }
            ("alarm", Some('+')) | ("alarmplus", _) | ("alarmhi", _) | ("alarmhigh", _) => {
                Some(Column::AlarmPlus)
            }
            ("alarm", None) | ("alarmdb", None) => Some(Column::Alarm),
            ("abort", Some('-')) | ("abortminus", _) | ("abortlo", _) | ("abortlow", _) => {
                Some(Column::AbortMinus)
            }
            ("abort", Some('+')) | ("abortplus", _) | ("aborthi", _) | ("aborthigh", _) => {
                Some(Column::AbortPlus)
            }
            ("abort", None) | ("abortdb", None) => Some(Column::Abort),
            _ => None,
        }
    }
}

/// Normalize a header cell: drop the part after `(` (unit suffix), lowercase, and
/// strip spaces / underscores / `/` / `^` / `.` / `#`.
fn normalize(raw: &str) -> String {
    let base = raw.split('(').next().unwrap_or(raw);
    base.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '/' | '^' | '.' | '#'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// --- alarm / abort frequency-domain envelope ---------------------

/// The alarm/abort envelope resolved onto a line grid: per-line reference PSD plus
/// the alarm and abort bands (as linear PSD ceilings/floors). This is the NEW
/// frequency-domain envelope, distinct from the per-channel tri-field kinematic abort.
/// Constructed by [`Envelope::from_table`].
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    /// The reference line spectrum (interpolated from the table).
    pub reference: LineSpectrum,
    /// Per-line alarm upper bound (linear `g^2/Hz`); `f64::INFINITY` out-of-band.
    pub alarm_hi: Vec<f64>,
    /// Per-line alarm lower bound (linear `g^2/Hz`); `0.0` out-of-band.
    pub alarm_lo: Vec<f64>,
    /// Per-line abort upper bound (linear `g^2/Hz`); `f64::INFINITY` out-of-band.
    pub abort_hi: Vec<f64>,
    /// Per-line abort lower bound (linear `g^2/Hz`); `0.0` out-of-band.
    pub abort_lo: Vec<f64>,
}

/// One bin that breached a band, for the [`EnvelopeReport`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandBreach {
    pub line: usize,
    pub freq_hz: f64,
    /// Signed dB of the measured PSD relative to the reference at this line
    /// (`10*log10(measured/reference)`). Positive = above reference.
    pub delta_db: f64,
}

/// The result of [`Envelope::check`]: which lines breached alarm / abort, and the
/// worst signed dB deviation seen against each band's reference.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EnvelopeReport {
    pub alarm_bins: Vec<BandBreach>,
    pub abort_bins: Vec<BandBreach>,
    /// Largest-magnitude signed dB deviation among alarm breaches (`None` if none).
    pub worst_alarm_db: Option<f64>,
    /// Largest-magnitude signed dB deviation among abort breaches (`None` if none).
    pub worst_abort_db: Option<f64>,
}

impl EnvelopeReport {
    /// True if any abort bin breached (the hard stop condition).
    pub fn aborted(&self) -> bool {
        !self.abort_bins.is_empty()
    }
    /// True if any alarm bin breached.
    pub fn alarmed(&self) -> bool {
        !self.alarm_bins.is_empty()
    }
}

impl Envelope {
    /// Build the envelope from a table on the grid `(df_hz, f_max)`. Rows carry
    /// per-segment alarm/abort overrides; where a row has none, the global default
    /// applies. Each band is a `(minus_db, plus_db)` pair of positive dB magnitudes:
    /// the alarm/abort HIGH bound is `reference * 10^(+plus_db/10)` and the LOW bound
    /// is `reference * 10^(-minus_db/10)`.
    ///
    /// A line's band is taken from the enclosing segment's LOWER anchor row override
    /// if present, else the global default. Out-of-band lines get `+inf` high / `0`
    /// low bounds (no breach possible), matching the reference's `0.0` out-of-band.
    pub fn from_table(
        table: &BreakpointTable,
        global_alarm_db: (f64, f64),
        global_abort_db: (f64, f64),
    ) -> Result<Envelope, BreakpointError> {
        // Default grid: df = 1 Hz spanning to the last anchor frequency.
        Self::from_table_on_grid(
            table,
            1.0,
            table.last_freq(),
            global_alarm_db,
            global_abort_db,
        )
    }

    /// Build the envelope on an explicit grid. This is the primary constructor; the
    /// convenience [`Envelope::from_table`] derives a default grid from the table.
    pub fn from_table_on_grid(
        table: &BreakpointTable,
        df_hz: f64,
        f_max: f64,
        global_alarm_db: (f64, f64),
        global_abort_db: (f64, f64),
    ) -> Result<Envelope, BreakpointError> {
        let reference = table.interpolate(df_hz, f_max)?;
        let anchors = table.resolve()?;
        let n = reference.psd.len();
        let mut alarm_hi = vec![f64::INFINITY; n];
        let mut alarm_lo = vec![0.0f64; n];
        let mut abort_hi = vec![f64::INFINITY; n];
        let mut abort_lo = vec![0.0f64; n];

        // Map each row's frequency to its per-row overrides for the segment that
        // STARTS at that row (the lower anchor governs the segment above it).
        for (k, r) in reference.psd.iter().enumerate() {
            let f = reference.freq(k);
            if *r <= 0.0 {
                continue; // out-of-band -> leave (+inf / 0)
            }
            // find the enclosing segment's lower anchor index.
            let mut alarm = global_alarm_db;
            let mut abort = global_abort_db;
            for (i, w) in anchors.windows(2).enumerate() {
                if f >= w[0].freq_hz && f <= w[1].freq_hz {
                    if let Some(a) = table.rows[i].alarm_db {
                        alarm = a;
                    }
                    if let Some(a) = table.rows[i].abort_db {
                        abort = a;
                    }
                    break;
                }
            }
            alarm_hi[k] = *r * 10f64.powf(alarm.1 / 10.0);
            alarm_lo[k] = *r * 10f64.powf(-alarm.0 / 10.0);
            abort_hi[k] = *r * 10f64.powf(abort.1 / 10.0);
            abort_lo[k] = *r * 10f64.powf(-abort.0 / 10.0);
        }

        Ok(Envelope {
            reference,
            alarm_hi,
            alarm_lo,
            abort_hi,
            abort_lo,
        })
    }

    /// Check a measured line spectrum against the envelope. The measured spectrum
    /// must share the envelope reference's grid (`f0`, `df`, length); mismatched
    /// grids return an empty report with the mismatch noted in `worst_*` = `None`.
    ///
    /// A line breaches the alarm band if its measured PSD is above `alarm_hi` or
    /// below `alarm_lo` (and the reference is in-band there). Abort likewise. The
    /// worst deviation is the largest-magnitude signed dB (`10*log10(meas/ref)`)
    /// among the breaching lines.
    pub fn check(&self, measured: &LineSpectrum) -> EnvelopeReport {
        let mut report = EnvelopeReport::default();
        if measured.df_hz != self.reference.df_hz
            || measured.f0_hz != self.reference.f0_hz
            || measured.psd.len() != self.reference.psd.len()
        {
            return report; // grid mismatch: caller error, no breach claims
        }
        for k in 0..self.reference.psd.len() {
            let refv = self.reference.psd[k];
            if refv <= 0.0 {
                continue; // out-of-band
            }
            let m = measured.psd[k];
            let delta_db = if m > 0.0 {
                10.0 * (m / refv).log10()
            } else {
                f64::NEG_INFINITY
            };
            let f = self.reference.freq(k);
            if m > self.abort_hi[k] || m < self.abort_lo[k] {
                report.abort_bins.push(BandBreach {
                    line: k,
                    freq_hz: f,
                    delta_db,
                });
            } else if m > self.alarm_hi[k] || m < self.alarm_lo[k] {
                report.alarm_bins.push(BandBreach {
                    line: k,
                    freq_hz: f,
                    delta_db,
                });
            }
        }
        report.worst_alarm_db = worst_signed(&report.alarm_bins);
        report.worst_abort_db = worst_signed(&report.abort_bins);
        report
    }
}

/// The largest-magnitude signed dB among a set of breaches.
fn worst_signed(bins: &[BandBreach]) -> Option<f64> {
    bins.iter()
        .map(|b| b.delta_db)
        .filter(|d| d.is_finite())
        .fold(None, |acc: Option<f64>, d| match acc {
            Some(a) if a.abs() >= d.abs() => Some(a),
            _ => Some(d),
        })
        // if every breach was -inf (measured zero), report -inf explicitly.
        .or_else(|| {
            if bins.iter().any(|b| b.delta_db == f64::NEG_INFINITY) {
                Some(f64::NEG_INFINITY)
            } else {
                None
            }
        })
}

// --- private helpers used by Envelope::from_table default-grid convenience -----

impl BreakpointTable {
    /// Last (highest) frequency in the resolved table, or `0.0` if empty/invalid.
    /// Used by the convenience [`Envelope::from_table`] to size its default grid.
    fn last_freq(&self) -> f64 {
        self.resolve()
            .ok()
            .and_then(|a| a.last().map(|x| x.freq_hz))
            .unwrap_or(0.0)
    }
}

// --- free functions -----------------------------------------------------------

/// gRMS of a line spectrum by direct discrete integration:
/// `sqrt( sum_k psd[k] * df )`. This is the estimator the runtime uses on measured
/// PSDs. It must agree with [`BreakpointTable::grms_of_table`] to within a small
/// tolerance on a sufficiently fine grid (verified in tests).
pub fn grms(spectrum: &LineSpectrum) -> f64 {
    let sum: f64 = spectrum.psd.iter().filter(|&&p| p > 0.0).sum();
    (sum * spectrum.df_hz).max(0.0).sqrt()
}

#[cfg(test)]
mod tests;
