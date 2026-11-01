//! Sine profile grammar, SHM kinematics, crossovers, alarm/abort bands, and
//! hardened import/export.

use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;
use std::fmt;

/// Standard gravity in inch units, `in/s^2`. Re-exported from
/// `eigen_dsp::integrate` (one constant shared with the DSP crate).
pub use eigen_dsp::integrate::G_TO_IN_S2;

/// Global alarm band default, `(minus_db, plus_db)` positive magnitudes.
pub const DEFAULT_ALARM_DB: (f64, f64) = (1.5, 1.5);

/// Global abort band default, `(minus_db, plus_db)` positive magnitudes.
pub const DEFAULT_ABORT_DB: (f64, f64) = (3.0, 3.0);

/// Amplitude-discontinuity threshold at a segment join, dB. Joins between
/// differing-quantity segments whose acceleration-domain levels disagree by more
/// than this are flagged as a validation warning.
pub const JOIN_DISCONTINUITY_WARN_DB: f64 = 0.5;

// =============================================================================
// Grammar
// =============================================================================

/// The physical quantity a profile row specifies. Sine profiles mix quantities
/// row-to-row (classic low-frequency displacement ramp into an acceleration
/// plateau); each maps to acceleration via SHM at the excitation frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantity {
    /// Acceleration, `g` peak.
    AccelGPk,
    /// Velocity, `in/s` peak.
    VelocityIpsPk,
    /// Displacement, `in` peak-to-peak.
    DisplacementInPp,
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Quantity::AccelGPk => write!(f, "g pk"),
            Quantity::VelocityIpsPk => write!(f, "in/s pk"),
            Quantity::DisplacementInPp => write!(f, "in pk-pk"),
        }
    }
}

/// One sine profile row: a frequency breakpoint carrying a quantity + value and
/// optional per-row alarm/abort `(minus_db, plus_db)` overrides (positive dB
/// magnitudes; `None` -> the profile's global band applies).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProfileRow {
    pub freq_hz: f64,
    pub quantity: Quantity,
    pub value: f64,
    /// `(minus_db, plus_db)` alarm band override, positive magnitudes.
    #[serde(default)]
    pub alarm_db: Option<(f64, f64)>,
    /// `(minus_db, plus_db)` abort band override, positive magnitudes.
    #[serde(default)]
    pub abort_db: Option<(f64, f64)>,
}

impl ProfileRow {
    /// An acceleration row (`g` pk) with no band overrides.
    pub fn accel(freq_hz: f64, g_pk: f64) -> Self {
        ProfileRow {
            freq_hz,
            quantity: Quantity::AccelGPk,
            value: g_pk,
            alarm_db: None,
            abort_db: None,
        }
    }

    /// A velocity row (`in/s` pk) with no band overrides.
    pub fn velocity(freq_hz: f64, ips_pk: f64) -> Self {
        ProfileRow {
            freq_hz,
            quantity: Quantity::VelocityIpsPk,
            value: ips_pk,
            alarm_db: None,
            abort_db: None,
        }
    }

    /// A displacement row (`in` pk-pk) with no band overrides.
    pub fn displacement(freq_hz: f64, in_pp: f64) -> Self {
        ProfileRow {
            freq_hz,
            quantity: Quantity::DisplacementInPp,
            value: in_pp,
            alarm_db: None,
            abort_db: None,
        }
    }

    /// Builder-style per-row alarm override.
    pub fn with_alarm_db(mut self, minus: f64, plus: f64) -> Self {
        self.alarm_db = Some((minus, plus));
        self
    }

    /// Builder-style per-row abort override.
    pub fn with_abort_db(mut self, minus: f64, plus: f64) -> Self {
        self.abort_db = Some((minus, plus));
        self
    }
}

/// The sine profile: ordered rows plus global alarm/abort bands. Defaults:
/// alarm `(1.5, 1.5)` dB, abort `(3.0, 3.0)` dB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SineProfile {
    pub label: String,
    pub rows: Vec<ProfileRow>,
    /// Global `(minus_db, plus_db)` alarm band, positive magnitudes.
    pub global_alarm_db: (f64, f64),
    /// Global `(minus_db, plus_db)` abort band, positive magnitudes.
    pub global_abort_db: (f64, f64),
}

impl Default for SineProfile {
    fn default() -> Self {
        SineProfile {
            label: String::new(),
            rows: Vec::new(),
            global_alarm_db: DEFAULT_ALARM_DB,
            global_abort_db: DEFAULT_ABORT_DB,
        }
    }
}

// =============================================================================
// Errors / warnings / validation report (mirrors eigen-breakpoints)
// =============================================================================

