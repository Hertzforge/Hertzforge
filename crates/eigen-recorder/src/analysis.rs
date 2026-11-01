//! Analysis pipeline: streaming anti-alias decimation -> overlap block
//! assembly -> spectra (EU output-type taxonomy), FRF (H1 default, labeled
//! Hv, coherence), transmissibility, time statistics, and velocity/
//! displacement derivation - in a LIVE streaming mode and an OFFLINE region
//! mode that reuse the same structs (one implementation of the
//! math, exercised by the live-vs-offline equivalence golden).
//!
//! # Shared-math rule
//!
//! Every spectral output here is derived from ONE primitive chain:
//! `eigen_dsp::spectrum_ext::periodogram` (raw, un-normalized, per block) ->
//! `eigen_dsp::averaging::SpectrumAverager` (over RAW periodograms; the
//! normalizations are linear so averaging first is equivalent) ->
//! `to_psd` / `to_autopower`. The offline region functions
//! ([`analyze_region`], [`analyze_region_frf`]) build the SAME
//! [`ChannelDecimator`] + [`BlockAssembler`] + [`SpectrumPipeline`] /
//! [`FrfPipeline`] the live path streams through - there is no second
//! spectrum implementation to drift.
//!
//! # EU output-type taxonomy and correction exclusivity
//!
//! | [`SpectrumOutput`] | Units    | Correction domain                     |
//! |--------------------|----------|---------------------------------------|
//! | `Eu2PerHz` (PSD)   | EU^2/Hz  | ENERGY (`1/(fs*sum(w^2))`)            |
//! | `Eu2Rms` (autopower)| EU^2 rms| AMPLITUDE (`1/sum(w)^2`)              |
//! | `EuRms` (linear)   | EU rms   | AMPLITUDE (sqrt of autopower)         |
//! | `EuPk` (linear)    | EU peak  | AMPLITUDE (sqrt of 2x autopower)      |
//!
//! Amplitude (coherent-gain) and energy corrections are MUTUALLY EXCLUSIVE by
//! output type: the `to_autopower` normalization already embeds the window's
//! amplitude correction and the `to_psd` normalization already embeds the
//! energy correction. Nothing in this module (and nothing downstream) may
//! stack a `WindowCorrections` factor on top of these outputs; the two
//! domains are tied together by the identity `autopower = psd * df *
//! enbw_bins`, which is pinned as a test. A discrete TONE is read from the
//! amplitude-corrected outputs (`EuPk`/`EuRms`/`Eu2Rms` peak bin); broadband /
//! random content is read from the density (`Eu2PerHz`).

use std::io;

use eigen_dsp::averaging::{AveragingMode, SpectrumAverager};
use eigen_dsp::decimate::DecimatorCascade;
use eigen_dsp::fft;
use eigen_dsp::integrate::{accel_to_velocity, velocity_to_displacement, G_TO_IN_S2};
use eigen_dsp::spectrum_ext::{periodogram, to_autopower, to_psd};
use eigen_dsp::window;
use eigen_dsp::window_ext::{self, corrections, WindowCorrections};

use crate::acq::{AcquisitionConfig, AveragingKind, WindowKind};
use crate::recording::RecordingReader;

// ---------------------------------------------------------------------------
// Window / hop plumbing shared by the spectrum and FRF pipelines
// ---------------------------------------------------------------------------

/// The window vector for a [`WindowKind`] at length `n` (periodic sampling,
/// matching `eigen_dsp::window::hann` / `eigen_dsp::window_ext` throughout).
pub fn window_vector(kind: WindowKind, n: usize) -> Vec<f64> {
    match kind {
        WindowKind::Hann => window::hann(n),
        WindowKind::Flattop => window_ext::flattop(n),
        WindowKind::Rectangular => window_ext::rectangular(n),
        WindowKind::BlackmanHarris => window_ext::blackman_harris(n),
    }
}

/// Hop size in frames for a block size and overlap percentage:
/// `round(block_size * (1 - overlap_pct/100))`, floored at 1.
pub fn hop_frames(block_size: usize, overlap_pct: f64) -> usize {
    ((block_size as f64) * (1.0 - overlap_pct / 100.0))
        .round()
        .max(1.0) as usize
}

/// Map the config's averaging selection onto the eigen-dsp averager mode:
/// `Linear` uses `average_count` directly; `Exponential` uses `alpha =
/// 1/average_count` (the config documents `average_count` as the equivalent
/// time constant in blocks); `PeakHold` has no parameter.
fn averaging_mode(acq: &AcquisitionConfig) -> AveragingMode {
    let count = acq.average_count.max(1);
    match acq.averaging {
        AveragingKind::Linear => AveragingMode::Linear { count },
        AveragingKind::Exponential => AveragingMode::Exponential {
            alpha: 1.0 / count as f64,
        },
        AveragingKind::PeakHold => AveragingMode::PeakHold,
    }
}

// ---------------------------------------------------------------------------
// 1. ChannelDecimator
// ---------------------------------------------------------------------------

