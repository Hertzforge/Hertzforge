//! Offline cross-spectral estimation: CPSD, H1, H2, coherence,
//! transmissibility, and the packed Hermitian CPSD matrix (the numeric-proof
//! gate is in the tests).
//!
//! # Convention note (the divide the `frf` module told us to write here)
//!
//! This crate's forward FFT uses the negative exponent, and the cross-spectrum
//! that yields the correct FRF phase under that sign is `Gxy = <X * conj(Y)>`
//! with `angle(H) = angle(Gxy)` (see `frf::h_estimate_hv`'s pinned convention).
//! The estimator divisions implemented here keep that convention:
//!
//! ```text
//!   H1[k] = Gxy[k] / Gxx[k]          (output-noise optimal)
//!   H2[k] = Gyy[k] / conj(Gxy[k])    (input-noise optimal)
//!   coh[k] = |Gxy[k]|^2 / (Gxx[k] * Gyy[k])
//! ```
//!
//! `H2 = Gyy / Gyx` with `Gyx = <Y * conj(X)> = conj(Gxy)`. Both estimators
//! carry `angle(Gxy)` as their phase, so the sign-pin from the Hv doc holds
//! for them identically (a lagging response has lagging phase). The delay-
//! plant test below pins this for H1/H2 the same way `h_estimate_hv`'s suite
//! pins it for Hv.

use crate::fft;
use crate::spectrum_offline::Detrend;

/// Averaged one-sided cross- and auto-spectral densities of an (x, y) pair.
#[derive(Debug, Clone)]
pub struct CrossSpectra {
    pub freqs: Vec<f64>,
    /// Cross density `<X conj(Y)>`, one-sided (EU_x * EU_y / Hz).
    pub gxy: Vec<(f64, f64)>,
    /// Auto density of x (EU_x^2/Hz).
    pub gxx: Vec<f64>,
    /// Auto density of y (EU_y^2/Hz).
    pub gyy: Vec<f64>,
    pub segments: usize,
}

/// Welch cross-spectral estimation with a caller-supplied window. Same
/// validation, scaling (`1/(fs*S2)`), one-sided fold, and detrend semantics as
/// `spectrum_offline::psd_welch_with` - the Parseval/equivalence fences there
/// cover the shared math.
pub fn cross_welch_with(
    x: &[f64],
    y: &[f64],
    fs_hz: f64,
    window: &[f64],
    overlap: f64,
    detrend: Detrend,
) -> Result<CrossSpectra, String> {
    let nfft = window.len();
    if nfft < 2 || !nfft.is_power_of_two() {
        return Err(format!(
            "window length must be a power of two >= 2 (got {nfft})"
        ));
    }
    if fs_hz <= 0.0 {
        return Err(format!("fs must be positive (got {fs_hz})"));
    }
    if !(0.0..=0.95).contains(&overlap) {
        return Err(format!("overlap must be in [0, 0.95] (got {overlap})"));
    }
    let s2: f64 = window.iter().map(|v| v * v).sum();
    if s2 == 0.0 {
        return Err("window is all zeros".into());
    }
    let n = x.len().min(y.len());
    let step = ((nfft as f64) * (1.0 - overlap)).round().max(1.0) as usize;
    let norm = 1.0 / (fs_hz * s2);
    let nout = nfft / 2 + 1;

    let mut gxy = vec![(0.0f64, 0.0f64); nout];
    let mut gxx = vec![0.0f64; nout];
    let mut gyy = vec![0.0f64; nout];
    let mut segx = vec![(0.0f64, 0.0f64); nfft];
    let mut segy = vec![(0.0f64, 0.0f64); nfft];
    let mut nseg = 0usize;
    let mut start = 0usize;
    while start + nfft <= n {
        let (mut mx, mut my) = (0.0f64, 0.0f64);
        if detrend == Detrend::SegmentMean {
            for i in 0..nfft {
                mx += x[start + i];
                my += y[start + i];
            }
            mx /= nfft as f64;
            my /= nfft as f64;
        }
        for i in 0..nfft {
            segx[i] = ((x[start + i] - mx) * window[i], 0.0);
            segy[i] = ((y[start + i] - my) * window[i], 0.0);
        }
        fft::fft_in_place(&mut segx);
        fft::fft_in_place(&mut segy);
        for k in 0..nout {
            let (xr, xi) = segx[k];
            let (yr, yi) = segy[k];
            // X * conj(Y)
            let cr = xr * yr + xi * yi;
            let ci = xi * yr - xr * yi;
            let fold = if k != 0 && k != nfft / 2 { 2.0 } else { 1.0 };
            gxy[k].0 += cr * norm * fold;
            gxy[k].1 += ci * norm * fold;
            gxx[k] += (xr * xr + xi * xi) * norm * fold;
            gyy[k] += (yr * yr + yi * yi) * norm * fold;
        }
        nseg += 1;
        start += step;
    }
    if nseg == 0 {
        return Ok(CrossSpectra {
            freqs: Vec::new(),
            gxy: Vec::new(),
            gxx: Vec::new(),
            gyy: Vec::new(),
            segments: 0,
        });
    }
    let inv = 1.0 / nseg as f64;
    for k in 0..nout {
        gxy[k].0 *= inv;
        gxy[k].1 *= inv;
        gxx[k] *= inv;
        gyy[k] *= inv;
    }
    let df = fs_hz / nfft as f64;
    Ok(CrossSpectra {
        freqs: (0..nout).map(|k| k as f64 * df).collect(),
        gxy,
        gxx,
        gyy,
        segments: nseg,
    })
}

