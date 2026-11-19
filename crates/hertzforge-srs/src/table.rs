//! SRS synthesis component tables.
//!
//! Two component types parameterize the two synthesis engines:
//!
//! - [`WaveletComponent`] drives [`crate::wavsyn`]: an odd-half-cycle WAVSYN wavelet
//!   (freq, amplitude, half-cycle count, delay).
//! - [`DampedSineComponent`] drives [`crate::seds`]: an exponentially-decaying sinusoid
//!   (freq, amplitude, decay `zeta`, delay, polarity).
//!
//! Both tables default to one component per band over
//! [`hertzforge_dsp::srs::octave_freqs`] (1/1, 1/3, 1/6, 1/12, or arbitrary
//! fractional-octave spacing), import from TSV/CSV/clipboard with the same hardened
//! conventions as `hertzforge-breakpoints` (header aliasing, RFC-4180 quoting, delimiter
//! auto-detect, comment tolerance), and round-trip through serde JSON (values
//! preserved to f64 shortest-representation precision; see the round-trip note on
//! [`WaveletTable::to_json`]).
//!
//! # Header aliasing (import)
//!
//! - frequency: `freq`, `frequency`, `hz`, `f`
//! - amplitude: `amp`, `amplitude`, `g`, `a`
//! - half-cycles (wavelet): `halfcycles`, `n`, `cycles`, `nhalf`
//! - decay (damped sine): `zeta`, `damping`, `decay`, `z`
//! - delay: `delay`, `t0`, `td`
//! - polarity (damped sine): `polarity`, `pol`, `sign`

use hertzforge_dsp::srs::{octave_freqs, OctaveFraction};
use serde::{Deserialize, Serialize};

/// The default decay ratio (`zeta`) assigned to a [`DampedSineComponent`] created by
/// the per-band default constructor. `zeta = 0.05` (5% of critical) is a common
/// starting decay for SEDS synthesis: enough ring-down that a component is a localized
/// transient, not a near-tone, but light enough to carry the spectral energy the band
/// needs. The iteration only touches amplitude, so this decay persists unless the
/// caller edits the table.
pub const DEFAULT_DAMPED_SINE_ZETA: f64 = 0.05;

/// The default half-cycle count for a per-band [`WaveletComponent`]. `N = 5` is chosen
/// (not the `N = 3` minimum): with the reverse-sine-sweep delay scheme the default table
/// uses, an `N = 3` wavelet is too broadband (only three carrier half-cycles), so its
/// SRS smears across neighboring bands and the per-band ratio iteration does not
/// converge cleanly; `N = 5` narrows each component enough that the composite converges
/// in a handful of iterations while staying odd (so the self-nulling invariant holds)
/// and short. `N = 3` remains VALID (the construction admits any odd `>= 3`); it is just
/// not the default because the swept default would not converge with it. Higher odd
/// counts narrow each component further at the cost of a longer record.
pub const DEFAULT_WAVELET_HALF_CYCLES: u32 = 5;

/// Errors from building or validating a synthesis component or table.
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentError {
    /// A frequency was not finite and strictly positive.
    BadFrequency(f64),
    /// An amplitude was not finite.
    BadAmplitude(f64),
    /// A wavelet half-cycle count was even, zero, or below the minimum (`>= 3`, odd).
    /// Odd half-cycles are what make the wavelet self-null (zero net velocity AND
    /// displacement); an even count breaks that invariant, so it is rejected.
    BadHalfCycles(u32),
    /// A damped-sine decay ratio was not finite or not strictly positive. A
    /// nonpositive decay would not ring down (net velocity/displacement diverge), so it
    /// is rejected.
    BadZeta(f64),
    /// A delay was negative or not finite.
    BadDelay(f64),
    /// A parse/import failure with a human-readable note.
    Parse(String),
}

