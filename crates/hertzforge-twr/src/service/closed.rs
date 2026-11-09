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

use std::panic::{catch_unwind, AssertUnwindSafe};

use hertzforge_dsp::fft::Cplx;
use hertzforge_shock_common::law_guard::{
    LawCall, LawCallRecord, LawClock, LawDescriptor, LawFault, LawRecordRing, LawValue,
    MissCounter, OutputRule, MAX_LAW_VALUES,
};
use hertzforge_shock_law_math::spectrum::{abs, conj, irfft_half, mul, rfft_half};
use serde::{Deserialize, Serialize};

use crate::control::{PlayInputs, PlayKind, PlayOutputs, TwrLaw, TwrLawFactory, TwrLawSetup};
use crate::grid::RecordGrid;
use crate::inverse::GridInverse;
use crate::service::plays::PlayLawCall;
use crate::sysid::SysIdResult;

/// The farthest the lag fit looks either side of the identification's lag,
/// frames.
pub(crate) const LAG_SEARCH_FRAMES: i64 = 16;

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

impl WorldPlant {
    /// The response the plant predicts to `drive` (one record), control units.
    pub fn predict(&self, drive: &[f64]) -> Result<Vec<f64>, String> {
        let d = rfft_half(drive)?;
        let y: Vec<Cplx> = d.iter().zip(&self.h).map(|(&a, &b)| mul(a, b)).collect();
        irfft_half(&y, drive.len())
    }

    /// The core's own full-level drive: the inverse applied to the full-level
    /// reference on the trusted bins, as the built-in seeds it without a
    /// backoff.
    pub fn full_level_drive(&self, reference: &[f64]) -> Result<Vec<f64>, String> {
        let r = rfft_half(reference)?;
        let d: Vec<Cplx> = r
            .iter()
            .zip(&self.inverse)
            .zip(&self.trusted)
            .map(|((&r, &g), &t)| if t { mul(r, g) } else { (0.0, 0.0) })
            .collect();
        irfft_half(&d, reference.len())
    }

    /// The play's lag against the identification's, frames: the peak of the
    /// cross-correlation of the response with the response this plant
    /// predicts to the drive as played, over the trusted bins, refined between
    /// frames by a parabola through the peak and its neighbours. The
    /// correlation's magnitude is used, so a plant that has changed its sign
    /// or its gain still reads its delay. `None` when nothing correlates.
    pub fn fit_drift(&self, response: &[f64], drive: &[f64]) -> Option<f64> {
        let n = response.len();
        let y = rfft_half(response).ok()?;
        let d = rfft_half(drive).ok()?;
        let x: Vec<Cplx> = (0..y.len().min(self.h.len()))
            .map(|k| {
                if self.trusted[k] {
                    mul(y[k], conj(mul(self.h[k], d[k])))
                } else {
                    (0.0, 0.0)
                }
            })
            .collect();
        let r = irfft_half(&x, n).ok()?;
        let at = |m: i64| r[m.rem_euclid(n as i64) as usize].abs();
        let (mut best, mut bm) = (0.0f64, 0i64);
        for m in -LAG_SEARCH_FRAMES..=LAG_SEARCH_FRAMES {
            let v = at(m);
            if v > best {
                best = v;
                bm = m;
            }
        }
        if best <= 0.0 {
            return None;
        }
        let (a, c) = (at(bm - 1), at(bm + 1));
        let denom = a - 2.0 * best + c;
        let frac = if denom < 0.0 {
            (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Some(bm as f64 + frac)
    }
}

/// `0` at `i = 0`, rising to just under `1` at `i = len - 1`.
fn rise(i: usize, len: usize) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * i as f64 / len as f64).cos()
}

/// The core's at-rest taper: the outer half of the leading and of the
/// trailing quiet pad fall to zero on a raised cosine, so the record's first
/// and last samples are 0 V. The replicated section and the inner halves of
/// the pads are untouched.
pub(crate) fn taper(d: &mut [f64], grid: &RecordGrid) {
    let n = d.len().min(grid.n);
    let head = (grid.pad_pre / 2).min(n);
    let tail = n.saturating_sub(grid.pad_pre + grid.section_len) / 2;
    for (i, v) in d[..head].iter_mut().enumerate() {
        *v *= rise(i, head);
    }
    for (j, v) in d[..n].iter_mut().rev().take(tail).enumerate() {
        *v *= rise(j, tail);
    }
}

