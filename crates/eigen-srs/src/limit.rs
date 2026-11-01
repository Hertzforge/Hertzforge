//! SRS predictive-notch response limiting.
//!
//! Implements the FRF-based predictive notching of Skousen, T. J.; Coleman, R. G.;
//! Smallwood, D. O., "Response Limited Shaker Shock Testing," report SAND2016-1335C
//! (86th Shock & Vibration Symposium, 2015; OSTI 1344695) and its Sound & Vibration
//! March-2017 companion: measure the FRF from the drive to each non-control LIMIT
//! channel at reduced level, predict each limit channel's full-level response, NOTCH
//! the drive per frequency line where a prediction exceeds its bound, re-synthesize,
//! and iterate.
//!
//! # The three grids (the load-bearing domain split)
//!
//! Three distinct frequency grids are in play and are NEVER assumed 1:1:
//!
//! 1. **SRS natural-frequency bands** (`band_freqs`, typically a fractional-octave
//!    grid): where the target SRS, the limit SRS bounds, and the predicted response
//!    SRS live.
//! 2. **Synthesis components** (the WAVSYN / SEDS table rows): what the outer
//!    SRS-matching iteration and the run-time law actually adjust.
//! 3. **FFT drive bins** (the `k * fs / nfft` grid): where the Coleman/Smallwood
//!    notch itself operates.
//!
//! The notch is computed in the DRIVE-SPECTRUM domain (grid 3). The projection back
//! to per-component gains (grid 2) is a LEAST-SQUARES RE-FIT: solve for the
//! component gain vector whose superposition best matches the notched drive
//! spectrum. It is NOT a per-line copy (there is no line-to-component
//! correspondence to copy through). The SRS-matching gain iteration
//! ([`crate::iterate`]) is a SEPARATE outer loop; this module only hands it
//! per-component caps.
//!
//! # The algorithm ([`PredictiveNotch::plan`])
//!
//! Given the current synthesized drive (as its per-component waveforms, so the
//! superposition IS the drive) and each limit channel's measured FRF magnitude:
//!
//! 1. **Predict** each limit channel's response spectrum: `R_c[k] = D[k] * |H_c[k]|`
//!    (the drive FFT times the FRF magnitude interpolated onto the FFT bin grid).
//! 2. **Score** the prediction as an SRS: inverse-transform `R_c` to a predicted
//!    response time history and run [`eigen_dsp::srs::srs_analyze`] on the band grid.
//!    (Magnitude-only FRFs give a ZERO-PHASE response model - see Honesty below.)
//! 3. **Find exceedances**: bands where any channel's predicted SRS exceeds its
//!    limit minus the headroom (`allowed = limit * 10^(-headroom_db/20)`).
//! 4. **Notch the drive spectrum**: every FFT bin inside an exceeding band's
//!    geometric edges is attenuated by the worst (smallest) `allowed / predicted`
//!    ratio over the channels exceeding there.
//! 5. **Least-squares re-fit**: solve for the per-component gain vector
//!    `g` minimizing `sum_k | sum_j g_j * C_j[k] - T[k] |^2`, where `C_j` are the
//!    component spectra and `T` is the notched drive spectrum, then clamp each
//!    `g_j` into `[0, previous g_j]` so the update is SAFETY-MONOTONE (a notch can
//!    only deepen within one plan; limit-down always wins).
//! 6. **Iterate** 1-5 (SRS bands couple across bins, so one pass under-delivers) up
//!    to [`MAX_REFINE_ITERS`] or until every prediction is under its allowed bound.
//!
//! The result is the reduced per-component gain vector plus a [`NotchReport`]. The
//! caller feeds `gains * current_amplitudes` to
//! [`crate::iterate::synthesize_to_target_capped`] as per-component amplitude CAPS,
//! so the SRS-matching outer loop may NOT re-inflate a notched component beyond its
//! limited gain (the Random arbitration rule: limit-down beats control-up).
//!
//! # Honesty notes
//!
//! - The FRF here is a MAGNITUDE (from
//!   [`eigen_dsp::frf::h_estimate_hv_with_coherence`] at reduced level, per
//!   SAND2016-1335C's reduced-level system ID). Prediction through `|H|` is a
//!   zero-phase response model: per-bin amplitudes are right, inter-bin phase
//!   alignment is not, so the predicted SRS is an estimate, not a bound. The
//!   `headroom_db` knob exists to absorb exactly that modeling slack.
//! - Outside the measured FRF grid the magnitude is HELD at the nearest measured
//!   endpoint (not zeroed): out-of-band drive content is never silently treated as
//!   producing zero response.
//! - Scaling the drive scales the prediction linearly (linear plant assumption -
//!   the same assumption the FRF itself encodes).
//!
//! # References
//!
//! - Skousen, Coleman, Smallwood, "Response Limited Shaker Shock Testing,"
//!   SAND2016-1335C, OSTI 1344695.
//! - The Random arbitration rule (`eigen-random::control`, limit-down beats
//!   control-up).