/// Hard errors that make a profile unusable.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileError {
    /// A sine profile needs at least two rows (one segment). Carries the count.
    TooFewRows(usize),
    /// A frequency is not strictly positive (SHM conversions and log-log
    /// interpolation need `f > 0`).
    NonPositiveFreq { row: usize, freq_hz: f64 },
    /// Frequencies are not strictly increasing.
    NonMonotonicFreq { row: usize, freq_hz: f64 },
    /// A row value is not strictly positive (a sine level of zero or below has
    /// no dB band and no log-log representation).
    NonPositiveValue { row: usize, value: f64 },
    /// An imported row populated more than one quantity column (the
    /// precedence rule: exactly ONE of accel / velocity / displacement per row).
    MultipleQuantities { row: usize, columns: Vec<String> },
    /// Parse failure from an import path.
    Parse(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::TooFewRows(n) => {
                write!(f, "sine profile needs at least 2 rows (got {n})")
            }
            ProfileError::NonPositiveFreq { row, freq_hz } => write!(
                f,
                "row {row}: frequency {freq_hz} Hz must be strictly positive"
            ),
            ProfileError::NonMonotonicFreq { row, freq_hz } => write!(
                f,
                "row {row}: frequency {freq_hz} Hz is not strictly greater than the previous row"
            ),
            ProfileError::NonPositiveValue { row, value } => {
                write!(f, "row {row}: value {value} must be strictly positive")
            }
            ProfileError::MultipleQuantities { row, columns } => write!(
                f,
                "row {row}: more than one quantity column populated ({}); exactly one of \
                 accel / velocity / displacement may be set per row",
                columns.join(", ")
            ),
            ProfileError::Parse(s) => write!(f, "parse error: {s}"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// Non-fatal findings from [`SineProfile::validate_report`].
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileWarning {
    /// A row's effective alarm band is wider than its effective abort band on at
    /// least one side (alarm should trip before abort). Only emitted for rows
    /// that carry an override; the all-global case is
    /// [`ProfileWarning::GlobalAlarmWiderThanAbort`], reported once.
    AlarmWiderThanAbort { row: usize },
    /// The global alarm band is wider than the global abort band on a side.
    GlobalAlarmWiderThanAbort,
    /// Amplitude discontinuity above [`JOIN_DISCONTINUITY_WARN_DB`] at the
    /// segment join STARTING the given row, measured by converting both sides to
    /// acceleration at the join frequency. `delta_db` is signed:
    /// `20*log10(left / right)`.
    JoinDiscontinuity { row: usize, delta_db: f64 },
    /// The import carried a `units` column, which is accepted and IGNORED (row
    /// quantities are fixed by the column the value appears in).
    UnitsColumnIgnored,
}

impl fmt::Display for ProfileWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileWarning::AlarmWiderThanAbort { row } => write!(
                f,
                "row {row}: alarm band is wider than the abort band (alarm should trip first)"
            ),
            ProfileWarning::GlobalAlarmWiderThanAbort => write!(
                f,
                "global alarm band is wider than the global abort band (alarm should trip first)"
            ),
            ProfileWarning::JoinDiscontinuity { row, delta_db } => write!(
                f,
                "row {row}: amplitude discontinuity of {delta_db:.2} dB at the segment join \
                 (> {JOIN_DISCONTINUITY_WARN_DB} dB); consider snapping the boundary to the \
                 computed crossover"
            ),
            ProfileWarning::UnitsColumnIgnored => {
                write!(
                    f,
                    "a 'units' column was present and ignored (quantity is fixed by the column)"
                )
            }
        }
    }
}

/// Full validation result: hard errors + non-fatal warnings, mirroring
/// `eigen-breakpoints::ValidationReport`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationReport {
    pub errors: Vec<ProfileError>,
    pub warnings: Vec<ProfileWarning>,
}

