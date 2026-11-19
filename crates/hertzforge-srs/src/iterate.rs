//! The SRS-matching outer loop.
//!
//! Given a target SRS on an analysis band grid, iterate a synthesis component table's
//! per-component amplitudes until the SYNTHESIZED waveform's SRS matches the target to
//! tolerance. This is the OFFLINE iteration (the drive is computed on the host, scored,
//! and re-computed); the run-time counterpart that does the same update pulse-by-pulse
//! from a MEASURED SRS is [`crate::law::SrsSynthesisLaw`].
//!
//! # The loop
//!
//! 1. Synthesize the composite waveform from the current table (WAVSYN or SEDS).
//! 2. Score it: `achieved = srs_analyze(waveform, fs, band_freqs, q)`.
//! 3. For each component, look up its NEAREST analysis band and form the linear ratio
//!    `target / achieved` there; multiply the component's amplitude by that ratio,
//!    clamped to `+/- max_gain_db_per_iter` dB so no single step blows up.
//! 4. Repeat until the worst per-band error is within `tol_db`, or `max_iters`.
//!
//! # Component count != band count (gate iv)
//!
//! The three grids - target/analysis bands, synthesis components, and (later) FFT drive
//! bins - are DISTINCT. The loop makes NO 1:1 assumption between components
//! and bands: each component is mapped to its nearest analysis band by frequency, so a
//! table with components at 1/3-octave spacing can be matched against an SRS analyzed at
//! 1/6-octave spacing (twice as many bands as components) and still converge. This is
//! asserted directly by gate iv.
//!
//! # Determinism
//!
//! No RNG. The result is a pure function of the inputs, so a converged waveform is
//! reproducible.

use crate::seds::seds_compensated;
use crate::table::{DampedSineTable, WaveletTable};
use crate::wavsyn::wavsyn_composite;
use hertzforge_dsp::srs::srs_analyze;
use serde::{Deserialize, Serialize};

/// Which synthesis engine the iteration drives. The two share the outer loop; they
/// differ only in how a table maps to a composite waveform (and SEDS re-derives its
/// compensation each iteration, since amplitude changes shift the residuals).
///
/// The accessors ([`Self::component_freqs`], [`Self::amplitudes`],
/// [`Self::scale_amplitude`], [`Self::synthesize`]) are public because the run
/// service ([`crate::service`]) holds the DRIVE table as a `SynthesisEngine` and
/// re-synthesizes it pulse-by-pulse from the run-time law's gain updates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SynthesisEngine {
    /// WAVSYN odd-half-cycle wavelets ([`crate::wavsyn`]). Self-nulling; no compensation.
    Wavsyn(WaveletTable),
    /// SEDS decaying sinusoids ([`crate::seds`]) with Smallwood-Nord compensation
    /// recomputed each iteration.
    Seds(DampedSineTable),
}

impl SynthesisEngine {
    /// The current component frequencies (table order).
    pub fn component_freqs(&self) -> Vec<f64> {
        match self {
            SynthesisEngine::Wavsyn(t) => t.component_freqs(),
            SynthesisEngine::Seds(t) => t.component_freqs(),
        }
    }

    /// The current per-component amplitudes (table order).
    pub fn amplitudes(&self) -> Vec<f64> {
        match self {
            SynthesisEngine::Wavsyn(t) => t.components.iter().map(|c| c.amplitude_g).collect(),
            SynthesisEngine::Seds(t) => t.components.iter().map(|c| c.amplitude_g).collect(),
        }
    }

    /// Multiply component `j`'s amplitude by `factor`.
    pub fn scale_amplitude(&mut self, j: usize, factor: f64) {
        match self {
            SynthesisEngine::Wavsyn(t) => t.components[j].amplitude_g *= factor,
            SynthesisEngine::Seds(t) => t.components[j].amplitude_g *= factor,
        }
    }

    /// Clamp component `j`'s amplitude MAGNITUDE to `cap_abs`, preserving its sign
    /// (the limit arbitration seam: a predictive-notch cap is an absolute
    /// ceiling on how hard a component may be driven).
    pub fn clamp_amplitude_abs(&mut self, j: usize, cap_abs: f64) {
        let amp = match self {
            SynthesisEngine::Wavsyn(t) => &mut t.components[j].amplitude_g,
            SynthesisEngine::Seds(t) => &mut t.components[j].amplitude_g,
        };
        if amp.abs() > cap_abs {
            *amp = amp.signum() * cap_abs;
        }
    }