use eigen_dsp::fft;
use eigen_dsp::srs::srs_analyze;
use serde::{Deserialize, Serialize};

/// Maximum notch refine iterations inside one [`PredictiveNotch::plan`] call. SRS
/// bands couple across FFT bins (an SDOF at `f_n` responds to energy near, not only
/// at, `f_n`), so a single notch pass under-delivers; the loop re-predicts and
/// deepens until every prediction is under its allowed bound. If the plan still
/// predicts an exceedance after this many passes it returns honestly with
/// `converged = false`.
pub const MAX_REFINE_ITERS: usize = 20;

/// Extra notch depth (dB) applied on top of the exact `allowed / predicted` ratio
/// each pass. The exact ratio only converges to the bound ASYMPTOTICALLY (the band's
/// SRS keeps a contribution from bins outside the notch, so each pass removes a
/// fraction of the remaining exceedance); a small fixed over-notch makes the loop
/// CROSS below the bound in finitely many passes. The cost is up to this much extra
/// attenuation on the notched bands - well inside the headroom budget.
pub const OVER_NOTCH_DB: f64 = 0.25;

/// A non-control location with a response limit (SAND2016-1335C's "limit channel").
///
/// Carries the measured FRF magnitude from the DRIVE to this channel (from a
/// reduced-level system ID - [`eigen_dsp::frf::h_estimate_hv_with_coherence`] is the
/// estimator, its magnitude sampled on `frf_freqs`) and the SRS limit curve the
/// channel's predicted response must stay under, one entry per analysis band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitChannel {
    /// Channel name (for reports / UI).
    pub name: String,
    /// FRF magnitude `|H(f)|` drive -> this channel, one entry per `frf_freqs` point
    /// (dimensionless for g-per-g accel channels). Linearly interpolated onto the
    /// FFT bin grid; held at the endpoints outside the measured range.
    pub frf_mag: Vec<f64>,
    /// The frequencies (Hz, ascending) the FRF magnitude is sampled on - the
    /// FFT/analysis grid of the system-ID estimate.
    pub frf_freqs: Vec<f64>,
    /// The SRS limit curve, one entry per analysis band (same grid as the
    /// `band_freqs` handed to [`PredictiveNotch::plan`]), in the channel's response
    /// units (g for accel). `f64::INFINITY` = no limit at that band.
    pub limit_srs: Vec<f64>,
}

impl LimitChannel {
    /// Validate the channel against an analysis grid of `nbands` bands.
    pub fn validate(&self, nbands: usize) -> Result<(), NotchError> {
        if self.frf_freqs.is_empty() || self.frf_freqs.len() != self.frf_mag.len() {
            return Err(NotchError::BadConfig(format!(
                "limit channel '{}': frf_freqs (len {}) and frf_mag (len {}) must be non-empty \
                 and the same length",
                self.name,
                self.frf_freqs.len(),
                self.frf_mag.len()
            )));
        }
        let mut prev = 0.0f64;
        for &f in &self.frf_freqs {
            if !f.is_finite() || f < 0.0 || f < prev {
                return Err(NotchError::BadConfig(format!(
                    "limit channel '{}': frf_freqs must be finite, nonnegative, ascending",
                    self.name
                )));
            }
            prev = f;
        }
        if self.frf_mag.iter().any(|&m| !m.is_finite() || m < 0.0) {
            return Err(NotchError::BadConfig(format!(
                "limit channel '{}': frf_mag entries must be finite and >= 0",
                self.name
            )));
        }
        if self.limit_srs.len() != nbands {
            return Err(NotchError::BadConfig(format!(
                "limit channel '{}': limit_srs has {} entries but the analysis grid has {nbands} \
                 bands",
                self.name,
                self.limit_srs.len()
            )));
        }
        if self.limit_srs.iter().any(|&l| l.is_nan() || l <= 0.0) {
            return Err(NotchError::BadConfig(format!(
                "limit channel '{}': limit_srs entries must be > 0 (use +inf for 'no limit')",
                self.name
            )));
        }
        Ok(())
    }

