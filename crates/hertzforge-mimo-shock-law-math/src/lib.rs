//! The mathematics of the multiple-drive shock control laws: several drives,
//! several control points, a pulse at a time, with nothing around it.
//!
//! A pulse test plays a whole drive record on every drive, measures every
//! control point, and between pulses a law builds the next pulse's drives. The
//! laws here work on plain records and on the plant model as a stack of
//! complex `n_c x n_d` matrices, one per spectral line of the pulse.
//!
//! - [`inverse`]: the regularized inverse of the model per line, a seed and a
//!   between-pulse waveform correction with a gain per line and a watchdog
//!   that cuts the gain on a line whose error grew. Options: refresh the model
//!   after every pulse ([`refresh`]), and remove each control's best-fit delay
//!   before the error is formed.
//! - [`shape`]: shock response spectrum (SRS) control with no waveform error
//!   formed anywhere. Each control's target is shaped by a real, smooth gain
//!   from its SRS ratio, and the drives are the model's inverse applied to the
//!   shaped targets. Options: the measured anchor (the measured response with
//!   its own phase, magnitude corrected), the refresh, and the least-effort
//!   seed timing.
//! - [`paired`]: each control paired with one drive, that drive's spectrum
//!   scaled by the ratio of wanted to measured magnitude. No cross-path model.
//!   The shortest law, written as the template a contributor copies.
//! - [`refresh`]: the model moved the least amount that makes it reproduce the
//!   last pulse.
//! - [`delay`]: delay estimation by cross-correlation, a delay applied as a
//!   linear phase, and the least-effort choice of per-control delays.
//! - [`check`]: the convergence checks a law check evaluates before a pulse:
//!   the safe gain and drive-side spectral radius per line, and the fraction
//!   of each control's target the drives can reach at all.
//!
//! The law modules use no clock, thread, file or random number, so a law file
//! built for WebAssembly and the controller's native laws run the same code.
//! The `reference-plant` feature adds the exact reference plant used by the
//! tests and the study example; no law uses it.
//!
//! # Conventions
//!
//! - Records are rows of `record_len` samples (`record_len` a power of two):
//!   drives in volts as played, responses in engineering units as measured.
//! - Spectra are the one-sided transform under the negative exponent,
//!   `X[k] = sum_n x[n] exp(-j 2 pi k n / N)`, `record_len / 2 + 1` lines at
//!   `fs / record_len`. A delay `tau` is the factor `exp(-j 2 pi f tau)`.
//! - The plant `H` maps drive spectra to control spectra: `Y = H D` per line.
//! - A pulse at `level_db` has the amplitude scale `s = 10^(level_db / 20)`.
//!   Laws hold their drives and errors normalized to full level (divided by
//!   `s`), so a level step is a rescaling and nothing else.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod check;
pub mod delay;
pub mod inverse;
pub mod lsq;
pub mod modal;
pub mod model;
pub mod paired;
pub mod provider;
pub mod refresh;
pub mod robust;
pub mod shape;
pub mod spectrum;
pub mod srs;

#[cfg(feature = "reference-plant")]
pub mod reference_plant;
#[cfg(feature = "reference-plant")]
pub mod research_plant;

pub use hertzforge_sdm::{MatrixStack, C64};

/// The amplitude scale of a level in dB: `10^(level_db / 20)`.
pub fn level_scale(level_db: f64) -> f64 {
    10f64.powf(level_db / 20.0)
}

/// The plant model a law is given once: the identified frequency response on
/// the pulse's own lines and which lines may be trusted.
#[derive(Clone, Debug, PartialEq)]
pub struct PlantModel {
    /// `record_len / 2 + 1` lines, each `n_c x n_d` (controls by drives).
    pub frf: MatrixStack,
    /// One flag per line. An untrusted line gets no drive.
    pub trusted: Vec<bool>,
}

/// An SRS target for every control point.
#[derive(Clone, Debug, PartialEq)]
pub struct SrsTarget {
    /// Natural frequencies of the SRS, ascending, Hz.
    pub natural_hz: Vec<f64>,
    /// Quality factor of the SRS oscillators.
    pub q: f64,
    /// Target SRS of every control at full level, `n_c` rows of
    /// `natural_hz.len()` values, engineering units.
    pub points: Vec<Vec<f64>>,
}

