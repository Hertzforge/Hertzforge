//! The core's checks and limits on a law's drive (design 8). The law proposes,
//! the core disposes: every rule here runs whatever law produced the drive.
//!
//! - [`check_output`]: dimensions, finiteness, Hermitian within `1e-9`, zero out of
//!   band, positive semi-definite (eigenvalues below `-1e-6 lambda_max` clamped
//!   with the diagonal restored and counted; one below `-1e-3 lambda_max` fails
//!   the call), Q30.
//! - [`cap_change`]: each drive's PSD may move at most `up_db` up and `down_db`
//!   down per loop from the drive in force, by a diagonal congruence
//!   `D^(1/2) G D^(1/2)` (design 8.1), which keeps the matrix positive
//!   semi-definite and every pair's coherence.
//! - [`apply_ceilings`]: each drive's rms at full level is held at its voltage
//!   ceiling, by the same congruence with one factor per drive.
//! - [`limit_responses`]: each limit channel's PSD, predicted as
//!   `(H_l G_dd H_l^H)_ii`, is held under its ceiling by scaling the whole line
//!   (design 8.2), which keeps every coherence and phase.

use crate::pool;
use hertzforge_sdm::linalg::{RepairReport, PSD_CLAMP_REL, PSD_REJECT_REL};
use hertzforge_sdm::{HermitianStack, MatrixStack};
use serde::{Deserialize, Serialize};

/// The Hermitian tolerance on a law's drive, relative Frobenius error per line.
pub const HERMITIAN_TOL: f64 = 1e-9;

/// A line that has no drive in force may rise from this far below the drive's
/// in-band mean (the single-output core's floor, applied per drive).
pub const CAP_FLOOR_DB: f64 = -60.0;

/// Why a law's drive was not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFault {
    /// The wrong number of lines or drives.
    Dimensions,
    /// A value that is not a finite number.
    NonFinite,
    /// A line that is not Hermitian within the tolerance.
    NotHermitian,
    /// A nonzero value outside the band.
    OutOfBand,
    /// A line with a negative eigenvalue beyond what the core repairs.
    NotPositiveSemiDefinite,
}

impl OutputFault {
    /// In plain words, said of the law's drive.
    pub fn words(self) -> &'static str {
        match self {
            OutputFault::Dimensions => "its drive had the wrong number of lines or drives",
            OutputFault::NonFinite => "its drive held a value that is not a number",
            OutputFault::NotHermitian => "its drive was not a Hermitian matrix",
            OutputFault::OutOfBand => "its drive was not zero outside the band",
            OutputFault::NotPositiveSemiDefinite => {
                "its drive asked for a negative power the core does not repair"
            }
        }
    }
}

/// What the output check repaired.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputRepair {
    /// Lines rebuilt by the eigenvalue clamp.
    pub lines_repaired: usize,
    /// Eigenvalues clamped, over every line.
    pub eigenvalues_clamped: usize,
    /// The most negative eigenvalue relative to its line's largest (0 for none).
    pub worst_relative: f64,
}

impl From<&RepairReport> for OutputRepair {
    fn from(r: &RepairReport) -> Self {
        OutputRepair {
            lines_repaired: r.lines_repaired(),
            eigenvalues_clamped: r.total_clamped(),
            worst_relative: r.worst_relative,
        }
    }
}

