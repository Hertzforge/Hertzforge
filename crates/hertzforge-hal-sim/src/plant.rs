//! The plant models: what the simulator's "device" is.
//!
//! A [`PlantModel`] is plain data on [`crate::SimConfig`]. [`crate::SimBackend`] turns
//! it into a running plant at `configure` (at the configured sample rate) and steps
//! that plant once per sample frame with the whole drive frame, every drive row at
//! once, producing one response per measurement input.
//!
//! | Model | What it is | Inputs x drives |
//! |---|---|---|
//! | [`PlantModel::Resonators`] | The single-path bank: a sum of resonators, feedthrough and transport delay, driven by the FIRST drive row and read identically by every measurement input (noise included). The default, unchanged from the original plant | any x any |
//! | [`PlantModel::Modal`] | A cross-coupled modal plant: each mode is one second-order resonator driven by the inner product of its input shape with the drive frame, seen by input `i` through its output shape. Independent noise per input | fixed by the model |
//! | [`PlantModel::Frf`] | Plays any FRF matrix by partitioned overlap-save block convolution. Independent noise per input | fixed by the stack |
//!
//! Every model gives its exact frequency response ([`PlantModel::analytic_frf`]): the
//! complex FRF of the discretized plant the backend actually steps, transport delay and
//! feedthrough included, so an identification can be checked against the truth.
//!
//! # Conventions
//!
//! - An FRF stack is `lines x n_inputs x n_drives` ([`MatrixStack`]), line `k` at
//!   `k * df`, one-sided, response units per drive volt.
//! - Phase follows the forward transform with the negative exponent: a response that
//!   lags its drive has negative phase, so a pure delay of `D` samples is
//!   `exp(-j 2 pi f D / fs)`. A response `Y = H X` therefore has the spectral matrix
//!   `G_yy = H G_xx H^H` under `G = E[X X^H]`.
//! - "Measurement inputs" are the enabled Control and Monitor rows, in table order. A
//!   Reference row is not a sensor on the structure: it reads the commanded first
//!   drive (see [`crate::SimBackend`]) and takes no plant output.

use crate::fitted::{FittedMode, Recorder, SofteningRun};
use crate::{DelayLine, Resonator, Xorshift64};
use hertzforge_dsp::fft::{self, Cplx};
use hertzforge_sdm::{MatrixStack, C64};
use std::f64::consts::TAU;

/// Default broadband feedthrough of the resonator bank and of a modal plant.
pub const DEFAULT_FEEDTHROUGH: f64 = 1e-3;

/// Default cap on the impulse-response length of an [`FrfPlant`], in samples.
pub const DEFAULT_FRF_MAX_TAPS: usize = 8192;

/// Default block (partition) length of an [`FrfPlant`]'s convolution, in samples. It is
/// also the plant's added latency.
pub const DEFAULT_FRF_BLOCK_FRAMES: usize = 256;

// ---------------------------------------------------------------------------------------
// The model description (plain data).
// ---------------------------------------------------------------------------------------

/// What the simulator's plant is. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub enum PlantModel {
    /// The single-path resonator bank (the default).
    Resonators(ResonatorBank),
    /// A cross-coupled modal plant.
    Modal(ModalPlant),
    /// A plant that plays a given FRF matrix.
    Frf(FrfPlant),
}

impl Default for PlantModel {
    fn default() -> Self {
        PlantModel::Resonators(ResonatorBank::default())
    }
}

/// The single-path plant: a sum of `(f_n, Q)` resonators plus a broadband
/// feedthrough, behind an integer transport delay, driven by the first drive row.
///
/// Every measurement input reads the same response sample, noise included, so the
/// response is rank one. Further drive rows are buffered and consumed in step (the
/// output-buffer contract) but do not reach this plant: a second drive row on a
/// single-path station is an auxiliary output (an amplitude reference, for one) that
/// must not feed the structure. For a plant that every drive reaches, use
/// [`PlantModel::Modal`] or [`PlantModel::Frf`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResonatorBank {
    /// One `(f_n Hz, Q)` pair per mode. Empty is a pure feedthrough plant.
    pub resonances: Vec<(f64, f64)>,
    /// Broadband drive-to-response gain added to the resonators.
    pub feedthrough: f64,
    /// Transport delay in whole samples.
    pub transport_delay_samples: usize,
}

impl Default for ResonatorBank {
    fn default() -> Self {
        ResonatorBank {
            resonances: vec![(crate::DEFAULT_RESONANCE_HZ, crate::DEFAULT_Q)],
            feedthrough: DEFAULT_FEEDTHROUGH,
            transport_delay_samples: 0,
        }
    }
}

/// The frequency response shape of one mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModeResponse {
    /// [`Resonator`]'s constant-peak bandpass: rises below `f_n`, peaks at `Q` on
    /// `f_n`, falls as `f_n / f` above. The shape of a collocated mobility; its real
    /// part is never negative on the unit circle (a passive response).
    #[default]
    Bandpass,
    /// The acceleration response of a mode: `s^2 / (s^2 + s w_n / Q + w_n^2)`. Small
    /// below `f_n` (as `(f / f_n)^2`), peaks near `Q` at `f_n`, flat at one above (the
    /// mass line). A rigid-body mode on a soft suspension.
    Acceleration,
}

/// One mode of a [`ModalPlant`].
#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    /// Natural frequency in Hz (below the Nyquist frequency of the configured rate).
    pub f_hz: f64,
    /// Quality factor (`1 / (2 zeta)`), at least 0.5.
    pub q: f64,
    /// Participation of each drive, `n_drives` long: the mode is driven by the inner
    /// product of this vector with the drive frame.
    pub input_shape: Vec<f64>,
    /// How each measurement input sees the mode, `n_inputs` long: input `i` reads
    /// `output_shape[i] * x` where `x` is the mode's resonator output.
    pub output_shape: Vec<f64>,
    /// The mode's frequency response shape.
    pub response: ModeResponse,
}

impl Mode {
    /// A [`ModeResponse::Bandpass`] mode.
    pub fn new(f_hz: f64, q: f64, input_shape: Vec<f64>, output_shape: Vec<f64>) -> Self {
        Mode {
            f_hz,
            q,
            input_shape,
            output_shape,
            response: ModeResponse::Bandpass,
        }
    }
}