impl ValidationReport {
    /// True when there are no hard errors (warnings allowed).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

// =============================================================================
// SHM kinematics (free functions, shared by evaluation + feasibility)
// =============================================================================

/// Convert a profile quantity to acceleration in `g` peak at frequency `f_hz`
/// via SHM: `a[in/s^2] = (2 pi f) * v = (2 pi f)^2 * (d_pp / 2)`, then
/// `a_g = a / G_TO_IN_S2`. Domain: `f_hz > 0` (out-of-domain inputs return
/// non-finite values; profile evaluation guards frequency before calling).
pub fn accel_g_pk_of(quantity: Quantity, value: f64, f_hz: f64) -> f64 {
    match quantity {
        Quantity::AccelGPk => value,
        Quantity::VelocityIpsPk => TAU * f_hz * value / G_TO_IN_S2,
        Quantity::DisplacementInPp => (TAU * f_hz).powi(2) * (value / 2.0) / G_TO_IN_S2,
    }
}

/// Peak velocity (`in/s`) implied by an acceleration target of `a_g_pk` g at
/// `f_hz`: `v = a_g * G_TO_IN_S2 / (2 pi f)`. Domain: `f_hz > 0`.
pub fn velocity_ips_pk_from_accel(a_g_pk: f64, f_hz: f64) -> f64 {
    a_g_pk * G_TO_IN_S2 / (TAU * f_hz)
}

/// Peak-to-peak displacement (`in`) implied by an acceleration target of
/// `a_g_pk` g at `f_hz`: `d_pp = 2 * a_g * G_TO_IN_S2 / (2 pi f)^2`.
/// Domain: `f_hz > 0`.
pub fn displacement_in_pp_from_accel(a_g_pk: f64, f_hz: f64) -> f64 {
    2.0 * a_g_pk * G_TO_IN_S2 / (TAU * f_hz).powi(2)
}

/// Log-log interpolation of a same-quantity segment value at `f` on the segment
/// joining `(f1, v1)` and `(f2, v2)`: `v(f) = v1 * (f/f1)^n` with
/// `n = ln(v2/v1) / ln(f2/f1)`. Equal endpoint values are the constant case.
/// Degenerate non-positive endpoints (forbidden by validation) return the
/// nearer endpoint value.
fn loglog_value_at(f: f64, f1: f64, v1: f64, f2: f64, v2: f64) -> f64 {
    if v1 <= 0.0 || v1.is_nan() || v2 <= 0.0 || v2.is_nan() {
        return if (f - f1).abs() <= (f2 - f).abs() {
            v1
        } else {
            v2
        };
    }
    if v1 == v2 {
        return v1; // constant fast path (avoids powf noise on flat segments)
    }
    let n = (v2 / v1).ln() / (f2 / f1).ln();
    v1 * (f / f1).powf(n)
}

// =============================================================================
// Percent <-> dB band conversion (ASYMMETRIC)
// =============================================================================

/// Convert a symmetric +/- percent tolerance to the ASYMMETRIC `(minus_db,
/// plus_db)` pair: `minus = -20*log10(1 - p/100)`,
/// `plus = 20*log10(1 + p/100)`, both returned as positive magnitudes. The MIL
/// preset +/-10% maps to `(0.9151, 0.8279)` dB. `pct <= 0` (or NaN) returns
/// `(0.0, 0.0)`; `pct >= 100` has an unbounded minus side (`+inf`).
pub fn pct_to_db(pct: f64) -> (f64, f64) {
    if pct <= 0.0 || pct.is_nan() {
        return (0.0, 0.0);
    }
    let minus = if pct >= 100.0 {
        f64::INFINITY
    } else {
        -20.0 * (1.0 - pct / 100.0).log10()
    };
    let plus = 20.0 * (1.0 + pct / 100.0).log10();
    (minus, plus)
}

// =============================================================================
// Crossovers (Irvine)
// =============================================================================

/// Crossover frequency between two constant-valued adjacent segments of
/// DIFFERENT quantities: the frequency where both specs demand the same motion
/// (Irvine). With `a` in `in/s^2` (`a = g_pk * G_TO_IN_S2`), `v` in `in/s` pk,
/// and `d0 = d_pp / 2` in `in` (zero-to-peak):
///
/// - accel / displacement: `f = (1 / 2 pi) * sqrt(a / d0)`
/// - accel / velocity:     `f = a / (2 pi v)`
/// - velocity / displacement: `f = v / (2 pi d0)`
///
/// Returns `None` for same-quantity rows or non-positive / non-finite values.
pub fn crossover_hz(left: &ProfileRow, right: &ProfileRow) -> Option<f64> {
    if left.quantity == right.quantity {
        return None;
    }
    if left.value <= 0.0 || left.value.is_nan() || right.value <= 0.0 || right.value.is_nan() {
        return None;
    }
    let mut a_in_s2: Option<f64> = None;
    let mut v_ips: Option<f64> = None;
    let mut d0_in: Option<f64> = None;
    for row in [left, right] {
        match row.quantity {
            Quantity::AccelGPk => a_in_s2 = Some(row.value * G_TO_IN_S2),
            Quantity::VelocityIpsPk => v_ips = Some(row.value),
            Quantity::DisplacementInPp => d0_in = Some(row.value / 2.0),
        }
    }
    let f = match (a_in_s2, v_ips, d0_in) {
        (Some(a), None, Some(d0)) => (a / d0).sqrt() / TAU,
        (Some(a), Some(v), None) => a / (TAU * v),
        (None, Some(v), Some(d0)) => v / (TAU * d0),
        _ => return None,
    };
    if f.is_finite() && f > 0.0 {
        Some(f)
    } else {
        None
    }
}

// =============================================================================
// Profile behavior
// =============================================================================

impl SineProfile {
    /// A profile with a label, no rows, and the default global bands.
    pub fn new(label: impl Into<String>) -> Self {
        SineProfile {
            label: label.into(),
            ..Default::default()
        }
    }

    /// Push a row (builder-style; does not validate).
    pub fn push(&mut self, row: ProfileRow) -> &mut Self {
        self.rows.push(row);
        self
    }

    // --- validation -----------------------------------------------------------