    /// The FRF magnitude linearly interpolated at `f_hz`, held at the endpoints
    /// outside the measured range (never zeroed - see the module honesty notes).
    fn frf_mag_at(&self, f_hz: f64) -> f64 {
        interp_clamped(&self.frf_freqs, &self.frf_mag, f_hz)
    }
}

/// The predictive-notch limiter (SAND2016-1335C): a set of limit channels plus the
/// prediction headroom.
#[derive(Debug, Clone, PartialEq)]
pub struct PredictiveNotch {
    /// The limit channels. Any channel exceeding notches the drive; the deepest
    /// (smallest-ratio) notch per bin wins (safety-monotone, the Random arbitration rule).
    pub limit_channels: Vec<LimitChannel>,
    /// Prediction headroom, dB (>= 0): predictions must stay `headroom_db` BELOW the
    /// limit (`allowed = limit * 10^(-headroom_db/20)`). Budgets the zero-phase
    /// prediction slack (module honesty notes).
    pub headroom_db: f64,
}

/// One band one limit channel's prediction exceeded (before notching).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotchedBand {
    /// The limit channel that exceeded.
    pub channel: String,
    /// Index into the analysis band grid.
    pub band_index: usize,
    /// The band's natural frequency, Hz.
    pub band_freq_hz: f64,
    /// Predicted exceedance over the allowed bound at plan entry, dB
    /// (`20*log10(predicted/allowed)`, > 0).
    pub exceedance_db: f64,
}

/// What a [`PredictiveNotch::plan`] did and how well it converged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotchReport {
    /// Every (channel, band) whose prediction exceeded its allowed bound at plan
    /// entry. Empty means the plan was a NO-OP (gains all exactly 1.0).
    pub notched_bands: Vec<NotchedBand>,
    /// Worst predicted exceedance over all channels/bands BEFORE notching, dB
    /// (positive = over the allowed bound). `None` if no channel/band produced a
    /// comparable prediction (e.g. all limits infinite).
    pub worst_exceedance_before_db: Option<f64>,
    /// Worst predicted exceedance AFTER the returned gains, dB. `<= 0` when
    /// `converged`; positive (honest) when the refine loop ran out of iterations.
    pub worst_exceedance_after_db: Option<f64>,
    /// Relative residual of the final least-squares re-fit,
    /// `||achieved - notched|| / ||notched||` over the drive spectrum. Measures how
    /// well the component basis can EXPRESS the requested notch (the
    /// re-fit is a projection, not a copy). `0.0` for a no-op plan.
    pub refit_residual_rel: f64,
    /// Notch refine iterations performed (0 for a no-op plan).
    pub iterations: usize,
    /// Whether every prediction ended at or under its allowed bound.
    pub converged: bool,
}

/// The outcome of a [`PredictiveNotch::plan`]: the reduced per-component gains plus
/// the report.
#[derive(Debug, Clone, PartialEq)]
pub struct NotchOutcome {
    /// Multiplicative gain per synthesis component, in `[0, 1]`. `1.0` = untouched.
    /// Multiply into the component amplitudes, and hand
    /// `gains[j] * |current_amplitude[j]|` to
    /// [`crate::iterate::synthesize_to_target_capped`] as the per-component cap so
    /// the SRS-matching loop cannot re-inflate a notched component.
    pub component_gains: Vec<f64>,
    /// What was notched and how well it converged.
    pub report: NotchReport,
}