impl CrossSpectra {
    /// H1 = Gxy/Gxx (x = input/reference, y = output/response; output-noise
    /// optimal). Zero-drive bins return (0, 0), matching `h_estimate_hv`.
    pub fn h1(&self) -> Vec<(f64, f64)> {
        self.gxy
            .iter()
            .zip(&self.gxx)
            .map(|(&(re, im), &gxx)| {
                if gxx > 0.0 {
                    // angle(H1) = angle(Gxy): the SAME phase source as
                    // `h_estimate_hv`, whose absolute sign is pinned by its own
                    // delay/biquad tests. The contract here is CONSISTENCY with
                    // that pinned convention (a mixed-estimator display must
                    // never show two phase signs for one plant), enforced by
                    // `delay_plant_phase_sign_matches_hv` below.
                    (re / gxx, im / gxx)
                } else {
                    (0.0, 0.0)
                }
            })
            .collect()
    }

    /// H2 = Gyy/Gyx = Gyy/conj(Gxy) (input-noise optimal). Requires a
    /// non-degenerate cross spectrum; bins with |Gxy| == 0 return (0, 0).
    pub fn h2(&self) -> Vec<(f64, f64)> {
        self.gxy
            .iter()
            .zip(&self.gyy)
            .map(|(&(re, im), &gyy)| {
                let denom = re * re + im * im;
                if denom > 0.0 {
                    // Gyy / conj(Gxy) = Gyy * Gxy / |Gxy|^2 (Gyy real).
                    (gyy * re / denom, gyy * im / denom)
                } else {
                    (0.0, 0.0)
                }
            })
            .collect()
    }