impl std::fmt::Display for ComponentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentError::BadFrequency(v) => {
                write!(f, "frequency must be finite and > 0 (got {v})")
            }
            ComponentError::BadAmplitude(v) => write!(f, "amplitude must be finite (got {v})"),
            ComponentError::BadHalfCycles(v) => write!(
                f,
                "half-cycles must be odd and >= 3 (got {v}); an even count breaks the \
                 zero-net-velocity/displacement invariant"
            ),
            ComponentError::BadZeta(v) => {
                write!(f, "decay (zeta) must be finite and > 0 (got {v})")
            }
            ComponentError::BadDelay(v) => write!(f, "delay must be finite and >= 0 (got {v})"),
            ComponentError::Parse(s) => write!(f, "component import parse error: {s}"),
        }
    }
}

impl std::error::Error for ComponentError {}

/// The two polarities a damped-sine component may fire with. The polarity multiplies
/// the component amplitude by `+1` or `-1`, letting a table place alternating-sign
/// components (a common SEDS trick to broaden the composite while limiting the peak).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Polarity {
    /// Positive-going first half-cycle (`+1` multiplier). The default.
    #[default]
    Positive,
    /// Negative-going first half-cycle (`-1` multiplier).
    Negative,
}

impl Polarity {
    /// The `+1` / `-1` sign multiplier.
    #[inline]
    pub fn sign(self) -> f64 {
        match self {
            Polarity::Positive => 1.0,
            Polarity::Negative => -1.0,
        }
    }
}

/// One WAVSYN wavelet component.
///
/// The synthesized component is
/// `W_m(t) = amplitude_g * sin(2*pi*freq_hz*t/half_cycles) * sin(2*pi*freq_hz*t)` on
/// `t in [delay_s, delay_s + half_cycles/(2*freq_hz)]`, zero elsewhere. With
/// `half_cycles` ODD (`>= 3`) the component has zero net velocity AND zero net
/// displacement by construction (measured: single-wavelet residual ~1e-15). See
/// [`crate::wavsyn`] for the synthesis and [`WaveletComponent::validate`] for the
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaveletComponent {
    /// Carrier frequency `f_m`, Hz. Must be finite and > 0.
    pub freq_hz: f64,
    /// Peak amplitude scale `A_m`, g. May be any finite value (sign folds into the
    /// wavelet directly; the iteration scales this).
    pub amplitude_g: f64,
    /// Number of half-cycles `N_m` of the modulation envelope. MUST be odd and `>= 3`
    /// (the self-nulling invariant); validated by [`Self::validate`].
    pub half_cycles: u32,
    /// Component start delay `t_m`, seconds. Must be finite and `>= 0`. The
    /// reverse-sine-sweep default assigns the highest-frequency component zero delay
    /// and progressively delays lower-frequency components (see [`WaveletTable::default_over_band`]).
    pub delay_s: f64,
}

impl WaveletComponent {
    /// Build and validate a wavelet component.
    pub fn new(
        freq_hz: f64,
        amplitude_g: f64,
        half_cycles: u32,
        delay_s: f64,
    ) -> Result<Self, ComponentError> {
        let c = WaveletComponent {
            freq_hz,
            amplitude_g,
            half_cycles,
            delay_s,
        };
        c.validate()?;
        Ok(c)
    }

    /// The finite support duration of the component's active lobe (seconds),
    /// `half_cycles / (2 * freq_hz)`, NOT including its delay.
    #[inline]
    pub fn support_s(&self) -> f64 {
        self.half_cycles as f64 / (2.0 * self.freq_hz)
    }

    /// The time at which the component's active lobe ends, `delay_s + support_s`.
    #[inline]
    pub fn end_time_s(&self) -> f64 {
        self.delay_s + self.support_s()
    }

    /// Validate the invariants: finite positive frequency, finite amplitude, ODD
    /// half-cycles `>= 3`, finite nonnegative delay.
    pub fn validate(&self) -> Result<(), ComponentError> {
        if !self.freq_hz.is_finite() || self.freq_hz <= 0.0 {
            return Err(ComponentError::BadFrequency(self.freq_hz));
        }
        if !self.amplitude_g.is_finite() {
            return Err(ComponentError::BadAmplitude(self.amplitude_g));
        }
        if self.half_cycles < 3 || self.half_cycles.is_multiple_of(2) {
            return Err(ComponentError::BadHalfCycles(self.half_cycles));
        }
        if !self.delay_s.is_finite() || self.delay_s < 0.0 {
            return Err(ComponentError::BadDelay(self.delay_s));
        }
        Ok(())
    }
}