/// Streaming anti-alias decimation for a set of channels: one
/// [`DecimatorCascade`] per channel at the config's `analysis_decimation`
/// rung (factor 1 is a pass-through). Stateful across calls - each channel's
/// FIR history carries over, so arbitrary chunk sizes stream correctly and
/// the output is chunking-invariant (which is what makes the offline chunked
/// path bit-comparable with the live path).
#[derive(Debug, Clone)]
pub struct ChannelDecimator {
    cascades: Vec<DecimatorCascade>,
}

impl ChannelDecimator {
    /// Build one cascade per channel for `acq.analysis_decimation`.
    /// Panics (via [`DecimatorCascade::new`]) if the factor is not a power of
    /// two.
    pub fn new(n_channels: usize, acq: &AcquisitionConfig) -> Self {
        Self {
            cascades: (0..n_channels)
                .map(|_| DecimatorCascade::new(acq.analysis_decimation))
                .collect(),
        }
    }

    /// Number of channels.
    pub fn n_channels(&self) -> usize {
        self.cascades.len()
    }

    /// The decimation factor (1 = pass-through).
    pub fn factor(&self) -> u32 {
        self.cascades.first().map_or(1, DecimatorCascade::factor)
    }

    /// Stream one channel's samples through its cascade, returning the
    /// decimated samples produced by this call (about `samples.len() /
    /// factor`, with the phase carried across calls).
    pub fn process(&mut self, ch: usize, samples: &[f64]) -> Vec<f64> {
        self.cascades[ch].process(samples)
    }

    /// Total cascade group delay in INPUT-rate samples (identical for every
    /// channel).
    pub fn group_delay_input_samples(&self) -> f64 {
        self.cascades
            .first()
            .map_or(0.0, DecimatorCascade::group_delay_samples)
    }

    /// Reset every channel's cascade to the zero-primed start state.
    pub fn reset(&mut self) {
        for c in self.cascades.iter_mut() {
            c.reset();
        }
    }
}

// ---------------------------------------------------------------------------
// 2. BlockAssembler
// ---------------------------------------------------------------------------

/// Assembles analysis blocks of `block_size` samples with `overlap_pct`
/// overlap from deinterleaved per-channel streams (post-decimation), keeping
/// all channels in LOCKSTEP: a block is emitted only when every channel has a
/// full block available, and every channel advances by the same hop
/// (`hop = round(block_size * (1 - overlap_pct/100))`, min 1).
///
/// Feed with [`BlockAssembler::push`] per channel, then drain with
/// [`BlockAssembler::next_block`] until it returns `None`.
#[derive(Debug, Clone)]
pub struct BlockAssembler {
    block_size: usize,
    hop: usize,
    /// Shared consumed-prefix offset (channels advance in lockstep, so one
    /// head serves all buffers).
    head: usize,
    bufs: Vec<Vec<f64>>,
}

impl BlockAssembler {
    /// Build an assembler. Panics if `n_channels` or `block_size` is zero, or
    /// `overlap_pct` is outside `0..=95` (the config validation range).
    pub fn new(n_channels: usize, block_size: usize, overlap_pct: f64) -> Self {
        assert!(n_channels >= 1, "BlockAssembler: need at least one channel");
        assert!(block_size >= 1, "BlockAssembler: block_size must be >= 1");
        assert!(
            (0.0..=95.0).contains(&overlap_pct),
            "BlockAssembler: overlap_pct must be within 0..=95 (got {overlap_pct})"
        );
        Self {
            block_size,
            hop: hop_frames(block_size, overlap_pct),
            head: 0,
            bufs: vec![Vec::new(); n_channels],
        }
    }

    /// [`BlockAssembler::new`] from the config's block size and overlap.
    pub fn from_config(n_channels: usize, acq: &AcquisitionConfig) -> Self {
        Self::new(n_channels, acq.block_size, acq.overlap_pct)
    }

    /// Block size in samples.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Hop (new samples consumed per emitted block).
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Number of channels.
    pub fn n_channels(&self) -> usize {
        self.bufs.len()
    }

    /// Append samples to one channel's pending buffer.
    pub fn push(&mut self, ch: usize, samples: &[f64]) {
        self.bufs[ch].extend_from_slice(samples);
    }

    /// Emit the next lockstep block (one `block_size` vector per channel) if
    /// EVERY channel has one buffered, advancing all channels by the hop.
    pub fn next_block(&mut self) -> Option<Vec<Vec<f64>>> {
        let ready = self
            .bufs
            .iter()
            .all(|b| b.len() - self.head >= self.block_size);
        if !ready {
            return None;
        }
        let out: Vec<Vec<f64>> = self
            .bufs
            .iter()
            .map(|b| b[self.head..self.head + self.block_size].to_vec())
            .collect();
        self.head += self.hop;
        // Compact the consumed prefix once it exceeds a block (amortized O(1)
        // per sample; `head <= len` always holds because `hop <= block_size`).
        if self.head >= self.block_size {
            for b in self.bufs.iter_mut() {
                b.drain(..self.head);
            }
            self.head = 0;
        }
        Some(out)
    }

    /// Drop all pending samples (blocks already emitted are unaffected).
    pub fn reset(&mut self) {
        for b in self.bufs.iter_mut() {
            b.clear();
        }
        self.head = 0;
    }
}

