//! Welch estimates of the spectral matrices a loop needs, in the `E[X X^H]`
//! convention (design 7.0).
//!
//! One pass over the loop's records gives the control response SDM `G_cc`, the
//! response-to-drive cross spectrum `G_rd = E[R D^H]` (controls, then limit
//! channels, by drives), the drive SDM `G_dd` and the autospectrum of every
//! response on every line. Each channel is transformed once per frame with the
//! workspace FFT and window (`hertzforge_dsp`), with the scaling of
//! `hertzforge_dsp::spectrum::psd_welch`: segment mean removed, periodic Hann,
//! `2 / (fs * sum(w^2) * K)` on the interior lines, so the integral of a PSD is the
//! signal's variance. The outer products are then accumulated per line, the band's
//! lines split over threads.
//!
//! **The conjugation (Q43).** The workspace FFT's forward transform uses the
//! positive exponent although its documentation states the negative one, so its
//! spectra are the conjugates of the analysis-convention spectra. The estimator
//! measures the sign ([`hertzforge_dsp::cola::forward_exponent_is_positive`]) and
//! conjugates explicitly, so the result is `E[X X^H]` with
//! `X(f) = sum x(t) e^(-j 2 pi f t)` whichever way the transform is written. The
//! tests pin it with a known delay: `y(t) = x(t - d)` must give
//! `G_yx = e^(-j 2 pi f d) G_xx`.
//!
//! Reference: J. S. Bendat and A. G. Piersol, "Random Data: Analysis and
//! Measurement Procedures", 4th ed., Wiley (2010); P. D. Welch, "The Use of Fast
//! Fourier Transform for the Estimation of Power Spectra", IEEE Transactions on
//! Audio and Electroacoustics 15 (1967).

use crate::pool;
use hertzforge_dsp::{cola, fft, window};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};
use std::ops::Range;

/// The spectral matrices of one loop (or an average of loops).
#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    /// Control response SDM, controls by controls, band lines (zero outside).
    pub g_cc: HermitianStack,
    /// Response-to-drive cross spectrum `E[R D^H]`: controls then limit channels,
    /// by drives; band lines.
    pub g_rd: MatrixStack,
    /// Drive SDM, drives by drives; band lines.
    pub g_dd: HermitianStack,
    /// Autospectrum of every response (controls, then limits), every line,
    /// line-major (`lines * responses`).
    pub auto: Vec<f64>,
    /// Welch frames behind the estimate (summed over averaged loops).
    pub frames: usize,
}

impl Estimate {
    /// All zeros.
    pub fn zeros(lines: usize, n_c: usize, n_r: usize, n_d: usize) -> Self {
        Estimate {
            g_cc: HermitianStack::zeros(lines, n_c),
            g_rd: MatrixStack::zeros(lines, n_r, n_d),
            g_dd: HermitianStack::zeros(lines, n_d),
            auto: vec![0.0; lines * n_r],
            frames: 0,
        }
    }

    /// Scale every quantity by a power ratio (a level change: every entry is a
    /// product of two amplitudes that both follow the level).
    pub fn scale(&mut self, power: f64) {
        let lines = self.g_cc.lines();
        let f = vec![power; lines];
        let _ = self.g_cc.scale_lines(&f);
        let _ = self.g_dd.scale_lines(&f);
        for v in self.g_rd.data_mut() {
            *v *= power;
        }
        for v in &mut self.auto {
            *v *= power;
        }
    }

    /// `self += w (other - self)`, every quantity; the frame count adds.
    pub fn blend(&mut self, other: &Estimate, w: f64) {
        fn mix(a: &mut [C64], b: &[C64], w: f64) {
            for (x, y) in a.iter_mut().zip(b) {
                *x += (*y - *x) * w;
            }
        }
        let mut cc = self.g_cc.data().to_vec();
        mix(&mut cc, other.g_cc.data(), w);
        self.g_cc =
            HermitianStack::from_packed(self.g_cc.lines(), self.g_cc.n(), cc).expect("same shape");
        let mut dd = self.g_dd.data().to_vec();
        mix(&mut dd, other.g_dd.data(), w);
        self.g_dd =
            HermitianStack::from_packed(self.g_dd.lines(), self.g_dd.n(), dd).expect("same shape");
        mix(self.g_rd.data_mut(), other.g_rd.data(), w);
        for (x, y) in self.auto.iter_mut().zip(&other.auto) {
            *x += (y - *x) * w;
        }
        self.frames += other.frames;
    }