impl NotchOutcome {
    /// The per-component amplitude CAPS for
    /// [`crate::iterate::synthesize_to_target_capped`]:
    /// `|current_amplitudes[j]| * gains[j]`. This is the arbitration seam (the
    /// Random rule): the SRS-matching outer loop may push a component's amplitude
    /// anywhere BELOW its cap but never above it, so limit-down always wins.
    pub fn amplitude_caps(&self, current_amplitudes: &[f64]) -> Vec<f64> {
        current_amplitudes
            .iter()
            .zip(&self.component_gains)
            .map(|(&a, &g)| a.abs() * g)
            .collect()
    }
}

/// Errors from configuring or running the predictive notch.
#[derive(Debug, Clone, PartialEq)]
pub enum NotchError {
    /// A configuration value was invalid (bad FRF grid, bad limit curve, bad
    /// headroom).
    BadConfig(String),
    /// A runtime input was invalid (empty/mismatched component waveforms, bad
    /// sample rate, empty band grid).
    BadInput(String),
}

impl std::fmt::Display for NotchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotchError::BadConfig(s) => write!(f, "predictive-notch config error: {s}"),
            NotchError::BadInput(s) => write!(f, "predictive-notch input error: {s}"),
        }
    }
}

impl std::error::Error for NotchError {}

impl PredictiveNotch {
    /// Validate the limiter against an analysis grid of `nbands` bands.
    pub fn validate(&self, nbands: usize) -> Result<(), NotchError> {
        if !self.headroom_db.is_finite() || self.headroom_db < 0.0 {
            return Err(NotchError::BadConfig(format!(
                "headroom_db must be finite and >= 0 (got {})",
                self.headroom_db
            )));
        }
        for ch in &self.limit_channels {
            ch.validate(nbands)?;
        }
        Ok(())
    }