/// One SEDS exponentially-decaying-sinusoid component.
///
/// The synthesized component is
/// `a_m(t) = polarity * amplitude_g * exp(-zeta*w_m*(t-delay_s)) * sin(w_m*(t-delay_s))`
/// for `t >= delay_s` (zero before), with `w_m = 2*pi*freq_hz`. A raw damped sine does
/// NOT self-null; the [`crate::seds`] compensation drives the COMPOSITE residuals to
/// ~0 (Smallwood & Nord 1974).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DampedSineComponent {
    /// Ringing frequency `f_m`, Hz. Must be finite and > 0.
    pub freq_hz: f64,
    /// Peak amplitude scale `A_m`, g. May be any finite value; the iteration scales it.
    pub amplitude_g: f64,
    /// Decay ratio `zeta` (fraction of critical). Must be finite and > 0 so the
    /// component rings DOWN (a nonpositive decay would diverge).
    pub zeta: f64,
    /// Component start delay `t_m`, seconds. Must be finite and `>= 0`.
    pub delay_s: f64,
    /// Sign of the first half-cycle.
    pub polarity: Polarity,
}

impl DampedSineComponent {
    /// Build and validate a damped-sine component.
    pub fn new(
        freq_hz: f64,
        amplitude_g: f64,
        zeta: f64,
        delay_s: f64,
        polarity: Polarity,
    ) -> Result<Self, ComponentError> {
        let c = DampedSineComponent {
            freq_hz,
            amplitude_g,
            zeta,
            delay_s,
            polarity,
        };
        c.validate()?;
        Ok(c)
    }

    /// The angular ringing frequency `w_m = 2*pi*freq_hz`.
    #[inline]
    pub fn omega(&self) -> f64 {
        std::f64::consts::TAU * self.freq_hz
    }

    /// Validate the invariants: finite positive frequency, finite amplitude, finite
    /// positive decay, finite nonnegative delay.
    pub fn validate(&self) -> Result<(), ComponentError> {
        if !self.freq_hz.is_finite() || self.freq_hz <= 0.0 {
            return Err(ComponentError::BadFrequency(self.freq_hz));
        }
        if !self.amplitude_g.is_finite() {
            return Err(ComponentError::BadAmplitude(self.amplitude_g));
        }
        if !self.zeta.is_finite() || self.zeta <= 0.0 {
            return Err(ComponentError::BadZeta(self.zeta));
        }
        if !self.delay_s.is_finite() || self.delay_s < 0.0 {
            return Err(ComponentError::BadDelay(self.delay_s));
        }
        Ok(())
    }
}

/// A validated table of WAVSYN wavelet components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveletTable {
    /// The component rows, in table order.
    pub components: Vec<WaveletComponent>,
}

/// A validated table of SEDS damped-sine components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DampedSineTable {
    /// The component rows, in table order.
    pub components: Vec<DampedSineComponent>,
}

impl WaveletTable {
    /// An empty table.
    pub fn new() -> Self {
        WaveletTable {
            components: Vec::new(),
        }
    }

    /// Validate every row.
    pub fn validate(&self) -> Result<(), ComponentError> {
        for c in &self.components {
            c.validate()?;
        }
        Ok(())
    }