pub(crate) fn peak(x: &[f64]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs()))
}

/// Build the law with `factory`, contained: a panic is a crash, not the
/// service's.
pub(crate) fn build_law(
    factory: &dyn TwrLawFactory,
    setup: &TwrLawSetup<'_>,
) -> Result<Box<dyn TwrLaw + Send>, LawFault> {
    match catch_unwind(AssertUnwindSafe(|| factory.build(setup))) {
        Ok(r) => r,
        Err(p) => Err(LawFault::Crashed(panic_text(p))),
    }
}

/// Call the law once, timed on `clock` and contained. A call that returns an
/// output after its budget is over budget; a call that fails keeps its own
/// reason.
pub(crate) fn guarded_call(
    law: &mut (dyn TwrLaw + Send),
    which: LawCall,
    inputs: &PlayInputs<'_>,
    clock: &dyn LawClock,
    budget_s: f64,
) -> (Result<PlayOutputs, LawFault>, f64) {
    let t0 = clock.now_s();
    let r = catch_unwind(AssertUnwindSafe(|| match which {
        LawCall::Seed => law.seed(inputs),
        LawCall::Iterate => law.iterate(inputs),
    }));
    let took_s = (clock.now_s() - t0).max(0.0);
    let r = match r {
        Ok(Ok(_)) if took_s > budget_s => Err(LawFault::OverBudget { took_s, budget_s }),
        Ok(r) => r,
        Err(p) => Err(LawFault::Crashed(panic_text(p))),
    };
    (r, took_s)
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic with no message".into()
    }
}

/// The checks on any output before it is looked at: its length, that every
/// number is finite, and how many values it reports.
pub(crate) fn check_shape(out: &PlayOutputs, n: usize) -> Result<(), LawFault> {
    if out.drive.len() != n {
        return Err(LawFault::BadOutput(OutputRule::WrongLength));
    }
    if out.drive.iter().any(|v| !v.is_finite()) || out.values.iter().any(|v| !v.value.is_finite()) {
        return Err(LawFault::BadOutput(OutputRule::NonFinite));
    }
    if out.values.len() > MAX_LAW_VALUES {
        return Err(LawFault::BadOutput(OutputRule::TooManyValues));
    }
    Ok(())
}

/// How far `candidate` rose over `last` (both full-level records as they
/// play, tapered alike): the peak's rise and the largest bin's, dB. A bin of
/// `last` under `floor_db` below its largest bin is held to that floor first,
/// so a bin rising out of a spectral null is not judged on its ratio alone.
pub(crate) fn rises(candidate: &[f64], last: &[f64], floor_db: f64) -> (f64, f64) {
    let (pn, pl) = (peak(candidate), peak(last));
    let peak_db = if pl > 0.0 && pn > 0.0 {
        20.0 * (pn / pl).log10()
    } else if pn > 0.0 {
        f64::INFINITY
    } else {
        f64::NEG_INFINITY
    };
    let (Ok(a), Ok(b)) = (rfft_half(candidate), rfft_half(last)) else {
        return (peak_db, f64::INFINITY);
    };
    let old: Vec<f64> = b.iter().map(|&c| abs(c)).collect();
    let floor = old.iter().fold(0.0f64, |m, &v| m.max(v)) * 10f64.powf(-floor_db / 20.0);
    let mut bin_db = f64::NEG_INFINITY;
    for (new, old) in a.iter().map(|&c| abs(c)).zip(&old) {
        let base = old.max(floor);
        if base > 0.0 && new > 0.0 {
            bin_db = bin_db.max(20.0 * (new / base).log10());
        } else if new > 0.0 {
            bin_db = f64::INFINITY;
        }
    }
    (peak_db, bin_db)
}