/// Check a law's drive (`n` by `n` on `lines` lines) and repair what the rules
/// allow. On an error the drive must not be used.
pub fn check_output(
    drive: &mut HermitianStack,
    lines: usize,
    n: usize,
    band: (usize, usize),
) -> Result<OutputRepair, (OutputFault, String)> {
    if drive.lines() != lines || drive.n() != n {
        return Err((
            OutputFault::Dimensions,
            format!(
                "{} lines of {} x {} against {lines} lines of {n} x {n}",
                drive.lines(),
                drive.n(),
                drive.n()
            ),
        ));
    }
    if let Some(line) = (0..lines).find(|&l| {
        drive
            .line(l)
            .iter()
            .any(|v| !(v.re.is_finite() && v.im.is_finite()))
    }) {
        return Err((
            OutputFault::NonFinite,
            format!("line {line} holds a value that is not a number"),
        ));
    }
    // A packed line can only be non-Hermitian through its diagonal's imaginary
    // parts; judge the line as the full matrix it stands for.
    let per = HermitianStack::packed_len(n);
    for line in 0..lines {
        let vals = drive.line(line);
        let mut num = 0.0;
        let mut den = 0.0;
        for r in 0..n {
            for c in r..n {
                let v = vals[HermitianStack::packed_index(n, r, c)];
                if r == c {
                    num += (2.0 * v.im) * (2.0 * v.im);
                    den += v.norm_sqr();
                } else {
                    den += 2.0 * v.norm_sqr();
                }
            }
        }
        let e = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
        if e > HERMITIAN_TOL {
            return Err((
                OutputFault::NotHermitian,
                format!("line {line}: relative error {e:.2e} against {HERMITIAN_TOL:.0e}"),
            ));
        }
    }
    let (lo, hi) = band;
    if let Some(line) = (0..lines)
        .filter(|&l| l < lo || l >= hi)
        .find(|&l| drive.line(l).iter().any(|v| v.re != 0.0 || v.im != 0.0))
    {
        return Err((
            OutputFault::OutOfBand,
            format!("line {line} is outside the band [{lo}, {hi}) and is not zero"),
        ));
    }
    // Drop the (tolerated) diagonal imaginary parts before the repair.
    let mut data = drive.data().to_vec();
    for line in 0..lines {
        for i in 0..n {
            data[line * per + HermitianStack::packed_index(n, i, i)].im = 0.0;
        }
    }
    *drive = HermitianStack::from_packed(lines, n, data).expect("same shape");
    match pool::psd_repair(drive, PSD_CLAMP_REL, PSD_REJECT_REL, band) {
        Ok(rep) => Ok(OutputRepair::from(&rep)),
        Err(e) => Err((OutputFault::NotPositiveSemiDefinite, e.to_string())),
    }
}

/// What a cap or a ceiling did: the power factor applied to each drive on each
/// line, line-major (`lines * drives`, 1 where nothing was done).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveFactors {
    pub drives: usize,
    pub factors: Vec<f64>,
    /// Drive-lines the rule changed.
    pub bound: usize,
    /// Largest change a drive's PSD asked for in one loop, dB (caps only).
    pub peak_change_db: f64,
}

impl DriveFactors {
    fn ones(lines: usize, drives: usize) -> Self {
        DriveFactors {
            drives,
            factors: vec![1.0; lines * drives],
            bound: 0,
            peak_change_db: 0.0,
        }
    }

    /// Drive-lines bound per line.
    pub fn bound_lines(&self) -> Vec<bool> {
        self.factors
            .chunks(self.drives.max(1))
            .map(|c| c.iter().any(|&f| f != 1.0))
            .collect()
    }
}

/// Hold each drive's PSD within `up_db` / `down_db` of the drive in force, per
/// line, by a diagonal congruence. A line of the drive in force at zero may rise
/// from [`CAP_FLOOR_DB`] below that drive's in-band mean.
pub fn cap_change(
    new: &mut HermitianStack,
    old: &HermitianStack,
    band: (usize, usize),
    up_db: f64,
    down_db: f64,
) -> DriveFactors {
    let (lines, n) = (new.lines(), new.n());
    let mut out = DriveFactors::ones(lines, n);
    let (lo, hi) = band;
    let up = 10f64.powf(up_db / 10.0);
    let down = 10f64.powf(-down_db / 10.0);
    let floor = 10f64.powf(CAP_FLOOR_DB / 10.0);
    let means: Vec<f64> = (0..n)
        .map(|k| {
            let s: f64 = (lo..hi).map(|l| old.get(l, k, k).re.max(0.0)).sum();
            s / (hi - lo).max(1) as f64
        })
        .collect();
    for line in lo..hi {
        for k in 0..n {
            let o = old.get(line, k, k).re.max(0.0);
            let v = new.get(line, k, k).re;
            if v <= 0.0 {
                continue;
            }
            let base = o.max(means[k] * floor);
            if base <= 0.0 {
                continue;
            }
            let change_db = 10.0 * (v / base).log10();
            if change_db.abs() > out.peak_change_db {
                out.peak_change_db = change_db.abs();
            }
            let f = if v > base * up {
                base * up / v
            } else if o > 0.0 && v < o * down {
                o * down / v
            } else {
                1.0
            };
            if f != 1.0 {
                out.factors[line * n + k] = f;
                out.bound += 1;
            }
        }
    }
    if out.bound > 0 {
        new.diagonal_congruence(&out.factors)
            .expect("one factor per drive per line");
    }
    out
}