/// What a law is given once, before the first pulse.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    /// Sample rate, Hz.
    pub fs_hz: f64,
    /// Samples in every record, a power of two.
    pub record_len: usize,
    /// Number of drives.
    pub n_drives: usize,
    /// Number of control points.
    pub n_controls: usize,
    /// Lowest controlled line, Hz.
    pub band_lo_hz: f64,
    /// Highest controlled line, Hz.
    pub band_hi_hz: f64,
    /// Target waveform of every control at full level, `n_c` rows of
    /// `record_len` samples. For an SRS test, the waveform synthesized for
    /// each control's SRS.
    pub reference: Vec<Vec<f64>>,
    /// The SRS of every control, present for an SRS test.
    pub srs: Option<SrsTarget>,
    /// The identified plant.
    pub plant: PlantModel,
}

/// What a law is given after each pulse.
#[derive(Clone, Debug, PartialEq)]
pub struct PulseInputs {
    /// Index of the pulse just played, from 0.
    pub pulse_index: usize,
    /// Level the pulse was played at, dB.
    pub level_db: f64,
    /// Level the next pulse will be played at, dB.
    pub next_level_db: f64,
    /// Drive records as played, `n_d` rows.
    pub drives: Vec<Vec<f64>>,
    /// Response records as measured, `n_c` rows.
    pub responses: Vec<Vec<f64>>,
    /// Measured SRS per control at `natural_hz` (not normalized), when the
    /// core computed it. A law computes it itself when absent.
    pub srs: Option<Vec<Vec<f64>>>,
}

/// What a law returns for the next pulse.
#[derive(Clone, Debug, PartialEq)]
pub struct PulseOutputs {
    /// Drive records for the next pulse at its level, `n_d` rows.
    pub drives: Vec<Vec<f64>>,
    /// The delay per control the law is building for, seconds, when it has
    /// one.
    pub delays_s: Option<Vec<f64>>,
    /// Named diagnostics, a small fixed set per law.
    pub values: Vec<(&'static str, f64)>,
}

/// The two steps every law in this crate has: a seed before the first pulse
/// and an iteration after each.
pub trait LawSteps {
    /// The drives of the first pulse, at `level_db`.
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, String>;
    /// The drives of the next pulse from the measurements of the last.
    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String>;
}

/// Checks the shapes in a setup and returns the number of lines.
pub(crate) fn check_setup(setup: &Setup) -> Result<usize, String> {
    let n = setup.record_len;
    if n < 4 || !n.is_power_of_two() {
        return Err(format!(
            "record length {n} is not a power of two of at least 4"
        ));
    }
    if !(setup.fs_hz > 0.0 && setup.fs_hz.is_finite()) {
        return Err("the sample rate must be positive".into());
    }
    if setup.n_drives == 0 || setup.n_controls == 0 {
        return Err("a test needs at least one drive and one control".into());
    }
    let lines = n / 2 + 1;
    let (l, r, c) = setup.plant.frf.dims();
    if (l, r, c) != (lines, setup.n_controls, setup.n_drives) {
        return Err(format!(
            "the plant is {l} lines of {r} x {c}; this test needs {lines} lines of {} x {}",
            setup.n_controls, setup.n_drives
        ));
    }
    if setup.plant.trusted.len() != lines {
        return Err("the trusted flags must have one entry per line".into());
    }
    if setup.reference.len() != setup.n_controls || setup.reference.iter().any(|r| r.len() != n) {
        return Err("the reference must have one record of record_len samples per control".into());
    }
    if let Some(t) = &setup.srs {
        if t.points.len() != setup.n_controls
            || t.points.iter().any(|p| p.len() != t.natural_hz.len())
            || t.natural_hz.is_empty()
            || t.q.is_nan()
            || t.q <= 0.0
        {
            return Err("the SRS target must have one row per control on its frequencies".into());
        }
    }
    Ok(lines)
}

/// Checks the shapes of a pulse's inputs against a setup.
pub(crate) fn check_inputs(setup: &Setup, inputs: &PulseInputs) -> Result<(), String> {
    let n = setup.record_len;
    if inputs.drives.len() != setup.n_drives || inputs.drives.iter().any(|d| d.len() != n) {
        return Err("the drives must be one record of record_len samples per drive".into());
    }
    if inputs.responses.len() != setup.n_controls || inputs.responses.iter().any(|d| d.len() != n) {
        return Err("the responses must be one record of record_len samples per control".into());
    }
    if !inputs.level_db.is_finite() || !inputs.next_level_db.is_finite() {
        return Err("levels must be finite".into());
    }
    Ok(())
}

/// The lines a law may drive: inside the band and trusted.
pub fn drive_mask(setup: &Setup) -> Vec<bool> {
    let lines = setup.record_len / 2 + 1;
    let df = setup.fs_hz / setup.record_len as f64;
    (0..lines)
        .map(|k| {
            let f = k as f64 * df;
            f >= setup.band_lo_hz && f <= setup.band_hi_hz && setup.plant.trusted[k]
        })
        .collect()
}