// ---------------------------------------------------------------------------
// 3. SpectrumPipeline
// ---------------------------------------------------------------------------

/// EU output-type tag for spectrum readouts. See the module docs
/// for the units table and the amplitude-vs-energy correction exclusivity
/// rule each variant encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectrumOutput {
    /// Linear amplitude spectrum in EU PEAK (an on-bin tone of peak amplitude
    /// `A` reads `A` in its peak bin). AMPLITUDE-corrected.
    EuPk,
    /// Linear amplitude spectrum in EU RMS (an on-bin tone of peak amplitude
    /// `A` reads `A/sqrt(2)`). AMPLITUDE-corrected.
    EuRms,
    /// Autopower in EU^2 rms per bin (an on-bin tone reads `A^2/2`).
    /// AMPLITUDE-corrected power.
    Eu2Rms,
    /// One-sided power spectral density in EU^2/Hz (`integral(psd) df ==
    /// variance`). ENERGY-corrected density.
    Eu2PerHz,
}

/// Per-channel averaged spectrum pipeline: window from the config's
/// [`WindowKind`], raw [`periodogram`] per block, one [`SpectrumAverager`]
/// over the RAW periodograms, and normalization to any [`SpectrumOutput`] on
/// readout (the normalizations are linear, so averaging raw and normalizing
/// after is exactly equivalent to averaging normalized spectra - one
/// averager serves every output type).
#[derive(Debug, Clone)]
pub struct SpectrumPipeline {
    win: Vec<f64>,
    corr: WindowCorrections,
    fs_hz: f64,
    df_hz: f64,
    averager: SpectrumAverager,
}

impl SpectrumPipeline {
    /// Build from the acquisition config: window `acq.window` at
    /// `acq.block_size`, sample rate `acq.analysis_rate_hz()` (the pipeline
    /// consumes POST-decimation blocks), averaging per `acq.averaging` /
    /// `acq.average_count`. Panics on an invalid config.
    pub fn new(acq: &AcquisitionConfig) -> Self {
        if let Err(e) = acq.validate() {
            panic!("SpectrumPipeline: invalid AcquisitionConfig: {e}");
        }
        let win = window_vector(acq.window, acq.block_size);
        let corr = corrections(&win);
        let n_bins = acq.block_size / 2 + 1;
        Self {
            win,
            corr,
            fs_hz: acq.analysis_rate_hz(),
            df_hz: acq.df_hz(),
            averager: SpectrumAverager::new(averaging_mode(acq), n_bins),
        }
    }

    /// Feed one analysis block (post-decimation, `block_size` samples).
    /// Panics if the length does not match.
    pub fn push_block(&mut self, block: &[f64]) {
        let raw = periodogram(block, &self.win, self.fs_hz);
        self.averager.push(&raw);
    }

    /// The averaged spectrum normalized to `kind` (see [`SpectrumOutput`] for
    /// units; zeros before the first block). Every output derives from the
    /// same averaged raw periodogram via `to_psd` / `to_autopower` - the
    /// exclusivity rule is structural, not a convention to remember.
    pub fn spectrum(&self, kind: SpectrumOutput) -> Vec<f64> {
        let mut out = self.averager.average().to_vec();
        match kind {
            SpectrumOutput::Eu2PerHz => to_psd(&mut out, &self.win, self.fs_hz),
            SpectrumOutput::Eu2Rms => to_autopower(&mut out, &self.win),
            SpectrumOutput::EuRms => {
                to_autopower(&mut out, &self.win);
                for v in out.iter_mut() {
                    *v = v.max(0.0).sqrt();
                }
            }
            SpectrumOutput::EuPk => {
                to_autopower(&mut out, &self.win);
                for v in out.iter_mut() {
                    *v = (2.0 * v.max(0.0)).sqrt();
                }
            }
        }
        out
    }

    /// One-sided PSD in EU^2/Hz ([`SpectrumOutput::Eu2PerHz`]; ENERGY
    /// correction domain).
    pub fn psd(&self) -> Vec<f64> {
        self.spectrum(SpectrumOutput::Eu2PerHz)
    }

    /// Autopower in EU^2 rms per bin ([`SpectrumOutput::Eu2Rms`]; AMPLITUDE
    /// correction domain).
    pub fn autopower(&self) -> Vec<f64> {
        self.spectrum(SpectrumOutput::Eu2Rms)
    }

    /// AMPLITUDE-corrected linear magnitude spectrum. `kind` must be
    /// [`SpectrumOutput::EuPk`] or [`SpectrumOutput::EuRms`]; the power kinds
    /// panic (they are not linear spectra - use [`SpectrumPipeline::psd`] /
    /// [`SpectrumPipeline::autopower`]).
    pub fn linear_spectrum(&self, kind: SpectrumOutput) -> Vec<f64> {
        assert!(
            matches!(kind, SpectrumOutput::EuPk | SpectrumOutput::EuRms),
            "linear_spectrum takes EuPk or EuRms (got {kind:?}); \
             use psd()/autopower() for the power output types"
        );
        self.spectrum(kind)
    }

    /// Frequency resolution in Hz (`analysis_rate / block_size`).
    pub fn df_hz(&self) -> f64 {
        self.df_hz
    }