    /// The control-to-drive block of `g_rd` (the first `n_c` rows).
    pub fn g_cd(&self, n_c: usize) -> MatrixStack {
        rows(&self.g_rd, 0..n_c)
    }

    /// The limit-to-drive block of `g_rd` (rows `n_c..`).
    pub fn g_ld(&self, n_c: usize) -> MatrixStack {
        rows(&self.g_rd, n_c..self.g_rd.rows())
    }
}

/// The lag search's window, log2 frames: 16 384, four times the service's longest
/// lag.
const LAG_WINDOW_LOG2: u32 = 14;

/// The lag between a drive and the responses it causes, frames in `0..=max_lag`:
/// the peak of the whitened cross-correlation of `drive` with each control,
/// summed in power over the controls.
///
/// Every control record has one length `len` and ends with `drive` (the drive as
/// played may begin earlier). The window is the last `n` frames of each, `n` the
/// power of two at or below `len`, at most 16 384, Hann weighted. Only the lines of
/// `band` (half-open, in lines of a `nfft`-point frame, mapped to `n` points) are
/// kept, and each is divided by its magnitude, so every line in the band weighs
/// alike and the peak falls at the response's onset, not where a lightly damped
/// structure's ringing lines up with the drive. A line with no cross power (a
/// silent drive or control) adds nothing.
///
/// `y(t) = x(t - d)` peaks at circular index `n - d` whichever sign the forward
/// transform uses, so lag `d` reads index `(n - d) mod n`. The correlation is
/// circular over `n`: a lag above `n / 2` is not told apart from a lead of
/// `n - lag`. A response does not lead the drive that causes it, so the index is
/// read as the lag. The Hann weighting leaves little of a lag near `n` to
/// correlate, so the window should be several times the longest lag: the
/// service's is four times it.
///
/// The smallest lag wins a tie, so no cross power at all gives 0.
///
/// Cost: one transform for the drive and two for each control, nine at four
/// controls, at `n` points.
pub fn whitened_lag(
    controls: &[&[f64]],
    drive: &[f64],
    band: (usize, usize),
    nfft: usize,
    max_lag: usize,
) -> usize {
    let len = controls.first().map(|c| c.len()).unwrap_or(0);
    if len == 0 || drive.len() < len || nfft == 0 {
        return 0;
    }
    let n = 1usize << len.ilog2().min(LAG_WINDOW_LOG2);
    let hann = window::hann(n);
    let spectrum = |x: &[f64]| {
        let mut z: Vec<(f64, f64)> = x.iter().zip(&hann).map(|(v, w)| (v * w, 0.0)).collect();
        fft::fft_in_place(&mut z);
        z
    };
    let d = spectrum(&drive[drive.len() - n..]);
    let lo = band.0 * n / nfft;
    let hi = (band.1 * n / nfft).min(n / 2);
    let mut power = vec![0.0f64; n];
    for r in controls {
        let y = spectrum(&r[r.len() - n..]);
        let mut c = vec![(0.0f64, 0.0f64); n];
        for k in lo..hi {
            let v = C64::new(y[k].0, y[k].1) * C64::new(d[k].0, -d[k].1);
            let m = v.norm();
            if m > 0.0 {
                c[k] = (v.re / m, v.im / m);
            }
        }
        fft::fft_in_place(&mut c);
        for (p, v) in power.iter_mut().zip(&c) {
            *p += v.0 * v.0 + v.1 * v.1;
        }
    }
    let mut best = (0usize, 0.0f64);
    for lag in 0..=max_lag.min(n - 1) {
        let p = power[(n - lag) % n];
        if p > best.1 {
            best = (lag, p);
        }
    }
    best.0
}

/// Rows `r` of every line of a matrix stack.
pub fn rows(m: &MatrixStack, r: Range<usize>) -> MatrixStack {
    let (lines, _, cols) = m.dims();
    let mut out = MatrixStack::zeros(lines, r.len(), cols);
    for line in 0..lines {
        for (i, row) in r.clone().enumerate() {
            for c in 0..cols {
                out.set(line, i, c, m.get(line, row, c));
            }
        }
    }
    out
}