    /// Hard-error gate: returns the first violating error. Use
    /// [`SineProfile::validate_report`] to collect all errors + warnings.
    ///
    /// Hard errors, in check order: at least 2 rows; every frequency strictly
    /// positive; frequencies strictly increasing; every value strictly positive.
    pub fn validate(&self) -> Result<(), ProfileError> {
        match self.validate_report().errors.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Full validation: all hard errors + all non-fatal warnings in one pass.
    /// Warnings: effective alarm band wider than the effective abort band on a
    /// side (per overriding row, plus once for the globals), and amplitude
    /// discontinuity above [`JOIN_DISCONTINUITY_WARN_DB`] at a differing-quantity
    /// segment join (same-quantity joins are continuous by construction of the
    /// log-log interpolation).
    pub fn validate_report(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        if self.rows.len() < 2 {
            report
                .errors
                .push(ProfileError::TooFewRows(self.rows.len()));
        }
        let mut prev_f: Option<f64> = None;
        for (i, row) in self.rows.iter().enumerate() {
            // `<= 0.0 || is_nan()` (not `!(x > 0.0)`) so NaN is caught without a
            // negated partial-order comparison (house clippy rule).
            if row.freq_hz <= 0.0 || row.freq_hz.is_nan() {
                report.errors.push(ProfileError::NonPositiveFreq {
                    row: i,
                    freq_hz: row.freq_hz,
                });
            }
            if let Some(pf) = prev_f {
                if row.freq_hz <= pf || row.freq_hz.is_nan() {
                    report.errors.push(ProfileError::NonMonotonicFreq {
                        row: i,
                        freq_hz: row.freq_hz,
                    });
                }
            }
            if row.value <= 0.0 || row.value.is_nan() {
                report.errors.push(ProfileError::NonPositiveValue {
                    row: i,
                    value: row.value,
                });
            }
            // Alarm wider than abort per side, on the EFFECTIVE (override-else-
            // global) bands, only for rows that carry an override (the all-global
            // case is reported once below).
            if row.alarm_db.is_some() || row.abort_db.is_some() {
                let alarm = row.alarm_db.unwrap_or(self.global_alarm_db);
                let abort = row.abort_db.unwrap_or(self.global_abort_db);
                if alarm.0 > abort.0 || alarm.1 > abort.1 {
                    report
                        .warnings
                        .push(ProfileWarning::AlarmWiderThanAbort { row: i });
                }
            }
            prev_f = Some(row.freq_hz);
        }
        if self.global_alarm_db.0 > self.global_abort_db.0
            || self.global_alarm_db.1 > self.global_abort_db.1
        {
            report
                .warnings
                .push(ProfileWarning::GlobalAlarmWiderThanAbort);
        }
        // Join-discontinuity warnings only make sense on a structurally sound
        // profile (positive, increasing frequencies; positive values).
        if report.errors.is_empty() {
            for (i, w) in self.rows.windows(2).enumerate() {
                let (l, r) = (&w[0], &w[1]);
                if l.quantity == r.quantity {
                    continue; // interpolated: continuous by construction
                }
                let f = r.freq_hz;
                let left_a = accel_g_pk_of(l.quantity, l.value, f);
                let right_a = accel_g_pk_of(r.quantity, r.value, f);
                if left_a > 0.0 && right_a > 0.0 {
                    let delta_db = 20.0 * (left_a / right_a).log10();
                    if delta_db.abs() > JOIN_DISCONTINUITY_WARN_DB {
                        report.warnings.push(ProfileWarning::JoinDiscontinuity {
                            row: i + 1,
                            delta_db,
                        });
                    }
                }
            }
        }
        report
    }

    // --- evaluation --------------------------------------------------------

    /// Index of the segment governing `f_hz`: segment `i` spans
    /// `[row_i.freq, row_{i+1}.freq)`, and the band's upper edge (`f == last
    /// row's freq`) belongs to the final segment. `None` out of band, on NaN, or
    /// with fewer than 2 rows. Defined behavior assumes a validated profile.
    fn segment_index(&self, f_hz: f64) -> Option<usize> {
        if self.rows.len() < 2 || f_hz.is_nan() {
            return None;
        }
        let first = self.rows[0].freq_hz;
        let last = self.rows[self.rows.len() - 1].freq_hz;
        if !(first..=last).contains(&f_hz) {
            return None;
        }
        for (i, w) in self.rows.windows(2).enumerate() {
            if (w[0].freq_hz..w[1].freq_hz).contains(&f_hz) {
                return Some(i);
            }
        }
        Some(self.rows.len() - 2) // f == band upper edge
    }

    /// The target at `f_hz` in the GOVERNING quantity (the enclosing segment's
    /// left row): log-log interpolated when the segment's endpoint rows share
    /// the quantity, else the left row's constant value. `None` out of band.
    pub fn target_in_quantity(&self, f_hz: f64) -> Option<(Quantity, f64)> {
        let i = self.segment_index(f_hz)?;
        let (l, r) = (&self.rows[i], &self.rows[i + 1]);
        if l.quantity == r.quantity {
            Some((
                l.quantity,
                loglog_value_at(f_hz, l.freq_hz, l.value, r.freq_hz, r.value),
            ))
        } else {
            Some((l.quantity, l.value))
        }
    }

    /// The acceleration-domain control target at `f_hz`, `g` peak (SHM
    /// conversion of the governing quantity). `None` out of band.
    pub fn target_accel_g_pk(&self, f_hz: f64) -> Option<f64> {
        let (q, v) = self.target_in_quantity(f_hz)?;
        Some(accel_g_pk_of(q, v, f_hz))
    }

    /// Peak velocity (`in/s`) implied by the acceleration target at `f_hz`.
    pub fn velocity_ips_pk(&self, f_hz: f64) -> Option<f64> {
        Some(velocity_ips_pk_from_accel(
            self.target_accel_g_pk(f_hz)?,
            f_hz,
        ))
    }

    /// Peak-to-peak displacement (`in`) implied by the acceleration target at
    /// `f_hz`.
    pub fn displacement_in_pp(&self, f_hz: f64) -> Option<f64> {
        Some(displacement_in_pp_from_accel(
            self.target_accel_g_pk(f_hz)?,
            f_hz,
        ))
    }

    // --- crossover snapping -----------------------------------------------------

    /// Recompute the boundary frequency of every differing-quantity adjacent
    /// row pair to the analytic crossover ([`crossover_hz`]), making those joins
    /// amplitude-continuous. A boundary is only moved when the crossover
    /// preserves strict frequency monotonicity against its neighbors; otherwise
    /// the row is left unchanged. Returns the indices of the rows whose
    /// frequency was snapped.
    pub fn snap_rows_to_crossovers(&mut self) -> Vec<usize> {
        let mut snapped = Vec::new();
        if self.rows.len() < 2 {
            return snapped;
        }
        for i in 0..self.rows.len() - 1 {
            if self.rows[i].quantity == self.rows[i + 1].quantity {
                continue;
            }
            let Some(fc) = crossover_hz(&self.rows[i], &self.rows[i + 1]) else {
                continue;
            };
            let lo = self.rows[i].freq_hz;
            let hi = self
                .rows
                .get(i + 2)
                .map(|r| r.freq_hz)
                .unwrap_or(f64::INFINITY);
            if fc > lo && fc < hi && fc != self.rows[i + 1].freq_hz {
                self.rows[i + 1].freq_hz = fc;
                snapped.push(i + 1);
            }
        }
        snapped
    }

    // --- alarm / abort bands ------------------------------------------------------

    /// Alarm `(low, high)` bounds on the acceleration target at `f_hz`, `g`
    /// peak: `target * 10^(-minus/20)` and `target * 10^(+plus/20)`, with the
    /// `(minus, plus)` dB pair taken from the enclosing segment's LEFT row
    /// override when present, else the global band. `None` out of band.
    pub fn alarm_bounds(&self, f_hz: f64) -> Option<(f64, f64)> {
        self.band_bounds(f_hz, |row| row.alarm_db, self.global_alarm_db)
    }

    /// Abort `(low, high)` bounds on the acceleration target at `f_hz`, `g`
    /// peak. Same override rule as [`SineProfile::alarm_bounds`].
    pub fn abort_bounds(&self, f_hz: f64) -> Option<(f64, f64)> {
        self.band_bounds(f_hz, |row| row.abort_db, self.global_abort_db)
    }

    fn band_bounds(
        &self,
        f_hz: f64,
        pick: impl Fn(&ProfileRow) -> Option<(f64, f64)>,
        global: (f64, f64),
    ) -> Option<(f64, f64)> {
        let i = self.segment_index(f_hz)?;
        let target = self.target_accel_g_pk(f_hz)?;
        let (minus, plus) = pick(&self.rows[i]).unwrap_or(global);
        Some((
            target * 10f64.powf(-minus / 20.0),
            target * 10f64.powf(plus / 20.0),
        ))
    }

    // --- imports (mirror eigen-breakpoints hardening) -----------------------------

    /// Parse a tab-separated profile (header aliasing + comment tolerance). See
    /// [`SineProfile::from_delimited`].
    pub fn from_tsv(s: &str) -> Result<Self, ProfileError> {
        Self::from_delimited(s, Some('\t'))
    }

    /// Parse a comma-separated profile with RFC-4180 quoting (quoted fields,
    /// `""` escapes, CRLF line endings).
    pub fn from_csv(s: &str) -> Result<Self, ProfileError> {
        Self::from_delimited(s, Some(','))
    }

    /// Clipboard paste with automatic delimiter detection (tab, comma,
    /// semicolon, or whitespace). Header aliasing, comment lines (`#`, `//`),
    /// and the two-column headerless `(freq, accel g pk)` shorthand are all
    /// tolerated.
    pub fn paste_from_clipboard(s: &str) -> Result<Self, ProfileError> {
        Self::from_delimited(s, None)
    }

    /// The shared import path. `delim = None` auto-detects the delimiter from
    /// the first data line. Recognized columns (case-insensitive header
    /// aliasing; unit suffixes in parentheses are dropped):
    ///
    /// - frequency: `freq`, `frequency`, `hz`, `f` (also `freq_hz`)
    /// - acceleration (`g` pk): `accel`, `acceleration`, `g`, `gpk`, `a`
    ///   (also `accel_g_pk`)
    /// - velocity (`in/s` pk): `vel`, `velocity`, `ips`, `in_s`, `v`
    ///   (also `vel_ips_pk`)
    /// - displacement (`in` pk-pk): `disp`, `displacement`, `in`, `inch`, `pp`,
    ///   `d` (also `disp_in_pp`)
    /// - alarm: `alarm` / `alarm_db` (symmetric dB) or `alarm-` / `alarm+`
    ///   (also `alarm_minus` / `alarm_plus`, `_db`-suffixed forms), or
    ///   `alarm_pct` (symmetric percent, converted ASYMMETRICALLY per
    ///   [`pct_to_db`])
    /// - abort: likewise
    /// - `units`: accepted and IGNORED ([`import_with_report`] warns)
    ///
    /// PRECEDENCE RULE: exactly ONE of the three quantity columns may be
    /// non-empty per row; more than one populated is a row error listing the
    /// columns. Headerless input must be exactly two columns, read as
    /// `(freq, accel g pk)`. Numbers tolerate `1e3` / `1E3` exponent notation,
    /// a leading `+`, and leading/trailing whitespace. Comment lines beginning
    /// with `#` or `//` and blank lines are skipped.
    pub fn from_delimited(s: &str, delim: Option<char>) -> Result<Self, ProfileError> {
        parse_delimited(s, delim).map(|(profile, _)| profile)
    }

    // --- exports ------------------------------------------------------------------

    /// Serialize rows to CSV with the canonical header
    /// `freq_hz, accel_g_pk, vel_ips_pk, disp_in_pp, alarm_minus_db,
    /// alarm_plus_db, abort_minus_db, abort_plus_db` - one quantity column
    /// populated per row, band columns empty where the row has no override.
    /// Numbers use Rust's shortest-round-trip float formatting, so re-import
    /// reproduces the rows exactly. The label and global bands are NOT carried
    /// by CSV/TSV; use JSON for the lossless full round-trip.
    pub fn to_csv(&self) -> String {
        self.to_delimited(',')
    }

    /// As [`SineProfile::to_csv`] with a tab delimiter.
    pub fn to_tsv(&self) -> String {
        self.to_delimited('\t')
    }

    fn to_delimited(&self, d: char) -> String {
        const HEADERS: [&str; 8] = [
            "freq_hz",
            "accel_g_pk",
            "vel_ips_pk",
            "disp_in_pp",
            "alarm_minus_db",
            "alarm_plus_db",
            "abort_minus_db",
            "abort_plus_db",
        ];
        let sep = d.to_string();
        let mut out = HEADERS.join(&sep);
        out.push('\n');
        for row in &self.rows {
            let mut cells = vec![String::new(); HEADERS.len()];
            cells[0] = fmt_num(row.freq_hz);
            let qi = match row.quantity {
                Quantity::AccelGPk => 1,
                Quantity::VelocityIpsPk => 2,
                Quantity::DisplacementInPp => 3,
            };
            cells[qi] = fmt_num(row.value);
            if let Some((m, p)) = row.alarm_db {
                cells[4] = fmt_num(m);
                cells[5] = fmt_num(p);
            }
            if let Some((m, p)) = row.abort_db {
                cells[6] = fmt_num(m);
                cells[7] = fmt_num(p);
            }
            out.push_str(&cells.join(&sep));
            out.push('\n');
        }
        out
    }

    /// Serialize to pretty JSON (lossless: label, rows, global bands).
    pub fn to_json(&self) -> Result<String, ProfileError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| ProfileError::Parse(format!("json serialize: {e}")))
    }