    /// Equivalent noise bandwidth of the configured window, in bins (derived
    /// numerically from the actual window vector, never hardcoded).
    pub fn enbw_bins(&self) -> f64 {
        self.corr.enbw_bins
    }

    /// Number of bins per spectrum (`block_size/2 + 1`).
    pub fn n_bins(&self) -> usize {
        self.averager.n_bins()
    }

    /// Blocks currently CONTRIBUTING to the average: for Linear averaging
    /// this saturates at `average_count` (later pushes are ignored, the
    /// standard "N averages then hold" behavior); Exponential and PeakHold
    /// count every push.
    pub fn n_averages(&self) -> usize {
        match self.averager.mode() {
            AveragingMode::Linear { count } => self.averager.n_pushed().min(count),
            AveragingMode::Exponential { .. } | AveragingMode::PeakHold => self.averager.n_pushed(),
        }
    }

    /// Whether the configured average is complete (Linear: `average_count`
    /// blocks accumulated; Exponential / PeakHold: at least one block).
    pub fn complete(&self) -> bool {
        self.averager.complete()
    }

    /// Restart the average.
    pub fn reset(&mut self) {
        self.averager.reset();
    }
}

// ---------------------------------------------------------------------------
// 4. FrfPipeline
// ---------------------------------------------------------------------------

/// Cross-channel FRF pipeline for any (reference `x`, response `y`) pair,
/// fed with the SAME windowed blocks as [`SpectrumPipeline`] (lockstep
/// [`BlockAssembler`] output). Accumulates the Welch auto/cross spectra
/// `Gxx = <|X|^2>`, `Gyy = <|Y|^2>`, `Gxy = <X*conj(Y)>` per block.
///
/// # Phase convention (matches `eigen_dsp::frf` EXACTLY - load-bearing)
///
/// The cross spectrum is `Gxy = <X*conj(Y)>` and every estimator's phase is
/// `angle(Gxy)`, the sign that maps onto the z-domain FRF phase under this
/// workspace's negative-exponent forward FFT (see the pinned convention note
/// on `eigen_dsp::frf::h_estimate_hv` and its `h_estimate_hv_phase_sign_pinned`
/// test: a lagging response reads NEGATIVE phase). Under that convention
/// `H1 = Gxy / Gxx` (the denominator is real), so `angle(H1) == angle(Gxy) ==
/// angle(Hv)` and the magnitude identity `|H1| = gamma * |Hv|` (with
/// `gamma = sqrt(coherence)`) holds bin-exactly - both are pinned as tests.
///
/// # Estimator labeling
///
/// [`FrfPipeline::h1`] is the DEFAULT estimator (unbiased by output noise;
/// biased low by input noise). [`FrfPipeline::hv`] is offered and LABELED:
/// its magnitude `sqrt(Gyy/Gxx)` is biased HIGH by output noise.
///
/// Windowing matches `eigen_dsp::frf::h_estimate_hv`: the block is windowed
/// with NO per-block mean removal (unlike the periodogram path, which removes
/// the mean for density readouts - the FRF ratios are insensitive to it and
/// bit-compatibility with the pinned eigen-dsp estimator wins).
#[derive(Debug, Clone)]
pub struct FrfPipeline {
    win: Vec<f64>,
    df_hz: f64,
    gxx: Vec<f64>,
    gyy: Vec<f64>,
    gxy: Vec<(f64, f64)>,
    n_blocks: usize,
}

impl FrfPipeline {
    /// Build from the acquisition config (window `acq.window`, block
    /// `acq.block_size`, analysis-rate bin spacing). Panics on an invalid
    /// config.
    pub fn new(acq: &AcquisitionConfig) -> Self {
        if let Err(e) = acq.validate() {
            panic!("FrfPipeline: invalid AcquisitionConfig: {e}");
        }
        let n_bins = acq.block_size / 2 + 1;
        Self {
            win: window_vector(acq.window, acq.block_size),
            df_hz: acq.df_hz(),
            gxx: vec![0.0; n_bins],
            gyy: vec![0.0; n_bins],
            gxy: vec![(0.0, 0.0); n_bins],
            n_blocks: 0,
        }
    }

    /// Accumulate one lockstep block pair (`x` = reference, `y` = response;
    /// both `block_size` samples). Panics on a length mismatch.
    pub fn push_block(&mut self, x_block: &[f64], y_block: &[f64]) {
        let n = self.win.len();
        assert_eq!(x_block.len(), n, "FrfPipeline: x block length mismatch");
        assert_eq!(y_block.len(), n, "FrfPipeline: y block length mismatch");
        let mut xbuf: Vec<fft::Cplx> = x_block
            .iter()
            .zip(&self.win)
            .map(|(&v, &w)| (v * w, 0.0))
            .collect();
        let mut ybuf: Vec<fft::Cplx> = y_block
            .iter()
            .zip(&self.win)
            .map(|(&v, &w)| (v * w, 0.0))
            .collect();
        fft::fft_in_place(&mut xbuf);
        fft::fft_in_place(&mut ybuf);
        let nout = self.gxx.len();
        for k in 0..nout {
            let (xr, xi) = xbuf[k];
            let (yr, yi) = ybuf[k];
            self.gxx[k] += xr * xr + xi * xi;
            self.gyy[k] += yr * yr + yi * yi;
            // Gxy = <X*conj(Y)> = <(xr + j xi)(yr - j yi)> - the pinned
            // eigen_dsp::frf convention (see the struct docs).
            self.gxy[k].0 += xr * yr + xi * yi;
            self.gxy[k].1 += xi * yr - xr * yi;
        }
        self.n_blocks += 1;
    }