    /// Build the default table: one wavelet component per fractional-octave band over
    /// `[f_lo_hz, f_hi_hz]` (via [`octave_freqs`]), each at unity amplitude
    /// and [`DEFAULT_WAVELET_HALF_CYCLES`] half-cycles.
    ///
    /// # Reverse-sine-sweep delays
    ///
    /// The default delays follow the WAVSYN reverse-sine-sweep convention: the
    /// HIGHEST-frequency component fires first (zero delay) and each lower-frequency
    /// component is progressively delayed so their lobes tile in time from high to low
    /// (a downward sweep). The per-component delay is the CUMULATIVE support of all
    /// higher-frequency components, so component `k` (ordered high->low) starts exactly
    /// when component `k-1`'s lobe ends. This keeps the composite compact while still
    /// separating the components, and the odd-half-cycle self-nulling of each component
    /// is preserved regardless of delay (delay is a pure time shift).
    pub fn default_over_band(
        f_lo_hz: f64,
        f_hi_hz: f64,
        fraction: OctaveFraction,
    ) -> Result<Self, ComponentError> {
        let freqs = octave_freqs(f_lo_hz, f_hi_hz, fraction);
        if freqs.is_empty() {
            return Err(ComponentError::Parse(format!(
                "octave_freqs produced no bands for [{f_lo_hz}, {f_hi_hz}] at fraction {fraction:?}"
            )));
        }
        // Order high -> low for the reverse (downward) sine-sweep delay assignment.
        let mut hi_to_lo = freqs.clone();
        hi_to_lo.sort_by(|a, b| b.partial_cmp(a).unwrap());

        let mut components = Vec::with_capacity(hi_to_lo.len());
        let mut cum_delay = 0.0f64;
        for &f in &hi_to_lo {
            let c = WaveletComponent::new(f, 1.0, DEFAULT_WAVELET_HALF_CYCLES, cum_delay)?;
            // The next (lower-frequency) component starts when this lobe ends.
            cum_delay += c.support_s();
            components.push(c);
        }
        // Return in ascending-frequency table order (delays already assigned).
        components.sort_by(|a, b| a.freq_hz.partial_cmp(&b.freq_hz).unwrap());
        Ok(WaveletTable { components })
    }

    /// The component carrier frequencies in table order (used by the iteration to feed
    /// [`hertzforge_shock_common::ShockLawInputs::component_freqs`]).
    pub fn component_freqs(&self) -> Vec<f64> {
        self.components.iter().map(|c| c.freq_hz).collect()
    }

    // --- import + JSON round-trip -------------------------------------------

    /// Parse a tab-separated wavelet table (header aliasing + comment tolerance).
    pub fn from_tsv(s: &str) -> Result<Self, ComponentError> {
        Self::from_delimited(s, Some('\t'))
    }

    /// Parse a comma-separated wavelet table with RFC-4180 quoting.
    pub fn from_csv(s: &str) -> Result<Self, ComponentError> {
        Self::from_delimited(s, Some(','))
    }

    /// Clipboard paste with automatic delimiter detection (tab, comma, semicolon, or
    /// whitespace). Header aliasing and comment lines (`#`, `//`) are tolerated.
    pub fn paste_from_clipboard(s: &str) -> Result<Self, ComponentError> {
        Self::from_delimited(s, None)
    }

    /// The shared wavelet import path. Columns via header aliasing (see module docs):
    /// `freq`, `amp`, `halfcycles`, `delay`. With no header, columns are positional
    /// `(freq, amp, halfcycles, delay)`. A missing `halfcycles` cell defaults to
    /// [`DEFAULT_WAVELET_HALF_CYCLES`]; a missing `delay` defaults to `0`.
    pub fn from_delimited(s: &str, delim: Option<char>) -> Result<Self, ComponentError> {
        let rows = parse_rows(s, delim, WAVELET_POSITIONAL)?;
        let mut components = Vec::with_capacity(rows.len());
        for (line, cells) in rows {
            let freq = cells.get(&Col::Freq).copied().flatten().ok_or_else(|| {
                ComponentError::Parse(format!("wavelet row missing a frequency: {line:?}"))
            })?;
            let amp = cells.get(&Col::Amp).copied().flatten().ok_or_else(|| {
                ComponentError::Parse(format!("wavelet row missing an amplitude: {line:?}"))
            })?;
            let half = match cells.get(&Col::HalfCycles).copied().flatten() {
                Some(v) => {
                    if !(v.is_finite()) || v < 0.0 || v.fract() != 0.0 {
                        return Err(ComponentError::Parse(format!(
                            "wavelet half-cycles must be a whole number: {line:?}"
                        )));
                    }
                    v as u32
                }
                None => DEFAULT_WAVELET_HALF_CYCLES,
            };
            let delay = cells.get(&Col::Delay).copied().flatten().unwrap_or(0.0);
            components.push(WaveletComponent::new(freq, amp, half, delay)?);
        }
        if components.is_empty() {
            return Err(ComponentError::Parse("no wavelet component rows".into()));
        }
        Ok(WaveletTable { components })
    }