    /// Parse from JSON produced by [`SineProfile::to_json`]. Does NOT validate.
    /// Missing global bands default to (1.5, 1.5) / (3.0, 3.0) dB.
    pub fn from_json(s: &str) -> Result<Self, ProfileError> {
        serde_json::from_str(s).map_err(|e| ProfileError::Parse(format!("json parse: {e}")))
    }
}

/// Import a delimited profile AND collect the full validation report (errors +
/// warnings), including the units-column-ignored warning that plain
/// [`SineProfile::from_delimited`] applies silently. This is the entry point a
/// profile editor should use so it can surface everything at once. Structural
/// problems that survive parsing (e.g. non-increasing frequencies) land in
/// `report.errors`; malformed rows (unparseable cells, the multi-quantity
/// precedence violation) fail the parse itself.
pub fn import_with_report(
    s: &str,
    delim: Option<char>,
) -> Result<(SineProfile, ValidationReport), ProfileError> {
    let (profile, units_seen) = parse_delimited(s, delim)?;
    let mut report = profile.validate_report();
    if units_seen {
        report.warnings.push(ProfileWarning::UnitsColumnIgnored);
    }
    Ok((profile, report))
}

// =============================================================================
// Import internals (mirror eigen-breakpoints)
// =============================================================================

/// The recognizable columns for import header aliasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    Freq,
    Accel,
    Vel,
    Disp,
    Alarm,
    AlarmMinus,
    AlarmPlus,
    AlarmPct,
    Abort,
    AbortMinus,
    AbortPlus,
    AbortPct,
    Units,
}

