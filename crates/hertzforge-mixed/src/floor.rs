//! The floor's estimate: Hann frames at half overlap of the residual (each
//! tracked tone already subtracted), averaged per line over the frames in
//! which the line could be read cleanly (design D6).
//!
//! - **Per-line average.** A frame adds to a line's average unless that line
//!   is excluded for that frame (inside a dwelling tone's exclusion width, or
//!   under a band with its guard). Each line keeps its own count, so each line
//!   has its own degrees of freedom. The average forgets geometrically once a
//!   line has `depth` frames, so it follows the drive and its degrees of
//!   freedom stop at `depth` frames' worth.
//! - **Degrees of freedom.** Hann frames at half overlap are correlated by
//!   `rho = 1/6` (Welch 1967), so `K` frames give `2K / (1 + 2 rho^2)`.
//! - **Lines with no clean average.** The floor law is given the error (dB,
//!   measured over target) interpolated from the nearest clean lines either
//!   side, so it neither chases a band or a tone nor loses its mean there.
//!   A line a swept band passed is read again from scratch once it is clean:
//!   its average is cleared when it is excluded ([`FloorEstimator::forget`]).

use hertzforge_dsp::fft;

/// Correlation of adjacent Hann frames at half overlap.
pub const HANN_HALF_OVERLAP_RHO: f64 = 1.0 / 6.0;

/// Degrees of freedom per Hann frame at half overlap, for a long average.
pub fn dof_per_frame() -> f64 {
    2.0 / (1.0 + 2.0 * HANN_HALF_OVERLAP_RHO * HANN_HALF_OVERLAP_RHO)
}

/// A periodic Hann window of `n`.
pub fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect()
}

/// One frame's one-sided density periodogram of `x` (length `nfft`, a power
/// of two) under `win`, normalized by `fs sum(w^2)`, mean removed, into
/// `out` (`nfft / 2 + 1` lines). `scratch` is reused (no allocation once it
/// has `nfft` elements).
pub fn periodogram(
    x: &[f64],
    win: &[f64],
    fs: f64,
    scratch: &mut Vec<(f64, f64)>,
    out: &mut [f64],
) {
    let n = x.len();
    scratch.clear();
    let mean = x.iter().sum::<f64>() / n as f64;
    scratch.extend(x.iter().zip(win).map(|(v, w)| ((v - mean) * w, 0.0)));
    fft::fft_in_place(scratch);
    let s2: f64 = win.iter().map(|w| w * w).sum();
    let norm = 1.0 / (fs * s2);
    let half = n / 2;
    for (k, o) in out.iter_mut().enumerate().take(half + 1) {
        let (re, im) = scratch[k];
        let fold = if k == 0 || k == half { 1.0 } else { 2.0 };
        *o = fold * (re * re + im * im) * norm;
    }
}

/// Mark the lines of a grid (`df` spacing, line `k` at `k df`) excluded for
/// one frame: within `half_hz` of each dwelling tone, and from each band's
/// lower edge less `guard` lines to its upper edge plus `guard` lines.
pub fn exclusion_mask(
    df: f64,
    dwelling_tones_hz: &[f64],
    half_hz: f64,
    bands: &[(f64, f64)],
    guard: usize,
    mask: &mut [bool],
) {
    mask.fill(false);
    let n = mask.len();
    let mut mark = |lo: f64, hi: f64| {
        let a = (lo / df).ceil().max(0.0) as usize;
        let b = ((hi / df).floor().max(-1.0) + 1.0) as usize;
        for m in mask.iter_mut().take(b.min(n)).skip(a) {
            *m = true;
        }
    };
    for &f in dwelling_tones_hz {
        mark(f - half_hz, f + half_hz);
    }
    for &(lo, hi) in bands {
        // A line whose bin shares any width with the band is under it.
        let a = ((lo / df - 0.5).floor() + 1.0).max(0.0) as usize;
        let b = (hi / df + 0.5).ceil().max(0.0) as usize;
        for m in mask
            .iter_mut()
            .take((b + guard).min(n))
            .skip(a.saturating_sub(guard))
        {
            *m = true;
        }
    }
}

/// The floor's per-line average over clean frames.
#[derive(Debug, Clone)]
pub struct FloorEstimator {
    sum: Vec<f64>,
    count: Vec<f64>,
    depth: f64,
}