    /// Serialize to pretty JSON.
    ///
    /// # Round-trip precision
    ///
    /// The JSON is structurally lossless (every field is preserved). Float VALUES
    /// round-trip to `serde_json`'s shortest-round-trippable representation, which for
    /// a few f64 values re-parses one ULP off the original (a ~1e-13 relative
    /// difference) - `serde_json` does not guarantee bit-exact f64 round-trip. Compare
    /// round-tripped tables with a small relative tolerance, not bit equality, if exact
    /// identity matters.
    pub fn to_json(&self) -> Result<String, ComponentError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| ComponentError::Parse(format!("json serialize: {e}")))
    }

    /// Parse from JSON produced by [`Self::to_json`]. Validates every row.
    pub fn from_json(s: &str) -> Result<Self, ComponentError> {
        let t: WaveletTable = serde_json::from_str(s)
            .map_err(|e| ComponentError::Parse(format!("json parse: {e}")))?;
        t.validate()?;
        Ok(t)
    }
}

impl Default for WaveletTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DampedSineTable {
    /// An empty table.
    pub fn new() -> Self {
        DampedSineTable {
            components: Vec::new(),
        }
    }

    /// Validate every row.
    pub fn validate(&self) -> Result<(), ComponentError> {
        for c in &self.components {
            c.validate()?;
        }
        Ok(())
    }

    /// Build the default table: one damped-sine component per fractional-octave band
    /// over `[f_lo_hz, f_hi_hz]`, each at unity amplitude,
    /// [`DEFAULT_DAMPED_SINE_ZETA`] decay, zero delay, positive polarity.
    ///
    /// Unlike the wavelet default, damped-sine components do NOT get a reverse-sweep
    /// delay: the classic SEDS composite superposes all components at the SAME start
    /// (they ring down together), and the compensation (see [`crate::seds`]) then
    /// nulls the composite residuals. Callers may edit `delay_s` per row afterward.
    pub fn default_over_band(
        f_lo_hz: f64,
        f_hi_hz: f64,
        fraction: OctaveFraction,
    ) -> Result<Self, ComponentError> {
        let freqs = octave_freqs(f_lo_hz, f_hi_hz, fraction);
        if freqs.is_empty() {
            return Err(ComponentError::Parse(format!(
                "octave_freqs produced no bands for [{f_lo_hz}, {f_hi_hz}] at fraction {fraction:?}"
            )));
        }
        let mut components = Vec::with_capacity(freqs.len());
        for &f in &freqs {
            components.push(DampedSineComponent::new(
                f,
                1.0,
                DEFAULT_DAMPED_SINE_ZETA,
                0.0,
                Polarity::Positive,
            )?);
        }
        Ok(DampedSineTable { components })
    }

    /// The component ringing frequencies in table order.
    pub fn component_freqs(&self) -> Vec<f64> {
        self.components.iter().map(|c| c.freq_hz).collect()
    }

    // --- import + JSON round-trip -------------------------------------------

    /// Parse a tab-separated damped-sine table (header aliasing + comment tolerance).
    pub fn from_tsv(s: &str) -> Result<Self, ComponentError> {
        Self::from_delimited(s, Some('\t'))
    }

    /// Parse a comma-separated damped-sine table with RFC-4180 quoting.
    pub fn from_csv(s: &str) -> Result<Self, ComponentError> {
        Self::from_delimited(s, Some(','))
    }

    /// Clipboard paste with automatic delimiter detection.
    pub fn paste_from_clipboard(s: &str) -> Result<Self, ComponentError> {
        Self::from_delimited(s, None)
    }