impl Column {
    /// Resolve a header cell (case/space/underscore-insensitive, unit suffix in
    /// parentheses dropped) to a column. Trailing `-` / `+` map alarm/abort to
    /// their minus/plus variants before normalization.
    fn from_header(raw: &str) -> Option<Column> {
        let t = raw.trim();
        let (base, sign) = if let Some(stripped) = t.strip_suffix('-') {
            (stripped, Some('-'))
        } else if let Some(stripped) = t.strip_suffix('+') {
            (stripped, Some('+'))
        } else {
            (t, None)
        };
        let n = normalize(base);
        match (n.as_str(), sign) {
            ("freq", _) | ("frequency", _) | ("hz", _) | ("f", _) | ("freqhz", _) => {
                Some(Column::Freq)
            }
            ("accel", _)
            | ("acceleration", _)
            | ("g", _)
            | ("gpk", _)
            | ("a", _)
            | ("accelgpk", _) => Some(Column::Accel),
            ("vel", _) | ("velocity", _) | ("ips", _) | ("ins", _) | ("v", _) | ("velipspk", _) => {
                Some(Column::Vel)
            }
            ("disp", _)
            | ("displacement", _)
            | ("in", _)
            | ("inch", _)
            | ("pp", _)
            | ("d", _)
            | ("inpkpk", _)
            | ("dispinpp", _) => Some(Column::Disp),
            ("alarm", Some('-'))
            | ("alarmminus", _)
            | ("alarmminusdb", _)
            | ("alarmlo", _)
            | ("alarmlow", _) => Some(Column::AlarmMinus),
            ("alarm", Some('+'))
            | ("alarmplus", _)
            | ("alarmplusdb", _)
            | ("alarmhi", _)
            | ("alarmhigh", _) => Some(Column::AlarmPlus),
            ("alarm", None) | ("alarmdb", None) => Some(Column::Alarm),
            ("alarmpct", _) | ("alarmpercent", _) | ("alarm%", _) => Some(Column::AlarmPct),
            ("abort", Some('-'))
            | ("abortminus", _)
            | ("abortminusdb", _)
            | ("abortlo", _)
            | ("abortlow", _) => Some(Column::AbortMinus),
            ("abort", Some('+'))
            | ("abortplus", _)
            | ("abortplusdb", _)
            | ("aborthi", _)
            | ("aborthigh", _) => Some(Column::AbortPlus),
            ("abort", None) | ("abortdb", None) => Some(Column::Abort),
            ("abortpct", _) | ("abortpercent", _) | ("abort%", _) => Some(Column::AbortPct),
            ("units", _) | ("unit", _) => Some(Column::Units),
            _ => None,
        }
    }
}