    /// The H1 estimator `H1 = Gxy / Gxx` as `(re, im)` per bin - the DEFAULT
    /// (unbiased by output noise). Bins with no reference energy (`Gxx <= 0`)
    /// return `(0, 0)`. Phase is `angle(Gxy)`; `|H1| = gamma * |Hv|`.
    pub fn h1(&self) -> Vec<(f64, f64)> {
        self.gxx
            .iter()
            .zip(&self.gxy)
            .map(|(&gxx, &(re, im))| {
                if gxx <= 0.0 || gxx.is_nan() {
                    (0.0, 0.0)
                } else {
                    (re / gxx, im / gxx)
                }
            })
            .collect()
    }

    /// The Hv estimator `|Hv| = sqrt(Gyy/Gxx)`, phase `angle(Gxy)` - LABELED:
    /// biased HIGH by output noise (it carries the response noise power in
    /// `Gyy`; see `eigen_dsp::frf::h_estimate_hv`'s magnitude-bias note).
    /// Bins with no reference or response energy return `(0, 0)`.
    pub fn hv(&self) -> Vec<(f64, f64)> {
        (0..self.gxx.len())
            .map(|k| {
                let (gxx, gyy) = (self.gxx[k], self.gyy[k]);
                if gxx <= 0.0 || gxx.is_nan() || gyy <= 0.0 || gyy.is_nan() {
                    return (0.0, 0.0);
                }
                let mag = (gyy / gxx).sqrt();
                let phase = self.gxy[k].1.atan2(self.gxy[k].0);
                (mag * phase.cos(), mag * phase.sin())
            })
            .collect()
    }

    /// Ordinary coherence `gamma^2 = |Gxy|^2 / (Gxx*Gyy)`, clamped to
    /// `[0, 1]`. Bins with no energy in either channel return `0.0`
    /// (untrusted). MEANINGLESS below two averages - gate on
    /// [`FrfPipeline::coherence_valid`] before displaying.
    pub fn coherence(&self) -> Vec<f64> {
        (0..self.gxx.len())
            .map(|k| {
                let (gxx, gyy) = (self.gxx[k], self.gyy[k]);
                if gxx <= 0.0 || gxx.is_nan() || gyy <= 0.0 || gyy.is_nan() {
                    return 0.0;
                }
                let (re, im) = self.gxy[k];
                ((re * re + im * im) / (gxx * gyy)).clamp(0.0, 1.0)
            })
            .collect()
    }

    /// Transmissibility `|H1|` per bin (exactly the magnitude of
    /// [`FrfPipeline::h1`] - same numbers, scalar view).
    pub fn transmissibility(&self) -> Vec<f64> {
        self.h1()
            .iter()
            .map(|&(re, im)| (re * re + im * im).sqrt())
            .collect()
    }

    /// Whether the coherence output is meaningful: at least TWO completed
    /// averages (on a single average `|Gxy|^2 == Gxx*Gyy`
    /// identically, so coherence reads 1.0 at every bin regardless of the
    /// actual SNR). Below two, the caller must SUPPRESS the coherence
    /// display ("insufficient averages"), not show the degenerate 1.0 trace.
    pub fn coherence_valid(&self) -> bool {
        self.n_blocks >= 2
    }

    /// Completed block-pair averages.
    pub fn n_averages(&self) -> usize {
        self.n_blocks
    }

    /// Frequency resolution in Hz.
    pub fn df_hz(&self) -> f64 {
        self.df_hz
    }

    /// Number of bins (`block_size/2 + 1`).
    pub fn n_bins(&self) -> usize {
        self.gxx.len()
    }

    /// Restart the accumulation.
    pub fn reset(&mut self) {
        self.gxx.iter_mut().for_each(|v| *v = 0.0);
        self.gyy.iter_mut().for_each(|v| *v = 0.0);
        self.gxy.iter_mut().for_each(|v| *v = (0.0, 0.0));
        self.n_blocks = 0;
    }
}

// ---------------------------------------------------------------------------
// 5. Time statistics
// ---------------------------------------------------------------------------

/// Time-domain statistics of a sample window.
///
/// `kurtosis` uses the VIBRATION convention (normalized 4th central moment;
/// a Gaussian signal reads 3.0, a sine reads 1.5) - NOT excess kurtosis
/// (label displays accordingly). `skew` is the normalized 3rd central
/// moment. `rms` is the true RMS about ZERO (not mean-removed); `crest` is
/// `peak / rms`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeStats {
    /// Arithmetic mean.
    pub mean: f64,
    /// True RMS (about zero).
    pub rms: f64,
    /// Max absolute value.
    pub peak: f64,
    /// `max - min`.
    pub pk_pk: f64,
    /// Crest factor `peak / rms` (0.0 when the RMS is zero).
    pub crest: f64,
    /// Normalized 3rd central moment (0.0 when the variance is zero).
    pub skew: f64,
    /// Normalized 4th central moment, VIBRATION convention: Gaussian = 3
    /// (0.0 when the variance is zero).
    pub kurtosis: f64,
}