    /// Plan the notch for the CURRENT drive (SAND2016-1335C predictive notching; see
    /// the module docs for the full algorithm).
    ///
    /// # Inputs
    ///
    /// - `component_waveforms`: one time record PER SYNTHESIS COMPONENT, all the
    ///   same length, at the component's CURRENT amplitude and delay, such that
    ///   their sample-for-sample sum IS the current drive. For WAVSYN that is
    ///   [`crate::wavsyn::wavsyn_component`] per table row; for SEDS pass the RAW
    ///   per-component records (the Smallwood-Nord compensators are NOT components -
    ///   they are re-derived after the gains are applied, and they live below the
    ///   analysis band by construction, so the notch does not fight them).
    /// - `band_freqs`: the SRS analysis grid (grid 1 of the module docs). Every limit
    ///   channel's `limit_srs` must have this length.
    /// - `q`: SDOF quality factor for the predicted-response SRS scoring.
    ///
    /// # Returns
    ///
    /// The per-component gains in `[0, 1]` plus the [`NotchReport`]. With no
    /// exceedance the plan is a NO-OP: gains are all EXACTLY `1.0` and
    /// `report.notched_bands` is empty (gate iii).
    ///
    /// The component count, band count, and FFT bin count are all independent
    /// (gate ii); nothing here assumes any of them match.
    pub fn plan(
        &self,
        component_waveforms: &[Vec<f64>],
        fs_hz: f64,
        band_freqs: &[f64],
        q: f64,
    ) -> Result<NotchOutcome, NotchError> {
        self.validate(band_freqs.len())?;
        if component_waveforms.is_empty() {
            return Err(NotchError::BadInput("no component waveforms".into()));
        }
        let record_len = component_waveforms[0].len();
        if record_len == 0 {
            return Err(NotchError::BadInput("empty component waveform".into()));
        }
        if component_waveforms.iter().any(|w| w.len() != record_len) {
            return Err(NotchError::BadInput(
                "component waveforms must all be the same length".into(),
            ));
        }
        if fs_hz <= 0.0 || !fs_hz.is_finite() {
            return Err(NotchError::BadInput(format!(
                "sample rate must be finite and > 0 (got {fs_hz})"
            )));
        }
        if band_freqs.is_empty() {
            return Err(NotchError::BadInput("empty analysis band grid".into()));
        }
        if !(q.is_finite()) || q <= 0.5 {
            return Err(NotchError::BadInput(format!(
                "q must be finite and > 0.5 (got {q})"
            )));
        }

        let m = component_waveforms.len();
        let nbands = band_freqs.len();
        let nfft = fft::next_pow2(record_len);
        let nh = nfft / 2 + 1;
        let bin_hz = fs_hz / nfft as f64;

        // Component spectra C_j on the FFT half-grid (grid 3 of the module docs). The
        // drive spectrum at gains g is sum_j g_j * C_j - exact, because synthesis is
        // a linear superposition and the records share one length.
        let comp_spectra: Vec<Vec<(f64, f64)>> = component_waveforms
            .iter()
            .map(|w| {
                let mut buf = fft::zero_pad_pow2(w);
                buf.resize(nfft, (0.0, 0.0)); // zero_pad_pow2 already pads to nfft
                fft::fft_in_place(&mut buf);
                buf.truncate(nh);
                buf
            })
            .collect();

        // Each channel's FRF magnitude interpolated onto the FFT bin grid.
        let frf_on_bins: Vec<Vec<f64>> = self
            .limit_channels
            .iter()
            .map(|ch| (0..nh).map(|k| ch.frf_mag_at(k as f64 * bin_hz)).collect())
            .collect();

        // Allowed bound per channel/band: limit minus headroom.
        let headroom_lin = 10.0_f64.powf(-self.headroom_db / 20.0);
        let over_notch_lin = 10.0_f64.powf(-OVER_NOTCH_DB / 20.0);
        let allowed: Vec<Vec<f64>> = self
            .limit_channels
            .iter()
            .map(|ch| ch.limit_srs.iter().map(|&l| l * headroom_lin).collect())
            .collect();

        // Geometric band edges: bins inside a band's edges belong to that band for
        // notching purposes.
        let edges = band_edges(band_freqs);

        let mut gains = vec![1.0f64; m];
        let mut notched_bands: Vec<NotchedBand> = Vec::new();
        let mut worst_before: Option<f64> = None;
        let mut worst_after: Option<f64> = None;
        let mut refit_residual_rel = 0.0f64;
        let mut iterations = 0usize;
        let mut converged = false;

        for iter in 0..=MAX_REFINE_ITERS {
            // Current drive spectrum D_g[k] = sum_j g_j C_j[k].
            let drive_spec = superpose(&comp_spectra, &gains, nh);

            // Predict each channel's response SRS and collect exceedances.
            let mut worst_this_pass: Option<f64> = None;
            // Per-bin notch multiplier for this pass (1.0 = leave the bin alone).
            let mut notch_mult = vec![1.0f64; nh];
            for (c, ch) in self.limit_channels.iter().enumerate() {
                let pred_srs =
                    response_srs(&drive_spec, &frf_on_bins[c], nfft, fs_hz, band_freqs, q);
                for b in 0..nbands {
                    let bound = allowed[c][b];
                    if !bound.is_finite() {
                        continue; // no limit at this band
                    }
                    let pred = pred_srs[b];
                    if pred <= 0.0 {
                        continue; // no predicted response -> nothing to compare
                    }
                    let exceed_db = 20.0 * (pred / bound).log10();
                    worst_this_pass = Some(worst_this_pass.map_or(exceed_db, |w: f64| {
                        if exceed_db > w {
                            exceed_db
                        } else {
                            w
                        }
                    }));
                    if exceed_db > 0.0 {
                        if iter == 0 {
                            notched_bands.push(NotchedBand {
                                channel: ch.name.clone(),
                                band_index: b,
                                band_freq_hz: band_freqs[b],
                                exceedance_db: exceed_db,
                            });
                        }
                        // Notch every FFT bin inside this band's geometric edges
                        // (widened to at least the SDOF half-power bandwidth
                        // f_b/(2Q), which is what actually drives the band's SRS)
                        // by the allowed/predicted ratio plus the fixed over-notch;
                        // the deepest notch per bin wins across channels/bands
                        // (safety-monotone).
                        let ratio = (bound / pred) * over_notch_lin; // < 1
                        let fb = band_freqs[b];
                        let half_power = fb / (2.0 * q);
                        let lo = edges[b].min(fb - half_power).max(0.0);
                        let hi = edges[b + 1].max(fb + half_power);
                        let k_lo = (lo / bin_hz).floor().max(0.0) as usize;
                        let k_hi = ((hi / bin_hz).ceil() as usize).min(nh - 1);
                        for mult in notch_mult.iter_mut().take(k_hi + 1).skip(k_lo) {
                            if ratio < *mult {
                                *mult = ratio;
                            }
                        }
                    }
                }
            }

            if iter == 0 {
                worst_before = worst_this_pass;
            }
            worst_after = worst_this_pass;

            match worst_this_pass {
                // Every prediction at or under its allowed bound: done.
                Some(w) if w <= 0.0 => {
                    converged = true;
                    break;
                }
                None => {
                    // Nothing comparable (all limits infinite / no response):
                    // vacuously converged, and necessarily a no-op.
                    converged = true;
                    break;
                }
                _ => {}
            }
            if iter == MAX_REFINE_ITERS {
                break; // out of refine budget; report honestly (converged = false)
            }

            // Notched target spectrum T[k] = notch_mult[k] * D_g[k], then the
            // least-squares re-fit back to component gains.
            let target: Vec<(f64, f64)> = drive_spec
                .iter()
                .zip(&notch_mult)
                .map(|(&(re, im), &nm)| (re * nm, im * nm))
                .collect();
            let fitted = least_squares_gains(&comp_spectra, &target, nh);

            // Safety-monotone clamp: within one plan a gain may only go DOWN (and
            // never below zero). This is the limit-down-always-wins arbitration.
            let mut moved = false;
            for j in 0..m {
                let clamped = fitted[j].clamp(0.0, gains[j]);
                if (clamped - gains[j]).abs() > 1e-15 {
                    moved = true;
                }
                gains[j] = clamped;
            }
            iterations += 1;

            // Re-fit residual against THIS pass's notched target (how well the
            // component basis expresses the notch).
            let achieved = superpose(&comp_spectra, &gains, nh);
            refit_residual_rel = relative_residual(&achieved, &target);

            if !moved {
                break; // fit stalled; report honestly (converged stays false)
            }
        }

        // A plan that never notched returns gains of exactly 1.0 (gate iii no-op).
        Ok(NotchOutcome {
            component_gains: gains,
            report: NotchReport {
                notched_bands,
                worst_exceedance_before_db: worst_before,
                worst_exceedance_after_db: worst_after,
                refit_residual_rel,
                iterations,
                converged,
            },
        })
    }
}