/// The shared parse path. Returns the profile plus whether a `units` column was
/// present (accepted and ignored; [`import_with_report`] surfaces the warning).
fn parse_delimited(s: &str, delim: Option<char>) -> Result<(SineProfile, bool), ProfileError> {
    // Gather non-comment, non-blank logical lines (CRLF tolerated).
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
        return Err(ProfileError::Parse("no data rows".into()));
    }

    let d = match delim {
        Some(c) => c,
        None => detect_delimiter(lines[0]),
    };

    // Header detection: the first line has at least one non-numeric cell in a
    // recognizable header position.
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
        // Headerless: only the two-column (freq, accel g pk) shorthand is
        // unambiguous for sine (which quantity would a third column be?).
        if first_cells.len() != 2 {
            return Err(ProfileError::Parse(format!(
                "headerless input must be two columns (freq, accel g pk); got {} columns - \
                 add a header row to import other layouts",
                first_cells.len()
            )));
        }
        (vec![Some(Column::Freq), Some(Column::Accel)], 0usize)
    };

    if !col_map.iter().any(|c| matches!(c, Some(Column::Freq))) {
        return Err(ProfileError::Parse(
            "no frequency column found (freq / frequency / hz / f)".into(),
        ));
    }
    if !col_map
        .iter()
        .any(|c| matches!(c, Some(Column::Accel | Column::Vel | Column::Disp)))
    {
        return Err(ProfileError::Parse(
            "no quantity column found (accel / velocity / displacement)".into(),
        ));
    }
    let units_seen = col_map.iter().any(|c| matches!(c, Some(Column::Units)));

    let mut profile = SineProfile::default();
    let mut row_i = 0usize;
    for line in &lines[data_start..] {
        let cells = split_cells(line, d);
        if cells.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let mut freq_raw: Option<String> = None;
        // (column name, quantity, raw cell) for every populated quantity cell.
        let mut quantities: Vec<(&'static str, Quantity, String)> = Vec::new();
        let mut alarm_sym: Option<f64> = None;
        let mut alarm_m: Option<f64> = None;
        let mut alarm_p: Option<f64> = None;
        let mut alarm_pct: Option<f64> = None;
        let mut abort_sym: Option<f64> = None;
        let mut abort_m: Option<f64> = None;
        let mut abort_p: Option<f64> = None;
        let mut abort_pct: Option<f64> = None;
        for (i, col) in col_map.iter().enumerate() {
            let Some(col) = col else { continue };
            let raw = cells.get(i).map(|s| s.trim()).unwrap_or("");
            if raw.is_empty() {
                continue;
            }
            match col {
                Column::Freq => freq_raw = Some(raw.to_string()),
                Column::Accel => {
                    quantities.push(("accel", Quantity::AccelGPk, raw.to_string()));
                }
                Column::Vel => {
                    quantities.push(("velocity", Quantity::VelocityIpsPk, raw.to_string()));
                }
                Column::Disp => {
                    quantities.push(("displacement", Quantity::DisplacementInPp, raw.to_string()));
                }
                Column::Alarm => alarm_sym = parse_num(raw),
                Column::AlarmMinus => alarm_m = parse_num(raw),
                Column::AlarmPlus => alarm_p = parse_num(raw),
                Column::AlarmPct => alarm_pct = parse_num(raw),
                Column::Abort => abort_sym = parse_num(raw),
                Column::AbortMinus => abort_m = parse_num(raw),
                Column::AbortPlus => abort_p = parse_num(raw),
                Column::AbortPct => abort_pct = parse_num(raw),
                Column::Units => {} // accepted and ignored
            }
        }
        let freq_raw = freq_raw.ok_or_else(|| {
            ProfileError::Parse(format!("row {row_i}: missing a frequency: {line:?}"))
        })?;
        let freq = parse_num(&freq_raw).ok_or_else(|| {
            ProfileError::Parse(format!("row {row_i}: cannot parse frequency {freq_raw:?}"))
        })?;
        // Precedence rule: exactly one quantity column per row.
        if quantities.len() > 1 {
            return Err(ProfileError::MultipleQuantities {
                row: row_i,
                columns: quantities
                    .iter()
                    .map(|(n, _, _)| (*n).to_string())
                    .collect(),
            });
        }
        let Some((name, quantity, raw)) = quantities.pop() else {
            return Err(ProfileError::Parse(format!(
                "row {row_i}: no quantity value (exactly one of accel / velocity / \
                 displacement must be set): {line:?}"
            )));
        };
        let value = parse_num(&raw).ok_or_else(|| {
            ProfileError::Parse(format!("row {row_i}: cannot parse {name} value {raw:?}"))
        })?;
        profile.rows.push(ProfileRow {
            freq_hz: freq,
            quantity,
            value,
            alarm_db: combine_band(alarm_sym, alarm_m, alarm_p, alarm_pct),
            abort_db: combine_band(abort_sym, abort_m, abort_p, abort_pct),
        });
        row_i += 1;
    }
    Ok((profile, units_seen))
}