    /// Synthesize the composite waveform at `fs_hz`. For SEDS this recomputes the
    /// compensation for the current amplitudes; the WAVSYN tail is sized from the
    /// lowest component period.
    pub fn synthesize(&self, fs_hz: f64) -> Vec<f64> {
        match self {
            SynthesisEngine::Wavsyn(t) => {
                // A tail of a few of the lowest-frequency period lets residual motion
                // (there should be none) develop; also gives the SRS residual response
                // room to ring out.
                let f_lo = t
                    .components
                    .iter()
                    .map(|c| c.freq_hz)
                    .fold(f64::INFINITY, f64::min);
                let tail = if f_lo.is_finite() && f_lo > 0.0 {
                    4.0 / f_lo
                } else {
                    0.0
                };
                wavsyn_composite(t, fs_hz, tail)
            }
            SynthesisEngine::Seds(t) => seds_compensated(t, fs_hz).waveform_g,
        }
    }
}

/// Knobs for [`synthesize_to_target`].
///
/// `#[serde(default)]` (2026-08-09): a missing knob takes its documented default
/// rather than failing the whole config parse. The front end sends three of these
/// four fields (`ui/src/lib/shock/types.ts`'s `SynthesisOpts` has no `q`), and
/// without this a local SRS configure died at `missing field 'q'` before it ever
/// reached the SRS-specific capture mismatch behind it. Pinned by
/// `ui/src-tauri`'s `the_front_ends_srs_config_json_deserializes`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SynthesisOpts {
    /// Maximum outer-loop iterations before returning the best-so-far (not converged).
    pub max_iters: usize,
    /// Convergence tolerance on the worst per-band |error|, dB. The loop stops when the
    /// worst band is within this.
    pub tol_db: f64,
    /// Per-iteration per-component amplitude clamp, dB. Bounds any single step so a
    /// badly-mismatched band cannot blow the amplitude up in one iteration (the classic
    /// SRS-iteration safety limit).
    pub max_gain_db_per_iter: f64,
    /// SDOF quality factor `Q` for the SRS analysis (`zeta = 1/(2 Q)`).
    pub q: f64,
}

impl Default for SynthesisOpts {
    fn default() -> Self {
        SynthesisOpts {
            max_iters: 50,
            tol_db: 1.5,
            max_gain_db_per_iter: 3.0,
            q: 10.0,
        }
    }
}

/// The outcome of an SRS-matching run.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthesisResult {
    /// The synthesized composite waveform at convergence (or best-so-far), g.
    pub waveform: Vec<f64>,
    /// The final component table (as a [`SynthesisEngine`], so callers can read the
    /// per-component amplitudes and, for SEDS, re-derive the compensation).
    pub components: SynthesisEngine,
    /// The achieved SRS on the analysis band grid at the returned waveform, g.
    pub achieved_srs: Vec<f64>,
    /// Per-band error in dB (`20*log10(achieved/target)`), one per analysis band.
    pub per_band_error_db: Vec<f64>,
    /// Number of outer-loop iterations performed.
    pub iters: usize,
    /// Whether the worst per-band |error| reached `tol_db`.
    pub converged: bool,
}