impl FloorEstimator {
    /// An empty estimate over `lines` lines, forgetting past `depth` frames.
    pub fn new(lines: usize, depth: usize) -> Self {
        FloorEstimator {
            sum: vec![0.0; lines],
            count: vec![0.0; lines],
            depth: depth.max(1) as f64,
        }
    }

    /// Lines.
    pub fn lines(&self) -> usize {
        self.sum.len()
    }

    /// Forget everything (a level change settles afresh).
    pub fn reset(&mut self) {
        self.sum.fill(0.0);
        self.count.fill(0.0);
    }

    /// Forget line `k` (a band passed over it).
    pub fn forget(&mut self, k: usize) {
        self.sum[k] = 0.0;
        self.count[k] = 0.0;
    }

    /// Add one frame's composite periodogram `psd`, skipping (and forgetting)
    /// the lines `excluded` marks.
    pub fn add_frame(&mut self, psd: &[f64], excluded: &[bool]) {
        let keep = (self.depth - 1.0) / self.depth;
        #[allow(clippy::needless_range_loop)]
        for k in 0..self.sum.len() {
            if excluded.get(k).copied().unwrap_or(false) {
                self.forget(k);
                continue;
            }
            if self.count[k] >= self.depth {
                self.sum[k] *= keep;
                self.count[k] *= keep;
            }
            self.sum[k] += psd[k];
            self.count[k] += 1.0;
        }
    }

    /// Frames in line `k`'s average (fractional once it forgets).
    pub fn count(&self, k: usize) -> f64 {
        self.count[k]
    }

    /// Line `k`'s degrees of freedom.
    pub fn dof(&self, k: usize) -> f64 {
        let c = self.count[k];
        if c <= 1.0 {
            2.0 * c
        } else {
            dof_per_frame() * c
        }
    }

    /// Line `k`'s average; `None` with no clean frame.
    pub fn line(&self, k: usize) -> Option<f64> {
        (self.count[k] > 0.0).then(|| self.sum[k] / self.count[k])
    }

    /// The estimate the floor law is given over `band` (`[lo, hi)` lines):
    /// the average where a line has one, and on the rest `target` times the
    /// error interpolated (in dB, linearly in line index) from the nearest
    /// clean lines either side (the nearest one at an end; 0 dB with none).
    /// Outside the band, the average or 0. Returns the clean lines in the
    /// band.
    pub fn estimate(&self, target: &[f64], band: (usize, usize), out: &mut [f64]) -> usize {
        let n = self.sum.len().min(out.len()).min(target.len());
        let (lo, hi) = (band.0.min(n), band.1.min(n));
        let err_db = |k: usize| -> Option<f64> {
            let m = self.line(k)?;
            (m > 0.0 && target[k] > 0.0).then(|| 10.0 * (m / target[k]).log10())
        };
        for (k, o) in out.iter_mut().enumerate().take(n) {
            *o = self.line(k).unwrap_or(0.0);
        }
        let mut clean = 0;
        let mut prev: Option<(usize, f64)> = None;
        let mut k = lo;
        while k < hi {
            if let Some(e) = err_db(k) {
                clean += 1;
                prev = Some((k, e));
                k += 1;
                continue;
            }
            // A run of unreadable lines [k, j).
            let mut j = k;
            while j < hi && err_db(j).is_none() {
                j += 1;
            }
            let next = (j < hi).then(|| (j, err_db(j).expect("clean")));
            for (m, o) in out.iter_mut().enumerate().take(j).skip(k) {
                let e = match (prev, next) {
                    (Some((a, ea)), Some((b, eb))) => {
                        ea + (eb - ea) * (m - a) as f64 / (b - a) as f64
                    }
                    (Some((_, e)), None) | (None, Some((_, e))) => e,
                    (None, None) => 0.0,
                };
                *o = target[m] * 10f64.powf(e / 10.0);
            }
            k = j;
        }
        clean
    }

    /// The least degrees of freedom over the clean lines of `band`; 0 with
    /// none.
    pub fn min_dof(&self, band: (usize, usize)) -> f64 {
        let mut m = f64::INFINITY;
        for k in band.0..band.1.min(self.sum.len()) {
            if self.count[k] > 0.0 {
                m = m.min(self.dof(k));
            }
        }
        if m.is_finite() {
            m
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests;