/// Turn a symmetric-dB / pair / percent alarm-or-abort spec into the stored
/// `(minus, plus)` tuple. Precedence: explicit minus/plus > symmetric dB >
/// percent (converted asymmetrically per [`pct_to_db`]). A symmetric dB
/// value maps to `(v, v)`.
fn combine_band(
    sym_db: Option<f64>,
    minus: Option<f64>,
    plus: Option<f64>,
    pct: Option<f64>,
) -> Option<(f64, f64)> {
    match (minus, plus, sym_db) {
        (Some(m), Some(p), _) => Some((m.abs(), p.abs())),
        (Some(m), None, Some(s)) => Some((m.abs(), s.abs())),
        (None, Some(p), Some(s)) => Some((s.abs(), p.abs())),
        (Some(m), None, None) => Some((m.abs(), m.abs())),
        (None, Some(p), None) => Some((p.abs(), p.abs())),
        (None, None, Some(s)) => Some((s.abs(), s.abs())),
        (None, None, None) => pct.map(pct_to_db),
    }
}

/// Detect the delimiter of a data line: prefer tab, then semicolon, then comma,
/// else fall back to whitespace.
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

/// Split one line into trimmed owned cells. For the comma delimiter, honor
/// RFC-4180 quoting; for a space delimiter, collapse runs of whitespace. Other
/// delimiters split literally.
fn split_cells(line: &str, delim: char) -> Vec<String> {
    match delim {
        ',' => parse_csv_line(line),
        ' ' => line.split_whitespace().map(|s| s.to_string()).collect(),
        d => line.split(d).map(|s| s.trim().to_string()).collect(),
    }
}

/// Minimal RFC-4180 field splitter for a single comma-delimited line: honors
/// double-quoted fields, `""` escapes, and commas inside quotes. Mirrors
/// `eigen-breakpoints`.
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

/// Parse a numeric cell, tolerating a leading `+`, surrounding whitespace,
/// `1e3` / `1E3` exponent notation, and a trailing unit token separated by a
/// space (e.g. `12 g`). Returns `None` if the leading token is not a finite
/// number.
fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let tok = t.split_whitespace().next().unwrap_or(t);
    let tok = tok.trim_start_matches('+');
    match tok.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => None,
    }
}

/// Normalize a header cell: drop the part after `(` (unit suffix), lowercase,
/// and strip spaces / underscores / `-` / `/` / `^` / `.` / `#`. Trailing
/// `+`/`-` band signs are extracted BEFORE this runs, so stripping interior
/// dashes (`in pk-pk` -> `inpkpk`) does not eat them.
fn normalize(raw: &str) -> String {
    let base = raw.split('(').next().unwrap_or(raw);
    base.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-' | '/' | '^' | '.' | '#'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Shortest-round-trip float formatting (Rust's `Display` for `f64`), used by
/// the CSV/TSV exports so re-import reproduces the rows exactly.
fn fmt_num(v: f64) -> String {
    format!("{v}")
}