/// A Welch spectral-matrix estimator on one grid.
#[derive(Debug, Clone)]
pub struct SdmEstimator {
    nfft: usize,
    step: usize,
    window: Vec<f64>,
    /// `1 / (fs * sum(w^2))`.
    norm: f64,
    /// Conjugate the transform's output to reach the analysis convention.
    conjugate: bool,
    band: (usize, usize),
}

impl SdmEstimator {
    /// `nfft` a power of two, `overlap` in `[0, 0.95]`, cross spectra on `band`.
    pub fn new(nfft: usize, fs_hz: f64, overlap: f64, band: (usize, usize)) -> Self {
        Self::with_window(
            nfft,
            fs_hz,
            overlap,
            crate::config::EstimationWindow::Hann,
            band,
        )
    }

    /// As [`Self::new`], through the chosen window.
    pub fn with_window(
        nfft: usize,
        fs_hz: f64,
        overlap: f64,
        kind: crate::config::EstimationWindow,
        band: (usize, usize),
    ) -> Self {
        let window = match kind {
            crate::config::EstimationWindow::Hann => window::hann(nfft),
            crate::config::EstimationWindow::Rectangular => vec![1.0; nfft],
        };
        let s2: f64 = window.iter().map(|w| w * w).sum();
        SdmEstimator {
            nfft,
            step: ((nfft as f64) * (1.0 - overlap.clamp(0.0, 0.95)))
                .round()
                .max(1.0) as usize,
            window,
            norm: 1.0 / (fs_hz * s2),
            conjugate: cola::forward_exponent_is_positive(),
            band,
        }
    }

    /// Lines on the grid.
    pub fn lines(&self) -> usize {
        self.nfft / 2 + 1
    }

    /// Welch frames a record of `len` samples gives.
    pub fn frames_in(&self, len: usize) -> usize {
        if len < self.nfft {
            0
        } else {
            (len - self.nfft) / self.step + 1
        }
    }

    /// The one-sided spectrum of every frame of `x`: `frames * lines` values,
    /// frame-major, in the analysis convention.
    fn spectra(&self, x: &[f64]) -> Vec<C64> {
        let n = self.nfft;
        let lines = self.lines();
        let k = self.frames_in(x.len());
        let mut out = Vec::with_capacity(k * lines);
        let mut seg = vec![(0.0f64, 0.0f64); n];
        for f in 0..k {
            let s = &x[f * self.step..f * self.step + n];
            let mean = s.iter().sum::<f64>() / n as f64;
            for i in 0..n {
                seg[i] = ((s[i] - mean) * self.window[i], 0.0);
            }
            fft::fft_in_place(&mut seg);
            for &(re, im) in &seg[..lines] {
                out.push(if self.conjugate {
                    C64::new(re, -im)
                } else {
                    C64::new(re, im)
                });
            }
        }
        out
    }

    /// The scale of line `k` for a sum over `frames` frames.
    fn scale(&self, k: usize, frames: usize) -> f64 {
        let fold = if k == 0 || k == self.nfft / 2 {
            1.0
        } else {
            2.0
        };
        fold * self.norm / frames.max(1) as f64
    }