/// The rms of each drive over the band at the stack's level, volts.
pub fn drive_rms(g: &HermitianStack, band: (usize, usize), df_hz: f64) -> Vec<f64> {
    (0..g.n())
        .map(|k| {
            let s: f64 = (band.0..band.1).map(|l| g.get(l, k, k).re.max(0.0)).sum();
            (s * df_hz).sqrt()
        })
        .collect()
}

/// Hold each drive's full-level rms at or under its ceiling: a drive over its
/// ceiling is scaled by `(ceiling / rms)^2` on every line (a diagonal congruence,
/// so the other drives and every coherence are untouched).
pub fn apply_ceilings(
    g: &mut HermitianStack,
    band: (usize, usize),
    df_hz: f64,
    ceilings_vrms: &[f64],
) -> DriveFactors {
    let (lines, n) = (g.lines(), g.n());
    let mut out = DriveFactors::ones(lines, n);
    let rms = drive_rms(g, band, df_hz);
    let mut per_drive = vec![1.0; n];
    for k in 0..n {
        if rms[k] > ceilings_vrms[k] && rms[k] > 0.0 {
            per_drive[k] = (ceilings_vrms[k] / rms[k]).powi(2);
        }
    }
    if per_drive.iter().all(|&f| f == 1.0) {
        return out;
    }
    for line in 0..lines {
        for k in 0..n {
            if per_drive[k] != 1.0 {
                out.factors[line * n + k] = per_drive[k];
                if (band.0..band.1).contains(&line) {
                    out.bound += 1;
                }
            }
        }
    }
    g.diagonal_congruence(&out.factors)
        .expect("one factor per drive per line");
    out
}

/// The predicted PSD of each limit channel per line, `(H_l G_dd H_l^H)_ii`,
/// line-major (`lines * limits`).
pub fn predict_limits(h_l: &MatrixStack, g: &HermitianStack, band: (usize, usize)) -> Vec<f64> {
    let (lines, nl, nd) = h_l.dims();
    let mut out = vec![0.0; lines * nl];
    for line in band.0..band.1 {
        for i in 0..nl {
            let mut acc = 0.0;
            for a in 0..nd {
                let ha = h_l.get(line, i, a);
                for b in 0..nd {
                    acc += (ha * g.get(line, a, b) * h_l.get(line, i, b).conj()).re;
                }
            }
            out[line * nl + i] = acc.max(0.0);
        }
    }
    out
}