/// A cross-coupled modal plant: `n_inputs` measurement inputs by `n_drives` drives.
///
/// Per sample, with `d` the drive frame after the transport delay:
///
/// ```text
///   x_m   = R_m( input_shape_m . d )                      one resonator per mode
///   y_i   = sum_m output_shape_m[i] x_m
///         + sum_j residual[i][j] d_j + feedthrough * sum_j d_j + noise_i
/// ```
///
/// so `H_ij(f) = exp(-j w D) (sum_m output_shape_m[i] input_shape_m[j] R_m(f) +
/// residual[i][j] + feedthrough)`: a full matrix with real resonances and phase.
/// The `residual` is a frequency-independent matrix (the out-of-band modes' residual
/// flexibility) and the scalar `feedthrough` keeps every element away from exactly
/// zero. The noise is independent per input.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalPlant {
    /// Measurement inputs (rows of the FRF).
    pub n_inputs: usize,
    /// Drives (columns of the FRF).
    pub n_drives: usize,
    pub modes: Vec<Mode>,
    /// Added to every FRF element.
    pub feedthrough: f64,
    /// Row-major `n_inputs x n_drives`, or empty for none.
    pub residual: Vec<f64>,
    /// Transport delay in whole samples, common to every path.
    pub transport_delay_samples: usize,
    /// How each mode is made a filter at the sample rate.
    pub discretization: Discretization,
    /// Modes whose natural frequency falls with their level, when given. Needs
    /// [`Discretization::Fitted`].
    pub softening: Option<ModalSoftening>,
    /// How many of the first inputs carry the noise: all of them when `None`. Inputs
    /// past these (a drive's readback through the residual, say) read clean.
    pub noisy_inputs: Option<usize>,
    /// A file every input of every sample is written to as played, noise included,
    /// little-endian `f32`, sample-major, when given. It is emptied at `configure`.
    pub record: Option<std::path::PathBuf>,
}

impl ModalPlant {
    /// A modal plant with the default feedthrough, no residual and no delay.
    pub fn new(n_inputs: usize, n_drives: usize, modes: Vec<Mode>) -> Self {
        ModalPlant {
            n_inputs,
            n_drives,
            modes,
            feedthrough: DEFAULT_FEEDTHROUGH,
            residual: Vec::new(),
            transport_delay_samples: 0,
            discretization: Discretization::Biquad,
            softening: None,
            noisy_inputs: None,
            record: None,
        }
    }

    /// The delay the discretization adds to every path, samples: the fitted modes'
    /// `delay`, which the residual and the feedthrough share.
    pub fn discretization_delay_samples(&self) -> usize {
        match self.discretization {
            Discretization::Biquad => 0,
            Discretization::Fitted { delay, .. } => delay,
        }
    }
}

/// How a [`ModalPlant`]'s modes are made filters at the sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Discretization {
    /// [`Resonator::with_response`]'s biquad, prewarped so the peak sits at `f_n`.
    /// Cheapest, and its [`PlantModel::analytic_frf`] is exact, but the bilinear map
    /// bends the frequency axis toward Nyquist: a mode well above the band adds far
    /// less below its resonance than the continuous mode does. On the 48 x 12 RC
    /// plate at 16 384 Hz the plant reads 3.3 dB low in the 4 kHz sixth-octave band.
    #[default]
    Biquad,
    /// The continuous mode's exact poles with a numerator of `taps` coefficients
    /// fitted to its response, delayed `delay` samples, on lines from 0 to `band_hz`
    /// (`crate::fitted`). Every path, residual and feedthrough included, is delayed
    /// `delay` samples. `taps + 2` multiply-adds a mode a sample against the biquad's
    /// five; nothing is held above `band_hz`.
    Fitted {
        taps: usize,
        delay: usize,
        band_hz: f64,
    },
}

impl Discretization {
    /// Nine taps behind four samples, fitted to `band_hz`: within 0.0001 dB of the
    /// continuous RC plate in every sixth-octave band to 4 kHz, fitted to 4400 Hz at
    /// 16 384 Hz.
    pub fn fitted(band_hz: f64) -> Self {
        Discretization::Fitted {
            taps: 9,
            delay: 4,
            band_hz,
        }
    }
}

/// Level-dependent softening of some of a [`ModalPlant`]'s modes. After each read of
/// `read_samples`, a softening mode's natural frequency is `f0 (1 - fraction r^2 / (1
/// + r^2))`, with `r` its running RMS modal displacement over `reference_rms`,
/// smoothed from read to read. The modal displacement is the mode's input through
/// `1 / (s^2 + s w / Q + w^2)`; its Q and shapes hold.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalSoftening {
    /// Indices into the plant's modes.
    pub modes: Vec<usize>,
    /// Each softening mode's modal displacement RMS at the reference level.
    pub reference_rms: Vec<f64>,
    /// The largest fractional frequency drop.
    pub fraction: f64,
    /// The weight of the newest read in the running mean square.
    pub smoothing: f64,
    /// Samples between frequency updates.
    pub read_samples: usize,
    /// Each read's time, s, and frequencies, Hz, are appended here as CSV, when given.
    pub log: Option<std::path::PathBuf>,
}

