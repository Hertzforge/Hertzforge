//! The closed loop under a changeable control law (`TwrConfig::control` set):
//! the identified plant in the law's convention, and the law, its drive in
//! force and its bookkeeping for one run.
//!
//! The service ([`super::TwrService`]) owns the run; this module holds what it
//! runs the loop with, so the iteration without the seam (`control: None`)
//! stays exactly as it was.
//!
//! # Full level
//!
//! A law works at full level. The drive in force is a full-level record; a
//! play scales it to the commanded level, and the measured response and the
//! drive as played are referred back by the scale the play commanded. Today's
//! iteration works at the level (`D_L += beta G (s R - Y)`); at full level it
//! is `D += beta G (R - Y / s)`, played as `s D`, the same arithmetic
//! reordered: the two agree to rounding, and at 0 dB (`s` exactly 1) to the
//! bit.
//!
//! # Conventions
//!
//! Spectra handed to a law are in the `twr-single` world's negative-exponent
//! convention. The identification's FRF is already in it (`hertzforge_dsp`'s
//! estimate returns the standard H); the core's inverse is built for the
//! positive-exponent transform its own spectra use, so it is conjugated here.

use hertzforge_dsp::fft::Cplx;
use hertzforge_shock_common::law_guard::{LawDescriptor, LawValue};

use crate::control::TwrLaw;
use crate::grid::RecordGrid;
use crate::inverse::GridInverse;
use crate::sysid::SysIdResult;

/// The identified plant on the record grid, in the law's convention.
#[derive(Debug, Clone)]
pub(crate) struct WorldPlant {
    /// `H[k]`, control units per volt.
    pub h: Vec<Cplx>,
    /// The core's regularized inverse, `conj(H) / (|H|^2 + eps)`.
    pub inverse: Vec<Cplx>,
    pub coherence: Vec<f64>,
    pub trusted: Vec<bool>,
    pub eps: f64,
}

impl WorldPlant {
    /// The plant behind `inv` and `mask` on `grid`: the identification's FRF
    /// interpolated onto the record grid's bins as the inverse is, the inverse
    /// conjugated into the law's convention.
    pub fn new(sysid: &SysIdResult, inv: &GridInverse, mask: &[bool], grid: &RecordGrid) -> Self {
        let df = sysid.df();
        let last = sysid.h.len() - 1;
        let h = (0..grid.n_bins())
            .map(|k| {
                let pos = grid.freq(k) / df;
                let i0 = (pos.floor() as usize).min(last);
                let i1 = (i0 + 1).min(last);
                let fr = (pos - i0 as f64).clamp(0.0, 1.0);
                let (a, b) = (sysid.h[i0], sysid.h[i1]);
                (a.0 * (1.0 - fr) + b.0 * fr, a.1 * (1.0 - fr) + b.1 * fr)
            })
            .collect();
        WorldPlant {
            h,
            inverse: inv.g.iter().map(|&(re, im)| (re, -im)).collect(),
            coherence: inv.coherence.clone(),
            trusted: mask.to_vec(),
            eps: inv.eps,
        }
    }
}

/// The law, its drive in force and its bookkeeping for one run.
pub(crate) struct LawRun {
    pub law: Box<dyn TwrLaw + Send>,
    pub descriptor: LawDescriptor,
    /// The full-level drive in force, volts: the law's last usable drive, or
    /// the drive as played when the core clipped it.
    pub drive: Vec<f64>,
    /// The values of the last call.
    pub last_values: Vec<LawValue>,
}

/// The closed loop's state for one run.
#[derive(Default)]
pub(crate) struct Closed {
    /// The plant, from a passing identification on.
    pub plant: Option<WorldPlant>,
    /// The law, from proceed on.
    pub run: Option<LawRun>,
}

/// The value a call reported under `key`, if it reported one.
pub(crate) fn value_of(values: &[LawValue], key: &str) -> Option<f64> {
    values.iter().find(|v| v.key == key).map(|v| v.value)
}