/// One TWR law call's record, as `law_calls` stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TwrCallPayload {
    /// What the play judged before the call was (an iteration at the seed).
    pub kind: PlayKind,
    /// The level the call's drive plays at, dB.
    pub level_db: f64,
    /// The rms error of the play the call followed (`None` at the seed).
    pub rms_error_pct: Option<f64>,
    pub peak_rise_db: Option<f64>,
    pub bin_rise_db: Option<f64>,
    pub predicted_peak_ratio: Option<f64>,
    /// The fitted lag of the play the call followed, frames.
    pub lag_frames: Option<f64>,
    pub values: Vec<LawValue>,
    /// The drive the law returned (empty when it gave none), full level.
    pub drive_out: Vec<f32>,
}

/// One TWR law call's record.
pub type TwrCallRecord = LawCallRecord<TwrCallPayload>;

/// The law's counts over a run.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct LawStats {
    pub calls: u64,
    pub misses: u64,
    pub refused: u64,
    pub worst_call_s: f64,
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
    pub misses: MissCounter,
    pub stats: LawStats,
    pub records: LawRecordRing<TwrCallRecord>,
    /// The call behind the drive the next play plays.
    pub pending: Option<PlayLawCall>,
    pub budget_s: f64,
}

/// The closed loop's state for one run.
#[derive(Default)]
pub(crate) struct Closed {
    /// The plant, from a passing identification on.
    pub plant: Option<WorldPlant>,
    /// The law, from proceed on.
    pub run: Option<LawRun>,
    /// `law.json` has been written into the run folder.
    pub descriptor_written: bool,
}

/// The value a call reported under `key`, if it reported one.
pub(crate) fn value_of(values: &[LawValue], key: &str) -> Option<f64> {
    values.iter().find(|v| v.key == key).map(|v| v.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> RecordGrid {
        RecordGrid::new(1024.0, 600, 128, 200)
    }

    #[test]
    fn the_taper_brings_both_ends_to_rest_and_leaves_the_section_alone() {
        let g = grid();
        let mut d = vec![1.0; g.n];
        taper(&mut d, &g);
        assert_eq!(d[0], 0.0);
        assert_eq!(d[g.n - 1], 0.0);
        let tail = (g.n - g.pad_pre - g.section_len) / 2;
        for (k, v) in d.iter().enumerate().take(g.n - tail).skip(g.pad_pre / 2) {
            assert_eq!(*v, 1.0, "sample {k}");
        }
        assert!(d[1] > 0.0 && d[1] < d[g.pad_pre / 2 - 1]);
    }

    #[test]
    fn a_bin_rising_out_of_a_null_is_held_to_the_floor() {
        let n = 256;
        let tone = |k: f64, a: f64| -> Vec<f64> {
            (0..n)
                .map(|i| a * (std::f64::consts::TAU * k * i as f64 / n as f64).sin())
                .collect()
        };
        let last = tone(10.0, 1.0);
        // A second tone at -50 dB of the first, against a floor of -40 dB: its
        // bin was at the numerical noise and is judged against the floor.
        let small: Vec<f64> = last
            .iter()
            .zip(tone(40.0, 10f64.powf(-50.0 / 20.0)))
            .map(|(a, b)| a + b)
            .collect();
        let (pk, bin) = rises(&small, &last, 40.0);
        assert!(pk.abs() < 0.1, "{pk}");
        // The tone itself is unchanged (0 dB); the small one under the floor
        // has not risen at all.
        assert!(bin < 1e-9, "under the floor it has not risen: {bin}");
        // At -30 dB it is 10 dB over the floor.
        let big: Vec<f64> = last
            .iter()
            .zip(tone(40.0, 10f64.powf(-30.0 / 20.0)))
            .map(|(a, b)| a + b)
            .collect();
        let (_, bin) = rises(&big, &last, 40.0);
        assert!((bin - 10.0).abs() < 0.01, "{bin}");
        let (pk, _) = rises(&tone(10.0, 2.0), &last, 40.0);
        assert!((pk - 20.0 * 2f64.log10()).abs() < 1e-9);
    }
}
