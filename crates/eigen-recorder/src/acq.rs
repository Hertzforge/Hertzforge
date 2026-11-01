//! Acquisition configuration: the "standard DAQ selection tools" (sample-rate
//! ladder, block size, window, overlap, averaging) as one validated, serializable
//! type, plus the derived readouts the UI shows live (df, spectral lines, span).

use serde::{Deserialize, Serialize};

/// FFT window selection for the analysis pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    /// General-purpose default (amplitude x2.0, ENBW 1.5 bins).
    Hann,
    /// Amplitude-accurate for tone measurements (very wide main lobe).
    Flattop,
    /// No window (transient/pre-windowed data only).
    Rectangular,
    /// 4-term Blackman-Harris (very low sidelobes).
    BlackmanHarris,
}

impl WindowKind {
    /// The conventional default overlap percentage for this window:
    /// Hann 50%, Flattop 75%, Rectangular 0%, Blackman-Harris 66.7%.
    pub fn default_overlap_pct(self) -> f64 {
        match self {
            WindowKind::Hann => 50.0,
            WindowKind::Flattop => 75.0,
            WindowKind::Rectangular => 0.0,
            WindowKind::BlackmanHarris => 66.7,
        }
    }
}

/// Spectrum averaging mode for the analysis pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AveragingKind {
    /// Equal-weight average over `average_count` completed blocks.
    Linear,
    /// Exponential (running) average with an `average_count`-equivalent constant.
    Exponential,
    /// Per-bin maximum hold.
    PeakHold,
}

/// The acquisition / analysis configuration - the operator's "standard selection
/// tools" (sample rate, windowing, block sizes) front and center.
///
/// # HONEST SEMANTICS
///
/// **Recording rate is always native.** `analysis_decimation` affects ANALYSIS
/// ONLY and requires the anti-alias decimator (`eigen-dsp::decimate`) wired at
/// analysis time - this type only models/validates the selection. The capture
/// path writes `telemetry.f32` at `native_rate_hz` with decimation 1, no
/// exceptions; the selector never implies the capture rate changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionConfig {
    /// The backend's native sample rate in Hz (from backend caps; 65,536 for now).
    pub native_rate_hz: f64,
    /// ANALYSIS-side decimation rung: a power of two in `1..=256`. The analysis
    /// rate is `native_rate_hz / analysis_decimation`. Capture is ALWAYS native.
    pub analysis_decimation: u32,
    /// FFT block size in frames: a power of two in `256..=65_536`.
    pub block_size: usize,
    /// FFT window applied per analysis block.
    pub window: WindowKind,
    /// Block overlap percentage, `0.0..=95.0`.
    pub overlap_pct: f64,
    /// Spectrum averaging mode.
    pub averaging: AveragingKind,
    /// Average count (Linear: blocks per completed average; Exponential: the
    /// equivalent time constant in blocks). Must be at least 1.
    pub average_count: usize,
}

impl Default for AcquisitionConfig {
    /// Hann window, 50% overlap, Linear averaging count 8, block 2048,
    /// decimation 1, native 65,536 Hz.
    fn default() -> Self {
        Self {
            native_rate_hz: 65_536.0,
            analysis_decimation: 1,
            block_size: 2048,
            window: WindowKind::Hann,
            overlap_pct: WindowKind::Hann.default_overlap_pct(),
            averaging: AveragingKind::Linear,
            average_count: 8,
        }
    }
}

impl AcquisitionConfig {
    /// The analysis-side sample rate: `native_rate_hz / analysis_decimation`.
    pub fn analysis_rate_hz(&self) -> f64 {
        self.native_rate_hz / f64::from(self.analysis_decimation)
    }

    /// Frequency resolution of one analysis block: `analysis_rate / block_size`.
    pub fn df_hz(&self) -> f64 {
        self.analysis_rate_hz() / self.block_size as f64
    }

    /// Alias-free spectral line count: `floor(block_size / 2.56)`.
    pub fn spectral_lines(&self) -> usize {
        (self.block_size as f64 / 2.56).floor() as usize
    }

    /// Alias-free frequency span: `analysis_rate / 2.56`.
    pub fn span_hz(&self) -> f64 {
        self.analysis_rate_hz() / 2.56
    }

    /// Wall-clock duration of one analysis block in seconds.
    pub fn block_seconds(&self) -> f64 {
        self.block_size as f64 / self.analysis_rate_hz()
    }

    /// Validate the selection. Returns the first violation as a human-readable
    /// message (the UI surfaces it verbatim).
    pub fn validate(&self) -> Result<(), String> {
        if self.native_rate_hz <= 0.0 || self.native_rate_hz.is_nan() {
            return Err(format!(
                "native_rate_hz must be positive (got {})",
                self.native_rate_hz
            ));
        }
        if !self.analysis_decimation.is_power_of_two() {
            return Err(format!(
                "analysis_decimation must be a power of two (got {})",
                self.analysis_decimation
            ));
        }
        if self.analysis_decimation > 256 {
            return Err(format!(
                "analysis_decimation must be at most 256 (got {})",
                self.analysis_decimation
            ));
        }
        if !self.block_size.is_power_of_two() {
            return Err(format!(
                "block_size must be a power of two (got {})",
                self.block_size
            ));
        }
        if !(256..=65_536).contains(&self.block_size) {
            return Err(format!(
                "block_size must be within 256..=65536 (got {})",
                self.block_size
            ));
        }
        if !(0.0..=95.0).contains(&self.overlap_pct) {
            return Err(format!(
                "overlap_pct must be within 0..=95 (got {})",
                self.overlap_pct
            ));
        }
        if self.average_count < 1 {
            return Err("average_count must be at least 1".to_string());
        }
        Ok(())
    }
}