const EMPTY_STATS: TimeStats = TimeStats {
    mean: 0.0,
    rms: 0.0,
    peak: 0.0,
    pk_pk: 0.0,
    crest: 0.0,
    skew: 0.0,
    kurtosis: 0.0,
};

/// Streaming accumulator behind [`time_stats`] and the offline region path -
/// ONE implementation of the moment math for both, so live stats
/// on a slice and offline stats accumulated chunk-by-chunk over the same
/// samples are bit-identical. Central moments use the numerically stable
/// one-pass (Welford / Pebay) update; `mean`, `rms` and `peak` follow exactly
/// the `eigen_dsp::stats` conventions (population moments, `peak =
/// max(|x|)`), which the test suite cross-checks against those functions.
#[derive(Debug, Clone, Copy)]
pub struct TimeStatsAccumulator {
    n: u64,
    mean: f64,
    m2: f64,
    m3: f64,
    m4: f64,
    min: f64,
    max: f64,
}

impl TimeStatsAccumulator {
    /// An empty accumulator.
    pub fn new() -> Self {
        Self {
            n: 0,
            mean: 0.0,
            m2: 0.0,
            m3: 0.0,
            m4: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }

    /// Accumulate one sample.
    pub fn push(&mut self, x: f64) {
        let n1 = self.n as f64;
        self.n += 1;
        let n = self.n as f64;
        let delta = x - self.mean;
        let delta_n = delta / n;
        let delta_n2 = delta_n * delta_n;
        let term1 = delta * delta_n * n1;
        self.mean += delta_n;
        self.m4 += term1 * delta_n2 * (n * n - 3.0 * n + 3.0) + 6.0 * delta_n2 * self.m2
            - 4.0 * delta_n * self.m3;
        self.m3 += term1 * delta_n * (n - 2.0) - 3.0 * delta_n * self.m2;
        self.m2 += term1;
        if x < self.min {
            self.min = x;
        }
        if x > self.max {
            self.max = x;
        }
    }

    /// Accumulate a slice.
    pub fn push_slice(&mut self, xs: &[f64]) {
        for &x in xs {
            self.push(x);
        }
    }

    /// Samples accumulated so far.
    pub fn n(&self) -> u64 {
        self.n
    }

    /// The statistics of everything accumulated (all zeros when empty).
    pub fn finish(&self) -> TimeStats {
        if self.n == 0 {
            return EMPTY_STATS;
        }
        let n = self.n as f64;
        let var = self.m2 / n;
        let rms = (self.mean * self.mean + var).sqrt();
        let peak = self.max.abs().max(self.min.abs());
        let crest = if rms <= 0.0 || rms.is_nan() {
            0.0
        } else {
            peak / rms
        };
        let (skew, kurtosis) = if var <= 0.0 || var.is_nan() {
            (0.0, 0.0)
        } else {
            ((self.m3 / n) / var.powf(1.5), (self.m4 / n) / (var * var))
        };
        TimeStats {
            mean: self.mean,
            rms,
            peak,
            pk_pk: self.max - self.min,
            crest,
            skew,
            kurtosis,
        }
    }
}

impl Default for TimeStatsAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Time statistics of a sample window (see [`TimeStats`] for conventions;
/// kurtosis is the vibration convention, Gaussian = 3). An empty slice
/// returns all zeros.
pub fn time_stats(x: &[f64]) -> TimeStats {
    let mut acc = TimeStatsAccumulator::new();
    acc.push_slice(x);
    acc.finish()
}

// ---------------------------------------------------------------------------
// 6. Velocity / displacement derivation
// ---------------------------------------------------------------------------

/// SPECTRAL omega-arithmetic: convert an acceleration PSD in
/// g^2/Hz to a velocity PSD in (in/s)^2/Hz by dividing each bin by
/// `(2*pi*f)^2` after the EU conversion `g -> in/s^2` (multiply by
/// `G_TO_IN_S2^2` since the quantity is squared). The DC bin has no defined
/// `1/omega^2` and returns 0.
pub fn psd_accel_to_velocity(psd_g2hz: &[f64], df_hz: f64) -> Vec<f64> {
    assert!(
        df_hz > 0.0,
        "psd_accel_to_velocity: df_hz must be positive (got {df_hz})"
    );
    psd_g2hz
        .iter()
        .enumerate()
        .map(|(k, &p)| {
            if k == 0 {
                0.0
            } else {
                let w = std::f64::consts::TAU * k as f64 * df_hz;
                p * (G_TO_IN_S2 * G_TO_IN_S2) / (w * w)
            }
        })
        .collect()
}

/// SPECTRAL omega-arithmetic, second stage: convert a velocity PSD in
/// (in/s)^2/Hz to a displacement PSD in in^2/Hz (`/(2*pi*f)^2`, no EU
/// conversion - the units are already the in/s system). DC bin -> 0.
pub fn psd_velocity_to_displacement(psd_ips2hz: &[f64], df_hz: f64) -> Vec<f64> {
    assert!(
        df_hz > 0.0,
        "psd_velocity_to_displacement: df_hz must be positive (got {df_hz})"
    );
    psd_ips2hz
        .iter()
        .enumerate()
        .map(|(k, &p)| {
            if k == 0 {
                0.0
            } else {
                let w = std::f64::consts::TAU * k as f64 * df_hz;
                p / (w * w)
            }
        })
        .collect()
}

/// Both spectral stages chained: acceleration PSD in g^2/Hz to displacement
/// PSD in in^2/Hz (g^2/Hz -> (in/s)^2/Hz -> in^2/Hz). DC bin -> 0.
pub fn psd_accel_to_displacement(psd_g2hz: &[f64], df_hz: f64) -> Vec<f64> {
    psd_velocity_to_displacement(&psd_accel_to_velocity(psd_g2hz, df_hz), df_hz)
}

/// Leading samples to trim for the high-pass integrator settle transient:
/// about five time constants of the first-order corner, `ceil(5/(2*pi*hp_hz)
/// * fs_hz)`. Zero when the high-pass is disabled (`hp_hz <= 0`).
pub fn settle_trim_samples(fs_hz: f64, hp_hz: f64) -> usize {
    if fs_hz <= 0.0 || fs_hz.is_nan() || hp_hz <= 0.0 || hp_hz.is_nan() {
        return 0;
    }
    (5.0 / (std::f64::consts::TAU * hp_hz) * fs_hz).ceil() as usize
}

/// TIME-domain integration of an acceleration region (g) to velocity (in/s)
/// and displacement (in) via the eigen-dsp banded high-pass integrators, with
/// the integrator settle transient TRIMMED from the returned windows (stats
/// over the raw integrator output would be polluted by the
/// high-pass step response; ~`5/(2*pi*hp_hz)` seconds are removed).
///
/// Returns `(vel_ips, disp_in, trimmed_lead_samples)`: both vectors start
/// `trimmed_lead_samples` samples after `accel_g[0]` (align time axes with
/// `t = (trimmed_lead_samples + i) / fs_hz`). The same trim count is applied
/// to both outputs. If the region is shorter than the trim, both come back
/// empty with the trim clamped to the region length.
pub fn integrate_region(accel_g: &[f64], fs_hz: f64, hp_hz: f64) -> (Vec<f64>, Vec<f64>, usize) {
    let vel = accel_to_velocity(accel_g, fs_hz, hp_hz);
    let disp = velocity_to_displacement(&vel, fs_hz, hp_hz);
    let trim = settle_trim_samples(fs_hz, hp_hz).min(accel_g.len());
    (vel[trim..].to_vec(), disp[trim..].to_vec(), trim)
}

// ---------------------------------------------------------------------------
// 7. Offline region analysis (reuses the live structs)
// ---------------------------------------------------------------------------

/// Frames per chunked reader pass (2 MB of 8-channel f32; the reader is never
/// asked for the whole region at once).
const REGION_CHUNK_FRAMES: usize = 65_536;

/// One channel's offline region results.
#[derive(Debug, Clone)]
pub struct ChannelAnalysis {
    /// The recording channel index this row analyzed.
    pub channel: usize,
    /// One-sided PSD, EU^2/Hz ([`SpectrumOutput::Eu2PerHz`]).
    pub psd: Vec<f64>,
    /// Autopower, EU^2 rms per bin ([`SpectrumOutput::Eu2Rms`]).
    pub autopower: Vec<f64>,
    /// Linear magnitude spectrum, EU rms ([`SpectrumOutput::EuRms`]).
    pub linear_rms: Vec<f64>,
    /// Time statistics of the RAW native-rate region samples (broadband -
    /// deliberately NOT the decimated analysis stream, so peak/RMS/crest are
    /// honest about everything the recorder captured).
    pub stats: TimeStats,
    /// Frequency resolution of the spectra in Hz (analysis-rate df).
    pub df_hz: f64,
}

/// Offline region analysis over selected channels.
#[derive(Debug, Clone)]
pub struct RegionAnalysis {
    /// Per-channel results, in the order of the `channels` argument.
    pub per_channel: Vec<ChannelAnalysis>,
    /// Analysis blocks ASSEMBLED from the region (identical for every
    /// channel - lockstep). With Linear averaging only the first
    /// `average_count` contribute to the spectra ("N averages then hold");
    /// compare with the per-pipeline `n_averages`.
    pub n_blocks: usize,
}

/// Offline FRF analysis of a channel pair.
#[derive(Debug, Clone)]
pub struct FrfAnalysis {
    /// H1 (DEFAULT estimator) as `(re, im)` per bin.
    pub h1: Vec<(f64, f64)>,
    /// Hv, LABELED: biased high by output noise.
    pub hv: Vec<(f64, f64)>,
    /// Ordinary coherence in `[0, 1]`. Only meaningful when
    /// `coherence_valid` - suppress the display otherwise.
    pub coherence: Vec<f64>,
    /// Transmissibility `|H1|`.
    pub transmissibility: Vec<f64>,
    /// `n_averages >= 2` (the single-average coherence trap).
    pub coherence_valid: bool,
    /// Completed block-pair averages.
    pub n_averages: usize,
    /// Frequency resolution in Hz.
    pub df_hz: f64,
}

/// Run `n_frames` frames starting at `start_frame` through the chunked
/// reader -> [`ChannelDecimator`] -> [`BlockAssembler`] -> per-channel
/// [`SpectrumPipeline`] chain - the SAME structs the live path streams
/// through (the live-vs-offline equivalence golden pins it).
///
/// Reads are chunked ([`REGION_CHUNK_FRAMES`] frames per pass); the whole
/// region is never resident. Requests past EOF are clamped by the reader.
/// Errors on an out-of-range channel index or an I/O failure.
pub fn analyze_region(
    reader: &mut RecordingReader,
    channels: &[usize],
    start_frame: u64,
    n_frames: usize,
    acq: &AcquisitionConfig,
) -> io::Result<RegionAnalysis> {
    let n_ch = reader.info().n_channels;
    for &ch in channels {
        if ch >= n_ch {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("channel {ch} out of range (recording has {n_ch} channels)"),
            ));
        }
    }
    let n_sel = channels.len();
    let mut decim = ChannelDecimator::new(n_sel, acq);
    let mut asm = BlockAssembler::from_config(n_sel, acq);
    let mut pipes: Vec<SpectrumPipeline> = (0..n_sel).map(|_| SpectrumPipeline::new(acq)).collect();
    let mut stats: Vec<TimeStatsAccumulator> =
        (0..n_sel).map(|_| TimeStatsAccumulator::new()).collect();
    let mut n_blocks = 0usize;