/// A plant that plays an FRF matrix.
///
/// # How it plays the stack, and the approximation
///
/// At the configured rate `fs`, each element's impulse response comes from the
/// inverse FFT of the stack:
///
/// 1. The FIR grid is `M = next_pow2(fs / df)` points, spacing `fs / M <= df`. The
///    stack is linearly interpolated (real and imaginary parts) onto it; above the
///    stack's last line the response is zero; the DC and Nyquist points keep their
///    real part only.
/// 2. The inverse FFT gives the `M`-sample impulse response of the stack's periodic
///    equivalent (a response longer than `1 / df` is time-aliased, which no FRF at
///    that spacing can avoid).
/// 3. It is truncated to `taps = min(M, max_taps)` samples. When that cuts it, the
///    last quarter of the kept samples is tapered by a half cosine (a half-Hann tail)
///    so the cut is not a step. No window otherwise (rectangular).
///
/// The FIR's response equals the stack exactly at the stack's lines when `fs / df` is
/// a power of two and nothing is truncated; otherwise it is the interpolated and
/// tapered approximation above, and [`PlantModel::analytic_frf`] reports exactly what
/// is played.
///
/// The convolution is uniformly partitioned overlap-save with the workspace FFT:
/// partitions of `block_frames` samples, FFTs of twice that. It adds a latency of
/// `block_frames` samples (on top of `transport_delay_samples`), which the analytic
/// FRF includes.
#[derive(Debug, Clone, PartialEq)]
pub struct FrfPlant {
    /// `lines x n_inputs x n_drives`, line `k` at `k * df_hz`.
    pub stack: MatrixStack,
    /// Line spacing of `stack`, Hz.
    pub df_hz: f64,
    /// Cap on the impulse-response length (a power of two).
    pub max_taps: usize,
    /// Partition length and added latency (a power of two; at most the tap count).
    pub block_frames: usize,
    /// Transport delay in whole samples, common to every path.
    pub transport_delay_samples: usize,
}

impl FrfPlant {
    /// Play `stack` (line `k` at `k * df_hz`) with the default tap cap and block.
    pub fn new(stack: MatrixStack, df_hz: f64) -> Self {
        FrfPlant {
            stack,
            df_hz,
            max_taps: DEFAULT_FRF_MAX_TAPS,
            block_frames: DEFAULT_FRF_BLOCK_FRAMES,
            transport_delay_samples: 0,
        }
    }

    /// The impulse responses this plant plays at sample rate `fs_hz` (see the type
    /// docs for how they are made).
    pub fn impulse_responses(&self, fs_hz: f64) -> Result<FrfTaps, String> {
        self.validate(fs_hz)?;
        let (lines, n_in, n_d) = self.stack.dims();
        let grid = fft::next_pow2(((fs_hz / self.df_hz) - 1e-9).ceil().max(2.0) as usize);
        let dg = fs_hz / grid as f64;
        let taps = grid.min(self.max_taps);
        let block = self.block_frames.min(taps);
        let last_line_hz = (lines - 1) as f64 * self.df_hz;
        let mut h = vec![0.0f64; n_in * n_d * taps];
        let mut spec: Vec<Cplx> = vec![(0.0, 0.0); grid];
        let taper_start = taps - taps / 4;
        for i in 0..n_in {
            for j in 0..n_d {
                for (m, s) in spec.iter_mut().enumerate().take(grid / 2 + 1) {
                    let f = m as f64 * dg;
                    let v = if f > last_line_hz + 1e-9 * self.df_hz {
                        C64::new(0.0, 0.0)
                    } else {
                        let x = f / self.df_hz;
                        let k0 = (x.floor() as usize).min(lines - 1);
                        let k1 = (k0 + 1).min(lines - 1);
                        let t = x - k0 as f64;
                        let a = self.stack.get(k0, i, j);
                        let b = self.stack.get(k1, i, j);
                        a + (b - a) * t
                    };
                    *s = (v.re, v.im);
                }
                spec[0].1 = 0.0;
                spec[grid / 2].1 = 0.0;
                for m in 1..grid / 2 {
                    let (re, im) = spec[m];
                    spec[grid - m] = (re, -im);
                }
                let time = inverse_dft(&spec);
                let dst = &mut h[(i * n_d + j) * taps..(i * n_d + j + 1) * taps];
                for (n, d) in dst.iter_mut().enumerate() {
                    *d = time[n].0;
                    if taps < grid && n >= taper_start {
                        let u = (n - taper_start) as f64 / (taps - taper_start) as f64;
                        *d *= 0.5 * (1.0 + (std::f64::consts::PI * u).cos());
                    }
                }
            }
        }
        Ok(FrfTaps {
            n_inputs: n_in,
            n_drives: n_d,
            taps,
            block,
            grid,
            h,
        })
    }

    fn validate(&self, fs_hz: f64) -> Result<(), String> {
        let (lines, n_in, n_d) = self.stack.dims();
        if lines < 2 || n_in == 0 || n_d == 0 {
            return Err(format!(
                "an FRF plant needs at least 2 lines and a non-empty matrix, got {lines} x {n_in} x {n_d}"
            ));
        }
        if !(self.df_hz.is_finite() && self.df_hz > 0.0) {
            return Err(format!(
                "FRF line spacing {} Hz is not positive",
                self.df_hz
            ));
        }
        if !(fs_hz.is_finite() && fs_hz > 0.0) {
            return Err(format!("sample rate {fs_hz} Hz is not positive"));
        }
        if fs_hz / self.df_hz > (1u64 << 24) as f64 {
            return Err(format!(
                "FRF line spacing {} Hz at {fs_hz} Hz needs more than 2^24 impulse-response points",
                self.df_hz
            ));
        }
        if !self.stack.is_finite() {
            return Err("the FRF stack holds a non-finite value".into());
        }
        if self.max_taps < 2 || !self.max_taps.is_power_of_two() {
            return Err(format!(
                "max_taps {} is not a power of two of at least 2",
                self.max_taps
            ));
        }
        if self.block_frames == 0 || !self.block_frames.is_power_of_two() {
            return Err(format!(
                "block_frames {} is not a power of two",
                self.block_frames
            ));
        }
        Ok(())
    }
}

/// The impulse responses an [`FrfPlant`] plays at one sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct FrfTaps {
    pub n_inputs: usize,
    pub n_drives: usize,
    /// Impulse-response length in samples.
    pub taps: usize,
    /// Convolution partition length and added latency, in samples.
    pub block: usize,
    /// The FIR grid size `M` before truncation.
    pub grid: usize,
    /// `h[(i * n_drives + j) * taps + n]`: input `i`, drive `j`, sample `n`.
    pub h: Vec<f64>,
}

impl FrfTaps {
    /// Element `(i, j)`'s impulse response.
    pub fn element(&self, i: usize, j: usize) -> &[f64] {
        let o = (i * self.n_drives + j) * self.taps;
        &self.h[o..o + self.taps]
    }
}