/// Scale each line whose predicted limit PSD exceeds its ceiling by
/// `min_i ceiling_i / predicted_i`. Returns the factor per line (1 where nothing
/// was done) and the lines limited.
pub fn limit_responses(
    g: &mut HermitianStack,
    h_l: &MatrixStack,
    ceilings: &dyn Fn(usize, usize) -> f64,
    band: (usize, usize),
) -> (Vec<f64>, usize) {
    let lines = g.lines();
    let nl = h_l.rows();
    let mut factors = vec![1.0; lines];
    if nl == 0 {
        return (factors, 0);
    }
    let pred = predict_limits(h_l, g, band);
    let mut limited = 0;
    for line in band.0..band.1 {
        let mut s = 1.0f64;
        for i in 0..nl {
            let p = pred[line * nl + i];
            let c = ceilings(i, line);
            if p > c && p > 0.0 {
                s = s.min(c / p);
            }
        }
        if s < 1.0 {
            factors[line] = s;
            limited += 1;
        }
    }
    if limited > 0 {
        g.scale_lines(&factors).expect("one factor per line");
    }
    (factors, limited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_sdm::C64;

    fn two_by_two(lines: usize, a: f64, b: f64, c: C64) -> HermitianStack {
        let mut g = HermitianStack::zeros(lines, 2);
        for l in 1..lines - 1 {
            g.set(l, 0, 0, C64::new(a, 0.0));
            g.set(l, 1, 1, C64::new(b, 0.0));
            g.set(l, 0, 1, c);
        }
        g
    }

    #[test]
    fn caps_hold_each_drive_and_keep_coherence() {
        let old = two_by_two(8, 1.0, 1.0, C64::new(0.5, 0.0));
        let mut new = two_by_two(8, 10.0, 1.0, C64::new(1.5, 1.0));
        let coh = |g: &HermitianStack| {
            g.get(3, 0, 1).norm_sqr() / (g.get(3, 0, 0).re * g.get(3, 1, 1).re)
        };
        let before = coh(&new);
        let f = cap_change(&mut new, &old, (1, 7), 3.5, 6.0);
        assert!((10.0 * new.get(3, 0, 0).re.log10() - 3.5).abs() < 1e-9);
        assert!((new.get(3, 1, 1).re - 1.0).abs() < 1e-12);
        assert!((coh(&new) - before).abs() < 1e-12);
        assert_eq!(f.bound, 6);
        assert!((f.peak_change_db - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_ceiling_scales_only_its_drive() {
        let mut g = two_by_two(8, 1.0, 4.0, C64::new(0.5, 0.5));
        let rms = drive_rms(&g, (1, 7), 1.0);
        assert!((rms[1] - (24.0f64).sqrt()).abs() < 1e-12);
        let f = apply_ceilings(&mut g, (1, 7), 1.0, &[10.0, 2.0]);
        let after = drive_rms(&g, (1, 7), 1.0);
        assert!((after[1] - 2.0).abs() < 1e-12);
        assert!((after[0] - rms[0]).abs() < 1e-12);
        assert!(f.bound > 0);
    }

    #[test]
    fn a_large_negative_eigenvalue_is_refused_and_a_small_one_repaired() {
        // [[1, 1.0005], [1.0005, 1]]: eigenvalues 2.0005 and -0.0005, relative
        // -2.5e-4: clamped (between 1e-6 and 1e-3).
        let mut g = two_by_two(8, 1.0, 1.0, C64::new(1.0005, 0.0));
        let rep = check_output(&mut g, 8, 2, (1, 7)).expect("repaired");
        assert_eq!(rep.lines_repaired, 6);
        assert!((g.get(3, 0, 0).re - 1.0).abs() < 1e-12, "diagonal restored");
        // [[1, 1.1], [1.1, 1]]: eigenvalue -0.1 of 2.1, refused.
        let mut g = two_by_two(8, 1.0, 1.0, C64::new(1.1, 0.0));
        let e = check_output(&mut g, 8, 2, (1, 7)).unwrap_err();
        assert_eq!(e.0, OutputFault::NotPositiveSemiDefinite);
    }

    #[test]
    fn limits_scale_the_whole_line() {
        let mut g = two_by_two(8, 1.0, 1.0, C64::new(0.0, 0.0));
        let mut h = MatrixStack::zeros(8, 1, 2);
        for l in 0..8 {
            h.set(l, 0, 0, C64::new(2.0, 0.0));
        }
        let (f, n) = limit_responses(&mut g, &h, &|_, _| 1.0, (1, 7));
        assert_eq!(n, 6);
        assert!((f[3] - 0.25).abs() < 1e-12);
        assert!((g.get(3, 1, 1).re - 0.25).abs() < 1e-12);
    }
}