    let mut frame = start_frame;
    let mut remaining = n_frames;
    while remaining > 0 {
        let take = remaining.min(REGION_CHUNK_FRAMES);
        let inter = reader.read_region(frame, take)?;
        let got = inter.len() / n_ch;
        if got == 0 {
            break; // region ran past EOF
        }
        for (i, &ch) in channels.iter().enumerate() {
            let samples: Vec<f64> = inter
                .chunks_exact(n_ch)
                .map(|fr| f64::from(fr[ch]))
                .collect();
            stats[i].push_slice(&samples);
            let dec = decim.process(i, &samples);
            asm.push(i, &dec);
        }
        while let Some(blocks) = asm.next_block() {
            for (pipe, block) in pipes.iter_mut().zip(&blocks) {
                pipe.push_block(block);
            }
            n_blocks += 1;
        }
        frame += got as u64;
        remaining -= got;
        if got < take {
            break; // reader clamped at EOF
        }
    }

    let per_channel = channels
        .iter()
        .zip(pipes.iter())
        .zip(stats.iter())
        .map(|((&ch, pipe), acc)| ChannelAnalysis {
            channel: ch,
            psd: pipe.psd(),
            autopower: pipe.autopower(),
            linear_rms: pipe.linear_spectrum(SpectrumOutput::EuRms),
            stats: acc.finish(),
            df_hz: pipe.df_hz(),
        })
        .collect();
    Ok(RegionAnalysis {
        per_channel,
        n_blocks,
    })
}