impl PlantModel {
    /// A short name for messages: `resonators`, `modal` or `frf`.
    pub fn kind(&self) -> &'static str {
        match self {
            PlantModel::Resonators(_) => "resonators",
            PlantModel::Modal(_) => "modal",
            PlantModel::Frf(_) => "frf",
        }
    }

    /// `(n_inputs, n_drives)` the model is built for, or `None` for the resonator
    /// bank, which fits any table.
    pub fn dims(&self) -> Option<(usize, usize)> {
        match self {
            PlantModel::Resonators(_) => None,
            PlantModel::Modal(m) => Some((m.n_inputs, m.n_drives)),
            PlantModel::Frf(f) => {
                let (_, r, c) = f.stack.dims();
                Some((r, c))
            }
        }
    }

    /// The transport delay of whichever model this is.
    pub fn transport_delay_samples(&self) -> usize {
        match self {
            PlantModel::Resonators(b) => b.transport_delay_samples,
            PlantModel::Modal(m) => m.transport_delay_samples,
            PlantModel::Frf(f) => f.transport_delay_samples,
        }
    }

    /// Set the transport delay of whichever model this is.
    pub fn set_transport_delay_samples(&mut self, n: usize) {
        match self {
            PlantModel::Resonators(b) => b.transport_delay_samples = n,
            PlantModel::Modal(m) => m.transport_delay_samples = n,
            PlantModel::Frf(f) => f.transport_delay_samples = n,
        }
    }

    /// Check the model can run at `fs_hz`; the message says what is wrong.
    pub fn validate(&self, fs_hz: f64) -> Result<(), String> {
        if !(fs_hz.is_finite() && fs_hz > 0.0) {
            return Err(format!("sample rate {fs_hz} Hz is not positive"));
        }
        match self {
            PlantModel::Resonators(_) => Ok(()),
            PlantModel::Modal(m) => {
                if m.n_inputs == 0 || m.n_drives == 0 {
                    return Err(format!(
                        "a modal plant needs at least one input and one drive, got {} x {}",
                        m.n_inputs, m.n_drives
                    ));
                }
                if !(m.residual.is_empty() || m.residual.len() == m.n_inputs * m.n_drives) {
                    return Err(format!(
                        "the residual holds {} values; a {} x {} plant needs {} or none",
                        m.residual.len(),
                        m.n_inputs,
                        m.n_drives,
                        m.n_inputs * m.n_drives
                    ));
                }
                if !m.feedthrough.is_finite() || m.residual.iter().any(|v| !v.is_finite()) {
                    return Err("a modal plant's feedthrough or residual is not finite".into());
                }
                for (k, mode) in m.modes.iter().enumerate() {
                    if mode.input_shape.len() != m.n_drives || mode.output_shape.len() != m.n_inputs
                    {
                        return Err(format!(
                            "mode {k} has shapes {} (drives) and {} (inputs); the plant is {} x {}",
                            mode.input_shape.len(),
                            mode.output_shape.len(),
                            m.n_inputs,
                            m.n_drives
                        ));
                    }
                    if !(mode.f_hz.is_finite() && mode.f_hz > 0.0 && mode.f_hz < 0.5 * fs_hz) {
                        return Err(format!(
                            "mode {k} at {} Hz is not between 0 and the Nyquist frequency of {fs_hz} Hz",
                            mode.f_hz
                        ));
                    }
                    if !(mode.q.is_finite() && mode.q > 0.0) {
                        return Err(format!("mode {k} has Q {}", mode.q));
                    }
                    if mode
                        .input_shape
                        .iter()
                        .chain(&mode.output_shape)
                        .any(|v| !v.is_finite())
                    {
                        return Err(format!("mode {k} has a non-finite shape value"));
                    }
                }
                if let Discretization::Fitted {
                    taps,
                    delay,
                    band_hz,
                } = m.discretization
                {
                    if taps < 3 || delay >= taps {
                        return Err(format!(
                            "a fitted mode needs at least 3 taps and a delay under them, got \
                             {taps} taps and {delay} samples"
                        ));
                    }
                    if !(band_hz.is_finite() && band_hz > 0.0 && band_hz < 0.5 * fs_hz) {
                        return Err(format!(
                            "a fitted mode's band, {band_hz} Hz, is not between 0 and the \
                             Nyquist frequency of {fs_hz} Hz"
                        ));
                    }
                }
                if m.noisy_inputs.is_some_and(|n| n > m.n_inputs) {
                    return Err(format!(
                        "noise on {} inputs of a plant with {}",
                        m.noisy_inputs.unwrap_or(0),
                        m.n_inputs
                    ));
                }
                if let Some(soft) = &m.softening {
                    if !matches!(m.discretization, Discretization::Fitted { .. }) {
                        return Err("softening needs the fitted discretization".into());
                    }
                    if soft.reference_rms.len() != soft.modes.len()
                        || soft.modes.iter().any(|&k| k >= m.modes.len())
                    {
                        return Err(
                            "softening: one reference RMS per softening mode, each a mode of \
                             the plant"
                                .into(),
                        );
                    }
                    if soft.read_samples == 0
                        || soft
                            .reference_rms
                            .iter()
                            .any(|r| !(r.is_finite() && *r > 0.0))
                    {
                        return Err(
                            "softening: a read is at least one sample and each reference RMS \
                             is positive"
                                .into(),
                        );
                    }
                }
                Ok(())
            }
            PlantModel::Frf(f) => f.validate(fs_hz),
        }
    }

    /// The exact complex FRF of the plant discretized at `fs_hz`, on lines
    /// `0, df, 2 df, ...`: `lines x n_inputs x n_drives` (`lines x 1 x 1` for the
    /// resonator bank, the one path from the first drive that every input reads).
    ///
    /// Panics when the model does not validate at `fs_hz`.
    pub fn analytic_frf(&self, lines: usize, df_hz: f64, fs_hz: f64) -> MatrixStack {
        if let PlantModel::Frf(f) = self {
            if let Some(stack) = frf_on_fft_grid(f, lines, df_hz, fs_hz) {
                return stack;
            }
        }
        let freqs: Vec<f64> = (0..lines).map(|k| k as f64 * df_hz).collect();
        self.analytic_frf_at(&freqs, fs_hz)
    }

    /// [`Self::analytic_frf`] at arbitrary frequencies (one stack line per entry).
    ///
    /// Panics when the model does not validate at `fs_hz`.
    pub fn analytic_frf_at(&self, freqs_hz: &[f64], fs_hz: f64) -> MatrixStack {
        if let Err(e) = self.validate(fs_hz) {
            panic!("analytic_frf on an invalid {} plant: {e}", self.kind());
        }
        let delay = |f: f64, samples: usize| -> C64 {
            let ph = -TAU * f * samples as f64 / fs_hz;
            C64::new(ph.cos(), ph.sin())
        };
        match self {
            PlantModel::Resonators(b) => {
                let rs: Vec<Resonator> = b
                    .resonances
                    .iter()
                    .map(|&(f_n, q)| Resonator::new(f_n, q, fs_hz))
                    .collect();
                let mut out = MatrixStack::zeros(freqs_hz.len(), 1, 1);
                for (k, &f) in freqs_hz.iter().enumerate() {
                    let mut h = C64::new(b.feedthrough, 0.0);
                    for r in &rs {
                        h += r.response(f, fs_hz);
                    }
                    out.set(k, 0, 0, h * delay(f, b.transport_delay_samples));
                }
                out
            }
            PlantModel::Modal(m) => {
                let filters = ModeFilters::new(m, fs_hz);
                let (n_in, n_d) = (m.n_inputs, m.n_drives);
                let mut out = MatrixStack::zeros(freqs_hz.len(), n_in, n_d);
                let mut rm = vec![C64::new(0.0, 0.0); m.modes.len()];
                for (k, &f) in freqs_hz.iter().enumerate() {
                    filters.responses(f, fs_hz, &mut rm);
                    // The fitted modes carry their delay; the residual and the
                    // feedthrough are delayed with them.
                    let dl = delay(f, m.transport_delay_samples);
                    let shared = delay(f, m.discretization_delay_samples());
                    let line = out.line_mut(k);
                    for i in 0..n_in {
                        for j in 0..n_d {
                            let mut h = C64::new(m.feedthrough, 0.0);
                            if !m.residual.is_empty() {
                                h.re += m.residual[i * n_d + j];
                            }
                            h *= shared;
                            for (mode, r) in m.modes.iter().zip(&rm) {
                                h += r * (mode.output_shape[i] * mode.input_shape[j]);
                            }
                            line[i * n_d + j] = h * dl;
                        }
                    }
                }
                out
            }
            PlantModel::Frf(p) => {
                let taps = p
                    .impulse_responses(fs_hz)
                    .expect("validated above, so the taps build");
                let mut out = MatrixStack::zeros(freqs_hz.len(), taps.n_inputs, taps.n_drives);
                for (k, &f) in freqs_hz.iter().enumerate() {
                    let dl = delay(f, p.transport_delay_samples + taps.block);
                    let w = -TAU * f / fs_hz;
                    for i in 0..taps.n_inputs {
                        for j in 0..taps.n_drives {
                            let mut acc = C64::new(0.0, 0.0);
                            for (n, &v) in taps.element(i, j).iter().enumerate() {
                                if v != 0.0 {
                                    let ph = w * n as f64;
                                    acc += C64::new(v * ph.cos(), v * ph.sin());
                                }
                            }
                            out.set(k, i, j, acc * dl);
                        }
                    }
                }
                out
            }
        }
    }
}