    /// The shared damped-sine import path. Columns via header aliasing (see module
    /// docs): `freq`, `amp`, `zeta`, `delay`, `polarity`. With no header, columns are
    /// positional `(freq, amp, zeta, delay)`; polarity defaults to positive and a
    /// missing/empty polarity cell (or a positive/negative marker) is tolerated.
    pub fn from_delimited(s: &str, delim: Option<char>) -> Result<Self, ComponentError> {
        let rows = parse_rows(s, delim, DAMPED_SINE_POSITIONAL)?;
        let mut components = Vec::with_capacity(rows.len());
        for (line, cells) in rows {
            let freq = cells.get(&Col::Freq).copied().flatten().ok_or_else(|| {
                ComponentError::Parse(format!("damped-sine row missing a frequency: {line:?}"))
            })?;
            let amp = cells.get(&Col::Amp).copied().flatten().ok_or_else(|| {
                ComponentError::Parse(format!("damped-sine row missing an amplitude: {line:?}"))
            })?;
            let zeta = cells.get(&Col::Zeta).copied().flatten().ok_or_else(|| {
                ComponentError::Parse(format!("damped-sine row missing a decay (zeta): {line:?}"))
            })?;
            let delay = cells.get(&Col::Delay).copied().flatten().unwrap_or(0.0);
            // Polarity: a numeric cell < 0 (or a parsed textual marker resolved during
            // cell scanning to -1.0) is negative; anything else positive.
            let polarity = match cells.get(&Col::Polarity).copied().flatten() {
                Some(v) if v < 0.0 => Polarity::Negative,
                _ => Polarity::Positive,
            };
            components.push(DampedSineComponent::new(freq, amp, zeta, delay, polarity)?);
        }
        if components.is_empty() {
            return Err(ComponentError::Parse(
                "no damped-sine component rows".into(),
            ));
        }
        Ok(DampedSineTable { components })
    }

    /// Serialize to pretty JSON. See the round-trip precision note on
    /// [`WaveletTable::to_json`] (float values round-trip to `serde_json`'s
    /// shortest-representation precision, not necessarily bit-exact).
    pub fn to_json(&self) -> Result<String, ComponentError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| ComponentError::Parse(format!("json serialize: {e}")))
    }

    /// Parse from JSON produced by [`Self::to_json`]. Validates every row.
    pub fn from_json(s: &str) -> Result<Self, ComponentError> {
        let t: DampedSineTable = serde_json::from_str(s)
            .map_err(|e| ComponentError::Parse(format!("json parse: {e}")))?;
        t.validate()?;
        Ok(t)
    }
}

impl Default for DampedSineTable {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Shared hardened parser (mirrors hertzforge-breakpoints' conventions exactly).
// ---------------------------------------------------------------------------

/// The recognizable columns for import header aliasing across both component tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Col {
    Freq,
    Amp,
    HalfCycles,
    Zeta,
    Delay,
    Polarity,
}

/// Positional column layout for a headerless wavelet table.
const WAVELET_POSITIONAL: &[Col] = &[Col::Freq, Col::Amp, Col::HalfCycles, Col::Delay];

/// Positional column layout for a headerless damped-sine table.
const DAMPED_SINE_POSITIONAL: &[Col] = &[Col::Freq, Col::Amp, Col::Zeta, Col::Delay];

/// One parsed row: the source line (for error messages) and a value per recognized
/// column. `None` for a column means "absent/blank" (the caller decides the default).
type ParsedRow = (String, std::collections::HashMap<Col, Option<f64>>);

/// The shared row scanner. Skips blank and comment (`#`, `//`) lines, auto-detects the
/// delimiter (or honors `delim`), detects a header row via [`col_from_header`], and
/// otherwise falls back to the `positional` layout. RFC-4180 quoting applies for the
/// comma delimiter.
fn parse_rows(
    s: &str,
    delim: Option<char>,
    positional: &[Col],
) -> Result<Vec<ParsedRow>, ComponentError> {
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
        return Err(ComponentError::Parse("no data rows".into()));
    }

    let d = match delim {
        Some(c) => c,
        None => detect_delimiter(lines[0]),
    };

    let first_cells = split_cells(lines[0], d);
    // A header row has at least one cell that resolves to a known column name but is
    // NOT itself a number (so a numeric first row is treated as data, not a header).
    let has_header = first_cells
        .iter()
        .any(|c| col_from_header(c).is_some() && parse_num(c).is_none());