/// Index of the band in `band_freqs` nearest `f_hz` (linear scan; grids are small).
fn nearest_band(band_freqs: &[f64], f_hz: f64) -> usize {
    let mut best = 0usize;
    let mut best_d = f64::INFINITY;
    for (i, &fb) in band_freqs.iter().enumerate() {
        let d = (fb - f_hz).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// Per-band error in dB and the worst |error| over the grid, for a given achieved SRS
/// against the target. Bands where either value is nonpositive contribute a `0.0`
/// error entry and are skipped for the worst-error search (no trusted ratio there).
fn band_errors(target: &[f64], achieved: &[f64]) -> (Vec<f64>, f64) {
    let n = target.len().min(achieved.len());
    let mut errs = vec![0.0f64; target.len()];
    let mut worst = 0.0f64;
    for b in 0..n {
        let t = target[b];
        let a = achieved[b];
        if t > 0.0 && a > 0.0 {
            let db = 20.0 * (a / t).log10();
            errs[b] = db;
            if db.abs() > worst {
                worst = db.abs();
            }
        }
    }
    (errs, worst)
}

/// Iterate a synthesis component table to match `target_srs` on `band_freqs` (feature
/// 1). See the module docs for the loop and the component-count != band-count handling
/// (gate iv).
///
/// The `engine` carries the initial table (its per-component amplitudes are the seed);
/// on return, `result.components` holds the adjusted table. `target_srs` and
/// `band_freqs` must have the same length (the analysis grid); a length mismatch or an
/// empty grid returns a non-converged result with the initial waveform.
///
/// This is the UNCAPPED loop; the limit arbitration uses
/// [`synthesize_to_target_capped`] (this function delegates there with no caps).
pub fn synthesize_to_target(
    target_srs: &[f64],
    band_freqs: &[f64],
    fs_hz: f64,
    engine: SynthesisEngine,
    opts: SynthesisOpts,
) -> SynthesisResult {
    synthesize_to_target_capped(target_srs, band_freqs, fs_hz, engine, opts, None)
}

/// [`synthesize_to_target`] with OPTIONAL per-component amplitude caps - the
/// predictive-notch arbitration seam (the same rule as Random control: limit-down ALWAYS
/// beats control-up).
///
/// `amplitude_caps`, when given, is one ABSOLUTE amplitude ceiling (g, magnitude)
/// per component, in table order - typically
/// [`crate::limit::NotchOutcome::amplitude_caps`]. The iteration may move a
/// component's amplitude anywhere BELOW its cap (down and back up), but the
/// per-band target/achieved ratio update can never re-inflate a component past it:
/// after every scale step the amplitude magnitude is clamped to the cap (sign
/// preserved), and the seed amplitudes are clamped before the first synthesis. A
/// notched band therefore converges to the CAPPED level (below target) while
/// unlimited bands still converge to the target - which also means a run with a
/// binding cap typically reports `converged = false` on the notched bands' error.
/// That is the honest outcome: the limit, not the loop, is what is holding the band
/// down.
///
/// A caps slice shorter than the component count caps only the components it
/// covers; entries must be `>= 0` (a NaN cap is ignored for that component).
pub fn synthesize_to_target_capped(
    target_srs: &[f64],
    band_freqs: &[f64],
    fs_hz: f64,
    mut engine: SynthesisEngine,
    opts: SynthesisOpts,
    amplitude_caps: Option<&[f64]>,
) -> SynthesisResult {
    let comp_freqs = engine.component_freqs();
    let max_lin = 10.0_f64.powf(opts.max_gain_db_per_iter / 20.0);
    let min_lin = 1.0 / max_lin;

    // Enforce the caps on the seed amplitudes (limit-down beats control-up from the
    // very first synthesis).
    let apply_caps = |engine: &mut SynthesisEngine| {
        if let Some(caps) = amplitude_caps {
            for (j, &cap) in caps.iter().enumerate().take(comp_freqs.len()) {
                if cap.is_nan() {
                    continue;
                }
                engine.clamp_amplitude_abs(j, cap.max(0.0));
            }
        }
    };
    apply_caps(&mut engine);

    // Degenerate inputs: return the initial synthesis, not converged.
    if target_srs.is_empty() || band_freqs.len() != target_srs.len() || fs_hz <= 0.0 {
        let waveform = engine.synthesize(fs_hz);
        let achieved = srs_analyze(&waveform, fs_hz, band_freqs, opts.q);
        let (errs, _worst) = band_errors(target_srs, &achieved);
        return SynthesisResult {
            waveform,
            components: engine,
            achieved_srs: achieved,
            per_band_error_db: errs,
            iters: 0,
            converged: false,
        };
    }

    let mut waveform = engine.synthesize(fs_hz);
    let mut achieved = srs_analyze(&waveform, fs_hz, band_freqs, opts.q);
    let (mut errs, mut worst) = band_errors(target_srs, &achieved);
    let mut iters = 0usize;
    let mut converged = worst <= opts.tol_db;

    while !converged && iters < opts.max_iters {
        // Adjust each component by its nearest-band target/achieved ratio, clamped.
        for (j, &fc) in comp_freqs.iter().enumerate() {
            let b = nearest_band(band_freqs, fc);
            let t = target_srs[b];
            let a = achieved.get(b).copied().unwrap_or(0.0);
            // Guard a dead/zero achieved band: no trusted ratio, so hold (unity).
            let ratio = if a > 0.0 && t > 0.0 { t / a } else { 1.0 };
            let clamped = ratio.clamp(min_lin, max_lin);
            engine.scale_amplitude(j, clamped);
        }
        // Limit arbitration: the ratio update may NOT re-inflate a capped component
        // past its predictive-notch ceiling (as in Random control: limit-down wins).
        apply_caps(&mut engine);
        iters += 1;

        waveform = engine.synthesize(fs_hz);
        achieved = srs_analyze(&waveform, fs_hz, band_freqs, opts.q);
        let (e, w) = band_errors(target_srs, &achieved);
        errs = e;
        worst = w;
        converged = worst <= opts.tol_db;
    }

    SynthesisResult {
        waveform,
        components: engine,
        achieved_srs: achieved,
        per_band_error_db: errs,
        iters,
        converged,
    }
}