/// Offline FRF of a channel pair (`x_ch` = reference, `y_ch` = response) over
/// a region, through the SAME chunked-read -> [`ChannelDecimator`] ->
/// [`BlockAssembler`] -> [`FrfPipeline`] chain as the live path.
pub fn analyze_region_frf(
    reader: &mut RecordingReader,
    x_ch: usize,
    y_ch: usize,
    start_frame: u64,
    n_frames: usize,
    acq: &AcquisitionConfig,
) -> io::Result<FrfAnalysis> {
    let n_ch = reader.info().n_channels;
    for ch in [x_ch, y_ch] {
        if ch >= n_ch {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("channel {ch} out of range (recording has {n_ch} channels)"),
            ));
        }
    }
    let mut decim = ChannelDecimator::new(2, acq);
    let mut asm = BlockAssembler::from_config(2, acq);
    let mut frf = FrfPipeline::new(acq);

    let mut frame = start_frame;
    let mut remaining = n_frames;
    while remaining > 0 {
        let take = remaining.min(REGION_CHUNK_FRAMES);
        let inter = reader.read_region(frame, take)?;
        let got = inter.len() / n_ch;
        if got == 0 {
            break;
        }
        for (i, ch) in [x_ch, y_ch].into_iter().enumerate() {
            let samples: Vec<f64> = inter
                .chunks_exact(n_ch)
                .map(|fr| f64::from(fr[ch]))
                .collect();
            let dec = decim.process(i, &samples);
            asm.push(i, &dec);
        }
        while let Some(blocks) = asm.next_block() {
            frf.push_block(&blocks[0], &blocks[1]);
        }
        frame += got as u64;
        remaining -= got;
        if got < take {
            break;
        }
    }

    Ok(FrfAnalysis {
        h1: frf.h1(),
        hv: frf.hv(),
        coherence: frf.coherence(),
        transmissibility: frf.transmissibility(),
        coherence_valid: frf.coherence_valid(),
        n_averages: frf.n_averages(),
        df_hz: frf.df_hz(),
    })
}