/// Predict a limit channel's response time history from a drive record and the
/// channel's FRF magnitude: `response = IFFT( FFT(drive) * |H| )` (zero-phase
/// magnitude filter - see the module honesty notes). Public so services and tests
/// can score a prediction the same way [`PredictiveNotch::plan`] does.
///
/// The returned record has `next_pow2(drive.len())` samples (the acausal zero-phase
/// filter rings into the padding; keeping it makes the SRS scoring conservative).
pub fn predict_response_waveform(
    drive_g: &[f64],
    fs_hz: f64,
    frf_freqs: &[f64],
    frf_mag: &[f64],
) -> Vec<f64> {
    if drive_g.is_empty() || fs_hz <= 0.0 || frf_freqs.is_empty() {
        return Vec::new();
    }
    let nfft = fft::next_pow2(drive_g.len());
    let nh = nfft / 2 + 1;
    let bin_hz = fs_hz / nfft as f64;
    let mut spec = fft::rfft(drive_g);
    spec.truncate(nh);
    for (k, s) in spec.iter_mut().enumerate() {
        let h = interp_clamped(frf_freqs, frf_mag, k as f64 * bin_hz);
        s.0 *= h;
        s.1 *= h;
    }
    irfft_real(&spec, nfft)
}

/// Predict a limit channel's response SRS on `band_freqs`:
/// [`predict_response_waveform`] followed by [`eigen_dsp::srs::srs_analyze`].
pub fn predict_response_srs(
    drive_g: &[f64],
    fs_hz: f64,
    frf_freqs: &[f64],
    frf_mag: &[f64],
    band_freqs: &[f64],
    q: f64,
) -> Vec<f64> {
    let resp = predict_response_waveform(drive_g, fs_hz, frf_freqs, frf_mag);
    srs_analyze(&resp, fs_hz, band_freqs, q)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Superpose component half-spectra at the given gains: `sum_j g_j * C_j[k]`.
fn superpose(comp_spectra: &[Vec<(f64, f64)>], gains: &[f64], nh: usize) -> Vec<(f64, f64)> {
    let mut out = vec![(0.0f64, 0.0f64); nh];
    for (spec, &g) in comp_spectra.iter().zip(gains) {
        for (o, &(re, im)) in out.iter_mut().zip(spec.iter()) {
            o.0 += g * re;
            o.1 += g * im;
        }
    }
    out
}

/// Predicted response SRS for a drive half-spectrum through a per-bin FRF magnitude:
/// multiply, inverse-transform, score with [`srs_analyze`].
fn response_srs(
    drive_spec: &[(f64, f64)],
    frf_on_bins: &[f64],
    nfft: usize,
    fs_hz: f64,
    band_freqs: &[f64],
    q: f64,
) -> Vec<f64> {
    let resp_spec: Vec<(f64, f64)> = drive_spec
        .iter()
        .zip(frf_on_bins)
        .map(|(&(re, im), &h)| (re * h, im * h))
        .collect();
    let resp = irfft_real(&resp_spec, nfft);
    srs_analyze(&resp, fs_hz, band_freqs, q)
}

/// Inverse FFT of a half-spectrum (bins `0 ..= nfft/2`) of a REAL signal: rebuild the
/// Hermitian-symmetric full spectrum, run [`fft::ifft`], take the real parts.
fn irfft_real(half: &[(f64, f64)], nfft: usize) -> Vec<f64> {
    let mut full = vec![(0.0f64, 0.0f64); nfft];
    let nh = half.len().min(nfft / 2 + 1);
    full[..nh].copy_from_slice(&half[..nh]);
    for k in 1..nfft / 2 {
        if k < nh {
            full[nfft - k] = (half[k].0, -half[k].1);
        }
    }
    fft::ifft(&full).iter().map(|&(re, _)| re).collect()
}

/// Geometric band edges for an ascending band-center grid: `edges[b] .. edges[b+1]`
/// covers band `b`. Inner edges are geometric means of adjacent centers; the outer
/// edges extrapolate the adjacent geometric step (half an octave each way for a
/// single-band grid, where no step exists).
fn band_edges(band_freqs: &[f64]) -> Vec<f64> {
    let n = band_freqs.len();
    let mut edges = Vec::with_capacity(n + 1);
    if n == 1 {
        let f = band_freqs[0];
        let half = 2.0_f64.powf(0.25);
        edges.push(f / half);
        edges.push(f * half);
        return edges;
    }
    // Lower outer edge: extrapolate the first geometric step downward.
    let first_step = (band_freqs[1] / band_freqs[0]).sqrt();
    edges.push(band_freqs[0] / first_step);
    for b in 0..n - 1 {
        edges.push((band_freqs[b] * band_freqs[b + 1]).sqrt());
    }
    let last_step = (band_freqs[n - 1] / band_freqs[n - 2]).sqrt();
    edges.push(band_freqs[n - 1] * last_step);
    edges
}

/// Linear interpolation of `(xs, ys)` at `x`, CLAMPED to the endpoint values outside
/// the sampled range. `xs` must be ascending (validated at config time).
fn interp_clamped(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    // Binary search for the bracketing segment.
    let mut lo = 0usize;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if xs[mid] <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (x0, x1) = (xs[lo], xs[hi]);
    let (y0, y1) = (ys[lo], ys[hi]);
    if x1 <= x0 {
        return y0;
    }
    y0 + (y1 - y0) * (x - x0) / (x1 - x0)
}

/// The least-squares re-fit: solve for the gain vector `g` minimizing
/// `sum_k | sum_j g_j C_j[k] - T[k] |^2` over the half-spectrum bins.
///
/// Normal equations: `A g = b` with `A[i][j] = sum_k Re(C_i[k] * conj(C_j[k]))` and
/// `b[i] = sum_k Re(C_i[k] * conj(T[k]))` (the real inner product of the complex
/// spectra - the imaginary parts cancel in the symmetric sum). A tiny ridge
/// (`1e-12 * max diag`) keeps the solve well-posed when components overlap heavily.
/// Solved by Gaussian elimination with partial pivoting (the component count is
/// small). No external dependencies.
fn least_squares_gains(
    comp_spectra: &[Vec<(f64, f64)>],
    target: &[(f64, f64)],
    nh: usize,
) -> Vec<f64> {
    let m = comp_spectra.len();
    let mut a = vec![vec![0.0f64; m]; m];
    let mut b = vec![0.0f64; m];
    for i in 0..m {
        let ci = &comp_spectra[i];
        for (j, row_val) in a[i].iter_mut().enumerate().skip(i) {
            let cj = &comp_spectra[j];
            let mut acc = 0.0f64;
            for k in 0..nh {
                // Re(Ci * conj(Cj)) = re_i*re_j + im_i*im_j
                acc += ci[k].0 * cj[k].0 + ci[k].1 * cj[k].1;
            }
            *row_val = acc;
        }
        let mut acc = 0.0f64;
        for k in 0..nh {
            acc += ci[k].0 * target[k].0 + ci[k].1 * target[k].1;
        }
        b[i] = acc;
    }
    // Mirror the symmetric Gram matrix (a[i][j] = a[j][i] for j < i).
    for i in 0..m {
        let (head, tail) = a.split_at_mut(i);
        let row_i = &mut tail[0];
        for (j, head_row) in head.iter().enumerate() {
            row_i[j] = head_row[i];
        }
    }
    // Ridge for numerical safety on heavily-overlapping components.
    let max_diag = (0..m).map(|i| a[i][i]).fold(0.0f64, f64::max);
    let ridge = 1e-12 * max_diag;
    for (i, row) in a.iter_mut().enumerate() {
        row[i] += ridge;
    }
    solve_gauss(&mut a, &mut b)
}

/// Gaussian elimination with partial pivoting. Returns the solution (zeros for a
/// singular system - the caller's clamp keeps that safe).
fn solve_gauss(a: &mut [Vec<f64>], b: &mut [f64]) -> Vec<f64> {
    let m = b.len();
    for col in 0..m {
        // Partial pivot.
        let mut piv = col;
        let mut piv_abs = a[col][col].abs();
        for (r, row) in a.iter().enumerate().skip(col + 1) {
            let v = row[col].abs();
            if v > piv_abs {
                piv = r;
                piv_abs = v;
            }
        }
        if piv_abs <= 0.0 {
            return vec![0.0; m]; // singular; caller clamps safely
        }
        if piv != col {
            a.swap(piv, col);
            b.swap(piv, col);
        }
        // Eliminate below the pivot. split_at_mut separates the pivot row from the
        // rows being updated so the borrows are disjoint.
        let (top, bottom) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        let pivot = pivot_row[col];
        for (r_off, row) in bottom.iter_mut().enumerate() {
            let f = row[col] / pivot;
            if f == 0.0 {
                continue;
            }
            for (c, val) in row.iter_mut().enumerate().skip(col) {
                *val -= f * pivot_row[c];
            }
            b[col + 1 + r_off] -= f * b[col];
        }
    }
    // Back-substitution.
    let mut x = vec![0.0f64; m];
    for row in (0..m).rev() {
        let mut acc = b[row];
        for col in row + 1..m {
            acc -= a[row][col] * x[col];
        }
        x[row] = acc / a[row][row];
    }
    x
}

/// `||achieved - target|| / ||target||` over complex half-spectra (`0` for a zero
/// target).
fn relative_residual(achieved: &[(f64, f64)], target: &[(f64, f64)]) -> f64 {
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (&(ar, ai), &(tr, ti)) in achieved.iter().zip(target) {
        let dr = ar - tr;
        let di = ai - ti;
        num += dr * dr + di * di;
        den += tr * tr + ti * ti;
    }
    if den <= 0.0 {
        return 0.0;
    }
    (num / den).sqrt()
}