    /// Estimate from equal-length records: `responses` (the controls first, then
    /// any limit channels) and `drives`, aligned so a response sample and a drive
    /// sample of the same index belong together. `n_controls` of the responses get
    /// the full SDM.
    pub fn estimate(&self, responses: &[&[f64]], n_controls: usize, drives: &[&[f64]]) -> Estimate {
        let lines = self.lines();
        let (n_r, n_d) = (responses.len(), drives.len());
        let n_c = n_controls.min(n_r);
        let len = responses
            .iter()
            .chain(drives)
            .map(|x| x.len())
            .min()
            .unwrap_or(0);
        let k = self.frames_in(len);
        if k == 0 {
            return Estimate::zeros(lines, n_c, n_r, n_d);
        }
        // Every channel's spectra, the channels split over threads.
        let chans: Vec<&[f64]> = responses.iter().chain(drives).map(|x| &x[..len]).collect();
        let spec: Vec<Vec<C64>> = pool::par_map((0, chans.len()), |r| {
            r.map(|c| self.spectra(chans[c])).collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect();
        // Autospectra of the responses, every line.
        let mut auto = vec![0.0; lines * n_r];
        for (ch, s) in spec.iter().take(n_r).enumerate() {
            for f in 0..k {
                for line in 0..lines {
                    auto[line * n_r + ch] += s[f * lines + line].norm_sqr();
                }
            }
        }
        for line in 0..lines {
            let sc = self.scale(line, k);
            for ch in 0..n_r {
                auto[line * n_r + ch] *= sc;
            }
        }
        // The matrices on the band's lines.
        let band = self.band;
        let parts = pool::par_map(band, |r| {
            let nl = r.len();
            let mut cc = HermitianStack::zeros(nl, n_c);
            let mut rd = MatrixStack::zeros(nl, n_r, n_d);
            let mut dd = HermitianStack::zeros(nl, n_d);
            let mut x = vec![C64::new(0.0, 0.0); n_r + n_d];
            let per_cc = HermitianStack::packed_len(n_c);
            let per_dd = HermitianStack::packed_len(n_d);
            let mut acc_cc = vec![C64::new(0.0, 0.0); per_cc];
            let mut acc_dd = vec![C64::new(0.0, 0.0); per_dd];
            let mut acc_rd = vec![C64::new(0.0, 0.0); n_r * n_d];
            for (li, line) in r.clone().enumerate() {
                acc_cc.fill(C64::new(0.0, 0.0));
                acc_dd.fill(C64::new(0.0, 0.0));
                acc_rd.fill(C64::new(0.0, 0.0));
                for f in 0..k {
                    for (c, s) in spec.iter().enumerate() {
                        x[c] = s[f * lines + line];
                    }
                    let (resp, drv) = x.split_at(n_r);
                    let mut p = 0;
                    for i in 0..n_c {
                        for j in i..n_c {
                            acc_cc[p] += resp[i] * resp[j].conj();
                            p += 1;
                        }
                    }
                    let mut p = 0;
                    for i in 0..n_d {
                        for j in i..n_d {
                            acc_dd[p] += drv[i] * drv[j].conj();
                            p += 1;
                        }
                    }
                    for i in 0..n_r {
                        for j in 0..n_d {
                            acc_rd[i * n_d + j] += resp[i] * drv[j].conj();
                        }
                    }
                }
                let sc = self.scale(line, k);
                let mut p = 0;
                for i in 0..n_c {
                    for j in i..n_c {
                        cc.set(li, i, j, acc_cc[p] * sc);
                        p += 1;
                    }
                }
                let mut p = 0;
                for i in 0..n_d {
                    for j in i..n_d {
                        dd.set(li, i, j, acc_dd[p] * sc);
                        p += 1;
                    }
                }
                for i in 0..n_r {
                    for j in 0..n_d {
                        rd.set(li, i, j, acc_rd[i * n_d + j] * sc);
                    }
                }
            }
            (cc, rd, dd)
        });
        let mut ccs = Vec::new();
        let mut rds = Vec::new();
        let mut dds = Vec::new();
        for (cc, rd, dd) in parts {
            ccs.push(cc);
            rds.push(rd);
            dds.push(dd);
        }
        Estimate {
            g_cc: pool::join_herm(lines, n_c, band, ccs),
            g_rd: pool::join_mat(lines, n_r, n_d, band, rds),
            g_dd: pool::join_herm(lines, n_d, band, dds),
            auto,
            frames: k,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_dsp::frf_ext::cross_welch_with;
    use hertzforge_dsp::spectrum::psd_welch;
    use hertzforge_dsp::spectrum_offline::Detrend;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64) - 0.5
        }
    }

    fn noise(n: usize, seed: u64) -> Vec<f64> {
        let mut r = Lcg(seed);
        (0..n).map(|_| r.next()).collect()
    }

    // ---- The lag search -------------------------------------------------------
    //
    // The station's shape: 12 800 Hz, a 1024-point frame, the band 25 to 2000 Hz
    // (lines 2 to 160). The drive as played starts `PRE` frames before the
    // response record, as the service's drive tail does, and the response record
    // ends with it.

    const FS: f64 = 12_800.0;
    const NFFT: usize = 1024;
    const BAND: (usize, usize) = (2, 160);
    const PRE: usize = 4096;
    const MAX_LAG: usize = 4096;

    /// The lag search before c271b4cb (e7d9b34b), kept here as the reference the
    /// resonant case is measured against: the raw cross-correlation's magnitude at
    /// every lag, summed over the controls, through the FFT.
    fn raw_lag_scores(controls: &[&[f64]], drive: &[f64], max_lag: usize) -> Vec<f64> {
        let mut acc = vec![0.0f64; max_lag + 1];
        let w = controls.first().map(|c| c.len()).unwrap_or(0);
        if w == 0 || drive.len() < w + max_lag {
            return acc;
        }
        let n = fft::next_pow2(w + max_lag);
        let mut y: Vec<fft::Cplx> = vec![(0.0, 0.0); n];
        for (d, &v) in y.iter_mut().zip(&drive[..w + max_lag]) {
            d.0 = v;
        }
        fft::fft_in_place(&mut y);
        for x in controls {
            let mut z: Vec<fft::Cplx> = vec![(0.0, 0.0); n];
            for (d, &v) in z.iter_mut().zip(x.iter().take(w)) {
                d.0 = v;
            }
            fft::fft_in_place(&mut z);
            let prod: Vec<fft::Cplx> = (0..n)
                .map(|m| {
                    let a = z[(n - m) % n];
                    let b = y[m];
                    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
                })
                .collect();
            let r = fft::ifft(&prod);
            for (lag, s) in acc.iter_mut().enumerate() {
                *s += r[max_lag - lag].0.abs();
            }
        }
        acc
    }

    /// Both searches over one record, sliced as `measure_lag` slices it (the raw
    /// one as it did before c271b4cb): `(whitened, raw)`.
    fn both_searches(controls: &[Vec<f64>], stream: &[f64]) -> (usize, usize) {
        let len = controls[0].len();
        let offset = stream.len() - len;
        let max_lag = MAX_LAG.min(offset).min(len / 2);
        let views: Vec<&[f64]> = controls.iter().map(|c| c.as_slice()).collect();
        let whitened = whitened_lag(&views, stream, BAND, NFFT, max_lag);
        let w = (len / 2).min(8192);
        let start = len - w;
        let windows: Vec<&[f64]> = controls.iter().map(|c| &c[start..start + w]).collect();
        let scores = raw_lag_scores(&windows, &stream[offset + start - max_lag..], max_lag);
        let mut raw = (0usize, 0.0f64);
        for (lag, &s) in scores.iter().enumerate() {
            if s > raw.1 {
                raw = (lag, s);
            }
        }
        (whitened, raw.0)
    }

    /// Unit-variance noise limited to `lo_hz..=hi_hz`, `n` frames.
    fn band_noise(n: usize, seed: u64, lo_hz: f64, hi_hz: f64) -> Vec<f64> {
        let m = fft::next_pow2(n);
        let mut z: Vec<fft::Cplx> = noise(m, seed).into_iter().map(|v| (v, 0.0)).collect();
        fft::fft_in_place(&mut z);
        for (k, v) in z.iter_mut().enumerate() {
            let f = k.min(m - k) as f64 * FS / m as f64;
            if f < lo_hz || f > hi_hz {
                *v = (0.0, 0.0);
            }
        }
        let x: Vec<f64> = fft::ifft(&z).into_iter().take(n).map(|v| v.0).collect();
        let rms = (x.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
        x.into_iter().map(|v| v / rms).collect()
    }

    /// The stream `d` frames late, through `gain`, over the last `len` frames.
    fn delayed(stream: &[f64], d: usize, gain: f64, len: usize) -> Vec<f64> {
        let off = stream.len() - len;
        (0..len).map(|t| gain * stream[off + t - d]).collect()
    }

    /// The stream `d` frames late through one lightly damped mode (two poles at
    /// `f_hz`, quality `q`, unit gain at the first frame), over the last `len`.
    fn ringing(stream: &[f64], d: usize, f_hz: f64, q: f64, len: usize) -> Vec<f64> {
        let th = std::f64::consts::TAU * f_hz / FS;
        let r = (-th / (2.0 * q)).exp();
        let (a1, a2) = (2.0 * r * th.cos(), -r * r);
        let mut y = vec![0.0f64; stream.len()];
        for t in 0..stream.len() {
            let x = if t >= d { stream[t - d] } else { 0.0 };
            let y1 = if t >= 1 { y[t - 1] } else { 0.0 };
            let y2 = if t >= 2 { y[t - 2] } else { 0.0 };
            y[t] = x + a1 * y1 + a2 * y2;
        }
        y.split_off(stream.len() - len)
    }

    /// (a) A pure delay through a flat plant: four controls, each with its own gain
    /// and sign, give the delay exactly, from none to near the longest the service
    /// looks back. 553 frames is what the loopback bench reads at 12 800 Hz. The
    /// shorter record takes a 4096-point window.
    #[test]
    fn a_pure_delay_through_a_flat_plant_gives_the_exact_lag() {
        for (len, d) in [
            (20_000usize, 0usize),
            (20_000, 1),
            (20_000, 41),
            (20_000, 553),
            (20_000, 4000),
            (6_000, 553),
        ] {
            let stream = band_noise(PRE + len, 11, 25.0, 2000.0);
            let controls: Vec<Vec<f64>> = [1.0, -0.6, 0.3, 2.0]
                .iter()
                .map(|&g| delayed(&stream, d, g, len))
                .collect();
            let (whitened, _) = both_searches(&controls, &stream);
            assert_eq!(whitened, d, "len {len}");
        }
    }

    /// (b) A lightly damped plant (50 Hz at Q 100 and 140 Hz at Q 60, 273 frames
    /// late): the raw correlation peaks where the ringing lines up with the drive,
    /// hundreds of frames late; the whitened search lands on the onset. A mode's
    /// two poles alone put its phase centre one frame early and bend the phase across
    /// the resonance, so the whitened search is held to within two frames, under
    /// 0.2 % of a 1024-point frame, which the FRF absorbs as phase.
    #[test]
    fn a_ringing_plant_fools_the_raw_correlation_and_not_the_whitened_one() {
        let (len, d) = (20_000usize, 273usize);
        let stream = band_noise(PRE + len, 12, 25.0, 2000.0);
        let controls = vec![
            ringing(&stream, d, 50.0, 100.0, len),
            ringing(&stream, d, 140.0, 60.0, len),
        ];
        let (whitened, raw) = both_searches(&controls, &stream);
        assert!(raw.abs_diff(d) > 100, "raw search {raw}, true {d}");
        assert!(
            whitened.abs_diff(d) <= 2,
            "whitened search {whitened}, true {d}"
        );
    }

    /// (c) A band-limited drive and broadband measurement noise ten times the
    /// response's RMS on each control (-20 dB): every in-band line weighs alike
    /// however much of it is noise, and the lag still comes out exact. (The band
    /// restriction itself is pinned by (a): searched over every line, the
    /// out-of-band lines of a band-limited drive, which carry only rounding,
    /// decide the peak, and (a) fails at a lag of 1.)
    #[test]
    fn broadband_noise_on_the_response_leaves_the_lag_exact() {
        let (len, d) = (20_000usize, 553usize);
        let stream = band_noise(PRE + len, 13, 25.0, 2000.0);
        let controls: Vec<Vec<f64>> = (0..4u64)
            .map(|k| {
                let n = noise(len, 130 + k);
                // `noise` is uniform on (-0.5, 0.5): RMS 1/sqrt(12).
                let g = 10.0 * 12f64.sqrt();
                delayed(&stream, d, 1.0, len)
                    .iter()
                    .zip(&n)
                    .map(|(y, e)| y + g * e)
                    .collect()
            })
            .collect();
        let (whitened, _) = both_searches(&controls, &stream);
        assert_eq!(whitened, d);
    }

    /// (d) Among the four controls one reads exactly zero and one only noise at
    /// 1e-9 (a gauge that is off): each whitened line counts alike, so the silent
    /// one adds nothing and the noisy one a floor, and the lag stays exact. With
    /// every control silent, or the drive silent, there is no cross power and the
    /// lag is 0, as before c271b4cb (whose `max_by` took the last of the equal
    /// scores and returned the longest lag searched, 4096).
    #[test]
    fn a_silent_control_leaves_the_lag_and_no_cross_power_gives_zero() {
        let (len, d) = (20_000usize, 553usize);
        let stream = band_noise(PRE + len, 14, 25.0, 2000.0);
        let controls = vec![
            delayed(&stream, d, 1.0, len),
            vec![0.0; len],
            noise(len, 140).iter().map(|v| 1e-9 * v).collect(),
            delayed(&stream, d, -0.4, len),
        ];
        assert_eq!(both_searches(&controls, &stream).0, d);

        let silent = vec![vec![0.0; len]; 4];
        assert_eq!(both_searches(&silent, &stream).0, 0);
        let quiet = vec![0.0; PRE + len];
        assert_eq!(both_searches(&controls, &quiet).0, 0);
    }

    /// The diagonal equals the workspace's own Welch PSD, and the cross spectrum
    /// equals its cross estimator in the analysis convention.
    #[test]
    fn autospectra_and_cross_spectra_match_the_workspace_estimators() {
        let (nfft, fs) = (256, 1024.0);
        let x = noise(8192, 1);
        let y: Vec<f64> = x
            .iter()
            .zip(noise(8192, 2))
            .map(|(a, b)| 0.7 * a + b)
            .collect();
        let est = SdmEstimator::new(nfft, fs, 0.5, (1, nfft / 2));
        let e = est.estimate(&[&y, &x], 2, &[&x]);
        let (_, py) = psd_welch(&y, fs, nfft, 0.5);
        for k in 1..nfft / 2 {
            let got = e.g_cc.get(k, 0, 0).re;
            assert!(
                (got - py[k]).abs() <= 1e-12 * py[k],
                "line {k}: {got} vs {}",
                py[k]
            );
            assert!((e.auto[k * 2] - py[k]).abs() <= 1e-12 * py[k]);
        }
        // E[Y X^H] against the workspace's <X conj(Y)> in its own sign: with the
        // positive forward exponent that is conj(<X conj(Y)>) of the analysis
        // convention, i.e. the analysis-convention E[Y conj(X)].
        let cw =
            cross_welch_with(&x, &y, fs, &window::hann(nfft), 0.5, Detrend::SegmentMean).unwrap();
        let positive = cola::forward_exponent_is_positive();
        for k in 1..nfft / 2 {
            let (re, im) = cw.gxy[k];
            let want = if positive {
                C64::new(re, im)
            } else {
                C64::new(re, -im)
            };
            let got = e.g_rd.get(k, 0, 0);
            assert!(
                (got - want).norm() <= 1e-10 * want.norm().max(1e-30),
                "line {k}"
            );
        }
    }

    /// A14 for the estimator: `y(t) = x(t - d)` gives `G_yx = e^(-j 2 pi f d) G_xx`,
    /// a known non-zero phase. With the conjugation removed the phase comes out
    /// with the opposite sign and this fails.
    #[test]
    fn a_known_delay_comes_out_with_the_analysis_convention_phase() {
        let (nfft, fs, d) = (512, 2048.0, 3usize);
        let x = noise(32_768 + d, 7);
        let y: Vec<f64> = x[..x.len() - d].to_vec();
        let xs: Vec<f64> = x[d..].to_vec();
        // y[t] = x_orig[t] and xs[t] = x_orig[t + d], so y lags xs by d samples.
        let est = SdmEstimator::new(nfft, fs, 0.5, (1, nfft / 2));
        let e = est.estimate(&[&y], 1, &[&xs]);
        let mut checked = 0;
        for k in 10..nfft / 4 {
            let g = e.g_rd.get(k, 0, 0);
            let want = -std::f64::consts::TAU * k as f64 * d as f64 / nfft as f64;
            let mut err = g.arg() - want;
            while err > std::f64::consts::PI {
                err -= std::f64::consts::TAU;
            }
            while err < -std::f64::consts::PI {
                err += std::f64::consts::TAU;
            }
            assert!(
                err.abs() < 0.05,
                "line {k}: phase {} against {want}",
                g.arg()
            );
            checked += 1;
        }
        assert!(checked > 100);
        // And the drive SDM's cross term between two outputs follows the same rule.
        let e2 = est.estimate(&[&y], 1, &[&xs, &y]);
        let g01 = e2.g_dd.get(40, 0, 1);
        let want = std::f64::consts::TAU * 40.0 * d as f64 / nfft as f64;
        assert!((g01.arg() - want).abs() < 0.05, "{} vs {want}", g01.arg());
    }
}