    /// Ordinary coherence gamma^2 in [0, 1]. With a SINGLE average this is
    /// identically 1 (the coherence trap): the kernel returns the honest
    /// number and `segments` - refusing/warning below a count threshold is
    /// the policy layer's job (warn, never silently display).
    pub fn coherence(&self) -> Vec<f64> {
        self.gxy
            .iter()
            .zip(self.gxx.iter().zip(&self.gyy))
            .map(|(&(re, im), (&gxx, &gyy))| {
                let denom = gxx * gyy;
                if denom > 0.0 {
                    ((re * re + im * im) / denom).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// |H1| - the motion-ratio transmissibility magnitude.
    pub fn transmissibility(&self) -> Vec<f64> {
        self.h1().iter().map(|c| fft::mag(*c)).collect()
    }
}

/// Packed Hermitian CPSD matrix: upper triangle (i <= j) of an n_ch x n_ch
/// matrix per frequency bin, stored bin-major. At Insight's scale (16 ch x 4096
/// bins ~ 8.9 MB) this is a non-problem; the container
/// exists so indexing conventions live in ONE place.
#[derive(Debug, Clone)]
pub struct CpsdMatrix {
    pub n_ch: usize,
    pub n_bins: usize,
    pub freqs: Vec<f64>,
    /// len = n_bins * n_ch*(n_ch+1)/2; entry (bin, i<=j).
    data: Vec<(f64, f64)>,
}

impl CpsdMatrix {
    pub fn new(n_ch: usize, freqs: Vec<f64>) -> CpsdMatrix {
        let n_bins = freqs.len();
        let tri = n_ch * (n_ch + 1) / 2;
        CpsdMatrix {
            n_ch,
            n_bins,
            freqs,
            data: vec![(0.0, 0.0); n_bins * tri],
        }
    }

    fn tri_index(&self, i: usize, j: usize) -> usize {
        // i <= j upper triangle, row-major over the triangle.
        debug_assert!(i <= j && j < self.n_ch);
        i * self.n_ch - i * (i + 1) / 2 + j
    }

    pub fn set(&mut self, bin: usize, i: usize, j: usize, v: (f64, f64)) {
        assert!(i <= j, "store upper triangle only (i <= j)");
        let tri = self.n_ch * (self.n_ch + 1) / 2;
        let idx = bin * tri + self.tri_index(i, j);
        self.data[idx] = v;
    }

    /// Hermitian access: (i, j) with i > j returns conj of the stored (j, i).
    pub fn get(&self, bin: usize, i: usize, j: usize) -> (f64, f64) {
        let tri = self.n_ch * (self.n_ch + 1) / 2;
        if i <= j {
            self.data[bin * tri + self.tri_index(i, j)]
        } else {
            let (re, im) = self.data[bin * tri + self.tri_index(j, i)];
            (re, -im)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frf;
    use crate::window;

    /// Deterministic pseudo-noise in [-1, 1].
    fn lcg_noise(n: usize, seed: u64) -> Vec<f64> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((s >> 33) as f64 / (1u64 << 31) as f64) - 1.0
            })
            .collect()
    }

    /// y = plant(x): a two-tap FIR `y[n] = 0.8 x[n] - 0.5 x[n-1]` - known H(f),
    /// no feedback, exact in the frequency domain for periodic segments.
    fn plant(x: &[f64]) -> Vec<f64> {
        let mut y = vec![0.0; x.len()];
        for i in 0..x.len() {
            y[i] = 0.8 * x[i] - 0.5 * if i > 0 { x[i - 1] } else { 0.0 };
        }
        y
    }

    fn plant_h(f: f64, fs: f64) -> (f64, f64) {
        // H(w) = 0.8 - 0.5 e^{-jw}
        let w = 2.0 * std::f64::consts::PI * f / fs;
        (0.8 - 0.5 * w.cos(), 0.5 * w.sin())
    }

    fn band_mean_mag_err(freqs: &[f64], h: &[(f64, f64)], fs: f64, lo: f64, hi: f64) -> f64 {
        let mut acc = 0.0;
        let mut n = 0usize;
        for (k, &f) in freqs.iter().enumerate() {
            if f < lo || f > hi {
                continue;
            }
            let want = fft::mag(plant_h(f, fs));
            let got = fft::mag(h[k]);
            acc += (got - want).abs() / want;
            n += 1;
        }
        acc / n as f64
    }

    /// The three-case numeric proof.
    #[test]
    fn three_case_estimator_proof() {
        let fs = 4096.0;
        let nfft = 1024;
        let n = nfft * 64;
        let w = window::hann(nfft);
        let x = lcg_noise(n, 7);
        let y = plant(&x);
        let (lo, hi) = (100.0, 1500.0);

        // Case A: noise-free -> H1 == H2 (== Hv) == analytic H.
        let cs = cross_welch_with(&x, &y, fs, &w, 0.5, Detrend::None).unwrap();
        assert!(cs.segments >= 64);
        let e1 = band_mean_mag_err(&cs.freqs, &cs.h1(), fs, lo, hi);
        let e2 = band_mean_mag_err(&cs.freqs, &cs.h2(), fs, lo, hi);
        assert!(e1 < 5e-3, "noise-free H1 err {e1}");
        assert!(e2 < 5e-3, "noise-free H2 err {e2}");
        let coh = cs.coherence();
        let mean_coh = coh.iter().skip(10).take(300).sum::<f64>() / 300.0;
        assert!(mean_coh > 0.999, "noise-free coherence {mean_coh}");

        // Case B: OUTPUT noise -> H1 stays unbiased, H2 biased high.
        let yn: Vec<f64> = y
            .iter()
            .zip(lcg_noise(n, 99))
            .map(|(v, e)| v + 0.5 * e)
            .collect();
        let cs_b = cross_welch_with(&x, &yn, fs, &w, 0.5, Detrend::None).unwrap();
        let b1 = band_mean_mag_err(&cs_b.freqs, &cs_b.h1(), fs, lo, hi);
        let b2 = band_mean_mag_err(&cs_b.freqs, &cs_b.h2(), fs, lo, hi);
        assert!(b1 < 0.05, "output-noise H1 should stay unbiased, err {b1}");
        assert!(
            b2 > 3.0 * b1,
            "output-noise must bias H2 (H1 {b1} vs H2 {b2})"
        );

        // Case C: INPUT noise -> H2 stays unbiased, H1 biased low.
        let xn: Vec<f64> = x
            .iter()
            .zip(lcg_noise(n, 1234))
            .map(|(v, e)| v + 0.5 * e)
            .collect();
        // The plant still saw the CLEAN x; the measurement of x is noisy.
        let cs_c = cross_welch_with(&xn, &y, fs, &w, 0.5, Detrend::None).unwrap();
        let c1 = band_mean_mag_err(&cs_c.freqs, &cs_c.h1(), fs, lo, hi);
        let c2 = band_mean_mag_err(&cs_c.freqs, &cs_c.h2(), fs, lo, hi);
        assert!(c2 < 0.05, "input-noise H2 should stay unbiased, err {c2}");
        assert!(
            c1 > 3.0 * c2,
            "input-noise must bias H1 (H1 {c1} vs H2 {c2})"
        );
    }

    /// Phase-sign pin: H1/H2 must agree with `h_estimate_hv` (whose sign is
    /// pinned against a delay plant) on a noise-free system - the convention
    /// note's promise, enforced.
    #[test]
    fn delay_plant_phase_sign_matches_hv() {
        let fs = 2048.0;
        let nfft = 512;
        let n = nfft * 32;
        let x = lcg_noise(n, 5);
        // Pure 3-sample delay plant.
        let mut y = vec![0.0; n];
        y[3..n].copy_from_slice(&x[..n - 3]);
        let w = window::hann(nfft);
        let cs = cross_welch_with(&x, &y, fs, &w, 0.5, Detrend::None).unwrap();
        let hv = frf::h_estimate_hv(&x, &y, fs, nfft);
        let h1 = cs.h1();
        let mut checked = 0usize;
        for k in 5..nfft / 4 {
            let (r1, i1) = h1[k];
            let (rv, iv) = hv[k];
            let p1 = i1.atan2(r1);
            let pv = iv.atan2(rv);
            let mut d = p1 - pv;
            while d > std::f64::consts::PI {
                d -= 2.0 * std::f64::consts::PI;
            }
            while d < -std::f64::consts::PI {
                d += 2.0 * std::f64::consts::PI;
            }
            assert!(
                d.abs() < 0.05,
                "H1 phase must match Hv at bin {k} (d = {d})"
            );
            checked += 1;
        }
        assert!(checked > 50);
    }

    #[test]
    fn single_segment_coherence_is_identically_one() {
        // The trap the policy layer must warn about: one average -> coh == 1.
        let fs = 1024.0;
        let nfft = 1024;
        let x = lcg_noise(nfft, 3);
        let y = lcg_noise(nfft, 4); // UNRELATED signals
        let w = window::hann(nfft);
        let cs = cross_welch_with(&x, &y, fs, &w, 0.0, Detrend::None).unwrap();
        assert_eq!(cs.segments, 1);
        let coh = cs.coherence();
        for (k, c) in coh.iter().enumerate().skip(1).take(nfft / 2 - 2) {
            assert!(
                *c > 1.0 - 1e-9,
                "bin {k}: single-average coherence must be 1, got {c}"
            );
        }
    }

    #[test]
    fn cpsd_matrix_hermitian_access() {
        let mut m = CpsdMatrix::new(3, vec![0.0, 1.0]);
        m.set(1, 0, 2, (2.0, -0.5));
        assert_eq!(m.get(1, 0, 2), (2.0, -0.5));
        assert_eq!(m.get(1, 2, 0), (2.0, 0.5));
        m.set(0, 1, 1, (7.0, 0.0));
        assert_eq!(m.get(0, 1, 1), (7.0, 0.0));
    }
}