/// The sign of the workspace FFT's forward exponent, measured: `-1` for the textbook
/// `X[k] = sum x[n] exp(-j 2 pi k n / N)`, `+1` for the other. Its documentation and its
/// butterflies have disagreed (the forward transform uses the positive exponent), so
/// every transform here whose meaning depends on the sign asks rather than assumes.
/// (The block convolution does not depend on it: a matched forward and inverse pair
/// convolves either way.)
fn fft_exponent_sign() -> f64 {
    let x = fft::fft(&[(0.0, 0.0), (1.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
    if x[1].1 > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// The textbook inverse DFT, `x[n] = (1/N) sum_k X[k] exp(+j 2 pi k n / N)`, whichever
/// sign the workspace FFT uses.
fn inverse_dft(spec: &[Cplx]) -> Vec<Cplx> {
    if fft_exponent_sign() < 0.0 {
        fft::ifft(spec)
    } else {
        let inv = 1.0 / spec.len() as f64;
        fft::fft(spec)
            .into_iter()
            .map(|(re, im)| (re * inv, im * inv))
            .collect()
    }
}

/// The FRF plant's analytic response by one FFT per element, when the requested grid
/// is an FFT grid of the configured rate (`fs / df` a power of two, at least the tap
/// count). `None` sends the caller to the direct sum.
fn frf_on_fft_grid(p: &FrfPlant, lines: usize, df_hz: f64, fs_hz: f64) -> Option<MatrixStack> {
    if !(df_hz.is_finite() && df_hz > 0.0) {
        return None;
    }
    let ratio = fs_hz / df_hz;
    let n = ratio.round();
    if (ratio - n).abs() > 1e-9 * ratio || n < 2.0 || n > (1u64 << 26) as f64 {
        return None;
    }
    let n = n as usize;
    if !n.is_power_of_two() || lines > n / 2 + 1 {
        return None;
    }
    let taps = p.impulse_responses(fs_hz).ok()?;
    if n < taps.taps {
        return None;
    }
    let mut out = MatrixStack::zeros(lines, taps.n_inputs, taps.n_drives);
    let mut buf: Vec<Cplx> = vec![(0.0, 0.0); n];
    let lag = p.transport_delay_samples + taps.block;
    // A real `h` transformed with the positive exponent is the conjugate of the
    // negative-exponent transform this FRF is defined by.
    let conj = fft_exponent_sign() > 0.0;
    for i in 0..taps.n_inputs {
        for j in 0..taps.n_drives {
            buf.iter_mut().for_each(|s| *s = (0.0, 0.0));
            for (s, &v) in buf.iter_mut().zip(taps.element(i, j)) {
                s.0 = v;
            }
            fft::fft_in_place(&mut buf);
            for (k, &(re, im)) in buf.iter().enumerate().take(lines) {
                let ph = -TAU * (k as f64 * df_hz) * lag as f64 / fs_hz;
                let dl = C64::new(ph.cos(), ph.sin());
                let v = C64::new(re, if conj { -im } else { im });
                out.set(k, i, j, v * dl);
            }
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------
// The running plant (crate-private).
// ---------------------------------------------------------------------------------------

/// A plant built from a [`PlantModel`] at one sample rate, stepped one frame at a time.
#[derive(Debug, Clone)]
pub(crate) enum Plant {
    Bank(BankPlant),
    Modal(Box<ModalRun>),
    Frf(Box<FrfRun>),
}

impl Plant {
    pub(crate) fn build(
        model: &PlantModel,
        fs_hz: f64,
        noise_level: f64,
        seed: u64,
    ) -> Result<Plant, String> {
        model.validate(fs_hz)?;
        Ok(match model {
            PlantModel::Resonators(b) => Plant::Bank(BankPlant::new(b, fs_hz, noise_level, seed)),
            PlantModel::Modal(m) => {
                Plant::Modal(Box::new(ModalRun::new(m, fs_hz, noise_level, seed)))
            }
            PlantModel::Frf(f) => Plant::Frf(Box::new(FrfRun::new(f, fs_hz, noise_level, seed)?)),
        })
    }

    /// Responses per frame: one for the bank (every input reads it), the model's input
    /// count otherwise.
    pub(crate) fn n_outputs(&self) -> usize {
        match self {
            Plant::Bank(_) => 1,
            Plant::Modal(m) => m.n_out,
            Plant::Frf(f) => f.n_out,
        }
    }

    /// Advance one frame: `drive` is the frame's drive values (every drive row),
    /// `out` receives [`Self::n_outputs`] responses.
    #[inline]
    pub(crate) fn step(&mut self, drive: &[f64], out: &mut [f64]) {
        match self {
            Plant::Bank(b) => out[0] = b.step(drive.first().copied().unwrap_or(0.0)),
            Plant::Modal(m) => m.step(drive, out),
            Plant::Frf(f) => f.step(drive, out),
        }
    }

    pub(crate) fn reset(&mut self) {
        match self {
            Plant::Bank(b) => b.reset(),
            Plant::Modal(m) => m.reset(),
            Plant::Frf(f) => f.reset(),
        }
    }
}

/// The original single-path plant, unchanged: a bank of resonators plus feedthrough
/// plus noise behind a transport delay. Kept sample-for-sample identical to the plant
/// it replaced (one noise draw per frame, shared by every input).
#[derive(Debug, Clone)]
pub(crate) struct BankPlant {
    resonators: Vec<Resonator>,
    delay: DelayLine,
    rng: Xorshift64,
    noise_level: f64,
    feedthrough: f64,
    // Last drive value seen (for emergency ramp-down / hold).
    last_drive: f64,
}

impl BankPlant {
    fn new(b: &ResonatorBank, fs_hz: f64, noise_level: f64, seed: u64) -> Self {
        BankPlant {
            resonators: b
                .resonances
                .iter()
                .map(|&(f_n, q)| Resonator::new(f_n, q, fs_hz))
                .collect(),
            delay: DelayLine::new(b.transport_delay_samples),
            rng: Xorshift64::new(seed),
            noise_level,
            feedthrough: b.feedthrough,
            last_drive: 0.0,
        }
    }

    #[inline]
    fn step(&mut self, drive: f64) -> f64 {
        self.last_drive = drive;
        let delayed = self.delay.process(drive);
        let mut resonant = 0.0;
        for r in &mut self.resonators {
            resonant += r.process(delayed);
        }
        // Broadband feedthrough keeps off-resonance response non-zero and finite.
        let response = resonant + self.feedthrough * delayed;
        let noise = self.noise_level * self.rng.next_gaussian();
        response + noise
    }

    fn reset(&mut self) {
        self.resonators.iter_mut().for_each(Resonator::reset);
        self.delay.reset();
        self.last_drive = 0.0;
    }
}

/// Independent Gaussian noise for `out.len()` inputs from one seeded stream, two
/// values per Box-Muller draw.
#[inline]
fn add_noise(rng: &mut Xorshift64, sigma: f64, out: &mut [f64]) {
    if sigma == 0.0 {
        return;
    }
    let mut chunks = out.chunks_exact_mut(2);
    for pair in &mut chunks {
        let (a, b) = rng.next_gaussian_pair();
        pair[0] += sigma * a;
        pair[1] += sigma * b;
    }
    if let [last] = chunks.into_remainder() {
        *last += sigma * rng.next_gaussian_pair().0;
    }
}

/// A modal plant's modes as filters at the sample rate, one per mode.
#[derive(Debug, Clone)]
pub(crate) enum ModeFilters {
    Biquad(Vec<Resonator>),
    Fitted {
        modes: Vec<FittedMode>,
        delay: usize,
        band_hz: f64,
    },
}

impl ModeFilters {
    pub(crate) fn new(m: &ModalPlant, fs_hz: f64) -> Self {
        match m.discretization {
            Discretization::Biquad => ModeFilters::Biquad(
                m.modes
                    .iter()
                    .map(|mode| Resonator::with_response(mode.f_hz, mode.q, fs_hz, mode.response))
                    .collect(),
            ),
            Discretization::Fitted {
                taps,
                delay,
                band_hz,
            } => {
                let softening = m
                    .softening
                    .as_ref()
                    .map(|s| s.modes.as_slice())
                    .unwrap_or(&[]);
                let modes = m
                    .modes
                    .iter()
                    .enumerate()
                    .map(|(k, mode)| {
                        let mut f = FittedMode::new(
                            mode.f_hz,
                            mode.q,
                            mode.response,
                            softening.contains(&k),
                            taps,
                        );
                        f.tune(mode.f_hz, fs_hz, delay, band_hz);
                        f
                    })
                    .collect();
                ModeFilters::Fitted {
                    modes,
                    delay,
                    band_hz,
                }
            }
        }
    }

    /// Each mode's response at `f_hz`, into `out`.
    fn responses(&self, f_hz: f64, fs_hz: f64, out: &mut [C64]) {
        match self {
            ModeFilters::Biquad(rs) => {
                for (r, v) in rs.iter().zip(out) {
                    *v = r.response(f_hz, fs_hz);
                }
            }
            ModeFilters::Fitted { modes, .. } => {
                for (r, v) in modes.iter().zip(out) {
                    *v = r.response(f_hz, fs_hz);
                }
            }
        }
    }

    #[inline]
    fn process(&mut self, k: usize, u: f64) -> f64 {
        match self {
            ModeFilters::Biquad(rs) => rs[k].process(u),
            ModeFilters::Fitted { modes, .. } => modes[k].process(u),
        }
    }

    fn displacement(&self, k: usize) -> f64 {
        match self {
            ModeFilters::Biquad(_) => 0.0,
            ModeFilters::Fitted { modes, .. } => modes[k].displacement(),
        }
    }

    fn retune(&mut self, k: usize, f_n: f64, fs_hz: f64) {
        if let ModeFilters::Fitted {
            modes,
            delay,
            band_hz,
        } = self
        {
            modes[k].tune(f_n, fs_hz, *delay, *band_hz);
        }
    }

    fn reset(&mut self) {
        match self {
            ModeFilters::Biquad(rs) => rs.iter_mut().for_each(Resonator::reset),
            ModeFilters::Fitted { modes, .. } => modes.iter_mut().for_each(FittedMode::reset),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ModalRun {
    n_out: usize,
    n_drive: usize,
    /// `n_modes x n_drive`, row-major.
    phi: Vec<f64>,
    /// `n_modes x n_out`, row-major (each mode's output shape, contiguous).
    psi: Vec<f64>,
    filters: ModeFilters,
    /// Non-zero residual entries `(input, drive, value)`.
    residual: Vec<(usize, usize, f64)>,
    feedthrough: f64,
    delays: Vec<DelayLine>,
    delayed: Vec<f64>,
    /// The discretization's delay on the residual and the feedthrough, per drive.
    shared: Vec<DelayLine>,
    shared_out: Vec<f64>,
    rng: Xorshift64,
    noise_level: f64,
    noisy: usize,
    fs_hz: f64,
    softening: Option<SofteningRun>,
    soft_modes: Vec<usize>,
    recorder: Option<Recorder>,
}

impl ModalRun {
    fn new(m: &ModalPlant, fs_hz: f64, noise_level: f64, seed: u64) -> Self {
        let mut phi = Vec::with_capacity(m.modes.len() * m.n_drives);
        let mut psi = Vec::with_capacity(m.modes.len() * m.n_inputs);
        for mode in &m.modes {
            phi.extend_from_slice(&mode.input_shape);
            psi.extend_from_slice(&mode.output_shape);
        }
        let residual = if m.residual.is_empty() {
            Vec::new()
        } else {
            (0..m.n_inputs)
                .flat_map(|i| (0..m.n_drives).map(move |j| (i, j)))
                .filter_map(|(i, j)| {
                    let v = m.residual[i * m.n_drives + j];
                    (v != 0.0).then_some((i, j, v))
                })
                .collect()
        };
        let softening = m
            .softening
            .as_ref()
            .map(|s| SofteningRun::new(s, s.modes.iter().map(|&k| m.modes[k].f_hz).collect()));
        ModalRun {
            n_out: m.n_inputs,
            n_drive: m.n_drives,
            phi,
            psi,
            filters: ModeFilters::new(m, fs_hz),
            residual,
            feedthrough: m.feedthrough,
            delays: (0..m.n_drives)
                .map(|_| DelayLine::new(m.transport_delay_samples))
                .collect(),
            delayed: vec![0.0; m.n_drives],
            shared: (0..m.n_drives)
                .map(|_| DelayLine::new(m.discretization_delay_samples()))
                .collect(),
            shared_out: vec![0.0; m.n_drives],
            rng: Xorshift64::new(seed),
            noise_level,
            noisy: m.noisy_inputs.unwrap_or(m.n_inputs).min(m.n_inputs),
            fs_hz,
            soft_modes: m
                .softening
                .as_ref()
                .map(|s| s.modes.clone())
                .unwrap_or_default(),
            softening,
            recorder: m.record.clone().map(Recorder::new),
        }
    }

    #[inline]
    fn step(&mut self, drive: &[f64], out: &mut [f64]) {
        let out = &mut out[..self.n_out];
        let mut sum = 0.0;
        for (((d, line), (s, shared)), &x) in self
            .delayed
            .iter_mut()
            .zip(&mut self.delays)
            .zip(self.shared_out.iter_mut().zip(&mut self.shared))
            .zip(drive)
        {
            *d = line.process(x);
            *s = shared.process(*d);
            sum += *s;
        }
        out.fill(self.feedthrough * sum);
        for &(i, j, v) in &self.residual {
            out[i] += v * self.shared_out[j];
        }
        let nd = self.n_drive;
        let no = self.n_out;
        for m in 0..self.phi.len() / nd {
            let u: f64 = self.phi[m * nd..(m + 1) * nd]
                .iter()
                .zip(&self.delayed)
                .map(|(a, b)| a * b)
                .sum();
            let x = self.filters.process(m, u);
            for (o, &s) in out.iter_mut().zip(&self.psi[m * no..(m + 1) * no]) {
                *o += s * x;
            }
        }
        add_noise(&mut self.rng, self.noise_level, &mut out[..self.noisy]);
        if let Some(soft) = &mut self.softening {
            let filters = &self.filters;
            let read = soft.sample(
                self.soft_modes.iter().map(|&k| filters.displacement(k)),
                self.fs_hz,
            );
            if let Some(f) = read {
                for (&k, f) in self.soft_modes.iter().zip(f) {
                    self.filters.retune(k, f, self.fs_hz);
                }
            }
        }
        if let Some(r) = &mut self.recorder {
            r.record(out);
        }
    }

    fn reset(&mut self) {
        self.filters.reset();
        self.delays.iter_mut().for_each(DelayLine::reset);
        self.shared.iter_mut().for_each(DelayLine::reset);
        self.delayed.iter_mut().for_each(|v| *v = 0.0);
        self.shared_out.iter_mut().for_each(|v| *v = 0.0);
        if let Some(soft) = &mut self.softening {
            for (&k, f) in self.soft_modes.iter().zip(soft.reset()) {
                self.filters.retune(k, f, self.fs_hz);
            }
        }
        if let Some(r) = &mut self.recorder {
            r.flush();
        }
    }
}

/// Uniformly partitioned overlap-save convolution of `n_drive` streams through an
/// `n_out x n_drive` FIR matrix.
#[derive(Debug, Clone)]
pub(crate) struct FrfRun {
    n_out: usize,
    n_drive: usize,
    /// Partition length (= latency).
    p: usize,
    /// Partitions per element.
    k: usize,
    /// Partition spectra, bins `0..=p` of each `2p` FFT:
    /// `[((i * n_drive + j) * k + q) * (p + 1) + b]`.
    hspec: Vec<Cplx>,
    /// Per drive, the last `2p` input samples (previous block, then the current one).
    in_buf: Vec<f64>,
    /// Frequency-domain delay line: per drive, `k` input-block spectra, newest at
    /// `fdl_head`: `[(j * k + slot) * (p + 1) + b]`.
    fdl: Vec<Cplx>,
    fdl_head: usize,
    /// The block of outputs being emitted now: `[i * p + t]`.
    out_q: Vec<f64>,
    pos: usize,
    scratch: Vec<Cplx>,
    acc: Vec<Cplx>,
    delays: Vec<DelayLine>,
    rng: Xorshift64,
    noise_level: f64,
}

impl FrfRun {
    fn new(f: &FrfPlant, fs_hz: f64, noise_level: f64, seed: u64) -> Result<Self, String> {
        let taps = f.impulse_responses(fs_hz)?;
        let p = taps.block;
        let k = taps.taps / p;
        let n2 = 2 * p;
        let (n_out, n_drive) = (taps.n_inputs, taps.n_drives);
        let mut hspec = vec![(0.0, 0.0); n_out * n_drive * k * (p + 1)];
        let mut buf: Vec<Cplx> = vec![(0.0, 0.0); n2];
        for i in 0..n_out {
            for j in 0..n_drive {
                let h = taps.element(i, j);
                for q in 0..k {
                    buf.iter_mut().for_each(|s| *s = (0.0, 0.0));
                    for (s, &v) in buf.iter_mut().zip(&h[q * p..(q + 1) * p]) {
                        s.0 = v;
                    }
                    fft::fft_in_place(&mut buf);
                    let o = ((i * n_drive + j) * k + q) * (p + 1);
                    hspec[o..o + p + 1].copy_from_slice(&buf[..p + 1]);
                }
            }
        }
        Ok(FrfRun {
            n_out,
            n_drive,
            p,
            k,
            hspec,
            in_buf: vec![0.0; n_drive * n2],
            fdl: vec![(0.0, 0.0); n_drive * k * (p + 1)],
            fdl_head: 0,
            out_q: vec![0.0; n_out * p],
            pos: 0,
            scratch: buf,
            acc: vec![(0.0, 0.0); p + 1],
            delays: (0..n_drive)
                .map(|_| DelayLine::new(f.transport_delay_samples))
                .collect(),
            rng: Xorshift64::new(seed),
            noise_level,
        })
    }

    #[inline]
    fn step(&mut self, drive: &[f64], out: &mut [f64]) {
        let p = self.p;
        let out = &mut out[..self.n_out];
        // Emit the previous block's output for this position (latency `p`).
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.out_q[i * p + self.pos];
        }
        // Ingest this frame's drive (after the transport delay).
        for (j, (line, &x)) in self.delays.iter_mut().zip(drive).enumerate() {
            self.in_buf[j * 2 * p + p + self.pos] = line.process(x);
        }
        self.pos += 1;
        if self.pos == p {
            self.pos = 0;
            self.convolve_block();
        }
        add_noise(&mut self.rng, self.noise_level, out);
    }

    /// One block: transform each drive's last `2p` samples into the delay line, sum
    /// the partition products per output, inverse transform, keep the last `p`.
    fn convolve_block(&mut self) {
        let (p, k, nd) = (self.p, self.k, self.n_drive);
        let n2 = 2 * p;
        let bins = p + 1;
        self.fdl_head = (self.fdl_head + k - 1) % k;
        for j in 0..nd {
            let src = &mut self.in_buf[j * n2..(j + 1) * n2];
            for (s, &v) in self.scratch.iter_mut().zip(src.iter()) {
                *s = (v, 0.0);
            }
            src.copy_within(p..n2, 0);
            fft::fft_in_place(&mut self.scratch);
            let o = (j * k + self.fdl_head) * bins;
            self.fdl[o..o + bins].copy_from_slice(&self.scratch[..bins]);
        }
        let inv = 1.0 / n2 as f64;
        for i in 0..self.n_out {
            self.acc.iter_mut().for_each(|a| *a = (0.0, 0.0));
            for j in 0..nd {
                for q in 0..k {
                    let slot = (self.fdl_head + q) % k;
                    let x = &self.fdl[(j * k + slot) * bins..(j * k + slot + 1) * bins];
                    let ho = ((i * nd + j) * k + q) * bins;
                    let h = &self.hspec[ho..ho + bins];
                    for ((a, &(hr, hi)), &(xr, xi)) in self.acc.iter_mut().zip(h).zip(x) {
                        a.0 += hr * xr - hi * xi;
                        a.1 += hr * xi + hi * xr;
                    }
                }
            }
            // Inverse transform of the Hermitian spectrum through the forward FFT:
            // ifft(Y) = conj(fft(conj(Y))) / N, and the result is real.
            for b in 0..bins {
                let (re, im) = self.acc[b];
                self.scratch[b] = (re, -im);
                if b > 0 && b < p {
                    self.scratch[n2 - b] = (re, im);
                }
            }
            self.scratch[0].1 = 0.0;
            self.scratch[p].1 = 0.0;
            fft::fft_in_place(&mut self.scratch);
            for t in 0..p {
                self.out_q[i * p + t] = self.scratch[p + t].0 * inv;
            }
        }
    }

    fn reset(&mut self) {
        self.in_buf.iter_mut().for_each(|v| *v = 0.0);
        self.fdl.iter_mut().for_each(|v| *v = (0.0, 0.0));
        self.out_q.iter_mut().for_each(|v| *v = 0.0);
        self.fdl_head = 0;
        self.pos = 0;
        self.delays.iter_mut().for_each(DelayLine::reset);
    }
}