    let col_map: Vec<Option<Col>> = if has_header {
        first_cells.iter().map(|c| col_from_header(c)).collect()
    } else {
        (0..first_cells.len())
            .map(|i| positional.get(i).copied())
            .collect()
    };
    let data_start = if has_header { 1 } else { 0 };

    let mut out = Vec::new();
    for line in &lines[data_start..] {
        let cells = split_cells(line, d);
        if cells.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let mut row: std::collections::HashMap<Col, Option<f64>> = std::collections::HashMap::new();
        for (i, col) in col_map.iter().enumerate() {
            let Some(col) = col else { continue };
            let raw = cells.get(i).map(|s| s.trim()).unwrap_or("");
            if raw.is_empty() {
                row.entry(*col).or_insert(None);
                continue;
            }
            let value = if *col == Col::Polarity {
                Some(parse_polarity(raw))
            } else {
                parse_num(raw)
            };
            row.insert(*col, value);
        }
        out.push(((*line).to_string(), row));
    }
    Ok(out)
}

/// Detect the delimiter of a data line: prefer tab, then semicolon, then comma, else
/// whitespace. Mirrors `hertzforge-breakpoints::detect_delimiter`.
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

/// Split one line into trimmed owned cells. RFC-4180 quoting for the comma delimiter;
/// whitespace-collapse for the space delimiter; literal split otherwise. Mirrors
/// `hertzforge-breakpoints::split_cells`.
fn split_cells(line: &str, delim: char) -> Vec<String> {
    match delim {
        ',' => parse_csv_line(line),
        ' ' => line.split_whitespace().map(|s| s.to_string()).collect(),
        d => line.split(d).map(|s| s.trim().to_string()).collect(),
    }
}

/// Minimal RFC-4180 single-line field splitter: honors double-quoted fields, `""`
/// escapes, and commas inside quotes. Mirrors `hertzforge-breakpoints::parse_csv_line`.
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
/// trailing unit token separated by a space. Mirrors `hertzforge-breakpoints::parse_num`.
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

/// Resolve a polarity cell to a `+1.0` / `-1.0` sentinel. A leading `-`, `neg`, or an
/// explicit negative number reads as negative; anything else (including `+`, `pos`,
/// `1`, empty-after-trim) reads as positive.
fn parse_polarity(raw: &str) -> f64 {
    let t = raw.trim().to_ascii_lowercase();
    if t.starts_with('-') || t.starts_with("neg") {
        return -1.0;
    }
    // A bare number: sign carries the polarity.
    if let Some(v) = parse_num(&t) {
        return if v < 0.0 { -1.0 } else { 1.0 };
    }
    1.0
}

/// Resolve a header cell (case/space/underscore-insensitive) to a column. Mirrors the
/// aliasing scheme of `hertzforge-breakpoints::Column::from_header`.
fn col_from_header(raw: &str) -> Option<Col> {
    let n = normalize(raw);
    match n.as_str() {
        "freq" | "frequency" | "hz" | "f" => Some(Col::Freq),
        "amp" | "amplitude" | "g" | "a" | "accel" | "acceleration" => Some(Col::Amp),
        "halfcycles" | "n" | "cycles" | "nhalf" | "halfcycle" => Some(Col::HalfCycles),
        "zeta" | "damping" | "decay" | "z" | "dampingratio" => Some(Col::Zeta),
        "delay" | "t0" | "td" | "tstart" | "start" => Some(Col::Delay),
        "polarity" | "pol" | "sign" => Some(Col::Polarity),
        _ => None,
    }
}

/// Normalize a header cell: drop the part after `(` (unit suffix), lowercase, and strip
/// spaces / underscores / `/` / `^` / `.` / `#` / `-`. Mirrors
/// `hertzforge-breakpoints::normalize` (with `-` added so `half-cycles` folds to
/// `halfcycles`).
fn normalize(raw: &str) -> String {
    let base = raw.split('(').next().unwrap_or(raw);
    base.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '/' | '^' | '.' | '#' | '-'))
        .flat_map(|c| c.to_lowercase())
        .collect()
}
